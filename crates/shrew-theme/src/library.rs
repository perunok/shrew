//! Where themes come from: the built-ins, custom `*.toml` files in the themes folder (`~/.config/shrew/themes`),
//! color schemes found in Sublime Text's user folders, and paths to a scheme file. [`Library::load`] resolves an id
//! from any of them; [`Library::list`] is what a theme picker shows.

use crate::accent::Accent;
use crate::builtin::{self, BUILTIN};
use crate::import;
use crate::theme::{Scheme, Theme, ThemeFile};
use std::path::{Path, PathBuf};

/// Where a listed theme lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Builtin,
    /// A `.toml` in the themes folder.
    Custom(PathBuf),
    /// A Sublime/TextMate color scheme (converted when used).
    Scheme(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeInfo {
    /// What `theme = "..."` takes: a built-in or custom id, or a scheme file's path.
    pub id: String,
    pub name: String,
    pub scheme: Option<Scheme>,
    pub source: Source,
}

/// A loaded theme and what was wrong with its files (unknown keys, bad colors). Never fatal.
#[derive(Clone, Debug)]
pub struct Loaded {
    pub theme: Theme,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Library {
    /// The custom themes folder (may not exist).
    pub dir: Option<PathBuf>,
    /// Folders searched for `.sublime-color-scheme` / `.tmTheme` files (Sublime's `Packages/User`).
    pub scheme_dirs: Vec<PathBuf>,
}

/// Longest `base` chain followed (cycles stop here too).
const MAX_DEPTH: usize = 8;

impl Library {
    pub fn new(dir: Option<PathBuf>, scheme_dirs: Vec<PathBuf>) -> Library {
        Library { dir, scheme_dirs }
    }

    /// The custom theme file for `id`, if there is one.
    pub fn custom_path(&self, id: &str) -> Option<PathBuf> {
        let ok = !id.is_empty() && !id.contains(['/', '\\']) && !id.starts_with('.');
        let p = self.dir.as_ref()?.join(format!("{id}.toml"));
        (ok && p.is_file()).then_some(p)
    }

    /// Whether `path` is a custom theme file (so saving it should reload the theme).
    pub fn is_custom_file(&self, path: &Path) -> bool {
        self.dir.as_deref().is_some_and(|d| path.parent() == Some(d)) && path.extension().is_some_and(|e| e == "toml")
    }

    /// Resolves `id` (built-in, custom, or a path to a theme or scheme file) with `accent` laid over it.
    pub fn load(&self, id: &str, accent: Option<&'static Accent>) -> Result<Loaded, String> {
        let mut warnings = Vec::new();
        let (theme_id, file) = self.file(id, None, 0, &mut warnings)?;
        Ok(Loaded { theme: Theme::resolve(&theme_id, file, accent), warnings })
    }

    /// The merged file for `id` (its base chain stacked under it). `shadowed`: a custom theme whose base has its own
    /// id means the built-in of that name.
    fn file(
        &self,
        id: &str,
        shadowed: Option<&str>,
        depth: usize,
        warnings: &mut Vec<String>,
    ) -> Result<(String, ThemeFile), String> {
        if depth > MAX_DEPTH {
            return Err(format!("{id}: base themes nest too deep (a cycle?)"));
        }
        let path = Path::new(id);
        if import::is_importable(path) {
            let f = import::read(&expand_home(path))?.to_theme_file();
            let name = f.name.clone().unwrap_or_else(|| id.to_string());
            return Ok((import::slug(&name), f));
        }
        let custom = if id.ends_with(".toml") {
            Some(expand_home(path))
        } else if shadowed == Some(id) {
            None
        } else {
            self.custom_path(id)
        };
        if let Some(p) = custom {
            let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            let (own, w) = ThemeFile::parse(&text).map_err(|e| format!("{}: {e}", p.display()))?;
            warnings.extend(w.into_iter().map(|w| format!("{}: {w}", p.display())));
            let own_id = p.file_stem().map_or_else(|| id.to_string(), |s| s.to_string_lossy().into_owned());
            let mut file = match &own.base {
                Some(base) => self.file(base, Some(&own_id), depth + 1, warnings)?.1,
                None => ThemeFile::default(),
            };
            file.overlay(&own);
            return Ok((own_id, file));
        }
        let b = builtin::find(id).ok_or_else(|| format!("unknown theme {id:?}"))?;
        Ok((b.id.to_string(), builtin::file(b.id).expect("found above")))
    }

    /// Every theme a picker can offer: built-ins, then custom files, then schemes found in the scheme folders.
    /// Reads the custom files' headers, so call it when the picker opens, not at startup.
    pub fn list(&self) -> Vec<ThemeInfo> {
        let mut out: Vec<ThemeInfo> = BUILTIN
            .iter()
            .map(|b| ThemeInfo {
                id: b.id.to_string(),
                name: b.name.to_string(),
                scheme: Some(b.scheme),
                source: Source::Builtin,
            })
            .collect();
        if let Some(dir) = &self.dir {
            let mut custom: Vec<ThemeInfo> = files(dir, |p| p.extension().is_some_and(|e| e == "toml"))
                .into_iter()
                .filter_map(|p| {
                    let id = p.file_stem()?.to_string_lossy().into_owned();
                    let mut warnings = Vec::new();
                    let (_, f) = self.file(&id, None, 0, &mut warnings).ok()?;
                    Some(ThemeInfo {
                        name: f.name.clone().unwrap_or_else(|| id.clone()),
                        scheme: f.scheme,
                        id,
                        source: Source::Custom(p),
                    })
                })
                .collect();
            custom.sort_by_key(|t| t.name.to_lowercase());
            out.extend(custom);
        }
        for d in &self.scheme_dirs {
            for p in files(d, import::is_importable) {
                let name = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                out.push(ThemeInfo { id: p.display().to_string(), name, scheme: None, source: Source::Scheme(p) });
            }
        }
        out
    }

    /// Converts a color scheme into a custom theme file in the themes folder. Returns the new theme's id, the file
    /// and contrast warnings (the import is written either way).
    pub fn import(&self, scheme: &Path) -> Result<(String, PathBuf, Vec<String>), String> {
        let dir = self.dir.as_ref().ok_or("no config directory on this system")?;
        let scheme = expand_home(scheme);
        let f = import::read(&scheme)?.to_theme_file();
        let name = f.name.clone().unwrap_or_else(|| "imported".into());
        let id = import::slug(&name);
        let header = format!(
            "Imported from {} by shrew.\nEdit freely: see docs/THEMES.md. Unset colors are derived from bg, text and accent.",
            scheme.display()
        );
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = dir.join(format!("{id}.toml"));
        std::fs::write(&path, f.to_toml(&header)).map_err(|e| format!("{}: {e}", path.display()))?;
        let theme = Theme::resolve(&id, f, None);
        let warnings = crate::audit::audit(&theme).iter().map(|f| f.to_string()).collect();
        Ok((id, path, warnings))
    }
}

/// Files directly in `dir` that pass `keep`, sorted.
fn files(dir: &Path, keep: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_file() && keep(p)).collect();
    v.sort();
    v
}

fn expand_home(p: &Path) -> PathBuf {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    match (p.strip_prefix("~"), home) {
        (Ok(rest), Some(h)) => PathBuf::from(h).join(rest),
        _ => p.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::Role;
    use crate::{Rgb, accent};

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shrew-theme-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn custom_theme_over_a_base() {
        let dir = tmp("custom");
        std::fs::write(
            dir.join("mine.toml"),
            "name = \"Mine\"\nbase = \"dracula\"\n[ui]\nbg = \"#101010\"\nbogus = \"#000\"\n[syntax]\nkeyword = { fg = \"#ff0000\", bold = true }\n",
        )
        .unwrap();
        // a custom theme named like a built-in shadows it, and may use the built-in as its base
        std::fs::write(dir.join("nord.toml"), "base = \"nord\"\n[syntax]\nstring = \"#00ff00\"\n").unwrap();
        let lib = Library::new(Some(dir.clone()), vec![]);
        let l = lib.load("mine", None).unwrap();
        assert_eq!(l.theme.name, "Mine");
        assert_eq!(l.theme.ui.bg, Rgb::hex(0x101010));
        assert_eq!(l.warnings.len(), 1, "{:?}", l.warnings);
        let dracula = Theme::builtin("dracula").unwrap();
        assert_eq!(l.theme.syntax[Role::String], dracula.syntax[Role::String]);
        assert!(l.theme.syntax[Role::Keyword].font.unwrap().bold);
        // derived tokens follow the new background
        assert_ne!(l.theme.ui.current_line, dracula.ui.current_line);
        let n = lib.load("nord", accent::accent("rose")).unwrap().theme;
        assert_eq!(n.syntax[Role::String].fg, Rgb::hex(0x00ff00));
        assert_eq!(n.accent, Some("rose"));
        let ids: Vec<String> = lib.list().into_iter().map(|t| t.id).collect();
        assert!(ids.contains(&"mine".to_string()) && ids.contains(&"dracula".to_string()));
        assert!(lib.load("nope", None).is_err());
        std::fs::write(dir.join("loop.toml"), "base = \"loop2\"\n").unwrap();
        std::fs::write(dir.join("loop2.toml"), "base = \"loop\"\n").unwrap();
        assert!(lib.load("loop", None).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_writes_a_custom_theme() {
        let dir = tmp("import");
        let schemes = tmp("schemes");
        let src = schemes.join("Tiny.sublime-color-scheme");
        std::fs::write(
            &src,
            r##"{ "name": "Tiny Scheme", "globals": { "background": "#fdf6e3", "foreground": "#333333" },
                 "rules": [ { "scope": "string", "foreground": "#2aa198" } ] }"##,
        )
        .unwrap();
        let lib = Library::new(Some(dir.clone()), vec![schemes.clone()]);
        assert!(lib.list().iter().any(|t| t.source == Source::Scheme(src.clone())));
        // usable directly by path, too
        assert_eq!(lib.load(&src.display().to_string(), None).unwrap().theme.ui.bg, Rgb::hex(0xfdf6e3));
        let (id, path, warnings) = lib.import(&src).unwrap();
        assert_eq!(id, "tiny-scheme");
        assert!(path.is_file());
        // #2aa198 on #fdf6e3 is below 4.5:1
        assert!(warnings.iter().any(|w| w.contains("string")), "{warnings:?}");
        let t = lib.load("tiny-scheme", None).unwrap();
        assert!(t.warnings.is_empty(), "{:?}", t.warnings);
        assert_eq!(t.theme.scheme, Scheme::Light);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&schemes);
    }
}

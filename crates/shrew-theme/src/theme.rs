//! A theme file ([`ThemeFile`]: what was written) and a resolved [`Theme`] (every token known). Resolving stacks the
//! base theme's file under the theme's own, lays an accent over it if one is chosen, then derives every token the
//! files leave out from the ones they set.

use crate::accent::Accent;
use crate::color::Rgb;
use crate::tokens::{Font, Role, Syntax, SyntaxSpec, SyntaxStyle, Ui, UiSpec};
use toml_edit::{Document, Item};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scheme {
    Light,
    Dark,
}

impl Scheme {
    pub fn parse(s: &str) -> Option<Scheme> {
        match s {
            "light" => Some(Scheme::Light),
            "dark" => Some(Scheme::Dark),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Scheme::Light => "light",
            Scheme::Dark => "dark",
        }
    }
}

/// A theme file as written: everything optional.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThemeFile {
    pub name: Option<String>,
    pub scheme: Option<Scheme>,
    /// The theme this one starts from (built-in or custom id).
    pub base: Option<String>,
    /// Held to WCAG AAA (7:1) by the contrast audit.
    pub high_contrast: Option<bool>,
    pub ui: UiSpec,
    pub syntax: SyntaxSpec,
}

impl ThemeFile {
    /// Parses theme TOML. Unknown keys and bad colors become warnings (`line: message`); only text that is not TOML
    /// at all is an error.
    pub fn parse(text: &str) -> Result<(ThemeFile, Vec<String>), String> {
        let doc = Document::parse(text).map_err(|e| e.message().trim().to_string())?;
        let line = |item: &Item| item.span().map_or(0, |s| text[..s.start.min(text.len())].matches('\n').count() + 1);
        let mut warnings = Vec::new();
        let mut warn = |item: &Item, msg: String| warnings.push(format!("line {}: {msg}", line(item)));
        let mut f = ThemeFile::default();
        for (key, item) in doc.as_table().iter() {
            match key {
                "name" | "base" => match item.as_str() {
                    Some(s) if key == "name" => f.name = Some(s.to_string()),
                    Some(s) => f.base = Some(s.to_string()),
                    None => warn(item, format!("{key}: expected a string")),
                },
                "scheme" => match item.as_str().and_then(Scheme::parse) {
                    Some(s) => f.scheme = Some(s),
                    None => warn(item, "scheme: expected \"light\" or \"dark\"".into()),
                },
                "high_contrast" | "high-contrast" => match item.as_bool() {
                    Some(b) => f.high_contrast = Some(b),
                    None => warn(item, format!("{key}: expected true or false")),
                },
                "ui" => {
                    let Some(t) = item.as_table_like() else {
                        warn(item, "[ui] should be a table".into());
                        continue;
                    };
                    for (k, v) in t.iter() {
                        let Some(slot) = f.ui.slot(k) else {
                            warn(v, format!("unknown ui color {k:?}"));
                            continue;
                        };
                        match v.as_str().and_then(Rgb::parse) {
                            Some(c) => *slot = Some(c),
                            None => warn(v, format!("ui.{k}: expected a color like \"#1f2328\"")),
                        }
                    }
                }
                "syntax" => {
                    let Some(t) = item.as_table_like() else {
                        warn(item, "[syntax] should be a table".into());
                        continue;
                    };
                    for (k, v) in t.iter() {
                        let Some(role) = Role::from_key(k) else {
                            warn(v, format!("unknown syntax role {k:?}"));
                            continue;
                        };
                        match syntax_style(v) {
                            Ok(s) => f.syntax.set(role, s),
                            Err(e) => warn(v, format!("syntax.{k}: {e}")),
                        }
                    }
                }
                _ => warn(item, format!("unknown key {key:?}")),
            }
        }
        Ok((f, warnings))
    }

    /// Puts `over` on top of `self` (`name`, `scheme` and `base` included when `over` sets them).
    pub fn overlay(&mut self, over: &ThemeFile) {
        if over.name.is_some() {
            self.name = over.name.clone();
        }
        if over.scheme.is_some() {
            self.scheme = over.scheme;
        }
        if over.high_contrast.is_some() {
            self.high_contrast = over.high_contrast;
        }
        self.base = over.base.clone();
        self.ui.overlay(&over.ui);
        self.syntax.overlay(&over.syntax);
    }

    /// The file as theme TOML (for imports and `--dump`).
    pub fn to_toml(&self, header: &str) -> String {
        let mut s = String::new();
        for l in header.lines() {
            s += &format!("# {l}\n").replace("# \n", "#\n");
        }
        if let Some(n) = &self.name {
            s += &format!("name = {}\n", quote(n));
        }
        if let Some(sc) = self.scheme {
            s += &format!("scheme = \"{}\"\n", sc.name());
        }
        if let Some(b) = &self.base {
            s += &format!("base = {}\n", quote(b));
        }
        if self.high_contrast == Some(true) {
            s += "high_contrast = true\n";
        }
        s += "\n[ui]\n";
        for (k, v) in self.ui.entries() {
            if let Some(c) = v {
                s += &format!("{k} = \"{c}\"\n");
            }
        }
        s += "\n[syntax]\n";
        for &r in Role::ALL {
            let Some(st) = self.syntax.get(r) else { continue };
            match st.font {
                None => s += &format!("{} = \"{}\"\n", r.key(), st.fg),
                Some(f) => {
                    s += &format!("{} = {{ fg = \"{}\"", r.key(), st.fg);
                    for (on, name) in [(f.bold, "bold"), (f.italic, "italic"), (f.underline, "underline")] {
                        s += &format!(", {name} = {on}");
                    }
                    s += " }\n";
                }
            }
        }
        s
    }
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `"#rrggbb"` or `{ fg = "#rrggbb", bold = true, italic = false, underline = false }`.
fn syntax_style(v: &Item) -> Result<SyntaxStyle, String> {
    if let Some(s) = v.as_str() {
        return Rgb::parse(s).map(SyntaxStyle::new).ok_or_else(|| format!("{s:?} is not a color like \"#1f2328\""));
    }
    let t = v.as_table_like().ok_or("expected a color or { fg = \"#...\", bold = true }")?;
    let fg = t.get("fg").and_then(Item::as_str).and_then(Rgb::parse).ok_or("missing or bad fg color")?;
    let mut font = None;
    for (k, item) in t.iter() {
        let flag = match k {
            "fg" => continue,
            "bold" | "italic" | "underline" => item.as_bool().ok_or(format!("{k}: expected true or false"))?,
            _ => return Err(format!("unknown key {k:?}")),
        };
        let f = font.get_or_insert(Font::PLAIN);
        match k {
            "bold" => f.bold = flag,
            "italic" => f.italic = flag,
            _ => f.underline = flag,
        }
    }
    Ok(SyntaxStyle { fg, font })
}

/// A theme with every token resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    /// Built-in or custom theme id (`dracula`, `my-theme`).
    pub id: String,
    pub name: String,
    pub scheme: Scheme,
    pub high_contrast: bool,
    /// The accent laid over the theme's own, if any.
    pub accent: Option<&'static str>,
    pub ui: Ui,
    pub syntax: Syntax,
    /// The merged file it was resolved from (base included), to re-derive with another accent.
    pub file: ThemeFile,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::builtin(crate::DEFAULT_DARK).expect("the default theme is built in")
    }
}

impl Theme {
    pub fn dark(&self) -> bool {
        self.scheme == Scheme::Dark
    }

    /// Resolves a merged theme file (its base already stacked under it).
    pub fn resolve(id: &str, file: ThemeFile, accent: Option<&'static Accent>) -> Theme {
        let scheme = file.scheme.unwrap_or_else(|| {
            let dark = file.ui.bg.is_none_or(|bg| bg.luminance() < 0.2);
            if dark { Scheme::Dark } else { Scheme::Light }
        });
        let high_contrast = file.high_contrast.unwrap_or(false);
        let mut ui = file.ui.clone();
        if let Some(a) = accent {
            lay_accent(&mut ui, a, scheme, high_contrast);
        }
        let (ui, syntax) = derive(&ui, &file.syntax, scheme, high_contrast);
        Theme {
            id: id.to_string(),
            name: file.name.clone().unwrap_or_else(|| id.to_string()),
            scheme,
            high_contrast,
            accent: accent.map(|a| a.id),
            ui,
            syntax,
            file,
        }
    }

    /// The same theme with another accent (`None`: its own).
    pub fn with_accent(&self, accent: Option<&'static Accent>) -> Theme {
        if accent.map(|a| a.id) == self.accent {
            return self.clone();
        }
        Theme::resolve(&self.id, self.file.clone(), accent)
    }
}

/// The scheme's defaults for the few tokens everything else derives from, when no file sets them.
fn fallback(scheme: Scheme) -> (Rgb, Rgb, Rgb) {
    match scheme {
        Scheme::Dark => (Rgb::hex(0x1f1b18), Rgb::hex(0xd8cec2), Rgb::hex(0xeaa35e)),
        Scheme::Light => (Rgb::hex(0xfbf8f3), Rgb::hex(0x3a322b), Rgb::hex(0xb35a1f)),
    }
}

/// Replaces the accent tokens with `a`'s (Slinger's rules: the accent fill, its ink, a soft tint and a selection
/// mixed from the background), and drops the tokens that follow the accent so they are derived from it again.
fn lay_accent(ui: &mut UiSpec, a: &Accent, scheme: Scheme, high_contrast: bool) {
    let dark = scheme == Scheme::Dark;
    let (fb_bg, fb_text, _) = fallback(scheme);
    let bg = ui.bg.unwrap_or(fb_bg);
    let text = ui.text.unwrap_or(fb_text);
    let need = need(high_contrast);
    let v = if dark { a.dark } else { a.light };
    // accent-colored text also sits on popups, which may be darker than the background
    let darkest = [Some(bg), ui.surface_raised, ui.popup].into_iter().flatten();
    let on_all = |c: Rgb| darkest.clone().fold(c, |c, b| c.readable_on(b, need));
    if high_contrast {
        // pushed towards the text color so it keeps AAA contrast; the ink is the background
        let fill = text.mix(v.fill, if dark { 0.70 } else { 0.58 });
        ui.accent = Some(fill);
        ui.accent_fg = Some(bg);
        let soft = tint(bg, v.fill, if dark { 0.22 } else { 0.10 }, &[text, fill], need);
        ui.accent_soft = Some(soft);
        ui.accent_text = Some(on_all(fill).readable_on(soft, need));
        ui.selection = Some(tint(bg, v.fill, if dark { 0.30 } else { 0.22 }, &[text], need));
    } else {
        ui.accent = Some(v.fill);
        ui.accent_fg = Some(v.fg);
        let soft = tint(bg, v.fill, if dark { 0.18 } else { 0.14 }, &[text, v.text], need);
        ui.accent_soft = Some(soft);
        ui.accent_text = Some(on_all(v.text).readable_on(soft, need));
        ui.selection = Some(tint(bg, v.fill, if dark { 0.24 } else { 0.16 }, &[text], need));
    }
    for t in [
        &mut ui.selection_inactive,
        &mut ui.cursor,
        &mut ui.cursor_fg,
        &mut ui.bracket_match,
        &mut ui.mode_normal,
        &mut ui.mode_normal_fg,
    ] {
        *t = None;
    }
}

/// Text contrast target: AA, or AAA for high-contrast themes (with a hair of margin for rounding).
fn need(high_contrast: bool) -> f32 {
    if high_contrast { 7.05 } else { 4.55 }
}

/// `c` mixed into `bg` by `t`, or less when that is needed for every color in `inks` to stay readable on it.
fn tint(bg: Rgb, c: Rgb, t: f32, inks: &[Rgb], need: f32) -> Rgb {
    let mut t = t;
    loop {
        let m = bg.mix(c, t);
        if t <= 0.02 || inks.iter().all(|i| i.contrast(m) >= need) {
            return m;
        }
        t -= 0.01;
    }
}

/// Fills in every token the spec leaves out.
fn derive(u: &UiSpec, s: &SyntaxSpec, scheme: Scheme, high_contrast: bool) -> (Ui, Syntax) {
    let dark = scheme == Scheme::Dark;
    let need = need(high_contrast);
    let d = |light: f32, dark_v: f32| if dark { dark_v } else { light };
    let (fb_bg, fb_text, fb_accent) = fallback(scheme);
    let bg = u.bg.unwrap_or(fb_bg);
    let text = u.text.unwrap_or(fb_text);
    let ink_dark = if dark { bg } else { Rgb::hex(0x10121a) };
    let ink = |c: Rgb| {
        let best = c.best_ink(ink_dark, Rgb::WHITE);
        if c.contrast(best) >= 4.5 { best } else { c.best_ink(Rgb::BLACK, Rgb::WHITE) }
    };
    let mixed = |t: f32| bg.mix(text, t);

    let surface = u.surface.unwrap_or_else(|| if dark { bg.mix(Rgb::BLACK, 0.25) } else { mixed(0.04) });
    let surface_raised = u.surface_raised.unwrap_or_else(|| mixed(0.05));
    let surface_hover = u.surface_hover.unwrap_or_else(|| mixed(0.10));
    let border = u.border.unwrap_or_else(|| mixed(0.15));
    let border_strong = u.border_strong.unwrap_or_else(|| mixed(0.30));
    let text_muted = u.text_muted.unwrap_or_else(|| text.mix(bg, 0.3).readable_on(bg, need));
    let text_faint = u.text_faint.unwrap_or_else(|| text.mix(bg, 0.5).readable_on(bg, 3.1));
    let accent = u.accent.unwrap_or(fb_accent);
    let accent_fg = u.accent_fg.unwrap_or_else(|| ink(accent));
    let accent_text = u.accent_text.unwrap_or_else(|| accent.readable_on(bg, need));
    let accent_soft = u.accent_soft.unwrap_or_else(|| tint(bg, accent, d(0.14, 0.18), &[text, accent_text], need));
    let selection = u.selection.unwrap_or_else(|| tint(bg, accent, d(0.16, 0.24), &[text], need));
    let danger = u.danger.unwrap_or_else(|| Rgb::hex(if dark { 0xff6b6b } else { 0xc62828 }));
    let success = u.success.unwrap_or_else(|| Rgb::hex(if dark { 0x4cc38a } else { 0x117d3a }));
    let warning = u.warning.unwrap_or_else(|| Rgb::hex(if dark { 0xf0b45a } else { 0x9e5a05 }));

    // syntax first: a few UI tokens (info, the visual-mode badge) take syntax hues
    let mut syn: [Option<SyntaxStyle>; Role::COUNT] = s.0;
    let mut fill = |r: Role, from: &dyn Fn(&[Option<SyntaxStyle>; Role::COUNT]) -> Rgb| {
        if syn[r as usize].is_none() {
            syn[r as usize] = Some(SyntaxStyle::new(from(&syn)));
        }
    };
    let get = |syn: &[Option<SyntaxStyle>; Role::COUNT], r: Role| syn[r as usize].expect("derived in order").fg;
    fill(Role::Keyword, &|_| accent_text);
    fill(Role::Control, &|s| get(s, Role::Keyword));
    fill(Role::String, &|_| success.readable_on(bg, need));
    fill(Role::Number, &|_| warning.readable_on(bg, need));
    fill(Role::Constant, &|s| get(s, Role::Number));
    fill(Role::StringEscape, &|s| get(s, Role::Constant));
    fill(Role::Regex, &|s| get(s, Role::StringEscape));
    fill(Role::Comment, &|_| text_faint);
    fill(Role::DocComment, &|s| get(s, Role::Comment));
    fill(Role::Function, &|_| text);
    fill(Role::Method, &|s| get(s, Role::Function));
    fill(Role::Type, &|s| get(s, Role::Function));
    fill(Role::BuiltinType, &|s| get(s, Role::Type));
    fill(Role::Namespace, &|s| get(s, Role::Type));
    fill(Role::Macro, &|s| get(s, Role::Function));
    fill(Role::Property, &|_| text);
    fill(Role::Attribute, &|s| get(s, Role::Property));
    fill(Role::Variable, &|_| text);
    fill(Role::Parameter, &|s| get(s, Role::Variable));
    fill(Role::Tag, &|s| get(s, Role::Keyword));
    fill(Role::Punctuation, &|_| text_muted);
    fill(Role::Operator, &|s| get(s, Role::Punctuation));
    fill(Role::Heading, &|s| get(s, Role::Function));
    fill(Role::Link, &|_| accent_text);
    fill(Role::Emphasis, &|_| text);
    fill(Role::DiffAdded, &|_| success);
    fill(Role::DiffRemoved, &|_| danger);
    fill(Role::DiffChanged, &|_| warning);
    fill(Role::Invalid, &|_| danger);
    let syntax = Syntax(syn.map(|s| s.expect("every role derived")));

    let cursor = u.cursor.unwrap_or(accent);
    let mode_normal = u.mode_normal.unwrap_or(accent);
    // badges must not look alike: take the first candidate whose hue is clearly different
    let pick = |explicit: Option<Rgb>, candidates: &[Rgb], avoid: &[Rgb]| {
        let far = |c: &Rgb, from: &[Rgb]| from.iter().all(|a| c.hue_distance(*a) >= 30.0);
        // a red badge reads like an error: only when nothing else is left
        let strict = |c: &Rgb| far(c, avoid) && far(c, &[danger]);
        let loose = |c: &Rgb| far(c, avoid);
        explicit
            .filter(loose)
            .or_else(|| candidates.iter().copied().find(strict))
            .or_else(|| candidates.iter().copied().find(loose))
            .unwrap_or(explicit.unwrap_or(candidates[0]))
    };
    let sy = |r: Role| syntax[r].fg;
    let mode_insert = pick(
        u.mode_insert,
        &[success, sy(Role::String), sy(Role::Type), sy(Role::Function), sy(Role::Keyword)],
        &[mode_normal],
    );
    let mode_visual = pick(
        u.mode_visual,
        &[
            sy(Role::Keyword),
            sy(Role::Function),
            sy(Role::Constant),
            sy(Role::Type),
            sy(Role::Tag),
            sy(Role::Number),
            danger,
        ],
        &[mode_normal, mode_insert],
    );
    let ui = Ui {
        bg,
        surface,
        surface_raised,
        surface_hover,
        border,
        border_strong,
        text,
        text_muted,
        text_faint,
        accent,
        accent_fg,
        accent_soft,
        accent_text,
        selection,
        selection_inactive: u.selection_inactive.unwrap_or_else(|| bg.mix(selection, 0.6)),
        cursor,
        cursor_fg: u.cursor_fg.unwrap_or_else(|| ink(cursor)),
        current_line: u.current_line.unwrap_or_else(|| mixed(d(0.035, 0.04))),
        gutter: u.gutter.unwrap_or(bg),
        line_number: u.line_number.unwrap_or(text_faint),
        line_number_active: u.line_number_active.unwrap_or_else(|| text_muted.mix(text, 0.5)),
        indent_guide: u.indent_guide.unwrap_or_else(|| mixed(d(0.10, 0.08))),
        indent_guide_active: u.indent_guide_active.unwrap_or_else(|| mixed(0.22)),
        whitespace: u.whitespace.unwrap_or_else(|| mixed(0.22)),
        bracket_match: u.bracket_match.unwrap_or_else(|| tint(bg, accent, d(0.2, 0.25), &[text], need)),
        search_match: u.search_match.unwrap_or_else(|| tint(bg, warning, d(0.25, 0.22), &[text], need)),
        search_current: u.search_current.unwrap_or_else(|| tint(bg, warning, d(0.42, 0.36), &[text], need)),
        word_highlight: u.word_highlight.unwrap_or_else(|| tint(bg, text, d(0.08, 0.09), &[text], need)),
        danger,
        danger_fg: u.danger_fg.unwrap_or_else(|| ink(danger)),
        danger_soft: u.danger_soft.unwrap_or_else(|| tint(bg, danger, d(0.12, 0.18), &[text, danger], need)),
        success,
        success_fg: u.success_fg.unwrap_or_else(|| ink(success)),
        success_soft: u.success_soft.unwrap_or_else(|| tint(bg, success, d(0.12, 0.18), &[text, success], need)),
        warning,
        warning_soft: u.warning_soft.unwrap_or_else(|| tint(bg, warning, d(0.12, 0.18), &[text, warning], need)),
        info: u.info.unwrap_or_else(|| sy(Role::Type).readable_on(surface, need)),
        mode_normal,
        mode_normal_fg: u.mode_normal_fg.unwrap_or_else(|| ink(mode_normal)),
        mode_insert,
        mode_insert_fg: u.mode_insert_fg.unwrap_or_else(|| ink(mode_insert)),
        mode_visual,
        mode_visual_fg: u.mode_visual_fg.unwrap_or_else(|| ink(mode_visual)),
        scrollbar: u.scrollbar.unwrap_or_else(|| mixed(0.28)),
        popup: u.popup.unwrap_or(surface_raised),
        popup_border: u.popup_border.unwrap_or(border),
        shadow: u.shadow.unwrap_or(Rgb::BLACK),
    };
    (ui, syntax)
}

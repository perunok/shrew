//! Command line: `srw [options] [file[:LINE[:COL]] | +LINE | DIR | -]...`, Sublime's `subl` semantics (see
//! docs/CLI.md). `shrew` is the same program under another name: messages use whichever name it was run as.

use crate::ipc::FileArg;
use std::path::{Path, PathBuf};

pub fn usage(name: &str) -> String {
    let title = if name == "shrew" { "shrew".to_string() } else { format!("{name} — shrew") };
    format!(
        "\
{title}, a tiny, fast, keyboard-first text editor

usage: {name} [options] [file[:LINE[:COL]] | +LINE | DIR | -]...

Files open in the running shrew, whose window comes to the front. Without one, shrew starts in the background
and the prompt comes back at once.

  file:LINE[:COL]       open file with the cursor on LINE (and char column COL)
  +LINE                 put the cursor on LINE of the first file
  DIR                   make DIR the project folder, shown in the sidebar
  -                     read stdin into a new buffer
  -w, --wait            return only once the files are closed in the editor: EDITOR=\"{name} -w\"
  -n, --new-window      start a new shrew instead of using the running one
  -f, --foreground      run in this process, attached to the terminal (implies -n; logs go to stderr)
  --                    the rest are files, even if they start with - or +
  -V, --version         print the version
  -h, --help            this help
  --features            list features: compiled in, on or off, what they do
  --print-config        print a commented default config.toml
  --mcp-stdio           run the MCP stdio bridge for AI assistants (no window)
  --stats               log frame timings (input→present, build, render) to stderr (with -f)
  --screenshot OUT.png  render one frame headless and save it (no window), with:
      --size WxH        image size in pixels (default 1200x800)
      --scale F         scale factor (default 1)
      --keys KEYS       keys to type first, vim notation: \"ggVj\", \"<C-p>main\", \":set novim<CR>\"
                        with mouse tokens in logical pixels: <click:X,Y> <dclick:..> <tclick:..> <rclick:..>
                        <mclick:..> <move:X,Y> <down:..> <up:..> (rdown/rup, mdown/mup) <drag:X1,Y1,X2,Y2>
                        <rdrag:..> <wheel:X,Y,DY[,DX]>; modifiers as in <C-click:X,Y> <S-click:..> <A-..>
      --hover X,Y       move the pointer here last, so hover states show
"
    )
}

/// The name this program was run as (`srw` or `shrew`), for help and error messages.
pub fn program_name() -> String {
    std::env::args_os()
        .next()
        .as_deref()
        .map(Path::new)
        .and_then(Path::file_stem)
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "srw".into())
}

/// The hidden flag a detached instance is started with: its request arrives on stdin, replies go to stdout.
pub const INSTANCE_FLAG: &str = "--instance";

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Edit,
    Version,
    Help,
    Features,
    PrintConfig,
    McpStdio,
    Screenshot(PathBuf),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cli {
    pub action: Action,
    pub files: Vec<FileArg>,
    /// `-`: stdin goes into a new scratch buffer.
    pub stdin: bool,
    pub wait: bool,
    pub new_window: bool,
    pub foreground: bool,
    /// Started by another `srw` as the detached editor process ([`INSTANCE_FLAG`]).
    pub instance: bool,
    pub size: (u32, u32),
    pub scale: f32,
    pub keys: String,
    /// `--hover X,Y`: the pointer's last position for `--screenshot` (logical pixels).
    pub hover: Option<(f32, f32)>,
    pub stats: bool,
}

impl Default for Cli {
    fn default() -> Self {
        Cli {
            action: Action::Edit,
            files: Vec::new(),
            stdin: false,
            wait: false,
            new_window: false,
            foreground: false,
            instance: false,
            size: (1200, 800),
            scale: 1.0,
            keys: String::new(),
            hover: None,
            stats: false,
        }
    }
}

impl Cli {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Cli, String> {
        let mut cli = Cli::default();
        let mut args = args.into_iter();
        let mut only_files = false;
        let mut plus_line: Option<usize> = None;
        while let Some(a) = args.next() {
            if only_files {
                cli.files.push(file_arg(&a));
                continue;
            }
            let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
            match a.as_str() {
                "--" => only_files = true,
                "-" => cli.stdin = true,
                "-V" | "--version" => cli.action = Action::Version,
                "-h" | "--help" => cli.action = Action::Help,
                "-w" | "--wait" => cli.wait = true,
                "-n" | "--new-window" => cli.new_window = true,
                "-f" | "--foreground" => cli.foreground = true,
                INSTANCE_FLAG => cli.instance = true,
                "--features" => cli.action = Action::Features,
                "--print-config" => cli.action = Action::PrintConfig,
                "--mcp-stdio" => cli.action = Action::McpStdio,
                "--stats" => cli.stats = true,
                "--screenshot" => cli.action = Action::Screenshot(value("--screenshot")?.into()),
                "--keys" => cli.keys = value("--keys")?,
                "--hover" => {
                    let v = value("--hover")?;
                    let p = v.split_once(',').and_then(|(x, y)| Some((x.trim().parse().ok()?, y.trim().parse().ok()?)));
                    cli.hover = Some(p.ok_or("--hover takes X,Y, e.g. 300,12")?);
                }
                "--size" => {
                    let v = value("--size")?;
                    let (w, h) = v.split_once(['x', 'X']).ok_or("--size takes WxH, e.g. 1200x800")?;
                    let n = |s: &str| s.trim().parse::<u32>().ok().filter(|&n| (16..=16384).contains(&n));
                    cli.size = n(w).zip(n(h)).ok_or("--size takes WxH between 16 and 16384")?;
                }
                "--scale" => {
                    cli.scale = value("--scale")?
                        .parse::<f32>()
                        .ok()
                        .filter(|s| (0.25..=8.0).contains(s))
                        .ok_or("--scale takes a number like 1 or 2")?;
                }
                s if s.starts_with('+') && s.len() > 1 => {
                    plus_line = Some(s[1..].parse::<usize>().map_err(|_| format!("bad line number {s}"))?);
                }
                s if s.starts_with("--") => return Err(format!("unknown option {s} (see --help)")),
                // short flags together: -nw
                s if s.starts_with('-') => {
                    for c in s[1..].chars() {
                        match c {
                            'w' => cli.wait = true,
                            'n' => cli.new_window = true,
                            'f' => cli.foreground = true,
                            'h' => cli.action = Action::Help,
                            'V' => cli.action = Action::Version,
                            _ => return Err(format!("unknown option -{c} in {s} (see --help)")),
                        }
                    }
                }
                _ => cli.files.push(file_arg(&a)),
            }
        }
        // vim: +LINE is for the first file, whatever its place
        if let Some(n) = plus_line
            && let Some(f) = cli.files.first_mut()
            && f.line.is_none()
        {
            f.line = Some(n);
        }
        if cli.foreground {
            cli.new_window = true;
        }
        Ok(cli)
    }
}

/// `path`, `path:LINE` or `path:LINE:COL`. A file whose name really ends in `:digits` is taken as it is.
fn file_arg(s: &str) -> FileArg {
    let (path, line, col) = split_location(s);
    if (line.is_some() && Path::new(s).exists()) || path.is_empty() {
        return FileArg { path: s.into(), line: None, col: None };
    }
    FileArg { path: path.into(), line, col }
}

fn split_location(s: &str) -> (&str, Option<usize>, Option<usize>) {
    let num =
        |s: &str| (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse::<usize>().ok()).flatten();
    let Some((rest, last)) = s.rsplit_once(':') else { return (s, None, None) };
    let Some(last) = num(last) else { return (s, None, None) };
    match rest.rsplit_once(':') {
        Some((path, line)) if num(line).is_some() => (path, num(line), Some(last)),
        _ => (rest, Some(last), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Cli, String> {
        Cli::parse(s.split_whitespace().map(String::from))
    }

    fn f(path: &str, line: Option<usize>, col: Option<usize>) -> FileArg {
        FileArg { path: path.into(), line, col }
    }

    #[test]
    fn files_and_lines() {
        let c = parse("a.rs +42 b.txt").unwrap();
        assert_eq!(c.action, Action::Edit);
        assert_eq!(c.files, vec![f("a.rs", Some(42), None), f("b.txt", None, None)]);
        assert_eq!(parse("+7 a.rs").unwrap().files, vec![f("a.rs", Some(7), None)]);
        assert_eq!(parse("-- --version").unwrap().files, vec![f("--version", None, None)]);
        assert_eq!(parse("-- -").unwrap().files, vec![f("-", None, None)]);
        assert!(parse("+3").unwrap().files.is_empty());
    }

    #[test]
    fn line_and_column_suffixes() {
        assert_eq!(parse("a.rs:42").unwrap().files, vec![f("a.rs", Some(42), None)]);
        assert_eq!(parse("a.rs:42:7").unwrap().files, vec![f("a.rs", Some(42), Some(7))]);
        // +LINE does not override a file's own :LINE
        assert_eq!(parse("+3 a.rs:9 b").unwrap().files, vec![f("a.rs", Some(9), None), f("b", None, None)]);
        assert_eq!(parse("a.rs:").unwrap().files, vec![f("a.rs:", None, None)]);
        assert_eq!(parse("a.rs:x").unwrap().files, vec![f("a.rs:x", None, None)]);
        assert_eq!(parse(":5").unwrap().files, vec![f(":5", None, None)]);
        assert_eq!(parse("a:b:3").unwrap().files, vec![f("a:b", Some(3), None)]);
        assert_eq!(parse(r"C:\src\main.rs:12:4").unwrap().files, vec![f(r"C:\src\main.rs", Some(12), Some(4))]);
        assert_eq!(parse("-- a.rs:3").unwrap().files, vec![f("a.rs", Some(3), None)]);
    }

    #[test]
    fn existing_files_with_colons_stay_whole() {
        let dir = std::env::temp_dir().join(format!("shrew-cli-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let odd = dir.join("notes:12");
        std::fs::write(&odd, "").unwrap();
        let s = odd.to_string_lossy().into_owned();
        assert_eq!(Cli::parse([s.clone()]).unwrap().files, vec![f(&s, None, None)]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn instance_flags() {
        let c = parse("-w -n -").unwrap();
        assert!(c.wait && c.new_window && c.stdin && !c.foreground);
        let c = parse("--wait --new-window x").unwrap();
        assert!(c.wait && c.new_window);
        let c = parse("-nw").unwrap();
        assert!(c.wait && c.new_window);
        let c = parse("-f").unwrap();
        assert!(c.foreground && c.new_window, "-f implies a new window");
        assert!(parse("--foreground").unwrap().foreground);
        assert!(parse(INSTANCE_FLAG).unwrap().instance);
        assert!(!parse("a").unwrap().instance);
    }

    #[test]
    fn flags() {
        assert_eq!(parse("--version").unwrap().action, Action::Version);
        assert_eq!(parse("-V").unwrap().action, Action::Version);
        assert_eq!(parse("-h").unwrap().action, Action::Help);
        assert_eq!(parse("--features").unwrap().action, Action::Features);
        assert_eq!(parse("--print-config").unwrap().action, Action::PrintConfig);
        assert_eq!(parse("--mcp-stdio").unwrap().action, Action::McpStdio);
        assert!(parse("--stats").unwrap().stats);
        let c = parse("--screenshot out.png --size 640x480 --scale 2 --keys ggVj --hover 10,20.5 main.rs").unwrap();
        assert_eq!(c.action, Action::Screenshot("out.png".into()));
        assert_eq!((c.size, c.scale, c.keys.as_str()), ((640, 480), 2.0, "ggVj"));
        assert_eq!(c.hover, Some((10.0, 20.5)));
        assert!(parse("--hover 10").is_err());
        assert_eq!(c.files, vec![f("main.rs", None, None)]);
    }

    #[test]
    fn errors() {
        assert!(parse("--size 12").is_err());
        assert!(parse("--bogus").is_err());
        assert!(parse("-x").is_err());
        assert!(parse("-wx").is_err());
        assert!(parse("+x").is_err());
        assert!(parse("--screenshot").is_err());
    }

    #[test]
    fn usage_uses_the_name() {
        assert!(usage("srw").contains("usage: srw ["));
        assert!(usage("shrew").contains("EDITOR=\"shrew -w\""));
    }
}

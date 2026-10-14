//! Command line: `shrew [files...] [+LINE]` plus a few flags.

use std::path::PathBuf;

pub const USAGE: &str = "\
shrew — a tiny, fast, keyboard-first text editor

usage: shrew [options] [files...] [+LINE]

  +LINE                 put the cursor on LINE of the first file
  --version, -V         print the version
  --help, -h            this help
  --features            list features: compiled in, on or off, what they do
  --print-config        print a commented default config.toml
  --mcp-stdio           run the MCP stdio bridge for AI assistants (no window)
  --stats               log frame timings (input→present, build, render) to stderr
  --screenshot OUT.png  render one frame headless and save it (no window), with:
      --size WxH        image size in pixels (default 1200x800)
      --scale F         scale factor (default 1)
      --keys KEYS       keys to type first, vim notation: \"ggVj\", \"<C-p>main\", \":set novim<CR>\"
";

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
    pub files: Vec<PathBuf>,
    /// 1-based line for the first file.
    pub line: Option<usize>,
    pub size: (u32, u32),
    pub scale: f32,
    pub keys: String,
    pub stats: bool,
}

impl Default for Cli {
    fn default() -> Self {
        Cli {
            action: Action::Edit,
            files: Vec::new(),
            line: None,
            size: (1200, 800),
            scale: 1.0,
            keys: String::new(),
            stats: false,
        }
    }
}

impl Cli {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Cli, String> {
        let mut cli = Cli::default();
        let mut args = args.into_iter();
        let mut only_files = false;
        while let Some(a) = args.next() {
            if only_files {
                cli.files.push(a.into());
                continue;
            }
            let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
            match a.as_str() {
                "--" => only_files = true,
                "-V" | "--version" => cli.action = Action::Version,
                "-h" | "--help" => cli.action = Action::Help,
                "--features" => cli.action = Action::Features,
                "--print-config" => cli.action = Action::PrintConfig,
                "--mcp-stdio" => cli.action = Action::McpStdio,
                "--stats" => cli.stats = true,
                "--screenshot" => cli.action = Action::Screenshot(value("--screenshot")?.into()),
                "--keys" => cli.keys = value("--keys")?,
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
                    cli.line = Some(s[1..].parse::<usize>().map_err(|_| format!("bad line number {s}"))?);
                }
                s if s.starts_with("--") => return Err(format!("unknown option {s} (see --help)")),
                _ => cli.files.push(a.into()),
            }
        }
        Ok(cli)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Cli, String> {
        Cli::parse(s.split_whitespace().map(String::from))
    }

    #[test]
    fn files_and_line() {
        let c = parse("a.rs +42 b.txt").unwrap();
        assert_eq!(c.action, Action::Edit);
        assert_eq!(c.files, vec![PathBuf::from("a.rs"), PathBuf::from("b.txt")]);
        assert_eq!(c.line, Some(42));
        assert_eq!(parse("-- --version").unwrap().files, vec![PathBuf::from("--version")]);
    }

    #[test]
    fn flags() {
        assert_eq!(parse("--version").unwrap().action, Action::Version);
        assert_eq!(parse("--mcp-stdio").unwrap().action, Action::McpStdio);
        let c = parse("--screenshot out.png --size 640x480 --scale 2 --keys ggVj main.rs").unwrap();
        assert_eq!(c.action, Action::Screenshot("out.png".into()));
        assert_eq!((c.size, c.scale, c.keys.as_str()), ((640, 480), 2.0, "ggVj"));
        assert_eq!(c.files, vec![PathBuf::from("main.rs")]);
    }

    #[test]
    fn errors() {
        assert!(parse("--size 12").is_err());
        assert!(parse("--bogus").is_err());
        assert!(parse("+x").is_err());
        assert!(parse("--screenshot").is_err());
    }
}

//! File formats shrew knows: name, how to detect it, comment tokens. Syntax grammars are looked up by `name`.

use std::path::Path;

pub struct Lang {
    pub name: &'static str,
    pub extensions: &'static [&'static str],
    /// Exact file names (`Makefile`, `Dockerfile`, ...).
    pub filenames: &'static [&'static str],
    pub line_comment: Option<&'static str>,
    pub block_comment: Option<(&'static str, &'static str)>,
}

const fn l(
    name: &'static str,
    extensions: &'static [&'static str],
    filenames: &'static [&'static str],
    line_comment: Option<&'static str>,
    block_comment: Option<(&'static str, &'static str)>,
) -> Lang {
    Lang { name, extensions, filenames, line_comment, block_comment }
}

const C_BLOCK: Option<(&str, &str)> = Some(("/*", "*/"));

pub static LANGS: &[Lang] = &[
    l("rust", &["rs"], &[], Some("//"), C_BLOCK),
    l("c", &["c", "h"], &[], Some("//"), C_BLOCK),
    l("cpp", &["cpp", "cc", "cxx", "hpp", "hh", "hxx"], &[], Some("//"), C_BLOCK),
    l("go", &["go"], &[], Some("//"), C_BLOCK),
    l("zig", &["zig"], &[], Some("//"), None),
    l("python", &["py", "pyi", "pyw"], &[], Some("#"), None),
    l("javascript", &["js", "mjs", "cjs", "jsx"], &[], Some("//"), C_BLOCK),
    l("typescript", &["ts", "mts", "cts"], &[], Some("//"), C_BLOCK),
    l("tsx", &["tsx"], &[], Some("//"), C_BLOCK),
    l("java", &["java"], &[], Some("//"), C_BLOCK),
    l("kotlin", &["kt", "kts"], &[], Some("//"), C_BLOCK),
    l("csharp", &["cs"], &[], Some("//"), C_BLOCK),
    l("swift", &["swift"], &[], Some("//"), C_BLOCK),
    l("dart", &["dart"], &[], Some("//"), C_BLOCK),
    l("php", &["php"], &[], Some("//"), C_BLOCK),
    l("ruby", &["rb"], &["Gemfile", "Rakefile"], Some("#"), None),
    l("lua", &["lua"], &[], Some("--"), Some(("--[[", "]]"))),
    l("bash", &["sh", "bash", "zsh"], &[".bashrc", ".zshrc", ".profile", "PKGBUILD"], Some("#"), None),
    l("fish", &["fish"], &[], Some("#"), None),
    l("json", &["json", "jsonc", "json5"], &[], Some("//"), None),
    l("toml", &["toml"], &["Cargo.lock"], Some("#"), None),
    l("yaml", &["yaml", "yml"], &[], Some("#"), None),
    l("markdown", &["md", "markdown"], &[], None, Some(("<!--", "-->"))),
    l("html", &["html", "htm", "xhtml"], &[], None, Some(("<!--", "-->"))),
    l("xml", &["xml", "svg", "xsd", "plist"], &[], None, Some(("<!--", "-->"))),
    l("css", &["css", "scss", "less"], &[], None, C_BLOCK),
    l("svelte", &["svelte"], &[], Some("//"), Some(("<!--", "-->"))),
    l("vue", &["vue"], &[], Some("//"), Some(("<!--", "-->"))),
    l("sql", &["sql"], &[], Some("--"), C_BLOCK),
    l("make", &["mk"], &["Makefile", "makefile", "GNUmakefile"], Some("#"), None),
    l("dockerfile", &["dockerfile"], &["Dockerfile", "Containerfile"], Some("#"), None),
    l("nix", &["nix"], &[], Some("#"), C_BLOCK),
    l("haskell", &["hs"], &[], Some("--"), Some(("{-", "-}"))),
    l("ocaml", &["ml", "mli"], &[], None, Some(("(*", "*)"))),
    l("elixir", &["ex", "exs"], &[], Some("#"), None),
    l("erlang", &["erl", "hrl"], &[], Some("%"), None),
    l("clojure", &["clj", "cljs", "edn"], &[], Some(";"), None),
    l("scala", &["scala", "sc"], &[], Some("//"), C_BLOCK),
    l("r", &["r", "R"], &[], Some("#"), None),
    l("julia", &["jl"], &[], Some("#"), Some(("#=", "=#"))),
    l("vim", &["vim"], &[".vimrc"], Some("\""), None),
    l("ini", &["ini", "cfg", "conf", "desktop"], &[], Some("#"), None),
    l("diff", &["diff", "patch"], &[], None, None),
    l("glsl", &["glsl", "vert", "frag", "comp"], &[], Some("//"), C_BLOCK),
    l("wgsl", &["wgsl"], &[], Some("//"), C_BLOCK),
    l("proto", &["proto"], &[], Some("//"), C_BLOCK),
    l("graphql", &["graphql", "gql"], &[], Some("#"), None),
    l("terraform", &["tf", "tfvars", "hcl"], &[], Some("#"), C_BLOCK),
    l("text", &["txt", "text", "log"], &[], None, None),
];

pub fn detect(path: &Path) -> Option<&'static Lang> {
    let name = path.file_name()?.to_str()?;
    if let Some(lang) = LANGS.iter().find(|l| l.filenames.contains(&name)) {
        return Some(lang);
    }
    let ext = path.extension()?.to_str()?;
    LANGS.iter().find(|l| l.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)))
}

/// Detects from a shebang line when the extension says nothing.
pub fn detect_shebang(first_line: &str) -> Option<&'static Lang> {
    let rest = first_line.strip_prefix("#!")?;
    let interp = rest.split_whitespace().flat_map(|w| w.rsplit('/').next()).find(|w| *w != "env")?;
    let name = match interp.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.') {
        "python" => "python",
        "node" | "deno" | "bun" => "javascript",
        "bash" | "sh" | "zsh" | "dash" => "bash",
        "fish" => "fish",
        "ruby" => "ruby",
        "lua" => "lua",
        _ => return None,
    };
    by_name(name)
}

pub fn by_name(name: &str) -> Option<&'static Lang> {
    LANGS.iter().find(|l| l.name == name)
}

//! The About panel's text (palette *About shrew*, `:about`): the version, the Shrew Manifesto, license, credits and
//! links. Everything it says lives here, in one place. The manifesto is also in README.md ("Manifesto"); keep the
//! two in step.

use shrew_render::{char_columns, columns};
use std::ops::Range;

pub const TAGLINE: &str = "A tiny, fast, keyboard-first text editor. Small, quick, yours.";

/// The Shrew Manifesto: (title, line).
pub const MANIFESTO: &[(&str, &str)] = &[
    ("Small is a superpower.", "One binary, a few megabytes, no runtime. Open before you finish blinking."),
    ("A heart at a thousand beats.", "Every keystroke answered within a frame. Latency is a bug."),
    (
        "Keys first, paws welcome.",
        "Your Vim and Sublime muscle memory just works; the mouse is there when you reach for it.",
    ),
    ("Forage only what you need.", "Every feature is optional. Switch off what you don't use and it costs nothing."),
    (
        "The burrow is yours.",
        "No account, no telemetry, no cloud. Bring your .vimrc and Sublime settings; they're welcome here.",
    ),
    ("Rust to the bone.", "Pure Rust, top to bottom. No hidden C, nothing you can't read."),
    ("Sharp senses.", "Syntax, search, brackets and multi-cursors: precise tools, never noise."),
    ("Never stop moving.", "Open source, MIT licensed, built in the open, one click to the latest version."),
];

pub const SIGN_OFF: &str = "Stay small. Stay quick. Shrew on.";

pub const AUTHOR: &str = "Made by Henok Moltotal (@perunok), with AI pair programming (Claude).";

pub const LICENSE: &str = "MIT License, (c) 2026 perunok. Free to use, change and share.";

/// (what, why it is thanked)
pub const CREDITS: &[(&str, &str)] = &[
    ("JetBrains Mono", "the bundled font, by JetBrains (SIL Open Font License 1.1)"),
    ("syntect", "highlighting with Sublime Text's grammars, on fancy-regex"),
    ("wgpu", "drawing on Vulkan, Metal, DX12 and GL"),
    ("cosmic-text", "shaping text and rasterising glyphs (swash, harfrust)"),
    ("winit", "windows, keys and the mouse"),
    ("ropey", "the rope behind every buffer"),
    ("regex", "search and :s"),
    ("ignore", "the .gitignore-aware walk behind Ctrl+P, the sidebar and find in project"),
    ("alacritty_terminal", "the terminal panel's emulator and PTY"),
    ("toml_edit", "config writes that keep your comments"),
    ("rustls + RustCrypto", "pure-Rust TLS for updates"),
    ("arboard", "the system clipboard"),
    ("Theme authors", "Dracula, Nord, Catppuccin, Gruvbox, Tokyo Night, Solarized and the rest"),
    ("Vim and Sublime Text", "the editors shrew learned its keys from"),
];

/// (label, url)
pub const LINKS: &[(&str, &str)] = &[
    ("Source", "https://github.com/perunok/shrew"),
    ("Issues", "https://github.com/perunok/shrew/issues"),
    ("Releases", "https://github.com/perunok/shrew/releases"),
];

/// How a piece of a line is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    /// Body text.
    Text,
    /// Secondary text.
    Muted,
    /// Bold, in the text color.
    Strong,
    /// The accent color, bold.
    Accent,
    /// A link: the accent color, underlined, opened by a click.
    Link,
}

/// One line of the panel: its text, the ink of the whole line, spans drawn differently and the links in it.
#[derive(Clone, Debug, Default)]
pub struct Line {
    pub text: String,
    pub ink: Option<Ink>,
    pub spans: Vec<(Range<usize>, Ink)>,
    /// (bytes of `text`, the URL a click opens)
    pub links: Vec<(Range<usize>, &'static str)>,
}

impl Line {
    fn new(text: impl Into<String>, ink: Ink) -> Line {
        Line { text: text.into(), ink: Some(ink), ..Line::default() }
    }
    fn blank() -> Line {
        Line::default()
    }
}

/// The panel's lines, wrapped to `cols` columns.
pub fn lines(cols: usize) -> Vec<Line> {
    let cols = cols.max(20);
    let mut out = Vec::new();
    let title = format!("shrew {}", env!("CARGO_PKG_VERSION"));
    out.push(Line { spans: vec![(0..5, Ink::Accent)], ..Line::new(title, Ink::Muted) });
    wrap_into(&mut out, TAGLINE, "", cols, Ink::Muted);
    out.push(Line::blank());

    out.push(Line::new("The Shrew Manifesto", Ink::Accent));
    out.push(Line::blank());
    for (i, (title, line)) in MANIFESTO.iter().enumerate() {
        let num = format!("{}. ", i + 1);
        let start = out.len();
        wrap_into(&mut out, &format!("{num}{title} {line}"), &" ".repeat(num.len()), cols, Ink::Text);
        // the number and the title stand out (titles are short enough to fit the first line)
        let first = &mut out[start];
        first.spans.push((0..num.len(), Ink::Accent));
        let end = (num.len() + title.len()).min(first.text.len());
        first.spans.push((num.len()..end, Ink::Strong));
    }
    out.push(Line::blank());
    wrap_into(&mut out, SIGN_OFF, "", cols, Ink::Strong);
    out.push(Line::blank());

    out.push(Line::new("License", Ink::Accent));
    wrap_into(&mut out, LICENSE, "", cols, Ink::Text);
    wrap_into(&mut out, AUTHOR, "", cols, Ink::Muted);
    out.push(Line::blank());

    out.push(Line::new("Thanks to", Ink::Accent));
    let name_w = CREDITS.iter().map(|c| columns(c.0)).max().unwrap_or(0) + 2;
    for (name, why) in CREDITS {
        if name_w + 12 > cols {
            wrap_into(&mut out, &format!("{name}: {why}"), "  ", cols, Ink::Muted);
            continue;
        }
        let pad = " ".repeat(name_w - columns(name));
        let start = out.len();
        wrap_into(&mut out, &format!("{name}{pad}{why}"), &" ".repeat(name_w), cols, Ink::Muted);
        out[start].spans.push((0..name.len(), Ink::Text));
    }
    out.push(Line::blank());

    out.push(Line::new("Links", Ink::Accent));
    let label_w = LINKS.iter().map(|l| columns(l.0)).max().unwrap_or(0) + 2;
    for (label, url) in LINKS {
        let start = out.len();
        let text = format!("{label}{}{url}", " ".repeat(label_w - columns(label)));
        wrap_into(&mut out, &text, &" ".repeat(label_w), cols, Ink::Muted);
        out[start].spans.push((0..label.len(), Ink::Text));
        // the URL is one word, so it is on one line (cut short when the panel is very narrow)
        if let Some((l, at)) = out[start..].iter_mut().find_map(|l| l.text.find(url).map(|at| (l, at))) {
            l.spans.push((at..at + url.len(), Ink::Link));
            l.links.push((at..at + url.len(), url));
        }
    }
    out
}

/// Word-wraps `text` to `cols` columns; lines after the first start with `indent`.
fn wrap_into(out: &mut Vec<Line>, text: &str, indent: &str, cols: usize, ink: Ink) {
    let mut line = String::new();
    let mut w = 0;
    for word in text.split(' ') {
        let ww = columns(word);
        if w > 0 && w + 1 + ww > cols {
            out.push(Line::new(std::mem::take(&mut line), ink));
            line.push_str(indent);
            w = columns(indent);
        } else if w > 0 {
            line.push(' ');
            w += 1;
        }
        // a word longer than the line is cut where it ends
        for ch in word.chars() {
            if w + char_columns(ch) > cols {
                break;
            }
            line.push(ch);
            w += char_columns(ch);
        }
    }
    out.push(Line::new(line, ink));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_fit_their_width() {
        for cols in [20, 40, 72, 200] {
            for l in lines(cols) {
                assert!(columns(&l.text) <= cols, "{cols}: {:?}", l.text);
                for (r, _) in &l.spans {
                    assert!(r.end <= l.text.len() && l.text.is_char_boundary(r.start), "{cols}: {:?}", l.text);
                }
            }
        }
    }

    #[test]
    fn links_are_clickable() {
        let lines = lines(72);
        let links: Vec<&str> = lines.iter().flat_map(|l| l.links.iter().map(|(_, u)| *u)).collect();
        assert_eq!(links, LINKS.iter().map(|l| l.1).collect::<Vec<_>>());
        for l in &lines {
            for (r, url) in &l.links {
                assert_eq!(&l.text[r.clone()], *url);
            }
        }
    }

    #[test]
    fn drawn_links_are_hit_targets() {
        use crate::{draw, layout::Layout, state::State};
        let mut st = State::new(shrew_config::Loaded::default(), std::sync::Arc::new(|| {}), true);
        st.ed.about = Some(usize::MAX); // scrolled to the end, where the links are
        let m = shrew_render::Metrics {
            font_size: 15.0,
            cell_width: 9.0,
            cell_height: 21.0,
            baseline: 16.0,
            ascent: 14.0,
            descent: 4.0,
            underline_offset: 18.0,
            underline_thickness: 1.0,
        };
        let lay = Layout::new(&m, (1280, 800), 1.0, 1, true, false);
        let (mut scene, mut ui) = (shrew_render::Scene::new(), draw::Ui::new());
        draw::build(&mut scene, &mut st, &mut ui, &lay, std::time::Instant::now());
        let panel = ui.hits.about.expect("the panel is drawn");
        let urls: Vec<&str> = ui.hits.about_links.iter().map(|l| l.1).collect();
        // the three links, then the repository again in the footer
        assert_eq!(urls[..3], LINKS.iter().map(|l| l.1).collect::<Vec<_>>()[..]);
        assert_eq!(urls.get(3), Some(&LINKS[0].1));
        for (r, url) in &ui.hits.about_links {
            assert!(panel.contains(r.x, r.y) && panel.contains(r.right() - 1.0, r.bottom() - 1.0), "{url}");
            assert_eq!(r.w, columns(url) as f32 * 9.0);
        }
    }

    #[test]
    fn manifesto_is_all_there() {
        let text: String = lines(72).iter().map(|l| l.text.trim().to_string() + " ").collect();
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        for (title, line) in MANIFESTO {
            assert!(text.contains(&format!("{title} {line}")), "{title}");
        }
        assert!(text.contains(SIGN_OFF));
    }

    #[test]
    fn readme_has_the_same_manifesto() {
        let readme = include_str!("../../../README.md");
        for (i, (title, line)) in MANIFESTO.iter().enumerate() {
            // the README marks the title bold and code in backticks
            let md = format!("{}. **{title}** {}", i + 1, line.replace(".vimrc", "`.vimrc`"));
            assert!(readme.contains(&md), "README.md is missing {md:?}");
        }
        assert!(readme.contains(&format!("*{SIGN_OFF}*")));
    }
}

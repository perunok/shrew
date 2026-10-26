//! Release notes (GitHub Markdown) as plain, wrapped lines for the Update shrew dialog: headings, bullets and code
//! keep their shape, the markup goes (links show their text, `**`, backticks, HTML comments and tags are dropped).
//! Not a Markdown renderer: just enough that GitHub's release bodies read well in a monospace panel.

use shrew_render::{char_columns, columns};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    Text,
    Heading,
    Code,
    /// Quotes.
    Muted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub ink: Ink,
}

/// One logical block: a paragraph, list item, heading or code line, before wrapping.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Block {
    /// Printed before the first line (`• `, `2. `, an indent).
    lead: String,
    text: String,
    ink: Ink,
}

/// `md` cleaned up and wrapped to `cols` columns. Blank lines separate blocks (never two in a row, none at the ends).
pub fn lines(md: &str, cols: usize) -> Vec<Line> {
    let cols = cols.max(8);
    let mut out = Vec::new();
    for b in blocks(md) {
        if b.text.is_empty() && b.lead.is_empty() {
            if out.last().is_some_and(|l: &Line| !l.text.is_empty()) {
                out.push(Line { text: String::new(), ink: Ink::Text });
            }
            continue;
        }
        if b.ink == Ink::Code {
            // code keeps its spaces; long lines are cut into pieces
            for piece in hard_wrap(&format!("{}{}", b.lead, b.text), cols) {
                out.push(Line { text: piece, ink: Ink::Code });
            }
            continue;
        }
        let hang = " ".repeat(columns(&b.lead));
        let room = cols.saturating_sub(columns(&b.lead)).max(4);
        for (i, l) in word_wrap(&b.text, room).into_iter().enumerate() {
            let lead = if i == 0 { b.lead.as_str() } else { hang.as_str() };
            out.push(Line { text: format!("{lead}{l}"), ink: b.ink });
        }
    }
    while out.last().is_some_and(|l| l.text.is_empty()) {
        out.pop();
    }
    out
}

fn blocks(md: &str) -> Vec<Block> {
    let md = strip_comments(&md.replace("\r\n", "\n").replace('\r', "\n"));
    let mut out: Vec<Block> = Vec::new();
    let mut fence: Option<&str> = None;
    // the block that soft-wrapped lines join (a paragraph or list item)
    let mut open = false;
    let blank = |out: &mut Vec<Block>| {
        if out.last().is_some_and(|b| !(b.text.is_empty() && b.lead.is_empty())) {
            out.push(Block { lead: String::new(), text: String::new(), ink: Ink::Text });
        }
    };
    for raw in md.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        let indent = columns(&line[..line.len() - trimmed.len()].replace('\t', "    "));
        if let Some(f) = fence {
            if trimmed.starts_with(f) {
                fence = None;
            } else {
                out.push(Block { lead: String::new(), text: line.replace('\t', "    "), ink: Ink::Code });
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = Some(&trimmed[..3]);
            open = false;
            continue;
        }
        if trimmed.is_empty() {
            blank(&mut out);
            open = false;
            continue;
        }
        // an indented block that does not continue a list item is code
        if indent >= 4 && !open {
            out.push(Block { lead: String::new(), text: trimmed.replace('\t', "    "), ink: Ink::Code });
            continue;
        }
        if is_table_separator(trimmed) {
            continue;
        }
        if is_rule(trimmed) {
            blank(&mut out);
            open = false;
            continue;
        }
        if trimmed.starts_with('|') {
            // a table row: its cells on one line
            let cells: Vec<String> = trimmed.trim_matches('|').split('|').map(|c| inline(c.trim())).collect();
            out.push(Block { lead: String::new(), text: cells.join(" │ "), ink: Ink::Text });
            open = false;
            continue;
        }
        if let Some(h) = heading(trimmed) {
            blank(&mut out);
            out.push(Block { lead: String::new(), text: inline(h), ink: Ink::Heading });
            blank(&mut out);
            open = false;
            continue;
        }
        let nest = " ".repeat((indent / 2).min(8));
        if let Some(item) = bullet(trimmed) {
            out.push(Block { lead: format!("{nest}• "), text: inline(item), ink: Ink::Text });
            open = true;
            continue;
        }
        if let Some((n, item)) = numbered(trimmed) {
            out.push(Block { lead: format!("{nest}{n}. "), text: inline(item), ink: Ink::Text });
            open = true;
            continue;
        }
        if let Some(q) = trimmed.strip_prefix('>') {
            out.push(Block { lead: "│ ".into(), text: inline(q.trim_start()), ink: Ink::Muted });
            open = false;
            continue;
        }
        let text = inline(trimmed);
        match out.last_mut() {
            // a soft-wrapped line continues its paragraph or list item
            Some(b) if open && b.ink == Ink::Text => {
                if !b.text.is_empty() && !text.is_empty() {
                    b.text.push(' ');
                }
                b.text.push_str(&text);
            }
            _ => {
                out.push(Block { lead: String::new(), text, ink: Ink::Text });
                open = true;
            }
        }
    }
    out
}

fn heading(s: &str) -> Option<&str> {
    let n = s.bytes().take_while(|&b| b == b'#').count();
    if !(1..=6).contains(&n) {
        return None;
    }
    let rest = &s[n..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None; // #hashtag
    }
    Some(rest.trim().trim_end_matches('#').trim_end())
}

fn bullet(s: &str) -> Option<&str> {
    let rest = s.strip_prefix(['-', '*', '+'])?;
    let item = rest.strip_prefix(' ')?;
    // task lists
    Some(item.strip_prefix("[ ] ").or_else(|| item.strip_prefix("[x] ")).unwrap_or(item).trim_start())
}

fn numbered(s: &str) -> Option<(&str, &str)> {
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 3 {
        return None;
    }
    let rest = s[digits..].strip_prefix(['.', ')'])?.strip_prefix(' ')?;
    Some((&s[..digits], rest.trim_start()))
}

fn is_rule(s: &str) -> bool {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    s.len() >= 3 && ['-', '*', '_'].iter().any(|&c| s.chars().all(|x| x == c))
}

fn is_table_separator(s: &str) -> bool {
    s.starts_with('|') && s.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// Removes `<!-- ... -->`, also across lines.
fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("<!--") {
        out.push_str(&rest[..i]);
        match rest[i..].find("-->") {
            Some(j) => rest = &rest[i + j + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Inline markup: links and images to their text, autolinks to their URL, emphasis, code and strike-through
/// markers and HTML tags dropped, backslash escapes resolved.
pub fn inline(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' if i + 1 < chars.len() && chars[i + 1].is_ascii_punctuation() => {
                out.push(chars[i + 1]);
                i += 2;
            }
            '!' if chars.get(i + 1) == Some(&'[') => match link(&chars, i + 1) {
                Some((text, end)) => {
                    out.push_str(&inline(&text));
                    i = end;
                }
                None => {
                    out.push(c);
                    i += 1;
                }
            },
            '[' => match link(&chars, i) {
                Some((text, end)) => {
                    out.push_str(&inline(&text));
                    i = end;
                }
                None => {
                    out.push(c);
                    i += 1;
                }
            },
            '<' => {
                let close = chars[i..].iter().position(|&x| x == '>').map(|p| i + p);
                match close {
                    Some(j) => {
                        let inner: String = chars[i + 1..j].iter().collect();
                        if inner.starts_with("http://") || inner.starts_with("https://") || inner.contains('@') {
                            out.push_str(&inner); // an autolink
                        } else if !(inner.starts_with(|c: char| c.is_ascii_alphabetic() || c == '/')) {
                            out.push('<');
                            i += 1;
                            continue;
                        }
                        i = j + 1;
                    }
                    None => {
                        out.push(c);
                        i += 1;
                    }
                }
            }
            '*' | '_' | '~' if chars.get(i + 1) == Some(&c) => {
                // ** __ ~~: emphasis and strike-through
                i += 2;
            }
            '*' if emphasis(&chars, i) => i += 1,
            '`' => i += 1,
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// `[text](url)` starting at `open` (the `[`): the text and the index after `)`.
fn link(chars: &[char], open: usize) -> Option<(String, usize)> {
    let mut depth = 0;
    let mut close = None;
    for (k, &c) in chars.iter().enumerate().skip(open) {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(k);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = chars[close + 1..].iter().position(|&c| c == ')')? + close + 1;
    Some((chars[open + 1..close].iter().collect(), end + 1))
}

/// A single `*` that opens or closes emphasis (next to a word, not a lone star or a multiplication).
fn emphasis(chars: &[char], i: usize) -> bool {
    let before = i.checked_sub(1).map(|k| chars[k]);
    let after = chars.get(i + 1).copied();
    let word = |c: Option<char>| c.is_some_and(|c| !c.is_whitespace());
    let open = word(after) && !word(before);
    let close = word(before) && !word(after);
    (open && chars[i + 1..].contains(&'*')) || (close && chars[..i].contains(&'*'))
}

/// Greedy word wrap; a word longer than the line is cut.
pub fn word_wrap(s: &str, cols: usize) -> Vec<String> {
    let mut out = vec![String::new()];
    for word in s.split(' ').filter(|w| !w.is_empty()) {
        let cur = out.last_mut().expect("one");
        let need = columns(word) + usize::from(!cur.is_empty());
        if columns(cur) + need <= cols {
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(word);
            continue;
        }
        if columns(word) <= cols {
            out.push(word.to_string());
            continue;
        }
        let mut pieces = hard_wrap(word, cols);
        let cur = out.last_mut().expect("one");
        if cur.is_empty() {
            *cur = pieces.remove(0);
        }
        out.extend(pieces);
    }
    out
}

fn hard_wrap(s: &str, cols: usize) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut w = 0;
    for ch in s.chars() {
        let cw = char_columns(ch);
        if w + cw > cols && w > 0 {
            out.push(String::new());
            w = 0;
        }
        out.last_mut().expect("one").push(ch);
        w += cw;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(md: &str, cols: usize) -> Vec<String> {
        lines(md, cols).into_iter().map(|l| l.text).collect()
    }

    #[test]
    fn headings_bullets_and_paragraphs() {
        let md = "## What's new\r\n\r\n- Git integration: change markers,\r\n  inline blame\r\n- A **Settings** panel\r\n\r\nPlain text\r\nthat soft-wraps.\r\n";
        let l = lines(md, 80);
        assert_eq!(l[0], Line { text: "What's new".into(), ink: Ink::Heading });
        let t: Vec<&str> = l.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(
            t,
            [
                "What's new",
                "",
                "• Git integration: change markers, inline blame",
                "• A Settings panel",
                "",
                "Plain text that soft-wraps."
            ]
        );
    }

    #[test]
    fn wrapping_hangs_under_the_bullet() {
        let t = texts("- one two three four five six", 14);
        assert_eq!(t, ["• one two", "  three four", "  five six"]);
        let t = texts("1. first item here", 10);
        assert_eq!(t, ["1. first", "   item", "   here"]);
        let t = texts("  - nested", 40);
        assert_eq!(t, [" • nested"]);
        // a word longer than the line is cut, never lost
        let t = texts("see https://github.com/perunok/shrew/compare/v0.1.0...v0.2.0", 20);
        assert_eq!(t.concat().replace(' ', ""), "seehttps://github.com/perunok/shrew/compare/v0.1.0...v0.2.0");
        assert!(t.iter().all(|l| columns(l) <= 20), "{t:?}");
    }

    #[test]
    fn links_images_code_and_markup() {
        assert_eq!(inline("see [docs/GIT.md](docs/GIT.md) and ![logo](x.png)"), "see docs/GIT.md and logo");
        assert_eq!(inline("**Full Changelog**: <https://x.y/z>"), "Full Changelog: https://x.y/z");
        assert_eq!(inline("run `srw -f`, ~~old~~ *new* __bold__"), "run srw -f, old new bold");
        assert_eq!(inline("a * b and 2*3"), "a * b and 2*3", "lone stars stay");
        assert_eq!(inline(r"\*literal\* \[x\]"), "*literal* [x]");
        assert_eq!(inline("line<br>break <b>bold</b> 1 < 2"), "linebreak bold 1 < 2");
        assert_eq!(inline("[unclosed"), "[unclosed");
        assert_eq!(inline("[a [nested] link](u)"), "a [nested] link");
    }

    #[test]
    fn code_blocks_comments_rules_and_tables() {
        let md = "Install:\n\n    curl -fsSL https://x/install.sh | sh\n\n```sh\nsrw   --version\n```\n<!-- hidden\nnote -->\n---\n| a | b |\n|---|---|\n| 1 | 2 |\n> quoted\n";
        let l = lines(md, 80);
        let find = |s: &str| l.iter().find(|x| x.text == s).unwrap_or_else(|| panic!("{s:?} in {l:?}")).ink;
        assert_eq!(find("Install:"), Ink::Text);
        assert_eq!(find("curl -fsSL https://x/install.sh | sh"), Ink::Code);
        assert_eq!(find("srw   --version"), Ink::Code, "code keeps its spaces");
        assert!(!l.iter().any(|x| x.text.contains("hidden") || x.text.contains("```") || x.text.contains("---")));
        assert_eq!(find("a │ b"), Ink::Text);
        let at = l.iter().position(|x| x.text == "a │ b").unwrap();
        assert_eq!(l[at + 1].text, "1 │ 2", "rows stay together, one per line");
        assert_eq!(find("│ quoted"), Ink::Muted);
        assert!(l.windows(2).all(|w| !(w[0].text.is_empty() && w[1].text.is_empty())), "no double blank lines");
        assert!(!l.first().unwrap().text.is_empty() && !l.last().unwrap().text.is_empty());
    }

    #[test]
    fn the_real_v0_2_0_body_reads_well() {
        let md = "## Install\r\n\r\nLinux and macOS, into `~/.local/bin` (checksum verified):\r\n\r\n    curl -fsSL https://raw.githubusercontent.com/perunok/shrew/master/scripts/install.sh | sh\r\n\r\nOr download below.\r\n\r\n- **Linux**: `x86_64-unknown-linux-gnu` (Intel/AMD).\r\n\r\n**Full Changelog**: https://github.com/perunok/shrew/compare/v0.1.0...v0.2.0\r\n";
        let t = texts(md, 60);
        assert_eq!(t[0], "Install");
        assert!(t.contains(&"Linux and macOS, into ~/.local/bin (checksum verified):".to_string()), "{t:?}");
        assert!(t.contains(&"• Linux: x86_64-unknown-linux-gnu (Intel/AMD).".to_string()), "{t:?}");
        assert!(t.iter().any(|l| l.starts_with("Full Changelog:")), "{t:?}");
        assert!(lines(md, 60).iter().all(|l| columns(&l.text) <= 60));
        assert!(lines("", 60).is_empty());
    }
}

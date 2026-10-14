//! TextMate `.tmTheme` files: an XML property list. A tiny reader for the subset color schemes use (dict, array,
//! key, string and scalars) keeps this free of an XML dependency.

use super::{Foreign, ForeignRule, font_style};
use crate::color::parse_rgba;

#[derive(Debug, PartialEq)]
enum Plist {
    Str(String),
    Dict(Vec<(String, Plist)>),
    Array(Vec<Plist>),
    Other,
}

impl Plist {
    fn get(&self, key: &str) -> Option<&Plist> {
        match self {
            Plist::Dict(d) => d.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn str(&self) -> Option<&str> {
        match self {
            Plist::Str(s) => Some(s),
            _ => None,
        }
    }
}

struct Reader<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> Reader<'a> {
    /// The next tag's name (`dict`, `/dict`) and whether it closes itself (`<true/>`), skipping text, comments,
    /// `<?xml?>` and `<!DOCTYPE>`.
    fn tag(&mut self) -> Option<(&'a str, bool)> {
        loop {
            let start = self.i + self.s[self.i..].find('<')?;
            let rest = &self.s[start..];
            if rest.starts_with("<!--") {
                self.i = start + rest.find("-->")? + 3;
                continue;
            }
            let end = start + rest.find('>')?;
            self.i = end + 1;
            if rest.starts_with("<?") || rest.starts_with("<!") {
                continue;
            }
            let inner = self.s[start + 1..end].trim();
            let empty = inner.ends_with('/');
            let name = inner.trim_end_matches('/').split_whitespace().next().unwrap_or("");
            return Some((name, empty));
        }
    }

    /// Text up to the closing `</name>`.
    fn text(&mut self, name: &str) -> Option<String> {
        let close = format!("</{name}>");
        let end = self.i + self.s[self.i..].find(&close)?;
        let raw = &self.s[self.i..end];
        self.i = end + close.len();
        Some(unescape(raw))
    }

    fn value(&mut self, (tag, empty): (&str, bool)) -> Option<Plist> {
        if empty {
            return Some(Plist::Other);
        }
        Some(match tag {
            "dict" => {
                let mut d = Vec::new();
                loop {
                    match self.tag()? {
                        ("/dict", _) => break Plist::Dict(d),
                        ("key", false) => {
                            let k = self.text("key")?;
                            let t = self.tag()?;
                            d.push((k, self.value(t)?));
                        }
                        _ => return None,
                    }
                }
            }
            "array" => {
                let mut a = Vec::new();
                loop {
                    match self.tag()? {
                        ("/array", _) => break Plist::Array(a),
                        t => a.push(self.value(t)?),
                    }
                }
            }
            "string" => Plist::Str(self.text("string")?),
            t => {
                self.text(t)?;
                Plist::Other
            }
        })
    }
}

fn unescape(s: &str) -> String {
    let s = s.trim();
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out += &rest[..p];
        let Some(semi) = rest[p..].find(';') else {
            out += &rest[p..];
            return out;
        };
        let ent = &rest[p + 1..p + semi];
        let c = match ent {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e => e
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| e.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match c {
            Some(c) => out.push(c),
            None => out += &rest[p..p + semi + 1],
        }
        rest = &rest[p + semi + 1..];
    }
    out + rest
}

pub fn parse(text: &str) -> Result<Foreign, String> {
    let mut r = Reader { s: text, i: 0 };
    let root = loop {
        match r.tag() {
            Some(("dict", false)) => break r.value(("dict", false)).ok_or("malformed property list")?,
            Some(_) => continue,
            None => return Err("not a property list (no <dict>)".into()),
        }
    };
    let mut f = Foreign { name: root.get("name").and_then(Plist::str).map(String::from), ..Foreign::default() };
    let Some(Plist::Array(items)) = root.get("settings") else { return Err("no settings array".into()) };
    for item in items {
        let Some(Plist::Dict(settings)) = item.get("settings") else { continue };
        let get = |k: &str| settings.iter().find(|(n, _)| n == k).and_then(|(_, v)| v.str());
        match item.get("scope").and_then(Plist::str) {
            // the first entry without a scope holds the globals
            None => {
                for (k, v) in settings {
                    if let Some(c) = v.str().and_then(parse_rgba) {
                        f.globals.insert(snake(k), c);
                    }
                }
            }
            Some(scope) => f.rules.push(ForeignRule {
                selector: scope.to_string(),
                fg: get("foreground").and_then(parse_rgba),
                font: get("fontStyle").map(font_style),
            }),
        }
    }
    Ok(f)
}

/// `lineHighlight` -> `line_highlight`, to share the globals' names with Sublime's format.
fn snake(k: &str) -> String {
    let mut s = String::with_capacity(k.len() + 4);
    for c in k.chars() {
        if c.is_ascii_uppercase() {
            s.push('_');
            s.push(c.to_ascii_lowercase());
        } else {
            s.push(c);
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_small_plist() {
        let t = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<!-- a comment -->
<key>name</key><string>Tiny &amp; Co</string>
<key>semanticClass</key><string>x</string>
<key>isDark</key><true/>
<key>settings</key><array>
<dict><key>settings</key><dict><key>background</key><string>#102030</string><key>lineHighlight</key><string>#FFFFFF10</string></dict></dict>
<dict><key>name</key><string>Comment</string><key>scope</key><string>comment</string><key>settings</key><dict><key>foreground</key><string>#778899</string><key>fontStyle</key><string>italic</string></dict></dict>
</array></dict></plist>"#;
        let f = parse(t).unwrap();
        assert_eq!(f.name.as_deref(), Some("Tiny & Co"));
        assert_eq!(f.globals["background"].0, crate::Rgb::hex(0x102030));
        assert_eq!(f.globals["line_highlight"].1, 0x10);
        assert_eq!(f.rules.len(), 1);
        assert_eq!(f.rules[0].selector, "comment");
        assert!(f.rules[0].font.unwrap().italic);
    }
}

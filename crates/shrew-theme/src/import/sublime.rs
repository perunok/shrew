//! Sublime Text `.sublime-color-scheme` files: JSON (with comments and trailing commas) holding `variables`,
//! `globals` and `rules`. Colors may be hex, `rgb()`/`rgba()`, `hsl()`/`hsla()`, CSS names, `var(name)` and the
//! `color(<base> alpha() blend() lightness() saturation())` mod function.

use super::{Foreign, ForeignRule, font_style};
use crate::color::{Rgb, parse_rgba};
use serde_json::Value;
use std::collections::HashMap;

/// A color with its alpha (0..=255), not yet flattened onto a background.
pub(crate) type Rgba = (Rgb, u8);

pub fn parse(text: &str) -> Result<Foreign, String> {
    let v: Value = serde_json::from_str(&strip_json(text)).map_err(|e| format!("not valid JSON: {e}"))?;
    let obj = v.as_object().ok_or("expected a JSON object")?;
    let mut vars: HashMap<String, String> = HashMap::new();
    if let Some(Value::Object(m)) = obj.get("variables") {
        for (k, v) in m {
            if let Some(s) = v.as_str() {
                vars.insert(k.clone(), s.to_string());
            }
        }
    }
    let col = |s: &str| eval(s, &vars, 0);
    let mut f = Foreign { name: obj.get("name").and_then(Value::as_str).map(String::from), ..Foreign::default() };
    if let Some(Value::Object(g)) = obj.get("globals") {
        for (k, v) in g {
            if let Some(c) = v.as_str().and_then(col) {
                f.globals.insert(k.clone(), c);
            }
        }
    }
    if let Some(Value::Array(rules)) = obj.get("rules") {
        for r in rules {
            let Some(scope) = r.get("scope").and_then(Value::as_str) else { continue };
            // a list of colors is a gradient for hashed syntax highlighting: take the first
            let fg = match r.get("foreground") {
                Some(Value::String(s)) => col(s),
                Some(Value::Array(a)) => a.first().and_then(Value::as_str).and_then(col),
                _ => None,
            };
            let font = r.get("font_style").and_then(Value::as_str).map(font_style);
            f.rules.push(ForeignRule { selector: scope.to_string(), fg, font });
        }
    }
    Ok(f)
}

/// Evaluates a color expression.
fn eval(s: &str, vars: &HashMap<String, String>, depth: u8) -> Option<Rgba> {
    let s = s.trim();
    if depth > 16 {
        return None;
    }
    if let Some(name) = call(s, "var") {
        return eval(vars.get(name.trim())?, vars, depth + 1);
    }
    if let Some(inner) = call(s, "color") {
        return color_mod(inner, vars, depth);
    }
    for f in ["rgba", "rgb"] {
        if let Some(args) = call(s, f) {
            let n = nums(args);
            let ch = |i: usize| n.get(i).map(|v| v.clamp(0.0, 255.0).round() as u8);
            let a = n.get(3).map_or(255, |a| (a.clamp(0.0, 1.0) * 255.0).round() as u8);
            return Some((Rgb { r: ch(0)?, g: ch(1)?, b: ch(2)? }, a));
        }
    }
    for f in ["hsla", "hsl"] {
        if let Some(args) = call(s, f) {
            let n = nums(args);
            let a = n.get(3).map_or(255, |a| (a.clamp(0.0, 1.0) * 255.0).round() as u8);
            return Some((Rgb::from_hsl(*n.first()?, n.get(1)? / 100.0, n.get(2)? / 100.0), a));
        }
    }
    if s.starts_with('#') {
        return parse_rgba(s);
    }
    named(s).map(|c| (c, if s.eq_ignore_ascii_case("transparent") { 0 } else { 255 }))
}

/// `name(args)` -> `args`.
fn call<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(name)?.trim_start().strip_prefix('(')?;
    rest.trim_end().strip_suffix(')')
}

/// Numbers in a comma/space separated list (`%` and `deg` dropped).
fn nums(s: &str) -> Vec<f32> {
    s.split([',', ' ', '/'])
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.trim_end_matches(['%']).trim_end_matches("deg").parse().ok())
        .collect()
}

/// Splits on top-level spaces (not inside parentheses).
fn words(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, None);
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if c.is_whitespace() && depth == 0 => {
                if let Some(st) = start.take() {
                    out.push(&s[st..i]);
                }
                continue;
            }
            _ => {}
        }
        start.get_or_insert(i);
    }
    if let Some(st) = start {
        out.push(&s[st..]);
    }
    out
}

/// `color(<base> alpha(0.5) blend(#fff 20%) lightness(+10%) ...)`.
fn color_mod(inner: &str, vars: &HashMap<String, String>, depth: u8) -> Option<Rgba> {
    let parts = words(inner);
    let (mut c, mut a) = eval(parts.first()?, vars, depth + 1)?;
    for adj in &parts[1..] {
        let (name, args) = adj.split_once('(')?;
        let args = args.strip_suffix(')')?;
        match name {
            "alpha" | "a" => {
                let v = nums(args).first().copied()?;
                a = (if args.contains('%') { v / 100.0 } else { v }.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
            "blend" | "blenda" => {
                let w = words(args);
                let (other, oa) = eval(w.first()?, vars, depth + 1)?;
                let pct = w.get(1).and_then(|p| p.trim_end_matches('%').parse::<f32>().ok()).unwrap_or(50.0) / 100.0;
                // `pct` of the base color, the rest the other
                c = other.mix(c, pct);
                if name == "blenda" {
                    a = (oa as f32 + (a as f32 - oa as f32) * pct).round() as u8;
                }
            }
            "lightness" | "l" | "saturation" | "s" => {
                let v = nums(args).first().copied()? / 100.0;
                let rel = args.trim_start().starts_with(['+', '-']);
                let (h, s, l) = c.hsl();
                let adj = |x: f32| if rel { x + v } else { v }.clamp(0.0, 1.0);
                c = if name.starts_with('l') { Rgb::from_hsl(h, s, adj(l)) } else { Rgb::from_hsl(h, adj(s), l) };
            }
            _ => {} // min-contrast() and friends: keep the color as is
        }
    }
    Some((c, a))
}

/// CSS color names Sublime schemes actually use.
fn named(s: &str) -> Option<Rgb> {
    let v = match s.to_ascii_lowercase().as_str() {
        "black" | "transparent" => 0x000000,
        "white" => 0xffffff,
        "red" => 0xff0000,
        "green" => 0x008000,
        "lime" => 0x00ff00,
        "blue" => 0x0000ff,
        "yellow" => 0xffff00,
        "orange" => 0xffa500,
        "purple" => 0x800080,
        "magenta" | "fuchsia" => 0xff00ff,
        "cyan" | "aqua" => 0x00ffff,
        "gray" | "grey" => 0x808080,
        "silver" => 0xc0c0c0,
        "maroon" => 0x800000,
        "navy" => 0x000080,
        "teal" => 0x008080,
        "olive" => 0x808000,
        "pink" => 0xffc0cb,
        "brown" => 0xa52a2a,
        _ => return None,
    };
    Some(Rgb::hex(v))
}

/// Removes `//` and `/* */` comments and trailing commas, which Sublime's JSON allows.
fn strip_json(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    let skip_line = |i: usize| b[i..].iter().position(|&c| c == b'\n').map_or(b.len(), |p| i + p);
    let skip_block = |i: usize| b[i + 2..].windows(2).position(|w| w == b"*/").map_or(b.len(), |p| i + 2 + p + 2);
    let next_significant = |mut i: usize| {
        while i < b.len() {
            match b[i] {
                c if c.is_ascii_whitespace() => i += 1,
                b'/' if b.get(i + 1) == Some(&b'/') => i = skip_line(i),
                b'/' if b.get(i + 1) == Some(&b'*') => i = skip_block(i),
                c => return Some(c),
            }
        }
        None
    };
    while i < b.len() {
        match b[i] {
            b'"' => {
                let start = i;
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i = (i + 1).min(b.len());
                out.extend_from_slice(&b[start..i]);
            }
            b'/' if b.get(i + 1) == Some(&b'/') => i = skip_line(i),
            b'/' if b.get(i + 1) == Some(&b'*') => i = skip_block(i),
            b',' => {
                if !matches!(next_significant(i + 1), Some(b'}' | b']')) {
                    out.push(b',');
                }
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_expressions() {
        let vars: HashMap<String, String> =
            [("blue", "#0000ff"), ("bg", "var(blue)")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        let e = |s: &str| eval(s, &vars, 0);
        assert_eq!(e("var(bg)"), Some((Rgb::hex(0x0000ff), 255)));
        assert_eq!(e("rgb(255, 0, 0)"), Some((Rgb::hex(0xff0000), 255)));
        assert_eq!(e("rgba(255, 0, 0, 0.5)"), Some((Rgb::hex(0xff0000), 128)));
        assert_eq!(e("hsl(120, 100%, 50%)"), Some((Rgb::hex(0x00ff00), 255)));
        assert_eq!(e("color(var(blue) alpha(0.25))"), Some((Rgb::hex(0x0000ff), 64)));
        assert_eq!(e("color(#000000 blend(#ffffff 50%))"), Some((Rgb::hex(0x808080), 255)));
        assert_eq!(e("white"), Some((Rgb::WHITE, 255)));
        assert_eq!(e("nonsense"), None);
    }
}

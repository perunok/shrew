//! Renders a sample editor frame headlessly to a PNG and times the hot path.
//!
//! `cargo run --release -p shrew-render --example screenshot -- [out.png] [--scale 2]`

use std::time::{Duration, Instant};

use shrew_render::{Color, FontConfig, Rect, Renderer, Scene, TextStyle};

const SAMPLE: &[&str] = &[
    "//! shrew: a tiny, fast editor.",
    "use std::collections::HashMap;",
    "",
    "fn main() -> Result<(), Box<dyn Error>> {",
    "    let mut words: HashMap<&str, usize> = HashMap::new();",
    "    for w in text.split_whitespace() {",
    "        *words.entry(w).or_insert(0) += 1; // count it",
    "    }",
    "    println!(\"日本語 テキスト, émoji 🦀🐭 and ligatures -> != <= =>\");",
    "    Ok(())",
    "}",
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let scale = args
        .iter()
        .position(|a| a == "--scale")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);
    let out = args.iter().find(|a| a.ends_with(".png")).cloned().unwrap_or_else(|| "shrew-screenshot.png".into());

    if args.iter().any(|a| a == "--startup") {
        startup(scale);
        return;
    }
    let t = Instant::now();
    let mut r = Renderer::headless((1000.0 * scale) as u32, (330.0 * scale) as u32, scale, FontConfig::default())
        .expect("renderer");
    let created = t.elapsed();
    println!("adapter: {} ({:?})", r.adapter_info().name, r.adapter_info().backend);
    println!("renderer created in {created:?}");

    // First frame with only the embedded font.
    let mut scene = Scene::new();
    let t = Instant::now();
    draw(&mut scene, &r);
    r.render(&scene, || {}).unwrap();
    println!("first frame (embedded font only): {:?} {:?}", t.elapsed(), r.stats());

    let t = Instant::now();
    let loaded = r.wait_for_fonts(Duration::from_secs(10));
    println!("system fonts ready={loaded} after another {:?}", t.elapsed());
    scene.clear();
    draw(&mut scene, &r);
    r.render(&scene, || {}).unwrap();
    println!("frame after font swap: {:?}", r.stats());
    r.save_png(&out).unwrap();
    println!("wrote {out}");

    bench(&mut r);
}

fn draw(s: &mut Scene, r: &Renderer) {
    let m = r.metrics();
    let (w, h) = r.size();
    let (w, h) = (w as f32, h as f32);
    let (ch, cw) = (m.cell_height, m.cell_width);
    let fg = TextStyle::new(Color::from_u32(0xcdd6f4));
    let kw = TextStyle::new(Color::from_u32(0xcba6f7)).bold();
    let string = TextStyle::new(Color::from_u32(0xa6e3a1));
    let comment = TextStyle::new(Color::from_u32(0x7f849c)).italic();
    let gutter_w = 4.0 * cw;

    s.background(Color::from_u32(0x1e1e2e));
    s.rect(Rect::new(0.0, 0.0, gutter_w, h), Color::from_u32(0x181825));
    s.rect(Rect::new(gutter_w, 4.0 * ch, w - gutter_w, ch), Color::from_u32(0x2a2b3c)); // current line
    s.rect(Rect::new(gutter_w + 8.0 * cw, 5.0 * ch, 5.0 * cw, ch), Color::from_u32(0x45475a)); // selection
    for (i, line) in SAMPLE.iter().enumerate() {
        let y = i as f32 * ch;
        s.text(0.0, y, &format!("{:>3}", i + 1), TextStyle::new(Color::from_u32(0x6c7086)), &[]);
        let mut spans = Vec::new();
        for k in ["use", "fn", "let", "mut", "for", "in", "dyn"] {
            for (at, _) in line.match_indices(k) {
                let before = line[..at].chars().last().is_none_or(|c| !c.is_alphanumeric());
                let after = line[at + k.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric());
                if before && after {
                    spans.push((at..at + k.len(), kw));
                }
            }
        }
        if let (Some(a), Some(b)) = (line.find('"'), line.rfind('"')) {
            spans.push((a..b + 1, string));
        }
        if let Some(c) = line.find("//") {
            spans.push((c..line.len(), comment));
        }
        if i == 6 {
            spans.push((8..13, TextStyle::new(Color::from_u32(0xf9e2af)).underline(Color::from_u32(0xf38ba8))));
        }
        spans.sort_by_key(|(r, _)| r.start);
        s.text(gutter_w, y, line, fg, &spans);
    }
    // Block cursor with inverted text.
    let (cx, cy) = (gutter_w + 4.0 * cw, 4.0 * ch);
    s.rect(Rect::new(cx, cy, cw, ch), Color::from_u32(0xf5e0dc));

    // A picker popup in an overlay layer, clipped to its box.
    s.push_layer();
    let pop = Rect::new(w - 46.0 * cw, 1.5 * ch, 44.0 * cw, 5.0 * ch);
    s.rounded_rect(Rect::new(pop.x + 3.0, pop.y + 4.0, pop.w, pop.h), 8.0, Color::rgba(0, 0, 0, 90), None); // shadow
    s.rounded_rect(pop, 8.0, Color::from_u32(0x313244), Some((1.0, Color::from_u32(0x89b4fa))));
    s.push_clip(Rect::new(pop.x + 4.0, pop.y + 4.0, pop.w - 8.0, pop.h - 8.0));
    let items =
        ["> render", "src/renderer.rs", "src/scene.rs", "a very long file name that the clip must cut off cleanly"];
    for (i, it) in items.iter().enumerate() {
        let y = pop.y + 0.5 * ch + i as f32 * ch;
        if i == 1 {
            s.rounded_rect(Rect::new(pop.x + 4.0, y, pop.w - 8.0, ch), 4.0, Color::from_u32(0x45475a), None);
        }
        s.text(pop.x + cw, y, it, fg, &[(0..it.len().min(2), TextStyle::new(Color::from_u32(0xfab387)).bold())]);
    }
    s.pop_clip();
}

/// A full 120x60 grid of syntax-colored text: time warm frames.
fn bench(r: &mut Renderer) {
    let m = r.metrics();
    r.resize((124.0 * m.cell_width) as u32, (60.0 * m.cell_height) as u32, r.scale());
    let lines: Vec<String> = (0..60)
        .map(|i| {
            let base =
                format!("{i:4} let value_{i} = compute(&items[{i}..], |x| x * 2 + offset) // trailing comment text ");
            base.chars().cycle().take(120).collect()
        })
        .collect();
    let fg = TextStyle::new(Color::from_u32(0xcdd6f4));
    let styles = [
        TextStyle::new(Color::from_u32(0xcba6f7)),
        TextStyle::new(Color::from_u32(0xa6e3a1)),
        TextStyle::new(Color::from_u32(0x89b4fa)),
    ];
    let mut spans = Vec::new();
    let mut scene = Scene::new();
    let mut fill = |scene: &mut Scene, frame: usize| {
        scene.clear();
        scene.background(Color::from_u32(0x1e1e2e));
        scene.rect(
            Rect::new(0.0, (frame % 60) as f32 * m.cell_height, 2000.0, m.cell_height),
            Color::from_u32(0x2a2b3c),
        );
        for (i, l) in lines.iter().enumerate() {
            spans.clear();
            // ~12 color spans per line, shifting with the frame so colors change but shaping stays cached.
            for k in 0..12 {
                let a = (k * 10 + frame) % 115;
                spans.push((a..a + 5, styles[k % 3]));
            }
            spans.sort_by_key(|(r, _)| r.start);
            spans.dedup_by(|b, a| b.0.start < a.0.end);
            scene.text(0.0, i as f32 * m.cell_height, l, fg, &spans);
        }
    };
    let t = Instant::now();
    fill(&mut scene, 0);
    r.render(&scene, || {}).unwrap();
    println!("bench cold frame: {:?} {:?}", t.elapsed(), r.stats());
    let t = Instant::now();
    r.prewarm_ascii();
    println!("prewarm_ascii: {:?}", t.elapsed());

    let n = 300;
    let mut cpu = Vec::with_capacity(n);
    let mut build = Duration::ZERO;
    let t_all = Instant::now();
    for f in 0..n {
        let t = Instant::now();
        fill(&mut scene, f);
        build += t.elapsed();
        r.render(&scene, || {}).unwrap();
        cpu.push(r.stats().cpu_time);
    }
    let wall = t_all.elapsed();
    cpu.sort();
    let s = r.stats();
    println!(
        "bench warm 120x60 ({} glyphs, {} rects): render cpu median {:?} p95 {:?} max {:?}; scene fill {:?}/frame; wall {:?}/frame",
        s.glyphs,
        s.rects,
        cpu[n / 2],
        cpu[n * 95 / 100],
        cpu[n - 1],
        build / n as u32,
        wall / n as u32
    );
    println!("last frame stats: {s:?}");
}

/// Startup path as the app sees it: renderer, first frame on the embedded font, then the fallback swap.
fn startup(scale: f32) {
    let t0 = Instant::now();
    let system_fonts = std::env::var_os("NO_SYSTEM_FONTS").is_none();
    let mut r =
        Renderer::headless(1600, 1000, scale, FontConfig { system_fonts, ..FontConfig::default() }).expect("renderer");
    let created = t0.elapsed();
    let mut scene = Scene::new();
    let m = r.metrics();
    let fg = TextStyle::new(Color::from_u32(0xcdd6f4));
    let kw = TextStyle::new(Color::from_u32(0xcba6f7)).bold();
    let src = include_str!("screenshot.rs");
    let fill = |scene: &mut Scene| {
        scene.clear();
        for (i, l) in src.lines().take(50).enumerate() {
            scene.text(0.0, i as f32 * m.cell_height, l, fg, &[(0..l.len().min(3), kw)]);
        }
    };
    fill(&mut scene);
    let t = Instant::now();
    let ready_before = r.fonts_ready();
    r.render(&scene, || {}).unwrap();
    println!(
        "renderer {created:?}; first frame {:?} (system fonts ready before it: {ready_before}) {:?}",
        t.elapsed(),
        r.stats()
    );
    let t = Instant::now();
    r.wait_for_fonts(Duration::from_secs(10));
    println!("waited {:?} for system fonts (since start {:?})", t.elapsed(), t0.elapsed());
    let t = Instant::now();
    fill(&mut scene);
    r.render(&scene, || {}).unwrap();
    println!("first frame after the swap {:?} {:?}", t.elapsed(), r.stats());
    fill(&mut scene);
    r.render(&scene, || {}).unwrap();
    println!("next frame {:?}", r.stats());
}

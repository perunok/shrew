//! Fonts: the configured family, grid metrics, and "soft loading" of system fonts.
//!
//! Scanning every installed font takes 50-500 ms, far too long to block the first frame on. So the renderer starts
//! with only the embedded JetBrains Mono (a few microseconds to register, nothing read from disk) and scans system
//! fonts on a background thread. When the scan lands, the font system is swapped, caches are dropped and the app is
//! woken through its callback; from then on fallback fonts (emoji, CJK, symbols) and system families work.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cosmic_text::fontdb::{Database, Source};
use cosmic_text::{
    Attrs, AttrsList, CacheKey, Family, FeatureTag, FontFeatures, FontSystem, ShapeLine, Shaping, Style, SwashCache,
    SwashImage, Weight,
};

/// Called (from the font thread) when fallback fonts finish loading.
pub type WakeFn = Arc<dyn Fn() + Send + Sync>;

/// The family the embedded font registers as.
pub const DEFAULT_FAMILY: &str = "JetBrains Mono";

#[cfg(feature = "embed-font")]
static EMBEDDED: [&[u8]; 3] = [
    include_bytes!("../assets/JetBrainsMono-Regular.ttf"),
    include_bytes!("../assets/JetBrainsMono-Bold.ttf"),
    include_bytes!("../assets/JetBrainsMono-Italic.ttf"),
];
#[cfg(not(feature = "embed-font"))]
static EMBEDDED: [&[u8]; 0] = [];

#[derive(Clone, Debug, PartialEq)]
pub struct FontConfig {
    /// Font family name; empty means the system's monospace family.
    pub family: String,
    /// Size in logical pixels (multiplied by the scale factor to get physical pixels).
    pub size: f32,
    /// Line height as a multiple of the font size.
    pub line_height: f32,
    /// Contextual alternates/ligatures (`->`, `!=` in fonts that have them).
    pub ligatures: bool,
    /// CSS weight of normal text (400 regular, 500 medium); bold text is 300 heavier. A family without that weight
    /// uses its nearest one.
    pub weight: u16,
    /// Scan system fonts in the background for fallback glyphs and non-embedded families.
    pub system_fonts: bool,
}

impl Default for FontConfig {
    fn default() -> Self {
        FontConfig {
            family: DEFAULT_FAMILY.into(),
            size: 14.0,
            line_height: 1.3,
            ligatures: true,
            weight: 400,
            system_fonts: true,
        }
    }
}

/// Grid metrics in physical pixels. Column `c` of a line starts at `x + c * cell_width`; row `r` at
/// `y + r * cell_height`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    /// Font size in physical pixels.
    pub font_size: f32,
    /// Advance of one narrow cell (not rounded: rounding would drift glyphs off their natural spacing).
    pub cell_width: f32,
    /// Line height, a whole number of pixels.
    pub cell_height: f32,
    /// Baseline offset from the top of a cell, whole pixels.
    pub baseline: f32,
    pub ascent: f32,
    pub descent: f32,
    /// Top of the underline, from the top of a cell.
    pub underline_offset: f32,
    pub underline_thickness: f32,
}

pub(crate) struct Fonts {
    pub system: FontSystem,
    pub swash: SwashCache,
    pub config: FontConfig,
    pub scale: f32,
    pub metrics: Metrics,
    features: FontFeatures,
    /// The platform's color emoji family, once system fonts are loaded and it is installed.
    pub emoji_family: Option<&'static str>,
    loader: Option<Receiver<FontSystem>>,
    frames: u64,
    ready: Arc<AtomicBool>,
    wake: Arc<Mutex<Option<WakeFn>>>,
}

impl Fonts {
    pub fn new(config: FontConfig, scale: f32) -> Self {
        let ready = Arc::new(AtomicBool::new(false));
        let wake: Arc<Mutex<Option<WakeFn>>> = Arc::default();
        let embedded = !EMBEDDED.is_empty();
        let (db, loader) = if embedded {
            let loader = config.system_fonts.then(|| spawn_loader(&config, ready.clone(), wake.clone()));
            (base_db(), loader)
        } else {
            // Nothing embedded: the scan is the only way to get any font, so it has to block.
            let mut db = base_db();
            db.load_system_fonts();
            ready.store(true, Ordering::Release);
            (db, None)
        };
        let mut system = FontSystem::new_with_locale_and_db(locale(), db);
        let metrics = compute_metrics(&mut system, &config, scale);
        let emoji_family = emoji_family(&system);
        Fonts {
            system,
            swash: SwashCache::new(),
            features: features(&config),
            emoji_family,
            config,
            scale,
            metrics,
            loader,
            frames: 0,
            ready,
            wake,
        }
    }

    /// Applies new settings; returns whether the metrics changed.
    pub fn set(&mut self, config: FontConfig, scale: f32) -> bool {
        if self.config.system_fonts != config.system_fonts
            && config.system_fonts
            && self.loader.is_none()
            && !self.ready.load(Ordering::Acquire)
        {
            self.loader = Some(spawn_loader(&config, self.ready.clone(), self.wake.clone()));
        }
        self.features = features(&config);
        self.config = config;
        self.scale = scale;
        let m = compute_metrics(&mut self.system, &self.config, scale);
        let changed = m != self.metrics;
        self.metrics = m;
        changed
    }

    pub fn set_wake(&mut self, wake: WakeFn) {
        *self.wake.lock().unwrap() = Some(wake.clone());
        if self.loader.is_some() && self.ready.load(Ordering::Acquire) {
            wake();
        }
    }

    /// Whether the system font scan finished (or was never needed).
    pub fn ready(&self) -> bool {
        self.loader.is_none()
    }

    /// Swaps in the system font set if it arrived. Returns true when it did: shaped lines are stale (fallback glyphs
    /// may now resolve), rasterized glyphs are not, because the embedded faces keep their ids (see `base_db`).
    pub fn poll(&mut self, block: Option<Duration>) -> bool {
        let Some(rx) = &self.loader else { return false };
        let system = match block {
            Some(t) => rx.recv_timeout(t).ok(),
            None => match rx.try_recv() {
                Ok(s) => Some(s),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    self.loader = None;
                    None
                }
            },
        };
        let Some(system) = system else { return false };
        self.loader = None;
        self.system = system;
        self.emoji_family = emoji_family(&self.system);
        self.metrics = compute_metrics(&mut self.system, &self.config, self.scale);
        log::debug!("system fonts swapped in ({} faces)", self.system.db().len());
        true
    }

    /// Rasterizes printable ASCII in the regular, bold and italic styles at all four subpixel x offsets: everything
    /// a typical first screen of code needs. Runs while the GPU initializes, so the first frame rasterizes nothing.
    pub fn prerender_ascii(&mut self) -> Vec<(CacheKey, SwashImage)> {
        let ascii: String = (b' '..=b'~').map(char::from).collect();
        let px = self.metrics.font_size;
        let mut out = Vec::with_capacity(ascii.len() * 12);
        for bits in [0, 1, 2] {
            let attrs = AttrsList::new(&self.attrs(bits));
            let line = ShapeLine::new(&mut self.system, &ascii, &attrs, Shaping::Advanced, 1);
            for g in line.spans.iter().flat_map(|s| &s.words).flat_map(|w| &w.glyphs) {
                for bin in 0..4 {
                    let (key, _, _) = CacheKey::new(
                        g.font_id,
                        g.glyph_id,
                        px,
                        (bin as f32 * 0.25, 0.0),
                        g.font_weight,
                        g.cache_key_flags,
                    );
                    // Blank glyphs (space) are kept too, as empty images, so they are not looked up again.
                    out.push((key, self.swash.get_image_uncached(&mut self.system, key).unwrap_or_default()));
                }
            }
        }
        out
    }

    /// Ages cosmic-text's per-word shaping cache (it makes new lines cheap: code repeats words). A word survives
    /// 128-192 frames without use; the sweep runs every 64 frames to keep per-frame cost flat.
    pub fn end_frame(&mut self) {
        self.frames += 1;
        if self.frames.is_multiple_of(64) {
            self.system.shape_run_cache.trim(2);
        }
    }

    /// Shaping attributes for a style (`bits`: 1 = bold, 2 = italic).
    pub fn attrs(&self, bits: u8) -> Attrs<'_> {
        let family = if self.config.family.is_empty() { Family::Monospace } else { Family::Name(&self.config.family) };
        let (regular, bold) = weights(&self.config);
        let mut a = Attrs::new().family(family).weight(if bits & 1 != 0 { bold } else { regular });
        if bits & 2 != 0 {
            a = a.style(Style::Italic);
        }
        if !self.features.features.is_empty() {
            a = a.font_features(self.features.clone());
        }
        a
    }
}

/// The weights of regular and bold text.
fn weights(config: &FontConfig) -> (Weight, Weight) {
    let w = config.weight.clamp(100, 900);
    (Weight(w), Weight((w + 300).clamp(700, 900)))
}

/// Installed monospace families (sorted, each once), for a font picker. Only the embedded family until the system
/// font scan has finished.
pub(crate) fn monospace_families(system: &FontSystem) -> Vec<String> {
    let mut v: Vec<String> = system
        .db()
        .faces()
        .filter(|f| f.monospaced)
        .filter_map(|f| f.families.first().map(|(n, _)| n.clone()))
        .collect();
    v.sort_unstable_by_key(|n| n.to_lowercase());
    v.dedup();
    v
}

/// Emoji go to the color emoji font first; generic fallback would often find a monochrome symbol font earlier.
fn emoji_family(system: &FontSystem) -> Option<&'static str> {
    let name = if cfg!(target_os = "macos") {
        "Apple Color Emoji"
    } else if cfg!(windows) {
        "Segoe UI Emoji"
    } else {
        "Noto Color Emoji"
    };
    system.db().faces().any(|f| f.families.iter().any(|(n, _)| n == name)).then_some(name)
}

fn features(config: &FontConfig) -> FontFeatures {
    let mut f = FontFeatures::new();
    if !config.ligatures {
        f.disable(FeatureTag::CONTEXTUAL_ALTERNATES)
            .disable(FeatureTag::STANDARD_LIGATURES)
            .disable(FeatureTag::CONTEXTUAL_LIGATURES);
    }
    f
}

/// A database holding just the embedded faces (no disk access). They are inserted first into a fresh database, so
/// they get the same ids every time: glyphs rasterized before the system-font swap stay valid after it.
fn base_db() -> Database {
    let mut db = Database::new();
    for data in EMBEDDED {
        db.load_font_source(Source::Binary(Arc::new(data)));
    }
    db.set_monospace_family(DEFAULT_FAMILY);
    db
}

fn spawn_loader(config: &FontConfig, ready: Arc<AtomicBool>, wake: Arc<Mutex<Option<WakeFn>>>) -> Receiver<FontSystem> {
    let (tx, rx) = mpsc::sync_channel(1);
    let family = config.family.clone();
    let (regular, bold) = weights(config);
    let spawned = std::thread::Builder::new().name("shrew-fonts".into()).spawn(move || {
        let t = Instant::now();
        let mut db = base_db();
        db.load_system_fonts();
        let faces = db.len();
        let mut system = FontSystem::new_with_locale_and_db(locale(), db);
        // The first shape per style ranks every installed face (opening many font files); do it here, not in a frame.
        let fam = if family.is_empty() { Family::Monospace } else { Family::Name(&family) };
        for (w, st) in
            [(regular, Style::Normal), (bold, Style::Normal), (regular, Style::Italic), (bold, Style::Italic)]
        {
            let attrs = Attrs::new().family(fam).weight(w).style(st);
            ShapeLine::new(&mut system, "warm", &AttrsList::new(&attrs), Shaping::Advanced, 1);
        }
        log::debug!("loaded {faces} system font faces in {:?}", t.elapsed());
        if tx.send(system).is_ok() {
            ready.store(true, Ordering::Release);
            let wake = wake.lock().unwrap().clone();
            if let Some(w) = wake {
                w();
            }
        }
    });
    if let Err(e) = spawned {
        log::warn!("cannot start the font thread: {e}");
    }
    rx
}

/// `LANG=de_DE.UTF-8` -> `de-DE`; affects which fallback fonts CJK text picks.
fn locale() -> String {
    let raw = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_CTYPE"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default();
    let l = raw.split(['.', '@']).next().unwrap_or("").replace('_', "-");
    if l.is_empty() || l == "C" || l == "POSIX" { "en-US".into() } else { l }
}

fn compute_metrics(system: &mut FontSystem, config: &FontConfig, scale: f32) -> Metrics {
    let px = (config.size * scale).max(1.0);
    let fam = if config.family.is_empty() { Family::Monospace } else { Family::Name(&config.family) };
    let attrs = Attrs::new().family(fam).weight(weights(config).0);
    let line = ShapeLine::new(system, "0", &AttrsList::new(&attrs), Shaping::Advanced, 1);
    let glyph = line.spans.iter().flat_map(|s| &s.words).flat_map(|w| &w.glyphs).next();
    let (cell_width, font_metrics) = match glyph {
        Some(g) => (g.x_advance * px, system.get_font(g.font_id, g.font_weight).map(|f| *f.metrics())),
        None => (px * 0.6, None),
    };
    let (mut ascent, mut descent, mut ul_offset, mut ul_thick) = (px * 0.8, px * 0.2, -px * 0.1, px / 14.0);
    if let Some(m) = font_metrics.filter(|m| m.units_per_em > 0) {
        let k = px / m.units_per_em as f32;
        ascent = m.ascent * k;
        descent = -m.descent * k;
        if let Some(u) = m.underline {
            ul_offset = u.offset * k;
            ul_thick = u.thickness * k;
        }
    }
    let cell_width = if cell_width > 0.0 { cell_width } else { px * 0.6 };
    let cell_height = (px * config.line_height.max(0.5)).round().max((ascent + descent).ceil()).max(1.0);
    let baseline = ((cell_height - ascent - descent) / 2.0 + ascent).round();
    let underline_thickness = ul_thick.round().max(1.0);
    let underline_offset = (baseline - ul_offset).round().min(cell_height - underline_thickness);
    Metrics { font_size: px, cell_width, cell_height, baseline, ascent, descent, underline_offset, underline_thickness }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> FontConfig {
        FontConfig { system_fonts: false, ..FontConfig::default() }
    }

    #[test]
    fn embedded_metrics() {
        let f = Fonts::new(cfg(), 1.0);
        let m = f.metrics;
        // JetBrains Mono advances are 600/1000 em.
        assert!((m.cell_width - 8.4).abs() < 0.01, "{m:?}");
        // 1.3 x 14 px is below the font's own ascent + descent (1.32 em), which wins.
        assert_eq!(m.cell_height, (14.0f32 * 1.32).ceil());
        assert!(m.baseline > 0.0 && m.baseline < m.cell_height);
        assert!(m.underline_offset > m.baseline && m.underline_offset + m.underline_thickness <= m.cell_height);
        assert!(f.ready());
    }

    #[test]
    fn scale_and_size_change_metrics() {
        let mut f = Fonts::new(cfg(), 1.0);
        let m1 = f.metrics;
        assert!(f.set(cfg(), 2.0));
        assert!((f.metrics.cell_width - 2.0 * m1.cell_width).abs() < 0.01);
        assert!(!f.set(FontConfig { ligatures: false, ..cfg() }, 2.0));
        assert_eq!(f.attrs(3).weight, Weight::BOLD);
        assert_eq!(f.attrs(0).weight, Weight::NORMAL);
        assert_eq!(f.attrs(3).style, Style::Italic);
        assert_eq!(f.attrs(0).font_features.features.len(), 3);
        f.set(FontConfig { weight: 500, ..cfg() }, 1.0);
        assert_eq!(f.attrs(0).weight, Weight(500));
        assert_eq!(f.attrs(1).weight, Weight(800));
        assert_eq!(monospace_families(&f.system), vec![DEFAULT_FAMILY.to_string()]);
    }

    #[test]
    #[cfg(feature = "embed-font")]
    fn embedded_ids_are_stable() {
        let ids = |db: Database| db.faces().map(|f| f.id).collect::<Vec<_>>();
        let a = ids(base_db());
        assert_eq!(a.len(), EMBEDDED.len());
        let mut full = base_db();
        full.load_font_data(EMBEDDED[0].to_vec());
        assert_eq!(&ids(full)[..a.len()], &a[..]);
        assert_eq!(ids(base_db()), a);
    }

    #[test]
    fn prerendered_ascii() {
        let mut f = Fonts::new(cfg(), 1.0);
        let imgs = f.prerender_ascii();
        assert_eq!(imgs.len(), 95 * 3 * 4);
        assert_eq!(imgs.iter().filter(|(_, i)| i.placement.width == 0).count(), 3 * 4, "only the spaces are blank");
        assert!(imgs.iter().all(|(_, i)| i.data.len() == (i.placement.width * i.placement.height) as usize));
    }

    #[test]
    fn locale_forms() {
        // Only checks the shape, since the environment is not ours to set in parallel tests.
        let l = locale();
        assert!(!l.contains('_') && !l.contains('.'));
    }
}

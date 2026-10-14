//! Opens a window, renders an animated grid of text for a moment and prints frame timings, then exits.
//! A smoke test for the surface path (present mode, resize/reconfigure, pre-present hook).
//!
//! `cargo run --release -p shrew-render --example window [-- seconds]`

use std::sync::Arc;
use std::time::{Duration, Instant};

use shrew_render::{Color, FontConfig, Rect, Renderer, Scene, TextStyle};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

struct App {
    start: Instant,
    run_for: Duration,
    proxy: EventLoopProxy<()>,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    scene: Scene,
    frames: Vec<(Duration, Duration)>,
    first_present: Option<Duration>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window =
            Arc::new(el.create_window(Window::default_attributes().with_title("shrew-render smoke test")).unwrap());
        let size = window.inner_size();
        let t = Instant::now();
        let mut r = Renderer::new(
            window.clone(),
            Some(Box::new(el.owned_display_handle())),
            size.width,
            size.height,
            window.scale_factor() as f32,
            FontConfig { system_fonts: std::env::var_os("NO_SYSTEM_FONTS").is_none(), ..FontConfig::default() },
        )
        .unwrap();
        println!(
            "window+renderer at {:?} (renderer {:?}); {:?} via {:?}, present {:?}, format {:?}",
            self.start.elapsed(),
            t.elapsed(),
            r.adapter_info().name,
            r.adapter_info().backend,
            r.present_mode(),
            r.format()
        );
        let proxy = self.proxy.clone();
        r.set_wake(Arc::new(move || {
            let _ = proxy.send_event(());
        }));
        self.renderer = Some(r);
        self.window = Some(window);
        self.window.as_ref().unwrap().request_redraw();
    }

    fn user_event(&mut self, _: &ActiveEventLoop, _: ()) {
        println!("fallback fonts ready at {:?}", self.start.elapsed());
        if let Some(r) = &mut self.renderer {
            r.poll_fonts();
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let (Some(window), Some(r)) = (&self.window, &mut self.renderer) else { return };
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(s) => {
                r.resize(s.width, s.height, window.scale_factor() as f32);
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let s = window.inner_size();
                r.resize(s.width, s.height, scale_factor as f32);
            }
            WindowEvent::RedrawRequested => {
                let n = self.frames.len();
                fill(&mut self.scene, r, n);
                let t = Instant::now();
                r.render(&self.scene, || window.pre_present_notify()).unwrap();
                let total = t.elapsed();
                if self.first_present.is_none() {
                    self.first_present = Some(self.start.elapsed());
                    println!("first frame presented at {:?}: {:?}", self.start.elapsed(), r.stats());
                }
                self.frames.push((r.stats().cpu_time, total));
                if self.start.elapsed() > self.run_for {
                    el.exit();
                } else {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }
}

fn fill(s: &mut Scene, r: &Renderer, frame: usize) {
    let m = r.metrics();
    let (w, h) = r.size();
    s.clear();
    s.background(Color::from_u32(0x1e1e2e));
    let rows = (h as f32 / m.cell_height) as usize;
    let cols = (w as f32 / m.cell_width) as usize;
    let cur = frame % rows.max(1);
    s.rect(Rect::new(0.0, cur as f32 * m.cell_height, w as f32, m.cell_height), Color::from_u32(0x313244));
    let fg = TextStyle::new(Color::from_u32(0xcdd6f4));
    let kw = TextStyle::new(Color::from_u32(0xcba6f7)).bold();
    for row in 0..rows {
        let line: String = format!("{row:4}  fn frame_{frame}() -> u64 {{ let x = {row} * 2; x }} // émoji 🦀 日本 ")
            .chars()
            .cycle()
            .take(cols)
            .collect();
        s.text(0.0, row as f32 * m.cell_height, &line, fg, &[(6..8, kw)]);
    }
    s.rect(
        Rect::new((frame % cols.max(1)) as f32 * m.cell_width, cur as f32 * m.cell_height, 2.0, m.cell_height),
        Color::from_u32(0xf5e0dc),
    );
}

fn main() {
    let secs = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(2.0);
    let start = Instant::now();
    let el = EventLoop::new().unwrap();
    let mut app = App {
        start,
        run_for: Duration::from_secs_f64(secs),
        proxy: el.create_proxy(),
        window: None,
        renderer: None,
        scene: Scene::new(),
        frames: Vec::new(),
        first_present: None,
    };
    el.run_app(&mut app).unwrap();
    let rss = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS")).map(str::to_owned));
    println!("{}", rss.unwrap_or_default());
    let mut cpu: Vec<Duration> = app.frames.iter().map(|f| f.0).collect();
    let mut total: Vec<Duration> = app.frames.iter().map(|f| f.1).collect();
    cpu.sort();
    total.sort();
    let n = cpu.len();
    if n > 0 {
        println!(
            "{n} frames in {secs}s; render cpu median {:?} p95 {:?}; render() incl. acquire median {:?}; last {:?}",
            cpu[n / 2],
            cpu[n * 95 / 100],
            total[n / 2],
            app.renderer.as_ref().map(|r| r.stats())
        );
    }
}

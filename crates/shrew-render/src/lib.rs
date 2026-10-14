//! shrew's GPU renderer: wgpu, grid text shaped by cosmic-text, a cached glyph atlas and instanced quads.
//!
//! The app fills a [`Scene`] each frame (rects and text lines in ordered layers, in physical pixels) and hands it to
//! [`Renderer::render`]. Text is laid out on a monospace grid: column `c` starts at `x + c * cell_width`, so the
//! app computes every position from [`Metrics`]. Nothing here knows about winit: the window comes in as a
//! `wgpu::SurfaceTarget`, or the renderer runs headless for tests and screenshots.
//!
//! ```no_run
//! use shrew_render::{Color, FontConfig, Rect, Renderer, Scene, TextStyle};
//!
//! let mut r = Renderer::headless(800, 200, 1.0, FontConfig::default()).unwrap();
//! let m = r.metrics();
//! let mut scene = Scene::new();
//! scene.background(Color::from_u32(0x1e1e2e));
//! scene.rect(Rect::new(0.0, 0.0, 800.0, m.cell_height), Color::from_u32(0x313244));
//! let kw = TextStyle::new(Color::from_u32(0xcba6f7)).bold();
//! scene.text(0.0, 0.0, "fn main() {}", TextStyle::new(Color::WHITE), &[(0..2, kw)]);
//! r.render(&scene, || {}).unwrap();
//! r.save_png("frame.png").unwrap();
//! ```
//!
//! With a window (winit), create the renderer once the window exists:
//!
//! ```ignore
//! let window = Arc::new(event_loop.create_window(attrs)?);
//! let size = window.inner_size();
//! let mut r = Renderer::new(window.clone(), Some(Box::new(event_loop.owned_display_handle())),
//!                           size.width, size.height, window.scale_factor() as f32, FontConfig::default())?;
//! let proxy = event_loop_proxy.clone();
//! r.set_wake(Arc::new(move || { let _ = proxy.send_event(UserEvent::FontsReady); })); // then request_redraw
//! // On RedrawRequested:
//! r.render(&scene, || window.pre_present_notify())?;
//! ```
//!
//! Startup: GPU setup runs on the calling thread while a helper thread registers the embedded JetBrains Mono,
//! computes metrics and rasterizes ASCII; system fonts (fallbacks: emoji, CJK) load on another thread and are swapped
//! in by the next `render` after they arrive. Nothing waits on a font scan.

mod atlas;
mod color;
mod fonts;
mod gpu;
mod layout;
mod renderer;
mod scene;

pub use color::Color;
pub use fonts::{DEFAULT_FAMILY, FontConfig, Metrics, WakeFn};
pub use layout::{char_columns, columns};
pub use renderer::{Error, FrameStats, Renderer, Warmup};
pub use scene::{Rect, Scene, TextStyle};

pub use wgpu;

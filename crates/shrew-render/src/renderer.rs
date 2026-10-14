//! The renderer: owns the wgpu device, the target (a window surface or an offscreen texture), fonts and caches, and
//! turns a [`Scene`] into two instanced draws per layer.
//!
//! A frame does all CPU work (shaping misses, glyph uploads, instance building) before acquiring the swapchain
//! image, so the time spent blocked on vsync is never in front of input handling.

use std::path::Path;
use std::time::{Duration, Instant};

use cosmic_text::CacheKey;

use crate::atlas::{Atlas, AtlasFull};
use crate::fonts::{FontConfig, Fonts, Metrics, WakeFn};
use crate::gpu::{Globals, GlyphInstance, InstanceBuffer, Pipelines, RectInstance};
use crate::layout::{LineCache, Placed, StyleRun, char_columns, style_runs};
use crate::scene::{Rect, TextCmd};
use crate::{Color, Scene};

#[derive(Debug)]
pub enum Error {
    /// No usable GPU adapter (or device) was found.
    Adapter(String),
    /// The window surface could not be created or configured.
    Surface(String),
    /// The surface is gone (e.g. the compositor dropped it); call [`Renderer::recreate_surface`].
    SurfaceLost,
    /// The operation needs a headless renderer.
    NotHeadless,
    /// Reading back or saving a frame failed.
    Capture(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Adapter(e) => write!(f, "no usable GPU: {e}"),
            Error::Surface(e) => write!(f, "window surface: {e}"),
            Error::SurfaceLost => f.write_str("window surface lost"),
            Error::NotHeadless => f.write_str("only a headless renderer can capture frames"),
            Error::Capture(e) => write!(f, "frame capture: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// What the last [`Renderer::render`] cost.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameStats {
    /// CPU time to build, upload, encode and submit the frame (excludes waiting for the swapchain image).
    pub cpu_time: Duration,
    /// Part of `cpu_time` spent turning the scene into instances (shaping, rasterizing, atlas lookups).
    pub build_time: Duration,
    /// Time blocked acquiring the swapchain image (vsync backpressure).
    pub acquire_time: Duration,
    pub glyph_hits: u32,
    /// Glyphs rasterized and uploaded this frame.
    pub glyph_misses: u32,
    pub line_hits: u32,
    /// Lines shaped this frame.
    pub line_misses: u32,
    pub rects: u32,
    pub glyphs: u32,
    /// True when the atlas filled up and was rebuilt during this frame.
    pub atlas_reset: bool,
    /// Glyphs held by the atlas and shaped lines held by the line cache after the frame.
    pub cached_glyphs: u32,
    pub cached_lines: u32,
}

enum Target {
    Surface { surface: wgpu::Surface<'static>, config: wgpu::SurfaceConfiguration },
    Headless { texture: wgpu::Texture, view: wgpu::TextureView },
}

/// Instance ranges of one scene layer.
struct Batch {
    rects: std::ops::Range<u32>,
    glyphs: std::ops::Range<u32>,
}

pub struct Renderer {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    target: Target,
    format: wgpu::TextureFormat,
    srgb_target: bool,
    pipes: Pipelines,
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    bind_generation: u64,
    rect_buf: InstanceBuffer,
    glyph_buf: InstanceBuffer,
    fonts: Fonts,
    atlas: Atlas,
    lines: LineCache,
    rects: Vec<RectInstance>,
    glyphs: Vec<GlyphInstance>,
    batches: Vec<Batch>,
    runs: Vec<StyleRun>,
    run_bytes: Vec<u8>,
    width: u32,
    height: u32,
    scale: f32,
    stats: FrameStats,
}

const HEADLESS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl Renderer {
    /// A renderer drawing into a window. `target` is typically an `Arc<winit::window::Window>`; `display` is the
    /// event loop's display handle (`Box::new(event_loop.owned_display_handle())`), needed by the GL backend on
    /// Wayland and harmless elsewhere. `width`/`height` are the window's physical size, `scale` its HiDPI factor.
    pub fn new<T: Into<wgpu::SurfaceTarget<'static>> + Clone>(
        target: T,
        display: Option<Box<dyn wgpu::wgt::WgpuHasDisplayHandle>>,
        width: u32,
        height: u32,
        scale: f32,
        font: FontConfig,
    ) -> Result<Self, Error> {
        with_fonts_in_parallel(font, scale, move || {
            let open = |backends, display: Option<Box<dyn wgpu::wgt::WgpuHasDisplayHandle>>, target: T| {
                let mut desc = match display {
                    Some(d) => wgpu::InstanceDescriptor::new_with_display_handle(d),
                    None => wgpu::InstanceDescriptor::new_without_display_handle(),
                };
                desc.backends = backends;
                let instance = wgpu::Instance::new(desc.with_env());
                let surface = instance.create_surface(target).map_err(|e| Error::Surface(e.to_string()))?;
                let (adapter, device, queue) = pollster::block_on(open_device(&instance, Some(&surface)))?;
                Ok::<_, Error>((instance, surface, adapter, device, queue))
            };
            // Vulkan/Metal/DX12 first; GL only when none works, since bringing up EGL as well costs startup time.
            // The display handle only matters to GL, so it goes to whichever attempt may use GL.
            let (instance, surface, adapter, device, queue) = if wgpu::Backends::from_env().is_some() {
                open(wgpu::Backends::all(), display, target)?
            } else {
                match open(wgpu::Backends::PRIMARY, None, target.clone()) {
                    Ok(gpu) => gpu,
                    Err(e) => {
                        log::warn!("{e}; trying OpenGL");
                        open(wgpu::Backends::GL, display, target)?
                    }
                }
            };

            let caps = surface.get_capabilities(&adapter);
            // Prefer a non-sRGB format: blending then happens on encoded values, like native text rendering.
            let format = caps
                .formats
                .iter()
                .copied()
                .find(|f| {
                    !f.is_srgb() && matches!(f, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm)
                })
                .or_else(|| caps.formats.first().copied())
                .ok_or_else(|| Error::Surface("adapter cannot present to this window".into()))?;
            let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
                wgpu::PresentMode::Mailbox
            } else {
                wgpu::PresentMode::Fifo
            };
            let alpha_mode = if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
                wgpu::CompositeAlphaMode::Opaque
            } else {
                wgpu::CompositeAlphaMode::Auto
            };
            let config = wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                color_space: wgpu::SurfaceColorSpace::Auto,
                width: width.max(1),
                height: height.max(1),
                present_mode,
                desired_maximum_frame_latency: 1,
                alpha_mode,
                view_formats: vec![],
            };
            if width > 0 && height > 0 {
                surface.configure(&device, &config);
            }
            log::debug!("surface {format:?} {present_mode:?} on {:?}", adapter.get_info().name);
            Ok(Gpu {
                instance,
                adapter,
                device,
                queue,
                target: Target::Surface { surface, config },
                format,
                width,
                height,
            })
        })
    }

    /// A renderer drawing into an offscreen `width` x `height` texture: tests, benchmarks and `--screenshot`.
    pub fn headless(width: u32, height: u32, scale: f32, font: FontConfig) -> Result<Self, Error> {
        let (width, height) = (width.max(1), height.max(1));
        with_fonts_in_parallel(font, scale, move || {
            let open = |backends| {
                let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
                desc.backends = backends;
                let instance = wgpu::Instance::new(desc.with_env());
                pollster::block_on(open_device(&instance, None)).map(|gpu| (instance, gpu))
            };
            let (instance, (adapter, device, queue)) = match open(wgpu::Backends::PRIMARY) {
                Ok(gpu) => gpu,
                Err(e) if wgpu::Backends::from_env().is_none() => {
                    log::warn!("{e}; trying OpenGL");
                    open(wgpu::Backends::GL)?
                }
                Err(e) => return Err(e),
            };
            let (texture, view) = offscreen(&device, width, height);
            Ok(Gpu {
                instance,
                adapter,
                device,
                queue,
                target: Target::Headless { texture, view },
                format: HEADLESS_FORMAT,
                width,
                height,
            })
        })
    }

    fn assemble(gpu: Gpu, scale: f32, fonts: Fonts, prerendered: Vec<(CacheKey, cosmic_text::SwashImage)>) -> Self {
        let Gpu { instance, adapter, device, queue, target, format, width, height } = gpu;
        let srgb_target = format.is_srgb();
        let pipes = Pipelines::new(&device, format);
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shrew globals"),
            size: size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut atlas = Atlas::new(&device);
        atlas.insert_prerendered(&prerendered, &queue);
        let bind_group = bind_group(&device, &pipes, &globals, &atlas);
        let r = Renderer {
            rect_buf: InstanceBuffer::new(&device, "shrew rects", 64 * 1024),
            glyph_buf: InstanceBuffer::new(&device, "shrew glyphs", 512 * 1024),
            bind_generation: atlas.generation,
            instance,
            adapter,
            device,
            queue,
            target,
            format,
            srgb_target,
            pipes,
            globals,
            bind_group,
            fonts,
            atlas,
            lines: LineCache::default(),
            rects: Vec::new(),
            glyphs: Vec::new(),
            batches: Vec::new(),
            runs: Vec::new(),
            run_bytes: Vec::new(),
            width,
            height,
            scale,
            stats: FrameStats::default(),
        };
        r.write_globals();
        r
    }

    fn write_globals(&self) {
        let g = Globals {
            viewport: [self.width.max(1) as f32, self.height.max(1) as f32],
            srgb_target: self.srgb_target as u32,
            _pad: 0,
        };
        self.queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&g));
    }

    pub fn metrics(&self) -> Metrics {
        self.fonts.metrics
    }

    pub fn font_config(&self) -> &FontConfig {
        &self.fonts.config
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn stats(&self) -> FrameStats {
        self.stats
    }

    /// The target's texture format (an sRGB format only when the surface offers nothing else).
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    pub fn adapter_info(&self) -> wgpu::AdapterInfo {
        self.adapter.get_info()
    }

    /// Lets the app tell the user which present mode it got (Mailbox when supported, else Fifo).
    pub fn present_mode(&self) -> Option<wgpu::PresentMode> {
        match &self.target {
            Target::Surface { config, .. } => Some(config.present_mode),
            Target::Headless { .. } => None,
        }
    }

    /// Switches the present mode (e.g. `Fifo` for strict vsync, `Immediate` for benchmarks) if the surface supports it.
    pub fn set_present_mode(&mut self, mode: wgpu::PresentMode) -> bool {
        let Target::Surface { surface, config } = &mut self.target else { return false };
        if !surface.get_capabilities(&self.adapter).present_modes.contains(&mode) {
            return false;
        }
        config.present_mode = mode;
        if self.width > 0 && self.height > 0 {
            surface.configure(&self.device, config);
        }
        true
    }

    /// New physical size and/or scale factor. Returns true when the metrics changed (the app must re-layout).
    pub fn resize(&mut self, width: u32, height: u32, scale: f32) -> bool {
        let changed = scale != self.scale && {
            self.scale = scale;
            let changed = self.fonts.set(self.fonts.config.clone(), scale);
            self.drop_glyph_caches();
            changed
        };
        if (width, height) != (self.width, self.height) {
            self.width = width;
            self.height = height;
            match &mut self.target {
                Target::Surface { surface, config } => {
                    if width > 0 && height > 0 {
                        config.width = width;
                        config.height = height;
                        surface.configure(&self.device, config);
                    }
                }
                Target::Headless { texture, view } => {
                    (*texture, *view) = offscreen(&self.device, width.max(1), height.max(1));
                }
            }
            self.write_globals();
        }
        changed
    }

    /// Applies a new font family/size/line height. Returns true when the metrics changed.
    pub fn set_font(&mut self, font: FontConfig) -> bool {
        if font == self.fonts.config {
            return false;
        }
        let changed = self.fonts.set(font, self.scale);
        self.drop_glyph_caches();
        changed
    }

    /// Changes only the font size (logical pixels), e.g. for Ctrl+= / Ctrl+-. Returns true when the metrics changed.
    pub fn set_font_size(&mut self, size: f32) -> bool {
        let font = FontConfig { size: size.clamp(4.0, 200.0), ..self.fonts.config.clone() };
        self.set_font(font)
    }

    /// Registers the callback the font thread calls once system fonts are loaded. The app should then request a
    /// redraw (typically through an `EventLoopProxy`) and call [`Renderer::poll_fonts`] or simply render.
    pub fn set_wake(&mut self, wake: WakeFn) {
        self.fonts.set_wake(wake);
    }

    /// Swaps in the system fonts if they finished loading. Returns true when it did: glyphs may change and, if the
    /// configured family is a system font, so may the metrics. [`Renderer::render`] calls this itself.
    pub fn poll_fonts(&mut self) -> bool {
        let swapped = self.fonts.poll(None);
        if swapped {
            self.lines.clear();
        }
        swapped
    }

    /// Blocks until system fonts are loaded (or `timeout` passes); for tests and screenshots.
    pub fn wait_for_fonts(&mut self, timeout: Duration) -> bool {
        if self.fonts.poll(Some(timeout)) {
            self.lines.clear();
        }
        self.fonts.ready()
    }

    /// Installed monospace font families, sorted. Just the embedded one until [`Renderer::fonts_ready`].
    pub fn monospace_families(&self) -> Vec<String> {
        crate::fonts::monospace_families(&self.fonts.system)
    }

    /// Whether the background system-font scan is done (or disabled).
    pub fn fonts_ready(&self) -> bool {
        self.fonts.ready()
    }

    fn drop_glyph_caches(&mut self) {
        self.atlas.clear();
        self.lines.clear();
    }

    /// Rasterizes printable ASCII (regular, bold, italic, every subpixel offset) into the atlas. `new` already does
    /// this during GPU setup; call it again after a font change, when idle, to keep the next frames upload-free.
    /// Costs a few to ~20 ms of CPU depending on the font size.
    pub fn prewarm_ascii(&mut self) {
        let pre = self.fonts.prerender_ascii();
        self.atlas.insert_prerendered(&pre, &self.queue);
    }

    /// Re-creates the window surface after [`Error::SurfaceLost`].
    pub fn recreate_surface(&mut self, target: impl Into<wgpu::SurfaceTarget<'static>>) -> Result<(), Error> {
        let Target::Surface { surface, config } = &mut self.target else {
            return Err(Error::Surface("headless renderer".into()));
        };
        *surface = self.instance.create_surface(target).map_err(|e| Error::Surface(e.to_string()))?;
        if self.width > 0 && self.height > 0 {
            surface.configure(&self.device, config);
        }
        Ok(())
    }

    /// Draws `scene`. `pre_present` runs right before the frame is handed to the compositor: pass
    /// `|| window.pre_present_notify()` for winit. A frame is silently skipped when the window is minimized,
    /// occluded or the swapchain timed out.
    pub fn render(&mut self, scene: &Scene, pre_present: impl FnOnce()) -> Result<(), Error> {
        let start = Instant::now();
        self.poll_fonts();
        if self.width == 0 || self.height == 0 {
            return Ok(());
        }
        self.prepare(scene);
        let clear = scene.background.unwrap_or(Color::BLACK).to_wgpu(self.srgb_target);

        let mut acquire_time = Duration::ZERO;
        let result = match &self.target {
            Target::Headless { view, .. } => {
                self.submit(view, clear);
                pre_present();
                Ok(())
            }
            Target::Surface { surface, config } => {
                let t = Instant::now();
                let mut acquired = surface.get_current_texture();
                if matches!(acquired, wgpu::CurrentSurfaceTexture::Outdated) {
                    surface.configure(&self.device, config);
                    acquired = surface.get_current_texture();
                }
                acquire_time = t.elapsed();
                let (frame, suboptimal) = match acquired {
                    wgpu::CurrentSurfaceTexture::Success(f) => (Some(f), false),
                    wgpu::CurrentSurfaceTexture::Suboptimal(f) => (Some(f), true),
                    wgpu::CurrentSurfaceTexture::Timeout
                    | wgpu::CurrentSurfaceTexture::Occluded
                    | wgpu::CurrentSurfaceTexture::Outdated => (None, false),
                    wgpu::CurrentSurfaceTexture::Lost => return Err(Error::SurfaceLost),
                    wgpu::CurrentSurfaceTexture::Validation => {
                        return Err(Error::Surface("validation error while acquiring a frame".into()));
                    }
                };
                if let Some(frame) = frame {
                    let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                    self.submit(&view, clear);
                    pre_present();
                    self.queue.present(frame);
                    if suboptimal {
                        surface.configure(&self.device, config);
                    }
                }
                Ok(())
            }
        };
        self.lines.end_frame();
        self.fonts.end_frame();
        self.stats.cached_lines = self.lines.len() as u32;
        self.stats.acquire_time = acquire_time;
        self.stats.cpu_time = start.elapsed().saturating_sub(acquire_time);
        result
    }

    /// Builds instance data for every layer and uploads it.
    fn prepare(&mut self, scene: &Scene) {
        self.atlas.hits = 0;
        self.atlas.misses = 0;
        self.lines.hits = 0;
        self.lines.misses = 0;
        let mut atlas_reset = false;
        let t = Instant::now();
        if self.build(scene, true).is_err() {
            // The atlas is at its size limit: start it over with only this frame's glyphs.
            log::debug!("glyph atlas full, rebuilding");
            self.atlas.clear();
            atlas_reset = true;
            let _ = self.build(scene, false);
        }
        let build_time = t.elapsed();
        if self.atlas.generation != self.bind_generation {
            self.bind_group = bind_group(&self.device, &self.pipes, &self.globals, &self.atlas);
            self.bind_generation = self.atlas.generation;
        }
        self.rect_buf.write(&self.device, &self.queue, bytemuck::cast_slice(&self.rects));
        self.glyph_buf.write(&self.device, &self.queue, bytemuck::cast_slice(&self.glyphs));
        self.stats = FrameStats {
            glyph_hits: self.atlas.hits,
            glyph_misses: self.atlas.misses,
            line_hits: self.lines.hits,
            line_misses: self.lines.misses,
            rects: self.rects.len() as u32,
            glyphs: self.glyphs.len() as u32,
            atlas_reset,
            build_time,
            cached_glyphs: self.atlas.len() as u32,
            ..self.stats
        };
    }

    fn build(&mut self, scene: &Scene, strict: bool) -> Result<(), AtlasFull> {
        self.rects.clear();
        self.glyphs.clear();
        self.batches.clear();
        let mut b = Builder {
            fonts: &mut self.fonts,
            atlas: &mut self.atlas,
            lines: &mut self.lines,
            device: &self.device,
            queue: &self.queue,
            rects: &mut self.rects,
            glyphs: &mut self.glyphs,
            runs: &mut self.runs,
            run_bytes: &mut self.run_bytes,
            viewport: Rect::new(0.0, 0.0, self.width as f32, self.height as f32),
            strict,
        };
        for layer in &scene.layers {
            if layer.is_empty() {
                continue;
            }
            let (r0, g0) = (b.rects.len() as u32, b.glyphs.len() as u32);
            for r in &layer.rects {
                b.rect(r.rect, r.color, r.radius, r.border, r.border_color, r.clip);
            }
            for t in &layer.texts {
                b.text(scene, t)?;
            }
            self.batches.push(Batch { rects: r0..b.rects.len() as u32, glyphs: g0..b.glyphs.len() as u32 });
        }
        Ok(())
    }

    fn submit(&self, view: &wgpu::TextureView, clear: wgpu::Color) {
        let mut enc =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("shrew frame") });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shrew pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.bind_group, &[]);
            for b in &self.batches {
                if !b.rects.is_empty() {
                    pass.set_pipeline(&self.pipes.rect);
                    pass.set_vertex_buffer(0, self.rect_buf.buffer.slice(..));
                    pass.draw(0..4, b.rects.clone());
                }
                if !b.glyphs.is_empty() {
                    pass.set_pipeline(&self.pipes.glyph);
                    pass.set_vertex_buffer(0, self.glyph_buf.buffer.slice(..));
                    pass.draw(0..4, b.glyphs.clone());
                }
            }
        }
        self.queue.submit([enc.finish()]);
    }

    /// The last rendered frame as tightly packed RGBA8 rows (headless renderers only).
    pub fn capture_rgba(&self) -> Result<Vec<u8>, Error> {
        let Target::Headless { texture, .. } = &self.target else { return Err(Error::NotHeadless) };
        let (w, h) = (self.width, self.height);
        let row = w * 4;
        let padded = row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shrew capture"),
            size: (padded * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("shrew capture") });
        enc.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| Error::Capture(e.to_string()))?;
        rx.recv().map_err(|e| Error::Capture(e.to_string()))?.map_err(|e| Error::Capture(e.to_string()))?;
        let data = slice.get_mapped_range().map_err(|e| Error::Capture(e.to_string()))?;
        let mut out = Vec::with_capacity((row * h) as usize);
        for y in 0..h as usize {
            out.extend_from_slice(&data[y * padded as usize..y * padded as usize + row as usize]);
        }
        Ok(out)
    }

    /// Saves the last rendered frame as a PNG (headless renderers only).
    pub fn save_png(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        let rgba = self.capture_rgba()?;
        image::save_buffer(path, &rgba, self.width, self.height, image::ExtendedColorType::Rgba8)
            .map_err(|e| Error::Capture(e.to_string()))
    }
}

/// Disjoint borrows of the renderer's state for building one frame's instances.
struct Builder<'a> {
    fonts: &'a mut Fonts,
    atlas: &'a mut Atlas,
    lines: &'a mut LineCache,
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    rects: &'a mut Vec<RectInstance>,
    glyphs: &'a mut Vec<GlyphInstance>,
    runs: &'a mut Vec<StyleRun>,
    run_bytes: &'a mut Vec<u8>,
    viewport: Rect,
    strict: bool,
}

impl Builder<'_> {
    fn rect(&mut self, rect: Rect, color: Color, radius: f32, border: f32, border_color: Color, clip: Rect) {
        let clip = clip.intersect(&self.viewport);
        if !rect.intersects(&clip) {
            return;
        }
        self.rects.push(RectInstance {
            rect: [rect.x, rect.y, rect.w, rect.h],
            color,
            border_color,
            params: [radius, border],
            clip: clip.corners(),
        });
    }

    fn text(&mut self, scene: &Scene, t: &TextCmd) -> Result<(), AtlasFull> {
        let m = self.fonts.metrics;
        let clip = t.clip.intersect(&self.viewport);
        if clip.is_empty() || t.y >= clip.bottom() || t.y + m.cell_height <= clip.y || t.x >= clip.right() {
            return Ok(());
        }
        let text = scene.text_of(t);
        let spans = scene.spans_of(t);

        // Underlines go first: they are rects, drawn under this layer's glyphs.
        let ul_y = t.y + m.underline_offset;
        if let Some(c) = t.style.underline {
            self.underline(text, 0, text.len(), t.x, ul_y, m, c, clip);
        }
        for (r, s) in spans {
            if let Some(c) = s.underline {
                self.underline(text, r.start as usize, r.end as usize, t.x, ul_y, m, c, clip);
            }
        }

        let default_bits = t.style.bold as u8 | (t.style.italic as u8) << 1;
        style_runs(text.len(), default_bits, spans, self.run_bytes, self.runs);
        let line = self.lines.get(self.fonts, text, self.runs);
        let sorted = spans.windows(2).all(|w| w[0].0.end <= w[1].0.start);
        let clip4 = clip.corners();
        let baseline = (t.y + m.baseline).round();
        // Atlas placements depend only on the origin's fractional x (the subpixel bins) and the atlas contents, so a
        // line drawn again at the same fraction reuses them and skips every glyph lookup. Negative origins truncate
        // the other way; they just take the slow path.
        let (ox, fx) = (t.x.floor(), t.x - t.x.floor());
        let resolve_key = (fx.to_bits(), self.atlas.epoch);
        let reuse = t.x >= 0.0 && line.resolved_key == Some(resolve_key);
        if !reuse {
            line.resolved.clear();
            line.resolved.resize(line.glyphs.len(), None);
            line.resolved_key = (t.x >= 0.0).then_some(resolve_key);
        }
        // Glyph bitmaps can start a little left of their cell (italics, overhangs).
        let left_slack = 2.0 * m.cell_width;
        let mut si = 0;
        let mut prev_byte = 0;
        for (g, slot) in line.glyphs.iter().zip(line.resolved.iter_mut()) {
            let x = t.x + g.x;
            if x >= clip.right() || x + left_slack <= clip.x {
                continue;
            }
            let style = if sorted {
                if g.byte < prev_byte {
                    si = 0; // right-to-left run: restart the span cursor
                }
                prev_byte = g.byte;
                while si < spans.len() && spans[si].0.end <= g.byte {
                    si += 1;
                }
                match spans.get(si) {
                    Some((r, s)) if r.start <= g.byte => s,
                    _ => &t.style,
                }
            } else {
                spans.iter().rev().find(|(r, _)| r.contains(&g.byte)).map_or(&t.style, |(_, s)| s)
            };
            if style.color.a == 0 {
                continue;
            }
            let placed = match slot {
                Some(p) => {
                    self.atlas.hits += 1;
                    *p
                }
                None => {
                    let dy = g.y.floor();
                    let (key, xi, yi) =
                        CacheKey::new(g.font_id, g.glyph_id, m.font_size, (fx + g.x, dy), g.weight, g.flags);
                    let p = match self.atlas.get(key, self.fonts, self.device, self.queue) {
                        Ok(e) => e.map(|e| Placed {
                            rel: [xi + e.left as i32, yi - e.top as i32],
                            uv: e.uv,
                            size: e.size,
                            color: e.color,
                        }),
                        Err(full) if self.strict => return Err(full),
                        Err(_) => continue,
                    };
                    *slot = Some(p);
                    p
                }
            };
            let Some(p) = placed else { continue };
            let pos = [ox + p.rel[0] as f32, baseline + p.rel[1] as f32];
            let size = [p.size[0] as f32, p.size[1] as f32];
            if pos[0] >= clip.right()
                || pos[1] >= clip.bottom()
                || pos[0] + size[0] <= clip.x
                || pos[1] + size[1] <= clip.y
            {
                continue;
            }
            let color = if p.color { Color::WHITE.with_alpha(style.color.a) } else { style.color };
            self.glyphs.push(GlyphInstance {
                pos,
                size,
                uv: [p.uv[0] as f32, p.uv[1] as f32],
                color,
                kind: p.color as u32,
                clip: clip4,
            });
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn underline(&mut self, text: &str, from: usize, to: usize, x: f32, y: f32, m: Metrics, color: Color, clip: Rect) {
        let col = |byte: usize| {
            text.char_indices().take_while(|(i, _)| *i < byte).map(|(_, c)| char_columns(c)).sum::<usize>() as f32
        };
        let (c0, c1) = (col(from), col(to));
        if c1 > c0 {
            let r = Rect::new(x + c0 * m.cell_width, y, (c1 - c0) * m.cell_width, m.underline_thickness);
            self.rect(r, color, 0.0, 0.0, Color::TRANSPARENT, clip);
        }
    }
}

/// What GPU setup produces before the renderer is assembled.
struct Gpu {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    target: Target,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
}

/// Runs GPU setup (`gpu`, on this thread: some platforms want surfaces made on the main thread) while another thread
/// loads fonts, computes metrics and prerenders ASCII. Device creation takes tens of milliseconds, so the first frame
/// gets its glyphs for free.
fn with_fonts_in_parallel(
    font: FontConfig,
    scale: f32,
    gpu: impl FnOnce() -> Result<Gpu, Error>,
) -> Result<Renderer, Error> {
    let t = Instant::now();
    let load = |font: FontConfig| {
        let mut fonts = Fonts::new(font, scale);
        let pre = fonts.prerender_ascii();
        (fonts, pre)
    };
    let (gpu, (fonts, prerendered)) = std::thread::scope(|s| {
        let spawned = std::thread::Builder::new().name("shrew-glyphs".into()).spawn_scoped(s, {
            let font = font.clone();
            move || load(font)
        });
        let gpu = gpu();
        let fonts = match spawned {
            Ok(h) => h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)),
            Err(_) => load(font),
        };
        (gpu, fonts)
    });
    let gpu = gpu?;
    log::debug!("renderer ready in {:?} ({} glyphs prerendered)", t.elapsed(), prerendered.len());
    Ok(Renderer::assemble(gpu, scale, fonts, prerendered))
}

async fn open_device(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'_>>,
) -> Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue), Error> {
    let power_preference = wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::LowPower);
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference,
            force_fallback_adapter: false,
            compatible_surface: surface,
            apply_limit_buckets: false,
        })
        .await
        .map_err(|e| Error::Adapter(e.to_string()))?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("shrew"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: wgpu::Trace::Off,
        })
        .await
        .map_err(|e| Error::Adapter(e.to_string()))?;
    Ok((adapter, device, queue))
}

fn offscreen(device: &wgpu::Device, width: u32, height: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("shrew offscreen"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: HEADLESS_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn bind_group(device: &wgpu::Device, pipes: &Pipelines, globals: &wgpu::Buffer, atlas: &Atlas) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("shrew bind group"),
        layout: &pipes.layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&atlas.mask.view) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&atlas.color.view) },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TextStyle;

    const BG: Color = Color::rgb(10, 20, 30);

    /// A headless renderer with only the embedded font, or `None` (test skipped) on machines without a GPU.
    fn renderer(w: u32, h: u32) -> Option<Renderer> {
        match Renderer::headless(w, h, 1.0, FontConfig { system_fonts: false, ..FontConfig::default() }) {
            Ok(r) => Some(r),
            Err(e) => {
                eprintln!("skipping GPU test: {e}");
                None
            }
        }
    }

    fn px(img: &[u8], w: u32, x: u32, y: u32) -> Color {
        let i = ((y * w + x) * 4) as usize;
        Color::rgba(img[i], img[i + 1], img[i + 2], img[i + 3])
    }

    #[test]
    fn rects_layers_and_clips() {
        let Some(mut r) = renderer(64, 32) else { return };
        let mut s = Scene::new();
        s.background(BG);
        s.rect(Rect::new(0.0, 0.0, 16.0, 16.0), Color::rgb(255, 0, 0));
        s.layer(1);
        s.rect(Rect::new(8.0, 8.0, 16.0, 16.0), Color::rgb(0, 255, 0));
        s.layer(0); // drawn under layer 1 even though it comes later
        s.rect(Rect::new(8.0, 8.0, 4.0, 4.0), Color::rgb(0, 0, 255));
        s.push_clip(Rect::new(40.0, 0.0, 10.0, 32.0));
        s.rect(Rect::new(30.0, 0.0, 30.0, 10.0), Color::WHITE);
        s.pop_clip();
        s.rect(Rect::new(30.0, 20.0, 10.0, 10.0), Color::rgba(255, 255, 255, 128));
        r.render(&s, || {}).unwrap();
        let img = r.capture_rgba().unwrap();
        assert_eq!(img.len(), 64 * 32 * 4);
        assert_eq!(px(&img, 64, 2, 2), Color::rgb(255, 0, 0));
        assert_eq!(px(&img, 64, 9, 9), Color::rgb(0, 255, 0));
        assert_eq!(px(&img, 64, 63, 31), BG);
        assert_eq!(px(&img, 64, 35, 5), BG, "clipped away");
        assert_eq!(px(&img, 64, 45, 5), Color::WHITE);
        assert_eq!(px(&img, 64, 52, 5), BG, "clipped away");
        let half = px(&img, 64, 35, 25);
        assert!((half.r as i32 - (10 + 255) / 2).abs() <= 1 && (half.b as i32 - (30 + 255) / 2).abs() <= 1, "{half:?}");
    }

    #[test]
    fn rounded_rect_corners_and_border() {
        let Some(mut r) = renderer(40, 40) else { return };
        let mut s = Scene::new();
        s.background(Color::BLACK);
        s.rounded_rect(
            Rect::new(0.0, 0.0, 40.0, 40.0),
            10.0,
            Color::rgb(0, 0, 255),
            Some((2.0, Color::rgb(255, 0, 0))),
        );
        r.render(&s, || {}).unwrap();
        let img = r.capture_rgba().unwrap();
        assert_eq!(px(&img, 40, 0, 0), Color::BLACK, "outside the rounded corner");
        assert_eq!(px(&img, 40, 20, 0), Color::rgb(255, 0, 0), "border");
        assert_eq!(px(&img, 40, 20, 20), Color::rgb(0, 0, 255), "fill");
    }

    #[test]
    #[cfg(feature = "embed-font")] // JetBrains Mono's metrics
    fn text_is_drawn_on_the_grid_with_span_colors() {
        let Some(mut r) = renderer(200, 40) else { return };
        let m = r.metrics();
        let mut s = Scene::new();
        s.background(Color::BLACK);
        let red = TextStyle::new(Color::rgb(255, 0, 0));
        s.text(0.0, 0.0, "MMMM WWWW", TextStyle::new(Color::rgb(0, 255, 0)), &[(5..9, red)]);
        r.render(&s, || {}).unwrap();
        let img = r.capture_rgba().unwrap();
        let (mut green_x, mut red_x) = (Vec::new(), Vec::new());
        for y in 0..m.cell_height as u32 {
            for x in 0..200 {
                let c = px(&img, 200, x, y);
                if c.g > 128 && c.r < 64 {
                    green_x.push(x);
                }
                if c.r > 128 && c.g < 64 {
                    red_x.push(x);
                }
            }
        }
        let (g0, g1) = (*green_x.iter().min().unwrap() as f32, *green_x.iter().max().unwrap() as f32);
        let (r0, r1) = (*red_x.iter().min().unwrap() as f32, *red_x.iter().max().unwrap() as f32);
        assert!(g0 < m.cell_width && g1 < 4.0 * m.cell_width + 1.0, "green {g0}..{g1}");
        assert!(r0 >= 5.0 * m.cell_width - 1.0 && r1 < 9.0 * m.cell_width + 1.0, "red {r0}..{r1}");
        // Nothing below the cell row.
        assert!((m.cell_height as u32..40).all(|y| (0..200).all(|x| px(&img, 200, x, y) == Color::BLACK)));
        let st = r.stats();
        assert_eq!(st.glyphs, 8);
        assert_eq!(st.glyph_misses, 0, "ASCII was prerendered");

        // Same text, other colors: shaping and placements are reused.
        s.clear();
        s.text(0.0, 0.0, "MMMM WWWW", red, &[]);
        r.render(&s, || {}).unwrap();
        assert_eq!((r.stats().line_hits, r.stats().line_misses, r.stats().glyph_misses), (1, 0, 0));
    }

    #[test]
    fn underline_spans_its_cells() {
        let Some(mut r) = renderer(120, 30) else { return };
        let m = r.metrics();
        let mut s = Scene::new();
        s.background(Color::BLACK);
        let ul = TextStyle::new(Color::TRANSPARENT).underline(Color::rgb(0, 0, 255));
        s.text(0.0, 0.0, "ab日c", TextStyle::new(Color::TRANSPARENT), &[(2..5, ul)]);
        r.render(&s, || {}).unwrap();
        let img = r.capture_rgba().unwrap();
        let y = m.underline_offset as u32;
        let blue: Vec<u32> = (0..120).filter(|&x| px(&img, 120, x, y).b > 100).collect();
        assert!((blue[0] as f32 - 2.0 * m.cell_width).abs() <= 1.0, "{blue:?}");
        assert!((*blue.last().unwrap() as f32 + 1.0 - 4.0 * m.cell_width).abs() <= 1.0, "{blue:?}");
        assert!(blue.windows(2).all(|w| w[1] == w[0] + 1));
    }

    #[test]
    fn atlas_grows_and_resets_when_full() {
        let Some(mut r) = renderer(1200, 400) else { return };
        r.set_font_size(160.0);
        let text: String = (b'!'..=b'~').map(char::from).collect();
        let mut s = Scene::new();
        for (i, chunk) in text.as_bytes().chunks(12).enumerate() {
            let line = std::str::from_utf8(chunk).unwrap();
            s.text(0.3 * i as f32, 0.0, line, TextStyle::new(Color::WHITE), &[]);
            s.text(0.6 * i as f32, 100.0, line, TextStyle::new(Color::WHITE).bold(), &[]);
        }
        let gen0 = r.atlas.generation;
        r.render(&s, || {}).unwrap();
        assert!(r.atlas.generation > gen0, "atlas grew");
        assert!(!r.stats().atlas_reset);
        let drawn = r.stats().glyphs;

        // An atlas capped below what one frame needs: the frame resets it and draws what fits.
        let Some(mut r) = renderer(1200, 400) else { return };
        r.set_font_size(160.0);
        r.atlas.max_size = r.atlas.mask_size();
        r.render(&s, || {}).unwrap();
        assert!(r.stats().atlas_reset);
        assert!(r.stats().glyphs > 0 && r.stats().glyphs < drawn, "{} of {drawn}", r.stats().glyphs);
    }

    #[test]
    fn prewarm_keeps_existing_glyphs() {
        let Some(mut r) = renderer(100, 30) else { return };
        let mut s = Scene::new();
        s.background(Color::BLACK);
        s.text(0.0, 0.0, "é日", TextStyle::new(Color::WHITE), &[]);
        r.render(&s, || {}).unwrap();
        let before = r.capture_rgba().unwrap();
        r.set_font_size(15.0);
        r.set_font_size(14.0); // atlas cleared, then refilled below, around the glyphs drawn next
        r.render(&s, || {}).unwrap();
        r.prewarm_ascii();
        r.render(&s, || {}).unwrap();
        assert_eq!(r.stats().glyph_misses, 0);
        assert_eq!(r.capture_rgba().unwrap(), before);
    }

    #[test]
    fn resize_and_metrics_change() {
        let Some(mut r) = renderer(50, 50) else { return };
        let m = r.metrics();
        assert!(!r.resize(80, 20, 1.0));
        assert_eq!(r.capture_rgba().unwrap().len(), 80 * 20 * 4);
        assert!(r.resize(80, 20, 2.0));
        assert!((r.metrics().cell_width - 2.0 * m.cell_width).abs() < 0.01);
        assert!(r.set_font_size(20.0));
        assert!(!r.set_font_size(20.0));
        let mut s = Scene::new();
        s.text(0.0, 0.0, "x", TextStyle::new(Color::WHITE), &[]);
        r.render(&s, || {}).unwrap();
        // Zero-sized windows (minimized) skip frames without errors.
        r.resize(0, 0, 2.0);
        let mut called = false;
        r.render(&s, || called = true).unwrap();
        assert!(!called);
    }

    #[test]
    fn color_emoji_after_system_fonts() {
        let Ok(mut r) = Renderer::headless(120, 40, 1.0, FontConfig::default()) else { return };
        if !r.wait_for_fonts(Duration::from_secs(20)) {
            // CI machines (Windows runners in particular) can take longer than this to scan their fonts
            eprintln!("system fonts did not arrive within 20 s; nothing to check");
            return;
        }
        let mut s = Scene::new();
        s.background(Color::BLACK);
        s.text(0.0, 0.0, "🦀", TextStyle::new(Color::WHITE), &[]);
        r.render(&s, || {}).unwrap();
        let img = r.capture_rgba().unwrap();
        let colorful = img.as_chunks::<4>().0.iter().any(|p| p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]) > 80);
        if r.fonts.emoji_family.is_some() {
            assert!(colorful, "the color emoji font must win over monochrome fallbacks");
        } else {
            eprintln!("no color emoji font installed; nothing to check");
        }
    }
}

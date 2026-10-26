//! The windowed app: winit's event loop driving a [`State`] and the GPU renderer.
//!
//! Nothing runs continuously. The loop sleeps in `ControlFlow::Wait` and a frame is drawn only after input, a wake
//! from a background thread (syntax ready, file list, fonts, update status, MCP job), or while an animation is
//! still moving. Timers (mapping timeout, drag autoscroll, cursor blink) use `WaitUntil`.
//!
//! The mouse is [`crate::mouse`]'s: this file turns winit's events into its calls and does the window work it asks
//! for (redraw, the pointer's shape, moving and resizing the window).

use crate::cli::Cli;
use crate::draw::{self, Ui};
use crate::ipc::{self, Request};
use crate::keys::{self, RawKey};
use crate::layout::Layout;
use crate::mouse::{Mouse, Response, Wheel};
use crate::state::{State, Waker};
use shrew_core::{KeyPress, Mode};
use shrew_render::{FontConfig, Rect, Renderer, Scene, Warmup};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::ModifiersState;
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{CursorIcon, Window, WindowId};

const BLINK_DELAY: Duration = Duration::from_millis(500);
const BLINK_PERIOD: Duration = Duration::from_millis(530);

pub enum UserEvent {
    /// Background work finished something: poll and redraw.
    Wake,
    /// A job from an MCP tool, to run on this thread.
    #[cfg(feature = "mcp")]
    Mcp(shrew_mcp::Job),
    /// Files from `srw` (see [`crate::ipc`]); the sender is for `--wait`.
    Open(Box<Request>, Option<Sender<()>>),
}

/// Lets MCP tools (on HTTP threads) run jobs on the UI thread.
#[cfg(feature = "mcp")]
struct AppHost(Mutex<winit::event_loop::EventLoopProxy<UserEvent>>);

#[cfg(feature = "mcp")]
impl shrew_mcp::Host for AppHost {
    fn run(&self, job: shrew_mcp::Job) -> Result<(), String> {
        let proxy = self.0.lock().unwrap_or_else(|e| e.into_inner());
        proxy.send_event(UserEvent::Mcp(job)).map_err(|_| "shrew is closing".to_string())
    }
}

#[derive(Default)]
struct Stats {
    /// (input -> frame presented, frame build, renderer CPU)
    frames: Vec<(Duration, Duration, Duration)>,
}

pub struct App {
    st: State,
    cli: Cli,
    started: Instant,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    /// The GPU being opened in the background until the window exists (see [`Warmup`]).
    warmup: Option<Warmup>,
    scene: Scene,
    ui: Ui,
    lay: Option<Layout>,
    wake_pending: Arc<AtomicBool>,
    mods: ModifiersState,
    mouse: Mouse,
    /// When the oldest input not yet on screen arrived.
    input_at: Option<Instant>,
    last_key: Instant,
    /// Input activity restarts the cursor blink.
    blink_epoch: Instant,
    preedit: bool,
    ime_allowed: bool,
    ime_rect: Option<Rect>,
    title: String,
    first_frame: bool,
    deferred: bool,
    stats: Stats,
    /// The single-instance endpoint, removed on exit.
    _ipc: Option<ipc::Guard>,
}

/// Runs the editor on `req`. `reply`: the pipe of the `srw` that started this process, told once the request is
/// taken (or why the window could not open). `server`: the single-instance endpoint, when this is the instance.
pub fn run(
    cli: Cli,
    req: Request,
    reply: Option<Box<dyn Write + Send>>,
    server: Option<ipc::Server>,
) -> Result<(), String> {
    let started = Instant::now();
    // Vulkan comes up while the config is read and the window is created
    let warmup = Warmup::start();
    // config files (and vimrc/Sublime imports) are read while the event loop connects to the display
    let config =
        std::thread::Builder::new().name("shrew-config".into()).spawn(shrew_config::load).map_err(|e| e.to_string())?;
    let el = match EventLoop::<UserEvent>::with_user_event().build() {
        Ok(el) => el,
        Err(e) => {
            let e = format!("cannot open a window: {e}");
            if let Some(w) = reply {
                let _ = ipc::write_line(w, &ipc::Reply::Error(e.clone()));
            }
            return Err(e);
        }
    };
    el.set_control_flow(ControlFlow::Wait);
    let proxy = el.create_proxy();
    let wake_pending = Arc::new(AtomicBool::new(false));
    let wake: Waker = {
        let proxy = Mutex::new(proxy.clone());
        let pending = wake_pending.clone();
        Arc::new(move || {
            // many wakes before the UI gets to them collapse into one
            if !pending.swap(true, Ordering::AcqRel) {
                let _ = proxy.lock().unwrap_or_else(|e| e.into_inner()).send_event(UserEvent::Wake);
            }
        })
    };
    let loaded = config.join().unwrap_or_default();
    let mut st = State::new(loaded, wake, false);
    #[cfg(feature = "mcp")]
    {
        shrew_mcp::set_app_version(env!("CARGO_PKG_VERSION"));
        st.mcp_host = Some(Arc::new(AppHost(Mutex::new(proxy.clone()))));
    }
    match reply {
        Some(w) => ipc::answer(
            req,
            |req, done| {
                st.open_request(req, done);
                true
            },
            w,
        ),
        None => st.open_request(req, None),
    }
    // ----- updates: a restart to update reopens what was open (unsaved changes too) -----
    #[cfg(feature = "update")]
    crate::updates::restore_session(&mut st, cli.restore.as_deref());
    // -----
    let ipc_guard = server.map(|s| {
        let proxy = Mutex::new(proxy.clone());
        s.serve(Arc::new(move |req, done| {
            let proxy = proxy.lock().unwrap_or_else(|e| e.into_inner());
            proxy.send_event(UserEvent::Open(Box::new(req), done)).is_ok()
        }))
    });
    log::debug!("state ready at {:?}", started.elapsed());
    // `srw file` in a terminal opens it here
    #[cfg(feature = "terminal")]
    {
        st.terminals.instance = ipc_guard.as_ref().map(|g| g.addr().to_path_buf());
    }
    let now = Instant::now();
    let mut app = App {
        st,
        cli,
        started,
        window: None,
        renderer: None,
        warmup,
        scene: Scene::new(),
        ui: Ui::new(),
        lay: None,
        wake_pending,
        mods: ModifiersState::empty(),
        mouse: Mouse::new(),
        input_at: Some(started),
        last_key: now,
        blink_epoch: now,
        preedit: false,
        ime_allowed: false,
        ime_rect: None,
        title: String::new(),
        first_frame: false,
        deferred: false,
        stats: Stats::default(),
        _ipc: ipc_guard,
    };
    el.run_app(&mut app).map_err(|e| e.to_string())?;
    if app.cli.stats {
        app.print_stats();
    }
    Ok(())
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(self.window_title())
            .with_inner_size(LogicalSize::new(1100.0, 720.0));
        let attrs = crate::titlebar::attributes(attrs, crate::titlebar::on(&self.st.ed));
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
        let attrs = {
            use winit::platform::startup_notify::{EventLoopExtStartupNotify, WindowAttributesExtStartupNotify};
            use winit::platform::{wayland::WindowAttributesExtWayland, x11::WindowAttributesExtX11};
            let attrs = WindowAttributesExtWayland::with_name(attrs, "shrew", "shrew");
            let attrs = WindowAttributesExtX11::with_name(attrs, "shrew", "shrew");
            // launched from a launcher or by `srw`: the activation token lets the new window take the focus
            match el.read_token_from_env() {
                Some(token) => attrs.with_activation_token(token),
                None => attrs,
            }
        };
        let window = match el.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => return fatal(el, format!("cannot create a window: {e}")),
        };
        log::debug!("window created at {:?}", self.started.elapsed());
        let size = window.inner_size();
        let t = Instant::now();
        let renderer = Renderer::with_warmup(
            self.warmup.take(),
            window.clone(),
            Some(Box::new(el.owned_display_handle())),
            size.width,
            size.height,
            window.scale_factor() as f32,
            self.st.font.clone(),
        );
        let mut r = match renderer {
            Ok(r) => r,
            Err(e) => return fatal(el, format!("no usable GPU: {e}")),
        };
        log::debug!(
            "window at {:?}, renderer in {:?} ({} via {:?})",
            self.started.elapsed(),
            t.elapsed(),
            r.adapter_info().name,
            r.adapter_info().backend
        );
        let wake = self.st.wake.clone();
        r.set_wake(Arc::new(move || wake()));
        // the desktop's light/dark preference, where winit knows it (macOS, Windows)
        if let Some(t) = window.theme() {
            self.st.set_os_dark(t == winit::window::Theme::Dark);
        }
        crate::titlebar::apply(&mut self.st, &window);
        window.set_cursor(CursorIcon::Text);
        self.mouse.set_icon(CursorIcon::Text);
        self.renderer = Some(r);
        self.window = Some(window);
        self.update_ime();
        self.redraw();
    }

    fn user_event(&mut self, el: &ActiveEventLoop, ev: UserEvent) {
        match ev {
            UserEvent::Wake => {
                self.wake_pending.store(false, Ordering::Release);
                if let Some(r) = &mut self.renderer {
                    r.poll_fonts();
                }
                self.st.poll();
                self.redraw();
            }
            #[cfg(feature = "mcp")]
            UserEvent::Mcp(job) => {
                self.st.run_job(job);
                self.redraw();
            }
            UserEvent::Open(req, done) => {
                self.st.open_request(*req, done);
                log::info!("request from srw applied: {} buffer(s) open", self.st.ed.buffers.len());
                self.raise(el);
                self.input(Instant::now());
            }
        }
    }

    fn new_events(&mut self, _el: &ActiveEventLoop, cause: StartCause) {
        if !matches!(cause, StartCause::ResumeTimeReached { .. }) {
            return;
        }
        let now = Instant::now();
        if self.st.ed.mapping_pending() && now >= self.mapping_deadline() {
            self.st.ed.flush_pending_mappings();
            self.st.after_input();
            self.input(now);
        }
        if self.mouse.deadline().is_some_and(|t| now >= t) {
            let r = self.mouse.tick(&mut self.st, &mut self.ui, self.lay, now);
            self.respond(r, now);
        }
        if self.st.cursor_blink && self.ui.focused {
            self.redraw();
        }
        // ----- git: the markers' debounce, the inline blame's pause -----
        #[cfg(feature = "git")]
        if crate::git::tick(&mut self.st, now) {
            self.redraw();
        }
        // ----- updates: the startup and daily checks, a note going away -----
        #[cfg(feature = "update")]
        if crate::updates::tick(&mut self.st, now) {
            self.redraw();
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if self.st.quit {
            el.exit();
            return;
        }
        if self.first_frame && !self.deferred {
            self.deferred = true;
            self.st.start_deferred();
            if self.st.ed.picker.is_some() || !self.st.ed.events.is_empty() {
                self.redraw();
            }
        }
        // the next timer, if any
        let mut next: Option<Instant> = None;
        let mut at = |t: Instant| next = Some(next.map_or(t, |n: Instant| n.min(t)));
        if self.st.ed.mapping_pending() {
            at(self.mapping_deadline());
        }
        if let Some(t) = self.mouse.deadline() {
            at(t);
        }
        if self.st.cursor_blink && self.ui.focused {
            at(self.next_blink(Instant::now()));
        }
        // ----- git -----
        #[cfg(feature = "git")]
        if let Some(t) = crate::git::deadline(&self.st) {
            at(t);
        }
        // ----- updates -----
        #[cfg(feature = "update")]
        if let Some(t) = crate::updates::deadline(&self.st) {
            at(t);
        }
        el.set_control_flow(match next {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::RedrawRequested => self.frame(),
            WindowEvent::CloseRequested => {
                self.st.close_requested();
                if self.st.quit {
                    el.exit();
                } else {
                    self.redraw();
                }
            }
            WindowEvent::Resized(size) => self.resize(size, None),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(w) = &self.window {
                    let size = w.inner_size();
                    self.resize(size, Some(scale_factor as f32));
                }
            }
            WindowEvent::Focused(f) => {
                self.ui.focused = f;
                // files may have changed while away: the sidebar reads its folders again
                if f && self.st.ed.sidebar_on()
                    && let Some(t) = &mut self.st.ed.tree
                {
                    t.refresh();
                }
                if !f {
                    self.mods = ModifiersState::empty();
                    self.mouse.mods = keys::mods_of(self.mods);
                    // menus close when the window loses the focus
                    self.st.titlebar.menu = None;
                    self.st.menu = None;
                }
                #[cfg(feature = "terminal")]
                self.st.terminal_window_focus(f);
                // ----- git: watch repositories only while focused; diffs re-read what changed meanwhile -----
                #[cfg(feature = "git")]
                crate::git::focus(&mut self.st, f);
                self.blink_epoch = Instant::now();
                self.redraw();
            }
            WindowEvent::ModifiersChanged(m) => {
                self.mods = m.state();
                self.mouse.mods = keys::mods_of(self.mods);
            }
            WindowEvent::ThemeChanged(t) => {
                self.st.set_os_dark(t == winit::window::Theme::Dark);
                self.redraw();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                let now = Instant::now();
                let unmodified = event.key_without_modifiers();
                let raw = RawKey {
                    logical: &event.logical_key,
                    unmodified: &unmodified,
                    physical: event.physical_key,
                    text: event.text.as_deref(),
                    mods: self.mods,
                };
                let presses = keys::convert(&raw);
                if presses.is_empty() {
                    return;
                }
                for k in presses {
                    self.st.key(k);
                }
                self.last_key = now;
                self.input(now);
            }
            WindowEvent::Ime(ime) => self.ime(ime),
            WindowEvent::CursorMoved { position, .. } => {
                let now = Instant::now();
                let (x, y) = (position.x as f32, position.y as f32);
                let r = self.mouse.moved(&mut self.st, &mut self.ui, self.lay, x, y, now);
                self.respond(r, now);
            }
            WindowEvent::CursorLeft { .. } => {
                let r = self.mouse.left(&mut self.st, &mut self.ui, self.lay);
                self.respond(r, Instant::now());
            }
            WindowEvent::MouseInput { state, button, .. } => self.mouse_button(state, button),
            WindowEvent::MouseWheel { delta, .. } => {
                let delta = match delta {
                    MouseScrollDelta::LineDelta(x, y) => Wheel::Lines(x, y),
                    MouseScrollDelta::PixelDelta(d) => Wheel::Pixels(d.x as f32, d.y as f32),
                };
                let r = self.mouse.wheel(&mut self.st, &mut self.ui, self.lay, delta);
                self.respond(r, Instant::now());
            }
            WindowEvent::DroppedFile(path) => {
                if path.is_dir() {
                    self.st.open_request(Request { dir: Some(path), ..Request::default() }, None);
                } else {
                    self.st.open_at(&path, None, None);
                }
                self.st.after_input();
                self.input(Instant::now());
            }
            _ => {}
        }
        if self.st.quit {
            el.exit();
        }
    }

    fn exiting(&mut self, _el: &ActiveEventLoop) {
        #[cfg(feature = "mcp")]
        if let Some(s) = self.st.mcp.take() {
            s.stop();
        }
    }
}

impl App {
    fn redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    /// Something the user did changed the editor: draw it, restart the blink, keep the input method in step.
    fn input(&mut self, now: Instant) {
        // window work the title bar queued (its buttons, menus, a feature switched)
        if let Some(w) = &self.window
            && crate::titlebar::apply(&mut self.st, w)
        {
            self.st.close_requested();
        }
        self.input_at.get_or_insert(now);
        self.blink_epoch = now;
        self.update_ime();
        self.redraw();
    }

    /// Brings the window to the front for a request from `srw`. X11, Windows and macOS: `focus_window` (the window
    /// manager may still prefer to flash it). Wayland has no focus stealing: winit can apply an activation token only
    /// to a window it creates, not the client's token to this one, so the compositor is asked for attention instead
    /// (KDE Plasma highlights the task bar entry).
    fn raise(&self, el: &ActiveEventLoop) {
        let Some(w) = &self.window else { return };
        if w.is_minimized() == Some(true) {
            w.set_minimized(false);
        }
        w.focus_window();
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
        {
            use winit::platform::wayland::ActiveEventLoopExtWayland;
            if el.is_wayland() && !self.ui.focused {
                w.request_user_attention(Some(winit::window::UserAttentionType::Informational));
            }
        }
        let _ = el;
    }

    fn mapping_deadline(&self) -> Instant {
        self.last_key + Duration::from_millis(self.st.ed.keymap.timeout_ms)
    }

    fn next_blink(&self, now: Instant) -> Instant {
        let start = self.blink_epoch + BLINK_DELAY;
        if now < start {
            return start;
        }
        let n = ((now - start).as_millis() / BLINK_PERIOD.as_millis()) as u32 + 1;
        start + BLINK_PERIOD * n
    }

    fn window_title(&self) -> String {
        crate::titlebar::window_title(&self.st.ed)
    }

    fn resize(&mut self, size: PhysicalSize<u32>, scale: Option<f32>) {
        let (Some(r), Some(w)) = (&mut self.renderer, &self.window) else { return };
        let scale = scale.unwrap_or(w.scale_factor() as f32);
        r.resize(size.width, size.height, scale);
        self.st.titlebar.maximized = w.is_maximized();
        self.redraw();
    }

    fn layout(&self) -> Option<Layout> {
        let r = self.renderer.as_ref()?;
        Some(crate::layout::compute(r, &self.st))
    }

    fn frame(&mut self) {
        // a theme/font picker previews its highlighted row, whatever moved it (keys, wheel, mouse)
        self.st.sync_preview();
        if let Some(r) = &self.renderer
            && self.st.look.font_families.is_none()
            && r.fonts_ready()
        {
            self.st.look.font_families = Some(r.monospace_families());
        }
        if self.st.font_changed {
            self.st.font_changed = false;
            let font: FontConfig = self.st.font.clone();
            if let Some(r) = &mut self.renderer {
                r.set_font(font);
            }
        }
        let Some(lay) = self.layout() else { return };
        let (Some(r), Some(window)) = (&mut self.renderer, &self.window) else { return };
        let now = Instant::now();
        self.ui.blink_off = self.st.cursor_blink && self.ui.focused && blink_phase(self.blink_epoch, now);
        let t = Instant::now();
        let animating = draw::build(&mut self.scene, &mut self.st, &mut self.ui, &lay, now);
        let build = t.elapsed();
        self.lay = Some(lay);
        match r.render(&self.scene, || window.pre_present_notify()) {
            Ok(()) => {}
            Err(shrew_render::Error::SurfaceLost) => {
                if let Err(e) = r.recreate_surface(window.clone()) {
                    log::error!("surface lost and not recreated: {e}");
                }
                window.request_redraw();
            }
            Err(e) => log::error!("render failed: {e}"),
        }
        let s = r.stats();
        if let Some(t0) = self.input_at.take() {
            let latency = t0.elapsed();
            if self.cli.stats {
                eprintln!(
                    "frame: input→present {latency:?}  build {build:?}  render cpu {:?}  glyph misses {}  line misses {}",
                    s.cpu_time, s.glyph_misses, s.line_misses
                );
            }
            self.stats.frames.push((latency, build, s.cpu_time));
        }
        log::debug!("frame build {build:?} render {:?} rects {} glyphs {}", s.cpu_time, s.rects, s.glyphs);
        if !self.first_frame {
            self.first_frame = true;
            log::info!("first frame at {:?}", self.started.elapsed());
            if self.cli.stats {
                eprintln!("first frame presented {:?} after start", self.started.elapsed());
            }
        }
        let title = self.window_title();
        if title != self.title {
            window.set_title(&title);
            self.title = title;
        }
        if self.ui.cursor_rect != self.ime_rect
            && let Some(c) = self.ui.cursor_rect
        {
            window.set_ime_cursor_area(
                PhysicalPosition::new(c.x as f64, c.y as f64),
                PhysicalSize::new(c.w as f64, c.h as f64),
            );
            self.ime_rect = Some(c);
        }
        if animating {
            window.request_redraw();
        }
    }

    fn print_stats(&self) {
        let f = &self.stats.frames;
        if f.is_empty() {
            return;
        }
        let pick = |i: usize| {
            let mut v: Vec<Duration> = f.iter().map(|x| [x.0, x.1, x.2][i]).collect();
            v.sort();
            (v[v.len() / 2], v[(v.len() * 95 / 100).min(v.len() - 1)], v[v.len() - 1])
        };
        let (a, b, c) = (pick(0), pick(1), pick(2));
        eprintln!("{} input frames  (median / p95 / max)", f.len());
        eprintln!("  input→present  {:?} / {:?} / {:?}", a.0, a.1, a.2);
        eprintln!("  frame build    {:?} / {:?} / {:?}", b.0, b.1, b.2);
        eprintln!("  render cpu     {:?} / {:?} / {:?}", c.0, c.1, c.2);
    }

    // ----- input methods -----

    /// Input methods are on where text is typed (insert mode, Sublime mode, prompts, pickers).
    fn update_ime(&mut self) {
        let ed = &self.st.ed;
        let typing = (ed.mode == Mode::Insert || !ed.vim()) && !ed.sidebar_focus;
        let allowed = typing || ed.prompt.is_some() || ed.picker.is_some();
        #[cfg(feature = "terminal")]
        let allowed = allowed || self.st.terminals.has_focus();
        #[cfg(feature = "git")]
        let allowed = allowed || crate::git::typing(&self.st);
        // an open menu takes the keys
        let allowed = allowed && !self.st.menu_open();
        if allowed != self.ime_allowed
            && let Some(w) = &self.window
        {
            w.set_ime_allowed(allowed);
            self.ime_allowed = allowed;
        }
    }

    fn ime(&mut self, ime: Ime) {
        match ime {
            Ime::Commit(s) => {
                let composed = std::mem::take(&mut self.preedit);
                let ed = &self.st.ed;
                let typing = ed.mode == Mode::Insert || !ed.vim();
                if composed && typing && ed.prompt.is_none() && ed.picker.is_none() {
                    self.st.text(&s);
                } else {
                    // plain keys that came through the input method: keys, so mappings like `jk` still work
                    for c in s.chars().filter(|c| !c.is_control()) {
                        self.st.key(KeyPress::ch(c));
                    }
                }
                self.ui_preedit(String::new());
                self.input(Instant::now());
            }
            Ime::Preedit(s, _) => {
                self.preedit = !s.is_empty();
                self.ui_preedit(s);
                self.redraw();
            }
            Ime::Enabled | Ime::Disabled => {}
        }
    }

    fn ui_preedit(&mut self, s: String) {
        self.ui.preedit = s;
    }

    // ----- mouse -----

    fn mouse_button(&mut self, state: ElementState, button: MouseButton) {
        let button = match button {
            MouseButton::Left => shrew_core::MouseButton::Left,
            MouseButton::Middle => shrew_core::MouseButton::Middle,
            MouseButton::Right => shrew_core::MouseButton::Right,
            _ => return,
        };
        let now = Instant::now();
        let (x, y) = self.mouse.pointer;
        let (st, ui) = (&mut self.st, &mut self.ui);
        let r = match state {
            ElementState::Pressed => self.mouse.press(st, ui, self.lay, x, y, button, now),
            ElementState::Released => self.mouse.release(st, ui, self.lay, x, y, button),
        };
        self.respond(r, now);
    }

    /// Does what the mouse asked: the window work it queued, a redraw, the pointer's shape.
    fn respond(&mut self, r: Response, now: Instant) {
        if r.input {
            self.input(now);
        } else {
            if let Some(w) = &self.window
                && !self.st.titlebar.actions.is_empty()
                && crate::titlebar::apply(&mut self.st, w)
            {
                self.st.close_requested();
            }
            if r.redraw {
                self.redraw();
            }
        }
        if let (Some(icon), Some(w)) = (r.icon, &self.window) {
            w.set_cursor(icon);
        }
    }
}

fn blink_phase(epoch: Instant, now: Instant) -> bool {
    let Some(t) = now.checked_duration_since(epoch + BLINK_DELAY) else { return false };
    (t.as_millis() / BLINK_PERIOD.as_millis()).is_multiple_of(2)
}

fn fatal(el: &ActiveEventLoop, msg: String) {
    eprintln!("shrew: {msg}");
    el.exit();
}

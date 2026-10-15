//! The windowed app: winit's event loop driving a [`State`] and the GPU renderer.
//!
//! Nothing runs continuously. The loop sleeps in `ControlFlow::Wait` and a frame is drawn only after input, a wake
//! from a background thread (syntax ready, file list, fonts, update status, MCP job), or while an animation is
//! still moving. Timers (mapping timeout, drag autoscroll, cursor blink) use `WaitUntil`.

use crate::cli::Cli;
use crate::draw::{self, Ui};
use crate::ipc::{self, Request};
use crate::keys::{self, RawKey};
use crate::layout::{Layout, Scroll};
use crate::state::{State, Waker};
use shrew_core::{KeyPress, Mode, Mods, text};
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

/// Clicks closer together than this (and than [`DOUBLE_CLICK_PX`]) count as double/triple clicks.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
const DOUBLE_CLICK_PX: f32 = 5.0;
const AUTOSCROLL_TICK: Duration = Duration::from_millis(16);
const BLINK_DELAY: Duration = Duration::from_millis(500);
const BLINK_PERIOD: Duration = Duration::from_millis(530);
/// Lines per mouse-wheel notch.
const WHEEL_LINES: f32 = 3.0;

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drag {
    None,
    Text,
    Scrollbar,
    /// The sidebar's right edge (resizing it).
    Sidebar,
    Minimap,
    /// A slider in the Settings panel.
    Settings,
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
    pointer: (f32, f32),
    drag: Drag,
    clicks: Option<(Instant, (f32, f32), u8)>,
    autoscroll: Option<Instant>,
    /// When the oldest input not yet on screen arrived.
    input_at: Option<Instant>,
    last_key: Instant,
    /// Input activity restarts the cursor blink.
    blink_epoch: Instant,
    preedit: bool,
    ime_allowed: bool,
    ime_rect: Option<Rect>,
    title: String,
    icon: CursorIcon,
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
        pointer: (0.0, 0.0),
        drag: Drag::None,
        clicks: None,
        autoscroll: None,
        input_at: Some(started),
        last_key: now,
        blink_epoch: now,
        preedit: false,
        ime_allowed: false,
        ime_rect: None,
        title: String::new(),
        icon: CursorIcon::Default,
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
        self.icon = CursorIcon::Text;
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
        if self.autoscroll.is_some_and(|t| now >= t) {
            self.autoscroll_tick(now);
        }
        if self.st.cursor_blink && self.ui.focused {
            self.redraw();
        }
        // ----- git: the markers' debounce, the inline blame's pause -----
        #[cfg(feature = "git")]
        if crate::git::tick(&mut self.st, now) {
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
        if let Some(t) = self.autoscroll {
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
                    self.st.titlebar.menu = None; // menus close when the window loses the focus
                }
                #[cfg(feature = "terminal")]
                self.st.terminal_window_focus(f);
                // ----- git: repositories are only watched while focused -----
                #[cfg(feature = "git")]
                self.st.git.set_focused(f);
                self.blink_epoch = Instant::now();
                self.redraw();
            }
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
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
            WindowEvent::CursorMoved { position, .. } => self.pointer_moved(position.x as f32, position.y as f32),
            WindowEvent::CursorLeft { .. } => {
                // both run: each clears its own hover state
                let bar = crate::titlebar::pointer_left(&mut self.st);
                let map = self.lay.is_some_and(|lay| crate::minimap::hover(&mut self.ui, &lay, None));
                if bar | map {
                    self.redraw();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => self.mouse_button(state, button),
            WindowEvent::MouseWheel { delta, .. } => self.wheel(delta),
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

    fn mods(&self) -> Mods {
        keys::mods_of(self.mods)
    }

    /// The char position under the pointer. `round`: the nearest caret position; else the char under it.
    fn pos_at(&self, x: f32, y: f32, round: bool) -> Option<usize> {
        let lay = self.lay.as_ref()?;
        let scroll = self.ui.anim.scroll;
        let b = self.st.ed.buf();
        let rope = &b.doc.rope;
        let last = text::last_line(rope);
        let line = scroll.line_at(lay, y).clamp(0, last as isize) as usize;
        let col = scroll.col_at(lay, x).max(0.0);
        let col = if round { col.round() } else { col.floor() } as usize;
        Some(text::pos_at_col(rope, line, col, self.st.ed.settings.tab_width.max(1)))
    }

    fn mouse_button(&mut self, state: ElementState, button: MouseButton) {
        let (x, y) = self.pointer;
        let now = Instant::now();
        if self.st.ed.mapping_pending() {
            self.st.ed.flush_pending_mappings();
        }
        #[cfg(feature = "terminal")]
        if self.terminal_button(state, button, x, y, now) {
            return;
        }
        match (state, button) {
            (ElementState::Pressed, MouseButton::Left) => self.left_press(x, y, now),
            (ElementState::Pressed, MouseButton::Right) => {
                if crate::titlebar::right_press(&mut self.st, &self.ui.hits, x, y) {
                    self.input(now);
                }
            }
            (ElementState::Released, MouseButton::Left) => {
                if self.drag == Drag::Settings {
                    self.st.settings_release();
                }
                if self.drag == Drag::Text {
                    self.st.ed.mouse_up();
                    self.st.after_input();
                }
                // a title bar button acts when released over it
                crate::titlebar::release(&mut self.st, &self.ui.hits, x, y);
                if self.drag == Drag::Sidebar {
                    self.st.persist_sidebar_width();
                }
                self.drag = Drag::None;
                self.autoscroll = None;
                self.input(now);
            }
            _ => {}
        }
    }

    /// The terminal panel takes presses over it; a click elsewhere gives the editor the focus back and goes on to
    /// it. True when the panel took the event.
    #[cfg(feature = "terminal")]
    fn terminal_button(&mut self, state: ElementState, button: MouseButton, x: f32, y: f32, now: Instant) -> bool {
        let button = match button {
            MouseButton::Left => shrew_core::MouseButton::Left,
            MouseButton::Middle => shrew_core::MouseButton::Middle,
            MouseButton::Right => shrew_core::MouseButton::Right,
            _ => return false,
        };
        let mods = self.mods();
        if state == ElementState::Released {
            self.st.terminal_release(button, mods);
            return false;
        }
        let modal = self.st.ed.picker.is_some()
            || self.st.ed.about.is_some()
            || self.st.settings.is_open()
            || self.st.titlebar.menu.is_some();
        if modal || !self.st.terminal_press(x, y, button, mods) {
            return false;
        }
        self.input(now);
        true
    }

    fn left_press(&mut self, x: f32, y: f32, now: Instant) {
        let Some(lay) = self.lay else { return };
        // the title bar, its menus and the window's resize borders come first
        if crate::titlebar::press(&mut self.st, &self.ui.hits, &lay, self.window.as_deref(), x, y, now) {
            return self.input(now);
        }
        let hits = &self.ui.hits;
        // the About panel is modal too: a click on a link opens it, a click outside closes the panel
        if self.st.ed.about.is_some() {
            if let Some((_, url)) = hits.about_links.iter().find(|(r, _)| r.contains(x, y)) {
                if let Err(e) = crate::open::url(url) {
                    self.st.ed.error(format!("cannot open {url}: {e}"));
                }
            } else if !hits.about.is_some_and(|r| r.contains(x, y)) {
                self.st.key(KeyPress::plain(shrew_core::Key::Esc));
            }
            return self.input(now);
        }
        // the picker is modal: a click picks a row or closes it
        if self.st.ed.picker.is_some() {
            let row = hits.picker_rows.iter().find(|(r, _)| r.contains(x, y)).map(|h| h.1);
            let inside = hits.picker.is_some_and(|r| r.contains(x, y));
            if let Some(row) = row {
                if let Some(p) = &mut self.st.ed.picker {
                    p.selected = row;
                }
                self.st.key(KeyPress::plain(shrew_core::Key::Enter));
            } else if !inside {
                self.st.key(KeyPress::plain(shrew_core::Key::Esc));
            }
            return self.input(now);
        }
        // the Settings panel is modal: a click outside closes it
        if self.st.settings.is_open() {
            if self.st.settings_click(x, y) {
                self.drag = Drag::Settings;
            }
            self.st.after_input();
            return self.input(now);
        }
        if hits.gear.is_some_and(|r| r.contains(x, y)) {
            self.st.ed.run(shrew_core::Command::Settings);
            self.st.after_input();
            return self.input(now);
        }
        if hits.update.is_some_and(|r| r.contains(x, y)) {
            self.st.activate_update();
            return self.input(now);
        }
        if hits.sidebar.edge.is_some_and(|r| r.contains(x, y)) {
            self.drag = Drag::Sidebar;
            return self.input(now);
        }
        if crate::sidebar::press(&mut self.st, &self.ui.hits.sidebar, x, y) {
            return self.input(now);
        }
        if let Some(&(_, i)) = hits.tabs.iter().find(|(r, _)| r.contains(x, y)) {
            self.st.ed.switch_to(i);
            self.st.after_input();
            return self.input(now);
        }
        // ----- git: the hunk peek's buttons, gutter markers, the branch in the status bar -----
        #[cfg(feature = "git")]
        if crate::git::press(&mut self.st, &self.ui.hits.git, &lay, self.ui.anim.scroll, x, y) {
            return self.input(now);
        }
        if crate::minimap::press(&mut self.st, &mut self.ui, &lay, x, y) {
            self.drag = Drag::Minimap;
            return self.input(now);
        }
        if let Some((track, total)) = self.ui.hits.scrollbar
            && track.contains(x, y)
        {
            self.drag = Drag::Scrollbar;
            self.scroll_to_pointer(track, total, y);
            return self.input(now);
        }
        if !lay.text.contains(x, y) {
            return;
        }
        // a click in the text leaves the command line, as in vim
        if self.st.ed.prompt.is_some() {
            self.st.key(KeyPress::plain(shrew_core::Key::Esc));
        }
        // click counting: double = word, triple = line
        let n = match self.clicks {
            Some((t, (px, py), n))
                if now - t < DOUBLE_CLICK && (px - x).hypot(py - y) < DOUBLE_CLICK_PX * lay.scale =>
            {
                (n % 3) + 1
            }
            _ => 1,
        };
        self.clicks = Some((now, (x, y), n));
        let in_gutter = x < lay.text_x();
        let ed = &self.st.ed;
        let caret = ed.mode == Mode::Insert || !ed.vim();
        let (pos, n) = if in_gutter {
            // a click on a line number selects the line
            let line_x = lay.text_x();
            (self.pos_at(line_x, y, true), 3)
        } else {
            (self.pos_at(x, y, caret), n)
        };
        let Some(pos) = pos else { return };
        self.st.ed.sidebar_focus = false;
        self.st.ed.mouse_down(pos, n, self.mods());
        self.st.after_input();
        self.drag = Drag::Text;
        self.input(now);
    }

    fn pointer_moved(&mut self, x: f32, y: f32) {
        self.pointer = (x, y);
        let Some(lay) = self.lay else { return };
        match self.drag {
            Drag::Text => {
                if let Some(pos) = self.pos_at(x, y, true) {
                    self.st.ed.mouse_drag(pos);
                    self.redraw();
                }
                // past the top or bottom edge: keep scrolling while the button is held
                let outside = y < lay.text.y || y >= lay.text.bottom();
                self.autoscroll = if outside { Some(self.autoscroll.unwrap_or_else(Instant::now)) } else { None };
            }
            Drag::Scrollbar => {
                if let Some((track, total)) = self.ui.hits.scrollbar {
                    self.scroll_to_pointer(track, total, y);
                    self.redraw();
                }
            }
            Drag::Sidebar => {
                crate::sidebar::drag_to(&mut self.st, x, lay.scale);
                self.redraw();
            }
            Drag::Minimap => {
                if crate::minimap::drag(&mut self.st, &mut self.ui, &lay, y) {
                    self.redraw();
                }
            }
            Drag::Settings => {
                self.st.settings_drag(x);
                self.st.after_input();
                self.redraw();
            }
            Drag::None => {}
        }
        let (redraw, bar_icon) =
            crate::titlebar::pointer_moved(&mut self.st, &self.ui.hits, &lay, self.window.as_deref(), x, y);
        let map = crate::minimap::hover(&mut self.ui, &lay, Some((x, y)));
        if redraw || map {
            self.redraw();
        }
        let on_edge = self.st.ed.picker.is_none() && self.ui.hits.sidebar.edge.is_some_and(|r| r.contains(x, y));
        let on_link = self.st.ed.about.is_some() && self.ui.hits.about_links.iter().any(|(r, _)| r.contains(x, y));
        let icon = if let Some(icon) = bar_icon {
            icon
        } else if on_link {
            CursorIcon::Pointer
        } else if on_edge || self.drag == Drag::Sidebar {
            CursorIcon::ColResize
        } else if self.st.ed.picker.is_none()
            && self.st.ed.about.is_none()
            && !self.st.settings.is_open()
            && lay.text_rect().contains(x, y)
            && !self.ui.hits.scrollbar.is_some_and(|(t, _)| t.contains(x, y))
        {
            CursorIcon::Text
        } else {
            CursorIcon::Default
        };
        #[cfg(feature = "terminal")]
        let icon = match self.drag {
            Drag::None if self.st.ed.picker.is_none() && self.st.ed.about.is_none() => {
                let (over, redraw) = self.st.terminal_motion(x, y, self.mods());
                if redraw {
                    self.redraw();
                }
                over.unwrap_or(icon)
            }
            _ => icon,
        };
        if icon != self.icon
            && let Some(w) = &self.window
        {
            w.set_cursor(icon);
            self.icon = icon;
        }
    }

    fn autoscroll_tick(&mut self, now: Instant) {
        let Some(lay) = self.lay else { return };
        let (x, y) = self.pointer;
        let dist = if y < lay.text.y { y - lay.text.y } else { y - lay.text.bottom() + 1.0 };
        if self.drag != Drag::Text || dist.abs() < 0.5 {
            self.autoscroll = None;
            return;
        }
        // faster the further away the pointer is
        let lines = (dist / lay.ch).clamp(-10.0, 10.0);
        let lines = if lines < 0.0 { lines.floor() } else { lines.ceil() } as isize;
        self.st.ed.scroll(lines, 0);
        self.ui.frac = 0.0;
        let ty = y.clamp(lay.text.y, lay.text.bottom() - 1.0);
        let scroll = self.ui.target_scroll(&self.st.ed);
        self.ui.anim.set_scroll(scroll);
        if let Some(pos) = self.pos_at(x, ty, true) {
            self.st.ed.mouse_drag(pos);
        }
        self.autoscroll = Some(now + AUTOSCROLL_TICK);
        self.redraw();
    }

    fn scroll_to_pointer(&mut self, track: Rect, total: usize, y: f32) {
        let f = ((y - track.y) / track.h.max(1.0)).clamp(0.0, 1.0);
        // the thumb centres on the pointer
        let rows = self.lay.map_or(0, |l| l.rows) as f32;
        let target = (f * total as f32 - rows / 2.0).clamp(0.0, total.saturating_sub(1) as f32) as isize;
        let top = self.st.ed.buf().view.top as isize;
        self.st.ed.scroll(target - top, 0);
        self.ui.frac = 0.0;
    }

    fn wheel(&mut self, delta: MouseScrollDelta) {
        let Some(lay) = self.lay else { return };
        if self.st.titlebar.menu.is_some() {
            return; // nothing scrolls under an open menu
        }
        if let Some(p) = &mut self.st.ed.picker {
            let dy = match delta {
                MouseScrollDelta::LineDelta(_, y) => -y,
                MouseScrollDelta::PixelDelta(d) => -(d.y as f32) / lay.ch,
            };
            if dy.abs() >= 0.5 {
                p.move_by(dy.round() as isize);
                self.redraw();
            }
            return;
        }
        if self.st.settings.is_open() {
            let dy = match delta {
                MouseScrollDelta::LineDelta(_, y) => -y * WHEEL_LINES * lay.ch,
                MouseScrollDelta::PixelDelta(d) => -(d.y as f32),
            };
            self.st.settings_wheel(dy);
            self.redraw();
            return;
        }
        if let Some(top) = &mut self.st.ed.about {
            let dy = match delta {
                MouseScrollDelta::LineDelta(_, y) => -y * WHEEL_LINES,
                MouseScrollDelta::PixelDelta(d) => -(d.y as f32) / lay.ch,
            };
            if dy.abs() >= 0.5 {
                *top = top.saturating_add_signed(dy.round() as isize);
                self.redraw();
            }
            return;
        }
        #[cfg(feature = "terminal")]
        {
            let lines = match delta {
                MouseScrollDelta::LineDelta(_, y) => y * WHEEL_LINES,
                MouseScrollDelta::PixelDelta(d) => d.y as f32 / lay.ch,
            };
            let (x, y) = self.pointer;
            if self.st.terminal_wheel(lines, x, y, self.mods()) {
                return self.redraw();
            }
        }
        let (px, py) = self.pointer;
        let lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => -y * WHEEL_LINES,
            MouseScrollDelta::PixelDelta(d) => -(d.y as f32) / lay.ch,
        };
        if crate::sidebar::wheel(&mut self.st, &self.ui.hits.sidebar, px, py, lines) {
            self.redraw();
            return;
        }
        // Ctrl+wheel zooms, like Ctrl+= / Ctrl+-
        if self.mods().ctrl {
            let dy = match delta {
                MouseScrollDelta::LineDelta(_, y) => y,
                MouseScrollDelta::PixelDelta(d) => d.y as f32 / lay.ch,
            };
            if dy.abs() >= 0.5 {
                let cmd = if dy > 0.0 { shrew_core::Command::FontBigger } else { shrew_core::Command::FontSmaller };
                self.st.ed.run(cmd);
                self.st.after_input();
                self.redraw();
            }
            return;
        }
        match delta {
            MouseScrollDelta::LineDelta(x, y) => {
                self.ui.frac = 0.0;
                self.st.ed.scroll((-y * WHEEL_LINES).round() as isize, (-x * WHEEL_LINES).round() as isize);
            }
            MouseScrollDelta::PixelDelta(d) => {
                // touchpads: follow the fingers pixel by pixel
                let v = self.st.ed.buf().view;
                let cur = v.top as f32 + if self.ui.frac_top == v.top { self.ui.frac } else { 0.0 };
                let last = text::last_line(self.st.ed.buf().rope()) as f32;
                let to = (cur - d.y as f32 / lay.ch).clamp(0.0, last);
                let top = to.floor();
                self.st.ed.scroll(top as isize - v.top as isize, 0);
                self.ui.frac = to - top;
                self.ui.frac_top = self.st.ed.buf().view.top;
                let cols = (-d.x as f32 / lay.cw).round() as isize;
                if cols != 0 {
                    self.st.ed.scroll(0, cols);
                }
                let s = self.ui.target_scroll(&self.st.ed);
                self.ui.anim.set_scroll(Scroll { top: s.top, left: self.ui.anim.scroll.left });
            }
        }
        self.redraw();
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

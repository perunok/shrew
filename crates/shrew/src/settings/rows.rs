//! The Settings panel's table. Every row is a descriptor: its section, its config key, a label, a description and a
//! [`Kind`] that carries the getter and setter. Adding a setting is one entry in [`build`] (the Layout & window
//! section is there for the sidebar, minimap, title bar and terminal to fill).
//!
//! Setters go through the same paths as `:set`, the pickers and `:features` (editor events, the toggles' events,
//! [`shrew_config::persist_setting`]), so config.toml stays the one source of truth and keeps its comments.

use super::{save_with, set_feature};
use crate::state::State;
use shrew_config::Setting;
use shrew_core::Event;
use shrew_core::features::{self, Feature};
use shrew_core::keymap::{MapMode, Rhs, keys_to_string};
use shrew_core::picker::Choice;

pub type Get<T> = Box<dyn Fn(&State) -> T>;
pub type Set<T> = Box<dyn Fn(&mut State, T)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Appearance,
    Editor,
    Features,
    Layout,
    Keybindings,
    Import,
    Ai,
    Updates,
    About,
}

impl Section {
    pub const ALL: [Section; 9] = [
        Section::Appearance,
        Section::Editor,
        Section::Features,
        Section::Layout,
        Section::Keybindings,
        Section::Import,
        Section::Ai,
        Section::Updates,
        Section::About,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::Appearance => "Appearance",
            Section::Editor => "Editor",
            Section::Features => "Features",
            Section::Layout => "Layout & window",
            Section::Keybindings => "Keybindings",
            Section::Import => "Import",
            Section::Ai => "AI assistants",
            Section::Updates => "Updates",
            Section::About => "About",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Section::Appearance => "Colors and type. The theme, accent and font lists preview each choice as you move.",
            Section::Editor => "How text is typed, indented and shown.",
            Section::Features => {
                "Every optional feature. One that is off is never started: no thread, no memory. n/a: not compiled \
                 into this build."
            }
            Section::Layout => "The parts of the window around the text.",
            Section::Keybindings => {
                "The chords that work right now and your own mappings (config.toml [keys], your vimrc, Sublime). \
                 Vim's keys are in docs/KEYBINDINGS.md."
            }
            Section::Import => "Settings and mappings read from other editors at startup.",
            Section::Ai => {
                "A local MCP server lets Claude Code, Claude Desktop or Cursor read and edit your open buffers. Every \
                 edit is one undo step and nothing is saved without you."
            }
            Section::Updates => "shrew checks GitHub Releases at most once a day; the download is verified before use.",
            Section::About => "Small, quick, yours.",
        }
    }

    /// Accepts the label or its first word, any case (`:settings ai`, `:settings layout`).
    pub fn find(name: &str) -> Option<Section> {
        let n = name.trim().to_lowercase();
        if n.is_empty() {
            return None;
        }
        Section::ALL.into_iter().find(|s| {
            let l = s.label().to_lowercase();
            l == n || l.split_whitespace().next() == Some(n.as_str()) || (n.len() >= 3 && l.starts_with(&n))
        })
    }
}

pub enum Kind {
    /// On or off. `get` answers `None` when the setting is not in this build (shown as n/a).
    Toggle { get: Get<Option<bool>>, set: Set<bool>, default: Option<bool> },
    /// A number in `range`, changed in `step`s (stepper, slider, `-`/`+`).
    Number {
        get: Get<f64>,
        set: Set<f64>,
        range: (f64, f64),
        step: f64,
        decimals: usize,
        unit: &'static str,
        default: f64,
    },
    /// A value chosen in one of the app's pickers, which preview as you move. `get`: the config value; `show`: what
    /// the row displays.
    Pick { choice: Choice, get: Get<String>, show: Get<String>, default: &'static str },
    /// A value from a short list that opens inside the panel: `options` gives (value, label).
    Options {
        options: Get<Vec<(String, String)>>,
        get: Get<String>,
        show: Get<String>,
        set: Set<String>,
        default: String,
    },
    /// A button. `confirm`: asked first (Enter twice).
    Action { button: &'static str, run: Box<dyn Fn(&mut State)>, confirm: Option<&'static str> },
    /// Read-only text.
    Info { get: Get<String> },
    /// A title between groups of rows.
    Heading,
}

pub struct Row {
    pub section: Section,
    /// Where it lives in config.toml (`editor.tab_width`), or a short id for rows that are not config.
    pub key: String,
    pub label: String,
    pub description: String,
    pub kind: Kind,
}

impl Row {
    fn new(section: Section, key: &str, label: &str, description: &str, kind: Kind) -> Row {
        Row { section, key: key.into(), label: label.into(), description: description.into(), kind }
    }

    /// Whether the row can be changed (not read-only, not n/a).
    pub fn editable(&self, st: &State) -> bool {
        match &self.kind {
            Kind::Toggle { get, .. } => get(st).is_some(),
            Kind::Info { .. } | Kind::Heading => false,
            _ => true,
        }
    }

    /// Puts the default back. False when the row has no default or already shows it.
    pub fn reset(&self, st: &mut State) -> bool {
        match &self.kind {
            Kind::Toggle { get, set, default: Some(d) } => {
                let now = get(st);
                if now.is_none() || now == Some(*d) {
                    return false;
                }
                set(st, *d);
            }
            Kind::Number { get, set, default, .. } => {
                if get(st) == *default {
                    return false;
                }
                set(st, *default);
            }
            Kind::Pick { choice, get, default, .. } => {
                if get(st) == *default {
                    return false;
                }
                st.ed.events.push(Event::Choose(*choice, default.to_string()));
            }
            Kind::Options { get, set, default, .. } => {
                if get(st) == *default {
                    return false;
                }
                set(st, default.clone());
            }
            _ => return false,
        }
        st.process_events();
        true
    }

    /// Whether every word of `query` is in the label, description, key or section name.
    pub fn matches(&self, query: &str) -> bool {
        let hay = format!("{} {} {} {}", self.label, self.description, self.key, self.section.label()).to_lowercase();
        query.to_lowercase().split_whitespace().all(|w| hay.contains(w))
    }
}

// ----- row constructors: one line per setting -----

/// A feature from features.rs as an on/off row (n/a when compiled out).
pub fn feature(section: Section, f: Feature, label: &str) -> Row {
    let info = features::info(f);
    let get: Get<Option<bool>> = Box::new(move |st| st.ed.features.available(f).then(|| st.ed.features.on(f)));
    let kind = Kind::Toggle { get, set: Box::new(move |st, on| set_feature(st, f, on)), default: Some(info.default) };
    let label = if label.is_empty() { info.key } else { label };
    Row::new(section, &format!("features.{}", info.key), label, info.description, kind)
}

pub fn toggle(
    section: Section,
    key: &str,
    label: &str,
    description: &str,
    default: bool,
    get: impl Fn(&State) -> bool + 'static,
    set: impl Fn(&mut State, bool) + 'static,
) -> Row {
    let kind = Kind::Toggle { get: Box::new(move |st| Some(get(st))), set: Box::new(set), default: Some(default) };
    Row::new(section, key, label, description, kind)
}

#[allow(clippy::too_many_arguments)]
pub fn number(
    section: Section,
    key: &str,
    label: &str,
    description: &str,
    (lo, hi, step): (f64, f64, f64),
    default: f64,
    get: impl Fn(&State) -> f64 + 'static,
    set: impl Fn(&mut State, f64) + 'static,
) -> Row {
    let decimals = if step.fract() == 0.0 {
        0
    } else if (step * 10.0).fract() == 0.0 {
        1
    } else {
        2
    };
    let kind =
        Kind::Number { get: Box::new(get), set: Box::new(set), range: (lo, hi), step, decimals, unit: "", default };
    Row::new(section, key, label, description, kind)
}

pub fn action(
    section: Section,
    key: &str,
    label: &str,
    description: &str,
    button: &'static str,
    run: impl Fn(&mut State) + 'static,
) -> Row {
    Row::new(section, key, label, description, Kind::Action { button, run: Box::new(run), confirm: None })
}

pub fn info(section: Section, label: &str, description: &str, get: impl Fn(&State) -> String + 'static) -> Row {
    Row::new(section, "", label, description, Kind::Info { get: Box::new(get) })
}

/// A fixed line of text.
pub fn text(section: Section, label: &str, description: &str, value: &str) -> Row {
    let v = value.to_string();
    info(section, label, description, move |_| v.clone())
}

pub fn heading(section: Section, label: &str) -> Row {
    Row::new(section, "", label, "", Kind::Heading)
}

fn with_unit(mut r: Row, u: &'static str) -> Row {
    if let Kind::Number { unit, .. } = &mut r.kind {
        *unit = u;
    }
    r
}

fn confirm(mut r: Row, text: &'static str) -> Row {
    if let Kind::Action { confirm, .. } = &mut r.kind {
        *confirm = Some(text);
    }
    r
}

// ----- the table -----

/// Every row, in order. Built when the panel opens (and again after an action that changes the lists), so a closed
/// panel costs nothing.
pub fn build(st: &State) -> Vec<Row> {
    let mut rows = Vec::with_capacity(96);
    appearance(&mut rows);
    editor(&mut rows);
    for i in features::FEATURES {
        rows.push(feature(Section::Features, i.feature, ""));
    }
    layout(&mut rows);
    keybindings(&mut rows, st);
    import(&mut rows, st);
    ai(&mut rows);
    updates(&mut rows);
    about(&mut rows);
    rows
}

fn appearance(rows: &mut Vec<Row>) {
    use Section::Appearance as S;
    let pick = |key: &str, label: &str, desc: &str, choice, default, get: Get<String>, show: Get<String>| {
        Row::new(S, key, label, desc, Kind::Pick { choice, get, show, default })
    };
    rows.push(pick(
        "theme.name",
        "Theme",
        "Colors of the text and the chrome. The list previews each theme as you move; Esc puts the old one back.",
        Choice::Theme,
        shrew_config::DEFAULT_THEME,
        Box::new(|st| st.look.cfg.name.clone()),
        Box::new(|st| {
            if st.look.cfg.name == shrew_theme::SYSTEM {
                format!("System ({})", st.look.theme.name)
            } else {
                st.look.theme.name.clone()
            }
        }),
    ));
    rows.push(pick(
        "theme.accent",
        "Accent",
        "The color of the cursor, badges and highlights, laid over any theme.",
        Choice::Accent,
        shrew_config::DEFAULT_ACCENT,
        Box::new(|st| st.look.cfg.accent.clone()),
        Box::new(|st| match shrew_theme::accent(&st.look.cfg.accent) {
            Some(a) => a.label.to_string(),
            None => "Theme default".into(),
        }),
    ));
    rows.push(toggle(
        S,
        "theme.name",
        "Follow the desktop",
        "Switch between a light and a dark theme with the system's light/dark setting.",
        false,
        |st| st.look.cfg.name == shrew_theme::SYSTEM,
        |st, on| {
            let v = if on { shrew_theme::SYSTEM.to_string() } else { st.look.theme.id.clone() };
            st.ed.events.push(Event::Choose(Choice::Theme, v));
        },
    ));
    for (dark, key, label, desc) in [
        (false, "theme.system_light", "Light desktop theme", "The theme used while the desktop is light."),
        (true, "theme.system_dark", "Dark desktop theme", "The theme used while the desktop is dark."),
    ] {
        let kind = Kind::Options {
            options: Box::new(move |st| theme_options(st, dark)),
            get: Box::new(move |st| system_theme(st, dark).to_string()),
            show: Box::new(move |st| theme_name(system_theme(st, dark))),
            set: Box::new(move |st, v| set_system_theme(st, dark, v)),
            default: if dark { shrew_config::DEFAULT_THEME } else { shrew_config::DEFAULT_LIGHT_THEME }.into(),
        };
        rows.push(Row::new(S, key, label, desc, kind));
    }
    rows.push(pick(
        "font.family",
        "Font",
        "Any installed monospace family; the list previews each one.",
        Choice::Font,
        shrew_config::DEFAULT_FONT_FAMILY,
        Box::new(font_value),
        Box::new(|st| match font_value(st).as_str() {
            shrew_config::DEFAULT_FONT_FAMILY => format!("{} (built in)", shrew_render::DEFAULT_FAMILY),
            f => f.to_string(),
        }),
    ));
    rows.push(with_unit(
        number(
            S,
            "font.size",
            "Font size",
            "In points. Ctrl+= / Ctrl+- / Ctrl+0 and Ctrl+wheel change it too.",
            (crate::state::MIN_FONT_SIZE as f64, crate::state::MAX_FONT_SIZE as f64, 1.0),
            shrew_config::DEFAULT_FONT_SIZE as f64,
            |st| st.font.size as f64,
            |st, v| {
                st.font.size = v as f32;
                st.font_changed = true;
                save_with(st, |p| shrew_config::persist_setting(p, Setting::FontSize, v));
            },
        ),
        "pt",
    ));
    rows.push(with_unit(
        number(
            S,
            "font.line_height",
            "Line height",
            "As a multiple of the font size.",
            (1.0, 2.5, 0.05),
            shrew_config::DEFAULT_LINE_HEIGHT as f64,
            |st| (st.font.line_height as f64 * 100.0).round() / 100.0,
            |st, v| {
                st.font.line_height = v as f32;
                st.font_changed = true;
                save_with(st, |p| shrew_config::persist_setting(p, Setting::LineHeight, v));
            },
        ),
        "×",
    ));
    let weights: [(u16, &str); 5] =
        [(300, "Light"), (400, "Regular"), (500, "Medium"), (600, "Semibold"), (700, "Bold")];
    let weight_name = move |w: u16| weights.iter().find(|x| x.0 == w).map_or_else(|| w.to_string(), |x| x.1.into());
    rows.push(Row::new(
        S,
        "font.weight",
        "Font weight",
        "Of normal text, where the family has that weight.",
        Kind::Options {
            options: Box::new(move |_| weights.iter().map(|(w, n)| (w.to_string(), n.to_string())).collect()),
            get: Box::new(|st| st.font.weight.to_string()),
            show: Box::new(move |st| weight_name(st.font.weight)),
            set: Box::new(|st, v| {
                let Ok(w) = v.parse::<u16>() else { return };
                st.font.weight = w;
                st.font_changed = true;
                save_with(st, |p| shrew_config::persist_setting(p, Setting::FontWeight, w as i64));
            }),
            default: shrew_config::DEFAULT_FONT_WEIGHT.to_string(),
        },
    ));
    rows.push(toggle(
        S,
        "font.ligatures",
        "Ligatures",
        "Programming ligatures (->, !=, >=) in fonts that have them.",
        true,
        |st| st.font.ligatures,
        |st, on| st.ed.events.push(Event::Ligatures(Some(on))),
    ));
    rows.push(toggle(
        S,
        "editor.cursor_blink",
        "Blinking cursor",
        "Off, shrew draws nothing while you are not typing.",
        false,
        |st| st.cursor_blink,
        |st, on| {
            st.cursor_blink = on;
            save_with(st, |p| shrew_config::persist_setting(p, Setting::CursorBlink, on));
        },
    ));
}

/// The theme `theme = "system"` uses on a dark (or light) desktop.
fn system_theme(st: &State, dark: bool) -> &str {
    if dark { &st.look.cfg.system_dark } else { &st.look.cfg.system_light }
}

fn set_system_theme(st: &mut State, dark: bool, v: String) {
    let s = if dark { Setting::SystemDark } else { Setting::SystemLight };
    save_with(st, |p| shrew_config::persist_setting(p, s, v.as_str()));
    if dark {
        st.look.cfg.system_dark = v;
    } else {
        st.look.cfg.system_light = v;
    }
    // following the desktop: show the new choice right away
    if st.look.cfg.name == shrew_theme::SYSTEM {
        st.ed.events.push(Event::Choose(Choice::Theme, shrew_theme::SYSTEM.into()));
    }
}

/// A built-in theme's name, else the id (a custom theme or a scheme file).
fn theme_name(id: &str) -> String {
    shrew_theme::BUILTIN.iter().find(|b| b.id == id).map_or_else(|| id.to_string(), |b| b.name.to_string())
}

/// Themes of the right scheme first, then the others.
fn theme_options(st: &State, dark: bool) -> Vec<(String, String)> {
    let mut list = st.look.lib.list();
    let want = if dark { shrew_theme::Scheme::Dark } else { shrew_theme::Scheme::Light };
    list.sort_by_key(|t| t.scheme != Some(want));
    list.into_iter().map(|t| (t.id, t.name)).collect()
}

/// The font family as config.toml writes it ("monospace" for the built-in one).
fn font_value(st: &State) -> String {
    let f = &st.font.family;
    if f.is_empty() || f == shrew_render::DEFAULT_FAMILY { shrew_config::DEFAULT_FONT_FAMILY.into() } else { f.clone() }
}

fn editor(rows: &mut Vec<Row>) {
    use Section::Editor as S;
    rows.push(feature(S, Feature::Vim, "Vim mode"));
    rows.push(number(
        S,
        "editor.tab_width",
        "Tab width",
        "Columns per indent level, and how wide a tab is drawn (:set ts).",
        (1.0, 16.0, 1.0),
        4.0,
        |st| st.ed.settings.tab_width as f64,
        |st, v| {
            st.ed.settings.tab_width = v as usize;
            save_with(st, |p| shrew_config::persist_setting(p, Setting::TabWidth, v as i64));
        },
    ));
    rows.push(toggle(
        S,
        "editor.expand_tab",
        "Indent with spaces",
        "Off: indent with tabs (:set et / noet).",
        true,
        |st| st.ed.settings.expand_tab,
        |st, on| {
            st.ed.settings.expand_tab = on;
            save_with(st, |p| shrew_config::persist_setting(p, Setting::ExpandTab, on));
        },
    ));
    rows.push(number(
        S,
        "editor.scrolloff",
        "Scroll margin",
        "Lines kept visible above and below the cursor (:set so).",
        (0.0, 30.0, 1.0),
        4.0,
        |st| st.ed.settings.scrolloff as f64,
        |st, v| {
            st.ed.settings.scrolloff = v as usize;
            save_with(st, |p| shrew_config::persist_setting(p, Setting::Scrolloff, v as i64));
        },
    ));
    rows.push(feature(S, Feature::AutoPairs, "Auto pairs"));
    rows.push(feature(S, Feature::AutoIndent, "Auto indent"));
    rows.push(feature(S, Feature::LineNumbers, "Line numbers"));
    rows.push(feature(S, Feature::RelativeNumbers, "Relative line numbers"));
    rows.push(feature(S, Feature::CurrentLine, "Highlight the current line"));
    rows.push(feature(S, Feature::IndentGuides, "Indent guides"));
    rows.push(feature(S, Feature::Whitespace, "Show whitespace"));
    rows.push(feature(S, Feature::BracketMatch, "Matching bracket"));
    rows.push(feature(S, Feature::SearchHighlight, "Highlight search matches"));
    rows.push(feature(S, Feature::WordHighlight, "Highlight the word under the cursor"));
    rows.push(feature(S, Feature::SmoothScroll, "Smooth scrolling"));
    rows.push(feature(S, Feature::CursorAnimation, "Cursor animation"));
    let leaders = || {
        vec![
            ("\\".to_string(), "\\ (backslash)".to_string()),
            (",".into(), ", (comma)".into()),
            (" ".into(), "Space".into()),
        ]
    };
    rows.push(Row::new(
        S,
        "editor.leader",
        "Leader key",
        "What <leader> means in mappings defined from now on (the config's on its next reload).",
        Kind::Options {
            options: Box::new(move |_| leaders()),
            get: Box::new(leader),
            show: Box::new(move |st| {
                let l = leader(st);
                leaders().into_iter().find(|x| x.0 == l).map_or(l, |x| x.1)
            }),
            set: Box::new(|st, v| {
                if st.ed.keymap.set_leader(&v) {
                    save_with(st, |p| shrew_config::persist_setting(p, Setting::Leader, v.as_str()));
                }
            }),
            default: "\\".into(),
        },
    ));
    rows.push(with_unit(
        number(
            S,
            "editor.mapping_timeout_ms",
            "Mapping timeout",
            "How long to wait for the rest of a mapping such as jk before taking the keys as typed.",
            (0.0, 3000.0, 50.0),
            1000.0,
            |st| st.ed.keymap.timeout_ms as f64,
            |st, v| {
                st.ed.keymap.timeout_ms = v as u64;
                save_with(st, |p| shrew_config::persist_setting(p, Setting::MappingTimeout, v as i64));
            },
        ),
        "ms",
    ));
}

/// The leader as config.toml writes it.
fn leader(st: &State) -> String {
    let k = st.ed.keymap.leader;
    match k.key {
        shrew_core::Key::Char(c) => c.to_string(),
        _ => k.to_string(),
    }
}

/// The Layout & window section: the parts of the window, top to bottom (title bar, sidebar, minimap, terminal), each
/// with its switch and its sizes. Sizes apply at once and are saved like a drag of the part's edge.
fn layout(rows: &mut Vec<Row>) {
    use Section::Layout as S;
    rows.push(heading(S, "Title bar"));
    rows.push(feature(S, Feature::CustomTitlebar, "Custom title bar"));
    rows.push(info(S, "Items", "[titlebar] left / right / hidden in config.toml: menu, sidebar, project, tabs, files, terminal, settings, about and the window buttons.", |st| {
        let l = &st.titlebar.layout;
        format!("{} left, {} right, {} hidden", l.left.len(), l.right.len(), l.hidden.len())
    }));
    rows.push(action(
        S,
        "titlebar.edit",
        "Arrange the items",
        "Opens config.toml at [titlebar]; saving it applies the new order.",
        "Edit",
        |st| super::open_config_at(st, "[titlebar]"),
    ));

    rows.push(heading(S, "Sidebar"));
    rows.push(feature(S, Feature::Sidebar, "Sidebar"));
    rows.push(with_unit(
        number(
            S,
            "sidebar.width",
            "Sidebar width",
            "Width of the file tree (dragging its edge sets it too).",
            (crate::sidebar::MIN_WIDTH as f64, 600.0, 10.0),
            crate::sidebar::DEFAULT_WIDTH as f64,
            |st| st.sidebar_width as f64,
            |st, v| {
                st.sidebar_width = v as f32;
                st.persist_sidebar_width();
            },
        ),
        "px",
    ));

    rows.push(heading(S, "Minimap"));
    rows.push(feature(S, Feature::Minimap, "Minimap"));
    rows.push(with_unit(
        number(
            S,
            "minimap.width",
            "Minimap width",
            "Width of the minimap; it steps aside when the text would keep fewer than 50 columns.",
            (40.0, 400.0, 10.0),
            shrew_config::DEFAULT_MINIMAP_WIDTH as f64,
            |st| st.minimap_width as f64,
            |st, v| {
                st.minimap_width = v as f32;
                save_with(st, |p| shrew_config::persist_value(p, "minimap", "width", v as i64));
            },
        ),
        "px",
    ));

    rows.push(heading(S, "Terminal"));
    rows.push(feature(S, Feature::Terminal, "Terminal panel"));
    #[cfg(feature = "terminal")]
    terminal(rows);
}

/// The terminal panel's settings (`[terminal]`); they apply to the open terminals where they can.
#[cfg(feature = "terminal")]
fn terminal(rows: &mut Vec<Row>) {
    use Section::Layout as S;
    rows.push(with_unit(
        number(
            S,
            "terminal.height",
            "Panel height",
            "Rows of the terminal panel (dragging its top edge sets it too).",
            (2.0, 100.0, 1.0),
            shrew_config::DEFAULT_TERMINAL_HEIGHT as f64,
            |st| st.terminals.rows as f64,
            |st, v| {
                st.terminals.rows = v as usize;
                st.terminals.cfg.height = Some(v as usize);
                save_with(st, |p| shrew_config::persist_value(p, "terminal", "height", v as i64));
            },
        ),
        "rows",
    ));
    rows.push(with_unit(
        number(
            S,
            "terminal.scrollback",
            "Scrollback",
            "Lines of history kept per terminal (terminals opened from now on).",
            (1000.0, 100_000.0, 1000.0),
            shrew_config::DEFAULT_SCROLLBACK as f64,
            |st| st.terminals.cfg.scrollback() as f64,
            |st, v| {
                st.terminals.cfg.scrollback = Some(v as usize);
                save_with(st, |p| shrew_config::persist_value(p, "terminal", "scrollback", v as i64));
            },
        ),
        "lines",
    ));
    rows.push(toggle(
        S,
        "terminal.copy_on_select",
        "Copy on select",
        "Copy a mouse selection in the terminal as soon as the button goes up.",
        true,
        |st| st.terminals.cfg.copy_on_select(),
        |st, on| {
            st.terminals.cfg.copy_on_select = Some(on);
            save_with(st, |p| shrew_config::persist_value(p, "terminal", "copy_on_select", on));
        },
    ));
    rows.push(info(S, "Shell", "[terminal] shell in config.toml; empty: $SHELL.", |st| {
        st.terminals.cfg.shell.clone().unwrap_or_else(|| std::env::var("SHELL").unwrap_or_else(|_| "default".into()))
    }));
}

fn keybindings(rows: &mut Vec<Row>, st: &State) {
    use Section::Keybindings as S;
    rows.push(action(
        S,
        "keys",
        "Edit key mappings",
        "Add or change mappings under [keys.normal], [keys.insert] and [keys.visual]; they apply when the file is saved.",
        "Edit in config.toml",
        |st| super::open_config_at(st, "[keys"),
    ));
    let km = &st.ed.keymap;
    rows.push(heading(S, "Your mappings"));
    let mut mine = 0;
    for mode in MapMode::ALL {
        for m in km.mappings(mode) {
            let rhs = match &m.rhs {
                Rhs::Keys { keys, .. } if keys.is_empty() => "nothing (<Nop>)".to_string(),
                Rhs::Keys { keys, .. } => keys_to_string(keys),
                Rhs::Command(c) => c.title().to_string(),
                Rhs::Ex(ex) => format!(":{ex}"),
            };
            rows.push(text(S, &format!("{:<8} {}", mode.name(), keys_to_string(&m.lhs)), "", &rhs));
            mine += 1;
        }
    }
    if mine == 0 {
        rows.push(text(S, "None yet", "", ""));
    }
    let mode = if st.ed.vim() { st.ed.mode.label().to_lowercase() } else { "Sublime mode".into() };
    rows.push(heading(S, &format!("Built-in chords ({mode})")));
    for (k, c) in st.ed.chords() {
        let taken = [MapMode::Normal, MapMode::Insert].iter().any(|&m| km.get(m, &[k]).is_some());
        let label = super::pretty_key(k);
        let desc = if taken { "overridden by your mapping" } else { "" };
        rows.push(text(S, &label, desc, c.title()));
    }
}

fn import(rows: &mut Vec<Row>, st: &State) {
    use Section::Import as S;
    rows.push(feature(S, Feature::Vimrc, "Read my vimrc"));
    rows.push(feature(S, Feature::SublimeImport, "Read Sublime Text's settings"));
    for (what, name) in [("vimrc", "vimrc"), ("sublime", "Sublime Text")] {
        match st.imports.iter().find(|r| r.what == what) {
            Some(r) => {
                let from = r.paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ");
                rows.push(text(S, &format!("{name}: found"), &from, &format!("{} applied", r.applied)));
                if r.unsupported > 0 {
                    rows.push(text(S, "  skipped", "Lines shrew does not understand.", &r.unsupported.to_string()));
                    for (line, t) in &r.samples {
                        let at = line.map_or_else(String::new, |l| format!("line {l}"));
                        rows.push(text(S, &format!("    {t}"), "", &at));
                    }
                }
            }
            None => rows.push(text(S, &format!("{name}: nothing read"), "Off, or no file in the usual places.", "")),
        }
    }
    rows.push(action(
        S,
        "import.reload",
        "Read them again",
        "Re-reads config.toml, your vimrc and Sublime Text's settings now.",
        "Reload",
        |st| st.reload_config(),
    ));
}

fn ai(rows: &mut Vec<Row>) {
    use Section::Ai as S;
    rows.push(feature(S, Feature::Mcp, "Let AI assistants work in shrew"));
    if !cfg!(feature = "mcp") {
        return;
    }
    rows.push(info(S, "Status", "", |st| {
        #[cfg(feature = "mcp")]
        if let Some(s) = &st.mcp {
            return format!("on, {}", s.url());
        }
        let _ = st;
        "off".into()
    }));
    rows.push(number(
        S,
        "mcp.port",
        "Port",
        "Localhost only. When it is taken, the next free one is used.",
        (1024.0, 65535.0, 1.0),
        shrew_config::DEFAULT_MCP_PORT as f64,
        |st| st.mcp_port.unwrap_or(shrew_config::DEFAULT_MCP_PORT) as f64,
        |st, v| {
            st.mcp_port = Some(v as u16);
            save_with(st, |p| shrew_config::persist_setting(p, Setting::McpPort, v as i64));
            if st.mcp_running() {
                st.set_mcp(false);
                st.set_mcp(true);
            }
        },
    ));
    rows.push(action(
        S,
        "mcp.claude_code",
        "Claude Code",
        "claude mcp add shrew -- srw --mcp-stdio, with this srw's path. Run it once in a terminal.",
        "Copy command",
        |st| super::copy(st, "Claude Code command", super::mcp_snippet(false)),
    ));
    rows.push(action(
        S,
        "mcp.json",
        "Claude Desktop, Cursor",
        "The mcpServers entry for the client's MCP config file; restart the client after adding it.",
        "Copy JSON",
        |st| super::copy(st, "MCP config entry", super::mcp_snippet(true)),
    ));
    rows.push(action(
        S,
        "mcp.guide",
        "Setup guide",
        "Every snippet, the config file locations and the direct HTTP variant, in a scratch buffer.",
        "Open",
        |st| {
            st.settings.close();
            st.ed.events.push(Event::McpSetup);
        },
    ));
    rows.push(confirm(
        action(
            S,
            "mcp.token",
            "New token",
            "Clients using the HTTP endpoint directly must be set up again; srw --mcp-stdio picks it up by itself.",
            "Regenerate",
            super::regenerate_token,
        ),
        "Enter again to make a new token: direct HTTP clients lose access",
    ));
}

fn updates(rows: &mut Vec<Row>) {
    use Section::Updates as S;
    rows.push(feature(S, Feature::UpdateCheck, "Check for updates automatically"));
    rows.push(text(S, "Version", "", concat!("shrew ", env!("CARGO_PKG_VERSION"))));
    if !cfg!(feature = "update") {
        return;
    }
    rows.push(info(S, "Status", "", |st| {
        #[cfg(feature = "update")]
        if let Some(u) = &st.updater {
            return u.status().label().unwrap_or_else(|| "not checked yet".into());
        }
        let _ = st;
        "not checked yet".into()
    }));
    rows.push(action(S, "update.check", "Check now", "Asks GitHub for the latest release.", "Check", |st| {
        st.ed.events.push(Event::CheckUpdates)
    }));
    rows.push(action(
        S,
        "update.install",
        "Install the update",
        "Downloads it, checks its sha256 and replaces this binary; Enter again restarts into it.",
        "Install",
        |st| st.ed.events.push(Event::InstallUpdate),
    ));
}

fn about(rows: &mut Vec<Row>) {
    use Section::About as S;
    rows.push(text(S, concat!("shrew ", env!("CARGO_PKG_VERSION")), crate::about::TAGLINE, ""));
    rows.push(action(
        S,
        "about",
        "About shrew",
        "The Shrew Manifesto in full, license, credits and links.",
        "Open",
        |st| {
            st.settings.close();
            st.ed.run(shrew_core::Command::About);
        },
    ));
    rows.push(info(S, "Config file", "Everything here is saved there, with your comments kept.", |st| {
        st.config_path.as_ref().map_or_else(|| "none (not saved)".into(), |p| p.display().to_string())
    }));
    rows.push(action(S, "config", "Edit config.toml", "Saving it applies it at once.", "Open", |st| {
        super::open_config_at(st, "")
    }));
    rows.push(heading(S, "The Shrew Manifesto"));
    for (i, (title, _)) in crate::about::MANIFESTO.iter().enumerate() {
        rows.push(text(S, &format!("{}. {title}", i + 1), "", ""));
    }
    rows.push(text(S, crate::about::SIGN_OFF, "", ""));
}

//! Optional features, extension-style: every one can be listed (`:features`, the palette) and switched off, either
//! at runtime (config `[features]`, `:set nosyntax`, the palette) or at compile time (cargo features, which drop
//! the code and its dependencies). A feature that is off is never initialised, so it costs nothing.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Feature {
    Vim,
    Syntax,
    BracketMatch,
    AutoPairs,
    AutoIndent,
    LineNumbers,
    RelativeNumbers,
    CurrentLine,
    IndentGuides,
    Whitespace,
    SearchHighlight,
    WordHighlight,
    SmoothScroll,
    CursorAnimation,
    Picker,
    Palette,
    Clipboard,
    Mcp,
    UpdateCheck,
    Vimrc,
    SublimeImport,
    // ----- phase 3: titlebar -----
    CustomTitlebar,
    // ----- project navigation -----
    Sidebar,
    ProjectSearch,
    Symbols,
    Recent,
}

pub struct FeatureInfo {
    pub feature: Feature,
    /// Name in the config file and in `:set <key>` / `:set no<key>`.
    pub key: &'static str,
    pub description: &'static str,
    pub default: bool,
}

const fn f(feature: Feature, key: &'static str, default: bool, description: &'static str) -> FeatureInfo {
    FeatureInfo { feature, key, description, default }
}

pub static FEATURES: &[FeatureInfo] = &[
    f(Feature::Vim, "vim", true, "Modal editing (normal/insert/visual). Off: always typing, Sublime-style"),
    f(Feature::Syntax, "syntax", true, "Syntax highlighting (Sublime grammars, incremental, in the background)"),
    f(Feature::BracketMatch, "bracket_match", true, "Highlight the bracket matching the one at the cursor"),
    f(Feature::AutoPairs, "auto_pairs", true, "Close brackets and quotes as you type them"),
    f(Feature::AutoIndent, "auto_indent", true, "Keep and extend indentation on new lines"),
    f(Feature::LineNumbers, "line_numbers", true, "Line number gutter"),
    f(
        Feature::RelativeNumbers,
        "relative_numbers",
        false,
        "Line numbers relative to the cursor (vim's relativenumber)",
    ),
    f(Feature::CurrentLine, "current_line", true, "Highlight the line the cursor is on"),
    f(Feature::IndentGuides, "indent_guides", true, "Thin vertical lines at each indent level"),
    f(Feature::Whitespace, "whitespace", false, "Show tabs and trailing spaces"),
    f(Feature::SearchHighlight, "search_highlight", true, "Highlight every match of the last search"),
    f(Feature::WordHighlight, "word_highlight", true, "Highlight other occurrences of the selected word"),
    f(Feature::SmoothScroll, "smooth_scroll", true, "Animated scrolling (redraws only while moving)"),
    f(Feature::CursorAnimation, "cursor_animation", true, "Cursor glides to its new position"),
    f(Feature::Picker, "picker", true, "Fuzzy file finder (Ctrl+P)"),
    f(Feature::Palette, "palette", true, "Command palette (Ctrl+Shift+P)"),
    f(Feature::Clipboard, "clipboard", true, "Share yanks and pastes with the system clipboard"),
    f(Feature::Mcp, "mcp", false, "MCP server so AI assistants can read and edit open files"),
    f(Feature::UpdateCheck, "update_check", true, "Check GitHub for new releases at startup"),
    f(Feature::Vimrc, "vimrc", true, "Read options and mappings from ~/.vimrc or init.vim at startup (see [import])"),
    f(
        Feature::SublimeImport,
        "sublime_import",
        true,
        "Read Sublime Text's user settings and key bindings at startup (see [import])",
    ),
    // ----- phase 3: titlebar -----
    f(
        Feature::CustomTitlebar,
        "custom_titlebar",
        true,
        "shrew's own title bar: tabs, menus and window buttons in one row. Off: the system title bar",
    ),
    // ----- project navigation -----
    f(Feature::Sidebar, "sidebar", false, "File tree of the project on the left (Ctrl+B, Ctrl+Shift+E)"),
    f(Feature::ProjectSearch, "project_search", true, "Find in project (Ctrl+Shift+F, :grep), respects .gitignore"),
    f(
        Feature::Symbols,
        "symbols",
        true,
        "Go to symbol in the file (Ctrl+R, @ in Ctrl+P) and the project (Ctrl+Shift+R)",
    ),
    f(Feature::Recent, "recent", true, "Remember recent files and projects (Open Recent, Ctrl+Alt+P) in the cache dir"),
];

pub fn info(feature: Feature) -> &'static FeatureInfo {
    FEATURES.iter().find(|i| i.feature == feature).expect("every feature is listed")
}

pub fn by_key(key: &str) -> Option<&'static FeatureInfo> {
    FEATURES.iter().find(|i| i.key == key || i.key.replace('_', "") == key)
}

#[inline]
fn bit(f: Feature) -> u64 {
    1 << (f as u32)
}

/// Which features are on, and which are compiled into this build at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Features {
    on: u64,
    available: u64,
}

impl Default for Features {
    fn default() -> Self {
        let on = FEATURES.iter().filter(|i| i.default).fold(0, |a, i| a | bit(i.feature));
        Features { on, available: u64::MAX }
    }
}

impl Features {
    #[inline]
    pub fn on(&self, f: Feature) -> bool {
        self.on & self.available & bit(f) != 0
    }
    pub fn set(&mut self, f: Feature, on: bool) {
        if on { self.on |= bit(f) } else { self.on &= !bit(f) }
    }
    pub fn toggle(&mut self, f: Feature) {
        self.on ^= bit(f);
    }
    /// Marks a feature as not compiled into this build: it then reads as off whatever the config says.
    pub fn set_unavailable(&mut self, f: Feature) {
        self.available &= !bit(f);
    }
    pub fn available(&self, f: Feature) -> bool {
        self.available & bit(f) != 0
    }
}

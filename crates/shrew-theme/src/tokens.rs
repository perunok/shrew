//! The tokens a theme sets: UI colors ([`Ui`]) and syntax roles ([`Role`] -> [`SyntaxStyle`]), plus their
//! "spec" forms where every token is optional. A theme file only writes some tokens; [`derive`](crate::Theme) fills
//! in the rest from the ones it has, so a custom theme that changes `bg` also gets a matching current line, gutter
//! and so on.

use crate::color::Rgb;

macro_rules! ui_tokens {
    ($($(#[$doc:meta])* $field:ident = $key:literal,)*) => {
        /// Every UI color, resolved. Keys in theme files are the kebab-case names (`surface-raised`); snake_case
        /// works too.
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct Ui {
            $($(#[$doc])* pub $field: Rgb,)*
        }

        /// UI colors as a theme file sets them: `None` = derived (or inherited from the base theme).
        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        pub struct UiSpec {
            $(pub $field: Option<Rgb>,)*
        }

        /// The UI token keys, in [`Ui`] field order.
        pub const UI_KEYS: &[&str] = &[$($key),*];

        impl UiSpec {
            pub fn slot(&mut self, key: &str) -> Option<&mut Option<Rgb>> {
                match key.replace('_', "-").as_str() {
                    $($key => Some(&mut self.$field),)*
                    _ => None,
                }
            }
            /// Takes every token `over` sets.
            pub fn overlay(&mut self, over: &UiSpec) {
                $(if over.$field.is_some() { self.$field = over.$field; })*
            }
            pub fn entries(&self) -> Vec<(&'static str, Option<Rgb>)> {
                vec![$(($key, self.$field)),*]
            }
        }

        impl Ui {
            pub fn get(&self, key: &str) -> Option<Rgb> {
                match key.replace('_', "-").as_str() {
                    $($key => Some(self.$field),)*
                    _ => None,
                }
            }
            pub fn entries(&self) -> Vec<(&'static str, Rgb)> {
                vec![$(($key, self.$field)),*]
            }
        }
    };
}

ui_tokens! {
    /// The text area.
    bg = "bg",
    /// Chrome: tab bar, status bar.
    surface = "surface",
    /// Popups: pickers, the command palette.
    surface_raised = "surface-raised",
    /// The selected row in a picker.
    surface_hover = "surface-hover",
    border = "border",
    border_strong = "border-strong",
    text = "text",
    /// Secondary text: paths, hints, the file type.
    text_muted = "text-muted",
    /// Hints that may be dimmer still (3:1).
    text_faint = "text-faint",
    accent = "accent",
    /// Text on an accent fill.
    accent_fg = "accent-fg",
    /// A tinted background for selected things.
    accent_soft = "accent-soft",
    /// Accent-colored text (fuzzy-match letters, links in the UI).
    accent_text = "accent-text",
    selection = "selection",
    /// Selection in an unfocused window.
    selection_inactive = "selection-inactive",
    cursor = "cursor",
    /// The character under a block cursor.
    cursor_fg = "cursor-fg",
    current_line = "current-line",
    /// Behind the line numbers.
    gutter = "gutter",
    line_number = "line-number",
    line_number_active = "line-number-active",
    indent_guide = "indent-guide",
    indent_guide_active = "indent-guide-active",
    /// Visible tabs and trailing spaces.
    whitespace = "whitespace",
    /// Behind the bracket matching the one at the cursor.
    bracket_match = "bracket-match",
    search_match = "search-match",
    /// The search match the cursor is on.
    search_current = "search-current",
    /// Other occurrences of the word under the cursor.
    word_highlight = "word-highlight",
    danger = "danger",
    /// Text on a `danger` fill.
    danger_fg = "danger-fg",
    danger_soft = "danger-soft",
    success = "success",
    success_fg = "success-fg",
    success_soft = "success-soft",
    warning = "warning",
    warning_soft = "warning-soft",
    /// Secondary accent for status items (cursor count, MCP).
    info = "info",
    mode_normal = "mode-normal",
    mode_normal_fg = "mode-normal-fg",
    mode_insert = "mode-insert",
    mode_insert_fg = "mode-insert-fg",
    mode_visual = "mode-visual",
    mode_visual_fg = "mode-visual-fg",
    scrollbar = "scrollbar",
    /// Picker and palette background and border.
    popup = "popup",
    popup_border = "popup-border",
    /// Drop shadow under popups (drawn translucent).
    shadow = "shadow",
}

/// Bold, italic, underline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Font {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl Font {
    pub const PLAIN: Font = Font { bold: false, italic: false, underline: false };
    pub const BOLD: Font = Font { bold: true, italic: false, underline: false };
    pub const ITALIC: Font = Font { bold: false, italic: true, underline: false };
    pub const UNDERLINE: Font = Font { bold: false, italic: false, underline: true };
}

/// A syntax role's color and, if the theme says so, its font style (`None`: the role's usual style, e.g. italic
/// comments).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SyntaxStyle {
    pub fg: Rgb,
    pub font: Option<Font>,
}

impl SyntaxStyle {
    pub fn new(fg: Rgb) -> SyntaxStyle {
        SyntaxStyle { fg, font: None }
    }
}

macro_rules! roles {
    ($($(#[$doc:meta])* $v:ident = $key:literal,)*) => {
        /// What a piece of code is, as far as coloring goes. [`SCOPE_RULES`](crate::SCOPE_RULES) maps Sublime/TextMate
        /// scope selectors onto these.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Role {
            $($(#[$doc])* $v,)*
        }

        impl Role {
            pub const ALL: &[Role] = &[$(Role::$v),*];
            pub const COUNT: usize = Role::ALL.len();
            pub fn key(self) -> &'static str {
                match self {
                    $(Role::$v => $key,)*
                }
            }
            pub fn from_key(key: &str) -> Option<Role> {
                match key.replace('_', "-").as_str() {
                    $($key => Some(Role::$v),)*
                    _ => None,
                }
            }
        }
    };
}

roles! {
    Keyword = "keyword",
    /// Control flow: `if`, `return`, `match`.
    Control = "control",
    String = "string",
    StringEscape = "string-escape",
    Number = "number",
    /// `true`, `None`, named constants.
    Constant = "constant",
    Comment = "comment",
    DocComment = "doc-comment",
    Function = "function",
    Method = "method",
    Type = "type",
    /// `u32`, `int`, `str`.
    BuiltinType = "builtin-type",
    /// Struct fields, object keys, config keys.
    Property = "property",
    Variable = "variable",
    Parameter = "parameter",
    Namespace = "namespace",
    Macro = "macro",
    /// Annotations and decorators (`#[derive]`, `@property`), HTML attribute names.
    Attribute = "attribute",
    Tag = "tag",
    Operator = "operator",
    Punctuation = "punctuation",
    Regex = "regex",
    /// Markdown headings.
    Heading = "heading",
    Link = "link",
    /// Markdown bold / italic.
    Emphasis = "emphasis",
    DiffAdded = "diff-added",
    DiffRemoved = "diff-removed",
    DiffChanged = "diff-changed",
    Invalid = "invalid",
}

/// Every syntax role, resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Syntax(pub [SyntaxStyle; Role::COUNT]);

impl std::ops::Index<Role> for Syntax {
    type Output = SyntaxStyle;
    fn index(&self, r: Role) -> &SyntaxStyle {
        &self.0[r as usize]
    }
}

/// Syntax roles as a theme file sets them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxSpec(pub [Option<SyntaxStyle>; Role::COUNT]);

impl Default for SyntaxSpec {
    fn default() -> Self {
        SyntaxSpec([None; Role::COUNT])
    }
}

impl SyntaxSpec {
    pub fn get(&self, r: Role) -> Option<SyntaxStyle> {
        self.0[r as usize]
    }
    pub fn set(&mut self, r: Role, s: SyntaxStyle) {
        self.0[r as usize] = Some(s);
    }
    pub fn overlay(&mut self, over: &SyntaxSpec) {
        for (a, b) in self.0.iter_mut().zip(over.0.iter()) {
            if b.is_some() {
                *a = *b;
            }
        }
    }
}

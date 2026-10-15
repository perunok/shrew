//! Which items the custom title bar shows, on which side, in which order (`[titlebar] left / right / hidden` in
//! config.toml), and where each one lands in the bar. The same model as Slinger's title bar layout: anything stored
//! is sanitised into a valid layout, and an item a config does not mention goes where the default puts it.

use shrew_render::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Item {
    Menu,
    Sidebar,
    Project,
    // ----- git: the branch -----
    Branch,
    Tabs,
    Files,
    Terminal,
    Settings,
    About,
    Minimize,
    Maximize,
    Close,
}

pub struct ItemInfo {
    pub item: Item,
    /// Its name in `[titlebar]`.
    pub key: &'static str,
    /// The command it runs, by name. Items whose command this build lacks (another feature's) are not drawn; `None`
    /// is built into the bar itself.
    pub command: Option<&'static str>,
}

const fn i(item: Item, key: &'static str, command: Option<&'static str>) -> ItemInfo {
    ItemInfo { item, key, command }
}

/// Every item, in the default order.
pub static ITEMS: &[ItemInfo] = &[
    i(Item::Menu, "menu", Some("menu")),
    i(Item::Sidebar, "sidebar", Some("toggle_sidebar")),
    // the recent projects picker when there is one, else the file picker (see `TitleBar::activate`)
    i(Item::Project, "project", None),
    // the branch of the project or file (git): opens the branch picker, when this build has one
    i(Item::Branch, "branch", None),
    i(Item::Tabs, "tabs", None),
    i(Item::Files, "files", Some("files")),
    i(Item::Terminal, "terminal", Some("toggle_terminal")),
    i(Item::Settings, "settings", Some("settings")),
    i(Item::About, "about", Some("about")),
    i(Item::Minimize, "minimize", Some("minimize")),
    i(Item::Maximize, "maximize", Some("maximize")),
    i(Item::Close, "close", None),
];

impl Item {
    pub fn info(self) -> &'static ItemInfo {
        ITEMS.iter().find(|i| i.item == self).expect("every item is listed")
    }

    pub fn from_key(key: &str) -> Option<Item> {
        ITEMS.iter().find(|i| i.key == key).map(|i| i.item)
    }

    /// Minimize, maximize and close: drawn by shrew only where it replaces the system's (not on macOS).
    pub fn is_window_button(self) -> bool {
        matches!(self, Item::Minimize | Item::Maximize | Item::Close)
    }

    /// Items that can move but never be hidden: the tabs, Settings (where the layout is changed back) and the
    /// window buttons (a window must stay closable).
    pub fn always_shown(self) -> bool {
        matches!(self, Item::Tabs | Item::Settings) || self.is_window_button()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Hidden,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarLayout {
    pub left: Vec<Item>,
    pub right: Vec<Item>,
    pub hidden: Vec<Item>,
}

impl Default for BarLayout {
    fn default() -> Self {
        use Item::*;
        BarLayout {
            left: vec![Menu, Sidebar, Project, Branch, Tabs],
            right: vec![Files, Terminal, Settings, About, Minimize, Maximize, Close],
            hidden: Vec::new(),
        }
    }
}

impl BarLayout {
    pub fn side_of(&self, item: Item) -> Side {
        if self.left.contains(&item) {
            Side::Left
        } else if self.right.contains(&item) {
            Side::Right
        } else {
            Side::Hidden
        }
    }

    fn side_mut(&mut self, side: Side) -> &mut Vec<Item> {
        match side {
            Side::Left => &mut self.left,
            Side::Right => &mut self.right,
            Side::Hidden => &mut self.hidden,
        }
    }

    /// A valid layout from the config's lists: unknown names (returned, for a warning) and repeats are dropped
    /// (the first place wins), an item none of the lists mentions goes where the default puts it, and an item
    /// that cannot be hidden moves from `hidden` to its default side.
    pub fn sanitize(cfg: &shrew_config::TitleBarConfig) -> (BarLayout, Vec<String>) {
        let mut unknown = Vec::new();
        let mut seen: Vec<Item> = Vec::new();
        let mut pick = |names: &Option<Vec<String>>| -> Vec<Item> {
            let mut out = Vec::new();
            for n in names.iter().flatten() {
                match Item::from_key(n.trim()) {
                    Some(item) if !seen.contains(&item) => {
                        seen.push(item);
                        out.push(item);
                    }
                    Some(_) => {}
                    None => unknown.push(n.clone()),
                }
            }
            out
        };
        let mut l = BarLayout { left: pick(&cfg.left), right: pick(&cfg.right), hidden: pick(&cfg.hidden) };
        let default = BarLayout::default();
        for info in ITEMS {
            if !seen.contains(&info.item) {
                l.side_mut(default.side_of(info.item)).push(info.item);
            }
        }
        for item in l.hidden.clone() {
            if item.always_shown() {
                l.hidden.retain(|&i| i != item);
                let side = match default.side_of(item) {
                    Side::Hidden => Side::Right,
                    s => s,
                };
                l.side_mut(side).push(item);
            }
        }
        (l, unknown)
    }
}

/// Where each shown item goes in `bar`. `width` gives every item but the tabs its width; the tabs take what is
/// left. Neighbouring window buttons touch, and one at the very edge sits in the window's corner (no padding), as
/// in a system title bar; other items are `gap` apart.
pub fn place(
    layout: &BarLayout,
    bar: Rect,
    shown: impl Fn(Item) -> bool,
    width: impl Fn(Item) -> f32,
    gap: f32,
) -> Vec<(Item, Rect)> {
    let left: Vec<Item> = layout.left.iter().copied().filter(|&i| shown(i)).collect();
    let right: Vec<Item> = layout.right.iter().copied().filter(|&i| shown(i)).collect();
    let space = |a: Item, b: Item| {
        if (a.is_window_button() && b.is_window_button()) || a == Item::Tabs || b == Item::Tabs { 0.0 } else { gap }
    };
    let edge = |i: Option<&Item>| if i.is_none_or(|i| i.is_window_button() || *i == Item::Tabs) { 0.0 } else { gap };
    let run = |items: &[Item]| -> f32 {
        let w: f32 = items.iter().filter(|&&i| i != Item::Tabs).map(|&i| width(i)).sum();
        w + items.windows(2).map(|p| space(p[0], p[1])).sum::<f32>()
    };
    let fixed = run(&left) + run(&right) + edge(left.first()) + edge(right.last());
    // the room between the two sides goes to the tabs, wherever they are
    let tabs_w = (bar.w - fixed).max(0.0).floor();
    let w = |i: Item| if i == Item::Tabs { tabs_w } else { width(i) };
    let mut out = Vec::with_capacity(left.len() + right.len());
    let mut x = bar.x + edge(left.first());
    for (n, &item) in left.iter().enumerate() {
        if n > 0 {
            x += space(left[n - 1], item);
        }
        out.push((item, Rect::new(x.round(), bar.y, w(item).round(), bar.h)));
        x += w(item);
    }
    let mut x = bar.right() - edge(right.last());
    for (n, &item) in right.iter().enumerate().rev() {
        if n + 1 < right.len() {
            x -= space(item, right[n + 1]);
        }
        x -= w(item);
        out.push((item, Rect::new(x.round(), bar.y, w(item).round(), bar.h)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use Item::*;

    fn cfg(left: Option<&[&str]>, right: Option<&[&str]>, hidden: Option<&[&str]>) -> shrew_config::TitleBarConfig {
        let v = |l: Option<&[&str]>| l.map(|l| l.iter().map(|s| s.to_string()).collect());
        shrew_config::TitleBarConfig { left: v(left), right: v(right), hidden: v(hidden) }
    }

    #[test]
    fn nothing_configured_is_the_default() {
        let (l, unknown) = BarLayout::sanitize(&shrew_config::TitleBarConfig::default());
        assert_eq!(l, BarLayout::default());
        assert!(unknown.is_empty());
        // every item has exactly one place in the default
        for i in ITEMS {
            let n = [&l.left, &l.right, &l.hidden].iter().filter(|s| s.contains(&i.item)).count();
            assert_eq!(n, 1, "{}", i.key);
        }
    }

    #[test]
    fn sanitize_drops_unknown_and_repeats_and_fills_in_the_rest() {
        let (l, unknown) = BarLayout::sanitize(&cfg(
            Some(&["close", "minimize", "maximize", "nope", "menu"]),
            Some(&["tabs", "menu", "about"]),
            Some(&["terminal", "close"]),
        ));
        assert_eq!(unknown, vec!["nope".to_string()]);
        // first place wins: menu stays left, close stays left
        assert_eq!(l.left, vec![Close, Minimize, Maximize, Menu, Sidebar, Project, Branch]);
        // items nobody mentioned go to their default side, in default order
        assert_eq!(l.right, vec![Tabs, About, Files, Settings]);
        assert_eq!(l.hidden, vec![Terminal]);
    }

    #[test]
    fn some_items_cannot_be_hidden() {
        let (l, _) = BarLayout::sanitize(&cfg(None, None, Some(&["settings", "tabs", "close", "about", "minimize"])));
        assert_eq!(l.hidden, vec![About]);
        assert!(l.left.contains(&Tabs));
        for i in [Settings, Close, Minimize] {
            assert_eq!(l.side_of(i), Side::Right, "{i:?}");
        }
        assert!(ITEMS.iter().all(|i| l.side_of(i.item) != Side::Hidden || !i.item.always_shown()));
    }

    #[test]
    fn placing_items_tabs_take_the_middle() {
        let bar = Rect::new(0.0, 0.0, 1000.0, 30.0);
        let p = place(&BarLayout::default(), bar, |_| true, |i| if i.is_window_button() { 40.0 } else { 30.0 }, 4.0);
        let at = |i: Item| p.iter().find(|x| x.0 == i).unwrap().1;
        assert_eq!(at(Menu).x, 4.0, "padded from the edge");
        assert_eq!(at(Sidebar).x, 38.0);
        assert_eq!(at(Close).right(), 1000.0, "the close button sits in the corner");
        assert_eq!(at(Maximize).right(), at(Close).x, "window buttons touch");
        assert_eq!(at(Minimize).right(), at(Maximize).x);
        assert_eq!(at(About).right() + 4.0, at(Minimize).x);
        // the tabs fill the gap between the sides exactly
        assert_eq!(at(Tabs).x, at(Branch).right());
        assert_eq!(at(Tabs).right(), at(Files).x);
        // no item overlaps another
        for (a, ra) in &p {
            for (b, rb) in &p {
                assert!(a == b || !ra.intersects(rb) || ra.w == 0.0 || rb.w == 0.0, "{a:?} {b:?}");
            }
        }
    }

    #[test]
    fn placing_hidden_or_unavailable_items_leaves_no_hole() {
        let bar = Rect::new(0.0, 0.0, 600.0, 30.0);
        let l = BarLayout { left: vec![Close, Maximize, Tabs], right: vec![Settings, Menu], hidden: vec![] };
        let p = place(&l, bar, |i| i != Settings, |_| 30.0, 4.0);
        let items: Vec<Item> = p.iter().map(|x| x.0).collect();
        assert_eq!(items, vec![Close, Maximize, Tabs, Menu]);
        assert_eq!(p[0].1.x, 0.0, "macOS order: window buttons in the left corner");
        assert_eq!(p[2].1.x, 60.0);
        assert_eq!(p[3].1.right(), 596.0);
        assert_eq!(p[2].1.right(), p[3].1.x);
        // too narrow: the tabs shrink to nothing, never negative
        let p = place(&l, Rect::new(0.0, 0.0, 50.0, 30.0), |_| true, |_| 30.0, 4.0);
        assert_eq!(p.iter().find(|x| x.0 == Tabs).unwrap().1.w, 0.0);
    }
}

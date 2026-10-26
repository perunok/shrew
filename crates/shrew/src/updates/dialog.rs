//! The Update shrew dialog's logic, apart from drawing: which stage it shows, its buttons, its keys, and the
//! progress wording. Pure, so the state machine is tested without a window.
//!
//! `Offer → Downloading → Verifying → Ready`, with `Failed` (Retry, View release) and `Unsupported` (View release)
//! beside them. Esc is the gentle way out of every stage: Hide while downloading (it goes on in the background),
//! Later once it is ready (the download is kept), Close otherwise.

use shrew_core::{Key, KeyPress};
use shrew_update::Phase;

/// What the dialog shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    /// No newer version known (opened after "up to date", or before any check).
    Nothing,
    /// This installation cannot replace itself; the reason.
    Unsupported(String),
    /// A newer version, not downloaded yet.
    Offer,
    Downloading {
        done: u64,
        total: u64,
        speed: u64,
    },
    Verifying,
    Ready,
    Failed(String),
}

impl Stage {
    pub fn of(phase: &Phase, available: bool, support: Result<(), &str>) -> Stage {
        match phase {
            Phase::Downloading { done, total, speed, .. } => {
                return Stage::Downloading { done: *done, total: *total, speed: *speed };
            }
            Phase::Verifying { .. } => return Stage::Verifying,
            Phase::Ready(_) => return Stage::Ready,
            Phase::Failed(e) => return Stage::Failed(e.clone()),
            Phase::Idle => {}
        }
        if !available {
            return Stage::Nothing;
        }
        match support {
            Err(why) => Stage::Unsupported(why.to_string()),
            Ok(()) => Stage::Offer,
        }
    }

    /// Left to right; the last one is the main one (Enter's default).
    pub fn buttons(&self) -> &'static [Button] {
        use Button::*;
        match self {
            Stage::Nothing => &[Close, CheckAgain],
            Stage::Unsupported(_) => &[Close, ViewRelease],
            Stage::Offer => &[Close, ViewRelease, Download],
            Stage::Downloading { .. } => &[Cancel, Hide],
            Stage::Verifying => &[Hide],
            Stage::Ready => &[Later, Restart],
            Stage::Failed(_) => &[Close, ViewRelease, Retry],
        }
    }

    /// What Esc (or a click outside) does.
    pub fn escape(&self) -> Button {
        match self {
            Stage::Downloading { .. } | Stage::Verifying => Button::Hide,
            Stage::Ready => Button::Later,
            _ => Button::Close,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Close,
    CheckAgain,
    ViewRelease,
    Download,
    Cancel,
    Hide,
    Later,
    Restart,
    Retry,
}

impl Button {
    pub fn label(self) -> &'static str {
        match self {
            Button::Close => "Close",
            Button::CheckAgain => "Check again",
            Button::ViewRelease => "View release",
            Button::Download => "Download",
            Button::Cancel => "Cancel",
            Button::Hide => "Hide",
            Button::Later => "Later",
            Button::Restart => "Restart to update",
            Button::Retry => "Retry",
        }
    }
}

/// The open dialog: where its notes are scrolled and which button has the keyboard.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dialog {
    pub scroll: usize,
    /// Index into the stage's buttons; `None`: the main (last) one.
    pub focus: Option<usize>,
    /// The stage last drawn, so the focus resets when the buttons change.
    pub stage: Option<Stage>,
}

/// What a key in the dialog does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Press(Button),
    /// Scroll the notes by this many lines.
    Scroll(isize),
    /// Moved the focus or nothing to do; the key is taken either way (the dialog is modal).
    None,
}

impl Dialog {
    /// Keeps the focus valid for `stage`'s buttons.
    pub fn sync(&mut self, stage: &Stage) {
        if self.stage.as_ref().is_none_or(|s| std::mem::discriminant(s) != std::mem::discriminant(stage)) {
            self.focus = None;
            self.stage = Some(stage.clone());
        }
    }

    pub fn focused(&self, stage: &Stage) -> usize {
        let n = stage.buttons().len();
        self.focus.filter(|&i| i < n).unwrap_or(n - 1)
    }

    pub fn key(&mut self, stage: &Stage, k: KeyPress, page: usize) -> Act {
        self.sync(stage);
        let buttons = stage.buttons();
        let n = buttons.len();
        let at = self.focused(stage);
        let m = k.mods;
        let page = page.max(1) as isize;
        match k.key {
            Key::Esc => return Act::Press(stage.escape()),
            Key::Char('c' | '[') if m.ctrl => return Act::Press(stage.escape()),
            Key::Enter => return Act::Press(buttons[at]),
            Key::Char(' ') if !m.ctrl => return Act::Press(buttons[at]),
            Key::Tab if m.shift => self.focus = Some((at + n - 1) % n),
            Key::Tab | Key::Right => self.focus = Some((at + 1) % n),
            Key::Left => self.focus = Some((at + n - 1) % n),
            Key::Down => return Act::Scroll(1),
            Key::Up => return Act::Scroll(-1),
            Key::PageDown => return Act::Scroll(page),
            Key::PageUp => return Act::Scroll(-page),
            Key::Home => return Act::Scroll(isize::MIN / 2),
            Key::End => return Act::Scroll(isize::MAX / 2),
            Key::Char('d') if m.ctrl => return Act::Scroll(page / 2),
            Key::Char('u') if m.ctrl => return Act::Scroll(-page / 2),
            _ => match k.text() {
                Some('j') => return Act::Scroll(1),
                Some('k') => return Act::Scroll(-1),
                Some('h') => self.focus = Some((at + n - 1) % n),
                Some('l') => self.focus = Some((at + 1) % n),
                Some('g') => return Act::Scroll(isize::MIN / 2),
                Some('G') => return Act::Scroll(isize::MAX / 2),
                Some('q') => return Act::Press(stage.escape()),
                _ => {}
            },
        }
        Act::None
    }
}

/// `1.2 MB`, `640 KB`, `12 B`.
pub fn bytes(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{} KB", n >> 10),
        n => format!("{n} B"),
    }
}

/// Whole percent of `done` in `total` (0 when unknown, at most 100).
pub fn percent(done: u64, total: u64) -> u8 {
    (done.min(total) * 100).checked_div(total).unwrap_or(0) as u8
}

/// "7 s" / "2 min" left at `speed` bytes per second, when it can be told.
pub fn time_left(done: u64, total: u64, speed: u64) -> Option<String> {
    if total == 0 || speed == 0 || done >= total {
        return None;
    }
    let secs = (total - done).div_ceil(speed);
    Some(if secs < 60 { format!("{secs} s") } else { format!("{} min", secs.div_ceil(60)) })
}

/// The two halves of the progress line: "5.2 MB of 22.3 MB (23%)" and "3.1 MB/s · 6 s left".
pub fn progress_text(done: u64, total: u64, speed: u64) -> (String, String) {
    let left = if total > 0 {
        format!("{} of {} ({}%)", bytes(done), bytes(total), percent(done, total))
    } else {
        format!("{} downloaded", bytes(done))
    };
    let right = match (speed, time_left(done, total, speed)) {
        (0, _) => String::new(),
        (s, Some(t)) => format!("{}/s · {t} left", bytes(s)),
        (s, None) => format!("{}/s", bytes(s)),
    };
    (left, right)
}

#[cfg(test)]
mod tests {
    use super::*;
    use shrew_update::{Release, Staged};

    fn staged() -> Phase {
        let release = Release {
            version: "0.3.0".into(),
            tag: "v0.3.0".into(),
            notes: String::new(),
            published: String::new(),
            asset_url: String::new(),
            checksum_url: String::new(),
            size: 0,
            page: String::new(),
        };
        Phase::Ready(Staged { path: "/tmp/x".into(), release, sha256: [0; 32] })
    }

    #[test]
    fn stages_follow_the_download() {
        let ok = Ok(());
        assert_eq!(Stage::of(&Phase::Idle, false, ok), Stage::Nothing);
        assert_eq!(Stage::of(&Phase::Idle, true, ok), Stage::Offer);
        assert_eq!(Stage::of(&Phase::Idle, true, Err("dev build")), Stage::Unsupported("dev build".into()));
        let dl = Phase::Downloading { version: "0.3.0".into(), done: 5, total: 10, speed: 2 };
        assert_eq!(Stage::of(&dl, true, ok), Stage::Downloading { done: 5, total: 10, speed: 2 });
        assert_eq!(Stage::of(&Phase::Verifying { version: "0.3.0".into() }, true, ok), Stage::Verifying);
        assert_eq!(Stage::of(&staged(), true, ok), Stage::Ready);
        assert_eq!(Stage::of(&Phase::Failed("boom".into()), true, ok), Stage::Failed("boom".into()));
    }

    #[test]
    fn buttons_and_escape() {
        use Button::*;
        assert_eq!(Stage::Offer.buttons(), [Close, ViewRelease, Download]);
        assert_eq!(Stage::Unsupported("x".into()).buttons(), [Close, ViewRelease]);
        assert_eq!(Stage::Downloading { done: 0, total: 0, speed: 0 }.buttons(), [Cancel, Hide]);
        assert_eq!(Stage::Ready.buttons(), [Later, Restart]);
        assert_eq!(Stage::Failed("x".into()).buttons(), [Close, ViewRelease, Retry]);
        assert_eq!(Stage::Downloading { done: 0, total: 0, speed: 0 }.escape(), Hide, "Esc hides, never cancels");
        assert_eq!(Stage::Verifying.escape(), Hide);
        assert_eq!(Stage::Ready.escape(), Later, "Esc keeps the download");
        assert_eq!(Stage::Offer.escape(), Close);
    }

    #[test]
    fn keys_move_the_focus_and_press() {
        let k = |s: &str| KeyPress::parse(s).unwrap();
        let mut d = Dialog::default();
        let offer = Stage::Offer;
        assert_eq!(d.key(&offer, k("<CR>"), 10), Act::Press(Button::Download), "Enter: the main button");
        assert_eq!(d.key(&offer, k("<Tab>"), 10), Act::None);
        assert_eq!(d.key(&offer, k("<CR>"), 10), Act::Press(Button::Close), "Tab wraps around");
        assert_eq!(d.key(&offer, k("<S-Tab>"), 10), Act::None);
        assert_eq!(d.key(&offer, k("<CR>"), 10), Act::Press(Button::Download));
        assert_eq!(d.key(&offer, k("<Left>"), 10), Act::None);
        assert_eq!(d.key(&offer, k("<Space>"), 10), Act::Press(Button::ViewRelease));
        assert_eq!(d.key(&offer, k("<Esc>"), 10), Act::Press(Button::Close));
        assert_eq!(d.key(&offer, k("j"), 10), Act::Scroll(1));
        assert_eq!(d.key(&offer, k("<PageUp>"), 10), Act::Scroll(-10));
        assert_eq!(d.key(&offer, k("x"), 10), Act::None, "modal: other keys do nothing");
        // the stage changes: the focus goes back to its main button
        let ready = Stage::Ready;
        assert_eq!(d.key(&ready, k("<CR>"), 10), Act::Press(Button::Restart));
        assert_eq!(d.key(&ready, k("<Esc>"), 10), Act::Press(Button::Later));
        let dl = Stage::Downloading { done: 1, total: 2, speed: 0 };
        assert_eq!(d.key(&dl, k("<CR>"), 10), Act::Press(Button::Hide));
        assert_eq!(d.key(&dl, k("<Left>"), 10), Act::None);
        assert_eq!(d.key(&dl, k("<CR>"), 10), Act::Press(Button::Cancel));
        // progress ticks keep the focus
        let dl2 = Stage::Downloading { done: 2, total: 2, speed: 9 };
        assert_eq!(d.key(&dl2, k("<CR>"), 10), Act::Press(Button::Cancel));
    }

    #[test]
    fn progress_wording() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(640 << 10), "640 KB");
        assert_eq!(bytes(22_347_552), "21.3 MB");
        assert_eq!(percent(5, 10), 50);
        assert_eq!(percent(15, 10), 100);
        assert_eq!(percent(5, 0), 0);
        assert_eq!(time_left(0, 100, 10).as_deref(), Some("10 s"));
        assert_eq!(time_left(0, 6000, 10).as_deref(), Some("10 min"));
        assert_eq!(time_left(0, 100, 0), None);
        assert_eq!(time_left(100, 100, 5), None);
        let (l, r) = progress_text(5 << 20, 20 << 20, 1 << 20);
        assert_eq!((l.as_str(), r.as_str()), ("5.0 MB of 20.0 MB (25%)", "1.0 MB/s · 15 s left"));
        assert_eq!(progress_text(3 << 10, 0, 0), ("3 KB downloaded".into(), String::new()));
    }
}

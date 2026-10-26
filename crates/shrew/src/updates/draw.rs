//! Drawing the update notification and the Update shrew dialog, and where their parts landed ([`Hits`]). Both draw
//! in layer 2, like the About and Settings panels: the toast in the app's one toast look, the dialog centered.

use super::dialog::{self, Button, Stage};
use super::notes::{self, Ink};
use super::{Toast, Updates};
use crate::colors::{Colors, mix};
use crate::layout::Layout;
use crate::state::State;
use shrew_render::{Color, Rect, Scene, TextStyle, columns};
use shrew_update::Phase;

#[derive(Clone, Debug, Default)]
pub struct Hits {
    pub toast: crate::toast::Hits,
    pub dialog: Option<Rect>,
    pub buttons: Vec<(Rect, Button)>,
    /// Notes rows on screen (a page for PageUp/PageDown).
    pub notes_rows: usize,
}

pub fn draw(scene: &mut Scene, st: &mut State, lay: &Layout, c: &Colors) {
    st.updates.hits = Hits::default();
    // above git's toast when that one shows
    #[cfg(feature = "git")]
    let bottom = st.git.ops.hits.area.map_or(crate::toast::bottom(lay), |a| a.y - 2.0 * lay.pad);
    #[cfg(not(feature = "git"))]
    let bottom = crate::toast::bottom(lay);
    let ends = (super::terminals(st), st.waits.len());
    let u = &mut st.updates;
    if u.dialog.is_some() {
        dialog(scene, u, ends, lay, c);
    } else if u.toast.is_some() {
        toast(scene, u, bottom, lay, c);
    }
}

fn toast(scene: &mut Scene, u: &mut Updates, bottom: f32, lay: &Layout, c: &Colors) {
    let Some(t) = u.toast.clone() else { return };
    let (title, lines, buttons, error, tint): (String, Vec<String>, Vec<&str>, bool, Color) = match &t {
        Toast::Available(r) => {
            let mut lines = vec![format!("You have {}.  Ctrl+Shift+U opens the update.", u.current)];
            let buttons = match u.support() {
                Ok(()) => vec!["Update now", "Release notes"],
                Err(why) => {
                    lines.push(format!("This installation cannot update itself: {why}."));
                    vec!["View release"]
                }
            };
            (format!("shrew {} is available", r.version), lines, buttons, false, c.accent)
        }
        Toast::Note { title, body, error, link, retry } => {
            let mut buttons = Vec::new();
            if *retry {
                buttons.push("Retry");
            }
            if let Some((label, _)) = link {
                buttons.push(*label);
            }
            let tint = if *error { c.error } else { c.accent };
            (title.clone(), vec![body.clone()], buttons, *error, tint)
        }
    };
    let spec = crate::toast::Spec {
        title: &title,
        tint,
        error,
        lines: &lines,
        max_lines: 6,
        more: "…",
        buttons: &buttons,
        close: "✕",
        hot: u.hot,
    };
    u.hits.toast = crate::toast::draw(scene, &spec, bottom, lay, c);
}

/// Most notes rows the dialog shows before it scrolls them.
const NOTES_ROWS: usize = 14;

/// `ends`: open terminals and `srw --wait` clients, which a restart ends.
fn dialog(scene: &mut Scene, u: &mut Updates, ends: (usize, usize), lay: &Layout, c: &Colors) {
    let (terminals, waits) = ends;
    let stage = u.stage();
    let Some(d) = u.dialog.as_mut() else { return };
    d.sync(&stage);
    let (focused, scroll) = (d.focused(&stage), d.scroll);
    let hot = u.dialog_hot;
    let (cw, ch, pad) = (lay.cw, lay.ch, lay.pad);
    let line_w = lay.scale.round().max(1.0);
    let radius = (5.0 * lay.scale).round();
    let w = (80.0 * cw).min(lay.width - 4.0 * cw).max(30.0 * cw).round();
    let inner = ((w - 6.0 * cw) / cw).floor().max(10.0) as usize;
    let x = ((lay.width - w) / 2.0).round();
    let tx = x + 3.0 * cw;

    // what it says, top to bottom
    let phase = u.phase();
    let release = match &phase {
        Phase::Ready(s) => Some(s.release.clone()),
        _ => u.available.clone(),
    };
    let version_line = match &release {
        Some(r) => {
            let date = r.published.get(..10).map(|d| format!("  ·  released {d}")).unwrap_or_default();
            vec![(format!("shrew {}  →  {}", u.current, r.version), date)]
        }
        None => vec![(format!("You have shrew {}.", u.current), String::new())],
    };
    let notes = match &release {
        Some(r) if !r.notes.trim().is_empty() => notes::lines(&r.notes, inner.saturating_sub(2)),
        Some(_) => vec![notes::Line { text: "This release has no notes.".into(), ink: Ink::Muted }],
        None => Vec::new(),
    };
    let wrap = |s: &str| notes::word_wrap(s, inner);
    let (status, status_ink): (Vec<String>, Color) = match &stage {
        Stage::Nothing if u.checking() => (vec!["Checking for updates…".into()], c.panel_muted),
        Stage::Nothing => {
            let s = match (&u.error, &u.latest) {
                (Some(e), _) => format!("Could not check for updates: {e}"),
                (None, Some(l)) => format!("shrew is up to date ({l} is the latest release)."),
                (None, None) => "No update is known yet: check again to ask GitHub.".into(),
            };
            (wrap(&s), if u.error.is_some() { c.error } else { c.panel_foreground })
        }
        Stage::Unsupported(why) => (
            wrap(&format!(
                "This installation cannot update itself: {why}. Download the new version from the release page."
            )),
            c.panel_foreground,
        ),
        Stage::Offer => {
            let size = release.as_ref().map(|r| r.size).filter(|&s| s > 0);
            let size = size.map(|s| format!(" ({})", dialog::bytes(s))).unwrap_or_default();
            let s = format!(
                "Download the new version{size} from GitHub and check it against the published sha256; then \
                 restart to install it."
            );
            (wrap(&s), c.panel_muted)
        }
        Stage::Downloading { .. } | Stage::Verifying => (Vec::new(), c.panel_muted),
        Stage::Ready => {
            let mut s = wrap("Restart to install it: your open files come back as they are, unsaved changes too.");
            if terminals > 0 {
                let n = if terminals == 1 {
                    "The open terminal closes".to_string()
                } else {
                    format!("The {terminals} open terminals close")
                };
                s.extend(wrap(&format!("{n}: their shells end with the restart.")));
            }
            if waits > 0 {
                s.extend(wrap("Programs waiting on `srw --wait` (git commit…) are told their files are closed."));
            }
            (s, c.panel_muted)
        }
        Stage::Failed(e) => (wrap(&format!("The download failed: {e}")), c.error),
    };

    // heights: title, versions, notes box, status, buttons
    let head_h = 2.0 * ch + 3.0 * pad;
    let progress = matches!(stage, Stage::Downloading { .. } | Stage::Verifying);
    let ready_line = matches!(stage, Stage::Ready);
    let status_rows = status.len() + usize::from(ready_line) + if progress { 2 } else { 0 };
    let status_h = status_rows as f32 * ch + if progress { pad } else { 0.0 };
    let button_h = (ch + 2.0 * pad).round();
    let foot_h = button_h + 4.0 * pad;
    let fixed = head_h + status_h + foot_h + 6.0 * pad;
    let room_rows = (((lay.status.y - 4.0 * pad - fixed) / ch).floor() as usize).saturating_sub(1);
    let shown = notes.len().min(NOTES_ROWS).min(room_rows.max(3));
    let notes_h = if notes.is_empty() { 0.0 } else { shown as f32 * ch + 2.0 * pad };
    let h = (fixed + notes_h + if notes.is_empty() { 0.0 } else { 3.0 * pad }).round();
    let y = ((lay.status.y - h) / 2.0).max(pad).round();
    let panel = Rect::new(x, y, w, h);
    scene.layer(2);
    crate::draw::popup(scene, panel, lay, c);
    u.hits.dialog = Some(panel);

    let mut cy = y + 2.0 * pad;
    scene.text(tx, cy.round(), "Update shrew", TextStyle::new(c.panel_foreground).bold(), &[]);
    let hint = match &stage {
        Stage::Downloading { .. } | Stage::Verifying => "Esc hide",
        Stage::Ready => "Esc later",
        _ => "Esc close",
    };
    let hx = panel.right() - 3.0 * cw - columns(hint) as f32 * cw;
    scene.text(hx.round(), cy.round(), hint, TextStyle::new(c.panel_muted), &[]);
    cy += ch + pad;
    for (text, extra) in &version_line {
        scene.text(tx, cy.round(), text, TextStyle::new(c.accent).bold(), &[]);
        if !extra.is_empty() {
            let ex = tx + columns(text) as f32 * cw;
            scene.text(ex.round(), cy.round(), extra, TextStyle::new(c.panel_muted), &[]);
        }
        cy += ch;
    }
    cy += 2.0 * pad;

    // the notes, in a box of their own, scrolled
    if !notes.is_empty() {
        let bx = Rect::new(tx - cw, cy.round(), w - 4.0 * cw, notes_h.round());
        scene.rounded_rect(bx, radius, mix(c.panel_selection, c.popup, 0.45), Some((line_w, c.panel_border)));
        let top = scroll.min(notes.len() - shown);
        if let Some(d) = u.dialog.as_mut() {
            d.scroll = top;
        }
        u.hits.notes_rows = shown;
        scene.push_clip(bx);
        for (i, l) in notes.iter().skip(top).take(shown).enumerate() {
            let style = match l.ink {
                Ink::Heading => TextStyle::new(c.panel_foreground).bold(),
                Ink::Code => TextStyle::new(c.accent_alt),
                Ink::Muted => TextStyle::new(c.panel_muted),
                Ink::Text => TextStyle::new(c.panel_foreground),
            };
            scene.text(tx, (bx.y + pad + i as f32 * ch).round(), &l.text, style, &[]);
        }
        scene.pop_clip();
        if shown < notes.len() {
            let bw = (3.0 * lay.scale).round().max(2.0);
            let track = bx.h - 2.0 * pad;
            let th = (track * shown as f32 / notes.len() as f32).max(ch);
            let ty = bx.y + pad + (track - th) * top as f32 / (notes.len() - shown) as f32;
            let sx = bx.right() - cw;
            scene.rounded_rect(Rect::new(sx.round(), ty.round(), bw, th.round()), bw / 2.0, c.scrollbar, None);
        }
        cy += notes_h + 3.0 * pad;
    }

    // the status: progress bar, message, error
    match &stage {
        Stage::Downloading { done, total, speed } => {
            let pct = dialog::percent(*done, *total);
            progress_bar(scene, Rect::new(tx, cy, w - 6.0 * cw, ch), pct as f32 / 100.0, c.accent, lay, c);
            cy += ch + pad;
            let (l, r) = dialog::progress_text(*done, *total, *speed);
            scene.text(tx, cy.round(), &l, TextStyle::new(c.panel_foreground), &[]);
            let rx = panel.right() - 3.0 * cw - columns(&r) as f32 * cw;
            scene.text(rx.round(), cy.round(), &r, TextStyle::new(c.panel_muted), &[]);
            cy += ch;
        }
        Stage::Verifying => {
            progress_bar(scene, Rect::new(tx, cy, w - 6.0 * cw, ch), 1.0, mix(c.accent, c.popup, 0.4), lay, c);
            cy += ch + pad;
            let s = "Verifying the download against the published sha256…";
            scene.text(tx, cy.round(), s, TextStyle::new(c.panel_muted), &[]);
            cy += ch;
        }
        Stage::Ready => {
            scene.text(tx, cy.round(), "✓ Downloaded and verified.", TextStyle::new(c.success).bold(), &[]);
            cy += ch;
        }
        _ => {}
    }
    for l in &status {
        scene.text(tx, cy.round(), l, TextStyle::new(status_ink), &[]);
        cy += ch;
    }

    // the buttons, right-aligned; the last is the main one
    let by = (panel.bottom() - 2.0 * pad - button_h).round();
    let sep = (by - 2.0 * pad).round();
    scene.rect(Rect::new(x, sep, w, line_w), c.panel_border);
    let buttons = stage.buttons();
    let mut bx = panel.right() - 3.0 * cw;
    let ty = lay.centered(by, button_h);
    for (i, b) in buttons.iter().enumerate().rev() {
        let label = b.label();
        let bw = ((columns(label) + 4) as f32 * cw).round();
        bx -= bw;
        let r = Rect::new(bx.round(), by, bw, button_h);
        let main = i + 1 == buttons.len();
        let (bg, fg) = if main { (c.accent, c.accent_fg) } else { (c.panel_selection, c.panel_foreground) };
        let border = if main { None } else { Some((line_w, c.panel_border)) };
        // under the pointer: a little lighter or darker, as the toast's buttons
        let bg = if hot == Some(*b) { crate::statusbar::lit(bg, fg) } else { bg };
        scene.rounded_rect(r, radius, bg, border);
        if i == focused {
            // the keyboard's button: a ring around it
            let ring = Rect::new(r.x - 2.0 * line_w, r.y - 2.0 * line_w, r.w + 4.0 * line_w, r.h + 4.0 * line_w);
            scene.rounded_rect(ring, radius + 2.0 * line_w, Color::TRANSPARENT, Some((line_w, c.accent)));
        }
        let lx = r.x + (r.w - columns(label) as f32 * cw) / 2.0;
        scene.text(lx.round(), ty, label, TextStyle::new(fg).bold(), &[]);
        u.hits.buttons.push((r, *b));
        bx -= cw * 1.5;
    }
    let keys = if notes.len() > shown { "j/k scroll · Tab · Enter" } else { "Tab · Enter" };
    if tx + (columns(keys) + 2) as f32 * cw < bx {
        scene.text(tx, ty, keys, TextStyle::new(c.panel_muted), &[]);
    }
    let _ = cy;
    scene.layer(0);
}

fn progress_bar(scene: &mut Scene, line: Rect, f: f32, fill: Color, lay: &Layout, c: &Colors) {
    let h = (0.45 * lay.ch).round().max(4.0);
    let track = Rect::new(line.x.round(), (line.y + (line.h - h) / 2.0).round(), line.w.round(), h);
    scene.rounded_rect(track, h / 2.0, mix(c.panel_muted, c.popup, 0.3), None);
    let fw = (track.w * f.clamp(0.0, 1.0)).round();
    if fw > 0.0 {
        scene.rounded_rect(Rect::new(track.x, track.y, fw.max(h), h), h / 2.0, fill, None);
    }
}

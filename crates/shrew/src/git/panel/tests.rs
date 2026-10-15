//! The panel model: sections from a status, keys, and the actions they ask for.

use super::*;
use shrew_git::{ConflictKind, FileState, StatusEntry};

fn entry(path: &str) -> StatusEntry {
    StatusEntry { path: path.into(), ..Default::default() }
}

fn status() -> Status {
    let entries = vec![
        StatusEntry { conflict: Some(ConflictKind::BothModified), ..entry("both.rs") },
        StatusEntry { staged: Some(FileState::Modified), ..entry("src/staged.rs") },
        StatusEntry { staged: Some(FileState::Added), unstaged: Some(FileState::Modified), ..entry("src/partly.rs") },
        StatusEntry { unstaged: Some(FileState::Deleted), ..entry("gone.txt") },
        StatusEntry { staged: Some(FileState::Renamed), orig_path: Some("old.rs".into()), ..entry("new.rs") },
        StatusEntry { untracked: true, ..entry("notes/") },
        StatusEntry { ignored: true, ..entry("target/") },
    ];
    Status { entries, ..Default::default() }
}

fn panel() -> Panel {
    let mut p = Panel::default();
    p.set_status(&status());
    p.fit(50);
    p
}

fn labels(p: &Panel) -> Vec<String> {
    p.rows
        .iter()
        .map(|r| match r {
            Row::Header(s, n) => format!("{} {n}", s.title()),
            Row::File(f) => format!("  {} {}", f.letter, f.path),
        })
        .collect()
}

fn keys(p: &mut Panel, s: &str) -> Vec<Action> {
    KeyPress::parse_seq(s).into_iter().filter_map(|k| p.key(k)).collect()
}

#[test]
fn sections_in_order_with_letters() {
    let p = panel();
    assert_eq!(
        labels(&p),
        vec![
            "MERGE CHANGES 1",
            "  ! both.rs",
            "STAGED CHANGES 3",
            "  M src/staged.rs",
            "  A src/partly.rs",
            "  R new.rs",
            "CHANGES 2",
            "  M src/partly.rs",
            "  D gone.txt",
            "UNTRACKED 1",
            "  U notes/",
        ]
    );
    assert_eq!(p.total(), 7, "ignored files are not listed");
    let renamed =
        p.rows.iter().find_map(|r| if let Row::File(f) = r { (f.path == "new.rs").then_some(f) } else { None });
    assert_eq!(renamed.unwrap().orig_path.as_deref(), Some("old.rs"));
    let f =
        File { section: Section::Changes, path: "a/b/c.rs".into(), orig_path: None, letter: 'M', mark: Mark::Modified };
    assert_eq!((f.name(), f.dir()), ("c.rs", "a/b"));
    let f = File { path: "notes/".into(), ..f };
    assert_eq!((f.name(), f.dir()), ("notes", ""));
}

#[test]
fn stage_unstage_and_discard_by_row() {
    let mut p = panel();
    p.selected = 7; // CHANGES: src/partly.rs
    assert_eq!(keys(&mut p, "s"), vec![Action::Op(Op::Stage(vec!["src/partly.rs".into()]))]);
    assert_eq!(keys(&mut p, "u"), vec![], "nothing staged in this row");
    assert!(matches!(keys(&mut p, "x").as_slice(), [Action::Ask(q, Op::Discard(_))] if q.contains("src/partly.rs")));
    p.selected = 5; // a staged rename unstages both of its paths
    assert_eq!(keys(&mut p, "-"), vec![Action::Op(Op::Unstage(vec!["new.rs".into(), "old.rs".into()]))]);
    p.selected = 10; // untracked: discarding deletes, after asking
    assert!(
        matches!(keys(&mut p, "x").as_slice(), [Action::Ask(q, Op::Clean(p))] if q.contains("for good") && p == &["notes/"])
    );
    p.selected = 1; // a conflict is staged by marking it resolved
    assert_eq!(keys(&mut p, "+"), vec![Action::Op(Op::MarkResolved(vec!["both.rs".into()]))]);
    assert_eq!(keys(&mut p, "SU"), vec![Action::Op(Op::StageAll), Action::Op(Op::UnstageAll)]);
}

#[test]
fn headers_act_on_their_section_even_when_folded() {
    let mut p = panel();
    p.selected = 6; // CHANGES
    keys(&mut p, "<CR>");
    assert!(p.is_collapsed(Section::Changes));
    assert_eq!(p.rows.len(), 9, "its two files are hidden");
    assert!(matches!(p.current(), Some(Row::Header(Section::Changes, 2))), "the header stays selected");
    assert_eq!(keys(&mut p, "s"), vec![Action::Op(Op::Stage(vec!["src/partly.rs".into(), "gone.txt".into()]))]);
    assert!(
        matches!(keys(&mut p, "x").as_slice(), [Action::Ask(q, Op::Discard(v))] if q.contains("2 files") && v.len() == 2)
    );
    p.selected = 2;
    assert_eq!(keys(&mut p, "u"), vec![Action::Op(Op::UnstageAll)]);
    keys(&mut p, "j<Left>");
    assert_eq!(p.selected, 2, "h on a file goes to its header");
}

#[test]
fn moving_opening_and_focus() {
    let mut p = panel();
    p.focused = true;
    keys(&mut p, "jj");
    assert_eq!(p.selected, 2);
    keys(&mut p, "G");
    assert_eq!(p.selected, p.rows.len() - 1);
    keys(&mut p, "gj");
    let opened = keys(&mut p, "<CR>");
    assert!(matches!(opened.as_slice(), [Action::OpenChange(f, false)] if f.path == "both.rs"));
    assert!(matches!(keys(&mut p, " ").as_slice(), [Action::OpenChange(_, true)]), "Space keeps the focus here");
    assert_eq!(keys(&mut p, "o"), vec![Action::OpenFile("both.rs".into(), false)]);
    assert_eq!(keys(&mut p, "<Esc>"), vec![Action::Unfocus]);
    assert_eq!(keys(&mut p, "q"), vec![Action::Hide]);
    assert_eq!(keys(&mut p, "b"), vec![Action::Run(Command::GitBranches)]);
    assert!(matches!(keys(&mut p, "<C-S-p>").as_slice(), [Action::Pass(_)]), "editor chords pass through");
}

#[test]
fn tab_walks_the_sections_then_the_message() {
    let mut p = panel();
    keys(&mut p, "<Tab>");
    assert!(matches!(p.current(), Some(Row::Header(Section::Staged, _))));
    keys(&mut p, "<Tab><Tab>");
    assert!(matches!(p.current(), Some(Row::Header(Section::Untracked, _))));
    keys(&mut p, "<Tab>");
    assert_eq!(p.focus, Focus::Message);
    keys(&mut p, "fix: it<CR>more");
    assert_eq!(p.message.text, "fix: it\nmore", "Enter is a line break in the message");
    assert_eq!(keys(&mut p, "<C-CR>"), vec![Action::Commit { amend: false }]);
    assert_eq!(keys(&mut p, "<C-S-CR>"), vec![Action::Commit { amend: true }]);
    assert_eq!(keys(&mut p, "<C-v>"), vec![Action::Paste]);
    keys(&mut p, "<Esc>");
    assert_eq!(p.focus, Focus::List);
    keys(&mut p, "c");
    assert_eq!(p.focus, Focus::Message);
    keys(&mut p, "<Tab>");
    assert_eq!((p.focus, p.selected), (Focus::List, 0));
}

#[test]
fn the_selection_follows_a_file_across_refreshes() {
    let mut p = panel();
    p.selected = 8; // CHANGES: gone.txt
    let mut s = status();
    s.entries.retain(|e| e.path != "src/staged.rs");
    p.set_status(&s);
    assert!(matches!(p.current_file(), Some(f) if f.path == "gone.txt" && f.section == Section::Changes));
    // staged: it leaves Changes; the selection stays where it was, on the next change
    let mut s = status();
    for e in &mut s.entries {
        if e.path == "gone.txt" {
            (e.staged, e.unstaged) = (Some(FileState::Deleted), None);
        }
    }
    let mut p = panel();
    p.selected = 8;
    p.set_status(&s);
    assert!(matches!(p.current_file(), Some(f) if f.path == "src/partly.rs" && f.section == Section::Changes));
}

#[test]
fn scrolling_keeps_the_selection_in_view() {
    let s = Status {
        entries: (0..40)
            .map(|i| StatusEntry { unstaged: Some(FileState::Modified), ..entry(&format!("f{i:02}")) })
            .collect(),
        ..Default::default()
    };
    let mut p = Panel::default();
    p.set_status(&s);
    p.fit(10);
    keys(&mut p, "G");
    p.fit(10);
    assert_eq!(p.top, 31);
    p.scroll(-100);
    assert_eq!(p.top, 0);
    p.scroll(5);
    assert_eq!(p.top, 5);
}

//! The log view's model: paging, the author filter, search, and the graph rows it draws from.

use super::*;
use shrew_git::graph::GraphBuilder;
use shrew_git::{RefKind, Signature};

fn oid(n: u8) -> Oid {
    Oid::from_bytes([n; 20])
}

fn row(n: u8, author: &str, summary: &str) -> Row {
    Row {
        commit: CommitInfo {
            id: oid(n),
            parents: vec![oid(n.wrapping_sub(1))],
            author: Signature {
                name: author.into(),
                email: format!("{author}@x.org"),
                time: 1000 + n as i64,
                offset: 0,
            },
            committer: Signature::default(),
            summary: summary.into(),
        },
        graph: None,
        refs: Vec::new(),
        path: None,
    }
}

fn view(rows: Vec<Row>) -> View {
    let mut v = View::new(1, PathBuf::from("/r"), Kind::Log { all: false });
    assert!(v.add(Page { rows, more: true }, 0));
    v
}

#[test]
fn pages_add_up_and_a_reload_replaces_them() {
    let mut v = view((1..=3).map(|n| row(n, "ann", "x")).collect());
    assert_eq!(v.shown, vec![0, 1, 2]);
    assert!(v.add(Page { rows: vec![row(4, "bob", "y")], more: false }, 3));
    assert_eq!((v.rows.len(), v.more), (4, false));
    assert!(!v.add(Page { rows: vec![row(5, "x", "y")], more: false }, 2), "a page that no longer fits is dropped");
    v.selected = 2;
    v.add(Page { rows: vec![row(9, "new", "z"), row(3, "ann", "x"), row(4, "bob", "y")], more: true }, 0);
    assert_eq!(v.rows.len(), 3);
    assert_eq!(v.current().unwrap().commit.id, oid(3), "the selected commit stays selected");
}

#[test]
fn the_author_filter_narrows_the_rows() {
    let mut v = view(vec![row(1, "Ann", "a"), row(2, "bob", "b"), row(3, "ann", "c")]);
    v.selected = 2;
    v.author = Some("ANN".into());
    v.refilter();
    assert_eq!(v.shown, vec![0, 2]);
    assert_eq!(v.selected, 1, "the selected commit is still selected");
    v.author = Some("bob@x".into());
    v.refilter();
    assert_eq!(v.shown, vec![1], "by email too");
}

#[test]
fn search_finds_summaries_authors_ids_and_refs() {
    let mut rows = vec![row(1, "ann", "fix the parser"), row(2, "bob", "add tests"), row(3, "cy", "docs")];
    rows[2].refs.push(RefLabel { name: "v1.0".into(), kind: RefKind::Tag, is_head: false });
    let v = view(rows);
    assert_eq!(v.find("PARSER", false, false), Some(0));
    assert_eq!(v.find("bob", false, false), Some(1));
    assert_eq!(v.find("v1.0", false, false), Some(2));
    assert_eq!(v.find(&oid(2).to_string()[..6], false, false), Some(1));
    assert_eq!(v.find("parser", false, true), None, "from the next row on");
    let mut v = v;
    v.selected = 2;
    assert_eq!(v.find("fix", true, true), Some(0), "backwards");
}

#[test]
fn more_is_asked_near_the_end() {
    let mut v = view((0..100).map(|n| row(n, "a", "s")).collect());
    v.fit(20);
    assert!(!v.wants_more(), "far from the end");
    v.selected = 95;
    v.follow = true;
    v.fit(20);
    assert!(v.wants_more());
    v.loading = true;
    assert!(!v.wants_more(), "one request at a time");
}

#[test]
fn graph_rows_come_through_pages() {
    // a merge: c3 has parents c2 and c1, c2 has c1
    let mut g = GraphBuilder::new();
    let r3 = g.push(oid(3), &[oid(2), oid(1)]);
    let r2 = g.push(oid(2), &[oid(1)]);
    let r1 = g.push(oid(1), &[]);
    assert!(r3.merge && r3.width >= 2);
    let f = graph::Frame { x0: 0.0, lane: 10.0, top: 0.0, mid: 10.0, bottom: 20.0 };
    for r in [&r3, &r2, &r1] {
        for (s, _) in graph::row(&f, r) {
            if let graph::Shape::V { y0, y1, .. } = s {
                assert!((0.0..=20.0).contains(&y0) && y0 <= y1 && y1 <= 20.0);
            }
        }
    }
    // the second parent's lane bends back into the first somewhere
    assert!([&r3, &r2, &r1].iter().any(|r| r.edges.iter().any(|e| e.from != e.to)));
}

#[test]
fn dates_are_in_the_authors_zone() {
    assert_eq!(draw::date(0, 0), "1970-01-01 00:00");
    assert_eq!(draw::date(951_782_400, 0), "2000-02-29 00:00");
    assert_eq!(draw::date(1_700_000_000, 3600), "2023-11-14 23:13");
    assert_eq!(draw::date(1_700_000_000, -5 * 3600), "2023-11-14 17:13");
}

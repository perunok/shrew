//! Lane assignment for a commit graph, like `git log --graph` or Sublime Merge: feed commits newest first (in
//! topological order) and get, per row, the lane of the commit's dot and the line segments crossing the row.
//!
//! Drawing a row of height `h`: lane `i` sits at `x = i * lane_width`. An [`EdgeKind::Pass`] runs from
//! `(from, top)` to `(to, bottom)`, an [`EdgeKind::In`] from `(from, top)` to the dot at `(node, h / 2)`, an
//! [`EdgeKind::Out`] from the dot to `(to, bottom)`. When `from != to` the segment is a diagonal or curve.
//! Lanes are compacted every row, so the graph stays as narrow as the history allows.

use crate::oid::Oid;

/// How a segment crosses a row.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum EdgeKind {
    /// Passes by the commit: top of lane `from` to bottom of lane `to`.
    Pass,
    /// Comes down from the row above into the dot (`to` is the node lane).
    In,
    /// Leaves the dot towards a parent (`from` is the node lane).
    Out,
}

/// One segment of a row.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Edge {
    pub from: u16,
    pub to: u16,
    pub kind: EdgeKind,
    /// Colour index of the line of history (stable along it; take it modulo the palette size).
    pub color: u16,
}

/// The graph cell of one commit.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct GraphRow {
    /// Lane of the commit's dot.
    pub node: u16,
    /// Colour index of the dot.
    pub color: u16,
    pub edges: Vec<Edge>,
    /// Lanes this row spans (enough room for the dot and every segment).
    pub width: u16,
    /// The commit has more than one parent.
    pub merge: bool,
}

#[derive(Copy, Clone, Debug)]
struct Lane {
    /// The commit this lane is waiting for.
    id: Oid,
    color: u16,
}

/// Incremental lane assignment; keeps the open lanes and the ids already placed.
#[derive(Default, Debug)]
pub struct GraphBuilder {
    lanes: Vec<Lane>,
    next_color: u16,
    /// Commits already laid out: a parent among them (date order with a skewed clock) gets no lane, which
    /// would otherwise wait forever.
    seen: std::collections::HashSet<Oid>,
}

impl GraphBuilder {
    pub fn new() -> GraphBuilder {
        GraphBuilder::default()
    }

    fn color(&mut self) -> u16 {
        let c = self.next_color;
        self.next_color = self.next_color.wrapping_add(1);
        c
    }

    /// Lays out the next commit (`parents` in order, first parent first).
    pub fn push(&mut self, id: Oid, parents: &[Oid]) -> GraphRow {
        self.seen.insert(id);
        let top: Vec<Lane> = std::mem::take(&mut self.lanes);
        let mine: Vec<usize> = top.iter().enumerate().filter(|(_, l)| l.id == id).map(|(i, _)| i).collect();
        let (node, color) = match mine.first() {
            Some(&i) => (i, top[i].color),
            None => (top.len(), self.color()),
        };

        // the lanes after this row, with holes where lanes ended
        let mut bottom: Vec<Option<Lane>> = top.iter().map(|l| (l.id != id).then_some(*l)).collect();
        if node == bottom.len() {
            bottom.push(None);
        }
        let mut outs: Vec<(usize, u16)> = Vec::with_capacity(parents.len());
        for (k, &p) in parents.iter().enumerate() {
            if self.seen.contains(&p) || outs.iter().any(|&(j, _)| bottom[j].is_some_and(|l| l.id == p)) {
                continue; // already shown above, or the same parent listed twice
            }
            if let Some(j) = bottom.iter().position(|l| l.is_some_and(|l| l.id == p)) {
                outs.push((j, bottom[j].unwrap().color));
            } else if k == 0 && bottom[node].is_none() {
                bottom[node] = Some(Lane { id: p, color });
                outs.push((node, color));
            } else {
                let c = self.color();
                let j = match bottom.iter().skip(node + 1).position(Option::is_none) {
                    Some(off) => node + 1 + off,
                    None => {
                        bottom.push(None);
                        bottom.len() - 1
                    }
                };
                bottom[j] = Some(Lane { id: p, color: c });
                outs.push((j, c));
            }
        }

        // compact: every open lane moves left over the holes
        let mut map = vec![0u16; bottom.len()];
        let mut n = 0u16;
        for (i, l) in bottom.iter().enumerate() {
            map[i] = n;
            if l.is_some() {
                n += 1;
            }
        }
        let mut edges = Vec::with_capacity(top.len() + outs.len());
        for (i, l) in top.iter().enumerate() {
            if l.id == id {
                edges.push(Edge { from: i as u16, to: node as u16, kind: EdgeKind::In, color: l.color });
            } else {
                edges.push(Edge { from: i as u16, to: map[i], kind: EdgeKind::Pass, color: l.color });
            }
        }
        for (j, c) in outs {
            edges.push(Edge { from: node as u16, to: map[j], kind: EdgeKind::Out, color: c });
        }
        self.lanes = bottom.into_iter().flatten().collect();
        let width = (top.len().max(self.lanes.len()).max(node + 1)) as u16;
        GraphRow { node: node as u16, color, edges, width, merge: parents.len() > 1 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> Oid {
        let mut b = [0u8; 20];
        b[0] = n;
        Oid::from_bytes(b)
    }

    /// Renders rows as `git log --graph`-like text: `*` for the dot, `|` for a straight lane.
    fn ascii(rows: &[GraphRow]) -> Vec<String> {
        rows.iter()
            .map(|r| {
                let mut s = vec![' '; r.width as usize];
                for e in &r.edges {
                    if e.kind == EdgeKind::Pass && e.from == e.to {
                        s[e.from as usize] = '|';
                    }
                }
                s[r.node as usize] = '*';
                s.into_iter().collect::<String>().trim_end().to_string()
            })
            .collect()
    }

    #[test]
    fn linear_history_stays_in_one_lane() {
        let mut g = GraphBuilder::new();
        let rows: Vec<_> = (0..4).map(|i| g.push(id(4 - i), &[])).collect();
        // unconnected commits each get a fresh lane at 0 because lanes are compacted
        assert!(rows.iter().all(|r| r.node == 0));
        let mut g = GraphBuilder::new();
        let r1 = g.push(id(3), &[id(2)]);
        let r2 = g.push(id(2), &[id(1)]);
        let r3 = g.push(id(1), &[]);
        assert_eq!((r1.node, r2.node, r3.node), (0, 0, 0));
        assert_eq!(
            r2.edges,
            vec![
                Edge { from: 0, to: 0, kind: EdgeKind::In, color: 0 },
                Edge { from: 0, to: 0, kind: EdgeKind::Out, color: 0 },
            ]
        );
        assert!(r3.edges.iter().all(|e| e.kind == EdgeKind::In));
    }

    #[test]
    fn merge_and_fork() {
        // 5 merges 4 and 3; 4 and 3 both come from 2; 2 from 1
        let mut g = GraphBuilder::new();
        let rows = vec![
            g.push(id(5), &[id(4), id(3)]),
            g.push(id(4), &[id(2)]),
            g.push(id(3), &[id(2)]),
            g.push(id(2), &[id(1)]),
            g.push(id(1), &[]),
        ];
        assert_eq!(ascii(&rows), ["*", "*|", "|*", "*", "*"]);
        assert!(rows[0].merge);
        assert_eq!(rows[0].width, 2);
        // the merge opens lane 1 in a new colour
        let out: Vec<_> = rows[0].edges.iter().filter(|e| e.kind == EdgeKind::Out).map(|e| (e.to, e.color)).collect();
        assert_eq!(out, [(0, 0), (1, 1)]);
        // 3 joins lane 0 (where 2 is expected) and its lane closes
        assert!(rows[2].edges.contains(&Edge { from: 1, to: 0, kind: EdgeKind::Out, color: 0 }));
        assert_eq!(rows[3].width, 1);
    }

    #[test]
    fn a_parent_shown_before_its_child_opens_no_lane() {
        let mut g = GraphBuilder::new();
        g.push(id(1), &[]);
        let r = g.push(id(2), &[id(1)]);
        assert!(r.edges.is_empty());
        assert_eq!(g.push(id(3), &[]).width, 1);
    }

    #[test]
    fn lanes_compact_when_one_ends() {
        // three branch tips: a(lane0)->x, b(lane1)->root, c(lane2)->y ; b ends, c shifts left
        let mut g = GraphBuilder::new();
        g.push(id(10), &[id(1)]);
        g.push(id(11), &[id(2)]);
        g.push(id(12), &[id(3)]);
        let r = g.push(id(2), &[]);
        assert_eq!(r.node, 1);
        assert!(r.edges.contains(&Edge { from: 2, to: 1, kind: EdgeKind::Pass, color: 2 }));
        let r = g.push(id(3), &[]);
        assert_eq!(r.node, 1);
    }
}

//! The commit graph's shapes for one row, from the backend's [`GraphRow`]: straight lines and quarter circles, so
//! a lane that moves sideways bends round (down, across, down) like a metro map. Pure geometry (tested); the view
//! draws the shapes with rects and clipped circle outlines.

use shrew_git::graph::{Edge, EdgeKind, GraphRow};

/// Which quarter of a circle an arc is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quarter {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// A vertical line at `x` from `y0` down to `y1`.
    V { x: f32, y0: f32, y1: f32 },
    /// A horizontal line at `y` from `x0` to `x1` (either order).
    H { y: f32, x0: f32, x1: f32 },
    /// A quarter of the circle at (`cx`, `cy`) of radius `r`.
    Arc { cx: f32, cy: f32, r: f32, q: Quarter },
}

/// A row's geometry: where lane 0's centre is, the lane pitch, and the row's top, middle and bottom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub x0: f32,
    pub lane: f32,
    pub top: f32,
    pub mid: f32,
    pub bottom: f32,
}

impl Frame {
    pub fn x(&self, lane: u16) -> f32 {
        (self.x0 + lane as f32 * self.lane).round()
    }

    /// The bend radius for a sideways move of `dx`.
    fn radius(&self, dx: f32) -> f32 {
        (dx.abs() / 2.0).min(self.mid - self.top).min(self.lane * 0.6).max(0.0).round()
    }
}

/// The shapes of one edge.
pub fn edge(f: &Frame, node: u16, e: &Edge) -> Vec<Shape> {
    let (xa, xb) = (f.x(e.from), f.x(e.to));
    let mut v = match e.kind {
        EdgeKind::Pass if e.from == e.to => vec![Shape::V { x: xa, y0: f.top, y1: f.bottom }],
        EdgeKind::Pass => {
            // half the move on the way down to the middle, half on the way on
            let r = f.radius((xb - xa) / 2.0);
            let mid_x = ((xa + xb) / 2.0).round();
            let mut v = down_then_across(f, xa, mid_x, r);
            v.extend(across_then_down(f, mid_x, xb, r));
            v
        }
        EdgeKind::In => {
            let xn = f.x(node);
            if xa == xn {
                return vec![Shape::V { x: xa, y0: f.top, y1: f.mid }];
            }
            down_then_across(f, xa, xn, f.radius(xn - xa))
        }
        EdgeKind::Out => {
            let xn = f.x(node);
            if xb == xn {
                return vec![Shape::V { x: xb, y0: f.mid, y1: f.bottom }];
            }
            across_then_down(f, xn, xb, f.radius(xb - xn))
        }
    };
    v.retain(|s| match s {
        Shape::H { x0, x1, .. } => (x0 - x1).abs() >= 0.5,
        Shape::V { y0, y1, .. } => y1 - y0 >= 0.5,
        Shape::Arc { r, .. } => *r >= 0.5,
    });
    v
}

/// From the top at `xa` down to the middle, bending towards `xb`, then along the middle to `xb`.
fn down_then_across(f: &Frame, xa: f32, xb: f32, r: f32) -> Vec<Shape> {
    let right = xb > xa;
    let cx = if right { xa + r } else { xa - r };
    let q = if right { Quarter::BottomLeft } else { Quarter::BottomRight };
    vec![
        Shape::V { x: xa, y0: f.top, y1: f.mid - r },
        Shape::Arc { cx, cy: f.mid - r, r, q },
        Shape::H { y: f.mid, x0: cx, x1: xb },
    ]
}

/// Along the middle from `xa` towards `xb`, bending down into `xb`'s lane, down to the bottom.
fn across_then_down(f: &Frame, xa: f32, xb: f32, r: f32) -> Vec<Shape> {
    let right = xb > xa;
    let cx = if right { xb - r } else { xb + r };
    let q = if right { Quarter::TopRight } else { Quarter::TopLeft };
    vec![
        Shape::H { y: f.mid, x0: xa, x1: cx },
        Shape::Arc { cx, cy: f.mid + r, r, q },
        Shape::V { x: xb, y0: f.mid + r, y1: f.bottom },
    ]
}

/// Every shape of a row, with its colour index.
pub fn row(f: &Frame, g: &GraphRow) -> Vec<(Shape, u16)> {
    let mut out = Vec::new();
    for e in &g.edges {
        for s in edge(f, g.node, e) {
            out.push((s, e.color));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Frame {
        Frame { x0: 10.0, lane: 20.0, top: 0.0, mid: 12.0, bottom: 24.0 }
    }

    fn e(from: u16, to: u16, kind: EdgeKind) -> Edge {
        Edge { from, to, kind, color: 0 }
    }

    #[test]
    fn straight_edges() {
        let f = frame();
        assert_eq!(edge(&f, 0, &e(1, 1, EdgeKind::Pass)), vec![Shape::V { x: 30.0, y0: 0.0, y1: 24.0 }]);
        assert_eq!(edge(&f, 0, &e(0, 0, EdgeKind::In)), vec![Shape::V { x: 10.0, y0: 0.0, y1: 12.0 }]);
        assert_eq!(edge(&f, 2, &e(2, 2, EdgeKind::Out)), vec![Shape::V { x: 50.0, y0: 12.0, y1: 24.0 }]);
    }

    #[test]
    fn an_edge_out_to_another_lane_bends_down() {
        // a merge's second parent: from the dot in lane 0 to lane 1
        let s = edge(&frame(), 0, &e(0, 1, EdgeKind::Out));
        let r = 10.0;
        assert_eq!(
            s,
            vec![
                Shape::H { y: 12.0, x0: 10.0, x1: 30.0 - r },
                Shape::Arc { cx: 30.0 - r, cy: 12.0 + r, r, q: Quarter::TopRight },
                Shape::V { x: 30.0, y0: 12.0 + r, y1: 24.0 },
            ]
        );
    }

    #[test]
    fn an_edge_into_the_dot_from_the_right_bends_left() {
        let s = edge(&frame(), 0, &e(2, 0, EdgeKind::In));
        // a wide move bends from the very top (the radius is half the row), then runs left into the dot
        match s.as_slice() {
            [Shape::Arc { cx, cy, r, q }, Shape::H { x0, x1, y }] => {
                assert_eq!((*cx + *r, *cy, *q), (50.0, 0.0, Quarter::BottomRight), "starts at lane 2's top");
                assert_eq!((*x0, *x1, *y), (*cx, 10.0, 12.0));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_passing_lane_that_moves_is_continuous() {
        let f = frame();
        let s = edge(&f, 0, &e(2, 1, EdgeKind::Pass));
        // starts at the top of lane 2, ends at the bottom of lane 1
        assert!(matches!(s.first(), Some(Shape::V { x, y0, .. }) if *x == 50.0 && *y0 == 0.0));
        assert!(matches!(s.last(), Some(Shape::V { x, y1, .. }) if *x == 30.0 && *y1 == 24.0));
        assert_eq!(s.iter().filter(|x| matches!(x, Shape::Arc { .. })).count(), 2);
    }
}

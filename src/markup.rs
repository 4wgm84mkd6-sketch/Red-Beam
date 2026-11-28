//! Markups (clouds, text, callouts, arrows, shapes, highlights, pen,
//! stamps): data model, hit testing, editing handles, and the drawing
//! primitives shared by the on-screen renderer and the PDF exporter.
//! Pure logic, testable anywhere. Coordinates are page DIPs.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MKind {
    /// Rectangular revision cloud (drag).
    Cloud,
    /// Revision cloud around clicked points.
    CloudPoly,
    Text,
    /// Text box with a leader to `points[0]`.
    Callout,
    Arrow,
    Line,
    Rect,
    Ellipse,
    Highlight,
    Pen,
    Stamp,
    /// Closed outline (imported from other apps).
    Polygon,
}

impl MKind {
    pub fn name(self) -> &'static str {
        match self {
            MKind::Cloud | MKind::CloudPoly => "Cloud",
            MKind::Text => "Text box",
            MKind::Callout => "Callout",
            MKind::Arrow => "Arrow",
            MKind::Line => "Line",
            MKind::Rect => "Rectangle",
            MKind::Ellipse => "Ellipse",
            MKind::Highlight => "Highlight",
            MKind::Pen => "Pen",
            MKind::Stamp => "Stamp",
            MKind::Polygon => "Polygon",
        }
    }
    /// Defined by two corner points (drag to create).
    pub fn is_box(self) -> bool {
        matches!(self, MKind::Cloud | MKind::Text | MKind::Rect | MKind::Ellipse | MKind::Highlight | MKind::Stamp)
    }
    pub fn has_text(self) -> bool {
        matches!(self, MKind::Text | MKind::Callout | MKind::Stamp)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Status {
    #[default]
    None,
    Open,
    Accepted,
    Rejected,
    Completed,
}

impl Status {
    pub const ALL: [Status; 5] = [Status::None, Status::Open, Status::Accepted, Status::Rejected, Status::Completed];
    pub fn name(self) -> &'static str {
        match self {
            Status::None => "No status",
            Status::Open => "Open",
            Status::Accepted => "Accepted",
            Status::Rejected => "Rejected",
            Status::Completed => "Completed",
        }
    }
}

pub const STAMPS: [&str; 9] = [
    "APPROVED",
    "APPROVED AS NOTED",
    "REVIEWED",
    "REVISE & RESUBMIT",
    "REJECTED",
    "FOR CONSTRUCTION",
    "NOT FOR CONSTRUCTION",
    "VOID",
    "DRAFT",
];

pub const COLORS: [(&str, u32); 7] = [
    ("Red", 0xe53935),
    ("Blue", 0x1e88e5),
    ("Green", 0x2e7d32),
    ("Orange", 0xf57c00),
    ("Magenta", 0xd81b60),
    ("Black", 0x212121),
    ("Yellow (highlight)", 0xffeb3b),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Markup {
    pub id: u64,
    pub sheet: u32,
    pub kind: MKind,
    pub points: Vec<[f32; 2]>,
    #[serde(default)]
    pub text: String,
    pub color: u32,
    /// Line width (paper DIPs).
    pub width: f32,
    /// Font size (paper DIPs).
    pub font: f32,
    #[serde(default)]
    pub status: Status,
    #[serde(default)]
    pub author: String,
    /// Seconds since 1970.
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    /// Text colour; `None` = the same as `color` (line/border).
    #[serde(default)]
    pub text_color: Option<u32>,
    /// Box fill of text boxes and callouts; `None` = white, `NO_FILL` = clear.
    #[serde(default)]
    pub fill: Option<u32>,
    /// Where an imported markup came from (the PDF annotation's id), so
    /// importing again doesn't duplicate it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// `Markup::fill` value for a see-through box.
pub const NO_FILL: u32 = u32::MAX;

/// Box fills offered in the format bar.
pub const FILLS: [(&str, u32); 6] =
    [("White", 0xffffff), ("None (see-through)", NO_FILL), ("Yellow", 0xfff59d), ("Light blue", 0xbbdefb), ("Light green", 0xc8e6c9), ("Light red", 0xffcdd2)];

/// Font sizes (points) that A+ / A- step through.
pub const FONT_STEPS_PT: [f32; 17] = [6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 32.0, 36.0, 48.0, 72.0];

/// Next font size (paper DIPs) up (`up`) or down from `font` (DIPs).
pub fn step_font(font: f32, up: bool) -> f32 {
    let pt = font * 0.75;
    let next = if up {
        FONT_STEPS_PT.iter().copied().find(|s| *s > pt + 0.01).unwrap_or(FONT_STEPS_PT[FONT_STEPS_PT.len() - 1])
    } else {
        FONT_STEPS_PT.iter().rev().copied().find(|s| *s < pt - 0.01).unwrap_or(FONT_STEPS_PT[0])
    };
    next / 0.75
}

/// A drawing primitive in page DIPs.
#[derive(Clone, Debug, PartialEq)]
pub enum Prim {
    /// One or more rings drawn as one path (inner rings become holes when
    /// filled).
    Path { rings: Vec<Vec<[f32; 2]>>, closed: bool, stroke: Option<(u32, f32)>, fill: Option<(u32, f32)> },
    /// Text laid out in a box (top-left origin), wrapped to its width.
    Text { rect: [f32; 4], text: String, size: f32, color: u32, bold: bool, italic: bool, center: bool },
}

fn bbox(pts: &[[f32; 2]]) -> [f32; 4] {
    let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for p in pts {
        b[0] = b[0].min(p[0]);
        b[1] = b[1].min(p[1]);
        b[2] = b[2].max(p[0]);
        b[3] = b[3].max(p[1]);
    }
    b
}

fn rect_ring(b: [f32; 4]) -> Vec<[f32; 2]> {
    vec![[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]]
}

fn ellipse_ring(b: [f32; 4]) -> Vec<[f32; 2]> {
    let (cx, cy, rx, ry) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0, (b[2] - b[0]) / 2.0, (b[3] - b[1]) / 2.0);
    (0..72)
        .map(|i| {
            let a = i as f32 / 72.0 * std::f32::consts::TAU;
            [cx + rx * a.cos(), cy + ry * a.sin()]
        })
        .collect()
}

/// Signed area (positive = clockwise in a y-down space).
fn signed_area(pts: &[[f32; 2]]) -> f32 {
    let mut a = 0.0;
    for i in 0..pts.len() {
        let (p, q) = (pts[i], pts[(i + 1) % pts.len()]);
        a += p[0] * q[1] - q[0] * p[1];
    }
    a / 2.0
}

/// Revision cloud: outward semicircular scallops of about `scallop` along
/// a closed polygon, as a polyline.
pub fn cloud_ring(poly: &[[f32; 2]], scallop: f32) -> Vec<[f32; 2]> {
    if poly.len() < 2 {
        return poly.to_vec();
    }
    // Walk clockwise (y down) so the bulge is on the outside.
    let mut pts = poly.to_vec();
    if signed_area(&pts) < 0.0 {
        pts.reverse();
    }
    let mut out = Vec::new();
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        if len < 1e-3 {
            continue;
        }
        let n = (len / scallop).round().max(1.0) as usize;
        let (ux, uy) = ((b[0] - a[0]) / len, (b[1] - a[1]) / len);
        // Outward normal for a clockwise ring in y-down coordinates.
        let (nx, ny) = (uy, -ux);
        let step = len / n as f32;
        for k in 0..n {
            let c = [a[0] + ux * step * (k as f32 + 0.5), a[1] + uy * step * (k as f32 + 0.5)];
            let r = step / 2.0;
            for j in 0..=8 {
                // Semicircle from the scallop start to its end, bulging out.
                let t = std::f32::consts::PI * (1.0 - j as f32 / 8.0);
                let (ct, st) = (t.cos(), t.sin());
                out.push([c[0] + (ux * ct + nx * st) * r, c[1] + (uy * ct + ny * st) * r]);
            }
        }
    }
    out
}

fn arrow_head(tip: [f32; 2], from: [f32; 2], size: f32) -> Vec<[f32; 2]> {
    let (dx, dy) = (tip[0] - from[0], tip[1] - from[1]);
    let len = dx.hypot(dy).max(1e-3);
    let (ux, uy) = (dx / len, dy / len);
    let (nx, ny) = (-uy, ux);
    let base = [tip[0] - ux * size, tip[1] - uy * size];
    vec![tip, [base[0] + nx * size * 0.4, base[1] + ny * size * 0.4], [base[0] - nx * size * 0.4, base[1] - ny * size * 0.4]]
}

impl Markup {
    /// Colour of the markup's text.
    pub fn text_rgb(&self) -> u32 {
        self.text_color.unwrap_or(self.color)
    }

    /// Fill of a text box / callout box.
    fn box_fill(&self, alpha: f32) -> Option<(u32, f32)> {
        match self.fill {
            None => Some((0xffffff, alpha)),
            Some(NO_FILL) => None,
            Some(c) => Some((c, alpha)),
        }
    }

    fn text_prim(&self, rect: [f32; 4]) -> Prim {
        Prim::Text { rect, text: self.text.clone(), size: self.font, color: self.text_rgb(), bold: self.bold, italic: self.italic, center: false }
    }

    /// Bounding box of the markup (page DIPs).
    pub fn bounds(&self) -> [f32; 4] {
        bbox(&self.points)
    }

    /// The callout's text box (points 1 and 2).
    fn callout_box(&self) -> [f32; 4] {
        bbox(&self.points[1..3.min(self.points.len())])
    }

    /// Where a callout's leader meets its box: the nearest edge midpoint.
    pub fn callout_anchor(&self) -> [f32; 2] {
        let b = self.callout_box();
        let tip = self.points[0];
        let mids = [[(b[0] + b[2]) / 2.0, b[1]], [(b[0] + b[2]) / 2.0, b[3]], [b[0], (b[1] + b[3]) / 2.0], [b[2], (b[1] + b[3]) / 2.0]];
        *mids
            .iter()
            .min_by(|a, c| {
                let da = (a[0] - tip[0]).hypot(a[1] - tip[1]);
                let dc = (c[0] - tip[0]).hypot(c[1] - tip[1]);
                da.total_cmp(&dc)
            })
            .unwrap()
    }

    /// Drawing primitives, shared by screen and PDF export.
    pub fn prims(&self) -> Vec<Prim> {
        let stroke = Some((self.color, self.width));
        let pad = self.font * 0.35;
        match self.kind {
            MKind::Rect => vec![Prim::Path { rings: vec![rect_ring(self.bounds())], closed: true, stroke, fill: None }],
            MKind::Ellipse => vec![Prim::Path { rings: vec![ellipse_ring(self.bounds())], closed: true, stroke, fill: None }],
            MKind::Cloud => {
                let ring = cloud_ring(&rect_ring(self.bounds()), (self.width * 7.0).max(10.0));
                vec![Prim::Path { rings: vec![ring], closed: true, stroke, fill: None }]
            }
            MKind::CloudPoly => {
                let ring = cloud_ring(&self.points, (self.width * 7.0).max(10.0));
                vec![Prim::Path { rings: vec![ring], closed: true, stroke, fill: None }]
            }
            MKind::Polygon => vec![Prim::Path { rings: vec![self.points.clone()], closed: true, stroke, fill: None }],
            MKind::Line | MKind::Pen => vec![Prim::Path { rings: vec![self.points.clone()], closed: false, stroke, fill: None }],
            MKind::Arrow => {
                let n = self.points.len();
                let mut v = vec![Prim::Path { rings: vec![self.points.clone()], closed: false, stroke, fill: None }];
                if n >= 2 {
                    let head = arrow_head(self.points[n - 1], self.points[n - 2], (self.width * 5.0).max(8.0));
                    v.push(Prim::Path { rings: vec![head], closed: true, stroke: None, fill: Some((self.color, 1.0)) });
                }
                v
            }
            MKind::Highlight => vec![Prim::Path { rings: vec![rect_ring(self.bounds())], closed: true, stroke: None, fill: Some((self.color, 0.35)) }],
            MKind::Text => {
                let b = self.bounds();
                vec![
                    Prim::Path { rings: vec![rect_ring(b)], closed: true, stroke: Some((self.color, (self.width * 0.5).max(0.75))), fill: self.box_fill(0.85) },
                    self.text_prim([b[0] + pad, b[1] + pad, b[2] - pad, b[3] - pad]),
                ]
            }
            MKind::Callout => {
                if self.points.len() < 3 {
                    return Vec::new();
                }
                let b = self.callout_box();
                let a = self.callout_anchor();
                let tip = self.points[0];
                vec![
                    Prim::Path { rings: vec![vec![a, tip]], closed: false, stroke, fill: None },
                    Prim::Path { rings: vec![arrow_head(tip, a, (self.width * 5.0).max(8.0))], closed: true, stroke: None, fill: Some((self.color, 1.0)) },
                    Prim::Path { rings: vec![rect_ring(b)], closed: true, stroke, fill: self.box_fill(0.9) },
                    self.text_prim([b[0] + pad, b[1] + pad, b[2] - pad, b[3] - pad]),
                ]
            }
            MKind::Stamp => {
                let b = self.bounds();
                let inset = (self.width * 2.0).max(3.0);
                let inner = [b[0] + inset, b[1] + inset, b[2] - inset, b[3] - inset];
                // Fit the text to the box: about 0.6 em per character.
                let chars = self.text.chars().count().max(1) as f32;
                let size = ((b[2] - b[0]) * 0.85 / (chars * 0.62)).min((b[3] - b[1]) * 0.55).max(4.0);
                vec![
                    Prim::Path { rings: vec![rect_ring(b)], closed: true, stroke: Some((self.color, self.width * 1.5)), fill: Some((0xffffff, 0.6)) },
                    Prim::Path { rings: vec![rect_ring(inner)], closed: true, stroke: Some((self.color, self.width * 0.6)), fill: None },
                    Prim::Text {
                        rect: [b[0], (b[1] + b[3]) / 2.0 - size * 0.62, b[2], (b[1] + b[3]) / 2.0 + size * 0.62],
                        text: self.text.clone(),
                        size,
                        color: self.text_rgb(),
                        bold: true,
                        italic: self.italic,
                        center: true,
                    },
                ]
            }
        }
    }

    /// Is `p` on this markup (page DIPs, tolerance `tol`)?
    pub fn hits(&self, p: [f32; 2], tol: f32) -> bool {
        let near = |pts: &[[f32; 2]], closed: bool| {
            let n = pts.len();
            let segs = if closed { n } else { n.saturating_sub(1) };
            (0..segs).any(|i| crate::takeoff::seg_dist2(p, pts[i], pts[(i + 1) % n]) <= tol * tol)
        };
        let inside = |b: [f32; 4]| p[0] >= b[0] - tol && p[0] <= b[2] + tol && p[1] >= b[1] - tol && p[1] <= b[3] + tol;
        match self.kind {
            MKind::Line | MKind::Arrow | MKind::Pen => near(&self.points, false),
            MKind::CloudPoly => crate::takeoff::point_in_polygon(p, &self.points) || near(&self.points, true),
            MKind::Polygon => near(&self.points, true),
            MKind::Callout => self.points.len() >= 3 && (inside(self.callout_box()) || near(&[self.points[0], self.callout_anchor()], false)),
            _ => inside(self.bounds()),
        }
    }

    /// Editing handles (page DIPs).
    pub fn handles(&self) -> Vec<[f32; 2]> {
        match self.kind {
            k if k.is_box() => rect_ring(self.bounds()),
            MKind::Callout if self.points.len() >= 3 => {
                let mut v = vec![self.points[0]];
                v.extend(rect_ring(self.callout_box()));
                v
            }
            MKind::Pen => Vec::new(),
            _ => self.points.clone(),
        }
    }

    /// Move handle `i` (as returned by `handles`) to `p`.
    pub fn set_handle(&mut self, i: usize, p: [f32; 2]) {
        // Corner k of a box: 0 top-left, 1 top-right, 2 bottom-right, 3 bottom-left.
        let set_corner = |pts: &mut [[f32; 2]], k: usize| {
            let b = bbox(pts);
            let (mut l, mut t, mut r, mut bo) = (b[0], b[1], b[2], b[3]);
            match k {
                0 => (l, t) = (p[0], p[1]),
                1 => (r, t) = (p[0], p[1]),
                2 => (r, bo) = (p[0], p[1]),
                _ => (l, bo) = (p[0], p[1]),
            }
            pts[0] = [l.min(r), t.min(bo)];
            pts[1] = [l.max(r), t.max(bo)];
        };
        match self.kind {
            k if k.is_box() && self.points.len() >= 2 => set_corner(&mut self.points[..2], i),
            MKind::Callout if self.points.len() >= 3 => {
                if i == 0 {
                    self.points[0] = p;
                } else {
                    set_corner(&mut self.points[1..3], i - 1);
                }
            }
            _ => {
                if let Some(q) = self.points.get_mut(i) {
                    *q = p;
                }
            }
        }
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        for p in &mut self.points {
            p[0] += dx;
            p[1] += dy;
        }
    }

    /// One-line summary for lists.
    pub fn summary(&self) -> String {
        let t = self.text.lines().next().unwrap_or("").trim();
        if t.is_empty() {
            self.kind.name().to_string()
        } else {
            format!("{}: {}", self.kind.name(), t)
        }
    }
}

/// Greedy word wrap with an approximate character width (for PDF export,
/// where real font metrics aren't available).
pub fn wrap(text: &str, size: f32, width: f32) -> Vec<String> {
    let max = ((width / (size * 0.52)).floor() as usize).max(1);
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > max {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(kind: MKind, points: Vec<[f32; 2]>) -> Markup {
        Markup {
            id: 1,
            sheet: 0,
            kind,
            points,
            text: "VERIFY IN FIELD".into(),
            color: 0xe53935,
            width: 2.0,
            font: 12.0,
            status: Status::Open,
            author: "x".into(),
            created: 0,
            bold: false,
            italic: false,
            text_color: None,
            fill: None,
            source: None,
        }
    }

    #[test]
    fn formatting() {
        // Font steps: 12pt (16 DIP) -> 14pt; 6pt floor; 72pt ceiling.
        assert!((step_font(16.0, true) * 0.75 - 14.0).abs() < 0.01);
        assert!((step_font(16.0, false) * 0.75 - 11.0).abs() < 0.01);
        assert!((step_font(8.0, false) * 0.75 - 6.0).abs() < 0.01);
        assert!((step_font(96.0, true) * 0.75 - 72.0).abs() < 0.01);
        let mut m = mk(MKind::Text, vec![[0.0, 0.0], [100.0, 40.0]]);
        m.bold = true;
        m.italic = true;
        m.text_color = Some(0x1e88e5);
        m.fill = Some(NO_FILL);
        let p = m.prims();
        assert!(matches!(&p[0], Prim::Path { fill: None, stroke: Some((0xe53935, _)), .. }));
        assert!(matches!(&p[1], Prim::Text { color: 0x1e88e5, bold: true, italic: true, .. }));
        // Old files without the new fields still load.
        let json = r#"{"id":1,"sheet":0,"kind":"Text","points":[[0,0],[1,1]],"color":0,"width":1,"font":12}"#;
        let old: Markup = serde_json::from_str(json).unwrap();
        assert!(!old.bold && old.fill.is_none() && old.text_rgb() == 0);
    }

    #[test]
    fn clouds_bulge_outward() {
        let ring = cloud_ring(&rect_ring([0.0, 0.0, 100.0, 50.0]), 10.0);
        let b = bbox(&ring);
        // Scallops stick out past the rectangle on every side, never inward much.
        assert!(b[0] < -3.0 && b[1] < -3.0 && b[2] > 103.0 && b[3] > 53.0, "{b:?}");
        assert!(b[0] > -8.0 && b[2] < 108.0);
        // Same result for a counter-clockwise input polygon.
        let mut ccw = rect_ring([0.0, 0.0, 100.0, 50.0]);
        ccw.reverse();
        assert_eq!(bbox(&cloud_ring(&ccw, 10.0)), b);
    }

    #[test]
    fn handles_and_hits() {
        let mut r = mk(MKind::Rect, vec![[10.0, 10.0], [110.0, 60.0]]);
        assert_eq!(r.handles().len(), 4);
        r.set_handle(2, [200.0, 100.0]);
        assert_eq!(r.bounds(), [10.0, 10.0, 200.0, 100.0]);
        // Dragging a corner past the opposite one flips cleanly.
        r.set_handle(0, [300.0, 300.0]);
        assert_eq!(r.bounds(), [200.0, 100.0, 300.0, 300.0]);
        assert!(r.hits([250.0, 200.0], 2.0) && !r.hits([150.0, 50.0], 2.0));
        let l = mk(MKind::Arrow, vec![[0.0, 0.0], [100.0, 0.0]]);
        assert!(l.hits([50.0, 1.5], 2.0) && !l.hits([50.0, 5.0], 2.0));
        let mut c = mk(MKind::Callout, vec![[0.0, 0.0], [100.0, 100.0], [200.0, 140.0]]);
        assert_eq!(c.callout_anchor(), [100.0, 120.0]);
        assert_eq!(c.handles().len(), 5);
        c.set_handle(0, [5.0, 5.0]);
        assert_eq!(c.points[0], [5.0, 5.0]);
        assert!(c.hits([150.0, 120.0], 1.0));
        assert_eq!(c.prims().len(), 4);
        assert_eq!(mk(MKind::Stamp, vec![[0.0, 0.0], [200.0, 60.0]]).prims().len(), 3);
    }

    #[test]
    fn wraps_text() {
        assert_eq!(wrap("VERIFY ALL DIMENSIONS IN FIELD", 10.0, 80.0), vec!["VERIFY ALL", "DIMENSIONS IN", "FIELD"]);
        assert_eq!(wrap("A\nB", 10.0, 100.0), vec!["A", "B"]);
    }
}

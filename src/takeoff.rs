//! Quantity takeoff model: sheet scales, measurements, units, persistence
//! and CSV export. Pure logic with no Windows dependencies so it can be
//! unit-tested anywhere.
//!
//! Geometry is stored in page DIPs (1/96 paper inch), the same space the
//! viewer uses. Real-world quantities are computed live from each sheet's
//! scale, so fixing a calibration updates every measurement on that sheet.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Paper DIPs per paper inch.
const DIP_PER_IN: f64 = 96.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Units {
    /// 12'-6 1/4"  and  SF
    #[default]
    FeetInches,
    /// 12.52'  and  SF
    DecimalFeet,
    /// 3.815 m  and  m²
    Metric,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Length,
    Polyline,
    Area,
    Count,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Length => "Length",
            Kind::Polyline => "Polyline",
            Kind::Area => "Area",
            Kind::Count => "Count",
        }
    }
    /// Points needed before the measurement can be finished.
    pub fn min_points(self) -> usize {
        match self {
            Kind::Length | Kind::Polyline => 2,
            Kind::Area => 3,
            Kind::Count => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scale {
    /// Real-world inches per page DIP.
    pub real_in_per_dip: f64,
    /// How it was set, e.g. `1/8" = 1'-0"` or `Calibrated`.
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub name: String,
    /// 0xRRGGBB
    pub color: u32,
    #[serde(default)]
    pub prices: Prices,
}

/// Unit prices for a group, in dollars per LF / SF / CY / EA. Wall areas
/// are priced per SF.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Prices {
    pub per_lf: Option<f64>,
    pub per_sf: Option<f64>,
    pub per_cy: Option<f64>,
    pub per_ea: Option<f64>,
}

/// Totals for a set of measurements (real inches, sq in, cu in).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Totals {
    pub len: f64,
    pub area: f64,
    pub perimeter: f64,
    pub wall: f64,
    pub vol: f64,
    pub count: usize,
    pub cost: f64,
    pub priced: bool,
    pub items: usize,
    pub unscaled: usize,
    pub sheets: std::collections::BTreeSet<u32>,
}

/// Arrowheads / caps for line ends and label leaders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum EndStyle {
    #[default]
    None,
    ClosedArrow,
    OpenArrow,
    /// Architectural slash.
    Tick,
    Dot,
    Square,
    Circle,
}

impl EndStyle {
    pub const ALL: [EndStyle; 7] = [
        EndStyle::None,
        EndStyle::ClosedArrow,
        EndStyle::OpenArrow,
        EndStyle::Tick,
        EndStyle::Dot,
        EndStyle::Square,
        EndStyle::Circle,
    ];
    pub fn name(self) -> &'static str {
        match self {
            EndStyle::None => "None",
            EndStyle::ClosedArrow => "Closed arrow",
            EndStyle::OpenArrow => "Open arrow",
            EndStyle::Tick => "Architectural tick",
            EndStyle::Dot => "Dot",
            EndStyle::Square => "Square",
            EndStyle::Circle => "Circle",
        }
    }
}

pub const DEFAULT_LAYER: &str = "Default";

fn default_layer() -> String {
    DEFAULT_LAYER.to_string()
}

fn default_ends() -> EndStyle {
    EndStyle::ClosedArrow
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Measure {
    pub id: u64,
    /// 0-based sheet index.
    pub sheet: u32,
    pub kind: Kind,
    /// Page DIPs.
    pub points: Vec<[f32; 2]>,
    pub group: String,
    #[serde(default = "default_layer")]
    pub layer: String,
    /// Caps on both ends of length / polyline measurements.
    #[serde(default)]
    pub ends: EndStyle,
    /// Label moved away from its default spot (page DIPs, relative to it).
    #[serde(default)]
    pub label_offset: Option<[f32; 2]>,
    /// Arrowhead where the leader meets the measurement.
    #[serde(default)]
    pub leader: EndStyle,
    /// Cut-outs subtracted from an area (each a polygon, page DIPs).
    #[serde(default)]
    pub holes: Vec<Vec<[f32; 2]>>,
    /// Depth / thickness for volume (real inches), areas only.
    #[serde(default)]
    pub depth_in: Option<f64>,
    /// Slope as rise per 12 of run (roofs, ramps): lengths and areas are
    /// measured in plan and multiplied up to their true sloped size.
    #[serde(default)]
    pub slope: Option<f64>,
    /// Wall height (real inches) for lengths: adds a wall area.
    #[serde(default)]
    pub height_in: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub name: String,
    pub visible: bool,
    pub locked: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Takeoff {
    pub version: u32,
    pub units: Units,
    /// Scale per sheet (0-based index).
    pub scales: BTreeMap<u32, Scale>,
    pub groups: Vec<Group>,
    pub items: Vec<Measure>,
    pub next_id: u64,
    #[serde(default)]
    pub layers: Vec<Layer>,
    /// Style for new length / polyline measurements (last one chosen).
    #[serde(default = "default_ends")]
    pub default_ends: EndStyle,
    /// Leader style for labels moved off their measurement.
    #[serde(default = "default_ends")]
    pub default_leader: EndStyle,
    /// Markups (clouds, text, callouts, ...) on this drawing.
    #[serde(default)]
    pub markups: Vec<crate::markup::Markup>,
}

/// Distinct, print-friendly colours handed to new groups in turn.
pub const PALETTE: [u32; 10] = [
    0xd32f2f, 0x1976d2, 0x388e3c, 0xf57c00, 0x7b1fa2, 0x00838f, 0xc2185b, 0x5d4037, 0x689f38, 0x455a64,
];

/// Common drawing scales: (label, real inches per paper inch).
pub const SCALES: [(&str, f64); 20] = [
    ("1/32\" = 1'-0\"", 384.0),
    ("1/16\" = 1'-0\"", 192.0),
    ("3/32\" = 1'-0\"", 128.0),
    ("1/8\" = 1'-0\"", 96.0),
    ("3/16\" = 1'-0\"", 64.0),
    ("1/4\" = 1'-0\"", 48.0),
    ("3/8\" = 1'-0\"", 32.0),
    ("1/2\" = 1'-0\"", 24.0),
    ("3/4\" = 1'-0\"", 16.0),
    ("1\" = 1'-0\"", 12.0),
    ("1 1/2\" = 1'-0\"", 8.0),
    ("3\" = 1'-0\"", 4.0),
    ("1\" = 10'", 120.0),
    ("1\" = 20'", 240.0),
    ("1\" = 30'", 360.0),
    ("1\" = 40'", 480.0),
    ("1\" = 50'", 600.0),
    ("1\" = 60'", 720.0),
    ("1\" = 100'", 1200.0),
    ("1:100 (metric)", 100.0),
];

impl Scale {
    pub fn from_ratio(label: &str, real_per_paper: f64) -> Scale {
        Scale { real_in_per_dip: real_per_paper / DIP_PER_IN, label: label.to_string() }
    }
    /// From two points `dip_len` apart that measure `real_in` in reality.
    pub fn calibrated(dip_len: f64, real_in: f64) -> Option<Scale> {
        (dip_len > 0.0 && real_in > 0.0).then(|| Scale { real_in_per_dip: real_in / dip_len, label: "Calibrated".into() })
    }
}

// ------------------------------------------------------------------ geometry

pub fn dist(a: [f32; 2], b: [f32; 2]) -> f64 {
    ((a[0] - b[0]) as f64).hypot((a[1] - b[1]) as f64)
}

/// Total length along the points (page DIPs).
pub fn path_len(pts: &[[f32; 2]]) -> f64 {
    pts.windows(2).map(|w| dist(w[0], w[1])).sum()
}

/// Closed-polygon perimeter (page DIPs).
pub fn perimeter(pts: &[[f32; 2]]) -> f64 {
    if pts.len() < 2 {
        return 0.0;
    }
    path_len(pts) + dist(pts[pts.len() - 1], pts[0])
}

/// Polygon area by the shoelace formula (page DIPs²).
pub fn area(pts: &[[f32; 2]]) -> f64 {
    if pts.len() < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    for i in 0..pts.len() {
        let (p, q) = (pts[i], pts[(i + 1) % pts.len()]);
        a += p[0] as f64 * q[1] as f64 - q[0] as f64 * p[1] as f64;
    }
    a.abs() * 0.5
}

/// Area-weighted centroid, for placing the label (falls back to the mean).
pub fn centroid(pts: &[[f32; 2]]) -> [f32; 2] {
    let n = pts.len();
    if n == 0 {
        return [0.0, 0.0];
    }
    let mean = [
        pts.iter().map(|p| p[0]).sum::<f32>() / n as f32,
        pts.iter().map(|p| p[1]).sum::<f32>() / n as f32,
    ];
    if n < 3 {
        return mean;
    }
    let (mut a, mut cx, mut cy) = (0.0f64, 0.0f64, 0.0f64);
    for i in 0..n {
        let (p, q) = (pts[i], pts[(i + 1) % n]);
        let c = p[0] as f64 * q[1] as f64 - q[0] as f64 * p[1] as f64;
        a += c;
        cx += (p[0] + q[0]) as f64 * c;
        cy += (p[1] + q[1]) as f64 * c;
    }
    if a.abs() < 1e-9 {
        return mean;
    }
    [(cx / (3.0 * a)) as f32, (cy / (3.0 * a)) as f32]
}

/// Constrain `p` relative to `from` to horizontal, vertical or 45°.
pub fn ortho(from: [f32; 2], p: [f32; 2]) -> [f32; 2] {
    let (dx, dy) = (p[0] - from[0], p[1] - from[1]);
    let ang = dy.atan2(dx);
    let snapped = (ang / std::f32::consts::FRAC_PI_4).round() * std::f32::consts::FRAC_PI_4;
    let len = dx.hypot(dy);
    // Project onto the snapped direction.
    let (ux, uy) = (snapped.cos(), snapped.sin());
    let along = (dx * ux + dy * uy).max(0.0).min(len);
    [from[0] + ux * along, from[1] + uy * along]
}

/// Squared distance from `p` to segment `a`–`b`.
pub fn seg_dist2(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (vx, vy) = (b[0] - a[0], b[1] - a[1]);
    let (wx, wy) = (p[0] - a[0], p[1] - a[1]);
    let l2 = vx * vx + vy * vy;
    let t = if l2 > 0.0 { ((wx * vx + wy * vy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let (dx, dy) = (a[0] + t * vx - p[0], a[1] + t * vy - p[1]);
    dx * dx + dy * dy
}

pub fn point_in_polygon(p: [f32; 2], pts: &[[f32; 2]]) -> bool {
    let mut inside = false;
    let n = pts.len();
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let (a, b) = (pts[i], pts[j]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
        j = i;
    }
    inside
}

// --------------------------------------------------------------- quantities

/// Real-world quantity of a measurement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Qty {
    /// Real inches.
    Length(f64),
    /// Net real square inches (cut-outs subtracted), perimeter in real
    /// inches including cut-out edges, and volume in real cubic inches when
    /// a depth is set.
    Area { area: f64, perimeter: f64, volume: Option<f64> },
    Count(usize),
    /// Sheet has no scale yet.
    Unscaled,
}

impl Measure {
    pub fn qty(&self, scale: Option<&Scale>) -> Qty {
        if self.kind == Kind::Count {
            return Qty::Count(self.points.len());
        }
        let Some(s) = scale else { return Qty::Unscaled };
        let k = s.real_in_per_dip;
        match self.kind {
            Kind::Length | Kind::Polyline => Qty::Length(path_len(&self.points) * k * self.slope_factor()),
            Kind::Area => {
                let net = (area(&self.points) - self.holes.iter().map(|h| area(h)).sum::<f64>()).max(0.0)
                    * k
                    * k
                    * self.slope_factor();
                let per = (perimeter(&self.points) + self.holes.iter().map(|h| perimeter(h)).sum::<f64>()) * k;
                Qty::Area { area: net, perimeter: per, volume: self.depth_in.map(|d| net * d) }
            }
            Kind::Count => unreachable!(),
        }
    }

    /// True-length multiplier for a slope given as rise per 12.
    pub fn slope_factor(&self) -> f64 {
        self.slope.map(|r| (1.0 + (r / 12.0).powi(2)).sqrt()).unwrap_or(1.0)
    }

    /// Wall area (real sq in) for a length with a wall height.
    pub fn wall_area(&self, scale: Option<&Scale>) -> Option<f64> {
        match (self.qty(scale), self.height_in) {
            (Qty::Length(l), Some(h)) => Some(l * h),
            _ => None,
        }
    }

    /// Where the label is drawn (page DIPs), honouring a moved label.
    pub fn label_pos(&self) -> [f32; 2] {
        let a = self.anchor();
        match self.label_offset {
            Some(o) => [a[0] + o[0], a[1] + o[1]],
            None => a,
        }
    }

    /// Default label spot, and where a leader points to (page DIPs).
    pub fn anchor(&self) -> [f32; 2] {
        match self.kind {
            Kind::Area => centroid(&self.points),
            Kind::Count => self.points.first().copied().unwrap_or([0.0, 0.0]),
            Kind::Length | Kind::Polyline => {
                // Midpoint along the path.
                let half = path_len(&self.points) / 2.0;
                let mut acc = 0.0;
                for w in self.points.windows(2) {
                    let d = dist(w[0], w[1]);
                    if acc + d >= half && d > 0.0 {
                        let t = ((half - acc) / d) as f32;
                        return [w[0][0] + (w[1][0] - w[0][0]) * t, w[0][1] + (w[1][1] - w[0][1]) * t];
                    }
                    acc += d;
                }
                self.points.first().copied().unwrap_or([0.0, 0.0])
            }
        }
    }
}

// -------------------------------------------------------------- formatting

/// Format real inches.
pub fn fmt_len(inches: f64, units: Units) -> String {
    match units {
        Units::FeetInches => {
            let quarters = (inches * 4.0).round() as i64;
            let neg = quarters < 0;
            let q = quarters.abs();
            let feet = q / 48;
            let rem = q % 48;
            let whole = rem / 4;
            let frac = match rem % 4 {
                1 => " 1/4",
                2 => " 1/2",
                3 => " 3/4",
                _ => "",
            };
            format!("{}{feet}'-{whole}{frac}\"", if neg { "-" } else { "" })
        }
        Units::DecimalFeet => format!("{}'", group_thousands(inches / 12.0, 2)),
        Units::Metric => format!("{} m", group_thousands(inches * 0.0254, 3)),
    }
}

/// Format real square inches.
pub fn fmt_area(sq_in: f64, units: Units) -> String {
    match units {
        Units::FeetInches | Units::DecimalFeet => format!("{} SF", group_thousands(sq_in / 144.0, 1)),
        Units::Metric => format!("{} m\u{00B2}", group_thousands(sq_in * 0.0254 * 0.0254, 2)),
    }
}

/// Format real cubic inches: cubic yards (concrete, fill) or m³.
pub fn fmt_volume(cu_in: f64, units: Units) -> String {
    match units {
        Units::FeetInches | Units::DecimalFeet => format!("{} CY", group_thousands(cu_in / 46_656.0, 2)),
        Units::Metric => format!("{} m\u{00B3}", group_thousands(cu_in * 0.0254f64.powi(3), 3)),
    }
}

pub fn fmt_qty(q: Qty, units: Units) -> String {
    match q {
        Qty::Length(l) => fmt_len(l, units),
        Qty::Area { area, volume: Some(v), .. } => format!("{} \u{00B7} {}", fmt_area(area, units), fmt_volume(v, units)),
        Qty::Area { area, .. } => fmt_area(area, units),
        Qty::Count(n) => format!("{n} ea"),
        Qty::Unscaled => "no scale".into(),
    }
}

/// `1234567.891` with 2 decimals → `1,234,567.89`.
pub fn group_thousands(v: f64, decimals: usize) -> String {
    let s = format!("{:.*}", decimals, v.abs());
    let (int, frac) = s.split_once('.').map(|(a, b)| (a, Some(b))).unwrap_or((&s, None));
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if let Some(f) = frac {
        out.push('.');
        out.push_str(f);
    }
    if v < 0.0 && out.chars().any(|c| c.is_ascii_digit() && c != '0') {
        out.insert(0, '-');
    }
    out
}

// ------------------------------------------------------------------ parsing

fn parse_num(s: &str) -> Option<f64> {
    // "6", "6.5", "1/2", "6 1/2", "6-1/2"
    let s = s.trim();
    if s.is_empty() {
        return Some(0.0);
    }
    let frac = |t: &str| -> Option<f64> {
        match t.split_once('/') {
            Some((a, b)) => {
                let (a, b) = (a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?);
                (b != 0.0).then(|| a / b)
            }
            None => t.trim().parse::<f64>().ok(),
        }
    };
    if s.contains('/') {
        let normal = s.replace('-', " ");
        let parts: Vec<&str> = normal.split_whitespace().collect();
        return match parts.as_slice() {
            [f] => frac(f),
            [w, f] => Some(w.parse::<f64>().ok()? + frac(f)?),
            _ => None,
        };
    }
    s.parse::<f64>().ok()
}

/// Parse a typed length into real inches. Accepts e.g. `12'-6"`, `12' 6 1/2"`,
/// `12.5'`, `150"`, `12 ft 6 in`, `3.5m`, `350 mm`, `35cm`; a bare number is feet.
pub fn parse_len(input: &str) -> Option<f64> {
    let s = input.trim().to_lowercase().replace(['\u{2019}', '\u{2032}'], "'").replace(['\u{201D}', '\u{2033}'], "\"");
    if s.is_empty() {
        return None;
    }
    for (suffix, mm) in [("mm", 1.0), ("cm", 10.0), ("m", 1000.0)] {
        if let Some(n) = s.strip_suffix(suffix) {
            if let Ok(v) = n.trim().parse::<f64>() {
                return (v > 0.0).then_some(v * mm / 25.4);
            }
        }
    }
    let s = s.replace("feet", "'").replace("foot", "'").replace("ft", "'").replace("inches", "\"").replace("inch", "\"").replace("in", "\"");
    let v = if let Some((ft, rest)) = s.split_once('\'') {
        let feet = parse_num(ft)?;
        let rest = rest.trim().trim_start_matches('-').trim().trim_end_matches('"').trim();
        feet * 12.0 + parse_num(rest)?
    } else if let Some(i) = s.strip_suffix('"') {
        parse_num(i)?
    } else {
        parse_num(&s)? * 12.0
    };
    (v > 0.0 && v.is_finite()).then_some(v)
}

/// Parse a slope into rise per 12 of run: `6` or `6:12` or `6/12` (pitch),
/// `4:12`, `1:20` (ramp), `26.57°` / `26.57 deg`, or `8.33%`.
pub fn parse_slope(input: &str) -> Option<f64> {
    let s = input.trim().to_lowercase().replace(' ', "");
    if s.is_empty() {
        return None;
    }
    let v = if let Some(d) = s.strip_suffix('\u{00B0}').or_else(|| s.strip_suffix("deg")) {
        let deg: f64 = d.parse().ok()?;
        (deg > 0.0 && deg < 89.0).then(|| deg.to_radians().tan() * 12.0)?
    } else if let Some(p) = s.strip_suffix('%') {
        p.parse::<f64>().ok()? / 100.0 * 12.0
    } else if let Some((a, b)) = s.split_once([':', '/']) {
        let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
        (b > 0.0).then(|| a / b * 12.0)?
    } else {
        s.parse().ok()?
    };
    (v > 0.0 && v.is_finite()).then_some(v)
}

/// `$1,234.56`
pub fn fmt_money(v: f64) -> String {
    format!("${}", group_thousands(v, 2))
}

/// Slope as pitch text, e.g. `6:12`.
pub fn fmt_slope(rise: f64) -> String {
    let r = (rise * 100.0).round() / 100.0;
    if (r - r.round()).abs() < 1e-9 {
        format!("{}:12", r.round())
    } else {
        format!("{r}:12")
    }
}

// -------------------------------------------------------------- persistence

impl Takeoff {
    pub fn new() -> Takeoff {
        Takeoff {
            version: 2,
            default_ends: default_ends(),
            default_leader: default_ends(),
            ..Default::default()
        }
    }

    pub fn scale(&self, sheet: u32) -> Option<&Scale> {
        self.scales.get(&sheet)
    }

    pub fn group(&self, name: &str) -> Option<&Group> {
        self.groups.iter().find(|g| g.name == name)
    }

    /// Cost of one measurement from its group's unit prices, if priced.
    pub fn cost(&self, m: &Measure) -> Option<f64> {
        let p = self.group(&m.group)?.prices;
        let scale = self.scale(m.sheet);
        let mut cost = 0.0;
        let mut priced = false;
        let mut add = |qty: f64, price: Option<f64>| {
            if let Some(pr) = price {
                cost += qty * pr;
                priced = true;
            }
        };
        match m.qty(scale) {
            Qty::Length(l) => add(l / 12.0, p.per_lf),
            Qty::Area { area, volume, .. } => {
                add(area / 144.0, p.per_sf);
                if let Some(v) = volume {
                    add(v / 46_656.0, p.per_cy);
                }
            }
            Qty::Count(n) => add(n as f64, p.per_ea),
            Qty::Unscaled => {}
        }
        if let Some(w) = m.wall_area(scale) {
            add(w / 144.0, p.per_sf);
        }
        priced.then_some(cost)
    }

    /// Add one measurement into running totals.
    pub fn accumulate(&self, t: &mut Totals, m: &Measure) {
        let scale = self.scale(m.sheet);
        t.items += 1;
        t.sheets.insert(m.sheet);
        match m.qty(scale) {
            Qty::Length(l) => t.len += l,
            Qty::Area { area, perimeter, volume } => {
                t.area += area;
                t.perimeter += perimeter;
                t.vol += volume.unwrap_or(0.0);
            }
            Qty::Count(n) => t.count += n,
            Qty::Unscaled => t.unscaled += 1,
        }
        t.wall += m.wall_area(scale).unwrap_or(0.0);
        if let Some(c) = self.cost(m) {
            t.cost += c;
            t.priced = true;
        }
    }

    /// Totals per group over visible layers, in group order (groups with
    /// no visible items are skipped).
    pub fn group_totals(&self) -> Vec<(&Group, Totals)> {
        self.groups
            .iter()
            .filter_map(|g| {
                let mut t = Totals::default();
                for m in self.visible_items().filter(|m| m.group == g.name) {
                    self.accumulate(&mut t, m);
                }
                (t.items > 0).then_some((g, t))
            })
            .collect()
    }

    /// Totals per (sheet, group) over visible layers.
    pub fn sheet_totals(&self) -> BTreeMap<(u32, String), Totals> {
        let mut out: BTreeMap<(u32, String), Totals> = BTreeMap::new();
        for m in self.visible_items() {
            let t = out.entry((m.sheet, m.group.clone())).or_default();
            self.accumulate(t, m);
        }
        out
    }

    pub fn group_color(&self, name: &str) -> u32 {
        self.groups.iter().find(|g| g.name == name).map(|g| g.color).unwrap_or(0x1976d2)
    }

    /// Add a group if it doesn't exist; returns its name.
    pub fn ensure_group(&mut self, name: &str) -> String {
        let name = name.trim();
        let name = if name.is_empty() { "General" } else { name };
        if !self.groups.iter().any(|g| g.name == name) {
            let color = PALETTE[self.groups.len() % PALETTE.len()];
            self.groups.push(Group { name: name.to_string(), color, prices: Prices::default() });
        }
        name.to_string()
    }

    /// Add a layer if it doesn't exist; returns its name.
    pub fn ensure_layer(&mut self, name: &str) -> String {
        let name = name.trim();
        let name = if name.is_empty() { DEFAULT_LAYER } else { name };
        if !self.layers.iter().any(|l| l.name == name) {
            self.layers.push(Layer { name: name.to_string(), visible: true, locked: false });
        }
        name.to_string()
    }

    pub fn layer_visible(&self, name: &str) -> bool {
        self.layers.iter().find(|l| l.name == name).is_none_or(|l| l.visible)
    }

    pub fn layer_locked(&self, name: &str) -> bool {
        self.layers.iter().find(|l| l.name == name).is_some_and(|l| l.locked)
    }

    /// Items that count toward quantities (on visible layers).
    pub fn visible_items(&self) -> impl Iterator<Item = &Measure> {
        self.items.iter().filter(|m| self.layer_visible(&m.layer))
    }

    pub fn add(&mut self, sheet: u32, kind: Kind, points: Vec<[f32; 2]>, group: &str, layer: &str) -> u64 {
        let group = self.ensure_group(group);
        let layer = self.ensure_layer(layer);
        self.next_id += 1;
        let id = self.next_id;
        let ends = if matches!(kind, Kind::Length | Kind::Polyline) { self.default_ends } else { EndStyle::None };
        let leader = self.default_leader;
        self.items.push(Measure {
            id,
            sheet,
            kind,
            points,
            group,
            layer,
            ends,
            label_offset: None,
            leader,
            holes: Vec::new(),
            depth_in: None,
            slope: None,
            height_in: None,
        });
        id
    }

    /// Copy of an existing measurement with a new id.
    pub fn duplicate(&mut self, m: &Measure, sheet: u32, offset: [f32; 2]) -> u64 {
        self.next_id += 1;
        let mut c = m.clone();
        c.id = self.next_id;
        c.sheet = sheet;
        for p in c.points.iter_mut().chain(c.holes.iter_mut().flatten()) {
            p[0] += offset[0];
            p[1] += offset[1];
        }
        self.ensure_group(&c.group);
        self.ensure_layer(&c.layer);
        self.items.push(c);
        self.next_id
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Measure> {
        self.items.iter_mut().find(|m| m.id == id)
    }

    /// Where a drawing's takeoff is stored: next to the PDF.
    pub fn sidecar_path(pdf: &Path) -> PathBuf {
        let stem = pdf.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "drawing".into());
        pdf.with_file_name(format!("{stem}.redbeam.json"))
    }

    /// Fallback location when the PDF's folder isn't writable.
    pub fn fallback_path(pdf: &Path, appdata: &Path) -> PathBuf {
        let key = pdf.to_string_lossy().bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
        let stem = pdf.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        appdata.join("takeoffs").join(format!("{stem}-{key:016x}.redbeam.json"))
    }

    pub fn load(path: &Path) -> Option<Takeoff> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Write atomically (temp file + rename) so a crash never leaves a
    /// half-written takeoff.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }

    /// Numbers in the takeoff's units for export: (length, area, volume)
    /// converters and their unit names.
    pub fn export_units(&self) -> ExportUnits {
        match self.units {
            Units::Metric => ExportUnits {
                len: 0.0254,
                area: 0.0254 * 0.0254,
                vol: 0.0254f64.powi(3),
                len_unit: "m",
                area_unit: "m2",
                vol_unit: "m3",
                decimals: 3,
            },
            _ => ExportUnits {
                len: 1.0 / 12.0,
                area: 1.0 / 144.0,
                vol: 1.0 / 46_656.0,
                len_unit: "LF",
                area_unit: "SF",
                vol_unit: "CY",
                decimals: 2,
            },
        }
    }

    /// Spreadsheet export: one row per measurement plus group totals.
    pub fn to_csv(&self, sheet_names: &dyn Fn(u32) -> String) -> String {
        let u = self.export_units();
        let n = |v: f64| format!("{:.*}", u.decimals, v);
        let esc = |s: &str| {
            if s.contains([',', '"', '\n']) {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.to_string()
            }
        };
        let mut out = String::from(
            "Group,Layer,Sheet,Type,Quantity,Unit,Perimeter,Perimeter unit,Volume,Volume unit,Wall area,Wall area unit,Slope,Cost,Display,Scale,Counted\n",
        );
        let mut items: Vec<&Measure> = self.items.iter().collect();
        items.sort_by(|a, b| (&a.group, a.sheet, a.id).cmp(&(&b.group, b.sheet, b.id)));
        for m in &items {
            let scale = self.scale(m.sheet);
            let q = m.qty(scale);
            let counted = self.layer_visible(&m.layer);
            let none = || (String::new(), "");
            let ((qty, unit), (per, per_unit), (vol, vol_unit)) = match q {
                Qty::Length(l) => ((n(l * u.len), u.len_unit), none(), none()),
                Qty::Area { area, perimeter, volume } => (
                    (n(area * u.area), u.area_unit),
                    (n(perimeter * u.len), u.len_unit),
                    volume.map(|v| (n(v * u.vol), u.vol_unit)).unwrap_or_else(none),
                ),
                Qty::Count(c) => ((c.to_string(), "EA"), none(), none()),
                Qty::Unscaled => ((String::new(), "no scale"), none(), none()),
            };
            let (wall, wall_unit) = match m.wall_area(scale) {
                Some(w) => (n(w * u.area), u.area_unit),
                None => (String::new(), ""),
            };
            out.push_str(&format!(
                "{},{},{},{},{qty},{unit},{per},{per_unit},{vol},{vol_unit},{wall},{wall_unit},{},{},{},{},{}\n",
                esc(&m.group),
                esc(&m.layer),
                esc(&sheet_names(m.sheet)),
                m.kind.name(),
                m.slope.map(fmt_slope).unwrap_or_default(),
                self.cost(m).map(|c| format!("{c:.2}")).unwrap_or_default(),
                esc(&fmt_qty(q, self.units)),
                esc(&scale.map(|s| s.label.clone()).unwrap_or_default()),
                if counted { "yes" } else { "no (hidden layer)" }
            ));
        }
        out.push_str(&format!(
            "\nTOTALS (visible layers)\nGroup,Length ({0}),Area ({1}),Perimeter ({0}),Wall area ({1}),Volume ({2}),Count (EA),Cost\n",
            u.len_unit, u.area_unit, u.vol_unit
        ));
        let mut grand = 0.0;
        for (g, t) in self.group_totals() {
            grand += t.cost;
            out.push_str(&format!(
                "{},{},{},{},{},{},{},{}\n",
                esc(&g.name),
                n(t.len * u.len),
                n(t.area * u.area),
                n(t.perimeter * u.len),
                n(t.wall * u.area),
                n(t.vol * u.vol),
                t.count,
                if t.priced { format!("{:.2}", t.cost) } else { String::new() }
            ));
        }
        out.push_str(&format!("GRAND TOTAL,,,,,,,{grand:.2}\n"));
        out
    }
}

/// Unit conversion from real inches for exports.
pub struct ExportUnits {
    pub len: f64,
    pub area: f64,
    pub vol: f64,
    pub len_unit: &'static str,
    pub area_unit: &'static str,
    pub vol_unit: &'static str,
    pub decimals: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lengths() {
        let cases = [
            ("12'-6\"", 150.0),
            ("12' 6\"", 150.0),
            ("12'6", 150.0),
            ("12'", 144.0),
            ("6\"", 6.0),
            ("12'-6 1/2\"", 150.5),
            ("12' 6-1/2\"", 150.5),
            ("12.5'", 150.0),
            ("12.5", 150.0),
            ("150in", 150.0),
            ("12 ft 6 in", 150.0),
            ("1/2\"", 0.5),
            ("25.4mm", 1.0),
            ("2.54 cm", 1.0),
            ("1m", 1000.0 / 25.4),
        ];
        for (s, want) in cases {
            let got = parse_len(s).unwrap_or_else(|| panic!("failed to parse {s}"));
            assert!((got - want).abs() < 1e-9, "{s}: got {got}, want {want}");
        }
        for bad in ["", "abc", "0", "-3'", "1/0\""] {
            assert!(parse_len(bad).is_none(), "{bad} should not parse");
        }
    }

    #[test]
    fn slopes() {
        for (s, want) in [("6", 6.0), ("6:12", 6.0), ("6/12", 6.0), ("1:20", 0.6), ("8.333%", 0.99996), ("45\u{00B0}", 12.0), ("45deg", 12.0)] {
            let got = parse_slope(s).unwrap_or_else(|| panic!("{s}"));
            assert!((got - want).abs() < 1e-3, "{s}: {got}");
        }
        assert!(parse_slope("").is_none() && parse_slope("x").is_none() && parse_slope("0").is_none());
        assert_eq!(fmt_slope(6.0), "6:12");
        assert_eq!(fmt_money(1234.5), "$1,234.50");
        // A 6:12 roof: plan 100 SF -> 111.8 SF.
        let s = Scale::from_ratio("1/8\" = 1'-0\"", 96.0);
        let mut t = Takeoff::new();
        // 120 DIP = 120" = 10' at 1/8" scale, so the plan area is 100 SF.
        t.add(0, Kind::Area, vec![[0.0, 0.0], [120.0, 0.0], [120.0, 120.0], [0.0, 120.0]], "Roof", "Default");
        t.scales.insert(0, s.clone());
        t.items[0].slope = Some(6.0);
        match t.items[0].qty(Some(&s)) {
            Qty::Area { area, .. } => assert!((area / 144.0 - 100.0 * 1.25f64.sqrt()).abs() < 1e-6),
            q => panic!("{q:?}"),
        }
    }

    #[test]
    fn formats() {
        assert_eq!(fmt_len(150.0, Units::FeetInches), "12'-6\"");
        assert_eq!(fmt_len(150.26, Units::FeetInches), "12'-6 1/4\"");
        assert_eq!(fmt_len(11.9, Units::FeetInches), "1'-0\"");
        assert_eq!(fmt_len(150.0, Units::DecimalFeet), "12.50'");
        assert_eq!(fmt_len(1000.0 / 0.0254 / 1000.0 * 1000.0, Units::Metric), "1,000.000 m");
        assert_eq!(fmt_area(144.0 * 1234.56, Units::FeetInches), "1,234.6 SF");
        assert_eq!(group_thousands(1234567.891, 2), "1,234,567.89");
        assert_eq!(group_thousands(999.0, 0), "999");
    }

    #[test]
    fn geometry_and_scale() {
        // 1/8" = 1'-0": one paper inch (96 DIP) is 8 ft (96 in).
        let s = Scale::from_ratio("1/8\" = 1'-0\"", 96.0);
        let mk = |kind, points: Vec<[f32; 2]>| Measure {
            id: 1,
            sheet: 0,
            kind,
            points,
            group: "G".into(),
            layer: DEFAULT_LAYER.into(),
            ends: EndStyle::None,
            label_offset: None,
            leader: EndStyle::None,
            holes: Vec::new(),
            depth_in: None,
            slope: None,
            height_in: None,
        };
        let m = mk(Kind::Length, vec![[0.0, 0.0], [96.0, 0.0]]);
        assert_eq!(m.qty(Some(&s)), Qty::Length(96.0));
        // A 2" x 1" paper rectangle at 1/8" scale is 16' x 8' = 128 SF, perimeter 48'.
        let mut r = mk(Kind::Area, vec![[0.0, 0.0], [192.0, 0.0], [192.0, 96.0], [0.0, 96.0]]);
        match r.qty(Some(&s)) {
            Qty::Area { area, perimeter, volume: None } => {
                assert!((area / 144.0 - 128.0).abs() < 1e-9);
                assert!((perimeter / 12.0 - 48.0).abs() < 1e-9);
            }
            q => panic!("{q:?}"),
        }
        // Cut out a 4' x 4' shaft (0.5" x 0.5" paper) and pour it 6" thick.
        let mut c = r.clone();
        c.holes.push(vec![[48.0, 24.0], [96.0, 24.0], [96.0, 72.0], [48.0, 72.0]]);
        c.depth_in = Some(6.0);
        match c.qty(Some(&s)) {
            Qty::Area { area, perimeter, volume: Some(v) } => {
                assert!((area / 144.0 - 112.0).abs() < 1e-9, "{}", area / 144.0);
                assert!((perimeter / 12.0 - 64.0).abs() < 1e-9);
                // 112 SF x 0.5 ft = 56 CF = 2.074 CY
                assert!((v / 1728.0 - 56.0).abs() < 1e-9);
                assert_eq!(fmt_qty(c.qty(Some(&s)), Units::FeetInches), "112.0 SF \u{00B7} 2.07 CY");
            }
            q => panic!("{q:?}"),
        }
        assert_eq!(centroid(&r.points), [96.0, 48.0]);
        assert_eq!(r.label_pos(), [96.0, 48.0]);
        r.label_offset = Some([10.0, -5.0]);
        assert_eq!(r.label_pos(), [106.0, 43.0]);
        assert_eq!(r.anchor(), [96.0, 48.0]);
        assert!(point_in_polygon([10.0, 10.0], &r.points));
        assert!(!point_in_polygon([200.0, 10.0], &r.points));
        // Calibration: 200 DIP measured as 25 ft.
        let c = Scale::calibrated(200.0, 300.0).unwrap();
        assert!((c.real_in_per_dip - 1.5).abs() < 1e-12);
        assert!(m.qty(None) == Qty::Unscaled);
        let o = ortho([0.0, 0.0], [100.0, 3.0]);
        assert!(o[1].abs() < 1e-4 && (o[0] - 100.0).abs() < 0.1);
        assert!(seg_dist2([5.0, 3.0], [0.0, 0.0], [10.0, 0.0]) == 9.0);
    }

    #[test]
    fn csv_and_roundtrip() {
        let mut t = Takeoff::new();
        t.scales.insert(0, Scale::from_ratio("1/8\" = 1'-0\"", 96.0));
        let a = t.add(0, Kind::Length, vec![[0.0, 0.0], [96.0, 0.0]], "Drywall, Type A", "Base");
        t.add(0, Kind::Area, vec![[0.0, 0.0], [192.0, 0.0], [192.0, 96.0], [0.0, 96.0]], "Flooring", "Base");
        t.add(1, Kind::Count, vec![[1.0, 1.0], [2.0, 2.0], [3.0, 3.0]], "Flooring", "Base");
        t.add(1, Kind::Length, vec![[0.0, 0.0], [10.0, 0.0]], "Flooring", "Base");
        // An alternate on a hidden layer: listed, not totalled.
        t.add(0, Kind::Area, vec![[0.0, 0.0], [96.0, 0.0], [96.0, 96.0]], "Flooring", "Alt 1");
        t.layers.iter_mut().find(|l| l.name == "Alt 1").unwrap().visible = false;
        assert_eq!(t.items[0].ends, EndStyle::ClosedArrow);
        assert_eq!(t.items[1].ends, EndStyle::None);
        let csv = t.to_csv(&|i| format!("Sheet {}", i + 1));
        assert!(csv.contains("\"Drywall, Type A\",Base,Sheet 1,Length,8.00,LF"), "{csv}");
        assert!(csv.contains("Flooring,Base,Sheet 1,Area,128.00,SF,48.00,LF"), "{csv}");
        assert!(csv.contains("Flooring,Base,Sheet 2,Count,3,EA"), "{csv}");
        assert!(csv.contains("Flooring,Base,Sheet 2,Length,,no scale"), "{csv}");
        assert!(csv.contains("Flooring,Alt 1,Sheet 1,Area,32.00,SF"), "{csv}");
        assert!(csv.contains("no (hidden layer)"), "{csv}");
        assert!(csv.contains("Flooring,0.00,128.00,48.00,0.00,0.00,3,\n"), "{csv}");
        // Prices: flooring $4.50/SF and $12/EA; drywall wall 8' high at $3/SF.
        t.groups.iter_mut().find(|g| g.name == "Flooring").unwrap().prices =
            Prices { per_sf: Some(4.5), per_ea: Some(12.0), ..Default::default() };
        t.groups.iter_mut().find(|g| g.name.starts_with("Drywall")).unwrap().prices =
            Prices { per_sf: Some(3.0), ..Default::default() };
        t.items.iter_mut().find(|m| m.id == a).unwrap().height_in = Some(96.0);
        let csv = t.to_csv(&|i| format!("Sheet {}", i + 1));
        // 8 LF x 8' = 64 SF wall x $3 = $192; flooring 128 SF x 4.5 + 3 x 12 = $612.
        assert!(csv.contains("\"Drywall, Type A\",8.00,0.00,0.00,64.00,0.00,0,192.00"), "{csv}");
        assert!(csv.contains("Flooring,0.00,128.00,48.00,0.00,0.00,3,612.00"), "{csv}");
        assert!(csv.contains("GRAND TOTAL,,,,,,,804.00"), "{csv}");
        assert_eq!(t.visible_items().count(), 4);
        let src = t.items.iter().find(|m| m.id == a).unwrap().clone();
        let d = t.duplicate(&src, 2, [5.0, 5.0]);
        let dup = t.items.iter().find(|m| m.id == d).unwrap();
        assert_eq!((dup.sheet, dup.points[0]), (2, [5.0, 5.0]));
        // Files saved before layers/leaders existed still load.
        let old = r#"{"version":1,"units":"FeetInches","scales":{},"groups":[],"next_id":1,
            "items":[{"id":1,"sheet":0,"kind":"Length","points":[[0,0],[1,1]],"group":"G"}]}"#;
        let o: Takeoff = serde_json::from_str(old).unwrap();
        assert_eq!(o.items[0].layer, DEFAULT_LAYER);
        assert_eq!(o.default_ends, EndStyle::ClosedArrow);
        let dir = std::env::temp_dir().join(format!("rb-test-{}", std::process::id()));
        let path = Takeoff::sidecar_path(&dir.join("A-101 Plans.pdf"));
        assert!(path.ends_with("A-101 Plans.redbeam.json"));
        t.save(&path).unwrap();
        assert_eq!(Takeoff::load(&path).unwrap(), t);
        let _ = std::fs::remove_dir_all(dir);
        assert_eq!(t.groups.len(), 2);
        assert_eq!(t.layers.len(), 2);
        assert_ne!(t.group_color("Flooring"), t.group_color("Drywall, Type A"));
    }
}

//! Importing markups and measurements that other apps (Bluebeam Revu,
//! Acrobat, consultants' tools) left in a PDF as annotations, so they can
//! be edited, restyled and totalled. The PDF engine reads each annotation
//! into a `RawAnnot` (page DIPs); this module turns them into Red-Beam
//! markups and measurement specs. Pure logic, testable anywhere.
//!
//! Scale: a measurement's own scale comes from its /Measure /R string
//! ("1 in = 8 ft") when the file stores it readably; otherwise it is
//! worked out from the value the other app displayed (its /Contents, e.g.
//! "30'-0\"" or "960.00 sf") divided by the drawn geometry.

use crate::markup::{MKind, Markup, Status, NO_FILL};
use crate::takeoff::{parse_len, Kind, SCALES};

// PDF annotation subtypes (FPDF_ANNOT_*).
pub const TEXT: i32 = 1;
pub const LINK: i32 = 2;
pub const FREETEXT: i32 = 3;
pub const LINE: i32 = 4;
pub const SQUARE: i32 = 5;
pub const CIRCLE: i32 = 6;
pub const POLYGON: i32 = 7;
pub const POLYLINE: i32 = 8;
pub const HIGHLIGHT: i32 = 9;
pub const STAMP: i32 = 13;
pub const INK: i32 = 15;
pub const POPUP: i32 = 16;
pub const WIDGET: i32 = 20;

/// Annotation subtypes that are never markups (links, form fields, popups).
pub fn is_structural(subtype: i32) -> bool {
    matches!(subtype, LINK | POPUP | WIDGET)
}

/// One annotation as read from the PDF, geometry in page DIPs.
#[derive(Clone, Debug, Default)]
pub struct RawAnnot {
    pub page: u32,
    pub index: usize,
    pub subtype: i32,
    /// [l, t, r, b]
    pub rect: [f32; 4],
    pub verts: Vec<[f32; 2]>,
    pub line: Option<[[f32; 2]; 2]>,
    pub ink: Vec<Vec<[f32; 2]>>,
    /// Text-markup boxes [l, t, r, b].
    pub quads: Vec<[f32; 4]>,
    pub color: Option<u32>,
    pub fill: Option<u32>,
    pub opacity: Option<f32>,
    /// Border width, page DIPs.
    pub width: Option<f32>,
    pub contents: String,
    pub subject: String,
    /// /IT intent: LineDimension, PolygonCloud, FreeTextCallout, ...
    pub it: String,
    pub author: String,
    pub nm: String,
    /// Default appearance of text ("/Helv 12 Tf 1 0 0 rg").
    pub da: String,
    /// Stamp /Name.
    pub name: String,
    /// Has a /Measure dictionary (a measurement).
    pub has_measure: bool,
    /// Real inches per paper inch, from /Measure /R if it was readable.
    pub measure_ratio: Option<f64>,
    /// Callout leader points (/CL), page DIPs: tip first.
    pub callout: Vec<[f32; 2]>,
    /// Red-Beam's own lossless copy (annotations it exported).
    pub data: Option<String>,
}

impl RawAnnot {
    /// Stable identity, so a re-import skips what was already imported.
    pub fn source_key(&self) -> String {
        if self.nm.is_empty() {
            format!("p{}#{}", self.page, self.index)
        } else {
            self.nm.clone()
        }
    }
}

/// A measurement to create.
#[derive(Clone, Debug, PartialEq)]
pub struct MeasureSpec {
    pub page: u32,
    pub kind: Kind,
    pub points: Vec<[f32; 2]>,
    pub group: String,
    pub color: Option<u32>,
    /// Real inches per paper inch this measurement was made at.
    pub ratio: Option<f64>,
    pub source: String,
}

#[derive(Debug, Default)]
pub struct Imported {
    pub markups: Vec<Markup>,
    pub measures: Vec<MeasureSpec>,
    /// Red-Beam measurements exported earlier (restored exactly).
    pub own_measures: Vec<crate::takeoff::Measure>,
    pub skipped: usize,
}

// ------------------------------------------------------------ parsing

/// "1 in = 8 ft", "1\" = 10'", "3/32 in = 1 ft", "1:100" -> real inches
/// per paper inch.
pub fn parse_ratio(r: &str) -> Option<f64> {
    let r = r.trim();
    if let Some((a, b)) = r.split_once(':') {
        let (a, b) = (num(a.trim())?, num(b.trim())?);
        return (a > 0.0).then(|| b / a);
    }
    let (paper, real) = r.split_once('=')?;
    let (pv, pu) = value_unit(paper)?;
    let (rv, ru) = value_unit(real)?;
    let p = pv * unit_in(&pu)?;
    let q = rv * unit_in(&ru)?;
    (p > 0.0).then(|| q / p)
}

fn num(s: &str) -> Option<f64> {
    let s = s.trim().replace(',', "");
    if let Some((a, b)) = s.split_once('/') {
        // "3/32"
        let (a, b) = (a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?);
        return (b != 0.0).then(|| a / b);
    }
    s.parse().ok()
}

fn value_unit(s: &str) -> Option<(f64, String)> {
    let s = s.trim();
    let split = s.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '/' || c == ',' || c == ' ')).unwrap_or(s.len());
    let (v, u) = s.split_at(split);
    Some((num(v)?, u.trim().to_lowercase()))
}

fn unit_in(u: &str) -> Option<f64> {
    Some(match u {
        "in" | "\"" | "inch" | "inches" => 1.0,
        "ft" | "'" | "foot" | "feet" => 12.0,
        "yd" | "yard" | "yards" => 36.0,
        "mm" => 1.0 / 25.4,
        "cm" => 1.0 / 2.54,
        "m" => 1.0 / 0.0254,
        "pt" => 1.0 / 72.0,
        _ => return None,
    })
}

/// "960.00 sf", "1,234 sq ft", "88.5 m²", "12 sy" -> square inches.
pub fn parse_area_in2(s: &str) -> Option<f64> {
    let t = s.trim().to_lowercase().replace(',', "");
    let end = t.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(t.len());
    let v: f64 = t[..end].parse().ok()?;
    let unit = t[end..].trim().replace(['.', ' '], "");
    let f = match unit.as_str() {
        "sf" | "sqft" | "ft²" | "ft2" | "squarefeet" | "squarefoot" => 144.0,
        "sy" | "sqyd" | "yd²" | "yd2" => 1296.0,
        "m²" | "m2" | "sqm" => 1550.0031,
        "in²" | "in2" | "sqin" => 1.0,
        _ => return None,
    };
    Some(v * f)
}

/// Font size (points) and fill colour from a /DA string.
pub fn parse_da(da: &str) -> (Option<f32>, Option<u32>) {
    let toks: Vec<&str> = da.split_whitespace().collect();
    let mut size = None;
    let mut color = None;
    for (i, t) in toks.iter().enumerate() {
        match *t {
            "Tf" if i >= 1 => size = toks[i - 1].parse::<f32>().ok(),
            "rg" if i >= 3 => {
                let c: Vec<f32> = toks[i - 3..i].iter().filter_map(|x| x.parse().ok()).collect();
                if c.len() == 3 {
                    color = Some(rgb(c[0], c[1], c[2]));
                }
            }
            "g" if i >= 1 => {
                if let Ok(v) = toks[i - 1].parse::<f32>() {
                    color = Some(rgb(v, v, v));
                }
            }
            _ => {}
        }
    }
    (size, color)
}

fn rgb(r: f32, g: f32, b: f32) -> u32 {
    let c = |v: f32| ((v.clamp(0.0, 1.0) * 255.0).round() as u32) & 0xff;
    (c(r) << 16) | (c(g) << 8) | c(b)
}

/// Values the PDF engine can't read (nested dictionaries and arrays),
/// found by looking at the annotation's object in the raw file. Works when
/// the object isn't compressed, as in Bluebeam's incremental saves.
#[derive(Debug, Default, PartialEq)]
pub struct RawExtra {
    /// /CL callout line, PDF user space.
    pub cl: Vec<f32>,
    /// /BS /W border width, PDF points.
    pub width: Option<f32>,
    /// /Measure /R scale string.
    pub ratio: Option<String>,
}

/// Find the object holding `/NM (nm)` in `bytes` and read its extras.
pub fn raw_extra(bytes: &[u8], nm: &str) -> RawExtra {
    let mut out = RawExtra::default();
    if nm.is_empty() {
        return out;
    }
    let needle = format!("/NM ({nm})").into_bytes();
    let Some(at) = find(bytes, &needle, 0) else { return out };
    // The enclosing object: back to "obj", forward to "endobj".
    let start = rfind(bytes, b" obj", at).unwrap_or(at.saturating_sub(4096));
    let end = find(bytes, b"endobj", at).unwrap_or((at + 4096).min(bytes.len()));
    let obj = String::from_utf8_lossy(&bytes[start..end]);
    if let Some(i) = obj.find("/CL") {
        if let Some(arr) = obj[i + 3..].trim_start().strip_prefix('[') {
            if let Some(close) = arr.find(']') {
                out.cl = arr[..close].split_whitespace().filter_map(|t| t.parse().ok()).collect();
            }
        }
    }
    if let Some(i) = obj.find("/BS") {
        let rest = &obj[i..];
        if let Some(j) = rest.find("/W ") {
            out.width = rest[j + 3..].split(|c: char| c.is_whitespace() || c == '/' || c == '>').find(|t| !t.is_empty()).and_then(|t| t.parse().ok());
        }
    }
    if let Some(i) = obj.find("/Measure") {
        let rest = &obj[i..];
        if let Some(j) = rest.find("/R") {
            let r = rest[j + 2..].trim_start();
            if let Some(body) = r.strip_prefix('(') {
                if let Some(close) = body.find(')') {
                    out.ratio = Some(body[..close].to_string());
                }
            }
        }
    }
    out
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

fn rfind(hay: &[u8], needle: &[u8], before: usize) -> Option<usize> {
    (0..before.min(hay.len().saturating_sub(needle.len()))).rev().find(|&i| &hay[i..i + needle.len()] == needle)
}

// --------------------------------------------------------- geometry

fn path_len(pts: &[[f32; 2]], closed: bool) -> f64 {
    let n = pts.len();
    let segs = if closed { n } else { n.saturating_sub(1) };
    (0..segs).map(|i| ((pts[(i + 1) % n][0] - pts[i][0]) as f64).hypot((pts[(i + 1) % n][1] - pts[i][1]) as f64)).sum()
}

fn poly_area(pts: &[[f32; 2]]) -> f64 {
    let n = pts.len();
    let mut a = 0.0;
    for i in 0..n {
        let (p, q) = (pts[i], pts[(i + 1) % n]);
        a += p[0] as f64 * q[1] as f64 - q[0] as f64 * p[1] as f64;
    }
    (a / 2.0).abs()
}

/// The standard scale nearest `ratio`, if within 1.5%.
pub fn snap_ratio(ratio: f64) -> (f64, Option<&'static str>) {
    SCALES
        .iter()
        .find(|(_, r)| (ratio / r - 1.0).abs() < 0.015)
        .map(|(label, r)| (*r, Some(*label)))
        .unwrap_or((ratio, None))
}

/// Ratio a measurement was made at, from its displayed value.
pub fn infer_ratio(kind: Kind, points: &[[f32; 2]], contents: &str) -> Option<f64> {
    let first = contents.lines().next().unwrap_or("").trim();
    let ratio = match kind {
        Kind::Length | Kind::Polyline => {
            let paper_in = path_len(points, false) / 96.0;
            let real = parse_len(first.split_whitespace().take(3).collect::<Vec<_>>().join(" ").as_str()).or_else(|| parse_len(first))?;
            (paper_in > 0.0).then(|| real / paper_in)?
        }
        Kind::Area => {
            let paper_in2 = poly_area(points) / (96.0 * 96.0);
            let real = parse_area_in2(first)?;
            (paper_in2 > 0.0).then(|| (real / paper_in2).sqrt())?
        }
        _ => return None,
    };
    (ratio.is_finite() && ratio > 0.0).then_some(ratio)
}

// --------------------------------------------------------- conversion

/// Group names other apps use for unconfigured measurement tools.
fn generic_subject(s: &str) -> bool {
    let l = s.trim().to_lowercase();
    l.is_empty() || (l.ends_with("measurement") && l.split_whitespace().count() <= 2) || l == "measure"
}

fn measure_kind(r: &RawAnnot) -> Option<Kind> {
    let it = r.it.to_lowercase();
    let subj = r.subject.to_lowercase();
    if subj.contains("count") {
        return Some(Kind::Count);
    }
    let is_measure = r.has_measure || it.ends_with("dimension") || subj.contains("measurement");
    if !is_measure {
        return None;
    }
    match r.subtype {
        LINE => Some(Kind::Length),
        POLYLINE => Some(Kind::Polyline),
        POLYGON => Some(Kind::Area),
        _ => None,
    }
}

fn rect_pts(r: [f32; 4]) -> Vec<[f32; 2]> {
    vec![[r[0], r[1]], [r[2], r[3]]]
}

/// Turn raw annotations into markups and measurement specs. `next_id`
/// supplies markup ids; `known` holds source keys already imported.
pub fn convert(raws: &[RawAnnot], next_id: &mut u64, known: &[String], now: u64) -> Imported {
    let mut out = Imported::default();
    // Counts merge into one measurement per (sheet, group, colour).
    let mut counts: Vec<MeasureSpec> = Vec::new();
    for r in raws {
        if is_structural(r.subtype) {
            continue;
        }
        let key = r.source_key();
        if known.contains(&key) {
            continue;
        }
        // Red-Beam's own exports carry a lossless copy.
        if let Some(d) = &r.data {
            if let Ok(mut m) = serde_json::from_str::<Markup>(d) {
                *next_id += 1;
                m.id = *next_id;
                m.sheet = r.page;
                m.source = Some(key);
                out.markups.push(m);
                continue;
            }
            if let Ok(mut m) = serde_json::from_str::<crate::takeoff::Measure>(d) {
                m.sheet = r.page;
                m.source = Some(key);
                out.own_measures.push(m);
                continue;
            }
        }
        if let Some(kind) = measure_kind(r) {
            let group = if generic_subject(&r.subject) {
                match kind {
                    Kind::Length | Kind::Polyline => "Imported lengths",
                    Kind::Area => "Imported areas",
                    Kind::Count => "Imported counts",
                }
                .to_string()
            } else {
                r.subject.trim().to_string()
            };
            if kind == Kind::Count {
                let c = [(r.rect[0] + r.rect[2]) / 2.0, (r.rect[1] + r.rect[3]) / 2.0];
                let pts = if r.verts.is_empty() { vec![c] } else { r.verts.clone() };
                match counts.iter_mut().find(|m| m.page == r.page && m.group == group && m.color == r.color) {
                    Some(m) => m.points.extend(pts),
                    None => counts.push(MeasureSpec { page: r.page, kind, points: pts, group, color: r.color, ratio: None, source: key }),
                }
                continue;
            }
            let points = match (kind, r.line) {
                (Kind::Length, Some(l)) => l.to_vec(),
                _ => r.verts.clone(),
            };
            if points.len() < kind.min_points() {
                out.skipped += 1;
                continue;
            }
            let ratio = r.measure_ratio.or_else(|| infer_ratio(kind, &points, &r.contents)).map(|x| snap_ratio(x).0);
            out.measures.push(MeasureSpec { page: r.page, kind, points, group, color: r.color, ratio, source: key });
            continue;
        }
        match markup_from(r, now) {
            Some(mut m) => {
                *next_id += 1;
                m.id = *next_id;
                m.source = Some(key);
                out.markups.push(m);
            }
            None => out.skipped += 1,
        }
    }
    out.measures.extend(counts);
    out
}

fn markup_from(r: &RawAnnot, now: u64) -> Option<Markup> {
    let color = r.color.unwrap_or(0xe53935);
    let width = r.width.unwrap_or(2.0).clamp(0.5, 12.0);
    let (da_size, da_color) = parse_da(&r.da);
    let font = da_size.map(|pt| pt / 0.75).unwrap_or(14.0);
    let it = r.it.to_lowercase();
    let text = r.contents.replace('\r', "\n");
    let (kind, points, text) = match r.subtype {
        SQUARE if r.opacity.is_some_and(|a| a < 0.7) && r.fill.is_some() => (MKind::Highlight, rect_pts(r.rect), String::new()),
        SQUARE => (MKind::Rect, rect_pts(r.rect), String::new()),
        CIRCLE => (MKind::Ellipse, rect_pts(r.rect), String::new()),
        POLYGON if it.contains("cloud") => (MKind::CloudPoly, r.verts.clone(), String::new()),
        POLYGON => (MKind::Polygon, r.verts.clone(), String::new()),
        POLYLINE => (if it.contains("arrow") { MKind::Arrow } else { MKind::Line }, r.verts.clone(), String::new()),
        LINE => {
            let l = r.line?;
            (if it.contains("arrow") || r.subject.to_lowercase().contains("arrow") { MKind::Arrow } else { MKind::Line }, l.to_vec(), String::new())
        }
        INK => {
            let longest = r.ink.iter().max_by_key(|p| p.len())?.clone();
            (MKind::Pen, longest, String::new())
        }
        FREETEXT if it.contains("callout") && r.callout.len() >= 2 => {
            // Tip, then the text box (the annotation rect minus the leader).
            let tip = r.callout[0];
            let knee = *r.callout.last().unwrap();
            let b = text_box_beside(r.rect, tip, knee);
            (MKind::Callout, vec![tip, [b[0], b[1]], [b[2], b[3]]], text)
        }
        FREETEXT => (MKind::Text, rect_pts(r.rect), text),
        TEXT => {
            // Sticky note: a small text box with its comment.
            let (x, y) = (r.rect[0], r.rect[1]);
            (MKind::Text, vec![[x, y], [x + 220.0, y + 60.0]], if text.is_empty() { r.subject.clone() } else { text })
        }
        STAMP => {
            let label = [&r.contents, &r.subject, &r.name].iter().map(|s| s.trim()).find(|s| !s.is_empty()).unwrap_or("STAMP").to_uppercase();
            (MKind::Stamp, rect_pts(r.rect), label)
        }
        HIGHLIGHT => {
            let b = if r.quads.is_empty() { r.rect } else { r.quads.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |a, q| [a[0].min(q[0]), a[1].min(q[1]), a[2].max(q[2]), a[3].max(q[3])]) };
            (MKind::Highlight, rect_pts(b), String::new())
        }
        _ => return None,
    };
    if points.len() < 2 {
        return None;
    }
    let is_text = kind.has_text();
    let (line_color, text_color, fill) = if matches!(kind, MKind::Text | MKind::Callout) {
        // FreeText /C is the box fill; the text colour lives in /DA.
        let tc = da_color.unwrap_or(color);
        let fill = match r.color {
            Some(0xffffff) | None => None,
            Some(c) => Some(c),
        };
        (tc, None, if r.subtype == TEXT { Some(0xfff59d) } else { fill })
    } else {
        (color, None, None)
    };
    Some(Markup {
        id: 0,
        sheet: r.page,
        kind,
        points,
        text,
        color: if kind == MKind::Highlight { r.fill.or(r.color).unwrap_or(0xffeb3b) } else { line_color },
        width,
        font: if is_text { font } else { 14.0 },
        status: Status::None,
        author: r.author.clone(),
        created: now,
        bold: false,
        italic: false,
        text_color,
        fill: if r.subtype == FREETEXT && r.color.is_none() { Some(NO_FILL) } else { fill },
        source: None,
    })
}

/// The part of a callout's rect that holds its text: the side of the
/// leader's knee away from its tip.
fn text_box_beside(rect: [f32; 4], tip: [f32; 2], knee: [f32; 2]) -> [f32; 4] {
    let [l, t, r, b] = rect;
    let (dx, dy) = (knee[0] - tip[0], knee[1] - tip[1]);
    let out = if dx.abs() >= dy.abs() {
        if dx > 0.0 {
            [knee[0].clamp(l, r), t, r, b]
        } else {
            [l, t, knee[0].clamp(l, r), b]
        }
    } else if dy > 0.0 {
        [l, knee[1].clamp(t, b), r, b]
    } else {
        [l, t, r, knee[1].clamp(t, b)]
    };
    if out[2] - out[0] < 10.0 || out[3] - out[1] < 10.0 {
        rect
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratios_and_values() {
        assert_eq!(parse_ratio("1 in = 8 ft"), Some(96.0));
        assert_eq!(parse_ratio("1\" = 20'"), Some(240.0));
        assert!((parse_ratio("3/32 in = 1 ft").unwrap() - 128.0).abs() < 1e-9);
        assert_eq!(parse_ratio("1:100"), Some(100.0));
        assert_eq!(parse_area_in2("960.00 sf"), Some(960.0 * 144.0));
        assert_eq!(parse_area_in2("1,234 sq ft"), Some(1234.0 * 144.0));
        assert!(parse_area_in2("12 apples").is_none());
        assert_eq!(parse_da("0 0 1 rg /Helv 14 Tf"), (Some(14.0), Some(0x0000ff)));
        assert_eq!(snap_ratio(96.4), (96.0, Some("1/8\" = 1'-0\"")));
    }

    #[test]
    fn infers_scale_from_displayed_values() {
        // 270 pt = 360 DIP = 3.75 paper inches shown as 30'-0" -> 96.
        let r = infer_ratio(Kind::Length, &[[0.0, 0.0], [360.0, 0.0]], "30'-0\"").unwrap();
        assert!((r - 96.0).abs() < 1e-6, "{r}");
        // 480 x 288 DIP = 5 x 3 in shown as 960 sf -> 40' x 24' -> 96.
        let a = infer_ratio(Kind::Area, &[[0.0, 0.0], [480.0, 0.0], [480.0, 288.0], [0.0, 288.0]], "960.00 sf").unwrap();
        assert!((a - 96.0).abs() < 1e-6, "{a}");
    }

    #[test]
    fn raw_lookup() {
        let pdf = b"12 0 obj\n<< /Type /Annot /Subtype /FreeText /IT /FreeTextCallout /NM (cb-1) /CL [1600 620 1650 725] /BS << /W 2.5 >> /Measure << /R (1 in = 8 ft) >> >>\nendobj\n";
        let x = raw_extra(pdf, "cb-1");
        assert_eq!(x.cl, vec![1600.0, 620.0, 1650.0, 725.0]);
        assert_eq!(x.width, Some(2.5));
        assert_eq!(x.ratio.as_deref(), Some("1 in = 8 ft"));
        assert_eq!(raw_extra(pdf, "missing"), RawExtra::default());
    }

    fn raw(subtype: i32, subject: &str, it: &str) -> RawAnnot {
        RawAnnot { subtype, subject: subject.into(), it: it.into(), color: Some(0xff0000), rect: [0.0, 0.0, 100.0, 50.0], ..Default::default() }
    }

    #[test]
    fn converts_bluebeam_style_annotations() {
        let mut area = raw(POLYGON, "Drywall Ceiling", "PolygonDimension");
        area.verts = vec![[0.0, 0.0], [480.0, 0.0], [480.0, 288.0], [0.0, 288.0]];
        area.contents = "960.00 sf".into();
        area.nm = "a1".into();
        let mut len = raw(LINE, "Length Measurement", "LineDimension");
        len.line = Some([[0.0, 0.0], [360.0, 0.0]]);
        len.contents = "30'-0\"".into();
        let counts: Vec<RawAnnot> = (0..3)
            .map(|i| {
                let mut c = raw(CIRCLE, "Count Measurement", "");
                c.rect = [i as f32 * 20.0, 0.0, i as f32 * 20.0 + 10.0, 10.0];
                c.index = 10 + i;
                c
            })
            .collect();
        let mut cloud = raw(POLYGON, "Cloud", "PolygonCloud");
        cloud.verts = vec![[0.0, 0.0], [50.0, 0.0], [50.0, 50.0]];
        let mut text = raw(FREETEXT, "Text Box", "");
        text.contents = "VERIFY".into();
        text.da = "0 0 1 rg /Helv 12 Tf".into();
        text.color = Some(0xffffff);
        let link = raw(LINK, "", "");
        let mut all = vec![area, len, cloud, text, link];
        all.extend(counts);
        let mut id = 100;
        let got = convert(&all, &mut id, &[], 0);
        assert_eq!(got.measures.len(), 3, "{:?}", got.measures);
        assert_eq!(got.measures[0].group, "Drywall Ceiling");
        assert_eq!(got.measures[0].ratio, Some(96.0));
        assert_eq!(got.measures[1].group, "Imported lengths");
        assert_eq!(got.measures[1].ratio, Some(96.0));
        assert_eq!(got.measures[2].kind, Kind::Count);
        assert_eq!(got.measures[2].points.len(), 3);
        assert_eq!(got.markups.len(), 2);
        assert_eq!(got.markups[0].kind, MKind::CloudPoly);
        assert_eq!(got.markups[1].kind, MKind::Text);
        assert_eq!(got.markups[1].color, 0x0000ff); // text colour from /DA
        assert!((got.markups[1].font * 0.75 - 12.0).abs() < 0.01);
        assert_eq!(id, 102);
        // Re-importing skips what is already there.
        let again = convert(&all, &mut id, &["a1".to_string()], 0);
        assert_eq!(again.measures.len(), 2);
    }
}

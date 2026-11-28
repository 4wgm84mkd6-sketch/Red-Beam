//! Takeoff UI: tool palette, click-to-measure tools, editing (move, reshape,
//! label leaders), layers, live readouts, selection/undo, the quantities
//! panel, menus and the typed-entry prompt. Lives in a child module of `app`
//! so it can use the App's private state.

use std::path::PathBuf;

use windows::core::PCWSTR;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::{IDC_HAND, IDC_SIZEALL};
use windows_numerics::Vector2;

use super::{d2r, rgb, App, CmpMode, ACCENT, BTN_BG, TEXT, TEXT_DIM};
use crate::markup::{MKind, Markup};
use crate::snap::SnapKind;
use crate::takeoff::{self, EndStyle, Kind, Layer, Measure, Qty, Scale, Takeoff, Units, DEFAULT_LAYER, SCALES};

/// Which unit price a prompt edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PriceKind {
    Lf,
    Sf,
    Cy,
    Ea,
}

impl PriceKind {
    fn unit(self) -> &'static str {
        match self {
            PriceKind::Lf => "LF",
            PriceKind::Sf => "SF",
            PriceKind::Cy => "CY",
            PriceKind::Ea => "EA",
        }
    }
}

/// Export formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportKind {
    Excel,
    Csv,
}

/// Parse a typed price: `3.25`, `$3.25`, `1,250` (blank clears).
fn parse_money(s: &str) -> Option<Option<f64>> {
    let t: String = s.chars().filter(|c| !matches!(c, '$' | ',' | ' ')).collect();
    if t.is_empty() {
        return Some(None);
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0).map(Some)
}
use crate::view::RectF;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    /// Pan with left-drag on empty space; click/drag measurements to edit.
    Select,
    /// Two clicks, then type the real distance.
    Calibrate,
    Measure(Kind),
    /// Area that then asks for a depth (volume).
    Volume,
    /// Outline a cut-out inside area measurement `id`.
    Cutout(u64),
    /// Draw a markup.
    Markup(MKind),
    /// Drag a box around a detail that has its own scale.
    Viewport,
}

impl Tool {
    /// The kind of shape being drawn.
    fn kind(self) -> Option<Kind> {
        match self {
            Tool::Measure(k) => Some(k),
            Tool::Volume | Tool::Cutout(_) => Some(Kind::Area),
            Tool::Calibrate => Some(Kind::Length),
            Tool::Select | Tool::Markup(_) | Tool::Viewport => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TBtn {
    /// Switch toolbar between takeoff and markup tools.
    Mode(bool),
    MTool(MKind),
    Stamp,
    MColor,
    MWidth,
    MFont,
    SavePdf,
    Presets,
    Select,
    Scale,
    Tool(Kind),
    Volume,
    Snap,
    Group,
    Layer,
    Units,
    Quantities,
    Export,
    // Format bar (selected markups).
    FmtInfo,
    FontDown,
    FontSize,
    FontUp,
    Bold,
    Italic,
    TextColor,
    LineColor,
    FillColor,
    FmtWidth,
    FmtStatus,
    SelectSame,
    MakeDefault,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuKind {
    Scale,
    Group,
    Layer,
    Units,
    Export,
    Stamp,
    MColor,
    MWidth,
    MFont,
    SavePdf,
    MarkupContext { id: u64 },
    ViewportScale,
    Presets,
    Sheets,
    FmtSize,
    FmtText,
    FmtLine,
    FmtFill,
    FmtWidth,
    FmtStatus,
    /// Right-click on a measurement: maybe on one of its points
    /// (ring 0 = outline, ring k = cut-out k-1) or inside a cut-out.
    Context { id: u64, vertex: Option<(usize, usize)>, hole: Option<usize> },
}

#[derive(Clone, Debug)]
pub enum PromptFor {
    /// Two calibration points `dip_len` apart on `sheet`.
    Calibrate { sheet: u32, dip_len: f64 },
    /// New group; also assign it to measurement `assign`.
    NewGroup { assign: Option<u64> },
    /// New layer; also move measurement `assign` onto it.
    NewLayer { assign: Option<u64> },
    RenameLayer(usize),
    /// Depth / thickness for an area's volume.
    Depth { id: u64 },
    Price { group: usize, kind: PriceKind },
    RenameGroup(usize),
    Slope { id: u64 },
    WallHeight { id: u64 },
    MarkupText { id: u64 },
    Find,
    GoTo,
    PresetName,
    SheetRange { delete: bool },
}

pub struct Prompt {
    pub title: String,
    pub hint: String,
    pub text: String,
    pub purpose: PromptFor,
}

pub enum Undo {
    Added(u64),
    Removed(Measure),
    /// State of a measurement before an edit.
    Modified(Measure),
    Scale(u32, Option<Scale>),
    AllScales(std::collections::BTreeMap<u32, Scale>),
    /// Items and layers before a layer rename/delete.
    Layers(Vec<Measure>, Vec<Layer>),
    /// Items and groups before a group rename.
    Groups(Vec<Measure>, Vec<takeoff::Group>),
    Viewports(Vec<takeoff::Viewport>),
    MarkupAdded(u64),
    MarkupRemoved(Markup),
    MarkupModified(Markup),
    /// Several markups before a format change.
    MarkupsModified(Vec<Markup>),
    /// The whole takeoff before a bulk change (import).
    Whole(Box<Takeoff>),
}

/// A drag in progress with the Select tool.
pub enum EditDrag {
    /// Whole measurement.
    Move { orig: Measure, start: [f32; 2], moved: bool },
    /// One point (corner, end or count marker) of ring `ring`
    /// (0 = outline, k = cut-out k-1).
    Vertex { orig: Measure, ring: usize, index: usize, moved: bool },
    /// The label, which then gets a leader back to the measurement.
    Label { orig: Measure, start: [f32; 2], moved: bool },
    MarkupMove { orig: Markup, start: [f32; 2], moved: bool },
    MarkupHandle { orig: Markup, index: usize, moved: bool },
}

// Menu command ids.
const CMD_BASE: usize = 100;
const CMD_CALIBRATE: usize = 900;
const CMD_APPLY_ALL: usize = 901;
const CMD_CLEAR: usize = 902;
const CMD_NEW_GROUP: usize = 903;
const CMD_NEW_LAYER: usize = 904;
const CMD_SHOW_ALL: usize = 905;
const CMD_RESET_LABEL: usize = 906;
const CMD_DELETE_POINT: usize = 907;
const CMD_DUPLICATE: usize = 908;
const CMD_DELETE: usize = 909;
const CMD_ADD_CUTOUT: usize = 910;
const CMD_REMOVE_CUTOUT: usize = 911;
const CMD_SET_DEPTH: usize = 912;
const CMD_CLEAR_DEPTH: usize = 913;
const CMD_SET_SLOPE: usize = 914;
const CMD_CLEAR_SLOPE: usize = 915;
const CMD_SET_HEIGHT: usize = 916;
const CMD_CLEAR_HEIGHT: usize = 917;
const CMD_EXPORT_XLSX: usize = 918;
const CMD_EXPORT_CSV: usize = 919;
const CMD_ADD_VP: usize = 920;
const CMD_VP_REMOVE: usize = 16_000;
const CMD_GROUP_ACTIVE: usize = 10_000;
const CMD_GROUP_RENAME: usize = 11_000;
const CMD_PRICE_LF: usize = 12_000;
const CMD_PRICE_SF: usize = 13_000;
const CMD_PRICE_CY: usize = 14_000;
const CMD_PRICE_EA: usize = 15_000;
const CMD_SET_GROUP: usize = 1000;
const CMD_SET_LAYER: usize = 2000;
const CMD_ENDS: usize = 3000;
const CMD_LEADER: usize = 4000;
const CMD_LAYER_ACTIVE: usize = 5000;
const CMD_LAYER_VISIBLE: usize = 6000;
const CMD_LAYER_LOCK: usize = 7000;
const CMD_LAYER_RENAME: usize = 8000;
const CMD_LAYER_DELETE: usize = 9000;

/// One entry of a native popup menu (built in main.rs).
pub struct MenuItem {
    pub id: usize,
    pub label: String,
    pub checked: bool,
    pub separator: bool,
    pub children: Vec<MenuItem>,
}

pub(super) fn item(id: usize, label: impl Into<String>, checked: bool) -> MenuItem {
    MenuItem { id, label: label.into(), checked, separator: false, children: Vec::new() }
}
pub(super) fn sep() -> MenuItem {
    MenuItem { id: 0, label: String::new(), checked: false, separator: true, children: Vec::new() }
}
pub(super) fn submenu(label: impl Into<String>, children: Vec<MenuItem>) -> MenuItem {
    MenuItem { id: 0, label: label.into(), checked: false, separator: false, children }
}

pub(super) fn v2(p: [f32; 2]) -> Vector2 {
    Vector2 { X: p[0], Y: p[1] }
}

/// Is the point within `tol` of the measurement's lines, inside its area, or
/// on one of its count markers? (page DIPs)
fn hits(m: &Measure, p: [f32; 2], tol: f32) -> bool {
    let tol2 = tol * tol;
    match m.kind {
        Kind::Count => m.points.iter().any(|q| takeoff::dist(*q, p) as f32 <= tol * 1.6),
        Kind::Length | Kind::Polyline => m.points.windows(2).any(|w| takeoff::seg_dist2(p, w[0], w[1]) <= tol2),
        Kind::Area => {
            let on_edge = |ring: &[[f32; 2]]| {
                (0..ring.len()).any(|i| takeoff::seg_dist2(p, ring[i], ring[(i + 1) % ring.len()]) <= tol2)
            };
            (takeoff::point_in_polygon(p, &m.points) && !m.holes.iter().any(|h| takeoff::point_in_polygon(p, h)))
                || on_edge(&m.points)
                || m.holes.iter().any(|h| on_edge(h))
        }
    }
}

/// Ring `r` of a measurement: 0 = outline, k = cut-out k-1.
fn ring(m: &Measure, r: usize) -> &[[f32; 2]] {
    if r == 0 {
        &m.points
    } else {
        &m.holes[r - 1]
    }
}

fn ring_mut(m: &mut Measure, r: usize) -> &mut Vec<[f32; 2]> {
    if r == 0 {
        &mut m.points
    } else {
        &mut m.holes[r - 1]
    }
}

fn ring_count(m: &Measure) -> usize {
    1 + m.holes.len()
}

impl App {
    // ------------------------------------------------------------ state

    /// The sheet takeoffs apply to (the current sheet of the open set),
    /// unless the old revision is being shown.
    pub(super) fn takeoff_sheet(&self) -> Option<u32> {
        if self.cmp_mode() == Some(CmpMode::Old) {
            return None;
        }
        self.doc.as_ref().map(|d| d.current)
    }

    pub(super) fn load_takeoff(&mut self) {
        let Some(doc) = &self.doc else { return };
        let side = Takeoff::sidecar_path(&doc.path);
        let fallback = local_dir().map(|d| Takeoff::fallback_path(&doc.path, &d));
        let (t, path) = match Takeoff::load(&side) {
            Some(t) => (t, Some(side)),
            None => match fallback.as_ref().and_then(|f| Takeoff::load(f).map(|t| (t, f.clone()))) {
                Some((t, f)) => (t, Some(f)),
                None => (Takeoff { units: self.settings.units_default, ..Takeoff::new() }, None),
            },
        };
        self.takeoff = t;
        self.takeoff_path = path;
        self.undo.clear();
        self.selected = None;
        self.edit = None;
        self.pending_pts.clear();
        self.prompt = None;
        if !self.takeoff.groups.iter().any(|g| g.name == self.group) {
            self.group = self.takeoff.groups.first().map(|g| g.name.clone()).unwrap_or_else(|| "General".into());
        }
        if !self.takeoff.layers.iter().any(|l| l.name == self.layer) {
            self.layer = self.takeoff.layers.first().map(|l| l.name.clone()).unwrap_or_else(|| DEFAULT_LAYER.into());
        }
    }

    /// Autosave next to the PDF; fall back to the app-data folder when the
    /// drawing's folder is read-only. The PDF itself is never modified.
    pub(super) fn save_takeoff(&mut self) {
        let Some(doc) = &self.doc else { return };
        let side = Takeoff::sidecar_path(&doc.path);
        let target = self.takeoff_path.clone().unwrap_or(side);
        match self.takeoff.save(&target) {
            Ok(()) => self.takeoff_path = Some(target),
            Err(e) => match local_dir().map(|d| Takeoff::fallback_path(&doc.path, &d)) {
                Some(fb) if self.takeoff.save(&fb).is_ok() => self.takeoff_path = Some(fb),
                _ => self.status = Some(format!("Could not save takeoff: {e}")),
            },
        }
    }

    pub(super) fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        self.pending_pts.clear();
        if !matches!(tool, Tool::Select | Tool::Cutout(_)) {
            self.selected = None;
            self.set_sel_markup(None);
        }
        self.mdraft = None;
        self.active_preset = None;
        if tool != Tool::Calibrate {
            self.calib_vp = None;
        }
        self.update_jobs(false);
        self.invalidate();
    }

    /// Start outlining a cut-out in the selected area.
    fn start_cutout(&mut self) {
        match self.selected_measure() {
            Some(m) if m.kind == Kind::Area && self.editable(m) => {
                let id = m.id;
                self.set_tool(Tool::Cutout(id));
                self.status = Some("Cut-out: click its corners inside the area, then Enter".into());
            }
            _ => self.status = Some("Select an area first, then press X (or right-click \u{2192} Add cut-out)".into()),
        }
        self.invalidate();
    }

    fn ask_depth(&mut self, id: u64) {
        let current = self.takeoff.items.iter().find(|m| m.id == id).and_then(|m| m.depth_in);
        let text = match (current, &self.last_depth) {
            (Some(d), _) => takeoff::fmt_len(d, Units::FeetInches),
            (None, Some(t)) => t.clone(),
            (None, None) => String::new(),
        };
        self.prompt = Some(Prompt {
            title: "Depth / thickness".into(),
            hint: "e.g. 6\", 4 1/2\", 1'-0\" or 150mm \u{2014} volume shows in CY (m\u{00B3} in metric)".into(),
            text,
            purpose: PromptFor::Depth { id },
        });
    }

    fn toggle_snap(&mut self) {
        self.snap_on = !self.snap_on;
        self.status = Some(if self.snap_on { "Snapping on (hold Alt to place freely)" } else { "Snapping off" }.into());
        self.update_jobs(false);
        self.invalidate();
    }

    /// Snap a page point to the sheet's line work (unless Alt is held).
    pub(super) fn snap_pt(&self, p: [f32; 2]) -> ([f32; 2], Option<SnapKind>) {
        let alt = unsafe { GetKeyState(VK_MENU.0 as i32) < 0 };
        if !self.snap_on || alt {
            return (p, None);
        }
        let Some(idx) = self.doc.as_ref().and_then(|d| self.snaps.get(&(d.id, d.current))) else { return (p, None) };
        let tol = 9.0 * self.s() / self.view.zoom;
        match idx.snap(p, tol) {
            Some((q, k)) => (q, Some(k)),
            None => (p, None),
        }
    }

    /// Does the current tool want snapping data for this sheet?
    pub(super) fn wants_snap(&self) -> bool {
        self.snap_on && self.takeoff_sheet().is_some() && (self.tool != Tool::Select || self.selected.is_some())
    }

    fn selected_measure(&self) -> Option<&Measure> {
        let id = self.selected?;
        self.takeoff.items.iter().find(|m| m.id == id)
    }

    /// Measurements drawn on the current sheet (visible layers).
    fn sheet_items(&self) -> impl Iterator<Item = &Measure> {
        let sheet = self.takeoff_sheet();
        self.takeoff.items.iter().filter(move |m| Some(m.sheet) == sheet && self.takeoff.layer_visible(&m.layer))
    }

    /// Can this measurement be selected / edited?
    fn editable(&self, m: &Measure) -> bool {
        !self.takeoff.layer_locked(&m.layer)
    }

    // ------------------------------------------------------------ layout

    pub(super) fn toolbar_layout(&self) -> Vec<(TBtn, RectF, String)> {
        if self.doc.is_none() {
            return Vec::new();
        }
        let s = self.s();
        let off = self.tabbar_h();
        let (top, bot) = (off + 8.0 * s, off + 40.0 * s);
        let scale = match self.takeoff_sheet().and_then(|p| self.takeoff.scale(p)) {
            Some(sc) => format!("Scale: {} \u{25BE}", sc.label),
            None => "Scale: not set \u{25BE}".into(),
        };
        let units = match self.takeoff.units {
            Units::FeetInches => "ft-in",
            Units::DecimalFeet => "dec ft",
            Units::Metric => "metric",
        };
        let specs: Vec<(TBtn, String, f32, f32)> = if self.markup_mode {
            self.markup_toolbar_specs()
        } else {
            vec![
            (TBtn::Mode(false), "Takeoff".into(), 70.0, 0.0),
            (TBtn::Mode(true), "Markup".into(), 70.0, 10.0),
            (TBtn::Select, "Select".into(), 60.0, 8.0),
            (TBtn::Scale, scale, 170.0, 8.0),
            (TBtn::Tool(Kind::Length), "Length".into(), 64.0, 4.0),
            (TBtn::Tool(Kind::Polyline), "Polyline".into(), 72.0, 4.0),
            (TBtn::Tool(Kind::Area), "Area".into(), 52.0, 4.0),
            (TBtn::Volume, "Volume".into(), 64.0, 4.0),
            (TBtn::Tool(Kind::Count), "Count".into(), 58.0, 8.0),
            (TBtn::Snap, if self.snap_on { "Snap \u{2713}".into() } else { "Snap".into() }, 64.0, 8.0),
            (TBtn::Group, format!("Group: {} \u{25BE}", self.group), 150.0, 4.0),
            (TBtn::Layer, format!("Layer: {} \u{25BE}", self.layer), 140.0, 4.0),
            (TBtn::Units, format!("{units} \u{25BE}"), 72.0, 8.0),
            (TBtn::Quantities, "Quantities".into(), 90.0, 4.0),
            (TBtn::Export, "Export".into(), 64.0, 8.0),
            (TBtn::Presets, self.presets_label(), 130.0, 0.0),
        ]
        };
        // Buttons wrap onto another row when the window is too narrow; the
        // format bar (selected markups) always starts on a row of its own.
        let (cw, _) = self.canvas();
        let (start, max_x) = (self.left_inset() + 8.0 * s, cw - 8.0 * s);
        let row_h = bot - top + 8.0 * s;
        let mut out = Vec::new();
        let mut y = top;
        for specs in [specs, self.format_specs()] {
            if specs.is_empty() {
                continue;
            }
            let mut x = start;
            for (b, label, w, gap) in specs {
                if x + w * s > max_x && x > start {
                    x = start;
                    y += row_h;
                }
                out.push((b, RectF::new(x, y, x + w * s, y + (bot - top)), label));
                x += (w + 2.0 + gap) * s;
            }
            y += row_h;
        }
        out
    }

    /// Rows the main toolbar (not the format bar) takes at this width.
    pub(super) fn toolbar_main_rows(&self) -> usize {
        let fmt: Vec<TBtn> = self.format_specs().into_iter().map(|f| f.0).collect();
        let mut tops: Vec<f32> = Vec::new();
        for (b, r, _) in self.toolbar_layout() {
            if !fmt.contains(&b) && !tops.iter().any(|t| (t - r.t).abs() < 0.5) {
                tops.push(r.t);
            }
        }
        tops.len().max(1)
    }

    /// Background of each toolbar row.
    fn toolbar_rects(&self) -> Vec<RectF> {
        let s = self.s();
        let mut rows: Vec<RectF> = Vec::new();
        for (_, r, _) in self.toolbar_layout() {
            let t = r.t - 4.0 * s;
            match rows.iter_mut().find(|q| (q.t - t).abs() < 0.5) {
                Some(q) => q.r = q.r.max(r.r + 4.0 * s),
                None => rows.push(RectF::new(r.l - 4.0 * s, t, r.r + 4.0 * s, r.b + 4.0 * s)),
            }
        }
        rows
    }

    fn qty_panel_rect(&self) -> Option<RectF> {
        if !self.show_qty || self.doc.is_none() {
            return None;
        }
        let s = self.s();
        let (cw, ch) = self.canvas();
        let top = self.toolbar_rects().iter().map(|r| r.b).fold(self.tabbar_h() + 44.0 * s, f32::max) + 8.0 * s;
        Some(RectF::new(cw - 360.0 * s, top, cw - 8.0 * s, ch - 8.0 * s))
    }

    /// Is (x, y) over takeoff UI (so canvas tools should ignore it)?
    pub(super) fn over_tool_ui(&self, x: f32, y: f32) -> bool {
        self.prompt.is_some()
            || self.over_panel(x, y)
            || (y < self.tabbar_h() && x >= self.left_inset())
            || self.toolbar_rects().iter().any(|r| r.contains_pt(x, y))
            || self.qty_panel_rect().is_some_and(|r| r.contains_pt(x, y))
    }

    /// What a toolbar button does, with its shortcut.
    pub(super) fn tbtn_tip(&self, b: TBtn) -> String {
        if let Some(t) = self.format_tip(b) {
            return t.into();
        }
        let t = match b {
            TBtn::Mode(false) => "Takeoff tools: measure lengths, areas, volumes, counts  (M switches)",
            TBtn::Mode(true) => "Markup tools: clouds, text, callouts, stamps  (M switches)",
            TBtn::Select => "Select, move and edit; Ctrl+click to select several  (V)",
            TBtn::MTool(MKind::Cloud) | TBtn::MTool(MKind::CloudPoly) => "Revision cloud: drag a box, or click points and press Enter  (C)",
            TBtn::MTool(MKind::Text) => "Text box: drag a box, then type  (T)",
            TBtn::MTool(MKind::Callout) => "Callout: press on the point, drag to where the text goes  (K)",
            TBtn::MTool(MKind::Arrow) => "Arrow  (A)",
            TBtn::MTool(MKind::Line) => "Line  (L)",
            TBtn::MTool(MKind::Rect) => "Rectangle  (R)",
            TBtn::MTool(MKind::Ellipse) => "Ellipse  (E)",
            TBtn::MTool(MKind::Highlight) => "Highlighter box  (H)",
            TBtn::MTool(MKind::Pen) => "Freehand pen",
            TBtn::MTool(MKind::Stamp) | TBtn::Stamp => "Stamp: APPROVED, REVIEWED, REVISE & RESUBMIT\u{2026}",
            TBtn::MColor => "Colour for new markups",
            TBtn::MWidth => "Line width for new markups",
            TBtn::MFont => "Text size for new markups",
            TBtn::SavePdf => "Save a PDF copy with markups, a flattened copy, or print  (Ctrl+P)",
            TBtn::Presets => "Saved tool presets (Tool Chest)",
            TBtn::Scale => "This sheet's scale, calibration and detail scales",
            TBtn::Tool(Kind::Length) => "Length: two clicks  (L)",
            TBtn::Tool(Kind::Polyline) => "Polyline: click points, Enter to finish  (Y)",
            TBtn::Tool(Kind::Area) => "Area: click corners, Enter to finish  (A)",
            TBtn::Tool(Kind::Count) => "Count: click each item, Enter to finish  (C)",
            TBtn::Volume => "Volume: draw an area, then type the depth  (U)",
            TBtn::Snap => "Snap to the drawing's line work  (S; hold Alt to place freely)",
            TBtn::Group => "Group (trade) for new measurements, and its prices",
            TBtn::Layer => "Layer for new measurements; show, hide or lock layers",
            TBtn::Units => "Units: feet-inches, decimal feet or metric",
            TBtn::Quantities => "Quantities panel: totals and costs  (Q)",
            TBtn::Export => "Export quantities to Excel or CSV",
            _ => "",
        };
        t.into()
    }

    pub(super) fn toolbar_hit(&self, x: f32, y: f32) -> Option<TBtn> {
        self.toolbar_layout().into_iter().find(|(_, r, _)| r.contains_pt(x, y)).map(|(b, _, _)| b)
    }

    pub(super) fn press_tool(&mut self, b: TBtn) {
        if self.format_press(b) {
            self.invalidate();
            return;
        }
        match b {
            TBtn::Mode(m) => {
                self.markup_mode = m;
                self.set_tool(Tool::Select);
            }
            TBtn::MTool(k) => self.set_tool(Tool::Markup(k)),
            TBtn::Stamp => {
                self.set_tool(Tool::Markup(MKind::Stamp));
                self.status = Some("Stamp: click where it goes".into());
            }
            TBtn::MColor => self.pending_menu = Some(MenuKind::MColor),
            TBtn::MWidth => self.pending_menu = Some(MenuKind::MWidth),
            TBtn::MFont => self.pending_menu = Some(MenuKind::MFont),
            TBtn::SavePdf => self.pending_menu = Some(MenuKind::SavePdf),
            TBtn::Presets => self.pending_menu = Some(MenuKind::Presets),
            TBtn::Select => self.set_tool(Tool::Select),
            TBtn::Tool(k) => self.set_tool(Tool::Measure(k)),
            TBtn::Volume => self.set_tool(Tool::Volume),
            TBtn::Snap => self.toggle_snap(),
            TBtn::Scale => self.pending_menu = Some(MenuKind::Scale),
            TBtn::Group => self.pending_menu = Some(MenuKind::Group),
            TBtn::Layer => self.pending_menu = Some(MenuKind::Layer),
            TBtn::Units => self.pending_menu = Some(MenuKind::Units),
            TBtn::Quantities => {
                self.show_qty = !self.show_qty;
                self.invalidate();
            }
            TBtn::Export => {
                if self.takeoff.items.is_empty() {
                    self.pending_info = Some("There are no measurements to export yet.".into());
                } else {
                    self.pending_menu = Some(MenuKind::Export);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------- menus

    fn style_items(base: usize, current: EndStyle) -> Vec<MenuItem> {
        EndStyle::ALL.iter().enumerate().map(|(i, st)| item(base + i, st.name(), *st == current)).collect()
    }

    pub fn menu_items(&self, kind: MenuKind) -> Vec<MenuItem> {
        if let Some(v) = self.markup_menu_items(kind) {
            return v;
        }
        if let Some(v) = self.format_menu_items(kind) {
            return v;
        }
        match kind {
            MenuKind::Scale => {
                let cur = self.takeoff_sheet().and_then(|p| self.takeoff.scale(p)).map(|s| s.label.clone());
                let mut v: Vec<MenuItem> = SCALES
                    .iter()
                    .enumerate()
                    .map(|(i, (label, _))| item(CMD_BASE + i, *label, cur.as_deref() == Some(*label)))
                    .collect();
                v.push(sep());
                v.push(item(CMD_CALIBRATE, "Calibrate by two points\u{2026}", cur.as_deref() == Some("Calibrated")));
                v.push(sep());
                v.push(item(CMD_APPLY_ALL, "Apply this sheet's scale to all sheets", false));
                v.push(item(CMD_CLEAR, "Clear this sheet's scale", false));
                v.push(sep());
                v.push(item(CMD_ADD_VP, "Add a detail scale (drag a box around a detail)\u{2026}", false));
                v.extend(self.viewport_remove_items(CMD_VP_REMOVE));
                v
            }
            MenuKind::Group => {
                let money = |v: Option<f64>| v.map(takeoff::fmt_money).unwrap_or_else(|| "\u{2014}".into());
                let mut v: Vec<MenuItem> = self
                    .takeoff
                    .groups
                    .iter()
                    .enumerate()
                    .map(|(i, g)| {
                        let p = g.prices;
                        let mut m = submenu(
                            g.name.clone(),
                            vec![
                                item(CMD_GROUP_ACTIVE + i, "Measure with this group", g.name == self.group),
                                sep(),
                                item(CMD_PRICE_LF + i, format!("Price per LF: {}\u{2026}", money(p.per_lf)), false),
                                item(CMD_PRICE_SF + i, format!("Price per SF (areas, walls): {}\u{2026}", money(p.per_sf)), false),
                                item(CMD_PRICE_CY + i, format!("Price per CY: {}\u{2026}", money(p.per_cy)), false),
                                item(CMD_PRICE_EA + i, format!("Price per EA: {}\u{2026}", money(p.per_ea)), false),
                                sep(),
                                item(CMD_GROUP_RENAME + i, "Rename\u{2026}", false),
                            ],
                        );
                        m.checked = g.name == self.group;
                        m
                    })
                    .collect();
                if !v.is_empty() {
                    v.push(sep());
                }
                v.push(item(CMD_NEW_GROUP, "New group\u{2026}", false));
                v
            }
            MenuKind::Layer => {
                let mut v: Vec<MenuItem> = self
                    .takeoff
                    .layers
                    .iter()
                    .enumerate()
                    .map(|(i, l)| {
                        let mut label = l.name.clone();
                        if !l.visible {
                            label.push_str("   (hidden)");
                        }
                        if l.locked {
                            label.push_str("   (locked)");
                        }
                        let mut m = submenu(
                            label,
                            vec![
                                item(CMD_LAYER_ACTIVE + i, "Draw on this layer", l.name == self.layer),
                                item(CMD_LAYER_VISIBLE + i, "Visible (counted in totals)", l.visible),
                                item(CMD_LAYER_LOCK + i, "Locked (can't be edited)", l.locked),
                                sep(),
                                item(CMD_LAYER_RENAME + i, "Rename\u{2026}", false),
                                item(CMD_LAYER_DELETE + i, "Delete layer (items move to another layer)", false),
                            ],
                        );
                        m.checked = l.name == self.layer;
                        m
                    })
                    .collect();
                if !v.is_empty() {
                    v.push(sep());
                    v.push(item(CMD_SHOW_ALL, "Show all layers", false));
                }
                v.push(item(CMD_NEW_LAYER, "New layer\u{2026}", false));
                v
            }
            MenuKind::Export => vec![
                item(CMD_EXPORT_XLSX, "Excel workbook (.xlsx) \u{2014} summary, costs, by sheet, items\u{2026}", false),
                item(CMD_EXPORT_CSV, "CSV (.csv)\u{2026}", false),
            ],
            MenuKind::Units => vec![
                item(CMD_BASE, "Feet-inches (12'-6 1/4\") and SF", self.takeoff.units == Units::FeetInches),
                item(CMD_BASE + 1, "Decimal feet (12.52') and SF", self.takeoff.units == Units::DecimalFeet),
                item(CMD_BASE + 2, "Metric (m and m\u{00B2})", self.takeoff.units == Units::Metric),
            ],
            MenuKind::ViewportScale => self.viewport_menu_items(),
            MenuKind::Presets => self.preset_menu_items(),
            MenuKind::Sheets => self.sheets_menu_items(),
            // Markup menus are handled in markups.rs.
            MenuKind::Stamp
            | MenuKind::MColor
            | MenuKind::MWidth
            | MenuKind::MFont
            | MenuKind::SavePdf
            | MenuKind::MarkupContext { .. }
            | MenuKind::FmtSize
            | MenuKind::FmtText
            | MenuKind::FmtLine
            | MenuKind::FmtFill
            | MenuKind::FmtWidth
            | MenuKind::FmtStatus => {
                Vec::new()
            }
            MenuKind::Context { id, vertex, hole } => {
                let Some(m) = self.takeoff.items.iter().find(|m| m.id == id) else { return Vec::new() };
                let mut groups: Vec<MenuItem> = self
                    .takeoff
                    .groups
                    .iter()
                    .enumerate()
                    .map(|(i, g)| item(CMD_SET_GROUP + i, g.name.clone(), g.name == m.group))
                    .collect();
                groups.push(sep());
                groups.push(item(CMD_NEW_GROUP, "New group\u{2026}", false));
                let mut layers: Vec<MenuItem> = self
                    .takeoff
                    .layers
                    .iter()
                    .enumerate()
                    .map(|(i, l)| item(CMD_SET_LAYER + i, l.name.clone(), l.name == m.layer))
                    .collect();
                layers.push(sep());
                layers.push(item(CMD_NEW_LAYER, "New layer\u{2026}", false));
                let mut v = vec![submenu(format!("Group: {}", m.group), groups), submenu(format!("Layer: {}", m.layer), layers)];
                if matches!(m.kind, Kind::Length | Kind::Polyline) {
                    v.push(submenu(format!("Line ends: {}", m.ends.name()), Self::style_items(CMD_ENDS, m.ends)));
                }
                v.push(submenu(format!("Leader: {}", m.leader.name()), Self::style_items(CMD_LEADER, m.leader)));
                if m.kind != Kind::Count {
                    v.push(sep());
                    let slope = match m.slope {
                        Some(r) => format!("Slope: {}\u{2026}", takeoff::fmt_slope(r)),
                        None => "Slope / pitch (roofs, ramps)\u{2026}".into(),
                    };
                    v.push(item(CMD_SET_SLOPE, slope, false));
                    if m.slope.is_some() {
                        v.push(item(CMD_CLEAR_SLOPE, "Remove slope", false));
                    }
                }
                if matches!(m.kind, Kind::Length | Kind::Polyline) {
                    let h = match m.height_in {
                        Some(h) => format!("Wall height: {}\u{2026}", takeoff::fmt_len(h, self.takeoff.units)),
                        None => "Wall height (adds wall area)\u{2026}".into(),
                    };
                    v.push(item(CMD_SET_HEIGHT, h, false));
                    if m.height_in.is_some() {
                        v.push(item(CMD_CLEAR_HEIGHT, "Remove wall height", false));
                    }
                }
                if m.label_offset.is_some() {
                    v.push(item(CMD_RESET_LABEL, "Put label back on the measurement", false));
                }
                if m.kind == Kind::Area {
                    v.push(sep());
                    v.push(item(CMD_ADD_CUTOUT, "Add cut-out\tX", false));
                    if hole.is_some() {
                        v.push(item(CMD_REMOVE_CUTOUT, "Remove this cut-out", false));
                    }
                    let depth = match m.depth_in {
                        Some(d) => format!("Depth / thickness: {}\u{2026}", takeoff::fmt_len(d, self.takeoff.units)),
                        None => "Set depth for volume\u{2026}".into(),
                    };
                    v.push(item(CMD_SET_DEPTH, depth, false));
                    if m.depth_in.is_some() {
                        v.push(item(CMD_CLEAR_DEPTH, "Remove depth (area only)", false));
                    }
                }
                if let Some((r, _)) = vertex {
                    let min = if r == 0 { m.kind.min_points() } else { 3 };
                    if ring(m, r).len() > min {
                        let what = if m.kind == Kind::Count { "Remove this marker" } else { "Delete this point" };
                        v.push(item(CMD_DELETE_POINT, what, false));
                    }
                }
                v.push(sep());
                v.push(item(CMD_DUPLICATE, "Duplicate\tCtrl+D", false));
                v.push(item(CMD_DELETE, "Delete\tDel", false));
                v
            }
        }
    }

    pub fn menu_result(&mut self, kind: MenuKind, cmd: usize) {
        if cmd == 0 {
            self.stamp_at = None;
            return;
        }
        if self.format_menu_result(kind, cmd) {
            self.invalidate();
            return;
        }
        if self.markup_menu_result(kind, cmd) {
            self.invalidate();
            return;
        }
        let sheet = self.takeoff_sheet();
        match kind {
            MenuKind::Scale => {
                let Some(sheet) = sheet else { return };
                if cmd == CMD_CALIBRATE {
                    self.set_tool(Tool::Calibrate);
                    self.status = Some("Calibrate: click two points a known distance apart".into());
                } else if cmd == CMD_APPLY_ALL {
                    if let (Some(sc), Some(doc)) = (self.takeoff.scale(sheet).cloned(), self.doc.as_ref()) {
                        self.undo.push(Undo::AllScales(self.takeoff.scales.clone()));
                        for p in 0..doc.pages.len() as u32 {
                            self.takeoff.scales.insert(p, sc.clone());
                        }
                        self.save_takeoff();
                    }
                } else if cmd == CMD_ADD_VP {
                    self.set_tool(Tool::Viewport);
                    self.status = Some("Detail scale: drag a box around the detail".into());
                } else if let Some(i) = cmd.checked_sub(CMD_VP_REMOVE).filter(|i| *i < self.takeoff.viewports.len()) {
                    self.remove_viewport(i);
                } else if cmd == CMD_CLEAR {
                    let old = self.takeoff.scales.remove(&sheet);
                    self.undo.push(Undo::Scale(sheet, old));
                    self.save_takeoff();
                } else if let Some((label, ratio)) = cmd.checked_sub(CMD_BASE).and_then(|i| SCALES.get(i)) {
                    let old = self.takeoff.scales.insert(sheet, Scale::from_ratio(label, *ratio));
                    self.undo.push(Undo::Scale(sheet, old));
                    self.save_takeoff();
                }
            }
            MenuKind::Group => self.group_menu(cmd),
            MenuKind::Export => {
                self.pending_export = match cmd {
                    CMD_EXPORT_XLSX => Some(ExportKind::Excel),
                    CMD_EXPORT_CSV => Some(ExportKind::Csv),
                    _ => None,
                }
            }
            MenuKind::Layer => self.layer_menu(cmd),
            MenuKind::Units => {
                self.takeoff.units = match cmd - CMD_BASE {
                    1 => Units::DecimalFeet,
                    2 => Units::Metric,
                    _ => Units::FeetInches,
                };
                self.save_takeoff();
            }
            MenuKind::Context { id, vertex, hole } => self.context_menu(id, vertex, hole, cmd),
            MenuKind::ViewportScale => self.viewport_menu_result(cmd),
            MenuKind::Presets => self.preset_menu_result(cmd),
            MenuKind::Sheets => self.sheets_menu_result(cmd),
            MenuKind::Stamp
            | MenuKind::MColor
            | MenuKind::MWidth
            | MenuKind::MFont
            | MenuKind::SavePdf
            | MenuKind::MarkupContext { .. }
            | MenuKind::FmtSize
            | MenuKind::FmtText
            | MenuKind::FmtLine
            | MenuKind::FmtFill
            | MenuKind::FmtWidth
            | MenuKind::FmtStatus => {}
        }
        self.invalidate();
    }

    fn group_menu(&mut self, cmd: usize) {
        let n = self.takeoff.groups.len();
        let idx = |base: usize| cmd.checked_sub(base).filter(|i| *i < n);
        if cmd == CMD_NEW_GROUP {
            self.ask_new_group(None);
        } else if let Some(i) = idx(CMD_GROUP_ACTIVE) {
            self.group = self.takeoff.groups[i].name.clone();
        } else if let Some(i) = idx(CMD_GROUP_RENAME) {
            self.prompt = Some(Prompt {
                title: format!("Rename group \u{201C}{}\u{201D}", self.takeoff.groups[i].name),
                hint: "New name".into(),
                text: self.takeoff.groups[i].name.clone(),
                purpose: PromptFor::RenameGroup(i),
            });
        } else {
            for (base, kind) in [(CMD_PRICE_LF, PriceKind::Lf), (CMD_PRICE_SF, PriceKind::Sf), (CMD_PRICE_CY, PriceKind::Cy), (CMD_PRICE_EA, PriceKind::Ea)] {
                if let Some(i) = idx(base) {
                    let g = &self.takeoff.groups[i];
                    let cur = match kind {
                        PriceKind::Lf => g.prices.per_lf,
                        PriceKind::Sf => g.prices.per_sf,
                        PriceKind::Cy => g.prices.per_cy,
                        PriceKind::Ea => g.prices.per_ea,
                    };
                    self.prompt = Some(Prompt {
                        title: format!("{} \u{2014} price per {}", g.name, kind.unit()),
                        hint: format!("Dollars per {} (e.g. 3.25). Leave blank to clear.", kind.unit()),
                        text: cur.map(|v| format!("{v:.2}")).unwrap_or_default(),
                        purpose: PromptFor::Price { group: i, kind },
                    });
                }
            }
        }
    }

    fn ask_new_group(&mut self, assign: Option<u64>) {
        self.prompt = Some(Prompt {
            title: "New group".into(),
            hint: "Name, e.g. Drywall, Flooring, Concrete".into(),
            text: String::new(),
            purpose: PromptFor::NewGroup { assign },
        });
    }

    fn ask_new_layer(&mut self, assign: Option<u64>) {
        self.prompt = Some(Prompt {
            title: "New layer".into(),
            hint: "Name, e.g. Base bid, Alt 1, Existing".into(),
            text: String::new(),
            purpose: PromptFor::NewLayer { assign },
        });
    }

    fn layer_menu(&mut self, cmd: usize) {
        let n = self.takeoff.layers.len();
        let idx = |base: usize| cmd.checked_sub(base).filter(|i| *i < n);
        if cmd == CMD_NEW_LAYER {
            self.ask_new_layer(None);
        } else if cmd == CMD_SHOW_ALL {
            for l in &mut self.takeoff.layers {
                l.visible = true;
            }
            self.save_takeoff();
        } else if let Some(i) = idx(CMD_LAYER_ACTIVE) {
            self.layer = self.takeoff.layers[i].name.clone();
            self.takeoff.layers[i].visible = true;
        } else if let Some(i) = idx(CMD_LAYER_VISIBLE) {
            let l = &mut self.takeoff.layers[i];
            l.visible = !l.visible;
            if !l.visible && self.selected_measure().is_some_and(|m| m.layer == self.takeoff.layers[i].name) {
                self.selected = None;
            }
            self.save_takeoff();
        } else if let Some(i) = idx(CMD_LAYER_LOCK) {
            let l = &mut self.takeoff.layers[i];
            l.locked = !l.locked;
            if l.locked && self.selected_measure().is_some_and(|m| m.layer == self.takeoff.layers[i].name) {
                self.selected = None;
            }
            self.save_takeoff();
        } else if let Some(i) = idx(CMD_LAYER_RENAME) {
            self.prompt = Some(Prompt {
                title: format!("Rename layer \u{201C}{}\u{201D}", self.takeoff.layers[i].name),
                hint: "New name".into(),
                text: self.takeoff.layers[i].name.clone(),
                purpose: PromptFor::RenameLayer(i),
            });
        } else if let Some(i) = idx(CMD_LAYER_DELETE) {
            self.delete_layer(i);
        }
    }

    fn delete_layer(&mut self, i: usize) {
        if self.takeoff.layers.len() < 2 {
            self.pending_info = Some("A drawing needs at least one layer. Create another layer first.".into());
            return;
        }
        self.undo.push(Undo::Layers(self.takeoff.items.clone(), self.takeoff.layers.clone()));
        let gone = self.takeoff.layers.remove(i).name;
        let to = self
            .takeoff
            .layers
            .iter()
            .find(|l| l.name == DEFAULT_LAYER)
            .or(self.takeoff.layers.first())
            .map(|l| l.name.clone())
            .unwrap_or_else(|| DEFAULT_LAYER.into());
        for m in self.takeoff.items.iter_mut().filter(|m| m.layer == gone) {
            m.layer = to.clone();
        }
        if self.layer == gone {
            self.layer = to;
        }
        self.save_takeoff();
    }

    fn context_menu(&mut self, id: u64, vertex: Option<(usize, usize)>, hole: Option<usize>, cmd: usize) {
        let Some(before) = self.takeoff.items.iter().find(|m| m.id == id).cloned() else { return };
        match cmd {
            CMD_ADD_CUTOUT => {
                self.selected = Some(id);
                return self.start_cutout();
            }
            CMD_SET_DEPTH => return self.ask_depth(id),
            CMD_SET_SLOPE => {
                let text = before.slope.map(takeoff::fmt_slope).unwrap_or_default();
                self.prompt = Some(Prompt {
                    title: "Slope / pitch".into(),
                    hint: "Rise per 12 (6 or 6:12), a ramp ratio (1:12), degrees (26.6\u{00B0}) or percent (8.33%)".into(),
                    text,
                    purpose: PromptFor::Slope { id },
                });
                return;
            }
            CMD_SET_HEIGHT => {
                let text = before.height_in.map(|h| takeoff::fmt_len(h, Units::FeetInches)).unwrap_or_default();
                self.prompt = Some(Prompt {
                    title: "Wall height".into(),
                    hint: "e.g. 9'-0\", 10', 2.7m \u{2014} adds wall area (priced per SF)".into(),
                    text,
                    purpose: PromptFor::WallHeight { id },
                });
                return;
            }
            CMD_NEW_GROUP => return self.ask_new_group(Some(id)),
            CMD_NEW_LAYER => return self.ask_new_layer(Some(id)),
            CMD_DUPLICATE => {
                self.selected = Some(id);
                return self.duplicate_selected();
            }
            CMD_DELETE => {
                self.selected = Some(id);
                return self.delete_selected();
            }
            _ => {}
        }
        let groups: Vec<String> = self.takeoff.groups.iter().map(|g| g.name.clone()).collect();
        let layers: Vec<String> = self.takeoff.layers.iter().map(|l| l.name.clone()).collect();
        let pick = |base: usize, n: usize| cmd.checked_sub(base).filter(|i| *i < n);
        let Some(m) = self.takeoff.get_mut(id) else { return };
        let mut new_ends = None;
        let mut new_leader = None;
        if let Some(i) = pick(CMD_SET_GROUP, groups.len()) {
            m.group = groups[i].clone();
        } else if let Some(i) = pick(CMD_SET_LAYER, layers.len()) {
            m.layer = layers[i].clone();
        } else if let Some(i) = pick(CMD_ENDS, EndStyle::ALL.len()) {
            m.ends = EndStyle::ALL[i];
            new_ends = Some(m.ends);
        } else if let Some(i) = pick(CMD_LEADER, EndStyle::ALL.len()) {
            m.leader = EndStyle::ALL[i];
            new_leader = Some(m.leader);
        } else if cmd == CMD_RESET_LABEL {
            m.label_offset = None;
        } else if cmd == CMD_DELETE_POINT {
            if let Some((r, v)) = vertex.filter(|(r, _)| *r < ring_count(m)) {
                let min = if r == 0 { m.kind.min_points() } else { 3 };
                let pts = ring_mut(m, r);
                if v < pts.len() && pts.len() > min {
                    pts.remove(v);
                }
            }
        } else if cmd == CMD_REMOVE_CUTOUT {
            if let Some(h) = hole.filter(|h| *h < m.holes.len()) {
                m.holes.remove(h);
            }
        } else if cmd == CMD_CLEAR_DEPTH {
            m.depth_in = None;
        } else if cmd == CMD_CLEAR_SLOPE {
            m.slope = None;
        } else if cmd == CMD_CLEAR_HEIGHT {
            m.height_in = None;
        } else {
            return;
        }
        let group = m.group.clone();
        // The style picked last becomes the default for new measurements.
        if let Some(e) = new_ends {
            self.takeoff.default_ends = e;
        }
        if let Some(l) = new_leader {
            self.takeoff.default_leader = l;
        }
        self.group = group;
        self.selected = Some(id);
        self.undo.push(Undo::Modified(before));
        self.save_takeoff();
    }

    /// Write the takeoff in the chosen format.
    pub fn export(&mut self, kind: ExportKind, path: PathBuf) {
        match kind {
            ExportKind::Csv => self.export_csv(path),
            ExportKind::Excel => {
                let title = self
                    .doc
                    .as_ref()
                    .and_then(|d| d.path.file_stem())
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Takeoff".into());
                match crate::xlsx::write(&self.takeoff, &path, &title, &|i| format!("Sheet {}", i + 1)) {
                    Ok(()) => {
                        self.pending_info = Some(format!(
                            "Exported {} measurements to\n{}\n\nTabs: Summary (with unit prices and live cost formulas), By sheet, Items.",
                            self.takeoff.items.len(),
                            path.display()
                        ))
                    }
                    Err(e) => self.pending_error = Some(format!("Could not write {}:\n{e}", path.display())),
                }
            }
        }
    }

    fn export_csv(&mut self, path: PathBuf) {
        let csv = self.takeoff.to_csv(&|i| format!("Sheet {}", i + 1));
        // UTF-8 BOM so Excel shows ², ° etc. correctly.
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(csv.as_bytes());
        match std::fs::write(&path, bytes) {
            Ok(()) => {
                self.pending_info =
                    Some(format!("Exported {} measurements to\n{}", self.takeoff.items.len(), path.display()))
            }
            Err(e) => self.pending_error = Some(format!("Could not write {}:\n{e}", path.display())),
        }
    }

    pub fn export_default_name(&self, kind: ExportKind) -> String {
        let stem = self
            .doc
            .as_ref()
            .and_then(|d| d.path.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "takeoff".into());
        match kind {
            ExportKind::Excel => format!("{stem} takeoff.xlsx"),
            ExportKind::Csv => format!("{stem} takeoff.csv"),
        }
    }

    // ------------------------------------------------------------- input

    pub(super) fn page_pt(&self, x: f32, y: f32) -> [f32; 2] {
        let r = self.view.screen_to_page(&RectF::new(x, y, x, y));
        [r.l, r.t]
    }

    pub(super) fn to_screen(&self, p: [f32; 2]) -> [f32; 2] {
        let r = self.view.page_to_screen(&RectF::new(p[0], p[1], p[0], p[1]));
        [r.l, r.t]
    }

    fn shift_down() -> bool {
        unsafe { GetKeyState(VK_SHIFT.0 as i32) < 0 }
    }

    /// The point a click at (x, y) would place: Shift = ortho, otherwise
    /// snapped to the drawing's line work when snapping is on.
    pub(super) fn place_pt(&self, x: f32, y: f32) -> [f32; 2] {
        self.place_pt_kind(x, y).0
    }

    fn place_pt_kind(&self, x: f32, y: f32) -> ([f32; 2], Option<SnapKind>) {
        let p = self.page_pt(x, y);
        match (Self::shift_down(), self.pending_pts.last()) {
            (true, Some(&last)) => (takeoff::ortho(last, p), None),
            _ => self.snap_pt(p),
        }
    }

    /// Left click on the canvas while a measuring tool is active.
    /// Returns false when the Select tool should handle it instead.
    pub(super) fn tools_canvas_down(&mut self, x: f32, y: f32) -> bool {
        let Some(sheet) = self.takeoff_sheet() else { return false };
        match self.tool {
            Tool::Select => false,
            Tool::Calibrate => {
                let p = self.place_pt(x, y);
                self.pending_pts.push(p);
                if self.pending_pts.len() == 2 {
                    let d = takeoff::dist(self.pending_pts[0], self.pending_pts[1]);
                    self.prompt = Some(Prompt {
                        title: "Calibrate scale".into(),
                        hint: "Real distance between the two points, e.g. 24'-6\" or 7.5m".into(),
                        text: String::new(),
                        purpose: PromptFor::Calibrate { sheet, dip_len: d },
                    });
                }
                self.invalidate();
                true
            }
            Tool::Markup(k) => self.markup_down(k, x, y),
            Tool::Viewport => self.markup_down(MKind::Rect, x, y),
            Tool::Measure(_) | Tool::Volume | Tool::Cutout(_) => {
                let p = self.place_pt(x, y);
                self.pending_pts.push(p);
                if self.tool == Tool::Measure(Kind::Length) && self.pending_pts.len() == 2 {
                    self.finish_measure();
                }
                self.invalidate();
                true
            }
        }
    }

    /// Double-click while measuring: the first click already placed the
    /// point, so finish. With the Select tool on a selected polyline/area
    /// edge: insert a point there.
    pub(super) fn tools_dblclick(&mut self, x: f32, y: f32) -> bool {
        if matches!(self.tool, Tool::Measure(_) | Tool::Volume | Tool::Cutout(_) | Tool::Markup(_)) && !self.pending_pts.is_empty() {
            self.finish_measure();
            return true;
        }
        if self.tool != Tool::Select {
            return false;
        }
        if self.markup_dblclick(x, y) {
            return true;
        }
        let p = self.page_pt(x, y);
        let tol = 6.0 * self.s() / self.view.zoom;
        let Some(m) = self.selected_measure().filter(|m| matches!(m.kind, Kind::Polyline | Kind::Area | Kind::Length)) else {
            return false;
        };
        let closed = m.kind == Kind::Area;
        let hit = (0..ring_count(m)).find_map(|r| {
            let pts = ring(m, r);
            let n = pts.len();
            let segs = if closed { n } else { n - 1 };
            (0..segs).find(|&i| takeoff::seg_dist2(p, pts[i], pts[(i + 1) % n]) <= tol * tol).map(|i| (r, i))
        });
        let Some((r, i)) = hit else { return false };
        let before = m.clone();
        let id = m.id;
        if let Some(m) = self.takeoff.get_mut(id) {
            if m.kind == Kind::Length {
                // A length with a bend becomes a polyline.
                m.kind = Kind::Polyline;
            }
            ring_mut(m, r).insert(i + 1, p);
        }
        self.undo.push(Undo::Modified(before));
        self.save_takeoff();
        self.invalidate();
        true
    }

    pub(super) fn finish_measure(&mut self) {
        if let Tool::Markup(_) = self.tool {
            self.finish_cloud_poly();
            return;
        }
        let (Some(kind), Some(sheet)) = (self.tool.kind(), self.takeoff_sheet()) else { return };
        if self.tool == Tool::Calibrate {
            return;
        }
        let mut pts = std::mem::take(&mut self.pending_pts);
        // A double-click can leave a duplicate final point.
        pts.dedup_by(|a, b| takeoff::dist(*a, *b) < 0.01);
        if pts.len() < kind.min_points() {
            self.invalidate();
            return;
        }
        if let Tool::Cutout(id) = self.tool {
            if let Some(before) = self.takeoff.items.iter().find(|m| m.id == id).cloned() {
                if let Some(m) = self.takeoff.get_mut(id) {
                    m.holes.push(pts);
                }
                self.undo.push(Undo::Modified(before));
                self.save_takeoff();
            }
            self.status = None;
            self.tool = Tool::Select;
            self.selected = Some(id);
            self.invalidate();
            return;
        }
        let (group, layer) = (self.group.clone(), self.layer.clone());
        let id = self.takeoff.add(sheet, kind, pts, &group, &layer);
        // Drawing on a hidden layer would make the new measurement vanish.
        if let Some(l) = self.takeoff.layers.iter_mut().find(|l| l.name == layer) {
            l.visible = true;
        }
        self.undo.push(Undo::Added(id));
        let first = self.takeoff.items.iter().find(|m| m.id == id).and_then(|m| m.points.first().copied()).unwrap_or([0.0, 0.0]);
        if self.takeoff.scale_at(sheet, first).is_none() && kind != Kind::Count {
            self.status = Some("This sheet has no scale \u{2014} set one from the Scale menu".into());
        }
        let preset_depth = self.preset_extras(id);
        self.save_takeoff();
        if self.tool == Tool::Volume && !preset_depth {
            self.ask_depth(id);
        }
        self.invalidate();
    }

    /// Screen rectangle of a measurement's label.
    fn label_rect(&self, m: &Measure) -> RectF {
        let text = self.label_text(m);
        let c = self.label_center(m);
        let s = self.s();
        let w = self.text_width(&text);
        RectF::new(c[0] - w / 2.0 - 6.0 * s, c[1] - 10.0 * s, c[0] + w / 2.0 + 6.0 * s, c[1] + 10.0 * s)
    }

    fn label_center(&self, m: &Measure) -> [f32; 2] {
        let mut c = self.to_screen(m.label_pos());
        if m.kind == Kind::Count && m.label_offset.is_none() {
            c[1] -= 20.0 * self.s();
        }
        c
    }

    fn label_text(&self, m: &Measure) -> String {
        let scale = self.takeoff.scale_of(m);
        let mut t = match m.kind {
            Kind::Count => format!("{} \u{00D7}{}", m.group, m.points.len()),
            _ => takeoff::fmt_qty(m.qty(scale), self.takeoff.units),
        };
        if let Some(w) = m.wall_area(scale) {
            t.push_str(&format!(" \u{00B7} {} wall", takeoff::fmt_area(w, self.takeoff.units)));
        }
        if let Some(r) = m.slope {
            t.push_str(&format!(" @ {}", takeoff::fmt_slope(r)));
        }
        t
    }

    fn text_width(&self, text: &str) -> f32 {
        let w: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            self.dwrite
                .CreateTextLayout(&w, &self.fonts.label, 2000.0, 100.0)
                .ok()
                .and_then(|l| {
                    let mut m = DWRITE_TEXT_METRICS::default();
                    l.GetMetrics(&mut m).ok().map(|_| m.width)
                })
                .unwrap_or(text.len() as f32 * 7.0 * self.s())
        }
    }

    /// Point handle of the selected measurement under (x, y): (ring, index).
    fn handle_at(&self, x: f32, y: f32) -> Option<(usize, usize)> {
        let m = self.selected_measure().filter(|m| self.editable(m))?;
        let r = 7.0 * self.s();
        (0..ring_count(m)).find_map(|ri| {
            ring(m, ri)
                .iter()
                .position(|p| {
                    let q = self.to_screen(*p);
                    (q[0] - x).abs() <= r && (q[1] - y).abs() <= r
                })
                .map(|i| (ri, i))
        })
    }

    /// Index of the selected area's cut-out under (x, y).
    fn hole_at(&self, id: u64, x: f32, y: f32) -> Option<usize> {
        let p = self.page_pt(x, y);
        let m = self.takeoff.items.iter().find(|m| m.id == id)?;
        m.holes.iter().position(|h| takeoff::point_in_polygon(p, h))
    }

    /// Topmost editable measurement whose label or body is under (x, y).
    /// Returns (id, on_label).
    fn measure_at(&self, x: f32, y: f32) -> Option<(u64, bool)> {
        let items: Vec<&Measure> = self.sheet_items().filter(|m| self.editable(m)).collect();
        if let Some(m) = items.iter().rev().find(|m| self.label_rect(m).contains_pt(x, y)) {
            return Some((m.id, true));
        }
        let p = self.page_pt(x, y);
        let tol = 6.0 * self.s() / self.view.zoom;
        items.iter().rev().find(|m| hits(m, p, tol)).map(|m| (m.id, false))
    }

    /// Left button down with the Select tool. Returns true if it started
    /// editing a measurement (otherwise the caller pans / click-selects).
    pub(super) fn select_mouse_down(&mut self, x: f32, y: f32) -> bool {
        if self.tool != Tool::Select || self.takeoff_sheet().is_none() {
            return false;
        }
        let start = self.page_pt(x, y);
        if self.markup_select_down(x, y, start) {
            return true;
        }
        self.set_sel_markup(None);
        if let (Some((ring, index)), Some(m)) = (self.handle_at(x, y), self.selected_measure()) {
            self.edit = Some(EditDrag::Vertex { orig: m.clone(), ring, index, moved: false });
            return true;
        }
        let Some((id, on_label)) = self.measure_at(x, y) else { return false };
        let Some(m) = self.takeoff.items.iter().find(|m| m.id == id).cloned() else { return false };
        self.selected = Some(id);
        self.group = m.group.clone();
        self.edit = Some(if on_label {
            EditDrag::Label { orig: m, start, moved: false }
        } else {
            EditDrag::Move { orig: m, start, moved: false }
        });
        self.invalidate();
        true
    }

    /// Mouse move during an edit drag. Returns true while editing.
    pub(super) fn edit_drag_move(&mut self, x: f32, y: f32) -> bool {
        let p = self.page_pt(x, y);
        let slop = 3.0 / self.view.zoom;
        let shift = Self::shift_down();
        // A count's label starts just above its first marker.
        let count_lift = [0.0, -20.0 * self.s() / self.view.zoom];
        let snapped = self.snap_pt(p).0;
        if self.markup_edit_move(p, slop) {
            return true;
        }
        let Some(edit) = self.edit.as_mut() else { return false };
        // Work out the edited measurement, or None if still within the
        // click slop (so a plain click doesn't nudge anything).
        let next: Option<Measure> = match edit {
            EditDrag::Move { orig, start, moved } => {
                let (dx, dy) = (p[0] - start[0], p[1] - start[1]);
                (*moved || dx.hypot(dy) >= slop).then(|| {
                    *moved = true;
                    let mut m = orig.clone();
                    for q in m.points.iter_mut().chain(m.holes.iter_mut().flatten()) {
                        q[0] += dx;
                        q[1] += dy;
                    }
                    m
                })
            }
            EditDrag::Vertex { orig, ring: r, index, moved } => {
                let (r, i) = (*r, *index);
                let pts = ring(orig, r);
                // Shift keeps the point square to its neighbour; otherwise snap.
                let target = match (shift, i.checked_sub(1).and_then(|j| pts.get(j))) {
                    (true, Some(prev)) if orig.kind != Kind::Count => takeoff::ortho(*prev, p),
                    _ => snapped,
                };
                (*moved || takeoff::dist(target, pts[i]) >= slop as f64).then(|| {
                    *moved = true;
                    let mut m = orig.clone();
                    ring_mut(&mut m, r)[i] = target;
                    m
                })
            }
            EditDrag::MarkupMove { .. } | EditDrag::MarkupHandle { .. } => None,
            EditDrag::Label { orig, start, moved } => {
                let (dx, dy) = (p[0] - start[0], p[1] - start[1]);
                (*moved || dx.hypot(dy) >= slop).then(|| {
                    *moved = true;
                    let mut m = orig.clone();
                    let o = orig.label_offset.unwrap_or(if orig.kind == Kind::Count { count_lift } else { [0.0, 0.0] });
                    m.label_offset = Some([o[0] + dx, o[1] + dy]);
                    m
                })
            }
        };
        if let Some(next) = next {
            if let Some(m) = self.takeoff.get_mut(next.id) {
                *m = next;
            }
            self.invalidate();
        }
        true
    }

    /// Mouse up after an edit drag. Returns true if an edit was in progress.
    pub(super) fn edit_drag_end(&mut self) -> bool {
        let Some(edit) = self.edit.take() else { return false };
        let (orig, moved) = match edit {
            EditDrag::Move { orig, moved, .. } | EditDrag::Vertex { orig, moved, .. } | EditDrag::Label { orig, moved, .. } => {
                (orig, moved)
            }
            EditDrag::MarkupMove { orig, moved, .. } | EditDrag::MarkupHandle { orig, moved, .. } => {
                if moved {
                    self.undo.push(Undo::MarkupModified(orig));
                    self.save_takeoff();
                }
                self.invalidate();
                return true;
            }
        };
        if moved {
            self.undo.push(Undo::Modified(orig));
            self.save_takeoff();
        }
        self.invalidate();
        true
    }

    /// Right click with the Select tool: properties menu for a measurement.
    pub(super) fn context_menu_at(&mut self, x: f32, y: f32) -> bool {
        if self.tool != Tool::Select || self.takeoff_sheet().is_none() {
            return false;
        }
        if let Some(id) = self.markup_at(x, y) {
            self.set_sel_markup(Some(id));
            self.selected = None;
            self.pending_menu = Some(MenuKind::MarkupContext { id });
            self.invalidate();
            return true;
        }
        let vertex = self.handle_at(x, y);
        let id = match (vertex, self.selected) {
            (Some(_), Some(id)) => id,
            _ => match self.measure_at(x, y) {
                Some((id, _)) => id,
                // Right-click inside a selected area's cut-out.
                None => match self.selected.filter(|id| self.hole_at(*id, x, y).is_some()) {
                    Some(id) => id,
                    None => return false,
                },
            },
        };
        let hole = self.hole_at(id, x, y);
        self.selected = Some(id);
        self.pending_menu = Some(MenuKind::Context { id, vertex, hole });
        self.invalidate();
        true
    }

    /// Cursor for the Select tool over editable things.
    pub(super) fn edit_cursor(&self, x: f32, y: f32) -> Option<PCWSTR> {
        if self.tool != Tool::Select || self.takeoff_sheet().is_none() {
            return None;
        }
        if self.edit.is_some() || self.handle_at(x, y).is_some() || self.markup_handle_at(x, y).is_some() || self.markup_at(x, y).is_some() {
            return Some(IDC_SIZEALL);
        }
        match self.measure_at(x, y) {
            Some((_, true)) => Some(IDC_HAND),
            Some((_, false)) => Some(IDC_SIZEALL),
            None => None,
        }
    }

    /// Select the measurement under (x, y), or clear the selection.
    pub(super) fn select_at(&mut self, x: f32, y: f32) {
        if self.takeoff_sheet().is_none() {
            return;
        }
        self.set_sel_markup(self.markup_at(x, y));
        if self.selected_markup.is_some() {
            self.selected = None;
            self.invalidate();
            return;
        }
        self.selected = self.measure_at(x, y).map(|(id, _)| id);
        if let Some(m) = self.selected_measure() {
            self.group = m.group.clone();
        }
        self.invalidate();
    }

    fn delete_selected(&mut self) {
        let ids = self.markup_selection();
        if !ids.is_empty() {
            for id in ids {
                if let Some(ix) = self.takeoff.markups.iter().position(|m| m.id == id) {
                    let m = self.takeoff.markups.remove(ix);
                    self.undo.push(Undo::MarkupRemoved(m));
                }
            }
            self.set_sel_markup(None);
            self.save_takeoff();
            self.invalidate();
            return;
        }
        let Some(id) = self.selected.take() else { return };
        if let Some(ix) = self.takeoff.items.iter().position(|m| m.id == id) {
            let m = self.takeoff.items.remove(ix);
            self.undo.push(Undo::Removed(m));
            self.save_takeoff();
        }
        self.invalidate();
    }

    pub(super) fn duplicate_selected(&mut self) {
        if self.selected_markup.is_some() {
            return self.duplicate_markup();
        }
        let (Some(m), Some(sheet)) = (self.selected_measure().cloned(), self.takeoff_sheet()) else { return };
        let off = 24.0 * self.s() / self.view.zoom;
        let id = self.takeoff.duplicate(&m, sheet, [off, off]);
        self.undo.push(Undo::Added(id));
        self.selected = Some(id);
        self.save_takeoff();
        self.invalidate();
    }

    /// Paste the copied measurement centred under the cursor.
    fn paste(&mut self) {
        let (Some(m), Some(sheet)) = (self.clip.clone(), self.takeoff_sheet()) else { return };
        let at = self.page_pt(self.mouse.0 as f32, self.mouse.1 as f32);
        let a = m.anchor();
        let id = self.takeoff.duplicate(&m, sheet, [at[0] - a[0], at[1] - a[1]]);
        self.undo.push(Undo::Added(id));
        self.selected = Some(id);
        self.set_tool_keep_selection();
        self.save_takeoff();
        self.invalidate();
    }

    fn set_tool_keep_selection(&mut self) {
        self.tool = Tool::Select;
        self.pending_pts.clear();
    }

    fn undo_last(&mut self) {
        if !self.pending_pts.is_empty() {
            self.pending_pts.pop();
            self.invalidate();
            return;
        }
        let Some(u) = self.undo.pop() else { return };
        match u {
            Undo::Added(id) => self.takeoff.items.retain(|m| m.id != id),
            Undo::Removed(m) => self.takeoff.items.push(m),
            Undo::Modified(m) => {
                if let Some(cur) = self.takeoff.get_mut(m.id) {
                    *cur = m;
                }
            }
            Undo::Scale(sheet, Some(s)) => {
                self.takeoff.scales.insert(sheet, s);
            }
            Undo::Scale(sheet, None) => {
                self.takeoff.scales.remove(&sheet);
            }
            Undo::AllScales(all) => self.takeoff.scales = all,
            Undo::Viewports(v) => self.takeoff.viewports = v,
            Undo::MarkupAdded(id) => self.takeoff.markups.retain(|m| m.id != id),
            Undo::MarkupRemoved(m) => self.takeoff.markups.push(m),
            Undo::MarkupModified(m) => {
                if let Some(cur) = self.takeoff.markups.iter_mut().find(|x| x.id == m.id) {
                    *cur = m;
                }
            }
            Undo::Whole(t) => {
                self.takeoff = *t;
                self.refresh_annot_visibility();
            }
            Undo::MarkupsModified(list) => {
                for m in list {
                    if let Some(cur) = self.takeoff.markups.iter_mut().find(|x| x.id == m.id) {
                        *cur = m;
                    }
                }
            }
            Undo::Groups(items, groups) => {
                self.takeoff.items = items;
                self.takeoff.groups = groups;
                if !self.takeoff.groups.iter().any(|g| g.name == self.group) {
                    self.group = self.takeoff.groups.first().map(|g| g.name.clone()).unwrap_or_else(|| "General".into());
                }
            }
            Undo::Layers(items, layers) => {
                self.takeoff.items = items;
                self.takeoff.layers = layers;
                if !self.takeoff.layers.iter().any(|l| l.name == self.layer) {
                    self.layer = self.takeoff.layers.first().map(|l| l.name.clone()).unwrap_or_else(|| DEFAULT_LAYER.into());
                }
            }
        }
        self.selected = None;
        self.set_sel_markup(None);
        self.save_takeoff();
        self.invalidate();
    }

    /// Keyboard for takeoffs. Returns true if handled.
    pub(super) fn tools_key(&mut self, vk: VIRTUAL_KEY, ctrl: bool) -> bool {
        if self.prompt.is_some() {
            let multiline = matches!(self.prompt.as_ref().map(|p| &p.purpose), Some(PromptFor::MarkupText { .. }));
            let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
            match vk {
                VK_RETURN if multiline && shift => {
                    if let Some(p) = self.prompt.as_mut() {
                        p.text.push('\n');
                    }
                    self.invalidate();
                }
                _ if multiline && ctrl && vk.0 == b'V' as u16 => {
                    if let Some(t) = super::clipboard_text() {
                        if let Some(p) = self.prompt.as_mut() {
                            p.text.push_str(&t.replace("\r\n", "\n").replace('\r', "\n"));
                            let keep: String = p.text.chars().take(4000).collect();
                            p.text = keep;
                        }
                        self.invalidate();
                    }
                }
                VK_RETURN => self.prompt_confirm(),
                VK_ESCAPE => {
                    self.prompt = None;
                    self.pending_pts.clear();
                    self.invalidate();
                }
                VK_BACK => {
                    if let Some(p) = self.prompt.as_mut() {
                        p.text.pop();
                    }
                    self.invalidate();
                }
                _ => {}
            }
            // Swallow everything else; characters arrive via WM_CHAR.
            return true;
        }
        if self.doc.is_none() {
            return false;
        }
        if self.format_key(vk, ctrl) {
            return true;
        }
        let letter = |c: u8| vk.0 == c as u16 && !ctrl;
        let ctrl_key = |c: u8| vk.0 == c as u16 && ctrl;
        match vk {
            _ if ctrl_key(b'Z') => self.undo_last(),
            _ if ctrl_key(b'C') && self.selected.is_some() => self.clip = self.selected_measure().cloned(),
            _ if ctrl_key(b'V') && self.clip.is_some() => self.paste(),
            _ if ctrl_key(b'D') && (self.selected.is_some() || self.selected_markup.is_some()) => self.duplicate_selected(),
            _ if letter(b'M') => {
                self.markup_mode = !self.markup_mode;
                self.set_tool(Tool::Select);
            }
            _ if self.markup_mode && self.markup_key(vk) => {}
            VK_DELETE => self.delete_selected(),
            VK_ESCAPE if !self.pending_pts.is_empty() => {
                self.pending_pts.clear();
                self.invalidate();
            }
            VK_ESCAPE if self.tool != Tool::Select => {
                let keep = match self.tool {
                    Tool::Cutout(id) => Some(id),
                    _ => None,
                };
                self.status = None;
                self.set_tool(Tool::Select);
                self.selected = keep;
            }
            VK_ESCAPE if self.selected.is_some() || self.selected_markup.is_some() || self.mdraft.is_some() => {
                self.selected = None;
                self.set_sel_markup(None);
                self.mdraft = None;
                self.invalidate();
            }
            VK_RETURN if !self.pending_pts.is_empty() && self.goto_buf.is_empty() => self.finish_measure(),
            _ if letter(b'L') => self.set_tool(Tool::Measure(Kind::Length)),
            _ if letter(b'Y') => self.set_tool(Tool::Measure(Kind::Polyline)),
            _ if letter(b'A') => self.set_tool(Tool::Measure(Kind::Area)),
            _ if letter(b'C') => self.set_tool(Tool::Measure(Kind::Count)),
            _ if letter(b'V') => self.set_tool(Tool::Select),
            _ if letter(b'U') => self.set_tool(Tool::Volume),
            _ if letter(b'X') => self.start_cutout(),
            _ if letter(b'S') => self.toggle_snap(),
            _ if letter(b'Q') => {
                self.show_qty = !self.show_qty;
                self.invalidate();
            }
            _ => return false,
        }
        true
    }

    /// Typed characters while a prompt is open. Returns true if consumed.
    pub(super) fn tools_char(&mut self, c: char) -> bool {
        let Some(p) = self.prompt.as_mut() else { return false };
        let max = if matches!(p.purpose, PromptFor::MarkupText { .. }) { 4000 } else { 40 };
        if !c.is_control() && p.text.chars().count() < max {
            p.text.push(c);
            self.invalidate();
        }
        true
    }

    fn prompt_confirm(&mut self) {
        let Some(p) = self.prompt.take() else { return };
        match p.purpose {
            PromptFor::Calibrate { sheet, dip_len } => match takeoff::parse_len(&p.text).and_then(|real| Scale::calibrated(dip_len, real)) {
                Some(sc) => {
                    match self.calib_vp.take() {
                        // Calibrating a detail viewport.
                        Some(rect) => self.add_viewport(sheet, rect, Scale { label: format!("{} (calibrated)", "Detail"), ..sc }),
                        None => {
                            let old = self.takeoff.scales.insert(sheet, sc);
                            self.undo.push(Undo::Scale(sheet, old));
                            self.save_takeoff();
                        }
                    }
                    self.status = None;
                    self.set_tool(Tool::Select);
                }
                None => {
                    // Keep the points and ask again.
                    self.prompt = Some(Prompt {
                        hint: format!("Couldn't read \u{201C}{}\u{201D}. Try 24'-6\", 24.5', 294\" or 7.5m", p.text),
                        text: String::new(),
                        ..p
                    });
                }
            },
            PromptFor::NewGroup { assign } => {
                let name = self.takeoff.ensure_group(&p.text);
                self.group = name.clone();
                if let Some(before) = assign.and_then(|id| self.takeoff.items.iter().find(|m| m.id == id).cloned()) {
                    if let Some(m) = self.takeoff.get_mut(before.id) {
                        m.group = name;
                    }
                    self.undo.push(Undo::Modified(before));
                }
                self.save_takeoff();
            }
            PromptFor::NewLayer { assign } => {
                let name = self.takeoff.ensure_layer(&p.text);
                if let Some(before) = assign.and_then(|id| self.takeoff.items.iter().find(|m| m.id == id).cloned()) {
                    if let Some(m) = self.takeoff.get_mut(before.id) {
                        m.layer = name;
                    }
                    self.undo.push(Undo::Modified(before));
                } else {
                    self.layer = name;
                }
                self.save_takeoff();
            }
            PromptFor::Depth { id } => match takeoff::parse_len(&p.text) {
                Some(d) => {
                    if let Some(before) = self.takeoff.items.iter().find(|m| m.id == id).cloned() {
                        if let Some(m) = self.takeoff.get_mut(id) {
                            m.depth_in = Some(d);
                        }
                        // Fold the area into the undo step that created it, if any.
                        self.undo.push(Undo::Modified(before));
                        self.last_depth = Some(p.text.trim().to_string());
                        self.save_takeoff();
                    }
                }
                None if p.text.trim().is_empty() => {}
                None => {
                    self.prompt = Some(Prompt {
                        hint: format!("Couldn't read \u{201C}{}\u{201D}. Try 6\", 4 1/2\", 0.5' or 150mm", p.text),
                        text: String::new(),
                        ..p
                    });
                }
            },
            PromptFor::Price { group, kind } => match parse_money(&p.text) {
                Some(v) => {
                    if let Some(g) = self.takeoff.groups.get_mut(group) {
                        let slot = match kind {
                            PriceKind::Lf => &mut g.prices.per_lf,
                            PriceKind::Sf => &mut g.prices.per_sf,
                            PriceKind::Cy => &mut g.prices.per_cy,
                            PriceKind::Ea => &mut g.prices.per_ea,
                        };
                        *slot = v;
                        self.show_qty = true;
                        self.save_takeoff();
                    }
                }
                None => {
                    self.prompt = Some(Prompt {
                        hint: format!("Couldn't read \u{201C}{}\u{201D}. Type a number like 3.25 (blank clears)", p.text),
                        text: String::new(),
                        ..p
                    })
                }
            },
            PromptFor::RenameGroup(i) => {
                let new = p.text.trim().to_string();
                let taken = self.takeoff.groups.iter().enumerate().any(|(j, g)| j != i && g.name == new);
                if !new.is_empty() && !taken && i < self.takeoff.groups.len() {
                    self.undo.push(Undo::Groups(self.takeoff.items.clone(), self.takeoff.groups.clone()));
                    let old = std::mem::replace(&mut self.takeoff.groups[i].name, new.clone());
                    for m in self.takeoff.items.iter_mut().filter(|m| m.group == old) {
                        m.group = new.clone();
                    }
                    if self.group == old {
                        self.group = new;
                    }
                    self.save_takeoff();
                }
            }
            PromptFor::Slope { id } => {
                let parsed = takeoff::parse_slope(&p.text);
                if parsed.is_none() && !p.text.trim().is_empty() {
                    self.prompt = Some(Prompt {
                        hint: format!("Couldn't read \u{201C}{}\u{201D}. Try 6, 6:12, 1:12, 26.6\u{00B0} or 8.33%", p.text),
                        text: String::new(),
                        ..p
                    });
                } else if let Some(before) = self.takeoff.items.iter().find(|m| m.id == id).cloned() {
                    if let Some(m) = self.takeoff.get_mut(id) {
                        m.slope = parsed;
                    }
                    self.undo.push(Undo::Modified(before));
                    self.save_takeoff();
                }
            }
            PromptFor::WallHeight { id } => {
                let parsed = takeoff::parse_len(&p.text);
                if parsed.is_none() && !p.text.trim().is_empty() {
                    self.prompt = Some(Prompt {
                        hint: format!("Couldn't read \u{201C}{}\u{201D}. Try 9'-0\", 10' or 2.7m", p.text),
                        text: String::new(),
                        ..p
                    });
                } else if let Some(before) = self.takeoff.items.iter().find(|m| m.id == id).cloned() {
                    if let Some(m) = self.takeoff.get_mut(id) {
                        m.height_in = parsed;
                    }
                    self.undo.push(Undo::Modified(before));
                    self.save_takeoff();
                }
            }
            PromptFor::MarkupText { id } => self.set_markup_text(id, p.text.clone()),
            PromptFor::Find => self.run_find(p.text.clone()),
            PromptFor::PresetName => self.save_preset(p.text.clone()),
            PromptFor::SheetRange { delete } => self.sheet_range_entered(&p.text.clone(), delete),
            PromptFor::GoTo => self.goto_sheet(p.text.clone()),
            PromptFor::RenameLayer(i) => {
                let new = p.text.trim().to_string();
                let taken = self.takeoff.layers.iter().enumerate().any(|(j, l)| j != i && l.name == new);
                if !new.is_empty() && !taken && i < self.takeoff.layers.len() {
                    self.undo.push(Undo::Layers(self.takeoff.items.clone(), self.takeoff.layers.clone()));
                    let old = std::mem::replace(&mut self.takeoff.layers[i].name, new.clone());
                    for m in self.takeoff.items.iter_mut().filter(|m| m.layer == old) {
                        m.layer = new.clone();
                    }
                    if self.layer == old {
                        self.layer = new;
                    }
                    self.save_takeoff();
                }
            }
        }
        self.invalidate();
    }

    // ---------------------------------------------------------- painting

    fn draw_label(&self, m: &Measure, color: u32, strong: bool) {
        let text = self.label_text(m);
        let r = self.label_rect(m);
        self.fill(&r, if strong { color } else { 0xffffff }, if strong { 1.0 } else { 0.92 });
        unsafe {
            self.brush.SetColor(&rgb(color, 1.0));
            self.gfx.ctx.DrawRectangle(&d2r(&r), &self.brush, 1.0, None);
        }
        self.text(&text, &self.fonts.label, &r, if strong { 0xffffff } else { color });
    }

    /// Readout box next to the cursor while drawing.
    fn readout(&self, text: &str, center: [f32; 2], color: u32) {
        let s = self.s();
        let w = self.text_width(text);
        let r = RectF::new(center[0] - w / 2.0 - 6.0 * s, center[1] - 10.0 * s, center[0] + w / 2.0 + 6.0 * s, center[1] + 10.0 * s);
        self.fill(&r, color, 1.0);
        self.text(text, &self.fonts.label, &r, 0xffffff);
    }

    fn geometry(&self, screen: &[[f32; 2]], closed: bool) -> Option<ID2D1PathGeometry> {
        self.geometry_rings(&[screen.to_vec()], closed)
    }

    /// Path with one figure per ring; with the default alternate fill mode,
    /// inner rings (cut-outs) are left unfilled.
    pub(super) fn geometry_rings(&self, rings: &[Vec<[f32; 2]>], closed: bool) -> Option<ID2D1PathGeometry> {
        if rings.first().is_none_or(|r| r.len() < 2) {
            return None;
        }
        unsafe {
            let factory = self.gfx.ctx.GetFactory().ok()?;
            let geom = factory.CreatePathGeometry().ok()?;
            let sink = geom.Open().ok()?;
            for r in rings.iter().filter(|r| r.len() >= 2) {
                let pts: Vec<Vector2> = r.iter().map(|p| v2(*p)).collect();
                sink.BeginFigure(pts[0], if closed { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW });
                sink.AddLines(&pts[1..]);
                sink.EndFigure(if closed { D2D1_FIGURE_END_CLOSED } else { D2D1_FIGURE_END_OPEN });
            }
            sink.Close().ok()?;
            Some(geom)
        }
    }

    /// An area with its cut-outs.
    fn area_shape(&self, m: &Measure, color: u32, fill_alpha: f32, width: f32) {
        let rings: Vec<Vec<[f32; 2]>> = (0..ring_count(m)).map(|r| ring(m, r).iter().map(|p| self.to_screen(*p)).collect()).collect();
        let Some(geom) = self.geometry_rings(&rings, true) else { return };
        unsafe {
            self.brush.SetColor(&rgb(color, fill_alpha));
            self.gfx.ctx.FillGeometry(&geom, &self.brush, None);
            self.brush.SetColor(&rgb(color, 1.0));
            self.gfx.ctx.DrawGeometry(&geom, &self.brush, width, None);
        }
    }

    fn polygon(&self, pts: &[[f32; 2]], closed: bool, color: u32, fill_alpha: f32, width: f32) {
        let screen: Vec<[f32; 2]> = pts.iter().map(|p| self.to_screen(*p)).collect();
        let Some(geom) = self.geometry(&screen, closed) else { return };
        let ctx = &self.gfx.ctx;
        unsafe {
            if closed && fill_alpha > 0.0 {
                self.brush.SetColor(&rgb(color, fill_alpha));
                ctx.FillGeometry(&geom, &self.brush, None);
            }
            self.brush.SetColor(&rgb(color, 1.0));
            ctx.DrawGeometry(&geom, &self.brush, width, None);
        }
    }

    fn dot(&self, c: [f32; 2], r: f32, color: u32, outline: bool) {
        let e = D2D1_ELLIPSE { point: v2(c), radiusX: r, radiusY: r };
        unsafe {
            self.brush.SetColor(&rgb(color, 1.0));
            self.gfx.ctx.FillEllipse(&e, &self.brush);
            if outline {
                self.brush.SetColor(&rgb(0xffffff, 1.0));
                self.gfx.ctx.DrawEllipse(&e, &self.brush, 1.5, None);
            }
        }
    }

    pub(super) fn line(&self, a: [f32; 2], b: [f32; 2], color: u32, width: f32) {
        unsafe {
            self.brush.SetColor(&rgb(color, 1.0));
            self.gfx.ctx.DrawLine(v2(a), v2(b), &self.brush, width, None);
        }
    }

    /// Draw an end cap at `tip` (screen), pointing away from `from`.
    fn cap(&self, style: EndStyle, tip: [f32; 2], from: [f32; 2], color: u32) {
        let s = self.s();
        let (dx, dy) = (tip[0] - from[0], tip[1] - from[1]);
        let len = dx.hypot(dy);
        if len < 1e-3 {
            return;
        }
        let (ux, uy) = (dx / len, dy / len);
        let (nx, ny) = (-uy, ux);
        let (l, w) = (12.0 * s, 4.5 * s);
        let base = [tip[0] - ux * l, tip[1] - uy * l];
        let left = [base[0] + nx * w, base[1] + ny * w];
        let right = [base[0] - nx * w, base[1] - ny * w];
        match style {
            EndStyle::None => {}
            EndStyle::ClosedArrow => {
                if let Some(g) = self.geometry(&[tip, left, right], true) {
                    unsafe {
                        self.brush.SetColor(&rgb(color, 1.0));
                        self.gfx.ctx.FillGeometry(&g, &self.brush, None);
                    }
                }
            }
            EndStyle::OpenArrow => {
                self.line(tip, left, color, 2.0 * s);
                self.line(tip, right, color, 2.0 * s);
            }
            EndStyle::Tick => {
                // 45° slash across the end, like an architectural dimension.
                let (tx, ty) = ((ux + nx) * std::f32::consts::FRAC_1_SQRT_2, (uy + ny) * std::f32::consts::FRAC_1_SQRT_2);
                let h = 7.0 * s;
                self.line([tip[0] - tx * h, tip[1] - ty * h], [tip[0] + tx * h, tip[1] + ty * h], color, 2.5 * s);
            }
            EndStyle::Dot => self.dot(tip, 3.5 * s, color, false),
            EndStyle::Square => {
                let h = 3.5 * s;
                self.fill(&RectF::new(tip[0] - h, tip[1] - h, tip[0] + h, tip[1] + h), color, 1.0);
            }
            EndStyle::Circle => unsafe {
                let e = D2D1_ELLIPSE { point: v2(tip), radiusX: 5.0 * s, radiusY: 5.0 * s };
                self.brush.SetColor(&rgb(color, 1.0));
                self.gfx.ctx.DrawEllipse(&e, &self.brush, 2.0 * s, None);
            },
        }
    }

    /// Revision cloud around a screen rectangle: outward scallops along a
    /// clockwise path.
    pub(super) fn draw_cloud(&self, r: &RectF, color: u32, width: f32) {
        let s = self.s();
        let scallop = 16.0 * s;
        unsafe {
            let Ok(factory) = self.gfx.ctx.GetFactory() else { return };
            let Ok(geom) = factory.CreatePathGeometry() else { return };
            let Ok(sink) = geom.Open() else { return };
            let corners = [[r.l, r.t], [r.r, r.t], [r.r, r.b], [r.l, r.b]];
            sink.BeginFigure(v2(corners[0]), D2D1_FIGURE_BEGIN_HOLLOW);
            for i in 0..4 {
                let (a, b) = (corners[i], corners[(i + 1) % 4]);
                let len = (b[0] - a[0]).hypot(b[1] - a[1]);
                let n = (len / scallop).round().max(1.0) as usize;
                let step = len / n as f32;
                for k in 1..=n {
                    let t = k as f32 / n as f32;
                    let p = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
                    sink.AddArc(&D2D1_ARC_SEGMENT {
                        point: v2(p),
                        size: D2D_SIZE_F { width: step / 2.0, height: step / 2.0 },
                        rotationAngle: 0.0,
                        sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                        arcSize: D2D1_ARC_SIZE_SMALL,
                    });
                }
            }
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            if sink.Close().is_err() {
                return;
            }
            self.brush.SetColor(&rgb(color, 1.0));
            self.gfx.ctx.DrawGeometry(&geom, &self.brush, width, None);
        }
    }

    /// The Red-Beam logo (same design as the app icon) at (x, y), `size` px.
    pub(super) fn draw_logo(&self, x: f32, y: f32, size: f32) {
        let k = size / 1024.0;
        let pt = |px: f32, py: f32| [x + px * k, y + py * k];
        unsafe {
            let tile = D2D1_ROUNDED_RECT {
                rect: d2r(&RectF::new(x + 24.0 * k, y + 24.0 * k, x + 1000.0 * k, y + 1000.0 * k)),
                radiusX: 200.0 * k,
                radiusY: 200.0 * k,
            };
            self.brush.SetColor(&rgb(0x2a2e33, 1.0));
            self.gfx.ctx.FillRoundedRectangle(&tile, &self.brush);
        }
        let (fl_l, fl_r, web_l, web_r, top, top_b, bot_t, bot) = (212.0, 812.0, 432.0, 592.0, 196.0, 356.0, 668.0, 828.0);
        let beam = |d: f32| -> Vec<[f32; 2]> {
            [
                (fl_l, top), (fl_r, top), (fl_r, top_b), (web_r, top_b), (web_r, bot_t), (fl_r, bot_t),
                (fl_r, bot), (fl_l, bot), (fl_l, bot_t), (web_l, bot_t), (web_l, top_b), (fl_l, top_b),
            ]
            .iter()
            .map(|(a, b)| pt(a + d, b + d))
            .collect()
        };
        for (shape, color) in [(beam(48.0), 0x781412), (beam(0.0), 0xe53935)] {
            if let Some(g) = self.geometry(&shape, true) {
                unsafe {
                    self.brush.SetColor(&rgb(color, 1.0));
                    self.gfx.ctx.FillGeometry(&g, &self.brush, None);
                }
            }
        }
        // Highlight on the top flange.
        let hl = RectF::new(pt(fl_l, top)[0], pt(fl_l, top)[1], pt(fl_r, top + 34.0)[0], pt(fl_r, top + 34.0)[1]);
        self.fill(&hl, 0xff7864, 1.0);
    }

    /// Measurements on the current sheet, plus the one being drawn.
    pub(super) fn paint_takeoff(&self) {
        let Some(sheet) = self.takeoff_sheet() else { return };
        self.paint_viewports();
        let s = self.s();
        let units = self.takeoff.units;
        let scale = self.pending_pts.first().and_then(|p| self.takeoff.scale_at(sheet, *p)).or(self.takeoff.scale(sheet));
        for m in self.sheet_items() {
            let color = self.takeoff.group_color(&m.group);
            let sel = self.selected == Some(m.id);
            let w = if sel { 3.5 * s } else { 2.0 * s };
            let screen: Vec<[f32; 2]> = m.points.iter().map(|p| self.to_screen(*p)).collect();
            match m.kind {
                Kind::Count => {
                    for c in &screen {
                        self.dot(*c, if sel { 8.0 * s } else { 6.0 * s }, color, true);
                    }
                }
                Kind::Area => self.area_shape(m, color, if sel { 0.30 } else { 0.18 }, w),
                Kind::Length | Kind::Polyline => {
                    self.polygon(&m.points, false, color, 0.0, w);
                    let n = screen.len();
                    if n >= 2 {
                        self.cap(m.ends, screen[0], screen[1], color);
                        self.cap(m.ends, screen[n - 1], screen[n - 2], color);
                    }
                }
            }
            // Leader from a moved label back to the measurement.
            if m.label_offset.is_some() {
                let anchor = self.to_screen(m.anchor());
                let lc = self.label_center(m);
                if (lc[0] - anchor[0]).hypot(lc[1] - anchor[1]) > 14.0 * s {
                    self.line(lc, anchor, color, 1.5 * s);
                    self.cap(m.leader, anchor, lc, color);
                }
            }
            self.draw_label(m, color, sel);
            // Editing handles on the selected measurement (outline and cut-outs).
            if sel && self.editable(m) {
                let h = 4.5 * s;
                let all: Vec<[f32; 2]> = (0..ring_count(m)).flat_map(|r| ring(m, r).to_vec()).map(|p| self.to_screen(p)).collect();
                for c in &all {
                    let r = RectF::new(c[0] - h, c[1] - h, c[0] + h, c[1] + h);
                    self.fill(&r, 0xffffff, 1.0);
                    unsafe {
                        self.brush.SetColor(&rgb(color, 1.0));
                        self.gfx.ctx.DrawRectangle(&d2r(&r), &self.brush, 1.5 * s, None);
                    }
                }
            }
        }

        // In-progress measurement with a rubber band to the cursor.
        if self.pending_pts.is_empty() || (self.prompt.is_some() && self.tool != Tool::Calibrate) {
            return;
        }
        let (mx, my) = (self.mouse.0 as f32, self.mouse.1 as f32);
        let mut pts = self.pending_pts.clone();
        let calibrating = self.tool == Tool::Calibrate;
        if !(calibrating && pts.len() == 2) {
            pts.push(self.place_pt(mx, my));
        }
        let kind = self.tool.kind().unwrap_or(Kind::Length);
        let color = match self.tool {
            Tool::Measure(_) | Tool::Volume => self.takeoff.group_color(&self.group),
            _ => ACCENT,
        };
        let at = [mx, my - 22.0 * s];
        match kind {
            Kind::Count => {
                for p in &self.pending_pts {
                    self.dot(self.to_screen(*p), 6.0 * s, color, true);
                }
                self.readout(&format!("{} ea \u{2014} Enter to finish", self.pending_pts.len()), at, color);
                return;
            }
            Kind::Area => self.polygon(&pts, pts.len() >= 3, color, 0.15, 2.0 * s),
            _ => self.polygon(&pts, false, color, 0.0, 2.0 * s),
        }
        for p in &pts[..pts.len() - 1] {
            self.dot(self.to_screen(*p), 3.5 * s, color, true);
        }
        if calibrating {
            let d = takeoff::path_len(&pts);
            let txt = match scale {
                Some(sc) => format!("currently {}", takeoff::fmt_len(d * sc.real_in_per_dip, units)),
                None => "click the second point".into(),
            };
            self.readout(&txt, at, color);
            return;
        }
        let tmp = Measure {
            id: 0,
            sheet,
            kind,
            points: pts.clone(),
            group: String::new(),
            layer: String::new(),
            ends: EndStyle::None,
            label_offset: None,
            leader: EndStyle::None,
            holes: Vec::new(),
            depth_in: None,
            slope: None,
            height_in: None,
            source: None,
        };
        let mut readout = takeoff::fmt_qty(tmp.qty(scale), units);
        if let Qty::Area { perimeter, .. } = tmp.qty(scale) {
            readout = format!("{readout}  \u{00B7}  perim {}", takeoff::fmt_len(perimeter, units));
        }
        if let Tool::Cutout(_) = self.tool {
            readout = format!("cut-out \u{2212}{readout}");
        }
        if kind != Kind::Length {
            readout.push_str("  \u{2014} Enter / double-click to finish");
        }
        self.readout(&readout, at, color);
    }

    /// Marker showing what the cursor will snap to.
    pub(super) fn paint_snap_marker(&self) {
        let editing_vertex = matches!(self.edit, Some(EditDrag::Vertex { .. }));
        if self.tool == Tool::Select && !editing_vertex || self.prompt.is_some() {
            return;
        }
        let (mx, my) = (self.mouse.0 as f32, self.mouse.1 as f32);
        if self.in_bar(my) || self.over_tool_ui(mx, my) {
            return;
        }
        let (p, kind) = if editing_vertex { self.snap_pt(self.page_pt(mx, my)) } else { self.place_pt_kind(mx, my) };
        let Some(kind) = kind else { return };
        let c = self.to_screen(p);
        let s = self.s();
        let h = 6.0 * s;
        let color = 0x00c853;
        unsafe {
            self.brush.SetColor(&rgb(color, 1.0));
            match kind {
                SnapKind::Endpoint => {
                    self.gfx.ctx.DrawRectangle(&d2r(&RectF::new(c[0] - h, c[1] - h, c[0] + h, c[1] + h)), &self.brush, 2.0 * s, None);
                }
                SnapKind::OnLine => {
                    self.line([c[0] - h, c[1] - h], [c[0] + h, c[1] + h], color, 2.0 * s);
                    self.line([c[0] - h, c[1] + h], [c[0] + h, c[1] - h], color, 2.0 * s);
                }
            }
        }
    }

    /// Tool palette, quantities panel and prompt (screen-space UI).
    pub(super) fn paint_tools_ui(&self) {
        let s = self.s();
        let rows = self.toolbar_rects();
        if !rows.is_empty() {
            for r in &rows {
                self.fill(r, 0x1e2023, 0.92);
            }
            for (b, r, label) in self.toolbar_layout() {
                if b == TBtn::FmtInfo {
                    self.text(&label, &self.fonts.ui_center, &r, TEXT_DIM);
                    continue;
                }
                let active = match b {
                    TBtn::Mode(m) => self.markup_mode == m,
                    TBtn::MTool(k) => self.tool == Tool::Markup(k) || (k == MKind::Cloud && self.tool == Tool::Markup(MKind::CloudPoly)),
                    TBtn::Stamp => self.tool == Tool::Markup(MKind::Stamp),
                    TBtn::Select => self.tool == Tool::Select,
                    TBtn::Tool(k) => self.tool == Tool::Measure(k),
                    TBtn::Volume => self.tool == Tool::Volume,
                    TBtn::Snap => self.snap_on,
                    TBtn::Scale => self.tool == Tool::Calibrate,
                    TBtn::Quantities => self.show_qty,
                    TBtn::Bold | TBtn::Italic => self.format_active(b),
                    _ => false,
                };
                let hover = self.tool_hover == Some(b);
                let warn = b == TBtn::Scale && self.takeoff_sheet().is_some_and(|p| self.takeoff.scale(p).is_none());
                let bg = if active || hover { ACCENT } else { BTN_BG };
                self.fill(&r, bg, 1.0);
                if matches!(b, TBtn::MColor) {
                    let sw = RectF::new(r.l + 8.0 * s, r.t + 8.0 * s, r.l + 24.0 * s, r.b - 8.0 * s);
                    self.fill(&sw, self.settings.markup_color, 1.0);
                    let tr = RectF::new(r.l + 20.0 * s, r.t, r.r, r.b);
                    self.text(&label, &self.fonts.ui_center, &tr, TEXT);
                } else if let TBtn::Group = b {
                    let sw = RectF::new(r.l + 6.0 * s, r.t + 10.0 * s, r.l + 16.0 * s, r.b - 10.0 * s);
                    self.fill(&sw, self.takeoff.group_color(&self.group), 1.0);
                    let tr = RectF::new(r.l + 14.0 * s, r.t, r.r, r.b);
                    self.text(&label, &self.fonts.ui_center, &tr, TEXT);
                } else if let Some(c) = self.format_swatch(b) {
                    let sw = RectF::new(r.l + 8.0 * s, r.t + 8.0 * s, r.l + 22.0 * s, r.b - 8.0 * s);
                    self.fill(&sw, 0x000000, 0.6);
                    let inner = RectF::new(sw.l + 1.0, sw.t + 1.0, sw.r - 1.0, sw.b - 1.0);
                    if c == crate::markup::NO_FILL {
                        self.fill(&inner, 0xffffff, 1.0);
                        self.line([inner.l, inner.b], [inner.r, inner.t], 0xe53935, 1.5 * s);
                    } else {
                        self.fill(&inner, c, 1.0);
                    }
                    let tr = RectF::new(r.l + 18.0 * s, r.t, r.r, r.b);
                    self.text(&label, &self.fonts.ui_center, &tr, TEXT);
                } else if b == TBtn::Bold {
                    self.text(&label, &self.fonts.label, &r, TEXT);
                } else if b == TBtn::Italic {
                    self.text_italic(&label, &self.fonts.ui_center, &r, TEXT);
                } else {
                    self.text(&label, &self.fonts.ui_center, &r, if warn && !active && !hover { 0xffb74d } else { TEXT });
                }
            }
        }
        if let Some(panel) = self.qty_panel_rect() {
            self.paint_qty_panel(&panel);
        }
    }

    /// The text prompt, centred over the drawing area (drawn on top of
    /// the side panel and toolbars).
    pub(super) fn paint_prompt(&self) {
        let s = self.s();
        if let Some(p) = &self.prompt {
            let (cw, ch) = self.canvas();
            // Centre in the area right of the side panel.
            let (l0, cw) = (self.left_inset(), cw - self.left_inset());
            let multiline = matches!(p.purpose, PromptFor::MarkupText { .. });
            let field_h = if multiline { 150.0 } else { 34.0 };
            let (w, h) = (if multiline { 600.0 } else { 460.0 } * s, (96.0 + field_h) * s);
            let w = w.min(cw - 16.0 * s);
            let r = RectF::new((l0 + (cw - w) / 2.0).round(), ((ch - h) / 2.0).round(), (l0 + (cw + w) / 2.0).round(), ((ch + h) / 2.0).round());
            self.fill(&r, 0x141518, 0.97);
            self.fill(&RectF::new(r.l, r.t, r.r, r.t + 3.0 * s), ACCENT, 1.0);
            self.text(&p.title, &self.fonts.ui, &RectF::new(r.l + 16.0 * s, r.t + 8.0 * s, r.r - 16.0 * s, r.t + 34.0 * s), TEXT);
            let field = RectF::new(r.l + 16.0 * s, r.t + 42.0 * s, r.r - 16.0 * s, r.t + (42.0 + field_h) * s);
            self.fill(&field, 0xffffff, 1.0);
            if multiline {
                let inner = RectF::new(field.l + 8.0 * s, field.t + 6.0 * s, field.r - 8.0 * s, field.b - 4.0 * s);
                self.text(&format!("{}\u{258F}", p.text), &self.fonts.body, &inner, 0x111111);
            } else {
                let inner = RectF::new(field.l + 8.0 * s, field.t, field.r - 8.0 * s, field.b);
                self.text(&format!("{}\u{258F}", p.text), &self.fonts.ui, &inner, 0x111111);
            }
            let y = field.b + 6.0 * s;
            self.text(&p.hint, &self.fonts.ui, &RectF::new(r.l + 16.0 * s, y, r.r - 16.0 * s, y + 22.0 * s), TEXT_DIM);
            let keys = if multiline {
                "Enter to finish \u{00B7} Shift+Enter for a new line \u{00B7} Ctrl+V to paste \u{00B7} Esc to cancel"
            } else {
                "Enter to confirm \u{00B7} Esc to cancel"
            };
            self.text(keys, &self.fonts.ui, &RectF::new(r.l + 16.0 * s, y + 20.0 * s, r.r - 16.0 * s, r.b - 4.0 * s), TEXT_DIM);
        }
    }

    fn paint_qty_panel(&self, panel: &RectF) {
        let s = self.s();
        let units = self.takeoff.units;
        self.fill(panel, 0x141518, 0.93);
        self.fill(&RectF::new(panel.l, panel.t, panel.l + 3.0 * s, panel.b), ACCENT, 1.0);
        let x0 = panel.l + 16.0 * s;
        let mut y = panel.t + 10.0 * s;
        let line = |y: &mut f32, txt: &str, color: u32, indent: f32| {
            if *y + 20.0 * s < panel.b {
                self.text(txt, &self.fonts.perf, &RectF::new(x0 + indent, *y, panel.r - 10.0 * s, *y + 20.0 * s), color);
            }
            *y += 20.0 * s;
        };
        line(&mut y, "QUANTITIES  (all sheets, visible layers)", TEXT, 0.0);
        if let Some(p) = self.takeoff_sheet() {
            let sc = self.takeoff.scale(p).map(|s| s.label.clone()).unwrap_or_else(|| "not set".into());
            line(&mut y, &format!("This sheet ({}): scale {sc}", p + 1), TEXT_DIM, 0.0);
        }
        let hidden: Vec<&str> = self.takeoff.layers.iter().filter(|l| !l.visible).map(|l| l.name.as_str()).collect();
        if !hidden.is_empty() {
            line(&mut y, &format!("Hidden, not counted: {}", hidden.join(", ")), 0xffb74d, 0.0);
        }
        y += 6.0 * s;
        if self.takeoff.items.is_empty() {
            line(&mut y, "No measurements yet.", TEXT_DIM, 0.0);
            line(&mut y, "1. Scale \u{25BE}: pick the sheet scale or calibrate", TEXT_DIM, 0.0);
            line(&mut y, "2. Group \u{25BE}: name what you're measuring", TEXT_DIM, 0.0);
            line(&mut y, "3. Length (L), Polyline (Y), Area (A), Volume (U), Count (C)", TEXT_DIM, 0.0);
            line(&mut y, "Snap (S) locks onto line ends and lines; Alt = free", TEXT_DIM, 0.0);
            line(&mut y, "Select (V): drag to move, drag handles to reshape,", TEXT_DIM, 0.0);
            line(&mut y, "drag labels for leaders, right-click for properties", TEXT_DIM, 0.0);
            return;
        }
        let mut unscaled = 0usize;
        let (mut grand, mut any_price) = (0.0, false);
        for (g, t) in self.takeoff.group_totals() {
            unscaled += t.unscaled;
            if y + 22.0 * s < panel.b {
                self.fill(&RectF::new(x0, y + 5.0 * s, x0 + 10.0 * s, y + 15.0 * s), g.color, 1.0);
            }
            let sheet_list: Vec<String> = t.sheets.iter().map(|n| (n + 1).to_string()).collect();
            line(&mut y, &format!("{}   ({} items, sheets {})", g.name, t.items, sheet_list.join(", ")), TEXT, 16.0 * s);
            if t.len > 0.0 {
                line(&mut y, &format!("Length\t{}", takeoff::fmt_len(t.len, units)), TEXT, 16.0 * s);
            }
            if t.wall > 0.0 {
                line(&mut y, &format!("Wall area\t{}", takeoff::fmt_area(t.wall, units)), TEXT, 16.0 * s);
            }
            if t.area > 0.0 {
                line(&mut y, &format!("Area\t{}", takeoff::fmt_area(t.area, units)), TEXT, 16.0 * s);
                line(&mut y, &format!("Perimeter\t{}", takeoff::fmt_len(t.perimeter, units)), TEXT_DIM, 16.0 * s);
            }
            if t.vol > 0.0 {
                line(&mut y, &format!("Volume\t{}", takeoff::fmt_volume(t.vol, units)), TEXT, 16.0 * s);
            }
            if t.count > 0 {
                line(&mut y, &format!("Count\t{} ea", t.count), TEXT, 16.0 * s);
            }
            if t.priced {
                line(&mut y, &format!("Cost\t{}", takeoff::fmt_money(t.cost)), 0x81c784, 16.0 * s);
                grand += t.cost;
                any_price = true;
            }
            y += 6.0 * s;
        }
        if any_price {
            line(&mut y, &format!("TOTAL COST\t{}", takeoff::fmt_money(grand)), 0x81c784, 0.0);
            y += 4.0 * s;
        } else {
            line(&mut y, "Add unit prices from Group \u{25BE} to see costs", TEXT_DIM, 0.0);
        }
        if unscaled > 0 {
            line(&mut y, &format!("\u{26A0} {unscaled} measurement(s) on sheets without a scale"), 0xffb74d, 0.0);
        }
        if let Some(p) = &self.takeoff_path {
            y = y.max(panel.b - 26.0 * s);
            line(&mut y, &format!("Saved: {}", p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()), TEXT_DIM, 0.0);
        }
    }
}

/// %LOCALAPPDATA%\Red-Beam
fn local_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Red-Beam"))
}

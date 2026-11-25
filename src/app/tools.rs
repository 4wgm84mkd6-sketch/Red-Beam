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
use crate::takeoff::{self, EndStyle, Kind, Layer, Measure, Qty, Scale, Takeoff, Units, DEFAULT_LAYER, SCALES};
use crate::view::RectF;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    /// Pan with left-drag on empty space; click/drag measurements to edit.
    Select,
    /// Two clicks, then type the real distance.
    Calibrate,
    Measure(Kind),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TBtn {
    Select,
    Scale,
    Tool(Kind),
    Group,
    Layer,
    Units,
    Quantities,
    Export,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuKind {
    Scale,
    Group,
    Layer,
    Units,
    /// Right-click on a measurement (and maybe one of its points).
    Context { id: u64, vertex: Option<usize> },
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
}

/// A drag in progress with the Select tool.
pub enum EditDrag {
    /// Whole measurement.
    Move { orig: Measure, start: [f32; 2], moved: bool },
    /// One point (corner, end or count marker).
    Vertex { orig: Measure, index: usize, moved: bool },
    /// The label, which then gets a leader back to the measurement.
    Label { orig: Measure, start: [f32; 2], moved: bool },
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

fn item(id: usize, label: impl Into<String>, checked: bool) -> MenuItem {
    MenuItem { id, label: label.into(), checked, separator: false, children: Vec::new() }
}
fn sep() -> MenuItem {
    MenuItem { id: 0, label: String::new(), checked: false, separator: true, children: Vec::new() }
}
fn submenu(label: impl Into<String>, children: Vec<MenuItem>) -> MenuItem {
    MenuItem { id: 0, label: label.into(), checked: false, separator: false, children }
}

fn v2(p: [f32; 2]) -> Vector2 {
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
            takeoff::point_in_polygon(p, &m.points)
                || (0..m.points.len()).any(|i| takeoff::seg_dist2(p, m.points[i], m.points[(i + 1) % m.points.len()]) <= tol2)
        }
    }
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
                None => (Takeoff::new(), None),
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
    fn save_takeoff(&mut self) {
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

    fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        self.pending_pts.clear();
        if tool != Tool::Select {
            self.selected = None;
        }
        self.invalidate();
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
        let (top, bot) = (8.0 * s, 40.0 * s);
        let scale = match self.takeoff_sheet().and_then(|p| self.takeoff.scale(p)) {
            Some(sc) => format!("Scale: {} \u{25BE}", sc.label),
            None => "Scale: not set \u{25BE}".into(),
        };
        let units = match self.takeoff.units {
            Units::FeetInches => "ft-in",
            Units::DecimalFeet => "dec ft",
            Units::Metric => "metric",
        };
        let specs: Vec<(TBtn, String, f32, f32)> = vec![
            (TBtn::Select, "Select".into(), 60.0, 8.0),
            (TBtn::Scale, scale, 170.0, 8.0),
            (TBtn::Tool(Kind::Length), "Length".into(), 64.0, 4.0),
            (TBtn::Tool(Kind::Polyline), "Polyline".into(), 72.0, 4.0),
            (TBtn::Tool(Kind::Area), "Area".into(), 52.0, 4.0),
            (TBtn::Tool(Kind::Count), "Count".into(), 58.0, 8.0),
            (TBtn::Group, format!("Group: {} \u{25BE}", self.group), 150.0, 4.0),
            (TBtn::Layer, format!("Layer: {} \u{25BE}", self.layer), 140.0, 4.0),
            (TBtn::Units, format!("{units} \u{25BE}"), 72.0, 8.0),
            (TBtn::Quantities, "Quantities".into(), 90.0, 4.0),
            (TBtn::Export, "Export CSV".into(), 92.0, 0.0),
        ];
        let mut x = 8.0 * s;
        specs
            .into_iter()
            .map(|(b, label, w, gap)| {
                let r = RectF::new(x, top, x + w * s, bot);
                x += (w + 2.0 + gap) * s;
                (b, r, label)
            })
            .collect()
    }

    fn toolbar_rect(&self) -> Option<RectF> {
        let l = self.toolbar_layout();
        let (first, last) = (l.first()?, l.last()?);
        let s = self.s();
        Some(RectF::new(first.1.l - 4.0 * s, first.1.t - 4.0 * s, last.1.r + 4.0 * s, last.1.b + 4.0 * s))
    }

    fn qty_panel_rect(&self) -> Option<RectF> {
        if !self.show_qty || self.doc.is_none() {
            return None;
        }
        let s = self.s();
        let (cw, ch) = self.canvas();
        Some(RectF::new(cw - 360.0 * s, 52.0 * s, cw - 8.0 * s, ch - 8.0 * s))
    }

    /// Is (x, y) over takeoff UI (so canvas tools should ignore it)?
    pub(super) fn over_tool_ui(&self, x: f32, y: f32) -> bool {
        self.prompt.is_some()
            || self.toolbar_rect().is_some_and(|r| r.contains_pt(x, y))
            || self.qty_panel_rect().is_some_and(|r| r.contains_pt(x, y))
    }

    pub(super) fn toolbar_hit(&self, x: f32, y: f32) -> Option<TBtn> {
        self.toolbar_layout().into_iter().find(|(_, r, _)| r.contains_pt(x, y)).map(|(b, _, _)| b)
    }

    pub(super) fn press_tool(&mut self, b: TBtn) {
        match b {
            TBtn::Select => self.set_tool(Tool::Select),
            TBtn::Tool(k) => self.set_tool(Tool::Measure(k)),
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
                    self.pending_export = true;
                }
            }
        }
    }

    // ------------------------------------------------------------- menus

    fn style_items(base: usize, current: EndStyle) -> Vec<MenuItem> {
        EndStyle::ALL.iter().enumerate().map(|(i, st)| item(base + i, st.name(), *st == current)).collect()
    }

    pub fn menu_items(&self, kind: MenuKind) -> Vec<MenuItem> {
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
                v
            }
            MenuKind::Group => {
                let mut v: Vec<MenuItem> = self
                    .takeoff
                    .groups
                    .iter()
                    .enumerate()
                    .map(|(i, g)| item(CMD_SET_GROUP + i, g.name.clone(), g.name == self.group))
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
            MenuKind::Units => vec![
                item(CMD_BASE, "Feet-inches (12'-6 1/4\") and SF", self.takeoff.units == Units::FeetInches),
                item(CMD_BASE + 1, "Decimal feet (12.52') and SF", self.takeoff.units == Units::DecimalFeet),
                item(CMD_BASE + 2, "Metric (m and m\u{00B2})", self.takeoff.units == Units::Metric),
            ],
            MenuKind::Context { id, vertex } => {
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
                if m.label_offset.is_some() {
                    v.push(item(CMD_RESET_LABEL, "Put label back on the measurement", false));
                }
                if vertex.is_some() && m.points.len() > m.kind.min_points() {
                    let what = if m.kind == Kind::Count { "Remove this marker" } else { "Delete this point" };
                    v.push(item(CMD_DELETE_POINT, what, false));
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
            MenuKind::Group => {
                if cmd == CMD_NEW_GROUP {
                    self.ask_new_group(None);
                } else if let Some(g) = self.takeoff.groups.get(cmd.wrapping_sub(CMD_SET_GROUP)) {
                    self.group = g.name.clone();
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
            MenuKind::Context { id, vertex } => self.context_menu(id, vertex, cmd),
        }
        self.invalidate();
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

    fn context_menu(&mut self, id: u64, vertex: Option<usize>, cmd: usize) {
        let Some(before) = self.takeoff.items.iter().find(|m| m.id == id).cloned() else { return };
        match cmd {
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
            if let Some(v) = vertex.filter(|v| *v < m.points.len() && m.points.len() > m.kind.min_points()) {
                m.points.remove(v);
            }
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

    pub fn export_csv(&mut self, path: PathBuf) {
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

    pub fn export_default_name(&self) -> String {
        let stem = self
            .doc
            .as_ref()
            .and_then(|d| d.path.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "takeoff".into());
        format!("{stem} takeoff.csv")
    }

    // ------------------------------------------------------------- input

    fn page_pt(&self, x: f32, y: f32) -> [f32; 2] {
        let r = self.view.screen_to_page(&RectF::new(x, y, x, y));
        [r.l, r.t]
    }

    fn to_screen(&self, p: [f32; 2]) -> [f32; 2] {
        let r = self.view.page_to_screen(&RectF::new(p[0], p[1], p[0], p[1]));
        [r.l, r.t]
    }

    fn shift_down() -> bool {
        unsafe { GetKeyState(VK_SHIFT.0 as i32) < 0 }
    }

    /// The point a click at (x, y) would place, with Shift = ortho.
    fn place_pt(&self, x: f32, y: f32) -> [f32; 2] {
        let p = self.page_pt(x, y);
        match (Self::shift_down(), self.pending_pts.last()) {
            (true, Some(&last)) => takeoff::ortho(last, p),
            _ => p,
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
            Tool::Measure(kind) => {
                let p = self.place_pt(x, y);
                self.pending_pts.push(p);
                if kind == Kind::Length && self.pending_pts.len() == 2 {
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
        if matches!(self.tool, Tool::Measure(_)) && !self.pending_pts.is_empty() {
            self.finish_measure();
            return true;
        }
        if self.tool != Tool::Select {
            return false;
        }
        let p = self.page_pt(x, y);
        let tol = 6.0 * self.s() / self.view.zoom;
        let Some(m) = self.selected_measure().filter(|m| matches!(m.kind, Kind::Polyline | Kind::Area | Kind::Length)) else {
            return false;
        };
        let n = m.points.len();
        let segs = if m.kind == Kind::Area { n } else { n - 1 };
        let Some(i) = (0..segs).find(|&i| takeoff::seg_dist2(p, m.points[i], m.points[(i + 1) % n]) <= tol * tol) else {
            return false;
        };
        let before = m.clone();
        let id = m.id;
        if let Some(m) = self.takeoff.get_mut(id) {
            if m.kind == Kind::Length {
                // A length with a bend becomes a polyline.
                m.kind = Kind::Polyline;
            }
            m.points.insert(i + 1, p);
        }
        self.undo.push(Undo::Modified(before));
        self.save_takeoff();
        self.invalidate();
        true
    }

    pub(super) fn finish_measure(&mut self) {
        let (Tool::Measure(kind), Some(sheet)) = (self.tool, self.takeoff_sheet()) else { return };
        let mut pts = std::mem::take(&mut self.pending_pts);
        // A double-click can leave a duplicate final point.
        pts.dedup_by(|a, b| takeoff::dist(*a, *b) < 0.01);
        if pts.len() < kind.min_points() {
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
        if self.takeoff.scale(sheet).is_none() && kind != Kind::Count {
            self.status = Some("This sheet has no scale \u{2014} set one from the Scale menu".into());
        }
        self.save_takeoff();
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
        match m.kind {
            Kind::Count => format!("{} \u{00D7}{}", m.group, m.points.len()),
            _ => takeoff::fmt_qty(m.qty(self.takeoff.scale(m.sheet)), self.takeoff.units),
        }
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

    /// Point handle of the selected measurement under (x, y).
    fn handle_at(&self, x: f32, y: f32) -> Option<usize> {
        let m = self.selected_measure().filter(|m| self.editable(m))?;
        let r = 7.0 * self.s();
        m.points.iter().position(|p| {
            let q = self.to_screen(*p);
            (q[0] - x).abs() <= r && (q[1] - y).abs() <= r
        })
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
        if let (Some(index), Some(m)) = (self.handle_at(x, y), self.selected_measure()) {
            self.edit = Some(EditDrag::Vertex { orig: m.clone(), index, moved: false });
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
        let Some(edit) = self.edit.as_mut() else { return false };
        // Work out the edited measurement, or None if still within the
        // click slop (so a plain click doesn't nudge anything).
        let next: Option<Measure> = match edit {
            EditDrag::Move { orig, start, moved } => {
                let (dx, dy) = (p[0] - start[0], p[1] - start[1]);
                (*moved || dx.hypot(dy) >= slop).then(|| {
                    *moved = true;
                    let mut m = orig.clone();
                    for q in &mut m.points {
                        q[0] += dx;
                        q[1] += dy;
                    }
                    m
                })
            }
            EditDrag::Vertex { orig, index, moved } => {
                let i = *index;
                // Shift keeps the point square to its neighbour.
                let target = match (shift, i.checked_sub(1).and_then(|j| orig.points.get(j))) {
                    (true, Some(prev)) if orig.kind != Kind::Count => takeoff::ortho(*prev, p),
                    _ => p,
                };
                (*moved || takeoff::dist(target, orig.points[i]) >= slop as f64).then(|| {
                    *moved = true;
                    let mut m = orig.clone();
                    m.points[i] = target;
                    m
                })
            }
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
        let vertex = self.handle_at(x, y);
        let id = match (vertex, self.selected) {
            (Some(_), Some(id)) => id,
            _ => match self.measure_at(x, y) {
                Some((id, _)) => id,
                None => return false,
            },
        };
        self.selected = Some(id);
        self.pending_menu = Some(MenuKind::Context { id, vertex });
        self.invalidate();
        true
    }

    /// Cursor for the Select tool over editable things.
    pub(super) fn edit_cursor(&self, x: f32, y: f32) -> Option<PCWSTR> {
        if self.tool != Tool::Select || self.takeoff_sheet().is_none() {
            return None;
        }
        if self.edit.is_some() || self.handle_at(x, y).is_some() {
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
        self.selected = self.measure_at(x, y).map(|(id, _)| id);
        if let Some(m) = self.selected_measure() {
            self.group = m.group.clone();
        }
        self.invalidate();
    }

    fn delete_selected(&mut self) {
        let Some(id) = self.selected.take() else { return };
        if let Some(ix) = self.takeoff.items.iter().position(|m| m.id == id) {
            let m = self.takeoff.items.remove(ix);
            self.undo.push(Undo::Removed(m));
            self.save_takeoff();
        }
        self.invalidate();
    }

    fn duplicate_selected(&mut self) {
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
            Undo::Layers(items, layers) => {
                self.takeoff.items = items;
                self.takeoff.layers = layers;
                if !self.takeoff.layers.iter().any(|l| l.name == self.layer) {
                    self.layer = self.takeoff.layers.first().map(|l| l.name.clone()).unwrap_or_else(|| DEFAULT_LAYER.into());
                }
            }
        }
        self.selected = None;
        self.save_takeoff();
        self.invalidate();
    }

    /// Keyboard for takeoffs. Returns true if handled.
    pub(super) fn tools_key(&mut self, vk: VIRTUAL_KEY, ctrl: bool) -> bool {
        if self.prompt.is_some() {
            match vk {
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
        let letter = |c: u8| vk.0 == c as u16 && !ctrl;
        let ctrl_key = |c: u8| vk.0 == c as u16 && ctrl;
        match vk {
            _ if ctrl_key(b'Z') => self.undo_last(),
            _ if ctrl_key(b'C') && self.selected.is_some() => self.clip = self.selected_measure().cloned(),
            _ if ctrl_key(b'V') && self.clip.is_some() => self.paste(),
            _ if ctrl_key(b'D') && self.selected.is_some() => self.duplicate_selected(),
            VK_DELETE => self.delete_selected(),
            VK_ESCAPE if !self.pending_pts.is_empty() => {
                self.pending_pts.clear();
                self.invalidate();
            }
            VK_ESCAPE if self.tool != Tool::Select => self.set_tool(Tool::Select),
            VK_ESCAPE if self.selected.is_some() => {
                self.selected = None;
                self.invalidate();
            }
            VK_RETURN if !self.pending_pts.is_empty() && self.goto_buf.is_empty() => self.finish_measure(),
            _ if letter(b'L') => self.set_tool(Tool::Measure(Kind::Length)),
            _ if letter(b'Y') => self.set_tool(Tool::Measure(Kind::Polyline)),
            _ if letter(b'A') => self.set_tool(Tool::Measure(Kind::Area)),
            _ if letter(b'C') => self.set_tool(Tool::Measure(Kind::Count)),
            _ if letter(b'V') => self.set_tool(Tool::Select),
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
        if !c.is_control() && p.text.chars().count() < 40 {
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
                    let old = self.takeoff.scales.insert(sheet, sc);
                    self.undo.push(Undo::Scale(sheet, old));
                    self.status = None;
                    self.save_takeoff();
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
        if screen.len() < 2 {
            return None;
        }
        unsafe {
            let factory = self.gfx.ctx.GetFactory().ok()?;
            let geom = factory.CreatePathGeometry().ok()?;
            let sink = geom.Open().ok()?;
            let pts: Vec<Vector2> = screen.iter().map(|p| v2(*p)).collect();
            sink.BeginFigure(pts[0], if closed { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW });
            sink.AddLines(&pts[1..]);
            sink.EndFigure(if closed { D2D1_FIGURE_END_CLOSED } else { D2D1_FIGURE_END_OPEN });
            sink.Close().ok()?;
            Some(geom)
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

    fn line(&self, a: [f32; 2], b: [f32; 2], color: u32, width: f32) {
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

    /// Measurements on the current sheet, plus the one being drawn.
    pub(super) fn paint_takeoff(&self) {
        let Some(sheet) = self.takeoff_sheet() else { return };
        let s = self.s();
        let units = self.takeoff.units;
        let scale = self.takeoff.scale(sheet);
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
                Kind::Area => self.polygon(&m.points, true, color, if sel { 0.30 } else { 0.18 }, w),
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
            // Editing handles on the selected measurement.
            if sel && self.editable(m) {
                let h = 4.5 * s;
                for c in &screen {
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
        let (kind, color) = match self.tool {
            Tool::Measure(k) => (k, self.takeoff.group_color(&self.group)),
            _ => (Kind::Length, ACCENT),
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
        };
        let mut readout = takeoff::fmt_qty(tmp.qty(scale), units);
        if let Qty::Area { perimeter, .. } = tmp.qty(scale) {
            readout = format!("{readout}  \u{00B7}  perim {}", takeoff::fmt_len(perimeter, units));
        }
        if kind != Kind::Length {
            readout.push_str("  \u{2014} Enter / double-click to finish");
        }
        self.readout(&readout, at, color);
    }

    /// Tool palette, quantities panel and prompt (screen-space UI).
    pub(super) fn paint_tools_ui(&self) {
        let s = self.s();
        if let Some(r) = self.toolbar_rect() {
            self.fill(&r, 0x1e2023, 0.92);
            for (b, r, label) in self.toolbar_layout() {
                let active = match b {
                    TBtn::Select => self.tool == Tool::Select,
                    TBtn::Tool(k) => self.tool == Tool::Measure(k),
                    TBtn::Scale => self.tool == Tool::Calibrate,
                    TBtn::Quantities => self.show_qty,
                    _ => false,
                };
                let hover = self.tool_hover == Some(b);
                let warn = b == TBtn::Scale && self.takeoff_sheet().is_some_and(|p| self.takeoff.scale(p).is_none());
                let bg = if active || hover { ACCENT } else { BTN_BG };
                self.fill(&r, bg, 1.0);
                if let TBtn::Group = b {
                    let sw = RectF::new(r.l + 6.0 * s, r.t + 10.0 * s, r.l + 16.0 * s, r.b - 10.0 * s);
                    self.fill(&sw, self.takeoff.group_color(&self.group), 1.0);
                    let tr = RectF::new(r.l + 14.0 * s, r.t, r.r, r.b);
                    self.text(&label, &self.fonts.ui_center, &tr, TEXT);
                } else {
                    self.text(&label, &self.fonts.ui_center, &r, if warn && !active && !hover { 0xffb74d } else { TEXT });
                }
            }
        }
        if let Some(panel) = self.qty_panel_rect() {
            self.paint_qty_panel(&panel);
        }
        if let Some(p) = &self.prompt {
            let (cw, ch) = self.canvas();
            let (w, h) = (460.0 * s, 130.0 * s);
            let r = RectF::new(((cw - w) / 2.0).round(), ((ch - h) / 2.0).round(), ((cw + w) / 2.0).round(), ((ch + h) / 2.0).round());
            self.fill(&r, 0x141518, 0.96);
            self.fill(&RectF::new(r.l, r.t, r.r, r.t + 3.0 * s), ACCENT, 1.0);
            self.text(&p.title, &self.fonts.ui, &RectF::new(r.l + 16.0 * s, r.t + 8.0 * s, r.r - 16.0 * s, r.t + 34.0 * s), TEXT);
            let field = RectF::new(r.l + 16.0 * s, r.t + 42.0 * s, r.r - 16.0 * s, r.t + 76.0 * s);
            self.fill(&field, 0xffffff, 1.0);
            let inner = RectF::new(field.l + 8.0 * s, field.t, field.r - 8.0 * s, field.b);
            self.text(&format!("{}\u{258F}", p.text), &self.fonts.ui, &inner, 0x111111);
            self.text(&p.hint, &self.fonts.ui, &RectF::new(r.l + 16.0 * s, r.t + 82.0 * s, r.r - 16.0 * s, r.t + 104.0 * s), TEXT_DIM);
            self.text("Enter to confirm \u{00B7} Esc to cancel", &self.fonts.ui, &RectF::new(r.l + 16.0 * s, r.t + 102.0 * s, r.r - 16.0 * s, r.b - 4.0 * s), TEXT_DIM);
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
            line(&mut y, "3. Length (L), Polyline (Y), Area (A), Count (C)", TEXT_DIM, 0.0);
            line(&mut y, "Select (V): drag to move, drag handles to reshape,", TEXT_DIM, 0.0);
            line(&mut y, "drag labels for leaders, right-click for properties", TEXT_DIM, 0.0);
            return;
        }
        let mut unscaled = 0usize;
        for g in &self.takeoff.groups {
            let items: Vec<&Measure> = self.takeoff.visible_items().filter(|m| m.group == g.name).collect();
            if items.is_empty() {
                continue;
            }
            let (mut len, mut area, mut per, mut count, mut sheets) = (0.0, 0.0, 0.0, 0usize, std::collections::BTreeSet::new());
            for m in &items {
                sheets.insert(m.sheet + 1);
                match m.qty(self.takeoff.scale(m.sheet)) {
                    Qty::Length(l) => len += l,
                    Qty::Area { area: a, perimeter } => {
                        area += a;
                        per += perimeter;
                    }
                    Qty::Count(n) => count += n,
                    Qty::Unscaled => unscaled += 1,
                }
            }
            if y + 22.0 * s < panel.b {
                self.fill(&RectF::new(x0, y + 5.0 * s, x0 + 10.0 * s, y + 15.0 * s), g.color, 1.0);
            }
            let sheet_list: Vec<String> = sheets.iter().map(|n| n.to_string()).collect();
            line(&mut y, &format!("{}   ({} items, sheets {})", g.name, items.len(), sheet_list.join(", ")), TEXT, 16.0 * s);
            if len > 0.0 {
                line(&mut y, &format!("Length\t{}", takeoff::fmt_len(len, units)), TEXT, 16.0 * s);
            }
            if area > 0.0 {
                line(&mut y, &format!("Area\t{}", takeoff::fmt_area(area, units)), TEXT, 16.0 * s);
                line(&mut y, &format!("Perimeter\t{}", takeoff::fmt_len(per, units)), TEXT_DIM, 16.0 * s);
            }
            if count > 0 {
                line(&mut y, &format!("Count\t{count} ea"), TEXT, 16.0 * s);
            }
            y += 6.0 * s;
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

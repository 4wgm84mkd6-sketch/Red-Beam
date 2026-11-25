//! Takeoff UI: tool palette, click-to-measure tools, live readouts,
//! selection/undo, the quantities panel, menus and the typed-entry prompt.
//! Lives in a child module of `app` so it can use the App's private state.

use std::path::PathBuf;

use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows_numerics::Vector2;

use super::{d2r, rgb, App, CmpMode, ACCENT, BTN_BG, TEXT, TEXT_DIM};
use crate::takeoff::{self, Kind, Qty, Scale, Takeoff, Units, SCALES};
use crate::view::RectF;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    /// Pan with left-drag, click to select.
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
    Units,
    Quantities,
    Export,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuKind {
    Scale,
    Group,
    Units,
}

#[derive(Clone, Debug)]
pub enum PromptFor {
    /// Two calibration points `dip_len` apart on `sheet`.
    Calibrate { sheet: u32, dip_len: f64 },
    NewGroup,
}

pub struct Prompt {
    pub title: String,
    pub hint: String,
    pub text: String,
    pub purpose: PromptFor,
}

pub enum Undo {
    Added(u64),
    Removed(takeoff::Measure),
    Scale(u32, Option<Scale>),
    AllScales(std::collections::BTreeMap<u32, Scale>),
}

/// Menu command ids.
const CMD_BASE: usize = 100;
const CMD_CALIBRATE: usize = 900;
const CMD_APPLY_ALL: usize = 901;
const CMD_CLEAR: usize = 902;
const CMD_NEW_GROUP: usize = 903;

pub struct MenuItem {
    pub id: usize,
    pub label: String,
    pub checked: bool,
    pub separator: bool,
}

fn v2(p: [f32; 2]) -> Vector2 {
    Vector2 { X: p[0], Y: p[1] }
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
                None => (Takeoff { version: 1, ..Default::default() }, None),
            },
        };
        self.takeoff = t;
        self.takeoff_path = path;
        self.undo.clear();
        self.selected = None;
        self.pending_pts.clear();
        self.prompt = None;
        if self.group.is_empty() || !self.takeoff.groups.iter().any(|g| g.name == self.group) {
            self.group = self.takeoff.groups.first().map(|g| g.name.clone()).unwrap_or_else(|| "General".into());
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
            (TBtn::Group, format!("Group: {} \u{25BE}", self.group), 160.0, 4.0),
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

    pub fn menu_items(&self, kind: MenuKind) -> Vec<MenuItem> {
        let item = |id: usize, label: String, checked: bool| MenuItem { id, label, checked, separator: false };
        let sep = || MenuItem { id: 0, label: String::new(), checked: false, separator: true };
        match kind {
            MenuKind::Scale => {
                let cur = self.takeoff_sheet().and_then(|p| self.takeoff.scale(p)).map(|s| s.label.clone());
                let mut v: Vec<MenuItem> = SCALES
                    .iter()
                    .enumerate()
                    .map(|(i, (label, _))| item(CMD_BASE + i, label.to_string(), cur.as_deref() == Some(*label)))
                    .collect();
                v.push(sep());
                v.push(item(CMD_CALIBRATE, "Calibrate by two points\u{2026}".into(), cur.as_deref() == Some("Calibrated")));
                v.push(sep());
                v.push(item(CMD_APPLY_ALL, "Apply this sheet's scale to all sheets".into(), false));
                v.push(item(CMD_CLEAR, "Clear this sheet's scale".into(), false));
                v
            }
            MenuKind::Group => {
                let mut v: Vec<MenuItem> = self
                    .takeoff
                    .groups
                    .iter()
                    .enumerate()
                    .map(|(i, g)| item(CMD_BASE + i, g.name.clone(), g.name == self.group))
                    .collect();
                if !v.is_empty() {
                    v.push(sep());
                }
                v.push(item(CMD_NEW_GROUP, "New group\u{2026}".into(), false));
                v
            }
            MenuKind::Units => vec![
                item(CMD_BASE, "Feet-inches (12'-6 1/4\") and SF".into(), self.takeoff.units == Units::FeetInches),
                item(CMD_BASE + 1, "Decimal feet (12.52') and SF".into(), self.takeoff.units == Units::DecimalFeet),
                item(CMD_BASE + 2, "Metric (m and m\u{00B2})".into(), self.takeoff.units == Units::Metric),
            ],
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
                } else if let Some((label, ratio)) = SCALES.get(cmd - CMD_BASE) {
                    let old = self.takeoff.scales.insert(sheet, Scale::from_ratio(label, *ratio));
                    self.undo.push(Undo::Scale(sheet, old));
                    self.save_takeoff();
                }
            }
            MenuKind::Group => {
                if cmd == CMD_NEW_GROUP {
                    self.prompt = Some(Prompt {
                        title: "New group".into(),
                        hint: "Name, e.g. Drywall, Flooring, Concrete".into(),
                        text: String::new(),
                        purpose: PromptFor::NewGroup,
                    });
                } else if let Some(g) = self.takeoff.groups.get(cmd - CMD_BASE) {
                    self.group = g.name.clone();
                    // Re-assign the selected measurement too, like Bluebeam's subject.
                    if let Some(id) = self.selected {
                        let name = self.group.clone();
                        if let Some(m) = self.takeoff.items.iter_mut().find(|m| m.id == id) {
                            m.group = name;
                        }
                        self.save_takeoff();
                    }
                }
            }
            MenuKind::Units => {
                self.takeoff.units = match cmd - CMD_BASE {
                    1 => Units::DecimalFeet,
                    2 => Units::Metric,
                    _ => Units::FeetInches,
                };
                self.save_takeoff();
            }
        }
        self.invalidate();
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

    /// The point a click at (x, y) would place, with Shift = ortho.
    fn place_pt(&self, x: f32, y: f32) -> [f32; 2] {
        let p = self.page_pt(x, y);
        let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        match (shift, self.pending_pts.last()) {
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

    /// Double-click: the first click already placed the point; finish.
    pub(super) fn tools_dblclick(&mut self) -> bool {
        if matches!(self.tool, Tool::Measure(_)) && !self.pending_pts.is_empty() {
            self.finish_measure();
            return true;
        }
        false
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
        let group = self.group.clone();
        let id = self.takeoff.add(sheet, kind, pts, &group);
        self.undo.push(Undo::Added(id));
        if self.takeoff.scale(sheet).is_none() && kind != Kind::Count {
            self.status = Some("This sheet has no scale \u{2014} set one from the Scale menu".into());
        }
        self.save_takeoff();
        self.invalidate();
    }

    /// Select the measurement under (x, y), if any.
    pub(super) fn select_at(&mut self, x: f32, y: f32) {
        let Some(sheet) = self.takeoff_sheet() else { return };
        let p = self.page_pt(x, y);
        let tol = 6.0 * self.s() / self.view.zoom;
        let tol2 = tol * tol;
        let hit = self.takeoff.items.iter().rev().filter(|m| m.sheet == sheet).find(|m| match m.kind {
            Kind::Count => m.points.iter().any(|q| takeoff::dist(*q, p) as f32 <= tol * 1.6),
            Kind::Length | Kind::Polyline => m.points.windows(2).any(|w| takeoff::seg_dist2(p, w[0], w[1]) <= tol2),
            Kind::Area => {
                takeoff::point_in_polygon(p, &m.points)
                    || (0..m.points.len())
                        .any(|i| takeoff::seg_dist2(p, m.points[i], m.points[(i + 1) % m.points.len()]) <= tol2)
            }
        });
        self.selected = hit.map(|m| m.id);
        if let Some(m) = hit {
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
            Undo::Scale(sheet, Some(s)) => {
                self.takeoff.scales.insert(sheet, s);
            }
            Undo::Scale(sheet, None) => {
                self.takeoff.scales.remove(&sheet);
            }
            Undo::AllScales(all) => self.takeoff.scales = all,
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
        match vk {
            _ if ctrl && vk.0 == b'Z' as u16 => self.undo_last(),
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
            PromptFor::NewGroup => {
                let name = self.takeoff.ensure_group(&p.text);
                self.group = name;
                self.save_takeoff();
            }
        }
        self.invalidate();
    }

    // ---------------------------------------------------------- painting

    fn label(&self, text: &str, center: [f32; 2], color: u32, strong: bool) {
        let s = self.s();
        let w: Vec<u16> = text.encode_utf16().collect();
        let width = unsafe {
            self.dwrite
                .CreateTextLayout(&w, &self.fonts.label, 2000.0, 100.0)
                .ok()
                .and_then(|l| {
                    let mut m = DWRITE_TEXT_METRICS::default();
                    l.GetMetrics(&mut m).ok().map(|_| m.width)
                })
                .unwrap_or(text.len() as f32 * 7.0 * s)
        };
        let (pw, ph) = (width / 2.0 + 6.0 * s, 10.0 * s);
        let r = RectF::new(center[0] - pw, center[1] - ph, center[0] + pw, center[1] + ph);
        self.fill(&r, if strong { color } else { 0xffffff }, if strong { 1.0 } else { 0.92 });
        unsafe {
            self.brush.SetColor(&rgb(color, 1.0));
            self.gfx.ctx.DrawRectangle(&d2r(&r), &self.brush, 1.0, None);
        }
        self.text(text, &self.fonts.label, &r, if strong { 0xffffff } else { color });
    }

    fn polygon(&self, pts: &[[f32; 2]], closed: bool, color: u32, fill_alpha: f32, width: f32) {
        if pts.len() < 2 {
            return;
        }
        let ctx = &self.gfx.ctx;
        unsafe {
            let Ok(factory) = ctx.GetFactory() else { return };
            let Ok(geom) = factory.CreatePathGeometry() else { return };
            let Ok(sink) = geom.Open() else { return };
            let screen: Vec<Vector2> = pts.iter().map(|p| v2(self.to_screen(*p))).collect();
            sink.BeginFigure(screen[0], if closed { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW });
            sink.AddLines(&screen[1..]);
            sink.EndFigure(if closed { D2D1_FIGURE_END_CLOSED } else { D2D1_FIGURE_END_OPEN });
            if sink.Close().is_err() {
                return;
            }
            if closed && fill_alpha > 0.0 {
                self.brush.SetColor(&rgb(color, fill_alpha));
                ctx.FillGeometry(&geom, &self.brush, None);
            }
            self.brush.SetColor(&rgb(color, 1.0));
            ctx.DrawGeometry(&geom, &self.brush, width, None);
        }
    }

    fn to_screen(&self, p: [f32; 2]) -> [f32; 2] {
        let r = self.view.page_to_screen(&RectF::new(p[0], p[1], p[0], p[1]));
        [r.l, r.t]
    }

    fn dot(&self, p: [f32; 2], r: f32, color: u32) {
        let c = self.to_screen(p);
        let e = D2D1_ELLIPSE { point: v2(c), radiusX: r, radiusY: r };
        unsafe {
            self.brush.SetColor(&rgb(color, 1.0));
            self.gfx.ctx.FillEllipse(&e, &self.brush);
            self.brush.SetColor(&rgb(0xffffff, 1.0));
            self.gfx.ctx.DrawEllipse(&e, &self.brush, 1.5, None);
        }
    }

    /// Measurements on the current sheet, plus the one being drawn.
    pub(super) fn paint_takeoff(&self) {
        let Some(sheet) = self.takeoff_sheet() else { return };
        let s = self.s();
        let units = self.takeoff.units;
        let scale = self.takeoff.scale(sheet);
        for m in self.takeoff.items.iter().filter(|m| m.sheet == sheet) {
            let color = self.takeoff.group_color(&m.group);
            let sel = self.selected == Some(m.id);
            let w = if sel { 4.0 * s } else { 2.0 * s };
            match m.kind {
                Kind::Count => {
                    for (i, p) in m.points.iter().enumerate() {
                        self.dot(*p, if sel { 8.0 * s } else { 6.0 * s }, color);
                        if i == 0 {
                            let sp = self.to_screen(*p);
                            self.label(&format!("{} \u{00D7}{}", m.group, m.points.len()), [sp[0], sp[1] - 20.0 * s], color, sel);
                        }
                    }
                    continue;
                }
                Kind::Area => self.polygon(&m.points, true, color, if sel { 0.30 } else { 0.18 }, w),
                Kind::Length | Kind::Polyline => {
                    self.polygon(&m.points, false, color, 0.0, w);
                    for p in [m.points[0], m.points[m.points.len() - 1]] {
                        self.dot(p, 3.0 * s, color);
                    }
                }
            }
            let text = takeoff::fmt_qty(m.qty(scale), units);
            self.label(&text, self.to_screen(m.label_pos()), color, sel);
        }

        // In-progress measurement with a rubber band to the cursor.
        if self.pending_pts.is_empty() || self.prompt.is_some() && !matches!(self.tool, Tool::Calibrate) {
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
        match kind {
            Kind::Count => {
                for p in &self.pending_pts {
                    self.dot(*p, 6.0 * s, color);
                }
                let readout = format!("{} ea \u{2014} Enter to finish", self.pending_pts.len());
                self.label(&readout, [mx, my - 22.0 * s], color, true);
                return;
            }
            Kind::Area => self.polygon(&pts, pts.len() >= 3, color, 0.15, 2.0 * s),
            _ => self.polygon(&pts, false, color, 0.0, 2.0 * s),
        }
        for p in &pts[..pts.len() - 1] {
            self.dot(*p, 3.5 * s, color);
        }
        if calibrating {
            let d = takeoff::path_len(&pts);
            let txt = match scale {
                Some(sc) => format!("currently {}", takeoff::fmt_len(d * sc.real_in_per_dip, units)),
                None => "click the second point".into(),
            };
            self.label(&txt, [mx, my - 22.0 * s], color, true);
            return;
        }
        let tmp = takeoff::Measure { id: 0, sheet, kind, points: pts.clone(), group: String::new() };
        let mut readout = takeoff::fmt_qty(tmp.qty(scale), units);
        if kind == Kind::Area {
            if let Qty::Area { perimeter, .. } = tmp.qty(scale) {
                readout = format!("{readout}  \u{00B7}  perim {}", takeoff::fmt_len(perimeter, units));
            }
        }
        if kind != Kind::Length {
            readout.push_str("  \u{2014} Enter / double-click to finish");
        }
        self.label(&readout, [mx, my - 22.0 * s], color, true);
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
        line(&mut y, "QUANTITIES  (all sheets)", TEXT, 0.0);
        if let Some(p) = self.takeoff_sheet() {
            let sc = self.takeoff.scale(p).map(|s| s.label.clone()).unwrap_or_else(|| "not set".into());
            line(&mut y, &format!("This sheet ({}): scale {sc}", p + 1), TEXT_DIM, 0.0);
        }
        y += 6.0 * s;
        if self.takeoff.items.is_empty() {
            line(&mut y, "No measurements yet.", TEXT_DIM, 0.0);
            line(&mut y, "1. Scale \u{25BE}: pick the sheet scale or calibrate", TEXT_DIM, 0.0);
            line(&mut y, "2. Group \u{25BE}: name what you're measuring", TEXT_DIM, 0.0);
            line(&mut y, "3. Length (L), Polyline (Y), Area (A), Count (C)", TEXT_DIM, 0.0);
            line(&mut y, "Shift = straight lines \u{00B7} Ctrl+Z undo \u{00B7} Del", TEXT_DIM, 0.0);
            return;
        }
        let mut unscaled = 0usize;
        for g in &self.takeoff.groups {
            let items: Vec<_> = self.takeoff.items.iter().filter(|m| m.group == g.name).collect();
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

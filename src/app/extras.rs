//! Detail viewport scales, tool presets (Tool Chest), and sheet tools
//! (combine, folder as set, extract, delete, rotate — always to a new PDF).

use std::path::PathBuf;

use super::tools::{item, sep, submenu, MenuItem, MenuKind, Prompt, PromptFor, Tool, Undo};
use super::{rgb, App};
use crate::markup::MKind;
use crate::settings::{Preset, PresetTool};
use crate::sheets;
use crate::takeoff::{Kind, Scale, Viewport, SCALES};
use crate::view::RectF;

const CMD_VP_SCALE: usize = 30_000;
const CMD_VP_CALIBRATE: usize = 30_900;
const CMD_PRESET: usize = 31_000;
const CMD_PRESET_DEL: usize = 32_000;
const CMD_PRESET_SAVE: usize = 32_900;
const CMD_SHEETS_COMBINE: usize = 33_000;
const CMD_SHEETS_FOLDER: usize = 33_001;
const CMD_SHEETS_EXTRACT: usize = 33_002;
const CMD_SHEETS_DELETE: usize = 33_003;
const CMD_SHEETS_CW: usize = 33_004;
const CMD_SHEETS_CCW: usize = 33_005;

/// A sheet tool that needs a file picker first (handled in main.rs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SheetPick {
    Combine,
    Folder,
}

/// A new PDF to build, waiting for a save location.
pub struct SheetJob {
    pub parts: Vec<(PathBuf, Vec<u32>)>,
    pub rotate: Vec<(u32, i32)>,
    pub default_name: String,
    pub default_dir: Option<PathBuf>,
    pub title: String,
}

impl App {
    // ------------------------------------------------------------ viewports

    /// A viewport box was drawn: pick its scale next.
    pub(super) fn viewport_drawn(&mut self, rect: [f32; 4]) {
        if rect[2] - rect[0] < 4.0 || rect[3] - rect[1] < 4.0 {
            self.status = Some("Drag a box around the detail".into());
            return;
        }
        self.pending_vp = Some(rect);
        self.pending_menu = Some(MenuKind::ViewportScale);
        self.set_tool(Tool::Select);
    }

    pub(super) fn viewport_menu_items(&self) -> Vec<MenuItem> {
        let mut v: Vec<MenuItem> = SCALES.iter().enumerate().map(|(i, (l, _))| item(CMD_VP_SCALE + i, *l, false)).collect();
        v.push(sep());
        v.push(item(CMD_VP_CALIBRATE, "Calibrate inside this detail\u{2026}", false));
        v
    }

    pub(super) fn viewport_menu_result(&mut self, cmd: usize) {
        let (Some(rect), Some(sheet)) = (self.pending_vp, self.takeoff_sheet()) else { return };
        if cmd == CMD_VP_CALIBRATE {
            self.calib_vp = Some(rect);
            self.pending_vp = None;
            self.set_tool(Tool::Calibrate);
            self.status = Some("Detail scale: click two points a known distance apart inside the detail".into());
            return;
        }
        if let Some((label, ratio)) = cmd.checked_sub(CMD_VP_SCALE).and_then(|i| SCALES.get(i)) {
            self.add_viewport(sheet, rect, Scale::from_ratio(label, *ratio));
        }
        self.pending_vp = None;
    }

    pub(super) fn add_viewport(&mut self, sheet: u32, rect: [f32; 4], scale: Scale) {
        self.undo.push(Undo::Viewports(self.takeoff.viewports.clone()));
        self.takeoff.viewports.push(Viewport { sheet, rect, scale });
        self.save_takeoff();
        self.invalidate();
    }

    /// Scale-menu entries to remove this sheet's viewports.
    pub(super) fn viewport_remove_items(&self, base: usize) -> Vec<MenuItem> {
        let Some(sheet) = self.takeoff_sheet() else { return Vec::new() };
        self.takeoff
            .viewports
            .iter()
            .enumerate()
            .filter(|(_, v)| v.sheet == sheet)
            .map(|(i, v)| item(base + i, format!("Remove detail scale: {}", v.scale.label), false))
            .collect()
    }

    pub(super) fn remove_viewport(&mut self, i: usize) {
        if i < self.takeoff.viewports.len() {
            self.undo.push(Undo::Viewports(self.takeoff.viewports.clone()));
            self.takeoff.viewports.remove(i);
            self.save_takeoff();
        }
    }

    /// Outline detail viewports on the current sheet (takeoff mode).
    pub(super) fn paint_viewports(&self) {
        let Some(sheet) = self.takeoff_sheet() else { return };
        if self.markup_mode {
            return;
        }
        let s = self.s();
        for v in self.takeoff.viewports.iter().filter(|v| v.sheet == sheet) {
            let tl = self.to_screen([v.rect[0], v.rect[1]]);
            let br = self.to_screen([v.rect[2], v.rect[3]]);
            let r = RectF::new(tl[0], tl[1], br[0], br[1]);
            unsafe {
                self.brush.SetColor(&rgb(0x8e24aa, 0.8));
                self.gfx.ctx.DrawRectangle(&super::d2r(&r), &self.brush, 1.5 * s, None);
            }
            let label = format!("Detail scale {}", v.scale.label);
            let w = label.chars().count() as f32 * 7.0 * s + 14.0 * s;
            let tag = RectF::new(r.l, r.t - 20.0 * s, r.l + w, r.t);
            self.fill(&tag, 0x8e24aa, 0.9);
            self.text(&label, &self.fonts.label, &tag, 0xffffff);
        }
        if let Some(rect) = self.pending_vp {
            let tl = self.to_screen([rect[0], rect[1]]);
            let br = self.to_screen([rect[2], rect[3]]);
            self.fill(&RectF::new(tl[0], tl[1], br[0], br[1]), 0x8e24aa, 0.12);
        }
    }

    // -------------------------------------------------------------- presets

    pub(super) fn preset_menu_items(&self) -> Vec<MenuItem> {
        let name = |t: PresetTool| match t {
            PresetTool::Measure(k) => k.name(),
            PresetTool::Volume => "Volume",
            PresetTool::Markup(k) => k.name(),
        };
        let mut v: Vec<MenuItem> = self
            .settings
            .presets
            .iter()
            .enumerate()
            .map(|(i, p)| item(CMD_PRESET + i, format!("{}\t{}", p.name, name(p.tool)), self.active_preset.as_ref().is_some_and(|a| a.name == p.name)))
            .collect();
        if v.is_empty() {
            v.push(MenuItem { id: 0, label: "No presets yet".into(), checked: false, separator: false, children: Vec::new() });
        }
        v.push(sep());
        v.push(item(CMD_PRESET_SAVE, "Save current tool as a preset\u{2026}", false));
        if !self.settings.presets.is_empty() {
            v.push(submenu("Delete a preset", self.settings.presets.iter().enumerate().map(|(i, p)| item(CMD_PRESET_DEL + i, p.name.clone(), false)).collect()));
        }
        v
    }

    pub(super) fn preset_menu_result(&mut self, cmd: usize) {
        let n = self.settings.presets.len();
        if cmd == CMD_PRESET_SAVE {
            if self.preset_from_context("x").is_none() {
                self.pending_info = Some(
                    "Pick a measuring or markup tool first (or select a measurement), then save it as a preset with its group, layer and settings.".into(),
                );
                return;
            }
            self.prompt = Some(Prompt {
                title: "Save tool preset".into(),
                hint: "Name, e.g. 6\" slab, Type A wall, Door count, Red cloud".into(),
                text: String::new(),
                purpose: PromptFor::PresetName,
            });
        } else if let Some(i) = cmd.checked_sub(CMD_PRESET_DEL).filter(|i| *i < n) {
            self.settings.presets.remove(i);
            self.settings.save();
        } else if let Some(i) = cmd.checked_sub(CMD_PRESET).filter(|i| *i < n) {
            let p = self.settings.presets[i].clone();
            self.apply_preset(p);
        }
    }

    /// A preset from the selected measurement, or the current tool and
    /// active group/layer/style.
    fn preset_from_context(&self, name: &str) -> Option<Preset> {
        let base = Preset {
            name: name.to_string(),
            tool: PresetTool::Volume,
            group: self.group.clone(),
            layer: self.layer.clone(),
            ends: self.takeoff.default_ends,
            leader: self.takeoff.default_leader,
            depth_in: None,
            height_in: None,
            slope: None,
            color: self.settings.markup_color,
            width: self.settings.markup_width,
            font: self.settings.markup_font,
        };
        if let Some(m) = self.selected.and_then(|id| self.takeoff.items.iter().find(|m| m.id == id)) {
            let tool = if m.kind == Kind::Area && m.depth_in.is_some() { PresetTool::Volume } else { PresetTool::Measure(m.kind) };
            return Some(Preset {
                tool,
                group: m.group.clone(),
                layer: m.layer.clone(),
                ends: m.ends,
                leader: m.leader,
                depth_in: m.depth_in,
                height_in: m.height_in,
                slope: m.slope,
                ..base
            });
        }
        let tool = match self.tool {
            Tool::Measure(k) => PresetTool::Measure(k),
            Tool::Volume => PresetTool::Volume,
            Tool::Markup(MKind::CloudPoly) => PresetTool::Markup(MKind::Cloud),
            Tool::Markup(k) => PresetTool::Markup(k),
            _ => return None,
        };
        Some(Preset { tool, ..base })
    }

    pub(super) fn save_preset(&mut self, name: String) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        if let Some(p) = self.preset_from_context(&name) {
            self.settings.presets.retain(|q| q.name != name);
            self.settings.presets.push(p);
            self.settings.save();
            self.status = Some(format!("Saved preset \u{201C}{name}\u{201D} (Presets \u{25BE})"));
        }
    }

    fn apply_preset(&mut self, p: Preset) {
        match p.tool {
            PresetTool::Markup(k) => {
                self.markup_mode = true;
                self.settings.markup_color = p.color;
                self.settings.markup_width = p.width;
                self.settings.markup_font = p.font;
                self.set_tool(Tool::Markup(k));
            }
            PresetTool::Measure(k) => {
                self.markup_mode = false;
                self.set_tool(Tool::Measure(k));
            }
            PresetTool::Volume => {
                self.markup_mode = false;
                self.set_tool(Tool::Volume);
            }
        }
        if !matches!(p.tool, PresetTool::Markup(_)) {
            self.group = self.takeoff.ensure_group(&p.group);
            self.layer = self.takeoff.ensure_layer(&p.layer);
        }
        self.status = Some(format!("Preset: {}", p.name));
        self.active_preset = Some(p);
        self.invalidate();
    }

    /// Apply the active preset's extras to a new measurement. Returns true
    /// if it supplied a depth (so the Volume tool needn't ask).
    pub(super) fn preset_extras(&mut self, id: u64) -> bool {
        let Some(p) = self.active_preset.clone() else { return false };
        let tool_matches = match (p.tool, self.tool) {
            (PresetTool::Measure(a), Tool::Measure(b)) => a == b,
            (PresetTool::Volume, Tool::Volume) => true,
            _ => false,
        };
        if !tool_matches {
            return false;
        }
        if let Some(m) = self.takeoff.get_mut(id) {
            if matches!(m.kind, Kind::Length | Kind::Polyline) {
                m.ends = p.ends;
                m.height_in = p.height_in;
            }
            m.leader = p.leader;
            m.slope = p.slope;
            if m.kind == Kind::Area {
                m.depth_in = p.depth_in;
            }
        }
        p.depth_in.is_some()
    }

    // --------------------------------------------------------- sheet tools

    pub(super) fn sheets_menu_items(&self) -> Vec<MenuItem> {
        let has = self.doc.is_some();
        let mut v = vec![
            item(CMD_SHEETS_COMBINE, "Combine PDFs into one set\u{2026}", false),
            item(CMD_SHEETS_FOLDER, "Open a folder of PDFs as one set\u{2026}", false),
        ];
        if has {
            v.extend([
                sep(),
                item(CMD_SHEETS_EXTRACT, "Extract sheets to a new PDF\u{2026}", false),
                item(CMD_SHEETS_DELETE, "Save a copy without some sheets\u{2026}", false),
                item(CMD_SHEETS_CW, "Save a copy with this sheet rotated \u{21BB}\u{2026}", false),
                item(CMD_SHEETS_CCW, "Save a copy with this sheet rotated \u{21BA}\u{2026}", false),
            ]);
        }
        v
    }

    pub(super) fn sheets_menu_result(&mut self, cmd: usize) {
        let cur = self.doc.as_ref().map(|d| d.current).unwrap_or(0);
        match cmd {
            CMD_SHEETS_COMBINE => self.pending_sheet_pick = Some(SheetPick::Combine),
            CMD_SHEETS_FOLDER => self.pending_sheet_pick = Some(SheetPick::Folder),
            CMD_SHEETS_EXTRACT | CMD_SHEETS_DELETE => {
                let delete = cmd == CMD_SHEETS_DELETE;
                self.prompt = Some(Prompt {
                    title: if delete { "Sheets to leave out".into() } else { "Sheets to extract".into() },
                    hint: format!("Positions, e.g. {}  or  1, 3-5 (this set has {})", cur + 1, self.doc.as_ref().map(|d| d.pages.len()).unwrap_or(0)),
                    text: format!("{}", cur + 1),
                    purpose: PromptFor::SheetRange { delete },
                });
            }
            CMD_SHEETS_CW | CMD_SHEETS_CCW => {
                let Some(doc) = &self.doc else { return };
                let stem = doc.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                self.pending_sheet_save = Some(SheetJob {
                    parts: vec![(doc.path.clone(), Vec::new())],
                    rotate: vec![(cur, if cmd == CMD_SHEETS_CW { 1 } else { -1 })],
                    default_name: format!("{stem} - rotated.pdf"),
                    default_dir: doc.path.parent().map(|p| p.to_path_buf()),
                    title: "Save the rotated copy".into(),
                });
            }
            _ => {}
        }
    }

    /// Typed sheet range for extract / leave-out.
    pub(super) fn sheet_range_entered(&mut self, text: &str, delete: bool) {
        let Some(doc) = &self.doc else { return };
        let n = doc.pages.len() as u32;
        let Some(sel) = sheets::parse_range(text, n) else {
            self.status = Some(format!("Couldn't read \u{201C}{text}\u{201D} \u{2014} use positions like 1, 3-5"));
            return;
        };
        let keep: Vec<u32> = if delete { (0..n).filter(|p| !sel.contains(p)).collect() } else { sel.clone() };
        if keep.is_empty() {
            self.status = Some("That would leave no sheets".into());
            return;
        }
        let stem = doc.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let label = if sel.len() == 1 {
            let name = self.sheet_label(sel[0]);
            if name.is_empty() { format!("sheet {}", sel[0] + 1) } else { name.split_whitespace().next().unwrap_or("").to_string() }
        } else {
            format!("{} sheets", keep.len())
        };
        self.pending_sheet_save = Some(SheetJob {
            parts: vec![(doc.path.clone(), keep)],
            rotate: Vec::new(),
            default_name: if delete { format!("{stem} - edited.pdf") } else { format!("{stem} - {label}.pdf") },
            default_dir: doc.path.parent().map(|p| p.to_path_buf()),
            title: "Save the new PDF".into(),
        });
    }

    /// Files picked for Combine / Folder: queue the save step.
    pub fn sheets_picked(&mut self, pick: SheetPick, mut files: Vec<PathBuf>, folder: Option<PathBuf>) {
        files.retain(|f| f.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")));
        files.sort_by_key(|f| sheets::natural_key(&f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
        if files.is_empty() {
            self.pending_info = Some("No PDF files were found there.".into());
            return;
        }
        let (name, dir) = match (pick, &folder) {
            (SheetPick::Folder, Some(f)) => (
                format!("{} - set.pdf", f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Drawings".into())),
                Some(f.clone()),
            ),
            _ => ("Combined set.pdf".into(), files[0].parent().map(|p| p.to_path_buf())),
        };
        self.pending_sheet_save = Some(SheetJob {
            parts: files.into_iter().map(|f| (f, Vec::new())).collect(),
            rotate: Vec::new(),
            default_name: name,
            default_dir: dir,
            title: "Save the combined set".into(),
        });
    }

    /// Build the new PDF on a background thread, then open it in a tab.
    pub fn run_sheet_job(&mut self, job: SheetJob, dst: PathBuf) {
        if job.parts.iter().any(|(p, _)| *p == dst) {
            self.pending_error = Some("Choose a new file name \u{2014} Red-Beam never overwrites the source PDFs.".into());
            return;
        }
        self.status = Some("Building the new PDF\u{2026}".into());
        self.run_bg_open(move || {
            crate::pdfium::assemble(&job.parts, &job.rotate, &dst)
                .map(|n| (format!("Created {} ({n} sheets)", dst.display()), Some(dst.clone())))
                .map_err(|e| format!("Could not create {}:\n{e}", dst.display()))
        });
    }

    /// Small helper for the toolbar: label of the preset button.
    pub(super) fn presets_label(&self) -> String {
        match &self.active_preset {
            Some(p) => format!("\u{2605} {} \u{25BE}", p.name),
            None => "Presets \u{25BE}".into(),
        }
    }
}

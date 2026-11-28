//! Markup UI: drawing tools, selection/editing, properties menus, the
//! Markups list (side panel), and saving a PDF copy with markups (and
//! optionally takeoffs) as real PDF annotations.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows_numerics::Vector2;

use super::tools::{item, sep, submenu, EditDrag, MenuItem, MenuKind, Prompt, PromptFor, TBtn, Tool, Undo};
use super::{d2r, rgb, App, TEXT, TEXT_DIM, WM_APP_BG};
use crate::markup::{MKind, Markup, Prim, Status, COLORS, STAMPS};
use crate::pdfium::AnnotSpec;
use crate::takeoff::{self, Kind};
use crate::view::RectF;

/// A markup being drawn.
pub struct MDraft {
    pub kind: MKind,
    pub start: [f32; 2],
    pub cur: [f32; 2],
    /// Pen strokes.
    pub pts: Vec<[f32; 2]>,
    /// Arrow/line/callout placed by click-click: waiting for the 2nd click.
    pub second: bool,
}

const CMD_STATUS: usize = 20_000;
const CMD_COLOR: usize = 21_000;
const CMD_WIDTH: usize = 22_000;
const CMD_FONT: usize = 23_000;
const CMD_STAMP: usize = 24_000;
const CMD_EDIT_TEXT: usize = 25_000;
const CMD_M_DUPLICATE: usize = 25_001;
const CMD_M_DELETE: usize = 25_002;
const CMD_SAVE_MARKUPS: usize = 25_003;
const CMD_SAVE_ALL: usize = 25_004;
const CMD_SAVE_FLAT: usize = 25_005;
const CMD_PRINT: usize = 25_006;
const CMD_IMPORT: usize = 25_007;
const CMD_TOGGLE_ANNOTS: usize = 25_008;

use super::format::WIDTHS;
const FONTS: [(&str, f32); 5] = [("Small (10)", 10.0), ("Medium (14)", 14.0), ("Large (20)", 20.0), ("X-Large (28)", 28.0), ("Huge (40)", 40.0)];

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn author() -> String {
    std::env::var("USERNAME").unwrap_or_default()
}

impl App {
    // ------------------------------------------------------------ toolbar

    pub(super) fn markup_toolbar_specs(&self) -> Vec<(TBtn, String, f32, f32)> {
        let width = WIDTHS.iter().find(|(_, w)| (*w - self.settings.markup_width).abs() < 0.01).map(|(n, _)| *n).unwrap_or("Width");
        vec![
            (TBtn::Mode(false), "Takeoff".into(), 70.0, 0.0),
            (TBtn::Mode(true), "Markup".into(), 70.0, 10.0),
            (TBtn::Select, "Select".into(), 60.0, 8.0),
            (TBtn::MTool(MKind::Cloud), "Cloud".into(), 58.0, 4.0),
            (TBtn::MTool(MKind::Text), "Text".into(), 50.0, 4.0),
            (TBtn::MTool(MKind::Callout), "Callout".into(), 66.0, 4.0),
            (TBtn::MTool(MKind::Arrow), "Arrow".into(), 58.0, 4.0),
            (TBtn::MTool(MKind::Line), "Line".into(), 48.0, 4.0),
            (TBtn::MTool(MKind::Rect), "Rect".into(), 50.0, 4.0),
            (TBtn::MTool(MKind::Ellipse), "Ellipse".into(), 62.0, 4.0),
            (TBtn::MTool(MKind::Highlight), "Highlight".into(), 76.0, 4.0),
            (TBtn::MTool(MKind::Pen), "Pen".into(), 46.0, 4.0),
            (TBtn::Stamp, "Stamp \u{25BE}".into(), 70.0, 8.0),
            (TBtn::MColor, "Color \u{25BE}".into(), 82.0, 4.0),
            (TBtn::MWidth, format!("{width} \u{25BE}"), 80.0, 4.0),
            (TBtn::MFont, format!("{:.0}pt \u{25BE}", self.settings.markup_font * 0.75), 66.0, 8.0),
            (TBtn::SavePdf, "Save PDF \u{25BE}".into(), 88.0, 8.0),
            (TBtn::Presets, self.presets_label(), 130.0, 0.0),
        ]
    }

    /// Markup-mode shortcuts. Returns true if handled.
    pub(super) fn markup_key(&mut self, vk: VIRTUAL_KEY) -> bool {
        let tool = match vk.0 as u8 {
            b'C' => MKind::Cloud,
            b'T' => MKind::Text,
            b'K' => MKind::Callout,
            b'A' => MKind::Arrow,
            b'L' => MKind::Line,
            b'R' => MKind::Rect,
            b'E' => MKind::Ellipse,
            b'H' => MKind::Highlight,
            // Takeoff-only letters do nothing in markup mode.
            b'Y' | b'U' | b'X' => return true,
            _ => return false,
        };
        self.set_tool(Tool::Markup(tool));
        true
    }

    // ------------------------------------------------------------- state

    fn markup(&self, id: u64) -> Option<&Markup> {
        self.takeoff.markups.iter().find(|m| m.id == id)
    }

    fn markup_mut(&mut self, id: u64) -> Option<&mut Markup> {
        self.takeoff.markups.iter_mut().find(|m| m.id == id)
    }

    /// Markups drawn on the current sheet.
    fn sheet_markups(&self) -> impl Iterator<Item = &Markup> {
        let sheet = self.takeoff_sheet();
        self.takeoff.markups.iter().filter(move |m| Some(m.sheet) == sheet)
    }

    pub(super) fn add_markup(&mut self, kind: MKind, points: Vec<[f32; 2]>, text: String) -> Option<u64> {
        let sheet = self.takeoff_sheet()?;
        self.takeoff.next_id += 1;
        let id = self.takeoff.next_id;
        let font = if kind == MKind::Stamp { self.settings.markup_font.max(18.0) } else { self.settings.markup_font };
        let color = if kind == MKind::Highlight && self.settings.markup_color != 0xffeb3b { 0xffeb3b } else { self.settings.markup_color };
        self.takeoff.markups.push(Markup {
            id,
            sheet,
            kind,
            points,
            text,
            color,
            width: self.settings.markup_width,
            font,
            status: crate::markup::Status::None,
            author: author(),
            created: now(),
            bold: self.settings.markup_bold && kind.has_text(),
            italic: self.settings.markup_italic && kind.has_text(),
            text_color: self.settings.markup_text_color.filter(|_| kind.has_text()),
            fill: self.settings.markup_fill.filter(|_| matches!(kind, MKind::Text | MKind::Callout)),
            source: None,
        });
        self.undo.push(Undo::MarkupAdded(id));
        self.set_sel_markup(Some(id));
        self.selected = None;
        self.save_takeoff();
        self.invalidate();
        Some(id)
    }

    pub(super) fn ask_markup_text(&mut self, id: u64) {
        let text = self.markup(id).map(|m| m.text.clone()).unwrap_or_default();
        self.prompt = Some(Prompt {
            title: "Markup text".into(),
            hint: "Type the text. It wraps to the box; drag the box's corners to resize it.".into(),
            text,
            purpose: PromptFor::MarkupText { id },
        });
    }

    pub(super) fn set_markup_text(&mut self, id: u64, text: String) {
        let Some(before) = self.markup(id).cloned() else { return };
        if before.text == text {
            return;
        }
        if let Some(m) = self.markup_mut(id) {
            m.text = text;
        }
        self.undo.push(Undo::MarkupModified(before));
        self.save_takeoff();
    }

    pub(super) fn duplicate_markup(&mut self) {
        let Some(m) = self.selected_markup.and_then(|id| self.markup(id)).cloned() else { return };
        let off = 24.0 * self.s() / self.view.zoom;
        self.takeoff.next_id += 1;
        let mut c = m;
        c.id = self.takeoff.next_id;
        c.created = now();
        c.translate(off, off);
        let id = c.id;
        self.takeoff.markups.push(c);
        self.undo.push(Undo::MarkupAdded(id));
        self.set_sel_markup(Some(id));
        self.save_takeoff();
        self.invalidate();
    }

    // ------------------------------------------------------------- input

    /// Left button down with a markup tool.
    pub(super) fn markup_down(&mut self, kind: MKind, x: f32, y: f32) -> bool {
        let p = self.place_pt(x, y);
        match kind {
            MKind::Stamp => {
                self.stamp_at = Some(p);
                self.pending_menu = Some(MenuKind::Stamp);
            }
            MKind::CloudPoly => {
                self.pending_pts.push(p);
            }
            _ => {
                // Second click of a click-click arrow / line / callout.
                if let Some(d) = self.mdraft.as_mut().filter(|d| d.second) {
                    d.cur = p;
                    let d = self.mdraft.take().unwrap();
                    self.commit_draft(d);
                    return true;
                }
                if kind == MKind::Cloud && !self.pending_pts.is_empty() {
                    // Already outlining an irregular cloud: keep clicking.
                    self.pending_pts.push(p);
                    self.tool = Tool::Markup(MKind::CloudPoly);
                    return true;
                }
                self.mdraft = Some(MDraft { kind, start: p, cur: p, pts: vec![p], second: false });
            }
        }
        self.invalidate();
        true
    }

    /// Mouse move while drawing a markup. Returns true if drawing.
    pub(super) fn markup_draft_move(&mut self, x: f32, y: f32) -> bool {
        if self.mdraft.is_none() {
            return false;
        }
        let p = self.place_pt(x, y);
        let min_step = 1.5 / self.view.zoom;
        if let Some(d) = self.mdraft.as_mut() {
            d.cur = p;
            if d.kind == MKind::Pen && d.pts.last().is_none_or(|q| takeoff::dist(*q, p) as f32 >= min_step) && !d.second {
                d.pts.push(p);
            }
        }
        self.invalidate();
        true
    }

    /// Left button up while drawing. Returns true if it was a markup draft.
    pub(super) fn markup_draft_up(&mut self, x: f32, y: f32) -> bool {
        let Some(d) = self.mdraft.as_ref() else { return false };
        if d.second {
            return true;
        }
        let p = self.place_pt(x, y);
        let dragged = {
            let (a, b) = (self.to_screen(d.start), self.to_screen(p));
            (a[0] - b[0]).hypot(a[1] - b[1]) > 5.0
        };
        let mut d = self.mdraft.take().unwrap();
        d.cur = p;
        if self.tool == Tool::Viewport && !dragged {
            self.status = Some("Drag a box around the detail".into());
            return true;
        }
        if dragged {
            self.commit_draft(d);
        } else {
            match d.kind {
                // A click (not a drag) with Cloud starts an irregular cloud.
                MKind::Cloud => {
                    self.pending_pts = vec![d.start];
                    self.tool = Tool::Markup(MKind::CloudPoly);
                    self.status = Some("Cloud: click around the area, then Enter or double-click".into());
                }
                // Click-click placement.
                MKind::Arrow | MKind::Line | MKind::Callout => {
                    d.second = true;
                    self.mdraft = Some(d);
                }
                // A click with box tools: a default-size box.
                MKind::Text | MKind::Rect | MKind::Ellipse | MKind::Highlight => {
                    let (w, h) = (180.0, if d.kind == MKind::Text { 60.0 } else { 100.0 });
                    d.cur = [d.start[0] + w, d.start[1] + h];
                    self.commit_draft(d);
                }
                _ => {}
            }
        }
        self.invalidate();
        true
    }

    fn commit_draft(&mut self, d: MDraft) {
        let (a, b) = (d.start, d.cur);
        let boxed = vec![[a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]];
        if self.tool == Tool::Viewport {
            self.viewport_drawn([boxed[0][0], boxed[0][1], boxed[1][0], boxed[1][1]]);
            return;
        }
        match d.kind {
            MKind::Cloud | MKind::Rect | MKind::Ellipse | MKind::Highlight => {
                self.add_markup(d.kind, boxed, String::new());
            }
            MKind::Text => {
                if let Some(id) = self.add_markup(MKind::Text, boxed, String::new()) {
                    self.ask_markup_text(id);
                }
            }
            MKind::Arrow | MKind::Line => {
                self.add_markup(d.kind, vec![a, b], String::new());
            }
            MKind::Callout => {
                // Tip at the press point, text box where the drag ended.
                let (w, h) = (170.0, 56.0);
                let (bx, by) = if b[0] >= a[0] { (b[0], b[1] - h / 2.0) } else { (b[0] - w, b[1] - h / 2.0) };
                if let Some(id) = self.add_markup(MKind::Callout, vec![a, [bx, by], [bx + w, by + h]], String::new()) {
                    self.ask_markup_text(id);
                }
            }
            MKind::Pen => {
                if d.pts.len() >= 2 {
                    self.add_markup(MKind::Pen, d.pts, String::new());
                }
            }
            MKind::CloudPoly | MKind::Stamp | MKind::Polygon => {}
        }
    }

    /// Enter / double-click / right-click finishes an irregular cloud.
    pub(super) fn finish_cloud_poly(&mut self) {
        let mut pts = std::mem::take(&mut self.pending_pts);
        pts.dedup_by(|a, b| takeoff::dist(*a, *b) < 0.01);
        if pts.len() >= 3 {
            self.add_markup(MKind::CloudPoly, pts, String::new());
        }
        self.status = None;
        self.tool = Tool::Markup(MKind::Cloud);
        self.invalidate();
    }

    fn place_stamp(&mut self, text: &str) {
        let Some(p) = self.stamp_at.take() else { return };
        let font = self.settings.markup_font.max(18.0);
        let w = text.chars().count() as f32 * font * 0.72 + font * 1.6;
        let h = font * 2.4;
        self.add_markup(MKind::Stamp, vec![[p[0] - w / 2.0, p[1] - h / 2.0], [p[0] + w / 2.0, p[1] + h / 2.0]], text.to_string());
    }

    /// Topmost markup on the current sheet under (x, y).
    pub(super) fn markup_at(&self, x: f32, y: f32) -> Option<u64> {
        let p = self.page_pt(x, y);
        let tol = 6.0 * self.s() / self.view.zoom;
        let items: Vec<&Markup> = self.sheet_markups().collect();
        items.iter().rev().find(|m| m.hits(p, tol)).map(|m| m.id)
    }

    pub(super) fn markup_handle_at(&self, x: f32, y: f32) -> Option<usize> {
        let m = self.selected_markup.and_then(|id| self.markup(id))?;
        let r = 7.0 * self.s();
        m.handles().iter().position(|p| {
            let q = self.to_screen(*p);
            (q[0] - x).abs() <= r && (q[1] - y).abs() <= r
        })
    }

    /// Select tool press: start moving/resizing a markup. True if consumed.
    pub(super) fn markup_select_down(&mut self, x: f32, y: f32, start: [f32; 2]) -> bool {
        if let (Some(index), Some(m)) = (self.markup_handle_at(x, y), self.selected_markup.and_then(|id| self.markup(id)).cloned()) {
            self.edit = Some(EditDrag::MarkupHandle { orig: m, index, moved: false });
            return true;
        }
        let Some(id) = self.markup_at(x, y) else { return false };
        let Some(m) = self.markup(id).cloned() else { return false };
        if unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0 {
            self.toggle_markup_selection(id);
            return true;
        }
        self.set_sel_markup(Some(id));
        self.selected = None;
        self.edit = Some(EditDrag::MarkupMove { orig: m, start, moved: false });
        self.invalidate();
        true
    }

    /// Drag of a markup (move or handle). True if a markup edit is active.
    pub(super) fn markup_edit_move(&mut self, p: [f32; 2], slop: f32) -> bool {
        let snapped = self.snap_pt(p).0;
        let next = match self.edit.as_mut() {
            Some(EditDrag::MarkupMove { orig, start, moved }) => {
                let (dx, dy) = (p[0] - start[0], p[1] - start[1]);
                (*moved || dx.hypot(dy) >= slop).then(|| {
                    *moved = true;
                    let mut m = orig.clone();
                    m.translate(dx, dy);
                    m
                })
            }
            Some(EditDrag::MarkupHandle { orig, index, moved }) => {
                *moved = true;
                let mut m = orig.clone();
                m.set_handle(*index, snapped);
                Some(m)
            }
            _ => return false,
        };
        if let Some(n) = next {
            if let Some(m) = self.markup_mut(n.id) {
                *m = n;
            }
            self.invalidate();
        }
        true
    }

    /// Double-click with Select on a text markup edits its text.
    pub(super) fn markup_dblclick(&mut self, x: f32, y: f32) -> bool {
        match self.markup_at(x, y).and_then(|id| self.markup(id)).filter(|m| m.kind.has_text()) {
            Some(m) => {
                let id = m.id;
                self.set_sel_markup(Some(id));
                self.ask_markup_text(id);
                true
            }
            None => false,
        }
    }

    // ------------------------------------------------------------- menus

    pub(super) fn markup_menu_items(&self, kind: MenuKind) -> Option<Vec<MenuItem>> {
        let colors = |base: usize, cur: u32| COLORS.iter().enumerate().map(|(i, (n, c))| item(base + i, *n, *c == cur)).collect::<Vec<_>>();
        let widths = |base: usize, cur: f32| WIDTHS.iter().enumerate().map(|(i, (n, w))| item(base + i, *n, (w - cur).abs() < 0.01)).collect::<Vec<_>>();
        let fonts = |base: usize, cur: f32| FONTS.iter().enumerate().map(|(i, (n, f))| item(base + i, *n, (f - cur).abs() < 0.01)).collect::<Vec<_>>();
        Some(match kind {
            MenuKind::Stamp => STAMPS.iter().enumerate().map(|(i, s)| item(CMD_STAMP + i, *s, false)).collect(),
            MenuKind::MColor => colors(CMD_COLOR, self.settings.markup_color),
            MenuKind::MWidth => widths(CMD_WIDTH, self.settings.markup_width),
            MenuKind::MFont => fonts(CMD_FONT, self.settings.markup_font),
            MenuKind::SavePdf => vec![
                item(CMD_SAVE_MARKUPS, "Save a copy with markups\u{2026}", false),
                item(CMD_SAVE_ALL, "Save a copy with markups and takeoffs\u{2026}", false),
                item(CMD_SAVE_FLAT, "Save a flattened copy (burned in, for issuing)\u{2026}", false),
                sep(),
                item(CMD_PRINT, "Print\u{2026}\tCtrl+P", false),
                sep(),
                item(CMD_IMPORT, "Import markups from this PDF (Bluebeam, Acrobat\u{2026})", false),
                item(CMD_TOGGLE_ANNOTS, "Show the PDF's original markups", !self.takeoff.hide_pdf_annots),
            ],
            MenuKind::MarkupContext { id } => {
                let m = self.markup(id)?;
                let mut v = vec![
                    submenu(format!("Status: {}", m.status.name()), Status::ALL.iter().enumerate().map(|(i, s)| item(CMD_STATUS + i, s.name(), *s == m.status)).collect()),
                    submenu("Color", colors(CMD_COLOR, m.color)),
                    submenu("Line width", widths(CMD_WIDTH, m.width)),
                ];
                if m.kind.has_text() {
                    v.push(submenu("Font size", fonts(CMD_FONT, m.font)));
                    v.push(item(CMD_EDIT_TEXT, "Edit text\u{2026}\tdouble-click", false));
                }
                v.push(sep());
                v.push(item(CMD_M_DUPLICATE, "Duplicate\tCtrl+D", false));
                v.push(item(CMD_M_DELETE, "Delete\tDel", false));
                v
            }
            _ => return None,
        })
    }

    /// Returns true if the menu was a markup menu.
    pub(super) fn markup_menu_result(&mut self, kind: MenuKind, cmd: usize) -> bool {
        let pick = |base: usize, n: usize| cmd.checked_sub(base).filter(|i| *i < n);
        match kind {
            MenuKind::Stamp => {
                if let Some(i) = pick(CMD_STAMP, STAMPS.len()) {
                    self.place_stamp(STAMPS[i]);
                }
            }
            MenuKind::MColor => {
                if let Some(i) = pick(CMD_COLOR, COLORS.len()) {
                    self.settings.markup_color = COLORS[i].1;
                    self.settings.save();
                }
            }
            MenuKind::MWidth => {
                if let Some(i) = pick(CMD_WIDTH, WIDTHS.len()) {
                    self.settings.markup_width = WIDTHS[i].1;
                    self.settings.save();
                }
            }
            MenuKind::MFont => {
                if let Some(i) = pick(CMD_FONT, FONTS.len()) {
                    self.settings.markup_font = FONTS[i].1;
                    self.settings.save();
                }
            }
            MenuKind::SavePdf => match cmd {
                CMD_SAVE_MARKUPS => self.pending_pdf_save = Some((false, false)),
                CMD_SAVE_ALL => self.pending_pdf_save = Some((true, false)),
                CMD_SAVE_FLAT => self.pending_pdf_save = Some((true, true)),
                CMD_PRINT => self.pending_print = true,
                CMD_IMPORT => self.start_import_scan(false),
                CMD_TOGGLE_ANNOTS => self.toggle_pdf_annots(),
                _ => {}
            },
            MenuKind::MarkupContext { id } => {
                let Some(before) = self.markup(id).cloned() else { return true };
                match cmd {
                    CMD_EDIT_TEXT => self.ask_markup_text(id),
                    CMD_M_DUPLICATE => {
                        self.set_sel_markup(Some(id));
                        self.duplicate_markup();
                    }
                    CMD_M_DELETE => {
                        self.set_sel_markup(Some(id));
                        if let Some(ix) = self.takeoff.markups.iter().position(|m| m.id == id) {
                            let m = self.takeoff.markups.remove(ix);
                            self.undo.push(Undo::MarkupRemoved(m));
                            self.set_sel_markup(None);
                            self.save_takeoff();
                        }
                    }
                    _ => {
                        let Some(m) = self.markup_mut(id) else { return true };
                        if let Some(i) = pick(CMD_STATUS, Status::ALL.len()) {
                            m.status = Status::ALL[i];
                        } else if let Some(i) = pick(CMD_COLOR, COLORS.len()) {
                            m.color = COLORS[i].1;
                        } else if let Some(i) = pick(CMD_WIDTH, WIDTHS.len()) {
                            m.width = WIDTHS[i].1;
                        } else if let Some(i) = pick(CMD_FONT, FONTS.len()) {
                            m.font = FONTS[i].1;
                        } else {
                            return true;
                        }
                        self.undo.push(Undo::MarkupModified(before));
                        self.save_takeoff();
                    }
                }
            }
            _ => return false,
        }
        true
    }

    // ---------------------------------------------------------- painting

    fn paint_prims(&self, prims: &[Prim]) {
        let ctx = &self.gfx.ctx;
        let z = self.view.zoom;
        for prim in prims {
            match prim {
                Prim::Path { rings, closed, stroke, fill } => {
                    let screen: Vec<Vec<[f32; 2]>> = rings.iter().map(|r| r.iter().map(|p| self.to_screen(*p)).collect()).collect();
                    let Some(geom) = self.geometry_rings(&screen, *closed) else { continue };
                    unsafe {
                        if let Some((c, a)) = fill {
                            self.brush.SetColor(&rgb(*c, *a));
                            ctx.FillGeometry(&geom, &self.brush, None);
                        }
                        if let Some((c, w)) = stroke {
                            self.brush.SetColor(&rgb(*c, 1.0));
                            ctx.DrawGeometry(&geom, &self.brush, (w * z).max(1.0), None);
                        }
                    }
                }
                Prim::Text { rect, text, size, color, bold, italic, center } => {
                    let tl = self.to_screen([rect[0], rect[1]]);
                    let br = self.to_screen([rect[2], rect[3]]);
                    let px = size * z;
                    if px < 2.0 || text.is_empty() {
                        continue;
                    }
                    let w: Vec<u16> = text.encode_utf16().collect();
                    unsafe {
                        let Ok(layout) = self.dwrite.CreateTextLayout(&w, &self.fonts.markup, (br[0] - tl[0]).max(1.0), (br[1] - tl[1]).max(1.0)) else {
                            continue;
                        };
                        let all = DWRITE_TEXT_RANGE { startPosition: 0, length: w.len() as u32 };
                        let _ = layout.SetFontSize(px, all);
                        if *bold {
                            let _ = layout.SetFontWeight(DWRITE_FONT_WEIGHT_BOLD, all);
                        }
                        if *italic {
                            let _ = layout.SetFontStyle(DWRITE_FONT_STYLE_ITALIC, all);
                        }
                        if *center {
                            let _ = layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
                            let _ = layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                        }
                        self.brush.SetColor(&rgb(*color, 1.0));
                        ctx.DrawTextLayout(Vector2 { X: tl[0], Y: tl[1] }, &layout, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP);
                    }
                }
            }
        }
    }

    /// Markups on this sheet, the one being drawn, and selection handles.
    pub(super) fn paint_markups(&self) {
        if self.takeoff_sheet().is_none() {
            return;
        }
        let s = self.s();
        for m in self.sheet_markups() {
            self.paint_prims(&m.prims());
            if m.status != Status::None {
                // Small status flag at the top-left corner.
                let b = m.bounds();
                let p = self.to_screen([b[0], b[1]]);
                let color = match m.status {
                    Status::Accepted | Status::Completed => 0x2e7d32,
                    Status::Rejected => 0xc62828,
                    _ => 0xf57c00,
                };
                let r = RectF::new(p[0] - 2.0 * s, p[1] - 18.0 * s, p[0] + 76.0 * s, p[1] - 2.0 * s);
                self.fill(&r, color, 0.9);
                self.text(m.status.name(), &self.fonts.label, &r, 0xffffff);
            }
        }
        // Other selected markups: outline only.
        for id in self.markup_selection().into_iter().skip(1) {
            let Some(m) = self.markup(id).filter(|m| Some(m.sheet) == self.takeoff_sheet()) else { continue };
            let b = m.bounds();
            let (tl, br) = (self.to_screen([b[0], b[1]]), self.to_screen([b[2], b[3]]));
            unsafe {
                self.brush.SetColor(&rgb(0x1e88e5, 0.9));
                self.gfx.ctx.DrawRectangle(&d2r(&RectF::new(tl[0] - 4.0 * s, tl[1] - 4.0 * s, br[0] + 4.0 * s, br[1] + 4.0 * s)), &self.brush, 1.5 * s, None);
            }
        }
        // Selection handles.
        if let Some(m) = self.selected_markup.and_then(|id| self.markup(id)) {
            let b = m.bounds();
            let (tl, br) = (self.to_screen([b[0], b[1]]), self.to_screen([b[2], b[3]]));
            unsafe {
                self.brush.SetColor(&rgb(0x1e88e5, 0.9));
                self.gfx.ctx.DrawRectangle(&d2r(&RectF::new(tl[0] - 4.0 * s, tl[1] - 4.0 * s, br[0] + 4.0 * s, br[1] + 4.0 * s)), &self.brush, 1.0, None);
            }
            let h = 4.5 * s;
            for p in m.handles() {
                let c = self.to_screen(p);
                let r = RectF::new(c[0] - h, c[1] - h, c[0] + h, c[1] + h);
                self.fill(&r, 0xffffff, 1.0);
                unsafe {
                    self.brush.SetColor(&rgb(0x1e88e5, 1.0));
                    self.gfx.ctx.DrawRectangle(&d2r(&r), &self.brush, 1.5 * s, None);
                }
            }
        }
        // Draft preview.
        let preview = |kind: MKind, points: Vec<[f32; 2]>| Markup {
            id: 0,
            sheet: 0,
            kind,
            points,
            text: String::new(),
            color: self.settings.markup_color,
            width: self.settings.markup_width,
            font: self.settings.markup_font,
            status: Status::None,
            author: String::new(),
            created: 0,
            bold: false,
            italic: false,
            text_color: None,
            fill: None,
            source: None,
        };
        if let Some(d) = &self.mdraft {
            let (a, b) = (d.start, d.cur);
            let boxed = vec![[a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]];
            let m = match d.kind {
                MKind::Pen => preview(MKind::Pen, d.pts.clone()),
                MKind::Arrow | MKind::Line => preview(d.kind, vec![a, b]),
                MKind::Callout => preview(MKind::Arrow, vec![b, a]),
                k => preview(k, boxed),
            };
            self.paint_prims(&m.prims());
        }
        if self.tool == Tool::Markup(MKind::CloudPoly) && !self.pending_pts.is_empty() {
            let mut pts = self.pending_pts.clone();
            pts.push(self.place_pt(self.mouse.0 as f32, self.mouse.1 as f32));
            if pts.len() >= 3 {
                self.paint_prims(&preview(MKind::CloudPoly, pts).prims());
            } else {
                let (a, b) = (self.to_screen(pts[0]), self.to_screen(pts[1]));
                self.line(a, b, self.settings.markup_color, 2.0 * s);
            }
        }
    }

    // -------------------------------------------------- Markups list (panel)

    /// Rows of the Markups tab: (markup id or header sheet, rect).
    pub(super) fn markup_rows(&self, c: &RectF, scroll: f32) -> (Vec<(Option<u64>, u32, RectF)>, f32) {
        let s = self.s();
        let mut y = c.t + 8.0 * s - scroll;
        let mut rows = Vec::new();
        let mut list: Vec<&Markup> = self.takeoff.markups.iter().collect();
        list.sort_by_key(|m| (m.sheet, m.created, m.id));
        let mut last_sheet = None;
        for m in list {
            if last_sheet != Some(m.sheet) {
                rows.push((None, m.sheet, RectF::new(c.l + 8.0 * s, y, c.r - 8.0 * s, y + 22.0 * s)));
                y += 24.0 * s;
                last_sheet = Some(m.sheet);
            }
            rows.push((Some(m.id), m.sheet, RectF::new(c.l + 6.0 * s, y, c.r - 6.0 * s, y + 40.0 * s)));
            y += 44.0 * s;
        }
        (rows, y + scroll - c.t + 20.0 * s)
    }

    pub(super) fn paint_markups_tab(&self, c: &RectF, scroll: f32) {
        let s = self.s();
        let (mx, my) = (self.mouse.0 as f32, self.mouse.1 as f32);
        let (rows, _) = self.markup_rows(c, scroll);
        if rows.is_empty() {
            self.text(
                "No markups yet. Switch the toolbar to Markup (M) and draw clouds, text, callouts, arrows or stamps.",
                &self.fonts.perf,
                &RectF::new(c.l + 12.0 * s, c.t + 12.0 * s, c.r - 8.0 * s, c.t + 80.0 * s),
                TEXT_DIM,
            );
            return;
        }
        for (id, sheet, r) in rows {
            if r.b < c.t || r.t > c.b {
                continue;
            }
            let Some(id) = id else {
                self.text(&format!("SHEET {}", sheet + 1), &self.fonts.ui, &r, TEXT_DIM);
                continue;
            };
            let Some(m) = self.markup(id) else { continue };
            let selected = self.selected_markup == Some(id);
            let hover = self.over_panel(mx, my) && r.contains_pt(mx, my);
            if selected || hover {
                self.fill(&r, if selected { 0x3d2a2a } else { 0x33373d }, 1.0);
            }
            self.fill(&RectF::new(r.l + 6.0 * s, r.t + 8.0 * s, r.l + 16.0 * s, r.t + 18.0 * s), m.color, 1.0);
            self.text(&m.summary(), &self.fonts.ui, &RectF::new(r.l + 24.0 * s, r.t + 1.0 * s, r.r - 84.0 * s, r.t + 21.0 * s), TEXT);
            let sub = if m.author.is_empty() { m.kind.name().to_string() } else { format!("{} \u{00B7} {}", m.kind.name(), m.author) };
            self.text(&sub, &self.fonts.small, &RectF::new(r.l + 24.0 * s, r.t + 20.0 * s, r.r - 84.0 * s, r.b - 2.0 * s), TEXT_DIM);
            if m.status != Status::None {
                let chip = RectF::new(r.r - 80.0 * s, r.t + 9.0 * s, r.r - 4.0 * s, r.b - 9.0 * s);
                let color = match m.status {
                    Status::Accepted | Status::Completed => 0x2e7d32,
                    Status::Rejected => 0xc62828,
                    _ => 0xf57c00,
                };
                self.fill(&chip, color, 1.0);
                self.text(m.status.name(), &self.fonts.label, &chip, 0xffffff);
            }
        }
    }

    /// Click in the Markups tab: go to the markup and select it.
    pub(super) fn markups_tab_click(&mut self, c: &RectF, scroll: f32, x: f32, y: f32) {
        let Some((Some(id), sheet, _)) = self.markup_rows(c, scroll).0.into_iter().find(|(id, _, r)| id.is_some() && r.contains_pt(x, y)) else {
            return;
        };
        self.go_to_page(sheet as i64);
        let Some(b) = self.markup(id).map(|m| m.bounds()) else { return };
        self.set_sel_markup(Some(id));
        self.selected = None;
        // Centre the markup without changing the zoom (unless it's off-screen-large).
        let (cw, ch) = self.canvas();
        let (left, top) = (self.left_inset(), self.top_inset());
        let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
        let z = self.view.zoom.min((cw - left) * 0.8 / (b[2] - b[0]).max(1.0)).min((ch - top) * 0.8 / (b[3] - b[1]).max(1.0));
        self.view = crate::view::View { zoom: z, ox: left + (cw - left) / 2.0 - cx * z, oy: top + (ch - top) / 2.0 - cy * z };
        self.fit = crate::view::FitMode::None;
        self.view_changed();
        self.update_jobs(true);
    }

    // ---------------------------------------------------------- PDF export

    /// Drawing primitives for a measurement (export only).
    fn measure_prims(&self, m: &takeoff::Measure) -> Vec<Prim> {
        let color = self.takeoff.group_color(&m.group);
        let mut v = match m.kind {
            Kind::Length | Kind::Polyline => vec![Prim::Path { rings: vec![m.points.clone()], closed: false, stroke: Some((color, 1.5)), fill: None }],
            Kind::Area => {
                let mut rings = vec![m.points.clone()];
                rings.extend(m.holes.iter().cloned());
                vec![Prim::Path { rings, closed: true, stroke: Some((color, 1.5)), fill: Some((color, 0.18)) }]
            }
            Kind::Count => m
                .points
                .iter()
                .map(|p| Prim::Path {
                    rings: vec![(0..16).map(|i| {
                        let a = i as f32 / 16.0 * std::f32::consts::TAU;
                        [p[0] + 4.0 * a.cos(), p[1] + 4.0 * a.sin()]
                    }).collect()],
                    closed: true,
                    stroke: Some((0xffffff, 0.75)),
                    fill: Some((color, 1.0)),
                })
                .collect(),
        };
        let scale = self.takeoff.scale_of(m);
        let mut label = match m.kind {
            Kind::Count => format!("{} x{}", m.group, m.points.len()),
            _ => takeoff::fmt_qty(m.qty(scale), self.takeoff.units),
        };
        if let Some(w) = m.wall_area(scale) {
            label.push_str(&format!(" + {} wall", takeoff::fmt_area(w, self.takeoff.units)));
        }
        let p = m.label_pos();
        let size = 9.0;
        let w = label.chars().count() as f32 * size * 0.55 + 6.0;
        v.push(Prim::Path {
            rings: vec![vec![[p[0] - w / 2.0, p[1] - 7.0], [p[0] + w / 2.0, p[1] - 7.0], [p[0] + w / 2.0, p[1] + 7.0], [p[0] - w / 2.0, p[1] + 7.0]]],
            closed: true,
            stroke: Some((color, 0.75)),
            fill: Some((0xffffff, 0.9)),
        });
        v.push(Prim::Text { rect: [p[0] - w / 2.0, p[1] - 6.5, p[0] + w / 2.0, p[1] + 7.0], text: label, size, color, bold: false, italic: false, center: true });
        v
    }

    /// Markups (and optionally visible takeoffs) as annotations to write.
    fn export_specs(&self, with_takeoffs: bool) -> Vec<AnnotSpec> {
        let mut specs: Vec<AnnotSpec> = self
            .takeoff
            .markups
            .iter()
            .map(|m| AnnotSpec {
                page: m.sheet,
                subject: m.kind.name().into(),
                contents: if m.status == Status::None { m.text.clone() } else { format!("{} [{}]", m.text, m.status.name()) },
                author: m.author.clone(),
                name: format!("redbeam-markup-{}", m.id),
                prims: m.prims(),
                data: serde_json::to_string(m).ok(),
            })
            .collect();
        if with_takeoffs {
            for m in self.takeoff.visible_items() {
                specs.push(AnnotSpec {
                    page: m.sheet,
                    subject: format!("Takeoff: {}", m.group),
                    contents: format!("{} \u{2014} {}", m.group, takeoff::fmt_qty(m.qty(self.takeoff.scale_of(m)), self.takeoff.units)),
                    author: author(),
                    name: format!("redbeam-takeoff-{}", m.id),
                    prims: self.measure_prims(m),
                    data: serde_json::to_string(m).ok(),
                });
            }
        }
        specs
    }

    /// Save a copy of the drawing with markups (and optionally takeoffs),
    /// as PDF annotations or flattened, on a background thread.
    pub fn start_pdf_export(&mut self, dst: PathBuf, with_takeoffs: bool, flatten: bool) {
        let Some(doc) = &self.doc else { return };
        if dst == doc.path {
            self.pending_error = Some("Choose a new file name \u{2014} Red-Beam never overwrites your drawing.".into());
            return;
        }
        let specs = self.export_specs(with_takeoffs);
        if specs.is_empty() {
            self.pending_info = Some("There are no markups or takeoffs to save yet.".into());
            return;
        }
        let src = doc.path.clone();
        self.status = Some(format!("Saving {} items\u{2026}", specs.len()));
        let drop = self.takeoff.hide_pdf_annots;
        self.run_bg(move || match crate::pdfium::export_annotated(&src, &dst, &specs, flatten, drop) {
            Ok(n) if flatten => Ok(format!("Saved a flattened copy with {n} items burned in:\n{}\n\nYour original drawing was not changed.", dst.display())),
            Ok(n) => Ok(format!("Saved {n} annotations to\n{}\n\nThey show in Bluebeam, Acrobat and web browsers. Your original drawing was not changed.", dst.display())),
            Err(e) => Err(format!("Could not save {}:\n{e}", dst.display())),
        });
    }

    /// Run `job` on a background thread; its message is shown when done.
    fn run_bg(&self, job: impl FnOnce() -> std::result::Result<String, String> + Send + 'static) {
        self.run_bg_open(move || job().map(|s| (s, None)));
    }

    /// Like `run_bg`, but the job may name a PDF to open when it's done.
    pub(super) fn run_bg_open(&self, job: impl FnOnce() -> std::result::Result<(String, Option<PathBuf>), String> + Send + 'static) {
        let tx = self.bg_tx.clone();
        let hwnd = self.hwnd.0 as isize;
        std::thread::spawn(move || {
            let _ = tx.send(job());
            unsafe {
                let _ = PostMessageW(Some(windows::Win32::Foundation::HWND(hwnd as *mut _)), WM_APP_BG, WPARAM(0), LPARAM(0));
            }
        });
    }

    /// (sheet count, current sheet) for the print dialog.
    pub fn print_request(&self) -> Option<(u32, u32)> {
        self.doc.as_ref().map(|d| (d.pages.len() as u32, d.current))
    }

    /// Print `pages` to printer DC `hdc` (taken over and deleted when done),
    /// with markups and visible takeoffs drawn on.
    pub fn start_print(&mut self, hdc: isize, pages: Vec<u32>) {
        let Some(doc) = &self.doc else { return };
        let src = doc.path.clone();
        let specs = self.export_specs(true);
        let drop = self.takeoff.hide_pdf_annots;
        let job = doc.name.clone();
        self.status = Some(format!("Printing {} sheet(s)\u{2026}", pages.len()));
        self.run_bg(move || {
            let result = (|| {
                let data = std::fs::read(&src).map_err(|e| e.to_string())?;
                let data = if specs.is_empty() && !drop { data } else { crate::pdfium::build_pdf(&data, &specs, true, drop)?.0 };
                crate::pdfium::print_pdf(&data, &pages, hdc, &job)
            })();
            unsafe {
                let _ = windows::Win32::Graphics::Gdi::DeleteDC(windows::Win32::Graphics::Gdi::HDC(hdc as *mut _));
            }
            match result {
                Ok(n) => Ok(format!("Sent {n} sheet(s) to the printer.")),
                Err(e) => Err(format!("Printing failed:\n{e}")),
            }
        });
    }

    pub fn pdf_default_name(&self) -> String {
        let stem = self.doc.as_ref().and_then(|d| d.path.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "drawing".into());
        format!("{stem} - marked up.pdf")
    }

    pub(super) fn on_bg_msgs(&mut self) {
        self.on_import_msgs();
        while let Ok(m) = self.bg_rx.try_recv() {
            self.status = None;
            match m {
                Ok((info, Some(open))) => {
                    self.open(open);
                    self.status = Some(info);
                }
                Ok((info, None)) => self.pending_info = Some(info),
                Err(e) => self.pending_error = Some(e),
            }
        }
        self.invalidate();
    }
}

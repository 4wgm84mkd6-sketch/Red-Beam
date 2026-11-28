//! Format bar: a second toolbar row that appears when markups are
//! selected. Font size (A- / size / A+), bold, italic, text / line / fill
//! colour, line width and status, applied to every selected markup at
//! once with one undo step. Ctrl+click adds markups to the selection,
//! Ctrl+A selects all markups on the sheet, and "Select same" picks every
//! markup of the same kind. "Make default" makes new markups look the same.

use windows::Win32::UI::Input::KeyboardAndMouse::*;

use super::tools::{item, sep, MenuItem, MenuKind, TBtn, Undo};
use super::App;
use crate::markup::{step_font, MKind, Markup, Status, COLORS, FILLS, FONT_STEPS_PT};

const CMD_F_SIZE: usize = 26_000;
const CMD_F_TEXT: usize = 26_100;
const CMD_F_TEXT_SAME: usize = 26_199;
const CMD_F_LINE: usize = 26_200;
const CMD_F_FILL: usize = 26_300;
const CMD_F_WIDTH: usize = 26_400;
const CMD_F_STATUS: usize = 26_500;

pub const WIDTHS: [(&str, f32); 4] = [("Thin", 1.0), ("Medium", 2.0), ("Thick", 3.5), ("Heavy", 6.0)];

/// Cloud and polygon cloud count as the same kind for "Select same".
fn same_kind(a: MKind, b: MKind) -> bool {
    let norm = |k: MKind| if k == MKind::CloudPoly { MKind::Cloud } else { k };
    norm(a) == norm(b)
}

impl App {
    // --------------------------------------------------------- selection

    /// Select one markup (or none), dropping any multi-selection.
    pub(super) fn set_sel_markup(&mut self, id: Option<u64>) {
        self.selected_markup = id;
        self.sel_more.clear();
    }

    /// Every selected markup id, the primary (last clicked) first.
    pub(super) fn markup_selection(&self) -> Vec<u64> {
        let Some(p) = self.selected_markup else { return Vec::new() };
        let mut v = vec![p];
        for id in &self.sel_more {
            if !v.contains(id) && self.takeoff.markups.iter().any(|m| m.id == *id) {
                v.push(*id);
            }
        }
        v
    }

    fn sel_markups(&self) -> Vec<&Markup> {
        let ids = self.markup_selection();
        ids.iter().filter_map(|id| self.takeoff.markups.iter().find(|m| m.id == *id)).collect()
    }

    /// Ctrl+click on a markup: add it to the selection, or take it out.
    pub(super) fn toggle_markup_selection(&mut self, id: u64) {
        self.selected = None;
        match self.selected_markup {
            None => self.selected_markup = Some(id),
            Some(p) if p == id => {
                // Promote the next one, if any.
                let rest: Vec<u64> = self.markup_selection().into_iter().skip(1).collect();
                self.selected_markup = rest.first().copied();
                self.sel_more = rest.into_iter().skip(1).collect();
            }
            Some(_) => {
                if let Some(i) = self.sel_more.iter().position(|x| *x == id) {
                    self.sel_more.remove(i);
                } else {
                    self.sel_more.push(id);
                }
            }
        }
        self.invalidate();
    }

    /// Select every markup on this sheet (optionally only of `kind`).
    pub(super) fn select_all_markups(&mut self, kind: Option<MKind>) {
        let Some(sheet) = self.takeoff_sheet() else { return };
        let ids: Vec<u64> = self
            .takeoff
            .markups
            .iter()
            .filter(|m| m.sheet == sheet && kind.is_none_or(|k| same_kind(k, m.kind)))
            .map(|m| m.id)
            .collect();
        self.selected = None;
        self.selected_markup = ids.first().copied();
        self.sel_more = ids.iter().skip(1).copied().collect();
        self.status = Some(match ids.len() {
            0 => "No markups on this sheet".into(),
            1 => "1 markup selected".into(),
            n => format!("{n} markups selected \u{2014} formatting applies to all of them"),
        });
        self.invalidate();
    }

    // ----------------------------------------------------------- applying

    /// Change every selected markup with `f`, as one undo step.
    fn apply_format(&mut self, f: impl Fn(&mut Markup)) {
        let ids = self.markup_selection();
        if ids.is_empty() {
            return;
        }
        let before: Vec<Markup> = self.takeoff.markups.iter().filter(|m| ids.contains(&m.id)).cloned().collect();
        for m in self.takeoff.markups.iter_mut().filter(|m| ids.contains(&m.id)) {
            f(m);
        }
        let changed = self.takeoff.markups.iter().filter(|m| ids.contains(&m.id)).zip(before.iter()).any(|(a, b)| a != b);
        if changed {
            self.undo.push(Undo::MarkupsModified(before));
            self.save_takeoff();
        }
        self.invalidate();
    }

    fn sel_has_text(&self) -> bool {
        self.sel_markups().iter().any(|m| m.kind.has_text())
    }

    pub(super) fn bump_font(&mut self, up: bool) {
        self.apply_format(|m| {
            if m.kind.has_text() {
                m.font = step_font(m.font, up);
            }
        });
    }

    pub(super) fn toggle_bold(&mut self) {
        let on = !self.sel_markups().iter().filter(|m| m.kind.has_text()).all(|m| m.bold);
        self.apply_format(|m| {
            if m.kind.has_text() {
                m.bold = on;
            }
        });
    }

    pub(super) fn toggle_italic(&mut self) {
        let on = !self.sel_markups().iter().filter(|m| m.kind.has_text()).all(|m| m.italic);
        self.apply_format(|m| {
            if m.kind.has_text() {
                m.italic = on;
            }
        });
    }

    /// New markups take the primary selected markup's look.
    fn make_default_style(&mut self) {
        let Some(m) = self.sel_markups().first().map(|m| (*m).clone()) else { return };
        self.settings.markup_color = m.color;
        self.settings.markup_width = m.width;
        if m.kind.has_text() {
            self.settings.markup_font = m.font;
            self.settings.markup_bold = m.bold;
            self.settings.markup_italic = m.italic;
            self.settings.markup_text_color = m.text_color;
        }
        if matches!(m.kind, MKind::Text | MKind::Callout) {
            self.settings.markup_fill = m.fill;
        }
        self.settings.save();
        self.status = Some(format!("New markups will look like this {}", m.kind.name().to_lowercase()));
        self.invalidate();
    }

    // ------------------------------------------------------------- toolbar

    /// The format bar's buttons (empty when no markup is selected).
    pub(super) fn format_specs(&self) -> Vec<(TBtn, String, f32, f32)> {
        let sel = self.sel_markups();
        let Some(first) = sel.first() else { return Vec::new() };
        let mut v = Vec::new();
        let info = if sel.len() == 1 { first.kind.name().to_string() } else { format!("{} selected", sel.len()) };
        v.push((TBtn::FmtInfo, info, 92.0, 6.0));
        if self.sel_has_text() {
            let t = sel.iter().find(|m| m.kind.has_text()).unwrap_or(first);
            let mixed = sel.iter().filter(|m| m.kind.has_text()).any(|m| (m.font - t.font).abs() > 0.01);
            let size = if mixed { "Mixed \u{25BE}".to_string() } else { format!("{:.0} pt \u{25BE}", t.font * 0.75) };
            v.push((TBtn::FontDown, "A\u{2212}".into(), 34.0, 2.0));
            v.push((TBtn::FontSize, size, 70.0, 2.0));
            v.push((TBtn::FontUp, "A+".into(), 34.0, 8.0));
            v.push((TBtn::Bold, "B".into(), 32.0, 2.0));
            v.push((TBtn::Italic, "I".into(), 32.0, 8.0));
            v.push((TBtn::TextColor, "Text \u{25BE}".into(), 76.0, 4.0));
        }
        v.push((TBtn::LineColor, if self.sel_has_text() { "Border \u{25BE}".into() } else { "Color \u{25BE}".into() }, 86.0, 4.0));
        if sel.iter().any(|m| matches!(m.kind, MKind::Text | MKind::Callout)) {
            v.push((TBtn::FillColor, "Fill \u{25BE}".into(), 70.0, 4.0));
        }
        let w = WIDTHS.iter().find(|(_, w)| (*w - first.width).abs() < 0.01).map(|(n, _)| *n).unwrap_or("Width");
        v.push((TBtn::FmtWidth, format!("{w} \u{25BE}"), 80.0, 4.0));
        v.push((TBtn::FmtStatus, format!("{} \u{25BE}", if first.status == Status::None { "Status" } else { first.status.name() }), 104.0, 8.0));
        v.push((TBtn::SelectSame, "Select same".into(), 96.0, 4.0));
        v.push((TBtn::MakeDefault, "Make default".into(), 104.0, 0.0));
        v
    }

    /// Swatch colour shown on a format-bar button.
    pub(super) fn format_swatch(&self, b: TBtn) -> Option<u32> {
        let m = self.sel_markups().first().map(|m| (*m).clone())?;
        let t = self.sel_markups().into_iter().find(|m| m.kind.has_text()).cloned().unwrap_or(m.clone());
        match b {
            TBtn::TextColor => Some(t.text_rgb()),
            TBtn::LineColor => Some(m.color),
            TBtn::FillColor => {
                let f = self.sel_markups().into_iter().find(|m| matches!(m.kind, MKind::Text | MKind::Callout)).and_then(|m| m.fill);
                Some(f.unwrap_or(0xffffff))
            }
            _ => None,
        }
    }

    /// Is a format toggle on (all selected text markups bold / italic)?
    pub(super) fn format_active(&self, b: TBtn) -> bool {
        let texts: Vec<&Markup> = self.sel_markups().into_iter().filter(|m| m.kind.has_text()).collect();
        !texts.is_empty()
            && match b {
                TBtn::Bold => texts.iter().all(|m| m.bold),
                TBtn::Italic => texts.iter().all(|m| m.italic),
                _ => false,
            }
    }

    /// Press on a format-bar button. Returns false for other buttons.
    pub(super) fn format_press(&mut self, b: TBtn) -> bool {
        match b {
            TBtn::FmtInfo => {}
            TBtn::FontDown => self.bump_font(false),
            TBtn::FontUp => self.bump_font(true),
            TBtn::FontSize => self.pending_menu = Some(MenuKind::FmtSize),
            TBtn::Bold => self.toggle_bold(),
            TBtn::Italic => self.toggle_italic(),
            TBtn::TextColor => self.pending_menu = Some(MenuKind::FmtText),
            TBtn::LineColor => self.pending_menu = Some(MenuKind::FmtLine),
            TBtn::FillColor => self.pending_menu = Some(MenuKind::FmtFill),
            TBtn::FmtWidth => self.pending_menu = Some(MenuKind::FmtWidth),
            TBtn::FmtStatus => self.pending_menu = Some(MenuKind::FmtStatus),
            TBtn::SelectSame => {
                if let Some(k) = self.sel_markups().first().map(|m| m.kind) {
                    self.select_all_markups(Some(k));
                }
            }
            TBtn::MakeDefault => self.make_default_style(),
            _ => return false,
        }
        true
    }

    // --------------------------------------------------------------- menus

    pub(super) fn format_menu_items(&self, kind: MenuKind) -> Option<Vec<MenuItem>> {
        let sel = self.sel_markups();
        let first = sel.first()?;
        let t = sel.iter().find(|m| m.kind.has_text()).unwrap_or(first);
        Some(match kind {
            MenuKind::FmtSize => FONT_STEPS_PT.iter().enumerate().map(|(i, pt)| item(CMD_F_SIZE + i, format!("{pt:.0} pt"), (t.font * 0.75 - pt).abs() < 0.01)).collect(),
            MenuKind::FmtText => {
                let mut v = vec![item(CMD_F_TEXT_SAME, "Same as border", t.text_color.is_none()), sep()];
                v.extend(COLORS.iter().enumerate().map(|(i, (n, c))| item(CMD_F_TEXT + i, *n, t.text_color == Some(*c))));
                v
            }
            MenuKind::FmtLine => COLORS.iter().enumerate().map(|(i, (n, c))| item(CMD_F_LINE + i, *n, first.color == *c)).collect(),
            MenuKind::FmtFill => {
                let f = sel.iter().find(|m| matches!(m.kind, MKind::Text | MKind::Callout)).and_then(|m| m.fill).unwrap_or(0xffffff);
                FILLS.iter().enumerate().map(|(i, (n, c))| item(CMD_F_FILL + i, *n, f == *c)).collect()
            }
            MenuKind::FmtWidth => WIDTHS.iter().enumerate().map(|(i, (n, w))| item(CMD_F_WIDTH + i, *n, (w - first.width).abs() < 0.01)).collect(),
            MenuKind::FmtStatus => Status::ALL.iter().enumerate().map(|(i, s)| item(CMD_F_STATUS + i, s.name(), *s == first.status)).collect(),
            _ => return None,
        })
    }

    /// Returns true if the menu was a format menu.
    pub(super) fn format_menu_result(&mut self, kind: MenuKind, cmd: usize) -> bool {
        let pick = |base: usize, n: usize| cmd.checked_sub(base).filter(|i| *i < n);
        match kind {
            MenuKind::FmtSize => {
                if let Some(i) = pick(CMD_F_SIZE, FONT_STEPS_PT.len()) {
                    let font = FONT_STEPS_PT[i] / 0.75;
                    self.apply_format(|m| {
                        if m.kind.has_text() {
                            m.font = font;
                        }
                    });
                }
            }
            MenuKind::FmtText => {
                let c = if cmd == CMD_F_TEXT_SAME { Some(None) } else { pick(CMD_F_TEXT, COLORS.len()).map(|i| Some(COLORS[i].1)) };
                if let Some(c) = c {
                    self.apply_format(|m| {
                        if m.kind.has_text() {
                            m.text_color = c;
                        }
                    });
                }
            }
            MenuKind::FmtLine => {
                if let Some(i) = pick(CMD_F_LINE, COLORS.len()) {
                    let c = COLORS[i].1;
                    self.apply_format(|m| m.color = c);
                }
            }
            MenuKind::FmtFill => {
                if let Some(i) = pick(CMD_F_FILL, FILLS.len()) {
                    let c = FILLS[i].1;
                    let fill = if c == 0xffffff { None } else { Some(c) };
                    self.apply_format(|m| {
                        if matches!(m.kind, MKind::Text | MKind::Callout) {
                            m.fill = fill;
                        }
                    });
                }
            }
            MenuKind::FmtWidth => {
                if let Some(i) = pick(CMD_F_WIDTH, WIDTHS.len()) {
                    let w = WIDTHS[i].1;
                    self.apply_format(|m| m.width = w);
                }
            }
            MenuKind::FmtStatus => {
                if let Some(i) = pick(CMD_F_STATUS, Status::ALL.len()) {
                    let st = Status::ALL[i];
                    self.apply_format(|m| m.status = st);
                }
            }
            _ => return false,
        }
        true
    }

    // ---------------------------------------------------------- shortcuts

    /// Formatting shortcuts while markups are selected. True if handled.
    pub(super) fn format_key(&mut self, vk: VIRTUAL_KEY, ctrl: bool) -> bool {
        if !ctrl {
            return false;
        }
        let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        if vk.0 == b'A' as u16 && self.markup_mode {
            self.select_all_markups(None);
            return true;
        }
        if self.selected_markup.is_none() || !self.sel_has_text() {
            return false;
        }
        match vk {
            _ if vk.0 == b'B' as u16 => self.toggle_bold(),
            _ if vk.0 == b'I' as u16 => self.toggle_italic(),
            VK_OEM_PERIOD if shift => self.bump_font(true),
            VK_OEM_COMMA if shift => self.bump_font(false),
            VK_OEM_6 => self.bump_font(true),
            VK_OEM_4 => self.bump_font(false),
            _ => return false,
        }
        true
    }

    /// Tooltip for a format-bar button.
    pub(super) fn format_tip(&self, b: TBtn) -> Option<&'static str> {
        Some(match b {
            TBtn::FontDown => "Smaller text  (Ctrl+[)",
            TBtn::FontUp => "Bigger text  (Ctrl+])",
            TBtn::FontSize => "Font size",
            TBtn::Bold => "Bold  (Ctrl+B)",
            TBtn::Italic => "Italic  (Ctrl+I)",
            TBtn::TextColor => "Text colour",
            TBtn::LineColor => "Border / line colour",
            TBtn::FillColor => "Box fill",
            TBtn::FmtWidth => "Line width",
            TBtn::FmtStatus => "Review status",
            TBtn::SelectSame => "Select every markup of this kind on the sheet, to format them together",
            TBtn::MakeDefault => "New markups will look like the selected one",
            TBtn::FmtInfo => "Ctrl+click markups to select several; Ctrl+A selects all on this sheet",
            _ => return None,
        })
    }
}

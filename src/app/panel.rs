//! Left side panel (like Bluebeam's): Pages (thumbnails), Files (pinned and
//! recent) and Settings. Drawn over the left edge of the canvas; the view's
//! fit and the takeoff toolbar leave room for it.

use std::path::PathBuf;

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use super::{d2r, App, Cached, FitMode, ACCENT, BTN_BG, TEXT, TEXT_DIM, WM_APP_OPEN};
use crate::settings::{same_path, PanelTab};
use crate::sheets;
use crate::takeoff::Units;
use crate::view::RectF;
use crate::worker::{Engine, Job, JobKind};

const PANEL_W: f32 = 330.0;
const TAB_H: f32 = 38.0;
/// Thumbnails render at this long-edge size (crisp up to ~150% UI scale).
const THUMB_PX: f32 = 360.0;
const PANEL_BG: u32 = 0x24272b;
/// Height of a chip in the Pages tab's discipline jump bar.
const CHIP_H: f32 = 28.0;
/// Row height of the Pages tab's list view.
const LIST_ROW: f32 = 26.0;

/// A chip in the Pages tab's jump bar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Chip {
    /// Discipline letter and its first sheet.
    Disc(char, u32),
    /// Switch between thumbnails and the compact list.
    ViewToggle,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SetId {
    Wheel,
    Snap,
    Units,
    Background,
    UiScale,
    Reopen,
    Updates,
    Engine,
    Perf,
}

pub struct FileRow {
    pub path: PathBuf,
    pub pinned: bool,
    pub rect: RectF,
    pub star: RectF,
}

impl App {
    // ------------------------------------------------------------ layout

    /// Width the panel takes from the left of the canvas (0 when closed).
    pub(super) fn left_inset(&self) -> f32 {
        if self.settings.panel_open { (PANEL_W * self.s()).round() } else { 0.0 }
    }

    pub(super) fn over_panel(&self, x: f32, y: f32) -> bool {
        let (_, ch) = self.canvas();
        self.settings.panel_open && x < self.left_inset() && y < ch
    }

    fn tab_rects(&self) -> [(PanelTab, RectF, &'static str); 5] {
        let s = self.s();
        let w = self.left_inset();
        let tw = (w - 16.0 * s) / 5.0;
        let r = |i: f32| RectF::new(8.0 * s + tw * i, 6.0 * s, 8.0 * s + tw * (i + 1.0) - 2.0 * s, TAB_H * s);
        [
            (PanelTab::Pages, r(0.0), "Pages"),
            (PanelTab::Files, r(1.0), "Files"),
            (PanelTab::Markups, r(2.0), "Markups"),
            (PanelTab::Find, r(3.0), "Find"),
            (PanelTab::Settings, r(4.0), "Settings"),
        ]
    }

    fn content_rect(&self) -> RectF {
        let (_, ch) = self.canvas();
        RectF::new(0.0, (TAB_H + 8.0) * self.s(), self.left_inset(), ch)
    }

    fn tab_index(tab: PanelTab) -> usize {
        match tab {
            PanelTab::Pages => 0,
            PanelTab::Files => 1,
            PanelTab::Markups => 2,
            PanelTab::Find => 3,
            PanelTab::Settings => 4,
        }
    }

    fn scroll(&self) -> f32 {
        self.panel_scroll[Self::tab_index(self.settings.panel_tab)]
    }

    /// Disciplines of the open set: (letter, first sheet, sheet count).
    pub(super) fn disc_groups(&self) -> Vec<(char, u32, usize)> {
        let Some(doc) = &self.doc else { return Vec::new() };
        let nums: Vec<Option<&str>> = doc.names.iter().map(|n| n.number.as_deref()).collect();
        sheets::discipline_groups(&nums)
    }

    /// Discipline letter of sheet `p`, if its number is known.
    pub(super) fn disc_of(&self, p: u32) -> Option<char> {
        self.doc.as_ref().and_then(|d| d.names.get(p as usize)).and_then(|n| n.number.as_deref()).and_then(sheets::discipline)
    }

    /// The jump bar's chips, and where the sheet list starts below them.
    pub(super) fn chip_layout(&self) -> (Vec<(Chip, RectF, String)>, f32) {
        let c = self.content_rect();
        if self.doc.is_none() {
            return (Vec::new(), c.t);
        }
        let s = self.s();
        let h = CHIP_H * s;
        let toggle_w = 62.0 * s;
        let mut y = c.t + 6.0 * s;
        let toggle = RectF::new(c.r - 10.0 * s - toggle_w, y, c.r - 10.0 * s, y + h);
        let mut chips = vec![(Chip::ViewToggle, toggle, if self.settings.pages_list { "Thumbs".to_string() } else { "List".to_string() })];
        let right = toggle.l - 6.0 * s;
        let mut x = c.l + 8.0 * s;
        for (d, first, n) in self.disc_groups() {
            let label = format!("{d} {n}");
            let w = (16.0 + label.chars().count() as f32 * 7.5) * s;
            if x + w > right && x > c.l + 8.0 * s {
                x = c.l + 8.0 * s;
                y += h + 4.0 * s;
            }
            chips.push((Chip::Disc(d, first), RectF::new(x, y, x + w, y + h), label));
            x += w + 4.0 * s;
        }
        (chips, y + h + 8.0 * s)
    }

    /// Where the sheet thumbnails / list scroll (below the jump bar).
    fn pages_rect(&self) -> RectF {
        let c = self.content_rect();
        RectF::new(c.l, self.chip_layout().1, c.r, c.b)
    }

    /// Jump to the first sheet of a discipline and scroll it to the top.
    pub(super) fn jump_discipline(&mut self, first: u32) {
        self.go_to_page(first as i64);
        let area = self.pages_rect();
        if let Some((_, r)) = self.thumb_rects().0.into_iter().find(|(p, _)| *p == first) {
            let i = Self::tab_index(PanelTab::Pages);
            self.panel_scroll[i] = (self.panel_scroll[i] + r.t - area.t - 6.0 * self.s()).max(0.0);
        }
        self.update_jobs(false);
        self.invalidate();
    }

    /// Ctrl+Shift+PgDn / PgUp: next or previous discipline.
    pub(super) fn step_discipline(&mut self, step: i32) {
        let groups = self.disc_groups();
        if groups.is_empty() {
            self.status = Some("No sheet numbers found to group by discipline".into());
            return;
        }
        let cur = self.doc.as_ref().map(|d| d.current).unwrap_or(0);
        let ix = match self.disc_of(cur).and_then(|d| groups.iter().position(|g| g.0 == d)) {
            Some(i) => (i as i32 + step).rem_euclid(groups.len() as i32) as usize,
            None => 0,
        };
        let (d, first, n) = groups[ix];
        self.status = Some(format!("{} ({d}), {n} sheets", sheets::discipline_name(d)));
        self.jump_discipline(first);
    }

    /// Thumbnail (or list row) slots for every sheet of the open set
    /// (screen coords, scroll applied), plus the total content height.
    pub(super) fn thumb_rects(&self) -> (Vec<(u32, RectF)>, f32) {
        let Some(doc) = &self.doc else { return (Vec::new(), 0.0) };
        let s = self.s();
        let c = self.pages_rect();
        if self.settings.pages_list {
            let mut y = c.t + 4.0 * s - self.scroll();
            let mut out = Vec::with_capacity(doc.pages.len());
            for i in 0..doc.pages.len() {
                out.push((i as u32, RectF::new(c.l + 6.0 * s, y.round(), c.r - 10.0 * s, (y + LIST_ROW * s - 2.0 * s).round())));
                y += LIST_ROW * s;
            }
            return (out, y + self.scroll() - c.t);
        }
        let tw = c.w() - 40.0 * s;
        let mut y = c.t + 10.0 * s - self.scroll();
        let mut out = Vec::with_capacity(doc.pages.len());
        for (i, (pw, ph)) in doc.pages.iter().enumerate() {
            let th = (tw * ph / pw).min(tw * 1.4);
            let w = th * pw / ph;
            let x = c.l + (c.w() - w) / 2.0;
            out.push((i as u32, RectF::new(x.round(), y.round(), (x + w).round(), (y + th).round())));
            y += th + 30.0 * s;
        }
        (out, y + self.scroll() - c.t)
    }

    fn file_rows(&self) -> (Vec<FileRow>, Vec<(f32, &'static str)>, f32) {
        let s = self.s();
        let c = self.content_rect();
        let mut y = c.t + 52.0 * s - self.scroll();
        let (mut rows, mut headers) = (Vec::new(), Vec::new());
        let row_h = 46.0 * s;
        let mut section = |title: &'static str, paths: Vec<PathBuf>, pinned: bool, y: &mut f32, rows: &mut Vec<FileRow>| {
            if paths.is_empty() {
                return;
            }
            headers.push((*y, title));
            *y += 24.0 * s;
            for p in paths {
                let rect = RectF::new(c.l + 6.0 * s, *y, c.r - 6.0 * s, *y + row_h - 4.0 * s);
                let star = RectF::new(rect.r - 32.0 * s, rect.t, rect.r, rect.b);
                rows.push(FileRow { path: p, pinned, rect, star });
                *y += row_h;
            }
            *y += 8.0 * s;
        };
        let pinned: Vec<PathBuf> = self.settings.pinned.clone();
        let recent: Vec<PathBuf> =
            self.settings.recent.iter().map(|r| r.path.clone()).filter(|p| !self.settings.is_pinned(p)).collect();
        section("PINNED", pinned, true, &mut y, &mut rows);
        section("RECENT", recent, false, &mut y, &mut rows);
        let total = y + self.scroll() - c.t;
        (rows, headers, total)
    }

    fn open_button_rect(&self) -> RectF {
        let s = self.s();
        let c = self.content_rect();
        RectF::new(c.l + 10.0 * s, c.t + 8.0 * s - self.scroll(), c.r - 10.0 * s, c.t + 40.0 * s - self.scroll())
    }

    fn setting_rows(&self) -> (Vec<(SetId, &'static str, String, RectF)>, f32) {
        let s = self.s();
        let c = self.content_rect();
        let on = |b: bool| if b { "On".to_string() } else { "Off".to_string() };
        let st = &self.settings;
        let items: Vec<(SetId, &'static str, String)> = vec![
            (SetId::Wheel, "Mouse wheel", if st.wheel_zooms { "Zooms".into() } else { "Scrolls".into() }),
            (SetId::UiScale, "Interface size", format!("{:.0}%", st.ui_scale * 100.0)),
            (SetId::Background, "Background", st.background.name().into()),
            (
                SetId::Units,
                "Takeoff units",
                match st.units_default {
                    Units::FeetInches => "ft-in".into(),
                    Units::DecimalFeet => "dec ft".into(),
                    Units::Metric => "metric".into(),
                },
            ),
            (SetId::Snap, "Snap by default", on(st.snap_default)),
            (SetId::Reopen, "Reopen last file", on(st.reopen_last)),
            (SetId::Updates, "Check for updates", on(st.check_updates)),
            (SetId::Engine, "PDF engine", self.worker.engine.name().into()),
            (SetId::Perf, "Performance readout", on(self.show_perf)),
        ];
        let row_h = 40.0 * s;
        let mut y = c.t + 8.0 * s - self.scroll();
        let rows = items
            .into_iter()
            .map(|(id, label, value)| {
                let r = RectF::new(c.l + 6.0 * s, y, c.r - 6.0 * s, y + row_h - 4.0 * s);
                y += row_h;
                (id, label, value, r)
            })
            .collect();
        (rows, y + self.scroll() - c.t + 80.0 * s)
    }

    fn content_height(&self) -> f32 {
        match self.settings.panel_tab {
            PanelTab::Pages => self.thumb_rects().1 + (self.pages_rect().t - self.content_rect().t),
            PanelTab::Files => self.file_rows().2,
            PanelTab::Markups => self.markup_rows(&self.content_rect(), self.scroll()).1,
            PanelTab::Find => self.find_content_height(&self.content_rect(), self.scroll()),
            PanelTab::Settings => self.setting_rows().1,
        }
    }

    // ------------------------------------------------------------ actions

    pub(super) fn toggle_panel(&mut self) {
        self.settings.panel_open = !self.settings.panel_open;
        self.settings.save();
        if self.fit != FitMode::None {
            self.apply_fit(self.fit);
        }
        self.update_jobs(false);
        self.invalidate();
    }

    pub(super) fn panel_wheel(&mut self, delta: f32) {
        let s = self.s();
        let c = self.content_rect();
        let max = (self.content_height() - c.h() + 20.0 * s).max(0.0);
        let i = Self::tab_index(self.settings.panel_tab);
        self.panel_scroll[i] = (self.panel_scroll[i] - delta * 90.0 * s).clamp(0.0, max);
        self.update_jobs(false);
        self.invalidate();
    }

    /// Keep the current sheet's thumbnail in view.
    pub(super) fn ensure_thumb_visible(&mut self) {
        let Some(cur) = self.doc.as_ref().map(|d| d.current) else { return };
        let (rects, _) = self.thumb_rects();
        let c = self.pages_rect();
        if let Some((_, r)) = rects.iter().find(|(p, _)| *p == cur) {
            let i = Self::tab_index(PanelTab::Pages);
            let pad = if self.settings.pages_list { 4.0 } else { 30.0 } * self.s();
            if r.t < c.t {
                self.panel_scroll[i] -= c.t - r.t + pad;
            } else if r.b > c.b {
                self.panel_scroll[i] += r.b - c.b + pad;
            }
            self.panel_scroll[i] = self.panel_scroll[i].max(0.0);
        }
    }

    /// Left click inside the panel.
    pub(super) fn panel_click(&mut self, x: f32, y: f32) {
        for (tab, r, _) in self.tab_rects() {
            if r.contains_pt(x, y) {
                self.settings.panel_tab = tab;
                self.settings.save();
                self.update_jobs(false);
                self.invalidate();
                return;
            }
        }
        if !self.content_rect().contains_pt(x, y) {
            return;
        }
        match self.settings.panel_tab {
            PanelTab::Pages => {
                if let Some((chip, ..)) = self.chip_layout().0.into_iter().find(|(_, r, _)| r.contains_pt(x, y)) {
                    match chip {
                        Chip::Disc(_, first) => self.jump_discipline(first),
                        Chip::ViewToggle => {
                            self.settings.pages_list = !self.settings.pages_list;
                            self.settings.save();
                            self.panel_scroll[Self::tab_index(PanelTab::Pages)] = 0.0;
                            self.ensure_thumb_visible();
                            self.update_jobs(false);
                        }
                    }
                    self.invalidate();
                    return;
                }
                if !self.pages_rect().contains_pt(x, y) {
                    return;
                }
                let list = self.settings.pages_list;
                let hit = self.thumb_rects().0.into_iter().find(|(_, r)| {
                    let s = self.s();
                    if list {
                        r.contains_pt(x, y)
                    } else {
                        RectF::new(r.l - 8.0 * s, r.t - 4.0 * s, r.r + 8.0 * s, r.b + 24.0 * s).contains_pt(x, y)
                    }
                });
                if let Some((p, _)) = hit {
                    self.go_to_page(p as i64);
                }
            }
            PanelTab::Files => {
                if self.open_button_rect().contains_pt(x, y) {
                    unsafe {
                        let _ = PostMessageW(Some(self.hwnd), WM_APP_OPEN, WPARAM(0), LPARAM(0));
                    }
                    return;
                }
                let Some(row) = self.file_rows().0.into_iter().find(|r| r.rect.contains_pt(x, y)) else { return };
                if row.star.contains_pt(x, y) {
                    self.settings.toggle_pin(&row.path);
                    self.settings.save();
                } else if row.path.exists() {
                    let page = self.settings.last_page(&row.path);
                    self.open_at(row.path, page);
                } else {
                    self.status = Some(format!("Not found: {}", row.path.display()));
                }
            }
            PanelTab::Markups => {
                let (c, sc) = (self.content_rect(), self.scroll());
                self.markups_tab_click(&c, sc, x, y);
            }
            PanelTab::Find => {
                let (c, sc) = (self.content_rect(), self.scroll());
                self.find_tab_click(&c, sc, x, y);
            }
            PanelTab::Settings => {
                if let Some((id, ..)) = self.setting_rows().0.into_iter().find(|(.., r)| r.contains_pt(x, y)) {
                    self.change_setting(id);
                }
            }
        }
        self.invalidate();
    }

    fn change_setting(&mut self, id: SetId) {
        match id {
            SetId::Wheel => self.settings.wheel_zooms = !self.settings.wheel_zooms,
            SetId::Snap => {
                self.settings.snap_default = !self.settings.snap_default;
                self.snap_on = self.settings.snap_default;
            }
            SetId::Units => {
                let next = match self.settings.units_default {
                    Units::FeetInches => Units::DecimalFeet,
                    Units::DecimalFeet => Units::Metric,
                    Units::Metric => Units::FeetInches,
                };
                self.settings.units_default = next;
                // Also switch the open drawing's takeoff.
                if self.doc.is_some() {
                    self.takeoff.units = next;
                    self.save_takeoff();
                }
            }
            SetId::Background => self.settings.background = self.settings.background.next(),
            SetId::UiScale => {
                self.settings.ui_scale = self.settings.next_ui_scale();
                self.set_dpi(self.dpi);
                if self.fit != FitMode::None {
                    self.apply_fit(self.fit);
                }
            }
            SetId::Reopen => self.settings.reopen_last = !self.settings.reopen_last,
            SetId::Updates => self.settings.check_updates = !self.settings.check_updates,
            SetId::Engine => self.toggle_engine(),
            SetId::Perf => self.show_perf = !self.show_perf,
        }
        self.settings.use_pdfium = self.worker.engine == Engine::Pdfium;
        self.settings.save();
        self.invalidate();
    }

    /// Thumbnail renders wanted for the visible part of the Pages tab.
    pub(super) fn thumb_jobs(&self) -> Vec<Job> {
        if !self.settings.panel_open || self.settings.panel_tab != PanelTab::Pages || self.settings.pages_list {
            return Vec::new();
        }
        let Some(doc) = &self.doc else { return Vec::new() };
        let c = self.content_rect();
        let margin = c.h();
        self.thumb_rects()
            .0
            .into_iter()
            .filter(|(_, r)| r.b > c.t - margin && r.t < c.b + margin)
            .filter(|(p, _)| !self.thumbs.contains_key(&(doc.id, *p)))
            .map(|(p, _)| {
                let (pw, ph) = doc.pages[p as usize];
                let scale = THUMB_PX / pw.max(ph);
                let (w, h) = ((pw * scale).ceil().max(1.0) as u32, (ph * scale).ceil().max(1.0) as u32);
                Job {
                    doc_id: doc.id,
                    page: p,
                    kind: JobKind::Thumb,
                    prefetch: true,
                    src: RectF::new(0.0, 0.0, w as f32 / scale, h as f32 / scale),
                    w,
                    h,
                    scale,
                    pair: None,
                }
            })
            .filter(|j| !self.failed.contains(&j.key()))
            .collect()
    }

    // ---------------------------------------------------------- painting

    pub(super) fn paint_panel(&self) {
        if !self.settings.panel_open {
            return;
        }
        let s = self.s();
        let (_, ch) = self.canvas();
        let w = self.left_inset();
        let ctx = &self.gfx.ctx;
        self.fill(&RectF::new(0.0, 0.0, w, ch), PANEL_BG, 1.0);
        self.fill(&RectF::new(w - 1.0, 0.0, w, ch), 0x000000, 0.7);
        let (mx, my) = (self.mouse.0 as f32, self.mouse.1 as f32);
        for (tab, r, label) in self.tab_rects() {
            let active = self.settings.panel_tab == tab;
            let hover = r.contains_pt(mx, my);
            self.fill(&r, if active { ACCENT } else if hover { 0x3a3e44 } else { BTN_BG }, 1.0);
            self.text(label, &self.fonts.ui_center, &r, TEXT);
        }
        let c = self.content_rect();
        unsafe {
            ctx.PushAxisAlignedClip(&d2r(&c), D2D1_ANTIALIAS_MODE_ALIASED);
        }
        match self.settings.panel_tab {
            PanelTab::Pages => self.paint_pages_tab(&c),
            PanelTab::Files => self.paint_files_tab(&c),
            PanelTab::Markups => self.paint_markups_tab(&c, self.scroll()),
            PanelTab::Find => self.paint_find_tab(&c, self.scroll()),
            PanelTab::Settings => self.paint_settings_tab(&c),
        }
        unsafe {
            ctx.PopAxisAlignedClip();
        }
        // Scroll indicator.
        let total = self.content_height();
        if total > c.h() {
            let frac = c.h() / total;
            let t = c.t + self.scroll() / total * c.h();
            self.fill(&RectF::new(w - 6.0 * s, t, w - 3.0 * s, t + c.h() * frac), 0x9aa0a6, 0.5);
        }
    }

    fn paint_pages_tab(&self, c: &RectF) {
        let s = self.s();
        let Some(doc) = &self.doc else {
            self.text("No drawing open \u{2014} recent files are in the Files tab", &self.fonts.ui_center, &RectF::new(c.l, c.t + 20.0 * s, c.r, c.t + 50.0 * s), TEXT_DIM);
            return;
        };
        let (mx, my) = (self.mouse.0 as f32, self.mouse.1 as f32);
        // Discipline jump bar.
        let cur_disc = self.disc_of(doc.current);
        let (chips, list_top) = self.chip_layout();
        for (chip, r, label) in &chips {
            let hover = self.over_panel(mx, my) && r.contains_pt(mx, my);
            let on = matches!(chip, Chip::Disc(d, _) if Some(*d) == cur_disc);
            self.fill(r, if hover { ACCENT } else if on { 0x4a3030 } else { BTN_BG }, 1.0);
            if on && !hover {
                self.fill(&RectF::new(r.l, r.b - 2.0 * s, r.r, r.b), ACCENT, 1.0);
            }
            self.text(label, &self.fonts.ui_center, r, TEXT);
        }
        self.fill(&RectF::new(c.l + 8.0 * s, list_top - 4.0 * s, c.r - 8.0 * s, list_top - 3.0 * s), 0x000000, 0.5);
        let area = RectF::new(c.l, list_top, c.r, c.b);
        unsafe {
            self.gfx.ctx.PushAxisAlignedClip(&d2r(&area), D2D1_ANTIALIAS_MODE_ALIASED);
        }
        if self.settings.pages_list {
            self.paint_pages_list(&area);
        } else {
            self.paint_thumbs(&area);
        }
        unsafe {
            self.gfx.ctx.PopAxisAlignedClip();
        }
    }

    /// Compact list: one row per sheet, discipline headers between groups.
    fn paint_pages_list(&self, c: &RectF) {
        let s = self.s();
        let Some(doc) = &self.doc else { return };
        let (mx, my) = (self.mouse.0 as f32, self.mouse.1 as f32);
        let mut last: Option<char> = None;
        for (p, r) in self.thumb_rects().0 {
            let d = self.disc_of(p);
            let new_group = d.is_some() && d != last;
            last = d.or(last);
            if r.b < c.t || r.t > c.b {
                continue;
            }
            let current = p == doc.current;
            let hover = self.over_panel(mx, my) && r.contains_pt(mx, my);
            if current || hover {
                self.fill(&r, if current { 0x3d2a2a } else { 0x33373d }, 1.0);
            }
            if current {
                self.fill(&RectF::new(r.l, r.t, r.l + 3.0 * s, r.b), ACCENT, 1.0);
            }
            if new_group {
                self.fill(&RectF::new(r.l, r.t, r.r, r.t + 1.0), 0x5a5f66, 1.0);
            }
            // Discipline badge.
            let badge = RectF::new(r.l + 8.0 * s, r.t + 4.0 * s, r.l + 26.0 * s, r.b - 4.0 * s);
            if let Some(d) = d {
                self.fill(&badge, if new_group { 0x50555c } else { 0x3a3e44 }, 1.0);
                self.text(&d.to_string(), &self.fonts.label, &badge, TEXT);
            }
            let name = self.sheet_label(p);
            let n_items = self.takeoff.items.iter().filter(|m| m.sheet == p).count();
            let mut text = if name.is_empty() { format!("Sheet {}", p + 1) } else { name };
            if n_items > 0 {
                text.push_str(&format!("  \u{00B7} {n_items}"));
            }
            let tr = RectF::new(badge.r + 8.0 * s, r.t, r.r - 34.0 * s, r.b);
            self.text(&text, &self.fonts.ui, &tr, if current { TEXT } else { 0xc8ccd1 });
            let nr = RectF::new(r.r - 40.0 * s, r.t, r.r - 6.0 * s, r.b);
            self.text(&format!("{}", p + 1), &self.fonts.ui_right, &nr, TEXT_DIM);
        }
    }

    fn paint_thumbs(&self, c: &RectF) {
        let s = self.s();
        let Some(doc) = &self.doc else { return };
        let (mx, my) = (self.mouse.0 as f32, self.mouse.1 as f32);
        for (p, r) in self.thumb_rects().0 {
            if r.b < c.t - 30.0 * s || r.t > c.b {
                continue;
            }
            let current = p == doc.current;
            let hover = self.over_panel(mx, my) && RectF::new(r.l - 8.0 * s, r.t - 4.0 * s, r.r + 8.0 * s, r.b + 24.0 * s).contains_pt(mx, my);
            if current || hover {
                let o = 4.0 * s;
                self.fill(&RectF::new(r.l - o, r.t - o, r.r + o, r.b + o), if current { ACCENT } else { 0x5a5f66 }, 1.0);
            }
            self.fill(&r, 0xffffff, 1.0);
            // Thumbnail, or the sheet's whole-page image if it's cached.
            let img: Option<&Cached> = self.thumbs.get(&(doc.id, p)).or_else(|| self.bases.get(&(doc.id, p)));
            match img {
                Some(cached) => unsafe {
                    self.gfx.ctx.DrawBitmap(&cached.bmp, Some(&d2r(&r)), 1.0, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, None, None);
                },
                None => {
                    self.text("\u{2026}", &self.fonts.ui_center, &r, 0xb0b4b9);
                }
            }
            let label = RectF::new(c.l, r.b + 4.0 * s, c.r, r.b + 22.0 * s);
            let n_items = self.takeoff.items.iter().filter(|m| m.sheet == p).count();
            let name = self.sheet_label(p);
            let base = if name.is_empty() { format!("{}", p + 1) } else { format!("{}  {name}", p + 1) };
            let text = if n_items > 0 { format!("{base}  \u{00B7} {n_items} takeoff") } else { base };
            self.text(&text, &self.fonts.ui_center, &label, if current { TEXT } else { TEXT_DIM });
        }
    }

    fn paint_files_tab(&self, c: &RectF) {
        let s = self.s();
        let (mx, my) = (self.mouse.0 as f32, self.mouse.1 as f32);
        let ob = self.open_button_rect();
        self.fill(&ob, if ob.contains_pt(mx, my) { ACCENT } else { BTN_BG }, 1.0);
        self.text("Open a PDF\u{2026}   (Ctrl+O)", &self.fonts.ui_center, &ob, TEXT);
        let (rows, headers, _) = self.file_rows();
        for (y, title) in headers {
            self.text(title, &self.fonts.ui, &RectF::new(c.l + 12.0 * s, y, c.r, y + 22.0 * s), TEXT_DIM);
        }
        if rows.is_empty() {
            self.text(
                "Files you open appear here. Click \u{2606} to pin one.",
                &self.fonts.ui,
                &RectF::new(c.l + 12.0 * s, ob.b + 14.0 * s, c.r - 8.0 * s, ob.b + 40.0 * s),
                TEXT_DIM,
            );
        }
        let open_path = self.doc.as_ref().map(|d| d.path.as_path());
        for row in rows {
            if row.rect.b < c.t || row.rect.t > c.b {
                continue;
            }
            let exists = row.path.exists();
            let is_open = open_path.is_some_and(|p| same_path(p, &row.path));
            let hover = self.over_panel(mx, my) && row.rect.contains_pt(mx, my);
            if is_open || hover {
                self.fill(&row.rect, if is_open { 0x3d2a2a } else { 0x33373d }, 1.0);
            }
            if is_open {
                self.fill(&RectF::new(row.rect.l, row.rect.t, row.rect.l + 3.0 * s, row.rect.b), ACCENT, 1.0);
            }
            let name = row.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let folder = row.path.parent().map(|p| p.display().to_string()).unwrap_or_default();
            let tr = RectF::new(row.rect.l + 10.0 * s, row.rect.t + 2.0 * s, row.star.l, row.rect.t + 22.0 * s);
            self.text(&name, &self.fonts.ui, &tr, if exists { TEXT } else { 0x6f7479 });
            let fr = RectF::new(row.rect.l + 10.0 * s, row.rect.t + 21.0 * s, row.star.l, row.rect.b - 2.0 * s);
            let sub = if exists { folder } else { format!("missing \u{2014} {folder}") };
            self.text(&sub, &self.fonts.small, &fr, TEXT_DIM);
            let star_hover = row.star.contains_pt(mx, my);
            let star = if row.pinned { "\u{2605}" } else { "\u{2606}" };
            let color = if row.pinned { 0xffc107 } else if star_hover { TEXT } else { 0x6f7479 };
            self.text(star, &self.fonts.ui_center, &row.star, color);
        }
    }

    fn paint_settings_tab(&self, c: &RectF) {
        let s = self.s();
        let (mx, my) = (self.mouse.0 as f32, self.mouse.1 as f32);
        let (rows, end) = self.setting_rows();
        for (_, label, value, r) in &rows {
            let hover = self.over_panel(mx, my) && r.contains_pt(mx, my);
            self.fill(r, if hover { 0x33373d } else { PANEL_BG }, 1.0);
            self.text(label, &self.fonts.ui, &RectF::new(r.l + 10.0 * s, r.t, r.r, r.b), TEXT);
            let chip = RectF::new(r.r - 110.0 * s, r.t + 6.0 * s, r.r - 6.0 * s, r.b - 6.0 * s);
            self.fill(&chip, if hover { ACCENT } else { BTN_BG }, 1.0);
            self.text(value, &self.fonts.ui_center, &chip, TEXT);
        }
        let y = c.t + end - self.scroll() - 70.0 * s;
        let about = format!("Red-Beam {}\nGPU: {}", crate::update::build_label(), self.gfx.adapter_name);
        self.text(&about, &self.fonts.perf, &RectF::new(c.l + 16.0 * s, y, c.r - 8.0 * s, y + 44.0 * s), TEXT_DIM);
        self.text(
            "Click a setting to change it. Settings are saved automatically.",
            &self.fonts.perf,
            &RectF::new(c.l + 16.0 * s, y + 44.0 * s, c.r - 8.0 * s, y + 66.0 * s),
            TEXT_DIM,
        );
    }
}

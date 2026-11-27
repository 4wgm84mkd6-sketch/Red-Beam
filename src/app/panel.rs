//! Left side panel (like Bluebeam's): Pages (thumbnails), Files (pinned and
//! recent) and Settings. Drawn over the left edge of the canvas; the view's
//! fit and the takeoff toolbar leave room for it.

use std::path::PathBuf;

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use super::{d2r, App, Cached, FitMode, ACCENT, BTN_BG, TEXT, TEXT_DIM, WM_APP_OPEN};
use crate::settings::{same_path, PanelTab};
use crate::takeoff::Units;
use crate::view::RectF;
use crate::worker::{Engine, Job, JobKind};

const PANEL_W: f32 = 300.0;
const TAB_H: f32 = 38.0;
/// Thumbnails render at this long-edge size (crisp up to ~150% UI scale).
const THUMB_PX: f32 = 360.0;
const PANEL_BG: u32 = 0x24272b;

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

    fn tab_rects(&self) -> [(PanelTab, RectF, &'static str); 3] {
        let s = self.s();
        let w = self.left_inset();
        let tw = (w - 16.0 * s) / 3.0;
        let r = |i: f32| RectF::new(8.0 * s + tw * i, 6.0 * s, 8.0 * s + tw * (i + 1.0) - 2.0 * s, TAB_H * s);
        [(PanelTab::Pages, r(0.0), "Pages"), (PanelTab::Files, r(1.0), "Files"), (PanelTab::Settings, r(2.0), "Settings")]
    }

    fn content_rect(&self) -> RectF {
        let (_, ch) = self.canvas();
        RectF::new(0.0, (TAB_H + 8.0) * self.s(), self.left_inset(), ch)
    }

    fn tab_index(tab: PanelTab) -> usize {
        match tab {
            PanelTab::Pages => 0,
            PanelTab::Files => 1,
            PanelTab::Settings => 2,
        }
    }

    fn scroll(&self) -> f32 {
        self.panel_scroll[Self::tab_index(self.settings.panel_tab)]
    }

    /// Thumbnail slots for every sheet of the open set (screen coords,
    /// scroll applied), plus the total content height.
    pub(super) fn thumb_rects(&self) -> (Vec<(u32, RectF)>, f32) {
        let Some(doc) = &self.doc else { return (Vec::new(), 0.0) };
        let s = self.s();
        let c = self.content_rect();
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
            PanelTab::Pages => self.thumb_rects().1,
            PanelTab::Files => self.file_rows().2,
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
        let c = self.content_rect();
        if let Some((_, r)) = rects.iter().find(|(p, _)| *p == cur) {
            let i = Self::tab_index(PanelTab::Pages);
            let pad = 30.0 * self.s();
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
                let hit = self.thumb_rects().0.into_iter().find(|(_, r)| {
                    let s = self.s();
                    RectF::new(r.l - 8.0 * s, r.t - 4.0 * s, r.r + 8.0 * s, r.b + 24.0 * s).contains_pt(x, y)
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
        if !self.settings.panel_open || self.settings.panel_tab != PanelTab::Pages {
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
            let text = if n_items > 0 { format!("{}   \u{00B7} {n_items} takeoff", p + 1) } else { format!("{}", p + 1) };
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

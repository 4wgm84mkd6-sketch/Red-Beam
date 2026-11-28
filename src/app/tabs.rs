//! Document tabs: several PDFs open at once, each with its own sheet,
//! view, takeoffs/markups, undo, compare and search state. The active tab's
//! state lives in the App's own fields; the others are parked here.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, SetWindowTextW};

use super::find::FindState;
use super::tools::{Tool, Undo};
use super::{App, Compare, Doc, FitMode, View, ACCENT, BTN_BG, TEXT, TEXT_DIM, WM_APP_OPEN};
use crate::settings::same_path;
use crate::takeoff::{Takeoff, DEFAULT_LAYER};
use crate::view::RectF;

/// A parked (inactive) tab.
pub struct TabState {
    pub doc: Option<Doc>,
    pub cmp: Option<Compare>,
    pub view: View,
    pub fit: FitMode,
    pub takeoff: Takeoff,
    pub takeoff_path: Option<PathBuf>,
    pub undo: Vec<Undo>,
    pub group: String,
    pub layer: String,
    pub find: Option<FindState>,
}

pub enum TabHit {
    Tab(usize),
    Close(usize),
    New,
}

const MAX_TABS: usize = 12;

impl App {
    /// Height of the tab strip at the top of the canvas.
    pub(super) fn tabbar_h(&self) -> f32 {
        if self.doc.is_some() || self.opening.is_some() || self.tabs.len() > 1 { (30.0 * self.s()).round() } else { 0.0 }
    }

    /// Take the active tab's state out of the App.
    fn park(&mut self) -> TabState {
        self.overlay = None;
        self.posted.clear();
        self.anim = None;
        self.edit = None;
        self.mdraft = None;
        self.pending_pts.clear();
        self.selected = None;
        self.set_sel_markup(None);
        self.tool = Tool::Select;
        TabState {
            doc: self.doc.take(),
            cmp: self.cmp.take(),
            view: self.view,
            fit: self.fit,
            takeoff: std::mem::replace(&mut self.takeoff, Takeoff::new()),
            takeoff_path: self.takeoff_path.take(),
            undo: std::mem::take(&mut self.undo),
            group: std::mem::replace(&mut self.group, "General".into()),
            layer: std::mem::replace(&mut self.layer, DEFAULT_LAYER.into()),
            find: self.find.take(),
        }
    }

    fn unpark(&mut self, t: TabState) {
        self.doc = t.doc;
        self.cmp = t.cmp;
        self.view = t.view;
        self.fit = t.fit;
        self.takeoff = t.takeoff;
        self.takeoff_path = t.takeoff_path;
        self.undo = t.undo;
        self.group = t.group;
        self.layer = t.layer;
        self.find = t.find;
    }

    fn busy_opening(&self) -> bool {
        self.opening.is_some() || self.cmp.as_ref().is_some_and(|c| c.opening.is_some())
    }

    fn after_tab_change(&mut self) {
        let title = match &self.doc {
            Some(d) => format!("{} \u{2014} Red-Beam", d.name),
            None => "Red-Beam".into(),
        };
        unsafe {
            let _ = SetWindowTextW(self.hwnd, &windows::core::HSTRING::from(title));
        }
        self.sync_docs();
        if self.fit != FitMode::None {
            self.apply_fit(self.fit);
        }
        self.ensure_thumb_visible();
        self.update_jobs(true);
        self.save_session();
        self.invalidate();
    }

    pub(super) fn switch_tab(&mut self, i: usize) {
        if i == self.active_tab || i >= self.tabs.len() {
            return;
        }
        if self.busy_opening() {
            self.status = Some("Wait for the file to finish opening".into());
            return;
        }
        let cur = self.park();
        self.tabs[self.active_tab] = Some(cur);
        let next = self.tabs[i].take().expect("parked tab");
        self.unpark(next);
        self.active_tab = i;
        self.after_tab_change();
    }

    /// Before opening `path`: switch to its tab if it's already open,
    /// otherwise make room for it (a new tab unless this one is empty).
    /// Returns false if it was already open (nothing more to do).
    pub(super) fn prepare_open(&mut self, path: &Path) -> bool {
        if self.doc.as_ref().is_some_and(|d| same_path(&d.path, path)) {
            return false;
        }
        if let Some(i) = self.tabs.iter().position(|t| t.as_ref().and_then(|t| t.doc.as_ref()).is_some_and(|d| same_path(&d.path, path))) {
            self.switch_tab(i);
            return false;
        }
        if self.doc.is_none() && self.opening.is_none() {
            return true;
        }
        if self.busy_opening() || self.tabs.len() >= MAX_TABS {
            // Replace this tab's file instead.
            return true;
        }
        let cur = self.park();
        self.tabs[self.active_tab] = Some(cur);
        self.tabs.push(None);
        self.active_tab = self.tabs.len() - 1;
        self.fit = FitMode::Page;
        true
    }

    pub(super) fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() || (i == self.active_tab && self.busy_opening()) {
            return;
        }
        if i != self.active_tab {
            if let Some(t) = self.tabs.remove(i) {
                let ids: Vec<u64> = t.doc.iter().map(|d| d.id).chain(t.cmp.iter().filter_map(|c| c.doc.as_ref().map(|d| d.id))).collect();
                for id in ids {
                    self.drop_doc_images(Some(id));
                }
            }
            if i < self.active_tab {
                self.active_tab -= 1;
            }
            self.sync_docs();
            self.save_session();
            self.invalidate();
            return;
        }
        // Closing the active tab.
        let gone = self.park();
        for id in gone.doc.iter().map(|d| d.id).chain(gone.cmp.iter().filter_map(|c| c.doc.as_ref().map(|d| d.id))) {
            self.drop_doc_images(Some(id));
        }
        if self.tabs.len() == 1 {
            self.tabs[0] = None;
            self.active_tab = 0;
        } else {
            self.tabs.remove(i);
            let next = i.min(self.tabs.len() - 1);
            let t = self.tabs[next].take().expect("parked tab");
            self.unpark(t);
            self.active_tab = next;
        }
        self.after_tab_change();
    }

    /// Ctrl+Tab / Ctrl+Shift+Tab.
    pub(super) fn cycle_tab(&mut self, step: i64) {
        let n = self.tabs.len() as i64;
        if n > 1 {
            self.switch_tab((self.active_tab as i64 + step).rem_euclid(n) as usize);
        }
    }

    fn tab_name(&self, i: usize) -> String {
        let name = if i == self.active_tab {
            self.doc.as_ref().map(|d| d.name.clone()).or_else(|| {
                self.opening.as_ref().map(|(_, p)| format!("Opening {}\u{2026}", p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()))
            })
        } else {
            self.tabs[i].as_ref().and_then(|t| t.doc.as_ref()).map(|d| d.name.clone())
        };
        name.unwrap_or_else(|| "New".into()).trim_end_matches(".pdf").trim_end_matches(".PDF").to_string()
    }

    fn tab_layout(&self) -> (Vec<(usize, RectF, RectF)>, RectF) {
        let s = self.s();
        let h = self.tabbar_h();
        let (cw, _) = self.canvas();
        let mut x = self.left_inset() + 6.0 * s;
        let n = self.tabs.len();
        let avail = (cw - x - 40.0 * s).max(100.0 * s);
        let w = (avail / n.max(1) as f32).clamp(90.0 * s, 240.0 * s);
        let mut out = Vec::new();
        for i in 0..n {
            let r = RectF::new(x, 3.0 * s, x + w - 3.0 * s, h);
            let close = RectF::new(r.r - 22.0 * s, r.t + 4.0 * s, r.r - 4.0 * s, r.b - 4.0 * s);
            out.push((i, r, close));
            x += w;
        }
        let plus = RectF::new(x, 3.0 * s, x + 30.0 * s, h);
        (out, plus)
    }

    pub(super) fn tab_hit(&self, x: f32, y: f32) -> Option<TabHit> {
        if y >= self.tabbar_h() || x < self.left_inset() {
            return None;
        }
        let (tabs, plus) = self.tab_layout();
        if plus.contains_pt(x, y) {
            return Some(TabHit::New);
        }
        tabs.into_iter().find(|(_, r, _)| r.contains_pt(x, y)).map(|(i, _, c)| if c.contains_pt(x, y) { TabHit::Close(i) } else { TabHit::Tab(i) })
    }

    pub(super) fn tab_click(&mut self, hit: TabHit) {
        match hit {
            TabHit::Tab(i) => self.switch_tab(i),
            TabHit::Close(i) => self.close_tab(i),
            TabHit::New => unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_APP_OPEN, WPARAM(0), LPARAM(0));
            },
        }
    }

    pub(super) fn paint_tabs(&self) {
        let h = self.tabbar_h();
        if h <= 0.0 {
            return;
        }
        let s = self.s();
        let (cw, _) = self.canvas();
        let l = self.left_inset();
        self.fill(&RectF::new(l, 0.0, cw, h), 0x1b1d20, 1.0);
        let (mx, my) = (self.mouse.0 as f32, self.mouse.1 as f32);
        let (tabs, plus) = self.tab_layout();
        for (i, r, close) in tabs {
            let active = i == self.active_tab;
            let hover = r.contains_pt(mx, my);
            self.fill(&r, if active { 0x2e3136 } else if hover { 0x26292d } else { 0x1f2124 }, 1.0);
            if active {
                self.fill(&RectF::new(r.l, r.t, r.r, r.t + 2.0 * s), ACCENT, 1.0);
            }
            let tr = RectF::new(r.l + 10.0 * s, r.t, close.l - 2.0 * s, r.b);
            self.text(&self.tab_name(i), &self.fonts.ui, &tr, if active { TEXT } else { TEXT_DIM });
            let ch = close.contains_pt(mx, my);
            if ch {
                self.fill(&close, ACCENT, 1.0);
            }
            if active || hover {
                self.text("\u{00D7}", &self.fonts.ui_center, &close, if ch { 0xffffff } else { TEXT_DIM });
            }
        }
        let ph = plus.contains_pt(mx, my);
        self.fill(&plus, if ph { ACCENT } else { BTN_BG }, if ph { 1.0 } else { 0.0 });
        self.text("+", &self.fonts.ui_center, &plus, TEXT);
    }
}

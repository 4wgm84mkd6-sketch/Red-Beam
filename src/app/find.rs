//! Sheet names, text search (Ctrl+F, F3), go-to-sheet (Ctrl+G) and
//! clickable references to other sheets.

use std::collections::HashMap;

use super::tools::{Prompt, PromptFor};
use super::{rgb, App, FitMode, View, ACCENT, BTN_BG, TEXT, TEXT_DIM};
use crate::sheets::{self, Hit};
use crate::view::RectF;
use crate::worker::{Engine, Job, JobKind};

pub struct FindState {
    pub query: String,
    pub hits: Vec<Hit>,
    pub ix: Option<usize>,
}

impl App {
    // ------------------------------------------------------------- names

    /// "A-101  Floor Plan" for sheet `p` of the open set ("" if unknown).
    pub(super) fn sheet_label(&self, p: u32) -> String {
        self.doc.as_ref().and_then(|d| d.names.get(p as usize)).map(|n| n.display()).unwrap_or_default()
    }

    /// Normalized sheet number -> sheet index, for links and go-to.
    fn number_map(&self) -> HashMap<String, u32> {
        let mut m = HashMap::new();
        if let Some(d) = &self.doc {
            for (i, n) in d.names.iter().enumerate() {
                if let Some(num) = &n.number {
                    m.entry(sheets::normalize(num)).or_insert(i as u32);
                }
            }
        }
        m
    }

    /// Words of sheet `p` of the open set arrived.
    pub(super) fn on_text(&mut self, doc_id: u64, p: u32, words: Vec<sheets::Word>) {
        let Some(doc) = self.doc.as_mut().filter(|d| d.id == doc_id) else { return };
        if let Some(name) = doc.names.get_mut(p as usize) {
            if name.number.is_none() {
                name.number = sheets::guess_number(&words, doc.pages[p as usize]);
            }
        }
        self.texts.insert((doc_id, p), words);
        if let Some(q) = self.find.as_ref().map(|f| f.query.clone()) {
            self.run_find_keep(&q);
        }
        self.invalidate();
    }

    /// Text extraction wanted: the current sheet first, then the rest.
    pub(super) fn text_jobs(&self) -> Vec<Job> {
        let Some(doc) = &self.doc else { return Vec::new() };
        if self.worker.engine != Engine::Pdfium {
            return Vec::new();
        }
        let cur = doc.current;
        let mut order: Vec<u32> = vec![cur];
        order.extend((0..doc.pages.len() as u32).filter(|p| *p != cur));
        order
            .into_iter()
            .filter(|p| !self.texts.contains_key(&(doc.id, *p)) && !self.text_failed.contains(&(doc.id, *p)))
            .map(|p| Job {
                doc_id: doc.id,
                page: p,
                kind: JobKind::Text,
                prefetch: p != cur,
                src: RectF::default(),
                w: 0,
                h: 0,
                scale: 0.0,
                pair: None,
            })
            .collect()
    }

    // ------------------------------------------------------------- links

    /// References to other sheets on the current sheet: (rect, target).
    fn links(&self) -> Vec<([f32; 4], u32)> {
        let Some(doc) = &self.doc else { return Vec::new() };
        let Some(words) = self.texts.get(&(doc.id, doc.current)) else { return Vec::new() };
        if self.cmp_mode() == Some(super::CmpMode::Old) {
            return Vec::new();
        }
        sheets::find_links(words, doc.current, &self.number_map())
    }

    pub(super) fn link_at(&self, x: f32, y: f32) -> Option<u32> {
        let p = self.page_pt(x, y);
        let pad = 3.0 / self.view.zoom;
        self.links()
            .into_iter()
            .find(|(r, _)| p[0] >= r[0] - pad && p[0] <= r[2] + pad && p[1] >= r[1] - pad && p[1] <= r[3] + pad)
            .map(|(_, t)| t)
    }

    pub(super) fn paint_links(&self) {
        let s = self.s();
        let (mx, my) = (self.mouse.0 as f32, self.mouse.1 as f32);
        let hover = if self.tool == super::Tool::Select { self.link_at(mx, my) } else { None };
        for (r, target) in self.links() {
            let tl = self.to_screen([r[0], r[1]]);
            let br = self.to_screen([r[2], r[3]]);
            if br[0] - tl[0] < 4.0 {
                continue;
            }
            let rr = RectF::new(tl[0] - 2.0, tl[1] - 1.0, br[0] + 2.0, br[1] + 1.0);
            let on = hover == Some(target) && rr.contains_pt(mx, my);
            if on {
                self.fill(&rr, 0x1e88e5, 0.25);
            }
            unsafe {
                self.brush.SetColor(&rgb(0x1e88e5, if on { 1.0 } else { 0.55 }));
                self.gfx.ctx.DrawRectangle(&super::d2r(&rr), &self.brush, if on { 2.0 * s } else { 1.0 }, None);
            }
            if on {
                let label = format!("\u{2192} {}", self.sheet_label(target).trim());
                let w = label.chars().count() as f32 * 7.5 * s + 16.0 * s;
                let tip = RectF::new(rr.l, rr.b + 4.0 * s, rr.l + w, rr.b + 26.0 * s);
                self.fill(&tip, 0x1e88e5, 1.0);
                self.text(&label, &self.fonts.label, &tip, 0xffffff);
            }
        }
    }

    // ------------------------------------------------------------ search

    pub(super) fn ask_find(&mut self) {
        let text = self.find.as_ref().map(|f| f.query.clone()).unwrap_or_default();
        self.prompt = Some(Prompt {
            title: "Find text in this set".into(),
            hint: "A word or phrase, e.g. \u{201C}verify in field\u{201D}, a room name or a sheet number".into(),
            text,
            purpose: PromptFor::Find,
        });
    }

    pub(super) fn ask_goto(&mut self) {
        self.prompt = Some(Prompt {
            title: "Go to sheet".into(),
            hint: "Sheet number (A-201), part of a title (roof plan) or a position (12)".into(),
            text: String::new(),
            purpose: PromptFor::GoTo,
        });
    }

    fn run_find_keep(&mut self, query: &str) {
        let Some(doc) = &self.doc else { return };
        let mut hits = Vec::new();
        for p in 0..doc.pages.len() as u32 {
            if let Some(w) = self.texts.get(&(doc.id, p)) {
                hits.extend(sheets::search_page(p, w, query));
            }
        }
        let ix = self.find.as_ref().and_then(|f| f.ix).filter(|i| *i < hits.len());
        self.find = Some(FindState { query: query.to_string(), hits, ix });
    }

    pub(super) fn run_find(&mut self, query: String) {
        let q = query.trim().to_string();
        if q.is_empty() {
            self.find = None;
            return;
        }
        self.find = None;
        self.run_find_keep(&q);
        self.settings.panel_open = true;
        self.settings.panel_tab = crate::settings::PanelTab::Find;
        self.panel_scroll[4] = 0.0;
        if self.find.as_ref().is_some_and(|f| !f.hits.is_empty()) {
            self.go_to_hit(1);
        } else {
            let pending = self.doc.as_ref().map(|d| d.pages.len()).unwrap_or(0) - self.indexed();
            self.status = Some(if pending > 0 {
                format!("No matches yet \u{2014} still reading {pending} sheet(s)")
            } else {
                format!("No matches for \u{201C}{q}\u{201D}")
            });
        }
    }

    fn indexed(&self) -> usize {
        let Some(doc) = &self.doc else { return 0 };
        (0..doc.pages.len() as u32).filter(|p| self.texts.contains_key(&(doc.id, *p))).count()
    }

    /// Step through search hits (F3 / Shift+F3), wrapping around.
    pub(super) fn go_to_hit(&mut self, step: i64) {
        let Some(f) = self.find.as_mut() else { return };
        let n = f.hits.len();
        if n == 0 {
            return;
        }
        let ix = match f.ix {
            Some(i) => (i as i64 + step).rem_euclid(n as i64) as usize,
            None if step < 0 => n - 1,
            None => 0,
        };
        f.ix = Some(ix);
        let hit = f.hits[ix].clone();
        self.go_to_page(hit.page as i64);
        let r = hit.rects.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |a, r| {
            [a[0].min(r[0]), a[1].min(r[1]), a[2].max(r[2]), a[3].max(r[3])]
        });
        self.zoom_to(RectF::new(r[0], r[1], r[2], r[3]), 0.25);
    }

    /// Show page rectangle `r` centred, filling about `frac` of the view.
    pub(super) fn zoom_to(&mut self, r: RectF, frac: f32) {
        let (cw, ch) = self.canvas();
        let (left, top) = (self.left_inset(), self.top_inset());
        let (aw, ah) = (cw - left, ch - top);
        let (mn, mx) = self.zoom_limits();
        let zoom = (aw * frac / r.w().max(1.0)).min(ah * frac / r.h().max(1.0)).clamp(mn.max(self.fit_zoom()), (6.0 * self.s()).min(mx));
        let (cx, cy) = ((r.l + r.r) / 2.0, (r.t + r.b) / 2.0);
        self.anim = None;
        self.fit = FitMode::None;
        self.view = View { zoom, ox: left + aw / 2.0 - cx * zoom, oy: top + ah / 2.0 - cy * zoom };
        self.view_changed();
        self.update_jobs(true);
    }

    pub(super) fn paint_find_hits(&self) {
        let (Some(f), Some(doc)) = (&self.find, &self.doc) else { return };
        if self.cmp_mode() == Some(super::CmpMode::Old) {
            return;
        }
        for (i, h) in f.hits.iter().enumerate().filter(|(_, h)| h.page == doc.current) {
            let current = f.ix == Some(i);
            for r in &h.rects {
                let tl = self.to_screen([r[0], r[1]]);
                let br = self.to_screen([r[2], r[3]]);
                let rr = RectF::new(tl[0] - 2.0, tl[1] - 2.0, br[0] + 2.0, br[1] + 2.0);
                self.fill(&rr, if current { 0xff9800 } else { 0xffeb3b }, if current { 0.55 } else { 0.4 });
                if current {
                    unsafe {
                        self.brush.SetColor(&rgb(0xe65100, 1.0));
                        self.gfx.ctx.DrawRectangle(&super::d2r(&rr), &self.brush, 2.0, None);
                    }
                }
            }
        }
    }

    /// Ctrl+G: by sheet number, title text, or position.
    pub(super) fn goto_sheet(&mut self, q: String) {
        let q = q.trim();
        let Some(doc) = &self.doc else { return };
        let target = if let Some(p) = self.number_map().get(&sheets::normalize(q)) {
            Some(*p)
        } else if let Ok(n) = q.parse::<u32>() {
            (n >= 1 && (n as usize) <= doc.pages.len()).then(|| n - 1)
        } else {
            let lq = q.to_lowercase();
            doc.names.iter().position(|n| n.display().to_lowercase().contains(&lq)).map(|i| i as u32)
        };
        match target {
            Some(p) => self.go_to_page(p as i64),
            None => self.status = Some(format!("No sheet matches \u{201C}{q}\u{201D}")),
        }
    }

    // ------------------------------------------------------- Find tab

    fn find_button_rect(&self, c: &RectF, scroll: f32) -> RectF {
        let s = self.s();
        RectF::new(c.l + 10.0 * s, c.t + 8.0 * s - scroll, c.r - 10.0 * s, c.t + 40.0 * s - scroll)
    }

    /// Rows: (hit index or None for a sheet header, sheet, rect).
    fn find_rows(&self, c: &RectF, scroll: f32) -> (Vec<(Option<usize>, u32, RectF)>, f32) {
        let s = self.s();
        let mut y = c.t + 74.0 * s - scroll;
        let mut rows = Vec::new();
        let mut last = None;
        if let Some(f) = &self.find {
            for (i, h) in f.hits.iter().enumerate() {
                if last != Some(h.page) {
                    rows.push((None, h.page, RectF::new(c.l + 8.0 * s, y, c.r - 8.0 * s, y + 22.0 * s)));
                    y += 24.0 * s;
                    last = Some(h.page);
                }
                rows.push((Some(i), h.page, RectF::new(c.l + 6.0 * s, y, c.r - 6.0 * s, y + 28.0 * s)));
                y += 30.0 * s;
            }
        }
        (rows, y + scroll - c.t + 20.0 * s)
    }

    pub(super) fn find_content_height(&self, c: &RectF, scroll: f32) -> f32 {
        self.find_rows(c, scroll).1
    }

    pub(super) fn paint_find_tab(&self, c: &RectF, scroll: f32) {
        let s = self.s();
        let (mx, my) = (self.mouse.0 as f32, self.mouse.1 as f32);
        let b = self.find_button_rect(c, scroll);
        self.fill(&b, if b.contains_pt(mx, my) { ACCENT } else { BTN_BG }, 1.0);
        let q = self.find.as_ref().map(|f| format!("\u{201C}{}\u{201D} \u{2014} change\u{2026}", f.query)).unwrap_or_else(|| "Search the set\u{2026}   (Ctrl+F)".into());
        self.text(&q, &self.fonts.ui_center, &b, TEXT);
        let total = self.doc.as_ref().map(|d| d.pages.len()).unwrap_or(0);
        let indexed = self.indexed();
        let info = match &self.find {
            Some(f) => {
                let sheets_hit: std::collections::BTreeSet<u32> = f.hits.iter().map(|h| h.page).collect();
                let pos = f.ix.map(|i| format!("{} of ", i + 1)).unwrap_or_default();
                format!("{pos}{} results on {} sheets \u{00B7} F3 next, Shift+F3 previous", f.hits.len(), sheets_hit.len())
            }
            None => "Ctrl+G jumps to a sheet by number or name".into(),
        };
        let info = if indexed < total { format!("{info}\nReading sheets: {indexed} of {total}") } else { info };
        self.text(&info, &self.fonts.perf, &RectF::new(c.l + 12.0 * s, b.b + 4.0 * s, c.r - 8.0 * s, b.b + 40.0 * s), TEXT_DIM);
        let cur = self.find.as_ref().and_then(|f| f.ix);
        for (ix, page, r) in self.find_rows(c, scroll).0 {
            if r.b < c.t || r.t > c.b {
                continue;
            }
            match ix {
                None => {
                    let label = format!("{}  {}", page + 1, self.sheet_label(page));
                    self.text(&label, &self.fonts.ui, &r, TEXT_DIM);
                }
                Some(i) => {
                    let hover = self.over_panel(mx, my) && r.contains_pt(mx, my);
                    if cur == Some(i) || hover {
                        self.fill(&r, if cur == Some(i) { 0x3d2a2a } else { 0x33373d }, 1.0);
                    }
                    let snippet = self.find.as_ref().map(|f| f.hits[i].snippet.clone()).unwrap_or_default();
                    self.text(&snippet, &self.fonts.small, &RectF::new(r.l + 12.0 * s, r.t, r.r - 4.0 * s, r.b), TEXT);
                }
            }
        }
    }

    pub(super) fn find_tab_click(&mut self, c: &RectF, scroll: f32, x: f32, y: f32) {
        if self.find_button_rect(c, scroll).contains_pt(x, y) {
            self.ask_find();
            return;
        }
        if let Some((Some(i), ..)) = self.find_rows(c, scroll).0.into_iter().find(|(i, _, r)| i.is_some() && r.contains_pt(x, y)) {
            if let Some(f) = self.find.as_mut() {
                f.ix = Some(i);
            }
            self.go_to_hit(0);
        }
    }
}

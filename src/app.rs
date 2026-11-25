//! Application state: document/view state, render-request scheduling, input
//! handling and GPU composition of the cached page bitmaps.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::{InvalidateRect, ScreenToClient, ValidateRect};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_numerics::Matrix3x2;

use crate::gfx::{self, Gfx};
use crate::view::{FitMode, RectF, View};
use crate::worker::{Job, JobKey, JobKind, Msg, Pool, WM_APP_RESULT};

pub const WM_APP_OPEN: u32 = WM_APP + 2;
const TIMER_SETTLE: usize = 1;
/// How long the view must be still before a pixel-exact detail render is requested.
const SETTLE_MS: u32 = 140;
/// Longest edge (px) of the whole-page "base" bitmap kept on the GPU per page,
/// chosen from the card's dedicated memory.
fn base_max_px(vram_mb: u64, warp: bool) -> f32 {
    match (warp, vram_mb) {
        (true, _) | (_, 0..=1023) => 2048.0,
        (_, 1024..=7999) => 4096.0,
        _ => 8192.0,
    }
}
/// Time constant of the animated zoom (seconds).
const ZOOM_TAU: f32 = 0.045;
/// Each sharp render costs about the same regardless of area (the whole page's
/// content is processed every time), so on cards with plenty of memory we
/// render well beyond the screen edges and let pans reuse it.
/// Returns (fraction of viewport added on each side, pixel cap).
fn detail_padding(vram_mb: u64) -> (f32, u64) {
    match vram_mb {
        0..=1023 => (0.2, 16_000_000),
        1024..=7999 => (0.35, 32_000_000),
        _ => (0.6, 64_000_000),
    }
}
const PREFETCH: [i64; 4] = [1, -1, 2, -2];
const MAX_DETAILS: usize = 24;
const KEEP_RADIUS: i64 = 3;

// Palette (0xRRGGBB)
const CANVAS: u32 = 0x3b3e43;
const BAR_BG: u32 = 0x1e2023;
const BTN_BG: u32 = 0x2e3136;
const ACCENT: u32 = 0xc62828;
const TEXT: u32 = 0xe8e8e8;
const TEXT_DIM: u32 = 0x9aa0a6;

const HELP: &str = "Ctrl+O  /  drag & drop\tOpen a PDF\n\
Mouse wheel\tZoom at cursor\n\
Left or middle drag\tPan\n\
Shift+wheel / Ctrl+wheel\tPan horizontally / vertically\n\
Ctrl+\u{2192} / Ctrl+\u{2190}\tNext / previous sheet\n\
PgDn / PgUp  or  N / P\tNext / previous sheet\n\
Home / End\tFirst / last sheet\n\
Type a number, Enter\tGo to sheet\n\
Ctrl+0  or  F\tFit page\n\
W\tFit width\n\
Ctrl+1\tActual size (100%)\n\
+ / \u{2212}\tZoom in / out\n\
Arrow keys\tPan\n\
F2\tPerformance readout\n\
F1  or  ?\tToggle this help";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Btn {
    Open,
    Prev,
    Next,
    ZoomOut,
    ZoomIn,
    FitPage,
    FitWidth,
    Help,
}

struct Cached {
    job: Job,
    bmp: ID2D1Bitmap1,
}

struct ZoomAnim {
    target: f32,
    ax: f32,
    ay: f32,
    last: Instant,
}

struct Doc {
    id: u64,
    path: PathBuf,
    name: String,
    pages: Vec<(f32, f32)>,
    current: u32,
}

struct Fonts {
    perf: IDWriteTextFormat,
    ui: IDWriteTextFormat,
    ui_center: IDWriteTextFormat,
    ui_right: IDWriteTextFormat,
    title: IDWriteTextFormat,
    body: IDWriteTextFormat,
}

/// What to restore after the GPU device is rebuilt.
pub struct Restore {
    path: PathBuf,
    page: u32,
    view: View,
    fit: FitMode,
}

pub struct App {
    // `worker` is declared first so it is stopped before the device is released.
    worker: Pool,
    gfx: Gfx,
    hwnd: HWND,
    brush: ID2D1SolidColorBrush,
    dwrite: IDWriteFactory,
    fonts: Fonts,
    dpi: u32,
    client: (f32, f32),

    doc: Option<Doc>,
    next_doc_id: u64,
    opening: Option<(u64, String)>,
    restore: Option<Restore>,

    bases: HashMap<u32, Cached>,
    /// Sharp renders of recently seen views (any sheet near the current one),
    /// least recently added first.
    details: Vec<Cached>,
    detail_budget: u64,
    posted_detail: Option<Job>,
    failed: HashSet<JobKey>,

    view: View,
    fit: FitMode,
    anim: Option<ZoomAnim>,
    drag: Option<(i32, i32)>,
    mouse: (i32, i32),
    hover: Option<Btn>,
    goto_buf: String,
    show_help: bool,
    show_perf: bool,
    frames: VecDeque<Instant>,
    paint_ms: f32,
    last_render: Option<(f32, u32, u32)>,
    status: Option<String>,

    pub pending_error: Option<String>,
    pub device_lost: bool,
}

fn rgb(c: u32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((c >> 16) & 0xff) as f32 / 255.0,
        g: ((c >> 8) & 0xff) as f32 / 255.0,
        b: (c & 0xff) as f32 / 255.0,
        a,
    }
}

fn d2r(r: &RectF) -> D2D_RECT_F {
    D2D_RECT_F { left: r.l, top: r.t, right: r.r, bottom: r.b }
}

fn lo(lp: LPARAM) -> i32 {
    (lp.0 & 0xffff) as i16 as i32
}
fn hi(lp: LPARAM) -> i32 {
    ((lp.0 >> 16) & 0xffff) as i16 as i32
}

fn make_fonts(dw: &IDWriteFactory, s: f32) -> Result<Fonts> {
    unsafe {
        let mk = |size: f32, weight: DWRITE_FONT_WEIGHT| -> Result<IDWriteTextFormat> {
            dw.CreateTextFormat(
                w!("Segoe UI"),
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size * s,
                w!("en-us"),
            )
        };
        let one_line = |f: &IDWriteTextFormat, align: DWRITE_TEXT_ALIGNMENT| -> Result<()> {
            f.SetTextAlignment(align)?;
            f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            let sign = dw.CreateEllipsisTrimmingSign(f)?;
            let trim = DWRITE_TRIMMING {
                granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                delimiter: 0,
                delimiterCount: 0,
            };
            f.SetTrimming(&trim, &sign)?;
            Ok(())
        };
        let ui = mk(13.0, DWRITE_FONT_WEIGHT_NORMAL)?;
        one_line(&ui, DWRITE_TEXT_ALIGNMENT_LEADING)?;
        let ui_center = mk(13.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
        one_line(&ui_center, DWRITE_TEXT_ALIGNMENT_CENTER)?;
        let ui_right = mk(12.0, DWRITE_FONT_WEIGHT_NORMAL)?;
        one_line(&ui_right, DWRITE_TEXT_ALIGNMENT_TRAILING)?;
        let title = mk(34.0, DWRITE_FONT_WEIGHT_BOLD)?;
        title.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
        title.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        let body = mk(14.0, DWRITE_FONT_WEIGHT_NORMAL)?;
        body.SetIncrementalTabStop(230.0 * s)?;
        let perf = mk(13.0, DWRITE_FONT_WEIGHT_NORMAL)?;
        perf.SetIncrementalTabStop(150.0 * s)?;
        Ok(Fonts { perf, ui, ui_center, ui_right, title, body })
    }
}

impl App {
    pub fn new(hwnd: HWND) -> Result<App> {
        let gfx = Gfx::new(hwnd)?;
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        let workers = if cores >= 8 { 3 } else { 2 };
        let worker = Pool::start(&gfx.gpu, hwnd, workers);
        let detail_budget = ((gfx.dedicated_vram_mb / 6).clamp(256, 4096)) << 20;
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        let dwrite: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        let fonts = make_fonts(&dwrite, dpi as f32 / 96.0)?;
        let brush = unsafe { gfx.ctx.CreateSolidColorBrush(&rgb(TEXT, 1.0), None)? };
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut rc);
        }
        Ok(App {
            worker,
            gfx,
            hwnd,
            brush,
            dwrite,
            fonts,
            dpi,
            client: ((rc.right - rc.left) as f32, (rc.bottom - rc.top) as f32),
            doc: None,
            next_doc_id: 1,
            opening: None,
            restore: None,
            bases: HashMap::new(),
            details: Vec::new(),
            detail_budget,
            posted_detail: None,
            failed: HashSet::new(),
            view: View::default(),
            fit: FitMode::Page,
            anim: None,
            drag: None,
            mouse: (0, 0),
            hover: None,
            goto_buf: String::new(),
            show_help: false,
            show_perf: false,
            frames: VecDeque::new(),
            paint_ms: 0.0,
            last_render: None,
            status: None,
            pending_error: None,
            device_lost: false,
        })
    }

    // ---------------------------------------------------------------- helpers

    fn s(&self) -> f32 {
        self.dpi as f32 / 96.0
    }
    fn bar_h(&self) -> f32 {
        (36.0 * self.s()).round()
    }
    fn canvas(&self) -> (f32, f32) {
        (self.client.0, (self.client.1 - self.bar_h()).max(1.0))
    }
    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }
    fn page_size(&self) -> Option<(f32, f32)> {
        self.doc.as_ref().map(|d| d.pages[d.current as usize])
    }
    fn margin(&self) -> f32 {
        16.0 * self.s()
    }
    fn fit_zoom(&self) -> f32 {
        let (cw, ch) = self.canvas();
        match self.page_size() {
            Some((pw, ph)) => View::fit_page(cw, ch, pw, ph, self.margin()).zoom,
            None => 1.0,
        }
    }
    fn zoom_limits(&self) -> (f32, f32) {
        ((self.fit_zoom() * 0.25).max(0.002), 64.0 * self.s())
    }
    fn zoom_percent(&self) -> f32 {
        self.view.zoom / self.s() * 100.0
    }

    /// Call after any change to the view: repaint now, detail render once settled.
    fn view_changed(&mut self) {
        if let Some((pw, ph)) = self.page_size() {
            let (cw, ch) = self.canvas();
            self.view.clamp_pan(cw, ch, pw, ph, 48.0 * self.s());
        }
        self.invalidate();
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_SETTLE, SETTLE_MS, None);
        }
    }

    fn apply_fit(&mut self, mode: FitMode) {
        self.fit = mode;
        self.anim = None;
        let Some((pw, ph)) = self.page_size() else { return };
        let (cw, ch) = self.canvas();
        match mode {
            FitMode::Page => self.view = View::fit_page(cw, ch, pw, ph, self.margin()),
            FitMode::Width => self.view = View::fit_width(cw, ch, pw, ph, self.margin()),
            FitMode::None => {}
        }
        self.view_changed();
    }

    fn zoom_center(&mut self, factor: f32) {
        let (cw, ch) = self.canvas();
        self.zoom_at(cw * 0.5, ch * 0.5, factor);
    }

    /// Start (or extend) an animated zoom about a screen point. Repeated wheel
    /// notches accumulate into the target so the motion stays continuous.
    fn zoom_at(&mut self, x: f32, y: f32, factor: f32) {
        if self.doc.is_none() {
            return;
        }
        let (mn, mx) = self.zoom_limits();
        let from = self.anim.as_ref().map(|a| a.target).unwrap_or(self.view.zoom);
        let target = (from * factor).clamp(mn, mx);
        let last = self.anim.as_ref().map(|a| a.last).unwrap_or_else(|| Instant::now() - Duration::from_millis(8));
        self.anim = Some(ZoomAnim { target, ax: x, ay: y, last });
        self.fit = FitMode::None;
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_SETTLE);
        }
        self.invalidate();
    }

    /// Advance the zoom animation by one frame (frames are paced by vsync).
    fn step_anim(&mut self) {
        let Some(a) = self.anim.as_mut() else { return };
        let now = Instant::now();
        let dt = (now - a.last).as_secs_f32().clamp(0.001, 0.05);
        a.last = now;
        let (ax, ay, target) = (a.ax, a.ay, a.target);
        let (lz, lt) = (self.view.zoom.ln(), target.ln());
        let done = (lt - lz).abs() < 0.002;
        let nz = if done { target } else { (lz + (lt - lz) * (1.0 - (-dt / ZOOM_TAU).exp())).exp() };
        self.view.zoom_at(ax, ay, nz / self.view.zoom, 1e-6, 1e6);
        if done {
            self.anim = None;
            self.view_changed();
        } else {
            if let Some((pw, ph)) = self.page_size() {
                let (cw, ch) = self.canvas();
                self.view.clamp_pan(cw, ch, pw, ph, 48.0 * self.s());
            }
            self.invalidate();
        }
    }

    fn pan_by(&mut self, dx: f32, dy: f32) {
        if self.doc.is_none() {
            return;
        }
        self.view.ox += dx;
        self.view.oy += dy;
        if self.fit == FitMode::Page {
            self.fit = FitMode::None;
        }
        self.view_changed();
    }

    // ------------------------------------------------------------- documents

    pub fn open(&mut self, path: PathBuf) {
        let id = self.next_doc_id;
        self.next_doc_id += 1;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.opening = Some((id, name));
        self.status = None;
        self.worker.open(path, id);
        self.invalidate();
    }

    pub fn restore_info(&self) -> Option<Restore> {
        self.doc.as_ref().map(|d| Restore {
            path: d.path.clone(),
            page: d.current,
            view: self.view,
            fit: self.fit,
        })
    }

    pub fn open_restore(&mut self, r: Restore) {
        let path = r.path.clone();
        self.restore = Some(r);
        self.open(path);
    }

    fn go_to_page(&mut self, page: i64) {
        let Some(doc) = self.doc.as_mut() else { return };
        let p = page.clamp(0, doc.pages.len() as i64 - 1) as u32;
        if p == doc.current {
            return;
        }
        let old = doc.pages[doc.current as usize];
        let new = doc.pages[p as usize];
        doc.current = p;
        self.posted_detail = None;
        self.anim = None;
        let same_size = (old.0 - new.0).abs() < 0.5 && (old.1 - new.1).abs() < 0.5;
        if !same_size || self.fit != FitMode::None {
            // Same-size sheets keep the zoomed location, so flipping between
            // sheets compares the same spot on the drawing.
            let mode = if same_size { self.fit } else { FitMode::Page };
            self.apply_fit(mode);
        }
        self.update_jobs(true);
        self.invalidate();
    }

    fn base_job(&self, doc: &Doc, page: u32) -> Job {
        let (pw, ph) = doc.pages[page as usize];
        let cap = base_max_px(self.gfx.dedicated_vram_mb, self.gfx.gpu.warp);
        let max_px = cap.min(self.gfx.max_bitmap as f32 - 2.0);
        let scale = (max_px / pw.max(ph)).min(4.0);
        let w = (pw * scale).ceil().max(1.0) as u32;
        let h = (ph * scale).ceil().max(1.0) as u32;
        Job {
            doc_id: doc.id,
            page,
            kind: JobKind::Base,
            prefetch: page != doc.current,
            src: RectF::new(0.0, 0.0, w as f32 / scale, h as f32 / scale),
            w,
            h,
            scale,
        }
    }

    /// The pixel-exact render that `page` needs for the current view, or
    /// `None` if a cached sharp view already covers it at this zoom.
    fn desired_detail(&self, page: u32) -> Option<Job> {
        let doc = self.doc.as_ref()?;
        let (pw, ph) = doc.pages[page as usize];
        let (cw, ch) = self.canvas();
        let page_scr = self.view.page_to_screen(&RectF::new(0.0, 0.0, pw, ph));
        let visible = page_scr.intersect(&RectF::new(0.0, 0.0, cw, ch));
        if visible.is_empty() {
            return None;
        }
        let visible_page = self.view.screen_to_page(&visible);
        let eps = 0.5 / self.view.zoom;
        let covers = |j: &Job| {
            j.page == page
                && j.doc_id == doc.id
                && ((j.scale - self.view.zoom) / self.view.zoom).abs() < 1e-4
                && j.src.contains_rect(&visible_page, eps)
        };
        if self.details.iter().any(|d| covers(&d.job)) {
            return None;
        }
        if page == doc.current {
            if let Some(d) = &self.posted_detail {
                if covers(d) && !self.failed.contains(&d.key()) {
                    return Some(d.clone());
                }
            }
        }
        let max = self.gfx.max_bitmap as f32;
        let (pad_max, max_pixels) = detail_padding(self.gfx.dedicated_vram_mb);
        for pad in [pad_max, pad_max * 0.5, 0.0] {
            let want = RectF::new(-cw * pad, -ch * pad, cw * (1.0 + pad), ch * (1.0 + pad))
                .intersect(&page_scr)
                .snap_out();
            if want.is_empty() {
                return None;
            }
            let (w, h) = (want.w(), want.h());
            if (w <= max && h <= max && (w as u64) * (h as u64) <= max_pixels) || pad == 0.0 {
                return Some(Job {
                    doc_id: doc.id,
                    page,
                    kind: JobKind::Detail,
                    prefetch: page != doc.current,
                    src: self.view.screen_to_page(&want),
                    w: (w as u32).clamp(1, max as u32),
                    h: (h as u32).clamp(1, max as u32),
                    scale: self.view.zoom,
                });
            }
        }
        None
    }

    /// Re-post the workers' priority-ordered wish list.
    fn update_jobs(&mut self, with_detail: bool) {
        let Some(doc) = self.doc.as_ref() else {
            self.worker.set_jobs(Vec::new());
            return;
        };
        let cur = doc.current as i64;
        let n = doc.pages.len() as i64;
        self.bases.retain(|&p, _| (p as i64 - cur).abs() <= KEEP_RADIUS);
        self.details.retain(|d| (d.job.page as i64 - cur).abs() <= KEEP_RADIUS);
        if with_detail && self.anim.is_none() {
            self.posted_detail = self.desired_detail(cur as u32);
        }
        let doc = self.doc.as_ref().unwrap();
        let mut jobs = Vec::new();
        // 1. Sharp render of what is on screen.
        if let Some(d) = &self.posted_detail {
            if d.page as i64 == cur && d.doc_id == doc.id {
                jobs.push(d.clone());
            }
        }
        // 2. Whole-page image of this sheet.
        if !self.bases.contains_key(&(cur as u32)) {
            jobs.push(self.base_job(doc, cur as u32));
        }
        // 3. Once settled: same view on neighbouring same-size sheets, so
        //    flipping sheets is sharp immediately.
        if self.posted_detail.is_none() && self.anim.is_none() && self.desired_detail(cur as u32).is_none() {
            let size = doc.pages[cur as usize];
            for off in [1i64, -1] {
                let p = cur + off;
                if p >= 0 && p < n {
                    let ps = doc.pages[p as usize];
                    if (ps.0 - size.0).abs() < 0.5 && (ps.1 - size.1).abs() < 0.5 {
                        if let Some(j) = self.desired_detail(p as u32) {
                            jobs.push(j);
                        }
                    }
                }
            }
        }
        // 4. Whole-page images of nearby sheets.
        for off in PREFETCH {
            let p = cur + off;
            if p >= 0 && p < n && !self.bases.contains_key(&(p as u32)) {
                jobs.push(self.base_job(doc, p as u32));
            }
        }
        jobs.retain(|j| !self.failed.contains(&j.key()));
        self.worker.set_jobs(jobs);
    }

    fn busy(&self) -> bool {
        if self.opening.is_some() {
            return true;
        }
        let Some(doc) = &self.doc else { return false };
        self.posted_detail.is_some() || !self.bases.contains_key(&doc.current)
    }

    fn insert_detail(&mut self, c: Cached) {
        self.details.push(c);
        let mut total: u64 = self.details.iter().map(|d| d.job.bytes()).sum();
        while (total > self.detail_budget || self.details.len() > MAX_DETAILS) && self.details.len() > 1 {
            total -= self.details.remove(0).job.bytes();
        }
    }

    fn on_results(&mut self) {
        while let Ok(msg) = self.worker.rx.try_recv() {
            match msg {
                Msg::Opened { doc_id, path, pages } => {
                    if self.opening.as_ref().map(|o| o.0) != Some(doc_id) {
                        continue;
                    }
                    self.opening = None;
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let n = pages.len() as u32;
                    self.bases.clear();
                    self.details.clear();
                    self.anim = None;
                    self.posted_detail = None;
                    self.failed.clear();
                    self.goto_buf.clear();
                    self.last_render = None;
                    let restore = self.restore.take().filter(|r| r.path == path);
                    self.doc = Some(Doc {
                        id: doc_id,
                        path,
                        name: name.clone(),
                        pages,
                        current: restore.as_ref().map(|r| r.page.min(n.saturating_sub(1))).unwrap_or(0),
                    });
                    unsafe {
                        let _ = SetWindowTextW(self.hwnd, &HSTRING::from(format!("{name} \u{2014} Red-Beam")));
                    }
                    match restore {
                        Some(r) => {
                            self.view = r.view;
                            self.fit = r.fit;
                            self.view_changed();
                        }
                        None => self.apply_fit(FitMode::Page),
                    }
                    if n == 0 {
                        self.status = Some("Document has no pages".into());
                    }
                    self.update_jobs(true);
                }
                Msg::OpenFailed { doc_id, path, error } => {
                    if self.opening.as_ref().map(|o| o.0) == Some(doc_id) {
                        self.opening = None;
                        self.restore = None;
                        self.pending_error = Some(format!(
                            "Could not open {}\n\n{}\n\nPassword-protected PDFs are not supported yet.",
                            path.display(),
                            error
                        ));
                    }
                }
                Msg::Rendered { job, handle, texture, ms } => {
                    self.worker.ack(&job.key());
                    let Some(doc) = &self.doc else { continue };
                    if job.doc_id != doc.id {
                        continue;
                    }
                    let bmp = match self.gfx.open_shared(handle) {
                        Ok(b) => b,
                        Err(e) => {
                            if gfx::is_device_lost(&e) {
                                self.device_lost = true;
                            }
                            self.status = Some(format!("Could not use rendered page: {}", e.message()));
                            continue;
                        }
                    };
                    drop(texture);
                    if job.kind == JobKind::Detail && !job.prefetch {
                        self.last_render = Some((ms, job.w, job.h));
                    }
                    if self.posted_detail.as_ref().map(|d| d.key()) == Some(job.key()) {
                        self.posted_detail = None;
                    }
                    match job.kind {
                        JobKind::Base => {
                            self.bases.insert(job.page, Cached { job, bmp });
                        }
                        JobKind::Detail => self.insert_detail(Cached { job, bmp }),
                    }
                    // A finished sharp view may unlock neighbour prefetch.
                    self.update_jobs(false);
                    self.invalidate();
                }
                Msg::RenderFailed { job, error } => {
                    if self.posted_detail.as_ref().map(|d| d.key()) == Some(job.key()) {
                        self.posted_detail = None;
                    }
                    self.failed.insert(job.key());
                    self.status = Some(format!("Render failed: {error}"));
                    self.invalidate();
                }
            }
        }
    }

    // -------------------------------------------------------------- painting

    fn fill(&self, r: &RectF, c: u32, a: f32) {
        unsafe {
            self.brush.SetColor(&rgb(c, a));
            self.gfx.ctx.FillRectangle(&d2r(r), &self.brush);
        }
    }

    fn text(&self, s: &str, fmt: &IDWriteTextFormat, r: &RectF, c: u32) {
        let w: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            self.brush.SetColor(&rgb(c, 1.0));
            self.gfx.ctx.DrawText(
                &w,
                fmt,
                &d2r(r),
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    fn draw_cached(&self, c: &Cached) {
        let dest = self.view.page_to_screen(&c.job.src);
        let ratio = self.view.zoom / c.job.scale;
        let (dest, mode) = if (ratio - 1.0).abs() < 1e-4 {
            let rd = dest.round();
            if (rd.l - dest.l).abs() < 0.05 && (rd.t - dest.t).abs() < 0.05 {
                // 1:1 and pixel aligned: blit exactly, no resampling blur.
                let exact = RectF::new(rd.l, rd.t, rd.l + c.job.w as f32, rd.t + c.job.h as f32);
                (exact, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR)
            } else {
                (dest, D2D1_INTERPOLATION_MODE_LINEAR)
            }
        } else if ratio < 0.75 {
            (dest, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC)
        } else {
            (dest, D2D1_INTERPOLATION_MODE_LINEAR)
        };
        unsafe {
            self.gfx.ctx.DrawBitmap(&c.bmp, Some(&d2r(&dest)), 1.0, mode, None, None);
        }
    }

    fn paint_page(&self, doc: &Doc) {
        let s = self.s();
        let (pw, ph) = doc.pages[doc.current as usize];
        let page_scr = self.view.page_to_screen(&RectF::new(0.0, 0.0, pw, ph));
        for i in 1..=4 {
            let o = i as f32 * 1.5 * s;
            let r = RectF::new(page_scr.l + o * 0.5, page_scr.t + o, page_scr.r + o, page_scr.b + o);
            self.fill(&r, 0x000000, 0.10);
        }
        self.fill(&page_scr, 0xffffff, 1.0);
        unsafe {
            self.gfx.ctx.PushAxisAlignedClip(&d2r(&page_scr), D2D1_ANTIALIAS_MODE_ALIASED);
        }
        if let Some(b) = self.bases.get(&doc.current) {
            self.draw_cached(b);
        }
        // Sharp views of this sheet: least suitable first, best match on top.
        let (cw, ch) = self.canvas();
        let screen = RectF::new(0.0, 0.0, cw, ch);
        let mut ds: Vec<&Cached> = self
            .details
            .iter()
            .filter(|d| d.job.page == doc.current && d.job.doc_id == doc.id)
            .filter(|d| !self.view.page_to_screen(&d.job.src).intersect(&screen).is_empty())
            .collect();
        let badness = |d: &&Cached| (d.job.scale / self.view.zoom).ln().abs();
        ds.sort_by(|a, b| badness(b).total_cmp(&badness(a)));
        for d in ds {
            self.draw_cached(d);
        }
        unsafe {
            self.gfx.ctx.PopAxisAlignedClip();
        }
    }

    fn paint_empty(&self) {
        let s = self.s();
        let (cw, ch) = self.canvas();
        let cy = ch * 0.38;
        self.text("Red-Beam", &self.fonts.title, &RectF::new(0.0, cy - 40.0 * s, cw, cy + 20.0 * s), TEXT);
        let sub = match &self.opening {
            Some((_, name)) => format!("Opening {name}\u{2026}"),
            None => "Open a PDF with Ctrl+O, or drag one onto this window.   F1 for shortcuts.".into(),
        };
        self.text(&sub, &self.fonts.ui_center, &RectF::new(0.0, cy + 24.0 * s, cw, cy + 50.0 * s), TEXT_DIM);
        let gpu = format!(
            "GPU: {}{}",
            self.gfx.adapter_name,
            if self.gfx.gpu.warp { "  (software fallback \u{2014} no hardware GPU found)" } else { "" }
        );
        self.text(&gpu, &self.fonts.ui_center, &RectF::new(0.0, cy + 54.0 * s, cw, cy + 78.0 * s), TEXT_DIM);
    }

    fn button_layout(&self) -> Vec<(Btn, RectF, &'static str)> {
        let s = self.s();
        let (_, h) = self.client;
        let (top, bot) = (h - self.bar_h() + 5.0 * s, h - 5.0 * s);
        let specs: [(Btn, &str, f32, f32); 8] = [
            (Btn::Open, "Open", 56.0, 12.0),
            (Btn::Prev, "\u{25C0}", 32.0, 4.0),
            (Btn::Next, "\u{25B6}", 32.0, 12.0),
            (Btn::ZoomOut, "\u{2212}", 32.0, 4.0),
            (Btn::ZoomIn, "+", 32.0, 4.0),
            (Btn::FitPage, "Fit", 44.0, 4.0),
            (Btn::FitWidth, "Width", 56.0, 12.0),
            (Btn::Help, "?", 30.0, 0.0),
        ];
        let mut x = 6.0 * s;
        specs
            .iter()
            .map(|&(b, label, w, gap)| {
                let r = RectF::new(x, top, x + w * s, bot);
                x += (w + 4.0 + gap) * s;
                (b, r, label)
            })
            .collect()
    }

    fn hit_button(&self, x: f32, y: f32) -> Option<Btn> {
        self.button_layout().into_iter().find(|(_, r, _)| r.contains_pt(x, y)).map(|(b, _, _)| b)
    }

    fn paint_bar(&self) {
        let s = self.s();
        let (w, h) = self.client;
        let bar = RectF::new(0.0, h - self.bar_h(), w, h);
        self.fill(&bar, BAR_BG, 1.0);
        self.fill(&RectF::new(0.0, bar.t, w, bar.t + 1.0), 0x000000, 0.8);
        let buttons = self.button_layout();
        for (b, r, label) in &buttons {
            let bg = if self.hover == Some(*b) { ACCENT } else { BTN_BG };
            self.fill(r, bg, 1.0);
            self.text(label, &self.fonts.ui_center, r, TEXT);
        }
        let mut x = buttons.last().map(|b| b.1.r).unwrap_or(0.0) + 14.0 * s;
        let right_w = (560.0 * s).min(w * 0.42);
        let mut seg = |txt: &str, width: f32, color: u32| {
            let r = RectF::new(x, bar.t, (x + width).min(w - right_w), bar.b);
            if r.w() > 4.0 {
                self.text(txt, &self.fonts.ui, &r, color);
            }
            x += width;
        };
        if let Some(doc) = &self.doc {
            let page_txt = if self.goto_buf.is_empty() {
                format!("Sheet {} / {}", doc.current + 1, doc.pages.len())
            } else {
                format!("Go to sheet: {}\u{258F}", self.goto_buf)
            };
            seg(&page_txt, 150.0 * s, TEXT);
            seg(&format!("{:.0}%", self.zoom_percent()), 70.0 * s, TEXT);
            let (pw, ph) = doc.pages[doc.current as usize];
            seg(&format!("{:.0}\u{2033} \u{00D7} {:.0}\u{2033}", pw / 96.0, ph / 96.0), 90.0 * s, TEXT_DIM);
            seg(&doc.name, 2000.0 * s, TEXT_DIM);
        }
        let mut right = String::new();
        if self.busy() {
            right.push_str("Rendering\u{2026}   ");
        }
        if let Some(st) = &self.status {
            right.push_str(&format!("\u{26A0} {st}   "));
        } else if let Some((ms, rw, rh)) = self.last_render {
            right.push_str(&format!("last render {ms:.0} ms ({rw}\u{00D7}{rh})   "));
        }
        right.push_str(&format!("GPU: {}", self.gfx.adapter_name));
        self.text(&right, &self.fonts.ui_right, &RectF::new(w - right_w, bar.t, w - 10.0 * s, bar.b), TEXT_DIM);
    }

    fn paint_help(&self) {
        let s = self.s();
        let (cw, ch) = self.canvas();
        let (pw, ph) = (560.0 * s, 440.0 * s);
        let panel = RectF::new(((cw - pw) * 0.5).round(), ((ch - ph) * 0.5).round(), 0.0, 0.0);
        let panel = RectF::new(panel.l, panel.t, panel.l + pw, panel.t + ph);
        self.fill(&panel, 0x141518, 0.94);
        self.fill(&RectF::new(panel.l, panel.t, panel.r, panel.t + 3.0 * s), ACCENT, 1.0);
        let inner = RectF::new(panel.l + 28.0 * s, panel.t + 22.0 * s, panel.r - 20.0 * s, panel.b - 16.0 * s);
        self.text(&format!("Keyboard & mouse\n\n{HELP}"), &self.fonts.body, &inner, TEXT);
    }

    fn paint_perf(&self) {
        let s = self.s();
        let now = Instant::now();
        let recent: Vec<&Instant> = self.frames.iter().filter(|t| now - **t < Duration::from_secs(1)).collect();
        let fps = if self.frames.back().is_some_and(|t| now - *t < Duration::from_millis(300)) && recent.len() > 1 {
            format!("{} fps", recent.len())
        } else {
            "idle (redraws only when something changes)".to_string()
        };
        let vram = match self.gfx.vram_usage_mb() {
            Some((u, b)) => format!("{u} MB used of {b} MB budget"),
            None => "n/a".into(),
        };
        let cache_mb: u64 = (self.bases.values().map(|c| c.job.bytes()).sum::<u64>()
            + self.details.iter().map(|c| c.job.bytes()).sum::<u64>())
            >> 20;
        let last = match self.last_render {
            Some((ms, w, h)) => format!("{ms:.0} ms  ({w}\u{00D7}{h} px)"),
            None => "\u{2014}".into(),
        };
        let txt = format!(
            "Frame rate\t{fps}\nFrame CPU time\t{:.1} ms\nGPU\t{}\nVideo memory\t{vram}\nRender workers\t{} ({} busy)\nCached on GPU\t{} sheets + {} sharp views ({cache_mb} MB)\nLast sharp render\t{last}",
            self.paint_ms,
            self.gfx.adapter_name,
            self.worker.size,
            self.worker.busy(),
            self.bases.len(),
            self.details.len(),
        );
        let panel = RectF::new(12.0 * s, 12.0 * s, 12.0 * s + 620.0 * s, 12.0 * s + 178.0 * s);
        self.fill(&panel, 0x141518, 0.88);
        self.fill(&RectF::new(panel.l, panel.t, panel.l + 3.0 * s, panel.b), ACCENT, 1.0);
        let inner = RectF::new(panel.l + 16.0 * s, panel.t + 10.0 * s, panel.r - 10.0 * s, panel.b - 6.0 * s);
        self.text(&txt, &self.fonts.perf, &inner, TEXT);
    }

    pub fn paint(&mut self) {
        if self.gfx.size.0 == 0 || self.gfx.size.1 == 0 {
            return;
        }
        let t0 = Instant::now();
        self.step_anim();
        let ctx = self.gfx.ctx.clone();
        let (cw, ch) = self.canvas();
        unsafe {
            ctx.BeginDraw();
            ctx.SetTransform(&Matrix3x2::identity());
            ctx.Clear(Some(&rgb(CANVAS, 1.0)));
            ctx.PushAxisAlignedClip(&d2r(&RectF::new(0.0, 0.0, cw, ch)), D2D1_ANTIALIAS_MODE_ALIASED);
            match &self.doc {
                Some(doc) => self.paint_page(doc),
                None => self.paint_empty(),
            }
            ctx.PopAxisAlignedClip();
            self.paint_bar();
            if self.show_help {
                self.paint_help();
            }
            if self.show_perf {
                self.paint_perf();
            }
            let r = ctx.EndDraw(None, None);
            self.paint_ms = t0.elapsed().as_secs_f32() * 1000.0;
            let r = r.and_then(|_| self.gfx.present());
            self.frames.push_back(Instant::now());
            while self.frames.len() > 240 {
                self.frames.pop_front();
            }
            if let Err(e) = r {
                if gfx::is_device_lost(&e) {
                    self.device_lost = true;
                } else {
                    self.status = Some(e.message().to_string());
                }
            }
        }
    }

    // ----------------------------------------------------------------- input

    fn on_key(&mut self, vk: u16) -> bool {
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        let has_doc = self.doc.is_some();
        let cur = self.doc.as_ref().map(|d| d.current as i64).unwrap_or(0);
        let last = self.doc.as_ref().map(|d| d.pages.len() as i64 - 1).unwrap_or(0);
        let (cw, ch) = self.canvas();
        let k = VIRTUAL_KEY(vk);
        let ch_ = |c: u8| vk == c as u16;
        if ctrl && ch_(b'O') {
            unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_APP_OPEN, WPARAM(0), LPARAM(0));
            }
        } else if ctrl && ch_(b'0') || (!ctrl && ch_(b'F')) {
            self.apply_fit(FitMode::Page);
        } else if ctrl && ch_(b'1') {
            let from = self.anim.as_ref().map(|a| a.target).unwrap_or(self.view.zoom);
            self.zoom_center(self.s() / from);
        } else if !ctrl && ch_(b'W') {
            self.apply_fit(FitMode::Width);
        } else if k == VK_NEXT || (ctrl && k == VK_RIGHT) || (!ctrl && ch_(b'N')) {
            self.go_to_page(cur + 1);
        } else if k == VK_PRIOR || (ctrl && k == VK_LEFT) || (!ctrl && ch_(b'P')) {
            self.go_to_page(cur - 1);
        } else if k == VK_HOME {
            self.go_to_page(0);
        } else if k == VK_END {
            self.go_to_page(last);
        } else if k == VK_ADD || k == VK_OEM_PLUS {
            self.zoom_center(1.25);
        } else if k == VK_SUBTRACT || k == VK_OEM_MINUS {
            self.zoom_center(1.0 / 1.25);
        } else if k == VK_LEFT {
            self.pan_by((cw / 8.0).round(), 0.0);
        } else if k == VK_RIGHT {
            self.pan_by(-(cw / 8.0).round(), 0.0);
        } else if k == VK_UP {
            self.pan_by(0.0, (ch / 8.0).round());
        } else if k == VK_DOWN {
            self.pan_by(0.0, -(ch / 8.0).round());
        } else if k == VK_F2 {
            self.show_perf = !self.show_perf;
            self.invalidate();
        } else if k == VK_F1 {
            self.show_help = !self.show_help;
            self.invalidate();
        } else if k == VK_RETURN && has_doc && !self.goto_buf.is_empty() {
            if let Ok(n) = self.goto_buf.parse::<i64>() {
                self.go_to_page(n - 1);
            }
            self.goto_buf.clear();
            self.invalidate();
        } else if k == VK_ESCAPE {
            self.goto_buf.clear();
            self.show_help = false;
            self.invalidate();
        } else if k == VK_BACK {
            self.goto_buf.pop();
            self.invalidate();
        } else {
            return false;
        }
        true
    }

    fn on_char(&mut self, c: u32) {
        match char::from_u32(c) {
            Some(d @ '0'..='9') if self.doc.is_some() && self.goto_buf.len() < 6 => {
                self.goto_buf.push(d);
                self.invalidate();
            }
            Some('?') => {
                self.show_help = !self.show_help;
                self.invalidate();
            }
            _ => {}
        }
    }

    fn press(&mut self, b: Btn) {
        let cur = self.doc.as_ref().map(|d| d.current as i64).unwrap_or(0);
        match b {
            Btn::Open => unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_APP_OPEN, WPARAM(0), LPARAM(0));
            },
            Btn::Prev => self.go_to_page(cur - 1),
            Btn::Next => self.go_to_page(cur + 1),
            Btn::ZoomOut => self.zoom_center(1.0 / 1.25),
            Btn::ZoomIn => self.zoom_center(1.25),
            Btn::FitPage => self.apply_fit(FitMode::Page),
            Btn::FitWidth => self.apply_fit(FitMode::Width),
            Btn::Help => {
                self.show_help = !self.show_help;
                self.invalidate();
            }
        }
    }

    fn in_bar(&self, y: f32) -> bool {
        y >= self.client.1 - self.bar_h()
    }

    fn cursor(&self) -> PCWSTR {
        let (x, y) = (self.mouse.0 as f32, self.mouse.1 as f32);
        if self.in_bar(y) {
            if self.hit_button(x, y).is_some() { IDC_HAND } else { IDC_ARROW }
        } else if self.drag.is_some() {
            IDC_SIZEALL
        } else if self.doc.is_some() {
            IDC_HAND
        } else {
            IDC_ARROW
        }
    }

    pub fn set_dpi(&mut self, dpi: u32) {
        self.dpi = dpi.max(96);
        if let Ok(f) = make_fonts(&self.dwrite, self.s()) {
            self.fonts = f;
        }
        self.invalidate();
    }

    fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.client = (w as f32, h as f32);
        if let Err(e) = self.gfx.resize(w, h) {
            if gfx::is_device_lost(&e) {
                self.device_lost = true;
            } else {
                self.status = Some(e.message().to_string());
            }
            return;
        }
        if self.fit != FitMode::None {
            self.apply_fit(self.fit);
        } else {
            self.view_changed();
        }
    }

    /// Window-message handler. `None` means "let DefWindowProc handle it".
    pub fn handle(&mut self, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<isize> {
        unsafe {
            match msg {
                WM_PAINT => {
                    let _ = ValidateRect(Some(hwnd), None);
                    self.paint();
                    Some(0)
                }
                WM_SIZE => {
                    self.resize((lp.0 & 0xffff) as u32, ((lp.0 >> 16) & 0xffff) as u32);
                    Some(0)
                }
                WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                    let delta = ((wp.0 >> 16) & 0xffff) as u16 as i16 as f32 / 120.0;
                    let keys = (wp.0 & 0xffff) as u32;
                    let mut pt = POINT { x: lo(lp), y: hi(lp) };
                    let _ = ScreenToClient(hwnd, &mut pt);
                    let step = 90.0 * self.s();
                    if msg == WM_MOUSEHWHEEL {
                        self.pan_by(-delta * step, 0.0);
                    } else if keys & 0x0004 != 0 {
                        self.pan_by(delta * step, 0.0);
                    } else if keys & 0x0008 != 0 {
                        self.pan_by(0.0, delta * step);
                    } else {
                        let (cw, ch) = self.canvas();
                        let (x, y) = if self.in_bar(pt.y as f32) {
                            (cw * 0.5, ch * 0.5)
                        } else {
                            (pt.x as f32, pt.y as f32)
                        };
                        self.zoom_at(x, y, 1.2f32.powf(delta));
                    }
                    Some(0)
                }
                WM_LBUTTONDOWN | WM_MBUTTONDOWN => {
                    let (x, y) = (lo(lp), hi(lp));
                    self.mouse = (x, y);
                    if self.in_bar(y as f32) {
                        if msg == WM_LBUTTONDOWN {
                            if let Some(b) = self.hit_button(x as f32, y as f32) {
                                self.press(b);
                            }
                        }
                    } else if self.doc.is_some() {
                        self.drag = Some((x, y));
                        SetCapture(hwnd);
                        SetCursor(LoadCursorW(None, IDC_SIZEALL).ok());
                    }
                    Some(0)
                }
                WM_MOUSEMOVE => {
                    let (x, y) = (lo(lp), hi(lp));
                    self.mouse = (x, y);
                    if let Some((lx, ly)) = self.drag {
                        self.drag = Some((x, y));
                        self.pan_by((x - lx) as f32, (y - ly) as f32);
                    } else {
                        let h = if self.in_bar(y as f32) { self.hit_button(x as f32, y as f32) } else { None };
                        if h != self.hover {
                            self.hover = h;
                            self.invalidate();
                        }
                    }
                    Some(0)
                }
                WM_LBUTTONUP | WM_MBUTTONUP => {
                    if self.drag.take().is_some() {
                        let _ = ReleaseCapture();
                    }
                    Some(0)
                }
                WM_CAPTURECHANGED => {
                    self.drag = None;
                    Some(0)
                }
                WM_KEYDOWN => self.on_key(wp.0 as u16).then_some(0),
                WM_CHAR => {
                    self.on_char(wp.0 as u32);
                    Some(0)
                }
                WM_TIMER if wp.0 == TIMER_SETTLE => {
                    let _ = KillTimer(Some(hwnd), TIMER_SETTLE);
                    if self.anim.is_some() {
                        return Some(0);
                    }
                    self.update_jobs(true);
                    self.invalidate();
                    Some(0)
                }
                WM_SETCURSOR if (lp.0 & 0xffff) as u32 == HTCLIENT => {
                    SetCursor(LoadCursorW(None, self.cursor()).ok());
                    Some(1)
                }
                WM_APP_RESULT => {
                    self.on_results();
                    Some(0)
                }
                _ => None,
            }
        }
    }
}

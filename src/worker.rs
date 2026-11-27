//! Pool of background render threads.
//!
//! Each worker owns its own D3D11 device + Direct2D context on the same GPU
//! as the UI, and its own `Windows.Data.Pdf` document instance, so several
//! regions/sheets rasterise in parallel and the UI thread never waits on a
//! render. Results are GPU textures shared with the UI device by handle.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Data::Pdf::{PdfDocument, PdfPage};
use windows::Storage::StorageFile;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGIResource, IDXGISurface};
use windows::Win32::System::Com::*;
use windows::Win32::System::WinRT::Pdf::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::gfx::{self, bitmap_props, GpuChoice};
use crate::pdfium;
use crate::snap::SnapIndex;
use crate::view::RectF;

pub const WM_APP_RESULT: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 1;

/// Moves a COM pointer across threads (D3D device-child objects are free-threaded).
pub struct Sendable<T>(pub T);
unsafe impl<T> Send for Sendable<T> {}

/// Which PDF engine rasterises pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// PDFium (Chrome/Edge's engine), embedded. Parses each page once, keeps
    /// it loaded, skips off-screen content and can abandon stale renders.
    /// Single-threaded.
    Pdfium,
    /// Windows.Data.Pdf rendering through Direct2D on the GPU.
    Windows,
}

impl Engine {
    pub fn name(self) -> &'static str {
        match self {
            Engine::Pdfium => "PDFium",
            Engine::Windows => "Windows built-in",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    /// Whole page at moderate resolution: instant feedback while moving.
    Base,
    /// A region at exact screen resolution: pixel-sharp when settled.
    Detail,
    /// Extract the sheet's line work and build a snap index.
    Geometry,
}

#[derive(Clone, Debug)]
pub struct Job {
    pub doc_id: u64,
    pub page: u32,
    pub kind: JobKind,
    /// Speculative work for neighbouring sheets.
    pub prefetch: bool,
    /// Source rectangle in page DIPs.
    pub src: RectF,
    pub w: u32,
    pub h: u32,
    /// Pixels per page DIP.
    pub scale: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JobKey(u64, u32, bool, [u32; 4], u32, u32);

impl Job {
    /// Work that is not the sharp render of the view on screen.
    pub fn is_background(&self) -> bool {
        self.kind == JobKind::Base || self.prefetch
    }
    pub fn bytes(&self) -> u64 {
        self.w as u64 * self.h as u64 * 4
    }
    pub fn key(&self) -> JobKey {
        JobKey(
            self.doc_id,
            self.page,
            self.kind == JobKind::Base,
            [self.src.l.to_bits(), self.src.t.to_bits(), self.src.r.to_bits(), self.src.b.to_bits()],
            self.w,
            self.h,
        )
    }
}

pub enum Msg {
    Opened { doc_id: u64, path: PathBuf, pages: Vec<(f32, f32)> },
    OpenFailed { doc_id: u64, path: PathBuf, error: String },
    /// `texture` keeps the shared resource alive until the UI has opened it.
    /// `parallel` = renders running at the moment this one started (incl. itself).
    Rendered { job: Job, handle: isize, texture: Sendable<ID3D11Texture2D>, ms: f32, parallel: usize },
    RenderFailed { job: Job, error: String },
    /// Abandoned because it was no longer wanted (PDFium only).
    Cancelled { job: Job, ms: f32 },
    /// PDFium could not be loaded; the pool fell back to the Windows engine.
    EngineFallback(String),
    /// Snap index for a sheet (or why there isn't one).
    Snap { job: Job, index: std::result::Result<Sendable<SnapIndex>, String>, ms: f32 },
}

#[derive(Default)]
struct State {
    /// Documents the UI wants loaded (the open set, plus a revision to
    /// compare against).
    docs: Vec<(u64, PathBuf)>,
    jobs: Vec<Job>,
    in_progress: HashSet<JobKey>,
    /// Cancel flags of running jobs, and whether each is background work.
    running: HashMap<JobKey, (Arc<AtomicBool>, bool)>,
    /// Finished but not yet acknowledged by the UI (avoids duplicate renders
    /// when the UI re-posts its wish list before a result arrives).
    delivered: HashSet<JobKey>,
    /// Background jobs (base pages + prefetch) currently running, and the cap
    /// that keeps at least one worker free for the view on screen.
    bg_running: usize,
    bg_limit: usize,
    fg_running: usize,
    quit: bool,
}

struct Shared {
    m: Mutex<State>,
    cv: Condvar,
}

pub struct Pool {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
    pub rx: Receiver<Msg>,
    pub size: usize,
    pub engine: Engine,
}

impl Pool {
    pub fn start(gpu: &GpuChoice, hwnd: HWND, size: usize, engine: Engine) -> Pool {
        // PDFium is single-threaded.
        let size = if engine == Engine::Pdfium { 1 } else { size.max(1) };
        let state = State { bg_limit: size.saturating_sub(1).max(1), ..Default::default() };
        let shared = Arc::new(Shared { m: Mutex::new(state), cv: Condvar::new() });
        let (tx, rx) = channel();
        let hwnd_raw = hwnd.0 as isize;
        let threads = (0..size)
            .map(|i| {
                let s = shared.clone();
                let tx = tx.clone();
                let gpu = gpu.clone();
                std::thread::Builder::new()
                    .name(format!("pdf-render-{i}"))
                    .spawn(move || run(i, s, gpu, tx, hwnd_raw, engine))
                    .expect("spawn render thread")
            })
            .collect();
        Pool { shared, threads, rx, size, engine }
    }

    /// Set which documents should be loaded. Work for any other document is
    /// dropped and the workers unload it.
    pub fn set_docs(&self, docs: Vec<(u64, PathBuf)>) {
        let mut s = self.shared.m.lock().unwrap();
        let keep = |id: u64| docs.iter().any(|(d, _)| *d == id);
        s.jobs.retain(|j| keep(j.doc_id));
        s.delivered.retain(|k| keep(k.0));
        for (key, (flag, _)) in &s.running {
            if !keep(key.0) {
                flag.store(true, Ordering::Relaxed);
            }
        }
        s.docs = docs;
        self.shared.cv.notify_all();
    }

    /// Replace the queue with a new priority-ordered wish list.
    pub fn set_jobs(&self, jobs: Vec<Job>) {
        let mut s = self.shared.m.lock().unwrap();
        // Abandon running work that no longer matters: background work when
        // the view on screen needs a render, or a view the user moved away from.
        let fg_wanted = jobs.iter().any(|j| !j.is_background() && !s.delivered.contains(&j.key()));
        for (key, (flag, bg)) in &s.running {
            let stale = if *bg { fg_wanted } else { !jobs.iter().any(|j| j.key() == *key) };
            if stale {
                flag.store(true, Ordering::Relaxed);
            }
        }
        s.jobs = jobs;
        self.shared.cv.notify_all();
    }

    pub fn ack(&self, key: &JobKey) {
        self.shared.m.lock().unwrap().delivered.remove(key);
    }

    pub fn busy(&self) -> usize {
        self.shared.m.lock().unwrap().in_progress.len()
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        {
            let mut s = self.shared.m.lock().unwrap();
            s.quit = true;
            for (flag, _) in s.running.values() {
                flag.store(true, Ordering::Relaxed);
            }
            self.shared.cv.notify_all();
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

enum Task {
    Open(PathBuf, u64),
    Unload(Vec<u64>),
    Render(Job, usize, Arc<AtomicBool>),
}

/// `tried` = documents this worker has loaded or attempted; `loaded` = the
/// ones that succeeded.
fn next_task(shared: &Shared, tried: &HashSet<u64>, loaded: &HashSet<u64>) -> Option<Task> {
    let mut s = shared.m.lock().unwrap();
    loop {
        if s.quit {
            return None;
        }
        let unwanted: Vec<u64> = tried.iter().copied().filter(|id| !s.docs.iter().any(|(d, _)| d == id)).collect();
        if !unwanted.is_empty() {
            return Some(Task::Unload(unwanted));
        }
        if let Some((id, p)) = s.docs.iter().find(|(id, _)| !tried.contains(id)) {
            return Some(Task::Open(p.clone(), *id));
        }
        {
            let eligible = |j: &Job| {
                loaded.contains(&j.doc_id) && !s.delivered.contains(&j.key()) && !s.in_progress.contains(&j.key())
            };
            // Foreground first: background work never starts while a sharp
            // render of the on-screen view is queued or running, so it can't
            // compete with it inside the PDF engine.
            let fg_waiting = s.jobs.iter().any(|j| !j.is_background() && eligible(j));
            let bg_ok = s.bg_running < s.bg_limit && s.fg_running == 0 && !fg_waiting;
            let pick = s.jobs.iter().position(|j| eligible(j) && (!j.is_background() || bg_ok));
            if let Some(ix) = pick {
                let j = s.jobs.remove(ix);
                s.in_progress.insert(j.key());
                if j.is_background() {
                    s.bg_running += 1;
                } else {
                    s.fg_running += 1;
                }
                let parallel = s.in_progress.len();
                let flag = Arc::new(AtomicBool::new(false));
                s.running.insert(j.key(), (flag.clone(), j.is_background()));
                return Some(Task::Render(j, parallel, flag));
            }
        }
        s = shared.cv.wait(s).unwrap();
    }
}

fn notify(hwnd: isize) {
    unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_APP_RESULT, WPARAM(0), LPARAM(0));
    }
}

fn load(path: &Path, want_sizes: bool) -> Result<(PdfDocument, Vec<(f32, f32)>)> {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(abs.as_os_str()))?.join()?;
    let doc = PdfDocument::LoadFromFileAsync(&file)?.join()?;
    let mut pages = Vec::new();
    if want_sizes {
        for i in 0..doc.PageCount()? {
            let s = doc.GetPage(i)?.Size()?;
            pages.push((s.Width.max(1.0), s.Height.max(1.0)));
        }
    }
    Ok((doc, pages))
}

/// One worker's private GPU objects.
struct WorkerGpu {
    d3d: ID3D11Device,
    imm: ID3D11DeviceContext,
    ctx: ID2D1DeviceContext,
    renderer: IPdfRendererNative,
    query: ID3D11Query,
}

impl WorkerGpu {
    fn new(gpu: &GpuChoice) -> Result<Self> {
        unsafe {
            let d3d = gfx::create_device(gpu)?;
            let imm = d3d.GetImmediateContext()?;
            let dxgi: IDXGIDevice = d3d.cast()?;
            let opts = D2D1_FACTORY_OPTIONS { debugLevel: D2D1_DEBUG_LEVEL_NONE };
            let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, Some(&opts))?;
            let dev = factory.CreateDevice(&dxgi)?;
            // Spread geometry processing of heavy vector pages across CPU
            // cores while the GPU rasterises.
            let ctx = dev
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_ENABLE_MULTITHREADED_OPTIMIZATIONS)
                .or_else(|_| dev.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE))?;
            ctx.SetDpi(96.0, 96.0);
            let renderer = PdfCreateRenderer(&dxgi)?;
            let mut query = None;
            d3d.CreateQuery(&D3D11_QUERY_DESC { Query: D3D11_QUERY_EVENT, MiscFlags: 0 }, Some(&mut query))?;
            Ok(WorkerGpu { d3d, imm, ctx, renderer, query: query.ok_or_else(|| Error::from(E_POINTER))? })
        }
    }

    fn render(&self, page: &PdfPage, job: &Job) -> Result<(ID3D11Texture2D, isize)> {
        unsafe {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: job.w,
                Height: job.h,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: D3D11_RESOURCE_MISC_SHARED.0 as u32,
            };
            let mut tex = None;
            self.d3d.CreateTexture2D(&desc, None, Some(&mut tex))?;
            let tex: ID3D11Texture2D = tex.ok_or_else(|| Error::from(E_POINTER))?;
            let surf: IDXGISurface = tex.cast()?;
            let props = bitmap_props(D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_BITMAP_OPTIONS_TARGET);
            let bmp = self.ctx.CreateBitmapFromDxgiSurface(&surf, Some(&props))?;

            let white = D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
            self.ctx.SetTarget(&bmp);
            self.ctx.BeginDraw();
            self.ctx.Clear(Some(&white));
            let params = PDF_RENDER_PARAMS {
                SourceRect: D2D_RECT_F { left: job.src.l, top: job.src.t, right: job.src.r, bottom: job.src.b },
                DestinationWidth: job.w,
                DestinationHeight: job.h,
                BackgroundColor: D2D_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 },
                IgnoreHighContrast: true,
            };
            let unk: IUnknown = page.cast()?;
            let r = self.renderer.RenderPageToDeviceContext(&unk, &self.ctx, Some(&params));
            let e = self.ctx.EndDraw(None, None);
            self.ctx.SetTarget(None);
            drop(bmp);
            r?;
            e?;

            self.finish(tex)
        }
    }

    /// Wait for the GPU, then share the texture with the UI device.
    fn finish(&self, tex: ID3D11Texture2D) -> Result<(ID3D11Texture2D, isize)> {
        unsafe {
            // Shared resources are not synchronised across devices, so make
            // sure the GPU is done before the UI samples the texture.
            self.imm.End(&self.query);
            self.imm.Flush();
            let t0 = Instant::now();
            loop {
                let mut done = BOOL(0);
                let _ = self.imm.GetData(&self.query, Some(&mut done as *mut BOOL as *mut _), size_of::<BOOL>() as u32, 0);
                if done.as_bool() || t0.elapsed() > Duration::from_secs(20) {
                    break;
                }
                std::thread::sleep(Duration::from_micros(500));
            }
            let handle = tex.cast::<IDXGIResource>()?.GetSharedHandle()?;
            Ok((tex, handle.0 as isize))
        }
    }

    /// Upload a CPU-rendered BGRA image into a shared GPU texture.
    fn upload(&self, w: u32, h: u32, pixels: &[u8]) -> Result<(ID3D11Texture2D, isize)> {
        unsafe {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: w,
                Height: h,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: D3D11_RESOURCE_MISC_SHARED.0 as u32,
            };
            let init = D3D11_SUBRESOURCE_DATA {
                pSysMem: pixels.as_ptr() as *const _,
                SysMemPitch: w * 4,
                SysMemSlicePitch: 0,
            };
            let mut tex = None;
            self.d3d.CreateTexture2D(&desc, Some(&init), Some(&mut tex))?;
            self.finish(tex.ok_or_else(|| Error::from(E_POINTER))?)
        }
    }
}

/// A loaded document, per engine.
enum DocHandle {
    Windows { doc: PdfDocument, pages: Vec<(u32, PdfPage)> },
    Pdfium { doc: pdfium::Document, sizes: Vec<(f32, f32)> },
}

fn render_job(g: &WorkerGpu, d: &mut DocHandle, job: &Job, cancel: &AtomicBool) -> Result<Option<(ID3D11Texture2D, isize)>> {
    match d {
        DocHandle::Windows { doc, pages } => {
            let page = match pages.iter().position(|(i, _)| *i == job.page) {
                Some(ix) => {
                    let p = pages.remove(ix);
                    pages.insert(0, p);
                    pages[0].1.clone()
                }
                None => {
                    let p = doc.GetPage(job.page)?;
                    pages.insert(0, (job.page, p.clone()));
                    pages.truncate(4);
                    p
                }
            };
            g.render(&page, job).map(Some)
        }
        DocHandle::Pdfium { doc, sizes } => {
            let size = sizes.get(job.page as usize).copied().unwrap_or((job.src.w(), job.src.h()));
            match doc.render(job, size, cancel) {
                Err(e) => Err(Error::new(E_FAIL, e)),
                Ok(None) => Ok(None),
                Ok(Some(px)) => g.upload(job.w, job.h, &px).map(Some),
            }
        }
    }
}

fn run(index: usize, shared: Arc<Shared>, gpu: GpuChoice, tx: Sender<Msg>, hwnd: isize, engine: Engine) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let mut wgpu = WorkerGpu::new(&gpu);
    let use_pdfium = match engine {
        Engine::Pdfium => match pdfium::lib() {
            Ok(_) => true,
            Err(e) => {
                if index == 0 {
                    let _ = tx.send(Msg::EngineFallback(e));
                    notify(hwnd);
                }
                false
            }
        },
        Engine::Windows => false,
    };
    let mut docs: HashMap<u64, DocHandle> = HashMap::new();
    let mut tried: HashSet<u64> = HashSet::new();

    loop {
        let loaded: HashSet<u64> = docs.keys().copied().collect();
        let Some(task) = next_task(&shared, &tried, &loaded) else { break };
        match task {
            Task::Unload(ids) => {
                for id in ids {
                    tried.remove(&id);
                    docs.remove(&id);
                }
            }
            Task::Open(path, doc_id) => {
                tried.insert(doc_id);
                // Worker 0 reports the document to the UI; any others just
                // load their own copy so they can render in parallel.
                let res: std::result::Result<(DocHandle, Vec<(f32, f32)>), String> = match &wgpu {
                    Err(e) => Err(format!("GPU renderer init failed: {}", e.message())),
                    Ok(_) if use_pdfium => pdfium::Document::open(&path)
                        .map(|(doc, sizes)| (DocHandle::Pdfium { doc, sizes: sizes.clone() }, sizes)),
                    Ok(_) => load(&path, true)
                        .map(|(doc, sizes)| (DocHandle::Windows { doc, pages: Vec::new() }, sizes))
                        .map_err(|e| e.message().to_string()),
                };
                let msg = match res {
                    Ok((handle, pages)) => {
                        docs.insert(doc_id, handle);
                        Msg::Opened { doc_id, path, pages }
                    }
                    Err(error) => Msg::OpenFailed { doc_id, path, error },
                };
                if index == 0 {
                    let _ = tx.send(msg);
                    notify(hwnd);
                }
                shared.cv.notify_all();
            }
            Task::Render(job, parallel, cancel) => {
                let t0 = Instant::now();
                // Line-work extraction for snapping (PDFium only).
                let mut snap: Option<std::result::Result<SnapIndex, String>> = None;
                let result = if job.kind == JobKind::Geometry {
                    match docs.get_mut(&job.doc_id) {
                        Some(DocHandle::Pdfium { doc, sizes }) => {
                            let size = sizes.get(job.page as usize).copied().unwrap_or((1.0, 1.0));
                            match doc.line_work(job.page, size, &cancel) {
                                Ok(Some(segs)) => {
                                    snap = Some(Ok(SnapIndex::build(&segs, 8.0)));
                                    Ok(None)
                                }
                                Ok(None) => Ok(None),
                                Err(e) => {
                                    snap = Some(Err(e));
                                    Ok(None)
                                }
                            }
                        }
                        Some(DocHandle::Windows { .. }) => {
                            snap = Some(Err("snapping needs the PDFium engine (F4)".into()));
                            Ok(None)
                        }
                        None => Ok(None),
                    }
                } else {
                    match (&wgpu, docs.get_mut(&job.doc_id)) {
                        (Err(e), _) => Err(e.clone()),
                        (Ok(_), None) => Ok(None), // document was unloaded meanwhile
                        (Ok(g), Some(d)) => render_job(g, d, &job, &cancel),
                    }
                };
                // GPU reset (driver update / TDR): rebuild this worker's device.
                if let Err(e) = &result {
                    if gfx::is_device_lost(e) {
                        wgpu = WorkerGpu::new(&gpu);
                    }
                }
                let ms = t0.elapsed().as_secs_f32() * 1000.0;
                let cancelled = matches!(result, Ok(None)) && snap.is_none();
                {
                    let mut s = shared.m.lock().unwrap();
                    s.in_progress.remove(&job.key());
                    s.running.remove(&job.key());
                    if job.is_background() {
                        s.bg_running -= 1;
                    } else {
                        s.fg_running -= 1;
                    }
                    if cancelled {
                        // Background work is still wanted later; put it back.
                        if job.is_background() && !s.jobs.iter().any(|j| j.key() == job.key()) {
                            s.jobs.push(job.clone());
                        }
                    } else {
                        s.delivered.insert(job.key());
                    }
                    shared.cv.notify_all();
                }
                let msg = match (result, snap) {
                    (_, Some(index)) => Msg::Snap { job, index: index.map(Sendable), ms },
                    (result, None) => match result {
                    Ok(Some((tex, handle))) => Msg::Rendered { job, handle, texture: Sendable(tex), ms, parallel },
                    Ok(None) => Msg::Cancelled { job, ms },
                    Err(e) => Msg::RenderFailed { job, error: e.message().to_string() },
                    },
                };
                let _ = tx.send(msg);
                notify(hwnd);
            }
        }
    }
    drop(docs);
    drop(wgpu);
    unsafe { CoUninitialize() };
}

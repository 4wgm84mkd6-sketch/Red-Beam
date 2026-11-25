//! Pool of background render threads.
//!
//! Each worker owns its own D3D11 device + Direct2D context on the same GPU
//! as the UI, and its own `Windows.Data.Pdf` document instance, so several
//! regions/sheets rasterise in parallel and the UI thread never waits on a
//! render. Results are GPU textures shared with the UI device by handle.

use std::collections::HashSet;
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
use crate::view::RectF;

pub const WM_APP_RESULT: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 1;

/// Moves a COM pointer across threads (D3D device-child objects are free-threaded).
pub struct Sendable<T>(pub T);
unsafe impl<T> Send for Sendable<T> {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    /// Whole page at moderate resolution: instant feedback while moving.
    Base,
    /// A region at exact screen resolution: pixel-sharp when settled.
    Detail,
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
    Rendered { job: Job, handle: isize, texture: Sendable<ID3D11Texture2D>, ms: f32 },
    RenderFailed { job: Job, error: String },
}

#[derive(Default)]
struct State {
    path: Option<PathBuf>,
    doc_id: u64,
    jobs: Vec<Job>,
    in_progress: HashSet<JobKey>,
    /// Finished but not yet acknowledged by the UI (avoids duplicate renders
    /// when the UI re-posts its wish list before a result arrives).
    delivered: HashSet<JobKey>,
    /// Background jobs (base pages + prefetch) currently running, and the cap
    /// that keeps at least one worker free for the view on screen.
    bg_running: usize,
    bg_limit: usize,
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
}

impl Pool {
    pub fn start(gpu: &GpuChoice, hwnd: HWND, size: usize) -> Pool {
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
                    .spawn(move || run(i, s, gpu, tx, hwnd_raw))
                    .expect("spawn render thread")
            })
            .collect();
        Pool { shared, threads, rx, size }
    }

    pub fn open(&self, path: PathBuf, doc_id: u64) {
        let mut s = self.shared.m.lock().unwrap();
        s.path = Some(path);
        s.doc_id = doc_id;
        s.jobs.clear();
        s.delivered.clear();
        self.shared.cv.notify_all();
    }

    /// Replace the queue with a new priority-ordered wish list.
    pub fn set_jobs(&self, jobs: Vec<Job>) {
        let mut s = self.shared.m.lock().unwrap();
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
            self.shared.cv.notify_all();
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

enum Task {
    Open(PathBuf, u64),
    Render(Job),
}

/// `have` = id of the document this worker has loaded (or tried to load).
fn next_task(shared: &Shared, have: u64, loaded: bool) -> Option<Task> {
    let mut s = shared.m.lock().unwrap();
    loop {
        if s.quit {
            return None;
        }
        if s.doc_id != have {
            if let Some(p) = &s.path {
                return Some(Task::Open(p.clone(), s.doc_id));
            }
        }
        if loaded && s.doc_id == have {
            let pick = s.jobs.iter().position(|j| {
                j.doc_id == s.doc_id
                    && !s.delivered.contains(&j.key())
                    && !s.in_progress.contains(&j.key())
                    && (!j.is_background() || s.bg_running < s.bg_limit)
            });
            if let Some(ix) = pick {
                let j = s.jobs.remove(ix);
                s.in_progress.insert(j.key());
                if j.is_background() {
                    s.bg_running += 1;
                }
                return Some(Task::Render(j));
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

            // Wait until the GPU has finished drawing before handing the
            // texture to the UI device (shared resources are not otherwise
            // synchronised across devices).
            self.imm.End(&self.query);
            self.imm.Flush();
            let t0 = Instant::now();
            loop {
                let mut done = BOOL(0);
                let _ = self.imm.GetData(
                    &self.query,
                    Some(&mut done as *mut BOOL as *mut _),
                    size_of::<BOOL>() as u32,
                    0,
                );
                if done.as_bool() || t0.elapsed() > Duration::from_secs(20) {
                    break;
                }
                std::thread::sleep(Duration::from_micros(500));
            }
            let handle = tex.cast::<IDXGIResource>()?.GetSharedHandle()?;
            Ok((tex, handle.0 as isize))
        }
    }
}

fn run(index: usize, shared: Arc<Shared>, gpu: GpuChoice, tx: Sender<Msg>, hwnd: isize) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let mut wgpu = WorkerGpu::new(&gpu);
    let mut have: u64 = 0;
    let mut doc: Option<PdfDocument> = None;
    // Small MRU of page objects so repeat renders of a sheet skip GetPage.
    let mut pages: Vec<(u32, PdfPage)> = Vec::new();

    while let Some(task) = next_task(&shared, have, doc.is_some()) {
        match task {
            Task::Open(path, doc_id) => {
                have = doc_id;
                doc = None;
                pages.clear();
                // Worker 0 reports the document to the UI; the others just load
                // their own copy so they can render in parallel.
                let res = match &wgpu {
                    Err(e) => Err(Error::new(e.code(), format!("GPU renderer init failed: {}", e.message()))),
                    Ok(_) => load(&path, index == 0),
                };
                match res {
                    Ok((d, sizes)) => {
                        doc = Some(d);
                        if index == 0 {
                            let _ = tx.send(Msg::Opened { doc_id, path, pages: sizes });
                            notify(hwnd);
                        }
                    }
                    Err(e) if index == 0 => {
                        let _ = tx.send(Msg::OpenFailed { doc_id, path, error: e.message().to_string() });
                        notify(hwnd);
                    }
                    Err(_) => {}
                }
                // Wake others: this worker may now take jobs.
                shared.cv.notify_all();
            }
            Task::Render(job) => {
                let t0 = Instant::now();
                let page = match pages.iter().position(|(i, _)| *i == job.page) {
                    Some(ix) => {
                        let p = pages.remove(ix);
                        pages.insert(0, p);
                        Ok(pages[0].1.clone())
                    }
                    None => doc.as_ref().ok_or_else(|| Error::from(E_FAIL)).and_then(|d| d.GetPage(job.page)).inspect(|p| {
                        pages.insert(0, (job.page, p.clone()));
                        pages.truncate(4);
                    }),
                };
                let mut result = match (&wgpu, &page) {
                    (Ok(g), Ok(p)) => g.render(p, &job),
                    (Err(e), _) | (_, Err(e)) => Err(e.clone()),
                };
                // GPU reset (driver update / TDR): rebuild this worker's device once.
                if let Err(e) = &result {
                    if gfx::is_device_lost(e) {
                        wgpu = WorkerGpu::new(&gpu);
                        if let (Ok(g), Ok(p)) = (&wgpu, &page) {
                            result = g.render(p, &job);
                        }
                    }
                }
                let ms = t0.elapsed().as_secs_f32() * 1000.0;
                {
                    let mut s = shared.m.lock().unwrap();
                    s.in_progress.remove(&job.key());
                    s.delivered.insert(job.key());
                    if job.is_background() {
                        s.bg_running -= 1;
                    }
                    shared.cv.notify_all();
                }
                let msg = match result {
                    Ok((tex, handle)) => Msg::Rendered { job, handle, texture: Sendable(tex), ms },
                    Err(e) => Msg::RenderFailed { job, error: e.message().to_string() },
                };
                let _ = tx.send(msg);
                notify(hwnd);
            }
        }
    }
    drop(pages);
    drop(doc);
    drop(wgpu);
    unsafe { CoUninitialize() };
}

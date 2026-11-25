//! Background render thread. Owns the `Windows.Data.Pdf` document and a
//! dedicated Direct2D device context, and rasterises page regions straight
//! into GPU bitmaps via `IPdfRendererNative::RenderPageToDeviceContext`.
//! Finished bitmaps live on the shared D2D device, so the UI thread can draw
//! them without any copy back to system memory.

use std::collections::HashSet;
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use windows::core::*;
use windows::Data::Pdf::{PdfDocument, PdfPage};
use windows::Storage::StorageFile;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::Com::*;
use windows::Win32::System::WinRT::Pdf::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::view::RectF;

pub const WM_APP_RESULT: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 1;

/// Moves a COM pointer across threads. Direct2D objects created from a
/// multi-threaded factory are safe to use from any thread.
pub struct Sendable<T>(pub T);
unsafe impl<T> Send for Sendable<T> {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    /// Whole page at a moderate resolution: instant feedback while panning/zooming.
    Base,
    /// The visible region at exact screen resolution: pixel-sharp when settled.
    Detail,
}

#[derive(Clone, Debug)]
pub struct Job {
    pub doc_id: u64,
    pub page: u32,
    pub kind: JobKind,
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
    Rendered { job: Job, bitmap: Sendable<ID2D1Bitmap1>, ms: f32 },
    RenderFailed { job: Job, error: String },
}

#[derive(Default)]
struct State {
    open: Option<(PathBuf, u64)>,
    doc_id: u64,
    jobs: Vec<Job>,
    /// Finished jobs the UI has not acknowledged yet (prevents duplicate work
    /// when the UI re-posts its wish list before a result arrives).
    delivered: HashSet<JobKey>,
    quit: bool,
}

struct Shared {
    m: Mutex<State>,
    cv: Condvar,
}

pub struct Worker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    pub rx: Receiver<Msg>,
}

impl Worker {
    pub fn start(device: &ID2D1Device, dxgi: &IDXGIDevice, hwnd: HWND) -> Worker {
        let shared = Arc::new(Shared { m: Mutex::new(State::default()), cv: Condvar::new() });
        let (tx, rx) = channel();
        let s2 = shared.clone();
        let dev = Sendable(device.clone());
        let dxgi = Sendable(dxgi.clone());
        let hwnd_raw = hwnd.0 as isize;
        let thread = std::thread::Builder::new()
            .name("pdf-render".into())
            .spawn(move || {
                let dev = dev;
                let dxgi = dxgi;
                run(s2, dev.0, dxgi.0, tx, hwnd_raw)
            })
            .expect("spawn render thread");
        Worker { shared, thread: Some(thread), rx }
    }

    pub fn open(&self, path: PathBuf, doc_id: u64) {
        let mut s = self.shared.m.lock().unwrap();
        s.open = Some((path, doc_id));
        s.doc_id = doc_id;
        s.jobs.clear();
        s.delivered.clear();
        self.shared.cv.notify_all();
    }

    /// Replace the worker's queue with a new priority-ordered wish list.
    pub fn set_jobs(&self, jobs: Vec<Job>) {
        let mut s = self.shared.m.lock().unwrap();
        s.jobs = jobs;
        self.shared.cv.notify_all();
    }

    pub fn ack(&self, key: &JobKey) {
        self.shared.m.lock().unwrap().delivered.remove(key);
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        {
            let mut s = self.shared.m.lock().unwrap();
            s.quit = true;
            self.shared.cv.notify_all();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

enum Task {
    Open(PathBuf, u64),
    Render(Job),
}

fn next_task(shared: &Shared) -> Option<Task> {
    let mut s = shared.m.lock().unwrap();
    loop {
        if s.quit {
            return None;
        }
        if let Some((p, id)) = s.open.take() {
            return Some(Task::Open(p, id));
        }
        while !s.jobs.is_empty() {
            let j = s.jobs.remove(0);
            if j.doc_id == s.doc_id && !s.delivered.contains(&j.key()) {
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

fn load(path: &Path) -> Result<(PdfDocument, Vec<(f32, f32)>)> {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(abs.as_os_str()))?.join()?;
    let doc = PdfDocument::LoadFromFileAsync(&file)?.join()?;
    let n = doc.PageCount()?;
    let mut pages = Vec::with_capacity(n as usize);
    for i in 0..n {
        let s = doc.GetPage(i)?.Size()?;
        pages.push((s.Width.max(1.0), s.Height.max(1.0)));
    }
    Ok((doc, pages))
}

const WHITE: D2D1_COLOR_F = D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };

fn render(
    ctx: &ID2D1DeviceContext,
    renderer: &IPdfRendererNative,
    page: &PdfPage,
    job: &Job,
) -> Result<ID2D1Bitmap1> {
    unsafe {
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
            colorContext: ManuallyDrop::new(None),
        };
        let bmp = ctx.CreateBitmap(D2D_SIZE_U { width: job.w, height: job.h }, None, 0, &props)?;
        ctx.SetTarget(&bmp);
        ctx.BeginDraw();
        ctx.Clear(Some(&WHITE));
        let params = PDF_RENDER_PARAMS {
            SourceRect: D2D_RECT_F {
                left: job.src.l,
                top: job.src.t,
                right: job.src.r,
                bottom: job.src.b,
            },
            DestinationWidth: job.w,
            DestinationHeight: job.h,
            BackgroundColor: D2D_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 },
            IgnoreHighContrast: true,
        };
        let unk: IUnknown = page.cast()?;
        let r = renderer.RenderPageToDeviceContext(&unk, ctx, Some(&params));
        let e = ctx.EndDraw(None, None);
        ctx.SetTarget(None);
        r?;
        e?;
        Ok(bmp)
    }
}

fn run(shared: Arc<Shared>, device: ID2D1Device, dxgi: IDXGIDevice, tx: Sender<Msg>, hwnd: isize) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    // Spread geometry processing of heavy vector pages across CPU cores while
    // the GPU does the rasterisation.
    let ctx = unsafe {
        device
            .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_ENABLE_MULTITHREADED_OPTIMIZATIONS)
            .or_else(|_| device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE))
    };
    let renderer = unsafe { PdfCreateRenderer(&dxgi) };
    let init_err = match (&ctx, &renderer) {
        (Err(e), _) | (_, Err(e)) => Some(e.to_string()),
        _ => None,
    };
    if let Ok(c) = &ctx {
        unsafe { c.SetDpi(96.0, 96.0) };
    }

    let mut doc: Option<(u64, PdfDocument)> = None;
    // Small MRU of page objects so repeated renders of the same sheet don't
    // re-fetch it from the document.
    let mut pages: Vec<(u32, PdfPage)> = Vec::new();

    while let Some(task) = next_task(&shared) {
        match task {
            Task::Open(path, doc_id) => {
                doc = None;
                pages.clear();
                let msg = match (&init_err, load(&path)) {
                    (Some(e), _) => Msg::OpenFailed { doc_id, path, error: format!("GPU renderer init failed: {e}") },
                    (None, Ok((d, sizes))) => {
                        doc = Some((doc_id, d));
                        Msg::Opened { doc_id, path, pages: sizes }
                    }
                    (None, Err(e)) => Msg::OpenFailed { doc_id, path, error: e.message().to_string() },
                };
                let _ = tx.send(msg);
                notify(hwnd);
            }
            Task::Render(job) => {
                let (Some((id, d)), Ok(ctx), Ok(renderer)) = (&doc, &ctx, &renderer) else { continue };
                if *id != job.doc_id {
                    continue;
                }
                let t0 = Instant::now();
                let page = match pages.iter().position(|(i, _)| *i == job.page) {
                    Some(ix) => {
                        let p = pages.remove(ix);
                        pages.insert(0, p);
                        Ok(pages[0].1.clone())
                    }
                    None => d.GetPage(job.page).inspect(|p| {
                        pages.insert(0, (job.page, p.clone()));
                        pages.truncate(6);
                    }),
                };
                let result = page.and_then(|p| render(ctx, renderer, &p, &job));
                let ms = t0.elapsed().as_secs_f32() * 1000.0;
                shared.m.lock().unwrap().delivered.insert(job.key());
                let msg = match result {
                    Ok(b) => Msg::Rendered { job, bitmap: Sendable(b), ms },
                    Err(e) => Msg::RenderFailed { job, error: e.to_string() },
                };
                let _ = tx.send(msg);
                notify(hwnd);
            }
        }
    }
    drop(pages);
    drop(doc);
    drop(renderer);
    drop(ctx);
    unsafe { CoUninitialize() };
}

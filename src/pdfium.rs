//! PDFium (the PDF engine in Chrome/Edge) loaded from a DLL embedded in the
//! exe. Compared with Windows.Data.Pdf it parses each page once and keeps it
//! loaded, so repeated renders of a sheet only rasterise; it skips content
//! outside the requested region; and its progressive API lets us abandon a
//! render that is no longer wanted.
//!
//! PDFium is not thread-safe, so every call is made under `LOCK`.

use std::ffi::{c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::worker::Job;

/// Serialises all PDFium calls process-wide.
pub static LOCK: Mutex<()> = Mutex::new(());

#[cfg(has_pdfium)]
static DLL: &[u8] = include_bytes!(env!("PDFIUM_DLL_PATH"));
#[cfg(not(has_pdfium))]
static DLL: &[u8] = &[];

type Doc = *mut c_void;
type Page = *mut c_void;
type Bitmap = *mut c_void;

#[repr(C)]
struct FsSizeF {
    width: f32,
    height: f32,
}

#[repr(C)]
struct Pause {
    version: c_int,
    need_to_pause_now: extern "C" fn(*mut Pause) -> c_int,
    user: *mut c_void,
}

extern "C" fn need_to_pause(p: *mut Pause) -> c_int {
    // SAFETY: `user` points at the AtomicBool owned by the caller of `render`,
    // which outlives the render call.
    unsafe { (*((*p).user as *const AtomicBool)).load(Ordering::Relaxed) as c_int }
}

const FPDF_ANNOT: c_int = 0x01;
const FPDF_NO_NATIVETEXT: c_int = 0x04;
const BITMAP_BGRA: c_int = 4;
const RENDER_TOBECONTINUED: c_int = 1;
const RENDER_FAILED: c_int = 3;

pub struct Lib {
    init_library: unsafe extern "system" fn(),
    get_last_error: unsafe extern "system" fn() -> u32,
    load_mem_document64: unsafe extern "system" fn(*const c_void, usize, *const c_char) -> Doc,
    close_document: unsafe extern "system" fn(Doc),
    get_page_count: unsafe extern "system" fn(Doc) -> c_int,
    get_page_size_by_index_f: unsafe extern "system" fn(Doc, c_int, *mut FsSizeF) -> c_int,
    load_page: unsafe extern "system" fn(Doc, c_int) -> Page,
    close_page: unsafe extern "system" fn(Page),
    bitmap_create_ex: unsafe extern "system" fn(c_int, c_int, c_int, *mut c_void, c_int) -> Bitmap,
    bitmap_fill_rect: unsafe extern "system" fn(Bitmap, c_int, c_int, c_int, c_int, u32) -> c_int,
    bitmap_destroy: unsafe extern "system" fn(Bitmap),
    render_start: unsafe extern "system" fn(Bitmap, Page, c_int, c_int, c_int, c_int, c_int, c_int, *mut Pause) -> c_int,
    render_continue: unsafe extern "system" fn(Page, *mut Pause) -> c_int,
    render_close: unsafe extern "system" fn(Page),
}
unsafe impl Send for Lib {}
unsafe impl Sync for Lib {}

pub fn available() -> bool {
    !DLL.is_empty()
}

fn fnv1a(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf29ce484222325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

/// Write the embedded DLL to %LOCALAPPDATA%\Red-Beam (once per version).
fn extract() -> Result<PathBuf, String> {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let dir = base.join("Red-Beam");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = dir.join(format!("pdfium-{:016x}.dll", fnv1a(DLL)));
    let ok = std::fs::metadata(&path).map(|m| m.len() == DLL.len() as u64).unwrap_or(false);
    if !ok {
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        std::fs::write(&tmp, DLL).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        if std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
            // Another instance may have won the race; accept its copy.
            if !std::fs::metadata(&path).map(|m| m.len() == DLL.len() as u64).unwrap_or(false) {
                return Err(format!("cannot install {}", path.display()));
            }
        }
    }
    Ok(path)
}

// Each symbol is cast to the exact signature declared in `Lib`.
#[allow(clippy::missing_transmute_annotations)]
fn load_lib() -> Result<Lib, String> {
    use windows::core::{HSTRING, PCSTR};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    if DLL.is_empty() {
        return Err("this build does not include PDFium".into());
    }
    let path = extract()?;
    unsafe {
        let module = LoadLibraryW(&HSTRING::from(path.as_os_str())).map_err(|e| format!("cannot load PDFium: {e}"))?;
        macro_rules! sym {
            ($name:literal) => {{
                let p = GetProcAddress(module, PCSTR(concat!($name, "\0").as_ptr()))
                    .ok_or_else(|| format!("PDFium is missing {}", $name))?;
                std::mem::transmute::<unsafe extern "system" fn() -> isize, _>(p)
            }};
        }
        let lib = Lib {
            init_library: sym!("FPDF_InitLibrary"),
            get_last_error: sym!("FPDF_GetLastError"),
            load_mem_document64: sym!("FPDF_LoadMemDocument64"),
            close_document: sym!("FPDF_CloseDocument"),
            get_page_count: sym!("FPDF_GetPageCount"),
            get_page_size_by_index_f: sym!("FPDF_GetPageSizeByIndexF"),
            load_page: sym!("FPDF_LoadPage"),
            close_page: sym!("FPDF_ClosePage"),
            bitmap_create_ex: sym!("FPDFBitmap_CreateEx"),
            bitmap_fill_rect: sym!("FPDFBitmap_FillRect"),
            bitmap_destroy: sym!("FPDFBitmap_Destroy"),
            render_start: sym!("FPDF_RenderPageBitmap_Start"),
            render_continue: sym!("FPDF_RenderPage_Continue"),
            render_close: sym!("FPDF_RenderPage_Close"),
        };
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        (lib.init_library)();
        Ok(lib)
    }
}

/// The process-wide PDFium instance (loaded on first use).
pub fn lib() -> Result<&'static Lib, String> {
    static LIB: OnceLock<Result<Lib, String>> = OnceLock::new();
    LIB.get_or_init(load_lib).as_ref().map_err(|e| e.clone())
}

fn error_text(code: u32) -> &'static str {
    match code {
        2 => "the file could not be read",
        3 => "the file is not a valid PDF or is damaged",
        4 => "the PDF is password-protected (not supported yet)",
        5 => "the PDF uses unsupported security",
        6 => "a page could not be loaded",
        _ => "unknown PDFium error",
    }
}

/// PDF points (1/72 in) to DIPs (1/96 in).
const PT_TO_DIP: f32 = 96.0 / 72.0;

pub struct Document {
    lib: &'static Lib,
    doc: Doc,
    /// PDFium reads from this buffer for the document's lifetime.
    _data: Vec<u8>,
    /// Loaded (parsed) pages, most recently used first.
    pages: Vec<(u32, Page)>,
}
unsafe impl Send for Document {}

const KEEP_PAGES: usize = 6;

impl Document {
    /// Returns the document and its page sizes in DIPs.
    pub fn open(path: &Path) -> Result<(Document, Vec<(f32, f32)>), String> {
        let lib = lib()?;
        let data = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            let doc = (lib.load_mem_document64)(data.as_ptr() as *const c_void, data.len(), std::ptr::null());
            if doc.is_null() {
                return Err(error_text((lib.get_last_error)()).to_string());
            }
            let n = (lib.get_page_count)(doc).max(0);
            let mut sizes = Vec::with_capacity(n as usize);
            for i in 0..n {
                let mut s = FsSizeF { width: 612.0, height: 792.0 };
                (lib.get_page_size_by_index_f)(doc, i, &mut s);
                sizes.push(((s.width * PT_TO_DIP).max(1.0), (s.height * PT_TO_DIP).max(1.0)));
            }
            Ok((Document { lib, doc, _data: data, pages: Vec::new() }, sizes))
        }
    }

    /// Caller must hold `LOCK`.
    unsafe fn page(&mut self, index: u32) -> Result<Page, String> {
        if let Some(ix) = self.pages.iter().position(|(i, _)| *i == index) {
            let p = self.pages.remove(ix);
            self.pages.insert(0, p);
            return Ok(p.1);
        }
        let p = unsafe { (self.lib.load_page)(self.doc, index as c_int) };
        if p.is_null() {
            return Err(error_text(6).into());
        }
        self.pages.insert(0, (index, p));
        while self.pages.len() > KEEP_PAGES {
            let (_, old) = self.pages.pop().unwrap();
            unsafe { (self.lib.close_page)(old) };
        }
        Ok(p)
    }

    /// Render the job's `src` (page DIPs) at `scale` px/DIP into a w×h BGRA
    /// buffer. Returns `Ok(None)` if `cancel` was raised mid-render.
    pub fn render(&mut self, job: &Job, page_size: (f32, f32), cancel: &AtomicBool) -> Result<Option<Vec<u8>>, String> {
        let (page, src, w, h, scale) = (job.page, &job.src, job.w, job.h, job.scale);
        let mut buf = vec![0u8; w as usize * h as usize * 4];
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            let p = self.page(page)?;
            let bmp = (self.lib.bitmap_create_ex)(w as c_int, h as c_int, BITMAP_BGRA, buf.as_mut_ptr() as *mut c_void, (w * 4) as c_int);
            if bmp.is_null() {
                return Err("out of memory creating render bitmap".into());
            }
            (self.lib.bitmap_fill_rect)(bmp, 0, 0, w as c_int, h as c_int, 0xFFFF_FFFF);
            let mut pause = Pause { version: 1, need_to_pause_now: need_to_pause, user: cancel as *const AtomicBool as *mut c_void };
            let start_x = (-src.l * scale).round() as c_int;
            let start_y = (-src.t * scale).round() as c_int;
            let size_x = (page_size.0 * scale).round().max(1.0) as c_int;
            let size_y = (page_size.1 * scale).round().max(1.0) as c_int;
            let mut status = (self.lib.render_start)(bmp, p, start_x, start_y, size_x, size_y, 0, FPDF_ANNOT | FPDF_NO_NATIVETEXT, &mut pause);
            let mut cancelled = false;
            while status == RENDER_TOBECONTINUED {
                if cancel.load(Ordering::Relaxed) {
                    cancelled = true;
                    break;
                }
                status = (self.lib.render_continue)(p, &mut pause);
            }
            (self.lib.render_close)(p);
            (self.lib.bitmap_destroy)(bmp);
            if cancelled {
                return Ok(None);
            }
            if status == RENDER_FAILED {
                return Err("PDFium could not render this page".into());
            }
        }
        Ok(Some(buf))
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            for (_, p) in self.pages.drain(..) {
                (self.lib.close_page)(p);
            }
            (self.lib.close_document)(self.doc);
        }
    }
}

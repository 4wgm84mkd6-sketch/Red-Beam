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
type Obj = *mut c_void;
type Seg = *mut c_void;
type Annot = *mut c_void;

#[repr(C)]
struct FsRectF {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

/// FPDF_FILEWRITE followed by our output buffer.
#[repr(C)]
struct FileWrite {
    version: c_int,
    write_block: extern "C" fn(*mut FileWrite, *const c_void, std::ffi::c_ulong) -> c_int,
    out: *mut Vec<u8>,
}

extern "C" fn write_block(this: *mut FileWrite, data: *const c_void, size: std::ffi::c_ulong) -> c_int {
    // SAFETY: `this` is the FileWrite we passed to FPDF_SaveAsCopy; `out`
    // points at a live Vec for the duration of that call.
    unsafe {
        let out = &mut *(*this).out;
        out.extend_from_slice(std::slice::from_raw_parts(data as *const u8, size as usize));
    }
    1
}

const ANNOT_STAMP: c_int = 13;
const ANNOT_FLAG_PRINT: c_int = 1 << 2;
const FPDF_INCREMENTAL: u32 = 1;

const OBJ_PATH: c_int = 2;
const OBJ_FORM: c_int = 5;
const SEG_MOVETO: c_int = 2;
/// Stop collecting beyond this many segments (the snap index caps too).
const MAX_SEGMENTS: usize = 5_000_000;

#[repr(C)]
struct FsSizeF {
    width: f32,
    height: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FsMatrix {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

/// Affine transform, row-vector convention: p' = p * self.
#[derive(Clone, Copy)]
struct Affine([f32; 6]);

impl Affine {
    /// Apply `self`, then `then`.
    fn then(self, then: Affine) -> Affine {
        let (a, b) = (self.0, then.0);
        Affine([
            a[0] * b[0] + a[1] * b[2],
            a[0] * b[1] + a[1] * b[3],
            a[2] * b[0] + a[3] * b[2],
            a[2] * b[1] + a[3] * b[3],
            a[4] * b[0] + a[5] * b[2] + b[4],
            a[4] * b[1] + a[5] * b[3] + b[5],
        ])
    }
    fn apply(self, x: f32, y: f32) -> [f32; 2] {
        let m = self.0;
        [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]]
    }
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
    page_count_objects: unsafe extern "system" fn(Page) -> c_int,
    page_get_object: unsafe extern "system" fn(Page, c_int) -> Obj,
    obj_get_type: unsafe extern "system" fn(Obj) -> c_int,
    obj_get_matrix: unsafe extern "system" fn(Obj, *mut FsMatrix) -> c_int,
    path_count_segments: unsafe extern "system" fn(Obj) -> c_int,
    path_get_segment: unsafe extern "system" fn(Obj, c_int) -> Seg,
    seg_get_point: unsafe extern "system" fn(Seg, *mut f32, *mut f32) -> c_int,
    seg_get_type: unsafe extern "system" fn(Seg) -> c_int,
    seg_get_close: unsafe extern "system" fn(Seg) -> c_int,
    form_count_objects: unsafe extern "system" fn(Obj) -> c_int,
    form_get_object: unsafe extern "system" fn(Obj, u32) -> Obj,
    page_to_device:
        unsafe extern "system" fn(Page, c_int, c_int, c_int, c_int, c_int, f64, f64, *mut c_int, *mut c_int) -> c_int,
    // Annotation export
    create_annot: unsafe extern "system" fn(Page, c_int) -> Annot,
    close_annot: unsafe extern "system" fn(Annot),
    annot_set_rect: unsafe extern "system" fn(Annot, *const FsRectF) -> c_int,
    annot_append_object: unsafe extern "system" fn(Annot, Obj) -> c_int,
    annot_set_string: unsafe extern "system" fn(Annot, *const c_char, *const u16) -> c_int,
    annot_set_flags: unsafe extern "system" fn(Annot, c_int) -> c_int,
    new_path: unsafe extern "system" fn(f32, f32) -> Obj,
    path_move_to: unsafe extern "system" fn(Obj, f32, f32) -> c_int,
    path_line_to: unsafe extern "system" fn(Obj, f32, f32) -> c_int,
    path_close: unsafe extern "system" fn(Obj) -> c_int,
    path_set_draw_mode: unsafe extern "system" fn(Obj, c_int, c_int) -> c_int,
    obj_set_stroke_color: unsafe extern "system" fn(Obj, u32, u32, u32, u32) -> c_int,
    obj_set_stroke_width: unsafe extern "system" fn(Obj, f32) -> c_int,
    obj_set_fill_color: unsafe extern "system" fn(Obj, u32, u32, u32, u32) -> c_int,
    obj_set_line_join: unsafe extern "system" fn(Obj, c_int) -> c_int,
    obj_set_line_cap: unsafe extern "system" fn(Obj, c_int) -> c_int,
    obj_transform: unsafe extern "system" fn(Obj, f64, f64, f64, f64, f64, f64),
    obj_destroy: unsafe extern "system" fn(Obj),
    new_text_obj: unsafe extern "system" fn(Doc, *const c_char, f32) -> Obj,
    text_set_text: unsafe extern "system" fn(Obj, *const u16) -> c_int,
    save_as_copy: unsafe extern "system" fn(Doc, *mut FileWrite, u32) -> c_int,
    // Text and bookmarks
    text_load_page: unsafe extern "system" fn(Page) -> *mut c_void,
    text_close_page: unsafe extern "system" fn(*mut c_void),
    text_count_chars: unsafe extern "system" fn(*mut c_void) -> c_int,
    text_get_unicode: unsafe extern "system" fn(*mut c_void, c_int) -> u32,
    text_get_char_box: unsafe extern "system" fn(*mut c_void, c_int, *mut f64, *mut f64, *mut f64, *mut f64) -> c_int,
    text_get_font_size: unsafe extern "system" fn(*mut c_void, c_int) -> f64,
    bm_first_child: unsafe extern "system" fn(Doc, *mut c_void) -> *mut c_void,
    bm_next_sibling: unsafe extern "system" fn(Doc, *mut c_void) -> *mut c_void,
    bm_get_title: unsafe extern "system" fn(*mut c_void, *mut c_void, u32) -> u32,
    bm_get_dest: unsafe extern "system" fn(Doc, *mut c_void) -> *mut c_void,
    bm_get_action: unsafe extern "system" fn(*mut c_void) -> *mut c_void,
    action_get_dest: unsafe extern "system" fn(Doc, *mut c_void) -> *mut c_void,
    dest_page_index: unsafe extern "system" fn(Doc, *mut c_void) -> c_int,
    get_page_label: unsafe extern "system" fn(Doc, c_int, *mut c_void, u32) -> u32,
    // Printing and flattening
    render_page_dc: unsafe extern "system" fn(*mut c_void, Page, c_int, c_int, c_int, c_int, c_int, c_int),
    page_insert_object: unsafe extern "system" fn(Page, Obj),
    page_generate_content: unsafe extern "system" fn(Page) -> c_int,
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
            page_count_objects: sym!("FPDFPage_CountObjects"),
            page_get_object: sym!("FPDFPage_GetObject"),
            obj_get_type: sym!("FPDFPageObj_GetType"),
            obj_get_matrix: sym!("FPDFPageObj_GetMatrix"),
            path_count_segments: sym!("FPDFPath_CountSegments"),
            path_get_segment: sym!("FPDFPath_GetPathSegment"),
            seg_get_point: sym!("FPDFPathSegment_GetPoint"),
            seg_get_type: sym!("FPDFPathSegment_GetType"),
            seg_get_close: sym!("FPDFPathSegment_GetClose"),
            form_count_objects: sym!("FPDFFormObj_CountObjects"),
            form_get_object: sym!("FPDFFormObj_GetObject"),
            page_to_device: sym!("FPDF_PageToDevice"),
            create_annot: sym!("FPDFPage_CreateAnnot"),
            close_annot: sym!("FPDFPage_CloseAnnot"),
            annot_set_rect: sym!("FPDFAnnot_SetRect"),
            annot_append_object: sym!("FPDFAnnot_AppendObject"),
            annot_set_string: sym!("FPDFAnnot_SetStringValue"),
            annot_set_flags: sym!("FPDFAnnot_SetFlags"),
            new_path: sym!("FPDFPageObj_CreateNewPath"),
            path_move_to: sym!("FPDFPath_MoveTo"),
            path_line_to: sym!("FPDFPath_LineTo"),
            path_close: sym!("FPDFPath_Close"),
            path_set_draw_mode: sym!("FPDFPath_SetDrawMode"),
            obj_set_stroke_color: sym!("FPDFPageObj_SetStrokeColor"),
            obj_set_stroke_width: sym!("FPDFPageObj_SetStrokeWidth"),
            obj_set_fill_color: sym!("FPDFPageObj_SetFillColor"),
            obj_set_line_join: sym!("FPDFPageObj_SetLineJoin"),
            obj_set_line_cap: sym!("FPDFPageObj_SetLineCap"),
            obj_transform: sym!("FPDFPageObj_Transform"),
            obj_destroy: sym!("FPDFPageObj_Destroy"),
            new_text_obj: sym!("FPDFPageObj_NewTextObj"),
            text_set_text: sym!("FPDFText_SetText"),
            save_as_copy: sym!("FPDF_SaveAsCopy"),
            text_load_page: sym!("FPDFText_LoadPage"),
            text_close_page: sym!("FPDFText_ClosePage"),
            text_count_chars: sym!("FPDFText_CountChars"),
            text_get_unicode: sym!("FPDFText_GetUnicode"),
            text_get_char_box: sym!("FPDFText_GetCharBox"),
            text_get_font_size: sym!("FPDFText_GetFontSize"),
            bm_first_child: sym!("FPDFBookmark_GetFirstChild"),
            bm_next_sibling: sym!("FPDFBookmark_GetNextSibling"),
            bm_get_title: sym!("FPDFBookmark_GetTitle"),
            bm_get_dest: sym!("FPDFBookmark_GetDest"),
            bm_get_action: sym!("FPDFBookmark_GetAction"),
            action_get_dest: sym!("FPDFAction_GetDest"),
            dest_page_index: sym!("FPDFDest_GetDestPageIndex"),
            get_page_label: sym!("FPDF_GetPageLabel"),
            render_page_dc: sym!("FPDF_RenderPage"),
            page_insert_object: sym!("FPDFPage_InsertObject"),
            page_generate_content: sym!("FPDFPage_GenerateContent"),
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

/// PDF user space -> page DIPs, exactly as rendering maps it (crop box and
/// rotation), derived from FPDF_PageToDevice at a large size for sub-pixel
/// precision. Caller holds `LOCK`.
unsafe fn page_to_dip(lib: &Lib, p: Page, page_size: (f32, f32)) -> Affine {
    const K: f32 = 1000.0;
    let (w, h) = ((page_size.0.round() * K) as c_int, (page_size.1.round() * K) as c_int);
    let dev = |x: f64, y: f64| {
        let (mut dx, mut dy) = (0, 0);
        unsafe { (lib.page_to_device)(p, 0, 0, w, h, 0, x, y, &mut dx, &mut dy) };
        [dx as f32 / K, dy as f32 / K]
    };
    let (o, ex, ey) = (dev(0.0, 0.0), dev(1000.0, 0.0), dev(0.0, 1000.0));
    Affine([(ex[0] - o[0]) / 1000.0, (ex[1] - o[1]) / 1000.0, (ey[0] - o[0]) / 1000.0, (ey[1] - o[1]) / 1000.0, o[0], o[1]])
}

impl Affine {
    fn inverse(self) -> Affine {
        let [a, b, c, d, e, f] = self.0;
        let det = a * d - b * c;
        let (ia, ib, ic, id) = (d / det, -b / det, -c / det, a / det);
        Affine([ia, ib, ic, id, -(e * ia + f * ic), -(e * ib + f * id)])
    }
}

/// One annotation to write: what it is, and how it looks.
pub struct AnnotSpec {
    pub page: u32,
    pub subject: String,
    pub contents: String,
    pub author: String,
    /// Unique name (/NM), e.g. `redbeam-markup-12`.
    pub name: String,
    pub prims: Vec<crate::markup::Prim>,
}

fn utf16z(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Write a copy of `src` to `dst` with `annots` added: as PDF annotations
/// (stamp annotations carrying their own appearance, so Acrobat, Bluebeam
/// and browsers all display them and they stay movable), or, with
/// `flatten`, burned into the page content. Returns how many were written.
pub fn export_annotated(src: &Path, dst: &Path, annots: &[AnnotSpec], flatten: bool) -> Result<usize, String> {
    let data = std::fs::read(src).map_err(|e| format!("cannot read {}: {e}", src.display()))?;
    let (out, written) = build_pdf(&data, annots, flatten)?;
    let tmp = dst.with_extension("pdf.tmp");
    std::fs::write(&tmp, &out).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, dst).map_err(|e| format!("cannot write {}: {e}", dst.display()))?;
    Ok(written)
}

/// The PDF bytes of `data` with `annots` added (see `export_annotated`).
pub fn build_pdf(data: &[u8], annots: &[AnnotSpec], flatten: bool) -> Result<(Vec<u8>, usize), String> {
    use crate::markup::{wrap, Prim};
    let lib = lib()?;
    let mut written = 0usize;
    let mut out: Vec<u8> = Vec::with_capacity(data.len() + 65536);
    {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            let doc = (lib.load_mem_document64)(data.as_ptr() as *const c_void, data.len(), std::ptr::null());
            if doc.is_null() {
                return Err(error_text((lib.get_last_error)()).to_string());
            }
            let mut pages: Vec<u32> = annots.iter().map(|a| a.page).collect();
            pages.sort_unstable();
            pages.dedup();
            for pg in pages {
                let p = (lib.load_page)(doc, pg as c_int);
                if p.is_null() {
                    continue;
                }
                let mut sz = FsSizeF { width: 612.0, height: 792.0 };
                (lib.get_page_size_by_index_f)(doc, pg as c_int, &mut sz);
                let to_pdf = page_to_dip(lib, p, (sz.width * PT_TO_DIP, sz.height * PT_TO_DIP)).inverse();
                let k = to_pdf.0[0].hypot(to_pdf.0[1]);
                // Unit vectors for upright text: DIP x, and DIP "up" (-y).
                let ux = [to_pdf.0[0] / k, to_pdf.0[1] / k];
                let uy = [-to_pdf.0[2] / k, -to_pdf.0[3] / k];
                for a in annots.iter().filter(|a| a.page == pg) {
                    let annot = if flatten { std::ptr::null_mut() } else { (lib.create_annot)(p, ANNOT_STAMP) };
                    if annot.is_null() && !flatten {
                        continue;
                    }
                    // Flattened: objects go straight into the page content.
                    let add = |obj: Obj| {
                        if flatten {
                            (lib.page_insert_object)(p, obj);
                        } else if (lib.annot_append_object)(annot, obj) == 0 {
                            (lib.obj_destroy)(obj);
                        }
                    };
                    // Annotation rectangle: bounds of everything, in PDF space.
                    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                    let mut grow = |q: [f32; 2], pad: f32| {
                        let r = to_pdf.apply(q[0], q[1]);
                        x0 = x0.min(r[0] - pad);
                        y0 = y0.min(r[1] - pad);
                        x1 = x1.max(r[0] + pad);
                        y1 = y1.max(r[1] + pad);
                    };
                    for prim in &a.prims {
                        match prim {
                            Prim::Path { rings, stroke, .. } => {
                                let pad = stroke.map(|s| s.1 * k).unwrap_or(0.0) + 1.0;
                                rings.iter().flatten().for_each(|q| grow(*q, pad));
                            }
                            Prim::Text { rect, .. } => {
                                for q in [[rect[0], rect[1]], [rect[2], rect[3]], [rect[0], rect[3]], [rect[2], rect[1]]] {
                                    grow(q, 1.0);
                                }
                            }
                        }
                    }
                    let r = FsRectF { left: x0, top: y1, right: x1, bottom: y0 };
                    if !flatten {
                        (lib.annot_set_rect)(annot, &r);
                    }
                    let rgb = |c: u32| ((c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff);
                    for prim in &a.prims {
                        match prim {
                            Prim::Path { rings, closed, stroke, fill } => {
                                let Some(first) = rings.iter().find(|r| !r.is_empty()) else { continue };
                                let f0 = to_pdf.apply(first[0][0], first[0][1]);
                                let obj = (lib.new_path)(f0[0], f0[1]);
                                for (ri, ring) in rings.iter().filter(|r| !r.is_empty()).enumerate() {
                                    for (i, q) in ring.iter().enumerate() {
                                        let t = to_pdf.apply(q[0], q[1]);
                                        if i == 0 {
                                            if ri > 0 {
                                                (lib.path_move_to)(obj, t[0], t[1]);
                                            }
                                        } else {
                                            (lib.path_line_to)(obj, t[0], t[1]);
                                        }
                                    }
                                    if *closed {
                                        (lib.path_close)(obj);
                                    }
                                }
                                if let Some((c, w)) = stroke {
                                    let (r, g, b) = rgb(*c);
                                    (lib.obj_set_stroke_color)(obj, r, g, b, 255);
                                    (lib.obj_set_stroke_width)(obj, w * k);
                                }
                                if let Some((c, alpha)) = fill {
                                    let (r, g, b) = rgb(*c);
                                    (lib.obj_set_fill_color)(obj, r, g, b, (alpha * 255.0).round() as u32);
                                }
                                (lib.obj_set_line_join)(obj, 1);
                                (lib.obj_set_line_cap)(obj, 1);
                                (lib.path_set_draw_mode)(obj, if fill.is_some() { 1 } else { 0 }, stroke.is_some() as c_int);
                                add(obj);
                            }
                            Prim::Text { rect, text, size, color, bold, center } => {
                                let font: &[u8] = if *bold { b"Helvetica-Bold\0" } else { b"Helvetica\0" };
                                let width = rect[2] - rect[0];
                                for (i, line) in wrap(text, *size, width).iter().enumerate() {
                                    if line.is_empty() {
                                        continue;
                                    }
                                    let baseline = rect[1] + size * (0.85 + i as f32 * 1.2);
                                    if baseline > rect[3] + size * 0.4 {
                                        break;
                                    }
                                    let est = line.chars().count() as f32 * size * if *bold { 0.62 } else { 0.52 };
                                    let x = if *center { rect[0] + ((width - est) / 2.0).max(0.0) } else { rect[0] };
                                    let obj = (lib.new_text_obj)(doc, font.as_ptr() as *const c_char, size * k);
                                    if obj.is_null() {
                                        continue;
                                    }
                                    let w = utf16z(line);
                                    (lib.text_set_text)(obj, w.as_ptr());
                                    let (r, g, b) = rgb(*color);
                                    (lib.obj_set_fill_color)(obj, r, g, b, 255);
                                    let o = to_pdf.apply(x, baseline);
                                    (lib.obj_transform)(obj, ux[0] as f64, ux[1] as f64, uy[0] as f64, uy[1] as f64, o[0] as f64, o[1] as f64);
                                    add(obj);
                                }
                            }
                        }
                    }
                    if !flatten {
                        for (key, val) in [("Contents", &a.contents), ("T", &a.author), ("Subj", &a.subject), ("NM", &a.name)] {
                            let k = std::ffi::CString::new(key).unwrap();
                            let v = utf16z(val);
                            (lib.annot_set_string)(annot, k.as_ptr(), v.as_ptr());
                        }
                        (lib.annot_set_flags)(annot, ANNOT_FLAG_PRINT);
                        (lib.close_annot)(annot);
                    }
                    written += 1;
                }
                if flatten {
                    (lib.page_generate_content)(p);
                }
                (lib.close_page)(p);
            }
            let mut fw = FileWrite { version: 1, write_block, out: &mut out };
            let ok = (lib.save_as_copy)(doc, &mut fw, FPDF_INCREMENTAL);
            (lib.close_document)(doc);
            if ok == 0 {
                return Err("PDFium could not save the document".into());
            }
        }
    }
    Ok((out, written))
}

/// Print `pages` (0-based) of the PDF `data` to printer DC `hdc`: each
/// sheet fitted to the printable area and rotated to match the paper.
/// Returns the number of pages printed.
pub fn print_pdf(data: &[u8], pages: &[u32], hdc: isize, job_name: &str) -> Result<usize, String> {
    use windows::core::HSTRING;
    use windows::Win32::Graphics::Gdi::{GetDeviceCaps, HDC, HORZRES, VERTRES};
    use windows::Win32::Storage::Xps::{EndDoc, EndPage, StartDocW, StartPage, DOCINFOW};
    let lib = lib()?;
    let dc = HDC(hdc as *mut c_void);
    let name = HSTRING::from(job_name);
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    unsafe {
        let doc = (lib.load_mem_document64)(data.as_ptr() as *const c_void, data.len(), std::ptr::null());
        if doc.is_null() {
            return Err(error_text((lib.get_last_error)()).to_string());
        }
        let info = DOCINFOW { cbSize: size_of::<DOCINFOW>() as i32, lpszDocName: windows::core::PCWSTR(name.as_ptr()), ..Default::default() };
        if StartDocW(dc, &info) <= 0 {
            (lib.close_document)(doc);
            return Err("the printer refused the job".into());
        }
        let (pw, ph) = (GetDeviceCaps(Some(dc), HORZRES), GetDeviceCaps(Some(dc), VERTRES));
        let mut printed = 0;
        for &pg in pages {
            let p = (lib.load_page)(doc, pg as c_int);
            if p.is_null() {
                continue;
            }
            let mut sz = FsSizeF { width: 612.0, height: 792.0 };
            (lib.get_page_size_by_index_f)(doc, pg as c_int, &mut sz);
            // Rotate a landscape sheet onto portrait paper (and vice versa).
            let rotate = ((sz.width > sz.height) != (pw > ph)) as c_int;
            let (w, h) = if rotate == 1 { (sz.height, sz.width) } else { (sz.width, sz.height) };
            let k = (pw as f32 / w).min(ph as f32 / h);
            let (sx, sy) = ((w * k) as c_int, (h * k) as c_int);
            StartPage(dc);
            (lib.render_page_dc)(dc.0, p, (pw - sx) / 2, (ph - sy) / 2, sx, sy, rotate, FPDF_ANNOT | 0x800);
            EndPage(dc);
            (lib.close_page)(p);
            printed += 1;
        }
        EndDoc(dc);
        (lib.close_document)(doc);
        Ok(printed)
    }
}

/// UTF-16LE bytes (with trailing NUL) from a PDFium "get string" call.
fn utf16_bytes_to_string(buf: &[u8]) -> String {
    let u: Vec<u16> = buf.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    String::from_utf16_lossy(&u).trim_end_matches('\0').trim().to_string()
}

impl Document {
    /// A label per page: the most specific bookmark pointing at it (Revit
    /// exports bookmark every sheet, e.g. "A101 - FLOOR PLAN"), else the
    /// PDF page label, else "".
    pub fn labels(&self, pages: usize) -> Vec<String> {
        let lib = self.lib;
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<(usize, String)> = vec![(0, String::new()); pages];
        unsafe {
            // Depth-first walk with an explicit stack (and a size guard).
            let mut stack: Vec<(*mut c_void, usize)> = vec![((lib.bm_first_child)(self.doc, std::ptr::null_mut()), 1)];
            let mut seen = 0;
            while let Some((bm, depth)) = stack.pop() {
                if bm.is_null() || seen > 20_000 {
                    continue;
                }
                seen += 1;
                stack.push(((lib.bm_next_sibling)(self.doc, bm), depth));
                stack.push(((lib.bm_first_child)(self.doc, bm), depth + 1));
                let n = (lib.bm_get_title)(bm, std::ptr::null_mut(), 0);
                if n < 2 {
                    continue;
                }
                let mut buf = vec![0u8; n as usize];
                (lib.bm_get_title)(bm, buf.as_mut_ptr() as *mut c_void, n);
                let title = utf16_bytes_to_string(&buf);
                let mut dest = (lib.bm_get_dest)(self.doc, bm);
                if dest.is_null() {
                    let act = (lib.bm_get_action)(bm);
                    if !act.is_null() {
                        dest = (lib.action_get_dest)(self.doc, act);
                    }
                }
                if dest.is_null() {
                    continue;
                }
                let p = (lib.dest_page_index)(self.doc, dest);
                if p >= 0 && (p as usize) < pages && depth > out[p as usize].0 {
                    out[p as usize] = (depth, title);
                }
            }
            for (i, slot) in out.iter_mut().enumerate() {
                if slot.1.is_empty() {
                    let n = (lib.get_page_label)(self.doc, i as c_int, std::ptr::null_mut(), 0);
                    if n >= 2 {
                        let mut buf = vec![0u8; n as usize];
                        (lib.get_page_label)(self.doc, i as c_int, buf.as_mut_ptr() as *mut c_void, n);
                        slot.1 = utf16_bytes_to_string(&buf);
                    }
                }
            }
        }
        out.into_iter().map(|(_, t)| t).collect()
    }

    /// Words on a page with boxes in page DIPs. `Ok(None)` if cancelled.
    pub fn words(&mut self, page: u32, page_size: (f32, f32), cancel: &AtomicBool) -> Result<Option<Vec<crate::sheets::Word>>, String> {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            let p = self.page(page)?;
            let lib = self.lib;
            let to_dip = page_to_dip(lib, p, page_size);
            let tp = (lib.text_load_page)(p);
            if tp.is_null() {
                return Ok(Some(Vec::new()));
            }
            let n = (lib.text_count_chars)(tp).max(0);
            let mut words = Vec::new();
            let mut cur = String::new();
            let mut bx = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
            let mut size = 0f32;
            let mut last_right = f32::MIN;
            let flush = |cur: &mut String, bx: &mut [f32; 4], size: &mut f32, words: &mut Vec<crate::sheets::Word>| {
                if !cur.trim().is_empty() {
                    words.push(crate::sheets::Word { text: std::mem::take(cur), rect: *bx, size: *size });
                }
                cur.clear();
                *bx = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
                *size = 0.0;
            };
            for i in 0..n {
                if i % 4096 == 0 && cancel.load(Ordering::Relaxed) {
                    (lib.text_close_page)(tp);
                    return Ok(None);
                }
                let c = char::from_u32((lib.text_get_unicode)(tp, i)).unwrap_or(' ');
                // Words break at spaces, line ends and "/" (detail callouts
                // like 5/A-501), and at big gaps between characters.
                if c.is_whitespace() || c == '/' || c == '\0' {
                    flush(&mut cur, &mut bx, &mut size, &mut words);
                    continue;
                }
                let (mut l, mut r, mut b, mut t) = (0f64, 0f64, 0f64, 0f64);
                if (lib.text_get_char_box)(tp, i, &mut l, &mut r, &mut b, &mut t) == 0 {
                    continue;
                }
                let pts = [to_dip.apply(l as f32, b as f32), to_dip.apply(r as f32, t as f32)];
                let rect = [pts[0][0].min(pts[1][0]), pts[0][1].min(pts[1][1]), pts[0][0].max(pts[1][0]), pts[0][1].max(pts[1][1])];
                let fs = (lib.text_get_font_size)(tp, i) as f32 * PT_TO_DIP;
                if !cur.is_empty() && (rect[0] - last_right > fs * 0.8 || rect[1] > bx[3] || rect[3] < bx[1]) {
                    flush(&mut cur, &mut bx, &mut size, &mut words);
                }
                cur.push(c);
                bx = [bx[0].min(rect[0]), bx[1].min(rect[1]), bx[2].max(rect[2]), bx[3].max(rect[3])];
                size = size.max(fs);
                last_right = rect[2];
            }
            flush(&mut cur, &mut bx, &mut size, &mut words);
            (lib.text_close_page)(tp);
            Ok(Some(words))
        }
    }

    /// All straight segments of the page's line work in page DIPs (curves
    /// contribute their chords), for snapping. `Ok(None)` if cancelled.
    pub fn line_work(&mut self, page: u32, page_size: (f32, f32), cancel: &AtomicBool) -> Result<Option<Vec<[[f32; 2]; 2]>>, String> {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            let p = self.page(page)?;
            let lib = self.lib;
            let to_dip = page_to_dip(lib, p, page_size);
            let mut out: Vec<[[f32; 2]; 2]> = Vec::new();
            // Explicit stack: (object, transform of its parent form).
            let mut stack: Vec<(Obj, Affine)> =
                (0..(lib.page_count_objects)(p)).rev().map(|i| ((lib.page_get_object)(p, i), to_dip)).collect();
            let mut visited = 0usize;
            while let Some((obj, parent)) = stack.pop() {
                visited += 1;
                if visited.is_multiple_of(4096) && cancel.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                if obj.is_null() || out.len() >= MAX_SEGMENTS {
                    continue;
                }
                let mut m = FsMatrix { a: 1.0, d: 1.0, ..Default::default() };
                (lib.obj_get_matrix)(obj, &mut m);
                let mm = Affine([m.a, m.b, m.c, m.d, m.e, m.f]).then(parent);
                match (lib.obj_get_type)(obj) {
                    OBJ_PATH => {
                        let (mut prev, mut start): (Option<[f32; 2]>, Option<[f32; 2]>) = (None, None);
                        for i in 0..(lib.path_count_segments)(obj) {
                            let seg = (lib.path_get_segment)(obj, i);
                            let (mut x, mut y) = (0f32, 0f32);
                            if seg.is_null() || (lib.seg_get_point)(seg, &mut x, &mut y) == 0 {
                                continue;
                            }
                            let q = mm.apply(x, y);
                            if (lib.seg_get_type)(seg) == SEG_MOVETO {
                                start = Some(q);
                            } else if let Some(a) = prev {
                                out.push([a, q]);
                            }
                            if (lib.seg_get_close)(seg) != 0 {
                                if let Some(s0) = start {
                                    out.push([q, s0]);
                                }
                            }
                            prev = Some(q);
                        }
                    }
                    OBJ_FORM => {
                        for i in (0..(lib.form_count_objects)(obj).max(0) as u32).rev() {
                            stack.push(((lib.form_get_object)(obj, i), mm));
                        }
                    }
                    _ => {}
                }
            }
            Ok(Some(out))
        }
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

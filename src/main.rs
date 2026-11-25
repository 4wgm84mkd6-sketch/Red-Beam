//! Red-Beam: a GPU-accelerated PDF viewer for large construction drawing sets.
#![windows_subsystem = "windows"]

#[cfg(not(windows))]
compile_error!("Red-Beam is a Windows application (it uses Direct2D and Windows.Data.Pdf).");

mod app;
mod gfx;
mod pdfium;
mod update;
mod view;
mod worker;

use std::cell::RefCell;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use app::{App, WM_APP_COMPARE, WM_APP_OPEN};

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Run `f` against the app unless it is already borrowed (re-entrant message).
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|c| c.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

fn error_box(hwnd: HWND, text: &str) {
    unsafe {
        MessageBoxW(Some(hwnd), &HSTRING::from(text), w!("Red-Beam"), MB_OK | MB_ICONERROR);
    }
}

fn open_dialog(hwnd: HWND, title: PCWSTR) -> Option<PathBuf> {
    unsafe {
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let filters = [
            COMDLG_FILTERSPEC { pszName: w!("PDF drawings"), pszSpec: w!("*.pdf") },
            COMDLG_FILTERSPEC { pszName: w!("All files"), pszSpec: w!("*.*") },
        ];
        dlg.SetFileTypes(&filters).ok()?;
        dlg.SetTitle(title).ok()?;
        dlg.Show(Some(hwnd)).ok()?;
        let item = dlg.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

/// Tear down and rebuild every GPU resource after a device-removed event
/// (driver update, TDR, GPU switch), then reopen the same file and view.
fn rebuild(hwnd: HWND) {
    let restore = APP.with(|c| c.borrow_mut().take().and_then(|a| a.restore_info()));
    match App::new(hwnd) {
        Ok(mut a) => {
            if let Some(r) = restore {
                a.open_restore(r);
            }
            APP.with(|c| *c.borrow_mut() = Some(a));
            unsafe {
                let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(hwnd), None, false);
            }
        }
        Err(e) => {
            error_box(hwnd, &format!("The graphics device was lost and could not be recreated:\n{e}"));
            unsafe { PostQuitMessage(1) };
        }
    }
}

/// Work that must run outside the `App` borrow (modal UI, device rebuild).
fn after(hwnd: HWND) {
    let Some((err, lost, info, prompt, quit)) = with_app(|a| {
        (
            a.pending_error.take(),
            a.device_lost,
            a.pending_info.take(),
            a.pending_update_prompt.take(),
            a.quit_for_update,
        )
    }) else {
        return;
    };
    if quit {
        // The updated copy has been launched; close this one.
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        return;
    }
    if lost {
        rebuild(hwnd);
    }
    if let Some(e) = err {
        error_box(hwnd, &e);
    }
    if let Some(i) = info {
        unsafe {
            MessageBoxW(Some(hwnd), &HSTRING::from(i), w!("Red-Beam"), MB_OK | MB_ICONINFORMATION);
        }
    }
    if let Some(b) = prompt {
        let text = format!(
            "Red-Beam build {b} is available (you have {}).\n\nDownload and restart now? Your open drawing will reopen on the same sheet.",
            update::build_label()
        );
        let r = unsafe { MessageBoxW(Some(hwnd), &HSTRING::from(text), w!("Update Red-Beam"), MB_YESNO | MB_ICONQUESTION) };
        if r == IDYES {
            with_app(|a| a.start_download(b));
        }
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_APP_OPEN => {
                if let Some(p) = open_dialog(hwnd, w!("Open PDF")) {
                    with_app(|a| a.open(p));
                }
                after(hwnd);
                return LRESULT(0);
            }
            WM_APP_COMPARE => {
                if let Some(p) = open_dialog(hwnd, w!("Choose the EARLIER revision to compare against")) {
                    with_app(|a| a.start_compare(p));
                }
                after(hwnd);
                return LRESULT(0);
            }
            WM_DROPFILES => {
                let hdrop = HDROP(wp.0 as *mut _);
                let mut buf = vec![0u16; 32768];
                let n = DragQueryFileW(hdrop, 0, Some(&mut buf));
                DragFinish(hdrop);
                if n > 0 {
                    let p = PathBuf::from(OsString::from_wide(&buf[..n as usize]));
                    with_app(|a| a.open(p));
                }
                return LRESULT(0);
            }
            WM_DPICHANGED => {
                with_app(|a| a.set_dpi((wp.0 & 0xffff) as u32));
                let r = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                return LRESULT(0);
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_DESTROY => {
                APP.with(|c| {
                    if let Ok(mut g) = c.try_borrow_mut() {
                        g.take();
                    }
                });
                PostQuitMessage(0);
                return LRESULT(0);
            }
            _ => {}
        }
        let r = with_app(|a| a.handle(hwnd, msg, wp, lp)).flatten();
        after(hwnd);
        match r {
            Some(v) => LRESULT(v),
            None => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

fn main() -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).ok()?;

        let hinst = GetModuleHandleW(None)?;
        let class = w!("RedBeamWindow");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hIcon: LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: class,
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err(Error::from_thread());
        }
        let hwnd = CreateWindowExW(
            WS_EX_ACCEPTFILES,
            class,
            w!("Red-Beam"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            None,
            None,
            Some(hinst.into()),
            None,
        )?;

        match App::new(hwnd) {
            Ok(a) => APP.with(|c| *c.borrow_mut() = Some(a)),
            Err(e) => {
                error_box(hwnd, &format!("Could not initialise the GPU renderer:\n{e}"));
                return Err(e);
            }
        }
        let _ = ShowWindow(hwnd, SW_SHOWMAXIMIZED);
        // RedBeam.exe [file.pdf] [--page=N]
        let mut file = None;
        let mut page = 0u32;
        for a in std::env::args_os().skip(1) {
            match a.to_str().and_then(|s| s.strip_prefix("--page=")) {
                Some(n) => page = n.parse::<u32>().unwrap_or(1).saturating_sub(1),
                None => file = Some(PathBuf::from(a)),
            }
        }
        if let Some(p) = file {
            with_app(|a| a.open_at(p, page));
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        APP.with(|c| drop(c.borrow_mut().take()));
        CoUninitialize();
    }
    Ok(())
}

//! Crash reports. A Rust panic or a native crash (for example inside the
//! PDF engine) appends a report to %LOCALAPPDATA%\Red-Beam\crash.log, and a
//! native crash also writes crash.dmp next to it. The next start notices
//! the new report and offers to reopen the drawings that were open.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Diagnostics::Debug::{
    MiniDumpNormal, MiniDumpWriteDump, SetUnhandledExceptionFilter, EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION,
};
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId};

/// What the user was doing (file, sheet), included in reports.
static CONTEXT: Mutex<String> = Mutex::new(String::new());

pub fn dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Red-Beam")
}

pub fn log_path() -> PathBuf {
    dir().join("crash.log")
}

/// Update the "what was open" line of future reports.
pub fn set_context(s: String) {
    if let Ok(mut c) = CONTEXT.try_lock() {
        *c = s;
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Seconds since 1970 when crash.log was last written (0 if never).
pub fn last_report_time() -> u64 {
    std::fs::metadata(log_path())
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn append(report: &str) {
    let _ = std::fs::create_dir_all(dir());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log_path()) {
        let ctx = CONTEXT.try_lock().map(|c| c.clone()).unwrap_or_default();
        let _ = writeln!(
            f,
            "==== Red-Beam {} crash at unix time {} ====\nOpen: {}\n{}\n",
            crate::update::build_label(),
            now(),
            if ctx.is_empty() { "(nothing)" } else { &ctx },
            report
        );
    }
}

unsafe extern "system" fn on_native_crash(info: *const EXCEPTION_POINTERS) -> i32 {
    unsafe {
        let (code, addr) = info
            .as_ref()
            .and_then(|p| p.ExceptionRecord.as_ref())
            .map(|r| (r.ExceptionCode.0 as u32, r.ExceptionAddress as usize))
            .unwrap_or((0, 0));
        append(&format!("Native exception 0x{code:08X} at address 0x{addr:X}. A minidump was written to crash.dmp."));
        if let Ok(f) = std::fs::File::create(dir().join("crash.dmp")) {
            use std::os::windows::io::AsRawHandle;
            let ex = MINIDUMP_EXCEPTION_INFORMATION {
                ThreadId: GetCurrentThreadId(),
                ExceptionPointers: info as *mut EXCEPTION_POINTERS,
                ClientPointers: false.into(),
            };
            let _ = MiniDumpWriteDump(
                GetCurrentProcess(),
                GetCurrentProcessId(),
                HANDLE(f.as_raw_handle()),
                MiniDumpNormal,
                Some(&ex),
                None,
                None,
            );
        }
    }
    // EXCEPTION_CONTINUE_SEARCH: let Windows end the process as usual.
    0
}

/// Install the panic hook and the native crash handler.
pub fn install() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |p| {
        let bt = std::backtrace::Backtrace::force_capture();
        let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
        append(&format!("Panic on thread '{thread}': {p}\n\nBacktrace:\n{bt}"));
        default(p);
    }));
    unsafe {
        SetUnhandledExceptionFilter(Some(on_native_crash));
    }
}

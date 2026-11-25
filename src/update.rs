//! Self-update from GitHub Releases.
//!
//! CI publishes every build of `main` as a release tagged `build-<n>` with
//! `RedBeam.exe` attached, and bakes `<n>` into the executable
//! (`REDBEAM_BUILD`). The app compares its own number with the latest
//! release and downloads the new exe to %LOCALAPPDATA%\Red-Beam\update.
//!
//! Installing never renames or overwrites a running program (antivirus or
//! Windows can refuse that with "Access is denied"): the downloaded copy is
//! started with `--finish-update <target> <pid> [args…]`, the old app exits,
//! and the new copy waits for it, copies itself over the target (retrying
//! while antivirus holds the file) and relaunches it.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use windows::core::*;
use windows::Foundation::Uri;
use windows::Storage::Streams::DataReader;
use windows::Web::Http::Filters::{HttpBaseProtocolFilter, HttpCacheReadBehavior};
use windows::Web::Http::HttpClient;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_APP_UPDATE: u32 = WM_APP + 3;
const REPO: &str = "4wgm84mkd6-sketch/Red-Beam";

/// Build number baked in by CI; 0 for local/dev builds.
pub fn current_build() -> u32 {
    option_env!("REDBEAM_BUILD").and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

pub fn build_label() -> String {
    match current_build() {
        0 => "dev build".into(),
        n => format!("build {n}"),
    }
}

pub enum UpdateMsg {
    Checked { latest: Option<u32>, manual: bool, error: Option<String> },
    Downloaded { build: u32, path: PathBuf },
    DownloadFailed(String),
}

fn http_get(url: &str) -> Result<Vec<u8>> {
    let filter = HttpBaseProtocolFilter::new()?;
    filter.CacheControl()?.SetReadBehavior(HttpCacheReadBehavior::NoCache)?;
    let client = HttpClient::Create(&filter)?;
    let _ = client.DefaultRequestHeaders()?.UserAgent()?.TryParseAdd(&HSTRING::from("Red-Beam-Updater"))?;
    let resp = client.GetAsync(&Uri::CreateUri(&HSTRING::from(url))?)?.join()?;
    resp.EnsureSuccessStatusCode()?;
    let buf = resp.Content()?.ReadAsBufferAsync()?.join()?;
    let mut bytes = vec![0u8; buf.Length()? as usize];
    DataReader::FromBuffer(&buf)?.ReadBytes(&mut bytes)?;
    Ok(bytes)
}

/// Pull `"tag_name": "build-123"` out of the releases API response.
fn parse_latest(json: &str) -> Option<u32> {
    let i = json.find("\"tag_name\"")? + "\"tag_name\"".len();
    let rest = json[i..].trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    let tag = &rest[..rest.find('"')?];
    tag.strip_prefix("build-")?.parse().ok()
}

fn notify(hwnd: isize) {
    unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_APP_UPDATE, WPARAM(0), LPARAM(0));
    }
}

/// %LOCALAPPDATA%\Red-Beam\update
fn update_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Red-Beam").join("update")
}

/// Delete leftovers: downloaded installers and files from the old
/// rename-based updater. Files still in use are skipped and retried later.
fn cleanup() {
    let me = std::env::current_exe().ok();
    if let Ok(dir) = std::fs::read_dir(update_dir()) {
        for e in dir.flatten() {
            if Some(e.path()) != me {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    if let Some(cur) = &me {
        for ext in ["exe.old", "exe.download"] {
            let _ = std::fs::remove_file(cur.with_extension(ext));
        }
    }
}

/// Check the latest release in the background. `manual` = user clicked.
pub fn spawn_check(hwnd: HWND, tx: Sender<UpdateMsg>, manual: bool) {
    let hwnd = hwnd.0 as isize;
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        // Remove the installer left by the last update. The helper process
        // may still be exiting, so try again a little later.
        cleanup();
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(5));
            cleanup();
        });
        let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
        let msg = match http_get(&url) {
            Ok(body) => {
                let latest = parse_latest(&String::from_utf8_lossy(&body));
                let error = latest.is_none().then(|| "no published builds found".to_string());
                UpdateMsg::Checked { latest, manual, error }
            }
            Err(e) => UpdateMsg::Checked { latest: None, manual, error: Some(e.message().to_string()) },
        };
        let _ = tx.send(msg);
        notify(hwnd);
        unsafe { CoUninitialize() };
    });
}

/// Download build `build` next to the running exe.
pub fn spawn_download(hwnd: HWND, tx: Sender<UpdateMsg>, build: u32) {
    let hwnd = hwnd.0 as isize;
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let url = format!("https://github.com/{REPO}/releases/download/build-{build}/RedBeam.exe");
        let msg = (|| -> std::result::Result<PathBuf, String> {
            let dir = update_dir();
            std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
            let dl = dir.join(format!("RedBeam-build-{build}.exe"));
            let bytes = http_get(&url).map_err(|e| e.message().to_string())?;
            if bytes.len() < 64 * 1024 || !bytes.starts_with(b"MZ") {
                return Err("the downloaded file is not a valid program".into());
            }
            std::fs::write(&dl, &bytes).map_err(|e| format!("could not save {}: {e}", dl.display()))?;
            Ok(dl)
        })();
        let _ = tx.send(match msg {
            Ok(path) => UpdateMsg::Downloaded { build, path },
            Err(e) => UpdateMsg::DownloadFailed(e),
        });
        notify(hwnd);
        unsafe { CoUninitialize() };
    });
}

/// Hand over to the downloaded copy, which finishes the install after this
/// process exits. The caller should close the app on success.
pub fn install_and_restart(downloaded: &Path, args: &[String]) -> std::result::Result<(), String> {
    let cur = std::env::current_exe().map_err(|e| e.to_string())?;
    std::process::Command::new(downloaded)
        .arg("--finish-update")
        .arg(&cur)
        .arg(std::process::id().to_string())
        .args(args)
        .spawn()
        .map_err(|e| format!("could not start the new version: {e}"))?;
    Ok(())
}

/// Runs in the downloaded copy: `--finish-update <target> <pid> [args…]`.
/// Waits for the old app to exit, copies itself over `target` and starts it.
pub fn finish_update(args: &[std::ffi::OsString]) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

    let (Some(target), Some(pid)) = (args.first(), args.get(1).and_then(|p| p.to_str()?.parse::<u32>().ok())) else {
        return;
    };
    let target = PathBuf::from(target);
    let rest = &args[2..];
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            WaitForSingleObject(h, 30_000);
            let _ = CloseHandle(h);
        }
    }
    let me = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return,
    };
    // Antivirus often scans the old file for a moment after it closes.
    let mut last_err = None;
    for _ in 0..60 {
        match std::fs::copy(&me, &target) {
            Ok(_) => {
                last_err = None;
                break;
            }
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        }
    }
    if let Some(e) = last_err {
        let text = format!(
            "Red-Beam could not replace\n{}\n\n{e}\n\nThe new version will run from\n{}\nfor now. Moving RedBeam.exe to a folder such as Documents\\Red-Beam usually fixes this.",
            target.display(),
            me.display()
        );
        unsafe {
            MessageBoxW(None, &HSTRING::from(text), &HSTRING::from("Red-Beam update"), MB_OK | MB_ICONERROR);
        }
        // Run the new version from here so the update still takes effect.
        let _ = std::process::Command::new(&me).args(rest).spawn();
        return;
    }
    let _ = std::process::Command::new(&target).args(rest).spawn();
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_tag() {
        assert_eq!(super::parse_latest(r#"{"url":"x","tag_name":"build-57","name":"y"}"#), Some(57));
        assert_eq!(super::parse_latest("{\n  \"tag_name\": \"build-8\",\n}"), Some(8));
        assert_eq!(super::parse_latest(r#"{"tag_name":"v0.1.0"}"#), None);
    }
}

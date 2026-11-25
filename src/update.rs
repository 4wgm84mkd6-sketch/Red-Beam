//! Self-update from GitHub Releases.
//!
//! CI publishes every build of `main` as a release tagged `build-<n>` with
//! `RedBeam.exe` attached, and bakes `<n>` into the executable
//! (`REDBEAM_BUILD`). The app compares its own number with the latest
//! release, downloads the new exe next to itself, swaps the files (Windows
//! allows renaming a running exe) and restarts.

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

fn exe_paths() -> std::io::Result<(PathBuf, PathBuf, PathBuf)> {
    let cur = std::env::current_exe()?;
    Ok((cur.clone(), cur.with_extension("exe.download"), cur.with_extension("exe.old")))
}

/// Check the latest release in the background. `manual` = user clicked.
pub fn spawn_check(hwnd: HWND, tx: Sender<UpdateMsg>, manual: bool) {
    let hwnd = hwnd.0 as isize;
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        // Remove the previous version left behind by the last update.
        if let Ok((_, _, old)) = exe_paths() {
            if old.exists() && std::fs::remove_file(&old).is_err() {
                // The previous process may still be shutting down.
                std::thread::sleep(std::time::Duration::from_secs(4));
                let _ = std::fs::remove_file(&old);
            }
        }
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
            let (_, dl, _) = exe_paths().map_err(|e| e.to_string())?;
            let bytes = http_get(&url).map_err(|e| e.message().to_string())?;
            if bytes.len() < 64 * 1024 || !bytes.starts_with(b"MZ") {
                return Err("the downloaded file is not a valid program".into());
            }
            std::fs::write(&dl, &bytes).map_err(|e| format!("could not save next to the app ({}): {e}", dl.display()))?;
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

/// Swap the downloaded exe into place and launch it with `args`.
pub fn install_and_restart(downloaded: &Path, args: &[String]) -> std::result::Result<(), String> {
    let (cur, _, old) = exe_paths().map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&cur, &old).map_err(|e| format!("could not replace {}: {e}", cur.display()))?;
    if let Err(e) = std::fs::rename(downloaded, &cur) {
        let _ = std::fs::rename(&old, &cur);
        return Err(format!("could not install the new version: {e}"));
    }
    std::process::Command::new(&cur)
        .args(args)
        .spawn()
        .map_err(|e| format!("installed, but could not restart: {e}"))?;
    Ok(())
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

//! Per-user install (no admin rights): copies RedBeam.exe to
//! %LOCALAPPDATA%\Programs\Red-Beam, adds a Start menu shortcut, offers
//! Red-Beam in "Open with" for PDFs (and as a default-app choice), and
//! registers an uninstaller in Settings > Apps. Updates keep coming through
//! the in-app Update button, which replaces the installed exe in place.

use std::path::{Path, PathBuf};

use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD,
    REG_NONE, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::UI::Shell::{IShellLinkW, SHChangeNotify, ShellExecuteW, ShellLink, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

const APP: &str = "Red-Beam";
const PROGID: &str = "RedBeam.PDF";
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Red-Beam";
const CAPS_KEY: &str = r"Software\Red-Beam\Capabilities";

pub fn install_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Programs").join(APP)
}

pub fn installed_exe() -> PathBuf {
    install_dir().join("RedBeam.exe")
}

fn start_menu_link() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join(r"Microsoft\Windows\Start Menu\Programs").join(format!("{APP}.lnk"))
}

/// Is this process the installed copy?
pub fn running_installed() -> bool {
    std::env::current_exe().ok().is_some_and(|me| crate::settings::same_path(&me, &installed_exe()))
}

/// Is Red-Beam installed for this user?
pub fn is_installed() -> bool {
    installed_exe().exists() && key_exists(UNINSTALL_KEY)
}

fn key_exists(sub: &str) -> bool {
    let mut hk = HKEY::default();
    unsafe {
        let ok = RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(sub), None, KEY_READ, &mut hk) == ERROR_SUCCESS;
        if ok {
            let _ = RegCloseKey(hk);
        }
        ok
    }
}

/// Write a value under HKCU\`sub` (created if needed). `name` None = default.
fn set_value(sub: &str, name: Option<&str>, value: Value) -> Result<(), String> {
    let mut hk = HKEY::default();
    unsafe {
        let r = RegCreateKeyExW(HKEY_CURRENT_USER, &HSTRING::from(sub), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut hk, None);
        if r != ERROR_SUCCESS {
            return Err(format!("cannot write HKCU\\{sub}: {}", r.0));
        }
        let name_h = name.map(HSTRING::from);
        let name_p = name_h.as_ref().map(|h| PCWSTR(h.as_ptr())).unwrap_or(PCWSTR::null());
        let r = match value {
            Value::Str(s) => {
                let w: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
                let bytes = std::slice::from_raw_parts(w.as_ptr() as *const u8, w.len() * 2);
                RegSetValueExW(hk, name_p, None, REG_SZ, Some(bytes))
            }
            Value::Dword(d) => RegSetValueExW(hk, name_p, None, REG_DWORD, Some(&d.to_le_bytes())),
            Value::Empty => RegSetValueExW(hk, name_p, None, REG_NONE, None),
        };
        let _ = RegCloseKey(hk);
        if r != ERROR_SUCCESS {
            return Err(format!("cannot write HKCU\\{sub}: {}", r.0));
        }
    }
    Ok(())
}

enum Value<'a> {
    Str(&'a str),
    Dword(u32),
    Empty,
}

fn delete_tree(sub: &str) {
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(sub));
    }
}

/// Register `exe` with Windows: PDF "Open with", default-app capabilities,
/// and the uninstaller entry.
fn register(exe: &Path) -> Result<(), String> {
    let exe_s = exe.display().to_string();
    let open = format!("\"{exe_s}\" \"%1\"");
    let icon = format!("\"{exe_s}\",0");
    // ProgID: what a PDF opened with Red-Beam is.
    set_value(&format!(r"Software\Classes\{PROGID}"), None, Value::Str("PDF drawing (Red-Beam)"))?;
    set_value(&format!(r"Software\Classes\{PROGID}\DefaultIcon"), None, Value::Str(&icon))?;
    set_value(&format!(r"Software\Classes\{PROGID}\shell\open\command"), None, Value::Str(&open))?;
    // Offer Red-Beam in "Open with" for .pdf.
    set_value(r"Software\Classes\.pdf\OpenWithProgids", Some(PROGID), Value::Empty)?;
    set_value(r"Software\Classes\Applications\RedBeam.exe\shell\open\command", None, Value::Str(&open))?;
    set_value(r"Software\Classes\Applications\RedBeam.exe\SupportedTypes", Some(".pdf"), Value::Empty)?;
    set_value(r"Software\Classes\Applications\RedBeam.exe", Some("FriendlyAppName"), Value::Str(APP))?;
    // Default-apps capabilities (Settings > Apps > Default apps).
    set_value(CAPS_KEY, Some("ApplicationName"), Value::Str(APP))?;
    set_value(CAPS_KEY, Some("ApplicationDescription"), Value::Str("Fast GPU PDF viewer for construction drawing sets"))?;
    set_value(&format!(r"{CAPS_KEY}\FileAssociations"), Some(".pdf"), Value::Str(PROGID))?;
    set_value(r"Software\RegisteredApplications", Some(APP), Value::Str(CAPS_KEY))?;
    // Uninstaller (Settings > Apps > Installed apps).
    let size_kb = std::fs::metadata(exe).map(|m| (m.len() / 1024) as u32).unwrap_or(0);
    set_value(UNINSTALL_KEY, Some("DisplayName"), Value::Str(APP))?;
    set_value(UNINSTALL_KEY, Some("DisplayIcon"), Value::Str(&icon))?;
    set_value(UNINSTALL_KEY, Some("DisplayVersion"), Value::Str(&crate::update::build_label()))?;
    set_value(UNINSTALL_KEY, Some("Publisher"), Value::Str(APP))?;
    set_value(UNINSTALL_KEY, Some("InstallLocation"), Value::Str(&install_dir().display().to_string()))?;
    set_value(UNINSTALL_KEY, Some("UninstallString"), Value::Str(&format!("\"{exe_s}\" --uninstall")))?;
    set_value(UNINSTALL_KEY, Some("QuietUninstallString"), Value::Str(&format!("\"{exe_s}\" --uninstall --quiet")))?;
    set_value(UNINSTALL_KEY, Some("NoModify"), Value::Dword(1))?;
    set_value(UNINSTALL_KEY, Some("NoRepair"), Value::Dword(1))?;
    set_value(UNINSTALL_KEY, Some("EstimatedSize"), Value::Dword(size_kb))?;
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
    Ok(())
}

fn create_shortcut(exe: &Path, link: &Path) -> Result<(), String> {
    unsafe {
        let sl: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| e.message().to_string())?;
        let exe_h = HSTRING::from(exe.as_os_str());
        sl.SetPath(&exe_h).map_err(|e| e.message().to_string())?;
        sl.SetIconLocation(&exe_h, 0).map_err(|e| e.message().to_string())?;
        sl.SetDescription(&HSTRING::from("Red-Beam PDF viewer")).map_err(|e| e.message().to_string())?;
        if let Some(dir) = exe.parent() {
            sl.SetWorkingDirectory(&HSTRING::from(dir.as_os_str())).map_err(|e| e.message().to_string())?;
        }
        let pf: IPersistFile = sl.cast().map_err(|e| e.message().to_string())?;
        if let Some(dir) = link.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        pf.Save(&HSTRING::from(link.as_os_str()), true).map_err(|e| e.message().to_string())?;
    }
    Ok(())
}

/// Install (or reinstall) this exe for the current user. Returns the
/// installed exe's path. Needs COM initialised on this thread.
pub fn install() -> Result<PathBuf, String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let dst = installed_exe();
    std::fs::create_dir_all(install_dir()).map_err(|e| format!("cannot create {}: {e}", install_dir().display()))?;
    if !crate::settings::same_path(&me, &dst) {
        let mut last = None;
        for _ in 0..20 {
            match std::fs::copy(&me, &dst) {
                Ok(_) => {
                    last = None;
                    break;
                }
                Err(e) => {
                    last = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            }
        }
        if let Some(e) = last {
            return Err(format!("cannot copy Red-Beam to {}: {e}", dst.display()));
        }
    }
    register(&dst)?;
    create_shortcut(&dst, &start_menu_link())?;
    Ok(dst)
}

/// Remove the registration, shortcut and installed files. The settings
/// and takeoff files are kept. The install folder is deleted a moment
/// after this process exits (it can't delete its own running exe).
pub fn uninstall() -> Result<(), String> {
    for k in [
        format!(r"Software\Classes\{PROGID}"),
        r"Software\Classes\Applications\RedBeam.exe".to_string(),
        r"Software\Red-Beam".to_string(),
        UNINSTALL_KEY.to_string(),
    ] {
        delete_tree(&k);
    }
    unsafe {
        use windows::Win32::System::Registry::RegDeleteKeyValueW;
        let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(r"Software\Classes\.pdf\OpenWithProgids"), &HSTRING::from(PROGID));
        let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(r"Software\RegisteredApplications"), &HSTRING::from(APP));
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
    let _ = std::fs::remove_file(start_menu_link());
    let dir = install_dir();
    if running_installed() {
        // cmd waits ~3 s for this process to exit, then removes the folder.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let script = format!("ping -n 4 127.0.0.1 >nul & rmdir /s /q \"{}\"", dir.display());
        std::process::Command::new("cmd").args(["/c", &script]).creation_flags(CREATE_NO_WINDOW).spawn().map_err(|e| e.to_string())?;
    } else if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("cannot remove {}: {e}", dir.display()))?;
    }
    Ok(())
}

/// Open Windows' Default apps page at Red-Beam's entry, where the user can
/// make it the default for PDFs (apps can't set that themselves).
pub fn open_default_apps() {
    unsafe {
        let target = HSTRING::from(format!("ms-settings:defaultapps?registeredAppUser={APP}"));
        ShellExecuteW(None, &HSTRING::from("open"), &target, None, None, SW_SHOWNORMAL);
    }
}

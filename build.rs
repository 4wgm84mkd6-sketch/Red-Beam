//! Build steps:
//! * Embeds pdfium.dll (prebuilt from bblanchon/pdfium-binaries) into the
//!   exe when available, so Red-Beam stays a single self-updating file. CI
//!   downloads it to vendor/pdfium; locally set PDFIUM_DLL or do the same.
//! * Compiles the Windows resources: app icon, version info and manifest.

use std::path::{Path, PathBuf};

/// Icon, version info (with the CI build number) and manifest.
fn resources(manifest_dir: &Path) {
    println!("cargo::rerun-if-changed=assets/redbeam.ico");
    println!("cargo::rerun-if-changed=assets/redbeam.manifest");
    println!("cargo::rerun-if-env-changed=REDBEAM_BUILD");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // MSVC's resource compiler only exists on Windows; when type-checking the
    // MSVC target from another OS, skip resources (real builds embed them).
    let host_windows = std::env::var("HOST").is_ok_and(|h| h.contains("windows"));
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") && !host_windows {
        println!("cargo::warning=skipping Windows resources (MSVC target on a non-Windows host)");
        return;
    }
    let build: u32 = std::env::var("REDBEAM_BUILD").ok().and_then(|b| b.trim().parse().ok()).unwrap_or(0);
    let esc = |p: PathBuf| p.display().to_string().replace('\\', "\\\\");
    let icon = esc(manifest_dir.join("assets").join("redbeam.ico"));
    let manifest = esc(manifest_dir.join("assets").join("redbeam.manifest"));
    let label = if build == 0 { "dev build".to_string() } else { format!("build {build}") };
    let rc = format!(
        r#"1 ICON "{icon}"
1 24 "{manifest}"
1 VERSIONINFO
FILEVERSION 0,1,{build},0
PRODUCTVERSION 0,1,{build},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Red-Beam"
      VALUE "FileDescription", "Red-Beam - GPU PDF viewer, takeoff and compare"
      VALUE "FileVersion", "{label}"
      VALUE "InternalName", "RedBeam"
      VALUE "OriginalFilename", "RedBeam.exe"
      VALUE "ProductName", "Red-Beam"
      VALUE "ProductVersion", "{label}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("redbeam.rc");
    std::fs::write(&out, rc).unwrap();
    embed_resource::compile(&out, embed_resource::NONE).manifest_required().unwrap();
}

fn main() {
    println!("cargo::rerun-if-env-changed=PDFIUM_DLL");
    println!("cargo::rustc-check-cfg=cfg(has_pdfium)");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    resources(&manifest);
    let default = manifest.join("vendor").join("pdfium").join("bin").join("pdfium.dll");
    let dll = std::env::var_os("PDFIUM_DLL").map(PathBuf::from).unwrap_or(default);
    println!("cargo::rerun-if-changed={}", dll.display());
    if dll.is_file() {
        println!("cargo::rustc-cfg=has_pdfium");
        println!("cargo::rustc-env=PDFIUM_DLL_PATH={}", dll.display());
    } else {
        println!(
            "cargo::warning=pdfium.dll not found at {} - building with the Windows PDF engine only",
            dll.display()
        );
    }
}

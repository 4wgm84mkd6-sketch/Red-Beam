//! Embeds pdfium.dll (prebuilt from bblanchon/pdfium-binaries) into the exe
//! when it is available, so Red-Beam stays a single self-updating file.
//! CI downloads it to vendor/pdfium; locally set PDFIUM_DLL or do the same.

use std::path::PathBuf;

fn main() {
    println!("cargo::rerun-if-env-changed=PDFIUM_DLL");
    println!("cargo::rustc-check-cfg=cfg(has_pdfium)");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
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

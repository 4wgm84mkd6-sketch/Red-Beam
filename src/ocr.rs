//! OCR for scanned sheets, using the engine built into Windows 10 and 11
//! (Windows.Media.Ocr). Sheets with no text layer are rendered at about
//! 200 DPI in tiles and read, so search, sheet names and sheet links work
//! on them too.

use std::cell::RefCell;

use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Security::Cryptography::CryptographicBuffer;

thread_local! {
    /// One engine per worker thread (None inside: no OCR language installed).
    static ENGINE: RefCell<Option<Option<OcrEngine>>> = const { RefCell::new(None) };
}

fn engine() -> Option<OcrEngine> {
    ENGINE.with(|e| {
        e.borrow_mut().get_or_insert_with(|| OcrEngine::TryCreateFromUserProfileLanguages().ok()).clone()
    })
}

/// Is OCR available (an OCR language is installed)?
pub fn available() -> bool {
    engine().is_some()
}

/// Largest image side the engine accepts.
pub fn max_dim() -> u32 {
    OcrEngine::MaxImageDimension().unwrap_or(2600)
}

/// Words in a BGRA image: text and box [l, t, r, b] in pixels.
pub fn recognize(bgra: &[u8], w: u32, h: u32) -> Result<Vec<(String, [f32; 4])>, String> {
    let engine = engine().ok_or("no OCR language is installed in Windows")?;
    let run = || -> windows::core::Result<Vec<(String, [f32; 4])>> {
        let buf = CryptographicBuffer::CreateFromByteArray(bgra)?;
        let bmp = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(&buf, BitmapPixelFormat::Bgra8, w as i32, h as i32, BitmapAlphaMode::Premultiplied)?;
        let result = engine.RecognizeAsync(&bmp)?.join()?;
        let mut out = Vec::new();
        for line in result.Lines()? {
            for word in line.Words()? {
                let r = word.BoundingRect()?;
                out.push((word.Text()?.to_string(), [r.X, r.Y, r.X + r.Width, r.Y + r.Height]));
            }
        }
        Ok(out)
    };
    run().map_err(|e| e.message().to_string())
}

# Red-Beam

Red-Beam is a fast Windows PDF viewer for large construction drawing sets. Pages are rendered on the GPU, so huge vector-heavy sheets (Revit exports, for example) stay smooth when you pan, zoom and flip between sheets.

> **About Revit files:** Red-Beam opens **PDFs**, including the very large sheets that Revit exports. It can't open native `.rvt` models. Reading those needs Autodesk's own software or a commercially licensed SDK.

## Download

* Every push to `main` builds `RedBeam.exe` on GitHub Actions. Open **Actions → Build → latest run → Artifacts → RedBeam-windows-x64**.
* Pushing a tag such as `v0.1.0` also attaches the exe to a GitHub Release.

`RedBeam.exe` is a single file with no installer. It only uses DLLs built into Windows 10 and 11.

## Using it

| Action | Input |
|---|---|
| Open a PDF | `Ctrl+O`, the **Open** button, drag and drop, or `RedBeam.exe file.pdf` |
| Zoom at the cursor | Mouse wheel |
| Pan | Left- or middle-drag, the arrow keys, `Shift+wheel` (horizontal), `Ctrl+wheel` (vertical) |
| Next or previous sheet | `PgDn` / `PgUp`, `N` / `P`, ◀ ▶ |
| First or last sheet | `Home` / `End` |
| Go to a sheet | Type the number, then `Enter` |
| Fit page / fit width | `F` or `Ctrl+0` / `W` |
| Actual size (100%) | `Ctrl+1` |
| Zoom in / out | `+` / `−` |
| Performance readout | `F2` |
| Help | `F1` or `?` |

When you flip between sheets of the same size, Red-Beam keeps your zoom and position. You can compare the same detail across a drawing set by holding `PgDn`.

## How it works

```
 UI thread (its own D3D11 device)          Render workers ×2–3 (each: own D3D11 device,
 ────────────────────────────────           own Direct2D context, own copy of the PDF)
 DXGI flip swap chain, vsync-paced          ──────────────────────────────────────────
 composition of cached GPU bitmaps  ◀────── Windows.Data.Pdf → IPdfRendererNative →
   ├─ whole-sheet "base" images              Direct2D → shared GPU texture (by handle)
   └─ sharp "detail" views (cache)
```

* **The screen never waits on rendering.** Each render worker has its own GPU device and its own copy of the document. Finished pages are handed to the UI as shared GPU textures, which are never copied through system memory. Panning and zooming are vsync-paced GPU composition of what's already cached.
* **Parallel rendering.** Two or three workers rasterise in parallel, depending on CPU cores. Background work (whole-sheet images, neighbouring sheets) can never take every worker, so the sharp render for the current view starts immediately.
* **Whole-sheet images.** Nearby sheets keep a whole-page image in GPU memory, sized to the card: 2048, 4096 or 8192 px on the long edge. Flipping sheets and fast zooms show those instantly.
* **Sharp views.** About 140 ms after the view stops moving, the visible area is rendered at exact screen resolution and drawn 1:1. Sharp views are cached, up to about 1/6 of GPU memory, so returning to a view is instant.
* **Rendering beyond the screen.** Each render costs about the same however much area it covers, because the PDF engine processes the whole sheet's content every time. So on cards with plenty of memory, each sharp render covers well beyond the screen edges and pans reuse it.
* **Neighbouring sheets.** Once a view is sharp, the same view on the next and previous same-size sheets is rendered too, so flipping between sheets stays sharp.
* **Animated zoom.** Wheel notches accumulate into one smooth zoom.
* **GPU choice.** It picks the high-performance GPU on multi-GPU systems, rebuilds after a GPU reset, and falls back to WARP if no hardware GPU is present.

Press **F2** for a performance readout: frame rate, video memory, busy workers, cache contents and the last render time.

## Building from source

You need Rust (stable), installed from <https://rustup.rs>.

On Windows (MSVC):

```bash
cargo build --release
```

The executable is written to `target\release\RedBeam.exe`.

To cross-compile from Linux, install the `gcc-mingw-w64-x86-64` package, then run:

```bash
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
```

## Code map

| File | Purpose |
|---|---|
| `src/main.rs` | Window creation, message loop, Open dialog, drag and drop, GPU device-loss rebuild |
| `src/app.rs` | View and document state, render-request scheduling, input handling, composition and status bar |
| `src/worker.rs` | Render worker pool: each worker has a private GPU device and loads its own copy of the PDF; renders into shared textures |
| `src/gfx.rs` | Direct3D 11, Direct2D and DXGI swap-chain setup |
| `src/view.rs` | Camera math: zoom about a point, fit page or width, pan clamping |

## Known limitations

* Viewing only for now: no markups, measurements or text search yet.
* Password-protected PDFs aren't supported.
* Opening a file reads every page size up front. Sets with several thousand sheets take a moment to open.
* Rendering quality and speed depend on the `Windows.Data.Pdf` engine built into Windows.

## Roadmap ideas (Bluebeam-style)

* Thumbnail and sheet-index sidebar, plus continuous-scroll mode
* View rotation
* Markups (cloud, callout, text, highlight) saved back into the PDF as standard annotations
* Calibrated measurements (length, area, count) using the sheet's scale
* Sheet overlay and compare (revision A vs. B in red and blue)
* Text search, and hyperlinks between sheets
* Tabs for several open documents

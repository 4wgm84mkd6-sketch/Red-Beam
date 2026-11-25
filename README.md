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
| Help | `F1` or `?` |

When you flip between sheets of the same size, Red-Beam keeps your zoom and position. You can compare the same detail across a drawing set by holding `PgDn`.

## How the GPU is used

```
 UI thread                          Render thread (pdf-render)
 ─────────                          ──────────────────────────
 Win32 window + DXGI flip swapchain Windows.Data.Pdf document (owned here)
 Direct2D composition each frame    Direct2D device context (multithreaded geometry)
   ├─ page "base" bitmaps  ◀──────── IPdfRendererNative::RenderPageToDeviceContext
   └─ viewport "detail" bitmap ◀──── rasterises vectors straight into GPU bitmaps
              ▲                                    ▲
              └──────── one shared ID3D11Device ───┘   (bitmaps never leave VRAM)
```

* **Vector rasterisation on the GPU.** Windows' built-in PDF engine (`Windows.Data.Pdf`) draws the page through Direct2D on a hardware Direct3D 11 device. The resulting bitmaps are GPU textures and are never copied to system memory.
* **Two layers.** Each nearby page keeps a whole-page **base** bitmap (up to 4096 px on its long edge) in VRAM. Pan, zoom and page flips are just GPU transforms of that bitmap, so they're instant. About 140 ms after you stop moving, the render thread draws a **detail** bitmap of the visible area at the exact screen resolution, plus 25% padding on each side. It's drawn 1:1 with no resampling, so lines stay crisp at any zoom up to 6400%.
* **Prefetch.** The next and previous two sheets are rendered in the background, so flipping through sheets is immediate.
* **A responsive UI.** All PDF work happens on a background thread. The UI thread only composites the cached bitmaps.
* **Recovery.** If the GPU resets (driver update, TDR or GPU switch), every device resource is rebuilt and the same file and view are reopened. If no hardware GPU is present, Red-Beam falls back to WARP, Microsoft's software Direct3D renderer.

The status bar shows which GPU is in use and how long the last detail render took.

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
| `src/worker.rs` | Render thread: loads the PDF and rasterises page regions into GPU bitmaps |
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

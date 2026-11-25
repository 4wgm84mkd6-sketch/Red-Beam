# Red-Beam

Red-Beam is a fast Windows PDF viewer for large construction drawing sets. Pages are rendered on the GPU, so huge vector-heavy sheets (Revit exports, for example) stay smooth when you pan, zoom and flip between sheets.

> **About Revit files:** Red-Beam opens **PDFs**, including the very large sheets that Revit exports. It can't open native `.rvt` models. Reading those needs Autodesk's own software or a commercially licensed SDK.

## Download and updates

* **First install:** open the [latest release](https://github.com/4wgm84mkd6-sketch/Red-Beam/releases/latest) and download `RedBeam.exe`. It's a single file with no installer, and it only uses DLLs built into Windows 10 and 11. Put it in a folder you can write to, such as Documents or Desktop, not Program Files.
* **Updates:** every push to `main` is built on GitHub Actions and published as a release named `build-<n>`. Red-Beam checks for a newer build at startup. When there is one, the **Update ●** button in the status bar turns red. Click it, confirm, and the app downloads the new version, replaces itself and reopens your drawing on the same sheet. Clicking **Update** at any other time checks again.

## Using it

| Action | Input |
|---|---|
| Open a PDF | `Ctrl+O`, the **Open** button, drag and drop, or `RedBeam.exe file.pdf` |
| Zoom at the cursor | Mouse wheel |
| Pan | Left- or middle-drag, the arrow keys, `Shift+wheel` (horizontal), `Ctrl+wheel` (vertical) |
| Next or previous sheet | `Ctrl+→` / `Ctrl+←`, `PgDn` / `PgUp`, `N` / `P`, ◀ ▶ |
| First or last sheet | `Home` / `End` |
| Go to a sheet | Type the number, then `Enter` |
| Fit page / fit width | `F` or `Ctrl+0` / `W` |
| Actual size (100%) | `Ctrl+1` |
| Zoom in / out | `+` / `−` |
| Takeoff tools: length / polyline / area / count | `L` / `Y` / `A` / `C` |
| Select tool, cancel | `V`, `Esc` |
| Finish polyline / area / count | `Enter`, double-click or right-click |
| Straight lines while measuring | hold `Shift` |
| Undo / delete selected | `Ctrl+Z` / `Del` |
| Quantities panel | `Q` |
| Compare with an earlier revision | `Ctrl+K` or **Compare** |
| Compare: overlay → old → new | `Tab` (or the **Overlay / Old / New** buttons) |
| Compare: shift which old sheet is paired | `[` / `]` |
| Performance readout | `F2` |
| Switch PDF engine (PDFium / Windows built-in) | `F4` |
| Help | `F1` or `?` |

When you flip between sheets of the same size, Red-Beam keeps your zoom and position. You can compare the same detail across a drawing set by holding `PgDn`.

### Takeoffs

The takeoff toolbar sits at the top-left of the drawing.

1. **Scale ▾:** pick the sheet's scale (1/8" = 1'-0", 1" = 20', …), or choose **Calibrate by two points…**, click both ends of a known dimension and type its length (`24'-6"`, `24.5'`, `294"`, `7.5m`). **Apply this sheet's scale to all sheets** sets the whole set at once.
2. **Group ▾:** choose or create the group you're measuring, such as *Drywall* or *Flooring*. Each group has its own colour. Selecting a measurement and then picking a group moves it to that group.
3. Measure:
   * **Length** (`L`): two clicks.
   * **Polyline** (`Y`): click each point, then press Enter, double-click or right-click to finish.
   * **Area** (`A`): click the corners, then finish. Shows the area in SF and its perimeter.
   * **Count** (`C`): click each item, then finish.
   * Hold **Shift** for straight 0°/45°/90° lines. **Ctrl+Z** undoes the last point or measurement. **Esc** cancels.
4. **Select** (`V`) to edit:
   * **Move:** drag a measurement.
   * **Reshape:** drag its square handles. Hold Shift to keep a point square to its neighbour.
   * **Add a point:** double-click an edge. A length with an added point becomes a polyline.
   * **Remove a point or count marker:** right-click it.
   * **Leaders:** drag a label off its measurement and a leader line connects it back.
   * **Properties:** right-click a measurement to set its group, layer, line ends and leader style, put the label back, duplicate it or delete it.
   * **Copy and paste:** **Ctrl+C**, **Ctrl+V** (pastes under the cursor, on any sheet) and **Ctrl+D** (duplicate). **Ctrl+Z** undoes edits.
   * **Line ends and leaders:** none, closed arrow, open arrow, architectural tick, dot, square or circle. The style chosen last becomes the default for new measurements.
5. **Layer ▾:** each measurement sits on a layer, such as *Base bid*, *Alt 1* or *Existing*. Choose which layer you're drawing on, and show/hide, lock, rename, delete or create layers. **Hidden layers are left out of the quantities and CSV totals**, so toggling an alternate shows its effect on the totals. Locked layers can't be edited by accident.
6. **Quantities** (`Q`): totals per group (length, area, perimeter, count) across all sheets, with warnings for sheets that have no scale.
7. **Export CSV:** one row per measurement, with group and layer, plus group totals for visible layers. It opens directly in Excel.
8. **Units ▾:** feet-inches (default), decimal feet, or metric.

Takeoffs save automatically to `<drawing name>.redbeam.json` next to the PDF. If that folder is read-only, they go to `%LOCALAPPDATA%\Red-Beam\takeoffs`. **The PDF itself is never modified.** Quantities are computed from each sheet's current scale, so correcting a calibration updates every measurement on that sheet.

### Comparing revisions

1. Open the **current** set, then click **Compare** (or press `Ctrl+K`) and pick the **earlier** revision.
2. The overlay shows:
   * **red:** only in the old revision (removed)
   * **blue:** only in the new revision (added)
   * **black:** unchanged
3. Press `Tab`, or use the **Overlay / Old / New** buttons, to flip between views at the same zoom and position.
4. Sheets pair by position: sheet 5 is compared with sheet 5. If sheets were added or removed between issues, press `[` or `]` to shift the pairing. The status bar shows which old sheet is paired.
5. Sheets are aligned at their top-left corners. That's correct for Revit sets with the same title block. Point-based alignment for re-scaled or shifted sheets is on the roadmap.

The two sheets are rendered separately, tinted on the GPU and blended with a darken (per-channel minimum) blend, so overlays stay smooth at any zoom.

## How it works

```
 UI thread (its own D3D11 device)          Render worker (own D3D11 device)
 ────────────────────────────────           ────────────────────────────────
 DXGI flip swap chain, vsync-paced          PDF engine → pixels → shared GPU texture
 composition of cached GPU bitmaps  ◀─────    • PDFium (default, embedded)
   ├─ whole-sheet "base" images               • Windows.Data.Pdf via Direct2D (F4)
   └─ sharp "detail" views (cache)
```

* **The GPU does everything you see.** Panning, zooming (animated, vsync-paced) and sheet flips are GPU composition of images already cached in video memory. The screen never waits for a render.
* **The PDF engine is the slow part, not the GPU.** Field data on a 72 MB, 18-sheet Revit set: Windows' built-in engine took 2–9 s per render while the GPU sat at about 3%, and it got slower when asked to render in parallel. So the default engine is now **PDFium**, the engine in Chrome and Edge:
  * Each sheet is parsed once and kept loaded (up to 6 sheets), so later renders only draw.
  * Only content inside the requested area is drawn, so zoomed-in views are cheap.
  * Renders are progressive and can be abandoned. When you move on, a stale render is cancelled immediately instead of blocking the next one.
  * Windows' engine is still available with **F4**, for comparison.
* **Whole-sheet images.** Nearby sheets keep a whole-page image (up to 4096 px) in GPU memory for instant flips and fast zooms.
* **Sharp views.** About 140 ms after the view stops moving, the visible area plus a margin is rendered at exact screen resolution and drawn 1:1. Sharp views are cached, so returning to a view is instant. Once a view is sharp, the same view on the neighbouring same-size sheets is pre-rendered.
* **The on-screen view goes first.** Background work never starts while the sharp render of the view is waiting, and it's cancelled if the view needs one.
* **GPU choice.** It picks the high-performance GPU on multi-GPU systems, rebuilds after a GPU reset, and falls back to WARP if no hardware GPU is present.

Press **F2** for a performance readout: engine, frame rate, video memory, cache contents, and a log of recent renders with their times.

## Building from source

You need Rust (stable), installed from <https://rustup.rs>.

On Windows (MSVC):

```bash
cargo build --release
```

The executable is written to `target\release\RedBeam.exe`.

PDFium is embedded when `vendor/pdfium/bin/pdfium.dll` exists, or when the `PDFIUM_DLL` environment variable points at one. Download `pdfium-win-x64.tgz` from [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries/releases/tag/chromium%2F8179) and extract it to `vendor/pdfium`. Without it, the build falls back to the Windows engine only.

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
| `src/worker.rs` | Render worker: private GPU device, PDFium or Windows engine, priority queue with cancellation, shared textures to the UI |
| `src/update.rs` | Checks GitHub Releases for a newer build, downloads it, swaps the exe and restarts |
| `src/pdfium.rs` | Loads the embedded PDFium DLL; parses sheets once and renders regions progressively (cancellable) |
| `src/takeoff.rs` | Takeoff model: scales, units, length parsing and formatting, geometry, autosave and CSV (unit-tested) |
| `src/app/tools.rs` | Takeoff UI: tool palette, measuring tools, selection and undo, quantities panel, menus, prompt |
| `src/gfx.rs` | Direct3D 11, Direct2D and DXGI swap-chain setup |
| `src/view.rs` | Camera math: zoom about a point, fit page or width, pan clamping |

## Known limitations

* Viewing only for now: no markups, measurements or text search yet.
* Password-protected PDFs aren't supported.
* Opening a file reads every page size up front. Sets with several thousand sheets take a moment to open.

## Roadmap (goal: replace Bluebeam)

1. **Revision compare:** overlay, old/new toggle and sheet pairing are done. Next: point-based alignment, sheet-number matching, and a list of changed areas.
2. **Takeoffs:** scales and calibration, length, polyline, area, perimeter, count, groups, a quantities panel and CSV export are done. Next: area deductions (cut-outs), editing points, volume (area × depth), per-viewport scales, snapping to drawing geometry, and a native Excel export.
3. **Markups:** cloud, callout, text, arrow, shapes, highlight and stamps, saved as standard PDF annotations that Bluebeam and Acrobat can read; a markups list; undo.
4. **Navigation:** sheet sidebar with thumbnails, text search, clickable sheet callouts, tabs.
5. **Rollout:** installer, "Open with Red-Beam", shared tool sets.

## Third-party software

Red-Beam embeds [PDFium](https://pdfium.googlesource.com/pdfium/) (BSD-3-Clause), built by [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries) (Apache-2.0 build scripts). PDFium includes third-party components such as FreeType, libjpeg-turbo, OpenJPEG, LittleCMS, zlib and AGG, each under its own permissive license. The full license texts ship in the `licenses/` folder of the PDFium release archive.

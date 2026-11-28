<img src="assets/redbeam-256.png" width="96" align="right" alt="Red-Beam logo">

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
| Takeoff tools: length / polyline / area / volume / count | `L` / `Y` / `A` / `U` / `C` |
| Cut-out in the selected area | `X` |
| Snap to line work on/off (Alt = place freely) | `S` |
| Select tool, cancel | `V`, `Esc` |
| Finish polyline / area / count | `Enter`, double-click or right-click |
| Straight lines while measuring | hold `Shift` |
| Undo / delete selected | `Ctrl+Z` / `Del` |
| Quantities panel | `Q` |
| Compare with an earlier revision | `Ctrl+K` or **Compare** |
| Compare: overlay → old → new | `Tab` (or the **Overlay / Old / New** buttons) |
| Compare: shift which old sheet is paired | `[` / `]` |
| Compare: previous / next detected change | `,` / `.` |
| Side panel: pages, files, markups, find, settings | `Ctrl+B` or **☰** |
| Next tab / previous tab / close tab | `Ctrl+Tab` / `Ctrl+Shift+Tab` / `Ctrl+W` |
| Split view: two sheets of this set side by side | `Ctrl+Shift+K` |
| Find text in the set / next / previous result | `Ctrl+F` / `F3` / `Shift+F3` |
| Go to a sheet by number (A-201), title or position | `Ctrl+G` |
| Print with markups and takeoffs | `Ctrl+P` |
| Performance readout | `F2` |
| Switch PDF engine (PDFium / Windows built-in) | `F4` |
| Next / previous discipline (A, S, M, E, P…) | `Ctrl+Shift+PgDn` / `Ctrl+Shift+PgUp` |
| Help | `F1` or `?`; hover any button for a tooltip |

When you flip between sheets of the same size, Red-Beam keeps your zoom and position. You can compare the same detail across a drawing set by holding `PgDn`.

### Sheet names, search and links

* **Sheet names:** Red-Beam names each sheet from the PDF's bookmarks (Revit exports bookmark every sheet, e.g. "A101 - FLOOR PLAN"), then from page labels, and otherwise from the largest sheet number in the title block. Names show under the thumbnails and in the status bar.
* **Find** (`Ctrl+F`): searches every sheet's text for a word or phrase. Hits are highlighted, and `F3` / `Shift+F3` zoom to each in turn. The **Find** tab lists all results by sheet; click one to go to it.
* **Go to sheet** (`Ctrl+G`): type a sheet number (`A-201`), part of a title (`roof`) or a position (`12`).
* **Sheet links:** sheet numbers printed on a drawing, such as the `A-501` in a detail bubble, get a faint blue outline. Hover to see where a link goes, and click (Select tool) to jump there.
* All of this needs the PDFium engine. Text is read in the background, starting with the sheet you're on.

### Side panel

Press **Ctrl+B** or click **☰** in the status bar to open the panel on the left. It has three tabs:

* **Pages:** a scrolling strip of thumbnails. Click one to jump to that sheet. The current sheet is highlighted, and sheets with takeoffs show how many. Thumbnails render in the background, and sheets already cached show instantly.
  * **Discipline jump bar** at the top: one chip per discipline found in the sheet numbers (**G**, **A**, **S**, **M**, **E**, **P**…) with its sheet count. Click a chip to jump to that discipline's first sheet. **Ctrl+Shift+PgDn / PgUp** steps to the next or previous discipline.
  * **List / Thumbs:** the **List** chip switches to a compact list with one row per sheet and a discipline badge, so a 140-sheet set fits in a few screens.
* **Files:** **Pinned** files, then **Recent** files, each with its folder. Click ☆ to pin a file. Clicking a file opens it on the sheet you were last viewing. Files that have moved or been deleted are greyed out.
* **Settings:** click a setting to change it:
  * mouse wheel zooms (default) or scrolls (Ctrl+wheel then zooms)
  * interface size (90–130%)
  * background colour
  * takeoff units
  * snap on by default
  * reopen last file at startup
  * check for updates at startup
  * PDF engine
  * performance readout

Settings, recent and pinned files are saved in `%LOCALAPPDATA%\Red-Beam\settings.json`.

### Markups

Click **Markup** at the left of the toolbar, or press `M`, to switch to the markup tools:

| Tool | How | Key |
|---|---|---|
| **Cloud** | Drag a box, or click around an irregular area and press Enter | `C` |
| **Text** | Drag a box (or click), then type | `T` |
| **Callout** | Press on the point to call out, drag to where the text goes, then type | `K` |
| **Arrow / Line** | Drag, or click the start then the end | `A` / `L` |
| **Rect / Ellipse / Highlight** | Drag a box | `R` / `E` / `H` |
| **Pen** | Draw freehand | |
| **Stamp ▾** | Click where it goes and pick APPROVED, REVIEWED, REVISE & RESUBMIT, REJECTED, FOR CONSTRUCTION, … | |

* **Color ▾ / Width ▾ / Font ▾** set the style for new markups. These settings are remembered.
* **Format bar:** select a markup and a second toolbar row appears:
  * **A− / size ▾ / A+** for text size, **B** and **I** for bold and italic
  * **Text ▾**, **Border ▾** and **Fill ▾** colours (fill can be see-through)
  * **Width ▾** and **Status ▾**

  Changes apply to every selected markup at once and undo in one step.
  * **Select several:** Ctrl+click markups, or **Ctrl+A** for all markups on the sheet. **Select same** picks every markup of the same kind on the sheet, e.g. all callouts.
  * **Make default:** new markups look like the selected one.
  * **Shortcuts** with text selected: **Ctrl+B** bold, **Ctrl+I** italic, **Ctrl+[** / **Ctrl+]** smaller / bigger.
* **Text:** text boxes and callouts take long text (up to 4,000 characters). **Shift+Enter** starts a new line and **Ctrl+V** pastes. The box wraps the text; drag its corners to resize it.
* With **Select**, you can drag markups to move them and drag their handles to resize. Double-click a text markup to edit it. Right-click a markup for its **status** (Open / Accepted / Rejected / Completed), colour, width, font size, duplicate and delete. **Ctrl+D** duplicates, **Del** deletes, and **Ctrl+Z** undoes.
* The **Markups** tab in the side panel lists every markup by sheet, with author and status. Click one to jump to it.
* Markups save automatically with the takeoff file next to the PDF.
* **Save PDF ▾:**
  * **Save a copy with markups** (optionally with takeoffs too) writes a new PDF with everything as real PDF annotations, each carrying its own appearance. They show in Bluebeam, Acrobat, Edge and Chrome, and can still be moved there.
  * **Save a flattened copy** burns markups and takeoffs into the drawing content, for issuing.
  * **Your original drawing is never modified.**
* **Print** (`Ctrl+P`, or Save PDF ▾ → Print): the standard Windows print dialog, with page range and copies. Each sheet is fitted and auto-rotated to the paper, with markups and visible takeoffs drawn on.

### Importing markups from Bluebeam, Acrobat and others

When you open a drawing that already has markups or measurements made in another app (Bluebeam Revu, Acrobat, a consultant's tool), Red-Beam asks once whether to import them. You can also run **Save PDF ▾ → Import markups from this PDF** at any time; anything already imported is skipped.

* **Markups:**

  | Other app | Becomes in Red-Beam |
  |---|---|
  | Clouds | Clouds |
  | Rectangles, ellipses, polygons | Rectangles, ellipses, polygons |
  | Lines, arrows, polylines | Lines and arrows |
  | Pen strokes | Pen |
  | Text boxes and callouts | Text boxes and callouts, with their font size and colours |
  | Sticky notes | Small yellow text boxes |
  | Stamps | Stamps |
  | Highlights | Highlights |

  Imported markups are fully editable, and the format bar works on them.
* **Measurements:** lengths, polylengths, areas and counts become Red-Beam measurements on an **Imported** layer, grouped by their subject (e.g. *Drywall Ceiling*), in their original colours. They're included in the quantities, costs and Excel export.
* **Scale:** each measurement's scale comes from the PDF when it's stored readably. Otherwise it's worked out from the value the other app showed (e.g. `30'-0"` or `960.00 sf`) and the drawn size, then snapped to the nearest standard scale. A sheet without a scale takes the most common one. A measurement made at a different scale gets a detail-scale box, so its quantity stays right.
* **No double display:** after importing, the PDF's own copies are hidden. Switch them back with **Save PDF ▾ → Show the PDF's original markups**. Saving or printing a copy then contains Red-Beam's version only.
* **Undo and originals:** **Ctrl+Z** undoes an import. **The PDF itself is never changed.**
* **Round trip:** PDFs saved from Red-Beam carry a copy of each markup and measurement. Opening that PDF in Red-Beam (for example on another computer) brings them back exactly.

### Takeoffs

The takeoff toolbar sits at the top-left of the drawing.

1. **Scale ▾:** pick the sheet's scale (1/8" = 1'-0", 1" = 20', …), or choose **Calibrate by two points…**, click both ends of a known dimension and type its length (`24'-6"`, `24.5'`, `294"`, `7.5m`). **Apply this sheet's scale to all sheets** sets the whole set at once.
2. **Group ▾:** choose or create the group you're measuring, such as *Drywall* or *Flooring*. Each group has its own colour. Selecting a measurement and then picking a group moves it to that group.
3. Measure:
   * **Length** (`L`): two clicks.
   * **Polyline** (`Y`): click each point, then press Enter, double-click or right-click to finish.
   * **Area** (`A`): click the corners, then finish. Shows the area in SF and its perimeter.
   * **Volume** (`U`): draw an area, then type a depth or thickness (`6"`, `4 1/2"`, `150mm`). Shows cubic yards (m³ in metric). Any area can get a depth from its right-click menu.
   * **Count** (`C`): click each item, then finish.
   * **Cut-outs:** select an area and press `X` (or right-click → **Add cut-out**), then outline the shaft, stair or opening. Area is net of cut-outs, and the perimeter includes their edges. Right-click inside a cut-out to remove it. Cut-out corners can be dragged like any other point.
   * **Snap** (`S`, on by default): clicks lock onto line ends and corners (green square) or onto a line (green ×) in the drawing itself. Hold **Alt** to place a point freely. The sheet's line work is read in the background the first time you measure on it (PDFium engine).
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
6. **Costs:** in **Group ▾**, open a group and set its prices: per LF, per SF (areas and wall areas), per CY and per EA. The quantities panel then shows each group's cost and a **total cost**.
7. **Slope and wall height** (right-click a measurement):
   * **Slope / pitch** (`6:12`, `1:12`, `26.6°`, `8.33%`) turns plan lengths and areas into true sloped size, for roofs and ramps.
   * **Wall height** on a length adds a **wall area**, e.g. drywall or paint, priced per SF.
8. **Detail scales:** in **Scale ▾ → Add a detail scale**, drag a box around a detail drawn at a different scale (e.g. a 1/4" detail on a 1/8" plan), then pick its scale or calibrate inside it. Measurements whose first point is inside the box use that scale. Remove one from the same menu.
9. **Presets ▾** (Bluebeam's Tool Chest): set up a tool or select a configured measurement, then **Save current tool as a preset**, e.g. *6" slab* (Volume, Concrete group, 6" depth) or *Type A wall* (Length, 9' wall height). Click a preset to measure with all its settings. Markup presets keep colour, width and font. Presets are saved with your settings and work in every drawing.
10. **Quantities** (`Q`): totals per group (length, area, perimeter, count) across all sheets, with warnings for sheets that have no scale.
11. **Export ▾:**
   * **Excel workbook:** a *Summary* tab with each group's quantities, unit prices (editable, highlighted yellow) and live cost formulas with totals; a *By sheet* tab; and an *Items* tab with every measurement, filterable.
   * **CSV:** the same data as plain text.
12. **Units ▾:** feet-inches (default), decimal feet, or metric.

Takeoffs save automatically to `<drawing name>.redbeam.json` next to the PDF. If that folder is read-only, they go to `%LOCALAPPDATA%\Red-Beam\takeoffs`. **The PDF itself is never modified.** Quantities are computed from each sheet's current scale, so correcting a calibration updates every measurement on that sheet.

### Sheet tools

**Sheets ▾** in the status bar:

* **Combine PDFs into one set**, or **Open a folder of PDFs as one set**: files are sorted naturally (A-2 before A-10).
* **Extract sheets to a new PDF**, e.g. `3` or `1, 4-6`.
* **Save a copy without some sheets.**
* **Save a copy with this sheet rotated** ↻ / ↺.

Each one writes a **new** PDF and opens it in a new tab. Source files are never modified.

### Tabs and split view

* **Tabs:** every PDF you open gets a tab along the top. Opening a file that's already open switches to its tab. Each tab keeps its own sheet, zoom, takeoffs, markups, undo, compare and search, and parked tabs stay loaded so switching is quick. Close a tab with **×**, a middle-click or `Ctrl+W`. **+** opens another PDF.
* **Side by side:** while comparing, the **Side by side** button (or `Tab`) shows the old sheet on the left and the new one on the right. **Zoom and pan are locked together.** You can measure and mark up in the right pane as usual.
* **Split view of one set:** `Ctrl+Shift+K` puts two sheets of the current set side by side. Choose the left sheet with `[` / `]` and flip the right one with PgUp/PgDn.

### Comparing revisions

1. Open the **current** set, then click **Compare** (or press `Ctrl+K`) and pick the **earlier** revision.
2. The overlay shows:
   * **red:** only in the old revision (removed)
   * **blue:** only in the new revision (added)
   * **black:** unchanged
3. Press `Tab`, or use the **Overlay / Old / New / Side by side** buttons, to flip between views at the same zoom and position.
4. Sheets pair by position: sheet 5 is compared with sheet 5. If sheets were added or removed between issues, press `[` or `]` to shift the pairing. The status bar shows which old sheet is paired.
5. **Automatic change detection:** Red-Beam compares the two revisions of the sheet you're viewing and draws a numbered **revision cloud** around each change. Clouds are **blue** for added work, **red** for removed work and **magenta** for both. **Change ▶ / ◀ Change** (or `.` / `,`) zoom to each change in turn, and the status bar shows "change 3 of 12". Small rendering differences and 1-pixel shifts are ignored. This needs the PDFium engine.
6. Sheets are aligned at their top-left corners. That's correct for Revit sets with the same title block. Point-based alignment for re-scaled or shifted sheets is on the roadmap.

The two sheets are rendered separately, tinted on the GPU and blended with a darken (per-channel minimum) blend, so overlays stay smooth at any zoom.

### Crash safety

* **Your work is saved as you go.** Takeoffs and markups are written to the sidecar file after every change.
* **Reopen after a crash.** If Red-Beam crashes or is closed by force, the next start says so and offers to reopen every drawing that was open, each in its tab and on its last sheet. The **Reopen last file** setting also reopens all tabs, and so does restarting after an update.
* **Crash reports.** A crash writes `%LOCALAPPDATA%\Red-Beam\crash.log`: which file and sheet were open, plus the error and a stack trace. A crash inside the PDF engine also writes `crash.dmp`. The next start shows where the report is, so it can be sent in and the cause fixed.
* **Tested on every build.** CI launches the real app on Windows (`RedBeam.exe --smoke tests/sample-set.pdf out`), steps through the main screens and saves a screenshot of each: open, page list, discipline jump, format bar, quantities, text editing, help and compare. A panic, crash or error dialog fails the build before it's released. The screenshots are uploaded as the `smoke-screenshots` artifact. `tests/make_sample.py` generates the sample set and its earlier revision.

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
| `src/app/panel.rs` | Side panel: page thumbnails, recent and pinned files, settings |
| `src/settings.rs` | Preferences, recent and pinned files, saved as JSON (unit-tested) |
| `src/app/tools.rs` | Takeoff UI: tool palette, measuring tools, selection and undo, quantities panel, menus, prompt |
| `src/xlsx.rs` | Formatted Excel export (Summary with cost formulas, By sheet, Items) |
| `src/diff.rs` | Revision change detection: ink masks, shift-tolerant diff, region grouping (unit-tested) |
| `src/markup.rs` | Markup model: kinds, hit testing, handles, and drawing primitives shared by the screen and PDF export (unit-tested) |
| `src/app/markups.rs` | Markup tools, editing, menus, Markups list, Save a copy with markups |
| `src/sheets.rs` | Sheet numbers and titles, sheet links, phrase search (unit-tested) |
| `src/app/extras.rs` | Detail viewport scales, tool presets, sheet tools (combine, folder as set, extract, rotate) |
| `src/app/format.rs` | Format bar for selected markups, multi-selection, text shortcuts |
| `src/app/session.rs` | Open tabs remembered for reopening after a crash or update; startup recovery prompt |
| `src/app/smoke.rs` | CI smoke test: steps through the screens and saves screenshots |
| `src/crash.rs` | Panic and native crash reports (crash.log, crash.dmp) |
| `src/png.rs` | Minimal PNG writer for smoke-test screenshots (unit-tested) |
| `src/import.rs` | Turns PDF annotations into markups and measurements; scale from /Measure or from displayed values (unit-tested) |
| `src/app/importer.rs` | Import flow: background read, one-time prompt, scales and detail boxes, hiding the PDF's own copies |
| `src/app/tabs.rs` | Document tabs (park and restore per-document state), tab strip |
| `src/app/find.rs` | Find / Go to sheet UI, highlighted hits, clickable sheet links |
| `src/snap.rs` | Snap index over a sheet's line work: nearest corner, else nearest point on a line (unit-tested) |
| `assets/` | App icon (generated by `mkicon.py`) and Windows manifest; `build.rs` embeds them with version info |
| `src/gfx.rs` | Direct3D 11, Direct2D and DXGI swap-chain setup |
| `src/view.rs` | Camera math: zoom about a point, fit page or width, pan clamping |

## Known limitations

* Password-protected PDFs aren't supported.
* Opening a file reads every page size up front. Sets with several thousand sheets take a moment to open.

## Roadmap (goal: replace Bluebeam)

1. **Revision compare:** overlay, old/new toggle and sheet pairing are done. Next: point-based alignment, sheet-number matching, and a list of changed areas.
2. **Takeoffs:** scales and calibration, length, polyline, area (with cut-outs), volume, count, snapping, groups, layers, editing, leaders, a quantities panel and CSV export are done. Next: per-viewport scales, intersection snapping, and a native Excel export.
3. **Markups:** cloud, text, callout, arrow, line, rectangle, ellipse, highlight, pen and stamps are done, with status, a markups list, a format bar, saving into a PDF copy, and importing markups and measurements from Bluebeam and Acrobat. Next: reply threads.
4. **Navigation and UI:** the side panel, sheet names, text search, go-to-sheet, clickable sheet links, tabs and side-by-side split view are done. Next: UI polish.
5. **Rollout:** installer, "Open with Red-Beam", sharing presets with a team.

## Third-party software

Red-Beam embeds [PDFium](https://pdfium.googlesource.com/pdfium/) (BSD-3-Clause), built by [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries) (Apache-2.0 build scripts). PDFium includes third-party components such as FreeType, libjpeg-turbo, OpenJPEG, LittleCMS, zlib and AGG, each under its own permissive license. The full license texts ship in the `licenses/` folder of the PDFium release archive.

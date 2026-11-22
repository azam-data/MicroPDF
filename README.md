# MicroPDF

**MicroPDF** is a lightweight, portable PDF viewer and power editor written in Rust using [`winit`](https://crates.io/crates/winit), [`softbuffer`](https://crates.io/crates/softbuffer), [`pdfium-render`](https://crates.io/crates/pdfium-render), [`lopdf`](https://crates.io/crates/lopdf), and [`image`](https://crates.io/crates/image). No GPU, no GUI toolkit, no installer: the menu bar, tab bar and status bar are drawn by the CPU straight into the window frame buffer.

**Footprint (v1.3.0, raw process memory measured by `ram_realistic_measurement`, release build on macOS, 1920×1080, without the window frame buffer of about 8 MB):**

- `micro_pdf.exe` is about 1.6 MB (`micro_pdf` on macOS about 1.4 MB). It needs `pdfium.dll` (about 7.8 MB, from [pdfium-binaries](https://github.com/bblanchon/pdfium-binaries)) in the **same folder**, so a portable install is about 9.5 MB.
- PDFium reads the file on demand, so a large file is not held in memory.

| Raw RSS | v1.2.1 | v1.3.0 |
|---|---|---|
| Small text PDF: idle after open | | 16.9 MB |
| 116 MB, 100-page 300-dpi scan: idle after open | 129 MB | 50.6 MB |
| Scan: peak while scrolling 50 pages | 228 MB | 118.2 MB |
| Scan: peak at 400% zoom, panning | 274 MB | 134.4 MB |
| Scan: peak in thumbnail grid | 301 MB | 187.8 MB |

All scan figures are within the target of file size + 120 MB (236 MB). Continuous scroll caches only visible pages ± 1, zoom above 2× uses a 16 MB LRU cache of 512×512 tiles, and the thumbnail grid is capped at 300 entries. Printing a 20-page scan at 600 dpi adds about 25 MB at peak (one decoded scan image plus one 5 MB strip).

---

## Key Features

- **Bright, Modern CPU-Rendered UI (Light Default + Dark Toggle, Zero GPU)**:
  - **Unified Top Menu & Customizable Icon Toolbar (32px)**: 7 crisp dropdown menus (`File`, `View`, `Edit`, `Page`, `Tools`, `Export`, `Help` including `Help -> User Guide (How to Use)`, `Help -> Keyboard & Mouse Shortcuts`, `Help -> Bookmark Import Format (.txt)`, `Help -> About MicroPDF`, and `File -> Recent`) paired with a customizable icon toolbar directly beside `Help` (`View -> Show Toolbar`, `View -> Customize Toolbar...`, and `View -> UI Scale`), rendered with antialiased proportional typography (`ab_glyph` + bounded 256 KB glyph cache), rounded buttons, soft shadows, and instant **Light / Dark UI** switching persisted in `micropdf.ini`.
  - **Built-in Step-by-Step Offline User Guide (`Help -> User Guide (How to Use)`)**: 10 plain-English beginner chapters covering every tool with numbered steps, friendly tips, and one-click `.txt` export for printing.
  - **Scalable Tabs, Unsaved-Changes Guard, Welcome Screen & Floating Search**: Folder-style tabs with unsaved-edit indicators (`•`), overflow scroll arrows + `▾` All-Tabs dropdown + tab right-click menu (`Close Other Tabs` / `Close Tabs to Right` / `Close All Tabs`), `Save / Discard / Cancel` unsaved-changes guard on tab/app close, drag-and-drop Welcome Screen, top-right floating `Ctrl/Cmd+F` search pill (`Aa` Match Case, `W` Whole Word, `▲`/`▼` prev/next), and a thin draggable vertical scrollbar.
- **View Modes (`1`–`4`, `T`), Independent Scroll Flow (`5`), Eye-Care Modes (`V`), Zoom (`25%–800%`), & Fullscreen/Presentation (`F11` / `F5`)**:
  1. **`FitPage` (`1`)**, **`FitWidth` (`2`)**, **`FitTextWidth` (`3`)**, **`TwoPageSpread` (`4`)**, and **Reflowable `TextView` (`T`)**.
  2. **Independent Scroll Modes**: Combine any page layout with **Single Page**, **Continuous Vertical (`5`)**, or **Continuous Horizontal Book** scroll.
  3. **6 Eye-Care Reading Modes (`V` or Bottom Bar Picker, persisted in `micropdf.ini`)**: `Normal`, **`Paper E-Ink` (Best for day)**, `Warm Sepia`, `Sage Eye-Care`, `Dark Invert`, and **`Night Amber` (Best for night)**.
  4. **Reading Continuity**: Automatically remembers your last page & zoom for up to 50 files in `micropdf.ini`, restores previous sessions on request, forwards second-instance opens in `< 200 ms`, and auto-reloads modified PDFs every 1 s while focused.
- **Annotations, Interactive Form Filling, & Power Editor Tools**:
  - **Select, Copy, Annotate & Inspect**: Drag or double-click text (`Ctrl/Cmd+C`), place Highlights, Underlines, Strikethroughs, Sticky Notes (multi-line reader popover with Edit/Delete), Freehand Pen, Text Boxes, Rectangles, and Arrows in 7 colors; **right-click any annotation** to edit text, recolor, or delete. Full 8-step **Undo (`Ctrl+Z`) & Redo (`Ctrl+Y` / `Ctrl+Shift+Z`)** across edits and metadata.
  - **Native Interactive Form Filling**: Click PDF Checkboxes, Radio Buttons, and Text Fields directly in Select mode with Undo, Save, and Print support.
  - **Side-by-Side PDF Compare (`Tools -> Compare with Another PDF...`)**: Synchronized dual-pane comparison with page offset, pane swap, and CPU **Visual Diff** overlay (red/blue pixel diff).
  - **Quick Measure / Ruler Tool (`Edit -> Measure Distance & Area (Ruler)`)**: Measure exact Length, ΔX × ΔY, Area, and Angle directly from PDF points in `mm`, `cm`, `in`, or `pt`.
  - **Black-Box Redaction & One-Click Privacy Scrubber (`File -> Document Properties & Privacy...`)**: True content/pixel redaction that strips underlying text, images, form fields, `/Info`, and `/StructTreeRoot` on save, plus one-click personal metadata & XMP scrubbing without altering page content.
  - **Create PDF from Images (`File -> Create PDF from Images...`) & Export Menu**: Convert PDFs offline to **Word (`.docx`)**, **Excel (`.xlsx`)** (Hybrid Lattice + Stream engine with merged cells, multi-table isolation, and financial comma-decimal normalization), **Markdown (`.md`)**, **LaTeX (`.tex`)**, or **PNG/JPEG Images** (150/200/300 DPI).
  - **Lightweight Matrix Page Deskew (`Page -> Deskew Page(s)...`)**: Straighten tilted/scanned pages (`-15.0°` to `+15.0°`, `0.1°` steps) with Draw Guideline auto-snap, alignment grid, and live preview using lossless 2D affine transforms (zero rasterization).
- **Print Preview & Printing (`Ctrl/Cmd+P` or `File -> Print...`)**: Live sheet preview on Windows, macOS, and Linux with **10 Paper Sizes**, `With Annots` / `No Annots`, **Auto-Trim** white space + `100%–200%` Print Zoom, automatic **2-Sided (Duplex)**, guided **Manual 2-Sided on any 1-sided printer**, and foldable **Booklet** printing.
- **Thumbnail Grid Multi-Select (`Tab`), Combine / Insert PDFs, Header/Footer Profiles, Page Numbers, Bookmarks & Optimize**:
  - Multi-select pages (`Ctrl+Click` / `Shift+Click`) for single-step batch Rotate, B/W, Extract, or Delete; right-click thumbnails to insert blanks or external PDFs before/after; visual **Header & Footer** dialog with live preview & reusable style profiles; visual **Page Numbering** dialog with Roman/Arabic & skip cover pages; **Bookmark Sidebar** with Add, Edit, Reorder, Indent/Outdent, Auto-Detect Headings, Import `.txt`, and Export `.txt`; and **Optimize (`Z`)** with scan cleanup and live preview.

---

## Keyboard & Mouse Shortcuts

| Shortcut / Action | Mode | Description |
| :--- | :--- | :--- |
| **Top Menu Bar Click** | Any | Open `[ File ]`, `[ View ]`, `[ Edit ]`, `[ Page ]`, `[ Tools ]`, `[ Export ]`, or `[ Help ]` |
| `1` / `2` / `3` / `4` / `5` / `T` | Page View | `1` Fit Page, `2` Fit Width, `3` Fit Text, `4` Two-Page, `5` Continuous Scroll, `T` Text View |
| `Ctrl/Cmd+Wheel` / `+` / `-` / `0` | Page View | Zoom (`25%–800%`) around cursor or reset zoom to `100%` (`0` / `Ctrl+0`) |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` / `Ctrl+1..9` | Any | Switch next tab, previous tab, or jump directly to tab 1–9 (`Ctrl+W` closes tab) |
| `F11` / `F5` / `Esc` | Any | Fullscreen (`F11`), Presentation Mode (`F5`), or exit tool/compare/modal (`Esc`) |
| `Alt+Left` / `Alt+Right` | Page View | Navigate Back / Forward in link & page jump history (up to 50 entries) |
| **Left-Click / Drag / Right-Click** | Page View | Select text (`Ctrl+C`), click Form Fields to fill, right-click any annotation to Edit/Recolor/Delete |
| `Tab` / `Ctrl+Click` / `Shift+Click` | Any / Grid | Toggle Thumbnail Grid (`Tab`); multi-select thumbnails for batch Rotate / B&W / Extract / Delete |
| `Cmd/Ctrl+F` & `F3` / `Shift+F3` | Any | Incremental search (`Aa` Case, `W` Whole Word) & Next (`F3` / `Ctrl+G`) / Prev (`Shift+F3` / `Ctrl+Shift+G`) |
| `V` | Page View | Cycle 6 Eye-Care Reading Modes (`Paper E-Ink` day, `Night Amber` night, Sepia, Sage, Dark, Normal) |
| `P` / `U` / `Z` | Any | Stamp Signature/Image (`P`), Flatten & Unlock (`U`), Optimize PDF (`Z`) |
| `Shift+M` / `X` | Any | **Combine Multiple PDFs** (`Shift+M`), Split / Extract range (`X`) |
| `B` / `A` / `L` | Any | Toggle Bookmark Sidebar (`B`), Add Bookmark (`A`), Import `.txt` bookmarks (`L`) |
| `G` | Normal | Smart Jump to Logical (`5`), Roman (`iii`), or Physical (`#5`) page |
| `N` / `H` / `W` | Normal | Page Numbering Dialog (`N`), Header/Footer & Profiles (`H`), Watermark (`W`) |
| `K` / `I` / `R` / `D` | Normal / Grid | Convert page to B&W (`K`), Insert Blank (`I`), Rotate 90° (`R`), Delete page (`D`) |
| `Ctrl+Z` / `Ctrl+Y` (`Ctrl+Shift+Z`) | Normal | Undo (`Ctrl+Z`) and Redo (`Ctrl+Y` / `Ctrl+Shift+Z`) up to 8 steps per tab |
| `Ctrl/Cmd+P` / `O` / `S` / `Ctrl+Shift+S` | Any | Live Print Preview (`Ctrl+P`), Open (`O`), Save (`S`), Save As (`Ctrl+Shift+S`) |
| `?` / `F1` / `Help -> User Guide` | Any | Quick Shortcut Sheet (`?` / `F1`) or 10-chapter Step-by-Step Beginner User Guide |

---

## Requirements

Keep `micro_pdf.exe` and `pdfium.dll` (Windows) or `micro_pdf` and `libpdfium.dylib` / `libpdfium.so` together in one folder. Since v1.1, PDFium is loaded **only** from the executable's folder, never from the current working directory or system paths. For development you can point to another folder with the `MICROPDF_PDFIUM_DIR` environment variable.

## Building from Source

```bash
# Native macOS / Linux Release Build
cargo build --release

# Portable Windows x86_64 Cross-Compile
cargo xwin build --release --target x86_64-pc-windows-msvc
```

## Optimize (`Z` and `Tools -> Optimize...`)

Writes `<name>_optimized.pdf` next to the original; the original and the open document are never changed, and nothing is written if the result would not be smaller. `Z` runs the Balanced preset: images are downsampled to 200 dpi and re-encoded as JPEG (quality 70). On scanned pages (an image covering most of the page) off-white paper is pushed to pure white and text is sharpened, while ink colour is kept, so blue signatures stay blue. `Tools -> Optimize...` also offers Smaller (150 dpi) and Print (300 dpi), gray and black-and-white (1-bit) output, scan cleaning on or off, and a preview of the current page. Everything runs locally; no Ghostscript or OCR.

On a 10-page, 24 MB scan: 6.0 MB in about 1 s per page; 0.54 MB in black-and-white mode.

## Testing

```bash
# PDFium must be reachable: copy it next to the test binary or set MICROPDF_PDFIUM_DIR
cargo test
```

Real memory figures (run on its own, takes a minute):

```bash
cargo test --release ram_realistic -- --ignored --nocapture --test-threads=1
cargo test --release ram_print -- --ignored --nocapture --test-threads=1
```

`test_redaction_leaves_no_trace` builds a PDF with secret text over an image, redacts it, saves it, and fails if the secret survives in extracted text, in any decompressed stream, or in the image pixels under the box.

## Contributing & Updating the Built-In User Guide

Before contributing code or opening a pull request, please read **[`AGENTS.md`](AGENTS.md)** and **[`docs/USER_GUIDE_FRAMEWORK.md`](docs/USER_GUIDE_FRAMEWORK.md)**. Any change that adds, renames, or alters user-facing features, menus, dialogs, or shortcuts **must** update the built-in beginner manual in [`src/viewer/user_guide.rs`](src/viewer/user_guide.rs) and the shortcut sheet in [`src/viewer/overlays.rs`](src/viewer/overlays.rs).

## License

MicroPDF is released under the MIT License (see `LICENSE`). Third-party components, including PDFium, keep their own licenses; see `THIRD_PARTY_NOTICES.md`.

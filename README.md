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

All scan figures are within the target of file size + 120 MB (236 MB). Continuous scroll caches only visible pages ± 1, zoom above 2× uses a 48 MB LRU cache of 512×512 tiles, and the thumbnail grid is capped at 300 entries. Printing a 20-page scan at 600 dpi adds about 25 MB at peak (one decoded scan image plus one 5 MB strip).

---

## Key Features

- **CPU-Rendered Menu Bar & Command/Status Bar**:
  - **24px Top Menu Bar (`#1E1E1E`)**: 6 clickable dropdown menus (`[ File ]  [ View ]  [ Edit ]  [ Page ]  [ Tools ]  [ Help ]`) rendered directly over the CPU frame buffer (`#252526` dropdown, `#007ACC` hover highlight), including `File -> Recent` (last 10 files).
  - **22px Bottom Command & Status Bar**: Displays interactive typing prompts with a live cursor inside the window (including masked `*` password prompts for encrypted PDFs, incremental search progress, external URL confirmation, and session restore prompts), plus live page, mode, zoom, mouse-tool, filter, and RAM telemetry.
- **Five View Modes (`1`, `2`, `3`, `4`, `5` Keys & `View` Menu), Zoom (`25%–800%`), & Fullscreen/Presentation (`F11` / `F5`)**:
  1. **`FitPage` (`1`)**: Fits the full page cleanly inside the available viewport.
  2. **`FitWidth` (`2`)**: Scales the page so its full width fills the viewport (minus 24px padding), with smooth vertical scrolling (`scroll_y`) and automatic page-to-page transitions.
  3. **`FitTextWidth` (`3`)**: Computes the union bounding box of page content objects (`page.objects().iter()`) to auto-trim left/right margins so the text column fills the viewport width.
  4. **`TwoPageSpread` (`4`)**: Renders `current_page` and `current_page + 1` side-by-side with a 12px gap, advancing 2 pages at a time.
  5. **`ContinuousScroll` (`5`)**: Vertical page stack with 12px gaps where the page number tracks the viewport centre and only visible pages ± 1 are cached in RAM.
  - **Cursor-Anchored Zoom (`Ctrl/Cmd + Wheel`, `+`, `-`, `0`)**: Zooms 25%–800% around the cursor; above 2× viewport size, renders only visible `512 × 512` tiles in a 48 MB LRU tile cache.
  - **Fullscreen (`F11`) & Presentation Mode (`F5`)**: `F11` hides all bars; `F5` fits one page to a solid black screen with arrow/mouse-click page turns (`Esc` exits).
- **Links, History (`Alt+Left` / `Alt+Right`), Session Restore (`micropdf.ini`), Single Instance, & Auto-Reload**:
  - Click internal PDF links to jump or external URLs (with status-bar `Enter` confirmation) to open in the browser; navigate back/forward up to 50 steps with `Alt+Left` / `Alt+Right`.
  - Automatically saves and offers to restore your previous tabs and recent files from `micropdf.ini`, forwards second-instance CLI opens to the running window in `< 200 ms`, and auto-reloads modified PDFs every 1 s while focused (ideal for LaTeX/Typst).
- **Text Selection (`Ctrl/Cmd+C`) & 6 Power Editor Tools**:
  - **Select & Copy Text (`Mouse Mode: Select Text`, default)**: Drag across text or double-click a word to highlight and press `Ctrl/Cmd+C` to copy to the clipboard (character boxes are cached for the current page only).
  1. **Time-Sliced Incremental Text Search (`Ctrl+F` / `Cmd+F`, `F3` / `Ctrl+G`, `Esc` to cancel)**: Searches ~8 ms of pages per frame on the main thread with live status-bar progress so 1,000+ page PDFs never freeze the UI.
  2. **Black-Box Redaction & Metadata Sanitizer (`Mouse Mode: Redact`)**: Drag a red box across any page region. Text, vector paths, form XObjects and annotations under the box are deleted; image pixels under the box are painted black inside the image itself (rotated or undecodable images are removed whole). On save, a redacted document also loses its `/Info` dictionary, embedded page thumbnails and every orphaned object, and the save is refused if that cleanup fails. Redaction does not touch bookmark titles or file names, so check those yourself.
  3. **Visual Page Cropping (`Mouse Mode: Crop Page Area`)**: Drag a green box to crop the page's `/CropBox` in-place, with one-click restore via `Page -> Reset Page Crop` or `Cmd/Ctrl+Z`.
  4. **Signature / Image Stamp (`P` Key or `Tools -> Stamp Signature / Image...`)**: Pick any `.png` (preserving alpha transparency) or `.jpg`/`.jpeg` file (auto-capped at 2,000 px) and click or drag on the page to stamp it via `PdfPageImageObject`.
  5. **Dark Mode / Sepia Reading Filter (`V` Key or `View` Menu)**: Cycles `Normal`, `DarkInvert`, and `WarmSepia` directly in the final `softbuffer` blit loop with zero extra RAM and zero modifications to the PDF file.
  6. **Flatten & Unlock PDF (`U` Key or `Tools -> Flatten & Unlock PDF`)**: Flattens annotations and form fields on all pages into page content streams and strips `/Encrypt`, `/AcroForm`, and `/Perms` restrictions.
- **Printing (`Ctrl/Cmd+P` or `File -> Print...`)**: Prints the document as it is in the app, including unsaved edits, redactions and filled-in form fields.
  - **Windows**: native print dialog (page range, copies); pages are sent one per frame with `Printing page N / M` in the status bar and `Esc` to cancel. Each page is rendered at the printer's resolution in 256-row strips, fitted to the printable area, paper orientation following the page.
  - **macOS**: bottom-bar prompt (printer from `lpstat`, copies, page range like `1-3,5,8-`, `F` fit-to-page) that sends a temporary PDF to `lp` in the background and deletes it once `lp` has it.
- **Optimize (`Z` or `Tools -> Optimize...`)**: Smaller / Balanced / Print presets, colour, gray or black-and-white, scan cleaning on or off, a preview of the current page, progress in the status bar and `Esc` to cancel. Writes `<name>_optimized.pdf`.
- **Batch Combine Multiple PDFs (`Shift+M` or `File -> Combine Multiple PDFs...`)**:
  - Select 2 or more PDF files in the native multi-file dialog to append all their pages in order and open the combined document in a new `"Combined.pdf"` tab.
- **Multi-Document Tab Bar, Thumbnail Grid (`Tab`), Grayscale (`[BW]` / `K`), Bookmarks (`B` / `L`), & Multi-Step Undo (`Cmd/Ctrl+Z`)**:
  - Inactive tabs hold no rendered bitmaps, `140 x 198` Thumbnail Grid (evicting outside visible rows ± 1 screen, capped at 300 entries) with drag-and-drop page reordering and `[BW]` / `[R]` badges, Roman/Arabic `/PageLabels` (`N`), aligned Headers/Footers (`H`), Margin Zone purge (`C`), hierarchical `.txt` bookmarks (`L`) embedded into `/Outlines`, and 8-step Undo (`Cmd/Ctrl+Z`).

---

## Keyboard & Mouse Shortcuts

| Shortcut / Action | Mode | Description |
| :--- | :--- | :--- |
| **Top Menu Bar Click** | Any | Open `[ File ]`, `[ View ]`, `[ Edit ]`, `[ Page ]`, `[ Tools ]`, or `[ Help ]` dropdown menu |
| `1` / `2` / `3` / `4` / `5` | Page View | Switch View Mode: `1` Fit Page, `2` Fit Width, `3` Fit Text, `4` Two-Page, `5` Continuous Scroll |
| `Ctrl/Cmd+Wheel` / `+` / `-` / `0` | Page View | Zoom in/out (`25%–800%`) around cursor or reset zoom to `100%` (`0`) |
| `F11` / `F5` / `Esc` | Any | Toggle Fullscreen (`F11`), Presentation Mode (`F5`), or exit/cancel (`Esc`) |
| `Alt+Left` / `Alt+Right` | Page View | Navigate Back / Forward in link & page jump history (up to 50 entries) |
| **Left-Click & Drag / Double-Click** | Page View | Select text / word (`Ctrl/Cmd+C` to copy), or Highlight, Redact, Crop, Stamp Image |
| `Tab` | Any | Toggle between **Page View** and **All-Pages Thumbnail Grid** (`Ctrl+Tab` cycles open tabs) |
| `Cmd/Ctrl+F` & `F3` / `Cmd/Ctrl+G` | Any | Time-sliced incremental text search (`Esc` cancels) & jump to next match |
| `V` | Page View | Cycle **Normal** / **Dark Invert** / **Warm Sepia** reading filter (in-place CPU blit) |
| `P` | Page View | Pick PNG/JPG signature or image -> Click or drag on page to stamp |
| `U` | Any | Flatten annotations/form fields on all pages & unlock PDF permissions |
| `Shift+M` / `M` | Any | **Combine Multiple PDFs** into `"Combined.pdf"` tab (`Shift+M`) / Merge 1 PDF (`M`) |
| `B` / `L` | Any | Toggle **Left Bookmark Sidebar** / Import `.txt` bookmarks (`Serial -- Title -- Page`) |
| `G` or `0`–`9` | Normal | Smart Jump to Logical page (`5`), Roman page (`ii`), or Physical page (`#5`) |
| `N` / `H` / `C` / `W` | Normal | Roman/Arabic Page Numbers (`N`), Aligned Header/Footer (`H`), Clear Margins (`C`), Watermark (`W`) |
| `K` / `I` / `R` / `D` | Normal / Grid | Convert page to Grayscale `[BW]` (`K`), Insert A4 (`I`), Rotate 90° (`R`), Delete page (`D`) |
| `Cmd+Z` / `Ctrl+Z` | Normal | Undo last structural or page edit (up to 8 steps per tab) |
| `Ctrl/Cmd+P` / `File -> Print...` | Any | Print the current document (Windows: print dialog, `Esc` cancels while printing; macOS: print bar, `Tab` printer, `+`/`-` copies, `F` fit-to-page) |
| `Tools -> Optimize...` | Any | Optimize menu in the bottom bar: `1`/`2`/`3` preset, `C`/`G`/`B` colour mode, `S` clean scans, `P` preview, `Enter` run, `Esc` cancel |
| `O` / `S` / `Z` / `X` | Normal | Open PDF in tab (`O`), Save PDF (`S`), Optimize PDF to `<name>_optimized.pdf` (`Z`), Split/Extract range (`X`) |
| `?` / `F1` / `Esc` | Any | Toggle Help Overlay / Close menu / Cancel prompt / Reset mouse mode |

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

```
# PDFium must be reachable: copy it next to the test binary or set MICROPDF_PDFIUM_DIR
cargo test
```

Real memory figures (run on its own, takes a minute):

```
cargo test --release ram_realistic -- --ignored --nocapture --test-threads=1
cargo test --release ram_print -- --ignored --nocapture --test-threads=1
```

`test_redaction_leaves_no_trace` builds a PDF with secret text over an image, redacts it, saves it, and fails if the secret survives in extracted text, in any decompressed stream, or in the image pixels under the box.

## License

MicroPDF is released under the MIT License (see `LICENSE`). Third-party components, including PDFium, keep their own licenses; see `THIRD_PARTY_NOTICES.md`.

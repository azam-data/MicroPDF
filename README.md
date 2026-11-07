# MicroPDF

**MicroPDF** is a lightweight, portable PDF viewer and power editor written in Rust using [`winit`](https://crates.io/crates/winit), [`softbuffer`](https://crates.io/crates/softbuffer), [`pdfium-render`](https://crates.io/crates/pdfium-render), [`lopdf`](https://crates.io/crates/lopdf), and [`image`](https://crates.io/crates/image). No GPU, no GUI toolkit, no installer: the menu bar, tab bar and status bar are drawn by the CPU straight into the window frame buffer.

**Footprint (v1.1, measured by the built-in RAM readout):**

- `micro_pdf.exe` is about 1 MB. It needs `pdfium.dll` (roughly 5–10 MB, from [pdfium-binaries](https://github.com/bblanchon/pdfium-binaries)) in the **same folder**, so a portable install is about 6–11 MB in total.
- RAM is roughly 12–15 MB with a small text PDF open. The whole file is held in memory, so add about 1x the file size, plus up to 64 MB of undo snapshots per tab while editing large files.

---

## Key Features

- **CPU-Rendered Menu Bar & Command/Status Bar**:
  - **24px Top Menu Bar (`#1E1E1E`)**: 6 clickable dropdown menus (`[ File ]  [ View ]  [ Edit ]  [ Page ]  [ Tools ]  [ Help ]`) rendered directly over the CPU frame buffer (`#252526` dropdown, `#007ACC` hover highlight).
  - **22px Bottom Command & Status Bar**: Displays interactive typing prompts with a live cursor inside the window whenever searching text, jumping pages, or stamping numbers/headers/watermarks, plus live page, mode, mouse-tool, filter, and RAM telemetry.
- **Four View Modes (`1`, `2`, `3`, `4` Keys & `View` Menu)**:
  1. **`FitPage` (`1`)**: Fits the full page cleanly inside the available viewport.
  2. **`FitWidth` (`2`)**: Scales the page so its full width fills the viewport (minus 24px padding), with smooth vertical scrolling (`scroll_y`) and automatic page-to-page transitions.
  3. **`FitTextWidth` (`3`)**: Computes the union bounding box of page content objects (`page.objects().iter()`) to auto-trim left/right margins so the text column fills the viewport width.
  4. **`TwoPageSpread` (`4`)**: Renders `current_page` and `current_page + 1` side-by-side with a 12px gap, advancing 2 pages at a time.
- **6 Power Editor Tools**:
  1. **Text Search (`Ctrl+F` / `Cmd+F`, `F3` / `Ctrl+G`)**: Searches `page.text()` across pages, jumps to matches, and highlights matching character bounding boxes on the CPU canvas in bright orange/cyan.
  2. **Black-Box Redaction & Metadata Sanitizer (`Mouse Mode: Redact`)**: Drag a red box across any page region. Text, vector paths, form XObjects and annotations under the box are deleted; image pixels under the box are painted black inside the image itself (rotated or undecodable images are removed whole). On save, a redacted document also loses its `/Info` dictionary, embedded page thumbnails and every orphaned object, and the save is refused if that cleanup fails. Redaction does not touch bookmark titles or file names, so check those yourself.
  3. **Visual Page Cropping (`Mouse Mode: Crop Page Area`)**: Drag a green box to crop the page's `/CropBox` in-place, with one-click restore via `Page -> Reset Page Crop` or `Cmd/Ctrl+Z`.
  4. **Signature / Image Stamp (`P` Key or `Tools -> Stamp Signature / Image...`)**: Pick any `.png` (preserving alpha transparency) or `.jpg`/`.jpeg` file and click or drag on the page to stamp it via `PdfPageImageObject`.
  5. **Dark Mode / Sepia Reading Filter (`V` Key or `View` Menu)**: Cycles `Normal`, `DarkInvert`, and `WarmSepia` directly in the final `softbuffer` blit loop with zero extra RAM and zero modifications to the PDF file.
  6. **Flatten & Unlock PDF (`U` Key or `Tools -> Flatten & Unlock PDF`)**: Flattens annotations and form fields on all pages into page content streams and strips `/Encrypt`, `/AcroForm`, and `/Perms` restrictions.
- **Batch Combine Multiple PDFs (`Shift+M` or `File -> Combine Multiple PDFs...`)**:
  - Select 2 or more PDF files in the native multi-file dialog to append all their pages in order and open the combined document in a new `"Combined.pdf"` tab.
- **Multi-Document Tab Bar, Thumbnail Grid (`Tab`), Grayscale (`[BW]` / `K`), Bookmarks (`B` / `L`), & Multi-Step Undo (`Cmd/Ctrl+Z`)**:
  - Inactive tabs hold no rendered bitmaps, `140 x 198` Thumbnail Grid with drag-and-drop page reordering and `[BW]` / `[R]` badges, Roman/Arabic `/PageLabels` (`N`), aligned Headers/Footers (`H`), Margin Zone purge (`C`), hierarchical `.txt` bookmarks (`L`) embedded into `/Outlines`, and 8-step Undo (`Cmd/Ctrl+Z`).

---

## Keyboard & Mouse Shortcuts

| Shortcut / Action | Mode | Description |
| :--- | :--- | :--- |
| **Top Menu Bar Click** | Any | Open `[ File ]`, `[ View ]`, `[ Edit ]`, `[ Page ]`, `[ Tools ]`, or `[ Help ]` dropdown menu |
| `1` / `2` / `3` / `4` | Page View | Switch View Mode: `1` Fit Page, `2` Fit Width, `3` Fit Text Width, `4` Two-Page Side-by-Side |
| `Tab` | Any | Toggle between **Page View** and **All-Pages Thumbnail Grid** (`Ctrl+Tab` cycles open tabs) |
| `Cmd/Ctrl+F` & `F3` / `Cmd/Ctrl+G` | Any | Find text in PDF & jump to next search match |
| `V` | Page View | Cycle **Normal** / **Dark Invert** / **Warm Sepia** reading filter (in-place CPU blit) |
| `P` | Page View | Pick PNG/JPG signature or image -> Click or drag on page to stamp |
| `U` | Any | Flatten annotations/form fields on all pages & unlock PDF permissions |
| `Shift+M` / `M` | Any | **Combine Multiple PDFs** into `"Combined.pdf"` tab (`Shift+M`) / Merge 1 PDF (`M`) |
| **Left-Click & Drag** | Page View | Highlight text (default), Black-Box Redact, Crop Page Area, or Stamp Image |
| `B` / `L` | Any | Toggle **Left Bookmark Sidebar** / Import `.txt` bookmarks (`Serial -- Title -- Page`) |
| `G` or `0`–`9` | Normal | Smart Jump to Logical page (`5`), Roman page (`ii`), or Physical page (`#5`) |
| `N` / `H` / `C` / `W` | Normal | Roman/Arabic Page Numbers (`N`), Aligned Header/Footer (`H`), Clear Margins (`C`), Watermark (`W`) |
| `K` / `I` / `R` / `D` | Normal / Grid | Convert page to Grayscale `[BW]` (`K`), Insert A4 (`I`), Rotate 90° (`R`), Delete page (`D`) |
| `Cmd+Z` / `Ctrl+Z` | Normal | Undo last structural or page edit (up to 8 steps per tab) |
| `O` / `S` / `Z` / `X` | Normal | Open PDF in tab (`O`), Save PDF (`S`), Compress PDF (`Z`), Split/Extract range (`X`) |
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

## Testing

```
# PDFium must be reachable: copy it next to the test binary or set MICROPDF_PDFIUM_DIR
cargo test
```

`test_redaction_leaves_no_trace` builds a PDF with secret text over an image, redacts it, saves it, and fails if the secret survives in extracted text, in any decompressed stream, or in the image pixels under the box.

## License

MicroPDF is released under the MIT License (see `LICENSE`). Third-party components, including PDFium, keep their own licenses; see `THIRD_PARTY_NOTICES.md`.

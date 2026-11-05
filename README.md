# MicroPDF

**MicroPDF** is an ultra-lightweight (**~12 MB RAM**), portable, zero-GPU-bloat PDF viewer and structural editor written in Rust using [`winit`](https://crates.io/crates/winit), [`softbuffer`](https://crates.io/crates/softbuffer), [`pdfium-render`](https://crates.io/crates/pdfium-render), and [`lopdf`](https://crates.io/crates/lopdf).

Designed for maximum speed, tiny binary size (**< 1 MB** executable), and strict single-page memory discipline, MicroPDF blits PDFium bitmaps directly into a CPU window buffer and uses the native OS Window Title Bar as a zero-overhead interactive HUD.

---

## Features

- **Strict Single-Page Viewport Virtualization (~12 MB RSS)**:
  - Rasterizes only the single currently visible page into a CPU frame buffer, dropping previous page buffers immediately on page turn or resize.
  - Automatic aspect-ratio fitting, centering, and crisp HiDPI / Retina scaling.
- **All-Pages Thumbnail Grid Overview (`Tab`)**:
  - Toggle between Single-Page View and a scrollable Thumbnail Grid (`140 x 198` cards).
  - **Drag-and-Drop Page Reordering**: Drag any page thumbnail onto/before another thumbnail (with a live cyan insertion bar) to reorder pages in-place.
  - **Per-Page Rotation**: Click the top-right `[R]` badge on any thumbnail (or press `R`) to rotate that page 90° clockwise.
  - **External PDF Drag-and-Drop Insertion**: Drop a `.pdf` file onto any thumbnail in Grid View to insert all its pages right before that page.
- **Left Bookmark Sidebar (`B`)**:
  - Toggle a 240px dark sidebar rendered with a built-in 8x8 bitmap font rasterizer.
  - Reads existing PDF outline bookmarks or `<filename>.bookmarks.txt` sidecars, and lets you jump to, add (`+`/`A`), edit (`E`/Right-Click), or delete (`[x]`/Middle-Click/`Del`) bookmarks interactively.
- **Interactive Mouse Text Highlighting**:
  - Left-click and drag across text in Single-Page View for a live semi-transparent yellow selection box; releasing bakes permanent highlight annotations and multiply-blend highlight paths onto the intersecting text lines.
- **Roman & Arabic Page Numbering (`N`)**:
  - Stamp lowercase Roman numerals (`i, ii, iii, iv, v...`) on pages `1..=K` and Arabic numerals (`1, 2, 3...`) after that at bottom-center.
  - Automatically purges existing page numbers in the margin zone before stamping.
- **Aligned Headers & Footers (`H`) + Margin Zone Purge (`C`)**:
  - 3-step interactive prompt supporting `HL` (Header-Left), `HC` (Header-Center), `HR` (Header-Right), `FL` (Footer-Left), `FC` (Footer-Center), and `FR` (Footer-Right).
  - Automatically clears old objects and applies a clean whiteout band in the target margin zone before stamping, or clears margins standalone via `C`.
- **Diagonal Watermarks (`W`), Blank Page Insertion (`I`), Split (`X`), & Merge (`M`)**:
  - Stamp 45° semi-transparent watermarks, insert blank A4 pages, extract arbitrary page ranges/lists (`1-3` or `2,4,6`), or merge PDFs.
- **PDF Stream Compression & Pruning (`Z`)**:
  - Prunes unused/orphaned PDF objects, strips dead revisions, removes zero-length streams, and compresses uncompressed streams into `<filename>_compressed.pdf`.
- **Safe In-Memory Loader**:
  - Reads PDFs into owned memory buffers (`std::fs::read` + `load_pdf_from_byte_vec`) with automatic empty-password (`Some("")`) fallback for read-only government/bank PDFs.

---

## Keyboard & Mouse Shortcuts

| Shortcut / Action | Mode | Description |
| :--- | :--- | :--- |
| `Right` / `Down` / `PageDown` / `Space` | Single-Page | Next page |
| `Left` / `Up` / `PageUp` | Single-Page | Previous page |
| `Home` / `End` | Single-Page | First page / Last page |
| **Mouse Wheel** | Single / Grid / Sidebar | Turn page (`120ms` debounce) / Scroll thumbnail grid / Scroll bookmarks |
| **Left-Click & Drag** | Single-Page | Select & highlight text lines in yellow |
| **Left-Click & Drag** | Thumbnail Grid | Drag thumbnail `A` before thumbnail `B` to reorder pages |
| **Double-Click** / `Enter` | Thumbnail Grid | Open selected thumbnail in Single-Page View |
| **OS Drag & Drop `.pdf`** | Single-Page | Open dropped `.pdf` as the active document |
| **OS Drag & Drop `.pdf`** | Thumbnail Grid | Insert all pages of dropped `.pdf` before target thumbnail (`Shift`+Drop opens) |
| `Tab` | Any | Toggle between **Single-Page View** and **All-Pages Thumbnail Grid** |
| `B` | Any | Toggle **Left Bookmark Sidebar** (`+`/`A` Add, `E`/Right-Click Edit, `[x]`/`Del` Delete) |
| `G` or `0`–`9` | Normal | Jump directly to any page number (`Enter` to confirm) |
| `N` | Normal | Stamp Roman (`1..=K`) + Arabic page numbers at bottom-center |
| `H` | Normal | Stamp aligned Header/Footer (`HL`, `HC`, `HR`, `FL`, `FC`, `FR`) across page range |
| `C` | Normal | Clear existing margin zones (`1` Footers/Page#, `2` Headers, `3` Both) |
| `W` | Normal | Stamp diagonal semi-transparent watermark across all pages |
| `I` | Normal | Insert a blank A4 page after the current page |
| `R` | Normal / Grid | Rotate current (or hovered thumbnail) page 90° clockwise |
| `D` | Normal | Delete current page |
| `X` | Normal | Split / extract page range (e.g. `1-3` or `2,4,6`) to a new PDF |
| `M` | Normal | Merge another PDF into the end of the current document |
| `Z` | Normal | Compress & optimize PDF (`<filename>_compressed.pdf`) |
| `O` / `S` | Normal | Open PDF file / Save modified PDF file |
| `?` / `F1` / `Esc` | Any | Toggle in-window Help Overlay / Cancel current prompt |

---

## Building from Source

### 1. Download Dynamic PDFium Library
MicroPDF dynamically binds to PDFium at runtime, looking in the executable's directory first and falling back to the current working directory (`./`).

Download the prebuilt PDFium shared library for your target platform from [`bblanchon/pdfium-binaries`](https://github.com/bblanchon/pdfium-binaries/releases):
- **Windows (`x86_64`)**: Extract `bin/pdfium.dll` next to `micro_pdf.exe` (or project root).
- **macOS (`arm64` / `x86_64`)**: Extract `lib/libpdfium.dylib` next to `micro_pdf` (or project root).
- **Linux (`x86_64`)**: Extract `lib/libpdfium.so` next to `micro_pdf` (or project root).

### 2. Build Native Release Binary
```bash
cargo build --release
```

### 3. Cross-Compile Portable Windows `.exe` from macOS/Linux (Optional)
Using [`cargo-xwin`](https://github.com/rust-cross/cargo-xwin):
```bash
cargo xwin build --release --target x86_64-pc-windows-msvc
```
Place `target/x86_64-pc-windows-msvc/release/micro_pdf.exe` and `pdfium.dll` in the same folder for a standalone, zero-install portable Windows bundle.

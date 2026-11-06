# MicroPDF

**MicroPDF** is an ultra-lightweight (**~12 MB RAM**), portable, zero-GPU-bloat PDF viewer and structural editor written in Rust using [`winit`](https://crates.io/crates/winit), [`softbuffer`](https://crates.io/crates/softbuffer), [`pdfium-render`](https://crates.io/crates/pdfium-render), and [`lopdf`](https://crates.io/crates/lopdf).

Designed for maximum speed, tiny binary size (**< 1 MB** executable), and strict single-page memory discipline, MicroPDF blits PDFium bitmaps directly into a CPU window buffer and uses the native OS Window Title Bar as a zero-overhead interactive HUD.

---

## Features

- **Strict Single-Page Viewport Virtualization (~12 MB RSS)**:
  - Rasterizes only the single currently visible page into a CPU frame buffer, dropping previous page buffers immediately on page turn or resize.
  - Automatic aspect-ratio fitting, centering, and crisp HiDPI / Retina scaling.
- **Multi-Document Tab System (Zero-RAM Inactive Tabs)**:
  - Open multiple PDFs simultaneously in a 28px top **Tab Bar** (`[ 1: fileA.pdf [x] ]  [ 2: fileB.pdf [x] ]  [ + ]`).
  - Only the active tab holds a rendered page or thumbnail cache; switching tabs frees cached bitmaps immediately.
- **Logical Page Numbering Synchronization (`/PageLabels` + Smart Jump `G`)**:
  - Stamp lowercase Roman numerals (`i, ii, iii, iv, v...`) on pages `1..=K` and Arabic numerals (`1, 2, 3...`) after that via `N`.
  - Automatically writes PDF `/PageLabels` into the Root Catalog on Save/Compress and restores `roman_end` when opening any PDF.
  - Displays both logical and physical page numbers (`Page 2 (5 of 10)` or `Page ii (2 of 10)`) and supports Smart Jump (`G`): type `5` for logical page 5, `ii` for Roman page 2, or `#5` for physical page 5.
- **Hierarchical Bookmark `.txt` Import (`L`) & Native PDF `/Outlines` Embedding**:
  - Import `--` delimited `.txt` bookmark files (`Serial -- Title -- LogicalPage`, e.g. `1 -- Chapter One -- 1` and `a -- Section A -- 2`) via hotkey `L`, clicking `[Import .txt]` in the Bookmark Sidebar (`B`), or dragging and dropping a `.txt` file into the window.
  - Top-level numeric chapters (`1, 2, 3`) render in bright white (`1. Chapter [p.1]`) and alphabetic sub-items (`a, b, c`) indent by 16px in soft cyan (`   a. Section [p.2]`).
  - Automatically re-syncs bookmark page indices whenever Roman/Arabic page numbering (`N`) is changed, and embeds the full hierarchical `/Outlines` tree into the PDF catalog on Save (`S`).
- **All-Pages Thumbnail Grid Overview (`Tab`) + Per-Page Grayscale (`[BW]` / `K`)**:
  - Toggle between Single-Page View and a scrollable Thumbnail Grid (`140 x 198` cards).
  - **Per-Page Grayscale (`[BW]`)**: Click the top-left `[BW]` badge on any thumbnail (or press `K`) to convert that page's text, paths, annotations, and `/DeviceRGB` raster images to 8-bit grayscale (`/DeviceGray`) in-place.
  - **Per-Page Rotation (`[R]`)**: Click the top-right `[R]` badge on any thumbnail (or press `R`) to rotate that page 90° clockwise.
  - **Drag-and-Drop Page Reordering & PDF Insertion**: Drag thumbnails to reorder pages live, or drop an external `.pdf` onto a thumbnail to insert its pages before that page.
- **OS-Native Multi-Step Undo (`Cmd+Z` on macOS, `Ctrl+Z` on Windows)**:
  - Maintains an in-memory per-tab undo stack (up to 8 states) across all mutating operations (Rotate, Delete, Insert, Highlight, Page Numbering, Header/Footer, Watermark, Clear Margins, Page Reorder, Drop-Insert, and `[BW]` conversion).
- **Interactive Mouse Text Highlighting**:
  - Left-click and drag across text in Single-Page View for a live semi-transparent yellow selection box; releasing bakes permanent highlight annotations and multiply-blend highlight paths onto the intersecting text lines.
- **Aligned Headers & Footers (`H`) + Margin Zone Purge (`C`)**:
  - 3-step interactive prompt supporting `HL` (Header-Left), `HC` (Header-Center), `HR` (Header-Right), `FL` (Footer-Left), `FC` (Footer-Center), and `FR` (Footer-Right), with automatic margin object removal and whiteout band.
- **Diagonal Watermarks (`W`), Blank Page Insertion (`I`), Split (`X`), Merge (`M`), & Compression (`Z`)**:
  - Stamp 45° semi-transparent watermarks, insert blank A4 pages, extract page ranges (`1-3` or `2,4,6`), merge PDFs, or prune & compress streams into `<filename>_compressed.pdf`.

---

## Keyboard & Mouse Shortcuts

| Shortcut / Action | Mode | Description |
| :--- | :--- | :--- |
| `Right` / `Down` / `PageDown` / `Space` | Single-Page | Next page |
| `Left` / `Up` / `PageUp` | Single-Page | Previous page |
| `Home` / `End` | Single-Page | First page / Last page |
| **Mouse Wheel** | Single / Grid / Sidebar | Turn page (`120ms` debounce) / Scroll thumbnail grid / Scroll bookmarks |
| **Top Tab Bar Click** / `Ctrl+Tab` | Any | Switch PDF tab, click `[x]` (or `Cmd/Ctrl+W`) to close tab, `[+]` to open PDF |
| **Left-Click & Drag** | Single-Page | Select & highlight text lines in yellow |
| **Left-Click & Drag** | Thumbnail Grid | Drag thumbnail `A` before thumbnail `B` to reorder pages |
| **Click `[BW]`** or press `K` | Normal / Grid | Convert current or clicked page to 8-bit Grayscale (Black & White) |
| **Click `[R]`** or press `R` | Normal / Grid | Rotate current or clicked page 90° clockwise |
| **Double-Click** / `Enter` | Thumbnail Grid | Open selected thumbnail in Single-Page View |
| **OS Drag & Drop `.pdf`** | Single / Grid | Open in new tab (Single View) / Insert pages before target thumbnail (Grid View) |
| **OS Drag & Drop `.txt`** | Any | Import `--` delimited hierarchical bookmarks into active PDF tab |
| `Tab` | Any | Toggle between **Single-Page View** and **All-Pages Thumbnail Grid** |
| `B` / `L` | Any | Toggle **Left Bookmark Sidebar** / Import `.txt` bookmarks (`Serial -- Title -- Page`) |
| `G` or `0`–`9` | Normal | Smart Jump to Logical page (`5`), Roman page (`ii`), or Physical page (`#5`) |
| `N` | Normal | Stamp Roman (`1..=K`) + Arabic page numbers & sync `/PageLabels` + bookmarks |
| `H` / `C` | Normal | Stamp aligned Header/Footer (`HL`,`HC`,`HR`,`FL`,`FC`,`FR`) / Clear margin zones |
| `W` | Normal | Stamp diagonal semi-transparent watermark across all pages |
| `I` / `D` | Normal | Insert blank A4 page after current page / Delete current page |
| `Cmd+Z` (macOS) / `Ctrl+Z` (Win) | Normal | Undo last structural or page edit (up to 8 steps per tab) |
| `X` / `M` / `Z` | Normal | Split page range / Merge PDF / Compress & optimize PDF (`<name>_compressed.pdf`) |
| `O` / `S` | Normal | Open PDF in new tab / Save modified PDF (embeds `/PageLabels` & `/Outlines`) |
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

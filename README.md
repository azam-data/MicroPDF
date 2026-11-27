# MicroPDF

**MicroPDF** is a fast, ultra-lightweight, portable PDF, Markdown, LaTeX & EPUB viewer and power editor written in Rust using [`winit`](https://crates.io/crates/winit), [`softbuffer`](https://crates.io/crates/softbuffer), [`pdfium-render`](https://crates.io/crates/pdfium-render), [`lopdf`](https://crates.io/crates/lopdf), [`image`](https://crates.io/crates/image), [`ttf-parser`](https://crates.io/crates/ttf-parser), [`rust_xlsxwriter`](https://crates.io/crates/rust_xlsxwriter), [`pulldown-cmark`](https://crates.io/crates/pulldown-cmark), and [`docx-rs`](https://crates.io/crates/docx-rs).

**Zero GPU. Zero GUI toolkit. Zero installer.** Every menu, tab, toolbar, modal dialog, and live preview is CPU-rendered directly into the window frame buffer with minimal RAM and zero OS breadcrumbs.

---

## Download & Portable Install (`v1.7.0`)

Grab the latest portable release from **[GitHub Releases](https://github.com/azam-data/MicroPDF/releases/latest)**:

1. Download **`MicroPDF-windows-x64.zip`** (and optional **`SHA256SUMS.txt`**).
2. Extract to any folder (or USB drive) — no installation or admin rights needed:
   ```text
   MicroPDF/
   ├── micro_pdf.exe          # Signed-metadata Windows executable (~2 MB)
   ├── pdfium.dll             # Bundled PDFium engine (~7.8 MB)
   ├── micropdf.ini           # Auto-created portable settings, favorites & history
   ├── micropdf_books.ini     # Auto-created portable EPUB highlights, notes & bookmarks
   └── micropdf_assets/       # Auto-created portable signatures & stamps
       ├── signatures/
       └── stamps/
   ```
3. *(Optional)* Verify integrity on Windows PowerShell:
   ```powershell
   Get-FileHash MicroPDF-windows-x64.zip -Algorithm SHA256
   ```

> **macOS / Linux:** Build natively from source with `cargo build --release` and place `libpdfium.dylib` / `libpdfium.so` from [pdfium-binaries](https://github.com/bblanchon/pdfium-binaries) next to the `micro_pdf` binary.

---

## What's New in v1.7.0

- **Classic vs. Distinct UI Skin + Multi-Family CPU Typography (`Help -> UI Skin: Classic / Distinct`):** Preserves MicroPDF's **Classic** UI 100%, while **Distinct** mode transforms the Light/Dark palette and workspace per document type using bundled zlib-compressed SIL OFL font families (`UiSans`, `BookSerif` Regular/Bold/Italic with kerning pairs, `CodeMono`, and lazy OS serif loading):
  - **PDF**: Precision slate + cobalt blue studio palette and streamlined bottom controls.
  - **EPUB**: Warm Apple Books / Foliate book atmosphere with line-level 2-page spread pagination, `hypher` syllable hyphenation & justification, **`Aa`** typography popover (`Book Serif`, `UI Sans`, OS serif fonts, `A-`/`A+`, spacing, margins, `Justify`/`Left`, `Hyph: On/Off`, `2-Page Book`/`1-Page Scroll`, 6 paper themes), Book Drawer (`Contents | Annotations | Bookmarks`, cover thumbnail, `(i)` details), and drag-to-Highlight / Add Note / Copy stored portably in `micropdf_books.ini`.
  - **Markdown**: Cool Writer/IDE palette with indigo accent, active-heading **Outline** sidebar, and live **Split** editor by default.
  - **LaTeX**: Warm stone Academic Typesetting palette with emerald-teal accent, **Document Structure** sidebar with equation count pill, and live **Split** editor by default.
- **Per-Tab File-Type Icons (Both `Classic` & `Distinct`):** Color-coded procedural badges (**PDF**, **EPUB** book, **Markdown**, **LaTeX**) appear on every open tab and row in the `▾` All-Tabs overflow list.
- **Markdown & LaTeX Studio Editor (`Split` Divider & `Pop Out ↗` Window):** Resizable vertical Split divider (`25%–75%`, double-click resets `50/50`), `CodeMono` monospace font, multi-line fenced code & math highlighting, active-line & matching-bracket highlights, a one-click Formatting/Snippet bar (`B`, `I`, `Code`, `H1–H3`, `List`, `Link`, `Table` / `\section`, `\textbf`, `\emph`, `$math$`, `Equation`, `\frac`, `\sum`), and **`Pop Out ↗`** to edit in a dedicated second window (`Dock Back` or closing the window restores Split View) with zero idle RAM when closed. Also supports `Tab`/`Shift+Tab`, auto-pairing, `Ctrl+B`/`Ctrl+I`, `Ctrl+Shift+D`, `Alt+Up/Down`, `Ctrl+/`, and `Ctrl+S`.
- **Magazine / Book Mode with Customizable Page-Turn Animation (`View -> Scroll: Magazine / Book` & Bottom-Bar Scroll Picker):** Front Cover (Page 1 alone) followed by two-page open book spreads (`2–3`, `4–5`, ...) with a central spine crease and customizable page-turn animation duration (`View -> Magazine Page-Turn Speed...`, `0–3000 ms`, default `450 ms`) across **PDF**, **EPUB**, **Markdown**, and **LaTeX**, plus uniform **60%–300%** zoom.
- **Export to EPUB (`.epub`):** Convert any PDF (`Export -> Export as EPUB (.epub)...`), Markdown, or LaTeX document into a spec-compliant EPUB 3 e-book with chapter splitting and clean typography.
- **Favorites Menu, Tab `★` Stars & Nested Folders (`Ctrl+D`):** Dedicated **Favorites** top menu plus a clickable `★` star inside every open tab to organize PDFs, Markdown, LaTeX, and EPUB documents into unlimited nested folders & subfolders.
- **Optional Encrypted Portable Config (`File -> Encrypt Portable Config (micropdf.ini)`):** Authenticated ChaCha20 + HMAC-SHA256 encryption so `micropdf.ini` (including Favorites, recent files, and session tabs) cannot be read in Notepad by anyone else while MicroPDF opens it automatically.
- **Markdown Math & LaTeX Equation Numbers:** Full `$...$` inline and `$$...$$` display math in Markdown, plus automatic `(1)`, `(2)` equation numbering and `\eqref`/`\ref` resolution for LaTeX `equation`/`align`/`gather` environments.
- **Complex-Graph Failsafe Guard:** Automatically scans `.tex` files before opening for unsupported vector-graph environments (`tikzpicture`, `pgfpicture`, `circuitikz`, `pspicture`, `asy`, `axis`/PGFPlots) and shows a clear warning without breaking the active tab.
- **Styled Markdown (`.md` / `.markdown`) Reader & Export (`PDF` + `.docx`):** Open Markdown files alongside PDFs via File → Open, drag-and-drop, or CLI. Features an editorial Light/Dark reading column, automatic `#`–`###` outline sidebar, GFM tables, code blocks with **Copy** button & horizontal scroll, clickable links, zoom (`Ctrl+`/`-`/`0`), live Reload, **Export to Styled PDF** (A4/Letter, repeated table headers, `Page X of Y`), and **Export to Styled Word (`.docx`)** via `docx-rs`.
- **Scrollable System Font Dropdown:** Compact, scrollable dropdown across **Watermark**, **Header & Footer**, and **Page Numbering** dialogs with mouse-wheel scrolling and automatic system font scanning via `ttf-parser`.
- **Hand Tool & Horizontal/Vertical Scrollbars (Normal & Compare Windows):** Click the **Hand** toolbar button (or **Hand** toggle in Compare Mode) to grab-and-pan zoomed pages in 2D, or drag the bottom horizontal and right vertical overlay scrollbars.
- **Full Compare Mode Viewing Modes:** Compare Two PDFs now supports **Fit Page**, **Fit Width**, **100% Actual Size**, and custom zoom (`25%–400%`) with lockstep 2D scrolling, panning, page offset, pane swap, and CPU **Visual Diff** overlay.
- **Split PDF to Single-Page PDFs:** Lossless folder export (`Tools -> Split to Single-Page PDFs...`) with All / Current / Custom Range and zero-padded `<prefix>_page_001.pdf` filenames.
- **Detailed Watermark Dialog with Live Preview (`W`):** Real-time rotated & alpha-blended preview with System Font Dropdown, size, 6 colors, `-180°..+180°` rotation (`-45°/0°/+45°` presets), opacity (`5%–100%`), Centered / Custom Position / Tiled Grid coverage, and page range.
- **Portable Stamp & Signature Manager (`P`):** Embedded freehand **Signature Pad** (Black/Blue ink, Clear, Save PNG), 5 customizable preset stamps (`APPROVED`, `CONFIDENTIAL`, `DRAFT`, `REVIEWED`, `COPY`), custom text & PNG stamps, Rename/Delete, and click-and-drag placement — stored portably in `./micropdf_assets/`.
- **Lightweight Matrix Page Deskew (`Page -> Deskew Page(s)...`):** Straighten tilted scans (`-15.0°..+15.0°`, `0.1°` step) with Draw Guideline auto-snap, alignment grid, and live preview using lossless 2D affine transforms (zero rasterization).
- **Hybrid Lattice + Stream PDF-to-Excel Engine (`Export -> Export to Excel (.xlsx)...`):** Vector gridline & thin-rectangle extraction with connected-component isolation for side-by-side tables, merged-cell detection, Stream gutter fallback, `/Rotate 90/180/270` landscape page normalization, financial comma-decimal parsing, and native desktop Microsoft Excel output via `rust_xlsxwriter`.

---

## Footprint & Memory Efficiency

PDFium reads documents on demand, inactive tabs hold zero rendered bitmaps, modals allocate nothing when closed, zoom above 2× uses a bounded 16 MB 512×512 tile LRU cache, and the thumbnail grid is capped at 300 entries.

| Raw Process RSS *(macOS release, 1920×1080, excluding ~8 MB window buffer)* | v1.2.1 | v1.3.0+ |
| :--- | :---: | :---: |
| Small text PDF: idle after open | — | **16.9 MB** |
| 116 MB, 100-page 300-dpi scan: idle after open | 129 MB | **50.6 MB** |
| Scan: peak while scrolling 50 pages | 228 MB | **118.2 MB** |
| Scan: peak at 400% zoom, panning | 274 MB | **134.4 MB** |
| Scan: peak in thumbnail grid | 301 MB | **187.8 MB** |

Printing a 20-page 600-dpi scan adds ~25 MB at peak (one decoded scan image plus one 5 MB strip).

---

## Key Features

### Reading, Navigation & UI
- **Compact 720p-Ready CPU UI (Light Default + Dark Mode, Classic / Distinct Skin):** Unified 32px menu bar (`File`, `View`, `Edit`, `Page`, `Tools`, `Export`, `Favorites`, `Help`) + customizable icon toolbar (`View -> Customize Toolbar...`, `View -> UI Scale`), `Help -> UI Skin: Classic / Distinct`, multi-family antialiased typography (`ab_glyph`), and instant Light/Dark switching.
- **Scalable Tabs & Unsaved-Changes Guard:** Multi-tab bar with color-coded per-tab file-type icons (`PDF`, `EPUB`, `Markdown`, `LaTeX`), overflow arrows, `▾` All-Tabs popover, tab right-click menu (`Close Other Tabs` / `Close Tabs to Right` / `Close All Tabs`), `Save / Discard / Cancel` guard on close, drag-and-drop Welcome Screen, and auto-reload on external file changes.
- **Flexible Layouts & Pan/Scroll:** `FitPage` (`1`), `FitWidth` (`2`), `FitTextWidth` (`3`), `TwoPageSpread` (`4`), reflowable `TextView` (`T`), plus independent **Single Page**, **Continuous Vertical (`5`)**, and **Continuous Horizontal Book** flows, `25%–800%` cursor-centered zoom, **Hand Tool**, and draggable **Horizontal & Vertical Scrollbars**.
- **6 Eye-Care Reading Modes (`V`):** `Normal`, `Paper E-Ink` (day), `Warm Sepia`, `Sage Eye-Care`, `Dark Invert`, and `Night Amber` (night), persisted in `micropdf.ini` alongside reading positions for up to 50 recent files.
- **Fast Search & Built-In Beginner Manual:** Floating `Ctrl/Cmd+F` search (`Aa` Case, `W` Whole Word, `F3`/`Shift+F3`), `Alt+Left`/`Alt+Right` navigation history, Smart Jump (`G` — logical `5`, Roman `iii`, or physical `#5`), and a 10-chapter offline **Step-by-Step User Guide** (`Help -> User Guide (How to Use)`) with `.txt` export.

### Editing, Annotations, Forms & Security
- **Annotations & 8-Step Undo/Redo:** Text select & copy (`Ctrl+C`), Highlight, Underline, Strikethrough, Sticky Notes (with reader popover), Freehand Pen, Text Boxes, Rectangles, and Arrows in 7 colors; **right-click any annotation** to edit, recolor, or delete; `Ctrl+Z` / `Ctrl+Y` (`Ctrl+Shift+Z`) Undo/Redo.
- **Interactive PDF Forms:** Click Checkboxes, Radio Buttons, and Text Fields directly in Select mode with Undo, Save, and Print support.
- **True Black-Box Redaction & Privacy Scrubber:** Strips underlying text, images, form fields, `/Info`, and `/StructTreeRoot` on save, plus one-click metadata & XMP scrubbing (`File -> Document Properties & Privacy...`).
- **Page & Document Tools:** Thumbnail Grid (`Tab`) with `Ctrl/Shift+Click` multi-select batch Rotate / B&W / Extract / Delete, right-click insert blanks or external PDFs before/after, **Combine PDFs (`Shift+M`)**, **Split/Extract (`X`)**, **Split to Single-Page PDFs**, **Matrix Deskew**, **Quick Measure / Ruler**, **Header & Footer Profiles (`H`)**, **Page Numbering (`N`)**, **Detailed Watermark (`W`)**, **Stamps & Signatures (`P`)**, **Bookmark Sidebar (`B`)** with Auto-Detect & `.txt` Import/Export, and **Flatten & Unlock (`U`)**.

### Export, Conversion, Printing & Optimize
- **Offline Export Menu (`Export`):** Convert to **Word (`.docx`)**, **Excel (`.xlsx`)** (Hybrid Lattice + Stream engine via `rust_xlsxwriter`), **Markdown (`.md`)**, **LaTeX (`.tex`)**, or **PNG/JPEG Images** (150/200/300 DPI), plus **Create PDF from Images (`File -> Create PDF from Images...`)**.
- **Live Print Preview (`Ctrl/Cmd+P`):** 10 Paper Sizes, `With Annots` / `No Annots`, Auto-Trim + `100%–200%` Print Zoom, automatic Duplex, guided **Manual 2-Sided** for any 1-sided printer, and foldable **Booklet** printing.
- **Local PDF Optimizer (`Z` / `Tools -> Optimize...`):** Writes `<name>_optimized.pdf` without touching the original. Supports Smaller (150 dpi), Balanced (200 dpi), Print (300 dpi), grayscale/1-bit B&W, ink-preserving scan whitening, and live page preview (no Ghostscript or OCR required).

---

## Keyboard & Mouse Shortcuts

| Shortcut / Action | Mode | Description |
| :--- | :--- | :--- |
| **Top Menu Bar Click** | Any | Open `File`, `View`, `Edit`, `Page`, `Tools`, `Export`, `Favorites`, or `Help` |
| `Help -> UI Skin` | Any | Toggle between **Classic** (original UI) and **Distinct** (per-document studio & book UI) |
| `1` / `2` / `3` / `4` / `5` / `T` | Page View | `1` Fit Page, `2` Fit Width, `3` Fit Text, `4` Two-Page, `5` Continuous Scroll, `T` Text View |
| `Ctrl/Cmd+Wheel` / `+` / `-` / `0` | Any | Zoom (`25%–800%` PDF, `60%–300%` EPUB/MD/LaTeX) or reset to `100%` (`0` / `Ctrl+0`) |
| **Hand Tool** / **Scrollbars** | Normal / Compare | Click the **Hand** icon to drag-pan in 2D, or drag bottom Horizontal / right Vertical scrollbars |
| `Ctrl+Tab` / `Ctrl+1..9` / `Ctrl+W` | Any | Switch tabs, jump to tab 1–9, or close tab (with unsaved-changes guard) |
| `F11` / `F5` / `Esc` | Any | Fullscreen (`F11`), Presentation Mode (`F5`), or return to Select / close modal (`Esc`) |
| `Alt+Left` / `Alt+Right` / `Alt+Up/Down` | View / Editor | History Back/Fwd in Page View; Move current line Up/Down in `.md`/`.tex` Editor |
| **Drag** / `Aa` / `★` | Distinct EPUB | Drag across words to Highlight / Add Note / Copy; click `Aa` for typography; click bottom `★` to bookmark |
| **Split Drag** / `Pop Out ↗` | `.md` / `.tex` Studio | Drag Split divider (`25%–75%`, double-click resets `50/50`); click `Pop Out ↗` / `Dock Back` for 2nd window |
| `Ctrl+B` / `I` / `Shift+D` / `/` / `S` | `.md` / `.tex` Editor | Bold, Italic, Duplicate line, Toggle comment, Save directly to disk |
| **Left-Click / Drag / Right-Click** | Page View | Select text (`Ctrl+C`), fill Form Fields, or right-click annotations to Edit/Recolor/Delete |
| `Tab` / `Ctrl+Click` / `Shift+Click` | Any / Grid | Toggle Thumbnail Grid (`Tab`); multi-select thumbnails for batch actions |
| `Ctrl/Cmd+F` & `F3` / `Shift+F3` | Any | Incremental search (`Aa` Case, `W` Whole Word) & Next / Previous match |
| `V` | Page View | Cycle 6 Eye-Care Reading Modes (`Paper E-Ink`, `Night Amber`, Sepia, Sage, Dark, Normal) |
| `W` / `P` / `N` / `H` | Normal | Detailed Watermark (`W`), Stamps & Signatures (`P`), Page Numbers (`N`), Header/Footer (`H`) |
| `Shift+M` / `X` / `Z` / `U` | Any | Combine PDFs (`Shift+M`), Split/Extract (`X`), Optimize (`Z`), Flatten & Unlock (`U`) |
| `B` / `A` / `L` / `G` | Any / Normal | Sidebar (Book Drawer / Outline / Bookmarks) (`B`), Add Bookmark (`A`), Import `.txt` (`L`), Smart Jump (`G`) |
| `K` / `I` / `R` / `D` | Normal / Grid | Page B&W (`K`), Insert Blank (`I`), Rotate 90° (`R`), Delete Page (`D`) |
| `Ctrl+Z` / `Ctrl+Y` (`Ctrl+Shift+Z`) | Normal | Undo / Redo up to 8 steps per tab |
| `Ctrl/Cmd+P` / `O` / `S` / `Ctrl+Shift+S` | Any | Print Preview or Export (`Ctrl+P`), Open PDF/`.md`/`.tex`/`.epub` (`O`), Save (`S`/`Ctrl+S`), Save As (`Ctrl+Shift+S`) |
| `?` / `F1` / `Help -> User Guide` | Any | Shortcut Sheet (`?` / `F1`) or 10-chapter Step-by-Step User Guide |

---

## Building & Testing from Source

Keep `micro_pdf.exe` and `pdfium.dll` (Windows) or `micro_pdf` and `libpdfium.dylib` / `libpdfium.so` (macOS / Linux) in the same folder. PDFium is loaded **only** from the executable directory (or `MICROPDF_PDFIUM_DIR` during development).

```bash
# Release build (macOS / Linux / Windows)
cargo build --release --locked

# Cross-compile Windows x64 from macOS / Linux
cargo xwin build --release --locked --target x86_64-pc-windows-msvc

# Run unit tests (set MICROPDF_PDFIUM_DIR if PDFium is not beside target/debug)
cargo test --locked

# Realistic memory & print RSS benchmarks
cargo test --release ram_realistic -- --ignored --nocapture --test-threads=1
cargo test --release ram_print -- --ignored --nocapture --test-threads=1
```

---

## Contributing & Updating the Built-In User Guide

Before contributing or opening a pull request, please read **[`AGENTS.md`](AGENTS.md)** and **[`docs/USER_GUIDE_FRAMEWORK.md`](docs/USER_GUIDE_FRAMEWORK.md)**. Any change that adds, renames, or alters user-facing features, menus, dialogs, or shortcuts **must** update the built-in beginner manual in [`src/viewer/user_guide.rs`](src/viewer/user_guide.rs) and the shortcut sheet in [`src/viewer/overlays.rs`](src/viewer/overlays.rs).

## License

MicroPDF is released under the MIT License (see [`LICENSE`](LICENSE)). Third-party components, including PDFium, retain their own licenses (see [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md)).

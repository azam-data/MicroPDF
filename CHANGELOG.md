# Changelog

## 1.3.0

### New
- **Printing (`Ctrl/Cmd+P`, `File -> Print...`).** Prints the document as it is in the app, so unsaved edits and redactions are what reaches the paper.
  - **Windows:** the native print dialog (page range, copies), then GDI. One page is rendered and sent per frame, so the window stays responsive; the status bar shows `Printing page 3 / 20` and `Esc` cancels. Each page is rendered at the printer's resolution in 256-row strips (about 5 MB at 600 dpi instead of about 140 MB for a whole page), fitted to the printable area, with the paper turned to match portrait or landscape pages.
  - **macOS:** a print bar at the bottom (printer, copies, page range such as `1-3,5,8-`, `F` toggles fit-to-page) that hands a temporary PDF to `lp` in the background. The temporary file is deleted as soon as `lp` has it, and also after any error.
  - Filled-in form fields are printed. Temporary print files left behind by a crash are removed the next time the app starts.
- **`Tools -> Optimize...`**: choose a preset (Smaller 150 dpi, Balanced 200 dpi, Print 300 dpi), colour, gray or black-and-white (with a warning that signatures look jagged), and whether to clean scans, in the bottom bar. Runs one image per frame with progress and `Esc` to cancel (nothing is written), shows before/after sizes, and can preview the current page first. `Z` still runs Balanced directly.

### Memory
- **Large files are no longer loaded into memory.** PDFium now reads the file on demand. On a 116 MB, 100-page scan: 50 MB after opening (was 129), 118 MB peak while scrolling (was 228), 134 MB at 400% zoom (was 274) and 188 MB in the thumbnail grid (was 301), all within the target of file size + 120 MB.
- Zoom tiles and thumbnails reuse their buffers, and each page is closed right after it is drawn, so PDFium's image cache for that page is freed.

### Fixed
- **A file overwritten by another program while open** (for example `cp other.pdf open.pdf`) could make pages you had not viewed yet show the other file's content. Such pages now show an error in the status bar instead, and auto-reload opens the new file.
- **Saving no longer rewrites a file in place.** Where the file system refuses to replace a file that is open (FAT32/exFAT USB sticks, some network shares), the app closes its own copy and tries again; if it still fails, the original is left untouched and the status bar names the saved copy next to it (`<name>.micropdf-save.pdf`). Tabs with unsaved edits of their own are never closed for this.
- **Filled-in form fields disappeared when zoomed in** above 2x the screen size.
- A page that cannot be drawn now leaves that page blank with an error in the status bar instead of skipping the whole window redraw.
- A save that failed while writing left its temporary file behind.

### Security
- **Single instance:** the token in the lock file is now 16 random bytes from the operating system (it was based on the time and process ID), and the lock file is readable only by you on macOS. Only existing `.pdf` files are opened. A program that connects and sends nothing, or too much, can no longer freeze the window; it is dropped after one frame.

### Tests
- 55 tests (plus 3 memory measurements run on demand), including: files changed, overwritten or deleted while open; saving when the file system refuses to replace an open file; printing state, page ranges, strip sizes and temporary-file cleanup without a printer; form fields in every view; single-instance abuse cases.
- `test_single_instance_socket_forwarding_under_200ms` now polls until the forwarded file opens within its existing 200 ms budget, because reads no longer block (agreed change), and also checks the file is not opened twice.
- New on-demand measurement: `ram_print_600dpi_scan_measurement`.

## 1.2.1

### New
- **Optimize (`Z`) replaces Compress.** Downsamples and re-encodes images (Balanced: 200 dpi, JPEG 70), whitens the paper and sharpens text on scanned pages while keeping ink colour, and writes `<name>_optimized.pdf`. Nothing is written if the file would not get smaller. Images are edited in place, so page content, stacking order and images shared across pages are untouched. Gray and 1-bit black-and-white modes and Smaller/Print presets exist in the engine (`OptimizeOptions`) for the upcoming Optimize menu. `OptimizeJob::step()` processes one image at a time so the UI can avoid freezing.

### Fixed
- **B/W button did nothing to colour photos.** lopdf 0.34 refuses to decompress any image stream, so the image half of B/W never ran; JPEG and ICC-profiled images were also excluded. Images are now converted through PDFium.
- **Auto-reload kept the old undo history**, so Ctrl+Z after a reload could silently restore the previous file content. The history is now cleared on reload.
- **Opening a file parsed it twice.** Page labels were read by loading the whole file again with lopdf on every open and auto-reload; they now come from PDFium.

### Tests
- The Phase 4 RAM test used a 0.93 MB "300-page scan" and subtracted an overhead figure from its readings. It now only checks cache limits. A new `ram_realistic_measurement` (run on demand) measures raw memory on a real-size scan and fails on the known zoom and grid overruns.
- New optimizer tests: whitening, signature colour, orientation, text stacking, 1-bit output, B/W on JPEG photos.

## 1.2.0

### New Features
- **Continuous Scroll (`5` key & `View -> Continuous Scroll`)**: Vertical page stack with 12 px gaps; page number follows the page at the viewport centre. Caches only visible pages ± 1 and evicts everything else immediately.
- **Cursor-Anchored Zoom (`Ctrl/Cmd + Wheel`, `+`, `-`, `0`, 25%–800%)**: Smooth zoom around the cursor position with horizontal/vertical panning. Above 2× the screen size, never allocates a full-page bitmap — renders only visible `512 × 512` tiles in a 48 MB LRU tile cache.
- **PDF Links & Navigation History (`Alt+Left` / `Alt+Right`)**: Click internal links to jump to the target page; external `http://`, `https://`, and `mailto:` links open in the OS browser after status-bar `Enter` confirmation. Shows a pointer hand cursor over links and keeps a 50-entry `(page, scroll)` back/forward history.
- **Text Selection & Copy (`Mouse Mode: Select Text`, `Ctrl/Cmd+C`)**: Default mouse mode is now `Select Text` (drag to select text with translucent blue highlight, double-click to select a word, `Ctrl/Cmd+C` to copy to the OS clipboard). Character boxes are loaded for the current page only and freed on page change. `Highlight Text` remains in the `Edit` menu.
- **Masked Password Prompt (`InputMode::PasswordPrompt`)**: Opening an encrypted PDF prompts for the password in the bottom command bar (masked with `*`, up to 3 attempts, zeroized immediately after use and never stored).
- **Session Restore & Recent Files (`micropdf.ini` & `File -> Recent`)**: Persists open tabs `(path, page, scroll, zoom, view_mode)` and the last 10 recent files in `micropdf.ini` next to the executable (falling back to the OS config folder with a status-bar notice if read-only). Launching without a file offers `Enter` to restore the previous session.
- **Single-Instance Tab Forwarding**: Launching a second `micro_pdf <file.pdf>` process forwards the file path over a local loopback socket (`127.0.0.1` with a random token in `micropdf_instance.lock`) to the running window to open as a new tab and exits in `< 200 ms`.
- **Focused Auto-Reload (LaTeX / Typst Live Preview)**: Polls the active file's modification time every 1 s while the window is focused. Automatically reloads and preserves `(page, scroll)` if there are no unsaved edits, or shows a status-bar warning if unsaved edits exist.
- **Fullscreen (`F11`) & Presentation Mode (`F5`)**: `F11` toggles borderless fullscreen with menu/tab/status bars and sidebar hidden. `F5` enters presentation mode (fullscreen, single page fit to screen, solid black background, arrows or left/right click to advance/step back, `Esc` to exit and restore view state).
- **Time-Sliced Incremental Search (`Ctrl/Cmd+F`, `Esc` to cancel)**: Searches ~8 ms of pages per frame on the main thread so 1,000+ page PDFs never freeze the UI, displaying live progress (`Searching '...' (Page X/Y)`) in the status bar and keeping only the `Vec<SearchMatch>` list in RAM.

### Memory & Review Fixes
- ** Modular Viewer Architecture**: Split `src/viewer.rs` into `src/viewer/{mod.rs, state.rs, input.rs, menu.rs, render.rs, tabs.rs, grid.rs, bookmarks.rs, statusbar.rs}`.
- **Direct BGRx In-Place Rendering**: Renders PDFium bitmaps directly into a reused `Vec<u32>` via `PdfBitmap::from_bytes` and masks the high byte in place, eliminating the intermediate `image::RgbaImage` copy on every page render.
- **Thumbnail Grid Cache Eviction**: Evicts thumbnails outside visible rows ± 1 screen and caps the thumbnail cache at 300 entries (~33 MB max).
- **Stamp Image Cap & Zero-Copy**: Downscales stamp images larger than 2,000 px on their longest side and passes `&stamp.image` by reference without cloning.
- **Clean Startup & Sidecar Discipline**: Launching with no file opens an empty window with an `"Open PDF (O)"` hint instead of creating `sample_test.pdf` on the Desktop; stopped writing `.bookmarks.txt` sidecars next to user PDFs by default; removed the destructive page-rebuild branch in `compress_doc` so bookmarks, page labels, and form fields are preserved.

## 1.1.0

### Security
- **Redaction no longer leaks image content.** v1.0 removed only text and path objects, so a scanned page kept its full image under the black box. Image pixels under the box are now painted black inside the image; rotated or undecodable images are removed whole. Form XObjects, shadings and overlapping annotations are removed too. Simple background shapes that fully enclose the box are kept.
- **Redacted documents save through a strict path:** orphaned objects are pruned, and `/Info` and page thumbnails are removed. If that cleanup fails, nothing is written.
- **PDFium is loaded only from the executable's folder** (DLL-planting fix). Dev override: `MICROPDF_PDFIUM_DIR`.
- **Flatten & Unlock asks for confirmation on signed PDFs** (press `U` twice), because it invalidates signatures.

### Reliability
- Saves are atomic (temp file + rename), so a crash or full disk can't leave a truncated PDF.
- Metadata sanitizing and Flatten & Unlock report failures instead of failing silently.
- Password-protected PDFs show a clear message.

### Memory
- Undo stacks are moved between tabs instead of deep-cloned on every tab switch.
- Undo snapshots are capped at 64 MB per tab (the newest snapshot is always kept).
- Opening a file no longer makes a full extra copy of it in RAM.

### Other
- Non-ASCII bookmark titles (e.g. Bengali) are written as UTF-16, so other readers display them correctly.
- New test: `test_redaction_leaves_no_trace`.
- Added `LICENSE` (MIT), `THIRD_PARTY_NOTICES.md`; README footprint claims corrected.

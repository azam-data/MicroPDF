================================================================================
                        MICRO_PDF — PORTABLE PDF EDITOR
               Ultra-Lightweight CPU-Rendered PDF Viewer & Editor
================================================================================

PORTABLE USAGE
--------------
- Keep `micro_pdf` and `libpdfium.dylib` (on macOS) or `micro_pdf.exe` and
  `pdfium.dll` (on Windows) together in the same folder.
- No installation or admin rights required!
- Double-click `micro_pdf` (or run `./micro_pdf [file.pdf]`) to launch, or
  drag-and-drop any `.pdf`, bookmark `.txt`, or `.png`/`.jpg` stamp into the window.


TOP MENU BAR & BOTTOM COMMAND/STATUS BAR (Zero-RAM CPU Rendered)
----------------------------------------------------------------
- Top 24px Menu Bar (`#1E1E1E`) with 6 clickable dropdown menus:
  `[ File ]  [ View ]  [ Edit ]  [ Page ]  [ Tools ]  [ Help ]`
- Click any menu header to open its dropdown (`#252526` with `#007ACC` hover)
  and click any item to run that command immediately.
- Bottom 22px Command & Status Bar displays live interactive prompts (Search,
  Jump, Page Numbers, Header/Footer, Clear Margins, Watermark, Split, Bookmarks)
  with a visible cursor inside the window, as well as live page/mode/RAM stats.


MULTI-DOCUMENT TAB BAR (Zero-RAM Inactive Tabs)
-----------------------------------------------
- 28px Tab Bar below the Menu Bar displays all open PDFs (`[ 1: fileA.pdf [x] ]  [ + ]`).
- Inactive tabs hold ZERO rendered page or thumbnail bitmaps in RAM!
- Left-Click a tab            : Switch active PDF tab immediately.
- Click `[x]` (or Cmd/Ctrl+W) : Close tab.
- Click `[+]` (or press `O`)  : Open a new PDF in a new tab.
- Press `Ctrl+Tab`            : Cycle to the next open PDF tab.


FIVE VIEW MODES (`1`, `2`, `3`, `4`, `5` Keys & `View` Menu) & ZOOM
-------------------------------------------------------------------
  `1` : Fit Full Page         — Fits the entire page inside the viewport.
  `2` : Fit Page Width        — Scales page width to fill the viewport (minus 24px
                                padding). Mouse wheel / Up / Down arrows smoothly
                                scroll vertically and transition across pages!
  `3` : Fit Text Width        — Automatically detects union bounding box of page
                                objects to trim left/right margins and zooms so
                                the text column fills the viewport width!
  `4` : Two-Page Side-by-Side — Renders `current_page` (left) and `current_page + 1`
                                (right) side-by-side with a 12px gap; Next/Prev
                                advances 2 pages at a time.
  `5` : Continuous Scroll     — Vertical page stack with 12px gaps; caches only
                                visible pages +-1 in RAM.
  `Ctrl/Cmd+Wheel`, `+`, `-`  — Zoom in/out (`25%..=800%`) around cursor (`0` resets).
  `F11` / `F5` / `Esc`        — Toggle Fullscreen (`F11`) or Presentation Mode (`F5`).
  `Tab` : Toggle All-Pages Thumbnail Grid Overview (`140 x 198` cards, capped at 300).


NAVIGATION, LINKS, SELECT/COPY & SHORTCUTS
------------------------------------------
  Right / Down / PageDown / Space : Next Page (or scroll down in FitWidth/Cont.)
  Left / Up / PageUp              : Previous Page (or scroll up in FitWidth/Cont.)
  Alt+Left / Alt+Right            : Back / Forward in link & jump history (max 50)
  Click Link on Page              : Jump to internal page or confirm external URL
  Drag / Double-Click Text        : Select text / word (`Ctrl/Cmd+C` copies to clipboard)
  Home / End                      : First Page / Last Page
  G (or type digits 0-9)          : Smart Jump to Page -> Enter:
                                    - Type Arabic `5` -> Jumps to Logical Page 5
                                    - Type Roman `ii` -> Jumps to Roman Page ii
                                    - Type `#5`       -> Jumps to 1-based Physical Page 5
  B                               : Toggle Left Bookmark Sidebar (240px panel)
  L                               : Import Hierarchical Bookmarks from `.txt` file
  Cmd+Z (macOS) / Ctrl+Z (Win)    : Undo last structural or page edit (up to 8 steps)
  ? or F1                         : Toggle in-window Keyboard & Mouse Help Overlay
  Esc                             : Exit Fullscreen/Presentation, Cancel Search/Prompt


6 ZERO-RAM POWER EDITOR FEATURES
--------------------------------
  1. Text Search (`Ctrl+F` / `Cmd+F`, `F3` / `Ctrl+G` / `Cmd+G`):
     - Type a search query and press Enter to search `page.text()` across pages,
       jump to the matching page, and highlight matches in bright orange/cyan.
     - Press `F3` (or `Ctrl+G` / `Cmd+G`) to jump to the next match.

  2. True Black-Box Redaction & Metadata Sanitizer (`Edit -> Mouse Mode: Black-Box Redact`):
     - Drag a red selection box across the page. On release, MicroPDF deletes
       any underlying text or path objects intersecting that rectangle, draws a
       permanent solid black rectangle (`0, 0, 0`), and strips `/Author`,
       `/Creator`, `/Producer`, and `/Keywords` metadata on save.

  3. Visual Page Cropping (`Edit -> Mouse Mode: Crop Page Area`):
     - Drag a green box across the page to crop `/CropBox` to that sub-rectangle.
     - Restore anytime via `Page -> Reset Page Crop` or `Cmd/Ctrl+Z` Undo.

  4. Signature / Image Stamp (`P` Key or `Tools -> Stamp Signature / Image...`):
     - Pick any `.png` (preserving alpha transparency) or `.jpg`/`.jpeg` image,
       then click or drag on the page to stamp your signature/image at that
       exact PDF location.

  5. Dark Mode / Sepia Reading Filter (`V` Key or `View` Menu):
     - Cycles `Normal` -> `DarkInvert` -> `WarmSepia` directly in the final CPU
       blit loop (0 extra RAM, 0 changes to the underlying PDF file!).

  6. Flatten & Unlock PDF (`U` Key or `Tools -> Flatten & Unlock PDF`):
     - Flattens all annotations and form fields into page content streams and
       strips `/Encrypt`, `/AcroForm`, and `/Perms` restrictions.


BATCH COMBINE & DOCUMENT STRUCTURAL TOOLS
-----------------------------------------
  Shift+M : Batch Combine Multiple PDFs (`File -> Combine Multiple PDFs...`):
            Pick 2 or more PDF files in the file dialog to merge all their pages
            in order into a new `"Combined.pdf"` tab!
  M       : Merge another PDF file into the end of the current document.
  N       : Stamp Page Numbers (Roman numerals `i, ii, iii...` on pages `1..=K`,
            followed by Arabic numerals `1, 2, 3...` at bottom-center; clears
            existing page numbers, updates logical page labels, and re-syncs
            bookmarks).
  H       : Stamp or Replace Aligned Header / Footer (3-step prompt):
            - Step 1/3: Page range (e.g. `1-5` or `all`)
            - Step 2/3: Position (`HL`, `HC`, `HR`, `FL`, `FC`, `FR`)
            - Step 3/3: Header/Footer text (or empty + Enter to erase)
  C       : Clear Existing Margin Zones (`1` = Footers/Page#, `2` = Headers,
            `3` = Both; optional range e.g. `1 1-5`).
  W       : Stamp Diagonal Semi-Transparent Watermark across all pages.
  K       : Convert current (or hovered thumbnail) page to 8-bit Grayscale (`[BW]`).
  I       : Insert Blank A4 Page right after the current page.
  R       : Rotate current (or hovered thumbnail) page 90° clockwise.
  D       : Delete current page.
  X       : Split / Extract Page Range (e.g. `1-3` or `2,4,6`) into a new PDF file.
  Z       : Optimize PDF: downsample/recompress images, clean scans (`<filename>_optimized.pdf`).
  O       : Open PDF File in a new tab.
  S       : Save Modified PDF (sanitizes metadata and embeds `/PageLabels` &
            hierarchical `/Outlines` bookmarks).
================================================================================

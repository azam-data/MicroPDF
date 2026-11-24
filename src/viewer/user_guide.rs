//! Tier 3: Built-in Offline Step-by-Step User Guide (`Help -> User Guide (How to Use)`).
//!
//! Written in plain everyday language for beginners and users with little IT experience.
//! Uses zero extra dependencies, ~20 KB static text, and 0 KB idle RAM when closed.
//!
//! Maintenance rule: Whenever any feature, menu item, dialog, or shortcut changes, update
//! the matching chapter(s) below following `docs/USER_GUIDE_FRAMEWORK.md` (see `AGENTS.md`).

use super::render::{blend_rect_rgba, fill_rect_clipped};
use super::state::*;
use super::ui_draw::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum GuideBlock {
    Heading(&'static str),
    Paragraph(&'static str),
    Step { number: u8, text: &'static str },
    Tip(&'static str),
}

pub(super) struct GuideChapter {
    pub(super) number: u8,
    pub(super) short_title: &'static str,
    pub(super) title: &'static str,
    pub(super) summary: &'static str,
    pub(super) blocks: &'static [GuideBlock],
}

pub(super) const USER_GUIDE_CHAPTERS: &[GuideChapter] = &[
    // =========================================================================
    // Chapter 1
    // =========================================================================
    GuideChapter {
        number: 1,
        short_title: "1. Welcome & Opening Files",
        title: "Welcome & Opening Files",
        summary: "How to open a PDF, create a PDF from photos, save safely, and undo mistakes.",
        blocks: &[
            GuideBlock::Paragraph(
                "MicroPDF is a fast, private PDF reader and editor. Everything runs directly on your computer — your files are never uploaded to the internet, and nothing is installed into Windows or macOS.",
            ),
            GuideBlock::Heading("How to Open a PDF or Markdown File (3 Easy Ways)"),
            GuideBlock::Step {
                number: 1,
                text: "Drag and drop any PDF (.pdf) or Markdown (.md / .markdown) file right onto the MicroPDF window.",
            },
            GuideBlock::Step {
                number: 2,
                text: "Or click the blue 'Open PDF (O)' button on the Welcome Screen (or click any file under Recent Files).",
            },
            GuideBlock::Step {
                number: 3,
                text: "Or click File -> Open PDF in Tab (O) at the top-left corner, pick any PDF or Markdown file, and click Open.",
            },
            GuideBlock::Heading("How to Create a New PDF from Photos or Scanned Images"),
            GuideBlock::Step {
                number: 1,
                text: "Click File -> Create PDF from Images... at the top-left menu.",
            },
            GuideBlock::Step {
                number: 2,
                text: "Click '+ Add Images...' (or drag PNG/JPG photos onto the window) and use Up / Down to arrange the order.",
            },
            GuideBlock::Step {
                number: 3,
                text: "Choose page size (Fit A4, Fit Letter, or Match Image Size) and click 'Create PDF'. Your new PDF opens in a new tab ready to save.",
            },
            GuideBlock::Heading("How to Save Your Work Safely"),
            GuideBlock::Step {
                number: 1,
                text: "When you make any change, a small blue dot appears on the tab name so you know you have unsaved edits.",
            },
            GuideBlock::Step {
                number: 2,
                text: "Click File -> Save PDF (or press S) to save changes into the file, or click File -> Save As... (Ctrl+Shift+S) to keep the original untouched and save a new copy.",
            },
            GuideBlock::Step {
                number: 3,
                text: "Made a mistake? Click Edit -> Undo (Ctrl+Z) to step backward, or Edit -> Redo (Ctrl+Y / Ctrl+Shift+Z) to bring a change back.",
            },
            GuideBlock::Tip(
                "Safety Net: If you try to close a tab or exit MicroPDF with unsaved edits, a clear 'Save / Discard / Cancel' dialog will always ask you first so you never lose work by accident.",
            ),
        ],
    },
    // =========================================================================
    // Chapter 2
    // =========================================================================
    GuideChapter {
        number: 2,
        short_title: "2. Reading, Zoom & Eye Care",
        title: "Reading Comfort, Zoom & Page Views",
        summary: "Make text larger, choose smooth scrolling or book style, and protect your eyes day or night.",
        blocks: &[
            GuideBlock::Heading("Choosing How Pages Fit on Your Screen"),
            GuideBlock::Step {
                number: 1,
                text: "Click View in the top menu (or the view icons on the bottom bar) to pick a page layout:",
            },
            GuideBlock::Paragraph(
                "• Fit Full Page (1): Shows one whole page at once.\n• Fit Page Width (2): Makes the page fill your screen from left to right.\n• Fit Text Width — Crop Margins (3): Zooms straight onto the text and hides empty white page borders.\n• Two-Page Side-by-Side (4): Shows two pages next to each other like an open book.\n• Text View (T): Turns dense PDFs into clean, easy-to-read reflowable text.",
            ),
            GuideBlock::Heading("Choosing How Pages Scroll (Independent of Page Layout)"),
            GuideBlock::Step {
                number: 1,
                text: "Click View -> Scroll: Single Page, Continuous Vertical (5), or Continuous Horizontal Book.",
            },
            GuideBlock::Step {
                number: 2,
                text: "You can mix and match! For example, choose Two-Page Side-by-Side (4) together with Continuous Vertical (5), or flip horizontally like a book.",
            },
            GuideBlock::Heading("Zooming In & Out"),
            GuideBlock::Step {
                number: 1,
                text: "Hold Ctrl (or Cmd on Mac) and roll your mouse wheel up to zoom in, or down to zoom out (25% to 800%).",
            },
            GuideBlock::Step {
                number: 2,
                text: "Or press + to zoom in, - to zoom out, and 0 (or Ctrl+0) to reset zoom back to 100%. When zoomed in, click the Hand Tool icon on the top toolbar to grab and drag the page in any direction, or drag the horizontal scrollbar along the bottom edge (press Esc to return to Select Text).",
            },
            GuideBlock::Heading("Eye-Care Reading Modes (Day & Night)"),
            GuideBlock::Step {
                number: 1,
                text: "Click the reading mode pill on the bottom bar (or press V) to choose from 6 comfortable paper tones:",
            },
            GuideBlock::Paragraph(
                "• Paper E-Ink (Best for day): Soft matte gray paper that cuts white screen glare.\n• Night Amber (Best for night): Deep dark background with warm amber text to minimize blue light.\n• Also available: Normal, Warm Sepia, Sage Eye-Care, and Dark Invert.",
            ),
            GuideBlock::Tip(
                "MicroPDF automatically remembers your favorite Reading Mode and the last page & zoom for up to 50 files you have read! You can also make all menus and buttons bigger anytime via View -> UI Scale: Larger (+10%) or switch Light / Dark UI.",
            ),
            GuideBlock::Heading("Distraction-Free Reading & Presentations"),
            GuideBlock::Paragraph(
                "Press F11 for Fullscreen (hides all toolbars) or F5 for Presentation Mode (full black background, click or press Arrow keys to advance slides). Press Esc anytime to return.",
            ),
        ],
    },
    // =========================================================================
    // Chapter 3
    // =========================================================================
    GuideChapter {
        number: 3,
        short_title: "3. Working with Multiple Tabs",
        title: "Working with Multiple Tabs",
        summary: "Open many PDFs at once, jump between them quickly, and keep your tab strip tidy.",
        blocks: &[
            GuideBlock::Heading("Opening & Switching Between Multiple PDFs"),
            GuideBlock::Step {
                number: 1,
                text: "Click the '+' button at the right end of the tab bar (or click File -> Open PDF in Tab) to open another PDF without closing your current one.",
            },
            GuideBlock::Step {
                number: 2,
                text: "Click any tab at the top to switch to it, or use keyboard shortcuts: Ctrl+Tab (next tab), Ctrl+Shift+Tab (previous tab), or Ctrl+1 through Ctrl+9 to jump directly to tabs 1–9.",
            },
            GuideBlock::Heading("When You Have 10 or More PDFs Open"),
            GuideBlock::Step {
                number: 1,
                text: "If tabs fill the width of the window, left (<) and right (>) scroll arrows appear automatically, and you can roll your mouse wheel over the tab bar to slide left or right.",
            },
            GuideBlock::Step {
                number: 2,
                text: "Click the '▾' All-Tabs button on the far right of the tab bar to see every open PDF in one clean scrollable list, jump straight to any file, or click its '×' button to close it.",
            },
            GuideBlock::Heading("Closing One Tab, Other Tabs, or All Tabs"),
            GuideBlock::Step {
                number: 1,
                text: "Click the '×' on any tab (or press Ctrl+W) to close it. Closing the very last open tab returns you cleanly to the Welcome Screen.",
            },
            GuideBlock::Step {
                number: 2,
                text: "Right-click any tab (or open the File menu) to choose: Close Tab, Close Other Tabs (keeps only that one PDF open!), Close Tabs to Right, or Close All Tabs.",
            },
            GuideBlock::Tip(
                "When you close MicroPDF and open it again later, a blue banner on the Welcome Screen offers 'Restore previous tab(s)?' so you can pick up right where you left off in one click.",
            ),
        ],
    },
    // =========================================================================
    // Chapter 4
    // =========================================================================
    GuideChapter {
        number: 4,
        short_title: "4. Notes, Markup & Forms",
        title: "Highlighting, Notes, Shapes & Filling Forms",
        summary: "Copy text, mark up pages, edit or delete existing annotations, and fill out PDF forms.",
        blocks: &[
            GuideBlock::Heading("Selecting & Copying Text"),
            GuideBlock::Step {
                number: 1,
                text: "Make sure you are in normal Select mode (Edit -> Mouse Mode: Select Text / Fill Forms, or press Esc).",
            },
            GuideBlock::Step {
                number: 2,
                text: "Drag across any words (or double-click a single word) to select text, then press Ctrl+C (Cmd+C on Mac) to copy it.",
            },
            GuideBlock::Heading("Highlighting, Sticky Notes, Pen & Shapes"),
            GuideBlock::Step {
                number: 1,
                text: "Open the Edit menu (or click the annotation icons on the top toolbar) and pick a tool: Highlight, Underline, Strikethrough, Sticky Note, Freehand Pen, Text Box, Rectangle, or Arrow.",
            },
            GuideBlock::Step {
                number: 2,
                text: "Click Edit -> Cycle Annotation Color (or the color dot on the toolbar) to pick from 7 colors.",
            },
            GuideBlock::Step {
                number: 3,
                text: "Drag across text to highlight/underline, click a spot to attach a Sticky Note or Text Box, or drag on the page to draw freehand ink, rectangles, or arrows. Press Esc when finished to return to Select mode.",
            },
            GuideBlock::Heading("Reading, Moving, Editing & Deleting Existing Annotations"),
            GuideBlock::Step {
                number: 1,
                text: "In Select mode, click any Sticky Note to open a readable multi-line popup card with built-in Edit and Delete buttons.",
            },
            GuideBlock::Step {
                number: 2,
                text: "Right-click ANY annotation on the page (Highlight, Note, Text Box, Pen, Shape) to open the Annotation Inspector: Edit Text, click any of the 7 color dots to recolor it, or Delete Annotation.",
            },
            GuideBlock::Step {
                number: 3,
                text: "You can also drag Sticky Notes and Text Boxes in Select mode to reposition them neatly.",
            },
            GuideBlock::Heading("Filling Interactive PDF Forms (Text, Checkboxes, Radio Buttons)"),
            GuideBlock::Step {
                number: 1,
                text: "Stay in Select mode (default). Move your mouse over a fillable form box — a soft blue frame appears to show it is interactive.",
            },
            GuideBlock::Step {
                number: 2,
                text: "Click any Checkbox to check/uncheck it immediately. Click any Radio Button to select that option.",
            },
            GuideBlock::Step {
                number: 3,
                text: "Click any Text Field to open a clean editor box, type your answer, and press Enter (or click Apply). Press S to save your filled form!",
            },
            GuideBlock::Tip(
                "Every annotation edit and form entry supports Ctrl+Z (Undo) and Ctrl+Y (Redo). Want to lock a filled form so nobody can alter your answers? Use Tools -> Flatten & Unlock PDF after saving a copy.",
            ),
        ],
    },
    // =========================================================================
    // Chapter 5
    // =========================================================================
    GuideChapter {
        number: 5,
        short_title: "5. Organizing Pages",
        title: "Organizing Pages (Insert, Delete, Rotate, Combine, Split)",
        summary: "Reorder pages visually, edit multiple pages at once, insert other PDFs, and split documents.",
        blocks: &[
            GuideBlock::Heading("Using the Visual Thumbnail Grid"),
            GuideBlock::Step {
                number: 1,
                text: "Press Tab (or click View -> Toggle Thumbnail Grid) to see miniature previews of all pages at once.",
            },
            GuideBlock::Step {
                number: 2,
                text: "Drag and drop any page thumbnail onto another spot to reorder pages. Double-click any page (or press Tab again) to jump back to reading it.",
            },
            GuideBlock::Step {
                number: 3,
                text: "Right-click any page thumbnail for an instant menu: Insert Blank Page Before/After, Insert PDF(s) Before/After, Rotate 90°, Convert to Black & White, or Delete Page.",
            },
            GuideBlock::Heading("Editing Multiple Pages at Once (Batch Select)"),
            GuideBlock::Step {
                number: 1,
                text: "In Thumbnail Grid view, hold Ctrl and click several pages (or click one page, hold Shift, and click another page to select a whole range).",
            },
            GuideBlock::Step {
                number: 2,
                text: "A floating Batch Action Bar appears at the bottom! Click Rotate 90°, Convert to B/W, Extract Selected (saves just those pages to a new PDF), or Delete Selected — all undoable in one single Ctrl+Z.",
            },
            GuideBlock::Heading("Inserting PDFs, Combining Files & Splitting Pages"),
            GuideBlock::Step {
                number: 1,
                text: "To insert another PDF inside your open file: click Page -> Insert PDF(s) Before / After Page... (or simply drag a PDF file from your computer and drop it right onto the Thumbnail Grid!).",
            },
            GuideBlock::Step {
                number: 2,
                text: "To merge several PDFs into one new document: click File -> Combine Multiple PDFs... (Shift+M), add files, arrange their order with live preview, and click Combine.",
            },
            GuideBlock::Step {
                number: 3,
                text: "To extract a few pages into one PDF: click Tools -> Split / Extract Pages... (X), type a range like '1-3' or '2, 5, 8-10', and press Enter. Or click Tools -> Split to Single-Page PDFs... to export each page losslessly as its own numbered PDF file into a folder!",
            },
            GuideBlock::Heading("Straightening Tilted or Scanned Pages (Page Deskew)"),
            GuideBlock::Step {
                number: 1,
                text: "If a scanned page looks slightly crooked, click Page -> Deskew Page(s)... to open the compact Deskew bar at the top of the page.",
            },
            GuideBlock::Step {
                number: 2,
                text: "Drag the Angle slider (from -15.0° to +15.0°), click - / + for 0.1° fine steps, or click 'Draw Guide' and drag a line along any crooked text margin or table edge — MicroPDF automatically straightens it with a live preview!",
            },
            GuideBlock::Step {
                number: 3,
                text: "Toggle 'Grid' for visual alignment lines, choose Current Page or Page Range (like '1-6'), and click Apply. Your page stays sharp (no re-scanning or blurring) and can be undone with Ctrl+Z.",
            },
            GuideBlock::Tip(
                "Quick Page Shortcuts: Press R to rotate the current page 90°, I to insert a blank page after, D to delete the current page, K to convert a page to crisp Black & White, or G to jump straight to any page number (even Roman numerals like 'iii' or physical '#5').",
            ),
        ],
    },
    // =========================================================================
    // Chapter 6
    // =========================================================================
    GuideChapter {
        number: 6,
        short_title: "6. Headers, Page # & Bookmarks",
        title: "Headers, Footers, Page Numbers & Bookmarks",
        summary: "Add professional running titles, Roman/Arabic page numbers, and a clickable Table of Contents.",
        blocks: &[
            GuideBlock::Heading("Adding, Updating, or Clearing Headers & Footers"),
            GuideBlock::Step {
                number: 1,
                text: "Click Tools -> Header & Footer / Clear Existing... (or press H). A live preview window opens so you can see exactly how your page will look before applying.",
            },
            GuideBlock::Step {
                number: 2,
                text: "Type text into any of the 6 positions (Top Left/Center/Right or Bottom Left/Center/Right), choose font size, margin, and page range.",
            },
            GuideBlock::Step {
                number: 3,
                text: "Want to replace old headers/footers cleanly? Check 'Update / Clear Existing'. Want to reuse a layout on future PDFs? Click 'Save' under Style Profiles (and use '<' / '>' to load saved profiles anytime).",
            },
            GuideBlock::Heading("Adding Smart Page Numbers (Roman + Arabic + Skip Covers)"),
            GuideBlock::Step {
                number: 1,
                text: "Click Tools -> Page Numbering... (or press N) to open the visual Page Numbering dialog with live preview.",
            },
            GuideBlock::Step {
                number: 2,
                text: "If your PDF has cover or picture pages at the front, set 'Skip First Pages' (for example, 2) so numbering starts cleanly after the covers.",
            },
            GuideBlock::Step {
                number: 3,
                text: "Optionally set 'Roman Pages' (e.g., 4 for pages i, ii, iii, iv followed by 1, 2, 3...), pick Bottom Left / Bottom Center / Bottom Right, and click Apply.",
            },
            GuideBlock::Heading("Building & Using the Bookmark Sidebar (Table of Contents)"),
            GuideBlock::Step {
                number: 1,
                text: "Press B (or click View -> Toggle Bookmark Sidebar) to open the left Table of Contents panel. Click any entry to jump straight to that chapter.",
            },
            GuideBlock::Step {
                number: 2,
                text: "Use the sidebar buttons: '+ Add' creates a bookmark on the current page, 'Auto' scans your PDF headings automatically to build a starter Table of Contents, and '↑ / ↓ / ← / →' reorders or indents sub-sections.",
            },
            GuideBlock::Step {
                number: 3,
                text: "Click 'Import' to load a list from a .txt file (see Help -> Bookmark Import Format (.txt) for a sample template) or 'Export' to save your bookmarks as a .txt file. Press S to embed bookmarks inside the PDF!",
            },
            GuideBlock::Tip(
                "Right-click any bookmark in the sidebar to rename it, or click the small '×' on a bookmark row to remove it. All bookmark changes can be undone with Ctrl+Z.",
            ),
        ],
    },
    // =========================================================================
    // Chapter 7
    // =========================================================================
    GuideChapter {
        number: 7,
        short_title: "7. Printing & 2-Sided Booklets",
        title: "Printing, Booklets & 2-Sided on Any Printer",
        summary: "Preview sheets before wasting ink, trim white margins, and print double-sided even on basic printers.",
        blocks: &[
            GuideBlock::Heading("Opening Live Print Preview"),
            GuideBlock::Step {
                number: 1,
                text: "Click File -> Print... (or press Ctrl+P / Cmd+P). You will see a live picture of your actual paper sheet on the left and clear print settings on the right.",
            },
            GuideBlock::Step {
                number: 2,
                text: "Pick your Printer, Copies, Page Range (All, Current, or Custom like '1-4, 7'), and choose from 10 Paper Sizes (A4, Letter, Legal, A3, A5, B4, B5, Executive, Tabloid, Statement).",
            },
            GuideBlock::Step {
                number: 3,
                text: "Choose 'With Annots' to print your highlights, notes, and stamps, or 'No Annots' to print a clean copy without markup (redactions and filled forms always stay intact!).",
            },
            GuideBlock::Heading("Making Small Text Bigger on Paper (Crop White Space)"),
            GuideBlock::Step {
                number: 1,
                text: "Turn on 'Auto-Trim' in Print Preview to automatically trim empty outer white margins so the actual text prints noticeably larger and clearer.",
            },
            GuideBlock::Step {
                number: 2,
                text: "Need even larger print? Adjust Print Zoom (100%–200%) and watch the live sheet preview update immediately.",
            },
            GuideBlock::Heading("2-Sided Printing — Even on a 1-Sided Printer!"),
            GuideBlock::Step {
                number: 1,
                text: "If your printer supports automatic 2-sided printing, select 'Long Edge' (book flip) or 'Short Edge' (calendar flip).",
            },
            GuideBlock::Step {
                number: 2,
                text: "If you only have a regular 1-sided printer, choose 'Manual 2-Sided'! MicroPDF prints the odd pages first, pauses with a clear on-screen guide telling you exactly how to flip the paper stack, and prints the even pages when you click 'Print Back Sides'.",
            },
            GuideBlock::Heading("Foldable Booklet Printing"),
            GuideBlock::Paragraph(
                "Turn on 'Booklet' in Print Preview to automatically arrange pages 2-per-sheet in booklet order so you can fold the printed stack in half and staple the spine like a real pamphlet.",
            ),
            GuideBlock::Tip(
                "Click 'System Dialog...' inside Print Preview anytime if you need special manufacturer printer settings (such as photo paper tray or custom ink quality).",
            ),
        ],
    },
    // =========================================================================
    // Chapter 8
    // =========================================================================
    GuideChapter {
        number: 8,
        short_title: "8. Converting & Exporting",
        title: "Converting & Exporting (Word, Excel, Images, Markdown, LaTeX)",
        summary: "Turn PDFs into editable Word documents, Excel spreadsheets, PNG/JPG pictures, or add stamps/watermarks.",
        blocks: &[
            GuideBlock::Heading("Exporting a PDF to Word (.docx) or Excel (.xlsx)"),
            GuideBlock::Step {
                number: 1,
                text: "Open your PDF, then click Export in the top menu bar.",
            },
            GuideBlock::Step {
                number: 2,
                text: "Click 'Export as Word (.docx)...' to create an editable document with headings, paragraphs, lists, and tables.",
            },
            GuideBlock::Step {
                number: 3,
                text: "Click 'Export as Excel (.xlsx)...' to open the Excel Export options: choose Auto (Hybrid) for most PDFs, Lattice for ruled tables with merged cells, or Stream for borderless columns; choose Stack on One Sheet or Separate Sheet per Table; and keep 'Normalize comma-decimal numbers' checked for financial tables (e.g. 123,9500 -> 123.95). Click 'Export Excel (.xlsx)...' to save offline.",
            },
            GuideBlock::Heading("Exporting Pages as PNG or JPEG Images"),
            GuideBlock::Step {
                number: 1,
                text: "Click Export -> Export Pages as Images (PNG/JPG)....",
            },
            GuideBlock::Step {
                number: 2,
                text: "Choose PNG (sharpest for text/diagrams) or JPEG (smaller files for sharing), pick quality (150, 200, or 300 DPI), and select Current Page, All Pages, or a Custom range.",
            },
            GuideBlock::Step {
                number: 3,
                text: "Click 'Choose Folder & Export' to save numbered images straight into your chosen folder.",
            },
            GuideBlock::Heading("Markdown (.md) Reader, Export, Stamps & Watermarks"),
            GuideBlock::Paragraph(
                "• Markdown (.md / .markdown) Reader & Export: open any Markdown file to read it in a clean column with automatic outline headings, copyable code blocks, tables, and zoom (Ctrl+ / Ctrl-). Click 'Export / Print' (or press Ctrl+P) on a Markdown tab to export it as a Styled PDF (A4 or Letter) or a Styled Word (.docx) document!\n• Writers & students can also export PDF pages to Markdown (.md) or LaTeX (.tex) from the Export menu (or directly inside Text View).\n• Stamps & Signatures (P): click Tools -> Stamps & Signatures... (P) to draw a signature on the built-in freehand pad (Black or Blue ink), import a transparent PNG, or customize APPROVED / CONFIDENTIAL / DRAFT / REVIEWED / COPY stamps — saved portably in ./micropdf_assets/ — then click 'Place' and drag on any page!\n• Detailed Watermarks (W): click Tools -> Add Watermark... (W) for a live-preview dialog with a scrollable System Font Dropdown, font size, color swatches, rotation (-180° to +180°), opacity, Centered/Custom/Tiled placement, and page range.",
            ),
            GuideBlock::Tip(
                "For the cleanest handwritten signature stamp, use a PNG image with a transparent background — MicroPDF preserves transparency automatically!",
            ),
        ],
    },
    // =========================================================================
    // Chapter 9
    // =========================================================================
    GuideChapter {
        number: 9,
        short_title: "9. Privacy, Redact & Shrink Size",
        title: "Privacy (Redaction, Metadata Scrub) & Shrinking PDF Size",
        summary: "Permanently black out sensitive details, strip hidden author info, and make huge scanned PDFs small.",
        blocks: &[
            GuideBlock::Heading("Permanently Redacting Sensitive Information (Black-Box Redact)"),
            GuideBlock::Step {
                number: 1,
                text: "Click Edit -> Mouse Mode: Black-Box Redact.",
            },
            GuideBlock::Step {
                number: 2,
                text: "Drag a red box over any private name, account number, phone number, or photo area you want gone forever.",
            },
            GuideBlock::Step {
                number: 3,
                text: "Save the PDF (File -> Save As... recommended so you keep your private copy separately). Unlike basic black marker tools, MicroPDF truly deletes the underlying text, pixels, and hidden metadata so nobody can copy or recover what was underneath.",
            },
            GuideBlock::Heading("Checking Document Properties & Stripping Hidden Personal Info"),
            GuideBlock::Step {
                number: 1,
                text: "Click File -> Document Properties & Privacy... to inspect page dimensions, file size, fonts, and hidden fields like Author Name, Creator Software, and Edit Dates.",
            },
            GuideBlock::Step {
                number: 2,
                text: "Click 'Strip Personal Metadata & XMP' to erase hidden personal details in one click without changing a single word on your pages, then press S to save.",
            },
            GuideBlock::Heading("Shrinking Huge PDFs for Email or Upload (Optimize)"),
            GuideBlock::Step {
                number: 1,
                text: "Click Tools -> Optimize... (or press Z) to open the Optimizer card.",
            },
            GuideBlock::Step {
                number: 2,
                text: "Pick a preset: Smaller (150 DPI, best for email), Balanced (200 DPI, everyday sharp), or Print (300 DPI). Choose Color, Gray, or crisp Black & White, and click Preview to check quality before running.",
            },
            GuideBlock::Step {
                number: 3,
                text: "Click Optimize! MicroPDF writes a brand-new '<filename>_optimized.pdf' right beside your original (your original file is NEVER overwritten), and tells you the exact MB saved.",
            },
            GuideBlock::Tip(
                "Need to lock edits or remove copy/print restrictions from a PDF you own? Click Tools -> Flatten & Unlock PDF (U). Press Esc anytime to cancel a running optimization job.",
            ),
        ],
    },
    // =========================================================================
    // Chapter 10
    // =========================================================================
    GuideChapter {
        number: 10,
        short_title: "10. Compare, Ruler & Quick Help",
        title: "Search, Compare PDFs, Measure Ruler & Troubleshooting",
        summary: "Find exact words, visually spot differences between two PDF drafts, measure dimensions, and solve common issues.",
        blocks: &[
            GuideBlock::Heading("Smart Text Search (Match Case & Whole Word)"),
            GuideBlock::Step {
                number: 1,
                text: "Press Ctrl+F (Cmd+F on Mac) or click Edit -> Find Text in PDF... to open the search bar at the top right.",
            },
            GuideBlock::Step {
                number: 2,
                text: "Type your word and press Enter. Click 'Aa' to match exact UPPER/lower case, or 'W' to match whole words only (so 'cat' won't match 'catalog').",
            },
            GuideBlock::Step {
                number: 3,
                text: "Click the ▲ / ▼ arrows (or press F3 for next match and Shift+F3 for previous match) to jump cleanly through every occurrence.",
            },
            GuideBlock::Heading("Comparing Two PDF Versions Side-by-Side"),
            GuideBlock::Step {
                number: 1,
                text: "With your first PDF open, click Tools -> Compare with Another PDF... and choose the second PDF file.",
            },
            GuideBlock::Step {
                number: 2,
                text: "Both PDFs appear side-by-side and turn, zoom, and scroll together! Switch freely between Fit, Width, 100%, or custom zoom (Ctrl+Wheel / + / -), toggle the Hand button (or click & drag either pane) to pan in sync, or use the bottom horizontal and right vertical scrollbars. If one version has an extra page, use Offset '-' / '+' in the top Compare bar.",
            },
            GuideBlock::Step {
                number: 3,
                text: "Click 'Visual Diff' to highlight pixel differences automatically: Red shows content removed from the left document, Blue shows content added in the right document! Press Esc (or click Close) to exit Compare Mode.",
            },
            GuideBlock::Heading("Measuring Distance, Area & Angle (Ruler Tool)"),
            GuideBlock::Step {
                number: 1,
                text: "Click Edit -> Measure Distance & Area (Ruler).",
            },
            GuideBlock::Step {
                number: 2,
                text: "Click and drag between any two points on the page. A floating readout shows exact Length, Width × Height, Area, and Angle.",
            },
            GuideBlock::Step {
                number: 3,
                text: "Click mm, cm, in, or pt on the floating Ruler bar to switch measurement units instantaneously. Press Esc or click Done when finished.",
            },
            GuideBlock::Heading("Quick Troubleshooting"),
            GuideBlock::Paragraph(
                "• Stuck in Highlight, Redact, Crop, or Ruler mode? Just press the Esc key to return to normal Select mode.\n• Password-protected PDF? Type the password when prompted (up to 3 tries) — MicroPDF never saves your password.\n• Portable folder rule: Always keep the MicroPDF executable and its 'pdfium' library file together in the same folder.\n• Want a printed copy of this manual? Click 'Save Guide (.txt)...' at the top-right corner of this window!",
            ),
            GuideBlock::Tip(
                "You can also press F1 or '?' anytime for a quick 1-page cheat sheet of keyboard and mouse shortcuts.",
            ),
        ],
    },
];

#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct UserGuideState {
    pub(super) chapter: usize,
    pub(super) scroll_y: f32,
    pub(super) max_scroll: f32,
}

impl Default for UserGuideState {
    fn default() -> Self {
        Self {
            chapter: 0,
            scroll_y: 0.0,
            max_scroll: 0.0,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum UserGuideHit {
    Chapter(usize),
    PrevChapter,
    NextChapter,
    SaveTxt,
    Close,
    InsideModal,
}

/// Formats all 10 chapters of the built-in User Guide into a clean, printable plain-text manual.
pub(super) fn format_full_user_guide_txt() -> String {
    let mut out = String::with_capacity(16 * 1024);
    out.push_str("========================================================================\n");
    out.push_str("                 MicroPDF v1.4 — Step-by-Step User Guide\n");
    out.push_str("========================================================================\n\n");
    out.push_str("Table of Contents:\n");
    for ch in USER_GUIDE_CHAPTERS {
        out.push_str(&format!("  {}. {}\n", ch.number, ch.title));
    }
    out.push('\n');

    for ch in USER_GUIDE_CHAPTERS {
        out.push_str("------------------------------------------------------------------------\n");
        out.push_str(&format!("CHAPTER {}: {}\n", ch.number, ch.title.to_uppercase()));
        out.push_str(&format!("{}\n", ch.summary));
        out.push_str("------------------------------------------------------------------------\n\n");
        for block in ch.blocks {
            match *block {
                GuideBlock::Heading(h) => {
                    out.push_str(&format!("## {h}\n"));
                }
                GuideBlock::Paragraph(p) => {
                    out.push_str(p);
                    out.push_str("\n\n");
                }
                GuideBlock::Step { number, text } => {
                    out.push_str(&format!("  Step {number}: {text}\n"));
                }
                GuideBlock::Tip(t) => {
                    out.push_str(&format!("\n  [TIP] {t}\n\n"));
                }
            }
        }
        out.push('\n');
    }
    out
}

/// Wraps `text` (preserving explicit `\n` line breaks) into lines fitting `max_width_px`.
fn wrap_guide_lines(text: &str, max_width_px: f32, font_sz: f32, bold: bool) -> Vec<String> {
    let mut out = Vec::new();
    let max_w = max_width_px.max(60.0);
    for raw_para in text.split('\n') {
        let mut line = String::new();
        for word in raw_para.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if !line.is_empty() && measure_text(&candidate, font_sz, bold) > max_w {
                out.push(line);
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        if !line.is_empty() || raw_para.is_empty() {
            out.push(line);
        }
    }
    out
}

#[derive(Clone, Copy)]
struct GuideHitBox {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    hit: UserGuideHit,
}

impl GuideHitBox {
    fn contains(self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w as i32 && py >= self.y && py < self.y + self.h as i32
    }
}

impl<'a> PdfViewerApp<'a> {
    pub(super) fn open_user_guide_modal(&mut self) {
        self.close_all_popovers();
        self.show_help_overlay = false;
        self.show_about_overlay = false;
        self.show_bookmark_format_overlay = false;
        self.user_guide_modal = Some(UserGuideState::default());
        self.request_redraw();
    }

    pub(super) fn close_user_guide_modal(&mut self) {
        if self.user_guide_modal.take().is_some() {
            self.request_redraw();
        }
    }

    pub(super) fn scroll_user_guide(&mut self, delta_y: f32) {
        if let Some(ref mut g) = self.user_guide_modal {
            let next = (g.scroll_y + delta_y).clamp(0.0, g.max_scroll.max(0.0));
            if (next - g.scroll_y).abs() > 0.1 {
                g.scroll_y = next;
                self.request_redraw();
            }
        }
    }

    pub(super) fn select_user_guide_chapter(&mut self, idx: usize) {
        if let Some(ref mut g) = self.user_guide_modal {
            let clamped = idx.min(USER_GUIDE_CHAPTERS.len().saturating_sub(1));
            g.chapter = clamped;
            g.scroll_y = 0.0;
            g.max_scroll = 0.0;
            self.request_redraw();
        }
    }

    pub(super) fn save_user_guide_txt_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Save MicroPDF Step-by-Step User Guide")
            .add_filter("Plain Text Manual", &["txt"])
            .set_file_name("MicroPDF_User_Guide.txt");
        if let Some(parent) = self.file_path.parent() {
            if !parent.as_os_str().is_empty() && parent.exists() {
                dialog = dialog.set_directory(parent);
            }
        }
        if let Some(save_path) = dialog.save_file() {
            let body = format_full_user_guide_txt();
            match std::fs::write(&save_path, body.as_bytes()) {
                Ok(()) => {
                    let fname = save_path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("MicroPDF_User_Guide.txt");
                    self.status_notice = Some(format!("Saved printable User Guide to '{fname}'"));
                }
                Err(e) => {
                    self.status_notice = Some(format!("Failed to save User Guide: {e}"));
                }
            }
            self.update_title_bar();
            self.request_redraw();
        }
    }

    fn layout_user_guide_modal(
        &self,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<((i32, i32, u32, u32), i32, i32, i32, Vec<GuideHitBox>)> {
        let _ = self.user_guide_modal?;
        let s = sf as f32;
        let cw = ((880.0 * s).round() as u32).min(buf_w.saturating_sub(28)).max(360);
        let ch = ((560.0 * s).round() as u32).min(buf_h.saturating_sub(28)).max(280);
        let cx = (buf_w.saturating_sub(cw) / 2) as i32;
        let cy = (buf_h.saturating_sub(ch) / 2) as i32;

        let hdr_h = (48.0 * s).round() as i32;
        let ftr_h = (46.0 * s).round() as i32;
        let left_w = ((236.0 * s).round() as i32).min((cw as i32) / 3);

        let mut hits = Vec::with_capacity(16);
        let pad = (14.0 * s).round() as i32;

        // Top-right header buttons: [ Save Guide (.txt)... ] [ × ]
        let close_sz = (26.0 * s).round() as u32;
        let close_x = cx + cw as i32 - pad - close_sz as i32;
        let close_y = cy + ((hdr_h - close_sz as i32) / 2);
        hits.push(GuideHitBox {
            x: close_x,
            y: close_y,
            w: close_sz,
            h: close_sz,
            hit: UserGuideHit::Close,
        });

        let save_w = (142.0 * s).round() as u32;
        let save_h = (26.0 * s).round() as u32;
        let save_x = close_x - (8.0 * s).round() as i32 - save_w as i32;
        let save_y = cy + ((hdr_h - save_h as i32) / 2);
        hits.push(GuideHitBox {
            x: save_x,
            y: save_y,
            w: save_w,
            h: save_h,
            hit: UserGuideHit::SaveTxt,
        });

        // Left chapter list items
        let body_top = cy + hdr_h;
        let body_h = (ch as i32 - hdr_h - ftr_h).max(60);
        let row_pad = (8.0 * s).round() as i32;
        let avail_list_h = (body_h - row_pad * 2).max(100);
        let row_h = ((avail_list_h / USER_GUIDE_CHAPTERS.len().max(1) as i32) as u32)
            .clamp((26.0 * s).round() as u32, (40.0 * s).round() as u32);
        for (idx, _) in USER_GUIDE_CHAPTERS.iter().enumerate() {
            let ry = body_top + row_pad + (idx as i32) * (row_h as i32);
            if ry + row_h as i32 <= body_top + body_h {
                hits.push(GuideHitBox {
                    x: cx + (8.0 * s).round() as i32,
                    y: ry,
                    w: (left_w - (16.0 * s).round() as i32).max(40) as u32,
                    h: row_h.saturating_sub(3).max(20),
                    hit: UserGuideHit::Chapter(idx),
                });
            }
        }

        // Bottom footer Prev / Next Chapter buttons
        let ftr_y = cy + ch as i32 - ftr_h;
        let nav_w = (122.0 * s).round() as u32;
        let nav_h = (28.0 * s).round() as u32;
        let nav_y = ftr_y + ((ftr_h - nav_h as i32) / 2);
        let next_x = cx + cw as i32 - pad - nav_w as i32;
        let prev_x = next_x - (8.0 * s).round() as i32 - nav_w as i32;
        hits.push(GuideHitBox {
            x: prev_x,
            y: nav_y,
            w: nav_w,
            h: nav_h,
            hit: UserGuideHit::PrevChapter,
        });
        hits.push(GuideHitBox {
            x: next_x,
            y: nav_y,
            w: nav_w,
            h: nav_h,
            hit: UserGuideHit::NextChapter,
        });

        Some(((cx, cy, cw, ch), hdr_h, ftr_h, left_w, hits))
    }

    pub(super) fn hit_test_user_guide_modal(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<UserGuideHit> {
        let ((cx, cy, cw, ch), _, _, _, hits) = self.layout_user_guide_modal(buf_w, buf_h, sf)?;
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix < cx || ix >= cx + cw as i32 || iy < cy || iy >= cy + ch as i32 {
            return None;
        }
        hits.into_iter()
            .find(|h| h.contains(ix, iy))
            .map(|h| h.hit)
            .or(Some(UserGuideHit::InsideModal))
    }

    pub(super) fn handle_user_guide_click(&mut self, hit: UserGuideHit) {
        let cur = self.user_guide_modal.map(|g| g.chapter).unwrap_or(0);
        match hit {
            UserGuideHit::Chapter(idx) => self.select_user_guide_chapter(idx),
            UserGuideHit::PrevChapter => {
                if cur > 0 {
                    self.select_user_guide_chapter(cur - 1);
                }
            }
            UserGuideHit::NextChapter => {
                if cur + 1 < USER_GUIDE_CHAPTERS.len() {
                    self.select_user_guide_chapter(cur + 1);
                }
            }
            UserGuideHit::SaveTxt => {
                self.pending_dialog = Some(PendingDialog::SaveUserGuideTxt);
                self.request_redraw();
            }
            UserGuideHit::Close => self.close_user_guide_modal(),
            UserGuideHit::InsideModal => {}
        }
    }

    pub(super) fn draw_user_guide_modal(
        &mut self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) {
        let Some(state) = self.user_guide_modal else {
            return;
        };
        let Some(((cx, cy, cw, ch), hdr_h, ftr_h, left_w, hits)) =
            self.layout_user_guide_modal(buf_w, buf_h, sf)
        else {
            return;
        };

        let theme = self.ui_theme();
        let s = sf as f32;
        let rad = (10.0 * s).clamp(6.0, 16.0);
        blend_rect_rgba(buffer, buf_w, buf_h, 0, 0, buf_w, buf_h, theme.modal_scrim);

        draw_soft_shadow(
            buffer,
            buf_w,
            buf_h,
            cx,
            cy,
            cw,
            ch,
            rad,
            (14.0 * s).round() as i32,
            (5.0 * s).round() as i32,
            theme.shadow_rgb,
            theme.shadow_max_alpha,
        );
        fill_rounded_rect(buffer, buf_w, buf_h, cx, cy, cw, ch, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, cx, cy, cw, ch, rad, theme.border_subtle);

        let title_sz = (15.0 * s).clamp(12.0, 30.0);
        let h2_sz = (13.0 * s).clamp(11.0, 26.0);
        let body_sz = (11.8 * s).clamp(10.0, 24.0);
        let small_sz = (10.8 * s).clamp(9.0, 21.0);
        let pad = (16.0 * s).round() as i32;

        // 1. Header bar
        let save_x = hits
            .iter()
            .find(|h| h.hit == UserGuideHit::SaveTxt)
            .map(|h| h.x)
            .unwrap_or(cx + cw as i32 - 160);
        draw_text_vcenter(
            buffer,
            buf_w,
            buf_h,
            cx + pad,
            cy,
            hdr_h as u32,
            save_x - 8,
            "MicroPDF User Guide — Step-by-Step How to Use",
            title_sz,
            theme.text_primary,
            true,
        );
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            cx + 1,
            cy + hdr_h - 1,
            cw.saturating_sub(2),
            1,
            theme.divider,
        );

        // Header buttons: Save Guide (.txt)... & Close
        for h in &hits {
            let hov = self.hovered_widget == Some(UiHit::UserGuide(h.hit));
            match h.hit {
                UserGuideHit::SaveTxt => {
                    fill_rounded_rect(
                        buffer,
                        buf_w,
                        buf_h,
                        h.x,
                        h.y,
                        h.w,
                        h.h,
                        5.0,
                        if hov { theme.hover_bg } else { theme.surface_elevated },
                    );
                    stroke_rounded_rect(buffer, buf_w, buf_h, h.x, h.y, h.w, h.h, 5.0, theme.border_subtle);
                    let lbl = "Save Guide (.txt)...";
                    let lw = measure_text(lbl, small_sz, true);
                    draw_text_vcenter(
                        buffer,
                        buf_w,
                        buf_h,
                        h.x + ((h.w as f32 - lw) * 0.5).round() as i32,
                        h.y,
                        h.h,
                        h.x + h.w as i32,
                        lbl,
                        small_sz,
                        theme.text_primary,
                        true,
                    );
                }
                UserGuideHit::Close => {
                    if hov {
                        fill_rounded_rect(buffer, buf_w, buf_h, h.x, h.y, h.w, h.h, 5.0, theme.hover_bg);
                    }
                    let ic = (13.0 * s).round() as u32;
                    draw_icon(
                        buffer,
                        buf_w,
                        buf_h,
                        UiIcon::Close,
                        h.x + ((h.w - ic) / 2) as i32,
                        h.y + ((h.h - ic) / 2) as i32,
                        ic,
                        theme.text_secondary,
                    );
                }
                _ => {}
            }
        }

        // 2. Left Chapter Sidebar
        let body_top = cy + hdr_h;
        let body_h = (ch as i32 - hdr_h - ftr_h).max(60) as u32;
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            cx + 1,
            body_top,
            (left_w - 1).max(1) as u32,
            body_h,
            theme.surface_elevated,
        );
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            cx + left_w,
            body_top,
            1,
            body_h,
            theme.divider,
        );

        let cur_idx = state.chapter.min(USER_GUIDE_CHAPTERS.len().saturating_sub(1));
        for h in &hits {
            if let UserGuideHit::Chapter(idx) = h.hit {
                let active = idx == cur_idx;
                let hov = self.hovered_widget == Some(UiHit::UserGuide(h.hit));
                if active {
                    fill_rounded_rect(buffer, buf_w, buf_h, h.x, h.y, h.w, h.h, 6.0, theme.accent_soft_bg);
                    fill_rounded_rect(
                        buffer,
                        buf_w,
                        buf_h,
                        h.x + 2,
                        h.y + 4,
                        3,
                        h.h.saturating_sub(8).max(6),
                        1.5,
                        theme.accent,
                    );
                } else if hov {
                    fill_rounded_rect(buffer, buf_w, buf_h, h.x, h.y, h.w, h.h, 6.0, theme.hover_bg);
                }
                let lbl = USER_GUIDE_CHAPTERS[idx].short_title;
                let max_tw = (h.w as f32 - 18.0 * s).max(30.0);
                let short = truncate_ellipsis(lbl, max_tw, small_sz, active);
                draw_text_vcenter(
                    buffer,
                    buf_w,
                    buf_h,
                    h.x + (10.0 * s).round() as i32,
                    h.y,
                    h.h,
                    h.x + h.w as i32 - 4,
                    &short,
                    small_sz,
                    if active { theme.accent_soft_fg } else { theme.text_primary },
                    active,
                );
            }
        }

        // 3. Right Scrollable Content Pane
        let right_x = cx + left_w + pad;
        let right_w = (cw as i32 - left_w - pad * 2 - (10.0 * s).round() as i32).max(120);
        let clip_top = body_top + (4.0 * s).round() as i32;
        let clip_bot = body_top + body_h as i32 - (4.0 * s).round() as i32;
        let ch_data = &USER_GUIDE_CHAPTERS[cur_idx];

        let line_h = (18.0 * s).round() as i32;
        let scroll = state.scroll_y.round() as i32;
        let mut cursor_y = body_top + (14.0 * s).round() as i32 - scroll;
        let content_start_y = cursor_y;

        // Chapter Title & Summary Banner
        let title_full = format!("Chapter {}: {}", ch_data.number, ch_data.title);
        if cursor_y >= clip_top && cursor_y + (20.0 * s).round() as i32 <= clip_bot {
            draw_text_clipped(
                buffer,
                buf_w,
                clip_bot.max(0) as u32,
                right_x,
                cursor_y,
                right_x + right_w,
                &title_full,
                title_sz,
                theme.accent_soft_fg,
                true,
            );
        }
        cursor_y += (24.0 * s).round() as i32;

        for sum_line in wrap_guide_lines(ch_data.summary, right_w as f32, body_sz, false) {
            if cursor_y >= clip_top && cursor_y + line_h <= clip_bot {
                draw_text_clipped(
                    buffer,
                    buf_w,
                    clip_bot.max(0) as u32,
                    right_x,
                    cursor_y,
                    right_x + right_w,
                    &sum_line,
                    body_sz,
                    theme.text_muted,
                    false,
                );
            }
            cursor_y += line_h;
        }
        cursor_y += (8.0 * s).round() as i32;
        if cursor_y >= clip_top && cursor_y < clip_bot {
            fill_rect_clipped(buffer, buf_w, buf_h, right_x, cursor_y, right_w as u32, 1, theme.divider);
        }
        cursor_y += (12.0 * s).round() as i32;

        // Render each GuideBlock cleanly
        for block in ch_data.blocks {
            match *block {
                GuideBlock::Heading(h_text) => {
                    cursor_y += (6.0 * s).round() as i32;
                    for hl in wrap_guide_lines(h_text, right_w as f32, h2_sz, true) {
                        if cursor_y >= clip_top && cursor_y + line_h <= clip_bot {
                            draw_text_clipped(
                                buffer,
                                buf_w,
                                clip_bot.max(0) as u32,
                                right_x,
                                cursor_y,
                                right_x + right_w,
                                &hl,
                                h2_sz,
                                theme.text_primary,
                                true,
                            );
                        }
                        cursor_y += (20.0 * s).round() as i32;
                    }
                    cursor_y += (3.0 * s).round() as i32;
                }
                GuideBlock::Paragraph(p_text) => {
                    for pl in wrap_guide_lines(p_text, right_w as f32, body_sz, false) {
                        if cursor_y >= clip_top && cursor_y + line_h <= clip_bot {
                            draw_text_clipped(
                                buffer,
                                buf_w,
                                clip_bot.max(0) as u32,
                                right_x,
                                cursor_y,
                                right_x + right_w,
                                &pl,
                                body_sz,
                                theme.text_secondary,
                                false,
                            );
                        }
                        cursor_y += line_h;
                    }
                    cursor_y += (6.0 * s).round() as i32;
                }
                GuideBlock::Step { number, text } => {
                    let badge_w = (50.0 * s).round() as u32;
                    let badge_h = (19.0 * s).round() as u32;
                    let text_x = right_x + badge_w as i32 + (10.0 * s).round() as i32;
                    let text_max_w = (right_w - badge_w as i32 - (10.0 * s).round() as i32).max(80);
                    let wrapped = wrap_guide_lines(text, text_max_w as f32, body_sz, false);

                    if cursor_y >= clip_top && cursor_y + badge_h as i32 <= clip_bot {
                        fill_rounded_rect(
                            buffer,
                            buf_w,
                            clip_bot.max(0) as u32,
                            right_x,
                            cursor_y - 1,
                            badge_w,
                            badge_h,
                            5.0,
                            theme.accent_soft_bg,
                        );
                        let blbl = format!("Step {number}");
                        let bw = measure_text(&blbl, small_sz, true);
                        draw_text_vcenter(
                            buffer,
                            buf_w,
                            clip_bot.max(0) as u32,
                            right_x + ((badge_w as f32 - bw) * 0.5).round() as i32,
                            cursor_y - 1,
                            badge_h,
                            right_x + badge_w as i32,
                            &blbl,
                            small_sz,
                            theme.accent_soft_fg,
                            true,
                        );
                    }

                    for wl in &wrapped {
                        if cursor_y >= clip_top && cursor_y + line_h <= clip_bot {
                            draw_text_clipped(
                                buffer,
                                buf_w,
                                clip_bot.max(0) as u32,
                                text_x,
                                cursor_y,
                                right_x + right_w,
                                wl,
                                body_sz,
                                theme.text_primary,
                                false,
                            );
                        }
                        cursor_y += line_h;
                    }
                    cursor_y += (6.0 * s).round() as i32;
                }
                GuideBlock::Tip(t_text) => {
                    cursor_y += (4.0 * s).round() as i32;
                    let tip_pad = (10.0 * s).round() as i32;
                    let tip_text_w = (right_w - tip_pad * 2 - 6).max(80);
                    let tip_label = format!("Tip: {t_text}");
                    let lines = wrap_guide_lines(&tip_label, tip_text_w as f32, body_sz, false);
                    let box_h = ((lines.len() as i32) * line_h + tip_pad * 2 - 4).max(32) as u32;

                    if cursor_y + box_h as i32 >= clip_top && cursor_y <= clip_bot {
                        let draw_y = cursor_y.max(clip_top);
                        let draw_end = (cursor_y + box_h as i32).min(clip_bot);
                        if draw_end > draw_y {
                            fill_rounded_rect(
                                buffer,
                                buf_w,
                                clip_bot.max(0) as u32,
                                right_x,
                                draw_y,
                                right_w as u32,
                                (draw_end - draw_y) as u32,
                                6.0,
                                theme.accent_soft_bg,
                            );
                            fill_rect_clipped(
                                buffer,
                                buf_w,
                                clip_bot.max(0) as u32,
                                right_x,
                                draw_y + 2,
                                4,
                                ((draw_end - draw_y) - 4).max(2) as u32,
                                theme.accent,
                            );
                        }
                    }

                    let mut ty = cursor_y + tip_pad - 2;
                    for tl in &lines {
                        if ty >= clip_top && ty + line_h <= clip_bot {
                            draw_text_clipped(
                                buffer,
                                buf_w,
                                clip_bot.max(0) as u32,
                                right_x + tip_pad + 4,
                                ty,
                                right_x + right_w - tip_pad,
                                tl,
                                body_sz,
                                theme.accent_soft_fg,
                                false,
                            );
                        }
                        ty += line_h;
                    }
                    cursor_y += box_h as i32 + (10.0 * s).round() as i32;
                }
            }
        }

        // Update max_scroll dynamically for mouse wheel & arrow navigation
        let total_content_h = (cursor_y - content_start_y + (20.0 * s).round() as i32).max(0) as f32;
        let visible_h = (body_h as f32 - 16.0 * s).max(40.0);
        let new_max_scroll = (total_content_h - visible_h).max(0.0);
        if let Some(ref mut g) = self.user_guide_modal {
            g.max_scroll = new_max_scroll;
            if g.scroll_y > new_max_scroll {
                g.scroll_y = new_max_scroll;
            }
        }

        // Subtle scrollbar indicator on right pane if scrollable
        if new_max_scroll > 1.0 {
            let sb_x = cx + cw as i32 - (10.0 * s).round() as i32;
            let sb_y = body_top + (8.0 * s).round() as i32;
            let sb_h = (body_h as i32 - (16.0 * s).round() as i32).max(24) as u32;
            let sb_w = (5.0 * s).round().clamp(4.0, 8.0) as u32;
            fill_rounded_rect(buffer, buf_w, buf_h, sb_x, sb_y, sb_w, sb_h, 2.5, theme.surface_elevated);
            let frac = (visible_h / total_content_h).clamp(0.12, 1.0);
            let thumb_h = ((sb_h as f32) * frac).round().max(18.0) as u32;
            let ratio = (state.scroll_y / new_max_scroll).clamp(0.0, 1.0);
            let thumb_y = sb_y + (((sb_h.saturating_sub(thumb_h)) as f32) * ratio).round() as i32;
            fill_rounded_rect(buffer, buf_w, buf_h, sb_x, thumb_y, sb_w, thumb_h, 2.5, theme.text_muted);
        }

        // 4. Footer Bar (Chapter counter + Prev / Next buttons)
        let ftr_y = cy + ch as i32 - ftr_h;
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            cx + 1,
            ftr_y,
            cw.saturating_sub(2),
            1,
            theme.divider,
        );
        let ftr_hint = format!(
            "Chapter {} of {}  •  Scroll with Mouse Wheel or Arrow Keys  •  Esc to close",
            cur_idx + 1,
            USER_GUIDE_CHAPTERS.len()
        );
        let prev_x = hits
            .iter()
            .find(|h| h.hit == UserGuideHit::PrevChapter)
            .map(|h| h.x)
            .unwrap_or(cx + cw as i32 - 260);
        draw_text_vcenter(
            buffer,
            buf_w,
            buf_h,
            cx + pad,
            ftr_y,
            ftr_h as u32,
            prev_x - 8,
            &ftr_hint,
            small_sz,
            theme.text_muted,
            false,
        );

        for h in &hits {
            let hov = self.hovered_widget == Some(UiHit::UserGuide(h.hit));
            match h.hit {
                UserGuideHit::PrevChapter => {
                    let enabled = cur_idx > 0;
                    let bg = if enabled && hov { theme.hover_bg } else { theme.surface_elevated };
                    fill_rounded_rect(buffer, buf_w, buf_h, h.x, h.y, h.w, h.h, 6.0, bg);
                    stroke_rounded_rect(buffer, buf_w, buf_h, h.x, h.y, h.w, h.h, 6.0, theme.border_subtle);
                    let lbl = "◀ Prev Chapter";
                    let lw = measure_text(lbl, small_sz, true);
                    draw_text_vcenter(
                        buffer,
                        buf_w,
                        buf_h,
                        h.x + ((h.w as f32 - lw) * 0.5).round() as i32,
                        h.y,
                        h.h,
                        h.x + h.w as i32,
                        lbl,
                        small_sz,
                        if enabled { theme.text_primary } else { theme.text_muted },
                        true,
                    );
                }
                UserGuideHit::NextChapter => {
                    let enabled = cur_idx + 1 < USER_GUIDE_CHAPTERS.len();
                    let bg = if enabled {
                        if hov { theme.accent_hover } else { theme.accent }
                    } else {
                        theme.surface_elevated
                    };
                    fill_rounded_rect(buffer, buf_w, buf_h, h.x, h.y, h.w, h.h, 6.0, bg);
                    if !enabled {
                        stroke_rounded_rect(buffer, buf_w, buf_h, h.x, h.y, h.w, h.h, 6.0, theme.border_subtle);
                    }
                    let lbl = "Next Chapter ▶";
                    let lw = measure_text(lbl, small_sz, true);
                    draw_text_vcenter(
                        buffer,
                        buf_w,
                        buf_h,
                        h.x + ((h.w as f32 - lw) * 0.5).round() as i32,
                        h.y,
                        h.h,
                        h.x + h.w as i32,
                        lbl,
                        small_sz,
                        if enabled { theme.text_on_accent } else { theme.text_muted },
                        true,
                    );
                }
                _ => {}
            }
        }
    }
}

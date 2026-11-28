# MicroPDF Built-In User Guide Framework

This document defines the exact structure, writing style, and maintenance checklist for MicroPDF's built-in offline manual (`Help -> User Guide (How to Use)` in [`src/viewer/user_guide.rs`](../src/viewer/user_guide.rs)).

---

## 1. Audience & Plain-English Writing Rules

MicroPDF is used by everyday people — including readers with **little or no IT literacy**. Every chapter in `src/viewer/user_guide.rs` MUST follow these rules:

1. **Zero jargon**: Avoid internal PDF/tech terms (`AcroForm`, `XObject`, `FlateDecode`, `softbuffer`, `CropBox`, `LRU`, `DPI` without plain explanation). Say *"fillable form box"*, *"invisible personal details"*, or *"150 DPI (best for email)"*.
2. **Numbered Step-by-Step Walkthroughs**: Every user task must be broken into short `GuideBlock::Step { number, text }` items (`Step 1`, `Step 2`, `Step 3`) using everyday action verbs (`Click`, `Drag`, `Type`, `Press`, `Choose`).
3. **Exact Menu Trails**: Always quote the **exact** menu path from `MenuCategory::items()` in `src/viewer/state.rs` (for example, `Click File -> Create PDF from Images...`, never an outdated or paraphrased menu label).
4. **Friendly Tip Callouts**: End each major chapter with at least one `GuideBlock::Tip(...)` highlighting a safety net (like `Ctrl+Z` Undo, `Esc` to exit a tool, or how originals stay safe).
5. **Zero External Dependencies & Zero Idle RAM**: Keep all chapters as static `&'static str` data inside `USER_GUIDE_CHAPTERS` so the guide uses `0 KB` of heap memory when closed and exports cleanly via `format_full_user_guide_txt()`.

---

## 2. Chapter Data Schema (`src/viewer/user_guide.rs`)

```rust
pub(super) struct GuideChapter {
    pub(super) number: u8,                  // 1-based chapter index (1..=N)
    pub(super) short_title: &'static str,   // Left sidebar label (<= 30 chars)
    pub(super) title: &'static str,         // Right pane heading
    pub(super) summary: &'static str,       // Friendly 1-sentence overview
    pub(super) blocks: &'static [GuideBlock],
}

pub(super) enum GuideBlock {
    Heading(&'static str),                  // Sub-task heading ("How to ...")
    Paragraph(&'static str),                // Short explanation or bullet lines
    Step { number: u8, text: &'static str },// Numbered "Step N" badge + instruction
    Tip(&'static str),                      // Soft-accent beginner tip callout box
}
```

### Standard Chapter Template

```rust
GuideChapter {
    number: X,
    short_title: "X. Short Topic Name",
    title: "Full Friendly Topic Title",
    summary: "One plain sentence explaining what the reader will learn here.",
    blocks: &[
        GuideBlock::Heading("How to <Do Task>"),
        GuideBlock::Step { number: 1, text: "Click <Exact Menu -> Exact Item>." },
        GuideBlock::Step { number: 2, text: "Adjust <simple setting name>." },
        GuideBlock::Step { number: 3, text: "Click <Exact Button Name>." },
        GuideBlock::Tip("Helpful everyday tip or safety net (e.g., Ctrl+Z to undo)."),
    ],
}
```

---

## 3. UI Module → Guide Chapter Maintenance Checklist

Whenever you modify any file below, update the corresponding chapter in `src/viewer/user_guide.rs` AND the matching shortcut row in `Help -> Keyboard & Mouse Shortcuts` (`src/viewer/overlays.rs`):

| Source Module / Feature Area | Guide Chapter(s) to Update | Also Check |
| :--- | :--- | :--- |
| Opening, Saving, Undo/Redo, Unsaved Guard (`tabs.rs`, `doc_props_modal.rs`, `image_tools_modal.rs`) | **Chapter 1** (*Welcome & Opening Files*) | `overlays.rs` Shortcuts |
| View Modes, Scroll Modes, Zoom, Eye-Care Modes, Reading Continuity, UI Scale (`render.rs`, `bottom_bar.rs`, `text_view.rs`, `state.rs`) | **Chapter 2** (*Reading Comfort, Zoom & Page Views*) | `overlays.rs` Shortcuts |
| Multi-Tab Strip, Overflow List, Tab Context Menu, Session Restore (`tabs.rs`, `state.rs`) | **Chapter 3** (*Working with Multiple Tabs*) | `overlays.rs` Shortcuts |
| Text Selection, Annotations, Note Popover, Annotation Inspector, Interactive Form Filling (`input.rs`, `doc_props_modal.rs`, `engine.rs`) | **Chapter 4** (*Highlighting, Notes, Shapes & Filling Forms*) | `overlays.rs` Shortcuts |
| Thumbnail Grid, Multi-Select Batch Bar, Drag-Drop, Insert PDFs, Combine, Split, Crop (`grid.rs`, `combine_modal.rs`, `input.rs`) | **Chapter 5** (*Organizing Pages*) | `overlays.rs` Shortcuts |
| Header/Footer + Profiles, Page Numbering + Skip Pages, Bookmark Sidebar (`doc_tools_modal.rs`, `bookmarks.rs`) | **Chapter 6** (*Headers, Footers, Page Numbers & Bookmarks*) | `overlays.rs` Shortcuts + Bookmark Format guide |
| Print Preview, Paper Sizes, Auto-Trim, Duplex, Manual 2-Sided, Booklet (`print_preview.rs`, `print.rs`) | **Chapter 7** (*Printing, Booklets & 2-Sided on Any Printer*) | `overlays.rs` Shortcuts |
| Word/Excel/Markdown/LaTeX/Image Export, Stamps, Watermarks (`export.rs`, `image_tools_modal.rs`) | **Chapter 8** (*Converting & Exporting*) | `README.md` |
| Redaction, Document Properties & Privacy Scrubber, Flatten/Unlock, Optimize (`engine.rs`, `doc_props_modal.rs`, `optimize.rs`) | **Chapter 9** (*Privacy & Shrinking PDF Size*) | `README.md` |
| Search (`Aa`/`W`/`▲▼`), Side-by-Side Compare + Visual Diff, Ruler Measure Tool (`overlays.rs`, `compare_mode.rs`, `measure.rs`) | **Chapter 10** (*Compare, Measure & Quick Troubleshooting*) | `overlays.rs` Shortcuts |
| Word `.docx` Reader & Navigation Pane, Comic `.cbz` Studio, Thumbnails, RTL Manga, Spreads & WebP (`docx_reader.rs`, `comic.rs`, `comic_view.rs`, `book_ui.rs`) | **Chapter 1** (*Welcome & Opening Files*) & **Chapter 11** (*Word & Comic Readers*) | `overlays.rs` Shortcuts & `README.md` |
| Any top menu rename (`MenuCategory::items()` in `src/viewer/state.rs`) | Search `user_guide.rs` for the old menu label and update every match | `docs/USER_GUIDE_FRAMEWORK.md` & `README.md` |

---

## 4. Verification Before Opening a PR

1. Run `cargo test test_tier3_user_guide` to verify chapter numbering, non-empty summaries/steps, and `.txt` export integrity.
2. Visually inspect the `Help -> User Guide (How to Use)` modal in both Light and Dark themes whenever adding a new chapter or changing modal layout.

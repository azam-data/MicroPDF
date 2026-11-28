# Third-Party Notices

MicroPDF depends on the following components. Each keeps its own license.

| Component | License | Notes |
|---|---|---|
| PDFium (`pdfium.dll` / `libpdfium`) | BSD-3-Clause (some parts Apache-2.0) | Shipped next to the executable. When you redistribute a build, include PDFium's `LICENSE` file from the pdfium-binaries archive you used. |
| pdfium-render | MIT OR Apache-2.0 | Rust bindings to PDFium |
| lopdf | MIT | PDF object editing |
| winit | Apache-2.0 | Windowing |
| softbuffer | MIT OR Apache-2.0 | CPU frame buffer |
| rfd | MIT | Native file dialogs |
| image | MIT OR Apache-2.0 | PNG/JPEG decoding |
| anyhow | MIT OR Apache-2.0 | Error handling |
| ab_glyph | Apache-2.0 | CPU TrueType font rasterizer for UI text |
| flate2 | MIT OR Apache-2.0 | Zlib decompression for embedded UI fonts |
| Liberation Sans (Latin subset) | SIL Open Font License 1.1 | Embedded UI font (`assets/ui_regular.ttf.z`, `assets/ui_bold.ttf.z`) |
| Liberation Serif (Latin subset) | SIL Open Font License 1.1 | Embedded reading serif font (`assets/book_serif_regular.ttf.z`, `assets/book_serif_bold.ttf.z`, `assets/book_serif_italic.ttf.z`) |
| Liberation Mono (Latin subset) | SIL Open Font License 1.1 | Embedded monospace code font (`assets/code_mono_regular.ttf.z`) |
| roxmltree | MIT OR Apache-2.0 | Namespace-aware XML/XHTML DOM parser for EPUB |
| hypher | MIT OR Apache-2.0 | Syllable hyphenation for justified book typography |

Run `cargo tree` for the full dependency list. Tools such as `cargo-about` can generate a complete notice file for release builds.

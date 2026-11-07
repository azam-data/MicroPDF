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

Run `cargo tree` for the full dependency list. Tools such as `cargo-about` can generate a complete notice file for release builds.

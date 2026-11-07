# Changelog

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

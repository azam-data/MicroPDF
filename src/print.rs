//! Printing (`Ctrl/Cmd+P`, File -> Print...). Always prints the current in-memory document, so
//! unsaved edits and redactions are what reaches the printer.
//!
//! - Windows: native `PrintDlgW`, then GDI. Each page is rendered at the printer's resolution in
//!   horizontal strips of `PRINT_STRIP_ROWS` rows into one reused buffer, never as a full page.
//! - macOS/Linux: the selected pages are written to a temporary PDF in the OS temp folder and
//!   handed to `lp`; the file is deleted as soon as `lp` has exited (CUPS has spooled its own
//!   copy by then), and on every error path.

use crate::engine::PdfEngine;
use anyhow::{Context, Result};
use pdfium_render::prelude::*;
use std::path::{Path, PathBuf};

/// Horizontal strip height (device pixel rows) for printing. A4 width at 600 dpi is ~4960 px,
/// so one strip is ~5 MB instead of ~140 MB for the whole page.
#[cfg_attr(not(any(windows, test)), allow(dead_code))] // GDI printing is Windows-only
pub const PRINT_STRIP_ROWS: u32 = 256;

/// Prefix of the temporary PDFs handed to `lp`.
const TEMP_PRINT_PREFIX: &str = "micropdf_print_";

#[cfg_attr(not(any(windows, test)), allow(dead_code))] // GDI printing is Windows-only
/// Where a page goes on the printable area, in device pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrintPlacement {
    pub render_w: u32,
    pub render_h: u32,
    pub offset_x: u32,
    pub offset_y: u32,
}

#[cfg_attr(not(any(windows, test)), allow(dead_code))] // GDI printing is Windows-only
/// Places a `page_w_pts` x `page_h_pts` page on a printable area of `avail_w` x `avail_h`
/// device pixels at `dpi_x` x `dpi_y`. With `fit_to_page` the page is scaled (up or down) to
/// fill the area; without it the page is printed at actual size and only shrunk if it does not
/// fit. Aspect ratio is kept and the page is centred.
pub fn place_page_on_printable_area(
    page_w_pts: f32,
    page_h_pts: f32,
    dpi_x: u32,
    dpi_y: u32,
    avail_w: u32,
    avail_h: u32,
    fit_to_page: bool,
) -> PrintPlacement {
    let page_w_pts = page_w_pts.max(1.0);
    let page_h_pts = page_h_pts.max(1.0);
    let avail_w = avail_w.max(1);
    let avail_h = avail_h.max(1);
    let nat_w = (page_w_pts * dpi_x.max(72) as f32 / 72.0).max(1.0);
    let nat_h = (page_h_pts * dpi_y.max(72) as f32 / 72.0).max(1.0);

    let scale = (avail_w as f32 / nat_w).min(avail_h as f32 / nat_h);
    let scale = if fit_to_page { scale } else { scale.min(1.0) };
    let render_w = ((nat_w * scale).round() as u32).clamp(1, avail_w);
    let render_h = ((nat_h * scale).round() as u32).clamp(1, avail_h);

    PrintPlacement {
        render_w,
        render_h,
        offset_x: (avail_w - render_w) / 2,
        offset_y: (avail_h - render_h) / 2,
    }
}

#[cfg_attr(not(any(windows, test)), allow(dead_code))] // GDI printing is Windows-only
/// Splits `render_h` rows into consecutive `(y, rows)` strips of at most `strip_rows` rows
/// (at least 16, so a bad value cannot cause thousands of tiny strips).
pub fn strip_bands(render_h: u32, strip_rows: u32) -> impl Iterator<Item = (u32, u32)> {
    let rows = strip_rows.max(16);
    (0..render_h)
        .step_by(rows as usize)
        .map(move |y| (y, rows.min(render_h - y)))
}

#[cfg_attr(not(any(windows, test)), allow(dead_code))] // GDI printing is Windows-only
/// Renders page `page_idx` of `doc` for printing at `dpi_x` x `dpi_y` onto a printable area of
/// `avail_w` x `avail_h` device pixels, one horizontal strip at a time. `strip_buf` is reused for
/// every strip, so peak memory is one strip (`render_w * strip_rows * 4` bytes), not one page.
///
/// `on_strip(dest_x, dest_y, w, h, pixels)` receives each strip in printable-area coordinates;
/// pixels are top-down rows of `0x00RRGGBB` (BGRx bytes, as a 32-bit DIB expects).
///
/// Filled-in form fields print as they look on screen. PDFium leaves form widgets out of
/// matrix renders once a document's form environment is set up (they are meant to be drawn by
/// `FPDF_FFLDraw`, which cannot render a strip), so a page with widgets is rendered from a
/// flattened one-page copy instead; `doc` itself is never modified. The page is closed before
/// returning. Returns the placement used.
#[allow(clippy::too_many_arguments)]
pub fn render_page_in_horizontal_strips<F>(
    engine: &PdfEngine,
    doc: &PdfDocument,
    page_idx: u16,
    dpi_x: u32,
    dpi_y: u32,
    avail_w: u32,
    avail_h: u32,
    fit_to_page: bool,
    strip_rows: u32,
    strip_buf: &mut Vec<u32>,
    on_strip: F,
) -> Result<PrintPlacement>
where
    F: FnMut(u32, u32, u32, u32, &[u32]) -> Result<()>,
{
    let page = doc
        .pages()
        .get(page_idx)
        .with_context(|| format!("Page {} is out of range for printing", page_idx + 1))?;
    let args = (dpi_x, dpi_y, avail_w, avail_h, fit_to_page, strip_rows);
    if !PdfEngine::page_has_form_widgets(&page) {
        return render_strips_of_page(engine, page, args, strip_buf, on_strip);
    }
    drop(page);

    let copy = engine
        .flattened_page_copy(doc, page_idx)
        .context("Failed to prepare a form page for printing")?;
    let copied = copy.pages().get(0).context("Print copy has no page")?;
    render_strips_of_page(engine, copied, args, strip_buf, on_strip)
}

/// The strip loop of `render_page_in_horizontal_strips`; closes `page` before returning.
fn render_strips_of_page<F>(
    engine: &PdfEngine,
    page: PdfPage,
    (dpi_x, dpi_y, avail_w, avail_h, fit_to_page, strip_rows): (u32, u32, u32, u32, bool, u32),
    strip_buf: &mut Vec<u32>,
    mut on_strip: F,
) -> Result<PrintPlacement>
where
    F: FnMut(u32, u32, u32, u32, &[u32]) -> Result<()>,
{
    let page_w_pts = page.width().value.max(1.0);
    let page_h_pts = page.height().value.max(1.0);
    let placement = place_page_on_printable_area(
        page_w_pts,
        page_h_pts,
        dpi_x,
        dpi_y,
        avail_w,
        avail_h,
        fit_to_page,
    );
    let render_w = placement.render_w;
    let sx = render_w as f32 / page_w_pts;
    let sy = placement.render_h as f32 / page_h_pts;
    let bindings = engine.pdfium.bindings();

    for (strip_y, strip_h) in strip_bands(placement.render_h, strip_rows) {
        let pixel_count = render_w as usize * strip_h as usize;
        strip_buf.clear();
        strip_buf.resize(pixel_count, 0x00FF_FFFF);

        // Same transform as the zoom tiles: scale page points to device pixels, then shift the
        // page up by `strip_y` so this strip's rows land in the bitmap.
        let config = PdfRenderConfig::new()
            .set_fixed_width(render_w as i32)
            .set_fixed_height(strip_h as i32)
            .render_form_data(false)
            .render_annotations(true)
            .use_print_quality(true)
            .limit_render_image_cache_size(true)
            .set_reverse_byte_order(false)
            .reset_matrix(PdfMatrix::new(sx, 0.0, 0.0, sy, 0.0, -(strip_y as f32)))
            .context("Invalid print strip transformation matrix")?;

        {
            let byte_slice: &mut [u8] = unsafe {
                std::slice::from_raw_parts_mut(strip_buf.as_mut_ptr() as *mut u8, pixel_count * 4)
            };
            let mut bitmap = unsafe {
                PdfBitmap::from_bytes(
                    render_w as i32,
                    strip_h as i32,
                    PdfBitmapFormat::BGRx,
                    byte_slice,
                    bindings,
                )
            }
            .context("Failed to wrap print strip buffer in PdfBitmap")?;
            page.render_into_bitmap_with_config(&mut bitmap, &config)
                .context("Failed to render PDF print strip")?;
        }
        for px in strip_buf.iter_mut() {
            *px &= 0x00FF_FFFF;
        }

        on_strip(
            placement.offset_x,
            placement.offset_y + strip_y,
            render_w,
            strip_h,
            &strip_buf[..pixel_count],
        )?;
    }

    drop(page);
    Ok(placement)
}

/// Parses a 1-based page selection such as `1-5, 8, 10-` into sorted, de-duplicated 0-based
/// indices. Empty or `all` selects every page. `N-` means page N to the end.
pub fn parse_page_selection(input: &str, total_pages: u16) -> Result<Vec<u16>> {
    if total_pages == 0 {
        anyhow::bail!("Document has 0 pages");
    }
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("all") {
        return Ok((0..total_pages).collect());
    }

    let parse_num = |s: &str, part: &str| -> Result<u16> {
        let n: u16 = s
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("'{part}' is not a page number or range"))?;
        if n == 0 || n > total_pages {
            anyhow::bail!("page {n} is outside 1-{total_pages}");
        }
        Ok(n)
    };

    let mut pages = std::collections::BTreeSet::new();
    for part in trimmed.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (start, end) = match part.split_once('-') {
            Some((a, b)) => {
                let start = parse_num(a, part)?;
                let end = if b.trim().is_empty() { total_pages } else { parse_num(b, part)? };
                if start > end {
                    anyhow::bail!("range '{part}' runs backwards");
                }
                (start, end)
            }
            None => {
                let n = parse_num(part, part)?;
                (n, n)
            }
        };
        pages.extend((start - 1)..end);
    }
    if pages.is_empty() {
        anyhow::bail!("no pages selected");
    }
    Ok(pages.into_iter().collect())
}

#[cfg_attr(windows, allow(dead_code))] // CUPS printing is macOS/Linux-only
/// Parses the output of `lpstat -p -d` into `(printers, index of the default printer)`.
pub fn parse_lpstat_output(output: &str) -> (Vec<String>, usize) {
    let mut printers: Vec<String> = Vec::new();
    let mut default_printer: Option<String> = None;

    for raw_line in output.lines() {
        let line = raw_line.trim();
        if let Some(rest) = line.strip_prefix("printer ") {
            if let Some(name) = rest.split_whitespace().next() {
                let clean = name.trim().to_string();
                if !clean.is_empty() && !printers.contains(&clean) {
                    printers.push(clean);
                }
            }
        } else if let Some(rest) = line.strip_prefix("system default destination:") {
            let def = rest.trim().to_string();
            if !def.is_empty() {
                default_printer = Some(def);
            }
        }
    }

    if let Some(ref def) = default_printer {
        if !printers.contains(def) {
            printers.insert(0, def.clone());
        }
    }

    let selected = default_printer
        .and_then(|def| printers.iter().position(|p| p == &def))
        .unwrap_or(0);

    (printers, selected)
}

#[cfg_attr(windows, allow(dead_code))] // CUPS printing is macOS/Linux-only
/// Lists CUPS printers with `lpstat -p -d`.
pub fn discover_cups_printers() -> (Vec<String>, usize) {
    match std::process::Command::new("lpstat").args(["-p", "-d"]).output() {
        Ok(out) => parse_lpstat_output(&String::from_utf8_lossy(&out.stdout)),
        Err(_) => (Vec::new(), 0),
    }
}

/// A temporary print PDF that is deleted when this value is dropped, so every return path
/// (success, `lp` failure, `lp` missing, serialization error) removes it.
pub struct TempPrintFile {
    path: PathBuf,
}

impl TempPrintFile {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempPrintFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Deletes print temp files older than an hour in `dir`, left behind if the app was killed
/// while `lp` was running.
fn remove_stale_print_temp_files(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let hour = std::time::Duration::from_secs(3600);
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with(TEMP_PRINT_PREFIX) && name.ends_with(".pdf")) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > hour);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Writes the selected 0-based `page_indices` of the in-memory `doc` (unsaved edits included)
/// to a new temporary PDF in `dir`.
///
/// Non-redacted documents are streamed straight to the file by PDFium (no full copy in RAM).
/// After a redaction the bytes go through the same deep clean as a save (orphaned objects,
/// /Info and thumbnails removed), because CUPS may keep spooled job files; if that clean-up
/// fails, nothing is written.
pub fn write_pages_to_temp_pdf(
    engine: &PdfEngine,
    doc: &PdfDocument,
    page_indices: &[u16],
    redacted: bool,
    dir: &Path,
) -> Result<TempPrintFile> {

    let total = doc.pages().len();
    if page_indices.is_empty() {
        anyhow::bail!("No pages selected to print");
    }
    if let Some(&bad) = page_indices.iter().find(|&&p| p >= total) {
        anyhow::bail!("Page {} out of range (1-{total})", bad + 1);
    }
    remove_stale_print_temp_files(dir);

    let all_pages_in_order = page_indices.len() == total as usize
        && page_indices.iter().enumerate().all(|(i, &p)| p as usize == i);
    let subset_doc = if all_pages_in_order {
        None
    } else {
        let mut dest = engine
            .pdfium
            .create_new_pdf()
            .context("Failed to create temporary print PDF")?;
        for &src_idx in page_indices {
            let dst_len = dest.pages().len();
            dest.pages_mut()
                .copy_page_from_document(doc, src_idx, dst_len)
                .with_context(|| format!("Failed to copy page {} for printing", src_idx + 1))?;
        }
        Some(dest)
    };

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = dir.join(format!("{TEMP_PRINT_PREFIX}{}_{nanos}.pdf", std::process::id()));
    // create_new: never write through an existing file or symlink of the same name.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| format!("Failed to create temporary print PDF {path:?}"))?;
    let temp = TempPrintFile { path };

    match &subset_doc {
        Some(subset) => write_doc_for_print(subset, redacted, &mut file)?,
        None => write_doc_for_print(doc, redacted, &mut file)?,
    }
    drop(file);
    Ok(temp)
}

/// Writes `doc` to `file`: streamed by PDFium, or deep-cleaned first after a redaction.
fn write_doc_for_print(doc: &PdfDocument, redacted: bool, file: &mut std::fs::File) -> Result<()> {
    use std::io::Write;
    if redacted {
        let raw = doc
            .save_to_bytes()
            .context("Failed to serialize in-memory PDF for printing")?;
        let cleaned = PdfEngine::embed_metadata_in_pdf_bytes(&raw, 0, &[], true)
            .context("Refusing to print: post-redaction clean-up failed")?;
        drop(raw);
        file.write_all(&cleaned)
            .context("Failed to write temporary print PDF")?;
    } else {
        doc.save_to_writer(file)
            .context("Failed to write temporary print PDF")?;
    }
    file.flush().context("Failed to write temporary print PDF")
}

/// What to print through CUPS.
pub struct CupsPrintRequest<'a> {
    pub printer: &'a str,
    pub copies: u16,
    pub range_input: &'a str,
    pub fit_to_page: bool,
    pub title: &'a str,
}

/// Result of a successful `lp` submission.
#[derive(Debug)]
pub struct CupsPrintOutcome {
    pub pages: usize,
    pub copies: u16,
    /// `lp`'s stdout, e.g. "request id is HP-42 (1 file(s))".
    pub job_info: String,
}

/// Prints the selected pages of the in-memory `doc` with
/// `lp -d <printer> -n <copies> -t <title> [-o fit-to-page] <temp file>`.
pub fn print_document_cups(
    engine: &PdfEngine,
    doc: &PdfDocument,
    req: &CupsPrintRequest,
    redacted: bool,
) -> Result<CupsPrintOutcome> {
    print_document_cups_with(engine, doc, req, redacted, "lp".as_ref(), &std::env::temp_dir())
}

/// `print_document_cups` with the `lp` program and temp folder given explicitly (for tests).
pub fn print_document_cups_with(
    engine: &PdfEngine,
    doc: &PdfDocument,
    req: &CupsPrintRequest,
    redacted: bool,
    lp_program: &std::ffi::OsStr,
    temp_dir: &Path,
) -> Result<CupsPrintOutcome> {
    let printer = req.printer.trim();
    if printer.is_empty() {
        anyhow::bail!("No printer selected");
    }
    let page_indices = parse_page_selection(req.range_input, doc.pages().len())?;
    let copies = req.copies.clamp(1, 99);

    let temp = write_pages_to_temp_pdf(engine, doc, &page_indices, redacted, temp_dir)?;

    let mut cmd = std::process::Command::new(lp_program);
    cmd.arg("-d")
        .arg(printer)
        .arg("-n")
        .arg(copies.to_string())
        .arg("-t")
        .arg(if req.title.is_empty() { "MicroPDF" } else { req.title });
    if req.fit_to_page {
        cmd.arg("-o").arg("fit-to-page");
    }
    cmd.arg(temp.path());

    // `lp` exits once CUPS has accepted and spooled its own copy of the file.
    let output = cmd.output();
    drop(temp);

    let output = output.context("Failed to run 'lp'")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        anyhow::bail!("lp failed: {detail}");
    }

    Ok(CupsPrintOutcome {
        pages: page_indices.len(),
        copies,
        job_info: String::from_utf8_lossy(&output.stdout).trim().to_string(),
    })
}

/// Shows the native print dialog and prints the in-memory `doc` with GDI.
///
/// Page range and copies come from the dialog. Each page is fitted to the printable area and
/// the paper orientation follows the page (portrait/landscape), switched with `ResetDCW` only
/// when it changes. Returns `Ok(None)` if the user cancelled.
#[cfg(target_os = "windows")]
pub fn print_document_windows(
    engine: &PdfEngine,
    doc: &PdfDocument,
    doc_title: &str,
    hwnd_owner: Option<isize>,
) -> Result<Option<(usize, u16)>> {
    use std::ffi::c_void;

    const PD_ALLPAGES: u32 = 0x0000_0000;
    const PD_PAGENUMS: u32 = 0x0000_0002;
    const PD_NOSELECTION: u32 = 0x0000_0004;
    const PD_RETURNDC: u32 = 0x0000_0100;
    const PD_USEDEVMODECOPIESANDCOLLATE: u32 = 0x0004_0000;

    const HORZRES: i32 = 8;
    const VERTRES: i32 = 10;
    const LOGPIXELSX: i32 = 88;
    const LOGPIXELSY: i32 = 90;

    const BI_RGB: u32 = 0;
    const DIB_RGB_COLORS: u32 = 0;
    const SRCCOPY: u32 = 0x00CC_0020;
    const GDI_ERROR: i32 = -1;

    const DM_ORIENTATION: u32 = 0x0000_0001;
    const DM_COPIES: u32 = 0x0000_0100;
    const DMORIENT_PORTRAIT: i16 = 1;
    const DMORIENT_LANDSCAPE: i16 = 2;

    // commdlg.h packs its structs to 1 byte on 32-bit Windows only.
    #[cfg_attr(target_pointer_width = "32", repr(C, packed))]
    #[cfg_attr(not(target_pointer_width = "32"), repr(C))]
    struct PrintDlgW {
        l_struct_size: u32,
        hwnd_owner: *mut c_void,
        h_dev_mode: *mut c_void,
        h_dev_names: *mut c_void,
        h_dc: *mut c_void,
        flags: u32,
        n_from_page: u16,
        n_to_page: u16,
        n_min_page: u16,
        n_max_page: u16,
        n_copies: u16,
        h_instance: *mut c_void,
        l_cust_data: isize,
        lpfn_print_hook: *const c_void,
        lpfn_setup_hook: *const c_void,
        lp_print_template_name: *const u16,
        lp_setup_template_name: *const u16,
        h_print_template: *mut c_void,
        h_setup_template: *mut c_void,
    }
    #[cfg(target_pointer_width = "64")]
    const _: () = assert!(std::mem::size_of::<PrintDlgW>() == 120);

    #[repr(C)]
    struct DocInfoW {
        cb_size: i32,
        lpsz_doc_name: *const u16,
        lpsz_output: *const u16,
        lpsz_datatype: *const u16,
        fw_type: u32,
    }

    #[repr(C)]
    struct BitmapInfoHeader {
        bi_size: u32,
        bi_width: i32,
        bi_height: i32,
        bi_planes: u16,
        bi_bit_count: u16,
        bi_compression: u32,
        bi_size_image: u32,
        bi_x_pels_per_meter: i32,
        bi_y_pels_per_meter: i32,
        bi_clr_used: u32,
        bi_clr_important: u32,
    }

    #[repr(C)]
    struct BitmapInfo {
        bmi_header: BitmapInfoHeader,
        bmi_colors: [u32; 1],
    }

    /// The leading fields of DEVMODEW, up to dmCopies (offsets 0..=88).
    #[repr(C)]
    struct DevModeWPrefix {
        dm_device_name: [u16; 32],
        dm_spec_version: u16,
        dm_driver_version: u16,
        dm_size: u16,
        dm_driver_extra: u16,
        dm_fields: u32,
        dm_orientation: i16,
        dm_paper_size: i16,
        dm_paper_length: i16,
        dm_paper_width: i16,
        dm_scale: i16,
        dm_copies: i16,
    }

    #[link(name = "comdlg32")]
    unsafe extern "system" {
        fn PrintDlgW(lppd: *mut PrintDlgW) -> i32;
    }

    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn StartDocW(hdc: *mut c_void, lpdi: *const DocInfoW) -> i32;
        fn EndDoc(hdc: *mut c_void) -> i32;
        fn StartPage(hdc: *mut c_void) -> i32;
        fn EndPage(hdc: *mut c_void) -> i32;
        fn AbortDoc(hdc: *mut c_void) -> i32;
        fn DeleteDC(hdc: *mut c_void) -> i32;
        fn GetDeviceCaps(hdc: *mut c_void, index: i32) -> i32;
        fn ResetDCW(hdc: *mut c_void, lpdm: *const c_void) -> *mut c_void;
        fn StretchDIBits(
            hdc: *mut c_void,
            x_dest: i32,
            y_dest: i32,
            dest_width: i32,
            dest_height: i32,
            x_src: i32,
            y_src: i32,
            src_width: i32,
            src_height: i32,
            lp_bits: *const c_void,
            lpbmi: *const BitmapInfo,
            i_usage: u32,
            rop: u32,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalLock(hmem: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(hmem: *mut c_void) -> i32;
        fn GlobalFree(hmem: *mut c_void) -> *mut c_void;
    }

    /// Frees what PrintDlgW returned, on every exit path.
    struct DialogResources {
        hdc: *mut c_void,
        h_dev_mode: *mut c_void,
        h_dev_names: *mut c_void,
    }
    impl Drop for DialogResources {
        fn drop(&mut self) {
            unsafe {
                if !self.hdc.is_null() {
                    DeleteDC(self.hdc);
                }
                if !self.h_dev_mode.is_null() {
                    GlobalFree(self.h_dev_mode);
                }
                if !self.h_dev_names.is_null() {
                    GlobalFree(self.h_dev_names);
                }
            }
        }
    }

    let total_pages = doc.pages().len();
    if total_pages == 0 {
        anyhow::bail!("Document has 0 pages");
    }

    let mut pd: PrintDlgW = unsafe { std::mem::zeroed() };
    pd.l_struct_size = std::mem::size_of::<PrintDlgW>() as u32;
    pd.hwnd_owner = hwnd_owner.unwrap_or(0) as *mut c_void;
    pd.flags = PD_ALLPAGES | PD_RETURNDC | PD_NOSELECTION | PD_USEDEVMODECOPIESANDCOLLATE;
    pd.n_from_page = 1;
    pd.n_to_page = total_pages;
    pd.n_min_page = 1;
    pd.n_max_page = total_pages;
    pd.n_copies = 1;

    let accepted = unsafe { PrintDlgW(&mut pd) };
    let res = DialogResources {
        hdc: pd.h_dc,
        h_dev_mode: pd.h_dev_mode,
        h_dev_names: pd.h_dev_names,
    };
    if accepted == 0 || res.hdc.is_null() {
        return Ok(None);
    }
    let hdc = res.hdc;

    let (start_1based, end_1based) = if (pd.flags & PD_PAGENUMS) != 0 {
        let from = pd.n_from_page.clamp(1, total_pages);
        (from, pd.n_to_page.clamp(from, total_pages))
    } else {
        (1, total_pages)
    };

    // With PD_USEDEVMODECOPIESANDCOLLATE, a driver that can make copies itself gets them via
    // DEVMODE and nCopies comes back as 1; otherwise nCopies is how often we send the pages.
    let loop_copies = pd.n_copies.max(1);
    let mut driver_copies: u16 = 1;
    let mut current_orientation: Option<i16> = None;
    if !res.h_dev_mode.is_null() {
        unsafe {
            let dm = GlobalLock(res.h_dev_mode) as *const DevModeWPrefix;
            if !dm.is_null() {
                if (*dm).dm_fields & DM_COPIES != 0 {
                    driver_copies = (*dm).dm_copies.max(1) as u16;
                }
                if (*dm).dm_fields & DM_ORIENTATION != 0 {
                    current_orientation = Some((*dm).dm_orientation);
                }
                GlobalUnlock(res.h_dev_mode);
            }
        }
    }

    let title_utf16: Vec<u16> = doc_title.encode_utf16().chain(std::iter::once(0)).collect();
    let doc_info = DocInfoW {
        cb_size: std::mem::size_of::<DocInfoW>() as i32,
        lpsz_doc_name: title_utf16.as_ptr(),
        lpsz_output: std::ptr::null(),
        lpsz_datatype: std::ptr::null(),
        fw_type: 0,
    };
    if unsafe { StartDocW(hdc, &doc_info) } <= 0 {
        anyhow::bail!("StartDocW failed (job cancelled or printer error)");
    }

    let mut strip_buf: Vec<u32> = Vec::new();
    let mut print_res: Result<()> = Ok(());

    'print_loop: for _copy in 0..loop_copies {
        for page_1based in start_1based..=end_1based {
            let page_idx = page_1based - 1;

            // Paper orientation follows the page; ResetDC only between pages and on change.
            if let Ok(sz) = doc.pages().page_size(page_idx) {
                let want = if sz.width().value > sz.height().value {
                    DMORIENT_LANDSCAPE
                } else {
                    DMORIENT_PORTRAIT
                };
                if current_orientation != Some(want) && !res.h_dev_mode.is_null() {
                    unsafe {
                        let dm = GlobalLock(res.h_dev_mode) as *mut DevModeWPrefix;
                        if !dm.is_null() {
                            (*dm).dm_fields |= DM_ORIENTATION;
                            (*dm).dm_orientation = want;
                            ResetDCW(hdc, dm as *const c_void);
                            GlobalUnlock(res.h_dev_mode);
                            current_orientation = Some(want);
                        }
                    }
                }
            }

            let dpi_x = unsafe { GetDeviceCaps(hdc, LOGPIXELSX) }.max(72) as u32;
            let dpi_y = unsafe { GetDeviceCaps(hdc, LOGPIXELSY) }.max(72) as u32;
            let horz_res = unsafe { GetDeviceCaps(hdc, HORZRES) }.max(1) as u32;
            let vert_res = unsafe { GetDeviceCaps(hdc, VERTRES) }.max(1) as u32;

            if unsafe { StartPage(hdc) } <= 0 {
                print_res = Err(anyhow::anyhow!("GDI StartPage failed on page {page_1based}"));
                break 'print_loop;
            }

            let page_res = render_page_in_horizontal_strips(
                engine,
                doc,
                page_idx,
                dpi_x,
                dpi_y,
                horz_res,
                vert_res,
                true,
                PRINT_STRIP_ROWS,
                &mut strip_buf,
                |dest_x, dest_y, strip_w, strip_h, strip_pixels| {
                    let bmi = BitmapInfo {
                        bmi_header: BitmapInfoHeader {
                            bi_size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                            bi_width: strip_w as i32,
                            // Negative height: top-down rows, as PDFium renders them.
                            bi_height: -(strip_h as i32),
                            bi_planes: 1,
                            bi_bit_count: 32,
                            bi_compression: BI_RGB,
                            bi_size_image: strip_w * strip_h * 4,
                            bi_x_pels_per_meter: 0,
                            bi_y_pels_per_meter: 0,
                            bi_clr_used: 0,
                            bi_clr_important: 0,
                        },
                        bmi_colors: [0],
                    };
                    let lines = unsafe {
                        StretchDIBits(
                            hdc,
                            dest_x as i32,
                            dest_y as i32,
                            strip_w as i32,
                            strip_h as i32,
                            0,
                            0,
                            strip_w as i32,
                            strip_h as i32,
                            strip_pixels.as_ptr() as *const c_void,
                            &bmi,
                            DIB_RGB_COLORS,
                            SRCCOPY,
                        )
                    };
                    if lines == 0 || lines == GDI_ERROR {
                        anyhow::bail!("StretchDIBits failed on page {page_1based}");
                    }
                    Ok(())
                },
            );

            let end_ok = unsafe { EndPage(hdc) } > 0;
            if let Err(e) = page_res {
                print_res = Err(e);
                break 'print_loop;
            }
            if !end_ok {
                print_res = Err(anyhow::anyhow!("GDI EndPage failed on page {page_1based}"));
                break 'print_loop;
            }
        }
    }

    unsafe {
        if print_res.is_ok() {
            EndDoc(hdc);
        } else {
            AbortDoc(hdc);
        }
    }
    drop(res);
    print_res?;

    let pages_printed = (end_1based - start_1based + 1) as usize;
    Ok(Some((pages_printed, loop_copies.max(driver_copies))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    /// 4 pages: portrait A4, landscape A4, portrait A4 with /Rotate 90, and portrait A4 with a
    /// filled-in text form field (widget with an appearance stream). Each content page has a
    /// red box near the top-left and a blue box near the bottom-right, plus text.
    fn build_print_test_pdf() -> Result<Vec<u8>> {
        let mut ldoc = lopdf::Document::with_version("1.5");
        let pages_id = ldoc.new_object_id();
        let font_id = ldoc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        });
        let resources_id =
            ldoc.add_object(dictionary! { "Font" => dictionary! { "F1" => font_id } });
        let content = |w: i64, h: i64, label: &str| {
            format!(
                "1 0 0 rg 40 {} 120 60 re f 0 0 1 rg {} 40 120 60 re f \
                 0 g BT /F1 28 Tf 150 {} Td ({label}) Tj ET",
                h - 100,
                w - 160,
                h / 2
            )
        };
        let mut kids = Vec::new();
        for (w, h, rotate, label) in [
            (595i64, 842i64, 0i64, "Portrait"),
            (842, 595, 0, "Landscape"),
            (595, 842, 90, "Rotated"),
        ] {
            let c = ldoc.add_object(lopdf::Stream::new(
                dictionary! {},
                content(w, h, label).into_bytes(),
            ));
            kids.push(ldoc.add_object(dictionary! {
                "Type" => "Page", "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), w.into(), h.into()],
                "Rotate" => rotate, "Resources" => resources_id, "Contents" => c,
            }));
        }

        // Page 4: a text field whose appearance is a solid black box at (100,600)-(400,650).
        let ap = ldoc.add_object(lopdf::Stream::new(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Form",
                "BBox" => vec![0.into(), 0.into(), 300.into(), 50.into()],
            },
            b"0 g 0 0 300 50 re f".to_vec(),
        ));
        let page4_id = ldoc.new_object_id();
        let widget = ldoc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx",
            "T" => lopdf::Object::string_literal("name"),
            "V" => lopdf::Object::string_literal("filled"),
            "F" => 4, "P" => page4_id,
            "Rect" => vec![100.into(), 600.into(), 400.into(), 650.into()],
            "AP" => dictionary! { "N" => ap },
        });
        let c4 = ldoc.add_object(lopdf::Stream::new(dictionary! {}, b"".to_vec()));
        ldoc.objects.insert(
            page4_id,
            lopdf::Object::Dictionary(dictionary! {
                "Type" => "Page", "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
                "Resources" => resources_id, "Contents" => c4,
                "Annots" => vec![widget.into()],
            }),
        );
        kids.push(page4_id);

        let count = kids.len() as i64;
        ldoc.objects.insert(
            pages_id,
            lopdf::Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => kids.into_iter().map(lopdf::Object::from).collect::<Vec<_>>(),
                "Count" => count,
            }),
        );
        let acroform = ldoc.add_object(dictionary! { "Fields" => vec![widget.into()] });
        let catalog = ldoc.add_object(dictionary! {
            "Type" => "Catalog", "Pages" => pages_id, "AcroForm" => acroform,
        });
        ldoc.trailer.set("Root", catalog);
        let mut out = Vec::new();
        ldoc.save_to(&mut out)?;
        Ok(out)
    }

    /// Renders `page_idx` through the strip renderer into one canvas of the placement size.
    fn render_strips_to_canvas(
        engine: &PdfEngine,
        doc: &PdfDocument,
        page_idx: u16,
        dpi: u32,
        avail: (u32, u32),
        strip_rows: u32,
    ) -> Result<(PrintPlacement, Vec<u32>, Vec<u32>)> {
        let mut canvas = vec![0x00AB_CDEFu32; (avail.0 * avail.1) as usize];
        let mut strip_heights = Vec::new();
        let mut buf = Vec::new();
        let placement = render_page_in_horizontal_strips(
            engine,
            doc,
            page_idx,
            dpi,
            dpi,
            avail.0,
            avail.1,
            true,
            strip_rows,
            &mut buf,
            |x, y, w, h, px| {
                strip_heights.push(h);
                for row in 0..h {
                    let dst = ((y + row) * avail.0 + x) as usize;
                    let src = (row * w) as usize;
                    canvas[dst..dst + w as usize].copy_from_slice(&px[src..src + w as usize]);
                }
                Ok(())
            },
        )?;
        Ok((placement, canvas, strip_heights))
    }

    fn rgb(px: u32) -> (u8, u8, u8) {
        ((px >> 16) as u8, (px >> 8) as u8, px as u8)
    }

    #[test]
    fn page_selection_parsing() -> Result<()> {
        assert_eq!(parse_page_selection("", 3)?, vec![0, 1, 2]);
        assert_eq!(parse_page_selection("  ALL ", 3)?, vec![0, 1, 2]);
        assert_eq!(parse_page_selection("2", 5)?, vec![1]);
        assert_eq!(parse_page_selection("1-3", 5)?, vec![0, 1, 2]);
        assert_eq!(parse_page_selection("1-3,5", 5)?, vec![0, 1, 2, 4]);
        assert_eq!(parse_page_selection(" 4 - 5 , 1 ", 5)?, vec![0, 3, 4]);
        assert_eq!(parse_page_selection("3-", 5)?, vec![2, 3, 4]);
        assert_eq!(parse_page_selection("2-3,3,1-2", 5)?, vec![0, 1, 2], "sorted, no duplicates");
        assert_eq!(parse_page_selection("1,,2,", 5)?, vec![0, 1], "empty items ignored");
        assert_eq!(parse_page_selection("5-5", 5)?, vec![4]);

        for bad in ["0", "6", "1-6", "3-1", "x", "1-x", "-", ",", "1.5", "-3"] {
            assert!(parse_page_selection(bad, 5).is_err(), "'{bad}' must be rejected");
        }
        let err = parse_page_selection("7", 5).unwrap_err().to_string();
        assert!(err.contains("outside 1-5"), "{err}");
        assert!(parse_page_selection("1", 0).is_err());
        Ok(())
    }

    #[test]
    fn lpstat_parsing_picks_default_printer() {
        let out = "printer Office_Laser is idle.  enabled since Mon\n\
                   printer HP_Color is idle.  enabled since Tue\n\
                   system default destination: HP_Color\n";
        let (printers, sel) = parse_lpstat_output(out);
        assert_eq!(printers, vec!["Office_Laser", "HP_Color"]);
        assert_eq!(sel, 1);
        assert_eq!(parse_lpstat_output("no system default destination\n"), (vec![], 0));
    }

    #[test]
    fn strip_bands_cover_the_page_in_bounded_strips() {
        // A4 height at 600 dpi.
        let bands: Vec<_> = strip_bands(7016, PRINT_STRIP_ROWS).collect();
        assert_eq!(bands.len(), 28);
        assert!(bands.iter().all(|&(_, h)| h >= 1 && h <= PRINT_STRIP_ROWS));
        let mut next_y = 0;
        for &(y, h) in &bands {
            assert_eq!(y, next_y, "strips must be contiguous");
            next_y += h;
        }
        assert_eq!(next_y, 7016, "strips must cover every row exactly once");
        assert_eq!(bands.last(), Some(&(6912, 104)));

        assert_eq!(strip_bands(256, 256).collect::<Vec<_>>(), vec![(0, 256)]);
        assert_eq!(strip_bands(0, 256).count(), 0);
        assert_eq!(strip_bands(100, 0).collect::<Vec<_>>().len(), 7, "minimum 16 rows");
    }

    #[test]
    fn placement_fits_and_centres() {
        // A4 at 600 dpi on an A4 printer with ~4 mm margins (4800 x 6814 printable).
        let p = place_page_on_printable_area(595.0, 842.0, 600, 600, 4800, 6814, true);
        assert!(p.render_w <= 4800 && p.render_h <= 6814);
        assert!(p.render_w == 4800 || p.render_h == 6814, "fit fills one dimension");
        let aspect = p.render_w as f32 / p.render_h as f32;
        assert!((aspect - 595.0 / 842.0).abs() < 0.002);
        assert_eq!(p.offset_x, (4800 - p.render_w) / 2);
        assert_eq!(p.offset_y, (6814 - p.render_h) / 2);
        // One strip of that page is at most render_w * 256 * 4 bytes = ~4.9 MB, not ~140 MB.
        assert!(p.render_w as usize * PRINT_STRIP_ROWS as usize * 4 < 5 * 1024 * 1024);

        // A5 page on that A4 printable area: actual size without fit, enlarged with fit.
        let actual = place_page_on_printable_area(420.0, 595.0, 600, 600, 4800, 6814, false);
        assert_eq!((actual.render_w, actual.render_h), (3500, 4958));
        assert_eq!((actual.offset_x, actual.offset_y), (650, 928));
        let fit = place_page_on_printable_area(420.0, 595.0, 600, 600, 4800, 6814, true);
        assert!(fit.render_w > actual.render_w);

        // Too large without fit: still shrunk to the printable area.
        let big = place_page_on_printable_area(1190.0, 1684.0, 600, 600, 4800, 6814, false);
        assert!(big.render_w <= 4800 && big.render_h <= 6814);

        // Different x/y resolution (e.g. 600 x 300 dpi) keeps the physical aspect ratio.
        let aniso = place_page_on_printable_area(595.0, 842.0, 600, 300, 4800, 3407, true);
        let phys = (aniso.render_w as f32 / 600.0) / (aniso.render_h as f32 / 300.0);
        assert!((phys - 595.0 / 842.0).abs() < 0.003);
    }

    /// Stitching the strips together must give exactly the picture of rendering the page in
    /// one go, the right way up, for portrait, landscape, /Rotate 90 and form-field pages.
    #[test]
    fn strips_reassemble_to_the_full_page_render() -> Result<()> {
        let engine = PdfEngine::new()?;
        let doc = engine.load_pdf_from_bytes_safe(build_print_test_pdf()?)?;
        assert_eq!(doc.pages().len(), 4);

        for page_idx in 0..4u16 {
            let page = doc.pages().get(page_idx)?;
            let (pw, ph) = (page.width().value, page.height().value);
            drop(page);
            // 144 dpi onto an area of exactly the page size, so the scale is 2.0.
            let avail = ((pw * 2.0).round() as u32, (ph * 2.0).round() as u32);

            let (placement, striped, heights) =
                render_strips_to_canvas(&engine, &doc, page_idx, 144, avail, 64)?;
            assert_eq!((placement.render_w, placement.render_h), avail);
            assert!(heights.len() > 10, "page {} was not split into strips", page_idx + 1);
            assert!(heights.iter().all(|&h| h <= 64));
            assert!(!striped.contains(&0x00AB_CDEF), "every canvas pixel must be painted");

            let (_, whole, one) =
                render_strips_to_canvas(&engine, &doc, page_idx, 144, avail, u32::MAX)?;
            assert_eq!(one.len(), 1);
            let differing = striped.iter().zip(&whole).filter(|(a, b)| a != b).count();
            assert!(
                differing * 1000 <= striped.len(),
                "page {}: {differing} of {} pixels differ between strips and one pass",
                page_idx + 1,
                striped.len()
            );

            let w = avail.0 as usize;
            let at = |x: usize, y: usize| rgb(striped[y * w + x]);
            match page_idx {
                // Portrait/landscape: red box top-left, blue box bottom-right, right way up.
                0 | 1 => {
                    let (r, g, b) = at(2 * 100, 2 * 70);
                    assert!(r > 200 && g < 60 && b < 60, "page {} top-left not red", page_idx + 1);
                    let (r, g, b) = at(w - 2 * 100, avail.1 as usize - 2 * 70);
                    assert!(b > 200 && r < 60 && g < 60, "page {} bottom-right not blue", page_idx + 1);
                }
                // /Rotate 90: displayed landscape; the red box (top-left on the unrotated page)
                // ends up top-right.
                2 => {
                    assert!(avail.0 > avail.1, "rotated page must print landscape");
                    let (r, g, b) = at(w - 2 * 70, 2 * 100);
                    assert!(r > 200 && g < 60 && b < 60, "rotated page: red box not top-right");
                }
                // Form field: its appearance (a black box) must print.
                _ => {
                    let (r, g, b) = at(2 * 250, 2 * (842 - 625));
                    assert!(
                        r < 40 && g < 40 && b < 40,
                        "filled form field missing from the print strips (got {r},{g},{b})"
                    );
                }
            }
        }
        // Printing renders a flattened copy; the open document keeps its live form field.
        let form_page = doc.pages().get(3)?;
        assert_eq!(form_page.annotations().len(), 1);
        assert_eq!(
            form_page.annotations().iter().next().map(|a| a.annotation_type()),
            Some(PdfPageAnnotationType::Widget)
        );
        Ok(())
    }

    /// At 600 dpi the reused strip buffer never grows beyond one strip.
    #[test]
    fn strip_buffer_stays_one_strip_at_600_dpi() -> Result<()> {
        let engine = PdfEngine::new()?;
        let doc = engine.load_pdf_from_bytes_safe(build_print_test_pdf()?)?;
        let mut buf = Vec::new();
        let mut max_h = 0;
        let placement = render_page_in_horizontal_strips(
            &engine, &doc, 0, 600, 600, 4800, 6814, true, PRINT_STRIP_ROWS, &mut buf,
            |_, _, _, h, _| {
                max_h = max_h.max(h);
                Ok(())
            },
        )?;
        assert_eq!(max_h, PRINT_STRIP_ROWS);
        let one_strip = placement.render_w as usize * PRINT_STRIP_ROWS as usize;
        assert!(
            buf.capacity() <= one_strip * 2,
            "strip buffer holds {} px, one strip is {one_strip} px",
            buf.capacity()
        );
        let full_page_bytes = placement.render_w as usize * placement.render_h as usize * 4;
        assert!(buf.capacity() * 4 * 20 < full_page_bytes);
        Ok(())
    }

    fn temp_print_files(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| {
                        p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with(TEMP_PRINT_PREFIX))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn fresh_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes an executable fake `lp` that copies the file it is given to `received.pdf` and
    /// its arguments to `args.txt` in `log_dir`, then exits with `exit_code`.
    #[cfg(unix)]
    fn fake_lp(script_dir: &Path, log_dir: &Path, exit_code: i32) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let script = script_dir.join(format!("fake_lp_{exit_code}"));
        let body = format!(
            "#!/bin/sh\n\
             for a in \"$@\"; do last=\"$a\"; done\n\
             printf '%s\\n' \"$@\" > '{log}/args.txt'\n\
             cp \"$last\" '{log}/received.pdf'\n\
             if [ {exit_code} -ne 0 ]; then echo 'lp: The printer or class does not exist.' >&2; exit {exit_code}; fi\n\
             echo 'request id is Fake-42 (1 file(s))'\n",
            log = log_dir.display()
        );
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    #[cfg(unix)]
    #[test]
    fn cups_print_sends_in_memory_pages_and_deletes_temp_file() -> Result<()> {
        let engine = PdfEngine::new()?;
        let mut doc = engine.load_pdf_from_bytes_safe(build_print_test_pdf()?)?;
        // Unsaved in-memory edit: a 5th page that exists nowhere on disk.
        doc.pages_mut().create_page_at_end(PdfPagePaperSize::from_points(PdfPoints::new(420.0), PdfPoints::new(595.0)))?;

        let temp_dir = fresh_dir("micropdf_print_test_ok_tmp");
        let log_dir = fresh_dir("micropdf_print_test_ok_log");
        let lp = fake_lp(&log_dir, &log_dir, 0);
        let req = CupsPrintRequest {
            printer: "Office_Laser",
            copies: 2,
            range_input: "2,5",
            fit_to_page: true,
            title: "report.pdf",
        };
        let out = print_document_cups_with(&engine, &doc, &req, false, lp.as_os_str(), &temp_dir)?;
        assert_eq!(out.pages, 2);
        assert_eq!(out.copies, 2);
        assert_eq!(out.job_info, "request id is Fake-42 (1 file(s))");

        assert!(temp_print_files(&temp_dir).is_empty(), "temp PDF must be deleted after lp");
        let args = std::fs::read_to_string(log_dir.join("args.txt"))?;
        let args: Vec<&str> = args.lines().collect();
        assert_eq!(
            &args[..8],
            ["-d", "Office_Laser", "-n", "2", "-t", "report.pdf", "-o", "fit-to-page"]
        );
        assert!(args[8].starts_with(temp_dir.to_str().unwrap()), "temp file in the given dir");

        // lp received exactly the selected pages of the in-memory document, edits included.
        let sent = engine.load_pdf_from_bytes_safe(std::fs::read(log_dir.join("received.pdf"))?)?;
        assert_eq!(sent.pages().len(), 2);
        let p1 = sent.pages().get(0)?;
        assert!((p1.width().value - 842.0).abs() < 1.0, "page 2 (landscape) first");
        drop(p1);
        let p2 = sent.pages().get(1)?;
        assert!((p2.width().value - 420.0).abs() < 2.0, "the unsaved A5 page second");
        drop(p2);
        drop(sent);

        // Fit off: no fit-to-page option; all pages.
        let req = CupsPrintRequest { fit_to_page: false, range_input: "", copies: 1, ..req };
        print_document_cups_with(&engine, &doc, &req, false, lp.as_os_str(), &temp_dir)?;
        let args = std::fs::read_to_string(log_dir.join("args.txt"))?;
        assert!(!args.contains("fit-to-page"));
        let sent = engine.load_pdf_from_bytes_safe(std::fs::read(log_dir.join("received.pdf"))?)?;
        assert_eq!(sent.pages().len(), 5);
        drop(sent);
        assert!(temp_print_files(&temp_dir).is_empty());

        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::remove_dir_all(&log_dir);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn cups_print_deletes_temp_file_on_every_failure() -> Result<()> {
        let engine = PdfEngine::new()?;
        let doc = engine.load_pdf_from_bytes_safe(build_print_test_pdf()?)?;
        let temp_dir = fresh_dir("micropdf_print_test_fail_tmp");
        let log_dir = fresh_dir("micropdf_print_test_fail_log");
        let req = CupsPrintRequest {
            printer: "Office_Laser",
            copies: 1,
            range_input: "1-2",
            fit_to_page: true,
            title: "t",
        };

        // lp rejects the job.
        let failing = fake_lp(&log_dir, &log_dir, 1);
        let err = print_document_cups_with(&engine, &doc, &req, false, failing.as_os_str(), &temp_dir)
            .unwrap_err();
        assert!(format!("{err:#}").contains("printer or class does not exist"), "{err:#}");
        assert!(log_dir.join("received.pdf").exists(), "lp did get the file");
        assert!(temp_print_files(&temp_dir).is_empty(), "temp PDF must be deleted after lp failed");

        // lp cannot be started at all.
        let missing = log_dir.join("no_such_lp");
        let err = print_document_cups_with(&engine, &doc, &req, false, missing.as_os_str(), &temp_dir)
            .unwrap_err();
        assert!(format!("{err:#}").contains("Failed to run 'lp'"), "{err:#}");
        assert!(temp_print_files(&temp_dir).is_empty(), "temp PDF must be deleted if lp is missing");

        // Bad range or no printer: rejected before anything is written.
        let ok = fake_lp(&log_dir, &log_dir, 0);
        let bad = CupsPrintRequest { range_input: "9", ..req };
        assert!(print_document_cups_with(&engine, &doc, &bad, false, ok.as_os_str(), &temp_dir).is_err());
        let none = CupsPrintRequest { printer: " ", ..req };
        assert!(print_document_cups_with(&engine, &doc, &none, false, ok.as_os_str(), &temp_dir).is_err());
        assert!(std::fs::read_dir(&temp_dir)?.next().is_none(), "temp dir must stay empty");

        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::remove_dir_all(&log_dir);
        Ok(())
    }

    #[test]
    fn temp_print_file_is_removed_on_drop_and_written_after_redaction() -> Result<()> {
        let engine = PdfEngine::new()?;
        let doc = engine.load_pdf_from_bytes_safe(build_print_test_pdf()?)?;
        let dir = fresh_dir("micropdf_print_test_drop");

        for redacted in [false, true] {
            let temp = write_pages_to_temp_pdf(&engine, &doc, &[0, 3], redacted, &dir)?;
            let path = temp.path().to_path_buf();
            let bytes = std::fs::read(&path)?;
            assert!(bytes.starts_with(b"%PDF-"));
            let check = engine.load_pdf_from_bytes_safe(bytes)?;
            assert_eq!(check.pages().len(), 2, "redacted={redacted}");
            drop(check);
            drop(temp);
            assert!(!path.exists(), "dropping the guard must delete the file");
        }

        assert!(write_pages_to_temp_pdf(&engine, &doc, &[], false, &dir).is_err());
        assert!(write_pages_to_temp_pdf(&engine, &doc, &[4], false, &dir).is_err());
        assert!(temp_print_files(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn stale_print_temp_files_are_swept() -> Result<()> {
        let dir = fresh_dir("micropdf_print_test_sweep");
        let two_hours_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
        let stale = dir.join(format!("{TEMP_PRINT_PREFIX}1_1.pdf"));
        let fresh = dir.join(format!("{TEMP_PRINT_PREFIX}2_2.pdf"));
        let unrelated = dir.join("report.pdf");
        for p in [&stale, &fresh, &unrelated] {
            std::fs::write(p, b"%PDF-1.4")?;
        }
        for p in [&stale, &unrelated] {
            std::fs::File::options().write(true).open(p)?.set_modified(two_hours_ago)?;
        }
        remove_stale_print_temp_files(&dir);
        assert!(!stale.exists(), "old print temp file must be removed");
        assert!(fresh.exists(), "a recent one may still be in use by lp");
        assert!(unrelated.exists(), "other files are never touched");
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}

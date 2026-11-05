use anyhow::{Context, Result};
use pdfium_render::prelude::*;
use std::path::{Path, PathBuf};

/// Margin zone to clear when replacing or deleting existing headers, footers, or page numbers.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MarginZone {
    /// Top 55 points of the page.
    Header,
    /// Bottom 55 points of the page (full width).
    Footer,
    /// Bottom-center 55 points of the page.
    PageNumber,
}

/// Horizontal alignment for headers and footers.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MarginPosition {
    HeaderLeft,
    HeaderCenter,
    HeaderRight,
    FooterLeft,
    FooterCenter,
    FooterRight,
}

impl MarginPosition {
    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_uppercase().as_str() {
            "HL" => Some(Self::HeaderLeft),
            "HC" | "H" => Some(Self::HeaderCenter),
            "HR" => Some(Self::HeaderRight),
            "FL" => Some(Self::FooterLeft),
            "FC" | "F" => Some(Self::FooterCenter),
            "FR" => Some(Self::FooterRight),
            _ => None,
        }
    }

    pub fn is_header(self) -> bool {
        matches!(
            self,
            Self::HeaderLeft | Self::HeaderCenter | Self::HeaderRight
        )
    }

    pub fn zone(self) -> MarginZone {
        if self.is_header() {
            MarginZone::Header
        } else {
            MarginZone::Footer
        }
    }
}

/// In-memory Bookmark item for the Left Bookmark Sidebar (`B` key).
#[derive(Clone, Debug, PartialEq)]
pub struct Bookmark {
    pub title: String,
    pub page_index: usize,
}

pub struct PdfEngine {
    pub pdfium: Pdfium,
}

#[allow(dead_code)]
impl PdfEngine {
    /// Initializes PDFium by looking for the library in the executable's directory first,
    /// falling back to `./` (current working directory), and then system library paths.
    pub fn new() -> Result<Self> {
        let bindings = Self::bind_pdfium()?;
        Ok(Self {
            pdfium: Pdfium::new(bindings),
        })
    }

    fn bind_pdfium() -> Result<Box<dyn PdfiumLibraryBindings>> {
        // 1. Try executable's directory first
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                if let Ok(bindings) =
                    Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(exe_dir))
                {
                    return Ok(bindings);
                }
            }
        }

        // 2. Fallback to current working directory `./`
        if let Ok(bindings) =
            Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path("./"))
        {
            return Ok(bindings);
        }

        // Also check `./lib` or `./bin` just in case, before system library
        if let Ok(bindings) =
            Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path("./lib"))
        {
            return Ok(bindings);
        }

        // 3. Fallback to system library paths
        Pdfium::bind_to_system_library().context(
            "Failed to bind to PDFium library in executable dir, current working dir, or system paths",
        )
    }

    /// Unified, safe PDF loader that NEVER passes file paths to PDFium's C loader (`load_pdf_from_file`).
    /// 1. Cleans/canonicalizes `path` (stripping any trailing newlines or surrounding quotes).
    /// 2. Reads the entire file into memory via `std::fs::read`.
    /// 3. Calls `self.pdfium.load_pdf_from_byte_vec(bytes.clone(), None)`.
    /// 4. If that fails, immediately retries with `Some("")` (empty password).
    /// 5. If it still fails, logs the exact `PdfiumError` variant and file path to `eprintln!`
    ///    and returns it in `Err` so the Window Title Bar displays `FS Error` vs `Pdfium Error`.
    pub fn load_pdf_safe<'a>(&'a self, path: &Path) -> Result<PdfDocument<'a>> {
        let cleaned_path = if let Some(raw_str) = path.to_str() {
            let trimmed = raw_str
                .trim_matches(|c: char| c == '\n' || c == '\r' || c == '"' || c == '\'');
            PathBuf::from(trimmed)
        } else {
            path.to_path_buf()
        };

        let resolved_path = std::fs::canonicalize(&cleaned_path).unwrap_or(cleaned_path);

        let bytes = match std::fs::read(&resolved_path) {
            Ok(b) => b,
            Err(fs_err) => {
                eprintln!(
                    "[load_pdf_safe] FS Error reading {:?}: {}",
                    resolved_path, fs_err
                );
                anyhow::bail!("FS Error reading {:?}: {}", resolved_path, fs_err);
            }
        };

        if bytes.is_empty() {
            eprintln!("[load_pdf_safe] FS Error: file {:?} is empty (0 bytes)", resolved_path);
            anyhow::bail!("FS Error: file {:?} is empty (0 bytes)", resolved_path);
        }

        match self.pdfium.load_pdf_from_byte_vec(bytes.clone(), None) {
            Ok(doc) => return Ok(doc),
            Err(err_none) => {
                match self.pdfium.load_pdf_from_byte_vec(bytes.clone(), Some("")) {
                    Ok(doc) => return Ok(doc),
                    Err(err_empty) => {
                        // Also check if the file has leading junk/BOM bytes before `%PDF-`
                        if let Some(pdf_offset) = bytes
                            .windows(5)
                            .position(|w| w == b"%PDF-")
                            .filter(|&pos| pos > 0)
                        {
                            let sliced = bytes[pdf_offset..].to_vec();
                            if let Ok(doc) = self.pdfium.load_pdf_from_byte_vec(sliced.clone(), None)
                            {
                                return Ok(doc);
                            }
                            if let Ok(doc) = self.pdfium.load_pdf_from_byte_vec(sliced, Some("")) {
                                return Ok(doc);
                            }
                        }

                        eprintln!(
                            "[load_pdf_safe] Pdfium Error opening {:?} ({} bytes): initial={:?}, empty_password_retry={:?}",
                            resolved_path,
                            bytes.len(),
                            err_none,
                            err_empty
                        );
                        anyhow::bail!(
                            "Pdfium Error ({:?} / {:?}) on {:?}",
                            err_none,
                            err_empty,
                            resolved_path
                        );
                    }
                }
            }
        }
    }

    /// Alias for `load_pdf_safe` so all callers use the safe in-memory byte vector loader.
    pub fn load_pdf_file<'a>(&'a self, path: &Path) -> Result<PdfDocument<'a>> {
        self.load_pdf_safe(path)
    }

    /// Safely saves a `PdfDocument` to disk by buffering into memory first so that
    /// in-place saves (`input_path == output_path`) never truncate a file currently mapped by PDFium.
    pub fn save_doc<P: AsRef<Path>>(doc: &PdfDocument, output_path: P) -> Result<()> {
        let bytes = doc
            .save_to_bytes()
            .context("Failed to serialize PDF document to bytes")?;
        std::fs::write(output_path.as_ref(), bytes)
            .with_context(|| format!("Failed to write PDF to {:?}", output_path.as_ref()))?;
        Ok(())
    }

    /// Sidecar file path (`<pdf_filename>.bookmarks.txt`) used to persist user bookmark edits.
    pub fn bookmarks_sidecar_path(pdf_path: &Path) -> PathBuf {
        let mut s = pdf_path.as_os_str().to_os_string();
        s.push(".bookmarks.txt");
        PathBuf::from(s)
    }

    /// Loads bookmarks for `doc`, checking `<pdf_filename>.bookmarks.txt` first so user edits
    /// are preserved, and falling back to `doc.bookmarks().iter()` from the PDF outline tree.
    pub fn load_bookmarks(doc: &PdfDocument, pdf_path: &Path) -> Vec<Bookmark> {
        let sidecar = Self::bookmarks_sidecar_path(pdf_path);
        let total_pages = doc.pages().len() as usize;

        if let Ok(content) = std::fs::read_to_string(&sidecar) {
            let mut list = Vec::new();
            for line in content.lines() {
                if let Some((idx_str, title)) = line.split_once('\t') {
                    if let Ok(page_idx) = idx_str.trim().parse::<usize>() {
                        if total_pages == 0 || page_idx < total_pages {
                            list.push(Bookmark {
                                title: title.trim().to_string(),
                                page_index: page_idx,
                            });
                        }
                    }
                }
            }
            return list;
        }

        let mut list = Vec::new();
        for bm in doc.bookmarks().iter() {
            let title = bm
                .title()
                .unwrap_or_else(|| "Untitled".to_string())
                .trim()
                .to_string();
            let page_opt = bm
                .destination()
                .and_then(|d| d.page_index().ok())
                .or_else(|| {
                    let act = bm.action()?;
                    let ld = act.as_local_destination_action()?;
                    let dest = ld.destination().ok()?;
                    dest.page_index().ok()
                });
            if let Some(page_idx) = page_opt {
                let idx = page_idx as usize;
                if total_pages == 0 || idx < total_pages {
                    list.push(Bookmark {
                        title: if title.is_empty() {
                            format!("Page {}", idx + 1)
                        } else {
                            title
                        },
                        page_index: idx,
                    });
                }
            }
        }
        list
    }

    /// Saves bookmarks to `<pdf_filename>.bookmarks.txt` so user edits are never lost.
    pub fn save_bookmarks_sidecar(pdf_path: &Path, bookmarks: &[Bookmark]) -> Result<()> {
        let sidecar = Self::bookmarks_sidecar_path(pdf_path);
        if bookmarks.is_empty() {
            if sidecar.exists() {
                let _ = std::fs::remove_file(&sidecar);
            }
            return Ok(());
        }
        let mut out = String::new();
        for bm in bookmarks {
            let clean_title: String = bm
                .title
                .chars()
                .map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c })
                .collect();
            out.push_str(&format!("{}\t{}\n", bm.page_index, clean_title.trim()));
        }
        std::fs::write(&sidecar, out)
            .with_context(|| format!("Failed to write bookmarks sidecar {:?}", sidecar))?;
        Ok(())
    }

    /// Clears existing headers, footers, or page numbers in the specified `MarginZone` on `page`:
    /// 1. Iterates over `page.objects_mut()` and removes any text/path objects whose bounding box
    ///    lies completely within that margin band.
    /// 2. Places an opaque white rectangle (`PdfPagePathObject::new_rect` filled with white `255, 255, 255`)
    ///    over that margin band so even flattened/scanned existing headers, footers, or page numbers are cleanly erased.
    pub fn clear_margin_zone(page: &mut PdfPage, zone: MarginZone) -> Result<usize> {
        let page_w = page.width().value;
        let page_h = page.height().value;
        let band_height = 55.0_f32;

        let (zone_bottom, zone_left, zone_top, zone_right) = match zone {
            MarginZone::Header => ((page_h - band_height).max(0.0), 0.0, page_h, page_w),
            MarginZone::Footer => (0.0, 0.0, band_height.min(page_h), page_w),
            MarginZone::PageNumber => {
                // Bottom-center 55pt band (centered horizontally across the middle 40% of the page)
                let center_x = page_w * 0.5;
                let half_span = (page_w * 0.22).max(70.0);
                (
                    0.0,
                    (center_x - half_span).max(0.0),
                    band_height.min(page_h),
                    (center_x + half_span).min(page_w),
                )
            }
        };

        // 1. Remove any text or path objects whose bounding box lies completely within the margin band
        let mut to_remove = Vec::new();
        let obj_count = page.objects().len();
        for i in 0..obj_count {
            if let Ok(obj) = page.objects().get(i) {
                let is_text_or_path = matches!(
                    obj.object_type(),
                    PdfPageObjectType::Text | PdfPageObjectType::Path
                );
                if is_text_or_path {
                    if let Ok(b) = obj.bounds() {
                        let b_bottom = b.bottom().value.min(b.top().value);
                        let b_top = b.bottom().value.max(b.top().value);
                        let b_left = b.left().value.min(b.right().value);
                        let b_right = b.left().value.max(b.right().value);

                        let eps = 2.0_f32;
                        let inside_band = b_bottom >= (zone_bottom - eps)
                            && b_top <= (zone_top + eps)
                            && b_left >= (zone_left - eps)
                            && b_right <= (zone_right + eps);
                        if inside_band {
                            to_remove.push(i);
                        }
                    }
                }
            }
        }

        let prev_strategy = page.content_regeneration_strategy();
        page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);

        let mut removed_count = 0;
        for &idx in to_remove.iter().rev() {
            if let Ok(obj) = page.objects_mut().remove_object_at_index(idx) {
                // Note: In PDFium, calling FPDFPageObj_Destroy after FPDFPage_RemoveObject +
                // FPDFPage_GenerateContent on reloaded page content stream objects can trigger
                // a double-free/segfault inside PDFium's CPDF_PageContentGenerator.
                std::mem::forget(obj);
                removed_count += 1;
            }
        }

        // 2. Place an opaque white rectangle over the margin band so flattened/scanned text is also cleanly erased
        let white_rect = PdfRect::new_from_values(zone_bottom, zone_left, zone_top, zone_right);
        let rect_res = page
            .objects_mut()
            .create_path_object_rect(white_rect, None, None, Some(PdfColor::WHITE))
            .context("Failed to create whiteout margin rectangle");

        let regen_res = page.regenerate_content();
        page.set_content_regeneration_strategy(prev_strategy);

        rect_res?;
        regen_res.context("Failed to regenerate page content after clearing margin zone")?;

        Ok(removed_count)
    }

    /// Clears the specified `MarginZone` across a 1-based inclusive page range `start_page..=end_page`.
    pub fn clear_margin_zone_in_doc(
        doc: &mut PdfDocument,
        zone: MarginZone,
        start_page: u16,
        end_page: u16,
    ) -> Result<()> {
        let total = doc.pages().len();
        if total == 0 {
            return Ok(());
        }
        let start_idx = start_page.saturating_sub(1);
        let end_idx = end_page.min(total).saturating_sub(1);
        if start_idx > end_idx {
            return Ok(());
        }
        for idx in start_idx..=end_idx {
            let mut page = doc.pages_mut().get(idx)?;
            Self::clear_margin_zone(&mut page, zone)?;
        }
        Ok(())
    }

    /// Generate a sample multi-page test PDF (`create_sample_pdf(output_path, num_pages)`)
    pub fn create_sample_pdf<P: AsRef<Path>>(&self, output_path: P, num_pages: u16) -> Result<()> {
        let mut doc = self
            .pdfium
            .create_new_pdf()
            .context("Failed to create new PDF document")?;

        let font = doc.fonts_mut().helvetica();
        let font_bold = doc.fonts_mut().helvetica_bold();

        for i in 1..=num_pages {
            let mut page = doc
                .pages_mut()
                .create_page_at_end(PdfPagePaperSize::a4())
                .with_context(|| format!("Failed to create page {i}"))?;

            let mut title = PdfPageTextObject::new(
                &doc,
                format!("Sample Document - Page {i} of {num_pages}"),
                font_bold,
                PdfPoints::new(24.0),
            )?;
            title.set_fill_color(PdfColor::new(30, 60, 120, 255))?;
            title.translate(PdfPoints::new(72.0), PdfPoints::new(720.0))?;
            page.objects_mut().add_text_object(title)?;

            let lines = [
                format!(
                    "This is auto-generated content for page {i}. Built with MicroPDF & PDFium."
                ),
                "Drag your mouse across any text on this page to highlight it interactively!".to_string(),
                "Press 'B' to toggle the Left Bookmark Sidebar, or 'Tab' for Thumbnail Grid View.".to_string(),
                "Use 'N' for Roman/Arabic page numbering, 'H' for aligned headers/footers, 'C' to clear margins.".to_string(),
                "Use 'W' for watermarks, 'I' to insert a page, 'X' to split, and 'M' or drag-drop to merge.".to_string(),
            ];

            for (line_idx, text_line) in lines.iter().enumerate() {
                let mut body = PdfPageTextObject::new(&doc, text_line, font, PdfPoints::new(12.0))?;
                body.set_fill_color(PdfColor::new(50, 50, 50, 255))?;
                let y = 680.0 - (line_idx as f32 * 24.0);
                body.translate(PdfPoints::new(72.0), PdfPoints::new(y))?;
                page.objects_mut().add_text_object(body)?;
            }
        }

        Self::save_doc(&doc, output_path).context("Failed to save sample PDF")
    }

    /// Highlights text in `pdf_rect` on `page_index` (0-based) of `doc`.
    pub fn highlight_text_in_rect(
        doc: &mut PdfDocument,
        page_index: u16,
        pdf_rect: PdfRect,
    ) -> Result<usize> {
        let sel_bottom = pdf_rect.bottom().value.min(pdf_rect.top().value);
        let sel_top = pdf_rect.bottom().value.max(pdf_rect.top().value);
        let sel_left = pdf_rect.left().value.min(pdf_rect.right().value);
        let sel_right = pdf_rect.left().value.max(pdf_rect.right().value);

        if (sel_right - sel_left) < 2.0 && (sel_top - sel_bottom) < 2.0 {
            return Ok(0);
        }

        let normalized_sel = PdfRect::new_from_values(sel_bottom, sel_left, sel_top, sel_right);

        let mut target_rects: Vec<PdfRect> = Vec::new();
        {
            let page = doc
                .pages()
                .get(page_index)
                .context("Page index out of bounds for highlighting")?;

            if let Ok(text_page) = page.text() {
                let mut char_boxes: Vec<PdfRect> = Vec::new();
                for ch in text_page.chars().iter() {
                    if let Some(uc) = ch.unicode_char() {
                        if uc.is_control() {
                            continue;
                        }
                    }
                    let bounds = ch.loose_bounds().or_else(|_| ch.tight_bounds());
                    if let Ok(b) = bounds {
                        let b_bottom = b.bottom().value.min(b.top().value);
                        let b_top = b.bottom().value.max(b.top().value);
                        let b_left = b.left().value.min(b.right().value);
                        let b_right = b.left().value.max(b.right().value);

                        if (b_right - b_left) < 0.1 || (b_top - b_bottom) < 0.1 {
                            continue;
                        }

                        let overlaps = b_left <= sel_right
                            && b_right >= sel_left
                            && b_bottom <= sel_top
                            && b_top >= sel_bottom;
                        if overlaps {
                            char_boxes.push(PdfRect::new_from_values(
                                b_bottom, b_left, b_top, b_right,
                            ));
                        }
                    }
                }

                for cb in char_boxes {
                    if let Some(last) = target_rects.last_mut() {
                        let last_mid_y = (last.bottom().value + last.top().value) * 0.5;
                        let cb_mid_y = (cb.bottom().value + cb.top().value) * 0.5;
                        let line_height = (last.top().value - last.bottom().value).max(4.0);

                        if (last_mid_y - cb_mid_y).abs() < line_height * 0.65 {
                            let merged_bottom = last.bottom().value.min(cb.bottom().value);
                            let merged_left = last.left().value.min(cb.left().value);
                            let merged_top = last.top().value.max(cb.top().value);
                            let merged_right = last.right().value.max(cb.right().value);
                            *last = PdfRect::new_from_values(
                                merged_bottom,
                                merged_left,
                                merged_top,
                                merged_right,
                            );
                            continue;
                        }
                    }
                    target_rects.push(cb);
                }

                if target_rects.is_empty() {
                    for seg in text_page.segments().iter() {
                        let b = seg.bounds();
                        if b.does_overlap(&normalized_sel) {
                            let clip_left = b.left().value.max(sel_left);
                            let clip_right = b.right().value.min(sel_right);
                            if clip_right > clip_left {
                                target_rects.push(PdfRect::new_from_values(
                                    b.bottom().value,
                                    clip_left,
                                    b.top().value,
                                    clip_right,
                                ));
                            }
                        }
                    }
                }
            }
        }

        if target_rects.is_empty() {
            target_rects.push(normalized_sel);
        }

        let highlight_fill = PdfColor::new(255, 235, 59, 115);

        let mut path_objects = Vec::with_capacity(target_rects.len());
        for rect in &target_rects {
            let padded = PdfRect::new_from_values(
                rect.bottom().value - 1.0,
                rect.left().value - 1.0,
                rect.top().value + 1.0,
                rect.right().value + 1.0,
            );
            let mut path_obj =
                PdfPagePathObject::new_rect(doc, padded, None, None, Some(highlight_fill))?;
            let _ = path_obj.set_blend_mode(PdfPageObjectBlendMode::Multiply);
            path_objects.push(path_obj);
        }

        let mut page = doc
            .pages_mut()
            .get(page_index)
            .context("Page index out of bounds for mutable highlight")?;

        if let Ok(mut annot) = page.annotations_mut().create_highlight_annotation() {
            let _ = annot.set_stroke_color(PdfColor::new(255, 235, 59, 160));
            let _ = annot.set_bounds(normalized_sel);
            for rect in &target_rects {
                let _ = annot
                    .attachment_points_mut()
                    .create_attachment_point_at_end(rect.to_quad_points());
            }
        }

        for path_obj in path_objects {
            page.objects_mut().add_path_object(path_obj)?;
        }

        Ok(target_rects.len())
    }

    /// In-memory page numbering on a `PdfDocument`: automatically clears existing `MarginZone::PageNumber`
    /// first so old page numbers are replaced cleanly instead of overlapping, then stamps Roman numerals
    /// (`i, ii, iii...`) on `1..=roman_end` and Arabic numerals (`1, 2, 3...`) after that at bottom-center.
    pub fn apply_page_numbers_to_doc(doc: &mut PdfDocument, roman_end: u16) -> Result<()> {
        let font = doc.fonts_mut().helvetica();
        let total_pages = doc.pages().len();

        for idx in 0..total_pages {
            // Clear existing page number zone first
            {
                let mut page = doc.pages_mut().get(idx)?;
                Self::clear_margin_zone(&mut page, MarginZone::PageNumber)?;
            }

            let page_num = idx + 1;
            let label = if page_num <= roman_end {
                Self::to_roman(page_num)
            } else {
                format!("{}", page_num - roman_end)
            };

            let page_width = doc.pages().get(idx)?.width();

            let mut text_obj = PdfPageTextObject::new(doc, &label, font, PdfPoints::new(10.0))?;
            text_obj.set_fill_color(PdfColor::new(80, 80, 80, 255))?;

            let text_width = text_obj.width().unwrap_or(PdfPoints::new(15.0));
            let x = (page_width - text_width) / 2.0;
            let y = PdfPoints::new(28.0);

            text_obj.translate(x, y)?;
            let mut page = doc.pages_mut().get(idx)?;
            page.objects_mut().add_text_object(text_obj)?;
        }
        Ok(())
    }

    /// Stamps or clears a header or footer with Left/Center/Right alignment (`MarginPosition`)
    /// across 1-based `start_page..=end_page`. Always calls `clear_margin_zone` first so old
    /// headers/footers are cleanly replaced (or deleted if `text` is empty).
    pub fn apply_aligned_header_footer_to_doc(
        doc: &mut PdfDocument,
        position: MarginPosition,
        text: &str,
        start_page: u16,
        end_page: u16,
    ) -> Result<()> {
        let font = doc.fonts_mut().helvetica();
        let total_pages = doc.pages().len();
        if total_pages == 0 {
            return Ok(());
        }

        let start_idx = start_page.saturating_sub(1);
        let end_idx = end_page.min(total_pages).saturating_sub(1);
        if start_idx > end_idx {
            return Ok(());
        }

        let trimmed = text.trim();
        let margin_pt = 36.0_f32;

        for idx in start_idx..=end_idx {
            // 1. Always clear the target margin zone first
            {
                let mut page = doc.pages_mut().get(idx)?;
                Self::clear_margin_zone(&mut page, position.zone())?;
            }

            if trimmed.is_empty() {
                continue;
            }

            let (page_width, page_height) = {
                let page = doc.pages().get(idx)?;
                (page.width().value, page.height().value)
            };

            let mut obj = PdfPageTextObject::new(doc, trimmed, font, PdfPoints::new(10.0))?;
            obj.set_fill_color(PdfColor::new(90, 90, 90, 255))?;
            let text_width = obj.width().unwrap_or(PdfPoints::new(60.0)).value;

            let x = match position {
                MarginPosition::HeaderLeft | MarginPosition::FooterLeft => margin_pt,
                MarginPosition::HeaderCenter | MarginPosition::FooterCenter => {
                    ((page_width - text_width) * 0.5).max(margin_pt)
                }
                MarginPosition::HeaderRight | MarginPosition::FooterRight => {
                    (page_width - text_width - margin_pt).max(margin_pt)
                }
            };

            let y = if position.is_header() {
                (page_height - 34.0).max(0.0)
            } else {
                26.0
            };

            obj.translate(PdfPoints::new(x), PdfPoints::new(y))?;
            let mut page = doc.pages_mut().get(idx)?;
            page.objects_mut().add_text_object(obj)?;
        }

        Ok(())
    }

    /// In-memory header/footer stamping on a `PdfDocument` across 1-based `start_page..=end_page`.
    pub fn apply_header_footer_to_doc(
        doc: &mut PdfDocument,
        header_text: Option<&str>,
        footer_text: Option<&str>,
        start_page: u16,
        end_page: u16,
    ) -> Result<()> {
        if let Some(h_text) = header_text {
            Self::apply_aligned_header_footer_to_doc(
                doc,
                MarginPosition::HeaderCenter,
                h_text,
                start_page,
                end_page,
            )?;
        }
        if let Some(f_text) = footer_text {
            Self::apply_aligned_header_footer_to_doc(
                doc,
                MarginPosition::FooterLeft,
                f_text,
                start_page,
                end_page,
            )?;
        }
        Ok(())
    }

    /// In-memory watermark stamping on a `PdfDocument` across all pages.
    pub fn apply_watermark_to_doc(
        doc: &mut PdfDocument,
        watermark_text: &str,
        diagonal: bool,
    ) -> Result<()> {
        let font = doc.fonts_mut().helvetica_bold();
        let total_pages = doc.pages().len();

        for idx in 0..total_pages {
            let (page_width, page_height) = {
                let page = doc.pages().get(idx)?;
                (page.width(), page.height())
            };

            let mut wm_obj =
                PdfPageTextObject::new(doc, watermark_text, font, PdfPoints::new(54.0))?;
            wm_obj.set_fill_color(PdfColor::new(180, 40, 40, 64))?;

            let text_width = wm_obj.width().unwrap_or(PdfPoints::new(250.0));
            let text_height = wm_obj.height().unwrap_or(PdfPoints::new(54.0));

            if diagonal {
                let angle_deg = 45.0_f32;
                let angle_rad = angle_deg.to_radians();
                wm_obj.rotate_counter_clockwise_degrees(angle_deg)?;

                let half_w = text_width.value / 2.0;
                let half_h = text_height.value / 2.0;
                let offset_x = half_w * angle_rad.cos() - half_h * angle_rad.sin();
                let offset_y = half_w * angle_rad.sin() + half_h * angle_rad.cos();

                let cx = page_width.value / 2.0 - offset_x;
                let cy = page_height.value / 2.0 - offset_y;
                wm_obj.translate(PdfPoints::new(cx), PdfPoints::new(cy))?;
            } else {
                let cx = (page_width - text_width) / 2.0;
                let cy = (page_height - text_height) / 2.0;
                wm_obj.translate(cx, cy)?;
            }

            let mut page = doc.pages_mut().get(idx)?;
            page.objects_mut().add_text_object(wm_obj)?;
        }
        Ok(())
    }

    /// Reorders a single page inside `doc` from `from_index` (0-based) to `to_index` (0-based)
    /// using PDFium page import/delete semantics.
    pub fn move_page_in_doc(&self, doc: &mut PdfDocument, from_index: u16, to_index: u16) -> Result<()> {
        let total = doc.pages().len();
        if from_index >= total || to_index >= total || from_index == to_index {
            return Ok(());
        }

        // Copy `from_index` into a temporary 1-page document, delete `from_index`, then import at `to_index`
        let mut temp_doc = self.pdfium.create_new_pdf()?;
        temp_doc
            .pages_mut()
            .copy_page_from_document(doc, from_index, 0)
            .context("Failed to copy page to temporary document for reordering")?;

        doc.pages()
            .get(from_index)
            .context("Source page out of bounds")?
            .delete()
            .context("Failed to delete original page during move")?;

        let dest_idx = to_index.min(doc.pages().len());
        doc.pages_mut()
            .copy_page_from_document(&temp_doc, 0, dest_idx)
            .context("Failed to re-insert moved page at target index")?;

        Ok(())
    }

    /// Imports all pages from an external PDF file (`external_path`) into `doc` right BEFORE
    /// `target_page_index` (0-based). Returns the number of imported pages.
    pub fn insert_external_pdf_at_index(
        &self,
        doc: &mut PdfDocument,
        external_path: &Path,
        target_page_index: u16,
    ) -> Result<u16> {
        let ext_doc = self.load_pdf_safe(external_path)?;
        let ext_count = ext_doc.pages().len();
        if ext_count == 0 {
            return Ok(0);
        }
        let dest_idx = target_page_index.min(doc.pages().len());
        if dest_idx >= doc.pages().len() {
            doc.pages_mut()
                .append(&ext_doc)
                .with_context(|| format!("Failed to append pages from {:?}", external_path))?;
        } else {
            doc.pages_mut()
                .copy_page_range_from_document(&ext_doc, 0..=(ext_count - 1), dest_idx)
                .with_context(|| format!("Failed to import pages from {:?}", external_path))?;
        }
        Ok(ext_count)
    }

    /// Extracts a comma-separated 1-based page specification (e.g. `"1-3"` or `"2,4,6"`)
    /// from `src_doc` and saves it to `output_path`.
    pub fn extract_page_spec_from_doc<P: AsRef<Path>>(
        &self,
        src_doc: &PdfDocument,
        page_spec: &str,
        output_path: P,
    ) -> Result<u16> {
        let mut dest_doc = self
            .pdfium
            .create_new_pdf()
            .context("Failed to create destination PDF for split/extraction")?;

        let cleaned_spec: String = page_spec.chars().filter(|c| !c.is_whitespace()).collect();
        dest_doc
            .pages_mut()
            .copy_pages_from_document(src_doc, &cleaned_spec, 0)
            .with_context(|| {
                format!("Invalid or out-of-range page specification '{cleaned_spec}'")
            })?;

        let count = dest_doc.pages().len();
        Self::save_doc(&dest_doc, output_path)?;
        Ok(count)
    }

    /// Inserts a blank A4 page into `doc` at `insert_index` (0-based).
    pub fn insert_blank_page_in_doc(doc: &mut PdfDocument, insert_index: u16) -> Result<u16> {
        let clamped_idx = insert_index.min(doc.pages().len());
        doc.pages_mut()
            .create_page_at_index(PdfPagePaperSize::a4(), clamped_idx)
            .context("Failed to insert blank A4 page")?;
        Ok(clamped_idx)
    }

    /// Rotate specific pages (`rotate_pages`).
    /// `pages` uses 1-based page numbers. `degrees` can be 0, 90, 180, or 270 (clockwise).
    pub fn rotate_pages<P: AsRef<Path>>(
        &self,
        input_path: P,
        output_path: P,
        pages: &[u16],
        degrees: i32,
    ) -> Result<()> {
        let mut doc = self
            .load_pdf_file(input_path.as_ref())
            .context("Failed to load PDF for rotation")?;

        let rotation = match degrees.rem_euclid(360) {
            0 => PdfPageRenderRotation::None,
            90 => PdfPageRenderRotation::Degrees90,
            180 => PdfPageRenderRotation::Degrees180,
            270 => PdfPageRenderRotation::Degrees270,
            _ => anyhow::bail!("Rotation degrees must be a multiple of 90 (got {degrees})"),
        };

        for &page_num in pages {
            let idx = page_num.saturating_sub(1);
            let mut page = doc
                .pages_mut()
                .get(idx)
                .with_context(|| format!("Page {page_num} out of bounds"))?;
            page.set_rotation(rotation);
        }

        Self::save_doc(&doc, output_path).context("Failed to save rotated PDF")
    }

    /// Delete specific pages (`delete_pages`).
    /// `pages` uses 1-based page numbers. Deletes in descending order so indices don't shift.
    pub fn delete_pages<P: AsRef<Path>>(
        &self,
        input_path: P,
        output_path: P,
        pages: &[u16],
    ) -> Result<()> {
        let doc = self
            .load_pdf_file(input_path.as_ref())
            .context("Failed to load PDF for deleting pages")?;

        let mut sorted_pages: Vec<u16> = pages.to_vec();
        sorted_pages.sort_unstable();
        sorted_pages.dedup();

        for &page_num in sorted_pages.iter().rev() {
            let idx = page_num.saturating_sub(1);
            let page = doc
                .pages()
                .get(idx)
                .with_context(|| format!("Page {page_num} out of bounds"))?;
            page.delete()
                .with_context(|| format!("Failed to delete page {page_num}"))?;
        }

        Self::save_doc(&doc, output_path).context("Failed to save PDF after deleting pages")
    }

    /// Merge two PDFs (`merge_pdfs`).
    pub fn merge_pdfs<P: AsRef<Path>>(
        &self,
        first_path: P,
        second_path: P,
        output_path: P,
    ) -> Result<()> {
        let mut first_doc = self
            .load_pdf_file(first_path.as_ref())
            .context("Failed to load first PDF for merge")?;
        let second_doc = self
            .load_pdf_file(second_path.as_ref())
            .context("Failed to load second PDF for merge")?;

        first_doc
            .pages_mut()
            .append(&second_doc)
            .context("Failed to append second PDF into first PDF")?;

        Self::save_doc(&first_doc, output_path).context("Failed to save merged PDF")
    }

    /// Split/extract a page range into a new PDF (`extract_pages`).
    /// `start_page` and `end_page` are 1-based inclusive page numbers.
    pub fn extract_pages<P: AsRef<Path>>(
        &self,
        input_path: P,
        output_path: P,
        start_page: u16,
        end_page: u16,
    ) -> Result<()> {
        let src_doc = self
            .load_pdf_file(input_path.as_ref())
            .context("Failed to load source PDF for extraction")?;
        let mut dest_doc = self
            .pdfium
            .create_new_pdf()
            .context("Failed to create destination PDF for extraction")?;

        let start_idx = start_page.saturating_sub(1);
        let end_idx = end_page.saturating_sub(1);
        dest_doc
            .pages_mut()
            .copy_page_range_from_document(&src_doc, start_idx..=end_idx, 0)
            .with_context(|| format!("Failed to extract page range {start_page}-{end_page}"))?;

        Self::save_doc(&dest_doc, output_path).context("Failed to save extracted PDF")
    }

    /// Stamp Page Numbers (`add_page_numbers`) supporting Roman numerals (e.g., i, ii, iii, iv, v)
    /// for pages `1..=roman_end` and standard Arabic numerals (1, 2, 3...) after that,
    /// positioned at bottom-center.
    pub fn add_page_numbers<P: AsRef<Path>>(
        &self,
        input_path: P,
        output_path: P,
        roman_end: u16,
    ) -> Result<()> {
        let mut doc = self
            .load_pdf_file(input_path.as_ref())
            .context("Failed to load PDF for adding page numbers")?;
        Self::apply_page_numbers_to_doc(&mut doc, roman_end)?;
        Self::save_doc(&doc, output_path).context("Failed to save PDF with page numbers")
    }

    /// Stamp Headers and Footers (`add_header_footer`) for a given 1-based inclusive page range.
    pub fn add_header_footer<P: AsRef<Path>>(
        &self,
        input_path: P,
        output_path: P,
        header_text: Option<&str>,
        footer_text: Option<&str>,
        start_page: u16,
        end_page: u16,
    ) -> Result<()> {
        let mut doc = self
            .load_pdf_file(input_path.as_ref())
            .context("Failed to load PDF for header/footer")?;
        Self::apply_header_footer_to_doc(&mut doc, header_text, footer_text, start_page, end_page)?;
        Self::save_doc(&doc, output_path).context("Failed to save PDF with header/footer")
    }

    /// Stamp a diagonal or centered semi-transparent Text Watermark (`add_watermark`) across pages.
    pub fn add_watermark<P: AsRef<Path>>(
        &self,
        input_path: P,
        output_path: P,
        watermark_text: &str,
        diagonal: bool,
    ) -> Result<()> {
        let mut doc = self
            .load_pdf_file(input_path.as_ref())
            .context("Failed to load PDF for watermark")?;
        Self::apply_watermark_to_doc(&mut doc, watermark_text, diagonal)?;
        Self::save_doc(&doc, output_path).context("Failed to save watermarked PDF")
    }

    /// Compresses and optimizes a PDF byte buffer (`compress_pdf(input_bytes) -> Result<Vec<u8>>`):
    /// - Parses the PDF structure via `lopdf::Document::load_mem`
    /// - Prunes unused/orphaned objects (`prune_objects`)
    /// - Deletes zero-length streams (`delete_zero_length_streams`)
    /// - Renumbers objects compactly (`renumber_objects`)
    /// - Compresses uncompressed streams (`compress`)
    pub fn compress_pdf(input_bytes: &[u8]) -> Result<Vec<u8>> {
        let mut ldoc = lopdf::Document::load_mem(input_bytes)
            .context("Failed to parse PDF structure with lopdf for compression")?;
        ldoc.prune_objects();
        ldoc.delete_zero_length_streams();
        ldoc.renumber_objects();
        ldoc.compress();

        let mut compressed = Vec::with_capacity(input_bytes.len());
        ldoc.save_to(&mut compressed)
            .context("Failed to serialize compressed PDF via lopdf")?;
        Ok(compressed)
    }

    /// Compresses the active `PdfDocument`, combining PDFium clean-document rebuild (stripping
    /// incremental revision history) with `lopdf` object pruning and stream compression.
    /// Returns `(compressed_bytes, original_size_bytes, compressed_size_bytes)`.
    pub fn compress_doc(&self, doc: &PdfDocument) -> Result<(Vec<u8>, usize, usize)> {
        let raw_bytes = doc
            .save_to_bytes()
            .context("Failed to serialize active PDF document")?;
        let orig_len = raw_bytes.len();

        let mut best_bytes = raw_bytes.clone();

        // 1. Try lopdf structural pruning + deflate stream compression on raw_bytes
        if let Ok(lopdf_out) = Self::compress_pdf(&raw_bytes) {
            if !lopdf_out.is_empty() && lopdf_out.len() < best_bytes.len() {
                if self.pdfium.load_pdf_from_byte_vec(lopdf_out.clone(), None).is_ok() {
                    best_bytes = lopdf_out;
                }
            }
        }

        // 2. Also try rebuilding into a fresh PDFium document (strips dead revisions) + lopdf compression
        if let Ok(mut clean_doc) = self.pdfium.create_new_pdf() {
            if clean_doc.pages_mut().append(doc).is_ok() {
                if let Ok(rebuilt_bytes) = clean_doc.save_to_bytes() {
                    if !rebuilt_bytes.is_empty() && rebuilt_bytes.len() < best_bytes.len() {
                        best_bytes = rebuilt_bytes.clone();
                    }
                    if let Ok(rebuilt_lopdf) = Self::compress_pdf(&rebuilt_bytes) {
                        if !rebuilt_lopdf.is_empty() && rebuilt_lopdf.len() < best_bytes.len() {
                            if self
                                .pdfium
                                .load_pdf_from_byte_vec(rebuilt_lopdf.clone(), None)
                                .is_ok()
                            {
                                best_bytes = rebuilt_lopdf;
                            }
                        }
                    }
                }
            }
        }

        let new_len = best_bytes.len();
        Ok((best_bytes, orig_len, new_len))
    }

    /// Converts a positive integer into lowercase Roman numerals (e.g. 1 -> "i", 4 -> "iv").
    pub fn to_roman(mut num: u16) -> String {
        let numerals = [
            (1000, "m"),
            (900, "cm"),
            (500, "d"),
            (400, "cd"),
            (100, "c"),
            (90, "xc"),
            (50, "l"),
            (40, "xl"),
            (10, "x"),
            (9, "ix"),
            (5, "v"),
            (4, "iv"),
            (1, "i"),
        ];
        let mut result = String::new();
        for &(val, sym) in &numerals {
            while num >= val {
                result.push_str(sym);
                num -= val;
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phase2_engine_upgrades() -> Result<()> {
        let engine = PdfEngine::new()?;
        let test_pdf = Path::new("test_unit_temp.pdf");
        engine.create_sample_pdf(test_pdf, 5)?;

        let mut doc = engine.load_pdf_file(test_pdf)?;
        assert_eq!(doc.pages().len(), 5);

        // 1. Stamp page numbers twice (verifies clear_margin_zone removes previous numbers without crashing)
        PdfEngine::apply_page_numbers_to_doc(&mut doc, 2)?;
        PdfEngine::apply_page_numbers_to_doc(&mut doc, 3)?;

        // 2. Stamp aligned headers/footers (Left, Center, Right) and clear them
        for pos in [
            MarginPosition::HeaderLeft,
            MarginPosition::HeaderCenter,
            MarginPosition::HeaderRight,
            MarginPosition::FooterLeft,
            MarginPosition::FooterCenter,
            MarginPosition::FooterRight,
        ] {
            PdfEngine::apply_aligned_header_footer_to_doc(&mut doc, pos, "Aligned Text", 1, 5)?;
        }
        PdfEngine::clear_margin_zone_in_doc(&mut doc, MarginZone::Header, 1, 5)?;
        PdfEngine::clear_margin_zone_in_doc(&mut doc, MarginZone::Footer, 1, 5)?;

        // Verify that the title text on page 1 is still near the top of the page (y > 500)
        {
            let page = doc.pages().get(0)?;
            let mut found_top_title = false;
            for obj in page.objects().iter() {
                if let Some(txt) = obj.as_text_object() {
                    if txt.text().contains("Sample Document") {
                        let b = obj.bounds()?;
                        assert!(b.bottom().value > 500.0);
                        found_top_title = true;
                    }
                }
            }
            assert!(found_top_title);
        }

        // 3. Reorder pages via move_page_in_doc
        engine.move_page_in_doc(&mut doc, 0, 3)?;
        assert_eq!(doc.pages().len(), 5);

        // 4. Insert external PDF at index
        let inserted = engine.insert_external_pdf_at_index(&mut doc, test_pdf, 2)?;
        assert_eq!(inserted, 5);
        assert_eq!(doc.pages().len(), 10);

        // 5. Bookmarks sidecar round-trip
        let bms = vec![
            Bookmark {
                title: "Intro".to_string(),
                page_index: 0,
            },
            Bookmark {
                title: "Chapter 2".to_string(),
                page_index: 4,
            },
        ];
        PdfEngine::save_bookmarks_sidecar(test_pdf, &bms)?;
        let loaded_bms = PdfEngine::load_bookmarks(&doc, test_pdf);
        assert_eq!(loaded_bms, bms);

        // 6. Compress PDF and verify compressed bytes load cleanly with 10 pages
        let (compressed_bytes, orig_len, new_len) = engine.compress_doc(&doc)?;
        assert!(!compressed_bytes.is_empty());
        assert!(new_len <= orig_len);
        let compressed_doc = engine
            .pdfium
            .load_pdf_from_byte_vec(compressed_bytes, None)?;
        assert_eq!(compressed_doc.pages().len(), 10);

        let _ = std::fs::remove_file(PdfEngine::bookmarks_sidecar_path(test_pdf));
        let _ = std::fs::remove_file(test_pdf);

        Ok(())
    }
}

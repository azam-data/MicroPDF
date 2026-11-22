//! Tier 1.3 Visual Modals:
//! 1. `File -> Create PDF from Images...` (PNG/JPG/BMP/WebP/GIF -> multi-page PDF opened in new tab)
//! 2. `Export -> Export Pages as Images (PNG/JPG)...` (All / Current / Custom range at 150/200/300 DPI)

use crate::engine::{ImageExportFormat, ImagePdfFitMode, PdfEngine};
use crate::print::parse_page_selection;
use std::path::PathBuf;

use super::render::{blend_rect_rgba, fill_rect_clipped};
use super::state::*;
use super::ui_draw::*;

#[derive(Clone, Debug)]
pub(super) struct ImageListEntry {
    pub(super) path: PathBuf,
    pub(super) file_name: String,
    pub(super) dims_label: String,
}

#[derive(Clone, Debug)]
pub(super) struct CreatePdfImagesModalState {
    pub(super) entries: Vec<ImageListEntry>,
    pub(super) scroll_offset: usize,
    pub(super) fit_mode: ImagePdfFitMode,
}

impl CreatePdfImagesModalState {
    pub(super) fn new() -> Self {
        Self {
            entries: Vec::new(),
            scroll_offset: 0,
            fit_mode: ImagePdfFitMode::FitA4,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum CreatePdfImagesHit {
    AddImages,
    MoveUp(usize),
    MoveDown(usize),
    RemoveRow(usize),
    FitMode(ImagePdfFitMode),
    CreateNow,
    Close,
    InsideModal,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum ExportImageScope {
    All,
    Current,
    Custom,
}

#[derive(Clone, Debug)]
pub(super) struct ExportImagesModalState {
    pub(super) scope: ExportImageScope,
    pub(super) custom_range: String,
    pub(super) editing_range: bool,
    pub(super) format: ImageExportFormat,
    pub(super) dpi: u32,
}

impl ExportImagesModalState {
    pub(super) fn new() -> Self {
        Self {
            scope: ExportImageScope::All,
            custom_range: String::new(),
            editing_range: false,
            format: ImageExportFormat::Png,
            dpi: 200,
        }
    }

    pub(super) fn resolved_pages(&self, current_page: u16, total_pages: u16) -> Result<Vec<u16>, String> {
        if total_pages == 0 {
            return Err("No pages in document".to_string());
        }
        match self.scope {
            ExportImageScope::All => Ok((0..total_pages).collect()),
            ExportImageScope::Current => Ok(vec![current_page.min(total_pages - 1)]),
            ExportImageScope::Custom => {
                let t = self.custom_range.trim();
                if t.is_empty() {
                    return Err("Enter a page range, e.g. 1-3, 5".to_string());
                }
                parse_page_selection(t, total_pages).map_err(|e| e.to_string())
            }
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum ExportImagesHit {
    Scope(ExportImageScope),
    CustomRangeBox,
    Format(ImageExportFormat),
    Dpi(u32),
    ExportNow,
    Close,
    InsideModal,
}

impl<'a> PdfViewerApp<'a> {
    pub(super) fn is_supported_image_path(path: &std::path::Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| {
                matches!(
                    ext.to_ascii_lowercase().as_str(),
                    "png" | "jpg" | "jpeg" | "bmp" | "webp" | "gif" | "tif" | "tiff"
                )
            })
    }

    pub(super) fn open_create_pdf_from_images_modal(&mut self) {
        self.close_all_popovers();
        self.create_pdf_images_modal = Some(CreatePdfImagesModalState::new());
        self.pending_dialog = Some(PendingDialog::PickImagesForPdf);
        self.request_redraw();
    }

    pub(super) fn add_images_to_create_modal(&mut self, paths: Vec<PathBuf>) {
        let Some(ref mut st) = self.create_pdf_images_modal else {
            return;
        };
        let mut added = 0usize;
        let mut skipped = 0usize;
        for path in paths {
            if !path.is_file() || !Self::is_supported_image_path(&path) {
                skipped += 1;
                continue;
            }
            let dims_label = match image::image_dimensions(&path) {
                Ok((w, h)) if w > 0 && h > 0 => format!("{w} × {h} px"),
                _ => {
                    skipped += 1;
                    continue;
                }
            };
            let file_name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("image")
                .to_string();
            st.entries.push(ImageListEntry {
                path,
                file_name,
                dims_label,
            });
            added += 1;
        }
        if added > 0 && skipped > 0 {
            self.status_notice = Some(format!("Added {added} image(s) (skipped {skipped} unsupported)"));
        } else if added > 0 {
            self.status_notice = Some(format!("Added {added} image(s) for PDF creation"));
        } else if skipped > 0 {
            self.status_notice = Some("Could not add selected files (use PNG, JPG, BMP, or WebP)".to_string());
        }
        self.request_redraw();
    }

    fn create_pdf_images_layout(
        &self,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<(i32, i32, u32, u32, Vec<(i32, i32, u32, u32, CreatePdfImagesHit)>, usize)> {
        let st = self.create_pdf_images_modal.as_ref()?;
        let s = sf as f32;
        let mw = ((580.0 * s).round() as u32).min(buf_w.saturating_sub(32)).max(360);
        let mh = ((480.0 * s).round() as u32).min(buf_h.saturating_sub(32)).max(340);
        let mx = ((buf_w.saturating_sub(mw)) / 2) as i32;
        let my = ((buf_h.saturating_sub(mh)) / 2) as i32;

        let pad = (16.0 * s).round() as i32;
        let hdr_h = (46.0 * s).round() as u32;
        let footer_h = (54.0 * s).round() as u32;
        let btn_h = (28.0 * s).round() as u32;
        let iw = mw.saturating_sub((pad * 2) as u32);
        let mut hits = Vec::new();

        // Top close button
        let close_sz = (24.0 * s).round() as u32;
        hits.push((
            mx + mw as i32 - pad - close_sz as i32,
            my + ((hdr_h.saturating_sub(close_sz)) / 2) as i32,
            close_sz,
            close_sz,
            CreatePdfImagesHit::Close,
        ));

        // Toolbar row: "+ Add Images..." button
        let top_row_y = my + hdr_h as i32 + (10.0 * s).round() as i32;
        let add_w = (140.0 * s).round() as u32;
        hits.push((mx + pad, top_row_y, add_w, btn_h, CreatePdfImagesHit::AddImages));

        // Fit mode pills near bottom above footer
        let fit_row_y = my + mh as i32 - footer_h as i32 - (44.0 * s).round() as i32;
        let gap = (8.0 * s).round() as i32;
        let modes = [
            ImagePdfFitMode::FitA4,
            ImagePdfFitMode::FitLetter,
            ImagePdfFitMode::MatchImageSize,
        ];
        let pill_w = (iw.saturating_sub((gap * 2) as u32)) / 3;
        for (i, mode) in modes.iter().enumerate() {
            let px = mx + pad + (i as i32) * (pill_w as i32 + gap);
            hits.push((px, fit_row_y, pill_w, btn_h, CreatePdfImagesHit::FitMode(*mode)));
        }

        // Scrollable image list rows
        let list_y = top_row_y + btn_h as i32 + (10.0 * s).round() as i32;
        let list_bottom = fit_row_y - (22.0 * s).round() as i32;
        let list_h = (list_bottom - list_y).max(40) as u32;
        let row_h = (34.0 * s).round() as u32;
        let visible_rows = (list_h / row_h).max(1) as usize;

        let sm_w = (24.0 * s).round() as u32;
        let sm_h = (22.0 * s).round() as u32;
        let start = st.scroll_offset.min(st.entries.len().saturating_sub(1));
        for (vis_i, idx) in (start..st.entries.len()).take(visible_rows).enumerate() {
            let ry = list_y + (vis_i as i32) * (row_h as i32);
            let by = ry + ((row_h.saturating_sub(sm_h)) / 2) as i32;
            let del_x = mx + mw as i32 - pad - sm_w as i32 - (6.0 * s).round() as i32;
            let dn_x = del_x - sm_w as i32 - 4;
            let up_x = dn_x - sm_w as i32 - 4;
            hits.push((up_x, by, sm_w, sm_h, CreatePdfImagesHit::MoveUp(idx)));
            hits.push((dn_x, by, sm_w, sm_h, CreatePdfImagesHit::MoveDown(idx)));
            hits.push((del_x, by, sm_w, sm_h, CreatePdfImagesHit::RemoveRow(idx)));
        }

        // Footer buttons: Cancel | Create PDF
        let fy = my + mh as i32 - footer_h as i32 + ((footer_h.saturating_sub(btn_h)) / 2) as i32;
        let create_w = (146.0 * s).round() as u32;
        let cancel_w = (84.0 * s).round() as u32;
        let create_x = mx + mw as i32 - pad - create_w as i32;
        let cancel_x = create_x - cancel_w as i32 - gap;
        hits.push((cancel_x, fy, cancel_w, btn_h, CreatePdfImagesHit::Close));
        hits.push((create_x, fy, create_w, btn_h, CreatePdfImagesHit::CreateNow));

        Some((mx, my, mw, mh, hits, visible_rows))
    }

    pub(super) fn draw_create_pdf_images_modal(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) {
        let Some(st) = self.create_pdf_images_modal.as_ref() else {
            return;
        };
        let Some((mx, my, mw, mh, hits, visible_rows)) = self.create_pdf_images_layout(buf_w, buf_h, sf) else {
            return;
        };
        let theme = self.ui_theme();
        let s = sf as f32;
        let pad = (16.0 * s).round() as i32;
        let hdr_h = (46.0 * s).round() as u32;
        let footer_h = (54.0 * s).round() as u32;
        let rad = (10.0 * s).max(6.0);
        let btn_rad = (6.0 * s).max(4.0);
        let title_sz = (14.5 * s).clamp(12.0, 28.0);
        let body_sz = (11.5 * s).clamp(10.0, 22.0);
        let sub_sz = (10.5 * s).clamp(9.0, 20.0);

        blend_rect_rgba(buffer, buf_w, buf_h, 0, 0, buf_w, buf_h, theme.modal_scrim);
        draw_soft_shadow(buffer, buf_w, buf_h, mx, my, mw, mh, rad, 18, 6, theme.shadow_rgb, theme.shadow_max_alpha);
        fill_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.border_subtle);

        // Header
        draw_text_vcenter(buffer, buf_w, buf_h, mx + pad, my, hdr_h, mx + mw as i32 - 48, "Create PDF from Images", title_sz, theme.text_primary, true);
        fill_rect_clipped(buffer, buf_w, buf_h, mx, my + hdr_h as i32, mw, 1, theme.divider);

        // Top row hint
        let top_row_y = my + hdr_h as i32 + (10.0 * s).round() as i32;
        let btn_h = (28.0 * s).round() as u32;
        let add_w = (140.0 * s).round() as u32;
        let hint_x = mx + pad + add_w as i32 + (12.0 * s).round() as i32;
        draw_text_vcenter(
            buffer,
            buf_w,
            buf_h,
            hint_x,
            top_row_y,
            btn_h,
            mx + mw as i32 - pad,
            "Tip: Drag & drop PNG / JPG files here too",
            sub_sz,
            theme.text_muted,
            false,
        );

        // List container
        let fit_row_y = my + mh as i32 - footer_h as i32 - (44.0 * s).round() as i32;
        let list_y = top_row_y + btn_h as i32 + (10.0 * s).round() as i32;
        let list_bottom = fit_row_y - (22.0 * s).round() as i32;
        let list_h = (list_bottom - list_y).max(40) as u32;
        let list_w = mw.saturating_sub((pad * 2) as u32);
        fill_rounded_rect(buffer, buf_w, buf_h, mx + pad, list_y, list_w, list_h, btn_rad, theme.surface_elevated);
        stroke_rounded_rect(buffer, buf_w, buf_h, mx + pad, list_y, list_w, list_h, btn_rad, theme.border_subtle);

        if st.entries.is_empty() {
            let empty_msg = "No images selected yet — click '+ Add Images...' or drop PNG/JPG files.";
            let ew = measure_text(empty_msg, body_sz, false);
            draw_text_vcenter(
                buffer,
                buf_w,
                buf_h,
                mx + ((mw as f32 - ew) * 0.5).round().max(pad as f32) as i32,
                list_y,
                list_h,
                mx + mw as i32 - pad,
                empty_msg,
                body_sz,
                theme.text_muted,
                false,
            );
        } else {
            let row_h = (34.0 * s).round() as u32;
            let start = st.scroll_offset.min(st.entries.len().saturating_sub(1));
            for (vis_i, idx) in (start..st.entries.len()).take(visible_rows).enumerate() {
                let entry = &st.entries[idx];
                let ry = list_y + (vis_i as i32) * (row_h as i32);
                if vis_i > 0 {
                    fill_rect_clipped(buffer, buf_w, buf_h, mx + pad + 6, ry, list_w.saturating_sub(12), 1, theme.divider);
                }
                let idx_lbl = format!("{}.", idx + 1);
                draw_text_vcenter(buffer, buf_w, buf_h, mx + pad + (8.0 * s).round() as i32, ry, row_h, mx + pad + (34.0 * s).round() as i32, &idx_lbl, sub_sz, theme.text_muted, true);
                let name_x = mx + pad + (34.0 * s).round() as i32;
                let right_controls = mx + mw as i32 - pad - (100.0 * s).round() as i32;
                let dims_w = measure_text(&entry.dims_label, sub_sz, false).ceil() as i32;
                let name_max = (right_controls - dims_w - (14.0 * s).round() as i32 - name_x).max(40) as f32;
                let clipped_name = truncate_ellipsis(&entry.file_name, name_max, body_sz, false);
                draw_text_vcenter(buffer, buf_w, buf_h, name_x, ry, row_h, right_controls - dims_w - 8, &clipped_name, body_sz, theme.text_primary, false);
                draw_text_vcenter(buffer, buf_w, buf_h, right_controls - dims_w - 4, ry, row_h, right_controls, &entry.dims_label, sub_sz, theme.text_muted, false);
            }
        }

        // Fit mode heading
        draw_text(
            buffer,
            buf_w,
            buf_h,
            mx + pad,
            fit_row_y - (17.0 * s).round() as i32,
            "Page Size & Fit:",
            sub_sz,
            theme.text_secondary,
            true,
        );

        // Footer divider & count label
        let footer_y = my + mh as i32 - footer_h as i32;
        fill_rect_clipped(buffer, buf_w, buf_h, mx, footer_y, mw, 1, theme.divider);
        let summary = format!("{} page(s) ready", st.entries.len());
        draw_text_vcenter(buffer, buf_w, buf_h, mx + pad, footer_y, footer_h, mx + mw as i32 / 2, &summary, body_sz, theme.text_secondary, false);

        // Render interactive buttons
        let ic_sz = (12.0 * s).round() as u32;
        for &(bx, by, bw, bh, hit) in &hits {
            let hov = self.hovered_widget == Some(UiHit::CreatePdfImages(hit));
            match hit {
                CreatePdfImagesHit::Close if by < my + hdr_h as i32 => {
                    if hov {
                        fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, theme.hover_bg);
                    }
                    draw_icon(buffer, buf_w, buf_h, UiIcon::Close, bx + ((bw.saturating_sub(ic_sz)) / 2) as i32, by + ((bh.saturating_sub(ic_sz)) / 2) as i32, ic_sz, theme.text_secondary);
                }
                CreatePdfImagesHit::Close => {
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, if hov { theme.hover_bg } else { theme.surface_elevated });
                    stroke_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, theme.border_subtle);
                    let tw = measure_text("Cancel", body_sz, false);
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round() as i32, by, bh, bx + bw as i32, "Cancel", body_sz, theme.text_primary, false);
                }
                CreatePdfImagesHit::AddImages => {
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, if hov { theme.hover_bg } else { theme.surface_elevated });
                    stroke_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, theme.accent);
                    let lbl = "+ Add Images...";
                    let tw = measure_text(lbl, body_sz, true);
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round() as i32, by, bh, bx + bw as i32, lbl, body_sz, theme.accent, true);
                }
                CreatePdfImagesHit::FitMode(mode) => {
                    let active = st.fit_mode == mode;
                    let bg = if active { theme.accent_soft_bg } else if hov { theme.hover_bg } else { theme.surface_elevated };
                    let bdr = if active { theme.accent } else { theme.border_subtle };
                    let fg = if active { theme.accent_soft_fg } else { theme.text_secondary };
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, bg);
                    stroke_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, bdr);
                    let lbl = mode.label();
                    let tw = measure_text(lbl, sub_sz, active);
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round().max(4.0) as i32, by, bh, bx + bw as i32 - 3, lbl, sub_sz, fg, active);
                }
                CreatePdfImagesHit::MoveUp(_) | CreatePdfImagesHit::MoveDown(_) | CreatePdfImagesHit::RemoveRow(_) => {
                    let is_del = matches!(hit, CreatePdfImagesHit::RemoveRow(_));
                    let bg = if hov { if is_del { theme.danger_soft_bg } else { theme.hover_bg } } else { theme.surface_bg };
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, 4.0, bg);
                    stroke_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, 4.0, theme.border_subtle);
                    let lbl = match hit {
                        CreatePdfImagesHit::MoveUp(_) => "↑",
                        CreatePdfImagesHit::MoveDown(_) => "↓",
                        _ => "×",
                    };
                    let fg = if is_del && hov { theme.danger } else { theme.text_secondary };
                    let tw = measure_text(lbl, sub_sz, true);
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round() as i32, by, bh, bx + bw as i32, lbl, sub_sz, fg, true);
                }
                CreatePdfImagesHit::CreateNow => {
                    let enabled = !st.entries.is_empty();
                    let bg = if !enabled { theme.surface_elevated } else if hov { theme.accent_hover } else { theme.accent };
                    let fg = if enabled { theme.text_on_accent } else { theme.text_muted };
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, bg);
                    let lbl = "Create PDF";
                    let tw = measure_text(lbl, body_sz, true);
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round() as i32, by, bh, bx + bw as i32, lbl, body_sz, fg, true);
                }
                CreatePdfImagesHit::InsideModal => {}
            }
        }
    }

    pub(super) fn hit_test_create_pdf_images_modal(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<CreatePdfImagesHit> {
        let (x, y, w, h, hits, _) = self.create_pdf_images_layout(buf_w, buf_h, sf)?;
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        for (bx, by, bw, bh, hit) in hits {
            if ix >= bx && ix < bx + bw as i32 && iy >= by && iy < by + bh as i32 {
                return Some(hit);
            }
        }
        if ix >= x && ix < x + w as i32 && iy >= y && iy < y + h as i32 {
            return Some(CreatePdfImagesHit::InsideModal);
        }
        None
    }

    pub(super) fn handle_create_pdf_images_click(&mut self, hit: CreatePdfImagesHit) {
        match hit {
            CreatePdfImagesHit::Close => {
                self.create_pdf_images_modal = None;
                self.request_redraw();
            }
            CreatePdfImagesHit::AddImages => {
                self.pending_dialog = Some(PendingDialog::PickImagesForPdf);
                self.request_redraw();
            }
            CreatePdfImagesHit::FitMode(mode) => {
                if let Some(ref mut st) = self.create_pdf_images_modal {
                    st.fit_mode = mode;
                }
                self.request_redraw();
            }
            CreatePdfImagesHit::MoveUp(idx) => {
                if let Some(ref mut st) = self.create_pdf_images_modal {
                    if idx > 0 && idx < st.entries.len() {
                        st.entries.swap(idx - 1, idx);
                    }
                }
                self.request_redraw();
            }
            CreatePdfImagesHit::MoveDown(idx) => {
                if let Some(ref mut st) = self.create_pdf_images_modal {
                    if idx + 1 < st.entries.len() {
                        st.entries.swap(idx, idx + 1);
                    }
                }
                self.request_redraw();
            }
            CreatePdfImagesHit::RemoveRow(idx) => {
                if let Some(ref mut st) = self.create_pdf_images_modal {
                    if idx < st.entries.len() {
                        st.entries.remove(idx);
                        if st.scroll_offset > 0 && st.scroll_offset >= st.entries.len() {
                            st.scroll_offset = st.entries.len().saturating_sub(1);
                        }
                    }
                }
                self.request_redraw();
            }
            CreatePdfImagesHit::CreateNow => {
                let Some(st) = self.create_pdf_images_modal.clone() else {
                    return;
                };
                if st.entries.is_empty() {
                    return;
                }
                let paths: Vec<PathBuf> = st.entries.iter().map(|e| e.path.clone()).collect();
                let count = paths.len();
                let default_dir = paths[0]
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(std::env::temp_dir);
                let virtual_path = default_dir.join("Images_PDF.pdf");
                match PdfEngine::create_pdf_from_images(&paths, st.fit_mode) {
                    Ok(bytes) => {
                        self.create_pdf_images_modal = None;
                        match self.open_bytes_in_new_tab(bytes, virtual_path) {
                            Ok(()) => {
                                self.status_notice = Some(format!(
                                    "Created {count}-page PDF from images — press Ctrl+Shift+S to Save As"
                                ));
                            }
                            Err(e) => {
                                self.status_notice = Some(format!("Failed to open generated PDF: {e}"));
                            }
                        }
                    }
                    Err(e) => {
                        self.status_notice = Some(format!("Create PDF from Images failed: {e}"));
                    }
                }
                self.update_title_bar();
                self.request_redraw();
            }
            CreatePdfImagesHit::InsideModal => {}
        }
    }

    // =========================================================================
    // Export Pages as Images (PNG / JPEG) Modal
    // =========================================================================

    pub(super) fn open_export_images_modal(&mut self) {
        if self.doc.is_none() {
            self.status_notice = Some("Open a PDF first to export pages as images".to_string());
            self.request_redraw();
            return;
        }
        self.close_all_popovers();
        self.export_images_modal = Some(ExportImagesModalState::new());
        self.request_redraw();
    }

    fn export_images_layout(
        &self,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<(i32, i32, u32, u32, Vec<(i32, i32, u32, u32, ExportImagesHit)>)> {
        let _st = self.export_images_modal.as_ref()?;
        let s = sf as f32;
        let mw = ((470.0 * s).round() as u32).min(buf_w.saturating_sub(32)).max(340);
        let mh = ((345.0 * s).round() as u32).min(buf_h.saturating_sub(32)).max(290);
        let mx = ((buf_w.saturating_sub(mw)) / 2) as i32;
        let my = ((buf_h.saturating_sub(mh)) / 2) as i32;

        let pad = (16.0 * s).round() as i32;
        let hdr_h = (44.0 * s).round() as u32;
        let footer_h = (52.0 * s).round() as u32;
        let btn_h = (28.0 * s).round() as u32;
        let gap = (8.0 * s).round() as i32;
        let iw = mw.saturating_sub((pad * 2) as u32);
        let mut hits = Vec::new();

        let close_sz = (24.0 * s).round() as u32;
        hits.push((
            mx + mw as i32 - pad - close_sz as i32,
            my + ((hdr_h.saturating_sub(close_sz)) / 2) as i32,
            close_sz,
            close_sz,
            ExportImagesHit::Close,
        ));

        // Scope row
        let mut cy = my + hdr_h as i32 + (28.0 * s).round() as i32;
        let sc_w = (iw.saturating_sub((gap * 2) as u32)) / 3;
        for (i, scope) in [
            ExportImageScope::All,
            ExportImageScope::Current,
            ExportImageScope::Custom,
        ]
        .iter()
        .enumerate()
        {
            hits.push((
                mx + pad + (i as i32) * (sc_w as i32 + gap),
                cy,
                sc_w,
                btn_h,
                ExportImagesHit::Scope(*scope),
            ));
        }

        // Custom page range input box
        cy += btn_h as i32 + (10.0 * s).round() as i32;
        hits.push((mx + pad, cy, iw, btn_h, ExportImagesHit::CustomRangeBox));

        // Format row
        cy += btn_h as i32 + (28.0 * s).round() as i32;
        let fmt_w = (iw.saturating_sub(gap as u32)) / 2;
        hits.push((mx + pad, cy, fmt_w, btn_h, ExportImagesHit::Format(ImageExportFormat::Png)));
        hits.push((
            mx + pad + fmt_w as i32 + gap,
            cy,
            iw.saturating_sub(fmt_w + gap as u32),
            btn_h,
            ExportImagesHit::Format(ImageExportFormat::Jpeg),
        ));

        // DPI row
        cy += btn_h as i32 + (28.0 * s).round() as i32;
        let dpi_w = (iw.saturating_sub((gap * 2) as u32)) / 3;
        for (i, dpi) in [150u32, 200u32, 300u32].iter().enumerate() {
            hits.push((
                mx + pad + (i as i32) * (dpi_w as i32 + gap),
                cy,
                dpi_w,
                btn_h,
                ExportImagesHit::Dpi(*dpi),
            ));
        }

        // Footer buttons
        let fy = my + mh as i32 - footer_h as i32 + ((footer_h.saturating_sub(btn_h)) / 2) as i32;
        let exp_w = (190.0 * s).round() as u32;
        let cancel_w = (84.0 * s).round() as u32;
        let exp_x = mx + mw as i32 - pad - exp_w as i32;
        let cancel_x = exp_x - cancel_w as i32 - gap;
        hits.push((cancel_x, fy, cancel_w, btn_h, ExportImagesHit::Close));
        hits.push((exp_x, fy, exp_w, btn_h, ExportImagesHit::ExportNow));

        Some((mx, my, mw, mh, hits))
    }

    pub(super) fn draw_export_images_modal(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) {
        let Some(st) = self.export_images_modal.as_ref() else {
            return;
        };
        let Some((mx, my, mw, mh, hits)) = self.export_images_layout(buf_w, buf_h, sf) else {
            return;
        };
        let theme = self.ui_theme();
        let s = sf as f32;
        let pad = (16.0 * s).round() as i32;
        let hdr_h = (44.0 * s).round() as u32;
        let footer_h = (52.0 * s).round() as u32;
        let btn_h = (28.0 * s).round() as u32;
        let rad = (10.0 * s).max(6.0);
        let btn_rad = (6.0 * s).max(4.0);
        let title_sz = (14.5 * s).clamp(12.0, 28.0);
        let body_sz = (11.5 * s).clamp(10.0, 22.0);
        let sub_sz = (10.5 * s).clamp(9.0, 20.0);

        blend_rect_rgba(buffer, buf_w, buf_h, 0, 0, buf_w, buf_h, theme.modal_scrim);
        draw_soft_shadow(buffer, buf_w, buf_h, mx, my, mw, mh, rad, 18, 6, theme.shadow_rgb, theme.shadow_max_alpha);
        fill_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.border_subtle);

        draw_text_vcenter(buffer, buf_w, buf_h, mx + pad, my, hdr_h, mx + mw as i32 - 48, "Export Pages as Images", title_sz, theme.text_primary, true);
        fill_rect_clipped(buffer, buf_w, buf_h, mx, my + hdr_h as i32, mw, 1, theme.divider);

        let scope_y = my + hdr_h as i32 + (28.0 * s).round() as i32;
        draw_text(buffer, buf_w, buf_h, mx + pad, scope_y - (17.0 * s).round() as i32, "Pages to Export:", sub_sz, theme.text_secondary, true);
        let fmt_y = scope_y + btn_h as i32 * 2 + (38.0 * s).round() as i32;
        draw_text(buffer, buf_w, buf_h, mx + pad, fmt_y - (17.0 * s).round() as i32, "Image Format:", sub_sz, theme.text_secondary, true);
        let dpi_y = fmt_y + btn_h as i32 + (28.0 * s).round() as i32;
        draw_text(buffer, buf_w, buf_h, mx + pad, dpi_y - (17.0 * s).round() as i32, "Resolution (DPI):", sub_sz, theme.text_secondary, true);

        let footer_y = my + mh as i32 - footer_h as i32;
        fill_rect_clipped(buffer, buf_w, buf_h, mx, footer_y, mw, 1, theme.divider);
        let resolved = st.resolved_pages(self.current_page_index, self.total_pages());
        let status_txt = match &resolved {
            Ok(pages) => format!("{} image(s)", pages.len()),
            Err(e) => e.clone(),
        };
        let st_color = if resolved.is_ok() { theme.text_secondary } else { theme.danger };
        draw_text_vcenter(buffer, buf_w, buf_h, mx + pad, footer_y, footer_h, mx + mw as i32 - (290.0 * s).round() as i32, &status_txt, sub_sz, st_color, false);

        let ic_sz = (12.0 * s).round() as u32;
        for &(bx, by, bw, bh, hit) in &hits {
            let hov = self.hovered_widget == Some(UiHit::ExportImages(hit));
            match hit {
                ExportImagesHit::Close if by < my + hdr_h as i32 => {
                    if hov {
                        fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, theme.hover_bg);
                    }
                    draw_icon(buffer, buf_w, buf_h, UiIcon::Close, bx + ((bw.saturating_sub(ic_sz)) / 2) as i32, by + ((bh.saturating_sub(ic_sz)) / 2) as i32, ic_sz, theme.text_secondary);
                }
                ExportImagesHit::Close => {
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, if hov { theme.hover_bg } else { theme.surface_elevated });
                    stroke_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, theme.border_subtle);
                    let tw = measure_text("Cancel", body_sz, false);
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round() as i32, by, bh, bx + bw as i32, "Cancel", body_sz, theme.text_primary, false);
                }
                ExportImagesHit::Scope(sc) => {
                    let active = st.scope == sc;
                    let bg = if active { theme.accent_soft_bg } else if hov { theme.hover_bg } else { theme.surface_elevated };
                    let bdr = if active { theme.accent } else { theme.border_subtle };
                    let fg = if active { theme.accent_soft_fg } else { theme.text_secondary };
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, bg);
                    stroke_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, bdr);
                    let lbl = match sc {
                        ExportImageScope::All => format!("All ({})", self.total_pages()),
                        ExportImageScope::Current => format!("Current (p.{})", self.current_page_index + 1),
                        ExportImageScope::Custom => "Custom Range".to_string(),
                    };
                    let tw = measure_text(&lbl, sub_sz, active);
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round().max(4.0) as i32, by, bh, bx + bw as i32 - 3, &lbl, sub_sz, fg, active);
                }
                ExportImagesHit::CustomRangeBox => {
                    let active = st.scope == ExportImageScope::Custom;
                    let bdr = if st.editing_range { theme.accent } else { theme.border_subtle };
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, theme.surface_elevated);
                    stroke_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, bdr);
                    let display = if st.custom_range.is_empty() {
                        if st.editing_range {
                            "_"
                        } else {
                            "Custom pages (e.g. 1-3, 5, 8)..."
                        }
                    } else {
                        &st.custom_range
                    };
                    let txt = if st.editing_range && !st.custom_range.is_empty() {
                        format!("{}_", st.custom_range)
                    } else {
                        display.to_string()
                    };
                    let fg = if !active || st.custom_range.is_empty() {
                        theme.text_muted
                    } else {
                        theme.text_primary
                    };
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + (10.0 * s).round() as i32, by, bh, bx + bw as i32 - 8, &txt, body_sz, fg, false);
                }
                ExportImagesHit::Format(fmt) => {
                    let active = st.format == fmt;
                    let bg = if active { theme.accent_soft_bg } else if hov { theme.hover_bg } else { theme.surface_elevated };
                    let bdr = if active { theme.accent } else { theme.border_subtle };
                    let fg = if active { theme.accent_soft_fg } else { theme.text_secondary };
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, bg);
                    stroke_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, bdr);
                    let lbl = fmt.label();
                    let tw = measure_text(lbl, sub_sz, active);
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round().max(4.0) as i32, by, bh, bx + bw as i32 - 3, lbl, sub_sz, fg, active);
                }
                ExportImagesHit::Dpi(dpi) => {
                    let active = st.dpi == dpi;
                    let bg = if active { theme.accent_soft_bg } else if hov { theme.hover_bg } else { theme.surface_elevated };
                    let bdr = if active { theme.accent } else { theme.border_subtle };
                    let fg = if active { theme.accent_soft_fg } else { theme.text_secondary };
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, bg);
                    stroke_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, bdr);
                    let lbl = match dpi {
                        150 => "150 DPI (Web)",
                        200 => "200 DPI (Balanced)",
                        _ => "300 DPI (Print)",
                    };
                    let tw = measure_text(lbl, sub_sz, active);
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round().max(4.0) as i32, by, bh, bx + bw as i32 - 3, lbl, sub_sz, fg, active);
                }
                ExportImagesHit::ExportNow => {
                    let enabled = resolved.is_ok();
                    let bg = if !enabled { theme.surface_elevated } else if hov { theme.accent_hover } else { theme.accent };
                    let fg = if enabled { theme.text_on_accent } else { theme.text_muted };
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, bg);
                    let lbl = "Choose Folder & Export...";
                    let tw = measure_text(lbl, body_sz, true);
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round() as i32, by, bh, bx + bw as i32, lbl, body_sz, fg, true);
                }
                ExportImagesHit::InsideModal => {}
            }
        }
    }

    pub(super) fn hit_test_export_images_modal(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<ExportImagesHit> {
        let (x, y, w, h, hits) = self.export_images_layout(buf_w, buf_h, sf)?;
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        for (bx, by, bw, bh, hit) in hits {
            if ix >= bx && ix < bx + bw as i32 && iy >= by && iy < by + bh as i32 {
                return Some(hit);
            }
        }
        if ix >= x && ix < x + w as i32 && iy >= y && iy < y + h as i32 {
            return Some(ExportImagesHit::InsideModal);
        }
        None
    }

    pub(super) fn handle_export_images_click(&mut self, hit: ExportImagesHit) {
        match hit {
            ExportImagesHit::Close => {
                self.export_images_modal = None;
                self.request_redraw();
            }
            ExportImagesHit::Scope(sc) => {
                if let Some(ref mut st) = self.export_images_modal {
                    st.scope = sc;
                    st.editing_range = sc == ExportImageScope::Custom;
                }
                self.request_redraw();
            }
            ExportImagesHit::CustomRangeBox => {
                if let Some(ref mut st) = self.export_images_modal {
                    st.scope = ExportImageScope::Custom;
                    st.editing_range = true;
                }
                self.request_redraw();
            }
            ExportImagesHit::Format(fmt) => {
                if let Some(ref mut st) = self.export_images_modal {
                    st.format = fmt;
                    st.editing_range = false;
                }
                self.request_redraw();
            }
            ExportImagesHit::Dpi(dpi) => {
                if let Some(ref mut st) = self.export_images_modal {
                    st.dpi = dpi;
                    st.editing_range = false;
                }
                self.request_redraw();
            }
            ExportImagesHit::ExportNow => {
                let valid = self
                    .export_images_modal
                    .as_ref()
                    .and_then(|st| st.resolved_pages(self.current_page_index, self.total_pages()).ok())
                    .is_some();
                if valid {
                    self.pending_dialog = Some(PendingDialog::ExportPagesAsImagesDir);
                    self.request_redraw();
                }
            }
            ExportImagesHit::InsideModal => {
                if let Some(ref mut st) = self.export_images_modal {
                    st.editing_range = false;
                }
                self.request_redraw();
            }
        }
    }

    pub(super) fn handle_export_images_key(
        &mut self,
        key: &winit::keyboard::Key,
        text: Option<&str>,
    ) {
        use winit::keyboard::{Key, NamedKey};
        match key {
            Key::Named(NamedKey::Escape) => {
                self.export_images_modal = None;
                self.request_redraw();
            }
            Key::Named(NamedKey::Enter) => {
                self.handle_export_images_click(ExportImagesHit::ExportNow);
            }
            Key::Named(NamedKey::Backspace) => {
                if let Some(ref mut st) = self.export_images_modal {
                    if st.editing_range {
                        st.custom_range.pop();
                    }
                }
                self.request_redraw();
            }
            Key::Named(NamedKey::Delete) => {
                if let Some(ref mut st) = self.export_images_modal {
                    if st.editing_range {
                        st.custom_range.clear();
                    }
                }
                self.request_redraw();
            }
            Key::Character(c) if self.cmd_or_ctrl_pressed && c.eq_ignore_ascii_case("v") => {
                if let Some(ref mut st) = self.export_images_modal {
                    if st.editing_range {
                        if let Ok(clip) = Self::read_text_from_os_clipboard() {
                            for ch in clip.chars() {
                                if ch.is_ascii_digit() || matches!(ch, '-' | ',' | ' ') {
                                    st.custom_range.push(ch);
                                }
                            }
                        }
                    }
                }
                self.request_redraw();
            }
            _ if !self.cmd_or_ctrl_pressed && !self.alt_pressed => {
                if let (Some(t), Some(st)) = (text, self.export_images_modal.as_mut()) {
                    if st.editing_range {
                        for ch in t.chars() {
                            if ch.is_ascii_digit() || matches!(ch, '-' | ',' | ' ') {
                                st.custom_range.push(ch);
                            }
                        }
                        self.request_redraw();
                    }
                }
            }
            _ => {}
        }
    }
}

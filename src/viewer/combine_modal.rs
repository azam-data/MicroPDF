//! Interactive Combine Multiple PDFs split-view modal:
//! Add or drop multiple PDFs, reorder up/down, specify per-file page ranges (`all` or `1-3, 5`),
//! inspect any selected PDF in a live right-hand preview pane (single reused bitmap, freed on close),
//! and combine into a new `"Combined.pdf"` tab.

use crate::print::parse_page_selection;
use pdfium_render::prelude::*;
use std::path::PathBuf;

use super::render::{blend_rect_rgba, fill_rect_clipped};
use super::state::*;
use super::ui_draw::*;

#[derive(Clone, Debug)]
pub(super) struct CombineEntry {
    pub(super) path: PathBuf,
    pub(super) file_name: String,
    pub(super) total_pages: u16,
    pub(super) page_range: String,
    pub(super) range_error: Option<String>,
}

impl CombineEntry {
    pub(super) fn resolved_pages(&self) -> Result<Option<Vec<u16>>, String> {
        let trimmed = self.page_range.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("all") {
            return Ok(None);
        }
        parse_page_selection(trimmed, self.total_pages)
            .map(Some)
            .map_err(|e| e.to_string())
    }

    pub(super) fn effective_count(&self) -> usize {
        match self.resolved_pages() {
            Ok(None) => self.total_pages as usize,
            Ok(Some(v)) => v.len(),
            Err(_) => 0,
        }
    }

    pub(super) fn validate(&mut self) {
        self.range_error = self.resolved_pages().err();
    }
}

pub(super) struct CombinePreviewBitmap {
    pub(super) path: PathBuf,
    pub(super) page_index: u16,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) pixels: Vec<u32>,
}

pub(super) struct CombineModalState {
    pub(super) entries: Vec<CombineEntry>,
    pub(super) selected: usize,
    pub(super) scroll_offset: usize,
    pub(super) preview_page: u16,
    pub(super) editing_range: bool,
    pub(super) cached_preview: Option<CombinePreviewBitmap>,
}

impl CombineModalState {
    pub(super) fn new() -> Self {
        Self {
            entries: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            preview_page: 0,
            editing_range: false,
            cached_preview: None,
        }
    }

    pub(super) fn total_output_pages(&self) -> usize {
        self.entries.iter().map(CombineEntry::effective_count).sum()
    }

    pub(super) fn has_errors(&self) -> bool {
        self.entries.iter().any(|e| e.range_error.is_some())
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum CombineHit {
    AddFiles,
    SelectRow(usize),
    RangeBox(usize),
    MoveUp(usize),
    MoveDown(usize),
    RemoveRow(usize),
    PreviewPrev,
    PreviewNext,
    CombineNow,
    Cancel,
    InsideModal,
}

#[derive(Clone, Copy)]
struct CombineBox {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    hit: CombineHit,
}

impl CombineBox {
    fn contains(self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w as i32 && py >= self.y && py < self.y + self.h as i32
    }
}

struct CombineLayout {
    mx: i32,
    my: i32,
    mw: u32,
    mh: u32,
    left_w: u32,
    preview_x: i32,
    preview_w: u32,
    visible_rows: usize,
    hits: Vec<CombineBox>,
}

impl<'a> PdfViewerApp<'a> {
    pub(super) fn open_combine_modal(&mut self) {
        self.open_menu = None;
        self.show_customize_toolbar = false;
        self.show_help_overlay = false;
        self.show_about_overlay = false;
        self.print_preview = None;
        let mut st = CombineModalState::new();
        // If a PDF is already open on disk, pre-seed it as the first item for convenience!
        if self.doc.is_some() && !self.file_path.as_os_str().is_empty() && self.file_path.exists() {
            let fname = self.filename_display();
            st.entries.push(CombineEntry {
                path: self.file_path.clone(),
                file_name: fname,
                total_pages: self.total_pages().max(1),
                page_range: "all".to_string(),
                range_error: None,
            });
        }
        self.combine_modal = Some(st);
        self.request_redraw();
    }

    pub(super) fn close_combine_modal(&mut self) {
        self.combine_modal = None;
        self.request_redraw();
    }

    pub(super) fn add_files_to_combine_modal(&mut self, paths: Vec<PathBuf>) {
        let Some(cm) = self.combine_modal.as_mut() else { return };
        let mut added = 0usize;
        for path in paths {
            let total_pages = match self.engine.load_pdf_safe(&path) {
                Ok(doc) => doc.pages().len().max(1),
                Err(_) => continue,
            };
            let file_name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("document.pdf")
                .to_string();
            cm.entries.push(CombineEntry {
                path,
                file_name,
                total_pages,
                page_range: "all".to_string(),
                range_error: None,
            });
            added += 1;
        }
        if added > 0 && cm.entries.len() == added {
            cm.selected = 0;
            cm.preview_page = 0;
            cm.cached_preview = None;
        }
        if added > 0 {
            self.status_notice = Some(format!("Added {added} PDF(s) to Combine queue"));
        }
        self.request_redraw();
    }

    pub(super) fn ensure_combine_preview_rendered(&mut self, buf_w: u32, buf_h: u32, sf: f64) {
        let Some(cm) = self.combine_modal.as_mut() else { return };
        let Some(entry) = cm.entries.get(cm.selected) else {
            cm.cached_preview = None;
            return;
        };
        let target_page = cm.preview_page.min(entry.total_pages.saturating_sub(1));
        cm.preview_page = target_page;

        if let Some(ref cached) = cm.cached_preview {
            if cached.path == entry.path && cached.page_index == target_page {
                return;
            }
        }

        let s = sf as f32;
        let mw = ((860.0 * s).round() as u32).min(buf_w.saturating_sub(36));
        let mh = ((580.0 * s).round() as u32).min(buf_h.saturating_sub(36));
        let left_w = ((mw as f32) * 0.56).round() as u32;
        let prev_w = mw.saturating_sub(left_w).saturating_sub((44.0 * s).round() as u32).max(120);
        let prev_h = mh.saturating_sub((130.0 * s).round() as u32).max(140);

        let Ok(doc) = self.engine.load_pdf_safe(&entry.path) else {
            cm.cached_preview = None;
            return;
        };
        let Ok(page) = doc.pages().get(target_page) else {
            cm.cached_preview = None;
            return;
        };
        let pw = page.width().value.max(1.0);
        let ph = page.height().value.max(1.0);
        let scale = (prev_w as f32 / pw).min(prev_h as f32 / ph);
        let rw = (pw * scale).round().clamp(40.0, 640.0) as i32;
        let rh = (ph * scale).round().clamp(40.0, 820.0) as i32;

        let cfg = PdfRenderConfig::new()
            .set_target_width(rw)
            .set_maximum_height(rh)
            .render_form_data(true)
            .render_annotations(true);
        if let Ok(bmp) = page.render_with_config(&cfg) {
            let img = bmp.as_image().to_rgba8();
            let (iw, ih) = img.dimensions();
            let raw = img.into_raw();
            let mut pixels = Vec::with_capacity((iw * ih) as usize);
            for chunk in raw.chunks_exact(4) {
                pixels.push(((chunk[0] as u32) << 16) | ((chunk[1] as u32) << 8) | (chunk[2] as u32));
            }
            cm.cached_preview = Some(CombinePreviewBitmap {
                path: entry.path.clone(),
                page_index: target_page,
                width: iw,
                height: ih,
                pixels,
            });
        }
    }

    fn compute_combine_layout(&self, buf_w: u32, buf_h: u32, sf: f64) -> CombineLayout {
        let s = sf as f32;
        let mw = ((860.0 * s).round() as u32).min(buf_w.saturating_sub(32));
        let mh = ((580.0 * s).round() as u32).min(buf_h.saturating_sub(32));
        let mx = (buf_w.saturating_sub(mw) / 2) as i32;
        let my = (buf_h.saturating_sub(mh) / 2) as i32;
        let left_w = ((mw as f32) * 0.57).round() as u32;
        let preview_x = mx + left_w as i32;
        let preview_w = mw.saturating_sub(left_w);
        let pad = (16.0 * s).round() as i32;
        let mut hits = Vec::new();

        // Top-left "+ Add PDFs..." button
        let add_w = (118.0 * s).round() as u32;
        let add_h = (26.0 * s).round() as u32;
        let add_x = preview_x - pad - add_w as i32;
        let add_y = my + (12.0 * s).round() as i32;
        hits.push(CombineBox { x: add_x, y: add_y, w: add_w, h: add_h, hit: CombineHit::AddFiles });

        // List rows
        let list_top = my + (54.0 * s).round() as i32;
        let footer_h = (56.0 * s).round() as i32;
        let list_h = (mh as i32 - (list_top - my) - footer_h).max(60);
        let row_h = (48.0 * s).round() as i32;
        let visible_rows = (list_h / row_h).max(1) as usize;

        if let Some(ref cm) = self.combine_modal {
            let btn_sz = (22.0 * s).round() as u32;
            let gap = (4.0 * s).round() as i32;
            let row_w = left_w.saturating_sub((pad * 2) as u32);
            for (slot, idx) in (cm.scroll_offset..cm.entries.len()).take(visible_rows).enumerate() {
                let rx = mx + pad;
                let ry = list_top + (slot as i32) * row_h;
                let rh = (row_h - (6.0 * s).round() as i32) as u32;

                // Action buttons on right side of row: Up, Down, Remove
                let rem_x = rx + row_w as i32 - (6.0 * s).round() as i32 - btn_sz as i32;
                let dn_x = rem_x - gap - btn_sz as i32;
                let up_x = dn_x - gap - btn_sz as i32;
                let by = ry + ((rh.saturating_sub(btn_sz)) / 2) as i32;

                // Per-file Page Range box just left of Up/Down/Remove
                let rng_w = (88.0 * s).round() as u32;
                let rng_h = (22.0 * s).round() as u32;
                let rng_x = up_x - (8.0 * s).round() as i32 - rng_w as i32;
                let rng_y = ry + ((rh.saturating_sub(rng_h)) / 2) as i32;

                hits.push(CombineBox { x: up_x, y: by, w: btn_sz, h: btn_sz, hit: CombineHit::MoveUp(idx) });
                hits.push(CombineBox { x: dn_x, y: by, w: btn_sz, h: btn_sz, hit: CombineHit::MoveDown(idx) });
                hits.push(CombineBox { x: rem_x, y: by, w: btn_sz, h: btn_sz, hit: CombineHit::RemoveRow(idx) });
                hits.push(CombineBox { x: rng_x, y: rng_y, w: rng_w, h: rng_h, hit: CombineHit::RangeBox(idx) });
                hits.push(CombineBox { x: rx, y: ry, w: row_w, h: rh, hit: CombineHit::SelectRow(idx) });
            }
        }

        // Preview sheet prev/next pills at bottom of right pane
        let nav_w = (160.0 * s).round().min(preview_w as f32 - 24.0) as u32;
        let nav_h = (26.0 * s).round() as u32;
        let nav_x = preview_x + ((preview_w.saturating_sub(nav_w)) / 2) as i32;
        let nav_y = my + mh as i32 - footer_h - (36.0 * s).round() as i32;
        let arrow_w = (28.0 * s).round() as u32;
        hits.push(CombineBox { x: nav_x, y: nav_y, w: arrow_w, h: nav_h, hit: CombineHit::PreviewPrev });
        hits.push(CombineBox { x: nav_x + nav_w as i32 - arrow_w as i32, y: nav_y, w: arrow_w, h: nav_h, hit: CombineHit::PreviewNext });

        // Footer buttons: Cancel & Combine PDFs
        let act_h = (32.0 * s).round() as u32;
        let act_y = my + mh as i32 - (44.0 * s).round() as i32;
        let cancel_w = (88.0 * s).round() as u32;
        let ok_w = (138.0 * s).round() as u32;
        let cancel_x = mx + mw as i32 - pad - cancel_w as i32;
        let ok_x = cancel_x - (10.0 * s).round() as i32 - ok_w as i32;
        hits.push(CombineBox { x: ok_x, y: act_y, w: ok_w, h: act_h, hit: CombineHit::CombineNow });
        hits.push(CombineBox { x: cancel_x, y: act_y, w: cancel_w, h: act_h, hit: CombineHit::Cancel });

        CombineLayout {
            mx,
            my,
            mw,
            mh,
            left_w,
            preview_x,
            preview_w,
            visible_rows,
            hits,
        }
    }

    pub(super) fn draw_combine_modal(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) {
        let Some(cm) = self.combine_modal.as_ref() else { return };
        let theme = self.ui_theme();
        let s = sf as f32;
        blend_rect_rgba(buffer, buf_w, buf_h, 0, 0, buf_w, buf_h, theme.modal_scrim);

        let layout = self.compute_combine_layout(buf_w, buf_h, sf);
        let rad = (10.0 * s).clamp(6.0, 16.0);
        let pad = (16.0 * s).round() as i32;
        let footer_h = (56.0 * s).round() as i32;

        draw_soft_shadow(
            buffer, buf_w, buf_h, layout.mx, layout.my, layout.mw, layout.mh, rad,
            (12.0 * s).round() as i32, (5.0 * s).round() as i32, theme.shadow_rgb, theme.shadow_max_alpha,
        );
        fill_rounded_rect(buffer, buf_w, buf_h, layout.mx, layout.my, layout.mw, layout.mh, rad, theme.surface_bg);
        // Right preview stage background (preserving top-right rounded corner, flush with left & footer dividers)
        let prev_h = layout.mh.saturating_sub(footer_h as u32);
        let r_seam = (rad.ceil() as u32).min(layout.preview_w).min(prev_h);
        fill_rounded_rect(
            buffer, buf_w, buf_h,
            layout.preview_x, layout.my, layout.preview_w, prev_h,
            rad, theme.canvas_bg,
        );
        fill_rect_clipped(
            buffer, buf_w, buf_h,
            layout.preview_x, layout.my + 1, r_seam, prev_h.saturating_sub(1),
            theme.canvas_bg,
        );
        fill_rect_clipped(
            buffer, buf_w, buf_h,
            layout.preview_x, layout.my + prev_h.saturating_sub(r_seam) as i32, layout.preview_w.saturating_sub(1), r_seam,
            theme.canvas_bg,
        );
        fill_rect_clipped(buffer, buf_w, buf_h, layout.preview_x, layout.my + 1, 1, prev_h.saturating_sub(1), theme.divider);
        fill_rect_clipped(buffer, buf_w, buf_h, layout.mx + 1, layout.my + layout.mh as i32 - footer_h, layout.mw.saturating_sub(2), 1, theme.divider);
        stroke_rounded_rect(buffer, buf_w, buf_h, layout.mx, layout.my, layout.mw, layout.mh, rad, theme.border_subtle);

        let title_sz = (14.5 * s).clamp(12.0, 30.0);
        let body_sz = (11.5 * s).clamp(10.0, 24.0);
        let small_sz = (10.5 * s).clamp(9.0, 20.0);
        let icon_sz = (12.0 * s).round().clamp(10.0, 22.0) as u32;

        draw_text(buffer, buf_w, buf_h, layout.mx + pad, layout.my + (14.0 * s).round() as i32, "Combine Multiple PDFs", title_sz, theme.text_primary, true);
        fill_rect_clipped(buffer, buf_w, buf_h, layout.mx + 1, layout.my + (46.0 * s).round() as i32, layout.left_w.saturating_sub(1), 1, theme.divider);

        let find_box = |hit: CombineHit| layout.hits.iter().copied().find(|b| b.hit == hit);

        // "+ Add PDFs..." button
        if let Some(b) = find_box(CombineHit::AddFiles) {
            let hov = self.hovered_widget == Some(UiHit::Combine(CombineHit::AddFiles));
            fill_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, (6.0 * s).max(4.0), if hov { theme.accent_hover } else { theme.accent });
            let lbl = "+ Add PDFs...";
            let lw = measure_text(lbl, body_sz, true);
            draw_text_vcenter(buffer, buf_w, buf_h, b.x + ((b.w as f32 - lw) * 0.5).round() as i32, b.y, b.h, b.x + b.w as i32, lbl, body_sz, theme.text_on_accent, true);
        }

        // Queue list or empty drop-zone hint
        if cm.entries.is_empty() {
            let cy = layout.my + (layout.mh as i32 - footer_h) / 2;
            let msg = "Click '+ Add PDFs...' or drop PDF files here";
            let mw_t = measure_text(msg, body_sz, false);
            draw_text(buffer, buf_w, buf_h, layout.mx + ((layout.left_w as f32 - mw_t) * 0.5).round() as i32, cy - 10, msg, body_sz, theme.text_muted, false);
        } else {
            for idx in cm.scroll_offset..cm.entries.len().min(cm.scroll_offset + layout.visible_rows) {
                let entry = &cm.entries[idx];
                let Some(row_b) = find_box(CombineHit::SelectRow(idx)) else { continue };
                let is_sel = idx == cm.selected;
                let bg = if is_sel { theme.accent_soft_bg } else { theme.surface_elevated };
                let bd = if is_sel { theme.accent } else { theme.border_subtle };
                fill_rounded_rect(buffer, buf_w, buf_h, row_b.x, row_b.y, row_b.w, row_b.h, (6.0 * s).max(4.0), bg);
                stroke_rounded_rect(buffer, buf_w, buf_h, row_b.x, row_b.y, row_b.w, row_b.h, (6.0 * s).max(4.0), bd);

                // Serial number + filename + page count
                let num_txt = format!("{}.", idx + 1);
                draw_text(buffer, buf_w, buf_h, row_b.x + (8.0 * s).round() as i32, row_b.y + (6.0 * s).round() as i32, &num_txt, body_sz, theme.accent_soft_fg, true);
                let rng_x = find_box(CombineHit::RangeBox(idx)).map(|b| b.x).unwrap_or(row_b.x + row_b.w as i32 - 100);
                let name_x = row_b.x + (26.0 * s).round() as i32;
                let name_budget = (rng_x - name_x - (8.0 * s).round() as i32).max(40) as f32;
                let fname_trunc = truncate_ellipsis(&entry.file_name, name_budget, body_sz, is_sel);
                draw_text(buffer, buf_w, buf_h, name_x, row_b.y + (6.0 * s).round() as i32, &fname_trunc, body_sz, theme.text_primary, is_sel);

                let sub = if let Some(ref err) = entry.range_error {
                    truncate_ellipsis(err, name_budget, small_sz, false)
                } else {
                    format!("{} pages  ·  using {}", entry.total_pages, entry.effective_count())
                };
                let sub_col = if entry.range_error.is_some() { theme.danger } else { theme.text_muted };
                draw_text(buffer, buf_w, buf_h, name_x, row_b.y + (23.0 * s).round() as i32, &sub, small_sz, sub_col, false);

                // Per-file page range box
                if let Some(rb) = find_box(CombineHit::RangeBox(idx)) {
                    let focused = is_sel && cm.editing_range;
                    let r_bd = if entry.range_error.is_some() {
                        theme.danger
                    } else if focused {
                        theme.accent
                    } else {
                        theme.input_border
                    };
                    fill_rounded_rect(buffer, buf_w, buf_h, rb.x, rb.y, rb.w, rb.h, (4.0 * s).max(3.0), theme.surface_bg);
                    stroke_rounded_rect(buffer, buf_w, buf_h, rb.x, rb.y, rb.w, rb.h, (4.0 * s).max(3.0), r_bd);
                    let display = if focused {
                        format!("{}|", entry.page_range)
                    } else if entry.page_range.trim().is_empty() {
                        "all".to_string()
                    } else {
                        entry.page_range.clone()
                    };
                    let clipped = truncate_ellipsis(&display, (rb.w as f32 - 10.0).max(16.0), small_sz, false);
                    draw_text_vcenter(buffer, buf_w, buf_h, rb.x + (5.0 * s).round() as i32, rb.y, rb.h, rb.x + rb.w as i32 - 3, &clipped, small_sz, theme.text_primary, false);
                }

                // Up / Down / Remove icon buttons
                for (hit, ic, danger) in [
                    (CombineHit::MoveUp(idx), UiIcon::MoveUp, false),
                    (CombineHit::MoveDown(idx), UiIcon::MoveDown, false),
                    (CombineHit::RemoveRow(idx), UiIcon::Close, true),
                ] {
                    if let Some(bb) = find_box(hit) {
                        let hov = self.hovered_widget == Some(UiHit::Combine(hit));
                        if hov {
                            fill_rounded_rect(
                                buffer, buf_w, buf_h, bb.x, bb.y, bb.w, bb.h, (4.0 * s).max(3.0),
                                if danger { theme.danger_soft_bg } else { theme.hover_bg },
                            );
                        }
                        draw_icon(
                            buffer, buf_w, buf_h, ic,
                            bb.x + ((bb.w.saturating_sub(icon_sz)) / 2) as i32,
                            bb.y + ((bb.h.saturating_sub(icon_sz)) / 2) as i32,
                            icon_sz,
                            if danger && hov { theme.danger } else { theme.text_secondary },
                        );
                    }
                }
            }
        }

        // Right preview pane
        let prev_top = layout.my + (16.0 * s).round() as i32;
        if let Some(entry) = cm.entries.get(cm.selected) {
            let hdr = truncate_ellipsis(&entry.file_name, (layout.preview_w as f32 - 32.0).max(60.0), body_sz, true);
            let hw = measure_text(&hdr, body_sz, true);
            draw_text(buffer, buf_w, buf_h, layout.preview_x + ((layout.preview_w as f32 - hw) * 0.5).round() as i32, prev_top, &hdr, body_sz, theme.text_primary, true);

            if let Some(ref bmp) = cm.cached_preview {
                let stage_y = prev_top + (24.0 * s).round() as i32;
                let stage_h = (layout.mh as i32 - footer_h - (84.0 * s).round() as i32).max(80) as u32;
                let stage_w = layout.preview_w.saturating_sub((32.0 * s).round() as u32).max(80);
                let sc = (stage_w as f32 / bmp.width.max(1) as f32)
                    .min(stage_h as f32 / bmp.height.max(1) as f32)
                    .min(1.0);
                let dw = (bmp.width as f32 * sc).round().max(1.0) as u32;
                let dh = (bmp.height as f32 * sc).round().max(1.0) as u32;
                let dx = layout.preview_x + ((layout.preview_w.saturating_sub(dw)) / 2) as i32;
                let dy = stage_y + ((stage_h.saturating_sub(dh)) / 2) as i32;
                draw_soft_shadow(
                    buffer, buf_w, buf_h, dx, dy, dw, dh, 2.0,
                    (8.0 * s).round() as i32, (3.0 * s).round() as i32, theme.shadow_rgb, theme.shadow_max_alpha,
                );
                fill_rect_clipped(buffer, buf_w, buf_h, dx, dy, dw, dh, 0x00FF_FFFF);
                for y in 0..dh {
                    let py = dy + y as i32;
                    if py < 0 || py >= buf_h as i32 {
                        continue;
                    }
                    let sy = ((y as u64 * bmp.height as u64) / dh.max(1) as u64).min(bmp.height.saturating_sub(1) as u64) as usize;
                    let d_row = (py as u32 * buf_w) as usize;
                    let s_row = sy * bmp.width as usize;
                    for x in 0..dw {
                        let px = dx + x as i32;
                        if px < 0 || px >= buf_w as i32 {
                            continue;
                        }
                        let sx = ((x as u64 * bmp.width as u64) / dw.max(1) as u64).min(bmp.width.saturating_sub(1) as u64) as usize;
                        buffer[d_row + px as usize] = bmp.pixels[s_row + sx];
                    }
                }
                stroke_rounded_rect(buffer, buf_w, buf_h, dx, dy, dw, dh, 1.0, theme.page_border);
            }

            // Preview page stepper bar
            if let (Some(pb), Some(nb)) = (find_box(CombineHit::PreviewPrev), find_box(CombineHit::PreviewNext)) {
                let pill_w = (nb.x + nb.w as i32 - pb.x) as u32;
                fill_rounded_rect(buffer, buf_w, buf_h, pb.x, pb.y, pill_w, pb.h, (6.0 * s).max(4.0), theme.surface_bg);
                stroke_rounded_rect(buffer, buf_w, buf_h, pb.x, pb.y, pill_w, pb.h, (6.0 * s).max(4.0), theme.border_subtle);
                draw_icon(buffer, buf_w, buf_h, UiIcon::ChevronLeft, pb.x + ((pb.w.saturating_sub(icon_sz)) / 2) as i32, pb.y + ((pb.h.saturating_sub(icon_sz)) / 2) as i32, icon_sz, theme.text_secondary);
                draw_icon(buffer, buf_w, buf_h, UiIcon::ChevronRight, nb.x + ((nb.w.saturating_sub(icon_sz)) / 2) as i32, nb.y + ((nb.h.saturating_sub(icon_sz)) / 2) as i32, icon_sz, theme.text_secondary);
                let plbl = format!("Page {} / {}", cm.preview_page + 1, entry.total_pages);
                let pw_t = measure_text(&plbl, small_sz, false);
                draw_text_vcenter(buffer, buf_w, buf_h, pb.x + ((pill_w as f32 - pw_t) * 0.5).round() as i32, pb.y, pb.h, nb.x, &plbl, small_sz, theme.text_primary, false);
            }
        } else {
            let cy = layout.my + (layout.mh as i32 - footer_h) / 2;
            let msg = "Select a file to preview";
            let mw_t = measure_text(msg, body_sz, false);
            draw_text(buffer, buf_w, buf_h, layout.preview_x + ((layout.preview_w as f32 - mw_t) * 0.5).round() as i32, cy - 8, msg, body_sz, theme.text_muted, false);
        }

        // Footer summary + Combine / Cancel buttons
        let summary = format!(
            "{} file(s)  ·  {} total pages  (click a file's Pages box to set e.g. 1-3, 5)",
            cm.entries.len(),
            cm.total_output_pages()
        );
        draw_text_vcenter(
            buffer, buf_w, buf_h,
            layout.mx + pad,
            layout.my + layout.mh as i32 - footer_h,
            footer_h as u32,
            layout.mx + layout.mw as i32 - (260.0 * s).round() as i32,
            &summary, small_sz, theme.text_secondary, false,
        );

        let can_combine = cm.entries.len() >= 1 && cm.total_output_pages() > 0 && !cm.has_errors();
        if let Some(b) = find_box(CombineHit::CombineNow) {
            let hov = self.hovered_widget == Some(UiHit::Combine(CombineHit::CombineNow));
            let bg = if !can_combine {
                theme.input_border
            } else if hov {
                theme.accent_hover
            } else {
                theme.accent
            };
            fill_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, (6.0 * s).max(4.0), bg);
            let lbl = "Combine PDFs";
            let lw = measure_text(lbl, body_sz, true);
            draw_text_vcenter(buffer, buf_w, buf_h, b.x + ((b.w as f32 - lw) * 0.5).round() as i32, b.y, b.h, b.x + b.w as i32, lbl, body_sz, theme.text_on_accent, true);
        }
        if let Some(b) = find_box(CombineHit::Cancel) {
            let hov = self.hovered_widget == Some(UiHit::Combine(CombineHit::Cancel));
            fill_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, (6.0 * s).max(4.0), if hov { theme.hover_bg } else { theme.surface_elevated });
            stroke_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, (6.0 * s).max(4.0), theme.border_subtle);
            let lbl = "Cancel";
            let lw = measure_text(lbl, body_sz, false);
            draw_text_vcenter(buffer, buf_w, buf_h, b.x + ((b.w as f32 - lw) * 0.5).round() as i32, b.y, b.h, b.x + b.w as i32, lbl, body_sz, theme.text_primary, false);
        }
    }

    pub(super) fn hit_test_combine_modal(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<CombineHit> {
        self.combine_modal.as_ref()?;
        let layout = self.compute_combine_layout(buf_w, buf_h, sf);
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix < layout.mx || ix >= layout.mx + layout.mw as i32 || iy < layout.my || iy >= layout.my + layout.mh as i32 {
            return None;
        }
        for b in &layout.hits {
            if b.contains(ix, iy) {
                return Some(b.hit);
            }
        }
        Some(CombineHit::InsideModal)
    }

    pub(super) fn handle_combine_click(&mut self, hit: CombineHit) {
        match hit {
            CombineHit::Cancel => {
                self.close_combine_modal();
                return;
            }
            CombineHit::AddFiles => {
                self.pending_dialog = Some(PendingDialog::CombineAddFiles);
                self.request_redraw();
                return;
            }
            CombineHit::CombineNow => {
                self.execute_combine_from_modal();
                return;
            }
            CombineHit::InsideModal => {
                if let Some(cm) = self.combine_modal.as_mut() {
                    cm.editing_range = false;
                }
            }
            CombineHit::SelectRow(idx) => {
                if let Some(cm) = self.combine_modal.as_mut() {
                    if idx < cm.entries.len() {
                        if cm.selected != idx {
                            cm.selected = idx;
                            cm.preview_page = 0;
                            cm.cached_preview = None;
                        }
                        cm.editing_range = false;
                    }
                }
            }
            CombineHit::RangeBox(idx) => {
                if let Some(cm) = self.combine_modal.as_mut() {
                    if idx < cm.entries.len() {
                        if cm.selected != idx {
                            cm.selected = idx;
                            cm.preview_page = 0;
                            cm.cached_preview = None;
                        }
                        cm.editing_range = true;
                        if cm.entries[idx].page_range.eq_ignore_ascii_case("all") {
                            cm.entries[idx].page_range.clear();
                        }
                    }
                }
            }
            CombineHit::MoveUp(idx) => {
                if let Some(cm) = self.combine_modal.as_mut() {
                    if idx > 0 && idx < cm.entries.len() {
                        cm.entries.swap(idx, idx - 1);
                        cm.selected = idx - 1;
                    }
                }
            }
            CombineHit::MoveDown(idx) => {
                if let Some(cm) = self.combine_modal.as_mut() {
                    if idx + 1 < cm.entries.len() {
                        cm.entries.swap(idx, idx + 1);
                        cm.selected = idx + 1;
                    }
                }
            }
            CombineHit::RemoveRow(idx) => {
                if let Some(cm) = self.combine_modal.as_mut() {
                    if idx < cm.entries.len() {
                        cm.entries.remove(idx);
                        if cm.selected >= cm.entries.len() {
                            cm.selected = cm.entries.len().saturating_sub(1);
                        }
                        cm.preview_page = 0;
                        cm.cached_preview = None;
                        cm.editing_range = false;
                    }
                }
            }
            CombineHit::PreviewPrev => {
                if let Some(cm) = self.combine_modal.as_mut() {
                    if cm.preview_page > 0 {
                        cm.preview_page -= 1;
                        cm.cached_preview = None;
                    }
                }
            }
            CombineHit::PreviewNext => {
                if let Some(cm) = self.combine_modal.as_mut() {
                    let max_p = cm.entries.get(cm.selected).map(|e| e.total_pages.saturating_sub(1)).unwrap_or(0);
                    if cm.preview_page < max_p {
                        cm.preview_page += 1;
                        cm.cached_preview = None;
                    }
                }
            }
        }
        self.request_redraw();
    }

    pub(super) fn handle_combine_key(&mut self, logical_key: winit::keyboard::Key, text: Option<&str>) {
        use winit::keyboard::{Key, NamedKey};
        match logical_key {
            Key::Named(NamedKey::Escape) => {
                self.close_combine_modal();
            }
            Key::Named(NamedKey::Enter) => {
                let editing = self.combine_modal.as_ref().map(|c| c.editing_range).unwrap_or(false);
                if editing {
                    if let Some(cm) = self.combine_modal.as_mut() {
                        cm.editing_range = false;
                        if let Some(e) = cm.entries.get_mut(cm.selected) {
                            if e.page_range.trim().is_empty() {
                                e.page_range = "all".to_string();
                            }
                            e.validate();
                        }
                    }
                    self.request_redraw();
                } else {
                    self.execute_combine_from_modal();
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                if let Some(cm) = self.combine_modal.as_mut() {
                    if cm.selected > 0 {
                        cm.selected -= 1;
                        cm.preview_page = 0;
                        cm.cached_preview = None;
                        if cm.selected < cm.scroll_offset {
                            cm.scroll_offset = cm.selected;
                        }
                    }
                }
                self.request_redraw();
            }
            Key::Named(NamedKey::ArrowDown) => {
                if let Some(cm) = self.combine_modal.as_mut() {
                    if cm.selected + 1 < cm.entries.len() {
                        cm.selected += 1;
                        cm.preview_page = 0;
                        cm.cached_preview = None;
                    }
                }
                self.request_redraw();
            }
            Key::Named(NamedKey::Backspace) => {
                if let Some(cm) = self.combine_modal.as_mut() {
                    if let Some(e) = cm.entries.get_mut(cm.selected) {
                        cm.editing_range = true;
                        e.page_range.pop();
                        e.validate();
                    }
                }
                self.request_redraw();
            }
            _ => {
                let s_in = text.or_else(|| match &logical_key {
                    Key::Character(c) => Some(c.as_str()),
                    _ => None,
                });
                if let Some(typed) = s_in {
                    if let Some(cm) = self.combine_modal.as_mut() {
                        if let Some(e) = cm.entries.get_mut(cm.selected) {
                            let mut changed = false;
                            for c in typed.chars() {
                                if c.is_ascii_digit() || c == '-' || c == ',' || c == ' ' {
                                    if !cm.editing_range && e.page_range.eq_ignore_ascii_case("all") {
                                        e.page_range.clear();
                                    }
                                    cm.editing_range = true;
                                    e.page_range.push(c);
                                    changed = true;
                                }
                            }
                            if changed {
                                e.validate();
                                self.request_redraw();
                            }
                        }
                    }
                }
            }
        }
    }

    pub(super) fn execute_combine_from_modal(&mut self) {
        let Some(cm) = self.combine_modal.as_ref() else { return };
        if cm.entries.is_empty() {
            self.status_notice = Some("Add at least 1 PDF to combine".to_string());
            self.request_redraw();
            return;
        }
        let mut plan: Vec<(PathBuf, Option<Vec<u16>>)> = Vec::with_capacity(cm.entries.len());
        for entry in &cm.entries {
            match entry.resolved_pages() {
                Ok(opt_pages) => plan.push((entry.path.clone(), opt_pages)),
                Err(e) => {
                    self.status_notice = Some(format!("Invalid range for {}: {e}", entry.file_name));
                    self.request_redraw();
                    return;
                }
            }
        }
        let file_count = plan.len();
        let first_dir = plan[0].0.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        self.close_combine_modal();

        match self.engine.combine_pdfs_with_ranges(&plan) {
            Ok((combined_bytes, total_pages)) => {
                let virtual_path = first_dir.join("Combined.pdf");
                match self.open_bytes_in_new_tab(combined_bytes, virtual_path) {
                    Ok(()) => {
                        self.has_unsaved_edits = true;
                        self.status_notice = Some(format!(
                            "Combined {file_count} PDF(s) ({total_pages} pages) — use Save As (Ctrl+Shift+S) to save"
                        ));
                    }
                    Err(e) => {
                        self.status_notice = Some(format!("Combine failed to open result: {e}"));
                    }
                }
            }
            Err(e) => {
                self.status_notice = Some(format!("Combine failed: {e}"));
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }
}

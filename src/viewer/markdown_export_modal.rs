//! Compact 720p Export / Print Modal for `.md` / `.markdown` tabs.
//!
//! Zero idle RAM when closed: allocates nothing until `Export / Print` is clicked.

use crate::markdown_docx::export_markdown_to_docx;
use crate::markdown_pdf::{export_markdown_to_pdf, MdPdfPageSize};
use super::render::blend_rect_rgba;
use super::state::*;
use super::ui_draw::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum MarkdownExportHit {
    PageSizeA4,
    PageSizeLetter,
    ToggleOpenInNewTab,
    ExportPdf,
    ExportDocx,
    Cancel,
    InsideModal,
}

#[derive(Clone, Debug)]
pub(super) struct MarkdownExportModalState {
    pub(super) page_size: MdPdfPageSize,
    pub(super) open_in_new_tab: bool,
    pub(super) status_msg: Option<(String, bool)>,
}

impl Default for MarkdownExportModalState {
    fn default() -> Self {
        Self {
            page_size: MdPdfPageSize::A4,
            open_in_new_tab: true,
            status_msg: None,
        }
    }
}

#[derive(Clone, Copy)]
struct MdExpBox {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    hit: MarkdownExportHit,
}

impl MdExpBox {
    fn contains(self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w as i32 && py >= self.y && py < self.y + self.h as i32
    }
}

impl<'a> PdfViewerApp<'a> {
    pub(super) fn open_markdown_export_modal(&mut self) {
        if !self.is_markdown_tab() {
            return;
        }
        self.close_all_popovers();
        self.markdown_export_modal = Some(MarkdownExportModalState::default());
        self.request_redraw();
    }

    fn layout_markdown_export_modal(
        &self,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> (i32, i32, u32, u32, Vec<MdExpBox>) {
        let s = sf as f32;
        let mw = ((420.0 * s).round() as u32).min(buf_w.saturating_sub(24));
        let mh = ((258.0 * s).round() as u32).min(buf_h.saturating_sub(24));
        let mx = (buf_w.saturating_sub(mw) / 2) as i32;
        let my = (buf_h.saturating_sub(mh) / 2) as i32;
        let p = (16.0 * s).round() as i32;
        let ix = mx + p;
        let iw = mw.saturating_sub((p * 2) as u32);
        let row_h = (28.0 * s).round() as u32;
        let gap = (8.0 * s).round() as i32;
        let mut hits = Vec::with_capacity(8);

        let mut cy = my + (56.0 * s).round() as i32;
        let half_w = (iw.saturating_sub(gap as u32)) / 2;
        hits.push(MdExpBox { x: ix, y: cy, w: half_w, h: row_h, hit: MarkdownExportHit::PageSizeA4 });
        hits.push(MdExpBox { x: ix + half_w as i32 + gap, y: cy, w: iw - half_w - gap as u32, h: row_h, hit: MarkdownExportHit::PageSizeLetter });
        cy += row_h as i32 + (14.0 * s).round() as i32;

        hits.push(MdExpBox { x: ix, y: cy, w: iw, h: row_h, hit: MarkdownExportHit::ToggleOpenInNewTab });
        cy += row_h as i32 + (16.0 * s).round() as i32;

        let btn_h = (32.0 * s).round() as u32;
        hits.push(MdExpBox { x: ix, y: cy, w: half_w, h: btn_h, hit: MarkdownExportHit::ExportPdf });
        hits.push(MdExpBox { x: ix + half_w as i32 + gap, y: cy, w: iw - half_w - gap as u32, h: btn_h, hit: MarkdownExportHit::ExportDocx });

        let cancel_w = (90.0 * s).round() as u32;
        let cancel_y = my + mh as i32 - p - (28.0 * s).round() as i32;
        hits.push(MdExpBox { x: ix + iw as i32 - cancel_w as i32, y: cancel_y, w: cancel_w, h: (28.0 * s).round() as u32, hit: MarkdownExportHit::Cancel });

        (mx, my, mw, mh, hits)
    }

    pub(super) fn hit_test_markdown_export_modal(
        &self,
        px: i32,
        py: i32,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<MarkdownExportHit> {
        self.markdown_export_modal.as_ref()?;
        let (mx, my, mw, mh, hits) = self.layout_markdown_export_modal(buf_w, buf_h, sf);
        for h in &hits {
            if h.contains(px, py) {
                return Some(h.hit);
            }
        }
        if px >= mx && px < mx + mw as i32 && py >= my && py < my + mh as i32 {
            Some(MarkdownExportHit::InsideModal)
        } else {
            None
        }
    }

    pub(super) fn draw_markdown_export_modal(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) {
        let Some(ref st) = self.markdown_export_modal else {
            return;
        };
        let theme = self.ui_theme();
        let s = sf as f32;
        blend_rect_rgba(buffer, buf_w, buf_h, 0, 0, buf_w, buf_h, theme.modal_scrim);

        let (mx, my, mw, mh, hits) = self.layout_markdown_export_modal(buf_w, buf_h, sf);
        let rad = 10.0 * s;
        draw_soft_shadow(buffer, buf_w, buf_h, mx, my, mw, mh, rad, (16.0 * s).round() as i32, (6.0 * s).round() as i32, theme.shadow_rgb, theme.shadow_max_alpha);
        fill_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.surface_elevated);
        stroke_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.border_subtle);

        let p = (16.0 * s).round() as i32;
        draw_text(buffer, buf_w, buf_h, mx + p, my + p, "Export / Print Markdown", 14.0 * s, theme.text_primary, true);
        draw_text(buffer, buf_w, buf_h, mx + p, my + (40.0 * s).round() as i32, "PDF Page Size", 10.5 * s, theme.text_muted, true);

        let find = |target: MarkdownExportHit| hits.iter().find(|b| b.hit == target).copied();
        let draw_btn = |buf: &mut [u32], b: MdExpBox, label: &str, active: bool, primary: bool| {
            let hov = self.hovered_widget == Some(UiHit::MarkdownExport(b.hit));
            let bg = if primary {
                if hov { theme.accent_hover } else { theme.accent }
            } else if active {
                theme.accent_soft_bg
            } else if hov {
                theme.hover_bg
            } else {
                theme.input_bg
            };
            let fg = if primary {
                theme.text_on_accent
            } else if active {
                theme.accent_soft_fg
            } else {
                theme.text_primary
            };
            fill_rounded_rect(buf, buf_w, buf_h, b.x, b.y, b.w, b.h, 5.0 * s, bg);
            if !primary {
                stroke_rounded_rect(buf, buf_w, buf_h, b.x, b.y, b.w, b.h, 5.0 * s, if active { theme.accent } else { theme.border_subtle });
            }
            let fs = 11.5 * s;
            let tw = measure_text(label, fs, active || primary);
            let tx = b.x + ((b.w as f32 - tw) * 0.5).round() as i32;
            draw_text_vcenter(buf, buf_w, buf_h, tx, b.y, b.h, b.x + b.w as i32 - 4, label, fs, fg, active || primary);
        };

        if let Some(b) = find(MarkdownExportHit::PageSizeA4) {
            draw_btn(buffer, b, "A4 (Default)", st.page_size == MdPdfPageSize::A4, false);
        }
        if let Some(b) = find(MarkdownExportHit::PageSizeLetter) {
            draw_btn(buffer, b, "Letter", st.page_size == MdPdfPageSize::Letter, false);
        }
        if let Some(b) = find(MarkdownExportHit::ToggleOpenInNewTab) {
            let chk = if st.open_in_new_tab { "[x]  Open exported PDF in new tab" } else { "[ ]  Open exported PDF in new tab" };
            draw_btn(buffer, b, chk, st.open_in_new_tab, false);
        }
        if let Some(b) = find(MarkdownExportHit::ExportPdf) {
            draw_btn(buffer, b, "Export Styled PDF...", false, true);
        }
        if let Some(b) = find(MarkdownExportHit::ExportDocx) {
            draw_btn(buffer, b, "Export Word (.docx)...", false, false);
        }
        if let Some((ref msg, is_err)) = st.status_msg {
            let col = if is_err { theme.danger } else { theme.success };
            let clipped = truncate_ellipsis(msg, (mw as i32 - p * 2 - 96).max(60) as f32, 10.5 * s, true);
            draw_text(buffer, buf_w, buf_h, mx + p, my + mh as i32 - p - (20.0 * s).round() as i32, &clipped, 10.5 * s, col, true);
        }
        if let Some(b) = find(MarkdownExportHit::Cancel) {
            draw_btn(buffer, b, "Cancel", false, false);
        }
    }

    pub(super) fn handle_markdown_export_click(&mut self, hit: MarkdownExportHit) {
        match hit {
            MarkdownExportHit::InsideModal => {}
            MarkdownExportHit::Cancel => {
                self.markdown_export_modal = None;
                self.request_redraw();
            }
            MarkdownExportHit::PageSizeA4 => {
                if let Some(m) = self.markdown_export_modal.as_mut() {
                    m.page_size = MdPdfPageSize::A4;
                }
                self.request_redraw();
            }
            MarkdownExportHit::PageSizeLetter => {
                if let Some(m) = self.markdown_export_modal.as_mut() {
                    m.page_size = MdPdfPageSize::Letter;
                }
                self.request_redraw();
            }
            MarkdownExportHit::ToggleOpenInNewTab => {
                if let Some(m) = self.markdown_export_modal.as_mut() {
                    m.open_in_new_tab = !m.open_in_new_tab;
                }
                self.request_redraw();
            }
            MarkdownExportHit::ExportPdf => {
                self.run_markdown_pdf_export();
            }
            MarkdownExportHit::ExportDocx => {
                self.run_markdown_docx_export();
            }
        }
    }

    fn run_markdown_docx_export(&mut self) {
        let Some(ref md) = self.markdown_doc else { return };
        let stem = self.file_path.file_stem().and_then(|s| s.to_str()).unwrap_or("document");
        let default_name = format!("{stem}.docx");
        let mut dlg = rfd::FileDialog::new()
            .set_title("Export Markdown to Styled Word (.docx)")
            .add_filter("Word Document", &["docx"])
            .set_file_name(&default_name);
        if let Some(parent) = self.file_path.parent() {
            if !parent.as_os_str().is_empty() && parent.exists() {
                dlg = dlg.set_directory(parent);
            }
        }
        let Some(save_path) = dlg.save_file() else {
            return;
        };

        match export_markdown_to_docx(md, &save_path) {
            Ok(()) => {
                self.markdown_export_modal = None;
                let fname = save_path.file_name().and_then(|s| s.to_str()).unwrap_or(&default_name).to_string();
                self.status_notice = Some(format!("Exported Word document '{fname}'"));
                self.update_title_bar();
                self.request_redraw();
            }
            Err(e) => {
                if let Some(m) = self.markdown_export_modal.as_mut() {
                    m.status_msg = Some((format!("Word export failed: {e}"), true));
                }
                self.request_redraw();
            }
        }
    }

    fn run_markdown_pdf_export(&mut self) {
        let Some(ref md) = self.markdown_doc else { return };
        let Some(ref modal) = self.markdown_export_modal else { return };
        let page_size = modal.page_size;
        let open_new = modal.open_in_new_tab;

        let stem = self.file_path.file_stem().and_then(|s| s.to_str()).unwrap_or("document");
        let default_name = format!("{stem}.pdf");
        let mut dlg = rfd::FileDialog::new()
            .set_title("Export Markdown to Styled PDF")
            .add_filter("PDF Document", &["pdf"])
            .set_file_name(&default_name);
        if let Some(parent) = self.file_path.parent() {
            if !parent.as_os_str().is_empty() && parent.exists() {
                dlg = dlg.set_directory(parent);
            }
        }
        let Some(save_path) = dlg.save_file() else {
            return;
        };

        match export_markdown_to_pdf(&self.engine.pdfium, md, &save_path, page_size) {
            Ok(()) => {
                self.markdown_export_modal = None;
                let fname = save_path.file_name().and_then(|s| s.to_str()).unwrap_or(&default_name).to_string();
                if open_new {
                    let _ = self.open_document_in_new_tab(save_path);
                }
                self.status_notice = Some(format!("Exported styled PDF '{fname}'"));
                self.update_title_bar();
                self.request_redraw();
            }
            Err(e) => {
                if let Some(m) = self.markdown_export_modal.as_mut() {
                    m.status_msg = Some((format!("PDF export failed: {e}"), true));
                }
                self.request_redraw();
            }
        }
    }
}

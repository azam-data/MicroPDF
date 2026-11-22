//! Milestone 1.2: Split PDF to Single-Page PDFs (Folder Export) Modal.
//!
//! Mirrors the Export Pages as Images UX: choose All Pages / Current Page / Custom Range,
//! customize the filename prefix (`<prefix>_page_001.pdf`), and pick a target folder for
//! lossless PDFium single-page export (zero rasterization, 0 bytes RAM when closed).

use super::deskew::parse_deskew_page_range;
use super::render::{blend_rect_rgba, fill_rect_clipped};
use super::state::*;
use super::ui_draw::*;
use crate::engine::PdfEngine;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum SplitPdfScope {
    All,
    Current,
    Custom,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum SplitPdfFocus {
    None,
    CustomRange,
    Prefix,
}

#[derive(Clone, Debug)]
pub(super) struct SplitPagesModalState {
    pub(super) scope: SplitPdfScope,
    pub(super) custom_range: String,
    pub(super) prefix: String,
    pub(super) focus: SplitPdfFocus,
}

impl SplitPagesModalState {
    pub(super) fn new(default_prefix: String) -> Self {
        Self {
            scope: SplitPdfScope::All,
            custom_range: String::new(),
            prefix: if default_prefix.trim().is_empty() {
                "document".to_string()
            } else {
                default_prefix
            },
            focus: SplitPdfFocus::None,
        }
    }

    pub(super) fn resolved_pages(&self, current_page: u16, total_pages: u16) -> Result<Vec<u16>, String> {
        if total_pages == 0 {
            return Err("No pages".to_string());
        }
        match self.scope {
            SplitPdfScope::All => Ok((0..total_pages).collect()),
            SplitPdfScope::Current => Ok(vec![current_page.min(total_pages - 1)]),
            SplitPdfScope::Custom => parse_deskew_page_range(&self.custom_range, total_pages)
                .ok_or_else(|| format!("Enter range 1-{total_pages} (e.g. 1-3, 5)")),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum SplitPagesHit {
    Close,
    Scope(SplitPdfScope),
    CustomRangeBox,
    PrefixBox,
    ExportNow,
    InsideModal,
}

impl<'a> PdfViewerApp<'a> {
    pub(super) fn open_split_pages_modal(&mut self) {
        if self.doc.is_none() {
            self.status_notice = Some("Open a PDF first to split into single-page PDFs".to_string());
            self.request_redraw();
            return;
        }
        self.close_all_popovers();
        let stem = self
            .file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document")
            .to_string();
        self.split_pages_modal = Some(SplitPagesModalState::new(stem));
        self.request_redraw();
    }

    fn split_pages_layout(
        &self,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<(i32, i32, u32, u32, Vec<(i32, i32, u32, u32, SplitPagesHit)>)> {
        let _st = self.split_pages_modal.as_ref()?;
        let s = sf as f32;
        let mw = ((476.0 * s).round() as u32).min(buf_w.saturating_sub(28)).max(340);
        let mh = ((318.0 * s).round() as u32).min(buf_h.saturating_sub(28)).max(270);
        let mx = ((buf_w.saturating_sub(mw)) / 2) as i32;
        let my = ((buf_h.saturating_sub(mh)) / 2) as i32;

        let pad = (16.0 * s).round() as i32;
        let hdr_h = (44.0 * s).round() as u32;
        let footer_h = (52.0 * s).round() as u32;
        let btn_h = (28.0 * s).round() as u32;
        let gap = (8.0 * s).round() as i32;
        let iw = mw.saturating_sub((pad * 2) as u32);
        let mut hits = Vec::with_capacity(8);

        let close_sz = (24.0 * s).round() as u32;
        hits.push((
            mx + mw as i32 - pad - close_sz as i32,
            my + ((hdr_h.saturating_sub(close_sz)) / 2) as i32,
            close_sz,
            close_sz,
            SplitPagesHit::Close,
        ));

        // Scope row
        let mut cy = my + hdr_h as i32 + (28.0 * s).round() as i32;
        let sc_w = (iw.saturating_sub((gap * 2) as u32)) / 3;
        for (i, scope) in [
            SplitPdfScope::All,
            SplitPdfScope::Current,
            SplitPdfScope::Custom,
        ]
        .iter()
        .enumerate()
        {
            hits.push((
                mx + pad + (i as i32) * (sc_w as i32 + gap),
                cy,
                sc_w,
                btn_h,
                SplitPagesHit::Scope(*scope),
            ));
        }

        // Custom range box
        cy += btn_h as i32 + (10.0 * s).round() as i32;
        hits.push((mx + pad, cy, iw, btn_h, SplitPagesHit::CustomRangeBox));

        // Filename prefix box
        cy += btn_h as i32 + (30.0 * s).round() as i32;
        hits.push((mx + pad, cy, iw, btn_h, SplitPagesHit::PrefixBox));

        // Footer buttons
        let fy = my + mh as i32 - footer_h as i32 + ((footer_h.saturating_sub(btn_h)) / 2) as i32;
        let exp_w = (196.0 * s).round() as u32;
        let cancel_w = (82.0 * s).round() as u32;
        let exp_x = mx + mw as i32 - pad - exp_w as i32;
        let cancel_x = exp_x - cancel_w as i32 - gap;
        hits.push((cancel_x, fy, cancel_w, btn_h, SplitPagesHit::Close));
        hits.push((exp_x, fy, exp_w, btn_h, SplitPagesHit::ExportNow));

        Some((mx, my, mw, mh, hits))
    }

    pub(super) fn draw_split_pages_modal(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) {
        let Some(st) = self.split_pages_modal.as_ref() else {
            return;
        };
        let Some((mx, my, mw, mh, hits)) = self.split_pages_layout(buf_w, buf_h, sf) else {
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

        draw_text_vcenter(
            buffer,
            buf_w,
            buf_h,
            mx + pad,
            my,
            hdr_h,
            mx + mw as i32 - 48,
            "Split to Single-Page PDFs (Lossless)",
            title_sz,
            theme.text_primary,
            true,
        );
        fill_rect_clipped(buffer, buf_w, buf_h, mx, my + hdr_h as i32, mw, 1, theme.divider);

        let scope_y = my + hdr_h as i32 + (28.0 * s).round() as i32;
        draw_text(buffer, buf_w, buf_h, mx + pad, scope_y - (17.0 * s).round() as i32, "Pages to Split:", sub_sz, theme.text_secondary, true);

        let prefix_y = scope_y + btn_h as i32 * 2 + (40.0 * s).round() as i32;
        draw_text(buffer, buf_w, buf_h, mx + pad, prefix_y - (17.0 * s).round() as i32, "Filename Prefix:", sub_sz, theme.text_secondary, true);

        let sample_name = PdfEngine::format_single_page_pdf_name(&st.prefix, 1, self.total_pages().max(1));
        let preview_lbl = format!("Example output: {sample_name}");
        draw_text(
            buffer,
            buf_w,
            buf_h,
            mx + pad,
            prefix_y + btn_h as i32 + (7.0 * s).round() as i32,
            &preview_lbl,
            sub_sz,
            theme.text_muted,
            false,
        );

        let footer_y = my + mh as i32 - footer_h as i32;
        fill_rect_clipped(buffer, buf_w, buf_h, mx, footer_y, mw, 1, theme.divider);
        let resolved = st.resolved_pages(self.current_page_index, self.total_pages());
        let status_txt = match &resolved {
            Ok(pages) => format!("{} PDF file(s)", pages.len()),
            Err(e) => e.clone(),
        };
        let st_color = if resolved.is_ok() { theme.text_secondary } else { theme.danger };
        draw_text_vcenter(buffer, buf_w, buf_h, mx + pad, footer_y, footer_h, mx + mw as i32 - (296.0 * s).round() as i32, &status_txt, sub_sz, st_color, false);

        for &(bx, by, bw, bh, hit) in &hits {
            let hov = self.hovered_widget == Some(UiHit::SplitPages(hit));
            match hit {
                SplitPagesHit::Close if by < my + hdr_h as i32 => {
                    if hov {
                        fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, theme.hover_bg);
                    }
                    let tw = measure_text("x", body_sz, true).round() as i32;
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + (bw as i32 - tw) / 2, by, bh, bx + bw as i32, "x", body_sz, theme.text_secondary, true);
                }
                SplitPagesHit::Scope(sc) => {
                    let active = st.scope == sc;
                    let bg = if active { theme.accent } else if hov { theme.hover_bg } else { theme.surface_elevated };
                    let fg = if active { theme.text_on_accent } else { theme.text_primary };
                    let bdr = if active { theme.accent } else { theme.border_subtle };
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, bg);
                    stroke_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, bdr);
                    let lbl = match sc {
                        SplitPdfScope::All => "All Pages",
                        SplitPdfScope::Current => "Current Page",
                        SplitPdfScope::Custom => "Custom Range",
                    };
                    let tw = measure_text(lbl, body_sz, active);
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round() as i32, by, bh, bx + bw as i32, lbl, body_sz, fg, active);
                }
                SplitPagesHit::CustomRangeBox => {
                    let focused = st.focus == SplitPdfFocus::CustomRange;
                    let enabled = st.scope == SplitPdfScope::Custom;
                    let bg = if enabled { theme.input_bg } else { theme.surface_elevated };
                    let bdr = if focused { theme.accent } else { theme.border_subtle };
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, bg);
                    stroke_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, bdr);
                    let txt = if st.custom_range.is_empty() {
                        if focused { "|" } else { "e.g. 1-5, 8, 12-15 (optional — leave on All Pages for full PDF)" }
                    } else if focused {
                        &st.custom_range
                    } else {
                        &st.custom_range
                    };
                    let display = if focused && !st.custom_range.is_empty() {
                        format!("{}|", st.custom_range)
                    } else {
                        txt.to_string()
                    };
                    let fg = if st.custom_range.is_empty() && !focused {
                        theme.text_muted
                    } else if enabled {
                        theme.text_primary
                    } else {
                        theme.text_muted
                    };
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + 10, by, bh, bx + bw as i32 - 8, &display, body_sz, fg, false);
                }
                SplitPagesHit::PrefixBox => {
                    let focused = st.focus == SplitPdfFocus::Prefix;
                    let bdr = if focused { theme.accent } else { theme.border_subtle };
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, theme.input_bg);
                    stroke_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, bdr);
                    let display = if focused {
                        format!("{}|", st.prefix)
                    } else if st.prefix.is_empty() {
                        "document".to_string()
                    } else {
                        st.prefix.clone()
                    };
                    let fg = if st.prefix.is_empty() && !focused {
                        theme.text_muted
                    } else {
                        theme.text_primary
                    };
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + 10, by, bh, bx + bw as i32 - 8, &display, body_sz, fg, false);
                }
                SplitPagesHit::Close => {
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, if hov { theme.hover_bg } else { theme.surface_elevated });
                    stroke_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, theme.border_subtle);
                    let tw = measure_text("Cancel", body_sz, false);
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round() as i32, by, bh, bx + bw as i32, "Cancel", body_sz, theme.text_primary, false);
                }
                SplitPagesHit::ExportNow => {
                    let enabled = resolved.is_ok();
                    let bg = if !enabled { theme.surface_elevated } else if hov { theme.accent_hover } else { theme.accent };
                    let fg = if !enabled { theme.text_muted } else { theme.text_on_accent };
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, bg);
                    let lbl = "Choose Folder & Split...";
                    let tw = measure_text(lbl, body_sz, true);
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round() as i32, by, bh, bx + bw as i32, lbl, body_sz, fg, true);
                }
                SplitPagesHit::InsideModal => {}
            }
        }
    }

    pub(super) fn hit_test_split_pages_modal(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<SplitPagesHit> {
        let (x, y, w, h, hits) = self.split_pages_layout(buf_w, buf_h, sf)?;
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix < x || ix >= x + w as i32 || iy < y || iy >= y + h as i32 {
            return None;
        }
        for (bx, by, bw, bh, hit) in hits {
            if ix >= bx && ix < bx + bw as i32 && iy >= by && iy < by + bh as i32 {
                return Some(hit);
            }
        }
        Some(SplitPagesHit::InsideModal)
    }

    pub(super) fn handle_split_pages_click(&mut self, hit: SplitPagesHit) {
        match hit {
            SplitPagesHit::Close => {
                self.split_pages_modal = None;
                self.request_redraw();
            }
            SplitPagesHit::Scope(sc) => {
                if let Some(ref mut st) = self.split_pages_modal {
                    st.scope = sc;
                    st.focus = if sc == SplitPdfScope::Custom {
                        SplitPdfFocus::CustomRange
                    } else {
                        SplitPdfFocus::None
                    };
                }
                self.request_redraw();
            }
            SplitPagesHit::CustomRangeBox => {
                if let Some(ref mut st) = self.split_pages_modal {
                    st.scope = SplitPdfScope::Custom;
                    st.focus = SplitPdfFocus::CustomRange;
                }
                self.request_redraw();
            }
            SplitPagesHit::PrefixBox => {
                if let Some(ref mut st) = self.split_pages_modal {
                    st.focus = SplitPdfFocus::Prefix;
                }
                self.request_redraw();
            }
            SplitPagesHit::ExportNow => {
                let valid = self
                    .split_pages_modal
                    .as_ref()
                    .is_some_and(|st| st.resolved_pages(self.current_page_index, self.total_pages()).is_ok());
                if valid {
                    self.pending_dialog = Some(PendingDialog::SplitSinglePagesDir);
                    self.request_redraw();
                }
            }
            SplitPagesHit::InsideModal => {
                if let Some(ref mut st) = self.split_pages_modal {
                    st.focus = SplitPdfFocus::None;
                }
                self.request_redraw();
            }
        }
    }

    pub(super) fn handle_split_pages_key(&mut self, key: &winit::keyboard::Key, text: Option<&str>) {
        use winit::keyboard::{Key, NamedKey};
        match key {
            Key::Named(NamedKey::Escape) => {
                self.split_pages_modal = None;
                self.request_redraw();
            }
            Key::Named(NamedKey::Enter) => {
                self.handle_split_pages_click(SplitPagesHit::ExportNow);
            }
            Key::Named(NamedKey::Backspace) => {
                if let Some(ref mut st) = self.split_pages_modal {
                    match st.focus {
                        SplitPdfFocus::CustomRange => {
                            st.custom_range.pop();
                        }
                        SplitPdfFocus::Prefix => {
                            st.prefix.pop();
                        }
                        SplitPdfFocus::None => {}
                    }
                }
                self.request_redraw();
            }
            _ => {
                if let (Some(t), Some(st)) = (text, self.split_pages_modal.as_mut()) {
                    match st.focus {
                        SplitPdfFocus::CustomRange => {
                            for ch in t.chars() {
                                if (ch.is_ascii_digit() || matches!(ch, '-' | ',' | ' '))
                                    && st.custom_range.len() < 40
                                {
                                    st.custom_range.push(ch);
                                }
                            }
                        }
                        SplitPdfFocus::Prefix => {
                            for ch in t.chars() {
                                if !ch.is_control()
                                    && !matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
                                    && st.prefix.len() < 48
                                {
                                    st.prefix.push(ch);
                                }
                            }
                        }
                        SplitPdfFocus::None => {}
                    }
                    self.request_redraw();
                }
            }
        }
    }

    pub(super) fn execute_split_single_pages_dialog(&mut self) {
        let Some(st) = self.split_pages_modal.clone() else {
            return;
        };
        let pages = match st.resolved_pages(self.current_page_index, self.total_pages()) {
            Ok(p) => p,
            Err(e) => {
                self.status_notice = Some(e);
                self.request_redraw();
                return;
            }
        };
        let mut dialog = rfd::FileDialog::new()
            .set_title("Choose Folder for Single-Page PDFs");
        if let Some(parent) = self.file_path.parent() {
            if !parent.as_os_str().is_empty() && parent.exists() {
                dialog = dialog.set_directory(parent);
            }
        }
        let Some(out_dir) = dialog.pick_folder() else {
            return;
        };
        self.split_pages_modal = None;
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let prefix = st.prefix.clone();
        let win_opt = self.window.clone();
        match self.engine.split_doc_to_single_page_pdfs(doc, &pages, &out_dir, &prefix, |done, total| {
            if let Some(w) = win_opt.as_ref() {
                w.set_title(&format!("MicroPDF — Splitting page {done}/{total}..."));
            }
        }) {
            Ok(count) => {
                self.status_notice = Some(format!(
                    "Split {count} lossless single-page PDF(s) into '{}'",
                    out_dir.display()
                ));
            }
            Err(e) => {
                self.status_notice = Some(format!("Split to Single-Page PDFs failed: {e}"));
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }
}

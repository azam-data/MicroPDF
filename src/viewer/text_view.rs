//! Reflowable Text View (`View -> Text View` / `T` key):
//! Extracts structured blocks from the active page on demand (zero idle RAM when closed)
//! and renders them in a clean, word-wrapped reading column respecting Light, Dark, and Sepia themes.

use crate::export::{extract_page_text_blocks, ExportBlock};

use super::render::fill_rect_clipped;
use super::state::*;
use super::ui_draw::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum TextViewHit {
    FontSmaller,
    FontLarger,
    CopyAll,
    ExportMarkdown,
    ExportLatex,
    BackToPdf,
}

#[derive(Clone, Copy)]
struct TvBox {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    hit: TextViewHit,
}

impl TvBox {
    fn contains(self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w as i32 && py >= self.y && py < self.y + self.h as i32
    }
}

pub(super) struct CachedTextViewPage {
    pub(super) page_index: u16,
    pub(super) blocks: Vec<ExportBlock>,
    pub(super) plain_text: String,
}

fn wrap_text_lines(text: &str, max_w: f32, font_sz: f32, bold: bool) -> Vec<String> {
    if text.trim().is_empty() || max_w <= 20.0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if !current.is_empty() && measure_text(&candidate, font_sz, bold) > max_w {
            lines.push(current);
            current = word.to_string();
        } else {
            current = candidate;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

impl<'a> PdfViewerApp<'a> {
    pub(super) fn ensure_text_view_cached(&mut self) {
        if self.view_mode != ViewMode::TextView {
            self.cached_text_view = None;
            return;
        }
        let p_idx = self.current_page_index;
        if let Some(ref c) = self.cached_text_view {
            if c.page_index == p_idx {
                return;
            }
        }
        let Some(doc) = self.doc.as_ref() else {
            self.cached_text_view = None;
            return;
        };
        let blocks = doc
            .pages()
            .get(p_idx)
            .map(|p| extract_page_text_blocks(&p))
            .unwrap_or_default();
        let plain_text = crate::export::render_blocks_to_markdown(std::slice::from_ref(&blocks));
        self.cached_text_view = Some(CachedTextViewPage {
            page_index: p_idx,
            blocks,
            plain_text,
        });
    }

    fn compute_text_view_bar_boxes(
        &self,
        sidebar_w: u32,
        top_bars_h: u32,
        buf_w: u32,
        sf: f64,
    ) -> (i32, i32, u32, u32, Vec<TvBox>) {
        let s = sf as f32;
        let content_w = buf_w.saturating_sub(sidebar_w);
        let col_w = ((760.0 * s).round() as u32).min(content_w.saturating_sub((32.0 * s).round() as u32).max(240));
        let col_x = sidebar_w as i32 + ((content_w.saturating_sub(col_w)) / 2) as i32;
        let bar_y = top_bars_h as i32 + (12.0 * s).round() as i32;
        let bar_h = (34.0 * s).round() as u32;
        let btn_h = (24.0 * s).round() as u32;
        let by = bar_y + ((bar_h.saturating_sub(btn_h)) / 2) as i32;
        let gap = (6.0 * s).round() as i32;

        let mut hits = Vec::new();
        let mut rx = col_x + col_w as i32 - (10.0 * s).round() as i32;
        let push_right = |hits: &mut Vec<TvBox>, rx: &mut i32, w: u32, hit: TextViewHit| {
            *rx -= w as i32;
            hits.push(TvBox { x: *rx, y: by, w, h: btn_h, hit });
            *rx -= gap;
        };

        push_right(&mut hits, &mut rx, (84.0 * s).round() as u32, TextViewHit::BackToPdf);
        push_right(&mut hits, &mut rx, (76.0 * s).round() as u32, TextViewHit::ExportLatex);
        push_right(&mut hits, &mut rx, (76.0 * s).round() as u32, TextViewHit::ExportMarkdown);
        push_right(&mut hits, &mut rx, (84.0 * s).round() as u32, TextViewHit::CopyAll);
        push_right(&mut hits, &mut rx, (28.0 * s).round() as u32, TextViewHit::FontLarger);
        push_right(&mut hits, &mut rx, (28.0 * s).round() as u32, TextViewHit::FontSmaller);

        (col_x, bar_y, col_w, bar_h, hits)
    }

    pub(super) fn draw_text_view(
        &mut self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        status_bar_h: u32,
        sf: f64,
    ) {
        self.ensure_text_view_cached();
        let theme = self.ui_theme();
        let s = sf as f32;
        let content_bottom = buf_h.saturating_sub(status_bar_h) as i32;

        // Reading surface colors adapt to all 6 eye-care ReadingFilter palettes
        let (paper_bg, text_fg, muted_fg, heading_fg) = match self.reading_filter {
            ReadingFilter::PaperEInk => (0x00EA_EBE6, 0x001E_1F1D, 0x005C_5E59, 0x0012_1311),
            ReadingFilter::WarmSepia => (0x00F7_F0DF, 0x002B_2318, 0x007A_684D, 0x001E_160C),
            ReadingFilter::SageEyeCare => (0x00E9_F2EA, 0x001D_271F, 0x0056_6B5A, 0x0012_1B14),
            ReadingFilter::DarkInvert => (0x001E_2024, 0x00D8_DCE2, 0x008E_96A4, 0x00EC_EFF4),
            ReadingFilter::NightAmber => (0x001A_1713, 0x00E5_CFA8, 0x009E_8B6B, 0x00F2_DFB9),
            ReadingFilter::Normal => (theme.surface_bg, theme.text_primary, theme.text_muted, theme.text_primary),
        };

        let (col_x, bar_y, col_w, bar_h, hits) = self.compute_text_view_bar_boxes(sidebar_w, top_bars_h, buf_w, sf);
        let sheet_top = bar_y;
        let sheet_h = (content_bottom - sheet_top - (12.0 * s).round() as i32).max(60) as u32;
        let rad = (8.0 * s).clamp(5.0, 12.0);

        draw_soft_shadow(
            buffer, buf_w, buf_h, col_x, sheet_top, col_w, sheet_h, rad,
            (10.0 * s).round() as i32, (3.0 * s).round() as i32, theme.shadow_rgb, theme.shadow_max_alpha,
        );
        fill_rounded_rect(buffer, buf_w, buf_h, col_x, sheet_top, col_w, sheet_h, rad, paper_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, col_x, sheet_top, col_w, sheet_h, rad, theme.border_subtle);

        // Top mini-toolbar inside reading sheet
        let small_sz = (11.0 * s).clamp(9.0, 22.0);
        let total = self.total_pages();
        let page_lbl = if total > 0 {
            format!("Text View  ·  Page {} / {total}", self.current_page_index + 1)
        } else {
            "Text View".to_string()
        };
        draw_text_vcenter(
            buffer, buf_w, buf_h,
            col_x + (14.0 * s).round() as i32,
            bar_y, bar_h,
            col_x + (240.0 * s).round() as i32,
            &page_lbl, small_sz, muted_fg, true,
        );
        fill_rect_clipped(buffer, buf_w, buf_h, col_x + 1, bar_y + bar_h as i32, col_w.saturating_sub(2), 1, theme.divider);

        for b in &hits {
            let hov = self.hovered_widget == Some(UiHit::TextView(b.hit));
            let lbl = match b.hit {
                TextViewHit::FontSmaller => "A-",
                TextViewHit::FontLarger => "A+",
                TextViewHit::CopyAll => "Copy Text",
                TextViewHit::ExportMarkdown => "Save .md",
                TextViewHit::ExportLatex => "Save .tex",
                TextViewHit::BackToPdf => "PDF View",
            };
            let bg = if b.hit == TextViewHit::BackToPdf {
                if hov { theme.accent_hover } else { theme.accent }
            } else if hov {
                theme.hover_bg
            } else {
                theme.surface_elevated
            };
            let fg = if b.hit == TextViewHit::BackToPdf { theme.text_on_accent } else { theme.text_primary };
            fill_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, (5.0 * s).max(3.0), bg);
            if b.hit != TextViewHit::BackToPdf {
                stroke_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, (5.0 * s).max(3.0), theme.border_subtle);
            }
            let lw = measure_text(lbl, small_sz, b.hit == TextViewHit::BackToPdf);
            draw_text_vcenter(buffer, buf_w, buf_h, b.x + ((b.w as f32 - lw) * 0.5).round() as i32, b.y, b.h, b.x + b.w as i32, lbl, small_sz, fg, b.hit == TextViewHit::BackToPdf);
        }

        // Flowing body text inside clipped region
        let body_top = bar_y + bar_h as i32 + (14.0 * s).round() as i32;
        let body_bottom = sheet_top + sheet_h as i32 - (12.0 * s).round() as i32;
        let pad_x = (28.0 * s).round() as i32;
        let text_w = (col_w as i32 - pad_x * 2).max(120) as f32;
        let base_sz = (self.text_view_font_size * s).clamp(11.0, 34.0);
        let line_step = (base_sz * 1.48).round() as i32;

        let blocks_opt = self.cached_text_view.as_ref().map(|c| c.blocks.clone());
        let Some(blocks) = blocks_opt else { return };

        if blocks.is_empty() {
            let msg = "No selectable text layer on this page (likely a scanned image PDF).";
            let mw = measure_text(msg, base_sz, false);
            draw_text(buffer, buf_w, buf_h, col_x + ((col_w as f32 - mw) * 0.5).round() as i32, body_top + (40.0 * s).round() as i32, msg, base_sz, muted_fg, false);
            return;
        }

        let mut y_cursor = body_top - self.scroll_y.round() as i32;
        let mut total_content_h = 0i32;

        for block in &blocks {
            if let ExportBlock::Table(rows) = block {
                if !rows.is_empty() {
                    let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(1).max(1);
                    let tbl_sz = (base_sz * 0.88).clamp(9.5, 24.0);
                    let cell_pad_x = (8.0 * s).round() as i32;
                    let row_h = (tbl_sz * 1.65).round().max(22.0 * s) as i32;
                    let tbl_x = col_x + pad_x;
                    let tbl_w = text_w.round().max(120.0) as u32;

                    // Measure natural column widths & detect numeric columns for right-alignment
                    let mut col_nat = vec![36.0f32; num_cols];
                    let mut col_num_hits = vec![0usize; num_cols];
                    let mut col_non_empty = vec![0usize; num_cols];
                    for (ri, r) in rows.iter().enumerate() {
                        for (ci, cell) in r.iter().enumerate().take(num_cols) {
                            let cw = measure_text(cell, tbl_sz, ri == 0) + (cell_pad_x * 2) as f32 + 4.0;
                            col_nat[ci] = col_nat[ci].max(cw);
                            if ri > 0 && !cell.trim().is_empty() {
                                col_non_empty[ci] += 1;
                                if crate::export::is_numeric_cell(cell) {
                                    col_num_hits[ci] += 1;
                                }
                            }
                        }
                    }
                    let sum_nat: f32 = col_nat.iter().sum::<f32>().max(1.0);
                    let mut col_w: Vec<i32> = col_nat
                        .iter()
                        .map(|&nw| ((nw / sum_nat) * (tbl_w as f32)).floor().max(28.0) as i32)
                        .collect();
                    let assigned: i32 = col_w.iter().sum();
                    if let Some(last) = col_w.last_mut() {
                        *last = (*last + (tbl_w as i32 - assigned)).max(28);
                    }
                    let col_right_align: Vec<bool> = (0..num_cols)
                        .map(|ci| col_non_empty[ci] > 0 && col_num_hits[ci] * 2 >= col_non_empty[ci])
                        .collect();

                    let tbl_top = y_cursor;
                    let tbl_h = (rows.len() as i32) * row_h;
                    let (hdr_bg, zebra_bg, grid_col) = match self.reading_filter {
                        ReadingFilter::PaperEInk => (0x00DC_DDD7, 0x00E3_E4DF, 0x00C8_C9C3),
                        ReadingFilter::WarmSepia => (0x00EA_E0C8, 0x00F2_EBD9, 0x00DF_D3B8),
                        ReadingFilter::SageEyeCare => (0x00DA_E8DC, 0x00E2_EDE4, 0x00C4_D8C7),
                        ReadingFilter::DarkInvert => (0x0028_2C34, 0x0022_252B, 0x0037_3C47),
                        ReadingFilter::NightAmber => (0x0026_211B, 0x0020_1C17, 0x0038_3026),
                        ReadingFilter::Normal => {
                            let z = if self.ui_theme_mode == super::theme::UiThemeMode::Dark {
                                0x0024_2832
                            } else {
                                0x00F8_FAFC
                            };
                            (theme.surface_elevated, z, theme.border_subtle)
                        }
                    };

                    for (ri, r) in rows.iter().enumerate() {
                        let ry = tbl_top + (ri as i32) * row_h;
                        if ry + row_h >= body_top && ry <= body_bottom {
                            let clip_y0 = ry.max(body_top);
                            let clip_y1 = (ry + row_h).min(body_bottom);
                            if clip_y1 > clip_y0 {
                                if ri == 0 {
                                    fill_rect_clipped(buffer, buf_w, buf_h, tbl_x, clip_y0, tbl_w, (clip_y1 - clip_y0) as u32, hdr_bg);
                                } else if ri % 2 == 0 {
                                    fill_rect_clipped(buffer, buf_w, buf_h, tbl_x, clip_y0, tbl_w, (clip_y1 - clip_y0) as u32, zebra_bg);
                                }
                                // Row bottom rule
                                if ry + row_h <= body_bottom {
                                    fill_rect_clipped(buffer, buf_w, buf_h, tbl_x, ry + row_h - 1, tbl_w, 1, grid_col);
                                }
                                let mut cx = tbl_x;
                                for ci in 0..num_cols {
                                    let cw = col_w[ci];
                                    if ci > 0 {
                                        fill_rect_clipped(buffer, buf_w, buf_h, cx, clip_y0, 1, (clip_y1 - clip_y0) as u32, grid_col);
                                    }
                                    if ry >= body_top && ry + row_h <= body_bottom + 2 {
                                        let cell_txt = r.get(ci).map(String::as_str).unwrap_or("");
                                        if !cell_txt.is_empty() {
                                            let is_bold = ri == 0;
                                            let inner_w = (cw - cell_pad_x * 2).max(12) as f32;
                                            let clipped = truncate_ellipsis(cell_txt, inner_w, tbl_sz, is_bold);
                                            let tx = if col_right_align[ci] {
                                                let tw = measure_text(&clipped, tbl_sz, is_bold).ceil() as i32;
                                                (cx + cw - cell_pad_x - tw).max(cx + cell_pad_x)
                                            } else {
                                                cx + cell_pad_x
                                            };
                                            let fg = if is_bold { heading_fg } else { text_fg };
                                            draw_text_vcenter(buffer, buf_w, buf_h, tx, ry, row_h as u32, cx + cw - 3, &clipped, tbl_sz, fg, is_bold);
                                        }
                                    }
                                    cx += cw;
                                }
                            }
                        }
                    }
                    // Outer left/right/top borders clipped to visible body
                    let vis_y0 = tbl_top.max(body_top);
                    let vis_y1 = (tbl_top + tbl_h).min(body_bottom);
                    if vis_y1 > vis_y0 {
                        if tbl_top >= body_top && tbl_top < body_bottom {
                            fill_rect_clipped(buffer, buf_w, buf_h, tbl_x, tbl_top, tbl_w, 1, grid_col);
                        }
                        fill_rect_clipped(buffer, buf_w, buf_h, tbl_x, vis_y0, 1, (vis_y1 - vis_y0) as u32, grid_col);
                        fill_rect_clipped(buffer, buf_w, buf_h, tbl_x + tbl_w as i32 - 1, vis_y0, 1, (vis_y1 - vis_y0) as u32, grid_col);
                    }

                    let para_gap = (base_sz * 0.75).round() as i32;
                    y_cursor += tbl_h + para_gap;
                    total_content_h += tbl_h + para_gap;
                }
                continue;
            }

            let (fsz, bold, indent, text_col, prefix, text) = match block {
                ExportBlock::Heading { level, text } => {
                    let mult = match level {
                        1 => 1.45,
                        2 => 1.25,
                        _ => 1.10,
                    };
                    (base_sz * mult, true, 0, heading_fg, String::new(), text.clone())
                }
                ExportBlock::Paragraph(spans) => {
                    let s_all: String = spans.iter().map(|sp| sp.text.as_str()).collect();
                    (base_sz, false, 0, text_fg, String::new(), s_all)
                }
                ExportBlock::BulletItem(spans) => {
                    let s_all: String = spans.iter().map(|sp| sp.text.as_str()).collect();
                    (base_sz, false, (16.0 * s).round() as i32, text_fg, "• ".to_string(), s_all)
                }
                ExportBlock::NumberedItem { num, spans } => {
                    let s_all: String = spans.iter().map(|sp| sp.text.as_str()).collect();
                    (base_sz, false, (16.0 * s).round() as i32, text_fg, format!("{num}. "), s_all)
                }
                ExportBlock::Table(_) => unreachable!(),
            };

            let step = (fsz * 1.45).round().max(line_step as f32) as i32;
            let full = format!("{prefix}{text}");
            for para_line in full.split('\n') {
                let wrapped = wrap_text_lines(para_line, (text_w - indent as f32).max(80.0), fsz, bold);
                for wl in wrapped {
                    if y_cursor >= body_top - step && y_cursor + step <= body_bottom + 4 {
                        if y_cursor >= body_top && y_cursor + (fsz.ceil() as i32) <= body_bottom {
                            draw_text_clipped(
                                buffer, buf_w, buf_h,
                                col_x + pad_x + indent,
                                y_cursor,
                                col_x + col_w as i32 - pad_x,
                                &wl, fsz, text_col, bold,
                            );
                        }
                    }
                    y_cursor += step;
                    total_content_h += step;
                }
            }
            let para_gap = (base_sz * 0.65).round() as i32;
            y_cursor += para_gap;
            total_content_h += para_gap;
        }

        let visible_h = (body_bottom - body_top).max(1);
        let max_scroll = (total_content_h - visible_h).max(0) as f32;
        self.text_view_max_scroll = max_scroll;
        self.scroll_y = self.scroll_y.clamp(0.0, max_scroll);
    }

    pub(super) fn hit_test_text_view(
        &self,
        mx: f64,
        my: f64,
        sidebar_w: u32,
        top_bars_h: u32,
        buf_w: u32,
        sf: f64,
    ) -> Option<TextViewHit> {
        if self.view_mode != ViewMode::TextView {
            return None;
        }
        let (_cx, _by, _cw, _bh, hits) = self.compute_text_view_bar_boxes(sidebar_w, top_bars_h, buf_w, sf);
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        hits.into_iter().find(|b| b.contains(ix, iy)).map(|b| b.hit)
    }

    pub(super) fn handle_text_view_click(&mut self, hit: TextViewHit) {
        match hit {
            TextViewHit::FontSmaller => {
                self.text_view_font_size = (self.text_view_font_size - 1.5).max(11.0);
            }
            TextViewHit::FontLarger => {
                self.text_view_font_size = (self.text_view_font_size + 1.5).min(26.0);
            }
            TextViewHit::CopyAll => {
                self.ensure_text_view_cached();
                let txt = self
                    .cached_text_view
                    .as_ref()
                    .map(|c| c.plain_text.clone())
                    .unwrap_or_default();
                if txt.trim().is_empty() {
                    self.status_notice = Some("No selectable text on this page to copy".to_string());
                } else {
                    match Self::copy_text_to_os_clipboard(&txt) {
                        Ok(()) => self.status_notice = Some("Copied page text to clipboard".to_string()),
                        Err(e) => self.status_notice = Some(format!("Copy failed: {e}")),
                    }
                }
            }
            TextViewHit::ExportMarkdown => {
                self.pending_dialog = Some(PendingDialog::ExportMarkdown);
            }
            TextViewHit::ExportLatex => {
                self.pending_dialog = Some(PendingDialog::ExportLatex);
            }
            TextViewHit::BackToPdf => {
                self.set_view_mode(self.last_page_view_mode);
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }
}

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

        // Reading surface colors adapt to ReadingFilter (Normal / Dark / Sepia) or UI theme
        let (paper_bg, text_fg, muted_fg, heading_fg) = match self.reading_filter {
            ReadingFilter::WarmSepia => (0x00FBF5E6, 0x003B2F1E, 0x007A684D, 0x00241A0E),
            ReadingFilter::DarkInvert => (0x00161B22, 0x00E6EDF3, 0x008B949E, 0x00FFFFFF),
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
                ExportBlock::Table(rows) => {
                    let mut joined = String::new();
                    for r in rows {
                        joined.push_str(&r.join("   |   "));
                        joined.push('\n');
                    }
                    (base_sz * 0.92, false, (8.0 * s).round() as i32, text_fg, String::new(), joined)
                }
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
                if let Some(txt) = self.cached_text_view.as_ref().map(|c| c.plain_text.clone()) {
                    if !txt.trim().is_empty() {
                        match Self::copy_text_to_os_clipboard(&txt) {
                            Ok(()) => self.status_notice = Some("Copied page text to clipboard".to_string()),
                            Err(e) => self.status_notice = Some(format!("Copy failed: {e}")),
                        }
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

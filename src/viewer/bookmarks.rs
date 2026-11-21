//! Themed Left Bookmark Sidebar (`B`) with crisp proportional typography, Add/Import pill
//! buttons, active-page accent bar, and hover-revealed delete button.

use crate::engine::PdfEngine;
use std::path::Path;
use super::render::fill_rect_clipped;
use super::state::*;
use super::ui_draw::*;

impl<'a> PdfViewerApp<'a> {
    pub(super) fn draw_bookmark_sidebar(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        scale_factor: f64,
    ) {
        let theme = self.ui_theme();
        let s = scale_factor as f32;
        let row_h = (28.0 * s).round() as u32;
        let header_h = (68.0 * s).round() as u32;
        let top_y = top_bars_h as i32;
        let pad = (10.0 * s).round() as i32;

        // Sidebar surface + right divider
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            0,
            top_y,
            sidebar_w,
            buf_h.saturating_sub(top_bars_h),
            theme.sidebar_bg,
        );
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            sidebar_w.saturating_sub(1) as i32,
            top_y,
            1,
            buf_h.saturating_sub(top_bars_h),
            theme.divider,
        );

        // Header strip
        fill_rect_clipped(buffer, buf_w, buf_h, 0, top_y, sidebar_w.saturating_sub(1), header_h, theme.sidebar_header_bg);
        fill_rect_clipped(buffer, buf_w, buf_h, 0, top_y + header_h as i32 - 1, sidebar_w.saturating_sub(1), 1, theme.divider);

        let title_sz = (12.0 * s).clamp(10.5, 26.0);
        let body_sz = (11.5 * s).clamp(10.0, 24.0);
        let sub_sz = (10.5 * s).clamp(9.0, 22.0);
        draw_text(buffer, buf_w, buf_h, pad, top_y + (10.0 * s).round() as i32, "Bookmarks", title_sz, theme.text_primary, true);

        // Header action buttons: `+ Add` and `Import .txt`
        let btn_y = top_y + (34.0 * s).round() as i32;
        let btn_h = (24.0 * s).round() as u32;
        let gap = (6.0 * s).round() as i32;
        let add_w = (62.0 * s).round() as u32;
        let imp_x = pad + add_w as i32 + gap;
        let imp_w = sidebar_w.saturating_sub((imp_x + pad + 1) as u32);
        let rad = (5.0 * s).max(3.0);

        let add_hov = self.hovered_widget == Some(UiHit::SidebarAdd);
        fill_rounded_rect(buffer, buf_w, buf_h, pad, btn_y, add_w, btn_h, rad, if add_hov { theme.accent_hover } else { theme.accent });
        let atw = measure_text("+ Add", sub_sz, true);
        draw_text_vcenter(buffer, buf_w, buf_h, pad + ((add_w as f32 - atw) * 0.5).round() as i32, btn_y, btn_h, pad + add_w as i32, "+ Add", sub_sz, theme.text_on_accent, true);

        let imp_hov = self.hovered_widget == Some(UiHit::SidebarImport);
        fill_rounded_rect(buffer, buf_w, buf_h, imp_x, btn_y, imp_w, btn_h, rad, if imp_hov { theme.hover_bg } else { theme.surface_elevated });
        stroke_rounded_rect(buffer, buf_w, buf_h, imp_x, btn_y, imp_w, btn_h, rad, theme.border_subtle);
        let itw = measure_text("Import .txt (L)", sub_sz, false);
        draw_text_vcenter(buffer, buf_w, buf_h, imp_x + ((imp_w as f32 - itw) * 0.5).round().max(4.0) as i32, btn_y, btn_h, imp_x + imp_w as i32 - 3, "Import .txt (L)", sub_sz, theme.text_secondary, false);

        if self.bookmarks.is_empty() {
            let empty_y = top_y + header_h as i32 + (18.0 * s).round() as i32;
            draw_text(buffer, buf_w, buf_h, pad, empty_y, "No bookmarks yet.", body_sz, theme.text_muted, false);
            draw_text(buffer, buf_w, buf_h, pad, empty_y + (20.0 * s).round() as i32, "Click + Add or press '+'", sub_sz, theme.text_muted, false);
            draw_text(buffer, buf_w, buf_h, pad, empty_y + (36.0 * s).round() as i32, "or import a .txt outline.", sub_sz, theme.text_muted, false);
            return;
        }

        let del_w = (22.0 * s).round() as u32;
        let ic_sz = (11.0 * s).round() as u32;

        for (vis_i, (bm_idx, bm)) in self
            .bookmarks
            .iter()
            .enumerate()
            .skip(self.bookmark_scroll_y)
            .enumerate()
        {
            let ry = top_y + header_h as i32 + (vis_i as i32) * (row_h as i32);
            if ry + row_h as i32 > buf_h as i32 {
                break;
            }

            let is_selected = self.selected_bookmark == Some(bm_idx);
            let is_current_page = bm.page_index == (self.current_page_index as usize);
            let is_hovered = matches!(
                self.hovered_widget,
                Some(UiHit::SidebarRow(i)) | Some(UiHit::SidebarDelete(i)) if i == bm_idx
            );

            if is_current_page || is_selected {
                fill_rounded_rect(buffer, buf_w, buf_h, (4.0 * s).round() as i32, ry + 1, sidebar_w.saturating_sub((9.0 * s).round() as u32), row_h.saturating_sub(2), (5.0 * s).max(3.0), theme.active_row_bg);
            } else if is_hovered {
                fill_rounded_rect(buffer, buf_w, buf_h, (4.0 * s).round() as i32, ry + 1, sidebar_w.saturating_sub((9.0 * s).round() as u32), row_h.saturating_sub(2), (5.0 * s).max(3.0), theme.hover_bg);
            }

            if is_current_page {
                fill_rounded_rect(buffer, buf_w, buf_h, (4.0 * s).round() as i32, ry + (5.0 * s).round() as i32, (3.0 * s).round().max(2.0) as u32, row_h.saturating_sub((10.0 * s).round() as u32), 1.5, theme.accent);
            }

            let indent_px = if bm.level > 0 {
                // Subtle indent guide line
                fill_rect_clipped(buffer, buf_w, buf_h, (14.0 * s).round() as i32, ry, 1, row_h, theme.divider);
                (22.0 * s).round() as i32
            } else {
                (12.0 * s).round() as i32
            };

            let prefix = if bm.serial.is_empty() {
                String::new()
            } else {
                format!("{}. ", bm.serial)
            };
            let page_tag = if !bm.logical_page.is_empty() {
                bm.logical_page.clone()
            } else {
                format!("{}", bm.page_index + 1)
            };
            let page_w = measure_text(&page_tag, sub_sz, false).ceil() as i32;
            let del_x = sidebar_w as i32 - del_w as i32 - (6.0 * s).round() as i32;
            let right_edge = if is_hovered { del_x - 4 } else { sidebar_w as i32 - (10.0 * s).round() as i32 };
            let page_x = right_edge - page_w;
            let title_budget = (page_x - indent_px - (6.0 * s).round() as i32).max(20) as f32;
            let full_title = format!("{prefix}{}", bm.title);
            let clipped = truncate_ellipsis(&full_title, title_budget, body_sz, is_current_page);

            let fg = if is_current_page {
                theme.accent_soft_fg
            } else if bm.level == 0 {
                theme.text_primary
            } else {
                theme.text_secondary
            };
            draw_text_vcenter(buffer, buf_w, buf_h, indent_px, ry, row_h, page_x - 4, &clipped, body_sz, fg, is_current_page);
            draw_text_vcenter(buffer, buf_w, buf_h, page_x, ry, row_h, right_edge, &page_tag, sub_sz, theme.text_muted, false);

            if is_hovered {
                let del_hov = self.hovered_widget == Some(UiHit::SidebarDelete(bm_idx));
                if del_hov {
                    fill_rounded_rect(buffer, buf_w, buf_h, del_x, ry + (3.0 * s).round() as i32, del_w, row_h.saturating_sub((6.0 * s).round() as u32), 4.0, theme.danger_soft_bg);
                }
                draw_icon(
                    buffer,
                    buf_w,
                    buf_h,
                    UiIcon::Close,
                    del_x + ((del_w.saturating_sub(ic_sz)) / 2) as i32,
                    ry + ((row_h.saturating_sub(ic_sz)) / 2) as i32,
                    ic_sz,
                    if del_hov { theme.danger } else { theme.text_muted },
                );
            }
        }
    }

    /// Hit-tests a pointer at `(mx, my)` inside the Left Bookmark Sidebar.
    pub(super) fn hit_test_bookmark_sidebar(
        &self,
        mx: f64,
        my: f64,
        sidebar_w: u32,
        top_bars_h: u32,
        scale_factor: f64,
    ) -> Option<SidebarHit> {
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if !self.show_bookmarks || ix < 0 || ix >= sidebar_w as i32 || iy < top_bars_h as i32 {
            return None;
        }
        let s = scale_factor as f32;
        let row_h = (28.0 * s).round() as i32;
        let header_h = (68.0 * s).round() as i32;
        let rel_y = iy - top_bars_h as i32;
        if rel_y < header_h {
            let pad = (10.0 * s).round() as i32;
            let btn_y = (34.0 * s).round() as i32;
            let btn_h = (24.0 * s).round() as i32;
            let gap = (6.0 * s).round() as i32;
            let add_w = (62.0 * s).round() as i32;
            if rel_y >= btn_y && rel_y < btn_y + btn_h {
                if ix >= pad && ix < pad + add_w {
                    return Some(SidebarHit::AddBookmarkButton);
                }
                if ix >= pad + add_w + gap && ix < sidebar_w as i32 - pad {
                    return Some(SidebarHit::ImportTxtButton);
                }
            }
            return None;
        }
        let vis_row = ((rel_y - header_h) / row_h) as usize;
        let bm_idx = self.bookmark_scroll_y + vis_row;
        if bm_idx < self.bookmarks.len() {
            let del_x = sidebar_w as i32 - (28.0 * s).round() as i32;
            let on_delete = ix >= del_x;
            Some(SidebarHit::BookmarkRow {
                index: bm_idx,
                on_delete,
            })
        } else {
            None
        }
    }

    pub(super) fn persist_bookmarks(&self) {
        // v1.2: Do not write `<pdf_filename>.bookmarks.txt` next to user PDFs by default.
        // Bookmarks are embedded into `/Outlines` on save (`S` or `Z`), while sidecar reading
        // in `PdfEngine::load_bookmarks` remains active for backward compatibility.
    }

    pub(super) fn import_bookmarks_from_path(&mut self, txt_path: &Path) {
        match std::fs::read_to_string(txt_path) {
            Ok(content) => {
                let total = self.total_pages() as usize;
                let parsed = PdfEngine::parse_bookmarks_txt(&content, self.roman_end, total);
                let count = parsed.len();
                self.bookmarks = parsed;
                self.selected_bookmark = if self.bookmarks.is_empty() {
                    None
                } else {
                    Some(0)
                };
                self.bookmark_scroll_y = 0;
                self.show_bookmarks = true;
                self.invalidate_page_caches();
                self.persist_bookmarks();
                self.status_notice = Some(format!("Imported {count} bookmarks!"));
                self.update_title_bar();
                self.request_redraw();
            }
            Err(e) => {
                self.status_notice = Some(format!("Failed to read bookmark .txt: {e}"));
                self.update_title_bar();
            }
        }
    }

    pub(super) fn import_bookmarks_txt_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Import Bookmarks .txt (Serial -- Title -- LogicalPage)")
            .add_filter("Text File", &["txt"])
            .pick_file()
        {
            self.import_bookmarks_from_path(&path);
        }
    }
}

use crate::engine::PdfEngine;
use std::path::Path;
use super::state::*;
use super::render::{draw_bitmap_str_8x8, draw_rect_border, fill_rect_clipped};

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
        let scale = (scale_factor.round() as u32).clamp(1, 2);
        let row_h = 24 * scale;
        let header_h = 54 * scale;
        let top_y = top_bars_h;

        // Fill sidebar background
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            0,
            top_y as i32,
            sidebar_w,
            buf_h.saturating_sub(top_y),
            SIDEBAR_BG_U32,
        );
        // Right separator line
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            (sidebar_w.saturating_sub(1)) as i32,
            top_y as i32,
            1,
            buf_h.saturating_sub(top_y),
            0x00455A64,
        );

        // Header bar + clickable `[Import .txt]` button
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            0,
            top_y as i32,
            sidebar_w,
            header_h,
            0x0014171C,
        );
        draw_bitmap_str_8x8(
            buffer,
            buf_w,
            buf_h,
            8 * scale,
            top_y + 7 * scale,
            "BOOKMARKS (+/E/Del)",
            0x0064B5F6,
            scale,
        );

        // `[Import .txt (L)]` button inside header
        let btn_x = 8 * scale;
        let btn_y = top_y + 24 * scale;
        let btn_w = sidebar_w.saturating_sub(16 * scale);
        let btn_h = 22 * scale;
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            btn_x as i32,
            btn_y as i32,
            btn_w,
            btn_h,
            0x00263238,
        );
        draw_rect_border(buffer, buf_w, buf_h, btn_x, btn_y, btn_w, btn_h, 0x004FC3F7);
        draw_bitmap_str_8x8(
            buffer,
            buf_w,
            buf_h,
            btn_x + 8 * scale,
            btn_y + 6 * scale,
            "[Import .txt] (L)",
            0x0080DEEA,
            scale,
        );

        if self.bookmarks.is_empty() {
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                10 * scale,
                top_y + header_h + 14 * scale,
                "No bookmarks yet.",
                0x009E9E9E,
                scale,
            );
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                10 * scale,
                top_y + header_h + 30 * scale,
                "Press '+' to add or",
                0x009E9E9E,
                scale,
            );
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                10 * scale,
                top_y + header_h + 46 * scale,
                "'L' to import .txt",
                0x009E9E9E,
                scale,
            );
            return;
        }

        for (vis_i, (bm_idx, bm)) in self
            .bookmarks
            .iter()
            .enumerate()
            .skip(self.bookmark_scroll_y)
            .enumerate()
        {
            let ry = top_y + header_h + (vis_i as u32) * row_h;
            if ry + row_h > buf_h {
                break;
            }

            let is_selected = self.selected_bookmark == Some(bm_idx);
            let is_current_page = bm.page_index == (self.current_page_index as usize);
            let row_bg = if is_selected {
                0x002C3E50
            } else if is_current_page {
                0x00263238
            } else {
                SIDEBAR_BG_U32
            };

            fill_rect_clipped(
                buffer,
                buf_w,
                buf_h,
                0,
                ry as i32,
                sidebar_w.saturating_sub(1),
                row_h,
                row_bg,
            );

            // Indent level == 1 sub-items by 16 * scale pixels
            let indent_px = if bm.level > 0 { 16 * scale } else { 6 * scale };
            let avail_px = sidebar_w.saturating_sub(indent_px + 30 * scale);
            let max_chars = (avail_px / (8 * scale)).max(5) as usize;

            let prefix = if bm.serial.is_empty() {
                String::new()
            } else {
                format!("{}. ", bm.serial)
            };
            let page_tag = if !bm.logical_page.is_empty() {
                format!("[p.{}]", bm.logical_page)
            } else {
                format!("[p.{}]", bm.page_index + 1)
            };

            let mut label = format!("{}{} {}", prefix, bm.title, page_tag);
            if label.chars().count() > max_chars {
                let title_budget =
                    max_chars.saturating_sub(prefix.chars().count() + page_tag.chars().count() + 1);
                let short_title: String = bm.title.chars().take(title_budget.max(3)).collect();
                label = format!("{}{} {}", prefix, short_title, page_tag);
                if label.chars().count() > max_chars {
                    label = label.chars().take(max_chars).collect();
                }
            }

            let text_color = if is_current_page {
                0x00FFEB3B
            } else if bm.level == 0 {
                0x00FFFFFF // Bright white for top-level chapters
            } else {
                0x0080DEEA // Soft cyan/gray for indented sub-items
            };
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                indent_px,
                ry + 7 * scale,
                &label,
                text_color,
                scale,
            );

            // Draw `[x]` delete button on right side of bookmark row
            let del_x = sidebar_w.saturating_sub(26 * scale);
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                del_x,
                ry + 7 * scale,
                "[x]",
                0x00EF5350,
                scale,
            );
        }
    }

    /// Hit-tests a click at `(mx, my)` inside the Left Bookmark Sidebar.
    pub(super) fn hit_test_bookmark_sidebar(
        &self,
        mx: f64,
        my: f64,
        sidebar_w: u32,
        top_bars_h: u32,
        scale_factor: f64,
    ) -> Option<SidebarHit> {
        if !self.show_bookmarks || (mx as u32) >= sidebar_w || (my as u32) < top_bars_h {
            return None;
        }
        let scale = (scale_factor.round() as u32).clamp(1, 2);
        let row_h = 24 * scale;
        let header_h = 54 * scale;
        let rel_y = (my.round() as u32).saturating_sub(top_bars_h);
        if rel_y < header_h {
            // Check if clicked inside `[Import .txt]` button (`22 * scale..48 * scale`)
            if rel_y >= 20 * scale && rel_y <= 50 * scale {
                return Some(SidebarHit::ImportTxtButton);
            }
            return None;
        }
        let vis_row = ((rel_y - header_h) / row_h) as usize;
        let bm_idx = self.bookmark_scroll_y + vis_row;
        if bm_idx < self.bookmarks.len() {
            let del_x = sidebar_w.saturating_sub(30 * scale);
            let on_delete = (mx.round() as u32) >= del_x;
            Some(SidebarHit::BookmarkRow {
                index: bm_idx,
                on_delete,
            })
        } else {
            None
        }
    }

    pub(super) fn persist_bookmarks(&self) {
        let _ = PdfEngine::save_bookmarks_sidecar(&self.file_path, &self.bookmarks);
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

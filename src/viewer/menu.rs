use super::state::*;
use super::render::{draw_bitmap_str_8x8, draw_rect_border, fill_rect_clipped};

impl<'a> PdfViewerApp<'a> {
    pub(super) fn menu_header_x_range(&self, cat: MenuCategory, scale: u32) -> (u32, u32) {
        let mut cur_x = 6 * scale;
        for c in MenuCategory::ALL {
            let w = (c.label().len() as u32) * 8 * scale + 12 * scale;
            if c == cat {
                return (cur_x, cur_x + w);
            }
            cur_x += w + 4 * scale;
        }
        (0, 0)
    }

    pub(super) fn draw_menu_bar(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        menu_bar_h: u32,
        scale_factor: f64,
    ) {
        if menu_bar_h == 0 {
            return;
        }
        let scale = (scale_factor.round() as u32).clamp(1, 2);
        // #1E1E1E crisp dark Menu Bar background
        fill_rect_clipped(buffer, buf_w, buf_h, 0, 0, buf_w, menu_bar_h, 0x001E1E1E);
        // 1px bottom divider
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            0,
            (menu_bar_h.saturating_sub(1)) as i32,
            buf_w,
            1,
            0x00333333,
        );

        let text_y = menu_bar_h.saturating_sub(8 * scale) / 2;
        let mx = self.cursor_pos.0.round() as u32;
        let my = self.cursor_pos.1.round() as u32;

        for cat in MenuCategory::ALL {
            let (x0, x1) = self.menu_header_x_range(cat, scale);
            let is_open = self.open_menu == Some(cat);
            let is_hover = my < menu_bar_h && mx >= x0 && mx < x1;
            if is_open || is_hover {
                let bg = if is_open { 0x00007ACC } else { 0x00333337 };
                fill_rect_clipped(
                    buffer,
                    buf_w,
                    buf_h,
                    x0 as i32,
                    1,
                    x1.saturating_sub(x0),
                    menu_bar_h.saturating_sub(2),
                    bg,
                );
            }
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                x0 + 6 * scale,
                text_y,
                cat.label(),
                0x00F0F0F0,
                scale,
            );
        }

        // Right-aligned live status badges in Menu Bar (View Mode, Mouse Mode, Filter)
        let right_info = format!(
            "View:{} | Mouse:{} | Filter:{}",
            self.view_mode.short_label(),
            self.mouse_mode.short_label(),
            self.reading_filter.label()
        );
        let info_w = (right_info.len() as u32) * 8 * scale + 10 * scale;
        let (_, last_menu_x1) = self.menu_header_x_range(MenuCategory::Help, scale);
        if buf_w > last_menu_x1 + info_w + 12 * scale {
            let info_col = match self.mouse_mode {
                MouseMode::Select => 0x00B3E5FC,
                MouseMode::Highlight => 0x0080DEEA,
                MouseMode::Redact => 0x00FF8A80,
                MouseMode::Crop => 0x0069F0AE,
                MouseMode::StampImage => 0x00FFD740,
            };
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                buf_w - info_w,
                text_y,
                &right_info,
                info_col,
                scale,
            );
        }
    }

    pub(super) fn menu_items_for_category(&self, cat: MenuCategory) -> Vec<String> {
        let mut list: Vec<String> = cat.items().iter().map(|s| (*s).to_string()).collect();
        if cat == MenuCategory::File {
            if self.recent_files.is_empty() {
                list.push("Recent: (none)".to_string());
            } else {
                for (idx, rpath) in self.recent_files.iter().take(MAX_RECENT_FILES).enumerate() {
                    let fname = rpath
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("document.pdf");
                    list.push(format!("Recent {}: {}", idx + 1, fname));
                }
            }
        }
        list
    }

    pub(super) fn draw_open_menu_dropdown(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        menu_bar_h: u32,
        cat: MenuCategory,
        scale_factor: f64,
    ) {
        let scale = (scale_factor.round() as u32).clamp(1, 2);
        let items = self.menu_items_for_category(cat);
        if items.is_empty() {
            return;
        }
        let (hx0, _) = self.menu_header_x_range(cat, scale);
        let row_h = 22 * scale;
        let max_chars = items.iter().map(|s| s.chars().count()).max().unwrap_or(20) as u32;
        let drop_w = (max_chars * 8 * scale + 24 * scale).min(buf_w.saturating_sub(8));
        let drop_h = (items.len() as u32) * row_h + 4 * scale;
        let drop_x = hx0.min(buf_w.saturating_sub(drop_w + 4));
        let drop_y = menu_bar_h;

        // #252526 dropdown background + border
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            drop_x as i32,
            drop_y as i32,
            drop_w,
            drop_h,
            0x00252526,
        );
        draw_rect_border(
            buffer,
            buf_w,
            buf_h,
            drop_x,
            drop_y,
            drop_w,
            drop_h,
            0x00454545,
        );

        let mx = self.cursor_pos.0.round() as u32;
        let my = self.cursor_pos.1.round() as u32;

        for (idx, item_str) in items.iter().enumerate() {
            let ry = drop_y + 2 * scale + (idx as u32) * row_h;
            let hovered =
                mx >= drop_x && mx < drop_x + drop_w && my >= ry && my < ry + row_h;
            if hovered {
                // #007ACC hover highlight
                fill_rect_clipped(
                    buffer,
                    buf_w,
                    buf_h,
                    (drop_x + 1) as i32,
                    ry as i32,
                    drop_w.saturating_sub(2),
                    row_h,
                    0x00007ACC,
                );
            }
            let text_y = ry + row_h.saturating_sub(8 * scale) / 2;
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                drop_x + 10 * scale,
                text_y,
                item_str,
                0x00F5F5F5,
                scale,
            );
        }
    }

    pub(super) fn hit_test_menu_bar_header(
        &self,
        mx: f64,
        my: f64,
        menu_bar_h: u32,
        scale_factor: f64,
    ) -> Option<MenuCategory> {
        let ix = mx.round() as u32;
        let iy = my.round() as u32;
        if iy >= menu_bar_h {
            return None;
        }
        let scale = (scale_factor.round() as u32).clamp(1, 2);
        for cat in MenuCategory::ALL {
            let (x0, x1) = self.menu_header_x_range(cat, scale);
            if ix >= x0 && ix < x1 {
                return Some(cat);
            }
        }
        None
    }

    pub(super) fn hit_test_open_menu_dropdown(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        menu_bar_h: u32,
        scale_factor: f64,
    ) -> Option<(MenuCategory, usize)> {
        let cat = self.open_menu?;
        let scale = (scale_factor.round() as u32).clamp(1, 2);
        let items = self.menu_items_for_category(cat);
        let (hx0, _) = self.menu_header_x_range(cat, scale);
        let row_h = 22 * scale;
        let max_chars = items.iter().map(|s| s.chars().count()).max().unwrap_or(20) as u32;
        let drop_w = (max_chars * 8 * scale + 24 * scale).min(buf_w.saturating_sub(8));
        let drop_h = (items.len() as u32) * row_h + 4 * scale;
        let drop_x = hx0.min(buf_w.saturating_sub(drop_w + 4));
        let drop_y = menu_bar_h;

        let ix = mx.round() as u32;
        let iy = my.round() as u32;
        if ix < drop_x || ix >= drop_x + drop_w || iy < drop_y + 2 * scale || iy >= drop_y + drop_h
        {
            return None;
        }
        let idx = ((iy - (drop_y + 2 * scale)) / row_h) as usize;
        if idx < items.len() {
            Some((cat, idx))
        } else {
            None
        }
    }

    pub(super) fn execute_menu_item(&mut self, cat: MenuCategory, item_idx: usize) {
        self.open_menu = None;
        match (cat, item_idx) {
            // File menu
            (MenuCategory::File, 0) => {
                self.pending_dialog = Some(PendingDialog::Open);
            }
            (MenuCategory::File, 1) => {
                self.pending_dialog = Some(PendingDialog::CombineMultiple);
            }
            (MenuCategory::File, 2) => {
                self.pending_dialog = Some(PendingDialog::ImportBookmarksTxt);
            }
            (MenuCategory::File, 3) => {
                self.pending_dialog = Some(PendingDialog::Save);
            }
            (MenuCategory::File, 4) => {
                self.compress_current_pdf();
            }
            (MenuCategory::File, 5) => {
                self.trigger_print();
            }
            (MenuCategory::File, 6) => {
                if self.tabs.len() > 1 {
                    let _ = self.close_tab(self.active_tab);
                } else {
                    self.status_notice = Some("Cannot close the last remaining tab".to_string());
                }
            }
            (MenuCategory::File, idx) if idx >= MenuCategory::File.items().len() => {
                let recent_idx = idx - MenuCategory::File.items().len();
                if let Some(rpath) = self.recent_files.get(recent_idx).cloned() {
                    let fname = rpath
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("document.pdf")
                        .to_string();
                    let res = if self.doc.is_none() {
                        self.load_document(rpath)
                    } else {
                        self.open_document_in_new_tab(rpath)
                    };
                    if let Err(e) = res {
                        self.status_notice = Some(format!("Failed to open recent '{fname}': {e}"));
                    }
                }
            }

            // View menu
            (MenuCategory::View, 0) => self.set_view_mode(ViewMode::FitPage),
            (MenuCategory::View, 1) => self.set_view_mode(ViewMode::FitWidth),
            (MenuCategory::View, 2) => self.set_view_mode(ViewMode::FitTextWidth),
            (MenuCategory::View, 3) => self.set_view_mode(ViewMode::TwoPageSpread),
            (MenuCategory::View, 4) => self.set_view_mode(ViewMode::ContinuousScroll),
            (MenuCategory::View, 5) => self.zoom_by_step(1.25),
            (MenuCategory::View, 6) => self.zoom_by_step(0.8),
            (MenuCategory::View, 7) => self.reset_zoom(),
            (MenuCategory::View, 8) => self.toggle_view_mode(),
            (MenuCategory::View, 9) => {
                self.show_bookmarks = !self.show_bookmarks;
                self.invalidate_page_caches();
                self.status_notice = Some(if self.show_bookmarks {
                    "Bookmarks Sidebar ON".to_string()
                } else {
                    "Bookmarks Sidebar OFF".to_string()
                });
            }
            (MenuCategory::View, 10) => {
                self.reading_filter = self.reading_filter.next();
                self.status_notice = Some(format!(
                    "Reading Filter: {} (Zero-RAM CPU Blit Filter)",
                    self.reading_filter.label()
                ));
            }
            (MenuCategory::View, 11) => self.toggle_fullscreen(),
            (MenuCategory::View, 12) => {
                if self.is_presentation {
                    self.exit_presentation_or_fullscreen();
                } else {
                    self.enter_presentation_mode();
                }
            }

            // Edit menu
            (MenuCategory::Edit, 0) => self.perform_undo(),
            (MenuCategory::Edit, 1) => self.copy_selected_text_to_clipboard(),
            (MenuCategory::Edit, 2) => {
                self.input_mode = InputMode::SearchText {
                    query_input: self.search_query.clone(),
                };
            }
            (MenuCategory::Edit, 3) => {
                self.mouse_mode = MouseMode::Select;
                self.status_notice = Some(
                    "Mouse Mode: Select Text (Drag or double-click text, Ctrl/Cmd+C to copy)"
                        .to_string(),
                );
            }
            (MenuCategory::Edit, 4) => {
                self.mouse_mode = MouseMode::Highlight;
                self.status_notice =
                    Some("Mouse Mode: Highlight Text (Drag across text)".to_string());
            }
            (MenuCategory::Edit, 5) => {
                self.mouse_mode = MouseMode::Redact;
                self.status_notice = Some(
                    "Mouse Mode: Black-Box Redact (Drag red box to permanently purge & black out)"
                        .to_string(),
                );
            }
            (MenuCategory::Edit, 6) => {
                self.mouse_mode = MouseMode::Crop;
                self.status_notice = Some(
                    "Mouse Mode: Crop Page Area (Drag green box to crop page)".to_string(),
                );
            }

            // Page menu
            (MenuCategory::Page, 0) => {
                self.input_mode = InputMode::JumpToPage {
                    input: String::new(),
                };
            }
            (MenuCategory::Page, 1) => {
                let target = if self.view_mode == ViewMode::ThumbnailGrid {
                    self.hovered_thumb.unwrap_or(self.current_page_index)
                } else {
                    self.current_page_index
                };
                self.rotate_page_clockwise(target);
            }
            (MenuCategory::Page, 2) => self.insert_blank_page_after_current(),
            (MenuCategory::Page, 3) => self.delete_current_page(),
            (MenuCategory::Page, 4) => {
                let target = if self.view_mode == ViewMode::ThumbnailGrid {
                    self.hovered_thumb.unwrap_or(self.current_page_index)
                } else {
                    self.current_page_index
                };
                self.convert_page_to_bw_in_place(target);
            }
            (MenuCategory::Page, 5) => self.reset_current_page_crop(),

            // Tools menu
            (MenuCategory::Tools, 0) => {
                self.input_mode = InputMode::PageNumbers {
                    input: String::new(),
                };
            }
            (MenuCategory::Tools, 1) => {
                self.input_mode = InputMode::HeaderFooterRange {
                    range_input: String::new(),
                };
            }
            (MenuCategory::Tools, 2) => {
                self.input_mode = InputMode::ClearMargins {
                    input: String::new(),
                };
            }
            (MenuCategory::Tools, 3) => {
                self.input_mode = InputMode::Watermark {
                    input: String::new(),
                };
            }
            (MenuCategory::Tools, 4) => {
                self.pending_dialog = Some(PendingDialog::PickStampImage);
            }
            (MenuCategory::Tools, 5) => {
                self.input_mode = InputMode::SplitRange {
                    input: String::new(),
                };
            }
            (MenuCategory::Tools, 6) => self.flatten_and_unlock_current_pdf(),
            (MenuCategory::Tools, 7) => self.open_optimize_prompt(),

            // Help menu
            (MenuCategory::Help, 0) => {
                self.show_help_overlay = !self.show_help_overlay;
            }
            _ => {}
        }
        self.update_title_bar();
        self.request_redraw();
    }
}

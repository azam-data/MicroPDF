use crate::engine::PdfEngine;
use anyhow::Result;
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use super::state::*;
use super::render::{draw_bitmap_str_8x8, draw_rect_border, fill_rect_clipped};

impl<'a> PdfViewerApp<'a> {
    pub(super) fn sync_active_tab_to_slot(&mut self) {
        if self.active_tab < self.tabs.len() {
            let slot = &mut self.tabs[self.active_tab];
            slot.doc = self.doc.take();
            slot.file_path = self.file_path.clone();
            slot.current_page_index = self.current_page_index;
            slot.view_mode = self.view_mode;
            slot.last_page_view_mode = self.last_page_view_mode;
            slot.zoom_level = self.zoom_level;
            slot.pan_x = self.pan_x;
            slot.scroll_y = self.scroll_y;
            slot.nav_back_stack = std::mem::take(&mut self.nav_back_stack);
            slot.nav_forward_stack = std::mem::take(&mut self.nav_forward_stack);
            slot.roman_end = self.roman_end;
            slot.bookmarks = self.bookmarks.clone();
            slot.selected_bookmark = self.selected_bookmark;
            slot.bookmark_scroll_y = self.bookmark_scroll_y;
            slot.grid_scroll_y = self.grid_scroll_y;
            slot.bw_pages = self.bw_pages.clone();
            // v1.1: move, don't deep-clone, up to 8 full-document snapshots on every tab switch.
            slot.undo_stack = std::mem::take(&mut self.undo_stack);
            slot.redacted = self.redacted;
            slot.has_unsaved_edits = self.has_unsaved_edits;
            slot.last_file_mtime = self.last_file_mtime;
        }
    }

    pub(super) fn load_slot_into_active(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active_tab = index;
            let slot = &mut self.tabs[index];
            self.doc = slot.doc.take();
            self.file_path = slot.file_path.clone();
            self.current_page_index = slot.current_page_index;
            self.view_mode = slot.view_mode;
            self.last_page_view_mode = slot.last_page_view_mode;
            self.zoom_level = slot.zoom_level;
            self.pan_x = slot.pan_x;
            self.scroll_y = slot.scroll_y;
            self.nav_back_stack = std::mem::take(&mut slot.nav_back_stack);
            self.nav_forward_stack = std::mem::take(&mut slot.nav_forward_stack);
            self.hovered_link = None;
            self.roman_end = slot.roman_end;
            self.bookmarks = slot.bookmarks.clone();
            self.selected_bookmark = slot.selected_bookmark;
            self.bookmark_scroll_y = slot.bookmark_scroll_y;
            self.grid_scroll_y = slot.grid_scroll_y;
            self.bw_pages = slot.bw_pages.clone();
            self.undo_stack = std::mem::take(&mut slot.undo_stack);
            self.redacted = slot.redacted;
            self.has_unsaved_edits = slot.has_unsaved_edits;
            self.last_file_mtime = slot.last_file_mtime;
            self.flatten_confirm_at = None;
            self.search_query.clear();
            self.search_matches.clear();
            self.current_search_match = 0;
        }
    }

    /// Loads `path` into the current tab (or creates the first tab on startup).
    /// If `path` is encrypted and requires a password, enters `InputMode::PasswordPrompt` (3 attempts) instead of failing.
    pub fn load_document(&mut self, path: PathBuf) -> Result<()> {
        match self.load_document_with_password(path.clone(), None) {
            Ok(()) => Ok(()),
            Err(e) if PdfEngine::is_password_error(&e) => {
                let fname = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("document.pdf")
                    .to_string();
                self.input_mode = InputMode::PasswordPrompt {
                    path,
                    password_input: String::new(),
                    attempts_left: 3,
                    open_in_new_tab: false,
                };
                self.status_notice = Some(format!(
                    "Password required to open '{fname}' (3 attempts left)"
                ));
                self.update_title_bar();
                self.request_redraw();
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Loads `path` into the current tab using an optional `password` (never stored after loading).
    pub(super) fn load_document_with_password(
        &mut self,
        path: PathBuf,
        password: Option<&str>,
    ) -> Result<()> {
        let loaded = self.engine.load_pdf_with_password(&path, password)?;
        let roman_end = PdfEngine::read_page_labels_roman_end_from_path(&path);
        let loaded_bookmarks = PdfEngine::load_bookmarks(&loaded, &path, roman_end);

        // Strictly drop previous cached page bitmaps, thumbnail cache, and previous document
        self.invalidate_page_caches();
        self.thumbnail_cache.clear();
        self.thumbnail_cache.shrink_to_fit();
        self.doc = None;

        let file_mtime = Self::read_file_mtime(&path);
        self.file_path = path.clone();
        self.current_page_index = 0;
        self.zoom_level = 1.0;
        self.pan_x = 0.0;
        self.scroll_y = 0.0;
        self.nav_back_stack.clear();
        self.nav_forward_stack.clear();
        self.hovered_link = None;
        self.roman_end = roman_end;
        self.bw_pages.clear();
        self.undo_stack.clear();
        self.redacted = false;
        self.has_unsaved_edits = false;
        self.last_file_mtime = file_mtime;
        self.flatten_confirm_at = None;
        self.input_mode = InputMode::Normal;
        self.drag_start = None;
        self.grid_drag_page = None;
        self.grid_scroll_y = 0.0;
        self.search_query.clear();
        self.search_matches.clear();
        self.current_search_match = 0;
        self.bookmarks = loaded_bookmarks;
        self.selected_bookmark = if self.bookmarks.is_empty() {
            None
        } else {
            Some(0)
        };
        self.bookmark_scroll_y = 0;
        self.doc = Some(loaded);

        self.record_recent_file(&path);

        if self.tabs.is_empty() {
            self.tabs.push(DocumentTab {
                doc: None,
                file_path: path,
                current_page_index: 0,
                view_mode: self.view_mode,
                last_page_view_mode: self.last_page_view_mode,
                zoom_level: 1.0,
                pan_x: 0.0,
                scroll_y: 0.0,
                nav_back_stack: VecDeque::new(),
                nav_forward_stack: Vec::new(),
                roman_end,
                bookmarks: self.bookmarks.clone(),
                selected_bookmark: self.selected_bookmark,
                bookmark_scroll_y: 0,
                grid_scroll_y: 0.0,
                bw_pages: HashSet::new(),
                undo_stack: VecDeque::with_capacity(MAX_UNDO_STATES),
                redacted: false,
                has_unsaved_edits: false,
                last_file_mtime: file_mtime,
            });
            self.active_tab = 0;
        } else if self.active_tab < self.tabs.len() {
            self.tabs[self.active_tab].file_path = path;
            self.tabs[self.active_tab].current_page_index = 0;
            self.tabs[self.active_tab].zoom_level = 1.0;
            self.tabs[self.active_tab].pan_x = 0.0;
            self.tabs[self.active_tab].scroll_y = 0.0;
            self.tabs[self.active_tab].nav_back_stack.clear();
            self.tabs[self.active_tab].nav_forward_stack.clear();
            self.tabs[self.active_tab].roman_end = roman_end;
            self.tabs[self.active_tab].bookmarks = self.bookmarks.clone();
            self.tabs[self.active_tab].selected_bookmark = self.selected_bookmark;
            self.tabs[self.active_tab].bookmark_scroll_y = 0;
            self.tabs[self.active_tab].grid_scroll_y = 0.0;
            self.tabs[self.active_tab].bw_pages.clear();
            self.tabs[self.active_tab].undo_stack.clear();
            self.tabs[self.active_tab].redacted = false;
            self.tabs[self.active_tab].has_unsaved_edits = false;
            self.tabs[self.active_tab].last_file_mtime = file_mtime;
        }

        self.update_title_bar();
        self.request_redraw();
        Ok(())
    }

    /// Opens `path` in a brand-new tab and switches to it immediately, freeing the previous tab's bitmaps.
    /// If `path` is encrypted and requires a password, enters `InputMode::PasswordPrompt` (3 attempts).
    pub fn open_document_in_new_tab(&mut self, path: PathBuf) -> Result<()> {
        if self.tabs.is_empty() {
            return self.load_document(path);
        }
        match self.open_document_in_new_tab_with_password(path.clone(), None) {
            Ok(()) => Ok(()),
            Err(e) if PdfEngine::is_password_error(&e) => {
                let fname = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("document.pdf")
                    .to_string();
                self.input_mode = InputMode::PasswordPrompt {
                    path,
                    password_input: String::new(),
                    attempts_left: 3,
                    open_in_new_tab: true,
                };
                self.status_notice = Some(format!(
                    "Password required to open '{fname}' (3 attempts left)"
                ));
                self.update_title_bar();
                self.request_redraw();
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    pub(super) fn open_document_in_new_tab_with_password(
        &mut self,
        path: PathBuf,
        password: Option<&str>,
    ) -> Result<()> {
        if self.tabs.is_empty() {
            return self.load_document_with_password(path, password);
        }
        let loaded = self.engine.load_pdf_with_password(&path, password)?;
        let roman_end = PdfEngine::read_page_labels_roman_end_from_path(&path);
        let loaded_bookmarks = PdfEngine::load_bookmarks(&loaded, &path, roman_end);
        let selected_bookmark = if loaded_bookmarks.is_empty() {
            None
        } else {
            Some(0)
        };

        self.record_recent_file(&path);

        // Save current active tab into its slot and release all cached bitmaps
        self.sync_active_tab_to_slot();
        self.invalidate_page_caches();
        self.thumbnail_cache.clear();
        self.thumbnail_cache.shrink_to_fit();

        let file_mtime = Self::read_file_mtime(&path);
        let new_idx = self.tabs.len();
        self.tabs.push(DocumentTab {
            doc: None,
            file_path: path.clone(),
            current_page_index: 0,
            view_mode: ViewMode::FitPage,
            last_page_view_mode: ViewMode::FitPage,
            zoom_level: 1.0,
            pan_x: 0.0,
            scroll_y: 0.0,
            nav_back_stack: VecDeque::new(),
            nav_forward_stack: Vec::new(),
            roman_end,
            bookmarks: loaded_bookmarks.clone(),
            selected_bookmark,
            bookmark_scroll_y: 0,
            grid_scroll_y: 0.0,
            bw_pages: HashSet::new(),
            undo_stack: VecDeque::with_capacity(MAX_UNDO_STATES),
            redacted: false,
            has_unsaved_edits: false,
            last_file_mtime: file_mtime,
        });

        self.active_tab = new_idx;
        self.doc = Some(loaded);
        self.file_path = path;
        self.current_page_index = 0;
        self.view_mode = ViewMode::FitPage;
        self.last_page_view_mode = ViewMode::FitPage;
        self.zoom_level = 1.0;
        self.pan_x = 0.0;
        self.scroll_y = 0.0;
        self.nav_back_stack.clear();
        self.nav_forward_stack.clear();
        self.hovered_link = None;
        self.roman_end = roman_end;
        self.bookmarks = loaded_bookmarks;
        self.selected_bookmark = selected_bookmark;
        self.bookmark_scroll_y = 0;
        self.grid_scroll_y = 0.0;
        self.bw_pages.clear();
        self.undo_stack.clear();
        self.redacted = false;
        self.has_unsaved_edits = false;
        self.last_file_mtime = file_mtime;
        self.flatten_confirm_at = None;
        self.search_query.clear();
        self.search_matches.clear();
        self.current_search_match = 0;
        self.input_mode = InputMode::Normal;
        self.drag_start = None;
        self.grid_drag_page = None;

        self.update_title_bar();
        self.request_redraw();
        Ok(())
    }

    /// Opens an in-memory PDF byte vector in a brand-new tab named `virtual_path` (used by Combine Multiple PDFs).
    pub fn open_bytes_in_new_tab(&mut self, bytes: Vec<u8>, virtual_path: PathBuf) -> Result<()> {
        let loaded = self.engine.load_pdf_from_bytes_safe(bytes)?;
        self.sync_active_tab_to_slot();
        self.invalidate_page_caches();
        self.thumbnail_cache.clear();
        self.thumbnail_cache.shrink_to_fit();

        let new_idx = self.tabs.len();
        self.tabs.push(DocumentTab {
            doc: None,
            file_path: virtual_path.clone(),
            current_page_index: 0,
            view_mode: ViewMode::FitPage,
            last_page_view_mode: ViewMode::FitPage,
            zoom_level: 1.0,
            pan_x: 0.0,
            scroll_y: 0.0,
            nav_back_stack: VecDeque::new(),
            nav_forward_stack: Vec::new(),
            roman_end: 0,
            bookmarks: Vec::new(),
            selected_bookmark: None,
            bookmark_scroll_y: 0,
            grid_scroll_y: 0.0,
            bw_pages: HashSet::new(),
            undo_stack: VecDeque::with_capacity(MAX_UNDO_STATES),
            redacted: false,
            has_unsaved_edits: true,
            last_file_mtime: None,
        });

        self.active_tab = new_idx;
        self.doc = Some(loaded);
        self.file_path = virtual_path;
        self.current_page_index = 0;
        self.view_mode = ViewMode::FitPage;
        self.last_page_view_mode = ViewMode::FitPage;
        self.zoom_level = 1.0;
        self.pan_x = 0.0;
        self.scroll_y = 0.0;
        self.nav_back_stack.clear();
        self.nav_forward_stack.clear();
        self.hovered_link = None;
        self.roman_end = 0;
        self.bookmarks.clear();
        self.selected_bookmark = None;
        self.bookmark_scroll_y = 0;
        self.grid_scroll_y = 0.0;
        self.bw_pages.clear();
        self.undo_stack.clear();
        self.redacted = false;
        self.has_unsaved_edits = true;
        self.last_file_mtime = None;
        self.flatten_confirm_at = None;
        self.search_query.clear();
        self.search_matches.clear();
        self.current_search_match = 0;
        self.input_mode = InputMode::Normal;
        self.drag_start = None;
        self.grid_drag_page = None;

        self.update_title_bar();
        self.request_redraw();
        Ok(())
    }

    pub(super) fn switch_to_tab(&mut self, target_idx: usize) {
        if target_idx >= self.tabs.len() || target_idx == self.active_tab {
            return;
        }
        self.sync_active_tab_to_slot();
        // Zero-RAM discipline for inactive tabs: immediately free cached page and thumbnail bitmaps
        self.invalidate_page_caches();
        self.thumbnail_cache.clear();
        self.thumbnail_cache.shrink_to_fit();
        self.drag_start = None;
        self.grid_drag_page = None;
        self.hovered_thumb = None;
        self.input_mode = InputMode::Normal;

        self.load_slot_into_active(target_idx);
        let fname = self.filename_display();
        self.status_notice = Some(format!(
            "Switched to tab {}: '{}'",
            target_idx + 1,
            fname
        ));
        self.update_title_bar();
        self.request_redraw();
    }

    pub(super) fn cycle_next_tab(&mut self) {
        if self.tabs.len() <= 1 {
            return;
        }
        let next = (self.active_tab + 1) % self.tabs.len();
        self.switch_to_tab(next);
    }

    /// Closes the tab at `close_idx`. Returns `true` if the last remaining tab was closed.
    pub(super) fn close_tab(&mut self, close_idx: usize) -> bool {
        if close_idx >= self.tabs.len() {
            return false;
        }
        self.sync_active_tab_to_slot();
        self.invalidate_page_caches();
        self.thumbnail_cache.clear();
        self.thumbnail_cache.shrink_to_fit();

        let closed_name = self.tabs[close_idx]
            .file_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("tab")
            .to_string();
        self.tabs.remove(close_idx);

        if self.tabs.is_empty() {
            self.doc = None;
            return true;
        }

        let next_active = if close_idx < self.active_tab {
            self.active_tab - 1
        } else {
            self.active_tab.min(self.tabs.len() - 1)
        };
        self.load_slot_into_active(next_active);
        self.status_notice = Some(format!("Closed tab '{closed_name}'"));
        self.update_title_bar();
        self.request_redraw();
        false
    }


    pub(super) fn draw_tab_bar(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        menu_bar_h: u32,
        tab_bar_h: u32,
        scale_factor: f64,
    ) {
        if tab_bar_h == 0 {
            return;
        }
        let scale = (scale_factor.round() as u32).clamp(1, 2);
        let top_y = menu_bar_h;
        // Dark slate tab bar backdrop
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            0,
            top_y as i32,
            buf_w,
            tab_bar_h,
            0x0015181E,
        );
        // Bottom 1px divider
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            0,
            (top_y + tab_bar_h.saturating_sub(1)) as i32,
            buf_w,
            1,
            0x0037474F,
        );

        let num_tabs = self.tabs.len().max(1);
        let plus_btn_w = 34 * scale;
        let avail_for_tabs = buf_w.saturating_sub(plus_btn_w + 12 * scale);
        let tab_w = (avail_for_tabs / (num_tabs as u32)).clamp(90 * scale, 210 * scale);
        let text_y = top_y + tab_bar_h.saturating_sub(8 * scale) / 2;

        let mut cur_x = 4 * scale;
        for idx in 0..num_tabs {
            if cur_x + tab_w > buf_w.saturating_sub(plus_btn_w) {
                break;
            }
            let is_active = idx == self.active_tab;
            let tab_bg = if is_active { 0x00263238 } else { 0x001C2026 };
            let border_col = if is_active { 0x0042A5F5 } else { 0x0037474F };

            fill_rect_clipped(
                buffer,
                buf_w,
                buf_h,
                cur_x as i32,
                (top_y + 2) as i32,
                tab_w.saturating_sub(2),
                tab_bar_h.saturating_sub(3),
                tab_bg,
            );
            draw_rect_border(
                buffer,
                buf_w,
                buf_h,
                cur_x,
                top_y + 2,
                tab_w.saturating_sub(2),
                tab_bar_h.saturating_sub(3),
                border_col,
            );

            let fname = if is_active {
                self.filename_display()
            } else if let Some(slot) = self.tabs.get(idx) {
                slot.file_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("doc.pdf")
                    .to_string()
            } else {
                "doc.pdf".to_string()
            };

            let max_label_chars =
                ((tab_w.saturating_sub(36 * scale)) / (8 * scale)).max(4) as usize;
            let mut label = format!("{}: {}", idx + 1, fname);
            if label.chars().count() > max_label_chars {
                label = label.chars().take(max_label_chars).collect();
            }

            let text_col = if is_active { 0x00FFFFFF } else { 0x00B0BEC5 };
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                cur_x + 6 * scale,
                text_y,
                &label,
                text_col,
                scale,
            );

            // Close `[x]` button on right side of tab
            let close_x = cur_x + tab_w.saturating_sub(26 * scale);
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                close_x,
                text_y,
                "[x]",
                0x00EF5350,
                scale,
            );

            cur_x += tab_w;
        }

        // Draw `[ + ]` New Tab button
        let plus_x = cur_x + 4 * scale;
        if plus_x + plus_btn_w <= buf_w {
            fill_rect_clipped(
                buffer,
                buf_w,
                buf_h,
                plus_x as i32,
                (top_y + 2) as i32,
                plus_btn_w,
                tab_bar_h.saturating_sub(3),
                0x00263238,
            );
            draw_rect_border(
                buffer,
                buf_w,
                buf_h,
                plus_x,
                top_y + 2,
                plus_btn_w,
                tab_bar_h.saturating_sub(3),
                0x0064B5F6,
            );
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                plus_x + 5 * scale,
                text_y,
                "[+]",
                0x0064B5F6,
                scale,
            );
        }
    }

    pub(super) fn hit_test_tab_bar(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        menu_bar_h: u32,
        tab_bar_h: u32,
        scale_factor: f64,
    ) -> Option<TabBarHit> {
        let ix = mx.round() as u32;
        let iy = my.round() as u32;
        if iy < menu_bar_h || iy >= menu_bar_h + tab_bar_h {
            return None;
        }
        let scale = (scale_factor.round() as u32).clamp(1, 2);
        let num_tabs = self.tabs.len().max(1);
        let plus_btn_w = 34 * scale;
        let avail_for_tabs = buf_w.saturating_sub(plus_btn_w + 12 * scale);
        let tab_w = (avail_for_tabs / (num_tabs as u32)).clamp(90 * scale, 210 * scale);

        let mut cur_x = 4 * scale;
        for idx in 0..num_tabs {
            if cur_x + tab_w > buf_w.saturating_sub(plus_btn_w) {
                break;
            }
            if ix >= cur_x && ix < cur_x + tab_w {
                let close_x = cur_x + tab_w.saturating_sub(28 * scale);
                if ix >= close_x {
                    return Some(TabBarHit::CloseTab(idx));
                } else {
                    return Some(TabBarHit::SwitchTab(idx));
                }
            }
            cur_x += tab_w;
        }

        let plus_x = cur_x + 4 * scale;
        if ix >= plus_x && ix <= plus_x + plus_btn_w {
            return Some(TabBarHit::NewTab);
        }
        None
    }
}

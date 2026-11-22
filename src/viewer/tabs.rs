use crate::engine::PdfEngine;
use anyhow::Result;
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use super::state::*;
use super::render::fill_rect_clipped;

impl<'a> PdfViewerApp<'a> {
    pub(super) fn sync_active_tab_to_slot(&mut self) {
        if self.active_tab < self.tabs.len() {
            let slot = &mut self.tabs[self.active_tab];
            slot.doc = self.doc.take();
            slot.file_path = self.file_path.clone();
            slot.current_page_index = self.current_page_index;
            slot.view_mode = self.view_mode;
            slot.last_page_view_mode = self.last_page_view_mode;
            slot.scroll_mode = self.scroll_mode;
            slot.zoom_level = self.zoom_level;
            slot.pan_x = self.pan_x;
            slot.scroll_y = self.scroll_y;
            slot.scroll_x = self.scroll_x;
            slot.nav_back_stack = std::mem::take(&mut self.nav_back_stack);
            slot.nav_forward_stack = std::mem::take(&mut self.nav_forward_stack);
            slot.roman_end = self.roman_end;
            slot.skip_pages = self.skip_pages;
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
            self.scroll_mode = slot.scroll_mode;
            self.zoom_level = slot.zoom_level;
            self.pan_x = slot.pan_x;
            self.scroll_y = slot.scroll_y;
            self.scroll_x = slot.scroll_x;
            self.nav_back_stack = std::mem::take(&mut slot.nav_back_stack);
            self.nav_forward_stack = std::mem::take(&mut slot.nav_forward_stack);
            self.hovered_link = None;
            self.roman_end = slot.roman_end;
            self.skip_pages = slot.skip_pages;
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
            self.incremental_search = None;
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
        let (skip_pages, roman_end) = PdfEngine::read_page_labels_skip_and_roman_from_doc(&loaded);
        let loaded_bookmarks =
            PdfEngine::load_bookmarks_with_skip(&loaded, &path, skip_pages, roman_end);

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
        self.scroll_x = 0.0;
        self.nav_back_stack.clear();
        self.nav_forward_stack.clear();
        self.hovered_link = None;
        self.roman_end = roman_end;
        self.skip_pages = skip_pages;
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
        self.incremental_search = None;
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
                scroll_mode: self.scroll_mode,
                zoom_level: 1.0,
                pan_x: 0.0,
                scroll_y: 0.0,
                scroll_x: 0.0,
                nav_back_stack: VecDeque::new(),
                nav_forward_stack: Vec::new(),
                roman_end,
                skip_pages,
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
            self.tab_scroll_offset = 0;
        } else if self.active_tab < self.tabs.len() {
            self.tabs[self.active_tab].file_path = path;
            self.tabs[self.active_tab].current_page_index = 0;
            self.tabs[self.active_tab].zoom_level = 1.0;
            self.tabs[self.active_tab].pan_x = 0.0;
            self.tabs[self.active_tab].scroll_y = 0.0;
            self.tabs[self.active_tab].scroll_x = 0.0;
            self.tabs[self.active_tab].nav_back_stack.clear();
            self.tabs[self.active_tab].nav_forward_stack.clear();
            self.tabs[self.active_tab].roman_end = roman_end;
            self.tabs[self.active_tab].skip_pages = skip_pages;
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
        let (skip_pages, roman_end) = PdfEngine::read_page_labels_skip_and_roman_from_doc(&loaded);
        let loaded_bookmarks =
            PdfEngine::load_bookmarks_with_skip(&loaded, &path, skip_pages, roman_end);
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
            scroll_mode: self.scroll_mode,
            zoom_level: 1.0,
            pan_x: 0.0,
            scroll_y: 0.0,
            scroll_x: 0.0,
            nav_back_stack: VecDeque::new(),
            nav_forward_stack: Vec::new(),
            roman_end,
            skip_pages,
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
        self.scroll_x = 0.0;
        self.nav_back_stack.clear();
        self.nav_forward_stack.clear();
        self.hovered_link = None;
        self.roman_end = roman_end;
        self.skip_pages = skip_pages;
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
        self.incremental_search = None;
        self.input_mode = InputMode::Normal;
        self.drag_start = None;
        self.grid_drag_page = None;
        self.ensure_active_tab_scrolled_into_view();

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
            scroll_mode: self.scroll_mode,
            zoom_level: 1.0,
            pan_x: 0.0,
            scroll_y: 0.0,
            scroll_x: 0.0,
            nav_back_stack: VecDeque::new(),
            nav_forward_stack: Vec::new(),
            roman_end: 0,
            skip_pages: 0,
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
        self.scroll_x = 0.0;
        self.nav_back_stack.clear();
        self.nav_forward_stack.clear();
        self.hovered_link = None;
        self.roman_end = 0;
        self.skip_pages = 0;
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
        self.incremental_search = None;
        self.input_mode = InputMode::Normal;
        self.drag_start = None;
        self.grid_drag_page = None;
        self.ensure_active_tab_scrolled_into_view();

        self.update_title_bar();
        self.request_redraw();
        Ok(())
    }

    pub(super) fn switch_to_tab(&mut self, target_idx: usize) {
        if target_idx >= self.tabs.len() || target_idx == self.active_tab {
            self.show_tab_list_popover = false;
            self.tab_context_menu = None;
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
        self.thumb_context_menu = None;
        self.show_tab_list_popover = false;
        self.tab_context_menu = None;
        self.input_mode = InputMode::Normal;

        self.load_slot_into_active(target_idx);
        self.ensure_active_tab_scrolled_into_view();
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

    /// Resets active document fields back to the clean Welcome Screen state (0 MB document RAM).
    fn reset_to_welcome_screen(&mut self, notice: String) {
        self.doc = None;
        self.tabs.clear();
        self.active_tab = 0;
        self.tab_scroll_offset = 0;
        self.show_tab_list_popover = false;
        self.tab_list_scroll = 0;
        self.tab_context_menu = None;
        self.thumb_context_menu = None;
        self.file_path = PathBuf::new();
        self.current_page_index = 0;
        self.view_mode = ViewMode::FitPage;
        self.last_page_view_mode = ViewMode::FitPage;
        self.zoom_level = 1.0;
        self.pan_x = 0.0;
        self.scroll_y = 0.0;
        self.scroll_x = 0.0;
        self.nav_back_stack.clear();
        self.nav_forward_stack.clear();
        self.hovered_link = None;
        self.roman_end = 0;
        self.skip_pages = 0;
        self.bookmarks.clear();
        self.selected_bookmark = None;
        self.bookmark_scroll_y = 0;
        self.grid_scroll_y = 0.0;
        self.bw_pages.clear();
        self.undo_stack.clear();
        self.redacted = false;
        self.has_unsaved_edits = false;
        self.last_file_mtime = None;
        self.flatten_confirm_at = None;
        self.search_query.clear();
        self.search_matches.clear();
        self.current_search_match = 0;
        self.incremental_search = None;
        self.input_mode = InputMode::Normal;
        self.drag_start = None;
        self.grid_drag_page = None;
        self.cached_text_view = None;
        self.active_note_popover = None;
        self.dragging_annotation = None;
        self.status_notice = Some(notice);
        self.update_title_bar();
        self.request_redraw();
    }

    /// Closes the tab at `close_idx`. Returns `true` if the last remaining tab was closed
    /// (which cleanly returns MicroPDF to the Welcome Screen).
    pub(super) fn close_tab(&mut self, close_idx: usize) -> bool {
        if close_idx >= self.tabs.len() {
            return false;
        }
        self.sync_active_tab_to_slot();
        self.invalidate_page_caches();
        self.thumbnail_cache.clear();
        self.thumbnail_cache.shrink_to_fit();
        self.tab_context_menu = None;
        self.thumb_context_menu = None;

        let closed_name = self.tabs[close_idx]
            .file_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("tab")
            .to_string();
        self.tabs.remove(close_idx);

        if self.tabs.is_empty() {
            self.reset_to_welcome_screen(format!("Closed '{closed_name}' — Welcome Screen"));
            return true;
        }

        let next_active = if close_idx < self.active_tab {
            self.active_tab - 1
        } else {
            self.active_tab.min(self.tabs.len() - 1)
        };
        self.load_slot_into_active(next_active);
        self.ensure_active_tab_scrolled_into_view();
        self.status_notice = Some(format!("Closed tab '{closed_name}'"));
        self.update_title_bar();
        self.request_redraw();
        false
    }

    /// Closes every open tab EXCEPT `keep_idx`, freeing all closed PDFs immediately.
    pub(super) fn close_other_tabs(&mut self, keep_idx: usize) {
        if self.tabs.len() <= 1 || keep_idx >= self.tabs.len() {
            self.tab_context_menu = None;
            self.show_tab_list_popover = false;
            return;
        }
        if keep_idx != self.active_tab {
            self.switch_to_tab(keep_idx);
        }
        let closed_count = self.tabs.len() - 1;
        let active_slot = self.tabs.remove(self.active_tab);
        self.tabs.clear();
        self.tabs.push(active_slot);
        self.active_tab = 0;
        self.tab_scroll_offset = 0;
        self.show_tab_list_popover = false;
        self.tab_context_menu = None;
        let kept = self.filename_display();
        self.status_notice = Some(format!(
            "Closed {closed_count} other tab(s) — kept '{kept}'"
        ));
        self.update_title_bar();
        self.request_redraw();
    }

    /// Closes all tabs to the right of `anchor_idx`.
    pub(super) fn close_tabs_to_right(&mut self, anchor_idx: usize) {
        if anchor_idx + 1 >= self.tabs.len() {
            self.tab_context_menu = None;
            return;
        }
        let closed_count = self.tabs.len() - (anchor_idx + 1);
        if self.active_tab > anchor_idx {
            self.switch_to_tab(anchor_idx);
        }
        self.tabs.truncate(anchor_idx + 1);
        self.tab_context_menu = None;
        self.ensure_active_tab_scrolled_into_view();
        self.status_notice = Some(format!("Closed {closed_count} tab(s) to the right"));
        self.update_title_bar();
        self.request_redraw();
    }

    /// Closes all open tabs and returns cleanly to the Welcome Screen.
    pub(super) fn close_all_tabs(&mut self) {
        if self.tabs.is_empty() && self.doc.is_none() {
            self.tab_context_menu = None;
            self.show_tab_list_popover = false;
            return;
        }
        let count = self.tabs.len().max(1);
        self.invalidate_page_caches();
        self.thumbnail_cache.clear();
        self.thumbnail_cache.shrink_to_fit();
        self.reset_to_welcome_screen(format!("Closed all {count} tab(s) — Welcome Screen"));
    }

    /// Computes horizontal tab strip geometry (supports Option A: Scrollable Tab Strip + `▾` All-Tabs button).
    /// Returns `(overflowing, visible_start, visible_end_excl, tab_w, strip_x, left_arr_x, right_arr_x, plus_x, list_x, btn_w, list_w)`.
    pub(super) fn tab_strip_layout(
        &self,
        buf_w: u32,
        scale_factor: f64,
    ) -> (bool, usize, usize, u32, i32, i32, i32, i32, i32, u32, u32) {
        let s = scale_factor as f32;
        let num_tabs = self.tabs.len().max(1);
        let pad_l = (8.0 * s).round() as i32;
        let pad_r = (8.0 * s).round() as i32;
        let tab_gap = (3.0 * s).round() as i32;
        let plus_w = (26.0 * s).round() as u32;
        let list_lbl = if self.tabs.len() > 1 {
            format!("{} ▾", self.tabs.len())
        } else {
            "▾".to_string()
        };
        let font_sz = (11.0 * s).clamp(9.5, 22.0);
        let list_w = (super::ui_draw::measure_text(&list_lbl, font_sz, true).ceil() as u32
            + (14.0 * s).round() as u32)
            .max((26.0 * s).round() as u32);
        let arr_w = (22.0 * s).round() as u32;
        let min_tab_w = (128.0 * s).round() as u32;
        let max_tab_w = (210.0 * s).round() as u32;

        let right_controls_w = plus_w + list_w + (8.0 * s).round() as u32;
        let base_avail = buf_w
            .saturating_sub((pad_l + pad_r) as u32 + right_controls_w)
            .max(min_tab_w);

        let needed_at_min = (num_tabs as u32) * min_tab_w + (num_tabs.saturating_sub(1) as u32) * (tab_gap as u32);
        let overflowing = self.tabs.len() > 1 && needed_at_min > base_avail;

        let (strip_x, strip_avail, left_arr_x, right_arr_x) = if overflowing {
            let sx = pad_l + arr_w as i32 + 3;
            let sa = base_avail.saturating_sub(arr_w * 2 + 8).max(min_tab_w);
            let rx = sx + sa as i32 + 3;
            (sx, sa, pad_l, rx)
        } else {
            (pad_l, base_avail, -100, -100)
        };

        let (tab_w, visible_count) = if overflowing {
            let vc = ((strip_avail + tab_gap as u32) / (min_tab_w + tab_gap as u32)).max(1) as usize;
            let tw = ((strip_avail.saturating_sub((vc.saturating_sub(1) as u32) * (tab_gap as u32)))
                / (vc as u32))
                .clamp(min_tab_w, max_tab_w);
            (tw, vc.min(num_tabs))
        } else {
            let tw = ((strip_avail.saturating_sub((num_tabs.saturating_sub(1) as u32) * (tab_gap as u32)))
                / (num_tabs as u32))
                .clamp(min_tab_w, max_tab_w);
            (tw, num_tabs)
        };

        let max_offset = num_tabs.saturating_sub(visible_count);
        let visible_start = if overflowing {
            self.tab_scroll_offset.min(max_offset)
        } else {
            0
        };
        let visible_end = (visible_start + visible_count).min(num_tabs);

        let list_x = buf_w as i32 - pad_r - list_w as i32;
        let plus_x = if overflowing {
            list_x - plus_w as i32 - 4
        } else {
            let end_of_tabs = strip_x + ((visible_end - visible_start) as i32) * (tab_w as i32 + tab_gap);
            (end_of_tabs + 2).min(list_x - plus_w as i32 - 4)
        };

        (
            overflowing,
            visible_start,
            visible_end,
            tab_w,
            strip_x,
            left_arr_x,
            right_arr_x,
            plus_x,
            list_x,
            plus_w,
            list_w,
        )
    }

    pub(super) fn ensure_active_tab_scrolled_into_view(&mut self) {
        let (buf_w, sf) = if let Some(w) = self.window.as_ref() {
            (w.inner_size().width.max(320), self.effective_scale_factor(w.scale_factor()))
        } else {
            (1024, 1.0)
        };
        let (overflowing, vis_start, vis_end, _, _, _, _, _, _, _, _) = self.tab_strip_layout(buf_w, sf);
        if !overflowing {
            self.tab_scroll_offset = 0;
            return;
        }
        let vis_count = vis_end.saturating_sub(vis_start).max(1);
        if self.active_tab < self.tab_scroll_offset {
            self.tab_scroll_offset = self.active_tab;
        } else if self.active_tab >= self.tab_scroll_offset + vis_count {
            self.tab_scroll_offset = self.active_tab + 1 - vis_count;
        }
    }

    pub(super) fn scroll_tab_strip(&mut self, delta: isize) {
        let (buf_w, sf) = if let Some(w) = self.window.as_ref() {
            (w.inner_size().width.max(320), self.effective_scale_factor(w.scale_factor()))
        } else {
            (1024, 1.0)
        };
        let (overflowing, vis_start, vis_end, _, _, _, _, _, _, _, _) = self.tab_strip_layout(buf_w, sf);
        if !overflowing {
            self.tab_scroll_offset = 0;
            return;
        }
        let vis_count = vis_end.saturating_sub(vis_start).max(1);
        let max_off = self.tabs.len().saturating_sub(vis_count);
        let next = (self.tab_scroll_offset as isize + delta).clamp(0, max_off as isize) as usize;
        if next != self.tab_scroll_offset {
            self.tab_scroll_offset = next;
            self.request_redraw();
        }
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
        let theme = self.ui_theme();
        let s = scale_factor as f32;
        let top_y = menu_bar_h as i32;
        let font_sz = (11.5 * s).clamp(10.0, 24.0);
        let badge_sz = (10.5 * s).clamp(9.0, 20.0);

        fill_rect_clipped(buffer, buf_w, buf_h, 0, top_y, buf_w, tab_bar_h, theme.tabbar_bg);
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            0,
            top_y + tab_bar_h.saturating_sub(1) as i32,
            buf_w,
            1,
            theme.divider,
        );

        let (
            overflowing,
            vis_start,
            vis_end,
            tab_w,
            strip_x,
            left_arr_x,
            right_arr_x,
            plus_x,
            list_x,
            plus_btn_w,
            list_btn_w,
        ) = self.tab_strip_layout(buf_w, scale_factor);

        let tab_gap = (3.0 * s).round() as i32;
        let tab_y = top_y + (3.0 * s).round() as i32;
        let tab_h = tab_bar_h.saturating_sub((3.0 * s).round() as u32);
        let rad = (6.0 * s).clamp(4.0, 10.0);
        let close_sz = (16.0 * s).round() as u32;
        let close_ic = (10.0 * s).round() as u32;
        let btn_h = (22.0 * s).round() as u32;
        let btn_y = top_y + ((tab_bar_h.saturating_sub(btn_h)) / 2) as i32;
        let arr_w = (22.0 * s).round() as u32;
        let arr_ic = (11.0 * s).round() as u32;

        // Scroll arrows `<` and `>` only when tabs overflow the window width
        if overflowing {
            let can_left = vis_start > 0;
            let can_right = vis_end < self.tabs.len();
            let l_hov = can_left && self.hovered_widget == Some(UiHit::TabScrollLeft);
            if l_hov {
                super::ui_draw::fill_rounded_rect(buffer, buf_w, buf_h, left_arr_x, btn_y, arr_w, btn_h, (4.0 * s).max(3.0), theme.hover_bg);
            }
            super::ui_draw::draw_icon(
                buffer, buf_w, buf_h, super::ui_draw::UiIcon::ChevronLeft,
                left_arr_x + ((arr_w.saturating_sub(arr_ic)) / 2) as i32,
                btn_y + ((btn_h.saturating_sub(arr_ic)) / 2) as i32,
                arr_ic,
                if can_left { theme.text_primary } else { theme.text_muted },
            );

            let r_hov = can_right && self.hovered_widget == Some(UiHit::TabScrollRight);
            if r_hov {
                super::ui_draw::fill_rounded_rect(buffer, buf_w, buf_h, right_arr_x, btn_y, arr_w, btn_h, (4.0 * s).max(3.0), theme.hover_bg);
            }
            super::ui_draw::draw_icon(
                buffer, buf_w, buf_h, super::ui_draw::UiIcon::ChevronRight,
                right_arr_x + ((arr_w.saturating_sub(arr_ic)) / 2) as i32,
                btn_y + ((btn_h.saturating_sub(arr_ic)) / 2) as i32,
                arr_ic,
                if can_right { theme.text_primary } else { theme.text_muted },
            );
        }

        let mut cur_x = strip_x;
        for idx in vis_start..vis_end {
            let is_active = idx == self.active_tab;
            let is_tab_hov = matches!(
                self.hovered_widget,
                Some(UiHit::Tab(i)) | Some(UiHit::TabClose(i)) if i == idx
            );
            let is_close_hov = self.hovered_widget == Some(UiHit::TabClose(idx));

            if is_active {
                super::ui_draw::fill_top_rounded_rect(
                    buffer, buf_w, buf_h, cur_x, tab_y, tab_w, tab_h, rad, theme.tab_active_bg,
                );
                super::ui_draw::stroke_rounded_rect(
                    buffer, buf_w, buf_h, cur_x, tab_y, tab_w, tab_h.saturating_sub(1), rad, theme.tab_active_border,
                );
                fill_rect_clipped(
                    buffer, buf_w, buf_h,
                    cur_x + 1, tab_y + tab_h as i32 - 2, tab_w.saturating_sub(2), 2,
                    theme.tab_active_bg,
                );
                super::ui_draw::fill_rounded_rect(
                    buffer, buf_w, buf_h,
                    cur_x + (6.0 * s).round() as i32, tab_y,
                    tab_w.saturating_sub((12.0 * s).round() as u32),
                    (2.0 * s).round().max(2.0) as u32,
                    1.0, theme.accent,
                );
            } else if is_tab_hov {
                super::ui_draw::fill_top_rounded_rect(
                    buffer, buf_w, buf_h, cur_x, tab_y + 1, tab_w, tab_h.saturating_sub(2), rad, theme.tab_hover_bg,
                );
            }

            let is_dirty = if is_active {
                self.has_unsaved_edits
            } else {
                self.tabs.get(idx).map(|t| t.has_unsaved_edits).unwrap_or(false)
            };

            let fname = if is_active {
                self.filename_display()
            } else if let Some(slot) = self.tabs.get(idx) {
                slot.file_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("document.pdf")
                    .to_string()
            } else {
                "No PDF Open".to_string()
            };

            let mut text_x = cur_x + (10.0 * s).round() as i32;
            if is_dirty {
                let dot_r = (3.0 * s).clamp(2.5, 5.0);
                super::ui_draw::fill_circle_aa(
                    buffer, buf_w, buf_h,
                    text_x as f32 + dot_r, tab_y as f32 + (tab_h as f32) * 0.5,
                    dot_r, theme.accent,
                );
                text_x += (dot_r * 2.0 + 6.0 * s).round() as i32;
            }

            let show_close = !self.tabs.is_empty();
            let close_x = cur_x + tab_w as i32 - close_sz as i32 - (6.0 * s).round() as i32;
            let close_y = tab_y + ((tab_h.saturating_sub(close_sz)) / 2) as i32;

            let label_right = if show_close { close_x - 2 } else { cur_x + tab_w as i32 - 6 };
            let label_budget = (label_right - text_x - (2.0 * s).round() as i32).max(20) as f32;
            let label = super::ui_draw::truncate_ellipsis(&fname, label_budget, font_sz, is_active);
            let text_col = if is_active { theme.text_primary } else { theme.text_secondary };
            super::ui_draw::draw_text_vcenter(
                buffer, buf_w, buf_h, text_x, tab_y, tab_h, label_right, &label, font_sz, text_col, is_active,
            );

            if show_close {
                if is_close_hov {
                    super::ui_draw::fill_rounded_rect(
                        buffer, buf_w, buf_h, close_x, close_y, close_sz, close_sz, (close_sz as f32) * 0.5, theme.danger_soft_bg,
                    );
                }
                super::ui_draw::draw_icon(
                    buffer, buf_w, buf_h, super::ui_draw::UiIcon::Close,
                    close_x + ((close_sz.saturating_sub(close_ic)) / 2) as i32,
                    close_y + ((close_sz.saturating_sub(close_ic)) / 2) as i32,
                    close_ic,
                    if is_close_hov { theme.danger } else { theme.text_muted },
                );
            }

            cur_x += tab_w as i32 + tab_gap;
        }

        // `+` New Tab button
        let plus_hov = self.hovered_widget == Some(UiHit::TabNew);
        if plus_hov {
            super::ui_draw::fill_rounded_rect(
                buffer, buf_w, buf_h, plus_x, btn_y, plus_btn_w, btn_h, (5.0 * s).max(3.0), theme.hover_bg,
            );
        }
        let ic = (12.0 * s).round() as u32;
        super::ui_draw::draw_icon(
            buffer, buf_w, buf_h, super::ui_draw::UiIcon::Plus,
            plus_x + ((plus_btn_w.saturating_sub(ic)) / 2) as i32,
            btn_y + ((btn_h.saturating_sub(ic)) / 2) as i32,
            ic,
            if plus_hov { theme.accent } else { theme.text_secondary },
        );

        // `▾` All-Tabs overflow list button (always reachable on the right; displays count when >1)
        let list_hov = self.hovered_widget == Some(UiHit::TabListToggle) || self.show_tab_list_popover;
        let list_bg = if self.show_tab_list_popover {
            theme.accent_soft_bg
        } else if list_hov {
            theme.hover_bg
        } else if overflowing {
            theme.surface_elevated
        } else {
            theme.tabbar_bg
        };
        if list_hov || overflowing {
            super::ui_draw::fill_rounded_rect(
                buffer, buf_w, buf_h, list_x, btn_y, list_btn_w, btn_h, (5.0 * s).max(3.0), list_bg,
            );
            super::ui_draw::stroke_rounded_rect(
                buffer, buf_w, buf_h, list_x, btn_y, list_btn_w, btn_h, (5.0 * s).max(3.0),
                if self.show_tab_list_popover { theme.accent } else { theme.border_subtle },
            );
        }
        let list_lbl = if self.tabs.len() > 1 {
            format!("{} ▾", self.tabs.len())
        } else {
            "▾".to_string()
        };
        let lw = super::ui_draw::measure_text(&list_lbl, badge_sz, true);
        let lfg = if self.show_tab_list_popover {
            theme.accent_soft_fg
        } else if overflowing || list_hov {
            theme.text_primary
        } else {
            theme.text_secondary
        };
        super::ui_draw::draw_text_vcenter(
            buffer, buf_w, buf_h,
            list_x + ((list_btn_w as f32 - lw) * 0.5).round() as i32,
            btn_y, btn_h, list_x + list_btn_w as i32,
            &list_lbl, badge_sz, lfg, true,
        );
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
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if iy < menu_bar_h as i32 || iy >= (menu_bar_h + tab_bar_h) as i32 {
            return None;
        }
        let s = scale_factor as f32;
        let (
            overflowing,
            vis_start,
            vis_end,
            tab_w,
            strip_x,
            left_arr_x,
            right_arr_x,
            plus_x,
            list_x,
            plus_btn_w,
            list_btn_w,
        ) = self.tab_strip_layout(buf_w, scale_factor);

        let arr_w = (22.0 * s).round() as i32;
        if overflowing {
            if ix >= left_arr_x && ix < left_arr_x + arr_w {
                return Some(TabBarHit::ScrollLeft);
            }
            if ix >= right_arr_x && ix < right_arr_x + arr_w {
                return Some(TabBarHit::ScrollRight);
            }
        }
        if ix >= list_x && ix <= list_x + list_btn_w as i32 {
            return Some(TabBarHit::TabListToggle);
        }
        if ix >= plus_x && ix <= plus_x + plus_btn_w as i32 {
            return Some(TabBarHit::NewTab);
        }

        let tab_gap = (3.0 * s).round() as i32;
        let close_sz = (18.0 * s).round() as i32;
        let mut cur_x = strip_x;
        for idx in vis_start..vis_end {
            if ix >= cur_x && ix < cur_x + tab_w as i32 {
                let close_x = cur_x + tab_w as i32 - close_sz - (6.0 * s).round() as i32;
                if !self.tabs.is_empty() && ix >= close_x {
                    return Some(TabBarHit::CloseTab(idx));
                } else {
                    return Some(TabBarHit::SwitchTab(idx));
                }
            }
            cur_x += tab_w as i32 + tab_gap;
        }
        None
    }

    // -------------------------------------------------------------------------
    // All-Tabs Overflow Dropdown Popover (`▾`)
    // -------------------------------------------------------------------------

    fn tab_list_popover_geometry(
        &self,
        buf_w: u32,
        buf_h: u32,
        top_bars_h: u32,
        sf: f64,
    ) -> (i32, i32, u32, u32, u32, u32, u32, usize) {
        let s = sf as f32;
        let pw = ((290.0 * s).round() as u32).min(buf_w.saturating_sub(24));
        let hdr_h = (28.0 * s).round() as u32;
        let row_h = (28.0 * s).round() as u32;
        let foot_h = (34.0 * s).round() as u32;
        let max_rows = (((buf_h.saturating_sub(top_bars_h + hdr_h + foot_h + 60)) / row_h.max(1)) as usize)
            .clamp(4, 14);
        let vis_rows = self.tabs.len().max(1).min(max_rows);
        let ph = hdr_h + (vis_rows as u32) * row_h + foot_h + (8.0 * s).round() as u32;
        let px = (buf_w as i32 - pw as i32 - (8.0 * s).round() as i32).max(8);
        let py = top_bars_h as i32 + (4.0 * s).round() as i32;
        (px, py, pw, ph, hdr_h, row_h, foot_h, vis_rows)
    }

    pub(super) fn draw_tab_list_popover(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        top_bars_h: u32,
        sf: f64,
    ) {
        if !self.show_tab_list_popover {
            return;
        }
        let theme = self.ui_theme();
        let s = sf as f32;
        let (px, py, pw, ph, hdr_h, row_h, foot_h, vis_rows) =
            self.tab_list_popover_geometry(buf_w, buf_h, top_bars_h, sf);
        let rad = (8.0 * s).clamp(6.0, 12.0);
        let lbl_sz = (11.0 * s).clamp(9.5, 20.0);
        let body_sz = (11.5 * s).clamp(10.0, 22.0);
        let close_sz = (18.0 * s).round() as u32;
        let close_ic = (10.0 * s).round() as u32;

        super::ui_draw::draw_soft_shadow(
            buffer, buf_w, buf_h, px, py, pw, ph, rad,
            (12.0 * s).round() as i32, (4.0 * s).round() as i32,
            theme.shadow_rgb, theme.shadow_max_alpha,
        );
        super::ui_draw::fill_rounded_rect(buffer, buf_w, buf_h, px, py, pw, ph, rad, theme.surface_bg);
        super::ui_draw::stroke_rounded_rect(buffer, buf_w, buf_h, px, py, pw, ph, rad, theme.border_subtle);

        let hdr_txt = format!("Open Tabs ({})", self.tabs.len());
        super::ui_draw::draw_text_vcenter(
            buffer, buf_w, buf_h, px + (12.0 * s).round() as i32, py, hdr_h,
            px + pw as i32 - 10, &hdr_txt, lbl_sz, theme.text_muted, true,
        );
        fill_rect_clipped(buffer, buf_w, buf_h, px + 1, py + hdr_h as i32, pw.saturating_sub(2), 1, theme.divider);

        let list_top = py + hdr_h as i32 + (4.0 * s).round() as i32;
        if self.tabs.is_empty() {
            super::ui_draw::draw_text_vcenter(
                buffer, buf_w, buf_h, px + (12.0 * s).round() as i32, list_top, row_h,
                px + pw as i32 - 12, "No open PDFs", body_sz, theme.text_muted, false,
            );
        } else {
            let max_scroll = self.tabs.len().saturating_sub(vis_rows);
            let start = self.tab_list_scroll.min(max_scroll);
            for slot_i in 0..vis_rows {
                let idx = start + slot_i;
                if idx >= self.tabs.len() {
                    break;
                }
                let ry = list_top + (slot_i as i32) * (row_h as i32);
                let is_active = idx == self.active_tab;
                let is_hov = matches!(
                    self.hovered_widget,
                    Some(UiHit::TabList(TabListHit::SwitchTab(i)))
                        | Some(UiHit::TabList(TabListHit::CloseTab(i))) if i == idx
                );
                let is_close_hov = self.hovered_widget == Some(UiHit::TabList(TabListHit::CloseTab(idx)));
                if is_active {
                    super::ui_draw::fill_rounded_rect(
                        buffer, buf_w, buf_h, px + 6, ry + 1, pw.saturating_sub(12), row_h.saturating_sub(2),
                        (5.0 * s).max(3.0), theme.accent_soft_bg,
                    );
                } else if is_hov {
                    super::ui_draw::fill_rounded_rect(
                        buffer, buf_w, buf_h, px + 6, ry + 1, pw.saturating_sub(12), row_h.saturating_sub(2),
                        (5.0 * s).max(3.0), theme.hover_bg,
                    );
                }
                let fname = if is_active {
                    self.filename_display()
                } else {
                    self.tabs[idx]
                        .file_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("document.pdf")
                        .to_string()
                };
                let is_dirty = if is_active {
                    self.has_unsaved_edits
                } else {
                    self.tabs[idx].has_unsaved_edits
                };
                let prefix = if is_active { "✓ " } else { "" };
                let dirty = if is_dirty { "• " } else { "" };
                let full_lbl = format!("{prefix}{dirty}{}. {fname}", idx + 1);
                let cx = px + pw as i32 - close_sz as i32 - (10.0 * s).round() as i32;
                let cy = ry + ((row_h.saturating_sub(close_sz)) / 2) as i32;
                let tx = px + (12.0 * s).round() as i32;
                let clipped = super::ui_draw::truncate_ellipsis(&full_lbl, (cx - tx - 6).max(20) as f32, body_sz, is_active);
                let fg = if is_active { theme.accent_soft_fg } else { theme.text_primary };
                super::ui_draw::draw_text_vcenter(
                    buffer, buf_w, buf_h, tx, ry, row_h, cx - 4, &clipped, body_sz, fg, is_active,
                );
                if is_close_hov {
                    super::ui_draw::fill_rounded_rect(
                        buffer, buf_w, buf_h, cx, cy, close_sz, close_sz, (close_sz as f32) * 0.5, theme.danger_soft_bg,
                    );
                }
                super::ui_draw::draw_icon(
                    buffer, buf_w, buf_h, super::ui_draw::UiIcon::Close,
                    cx + ((close_sz.saturating_sub(close_ic)) / 2) as i32,
                    cy + ((close_sz.saturating_sub(close_ic)) / 2) as i32,
                    close_ic,
                    if is_close_hov { theme.danger } else { theme.text_muted },
                );
            }
        }

        // Footer actions: Close Other Tabs | Close All Tabs
        let foot_y = py + ph as i32 - foot_h as i32;
        fill_rect_clipped(buffer, buf_w, buf_h, px + 1, foot_y, pw.saturating_sub(2), 1, theme.divider);
        let pad = (8.0 * s).round() as i32;
        let gap = (6.0 * s).round() as i32;
        let btn_w = (pw.saturating_sub((pad * 2 + gap) as u32)) / 2;
        let bh = (24.0 * s).round() as u32;
        let by = foot_y + ((foot_h.saturating_sub(bh)) / 2) as i32;
        let b_rad = (5.0 * s).max(3.0);

        let o_hov = self.hovered_widget == Some(UiHit::TabList(TabListHit::CloseOthers));
        super::ui_draw::fill_rounded_rect(buffer, buf_w, buf_h, px + pad, by, btn_w, bh, b_rad, if o_hov { theme.hover_bg } else { theme.surface_elevated });
        super::ui_draw::stroke_rounded_rect(buffer, buf_w, buf_h, px + pad, by, btn_w, bh, b_rad, theme.border_subtle);
        let o_txt = "Close Others";
        let ow = super::ui_draw::measure_text(o_txt, lbl_sz, false);
        super::ui_draw::draw_text_vcenter(
            buffer, buf_w, buf_h, px + pad + ((btn_w as f32 - ow) * 0.5).round() as i32,
            by, bh, px + pad + btn_w as i32 - 2, o_txt, lbl_sz,
            if self.tabs.len() > 1 { theme.text_primary } else { theme.text_muted }, false,
        );

        let ax = px + pad + btn_w as i32 + gap;
        let a_hov = self.hovered_widget == Some(UiHit::TabList(TabListHit::CloseAll));
        super::ui_draw::fill_rounded_rect(buffer, buf_w, buf_h, ax, by, btn_w, bh, b_rad, if a_hov { theme.danger_soft_bg } else { theme.surface_elevated });
        super::ui_draw::stroke_rounded_rect(buffer, buf_w, buf_h, ax, by, btn_w, bh, b_rad, if a_hov { theme.danger } else { theme.border_subtle });
        let a_txt = "Close All";
        let aw = super::ui_draw::measure_text(a_txt, lbl_sz, true);
        super::ui_draw::draw_text_vcenter(
            buffer, buf_w, buf_h, ax + ((btn_w as f32 - aw) * 0.5).round() as i32,
            by, bh, ax + btn_w as i32 - 2, a_txt, lbl_sz,
            if !self.tabs.is_empty() { theme.danger } else { theme.text_muted }, true,
        );
    }

    pub(super) fn hit_test_tab_list_popover(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        top_bars_h: u32,
        sf: f64,
    ) -> Option<TabListHit> {
        if !self.show_tab_list_popover {
            return None;
        }
        let s = sf as f32;
        let (px, py, pw, ph, hdr_h, row_h, foot_h, vis_rows) =
            self.tab_list_popover_geometry(buf_w, buf_h, top_bars_h, sf);
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix < px || ix >= px + pw as i32 || iy < py || iy >= py + ph as i32 {
            return None;
        }
        let foot_y = py + ph as i32 - foot_h as i32;
        if iy >= foot_y {
            let mid = px + (pw as i32) / 2;
            if ix < mid {
                return Some(TabListHit::CloseOthers);
            } else {
                return Some(TabListHit::CloseAll);
            }
        }
        let list_top = py + hdr_h as i32 + (4.0 * s).round() as i32;
        if iy >= list_top && !self.tabs.is_empty() {
            let slot_i = ((iy - list_top) / (row_h as i32)) as usize;
            let max_scroll = self.tabs.len().saturating_sub(vis_rows);
            let idx = self.tab_list_scroll.min(max_scroll) + slot_i;
            if slot_i < vis_rows && idx < self.tabs.len() {
                let close_x = px + pw as i32 - (28.0 * s).round() as i32;
                if ix >= close_x {
                    return Some(TabListHit::CloseTab(idx));
                } else {
                    return Some(TabListHit::SwitchTab(idx));
                }
            }
        }
        Some(TabListHit::InsidePopover)
    }

    // -------------------------------------------------------------------------
    // Right-Click Tab Context Menu (Close Tab / Close Others / Close Right / Close All)
    // -------------------------------------------------------------------------

    fn tab_context_menu_geometry(
        &self,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<(usize, i32, i32, u32, u32, u32)> {
        let (tab_idx, rx, ry) = self.tab_context_menu?;
        let s = sf as f32;
        let mw = (188.0 * s).round() as u32;
        let row_h = (28.0 * s).round() as u32;
        let mh = row_h * 4 + (10.0 * s).round() as u32;
        let x = rx.clamp(6, (buf_w as i32 - mw as i32 - 6).max(6));
        let y = ry.clamp(6, (buf_h as i32 - mh as i32 - 6).max(6));
        Some((tab_idx, x, y, mw, mh, row_h))
    }

    pub(super) fn draw_tab_context_menu(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) {
        let Some((tab_idx, mx, my, mw, mh, row_h)) = self.tab_context_menu_geometry(buf_w, buf_h, sf) else {
            return;
        };
        let theme = self.ui_theme();
        let s = sf as f32;
        let rad = (7.0 * s).clamp(5.0, 10.0);
        let body_sz = (11.5 * s).clamp(10.0, 20.0);

        super::ui_draw::draw_soft_shadow(
            buffer, buf_w, buf_h, mx, my, mw, mh, rad,
            (10.0 * s).round() as i32, (3.0 * s).round() as i32,
            theme.shadow_rgb, theme.shadow_max_alpha,
        );
        super::ui_draw::fill_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.surface_bg);
        super::ui_draw::stroke_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.border_subtle);

        let items: [(TabContextHit, &str, bool, bool); 4] = [
            (TabContextHit::CloseThis, "Close Tab", true, false),
            (TabContextHit::CloseOthers, "Close Other Tabs", self.tabs.len() > 1, false),
            (TabContextHit::CloseRight, "Close Tabs to Right", tab_idx + 1 < self.tabs.len(), false),
            (TabContextHit::CloseAll, "Close All Tabs", !self.tabs.is_empty(), true),
        ];

        let top_pad = (5.0 * s).round() as i32;
        for (i, &(hit, label, enabled, danger)) in items.iter().enumerate() {
            let ry = my + top_pad + (i as i32) * (row_h as i32);
            let hov = enabled && self.hovered_widget == Some(UiHit::TabContext(hit));
            if hov {
                let bg = if danger { theme.danger_soft_bg } else { theme.hover_bg };
                super::ui_draw::fill_rounded_rect(
                    buffer, buf_w, buf_h, mx + 5, ry + 1, mw.saturating_sub(10), row_h.saturating_sub(2),
                    (4.0 * s).max(3.0), bg,
                );
            }
            let fg = if !enabled {
                theme.text_muted
            } else if danger {
                theme.danger
            } else {
                theme.text_primary
            };
            super::ui_draw::draw_text_vcenter(
                buffer, buf_w, buf_h, mx + (12.0 * s).round() as i32, ry, row_h,
                mx + mw as i32 - 8, label, body_sz, fg, danger && enabled,
            );
        }
    }

    pub(super) fn hit_test_tab_context_menu(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<TabContextHit> {
        let (_, x, y, mw, mh, row_h) = self.tab_context_menu_geometry(buf_w, buf_h, sf)?;
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix < x || ix >= x + mw as i32 || iy < y || iy >= y + mh as i32 {
            return None;
        }
        let s = sf as f32;
        let top_pad = (5.0 * s).round() as i32;
        let idx = ((iy - (y + top_pad)) / (row_h as i32)).clamp(0, 3) as usize;
        Some(match idx {
            0 => TabContextHit::CloseThis,
            1 => TabContextHit::CloseOthers,
            2 => TabContextHit::CloseRight,
            _ => TabContextHit::CloseAll,
        })
    }

    pub(super) fn execute_tab_context_action(&mut self, hit: TabContextHit) {
        let Some((tab_idx, _, _)) = self.tab_context_menu.take() else {
            return;
        };
        match hit {
            TabContextHit::CloseThis => {
                let _ = self.close_tab(tab_idx);
            }
            TabContextHit::CloseOthers => {
                self.close_other_tabs(tab_idx);
            }
            TabContextHit::CloseRight => {
                self.close_tabs_to_right(tab_idx);
            }
            TabContextHit::CloseAll => {
                self.close_all_tabs();
            }
        }
    }
}

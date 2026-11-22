//! Unified Top Menu + Inline Customizable Toolbar bar and shadowed rounded dropdown menus.

use super::render::fill_rect_clipped;
use super::state::*;
use super::ui_draw::*;

impl<'a> PdfViewerApp<'a> {
    /// Returns the horizontal `(x0, x1)` range of `cat` on the unified top bar.
    pub(super) fn menu_header_x_range(&self, cat: MenuCategory, sf: f64) -> (i32, i32) {
        let s = sf as f32;
        let font_sz = (12.0 * s).clamp(10.5, 26.0);
        let pad_x = (9.0 * s).round() as i32;
        let gap = (2.0 * s).round() as i32;
        let mut cur_x = (8.0 * s).round() as i32;
        for c in MenuCategory::ALL {
            let tw = measure_text(c.label(), font_sz, false).ceil() as i32;
            let w = tw + pad_x * 2;
            if c == cat {
                return (cur_x, cur_x + w);
            }
            cur_x += w + gap;
        }
        (0, 0)
    }

    /// Returns the x coordinate immediately after the last menu header (`Help`), where the
    /// inline customizable icon toolbar begins.
    pub(super) fn toolbar_start_x(&self, sf: f64) -> i32 {
        let (_, help_x1) = self.menu_header_x_range(MenuCategory::Help, sf);
        help_x1 + (6.0 * sf as f32).round() as i32
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
        let theme = self.ui_theme();
        let s = scale_factor as f32;
        let font_sz = (12.0 * s).clamp(10.5, 26.0);
        let rad = (5.0 * s).clamp(3.0, 10.0);

        fill_rect_clipped(buffer, buf_w, buf_h, 0, 0, buf_w, menu_bar_h, theme.topbar_bg);
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            0,
            menu_bar_h.saturating_sub(1) as i32,
            buf_w,
            1,
            theme.topbar_border,
        );

        let pill_h = (22.0 * s).round().clamp(18.0, (menu_bar_h.saturating_sub(6) as f32).max(18.0)) as u32;
        let pill_y = ((menu_bar_h.saturating_sub(pill_h)) / 2) as i32;

        for cat in MenuCategory::ALL {
            let (x0, x1) = self.menu_header_x_range(cat, scale_factor);
            let w = (x1 - x0).max(0) as u32;
            let is_open = self.open_menu == Some(cat);
            let is_hover = self.hovered_widget == Some(UiHit::MenuHeader(cat));
            if is_open {
                fill_rounded_rect(buffer, buf_w, buf_h, x0, pill_y, w, pill_h, rad, theme.accent_soft_bg);
            } else if is_hover {
                fill_rounded_rect(buffer, buf_w, buf_h, x0, pill_y, w, pill_h, rad, theme.hover_bg);
            }
            let fg = if is_open {
                theme.accent_soft_fg
            } else if is_hover {
                theme.text_primary
            } else {
                theme.text_secondary
            };
            let pad_x = (9.0 * s).round() as i32;
            draw_text_vcenter(
                buffer,
                buf_w,
                buf_h,
                x0 + pad_x,
                pill_y,
                pill_h,
                x1,
                cat.label(),
                font_sz,
                fg,
                is_open,
            );
        }

        // Inline customizable icon toolbar directly beside Help
        let tb_x = self.toolbar_start_x(scale_factor);
        self.draw_inline_toolbar(buffer, buf_w, buf_h, tb_x, menu_bar_h, scale_factor);
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

    fn menu_item_checked(&self, cat: MenuCategory, idx: usize) -> bool {
        match (cat, idx) {
            (MenuCategory::View, 0) => self.effective_layout_mode() == ViewMode::FitPage && self.view_mode.is_page_view(),
            (MenuCategory::View, 1) => self.effective_layout_mode() == ViewMode::FitWidth && self.view_mode.is_page_view(),
            (MenuCategory::View, 2) => self.effective_layout_mode() == ViewMode::FitTextWidth && self.view_mode.is_page_view(),
            (MenuCategory::View, 3) => self.effective_layout_mode() == ViewMode::TwoPageSpread && self.view_mode.is_page_view(),
            (MenuCategory::View, 4) => self.effective_scroll_mode() == ScrollMode::SinglePage,
            (MenuCategory::View, 5) => self.effective_scroll_mode() == ScrollMode::ContinuousVertical,
            (MenuCategory::View, 6) => self.effective_scroll_mode() == ScrollMode::ContinuousHorizontal,
            (MenuCategory::View, 7) => self.view_mode == ViewMode::TextView,
            (MenuCategory::View, 11) => self.view_mode == ViewMode::ThumbnailGrid,
            (MenuCategory::View, 12) => self.show_bookmarks,
            (MenuCategory::View, 14) => self.is_fullscreen && !self.is_presentation,
            (MenuCategory::View, 15) => self.is_presentation,
            (MenuCategory::View, 16) => self.show_toolbar,
            (MenuCategory::View, 21) => self.ui_theme_mode == super::theme::UiThemeMode::Dark,
            (MenuCategory::Edit, 3) => self.mouse_mode == MouseMode::Select,
            (MenuCategory::Edit, 4) => self.mouse_mode == MouseMode::Highlight,
            (MenuCategory::Edit, 5) => self.mouse_mode == MouseMode::Underline,
            (MenuCategory::Edit, 6) => self.mouse_mode == MouseMode::Strikethrough,
            (MenuCategory::Edit, 7) => self.mouse_mode == MouseMode::StickyNote,
            (MenuCategory::Edit, 8) => self.mouse_mode == MouseMode::InkPen,
            (MenuCategory::Edit, 9) => self.mouse_mode == MouseMode::FreeText,
            (MenuCategory::Edit, 10) => self.mouse_mode == MouseMode::RectShape,
            (MenuCategory::Edit, 11) => self.mouse_mode == MouseMode::ArrowShape,
            (MenuCategory::Edit, 13) => self.mouse_mode == MouseMode::Redact,
            (MenuCategory::Edit, 14) => self.mouse_mode == MouseMode::Crop,
            _ => false,
        }
    }

    /// Splits a menu label like `"Open PDF in Tab (O)"` into `("Open PDF in Tab", Some("O"))`
    /// so shortcuts can be cleanly right-aligned in the dropdown.
    fn split_label_and_shortcut(item: &str) -> (&str, Option<&str>) {
        if item.starts_with("Recent")
            || item.ends_with("(default)")
            || item.ends_with("(.txt)")
            || item.ends_with("(+10%)")
            || item.ends_with("(-10%)")
        {
            return (item, None);
        }
        if let Some(open_idx) = item.rfind(" (") {
            if item.ends_with(')') && open_idx > 2 {
                let left = &item[..open_idx];
                let sc = &item[open_idx + 2..item.len() - 1];
                return (left, Some(sc));
            }
        }
        (item, None)
    }

    fn dropdown_geometry(
        &self,
        buf_w: u32,
        menu_bar_h: u32,
        cat: MenuCategory,
        sf: f64,
    ) -> (i32, i32, u32, u32, u32, Vec<String>) {
        let s = sf as f32;
        let items = self.menu_items_for_category(cat);
        let font_sz = (12.0 * s).clamp(10.5, 26.0);
        let sc_sz = (10.5 * s).clamp(9.0, 22.0);
        let row_h = (26.0 * s).round() as u32;
        let pad_v = (6.0 * s).round() as u32;

        let mut max_w = (190.0 * s).round();
        for item in &items {
            let (lbl, sc) = Self::split_label_and_shortcut(item);
            let lw = measure_text(lbl, font_sz, false);
            let sw = sc.map(|t| measure_text(t, sc_sz, false) + 22.0 * s).unwrap_or(0.0);
            max_w = max_w.max(lw + sw + 44.0 * s);
        }
        let drop_w = (max_w.ceil() as u32).min(buf_w.saturating_sub(12));
        let drop_h = (items.len() as u32) * row_h + pad_v * 2;
        let (hx0, _) = self.menu_header_x_range(cat, sf);
        let drop_x = hx0.min((buf_w as i32 - drop_w as i32 - 6).max(4));
        let drop_y = menu_bar_h as i32 + (2.0 * s).round() as i32;
        (drop_x, drop_y, drop_w, drop_h, row_h, items)
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
        let theme = self.ui_theme();
        let s = scale_factor as f32;
        let (drop_x, drop_y, drop_w, drop_h, row_h, items) =
            self.dropdown_geometry(buf_w, menu_bar_h, cat, scale_factor);
        if items.is_empty() {
            return;
        }
        let pad_v = (6.0 * s).round() as i32;
        let rad = (8.0 * s).clamp(5.0, 14.0);
        let font_sz = (12.0 * s).clamp(10.5, 26.0);
        let sc_sz = (10.5 * s).clamp(9.0, 22.0);

        draw_soft_shadow(
            buffer,
            buf_w,
            buf_h,
            drop_x,
            drop_y,
            drop_w,
            drop_h,
            rad,
            (10.0 * s).round() as i32,
            (4.0 * s).round() as i32,
            theme.shadow_rgb,
            theme.shadow_max_alpha,
        );
        fill_rounded_rect(buffer, buf_w, buf_h, drop_x, drop_y, drop_w, drop_h, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, drop_x, drop_y, drop_w, drop_h, rad, theme.border_subtle);

        let file_base_len = MenuCategory::File.items().len();
        let ic_sz = (12.0 * s).round() as u32;
        let text_indent = (26.0 * s).round() as i32;

        for (idx, item_str) in items.iter().enumerate() {
            let ry = drop_y + pad_v + (idx as i32) * (row_h as i32);
            // Draw subtle group separators between logical menu sections
            if (cat == MenuCategory::File && (idx == 3 || idx == 5 || idx == 6 || idx == file_base_len))
                || (cat == MenuCategory::View && (idx == 4 || idx == 7 || idx == 8 || idx == 11 || idx == 14 || idx == 16 || idx == 18))
                || (cat == MenuCategory::Edit && (idx == 3 || idx == 13))
                || (cat == MenuCategory::Page && (idx == 2 || idx == 5))
                || (cat == MenuCategory::Export && idx == 2)
            {
                fill_rect_clipped(
                    buffer,
                    buf_w,
                    buf_h,
                    drop_x + (8.0 * s).round() as i32,
                    ry,
                    drop_w.saturating_sub((16.0 * s).round() as u32),
                    1,
                    theme.divider,
                );
            }

            let is_disabled = item_str == "Recent: (none)";
            let hovered = !is_disabled && self.hovered_widget == Some(UiHit::MenuItem(idx));
            if hovered {
                fill_rounded_rect(
                    buffer,
                    buf_w,
                    buf_h,
                    drop_x + (5.0 * s).round() as i32,
                    ry + 1,
                    drop_w.saturating_sub((10.0 * s).round() as u32),
                    row_h.saturating_sub(2),
                    (5.0 * s).max(3.0),
                    theme.accent,
                );
            }

            let fg = if is_disabled {
                theme.text_muted
            } else if hovered {
                theme.text_on_accent
            } else {
                theme.text_primary
            };
            let sc_fg = if hovered { theme.text_on_accent } else { theme.text_muted };

            if self.menu_item_checked(cat, idx) {
                draw_icon(
                    buffer,
                    buf_w,
                    buf_h,
                    UiIcon::Check,
                    drop_x + (9.0 * s).round() as i32,
                    ry + ((row_h.saturating_sub(ic_sz)) / 2) as i32,
                    ic_sz,
                    if hovered { theme.text_on_accent } else { theme.accent },
                );
            }

            let (lbl, sc) = Self::split_label_and_shortcut(item_str);
            let sc_w = sc.map(|t| measure_text(t, sc_sz, false).ceil() as i32).unwrap_or(0);
            let right_pad = (12.0 * s).round() as i32;
            let lbl_max_x = drop_x + drop_w as i32 - right_pad - if sc_w > 0 { sc_w + 10 } else { 0 };
            let clipped_lbl = truncate_ellipsis(lbl, (lbl_max_x - (drop_x + text_indent)).max(16) as f32, font_sz, false);
            draw_text_vcenter(buffer, buf_w, buf_h, drop_x + text_indent, ry, row_h, lbl_max_x, &clipped_lbl, font_sz, fg, false);

            if let Some(sc_txt) = sc {
                draw_text_vcenter(
                    buffer,
                    buf_w,
                    buf_h,
                    drop_x + drop_w as i32 - right_pad - sc_w,
                    ry,
                    row_h,
                    drop_x + drop_w as i32 - 4,
                    sc_txt,
                    sc_sz,
                    sc_fg,
                    false,
                );
            }
        }
    }

    pub(super) fn hit_test_menu_bar_header(
        &self,
        mx: f64,
        my: f64,
        menu_bar_h: u32,
        scale_factor: f64,
    ) -> Option<MenuCategory> {
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if iy < 0 || iy >= menu_bar_h as i32 {
            return None;
        }
        for cat in MenuCategory::ALL {
            let (x0, x1) = self.menu_header_x_range(cat, scale_factor);
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
        let s = scale_factor as f32;
        let (drop_x, drop_y, drop_w, drop_h, row_h, items) =
            self.dropdown_geometry(buf_w, menu_bar_h, cat, scale_factor);
        let pad_v = (6.0 * s).round() as i32;
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix < drop_x || ix >= drop_x + drop_w as i32 || iy < drop_y + pad_v || iy >= drop_y + drop_h as i32 - pad_v {
            return None;
        }
        let idx = ((iy - (drop_y + pad_v)) / row_h as i32) as usize;
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
                self.open_create_pdf_from_images_modal();
                return;
            }
            (MenuCategory::File, 2) => {
                self.open_combine_modal();
                return;
            }
            (MenuCategory::File, 3) => {
                self.pending_dialog = Some(PendingDialog::ImportBookmarksTxt);
            }
            (MenuCategory::File, 4) => {
                self.open_document_properties_modal();
                return;
            }
            (MenuCategory::File, 5) => {
                self.pending_dialog = Some(PendingDialog::Save);
            }
            (MenuCategory::File, 6) => {
                if self.doc.is_some() {
                    self.pending_dialog = Some(PendingDialog::SaveAs);
                }
            }
            (MenuCategory::File, 7) => {
                self.trigger_print();
            }
            (MenuCategory::File, 8) => {
                if !self.tabs.is_empty() {
                    let _ = self.request_close_action(
                        super::doc_props_modal::PendingCloseAction::CloseTab(self.active_tab),
                    );
                    return;
                }
            }
            (MenuCategory::File, 9) => {
                let _ = self.request_close_action(
                    super::doc_props_modal::PendingCloseAction::CloseOtherTabs(self.active_tab),
                );
                return;
            }
            (MenuCategory::File, 10) => {
                let _ = self.request_close_action(
                    super::doc_props_modal::PendingCloseAction::CloseAllTabs,
                );
                return;
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
            (MenuCategory::View, 4) => {
                self.set_scroll_mode(ScrollMode::SinglePage);
                return;
            }
            (MenuCategory::View, 5) => {
                self.set_scroll_mode(ScrollMode::ContinuousVertical);
                return;
            }
            (MenuCategory::View, 6) => {
                self.set_scroll_mode(ScrollMode::ContinuousHorizontal);
                return;
            }
            (MenuCategory::View, 7) => {
                if self.view_mode == ViewMode::TextView {
                    self.set_view_mode(self.last_page_view_mode);
                } else {
                    self.set_view_mode(ViewMode::TextView);
                }
            }
            (MenuCategory::View, 8) => self.zoom_by_step(1.25),
            (MenuCategory::View, 9) => self.zoom_by_step(0.8),
            (MenuCategory::View, 10) => self.reset_zoom(),
            (MenuCategory::View, 11) => self.toggle_view_mode(),
            (MenuCategory::View, 12) => {
                self.show_bookmarks = !self.show_bookmarks;
                self.invalidate_page_caches();
                self.status_notice = Some(if self.show_bookmarks {
                    "Bookmarks Sidebar ON".to_string()
                } else {
                    "Bookmarks Sidebar OFF".to_string()
                });
            }
            (MenuCategory::View, 13) => {
                self.set_reading_filter(self.reading_filter.next());
                return;
            }
            (MenuCategory::View, 14) => self.toggle_fullscreen(),
            (MenuCategory::View, 15) => {
                if self.is_presentation {
                    self.exit_presentation_or_fullscreen();
                } else {
                    self.enter_presentation_mode();
                }
            }
            (MenuCategory::View, 16) => {
                self.show_toolbar = !self.show_toolbar;
                self.save_session_to_ini();
                self.status_notice = Some(if self.show_toolbar {
                    "Toolbar shown beside menus".to_string()
                } else {
                    "Toolbar hidden (re-enable via View -> Show Toolbar)".to_string()
                });
            }
            (MenuCategory::View, 17) => {
                self.show_customize_toolbar = true;
            }
            (MenuCategory::View, 18) => {
                self.adjust_ui_scale(0.10);
                return;
            }
            (MenuCategory::View, 19) => {
                self.adjust_ui_scale(-0.10);
                return;
            }
            (MenuCategory::View, 20) => {
                self.reset_ui_scale();
                return;
            }
            (MenuCategory::View, 21) => {
                self.ui_theme_mode = self.ui_theme_mode.toggle();
                self.save_session_to_ini();
                self.status_notice = Some(format!("UI Theme: {}", self.ui_theme_mode.label()));
            }

            // Edit menu
            (MenuCategory::Edit, 0) => self.perform_undo(),
            (MenuCategory::Edit, 1) => self.perform_redo(),
            (MenuCategory::Edit, 2) => self.copy_selected_text_to_clipboard(),
            (MenuCategory::Edit, 3) => {
                self.input_mode = InputMode::SearchText {
                    query_input: self.search_query.clone(),
                };
            }
            (MenuCategory::Edit, 4) => {
                self.mouse_mode = MouseMode::Select;
                self.status_notice = Some(
                    "Mouse Mode: Select Text (Drag or double-click text, Ctrl/Cmd+C to copy)"
                        .to_string(),
                );
            }
            (MenuCategory::Edit, 5) => {
                self.mouse_mode = MouseMode::Highlight;
                self.status_notice = Some(format!(
                    "Annotate: Highlight ({}) — drag across text",
                    self.annotation_color.label()
                ));
            }
            (MenuCategory::Edit, 6) => {
                self.mouse_mode = MouseMode::Underline;
                self.status_notice = Some(format!(
                    "Annotate: Underline ({}) — drag across text",
                    self.annotation_color.label()
                ));
            }
            (MenuCategory::Edit, 7) => {
                self.mouse_mode = MouseMode::Strikethrough;
                self.status_notice = Some(format!(
                    "Annotate: Strikethrough ({}) — drag across text",
                    self.annotation_color.label()
                ));
            }
            (MenuCategory::Edit, 8) => {
                self.mouse_mode = MouseMode::StickyNote;
                self.status_notice = Some(format!(
                    "Annotate: Sticky Note ({}) — click page to place note",
                    self.annotation_color.label()
                ));
            }
            (MenuCategory::Edit, 9) => {
                self.mouse_mode = MouseMode::InkPen;
                self.status_notice = Some(format!(
                    "Annotate: Freehand Pen ({}) — drag to draw",
                    self.annotation_color.label()
                ));
            }
            (MenuCategory::Edit, 10) => {
                self.mouse_mode = MouseMode::FreeText;
                self.status_notice = Some(format!(
                    "Annotate: Text Box ({}) — click page to add text",
                    self.annotation_color.label()
                ));
            }
            (MenuCategory::Edit, 11) => {
                self.mouse_mode = MouseMode::RectShape;
                self.status_notice = Some(format!(
                    "Annotate: Rectangle ({}) — drag box",
                    self.annotation_color.label()
                ));
            }
            (MenuCategory::Edit, 12) => {
                self.mouse_mode = MouseMode::ArrowShape;
                self.status_notice = Some(format!(
                    "Annotate: Arrow ({}) — drag start to tip",
                    self.annotation_color.label()
                ));
            }
            (MenuCategory::Edit, 13) => {
                self.annotation_color = self.annotation_color.next();
                self.status_notice = Some(format!(
                    "Annotation color: {}",
                    self.annotation_color.label()
                ));
            }
            (MenuCategory::Edit, 14) => {
                self.mouse_mode = MouseMode::Measure;
                self.last_measurement = None;
                self.status_notice = Some(
                    "Measure Tool: Drag between any two points on a page to measure length, area & angle (Esc exits)"
                        .to_string(),
                );
            }
            (MenuCategory::Edit, 15) => {
                self.mouse_mode = MouseMode::Redact;
                self.status_notice = Some(
                    "Mouse Mode: Black-Box Redact (Drag red box to permanently purge & black out)"
                        .to_string(),
                );
            }
            (MenuCategory::Edit, 16) => {
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
            (MenuCategory::Page, 2) => {
                self.open_deskew_bar();
                return;
            }
            (MenuCategory::Page, 3) => self.insert_blank_page_after_current(),
            (MenuCategory::Page, 4) => self.insert_blank_page_before_current(),
            (MenuCategory::Page, 5) => {
                let target = if self.view_mode == ViewMode::ThumbnailGrid {
                    self.hovered_thumb.unwrap_or(self.current_page_index)
                } else {
                    self.current_page_index
                };
                self.open_insert_pdf_modal(false, target);
                return;
            }
            (MenuCategory::Page, 6) => self.delete_current_page(),
            (MenuCategory::Page, 7) => {
                let target = if self.view_mode == ViewMode::ThumbnailGrid {
                    self.hovered_thumb.unwrap_or(self.current_page_index)
                } else {
                    self.current_page_index
                };
                self.convert_page_to_bw_in_place(target);
            }
            (MenuCategory::Page, 8) => self.reset_current_page_crop(),

            // Tools menu
            (MenuCategory::Tools, 0) => {
                self.open_page_number_modal();
                return;
            }
            (MenuCategory::Tools, 1) => {
                self.open_header_footer_modal();
                return;
            }
            (MenuCategory::Tools, 2) => {
                self.input_mode = InputMode::Watermark {
                    input: String::new(),
                };
            }
            (MenuCategory::Tools, 3) => {
                self.pending_dialog = Some(PendingDialog::PickStampImage);
            }
            (MenuCategory::Tools, 4) => {
                self.input_mode = InputMode::SplitRange {
                    input: String::new(),
                };
            }
            (MenuCategory::Tools, 5) => {
                self.open_split_pages_modal();
                return;
            }
            (MenuCategory::Tools, 6) => {
                if self.doc.is_some() {
                    self.pending_dialog = Some(PendingDialog::PickComparePdf);
                } else {
                    self.status_notice = Some("Open a PDF first before starting Compare Mode".to_string());
                }
            }
            (MenuCategory::Tools, 7) => self.flatten_and_unlock_current_pdf(),
            (MenuCategory::Tools, 8) => self.open_optimize_prompt(),

            // Export menu
            (MenuCategory::Export, 0) => {
                if self.doc.is_some() {
                    self.pending_dialog = Some(PendingDialog::ExportDocx);
                } else {
                    self.status_notice = Some("Open a PDF first to export to Word (.docx)".to_string());
                }
            }
            (MenuCategory::Export, 1) => {
                self.open_excel_export_modal();
                return;
            }
            (MenuCategory::Export, 2) => {
                if self.doc.is_some() {
                    self.pending_dialog = Some(PendingDialog::ExportMarkdown);
                } else {
                    self.status_notice = Some("Open a PDF first to export to Markdown (.md)".to_string());
                }
            }
            (MenuCategory::Export, 3) => {
                if self.doc.is_some() {
                    self.pending_dialog = Some(PendingDialog::ExportLatex);
                } else {
                    self.status_notice = Some("Open a PDF first to export to LaTeX (.tex)".to_string());
                }
            }
            (MenuCategory::Export, 4) => {
                self.open_export_images_modal();
                return;
            }

            // Help menu
            (MenuCategory::Help, 0) => {
                self.open_user_guide_modal();
                return;
            }
            (MenuCategory::Help, 1) => {
                self.show_help_overlay = !self.show_help_overlay;
                self.show_bookmark_format_overlay = false;
                self.show_about_overlay = false;
                self.user_guide_modal = None;
            }
            (MenuCategory::Help, 2) => {
                self.show_bookmark_format_overlay = !self.show_bookmark_format_overlay;
                self.show_help_overlay = false;
                self.show_about_overlay = false;
                self.user_guide_modal = None;
            }
            (MenuCategory::Help, 3) => {
                self.show_about_overlay = !self.show_about_overlay;
                self.show_help_overlay = false;
                self.show_bookmark_format_overlay = false;
                self.user_guide_modal = None;
            }
            _ => {}
        }
        self.update_title_bar();
        self.request_redraw();
    }
}

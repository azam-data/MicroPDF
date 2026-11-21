//! Clean Light/Dark bottom status bar with hovered toolbar tooltips, page/zoom info,
//! and interactive command prompt formatting (preserved for keyboard & test parity).

use super::get_current_rss_mb;
use super::render::fill_rect_clipped;
use super::state::*;
use super::ui_draw::*;

impl<'a> PdfViewerApp<'a> {
    /// Formats the active interactive prompt string (if any) for accessibility & tests.
    #[allow(dead_code)]
    pub(super) fn format_active_prompt_text(&self) -> Option<String> {
        match &self.input_mode {
            InputMode::Normal => None,
            InputMode::SearchText { query_input } => Some(format!(
                "FIND TEXT IN PDF: {}_  (Enter to search, Esc to cancel, F3 / Cmd+G next)",
                query_input
            )),
            InputMode::JumpToPage { input } => Some(format!(
                "JUMP TO PAGE (Logical '5', Roman 'ii', or Physical '#5'): {}_  (Enter confirm, Esc cancel)",
                input
            )),
            InputMode::PageNumbers { input } => Some(format!(
                "PAGE NUMBERS: Last Roman page # (e.g. '5' for i..v then 1..N, or '0' all Arabic): {}_",
                input
            )),
            InputMode::HeaderFooterRange { range_input } => Some(format!(
                "HEADER/FOOTER (1/3): Enter page range (e.g. '1-5' or 'all'): {}_",
                range_input
            )),
            InputMode::HeaderFooterPosition { pos_input, .. } => Some(format!(
                "HEADER/FOOTER (2/3): Position ('HL', 'HC', 'HR', 'FL', 'FC', 'FR'): {}_",
                pos_input
            )),
            InputMode::HeaderFooterText { text_input, .. } => Some(format!(
                "HEADER/FOOTER (3/3): Enter text (or empty + Enter to erase existing): {}_",
                text_input
            )),
            InputMode::ClearMargins { input } => Some(format!(
                "CLEAR MARGINS: '1' Footers/Page#, '2' Headers, '3' Both (optional range '1 1-5'): {}_",
                input
            )),
            InputMode::Watermark { input } => Some(format!(
                "WATERMARK: Type watermark text and press Enter: {}_",
                input
            )),
            InputMode::SplitRange { input } => Some(format!(
                "SPLIT PDF: Enter page range to extract (e.g. '1-3' or '2,4,6'): {}_",
                input
            )),
            InputMode::AddBookmark { title_input } => Some(format!(
                "ADD BOOKMARK (Page {}): Type title and press Enter: {}_",
                self.current_page_index + 1,
                title_input
            )),
            InputMode::EditBookmark {
                bookmark_idx,
                title_input,
            } => {
                let page_num = self
                    .bookmarks
                    .get(*bookmark_idx)
                    .map(|b| b.page_index + 1)
                    .unwrap_or(1);
                Some(format!(
                    "EDIT BOOKMARK (Page {}): Type new title and press Enter: {}_",
                    page_num, title_input
                ))
            }
            InputMode::ConfirmExternalUrl { url } => Some(format!(
                "OPEN EXTERNAL URL IN BROWSER? {}  (Press Enter to confirm, Esc to cancel)",
                url
            )),
            InputMode::PasswordPrompt {
                path,
                password_input,
                attempts_left,
                ..
            } => {
                let fname = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("document.pdf");
                let masked: String = std::iter::repeat('*')
                    .take(password_input.chars().count())
                    .collect();
                Some(format!(
                    "PASSWORD REQUIRED for '{fname}' ({attempts_left} attempt(s) left): {masked}_  (Enter unlock, Esc cancel)"
                ))
            }
            InputMode::RestoreSessionPrompt { tab_count } => Some(format!(
                "RESTORE PREVIOUS SESSION ({tab_count} saved tab(s))? Press Enter or Y to restore, Esc or N to dismiss"
            )),
            InputMode::OptimizePrompt {
                preset,
                mode,
                clean_scans,
                previewing,
            } => {
                let preset_lbl = match preset {
                    crate::optimize::OptimizePreset::Smaller => "Smaller(150dpi)",
                    crate::optimize::OptimizePreset::Balanced => "Balanced(200dpi)",
                    crate::optimize::OptimizePreset::Print => "Print(300dpi)",
                };
                let mode_lbl = match mode {
                    crate::optimize::ColorMode::Color => "Color",
                    crate::optimize::ColorMode::Gray => "Gray",
                    crate::optimize::ColorMode::BlackWhite => {
                        "B/W (signatures will look jagged)"
                    }
                };
                let clean_lbl = if *clean_scans { "ON" } else { "OFF" };
                let prev_lbl = if *previewing {
                    "[P]Preview:ON"
                } else {
                    "[P]Preview"
                };
                Some(format!(
                    "OPTIMIZE: [1/2/3]{preset_lbl} [C/G/B]{mode_lbl} [S]Clean:{clean_lbl} {prev_lbl} | Enter=Run Esc=Cancel"
                ))
            }
            InputMode::PrintPrompt {
                printers,
                selected_printer,
                copies,
                range_input,
                fit_to_page,
            } => {
                let fit_lbl = if *fit_to_page { "ON" } else { "OFF" };
                let printer_lbl = printers
                    .get(*selected_printer)
                    .map(|s| s.as_str())
                    .unwrap_or("(no printer)");
                let range_lbl = if range_input.is_empty() {
                    "all"
                } else {
                    range_input.as_str()
                };
                Some(format!(
                    "PRINT: Printer [Tab]: {printer_lbl} ({}/{}) | Copies [+/-]: {copies} | [F]Fit:{fit_lbl} | Pages (e.g. 1-3,5): {range_lbl}_ | Enter=Print Esc=Cancel",
                    if printers.is_empty() { 0 } else { selected_printer + 1 },
                    printers.len()
                ))
            }
        }
    }

    pub(super) fn draw_status_bar(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        status_bar_h: u32,
        scale_factor: f64,
    ) {
        if status_bar_h == 0 || buf_h < status_bar_h {
            return;
        }
        let theme = self.ui_theme();
        let s = scale_factor as f32;
        let bar_y = (buf_h - status_bar_h) as i32;
        let font_sz = (11.0 * s).clamp(9.5, 24.0);
        let pad_x = (12.0 * s).round() as i32;

        fill_rect_clipped(buffer, buf_w, buf_h, 0, bar_y, buf_w, status_bar_h, theme.statusbar_bg);
        fill_rect_clipped(buffer, buf_w, buf_h, 0, bar_y, buf_w, 1, theme.divider);

        let total = self.total_pages();
        let cur = if total > 0 { self.current_page_index + 1 } else { 0 };
        let zoom_pct = (self.zoom_level * 100.0).round() as u32;
        let ram_mb = get_current_rss_mb().unwrap_or(0.0);

        let right_text = format!(
            "{}  ·  {}  ·  {ram_mb:.1} MB",
            self.mouse_mode.short_label(),
            self.reading_filter.label()
        );
        let right_w = measure_text(&right_text, font_sz, false).ceil() as i32;
        let right_x = (buf_w as i32 - pad_x - right_w).max(pad_x + 40);

        // Show toolbar hover tooltip when hovering a toolbar button, otherwise page + notice
        let left_text = if let Some(UiHit::Toolbar(action)) = self.hovered_widget {
            action.tooltip().to_string()
        } else if let Some(ref notice) = self.status_notice {
            if total > 0 {
                format!("Page {cur} of {total}  ·  {} ({zoom_pct}%)  ·  {notice}", self.view_mode.short_label())
            } else {
                notice.clone()
            }
        } else if total > 0 {
            format!("Page {cur} of {total}  ·  {} ({zoom_pct}%)", self.view_mode.short_label())
        } else {
            "Ready — Drop a PDF or press O to open".to_string()
        };

        let left_budget = (right_x - pad_x - (16.0 * s).round() as i32).max(20) as f32;
        let left_clipped = truncate_ellipsis(&left_text, left_budget, font_sz, false);
        draw_text_vcenter(buffer, buf_w, buf_h, pad_x, bar_y, status_bar_h, right_x - 8, &left_clipped, font_sz, theme.text_secondary, false);

        let tool_color = match self.mouse_mode {
            MouseMode::Select => theme.text_muted,
            MouseMode::Highlight => theme.warning,
            MouseMode::Redact => theme.danger,
            MouseMode::Crop => theme.success,
            MouseMode::StampImage => theme.violet,
        };
        draw_text_vcenter(buffer, buf_w, buf_h, right_x, bar_y, status_bar_h, buf_w as i32 - 4, &right_text, font_sz, tool_color, false);
    }
}

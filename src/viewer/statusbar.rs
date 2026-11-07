use super::state::*;
use super::render::{draw_bitmap_str_8x8, fill_rect_clipped};
use super::get_current_rss_mb;

impl<'a> PdfViewerApp<'a> {
    /// Formats the active interactive prompt string (if any) for both the Bottom Command Bar and Title Bar.
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
        let scale = (scale_factor.round() as u32).clamp(1, 2);
        let bar_y = buf_h - status_bar_h;

        if let Some(prompt) = self.format_active_prompt_text() {
            // Active interactive command prompt: bright #007ACC command bar at bottom of window
            fill_rect_clipped(
                buffer,
                buf_w,
                buf_h,
                0,
                bar_y as i32,
                buf_w,
                status_bar_h,
                0x000E639C,
            );
            fill_rect_clipped(buffer, buf_w, buf_h, 0, bar_y as i32, buf_w, 1, 0x004FC3F7);
            let text_y = bar_y + status_bar_h.saturating_sub(8 * scale) / 2;
            let max_chars = (buf_w.saturating_sub(16 * scale) / (8 * scale)).max(8) as usize;
            let display_str: String = if prompt.chars().count() > max_chars {
                let skip = prompt.chars().count() - max_chars;
                prompt.chars().skip(skip).collect()
            } else {
                prompt
            };
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                8 * scale,
                text_y,
                &display_str,
                0x00FFEB3B,
                scale,
            );
            return;
        }

        // Standard Status Bar (#1E1E1E background, 1px top border)
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            0,
            bar_y as i32,
            buf_w,
            status_bar_h,
            0x001E1E1E,
        );
        fill_rect_clipped(buffer, buf_w, buf_h, 0, bar_y as i32, buf_w, 1, 0x00333333);

        let total = self.total_pages();
        let cur = if total > 0 {
            self.current_page_index + 1
        } else {
            0
        };
        let ram_mb = get_current_rss_mb().unwrap_or(0.0);
        let notice = self
            .status_notice
            .as_deref()
            .unwrap_or("Ready — Click Top Menu or press 1-4 View, Ctrl+F Search, P Stamp, V Filter, U Flatten");

        let zoom_pct = (self.zoom_level * 100.0).round() as u32;
        let status_line = format!(
            "Page {cur}/{total} | Mode: {} ({zoom_pct}%) | Mouse: {} | RAM: {ram_mb:.1}MB | {notice}",
            self.view_mode.short_label(),
            self.mouse_mode.short_label()
        );
        let max_chars = (buf_w.saturating_sub(16 * scale) / (8 * scale)).max(8) as usize;
        let clipped: String = status_line.chars().take(max_chars).collect();
        let text_y = bar_y + status_bar_h.saturating_sub(8 * scale) / 2;
        draw_bitmap_str_8x8(
            buffer,
            buf_w,
            buf_h,
            8 * scale,
            text_y,
            &clipped,
            0x00CCCCCC,
            scale,
        );
    }
}

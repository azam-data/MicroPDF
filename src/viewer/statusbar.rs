//! Interactive command prompt formatting (for accessibility & tests) and bottom bar entry point.

use super::state::*;

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
            InputMode::StickyNotePrompt { text_input, .. } => Some(format!(
                "STICKY NOTE: Type comment and press Enter: {}_",
                text_input
            )),
            InputMode::FreeTextPrompt { text_input, .. } => Some(format!(
                "TEXT BOX: Type text to place on page and press Enter: {}_",
                text_input
            )),
            InputMode::EditAnnotationText { label, text_input, .. } => Some(format!(
                "EDIT {}: Type updated text and press Enter: {}_",
                label.to_ascii_uppercase(),
                text_input
            )),
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
        self.draw_bottom_view_bar(buffer, buf_w, buf_h, status_bar_h, scale_factor);
    }
}

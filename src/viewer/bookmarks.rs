//! Themed Left Bookmark Sidebar (`B`) with crisp proportional typography, Add/Import/Export/Auto
//! action controls, Reorder (Up/Down) & Indent/Outdent controls, active-page accent bar, and hover delete.

use crate::engine::{Bookmark, PdfEngine};
use std::path::Path;
use super::render::fill_rect_clipped;
use super::state::*;
use super::ui_draw::*;

impl<'a> PdfViewerApp<'a> {
    fn bookmark_sidebar_header_height(s: f32) -> u32 {
        (96.0 * s).round() as u32
    }

    pub(super) fn draw_bookmark_sidebar(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        scale_factor: f64,
    ) {
        let rail_w = self.format_rail_width_px(scale_factor);
        let panel_x = rail_w as i32;
        let panel_w = sidebar_w.saturating_sub(rail_w);
        if panel_w < 48 {
            return;
        }
        let panel_r = panel_x + panel_w as i32;
        let theme = self.ui_theme();
        let s = scale_factor as f32;
        let row_h = (28.0 * s).round() as u32;
        let header_h = Self::bookmark_sidebar_header_height(s);
        let top_y = top_bars_h as i32;
        let pad = (8.0 * s).round() as i32;
        let gap = (4.0 * s).round() as i32;
        let rad = (4.5 * s).max(3.0);

        // Sidebar surface + right divider
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            panel_x,
            top_y,
            panel_w,
            buf_h.saturating_sub(top_bars_h),
            theme.sidebar_bg,
        );
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            panel_r - 1,
            top_y,
            1,
            buf_h.saturating_sub(top_bars_h),
            theme.divider,
        );

        // Header strip
        fill_rect_clipped(buffer, buf_w, buf_h, panel_x, top_y, panel_w.saturating_sub(1), header_h, theme.sidebar_header_bg);
        fill_rect_clipped(buffer, buf_w, buf_h, panel_x, top_y + header_h as i32 - 1, panel_w.saturating_sub(1), 1, theme.divider);

        let title_sz = (12.0 * s).clamp(10.5, 26.0);
        let body_sz = (11.5 * s).clamp(10.0, 24.0);
        let sub_sz = (10.0 * s).clamp(8.5, 20.0);
        draw_text(buffer, buf_w, buf_h, panel_x + pad, top_y + (9.0 * s).round() as i32, "Bookmarks", title_sz, theme.text_primary, true);
        let count_lbl = format!("({})", self.bookmarks.len());
        let cw = measure_text(&count_lbl, sub_sz, false).ceil() as i32;
        draw_text(buffer, buf_w, buf_h, panel_r - pad - cw - 2, top_y + (10.0 * s).round() as i32, &count_lbl, sub_sz, theme.text_muted, false);

        // Row 1 buttons: `+ Add` | `Import` | `Export` | `Auto`
        let inner_w = panel_w.saturating_sub((pad * 2 + 1) as u32);
        let btn_h = (22.0 * s).round() as u32;
        let row1_y = top_y + (32.0 * s).round() as i32;
        let b1_w = (inner_w.saturating_sub((gap * 3) as u32)) / 4;
        let row1_btns = [
            ("+ Add", UiHit::SidebarAdd, true),
            ("Import", UiHit::SidebarImport, false),
            ("Export", UiHit::SidebarExport, false),
            ("Auto", UiHit::SidebarAuto, false),
        ];
        for (i, &(lbl, hit, primary)) in row1_btns.iter().enumerate() {
            let bx = panel_x + pad + (i as i32) * (b1_w as i32 + gap);
            let bw = if i == 3 {
                (panel_r - pad - 1 - bx).max(20) as u32
            } else {
                b1_w
            };
            let hov = self.hovered_widget == Some(hit);
            let (bg, fg) = if primary {
                (if hov { theme.accent_hover } else { theme.accent }, theme.text_on_accent)
            } else {
                (if hov { theme.hover_bg } else { theme.surface_elevated }, theme.text_secondary)
            };
            fill_rounded_rect(buffer, buf_w, buf_h, bx, row1_y, bw, btn_h, rad, bg);
            if !primary {
                stroke_rounded_rect(buffer, buf_w, buf_h, bx, row1_y, bw, btn_h, rad, theme.border_subtle);
            }
            let tw = measure_text(lbl, sub_sz, primary);
            draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round().max(2.0) as i32, row1_y, btn_h, bx + bw as i32 - 2, lbl, sub_sz, fg, primary);
        }

        // Row 2 buttons: `↑ Up` | `↓ Down` | `← Out` | `→ In`
        let row2_y = row1_y + btn_h as i32 + (6.0 * s).round() as i32;
        let has_sel = self.selected_bookmark.is_some_and(|i| i < self.bookmarks.len());
        let row2_btns = [
            ("↑ Up", UiHit::SidebarMoveUp),
            ("↓ Down", UiHit::SidebarMoveDown),
            ("← Out", UiHit::SidebarOutdent),
            ("→ In", UiHit::SidebarIndent),
        ];
        for (i, &(lbl, hit)) in row2_btns.iter().enumerate() {
            let bx = panel_x + pad + (i as i32) * (b1_w as i32 + gap);
            let bw = if i == 3 {
                (panel_r - pad - 1 - bx).max(20) as u32
            } else {
                b1_w
            };
            let hov = has_sel && self.hovered_widget == Some(hit);
            let bg = if hov { theme.hover_bg } else { theme.surface_elevated };
            let fg = if has_sel { theme.text_secondary } else { theme.text_muted };
            fill_rounded_rect(buffer, buf_w, buf_h, bx, row2_y, bw, btn_h, rad, bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, bx, row2_y, bw, btn_h, rad, theme.border_subtle);
            let tw = measure_text(lbl, sub_sz, false);
            draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round().max(2.0) as i32, row2_y, btn_h, bx + bw as i32 - 2, lbl, sub_sz, fg, false);
        }

        if self.bookmarks.is_empty() {
            let empty_y = top_y + header_h as i32 + (18.0 * s).round() as i32;
            draw_text(buffer, buf_w, buf_h, panel_x + pad + 2, empty_y, "No bookmarks yet.", body_sz, theme.text_muted, false);
            draw_text(buffer, buf_w, buf_h, panel_x + pad + 2, empty_y + (20.0 * s).round() as i32, "Click + Add (A), Import,", sub_sz, theme.text_muted, false);
            draw_text(buffer, buf_w, buf_h, panel_x + pad + 2, empty_y + (36.0 * s).round() as i32, "or Auto to detect headings.", sub_sz, theme.text_muted, false);
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
                fill_rounded_rect(buffer, buf_w, buf_h, panel_x + (4.0 * s).round() as i32, ry + 1, panel_w.saturating_sub((9.0 * s).round() as u32), row_h.saturating_sub(2), (5.0 * s).max(3.0), theme.active_row_bg);
            } else if is_hovered {
                fill_rounded_rect(buffer, buf_w, buf_h, panel_x + (4.0 * s).round() as i32, ry + 1, panel_w.saturating_sub((9.0 * s).round() as u32), row_h.saturating_sub(2), (5.0 * s).max(3.0), theme.hover_bg);
            }

            if is_current_page {
                fill_rounded_rect(buffer, buf_w, buf_h, panel_x + (4.0 * s).round() as i32, ry + (5.0 * s).round() as i32, (3.0 * s).round().max(2.0) as u32, row_h.saturating_sub((10.0 * s).round() as u32), 1.5, theme.accent);
            }

            let indent_px = if bm.level > 0 {
                fill_rect_clipped(buffer, buf_w, buf_h, panel_x + (14.0 * s).round() as i32, ry, 1, row_h, theme.divider);
                panel_x + (22.0 * s).round() as i32
            } else {
                panel_x + (12.0 * s).round() as i32
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
            let del_x = panel_r - del_w as i32 - (6.0 * s).round() as i32;
            let right_edge = if is_hovered { del_x - 4 } else { panel_r - (10.0 * s).round() as i32 };
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
        let rail_w = self.format_rail_width_px(scale_factor);
        let panel_w = sidebar_w.saturating_sub(rail_w);
        let ix = (mx - rail_w as f64).round() as i32;
        let iy = my.round() as i32;
        if !self.show_bookmarks || panel_w < 48 || ix < 0 || ix >= panel_w as i32 || iy < top_bars_h as i32 {
            return None;
        }
        let s = scale_factor as f32;
        let row_h = (28.0 * s).round() as i32;
        let header_h = Self::bookmark_sidebar_header_height(s) as i32;
        let rel_y = iy - top_bars_h as i32;
        if rel_y < header_h {
            let pad = (8.0 * s).round() as i32;
            let gap = (4.0 * s).round() as i32;
            let inner_w = panel_w.saturating_sub((pad * 2 + 1) as u32);
            let btn_h = (22.0 * s).round() as i32;
            let b1_w = ((inner_w.saturating_sub((gap * 3) as u32)) / 4) as i32;
            let row1_y = (32.0 * s).round() as i32;
            let row2_y = row1_y + btn_h + (6.0 * s).round() as i32;

            let hit_col = |x: i32| -> Option<usize> {
                if x < pad || x >= panel_w as i32 - pad {
                    return None;
                }
                for i in 0..4 {
                    let bx = pad + (i as i32) * (b1_w + gap);
                    let bw = if i == 3 {
                        (panel_w as i32 - pad - 1 - bx).max(20)
                    } else {
                        b1_w
                    };
                    if x >= bx && x < bx + bw {
                        return Some(i);
                    }
                }
                None
            };

            if rel_y >= row1_y && rel_y < row1_y + btn_h {
                return match hit_col(ix) {
                    Some(0) => Some(SidebarHit::AddBookmarkButton),
                    Some(1) => Some(SidebarHit::ImportTxtButton),
                    Some(2) => Some(SidebarHit::ExportTxtButton),
                    Some(3) => Some(SidebarHit::AutoDetectButton),
                    _ => None,
                };
            }
            if rel_y >= row2_y && rel_y < row2_y + btn_h {
                return match hit_col(ix) {
                    Some(0) => Some(SidebarHit::MoveUpButton),
                    Some(1) => Some(SidebarHit::MoveDownButton),
                    Some(2) => Some(SidebarHit::OutdentButton),
                    Some(3) => Some(SidebarHit::IndentButton),
                    _ => None,
                };
            }
            return None;
        }
        let vis_row = ((rel_y - header_h) / row_h) as usize;
        let bm_idx = self.bookmark_scroll_y + vis_row;
        if bm_idx < self.bookmarks.len() {
            let del_x = panel_w as i32 - (28.0 * s).round() as i32;
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

    pub(super) fn move_selected_bookmark(&mut self, delta: i32) {
        let Some(idx) = self.selected_bookmark else {
            return;
        };
        if idx >= self.bookmarks.len() {
            return;
        }
        let target = if delta < 0 {
            if idx == 0 {
                return;
            }
            idx - 1
        } else {
            if idx + 1 >= self.bookmarks.len() {
                return;
            }
            idx + 1
        };
        self.push_metadata_undo_snapshot();
        self.bookmarks.swap(idx, target);
        self.selected_bookmark = Some(target);
        self.status_notice = Some(format!(
            "Moved bookmark '{}' {} (Ctrl+Z to undo)",
            self.bookmarks[target].title,
            if delta < 0 { "up" } else { "down" }
        ));
        self.update_title_bar();
        self.request_redraw();
    }

    pub(super) fn adjust_selected_bookmark_level(&mut self, delta: i8) {
        let Some(idx) = self.selected_bookmark else {
            return;
        };
        if idx >= self.bookmarks.len() {
            return;
        }
        let old_lvl = self.bookmarks[idx].level;
        let new_lvl = if delta > 0 {
            (old_lvl + 1).min(1)
        } else {
            old_lvl.saturating_sub(1)
        };
        if new_lvl == old_lvl {
            return;
        }
        self.push_metadata_undo_snapshot();
        self.bookmarks[idx].level = new_lvl;
        self.status_notice = Some(format!(
            "{} '{}' (Ctrl+Z to undo)",
            if new_lvl > old_lvl { "Indented" } else { "Outdented" },
            self.bookmarks[idx].title
        ));
        self.update_title_bar();
        self.request_redraw();
    }

    pub(super) fn auto_detect_bookmarks_from_headings(&mut self) {
        let Some(doc) = self.doc.as_ref() else {
            self.status_notice = Some("Open a PDF first to auto-detect headings".to_string());
            self.request_redraw();
            return;
        };
        let detected = crate::export::detect_document_headings(doc);
        if detected.is_empty() {
            self.status_notice = Some(
                "No distinct headings detected (scanned/uniform-font PDF — add manually with 'A')"
                    .to_string(),
            );
            self.request_redraw();
            return;
        }
        self.push_metadata_undo_snapshot();
        let mut major = 0usize;
        let mut minor = 0usize;
        let mut generated = Vec::with_capacity(detected.len());
        for (page_idx, level, title) in detected {
            let serial = if level == 0 {
                major += 1;
                minor = 0;
                major.to_string()
            } else {
                if major == 0 {
                    major = 1;
                }
                minor += 1;
                format!("{major}.{minor}")
            };
            let logical_page = PdfEngine::format_logical_page_label_with_skip(
                page_idx,
                self.skip_pages,
                self.roman_end,
            );
            generated.push(Bookmark {
                serial,
                title,
                logical_page,
                page_index: page_idx,
                level,
            });
        }
        let count = generated.len();
        self.bookmarks = generated;
        self.selected_bookmark = Some(0);
        self.bookmark_scroll_y = 0;
        self.show_bookmarks = true;
        self.status_notice = Some(format!(
            "Auto-detected {count} heading bookmark(s)! (Ctrl+Z to undo, S to save)"
        ));
        self.update_title_bar();
        self.request_redraw();
    }

    pub(super) fn export_bookmarks_to_path(&mut self, txt_path: &Path) {
        let content = PdfEngine::format_bookmarks_as_txt(&self.bookmarks);
        match std::fs::write(txt_path, content.as_bytes()) {
            Ok(()) => {
                let fname = txt_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("bookmarks.txt");
                self.status_notice = Some(format!(
                    "Exported {} bookmark(s) to '{fname}'",
                    self.bookmarks.len()
                ));
            }
            Err(e) => {
                self.status_notice = Some(format!("Export bookmarks failed: {e}"));
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    pub(super) fn import_bookmarks_from_path(&mut self, txt_path: &Path) {
        match std::fs::read_to_string(txt_path) {
            Ok(content) => {
                let total = self.total_pages() as usize;
                let parsed = PdfEngine::parse_bookmarks_txt_with_skip(
                    &content,
                    self.skip_pages,
                    self.roman_end,
                    total,
                );
                let count = parsed.len();
                self.push_metadata_undo_snapshot();
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
                self.status_notice = Some(format!("Imported {count} bookmarks! Press S to save into PDF."));
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

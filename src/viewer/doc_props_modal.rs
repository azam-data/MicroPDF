//! Tier 1 Modals & Floating Popovers:
//! 1. `UnsavedChangesModalState`: Save / Discard / Cancel guard on dirty tab close or app exit.
//! 2. `DocPropsModalState`: `File -> Document Properties...` + one-click Privacy Metadata Scrubber.
//! 3. `AnnotationContextMenuState`: Right-click inspector over any annotation (Edit Text, Recolor, Delete).

use crate::engine::{AnnotationColor, DocumentPropertiesInfo, PdfEngine};

use super::render::{blend_rect_rgba, fill_rect_clipped};
use super::state::*;
use super::ui_draw::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum PendingCloseAction {
    CloseTab(usize),
    CloseOtherTabs(usize),
    CloseTabsToRight(usize),
    CloseAllTabs,
    ExitApp,
}

#[derive(Clone, Debug)]
pub(super) struct UnsavedChangesModalState {
    pub(super) action: PendingCloseAction,
    pub(super) dirty_tab_indices: Vec<usize>,
    pub(super) dirty_titles: Vec<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum UnsavedChangesHit {
    Save,
    Discard,
    Cancel,
    InsideModal,
}

#[derive(Clone, Debug)]
pub(super) struct DocPropsModalState {
    pub(super) info: DocumentPropertiesInfo,
    pub(super) scrubbed_just_now: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum DocPropsHit {
    ScrubMetadata,
    Close,
    InsideModal,
}

#[derive(Clone, Debug)]
pub(super) struct AnnotationContextMenuState {
    pub(super) page_idx: u16,
    pub(super) annot_idx: usize,
    pub(super) label: &'static str,
    pub(super) is_text_editable: bool,
    pub(super) current_text: String,
    pub(super) x: i32,
    pub(super) y: i32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum AnnotContextHit {
    EditText,
    Color(AnnotationColor),
    Delete,
    InsideMenu,
}

#[derive(Clone, Debug)]
pub(super) struct ActiveNotePopover {
    pub(super) page_idx: u16,
    pub(super) annot_idx: usize,
    pub(super) text: String,
    pub(super) x: i32,
    pub(super) y: i32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum NotePopoverHit {
    Edit,
    Delete,
    Close,
    InsidePopover,
}

impl<'a> PdfViewerApp<'a> {
    // =========================================================================
    // 1. Unsaved-Changes Confirmation Modal (Save / Discard / Cancel)
    // =========================================================================

    fn collect_dirty_tabs_for_action(&self, action: PendingCloseAction) -> Vec<(usize, String)> {
        let mut out = Vec::new();
        let tab_count = self.tabs.len();
        let check_indices: Vec<usize> = match action {
            PendingCloseAction::CloseTab(idx) => {
                if idx < tab_count {
                    vec![idx]
                } else {
                    vec![]
                }
            }
            PendingCloseAction::CloseOtherTabs(keep) => (0..tab_count).filter(|&i| i != keep).collect(),
            PendingCloseAction::CloseTabsToRight(anchor) => ((anchor + 1)..tab_count).collect(),
            PendingCloseAction::CloseAllTabs | PendingCloseAction::ExitApp => (0..tab_count).collect(),
        };
        for idx in check_indices {
            let is_dirty = if idx == self.active_tab {
                self.has_unsaved_edits
            } else {
                self.tabs[idx].has_unsaved_edits
            };
            if is_dirty {
                let path = if idx == self.active_tab {
                    &self.file_path
                } else {
                    &self.tabs[idx].file_path
                };
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Untitled.pdf")
                    .to_string();
                out.push((idx, name));
            }
        }
        out
    }

    /// Requests a close/exit operation, opening the Unsaved Changes confirmation dialog if any affected tab is dirty.
    /// Returns `true` if the action executed immediately (no unsaved changes).
    pub(super) fn request_close_action(&mut self, action: PendingCloseAction) -> bool {
        let dirty = self.collect_dirty_tabs_for_action(action);
        if dirty.is_empty() {
            self.execute_close_action_confirmed(action);
            return true;
        }
        self.close_all_popovers();
        self.unsaved_changes_modal = Some(UnsavedChangesModalState {
            action,
            dirty_tab_indices: dirty.iter().map(|(i, _)| *i).collect(),
            dirty_titles: dirty.into_iter().map(|(_, n)| n).collect(),
        });
        self.request_redraw();
        false
    }

    pub(super) fn execute_close_action_confirmed(&mut self, action: PendingCloseAction) {
        match action {
            PendingCloseAction::CloseTab(idx) => {
                let _ = self.close_tab(idx);
            }
            PendingCloseAction::CloseOtherTabs(keep) => {
                self.close_other_tabs(keep);
            }
            PendingCloseAction::CloseTabsToRight(anchor) => {
                self.close_tabs_to_right(anchor);
            }
            PendingCloseAction::CloseAllTabs => {
                self.close_all_tabs();
            }
            PendingCloseAction::ExitApp => {
                self.save_session_to_ini();
                self.stop_single_instance_listener();
                self.pending_app_exit = true;
                self.request_redraw();
            }
        }
    }

    fn unsaved_modal_layout(
        &self,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<(i32, i32, u32, u32, Vec<(i32, i32, u32, u32, UnsavedChangesHit)>)> {
        let st = self.unsaved_changes_modal.as_ref()?;
        let s = sf as f32;
        let mw = ((460.0 * s).round() as u32).min(buf_w.saturating_sub(32)).max(320);
        let list_lines = st.dirty_titles.len().min(5) as u32;
        let mh = ((176.0 + list_lines as f32 * 20.0) * s).round() as u32;
        let mh = mh.min(buf_h.saturating_sub(32)).max(170);
        let mx = ((buf_w.saturating_sub(mw)) / 2) as i32;
        let my = ((buf_h.saturating_sub(mh)) / 2) as i32;

        let pad = (18.0 * s).round() as i32;
        let btn_h = (30.0 * s).round() as u32;
        let gap = (8.0 * s).round() as i32;
        let fy = my + mh as i32 - pad - btn_h as i32;

        let save_w = (120.0 * s).round() as u32;
        let discard_w = (102.0 * s).round() as u32;
        let cancel_w = (86.0 * s).round() as u32;

        let save_x = mx + mw as i32 - pad - save_w as i32;
        let discard_x = save_x - discard_w as i32 - gap;
        let cancel_x = mx + pad;

        let hits = vec![
            (cancel_x, fy, cancel_w, btn_h, UnsavedChangesHit::Cancel),
            (discard_x, fy, discard_w, btn_h, UnsavedChangesHit::Discard),
            (save_x, fy, save_w, btn_h, UnsavedChangesHit::Save),
        ];
        Some((mx, my, mw, mh, hits))
    }

    pub(super) fn draw_unsaved_changes_modal(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) {
        let Some(st) = self.unsaved_changes_modal.as_ref() else {
            return;
        };
        let Some((mx, my, mw, mh, hits)) = self.unsaved_modal_layout(buf_w, buf_h, sf) else {
            return;
        };
        let theme = self.ui_theme();
        let s = sf as f32;
        let pad = (18.0 * s).round() as i32;
        let rad = (10.0 * s).max(6.0);
        let btn_rad = (6.0 * s).max(4.0);
        let title_sz = (14.5 * s).clamp(12.0, 28.0);
        let body_sz = (11.5 * s).clamp(10.0, 22.0);
        let sub_sz = (10.5 * s).clamp(9.0, 20.0);

        blend_rect_rgba(buffer, buf_w, buf_h, 0, 0, buf_w, buf_h, theme.modal_scrim);
        draw_soft_shadow(buffer, buf_w, buf_h, mx, my, mw, mh, rad, 18, 6, theme.shadow_rgb, theme.shadow_max_alpha);
        fill_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.border_subtle);

        let title = if st.dirty_titles.len() == 1 {
            "Save changes before closing?"
        } else {
            "Save changes to modified tabs?"
        };
        draw_text(buffer, buf_w, buf_h, mx + pad, my + pad, title, title_sz, theme.text_primary, true);

        let subtitle = if st.dirty_titles.len() == 1 {
            "Your edits will be lost if you close without saving:"
        } else {
            "The following tabs have unsaved changes:"
        };
        draw_text(buffer, buf_w, buf_h, mx + pad, my + pad + (26.0 * s).round() as i32, subtitle, body_sz, theme.text_secondary, false);

        let mut ly = my + pad + (50.0 * s).round() as i32;
        let line_step = (20.0 * s).round() as i32;
        for (i, name) in st.dirty_titles.iter().take(4).enumerate() {
            let text = if i == 3 && st.dirty_titles.len() > 4 {
                format!("• {} (+{} more)", name, st.dirty_titles.len() - 4)
            } else {
                format!("• {name}")
            };
            let clipped = truncate_ellipsis(&text, (mw.saturating_sub((pad * 2) as u32)) as f32, sub_sz, true);
            draw_text(buffer, buf_w, buf_h, mx + pad + (6.0 * s).round() as i32, ly, &clipped, sub_sz, theme.text_primary, true);
            ly += line_step;
        }

        for &(bx, by, bw, bh, hit) in &hits {
            let hov = self.hovered_widget == Some(UiHit::UnsavedChanges(hit));
            match hit {
                UnsavedChangesHit::Cancel => {
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, if hov { theme.hover_bg } else { theme.surface_elevated });
                    stroke_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, theme.border_subtle);
                    let tw = measure_text("Cancel", body_sz, false);
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round() as i32, by, bh, bx + bw as i32, "Cancel", body_sz, theme.text_primary, false);
                }
                UnsavedChangesHit::Discard => {
                    let bg = if hov { theme.danger } else { theme.danger_soft_bg };
                    let fg = if hov { theme.text_on_accent } else { theme.danger };
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, bg);
                    stroke_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, theme.danger);
                    let tw = measure_text("Discard", body_sz, true);
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round() as i32, by, bh, bx + bw as i32, "Discard", body_sz, fg, true);
                }
                UnsavedChangesHit::Save => {
                    let lbl = if st.dirty_titles.len() > 1 { "Save All" } else { "Save" };
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, if hov { theme.accent_hover } else { theme.accent });
                    let tw = measure_text(lbl, body_sz, true);
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round() as i32, by, bh, bx + bw as i32, lbl, body_sz, theme.text_on_accent, true);
                }
                UnsavedChangesHit::InsideModal => {}
            }
        }
    }

    pub(super) fn hit_test_unsaved_modal(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<UnsavedChangesHit> {
        let (x, y, w, h, hits) = self.unsaved_modal_layout(buf_w, buf_h, sf)?;
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        for (bx, by, bw, bh, hit) in hits {
            if ix >= bx && ix < bx + bw as i32 && iy >= by && iy < by + bh as i32 {
                return Some(hit);
            }
        }
        if ix >= x && ix < x + w as i32 && iy >= y && iy < y + h as i32 {
            return Some(UnsavedChangesHit::InsideModal);
        }
        None
    }

    pub(super) fn handle_unsaved_modal_click(&mut self, hit: UnsavedChangesHit) {
        let Some(st) = self.unsaved_changes_modal.clone() else {
            return;
        };
        match hit {
            UnsavedChangesHit::Cancel => {
                self.unsaved_changes_modal = None;
                self.request_redraw();
            }
            UnsavedChangesHit::Discard => {
                self.unsaved_changes_modal = None;
                self.execute_close_action_confirmed(st.action);
            }
            UnsavedChangesHit::Save => {
                self.unsaved_changes_modal = None;
                let prev_active = self.active_tab;
                let mut all_saved = true;
                for idx in st.dirty_tab_indices {
                    if idx >= self.tabs.len() {
                        continue;
                    }
                    if idx != self.active_tab {
                        self.switch_to_tab(idx);
                    }
                    let target = self.file_path.clone();
                    let ok = if target.as_os_str().is_empty() || !target.exists() {
                        if let Some(save_path) = rfd::FileDialog::new()
                            .set_title("Save Modified PDF")
                            .add_filter("PDF Document", &["pdf"])
                            .set_file_name(self.filename_display())
                            .save_file()
                        {
                            self.save_active_doc_to(save_path)
                        } else {
                            false
                        }
                    } else {
                        self.save_active_doc_to(target)
                    };
                    if !ok {
                        all_saved = false;
                        break;
                    }
                }
                if all_saved {
                    if prev_active < self.tabs.len() && prev_active != self.active_tab {
                        self.switch_to_tab(prev_active);
                    }
                    self.execute_close_action_confirmed(st.action);
                } else {
                    self.status_notice = Some("Close cancelled — unsaved changes were kept open".to_string());
                    self.update_title_bar();
                    self.request_redraw();
                }
            }
            UnsavedChangesHit::InsideModal => {}
        }
    }

    // =========================================================================
    // 2. Document Properties & Privacy Metadata Scrubber Modal
    // =========================================================================

    pub(super) fn open_document_properties_modal(&mut self) {
        if self.doc.is_none() {
            self.status_notice = Some("Open a PDF first to inspect Document Properties".to_string());
            self.request_redraw();
            return;
        }
        self.close_all_popovers();
        let info = PdfEngine::inspect_document_properties(
            self.doc.as_ref().unwrap(),
            &self.file_path,
            self.current_page_index,
        );
        self.doc_props_modal = Some(DocPropsModalState {
            info,
            scrubbed_just_now: false,
        });
        self.request_redraw();
    }

    fn doc_props_layout(
        &self,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<(i32, i32, u32, u32, Vec<(i32, i32, u32, u32, DocPropsHit)>)> {
        let _st = self.doc_props_modal.as_ref()?;
        let s = sf as f32;
        let mw = ((580.0 * s).round() as u32).min(buf_w.saturating_sub(32)).max(360);
        let mh = ((480.0 * s).round() as u32).min(buf_h.saturating_sub(32)).max(350);
        let mx = ((buf_w.saturating_sub(mw)) / 2) as i32;
        let my = ((buf_h.saturating_sub(mh)) / 2) as i32;
        let pad = (16.0 * s).round() as i32;
        let hdr_h = (44.0 * s).round() as u32;
        let footer_h = (54.0 * s).round() as u32;
        let btn_h = (30.0 * s).round() as u32;

        let close_sz = (24.0 * s).round() as u32;
        let fy = my + mh as i32 - footer_h as i32 + ((footer_h.saturating_sub(btn_h)) / 2) as i32;
        let scrub_w = (232.0 * s).round() as u32;
        let close_w = (88.0 * s).round() as u32;

        let hits = vec![
            (
                mx + mw as i32 - pad - close_sz as i32,
                my + ((hdr_h.saturating_sub(close_sz)) / 2) as i32,
                close_sz,
                close_sz,
                DocPropsHit::Close,
            ),
            (mx + pad, fy, scrub_w, btn_h, DocPropsHit::ScrubMetadata),
            (mx + mw as i32 - pad - close_w as i32, fy, close_w, btn_h, DocPropsHit::Close),
        ];
        Some((mx, my, mw, mh, hits))
    }

    pub(super) fn draw_doc_props_modal(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) {
        let Some(st) = self.doc_props_modal.as_ref() else {
            return;
        };
        let Some((mx, my, mw, mh, hits)) = self.doc_props_layout(buf_w, buf_h, sf) else {
            return;
        };
        let theme = self.ui_theme();
        let s = sf as f32;
        let pad = (16.0 * s).round() as i32;
        let hdr_h = (44.0 * s).round() as u32;
        let footer_h = (54.0 * s).round() as u32;
        let rad = (10.0 * s).max(6.0);
        let btn_rad = (6.0 * s).max(4.0);
        let title_sz = (14.5 * s).clamp(12.0, 28.0);
        let sec_sz = (11.5 * s).clamp(10.0, 22.0);
        let body_sz = (11.0 * s).clamp(9.5, 21.0);

        blend_rect_rgba(buffer, buf_w, buf_h, 0, 0, buf_w, buf_h, theme.modal_scrim);
        draw_soft_shadow(buffer, buf_w, buf_h, mx, my, mw, mh, rad, 18, 6, theme.shadow_rgb, theme.shadow_max_alpha);
        fill_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.border_subtle);

        draw_text_vcenter(buffer, buf_w, buf_h, mx + pad, my, hdr_h, mx + mw as i32 - 48, "Document Properties & Privacy", title_sz, theme.text_primary, true);
        fill_rect_clipped(buffer, buf_w, buf_h, mx, my + hdr_h as i32, mw, 1, theme.divider);

        let info = &st.info;
        let size_str = if info.file_size_bytes >= 1024 * 1024 {
            format!("{:.2} MB ({} bytes)", info.file_size_bytes as f64 / (1024.0 * 1024.0), info.file_size_bytes)
        } else if info.file_size_bytes >= 1024 {
            format!("{:.1} KB ({} bytes)", info.file_size_bytes as f64 / 1024.0, info.file_size_bytes)
        } else {
            format!("{} bytes", info.file_size_bytes)
        };
        let (pw, ph) = info.current_page_size_pt;
        let page_dim_str = format!(
            "{:.1} × {:.1} mm  ({:.2} × {:.2} in · {:.0} × {:.0} pt)",
            pw * 25.4 / 72.0,
            ph * 25.4 / 72.0,
            pw / 72.0,
            ph / 72.0,
            pw,
            ph
        );
        let sec_str = if info.is_encrypted { "Encrypted / Restricted" } else { "Standard (Unencrypted)" };
        let fonts_str = if info.fonts.is_empty() {
            "None / Image-only PDF".to_string()
        } else {
            info.fonts.iter().take(6).cloned().collect::<Vec<_>>().join(", ")
        };
        let xmp_str = if info.has_xmp_metadata {
            "Present (XML metadata embedded)"
        } else {
            "Clean (no XMP stream)"
        };
        let or_dash = |v: &str| if v.trim().is_empty() { "—".to_string() } else { v.to_string() };

        let rows_general: [(&str, String); 6] = [
            ("File Name", info.file_name.clone()),
            ("File Size", size_str),
            ("PDF Version", format!("PDF {}  ·  {} page(s)", info.pdf_version, info.page_count)),
            ("Page Size", page_dim_str),
            ("Security", sec_str.to_string()),
            ("Fonts", fonts_str),
        ];
        let rows_meta: [(&str, String); 7] = [
            ("Title", or_dash(&info.title)),
            ("Author", or_dash(&info.author)),
            ("Subject", or_dash(&info.subject)),
            ("Creator App", or_dash(&info.creator)),
            ("PDF Producer", or_dash(&info.producer)),
            ("Created / Mod", format!("{}  /  {}", or_dash(&info.creation_date), or_dash(&info.mod_date))),
            ("XMP Metadata", xmp_str.to_string()),
        ];

        let label_w = (108.0 * s).round() as i32;
        let val_x = mx + pad + label_w;
        let val_budget = (mw as i32 - pad * 2 - label_w).max(60) as f32;
        let row_step = (22.0 * s).round() as i32;

        let mut cy = my + hdr_h as i32 + (12.0 * s).round() as i32;
        draw_text(buffer, buf_w, buf_h, mx + pad, cy, "File & Page Summary", sec_sz, theme.accent, true);
        cy += (22.0 * s).round() as i32;
        for (k, v) in &rows_general {
            draw_text(buffer, buf_w, buf_h, mx + pad, cy, k, body_sz, theme.text_muted, true);
            let clipped = truncate_ellipsis(v, val_budget, body_sz, false);
            draw_text(buffer, buf_w, buf_h, val_x, cy, &clipped, body_sz, theme.text_primary, false);
            cy += row_step;
        }

        cy += (6.0 * s).round() as i32;
        fill_rect_clipped(buffer, buf_w, buf_h, mx + pad, cy, mw.saturating_sub((pad * 2) as u32), 1, theme.divider);
        cy += (10.0 * s).round() as i32;

        let meta_hdr = if st.scrubbed_just_now {
            "Embedded Metadata — Scrubbed Clean! (Press S to save)"
        } else {
            "Embedded Metadata (/Info & XMP)"
        };
        draw_text(buffer, buf_w, buf_h, mx + pad, cy, meta_hdr, sec_sz, if st.scrubbed_just_now { theme.success } else { theme.accent }, true);
        cy += (22.0 * s).round() as i32;
        for (k, v) in &rows_meta {
            draw_text(buffer, buf_w, buf_h, mx + pad, cy, k, body_sz, theme.text_muted, true);
            let clipped = truncate_ellipsis(v, val_budget, body_sz, false);
            draw_text(buffer, buf_w, buf_h, val_x, cy, &clipped, body_sz, theme.text_primary, false);
            cy += row_step;
        }

        let footer_y = my + mh as i32 - footer_h as i32;
        fill_rect_clipped(buffer, buf_w, buf_h, mx, footer_y, mw, 1, theme.divider);

        let ic_sz = (12.0 * s).round() as u32;
        for &(bx, by, bw, bh, hit) in &hits {
            let hov = self.hovered_widget == Some(UiHit::DocProps(hit));
            match hit {
                DocPropsHit::Close if by < my + hdr_h as i32 => {
                    if hov {
                        fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, theme.hover_bg);
                    }
                    draw_icon(buffer, buf_w, buf_h, UiIcon::Close, bx + ((bw.saturating_sub(ic_sz)) / 2) as i32, by + ((bh.saturating_sub(ic_sz)) / 2) as i32, ic_sz, theme.text_secondary);
                }
                DocPropsHit::Close => {
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, if hov { theme.hover_bg } else { theme.surface_elevated });
                    stroke_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, theme.border_subtle);
                    let tw = measure_text("Close", body_sz, true);
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round() as i32, by, bh, bx + bw as i32, "Close", body_sz, theme.text_primary, true);
                }
                DocPropsHit::ScrubMetadata => {
                    let has_any = info.has_info_dict || info.has_xmp_metadata;
                    let can_scrub = !info.is_encrypted && has_any;
                    let bg = if !can_scrub {
                        theme.surface_elevated
                    } else if hov {
                        theme.danger
                    } else {
                        theme.danger_soft_bg
                    };
                    let fg = if !can_scrub {
                        theme.text_muted
                    } else if hov {
                        theme.text_on_accent
                    } else {
                        theme.danger
                    };
                    let bdr = if can_scrub { theme.danger } else { theme.border_subtle };
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, bg);
                    stroke_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, btn_rad, bdr);
                    let lbl = if !has_any {
                        "Metadata Already Clean"
                    } else {
                        "Strip Personal Metadata & XMP"
                    };
                    let tw = measure_text(lbl, body_sz, true);
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round() as i32, by, bh, bx + bw as i32, lbl, body_sz, fg, true);
                }
                DocPropsHit::InsideModal => {}
            }
        }
    }

    pub(super) fn hit_test_doc_props_modal(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<DocPropsHit> {
        let (x, y, w, h, hits) = self.doc_props_layout(buf_w, buf_h, sf)?;
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        for (bx, by, bw, bh, hit) in hits {
            if ix >= bx && ix < bx + bw as i32 && iy >= by && iy < by + bh as i32 {
                return Some(hit);
            }
        }
        if ix >= x && ix < x + w as i32 && iy >= y && iy < y + h as i32 {
            return Some(DocPropsHit::InsideModal);
        }
        None
    }

    pub(super) fn handle_doc_props_click(&mut self, hit: DocPropsHit) {
        match hit {
            DocPropsHit::Close => {
                self.doc_props_modal = None;
                self.request_redraw();
            }
            DocPropsHit::ScrubMetadata => {
                let Some(doc) = self.doc.as_ref() else {
                    return;
                };
                let Ok(bytes) = doc.save_to_bytes() else {
                    self.status_notice = Some("Failed to read PDF bytes for metadata scrubbing".to_string());
                    self.request_redraw();
                    return;
                };
                match PdfEngine::scrub_document_metadata_bytes(&bytes) {
                    Ok(cleaned) => {
                        self.push_undo_snapshot();
                        match self.engine.load_pdf_from_bytes_safe(cleaned.clone()) {
                            Ok(new_doc) => {
                                self.doc = Some(new_doc);
                                self.invalidate_page_caches();
                                let new_info = if let Some(d) = self.doc.as_ref() {
                                    let mut inf = PdfEngine::inspect_document_properties_from_bytes(
                                        &cleaned,
                                        &self.filename_display(),
                                        d.pages().len(),
                                        d.pages()
                                            .get(self.current_page_index)
                                            .map(|p| (p.width().value, p.height().value))
                                            .unwrap_or((595.28, 841.89)),
                                    );
                                    inf.file_size_bytes = cleaned.len() as u64;
                                    inf
                                } else {
                                    DocumentPropertiesInfo::default()
                                };
                                self.doc_props_modal = Some(DocPropsModalState {
                                    info: new_info,
                                    scrubbed_just_now: true,
                                });
                                self.status_notice = Some(
                                    "Stripped /Info & XMP metadata (Ctrl+Z to undo, S to save)"
                                        .to_string(),
                                );
                            }
                            Err(e) => {
                                self.status_notice = Some(format!("Failed to reload scrubbed PDF: {e}"));
                            }
                        }
                    }
                    Err(e) => {
                        self.status_notice = Some(format!("Metadata scrub failed: {e}"));
                    }
                }
                self.update_title_bar();
                self.request_redraw();
            }
            DocPropsHit::InsideModal => {}
        }
    }

    // =========================================================================
    // 3. Right-Click Annotation Context Menu (Edit Text, Recolor, Delete)
    // =========================================================================

    fn annot_context_layout(
        &self,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<(AnnotationContextMenuState, i32, i32, u32, u32, Vec<(i32, i32, u32, u32, AnnotContextHit)>)> {
        let st = self.annot_context_menu.clone()?;
        let s = sf as f32;
        let mw = (224.0 * s).round() as u32;
        let pad = (8.0 * s).round() as i32;
        let hdr_h = (26.0 * s).round() as u32;
        let row_h = (28.0 * s).round() as u32;
        let swatch_row_h = (32.0 * s).round() as u32;
        let gap = (4.0 * s).round() as i32;
        let edit_h = if st.is_text_editable { row_h + gap as u32 } else { 0 };
        let mh = hdr_h + edit_h + swatch_row_h + row_h + (16.0 * s).round() as u32;
        let mx = st.x.clamp(8, (buf_w as i32 - mw as i32 - 8).max(8));
        let my = st.y.clamp(8, (buf_h as i32 - mh as i32 - 8).max(8));

        let iw = mw.saturating_sub((pad * 2) as u32);
        let mut hits = Vec::new();
        let mut cy = my + hdr_h as i32 + (4.0 * s).round() as i32;

        if st.is_text_editable {
            hits.push((mx + pad, cy, iw, row_h, AnnotContextHit::EditText));
            cy += row_h as i32 + gap;
        }

        // Row of 7 clickable color dots
        let dot_sz = (20.0 * s).round() as u32;
        let dot_gap = ((iw.saturating_sub(dot_sz * 7)) / 6).max(3);
        let dot_y = cy + ((swatch_row_h.saturating_sub(dot_sz)) / 2) as i32;
        for (i, color) in AnnotationColor::ALL.iter().enumerate() {
            let dx = mx + pad + (i as i32) * (dot_sz + dot_gap) as i32;
            hits.push((dx, dot_y, dot_sz, dot_sz, AnnotContextHit::Color(*color)));
        }
        cy += swatch_row_h as i32 + gap;

        hits.push((mx + pad, cy, iw, row_h, AnnotContextHit::Delete));
        Some((st, mx, my, mw, mh, hits))
    }

    pub(super) fn draw_annot_context_menu(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) {
        let Some((st, mx, my, mw, mh, hits)) = self.annot_context_layout(buf_w, buf_h, sf) else {
            return;
        };
        let theme = self.ui_theme();
        let s = sf as f32;
        let pad = (8.0 * s).round() as i32;
        let hdr_h = (26.0 * s).round() as u32;
        let rad = (7.0 * s).max(4.0);
        let body_sz = (11.5 * s).clamp(10.0, 22.0);
        let sub_sz = (10.5 * s).clamp(9.0, 20.0);

        draw_soft_shadow(buffer, buf_w, buf_h, mx, my, mw, mh, rad, 12, 4, theme.shadow_rgb, theme.shadow_max_alpha);
        fill_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.border_subtle);

        let hdr = format!("{} (Page {})", st.label, st.page_idx + 1);
        draw_text_vcenter(buffer, buf_w, buf_h, mx + pad + 2, my, hdr_h, mx + mw as i32 - pad, &hdr, sub_sz, theme.text_muted, true);
        fill_rect_clipped(buffer, buf_w, buf_h, mx, my + hdr_h as i32, mw, 1, theme.divider);

        for &(bx, by, bw, bh, hit) in &hits {
            let hov = self.hovered_widget == Some(UiHit::AnnotContext(hit));
            match hit {
                AnnotContextHit::EditText => {
                    if hov {
                        fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, 5.0, theme.hover_bg);
                    }
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + (8.0 * s).round() as i32, by, bh, bx + bw as i32 - 6, "Edit Text...", body_sz, theme.text_primary, false);
                }
                AnnotContextHit::Color(color) => {
                    let (r, g, b) = color.rgb();
                    let rgb = ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, (bw as f32) * 0.5, rgb);
                    let ring = if hov { theme.accent } else { theme.border_subtle };
                    stroke_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, (bw as f32) * 0.5, ring);
                }
                AnnotContextHit::Delete => {
                    if hov {
                        fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, 5.0, theme.danger_soft_bg);
                    }
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + (8.0 * s).round() as i32, by, bh, bx + bw as i32 - 6, "Delete Annotation", body_sz, theme.danger, true);
                }
                AnnotContextHit::InsideMenu => {}
            }
        }
    }

    pub(super) fn hit_test_annot_context_menu(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<AnnotContextHit> {
        let (_, x, y, w, h, hits) = self.annot_context_layout(buf_w, buf_h, sf)?;
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        for (bx, by, bw, bh, hit) in hits {
            if ix >= bx && ix < bx + bw as i32 && iy >= by && iy < by + bh as i32 {
                return Some(hit);
            }
        }
        if ix >= x && ix < x + w as i32 && iy >= y && iy < y + h as i32 {
            return Some(AnnotContextHit::InsideMenu);
        }
        None
    }

    pub(super) fn handle_annot_context_click(&mut self, hit: AnnotContextHit) {
        let Some(st) = self.annot_context_menu.clone() else {
            return;
        };
        match hit {
            AnnotContextHit::EditText => {
                self.annot_context_menu = None;
                self.active_note_popover = None;
                self.input_mode = InputMode::EditAnnotationText {
                    page_idx: st.page_idx,
                    annot_idx: st.annot_idx,
                    label: st.label,
                    text_input: st.current_text,
                };
                self.update_title_bar();
                self.request_redraw();
            }
            AnnotContextHit::Color(color) => {
                self.annot_context_menu = None;
                self.push_undo_snapshot();
                if let Some(doc) = self.doc.as_mut() {
                    match PdfEngine::recolor_annotation_at(doc, st.page_idx, st.annot_idx, color) {
                        Ok(()) => {
                            self.invalidate_page_caches();
                            self.thumbnail_cache.remove(&st.page_idx);
                            self.status_notice = Some(format!(
                                "Changed {} color to {} (Ctrl+Z to undo)",
                                st.label,
                                color.label()
                            ));
                        }
                        Err(e) => {
                            self.status_notice = Some(format!("Recolor annotation failed: {e}"));
                        }
                    }
                }
                self.update_title_bar();
                self.request_redraw();
            }
            AnnotContextHit::Delete => {
                self.annot_context_menu = None;
                self.active_note_popover = None;
                self.push_undo_snapshot();
                if let Some(doc) = self.doc.as_mut() {
                    match PdfEngine::delete_annotation_at(doc, st.page_idx, st.annot_idx) {
                        Ok(()) => {
                            self.invalidate_page_caches();
                            self.thumbnail_cache.remove(&st.page_idx);
                            self.status_notice = Some(format!(
                                "Deleted {} on Page {} (Ctrl+Z to undo)",
                                st.label,
                                st.page_idx + 1
                            ));
                        }
                        Err(e) => {
                            self.status_notice = Some(format!("Delete annotation failed: {e}"));
                        }
                    }
                }
                self.update_title_bar();
                self.request_redraw();
            }
            AnnotContextHit::InsideMenu => {}
        }
    }

    // =========================================================================
    // 4. Multi-Line Sticky Note Reader Popover with Edit & Delete Buttons
    // =========================================================================

    fn wrap_popover_lines(text: &str, max_w: f32, font_sz: f32) -> Vec<String> {
        let mut lines = Vec::new();
        for para in text.split('\n') {
            if para.trim().is_empty() {
                lines.push(String::new());
                continue;
            }
            let mut cur = String::new();
            for word in para.split_whitespace() {
                let candidate = if cur.is_empty() {
                    word.to_string()
                } else {
                    format!("{cur} {word}")
                };
                if measure_text(&candidate, font_sz, false) <= max_w || cur.is_empty() {
                    cur = candidate;
                } else {
                    lines.push(cur);
                    cur = word.to_string();
                }
                if lines.len() >= 10 {
                    break;
                }
            }
            if !cur.is_empty() && lines.len() < 10 {
                lines.push(cur);
            }
            if lines.len() >= 10 {
                break;
            }
        }
        if lines.is_empty() {
            lines.push("(Empty note)".to_string());
        }
        lines
    }

    pub(super) fn note_popover_layout(
        &self,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<(ActiveNotePopover, i32, i32, u32, u32, Vec<String>, Vec<(i32, i32, u32, u32, NotePopoverHit)>)> {
        let note = self.active_note_popover.clone()?;
        let s = sf as f32;
        let pad = (12.0 * s).round() as i32;
        let body_sz = (11.5 * s).clamp(10.0, 22.0);
        let max_pw = ((340.0 * s).round() as u32).min(buf_w.saturating_sub(24)).max(220);
        let text_w = (max_pw as i32 - pad * 2).max(120) as f32;
        let lines = Self::wrap_popover_lines(&note.text, text_w, body_sz);
        let line_h = (17.0 * s).round() as u32;
        let hdr_h = (28.0 * s).round() as u32;
        let footer_h = (32.0 * s).round() as u32;
        let body_h = (lines.len() as u32) * line_h + (10.0 * s).round() as u32;
        let ph = (hdr_h + body_h + footer_h + (8.0 * s).round() as u32).min(buf_h.saturating_sub(24));
        let px = note.x.clamp(12, (buf_w as i32 - max_pw as i32 - 12).max(12));
        let py = (note.y - ph as i32 - 10).clamp(48, (buf_h as i32 - ph as i32 - 36).max(48));

        let btn_h = (22.0 * s).round() as u32;
        let edit_w = (56.0 * s).round() as u32;
        let del_w = (62.0 * s).round() as u32;
        let close_sz = (20.0 * s).round() as u32;
        let by = py + ph as i32 - footer_h as i32 + ((footer_h.saturating_sub(btn_h)) / 2) as i32;
        let hits = vec![
            (
                px + max_pw as i32 - pad - close_sz as i32,
                py + ((hdr_h.saturating_sub(close_sz)) / 2) as i32,
                close_sz,
                close_sz,
                NotePopoverHit::Close,
            ),
            (px + pad, by, edit_w, btn_h, NotePopoverHit::Edit),
            (px + pad + edit_w as i32 + (8.0 * s).round() as i32, by, del_w, btn_h, NotePopoverHit::Delete),
        ];
        Some((note, px, py, max_pw, ph, lines, hits))
    }

    pub(super) fn draw_active_note_popover(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) {
        let Some((note, px, py, pw, ph, lines, hits)) = self.note_popover_layout(buf_w, buf_h, sf) else {
            return;
        };
        let theme = self.ui_theme();
        let s = sf as f32;
        let pad = (12.0 * s).round() as i32;
        let hdr_h = (28.0 * s).round() as u32;
        let line_h = (17.0 * s).round() as i32;
        let rad = (8.0 * s).max(5.0);
        let body_sz = (11.5 * s).clamp(10.0, 22.0);
        let sub_sz = (10.5 * s).clamp(9.0, 20.0);

        let bg = if self.ui_theme_mode == super::theme::UiThemeMode::Dark {
            0x002B_281D
        } else {
            0x00FF_FBEB
        };
        let border = 0x00D9_7706;
        draw_soft_shadow(buffer, buf_w, buf_h, px, py, pw, ph, rad, 12, 4, theme.shadow_rgb, theme.shadow_max_alpha);
        fill_rounded_rect(buffer, buf_w, buf_h, px, py, pw, ph, rad, bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, px, py, pw, ph, rad, border);

        let hdr_txt = format!("Sticky Note — Page {}", note.page_idx + 1);
        draw_text_vcenter(buffer, buf_w, buf_h, px + pad, py, hdr_h, px + pw as i32 - 36, &hdr_txt, sub_sz, border, true);
        fill_rect_clipped(buffer, buf_w, buf_h, px + 1, py + hdr_h as i32, pw.saturating_sub(2), 1, theme.divider);

        let mut ly = py + hdr_h as i32 + (7.0 * s).round() as i32;
        for line in &lines {
            draw_text_clipped(buffer, buf_w, buf_h, px + pad, ly, px + pw as i32 - pad, line, body_sz, theme.text_primary, false);
            ly += line_h;
        }

        let ic_sz = (11.0 * s).round() as u32;
        for &(bx, by, bw, bh, hit) in &hits {
            let hov = self.hovered_widget == Some(UiHit::NotePopover(hit));
            match hit {
                NotePopoverHit::Close => {
                    if hov {
                        fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, 4.0, theme.hover_bg);
                    }
                    draw_icon(buffer, buf_w, buf_h, UiIcon::Close, bx + ((bw.saturating_sub(ic_sz)) / 2) as i32, by + ((bh.saturating_sub(ic_sz)) / 2) as i32, ic_sz, theme.text_secondary);
                }
                NotePopoverHit::Edit => {
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, 4.0, if hov { theme.accent_hover } else { theme.accent });
                    let tw = measure_text("Edit", sub_sz, true);
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round() as i32, by, bh, bx + bw as i32, "Edit", sub_sz, theme.text_on_accent, true);
                }
                NotePopoverHit::Delete => {
                    fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, 4.0, if hov { theme.danger } else { theme.danger_soft_bg });
                    let fg = if hov { theme.text_on_accent } else { theme.danger };
                    let tw = measure_text("Delete", sub_sz, true);
                    draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round() as i32, by, bh, bx + bw as i32, "Delete", sub_sz, fg, true);
                }
                NotePopoverHit::InsidePopover => {}
            }
        }
    }

    pub(super) fn hit_test_note_popover(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<NotePopoverHit> {
        let (_, x, y, w, h, _, hits) = self.note_popover_layout(buf_w, buf_h, sf)?;
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        for (bx, by, bw, bh, hit) in hits {
            if ix >= bx && ix < bx + bw as i32 && iy >= by && iy < by + bh as i32 {
                return Some(hit);
            }
        }
        if ix >= x && ix < x + w as i32 && iy >= y && iy < y + h as i32 {
            return Some(NotePopoverHit::InsidePopover);
        }
        None
    }

    pub(super) fn handle_note_popover_click(&mut self, hit: NotePopoverHit) {
        let Some(note) = self.active_note_popover.clone() else {
            return;
        };
        match hit {
            NotePopoverHit::Close => {
                self.active_note_popover = None;
                self.request_redraw();
            }
            NotePopoverHit::Edit => {
                self.active_note_popover = None;
                self.input_mode = InputMode::EditAnnotationText {
                    page_idx: note.page_idx,
                    annot_idx: note.annot_idx,
                    label: "Sticky Note",
                    text_input: note.text,
                };
                self.update_title_bar();
                self.request_redraw();
            }
            NotePopoverHit::Delete => {
                self.active_note_popover = None;
                self.push_undo_snapshot();
                if let Some(doc) = self.doc.as_mut() {
                    match PdfEngine::delete_annotation_at(doc, note.page_idx, note.annot_idx) {
                        Ok(()) => {
                            self.invalidate_page_caches();
                            self.thumbnail_cache.remove(&note.page_idx);
                            self.status_notice = Some(format!(
                                "Deleted Sticky Note on Page {} (Ctrl+Z to undo)",
                                note.page_idx + 1
                            ));
                        }
                        Err(e) => {
                            self.status_notice = Some(format!("Delete note failed: {e}"));
                        }
                    }
                }
                self.update_title_bar();
                self.request_redraw();
            }
            NotePopoverHit::InsidePopover => {}
        }
    }
}

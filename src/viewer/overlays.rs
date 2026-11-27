//! Friendly light/dark overlays: Welcome drop-zone + Recent Files, Floating Search pill,
//! clickable interactive dialog cards (Optimize, Password, Jump, etc.), Help (? / F1), and About.

use super::get_current_rss_mb;
use super::render::{blend_rect_rgba, fill_rect_clipped};
use super::state::*;
use super::ui_draw::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum OverlayHit {
    WelcomeOpen,
    WelcomeFavorites,
    WelcomeRecent(usize),
    WelcomeRestoreSession,
    WelcomeDismissSession,
    SearchPrev,
    SearchNext,
    SearchCaseToggle,
    SearchWholeWordToggle,
    SearchClose,
    OptimizePreset(u8),
    OptimizeColor(u8),
    OptimizeToggleClean,
    OptimizeTogglePreview,
    PageTurnPreset(u16),
    PromptSubmit,
    PromptCancel,
    InsideModal,
}

#[derive(Clone, Copy)]
struct BoxHit {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    hit: OverlayHit,
}

impl BoxHit {
    fn contains(self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w as i32 && py >= self.y && py < self.y + self.h as i32
    }
}

impl<'a> PdfViewerApp<'a> {
    /// Computes Welcome card geometry and clickable regions when `self.doc.is_none()`.
    fn layout_welcome(&self, buf_w: u32, buf_h: u32, top_bars_h: u32, status_bar_h: u32, sf: f64) -> ((i32, i32, u32, u32), Vec<BoxHit>) {
        let s = sf as f32;
        let has_restore = matches!(self.input_mode, InputMode::RestoreSessionPrompt { .. });
        let recent_count = self.recent_files.len().min(5);
        let cw = ((460.0 * s).round() as u32).min(buf_w.saturating_sub(32));
        let mut ch = (210.0 * s).round() as u32;
        if has_restore {
            ch += (54.0 * s).round() as u32;
        }
        if recent_count > 0 {
            ch += (34.0 * s).round() as u32 + (recent_count as u32) * ((28.0 * s).round() as u32);
        }
        let avail_h = buf_h.saturating_sub(top_bars_h + status_bar_h);
        ch = ch.min(avail_h.saturating_sub(24).max(120));
        let cx = (buf_w.saturating_sub(cw) / 2) as i32;
        let cy = top_bars_h as i32 + ((avail_h.saturating_sub(ch)) / 2) as i32;

        let mut hits = Vec::new();
        let pad = (22.0 * s).round() as i32;
        let mut cur_y = cy + (132.0 * s).round() as i32;

        // Primary "Open PDF" + "Favorites" buttons
        let btn_w = (140.0 * s).round() as u32;
        let fav_w = (136.0 * s).round() as u32;
        let btn_gap = (10.0 * s).round() as i32;
        let total_bw = btn_w + fav_w + btn_gap as u32;
        let btn_h = (34.0 * s).round() as u32;
        let btn_x = cx + ((cw.saturating_sub(total_bw)) / 2) as i32;
        hits.push(BoxHit { x: btn_x, y: cur_y, w: btn_w, h: btn_h, hit: OverlayHit::WelcomeOpen });
        hits.push(BoxHit { x: btn_x + btn_w as i32 + btn_gap, y: cur_y, w: fav_w, h: btn_h, hit: OverlayHit::WelcomeFavorites });
        cur_y += btn_h as i32 + (18.0 * s).round() as i32;

        if has_restore {
            let bh = (38.0 * s).round() as u32;
            let bw = cw.saturating_sub((pad * 2) as u32);
            let r_w = (96.0 * s).round() as u32;
            let d_w = (68.0 * s).round() as u32;
            let rb_h = (24.0 * s).round() as u32;
            let rby = cur_y + ((bh.saturating_sub(rb_h)) / 2) as i32;
            let dx = cx + pad + bw as i32 - d_w as i32 - (8.0 * s).round() as i32;
            let rx = dx - r_w as i32 - (6.0 * s).round() as i32;
            hits.push(BoxHit { x: rx, y: rby, w: r_w, h: rb_h, hit: OverlayHit::WelcomeRestoreSession });
            hits.push(BoxHit { x: dx, y: rby, w: d_w, h: rb_h, hit: OverlayHit::WelcomeDismissSession });
            cur_y += bh as i32 + (14.0 * s).round() as i32;
        }

        if recent_count > 0 {
            cur_y += (24.0 * s).round() as i32;
            let row_h = (28.0 * s).round() as u32;
            for i in 0..recent_count {
                let ry = cur_y + (i as i32) * (row_h as i32);
                if ry + row_h as i32 <= cy + ch as i32 - (8.0 * s).round() as i32 {
                    hits.push(BoxHit {
                        x: cx + pad,
                        y: ry,
                        w: cw.saturating_sub((pad * 2) as u32),
                        h: row_h,
                        hit: OverlayHit::WelcomeRecent(i),
                    });
                }
            }
        }

        ((cx, cy, cw, ch), hits)
    }

    pub(super) fn draw_welcome_screen(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        top_bars_h: u32,
        status_bar_h: u32,
        sf: f64,
    ) {
        let theme = self.ui_theme();
        let s = sf as f32;
        let ((cx, cy, cw, ch), hits) = self.layout_welcome(buf_w, buf_h, top_bars_h, status_bar_h, sf);
        let rad = (12.0 * s).clamp(8.0, 18.0);

        draw_soft_shadow(buffer, buf_w, buf_h, cx, cy, cw, ch, rad, (12.0 * s).round() as i32, (4.0 * s).round() as i32, theme.shadow_rgb, theme.shadow_max_alpha);
        fill_rounded_rect(buffer, buf_w, buf_h, cx, cy, cw, ch, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, cx, cy, cw, ch, rad, theme.border_subtle);

        let pad = (22.0 * s).round() as i32;
        let dz_h = (164.0 * s).round() as u32;
        draw_dashed_rect(buffer, buf_w, buf_h, cx + pad, cy + (18.0 * s).round() as i32, cw.saturating_sub((pad * 2) as u32), dz_h, 5, 5, theme.border_subtle);

        // Document icon centered at top of drop zone
        let ic_bg = (42.0 * s).round() as u32;
        let ic_x = cx + ((cw.saturating_sub(ic_bg)) / 2) as i32;
        let ic_y = cy + (32.0 * s).round() as i32;
        fill_rounded_rect(buffer, buf_w, buf_h, ic_x, ic_y, ic_bg, ic_bg, (10.0 * s).max(6.0), theme.accent_soft_bg);
        let ic_sz = (24.0 * s).round() as u32;
        draw_icon(buffer, buf_w, buf_h, UiIcon::Document, ic_x + ((ic_bg - ic_sz) / 2) as i32, ic_y + ((ic_bg - ic_sz) / 2) as i32, ic_sz, theme.accent);

        let title_sz = (16.0 * s).clamp(13.0, 34.0);
        let sub_sz = (12.0 * s).clamp(10.0, 26.0);
        let title = "Drop a document here to open";
        let tw = measure_text(title, title_sz, true);
        draw_text(buffer, buf_w, buf_h, cx + ((cw as f32 - tw) * 0.5).round() as i32, cy + (84.0 * s).round() as i32, title, title_sz, theme.text_primary, true);

        let sub = "Or open a file from disk — fast, local, and lightweight";
        let sw = measure_text(sub, sub_sz, false);
        draw_text(buffer, buf_w, buf_h, cx + ((cw as f32 - sw) * 0.5).round() as i32, cy + (107.0 * s).round() as i32, sub, sub_sz, theme.text_muted, false);

        // Open Documents & Favorites buttons
        if let Some(btn) = hits.iter().find(|h| h.hit == OverlayHit::WelcomeOpen) {
            let hov = self.hovered_widget == Some(UiHit::Overlay(OverlayHit::WelcomeOpen));
            fill_rounded_rect(buffer, buf_w, buf_h, btn.x, btn.y, btn.w, btn.h, (7.0 * s).max(4.0), if hov { theme.accent_hover } else { theme.accent });
            let lbl = "Open Documents  (O)";
            let lw = measure_text(lbl, sub_sz, true);
            draw_text_vcenter(buffer, buf_w, buf_h, btn.x + ((btn.w as f32 - lw) * 0.5).round() as i32, btn.y, btn.h, btn.x + btn.w as i32, lbl, sub_sz, theme.text_on_accent, true);
        }
        if let Some(btn) = hits.iter().find(|h| h.hit == OverlayHit::WelcomeFavorites) {
            let hov = self.hovered_widget == Some(UiHit::Overlay(OverlayHit::WelcomeFavorites));
            fill_rounded_rect(buffer, buf_w, buf_h, btn.x, btn.y, btn.w, btn.h, (7.0 * s).max(4.0), if hov { theme.hover_bg } else { theme.surface_elevated });
            stroke_rounded_rect(buffer, buf_w, buf_h, btn.x, btn.y, btn.w, btn.h, (7.0 * s).max(4.0), theme.border_subtle);
            let lbl = "Favorites  (Ctrl+D)";
            let lw = measure_text(lbl, sub_sz, false);
            draw_text_vcenter(buffer, buf_w, buf_h, btn.x + ((btn.w as f32 - lw) * 0.5).round() as i32, btn.y, btn.h, btn.x + btn.w as i32, lbl, sub_sz, theme.text_primary, false);
        }

        // Optional Restore Session banner
        if let InputMode::RestoreSessionPrompt { tab_count } = self.input_mode {
            if let (Some(rb), Some(db)) = (
                hits.iter().find(|h| h.hit == OverlayHit::WelcomeRestoreSession),
                hits.iter().find(|h| h.hit == OverlayHit::WelcomeDismissSession),
            ) {
                let by = rb.y - (7.0 * s).round() as i32;
                let bh = (38.0 * s).round() as u32;
                let bw = cw.saturating_sub((pad * 2) as u32);
                fill_rounded_rect(buffer, buf_w, buf_h, cx + pad, by, bw, bh, (7.0 * s).max(4.0), theme.accent_soft_bg);
                stroke_rounded_rect(buffer, buf_w, buf_h, cx + pad, by, bw, bh, (7.0 * s).max(4.0), theme.accent);
                let msg = format!("Restore {tab_count} previous tab(s)?");
                draw_text_vcenter(buffer, buf_w, buf_h, cx + pad + (10.0 * s).round() as i32, by, bh, rb.x - 6, &msg, sub_sz, theme.accent_soft_fg, true);

                let rhov = self.hovered_widget == Some(UiHit::Overlay(OverlayHit::WelcomeRestoreSession));
                fill_rounded_rect(buffer, buf_w, buf_h, rb.x, rb.y, rb.w, rb.h, (5.0 * s).max(3.0), if rhov { theme.accent_hover } else { theme.accent });
                let rlw = measure_text("Restore", (11.0 * s).max(9.5), true);
                draw_text_vcenter(buffer, buf_w, buf_h, rb.x + ((rb.w as f32 - rlw) * 0.5).round() as i32, rb.y, rb.h, rb.x + rb.w as i32, "Restore", (11.0 * s).max(9.5), theme.text_on_accent, true);

                let dhov = self.hovered_widget == Some(UiHit::Overlay(OverlayHit::WelcomeDismissSession));
                fill_rounded_rect(buffer, buf_w, buf_h, db.x, db.y, db.w, db.h, (5.0 * s).max(3.0), if dhov { theme.hover_bg } else { theme.surface_bg });
                stroke_rounded_rect(buffer, buf_w, buf_h, db.x, db.y, db.w, db.h, (5.0 * s).max(3.0), theme.border_subtle);
                let dlw = measure_text("Dismiss", (11.0 * s).max(9.5), false);
                draw_text_vcenter(buffer, buf_w, buf_h, db.x + ((db.w as f32 - dlw) * 0.5).round() as i32, db.y, db.h, db.x + db.w as i32, "Dismiss", (11.0 * s).max(9.5), theme.text_secondary, false);
            }
        }

        // Recent Files section
        let recent_hits: Vec<BoxHit> = hits.iter().copied().filter(|h| matches!(h.hit, OverlayHit::WelcomeRecent(_))).collect();
        if let Some(first) = recent_hits.first() {
            let hdr_y = first.y - (20.0 * s).round() as i32;
            draw_text(buffer, buf_w, buf_h, cx + pad, hdr_y, "Recent Files", (11.0 * s).max(9.5), theme.text_muted, true);
            for rh in recent_hits {
                let OverlayHit::WelcomeRecent(idx) = rh.hit else { continue };
                let Some(path) = self.recent_files.get(idx) else { continue };
                let hov = self.hovered_widget == Some(UiHit::Overlay(rh.hit));
                if hov {
                    fill_rounded_rect(buffer, buf_w, buf_h, rh.x, rh.y, rh.w, rh.h, (5.0 * s).max(3.0), theme.hover_bg);
                }
                let ic = (14.0 * s).round() as u32;
                draw_icon(buffer, buf_w, buf_h, UiIcon::Document, rh.x + (6.0 * s).round() as i32, rh.y + ((rh.h - ic) / 2) as i32, ic, theme.accent);
                let fname = path.file_name().and_then(|s| s.to_str()).unwrap_or("document.pdf");
                let text_x = rh.x + (26.0 * s).round() as i32;
                let max_w = (rh.x + rh.w as i32 - text_x - 8).max(20) as f32;
                let short = truncate_ellipsis(fname, max_w, sub_sz, false);
                draw_text_vcenter(buffer, buf_w, buf_h, text_x, rh.y, rh.h, rh.x + rh.w as i32 - 6, &short, sub_sz, theme.text_primary, false);
            }
        }
    }

    pub(super) fn hit_test_welcome(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        top_bars_h: u32,
        status_bar_h: u32,
        sf: f64,
    ) -> Option<OverlayHit> {
        if self.doc.is_some() {
            return None;
        }
        let (_, hits) = self.layout_welcome(buf_w, buf_h, top_bars_h, status_bar_h, sf);
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        hits.into_iter().find(|h| h.contains(ix, iy)).map(|h| h.hit)
    }

    /// Computes Floating Search Pill geometry anchored at the top-right of the page canvas.
    fn layout_floating_search(&self, buf_w: u32, top_bars_h: u32, sf: f64) -> ((i32, i32, u32, u32), Vec<BoxHit>) {
        let s = sf as f32;
        let pw = ((446.0 * s).round() as u32).min(buf_w.saturating_sub(24));
        let ph = (38.0 * s).round() as u32;
        let px = (buf_w as i32) - pw as i32 - (16.0 * s).round() as i32;
        let py = top_bars_h as i32 + (12.0 * s).round() as i32;

        let btn_sz = (24.0 * s).round() as u32;
        let aa_w = (28.0 * s).round() as u32;
        let ww_w = (26.0 * s).round() as u32;
        let gap = (4.0 * s).round() as i32;
        let by = py + ((ph.saturating_sub(btn_sz)) / 2) as i32;

        let close_x = px + pw as i32 - btn_sz as i32 - (7.0 * s).round() as i32;
        let next_x = close_x - btn_sz as i32 - gap;
        let prev_x = next_x - btn_sz as i32 - gap;
        let ww_x = prev_x - ww_w as i32 - gap - (2.0 * s).round() as i32;
        let aa_x = ww_x - aa_w as i32 - gap;
        let hits = vec![
            BoxHit { x: aa_x, y: by, w: aa_w, h: btn_sz, hit: OverlayHit::SearchCaseToggle },
            BoxHit { x: ww_x, y: by, w: ww_w, h: btn_sz, hit: OverlayHit::SearchWholeWordToggle },
            BoxHit { x: prev_x, y: by, w: btn_sz, h: btn_sz, hit: OverlayHit::SearchPrev },
            BoxHit { x: next_x, y: by, w: btn_sz, h: btn_sz, hit: OverlayHit::SearchNext },
            BoxHit { x: close_x, y: by, w: btn_sz, h: btn_sz, hit: OverlayHit::SearchClose },
        ];
        ((px, py, pw, ph), hits)
    }

    /// Draws the Floating Search Bar (`Ctrl/Cmd+F`) or interactive dialog card for `InputMode`.
    pub(super) fn draw_active_input_overlay(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        top_bars_h: u32,
        sf: f64,
    ) {
        let theme = self.ui_theme();
        let s = sf as f32;
        match &self.input_mode {
            InputMode::Normal => {}
            InputMode::RestoreSessionPrompt { .. } if self.doc.is_none() => {
                // Rendered inline inside the Welcome card
            }
            InputMode::SearchText { query_input } => {
                let ((px, py, pw, ph), hits) = self.layout_floating_search(buf_w, top_bars_h, sf);
                let rad = (8.0 * s).clamp(5.0, 14.0);
                draw_soft_shadow(buffer, buf_w, buf_h, px, py, pw, ph, rad, (8.0 * s).round() as i32, (3.0 * s).round() as i32, theme.shadow_rgb, theme.shadow_max_alpha);
                fill_rounded_rect(buffer, buf_w, buf_h, px, py, pw, ph, rad, theme.surface_bg);
                stroke_rounded_rect(buffer, buf_w, buf_h, px, py, pw, ph, rad, theme.accent);

                let ic = (14.0 * s).round() as u32;
                draw_icon(buffer, buf_w, buf_h, UiIcon::Search, px + (10.0 * s).round() as i32, py + ((ph - ic) / 2) as i32, ic, theme.accent);

                let body_sz = (12.0 * s).clamp(10.0, 26.0);
                let small_sz = (10.5 * s).clamp(9.0, 22.0);
                let first_btn_x = hits[0].x;
                let total_matches = self.active_search_match_count();
                let count_str = if total_matches > 0 {
                    format!("{}/{}", self.current_search_match + 1, total_matches)
                } else {
                    String::new()
                };
                let count_w = if count_str.is_empty() {
                    0
                } else {
                    measure_text(&count_str, small_sz, false).ceil() as i32 + (8.0 * s).round() as i32
                };
                let input_x = px + (30.0 * s).round() as i32;
                let input_max_x = (first_btn_x - count_w - 6).max(input_x + 20);
                let display = if query_input.is_empty() {
                    "Find in the document... |".to_string()
                } else {
                    format!("{query_input}|")
                };
                let fg = if query_input.is_empty() { theme.text_muted } else { theme.text_primary };
                draw_text_vcenter(buffer, buf_w, buf_h, input_x, py, ph, input_max_x, &display, body_sz, fg, false);

                if !count_str.is_empty() {
                    draw_text_vcenter(buffer, buf_w, buf_h, input_max_x + 2, py, ph, first_btn_x - 4, &count_str, small_sz, theme.text_muted, false);
                }

                for h in hits {
                    let hov = self.hovered_widget == Some(UiHit::Overlay(h.hit));
                    match h.hit {
                        OverlayHit::SearchCaseToggle => {
                            let active = self.search_case_sensitive;
                            let bg = if active {
                                theme.accent
                            } else if hov {
                                theme.hover_bg
                            } else {
                                theme.surface_elevated
                            };
                            fill_rounded_rect(buffer, buf_w, buf_h, h.x, h.y, h.w, h.h, 4.0, bg);
                            if !active {
                                stroke_rounded_rect(buffer, buf_w, buf_h, h.x, h.y, h.w, h.h, 4.0, theme.border_subtle);
                            }
                            let fg_c = if active { theme.text_on_accent } else { theme.text_secondary };
                            let tw = measure_text("Aa", small_sz, true);
                            draw_text_vcenter(buffer, buf_w, buf_h, h.x + ((h.w as f32 - tw) * 0.5).round() as i32, h.y, h.h, h.x + h.w as i32, "Aa", small_sz, fg_c, true);
                        }
                        OverlayHit::SearchWholeWordToggle => {
                            let active = self.search_whole_word;
                            let bg = if active {
                                theme.accent
                            } else if hov {
                                theme.hover_bg
                            } else {
                                theme.surface_elevated
                            };
                            fill_rounded_rect(buffer, buf_w, buf_h, h.x, h.y, h.w, h.h, 4.0, bg);
                            if !active {
                                stroke_rounded_rect(buffer, buf_w, buf_h, h.x, h.y, h.w, h.h, 4.0, theme.border_subtle);
                            }
                            let fg_c = if active { theme.text_on_accent } else { theme.text_secondary };
                            let tw = measure_text("W", small_sz, true);
                            draw_text_vcenter(buffer, buf_w, buf_h, h.x + ((h.w as f32 - tw) * 0.5).round() as i32, h.y, h.h, h.x + h.w as i32, "W", small_sz, fg_c, true);
                        }
                        OverlayHit::SearchPrev | OverlayHit::SearchNext | OverlayHit::SearchClose => {
                            if hov {
                                fill_rounded_rect(buffer, buf_w, buf_h, h.x, h.y, h.w, h.h, 4.0, theme.hover_bg);
                            }
                            let icon = match h.hit {
                                OverlayHit::SearchPrev => UiIcon::MoveUp,
                                OverlayHit::SearchNext => UiIcon::ChevronDown,
                                _ => UiIcon::Close,
                            };
                            let b_ic = (12.0 * s).round() as u32;
                            draw_icon(buffer, buf_w, buf_h, icon, h.x + ((h.w - b_ic) / 2) as i32, h.y + ((h.h - b_ic) / 2) as i32, b_ic, theme.text_secondary);
                        }
                        _ => {}
                    }
                }
            }
            InputMode::OptimizePrompt {
                preset,
                mode,
                clean_scans,
                previewing,
            } => {
                self.draw_optimize_card(buffer, buf_w, buf_h, top_bars_h, sf, *preset, *mode, *clean_scans, *previewing);
            }
            _ => {
                self.draw_generic_prompt_card(buffer, buf_w, buf_h, top_bars_h, sf);
            }
        }
    }

    fn layout_optimize_card(&self, buf_w: u32, top_bars_h: u32, sf: f64) -> ((i32, i32, u32, u32), Vec<BoxHit>) {
        let s = sf as f32;
        let cw = ((420.0 * s).round() as u32).min(buf_w.saturating_sub(24));
        let ch = (218.0 * s).round() as u32;
        let cx = (buf_w.saturating_sub(cw) / 2) as i32;
        let cy = top_bars_h as i32 + (18.0 * s).round() as i32;

        let pad = (16.0 * s).round() as i32;
        let iw = cw.saturating_sub((pad * 2) as u32);
        let gap = (6.0 * s).round() as i32;
        let row_h = (27.0 * s).round() as u32;
        let w3 = (iw.saturating_sub((gap * 2) as u32)) / 3;
        let mut hits = Vec::new();

        let y1 = cy + (54.0 * s).round() as i32;
        hits.push(BoxHit { x: cx + pad, y: y1, w: w3, h: row_h, hit: OverlayHit::OptimizePreset(0) });
        hits.push(BoxHit { x: cx + pad + w3 as i32 + gap, y: y1, w: w3, h: row_h, hit: OverlayHit::OptimizePreset(1) });
        hits.push(BoxHit { x: cx + pad + (w3 as i32 + gap) * 2, y: y1, w: iw.saturating_sub(w3 * 2 + (gap * 2) as u32), h: row_h, hit: OverlayHit::OptimizePreset(2) });

        let y2 = y1 + row_h as i32 + (22.0 * s).round() as i32;
        hits.push(BoxHit { x: cx + pad, y: y2, w: w3, h: row_h, hit: OverlayHit::OptimizeColor(0) });
        hits.push(BoxHit { x: cx + pad + w3 as i32 + gap, y: y2, w: w3, h: row_h, hit: OverlayHit::OptimizeColor(1) });
        hits.push(BoxHit { x: cx + pad + (w3 as i32 + gap) * 2, y: y2, w: iw.saturating_sub(w3 * 2 + (gap * 2) as u32), h: row_h, hit: OverlayHit::OptimizeColor(2) });

        let y3 = y2 + row_h as i32 + (12.0 * s).round() as i32;
        let w2 = (iw.saturating_sub(gap as u32)) / 2;
        hits.push(BoxHit { x: cx + pad, y: y3, w: w2, h: row_h, hit: OverlayHit::OptimizeToggleClean });
        hits.push(BoxHit { x: cx + pad + w2 as i32 + gap, y: y3, w: iw.saturating_sub(w2 + gap as u32), h: row_h, hit: OverlayHit::OptimizeTogglePreview });

        let y4 = cy + ch as i32 - (40.0 * s).round() as i32;
        let btn_h = (28.0 * s).round() as u32;
        let cancel_w = (80.0 * s).round() as u32;
        let run_w = (120.0 * s).round() as u32;
        let cancel_x = cx + cw as i32 - pad - cancel_w as i32;
        let run_x = cancel_x - run_w as i32 - gap;
        hits.push(BoxHit { x: run_x, y: y4, w: run_w, h: btn_h, hit: OverlayHit::PromptSubmit });
        hits.push(BoxHit { x: cancel_x, y: y4, w: cancel_w, h: btn_h, hit: OverlayHit::PromptCancel });

        ((cx, cy, cw, ch), hits)
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_optimize_card(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        top_bars_h: u32,
        sf: f64,
        preset: crate::optimize::OptimizePreset,
        mode: crate::optimize::ColorMode,
        clean_scans: bool,
        previewing: bool,
    ) {
        let theme = self.ui_theme();
        let s = sf as f32;
        let ((cx, cy, cw, ch), hits) = self.layout_optimize_card(buf_w, top_bars_h, sf);
        let rad = (10.0 * s).clamp(6.0, 16.0);
        draw_soft_shadow(buffer, buf_w, buf_h, cx, cy, cw, ch, rad, (10.0 * s).round() as i32, (4.0 * s).round() as i32, theme.shadow_rgb, theme.shadow_max_alpha);
        fill_rounded_rect(buffer, buf_w, buf_h, cx, cy, cw, ch, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, cx, cy, cw, ch, rad, theme.border_subtle);

        let pad = (16.0 * s).round() as i32;
        let title_sz = (14.0 * s).clamp(11.5, 30.0);
        let lbl_sz = (10.5 * s).clamp(9.0, 22.0);
        let body_sz = (11.5 * s).clamp(10.0, 24.0);

        draw_text(buffer, buf_w, buf_h, cx + pad, cy + (12.0 * s).round() as i32, "Optimize PDF", title_sz, theme.text_primary, true);
        fill_rect_clipped(buffer, buf_w, buf_h, cx + 1, cy + (34.0 * s).round() as i32, cw.saturating_sub(2), 1, theme.divider);

        let draw_pill = |buf: &mut [u32], b: BoxHit, txt: &str, active: bool| {
            let hov = self.hovered_widget == Some(UiHit::Overlay(b.hit));
            let bg = if active { theme.accent_soft_bg } else if hov { theme.hover_bg } else { theme.surface_elevated };
            let fg = if active { theme.accent_soft_fg } else { theme.text_primary };
            let brd = if active { theme.accent } else { theme.border_subtle };
            fill_rounded_rect(buf, buf_w, buf_h, b.x, b.y, b.w, b.h, (5.0 * s).max(3.0), bg);
            stroke_rounded_rect(buf, buf_w, buf_h, b.x, b.y, b.w, b.h, (5.0 * s).max(3.0), brd);
            let tw = measure_text(txt, body_sz, active);
            draw_text_vcenter(buf, buf_w, buf_h, b.x + ((b.w as f32 - tw) * 0.5).round() as i32, b.y, b.h, b.x + b.w as i32 - 2, txt, body_sz, fg, active);
        };

        draw_text(buffer, buf_w, buf_h, cx + pad, hits[0].y - (15.0 * s).round() as i32, "Quality Preset (1 / 2 / 3)", lbl_sz, theme.text_muted, true);
        draw_pill(buffer, hits[0], "Smaller 150dpi", preset == crate::optimize::OptimizePreset::Smaller);
        draw_pill(buffer, hits[1], "Balanced 200dpi", preset == crate::optimize::OptimizePreset::Balanced);
        draw_pill(buffer, hits[2], "Print 300dpi", preset == crate::optimize::OptimizePreset::Print);

        let color_lbl = if mode == crate::optimize::ColorMode::BlackWhite {
            "Color Mode (B/W makes signatures jagged)"
        } else {
            "Color Mode (C / G / B)"
        };
        draw_text(buffer, buf_w, buf_h, cx + pad, hits[3].y - (15.0 * s).round() as i32, color_lbl, lbl_sz, if mode == crate::optimize::ColorMode::BlackWhite { theme.warning } else { theme.text_muted }, true);
        draw_pill(buffer, hits[3], "Color", mode == crate::optimize::ColorMode::Color);
        draw_pill(buffer, hits[4], "Grayscale", mode == crate::optimize::ColorMode::Gray);
        draw_pill(buffer, hits[5], "1-Bit B/W", mode == crate::optimize::ColorMode::BlackWhite);

        draw_pill(buffer, hits[6], if clean_scans { "✓ Clean Scans (S)" } else { "Clean Scans (S)" }, clean_scans);
        draw_pill(buffer, hits[7], if previewing { "✓ Preview Page (P)" } else { "Preview Page (P)" }, previewing);

        let run_b = hits[8];
        let run_hov = self.hovered_widget == Some(UiHit::Overlay(OverlayHit::PromptSubmit));
        fill_rounded_rect(buffer, buf_w, buf_h, run_b.x, run_b.y, run_b.w, run_b.h, (6.0 * s).max(4.0), if run_hov { theme.accent_hover } else { theme.accent });
        let rtw = measure_text("Optimize (Enter)", body_sz, true);
        draw_text_vcenter(buffer, buf_w, buf_h, run_b.x + ((run_b.w as f32 - rtw) * 0.5).round() as i32, run_b.y, run_b.h, run_b.x + run_b.w as i32, "Optimize (Enter)", body_sz, theme.text_on_accent, true);

        draw_pill(buffer, hits[9], "Cancel", false);
    }

    fn layout_generic_prompt_card(&self, buf_w: u32, top_bars_h: u32, sf: f64) -> ((i32, i32, u32, u32), Vec<BoxHit>) {
        let s = sf as f32;
        let is_speed = matches!(self.input_mode, InputMode::PageTurnSpeed { .. });
        let cw = ((if is_speed { 510.0 } else { 480.0 } * s).round() as u32).min(buf_w.saturating_sub(24));
        let ch = ((if is_speed { 164.0 } else { 128.0 }) * s).round() as u32;
        let cx = (buf_w.saturating_sub(cw) / 2) as i32;
        let cy = top_bars_h as i32 + (18.0 * s).round() as i32;
        let pad = (16.0 * s).round() as i32;
        let btn_h = (28.0 * s).round() as u32;
        let by = cy + ch as i32 - (38.0 * s).round() as i32;
        let cancel_w = (76.0 * s).round() as u32;
        let ok_w = (96.0 * s).round() as u32;
        let cancel_x = cx + cw as i32 - pad - cancel_w as i32;
        let ok_x = cancel_x - ok_w as i32 - (6.0 * s).round() as i32;
        let mut hits = vec![
            BoxHit { x: ok_x, y: by, w: ok_w, h: btn_h, hit: OverlayHit::PromptSubmit },
            BoxHit { x: cancel_x, y: by, w: cancel_w, h: btn_h, hit: OverlayHit::PromptCancel },
        ];
        if is_speed {
            let presets: [u16; 6] = [0, 220, 450, 700, 1000, 1400];
            let py = cy + (88.0 * s).round() as i32;
            let ph = (24.0 * s).round() as u32;
            let gap = (6.0 * s).round() as i32;
            let total_w = cw.saturating_sub((pad * 2) as u32) as i32;
            let pw = ((total_w - gap * (presets.len() as i32 - 1)) / presets.len() as i32).max(40) as u32;
            let mut px = cx + pad;
            for ms in presets {
                hits.push(BoxHit { x: px, y: py, w: pw, h: ph, hit: OverlayHit::PageTurnPreset(ms) });
                px += pw as i32 + gap;
            }
        }
        ((cx, cy, cw, ch), hits)
    }

    fn draw_generic_prompt_card(&self, buffer: &mut [u32], buf_w: u32, buf_h: u32, top_bars_h: u32, sf: f64) {
        let theme = self.ui_theme();
        let s = sf as f32;
        let ((cx, cy, cw, ch), hits) = self.layout_generic_prompt_card(buf_w, top_bars_h, sf);
        let rad = (10.0 * s).clamp(6.0, 16.0);
        draw_soft_shadow(buffer, buf_w, buf_h, cx, cy, cw, ch, rad, (10.0 * s).round() as i32, (4.0 * s).round() as i32, theme.shadow_rgb, theme.shadow_max_alpha);
        fill_rounded_rect(buffer, buf_w, buf_h, cx, cy, cw, ch, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, cx, cy, cw, ch, rad, theme.border_subtle);

        let (title, hint, value, ok_lbl, is_confirm_only) = match &self.input_mode {
            InputMode::JumpToPage { input } => ("Jump to Page", "Logical '5', Roman 'ii', or Physical '#5'", input.clone(), "Jump", false),
            InputMode::PageTurnSpeed { input } => (
                "Magazine Page-Turn Animation Speed",
                "Enter duration in milliseconds (0 = Off, up to 3000 ms) or pick a preset:",
                input.clone(),
                "Apply",
                false,
            ),
            InputMode::PageNumbers { input } => ("Page Numbering", "Last Roman page # (e.g. '5' or '0' for all Arabic)", input.clone(), "Apply", false),
            InputMode::HeaderFooterRange { range_input } => ("Header / Footer (1 of 3)", "Page range (e.g. '1-5' or 'all')", range_input.clone(), "Next", false),
            InputMode::HeaderFooterPosition { pos_input, .. } => ("Header / Footer (2 of 3)", "Position: HL, HC, HR, FL, FC, or FR", pos_input.clone(), "Next", false),
            InputMode::HeaderFooterText { text_input, .. } => ("Header / Footer (3 of 3)", "Enter text (leave empty to erase existing)", text_input.clone(), "Apply", false),
            InputMode::ClearMargins { input } => ("Clear Margins", "'1' Footers/Page#, '2' Headers, '3' Both (optional range)", input.clone(), "Clear", false),
            InputMode::Watermark { input } => ("Add Watermark", "Type watermark text", input.clone(), "Apply", false),
            InputMode::SplitRange { input } => ("Split / Extract Pages", "Page range (e.g. '1-3' or '2,4,6')", input.clone(), "Extract", false),
            InputMode::AddBookmark { title_input } => ("Add Bookmark", "Bookmark title for current page", title_input.clone(), "Add", false),
            InputMode::EditBookmark { title_input, .. } => ("Edit Bookmark", "New bookmark title", title_input.clone(), "Save", false),
            InputMode::StickyNotePrompt { text_input, .. } => ("Add Sticky Note", "Type comment text to attach to this spot", text_input.clone(), "Attach", false),
            InputMode::EpubNotePrompt { text_input, .. } => ("Add Highlight Note", "Type note for the highlighted passage and press Enter", text_input.clone(), "Save Note", false),
            InputMode::FreeTextPrompt { text_input, .. } => ("Add Text Box", "Type text to place directly on this PDF page", text_input.clone(), "Place", false),
            InputMode::EditAnnotationText { label, text_input, .. } => (
                *label,
                "Edit annotation text and press Enter to save",
                text_input.clone(),
                "Update",
                false,
            ),
            InputMode::EditFormField { field_name, value_input, .. } => {
                return self.draw_prompt_card_inner(
                    buffer, buf_w, buf_h, cx, cy, cw, ch, &hits, sf,
                    &format!("Fill Form Field: {field_name}"),
                    "Type field value and press Enter to apply (Ctrl+Z to undo)",
                    value_input,
                    "Apply",
                    false,
                );
            }
            InputMode::ConfirmExternalUrl { url } => ("Open External Link?", "This will open in your default browser:", url.clone(), "Open", true),
            InputMode::PasswordPrompt { path, password_input, attempts_left, .. } => {
                let fname = path.file_name().and_then(|s| s.to_str()).unwrap_or("document.pdf");
                let masked: String = "*".repeat(password_input.chars().count());
                return self.draw_prompt_card_inner(
                    buffer, buf_w, buf_h, cx, cy, cw, ch, &hits, sf,
                    &format!("Password Required ({attempts_left} left)"),
                    fname,
                    &masked,
                    "Unlock",
                    false,
                );
            }
            InputMode::RestoreSessionPrompt { tab_count } => {
                return self.draw_prompt_card_inner(
                    buffer, buf_w, buf_h, cx, cy, cw, ch, &hits, sf,
                    "Restore Previous Session?",
                    &format!("{tab_count} saved tab(s) from your last session"),
                    "Press Enter / Y to restore, or Esc / N to start fresh",
                    "Restore",
                    true,
                );
            }
            _ => return,
        };
        self.draw_prompt_card_inner(buffer, buf_w, buf_h, cx, cy, cw, ch, &hits, sf, title, hint, &value, ok_lbl, is_confirm_only);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_prompt_card_inner(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        cx: i32,
        cy: i32,
        cw: u32,
        _ch: u32,
        hits: &[BoxHit],
        sf: f64,
        title: &str,
        hint: &str,
        value: &str,
        ok_lbl: &str,
        is_confirm_only: bool,
    ) {
        let theme = self.ui_theme();
        let s = sf as f32;
        let pad = (16.0 * s).round() as i32;
        let title_sz = (13.5 * s).clamp(11.0, 28.0);
        let hint_sz = (11.0 * s).clamp(9.5, 22.0);
        let body_sz = (12.0 * s).clamp(10.0, 24.0);

        draw_text_clipped(buffer, buf_w, buf_h, cx + pad, cy + (12.0 * s).round() as i32, cx + cw as i32 - pad, title, title_sz, theme.text_primary, true);
        draw_text_clipped(buffer, buf_w, buf_h, cx + pad, cy + (31.0 * s).round() as i32, cx + cw as i32 - pad, hint, hint_sz, theme.text_muted, false);

        let inp_y = cy + (52.0 * s).round() as i32;
        let inp_h = (28.0 * s).round() as u32;
        let inp_w = cw.saturating_sub((pad * 2) as u32);
        if is_confirm_only {
            fill_rounded_rect(buffer, buf_w, buf_h, cx + pad, inp_y, inp_w, inp_h, (6.0 * s).max(4.0), theme.surface_elevated);
            stroke_rounded_rect(buffer, buf_w, buf_h, cx + pad, inp_y, inp_w, inp_h, (6.0 * s).max(4.0), theme.border_subtle);
            let clipped = truncate_ellipsis(value, (inp_w as f32 - 16.0 * s).max(40.0), hint_sz, false);
            draw_text_vcenter(buffer, buf_w, buf_h, cx + pad + (8.0 * s).round() as i32, inp_y, inp_h, cx + pad + inp_w as i32 - 6, &clipped, hint_sz, theme.text_secondary, false);
        } else {
            fill_rounded_rect(buffer, buf_w, buf_h, cx + pad, inp_y, inp_w, inp_h, (6.0 * s).max(4.0), theme.input_bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, cx + pad, inp_y, inp_w, inp_h, (6.0 * s).max(4.0), theme.input_focus_border);
            let txt = format!("{value}|");
            draw_text_vcenter(buffer, buf_w, buf_h, cx + pad + (8.0 * s).round() as i32, inp_y, inp_h, cx + pad + inp_w as i32 - 6, &txt, body_sz, theme.text_primary, false);
        }

        // Optional preset pills (used by Magazine Page-Turn Speed)
        let pill_sz = (10.5 * s).clamp(9.0, 18.0);
        for hb in hits.iter().skip(2) {
            if let OverlayHit::PageTurnPreset(ms) = hb.hit {
                let active = self.page_turn_anim_ms == ms as u64;
                let hov = self.hovered_widget == Some(UiHit::Overlay(hb.hit));
                let bg = if active {
                    theme.accent_soft_bg
                } else if hov {
                    theme.hover_bg
                } else {
                    theme.surface_elevated
                };
                let bd = if active { theme.accent } else { theme.border_subtle };
                let fg = if active { theme.accent_soft_fg } else { theme.text_primary };
                fill_rounded_rect(buffer, buf_w, buf_h, hb.x, hb.y, hb.w, hb.h, (5.0 * s).max(3.0), bg);
                stroke_rounded_rect(buffer, buf_w, buf_h, hb.x, hb.y, hb.w, hb.h, (5.0 * s).max(3.0), bd);
                let lbl = if ms == 0 { "Off".to_string() } else { format!("{ms}ms") };
                let lw = measure_text(&lbl, pill_sz, active);
                draw_text_vcenter(buffer, buf_w, buf_h, hb.x + ((hb.w as f32 - lw) * 0.5).round() as i32, hb.y, hb.h, hb.x + hb.w as i32 - 2, &lbl, pill_sz, fg, active);
            }
        }

        let ok_b = hits[0];
        let ok_hov = self.hovered_widget == Some(UiHit::Overlay(OverlayHit::PromptSubmit));
        fill_rounded_rect(buffer, buf_w, buf_h, ok_b.x, ok_b.y, ok_b.w, ok_b.h, (6.0 * s).max(4.0), if ok_hov { theme.accent_hover } else { theme.accent });
        let otw = measure_text(ok_lbl, body_sz, true);
        draw_text_vcenter(buffer, buf_w, buf_h, ok_b.x + ((ok_b.w as f32 - otw) * 0.5).round() as i32, ok_b.y, ok_b.h, ok_b.x + ok_b.w as i32, ok_lbl, body_sz, theme.text_on_accent, true);

        let can_b = hits[1];
        let can_hov = self.hovered_widget == Some(UiHit::Overlay(OverlayHit::PromptCancel));
        fill_rounded_rect(buffer, buf_w, buf_h, can_b.x, can_b.y, can_b.w, can_b.h, (6.0 * s).max(4.0), if can_hov { theme.hover_bg } else { theme.surface_elevated });
        stroke_rounded_rect(buffer, buf_w, buf_h, can_b.x, can_b.y, can_b.w, can_b.h, (6.0 * s).max(4.0), theme.border_subtle);
        let ctw = measure_text("Cancel", body_sz, false);
        draw_text_vcenter(buffer, buf_w, buf_h, can_b.x + ((can_b.w as f32 - ctw) * 0.5).round() as i32, can_b.y, can_b.h, can_b.x + can_b.w as i32, "Cancel", body_sz, theme.text_primary, false);
    }

    pub(super) fn hit_test_active_input_overlay(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        top_bars_h: u32,
        sf: f64,
    ) -> Option<OverlayHit> {
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        match &self.input_mode {
            InputMode::Normal => None,
            InputMode::RestoreSessionPrompt { .. } if self.doc.is_none() => None,
            InputMode::SearchText { .. } => {
                let ((px, py, pw, ph), hits) = self.layout_floating_search(buf_w, top_bars_h, sf);
                if ix < px || ix >= px + pw as i32 || iy < py || iy >= py + ph as i32 {
                    return None;
                }
                hits.into_iter().find(|h| h.contains(ix, iy)).map(|h| h.hit).or(Some(OverlayHit::InsideModal))
            }
            InputMode::OptimizePrompt { .. } => {
                let ((cx, cy, cw, ch), hits) = self.layout_optimize_card(buf_w, top_bars_h, sf);
                if ix < cx || ix >= cx + cw as i32 || iy < cy || iy >= cy + ch as i32 {
                    return None;
                }
                hits.into_iter().find(|h| h.contains(ix, iy)).map(|h| h.hit).or(Some(OverlayHit::InsideModal))
            }
            _ => {
                let ((cx, cy, cw, ch), hits) = self.layout_generic_prompt_card(buf_w, top_bars_h, sf);
                if ix < cx || ix >= cx + cw as i32 || iy < cy || iy >= cy + ch as i32 {
                    return None;
                }
                hits.into_iter().find(|h| h.contains(ix, iy)).map(|h| h.hit).or(Some(OverlayHit::InsideModal))
            }
        }
    }

    /// Clean 2-column Help Overlay (`?` / `F1`) with styled keycaps.
    pub(super) fn draw_themed_help_overlay(&self, buffer: &mut [u32], buf_w: u32, buf_h: u32, sf: f64) {
        let theme = self.ui_theme();
        let s = sf as f32;
        blend_rect_rgba(buffer, buf_w, buf_h, 0, 0, buf_w, buf_h, theme.modal_scrim);

        let cw = ((760.0 * s).round() as u32).min(buf_w.saturating_sub(28));
        let ch = ((480.0 * s).round() as u32).min(buf_h.saturating_sub(28));
        let cx = (buf_w.saturating_sub(cw) / 2) as i32;
        let cy = (buf_h.saturating_sub(ch) / 2) as i32;
        let rad = (10.0 * s).clamp(6.0, 16.0);

        draw_soft_shadow(buffer, buf_w, buf_h, cx, cy, cw, ch, rad, (12.0 * s).round() as i32, (4.0 * s).round() as i32, theme.shadow_rgb, theme.shadow_max_alpha);
        fill_rounded_rect(buffer, buf_w, buf_h, cx, cy, cw, ch, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, cx, cy, cw, ch, rad, theme.border_subtle);

        let pad = (18.0 * s).round() as i32;
        let title_sz = (15.0 * s).clamp(12.0, 32.0);
        let body_sz = (11.5 * s).clamp(10.0, 24.0);
        let key_sz = (10.5 * s).clamp(9.0, 22.0);

        draw_text(buffer, buf_w, buf_h, cx + pad, cy + (14.0 * s).round() as i32, "Keyboard & Mouse Shortcuts — MicroPDF v1.7", title_sz, theme.text_primary, true);
        let close_hint = "Esc or Click to close";
        let chw = measure_text(close_hint, key_sz, false);
        draw_text(buffer, buf_w, buf_h, cx + cw as i32 - pad - chw.ceil() as i32, cy + (17.0 * s).round() as i32, close_hint, key_sz, theme.text_muted, false);
        fill_rect_clipped(buffer, buf_w, buf_h, cx + 1, cy + (42.0 * s).round() as i32, cw.saturating_sub(2), 1, theme.divider);

        let left_items: &[(&str, &str)] = &[
            ("1 - 4 / 5", "Fit Page/Width/Text/2-Pg & Scroll (Vert/Book/Mag)"),
            ("T", "Reflowable Text View (Tables + Export .md/.tex)"),
            ("Ctrl+Wheel", "Zoom 25%–800% (60%–300% EPUB/MD/LaTeX; 0 resets)"),
            ("Tab", "Thumbnail Grid (Ctrl/Shift+Click multi-select)"),
            ("Ctrl+1..9", "Jump to Tab 1–9 (Ctrl+Tab / Ctrl+Shift+Tab)"),
            ("B / A / L", "Sidebar (Book/Outline/Bookmarks) / Add / Import .txt"),
            ("G", "Smart Jump to Page (5, iii, or #5)"),
            ("Alt+Arrows", "History Back/Fwd (or Move Line in .md/.tex Editor)"),
            ("F11 / F5", "Fullscreen / Presentation Mode"),
            ("V / Aa", "Reading Modes (V) / EPUB Aa Fonts, Margins, Justify & Curl"),
            ("Ctrl+F", "Find in the document (PDF/EPUB/MD/LaTeX; F3 / Shift+F3)"),
        ];
        let right_items: &[(&str, &str)] = &[
            ("O / S / W", "Open Documents / Save / Watermark Dialog"),
            ("Ctrl+D / ★", "Favorites Menu & Tab ★ Star / Close Tab (Ctrl+W)"),
            ("Ctrl+P", "Print Preview (or Export .md/.tex/.epub to PDF/Word)"),
            ("Ctrl+Z / Y", "Undo (Ctrl+Z) / Redo (Ctrl+Y or Ctrl+Shift+Z)"),
            ("Ctrl+B/I/D", "Editor: Bold / Italic / Dup (Shift+D) / Comment (/)"),
            ("Pop Out ↗", "Detach .md/.tex Editor Window (Dock Back restores Split)"),
            ("Drag / 2×Div", "EPUB Highlight/Note pill; Drag Split Divider (2×=50/50)"),
            ("Shift+M / X", "Combine PDFs / Split & Extract Pages"),
            ("N / H / P / Z", "Page Numbers / Header & Footer / Stamps / Optimize"),
            ("I / R / D / K", "Insert Blank / Rotate 90° / Delete / Page B&W"),
            ("Help Menu", "User Guide, UI Skin (Classic/Distinct), Compare & Ruler"),
        ];

        let col_w = (cw.saturating_sub((pad * 3) as u32)) / 2;
        let row_h = (35.0 * s).round() as i32;
        let start_y = cy + (54.0 * s).round() as i32;
        let key_w = (92.0 * s).round().min((col_w as f32 * 0.34).round()) as u32;
        let key_h = (22.0 * s).round() as u32;

        for (col_idx, list) in [left_items, right_items].iter().enumerate() {
            let col_x = cx + pad + (col_idx as i32) * (col_w as i32 + pad);
            for (i, &(k, desc)) in list.iter().enumerate() {
                let ry = start_y + (i as i32) * row_h;
                if ry + key_h as i32 > cy + ch as i32 - (12.0 * s).round() as i32 {
                    break;
                }
                fill_rounded_rect(buffer, buf_w, buf_h, col_x, ry, key_w, key_h, (5.0 * s).max(3.0), theme.surface_elevated);
                stroke_rounded_rect(buffer, buf_w, buf_h, col_x, ry, key_w, key_h, (5.0 * s).max(3.0), theme.border_subtle);
                let kw = measure_text(k, key_sz, true);
                draw_text_vcenter(buffer, buf_w, buf_h, col_x + ((key_w as f32 - kw) * 0.5).round() as i32, ry, key_h, col_x + key_w as i32 - 2, k, key_sz, theme.accent_soft_fg, true);
                draw_text_vcenter(buffer, buf_w, buf_h, col_x + key_w as i32 + (10.0 * s).round() as i32, ry, key_h, col_x + col_w as i32, desc, body_sz, theme.text_primary, false);
            }
        }
    }

    /// Clean `Help -> About MicroPDF` modal card.
    pub(super) fn draw_about_overlay(&self, buffer: &mut [u32], buf_w: u32, buf_h: u32, sf: f64) {
        let theme = self.ui_theme();
        let s = sf as f32;
        blend_rect_rgba(buffer, buf_w, buf_h, 0, 0, buf_w, buf_h, theme.modal_scrim);

        let cw = ((380.0 * s).round() as u32).min(buf_w.saturating_sub(28));
        let ch = ((240.0 * s).round() as u32).min(buf_h.saturating_sub(28));
        let cx = (buf_w.saturating_sub(cw) / 2) as i32;
        let cy = (buf_h.saturating_sub(ch) / 2) as i32;
        let rad = (10.0 * s).clamp(6.0, 16.0);

        draw_soft_shadow(buffer, buf_w, buf_h, cx, cy, cw, ch, rad, (12.0 * s).round() as i32, (4.0 * s).round() as i32, theme.shadow_rgb, theme.shadow_max_alpha);
        fill_rounded_rect(buffer, buf_w, buf_h, cx, cy, cw, ch, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, cx, cy, cw, ch, rad, theme.border_subtle);

        let ic_bg = (40.0 * s).round() as u32;
        let ic_x = cx + ((cw.saturating_sub(ic_bg)) / 2) as i32;
        let ic_y = cy + (20.0 * s).round() as i32;
        fill_rounded_rect(buffer, buf_w, buf_h, ic_x, ic_y, ic_bg, ic_bg, (10.0 * s).max(6.0), theme.accent_soft_bg);
        let ic_sz = (22.0 * s).round() as u32;
        draw_icon(buffer, buf_w, buf_h, UiIcon::Document, ic_x + ((ic_bg - ic_sz) / 2) as i32, ic_y + ((ic_bg - ic_sz) / 2) as i32, ic_sz, theme.accent);

        let title_sz = (16.0 * s).clamp(13.0, 34.0);
        let body_sz = (12.0 * s).clamp(10.0, 24.0);
        let sub_sz = (11.0 * s).clamp(9.5, 22.0);
        let center_text = |buf: &mut [u32], y: i32, txt: &str, sz: f32, col: u32, bold: bool| {
            let tw = measure_text(txt, sz, bold);
            draw_text(buf, buf_w, buf_h, cx + ((cw as f32 - tw) * 0.5).round() as i32, y, txt, sz, col, bold);
        };

        center_text(buffer, cy + (70.0 * s).round() as i32, "MicroPDF", title_sz, theme.text_primary, true);
        center_text(buffer, cy + (93.0 * s).round() as i32, "Version 1.7.0 — Portable PDF, Markdown, LaTeX & EPUB Suite", body_sz, theme.text_secondary, false);
        let ram = get_current_rss_mb().unwrap_or(0.0);
        center_text(buffer, cy + (116.0 * s).round() as i32, &format!("CPU-Rendered (Zero GPU) · Current RAM: {ram:.1} MB"), sub_sz, theme.text_muted, false);
        center_text(buffer, cy + (136.0 * s).round() as i32, "Powered by PDFium (Apache-2.0), winit, softbuffer, lopdf & ab_glyph", sub_sz, theme.text_muted, false);
        center_text(buffer, cy + (156.0 * s).round() as i32, "MIT License · Fonts: Liberation Sans (SIL OFL) · Press ? for Shortcuts", sub_sz, theme.text_muted, false);

        let btn_w = (88.0 * s).round() as u32;
        let btn_h = (28.0 * s).round() as u32;
        let bx = cx + ((cw.saturating_sub(btn_w)) / 2) as i32;
        let by = cy + ch as i32 - (44.0 * s).round() as i32;
        fill_rounded_rect(buffer, buf_w, buf_h, bx, by, btn_w, btn_h, (6.0 * s).max(4.0), theme.accent);
        let lw = measure_text("Close", body_sz, true);
        draw_text_vcenter(buffer, buf_w, buf_h, bx + ((btn_w as f32 - lw) * 0.5).round() as i32, by, btn_h, bx + btn_w as i32, "Close", body_sz, theme.text_on_accent, true);
    }

    pub(super) fn handle_overlay_click(&mut self, hit: OverlayHit) {
        match hit {
            OverlayHit::WelcomeOpen => {
                self.pending_dialog = Some(PendingDialog::Open);
            }
            OverlayHit::WelcomeFavorites => {
                self.open_favorites_modal();
            }
            OverlayHit::WelcomeRecent(idx) => {
                if let Some(path) = self.recent_files.get(idx).cloned() {
                    let fname = path.file_name().and_then(|s| s.to_str()).unwrap_or("document.pdf").to_string();
                    if let Err(e) = self.load_document(path) {
                        self.status_notice = Some(format!("Failed to open '{fname}': {e}"));
                    }
                }
            }
            OverlayHit::WelcomeRestoreSession => {
                self.input_mode = InputMode::Normal;
                self.restore_pending_session();
            }
            OverlayHit::WelcomeDismissSession => {
                self.input_mode = InputMode::Normal;
                self.pending_restore_tabs.clear();
            }
            OverlayHit::SearchPrev => {
                if let InputMode::SearchText { ref query_input } = self.input_mode {
                    if !query_input.is_empty() && query_input.trim() != self.search_query {
                        let q = query_input.clone();
                        self.execute_text_search(&q);
                        return;
                    }
                }
                self.jump_to_prev_search_match();
            }
            OverlayHit::SearchNext => {
                if let InputMode::SearchText { ref query_input } = self.input_mode {
                    if !query_input.is_empty() && query_input.trim() != self.search_query {
                        let q = query_input.clone();
                        self.execute_text_search(&q);
                        return;
                    }
                }
                self.jump_to_next_search_match();
            }
            OverlayHit::SearchCaseToggle => {
                self.search_case_sensitive = !self.search_case_sensitive;
                let active_q = match &self.input_mode {
                    InputMode::SearchText { query_input } if !query_input.trim().is_empty() => {
                        Some(query_input.clone())
                    }
                    _ if !self.search_query.is_empty() => Some(self.search_query.clone()),
                    _ => None,
                };
                if let Some(q) = active_q {
                    self.execute_text_search(&q);
                } else {
                    self.status_notice = Some(format!(
                        "Search Match Case (Aa): {}",
                        if self.search_case_sensitive { "ON" } else { "OFF" }
                    ));
                }
            }
            OverlayHit::SearchWholeWordToggle => {
                self.search_whole_word = !self.search_whole_word;
                let active_q = match &self.input_mode {
                    InputMode::SearchText { query_input } if !query_input.trim().is_empty() => {
                        Some(query_input.clone())
                    }
                    _ if !self.search_query.is_empty() => Some(self.search_query.clone()),
                    _ => None,
                };
                if let Some(q) = active_q {
                    self.execute_text_search(&q);
                } else {
                    self.status_notice = Some(format!(
                        "Search Whole Word (W): {}",
                        if self.search_whole_word { "ON" } else { "OFF" }
                    ));
                }
            }
            OverlayHit::SearchClose => {
                self.input_mode = InputMode::Normal;
                self.cancel_incremental_search();
                self.search_matches.clear();
                self.md_search_matches.clear();
                self.search_query.clear();
            }
            OverlayHit::OptimizePreset(p) => {
                let mut preview_args = None;
                if let InputMode::OptimizePrompt { preset, mode, clean_scans, previewing } = &mut self.input_mode {
                    *preset = match p {
                        0 => crate::optimize::OptimizePreset::Smaller,
                        2 => crate::optimize::OptimizePreset::Print,
                        _ => crate::optimize::OptimizePreset::Balanced,
                    };
                    if *previewing {
                        preview_args = Some((*preset, *mode, *clean_scans));
                    }
                }
                if let Some((pr, md, cs)) = preview_args {
                    self.render_optimize_preview_current_page(pr, md, cs);
                }
            }
            OverlayHit::OptimizeColor(c) => {
                let mut preview_args = None;
                if let InputMode::OptimizePrompt { preset, mode, clean_scans, previewing } = &mut self.input_mode {
                    *mode = match c {
                        1 => crate::optimize::ColorMode::Gray,
                        2 => crate::optimize::ColorMode::BlackWhite,
                        _ => crate::optimize::ColorMode::Color,
                    };
                    if *previewing {
                        preview_args = Some((*preset, *mode, *clean_scans));
                    }
                }
                if let Some((pr, md, cs)) = preview_args {
                    self.render_optimize_preview_current_page(pr, md, cs);
                }
            }
            OverlayHit::OptimizeToggleClean => {
                let mut preview_args = None;
                if let InputMode::OptimizePrompt { preset, mode, clean_scans, previewing } = &mut self.input_mode {
                    *clean_scans = !*clean_scans;
                    if *previewing {
                        preview_args = Some((*preset, *mode, *clean_scans));
                    }
                }
                if let Some((pr, md, cs)) = preview_args {
                    self.render_optimize_preview_current_page(pr, md, cs);
                }
            }
            OverlayHit::OptimizeTogglePreview => {
                let mut preview_action = None;
                if let InputMode::OptimizePrompt { preset, mode, clean_scans, previewing } = &mut self.input_mode {
                    *previewing = !*previewing;
                    preview_action = Some(if *previewing { Some((*preset, *mode, *clean_scans)) } else { None });
                }
                if let Some(act) = preview_action {
                    match act {
                        Some((pr, md, cs)) => self.render_optimize_preview_current_page(pr, md, cs),
                        None => self.invalidate_page_caches(),
                    }
                }
            }
            OverlayHit::PageTurnPreset(ms) => {
                if let InputMode::PageTurnSpeed { input } = &mut self.input_mode {
                    *input = ms.to_string();
                }
                self.set_page_turn_anim_ms(ms as u64);
                self.input_mode = InputMode::Normal;
                return;
            }
            OverlayHit::PromptSubmit => {
                self.handle_prompt_submission();
                return;
            }
            OverlayHit::PromptCancel => {
                if let InputMode::OptimizePrompt { previewing: true, .. } = self.input_mode {
                    self.invalidate_page_caches();
                }
                self.input_mode = InputMode::Normal;
                self.pending_restore_tabs.clear();
                self.status_notice = Some("Cancelled".to_string());
            }
            OverlayHit::InsideModal => {}
        }
        self.update_title_bar();
        self.request_redraw();
    }

    /// Computes layout for the `Help -> Bookmark Import Format (.txt)` modal card.
    /// Returns `(card_rect, save_sample_btn_rect, close_btn_rect)`.
    fn bookmark_format_overlay_layout(
        &self,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> ((i32, i32, u32, u32), (i32, i32, u32, u32), (i32, i32, u32, u32)) {
        let s = sf as f32;
        let cw = ((660.0 * s).round() as u32).min(buf_w.saturating_sub(28));
        let ch = ((485.0 * s).round() as u32).min(buf_h.saturating_sub(28));
        let cx = (buf_w.saturating_sub(cw) / 2) as i32;
        let cy = (buf_h.saturating_sub(ch) / 2) as i32;
        let pad = (18.0 * s).round() as i32;
        let btn_h = (30.0 * s).round() as u32;
        let by = cy + ch as i32 - btn_h as i32 - (14.0 * s).round() as i32;
        let close_w = (88.0 * s).round() as u32;
        let sample_w = (168.0 * s).round() as u32;
        let close_x = cx + cw as i32 - pad - close_w as i32;
        let sample_x = close_x - sample_w as i32 - (10.0 * s).round() as i32;
        (
            (cx, cy, cw, ch),
            (sample_x, by, sample_w, btn_h),
            (close_x, by, close_w, btn_h),
        )
    }

    /// Clean `Help -> Bookmark Import Format (.txt)` documentation card with live example
    /// and a one-click `Save Sample .txt...` button.
    pub(super) fn draw_bookmark_format_overlay(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) {
        let theme = self.ui_theme();
        let s = sf as f32;
        blend_rect_rgba(buffer, buf_w, buf_h, 0, 0, buf_w, buf_h, theme.modal_scrim);

        let ((cx, cy, cw, ch), (sx, sy, sw, sh), (kx, ky, kw, kh)) =
            self.bookmark_format_overlay_layout(buf_w, buf_h, sf);
        let rad = (10.0 * s).clamp(6.0, 16.0);
        let pad = (18.0 * s).round() as i32;
        let title_sz = (15.0 * s).clamp(12.0, 32.0);
        let head_sz = (11.5 * s).clamp(10.0, 22.0);
        let body_sz = (11.5 * s).clamp(10.0, 22.0);
        let code_sz = (11.0 * s).clamp(9.5, 21.0);
        let hint_sz = (10.5 * s).clamp(9.0, 20.0);

        draw_soft_shadow(
            buffer, buf_w, buf_h, cx, cy, cw, ch, rad,
            (12.0 * s).round() as i32, (4.0 * s).round() as i32,
            theme.shadow_rgb, theme.shadow_max_alpha,
        );
        fill_rounded_rect(buffer, buf_w, buf_h, cx, cy, cw, ch, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, cx, cy, cw, ch, rad, theme.border_subtle);

        draw_text(
            buffer, buf_w, buf_h, cx + pad, cy + (14.0 * s).round() as i32,
            "Bookmark Import Format (.txt) — File -> Import Bookmarks (L)",
            title_sz, theme.text_primary, true,
        );
        let esc_hint = "Esc to close";
        let ehw = measure_text(esc_hint, hint_sz, false);
        draw_text(
            buffer, buf_w, buf_h, cx + cw as i32 - pad - ehw.ceil() as i32, cy + (17.0 * s).round() as i32,
            esc_hint, hint_sz, theme.text_muted, false,
        );
        fill_rect_clipped(buffer, buf_w, buf_h, cx + 1, cy + (42.0 * s).round() as i32, cw.saturating_sub(2), 1, theme.divider);

        let max_x = cx + cw as i32 - pad;
        let mut y = cy + (54.0 * s).round() as i32;
        let line_h = (20.0 * s).round() as i32;

        draw_text(buffer, buf_w, buf_h, cx + pad, y, "Line Syntax (recommended — double-hyphen separated):", head_sz, theme.accent, true);
        y += line_h;

        let syn_h = (26.0 * s).round() as u32;
        fill_rounded_rect(buffer, buf_w, buf_h, cx + pad, y, cw.saturating_sub((pad * 2) as u32), syn_h, (6.0 * s).max(4.0), theme.accent_soft_bg);
        draw_text_vcenter(
            buffer, buf_w, buf_h, cx + pad + (10.0 * s).round() as i32, y, syn_h, max_x - 8,
            "Serial -- Bookmark Title -- Page", body_sz, theme.accent_soft_fg, true,
        );
        y += syn_h as i32 + (12.0 * s).round() as i32;

        let rules: &[(&str, &str)] = &[
            ("Hierarchy:", "Serial starting with a digit (1, 2, 3) = Top-Level Chapter; starting with a letter (a, b, c) = Nested Sub-Section."),
            ("Page Numbers:", "Supports Roman (i, ii, iii), logical Arabic (1, 2, 15 — auto-offset after Roman pages), or exact physical (#5)."),
            ("Comments:", "Blank lines and lines starting with // are ignored."),
            ("Titles with '--':", "Extra '--' inside the title are preserved; first token is Serial and last token is Page."),
            ("Legacy Tab Format:", "Also supports 1-based physical page + TAB + Title (e.g. 5<TAB>Chapter One)."),
        ];
        let key_w = (128.0 * s).round() as i32;
        for &(label, desc) in rules {
            draw_text_clipped(buffer, buf_w, buf_h, cx + pad, y, cx + pad + key_w, label, body_sz, theme.text_primary, true);
            draw_text_clipped(buffer, buf_w, buf_h, cx + pad + key_w + (6.0 * s).round() as i32, y, max_x, desc, hint_sz, theme.text_secondary, false);
            y += line_h;
        }

        y += (8.0 * s).round() as i32;
        draw_text(buffer, buf_w, buf_h, cx + pad, y, "Example bookmarks.txt:", head_sz, theme.text_primary, true);
        y += (18.0 * s).round() as i32;

        let code_lines = [
            "// Front matter (Roman numerals when PDF uses Roman labels)",
            "1 -- Preface -- i",
            "2 -- Table of Contents -- iii",
            "3 -- Chapter 1: Introduction -- 1",
            "a -- 1.1 Overview -- 1",
            "b -- 1.2 Architecture -- 4",
            "4 -- Chapter 2: Results -- 12",
            "5 -- Appendix (exact physical sheet #25) -- #25",
        ];
        let code_line_h = (18.0 * s).round() as i32;
        let box_h = ((code_lines.len() as i32) * code_line_h + (14.0 * s).round() as i32).max(40) as u32;
        let max_box_h = (sy - y - (12.0 * s).round() as i32).max(40) as u32;
        let box_h = box_h.min(max_box_h);
        fill_rounded_rect(buffer, buf_w, buf_h, cx + pad, y, cw.saturating_sub((pad * 2) as u32), box_h, (7.0 * s).max(4.0), theme.surface_elevated);
        stroke_rounded_rect(buffer, buf_w, buf_h, cx + pad, y, cw.saturating_sub((pad * 2) as u32), box_h, (7.0 * s).max(4.0), theme.border_subtle);

        let mut cy_code = y + (7.0 * s).round() as i32;
        for line in code_lines {
            if cy_code + code_line_h > y + box_h as i32 {
                break;
            }
            let fg = if line.starts_with("//") {
                theme.text_muted
            } else if line.starts_with('a') || line.starts_with('b') {
                theme.text_secondary
            } else {
                theme.text_primary
            };
            let indent = if line.starts_with('a') || line.starts_with('b') {
                (22.0 * s).round() as i32
            } else {
                (10.0 * s).round() as i32
            };
            draw_text_clipped(buffer, buf_w, buf_h, cx + pad + indent, cy_code, max_x - 8, line, code_sz, fg, false);
            cy_code += code_line_h;
        }

        // Bottom note on left + action buttons on right
        draw_text_clipped(
            buffer, buf_w, buf_h, cx + pad, sy + (8.0 * s).round() as i32, sx - 8,
            "Tip: Press L or use File -> Import Bookmarks .txt to load your file.",
            hint_sz, theme.text_muted, false,
        );

        let s_hov = self.hovered_widget == Some(UiHit::BookmarkFormatSaveSample);
        fill_rounded_rect(buffer, buf_w, buf_h, sx, sy, sw, sh, (6.0 * s).max(4.0), if s_hov { theme.accent_soft_bg } else { theme.surface_elevated });
        stroke_rounded_rect(buffer, buf_w, buf_h, sx, sy, sw, sh, (6.0 * s).max(4.0), if s_hov { theme.accent } else { theme.border_subtle });
        let slbl = "Save Sample .txt...";
        let slw = measure_text(slbl, body_sz, true);
        draw_text_vcenter(buffer, buf_w, buf_h, sx + ((sw as f32 - slw) * 0.5).round() as i32, sy, sh, sx + sw as i32 - 2, slbl, body_sz, if s_hov { theme.accent_soft_fg } else { theme.text_primary }, true);

        let k_hov = self.hovered_widget == Some(UiHit::BookmarkFormatClose);
        fill_rounded_rect(buffer, buf_w, buf_h, kx, ky, kw, kh, (6.0 * s).max(4.0), if k_hov { theme.accent_hover } else { theme.accent });
        let klw = measure_text("Close", body_sz, true);
        draw_text_vcenter(buffer, buf_w, buf_h, kx + ((kw as f32 - klw) * 0.5).round() as i32, ky, kh, kx + kw as i32, "Close", body_sz, theme.text_on_accent, true);
    }

    /// Hit-tests `Help -> Bookmark Import Format (.txt)` overlay.
    /// Returns `Some(true)` for `Save Sample .txt...`, `Some(false)` for `Close` or outside click,
    /// `None` if clicked inside the card body (non-button area).
    pub(super) fn hit_test_bookmark_format_overlay(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<UiHit> {
        if !self.show_bookmark_format_overlay {
            return None;
        }
        let ((cx, cy, cw, ch), (sx, sy, sw, sh), (kx, ky, kw, kh)) =
            self.bookmark_format_overlay_layout(buf_w, buf_h, sf);
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix >= sx && ix < sx + sw as i32 && iy >= sy && iy < sy + sh as i32 {
            return Some(UiHit::BookmarkFormatSaveSample);
        }
        if ix >= kx && ix < kx + kw as i32 && iy >= ky && iy < ky + kh as i32 {
            return Some(UiHit::BookmarkFormatClose);
        }
        if ix < cx || ix >= cx + cw as i32 || iy < cy || iy >= cy + ch as i32 {
            return Some(UiHit::BookmarkFormatClose);
        }
        None
    }
}

/// Ready-to-edit template written by `Help -> Bookmark Import Format (.txt) -> Save Sample .txt...`.
pub(super) const BOOKMARK_SAMPLE_TXT: &str = "\
// MicroPDF Bookmark Import File
// Format: Serial -- Bookmark Title -- Page
//
// Rules:
// - Serial starting with a digit (1, 2, 3...) creates a Top-Level Chapter bookmark.
// - Serial starting with a letter (a, b, c...) creates a Nested Sub-Section bookmark.
// - Page can be Roman (i, ii, iii), logical Arabic (1, 2, 15), or exact physical (#5).
// - Lines starting with // and blank lines are ignored.
// - Legacy fallback also supported: <1-based physical page><TAB><Title>

1 -- Preface -- i
2 -- Table of Contents -- iii
3 -- Chapter 1: Introduction -- 1
a -- 1.1 Overview -- 1
b -- 1.2 Architecture -- 4
4 -- Chapter 2: Results -- 12
a -- 2.1 Summary Table -- 13
5 -- Appendix (exact physical page #25) -- #25
";


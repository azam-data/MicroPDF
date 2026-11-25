//! Centered, fanning-out Bottom Page View & Status Bar hosting Bookmarks, Thumbnail Grid,
//! Page Navigation/Jump, Zoom Controls, View Modes (`FitPage`, `FitWidth`, `FitTextWidth`,
//! `TwoPage`, `Scroll`, `TextView`), Reading Filter (`Normal` / `Dark` / `Sepia`),
//! Fullscreen (`F11`), Presentation (`F5`), and the clickable timestamped Session Notification Board.

use super::get_current_rss_mb;
use super::render::fill_rect_clipped;
use super::state::*;
use super::ui_draw::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum BottomBarAction {
    ToggleSidebar,
    ToggleGrid,
    PrevPage,
    PageJump,
    NextPage,
    ZoomOut,
    ZoomReset,
    ZoomIn,
    FitPage,
    FitWidth,
    FitTextWidth,
    TwoPage,
    Continuous,
    TextView,
    ReadingFilter,
    Fullscreen,
    Presentation,
    NotificationBoard,
}

impl BottomBarAction {
    pub fn tooltip(self) -> &'static str {
        match self {
            Self::ToggleSidebar => "Bookmarks Sidebar (B)",
            Self::ToggleGrid => "Thumbnail Grid View (Tab)",
            Self::PrevPage => "Previous Page (Left / Up)",
            Self::PageJump => "Jump to Page (G)",
            Self::NextPage => "Next Page (Right / Down)",
            Self::ZoomOut => "Zoom Out (-)",
            Self::ZoomReset => "Reset Zoom to 100% (0)",
            Self::ZoomIn => "Zoom In (+)",
            Self::FitPage => "Fit Full Page (1)",
            Self::FitWidth => "Fit Page Width (2)",
            Self::FitTextWidth => "Fit Text Width — Crop Margins (3)",
            Self::TwoPage => "Two-Page Spread (4)",
            Self::Continuous => "Scroll Flow: Single / Continuous Vertical / Book Horizontal (5)",
            Self::TextView => "Reflowable Text View (T)",
            Self::ReadingFilter => "Reading Mode Picker — E-Ink (Day) / Sepia / Sage / Dark / Night Amber (V)",
            Self::Fullscreen => "Fullscreen Mode (F11)",
            Self::Presentation => "Presentation Mode (F5)",
            Self::NotificationBoard => "Click to view Session Notification History",
        }
    }
}

#[derive(Clone)]
pub(super) struct BottomWidgetRect {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) w: u32,
    pub(super) h: u32,
    pub(super) action: BottomBarAction,
    pub(super) icon: Option<UiIcon>,
    pub(super) label: Option<String>,
    pub(super) active: bool,
}

#[derive(Clone, Copy)]
pub(super) struct BottomSepX(pub(super) i32);

pub(super) struct BottomBarLayout {
    pub(super) widgets: Vec<BottomWidgetRect>,
    pub(super) seps: Vec<BottomSepX>,
    pub(super) cluster_left: i32,
    pub(super) cluster_right: i32,
}

impl<'a> PdfViewerApp<'a> {
    pub(super) fn compute_bottom_bar_layout(
        &self,
        buf_w: u32,
        buf_h: u32,
        bar_h: u32,
        sf: f64,
    ) -> BottomBarLayout {
        let s = sf as f32;
        let top_y = buf_h.saturating_sub(bar_h) as i32;
        let btn_h = (22.0 * s).round().clamp(16.0, ((bar_h as f32) - 4.0).max(16.0)) as u32;
        let btn_y = top_y + ((bar_h.saturating_sub(btn_h)) / 2) as i32;
        let sq_w = (24.0 * s).round().max(18.0) as u32;
        let gap = (2.0 * s).round().max(1.0) as i32;
        let sep_pad = (6.0 * s).round().max(4.0) as i32;
        let font_sz = (11.0 * s).clamp(9.0, 22.0);

        let page_txt = if self.is_markdown_tab() {
            let words = self.md_layout.as_ref().map(|c| c.word_count).unwrap_or(0);
            let mins = (words / 230).max(1);
            if self.effective_scroll_mode() == ScrollMode::MagazineBook
                && self.md_editor_mode == super::text_editor::MdEditorMode::Preview
            {
                if let Some((l_p, r_opt, tot_p)) = self.md_magazine_page_info(buf_h, sf) {
                    let p_str = match r_opt {
                        Some(r_p) => format!("{l_p}–{r_p}/{tot_p}"),
                        None => format!("{l_p}/{tot_p}"),
                    };
                    if words > 0 {
                        format!("{p_str} · {words}w · ~{mins}m")
                    } else {
                        p_str
                    }
                } else {
                    "— / —".to_string()
                }
            } else {
                let max_scroll = self.md_max_scroll_y(buf_h, sf);
                let pct = if max_scroll > 1.0 {
                    ((self.scroll_y / max_scroll) * 100.0).round().clamp(0.0, 100.0) as u32
                } else {
                    100
                };
                if words > 0 {
                    format!("{pct}% · {words}w · ~{mins}m")
                } else {
                    format!("{pct}%")
                }
            }
        } else if self.doc.is_some() {
            let phys = self.current_page_index + 1;
            let total = self.total_pages();
            if self.effective_scroll_mode() == ScrollMode::MagazineBook && self.view_mode.is_page_view() {
                if self.current_page_index == 0 {
                    format!("1/{total}")
                } else {
                    let l_idx = if self.current_page_index % 2 == 0 {
                        self.current_page_index - 1
                    } else {
                        self.current_page_index
                    };
                    if l_idx + 1 < total {
                        format!("{}–{}/{total}", l_idx + 1, l_idx + 2)
                    } else {
                        format!("{}/{total}", l_idx + 1)
                    }
                }
            } else if self.roman_end > 0 || self.skip_pages > 0 {
                let logical = crate::engine::PdfEngine::format_logical_page_label_with_skip(
                    self.current_page_index as usize,
                    self.skip_pages,
                    self.roman_end,
                );
                format!("{logical} ({phys}/{total})")
            } else {
                format!("{phys}/{total}")
            }
        } else {
            "— / —".to_string()
        };
        let page_pill_w = (measure_text(&page_txt, font_sz, false).ceil() as u32 + (14.0 * s).round() as u32)
            .max((48.0 * s).round() as u32);

        let zoom_txt = format!("{}%", (self.zoom_level * 100.0).round() as i32);
        let zoom_pill_w = (measure_text(&zoom_txt, font_sz, false).ceil() as u32 + (10.0 * s).round() as u32)
            .max((42.0 * s).round() as u32);

        let scroll_mode = self.effective_scroll_mode();
        let scroll_txt = scroll_mode.short_label().to_string();
        let scroll_pill_w = (measure_text(&scroll_txt, font_sz, false).ceil() as u32
            + (24.0 * s).round() as u32)
            .max((52.0 * s).round() as u32);

        let filter_txt = self.reading_filter.label().to_string();
        let filter_pill_w = (measure_text(&filter_txt, font_sz, false).ceil() as u32
            + (26.0 * s).round() as u32)
            .max((58.0 * s).round() as u32);

        let mk_icon = |w: u32, action: BottomBarAction, icon: UiIcon, active: bool| BottomWidgetRect {
            x: 0,
            y: btn_y,
            w,
            h: btn_h,
            action,
            icon: Some(icon),
            label: None,
            active,
        };
        let mk_lbl = |w: u32, action: BottomBarAction, label: String, icon: Option<UiIcon>, active: bool| BottomWidgetRect {
            x: 0,
            y: btn_y,
            w,
            h: btn_h,
            action,
            icon,
            label: Some(label),
            active,
        };

        let layout_mode = self.effective_layout_mode();
        let on_page_view = self.view_mode.is_page_view();

        // Group 0: Bookmarks Sidebar & Thumbnail Grid (left of Page Selector)
        let g_panels = vec![
            mk_icon(sq_w, BottomBarAction::ToggleSidebar, UiIcon::Sidebar, self.show_bookmarks),
            mk_icon(sq_w, BottomBarAction::ToggleGrid, UiIcon::Grid, self.view_mode == ViewMode::ThumbnailGrid),
        ];
        // Group 1: Page Nav (< | 1/12 | >)
        let g_pages = vec![
            mk_icon(sq_w, BottomBarAction::PrevPage, UiIcon::ChevronLeft, false),
            mk_lbl(page_pill_w, BottomBarAction::PageJump, page_txt, None, matches!(self.input_mode, InputMode::JumpToPage { .. })),
            mk_icon(sq_w, BottomBarAction::NextPage, UiIcon::ChevronRight, false),
        ];
        // Group 2: Zoom (- | 100% | +)
        let g_zoom = vec![
            mk_icon(sq_w, BottomBarAction::ZoomOut, UiIcon::ZoomOut, false),
            mk_lbl(zoom_pill_w, BottomBarAction::ZoomReset, zoom_txt, None, false),
            mk_icon(sq_w, BottomBarAction::ZoomIn, UiIcon::ZoomIn, false),
        ];
        // Group 3: Layout/Fit Modes + Independent Scroll Flow + Text View
        let g_views = vec![
            mk_icon(sq_w, BottomBarAction::FitPage, UiIcon::FitPage, on_page_view && layout_mode == ViewMode::FitPage),
            mk_icon(sq_w, BottomBarAction::FitWidth, UiIcon::FitWidth, on_page_view && layout_mode == ViewMode::FitWidth),
            mk_icon(sq_w, BottomBarAction::FitTextWidth, UiIcon::FitText, on_page_view && layout_mode == ViewMode::FitTextWidth),
            mk_icon(sq_w, BottomBarAction::TwoPage, UiIcon::TwoPage, on_page_view && layout_mode == ViewMode::TwoPageSpread),
            mk_lbl(
                scroll_pill_w,
                BottomBarAction::Continuous,
                scroll_txt,
                Some(UiIcon::Continuous),
                scroll_mode != ScrollMode::SinglePage || self.show_scroll_mode_popover,
            ),
            mk_icon(sq_w, BottomBarAction::TextView, UiIcon::TextView, self.view_mode == ViewMode::TextView),
        ];
        // Group 4: Reading Filter picker pill
        let g_filter = vec![mk_lbl(
            filter_pill_w,
            BottomBarAction::ReadingFilter,
            filter_txt,
            Some(UiIcon::Filter),
            self.reading_filter != ReadingFilter::Normal || self.show_reading_mode_popover,
        )];
        // Group 5: Fullscreen & Presentation
        let g_focus = vec![
            mk_icon(sq_w, BottomBarAction::Fullscreen, UiIcon::Fullscreen, self.is_fullscreen),
            mk_icon(sq_w, BottomBarAction::Presentation, UiIcon::Presentation, self.is_presentation),
        ];

        let group_width = |g: &[BottomWidgetRect]| -> i32 {
            let mut sum = 0i32;
            for (i, w) in g.iter().enumerate() {
                sum += w.w as i32;
                if i + 1 < g.len() {
                    sum += gap;
                }
            }
            sum
        };

        // Progressively include groups as width allows (always centered!)
        let max_center_w = (buf_w as i32).saturating_sub((210.0 * s).round() as i32).max((260.0 * s).round() as i32);
        let all_groups = [g_panels, g_pages, g_zoom, g_views, g_filter, g_focus];
        let mut chosen: Vec<&[BottomWidgetRect]> = Vec::new();
        let mut total_w = 0i32;
        for g in &all_groups {
            let gw = group_width(g);
            let extra = if chosen.is_empty() { gw } else { sep_pad * 2 + gw };
            if !chosen.is_empty() && total_w + extra > max_center_w {
                break;
            }
            total_w += extra;
            chosen.push(g.as_slice());
        }

        let start_x = ((buf_w as i32 - total_w) / 2).max(8);
        let mut cur_x = start_x;
        let mut widgets = Vec::new();
        let mut seps = Vec::new();

        for (gi, g) in chosen.iter().enumerate() {
            if gi > 0 {
                cur_x += sep_pad / 2;
                seps.push(BottomSepX(cur_x));
                cur_x += sep_pad + sep_pad / 2;
            }
            for (wi, item) in g.iter().enumerate() {
                let mut cloned = item.clone();
                cloned.x = cur_x;
                cur_x += cloned.w as i32;
                if wi + 1 < g.len() {
                    cur_x += gap;
                }
                widgets.push(cloned);
            }
        }

        BottomBarLayout {
            widgets,
            seps,
            cluster_left: start_x,
            cluster_right: cur_x,
        }
    }

    /// Returns the `(x, y, w, h)` bounding box of a bottom-bar button (for floating tooltip anchoring).
    pub(super) fn bottom_bar_widget_rect(
        &self,
        action: BottomBarAction,
        buf_w: u32,
        buf_h: u32,
        bar_h: u32,
        sf: f64,
    ) -> Option<(i32, i32, u32, u32)> {
        let layout = self.compute_bottom_bar_layout(buf_w, buf_h, bar_h, sf);
        if action == BottomBarAction::NotificationBoard {
            let s = sf as f32;
            let pad_x = (8.0 * s).round() as i32;
            let top_y = buf_h.saturating_sub(bar_h) as i32;
            let left_max_x = layout.cluster_left - (10.0 * s).round() as i32;
            if left_max_x > pad_x + 40 {
                return Some((pad_x, top_y + 2, (left_max_x - pad_x) as u32, bar_h.saturating_sub(4)));
            }
            return None;
        }
        layout
            .widgets
            .into_iter()
            .find(|w| w.action == action)
            .map(|w| (w.x, w.y, w.w, w.h))
    }

    pub(super) fn draw_bottom_view_bar(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        bar_h: u32,
        sf: f64,
    ) {
        if bar_h == 0 || buf_h < bar_h {
            return;
        }
        let theme = self.ui_theme();
        let s = sf as f32;
        let top_y = buf_h.saturating_sub(bar_h) as i32;

        fill_rect_clipped(buffer, buf_w, buf_h, 0, top_y, buf_w, bar_h, theme.statusbar_bg);
        fill_rect_clipped(buffer, buf_w, buf_h, 0, top_y, buf_w, 1, theme.divider);
        if self.is_markdown_tab() {
            let max_scroll = self.md_max_scroll_y(buf_h, sf);
            let ratio = if max_scroll > 1.0 {
                (self.scroll_y / max_scroll).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let fill_w = (buf_w as f32 * ratio).round() as u32;
            if fill_w > 0 {
                fill_rect_clipped(buffer, buf_w, buf_h, 0, top_y, fill_w, 2, theme.accent);
            }
        }

        let layout = self.compute_bottom_bar_layout(buf_w, buf_h, bar_h, sf);
        let font_sz = (11.0 * s).clamp(9.0, 22.0);
        let small_sz = (10.5 * s).clamp(8.5, 20.0);
        let icon_sz = (13.0 * s).round().clamp(10.0, 26.0) as u32;
        let btn_rad = (4.5 * s).clamp(3.0, 8.0);
        let pad_x = (10.0 * s).round() as i32;

        // Vertical separators inside centered cluster
        let sep_h = (14.0 * s).round().max(10.0) as u32;
        let sep_y = top_y + ((bar_h.saturating_sub(sep_h)) / 2) as i32;
        for sep in &layout.seps {
            fill_rect_clipped(buffer, buf_w, buf_h, sep.0, sep_y, 1, sep_h, theme.divider);
        }

        // Draw centered view-control buttons
        for w in &layout.widgets {
            let is_hov = self.hovered_widget == Some(UiHit::BottomBar(w.action));
            let is_pill = matches!(w.action, BottomBarAction::PageJump);
            if w.active {
                fill_rounded_rect(buffer, buf_w, buf_h, w.x, w.y, w.w, w.h, btn_rad, theme.accent_soft_bg);
                stroke_rounded_rect(buffer, buf_w, buf_h, w.x, w.y, w.w, w.h, btn_rad, theme.accent);
            } else if is_hov {
                fill_rounded_rect(buffer, buf_w, buf_h, w.x, w.y, w.w, w.h, btn_rad, theme.hover_bg);
            } else if is_pill {
                fill_rounded_rect(buffer, buf_w, buf_h, w.x, w.y, w.w, w.h, btn_rad, theme.surface_elevated);
                stroke_rounded_rect(buffer, buf_w, buf_h, w.x, w.y, w.w, w.h, btn_rad, theme.border_subtle);
            }

            let fg = if w.active {
                theme.accent_soft_fg
            } else if is_hov {
                theme.text_primary
            } else {
                theme.text_secondary
            };

            match (w.icon, &w.label) {
                (Some(ic), Some(lbl)) => {
                    let ix = w.x + (5.0 * s).round() as i32;
                    let iy = w.y + ((w.h.saturating_sub(icon_sz)) / 2) as i32;
                    draw_icon(buffer, buf_w, buf_h, ic, ix, iy, icon_sz, fg);
                    let tx = ix + icon_sz as i32 + (4.0 * s).round() as i32;
                    draw_text_vcenter(buffer, buf_w, buf_h, tx, w.y, w.h, w.x + w.w as i32 - 3, lbl, font_sz, fg, w.active);
                }
                (Some(ic), None) => {
                    let ix = w.x + ((w.w.saturating_sub(icon_sz)) / 2) as i32;
                    let iy = w.y + ((w.h.saturating_sub(icon_sz)) / 2) as i32;
                    draw_icon(buffer, buf_w, buf_h, ic, ix, iy, icon_sz, fg);
                }
                (None, Some(lbl)) => {
                    let lw = measure_text(lbl, font_sz, w.active);
                    let tx = w.x + ((w.w as f32 - lw) * 0.5).round() as i32;
                    draw_text_vcenter(buffer, buf_w, buf_h, tx, w.y, w.h, w.x + w.w as i32 - 2, lbl, font_sz, fg, w.active);
                }
                (None, None) => {}
            }
        }

        // Far-left clickable Session Notification status area (clipped cleanly before centered cluster)
        let left_max_x = layout.cluster_left - (10.0 * s).round() as i32;
        if left_max_x > pad_x + 40 {
            let notif_hov = self.hovered_widget == Some(UiHit::BottomBar(BottomBarAction::NotificationBoard));
            let pill_x = (6.0 * s).round() as i32;
            let pill_w = (left_max_x - pill_x).max(20) as u32;
            let pill_h = bar_h.saturating_sub(6).max(16);
            let pill_y = top_y + ((bar_h.saturating_sub(pill_h)) / 2) as i32;
            if self.show_notifications {
                fill_rounded_rect(buffer, buf_w, buf_h, pill_x, pill_y, pill_w, pill_h, btn_rad, theme.accent_soft_bg);
                stroke_rounded_rect(buffer, buf_w, buf_h, pill_x, pill_y, pill_w, pill_h, btn_rad, theme.accent);
            } else if notif_hov {
                fill_rounded_rect(buffer, buf_w, buf_h, pill_x, pill_y, pill_w, pill_h, btn_rad, theme.hover_bg);
            }

            let mut text_start_x = pad_x;
            if !self.notifications.is_empty() {
                let cnt = format!("{}", self.notifications.len());
                let badge_sz = (9.5 * s).clamp(8.0, 18.0);
                let bw = (measure_text(&cnt, badge_sz, true).ceil() as u32 + (8.0 * s).round() as u32)
                    .max((16.0 * s).round() as u32);
                let bh = (15.0 * s).round().clamp(12.0, (pill_h as f32).max(12.0)) as u32;
                let by = top_y + ((bar_h.saturating_sub(bh)) / 2) as i32;
                let bg = if self.show_notifications { theme.accent } else { theme.accent_soft_bg };
                let fg = if self.show_notifications { theme.text_on_accent } else { theme.accent_soft_fg };
                fill_rounded_rect(buffer, buf_w, buf_h, pad_x, by, bw, bh, (bh as f32) * 0.5, bg);
                let tw = measure_text(&cnt, badge_sz, true);
                draw_text_vcenter(buffer, buf_w, buf_h, pad_x + ((bw as f32 - tw) * 0.5).round() as i32, by, bh, pad_x + bw as i32, &cnt, badge_sz, fg, true);
                text_start_x = pad_x + bw as i32 + (6.0 * s).round() as i32;
            }

            let (left_text, left_col, left_bold) = if let Some(ref notice) = self.status_notice {
                let col = if notice.starts_with("Error") || notice.contains("failed") || notice.contains("Invalid") {
                    theme.danger
                } else {
                    theme.accent_soft_fg
                };
                (notice.clone(), col, false)
            } else if let Some(ref job) = self.print_job {
                let (done, total) = job.job.progress();
                (format!("Printing {done}/{total} (Esc cancels)"), theme.accent_soft_fg, true)
            } else if self.doc.is_none() {
                ("Drop a PDF or press O".to_string(), theme.text_secondary, false)
            } else {
                let mut fname = self.filename_display();
                if self.has_unsaved_edits {
                    fname.push_str(" •");
                }
                (fname, theme.text_secondary, false)
            };

            let avail_w = (left_max_x - text_start_x - 4).max(16) as f32;
            let clipped = truncate_ellipsis(&left_text, avail_w, small_sz, left_bold);
            draw_text_vcenter(buffer, buf_w, buf_h, text_start_x, top_y, bar_h, left_max_x - 2, &clipped, small_sz, left_col, left_bold);
        }

        // Far-right active tool indicator + RAM (clipped cleanly after centered cluster)
        let right_min_x = layout.cluster_right + (10.0 * s).round() as i32;
        let right_avail = (buf_w as i32 - pad_x - right_min_x).max(0);
        if right_avail >= (50.0 * s).round() as i32 {
            let rss_str = get_current_rss_mb()
                .map(|mb| format!("{mb:.1} MB"))
                .unwrap_or_default();
            let tool_str = self.mouse_mode.short_label();
            let right_txt = if rss_str.is_empty() {
                tool_str.to_string()
            } else {
                format!("{tool_str}  ·  {rss_str}")
            };
            let show_swatch = self.mouse_mode.uses_annotation_color();
            let swatch_extra = if show_swatch { (14.0 * s).round() as i32 } else { 0 };
            let full_w = measure_text(&right_txt, small_sz, false).ceil() as i32 + swatch_extra;
            let text_to_draw = if full_w <= right_avail {
                right_txt
            } else if !rss_str.is_empty() && (measure_text(&rss_str, small_sz, false).ceil() as i32) <= right_avail {
                rss_str
            } else {
                truncate_ellipsis(&right_txt, right_avail as f32, small_sz, false)
            };
            let tw = measure_text(&text_to_draw, small_sz, false).ceil() as i32;
            let rx = (buf_w as i32 - pad_x - tw).max(right_min_x);
            if show_swatch && rx - swatch_extra >= right_min_x {
                let dot_r = (4.0 * s).clamp(3.0, 6.5);
                fill_circle_aa(
                    buffer,
                    buf_w,
                    buf_h,
                    (rx - (8.0 * s).round() as i32) as f32,
                    top_y as f32 + bar_h as f32 * 0.5,
                    dot_r,
                    self.annotation_color.u32_rgb(),
                );
            }
            let r_col = match self.mouse_mode {
                MouseMode::Redact => theme.danger,
                MouseMode::Select => theme.text_muted,
                _ => theme.accent_soft_fg,
            };
            draw_text_vcenter(buffer, buf_w, buf_h, rx, top_y, bar_h, buf_w as i32 - pad_x + 2, &text_to_draw, small_sz, r_col, false);
        }
    }

    pub(super) fn hit_test_bottom_bar(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        bar_h: u32,
        sf: f64,
    ) -> Option<BottomBarAction> {
        let top_y = buf_h.saturating_sub(bar_h) as i32;
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if iy < top_y || iy >= buf_h as i32 || ix < 0 || ix >= buf_w as i32 {
            return None;
        }
        let layout = self.compute_bottom_bar_layout(buf_w, buf_h, bar_h, sf);
        for w in &layout.widgets {
            if ix >= w.x && ix < w.x + w.w as i32 && iy >= w.y && iy < w.y + w.h as i32 {
                return Some(w.action);
            }
        }
        let s = sf as f32;
        let left_max_x = layout.cluster_left - (10.0 * s).round() as i32;
        if ix >= 4 && ix < left_max_x {
            return Some(BottomBarAction::NotificationBoard);
        }
        None
    }

    pub(super) fn execute_bottom_bar_action(&mut self, action: BottomBarAction) {
        self.open_menu = None;
        if action != BottomBarAction::NotificationBoard {
            self.show_notifications = false;
        }
        match action {
            BottomBarAction::ToggleSidebar => {
                self.show_bookmarks = !self.show_bookmarks;
                self.invalidate_page_caches();
            }
            BottomBarAction::ToggleGrid => {
                self.toggle_view_mode();
            }
            BottomBarAction::PrevPage => {
                if self.is_markdown_tab() {
                    let step = self.window.as_ref().map(|w| (w.inner_size().height as f32 * 0.80).max(240.0)).unwrap_or(480.0);
                    self.scroll_or_turn_page_vertical(-step);
                } else {
                    self.step_pages_backward();
                }
            }
            BottomBarAction::PageJump => {
                if self.doc.is_some() {
                    self.input_mode = InputMode::JumpToPage { input: String::new() };
                }
            }
            BottomBarAction::NextPage => {
                if self.is_markdown_tab() {
                    let step = self.window.as_ref().map(|w| (w.inner_size().height as f32 * 0.80).max(240.0)).unwrap_or(480.0);
                    self.scroll_or_turn_page_vertical(step);
                } else {
                    self.step_pages_forward();
                }
            }
            BottomBarAction::ZoomOut => {
                self.zoom_by_step(0.8);
            }
            BottomBarAction::ZoomReset => {
                self.reset_zoom();
            }
            BottomBarAction::ZoomIn => {
                self.zoom_by_step(1.25);
            }
            BottomBarAction::FitPage => self.set_view_mode(ViewMode::FitPage),
            BottomBarAction::FitWidth => self.set_view_mode(ViewMode::FitWidth),
            BottomBarAction::FitTextWidth => self.set_view_mode(ViewMode::FitTextWidth),
            BottomBarAction::TwoPage => self.set_view_mode(ViewMode::TwoPageSpread),
            BottomBarAction::Continuous => {
                self.show_scroll_mode_popover = !self.show_scroll_mode_popover;
                self.show_reading_mode_popover = false;
                self.show_notifications = false;
            }
            BottomBarAction::TextView => {
                if self.view_mode == ViewMode::TextView {
                    self.set_view_mode(self.last_page_view_mode);
                } else {
                    self.set_view_mode(ViewMode::TextView);
                }
            }
            BottomBarAction::ReadingFilter => {
                self.show_reading_mode_popover = !self.show_reading_mode_popover;
                self.show_scroll_mode_popover = false;
                self.show_notifications = false;
            }
            BottomBarAction::Fullscreen => {
                self.toggle_fullscreen();
            }
            BottomBarAction::Presentation => {
                if self.is_presentation {
                    self.exit_presentation_or_fullscreen();
                } else {
                    self.enter_presentation_mode();
                }
            }
            BottomBarAction::NotificationBoard => {
                self.show_notifications = !self.show_notifications;
                self.show_reading_mode_popover = false;
                self.show_scroll_mode_popover = false;
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    // -------------------------------------------------------------------------
    // Timestamped Session Notification Board Popover (bottom-left)
    // -------------------------------------------------------------------------

    pub(super) fn notification_board_geometry(
        &self,
        buf_w: u32,
        buf_h: u32,
        status_bar_h: u32,
        sf: f64,
    ) -> (i32, i32, u32, u32, (i32, i32, u32, u32)) {
        let s = sf as f32;
        let pw = (440.0 * s).round().min(buf_w.saturating_sub(24) as f32).max(240.0) as u32;
        let row_h = (22.0 * s).round() as u32;
        let visible_rows = self.notifications.len().clamp(1, 12) as u32;
        let header_h = (34.0 * s).round() as u32;
        let ph = (header_h + visible_rows * row_h + (14.0 * s).round() as u32)
            .min(buf_h.saturating_sub(status_bar_h + 64).max(96));
        let px = (10.0 * s).round() as i32;
        let py = (buf_h.saturating_sub(status_bar_h + ph) as i32 - (8.0 * s).round() as i32).max(40);
        let clear_w = (54.0 * s).round() as u32;
        let clear_h = (20.0 * s).round() as u32;
        let clear_x = px + pw as i32 - clear_w as i32 - (10.0 * s).round() as i32;
        let clear_y = py + ((header_h.saturating_sub(clear_h)) / 2) as i32;
        (px, py, pw, ph, (clear_x, clear_y, clear_w, clear_h))
    }

    pub(super) fn draw_notification_board_popover(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        status_bar_h: u32,
        sf: f64,
    ) {
        if !self.show_notifications {
            return;
        }
        let theme = self.ui_theme();
        let s = sf as f32;
        let (px, py, pw, ph, (cx, cy, cw, ch)) =
            self.notification_board_geometry(buf_w, buf_h, status_bar_h, sf);
        let rad = (8.0 * s).clamp(5.0, 14.0);
        let header_h = (34.0 * s).round() as u32;
        let title_sz = (12.0 * s).clamp(10.0, 22.0);
        let body_sz = (11.0 * s).clamp(9.0, 20.0);
        let ts_sz = (10.0 * s).clamp(8.5, 18.0);

        draw_soft_shadow(
            buffer, buf_w, buf_h, px, py, pw, ph, rad,
            (10.0 * s).round() as i32, (4.0 * s).round() as i32,
            theme.shadow_rgb, theme.shadow_max_alpha,
        );
        fill_rounded_rect(buffer, buf_w, buf_h, px, py, pw, ph, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, px, py, pw, ph, rad, theme.border_subtle);

        let title = format!("Session Notifications ({})", self.notifications.len());
        draw_text_vcenter(buffer, buf_w, buf_h, px + (12.0 * s).round() as i32, py, header_h, cx - 6, &title, title_sz, theme.text_primary, true);

        let clr_hov = self.hovered_widget == Some(UiHit::NotificationClear);
        fill_rounded_rect(buffer, buf_w, buf_h, cx, cy, cw, ch, (4.0 * s).max(3.0), if clr_hov { theme.hover_bg } else { theme.surface_elevated });
        stroke_rounded_rect(buffer, buf_w, buf_h, cx, cy, cw, ch, (4.0 * s).max(3.0), theme.border_subtle);
        let clw = measure_text("Clear", ts_sz, false);
        draw_text_vcenter(buffer, buf_w, buf_h, cx + ((cw as f32 - clw) * 0.5).round() as i32, cy, ch, cx + cw as i32, "Clear", ts_sz, theme.text_secondary, false);

        fill_rect_clipped(buffer, buf_w, buf_h, px + 1, py + header_h as i32, pw.saturating_sub(2), 1, theme.divider);

        let row_h = (22.0 * s).round() as i32;
        let list_y = py + header_h as i32 + (6.0 * s).round() as i32;
        let max_y = py + ph as i32 - (6.0 * s).round() as i32;
        if self.notifications.is_empty() {
            draw_text_vcenter(buffer, buf_w, buf_h, px + (12.0 * s).round() as i32, list_y, row_h as u32, px + pw as i32 - 12, "No notifications yet this session.", body_sz, theme.text_muted, false);
            return;
        }

        let ts_w = (54.0 * s).round() as i32;
        // Show newest first
        for (idx, entry) in self.notifications.iter().rev().enumerate() {
            let ry = list_y + (idx as i32) * row_h;
            if ry + row_h > max_y {
                break;
            }
            if idx % 2 == 1 {
                fill_rect_clipped(buffer, buf_w, buf_h, px + 2, ry, pw.saturating_sub(4), row_h as u32, theme.surface_elevated);
            }
            draw_text_vcenter(buffer, buf_w, buf_h, px + (12.0 * s).round() as i32, ry, row_h as u32, px + (12.0 * s).round() as i32 + ts_w, &entry.timestamp, ts_sz, theme.text_muted, false);
            let msg_x = px + (14.0 * s).round() as i32 + ts_w;
            let avail_w = (px + pw as i32 - msg_x - (10.0 * s).round() as i32).max(40) as f32;
            let clipped = truncate_ellipsis(&entry.message, avail_w, body_sz, false);
            let col = if entry.is_error { theme.danger } else { theme.text_primary };
            draw_text_vcenter(buffer, buf_w, buf_h, msg_x, ry, row_h as u32, px + pw as i32 - (8.0 * s).round() as i32, &clipped, body_sz, col, false);
        }
    }

    /// Returns `Some(true)` if clicking Clear, `Some(false)` if clicking inside the popover, `None` if outside.
    pub(super) fn hit_test_notification_board(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        status_bar_h: u32,
        sf: f64,
    ) -> Option<bool> {
        if !self.show_notifications {
            return None;
        }
        let (px, py, pw, ph, (cx, cy, cw, ch)) =
            self.notification_board_geometry(buf_w, buf_h, status_bar_h, sf);
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix < px || ix >= px + pw as i32 || iy < py || iy >= py + ph as i32 {
            return None;
        }
        let on_clear = ix >= cx && ix < cx + cw as i32 && iy >= cy && iy < cy + ch as i32;
        Some(on_clear)
    }

    // -------------------------------------------------------------------------
    // Reading Mode Picker Popover (6 evidence-based eye-care modes)
    // -------------------------------------------------------------------------

    pub(super) fn reading_mode_popover_geometry(
        &self,
        buf_w: u32,
        buf_h: u32,
        status_bar_h: u32,
        sf: f64,
    ) -> (i32, i32, u32, u32, u32, u32) {
        let s = sf as f32;
        let pw = (292.0 * s).round().min(buf_w.saturating_sub(20) as f32).max(220.0) as u32;
        let hdr_h = (28.0 * s).round() as u32;
        let row_h = (38.0 * s).round() as u32;
        let ph = hdr_h + (ReadingFilter::ALL.len() as u32) * row_h + (10.0 * s).round() as u32;
        let anchor_cx = self
            .bottom_bar_widget_rect(BottomBarAction::ReadingFilter, buf_w, buf_h, status_bar_h, sf)
            .map(|r| r.0 + (r.2 as i32) / 2)
            .unwrap_or((buf_w / 2) as i32);
        let max_px = (buf_w as i32 - pw as i32 - 8).max(8);
        let px = (anchor_cx - (pw as i32) / 2).clamp(8, max_px);
        let py = (buf_h.saturating_sub(status_bar_h + ph) as i32 - (8.0 * s).round() as i32).max(40);
        (px, py, pw, ph, hdr_h, row_h)
    }

    pub(super) fn draw_reading_mode_popover(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        status_bar_h: u32,
        sf: f64,
    ) {
        if !self.show_reading_mode_popover || self.is_fullscreen || self.is_presentation {
            return;
        }
        let theme = self.ui_theme();
        let s = sf as f32;
        let (px, py, pw, ph, hdr_h, row_h) =
            self.reading_mode_popover_geometry(buf_w, buf_h, status_bar_h, sf);
        let rad = (8.0 * s).clamp(5.0, 12.0);
        let hdr_sz = (11.5 * s).clamp(9.5, 19.0);
        let title_sz = (11.5 * s).clamp(9.5, 19.0);
        let hint_sz = (9.8 * s).clamp(8.5, 16.0);

        draw_soft_shadow(
            buffer, buf_w, buf_h, px, py, pw, ph, rad,
            (12.0 * s).round() as i32, (4.0 * s).round() as i32,
            theme.shadow_rgb, theme.shadow_max_alpha,
        );
        fill_rounded_rect(buffer, buf_w, buf_h, px, py, pw, ph, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, px, py, pw, ph, rad, theme.border_subtle);

        draw_text_vcenter(
            buffer, buf_w, buf_h,
            px + (12.0 * s).round() as i32, py, hdr_h, px + pw as i32 - 8,
            "Reading Color Mode (V to cycle)", hdr_sz, theme.text_muted, true,
        );
        fill_rect_clipped(buffer, buf_w, buf_h, px + 1, py + hdr_h as i32, pw.saturating_sub(2), 1, theme.divider);

        let sw_w = (28.0 * s).round() as u32;
        let sw_h = (22.0 * s).round() as u32;
        let sw_rad = (4.0 * s).clamp(3.0, 7.0);
        let sw_sz = (10.5 * s).clamp(8.5, 17.0);
        let row_pad = (6.0 * s).round() as i32;

        for (i, &mode) in ReadingFilter::ALL.iter().enumerate() {
            let ry = py + hdr_h as i32 + (4.0 * s).round() as i32 + (i as i32) * (row_h as i32);
            let active = self.reading_filter == mode;
            let hovered = self.hovered_widget == Some(UiHit::ReadingModeOption(mode));
            if active || hovered {
                let bg = if active { theme.accent_soft_bg } else { theme.hover_bg };
                fill_rounded_rect(buffer, buf_w, buf_h, px + row_pad, ry + 1, pw.saturating_sub((row_pad * 2) as u32), row_h.saturating_sub(2), sw_rad, bg);
                if active {
                    stroke_rounded_rect(buffer, buf_w, buf_h, px + row_pad, ry + 1, pw.saturating_sub((row_pad * 2) as u32), row_h.saturating_sub(2), sw_rad, theme.accent);
                }
            }
            let (sw_bg, sw_fg) = mode.swatch_colors();
            let sw_x = px + (12.0 * s).round() as i32;
            let sw_y = ry + ((row_h.saturating_sub(sw_h)) / 2) as i32;
            fill_rounded_rect(buffer, buf_w, buf_h, sw_x, sw_y, sw_w, sw_h, sw_rad, sw_bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, sw_x, sw_y, sw_w, sw_h, sw_rad, theme.border_subtle);
            let aa_w = measure_text("Aa", sw_sz, true);
            draw_text_vcenter(buffer, buf_w, buf_h, sw_x + ((sw_w as f32 - aa_w) * 0.5).round() as i32, sw_y, sw_h, sw_x + sw_w as i32, "Aa", sw_sz, sw_fg, true);

            let tx = sw_x + sw_w as i32 + (10.0 * s).round() as i32;
            let max_tx = px + pw as i32 - (10.0 * s).round() as i32;
            let title_fg = if active { theme.accent_soft_fg } else { theme.text_primary };
            let title_txt = truncate_ellipsis(mode.picker_title(), (max_tx - tx).max(40) as f32, title_sz, active);
            draw_text(buffer, buf_w, buf_h, tx, ry + (5.0 * s).round() as i32, &title_txt, title_sz, title_fg, active);
            let hint_txt = truncate_ellipsis(mode.picker_hint(), (max_tx - tx).max(40) as f32, hint_sz, false);
            draw_text(buffer, buf_w, buf_h, tx, ry + (20.0 * s).round() as i32, &hint_txt, hint_sz, theme.text_muted, false);
        }
    }

    /// Returns `Some(Some(filter))` if clicking a mode row, `Some(None)` if inside header/border, `None` if outside.
    pub(super) fn hit_test_reading_mode_popover(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        status_bar_h: u32,
        sf: f64,
    ) -> Option<Option<ReadingFilter>> {
        if !self.show_reading_mode_popover {
            return None;
        }
        let s = sf as f32;
        let (px, py, pw, ph, hdr_h, row_h) =
            self.reading_mode_popover_geometry(buf_w, buf_h, status_bar_h, sf);
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix < px || ix >= px + pw as i32 || iy < py || iy >= py + ph as i32 {
            return None;
        }
        let list_top = py + hdr_h as i32 + (4.0 * s).round() as i32;
        if iy >= list_top {
            let idx = ((iy - list_top) / (row_h as i32)) as usize;
            if let Some(&mode) = ReadingFilter::ALL.get(idx) {
                return Some(Some(mode));
            }
        }
        Some(None)
    }

    // -------------------------------------------------------------------------
    // Scroll Mode Option Menu Popover (Single / Vert / Book / Magazine)
    // -------------------------------------------------------------------------

    pub(super) fn scroll_mode_popover_geometry(
        &self,
        buf_w: u32,
        buf_h: u32,
        status_bar_h: u32,
        sf: f64,
    ) -> (i32, i32, u32, u32, u32, u32) {
        let s = sf as f32;
        let pw = (292.0 * s).round().min(buf_w.saturating_sub(20) as f32).max(220.0) as u32;
        let hdr_h = (28.0 * s).round() as u32;
        let row_h = (38.0 * s).round() as u32;
        let ph = hdr_h + (ScrollMode::ALL.len() as u32) * row_h + (10.0 * s).round() as u32;
        let anchor_cx = self
            .bottom_bar_widget_rect(BottomBarAction::Continuous, buf_w, buf_h, status_bar_h, sf)
            .map(|r| r.0 + (r.2 as i32) / 2)
            .unwrap_or((buf_w / 2) as i32);
        let max_px = (buf_w as i32 - pw as i32 - 8).max(8);
        let px = (anchor_cx - (pw as i32) / 2).clamp(8, max_px);
        let py = (buf_h.saturating_sub(status_bar_h + ph) as i32 - (8.0 * s).round() as i32).max(40);
        (px, py, pw, ph, hdr_h, row_h)
    }

    pub(super) fn draw_scroll_mode_popover(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        status_bar_h: u32,
        sf: f64,
    ) {
        if !self.show_scroll_mode_popover || self.is_fullscreen || self.is_presentation {
            return;
        }
        let theme = self.ui_theme();
        let s = sf as f32;
        let (px, py, pw, ph, hdr_h, row_h) =
            self.scroll_mode_popover_geometry(buf_w, buf_h, status_bar_h, sf);
        let rad = (8.0 * s).clamp(5.0, 12.0);
        let hdr_sz = (11.5 * s).clamp(9.5, 19.0);
        let title_sz = (11.5 * s).clamp(9.5, 19.0);
        let hint_sz = (9.8 * s).clamp(8.5, 16.0);

        draw_soft_shadow(
            buffer, buf_w, buf_h, px, py, pw, ph, rad,
            (12.0 * s).round() as i32, (4.0 * s).round() as i32,
            theme.shadow_rgb, theme.shadow_max_alpha,
        );
        fill_rounded_rect(buffer, buf_w, buf_h, px, py, pw, ph, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, px, py, pw, ph, rad, theme.border_subtle);

        draw_text_vcenter(
            buffer, buf_w, buf_h,
            px + (12.0 * s).round() as i32, py, hdr_h, px + pw as i32 - 8,
            "Scroll & Page Flow Mode", hdr_sz, theme.text_muted, true,
        );
        fill_rect_clipped(buffer, buf_w, buf_h, px + 1, py + hdr_h as i32, pw.saturating_sub(2), 1, theme.divider);

        let sw_w = (28.0 * s).round() as u32;
        let sw_h = (22.0 * s).round() as u32;
        let sw_rad = (4.0 * s).clamp(3.0, 7.0);
        let sw_sz = (10.5 * s).clamp(8.5, 17.0);
        let row_pad = (6.0 * s).round() as i32;
        let cur_mode = self.effective_scroll_mode();

        for (i, &mode) in ScrollMode::ALL.iter().enumerate() {
            let ry = py + hdr_h as i32 + (4.0 * s).round() as i32 + (i as i32) * (row_h as i32);
            let active = cur_mode == mode;
            let hovered = self.hovered_widget == Some(UiHit::ScrollModeOption(mode));
            if active || hovered {
                let bg = if active { theme.accent_soft_bg } else { theme.hover_bg };
                fill_rounded_rect(buffer, buf_w, buf_h, px + row_pad, ry + 1, pw.saturating_sub((row_pad * 2) as u32), row_h.saturating_sub(2), sw_rad, bg);
                if active {
                    stroke_rounded_rect(buffer, buf_w, buf_h, px + row_pad, ry + 1, pw.saturating_sub((row_pad * 2) as u32), row_h.saturating_sub(2), sw_rad, theme.accent);
                }
            }
            let sw_x = px + (12.0 * s).round() as i32;
            let sw_y = ry + ((row_h.saturating_sub(sw_h)) / 2) as i32;
            let badge_bg = if active { theme.accent } else { theme.surface_elevated };
            let badge_fg = if active { theme.text_on_accent } else { theme.text_primary };
            fill_rounded_rect(buffer, buf_w, buf_h, sw_x, sw_y, sw_w, sw_h, sw_rad, badge_bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, sw_x, sw_y, sw_w, sw_h, sw_rad, if active { theme.accent } else { theme.border_subtle });
            let lbl = mode.badge_label();
            let bw = measure_text(lbl, sw_sz, true);
            draw_text_vcenter(buffer, buf_w, buf_h, sw_x + ((sw_w as f32 - bw) * 0.5).round() as i32, sw_y, sw_h, sw_x + sw_w as i32, lbl, sw_sz, badge_fg, true);

            let tx = sw_x + sw_w as i32 + (10.0 * s).round() as i32;
            let max_tx = px + pw as i32 - (10.0 * s).round() as i32;
            let title_fg = if active { theme.accent_soft_fg } else { theme.text_primary };
            let title_txt = truncate_ellipsis(mode.picker_title(), (max_tx - tx).max(40) as f32, title_sz, active);
            draw_text(buffer, buf_w, buf_h, tx, ry + (5.0 * s).round() as i32, &title_txt, title_sz, title_fg, active);
            let hint_txt = truncate_ellipsis(mode.picker_hint(), (max_tx - tx).max(40) as f32, hint_sz, false);
            draw_text(buffer, buf_w, buf_h, tx, ry + (20.0 * s).round() as i32, &hint_txt, hint_sz, theme.text_muted, false);
        }
    }

    /// Returns `Some(Some(mode))` if clicking a scroll mode row, `Some(None)` if inside header/border, `None` if outside.
    pub(super) fn hit_test_scroll_mode_popover(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        status_bar_h: u32,
        sf: f64,
    ) -> Option<Option<ScrollMode>> {
        if !self.show_scroll_mode_popover {
            return None;
        }
        let s = sf as f32;
        let (px, py, pw, ph, hdr_h, row_h) =
            self.scroll_mode_popover_geometry(buf_w, buf_h, status_bar_h, sf);
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix < px || ix >= px + pw as i32 || iy < py || iy >= py + ph as i32 {
            return None;
        }
        let list_top = py + hdr_h as i32 + (4.0 * s).round() as i32;
        if iy >= list_top {
            let idx = ((iy - list_top) / (row_h as i32)) as usize;
            if let Some(&mode) = ScrollMode::ALL.get(idx) {
                return Some(Some(mode));
            }
        }
        Some(None)
    }
}

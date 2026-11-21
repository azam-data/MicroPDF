//! Centered, fanning-out Bottom Page View & Status Bar hosting Page Navigation/Jump,
//! Zoom Controls, View Modes (`FitPage`, `FitWidth`, `FitTextWidth`, `TwoPage`, `Scroll`, `TextView`),
//! Reading Filter (`Normal` / `Dark` / `Sepia`), Fullscreen (`F11`), and Presentation (`F5`).

use super::get_current_rss_mb;
use super::render::fill_rect_clipped;
use super::state::*;
use super::ui_draw::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum BottomBarAction {
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
}

impl BottomBarAction {
    pub fn tooltip(self) -> &'static str {
        match self {
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
            Self::Continuous => "Continuous Scroll (5)",
            Self::TextView => "Reflowable Text View (T)",
            Self::ReadingFilter => "Cycle Reading Mode: Normal / Dark / Sepia (V)",
            Self::Fullscreen => "Fullscreen Mode (F11)",
            Self::Presentation => "Presentation Mode (F5)",
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
        let btn_h = (22.0 * s).round().clamp(16.0, (bar_h as f32) - 4.0) as u32;
        let btn_y = top_y + ((bar_h.saturating_sub(btn_h)) / 2) as i32;
        let sq_w = (24.0 * s).round().max(18.0) as u32;
        let gap = (2.0 * s).round().max(1.0) as i32;
        let sep_pad = (6.0 * s).round().max(4.0) as i32;
        let font_sz = (11.0 * s).clamp(9.0, 22.0);

        let page_txt = if self.doc.is_some() {
            format!("{}/{}", self.current_page_index + 1, self.total_pages())
        } else {
            "— / —".to_string()
        };
        let page_pill_w = (measure_text(&page_txt, font_sz, false).ceil() as u32 + (14.0 * s).round() as u32)
            .max((48.0 * s).round() as u32);

        let zoom_txt = format!("{}%", (self.zoom_level * 100.0).round() as i32);
        let zoom_pill_w = (measure_text(&zoom_txt, font_sz, false).ceil() as u32 + (10.0 * s).round() as u32)
            .max((42.0 * s).round() as u32);

        let filter_txt = self.reading_filter.label().to_string();
        let filter_pill_w = (measure_text(&filter_txt, font_sz, false).ceil() as u32
            + (26.0 * s).round() as u32)
            .max((62.0 * s).round() as u32);

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

        // Build logical groups in display order (center-fanned):
        // Group 0: Page Nav (< | 1/12 | >)
        let g_pages = vec![
            mk_icon(sq_w, BottomBarAction::PrevPage, UiIcon::ChevronLeft, false),
            mk_lbl(page_pill_w, BottomBarAction::PageJump, page_txt, None, matches!(self.input_mode, InputMode::JumpToPage { .. })),
            mk_icon(sq_w, BottomBarAction::NextPage, UiIcon::ChevronRight, false),
        ];
        // Group 1: Zoom (- | 100% | +)
        let g_zoom = vec![
            mk_icon(sq_w, BottomBarAction::ZoomOut, UiIcon::ZoomOut, false),
            mk_lbl(zoom_pill_w, BottomBarAction::ZoomReset, zoom_txt, None, false),
            mk_icon(sq_w, BottomBarAction::ZoomIn, UiIcon::ZoomIn, false),
        ];
        // Group 2: View Modes (FitPage, FitWidth, FitTextWidth, 2-Page, Scroll, TextView)
        let g_views = vec![
            mk_icon(sq_w, BottomBarAction::FitPage, UiIcon::FitPage, self.view_mode == ViewMode::FitPage),
            mk_icon(sq_w, BottomBarAction::FitWidth, UiIcon::FitWidth, self.view_mode == ViewMode::FitWidth),
            mk_icon(sq_w, BottomBarAction::FitTextWidth, UiIcon::FitText, self.view_mode == ViewMode::FitTextWidth),
            mk_icon(sq_w, BottomBarAction::TwoPage, UiIcon::TwoPage, self.view_mode == ViewMode::TwoPageSpread),
            mk_icon(sq_w, BottomBarAction::Continuous, UiIcon::Continuous, self.view_mode == ViewMode::ContinuousScroll),
            mk_icon(sq_w, BottomBarAction::TextView, UiIcon::TextView, self.view_mode == ViewMode::TextView),
        ];
        // Group 3: Reading Filter pill
        let g_filter = vec![mk_lbl(
            filter_pill_w,
            BottomBarAction::ReadingFilter,
            filter_txt,
            Some(UiIcon::Filter),
            self.reading_filter != ReadingFilter::Normal,
        )];
        // Group 4: Fullscreen & Presentation
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
        let max_center_w = (buf_w as i32).saturating_sub((220.0 * s).round() as i32).max((260.0 * s).round() as i32);
        let all_groups = [g_pages, g_zoom, g_views, g_filter, g_focus];
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

        // Far-left status / hover tooltip (clipped cleanly before centered cluster)
        let left_max_x = layout.cluster_left - (10.0 * s).round() as i32;
        if left_max_x > pad_x + 40 {
            let (left_text, left_col, left_bold) = if let Some(UiHit::Toolbar(act)) = self.hovered_widget {
                (act.tooltip().to_string(), theme.accent_soft_fg, false)
            } else if let Some(UiHit::BottomBar(act)) = self.hovered_widget {
                (act.tooltip().to_string(), theme.accent_soft_fg, false)
            } else if let Some(ref notice) = self.status_notice {
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

            let avail_w = (left_max_x - pad_x).max(20) as f32;
            let clipped = truncate_ellipsis(&left_text, avail_w, small_sz, left_bold);
            draw_text_vcenter(buffer, buf_w, buf_h, pad_x, top_y, bar_h, left_max_x, &clipped, small_sz, left_col, left_bold);
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
        if iy < top_y || iy >= buf_h as i32 {
            return None;
        }
        let layout = self.compute_bottom_bar_layout(buf_w, buf_h, bar_h, sf);
        for w in &layout.widgets {
            if ix >= w.x && ix < w.x + w.w as i32 && iy >= w.y && iy < w.y + w.h as i32 {
                return Some(w.action);
            }
        }
        None
    }

    pub(super) fn execute_bottom_bar_action(&mut self, action: BottomBarAction) {
        self.open_menu = None;
        match action {
            BottomBarAction::PrevPage => {
                self.step_pages_backward();
            }
            BottomBarAction::PageJump => {
                if self.doc.is_some() {
                    self.input_mode = InputMode::JumpToPage { input: String::new() };
                }
            }
            BottomBarAction::NextPage => {
                self.step_pages_forward();
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
            BottomBarAction::Continuous => self.set_view_mode(ViewMode::ContinuousScroll),
            BottomBarAction::TextView => {
                if self.view_mode == ViewMode::TextView {
                    self.set_view_mode(self.last_page_view_mode);
                } else {
                    self.set_view_mode(ViewMode::TextView);
                }
            }
            BottomBarAction::ReadingFilter => {
                self.reading_filter = self.reading_filter.next();
                self.status_notice = Some(format!("Reading mode: {}", self.reading_filter.label()));
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
        }
        self.update_title_bar();
        self.request_redraw();
    }
}

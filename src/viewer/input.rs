use crate::engine::{Bookmark, MarginPosition, MarginZone, PdfEngine};
use pdfium_render::prelude::*;
use softbuffer::{Context as SoftContext, Surface};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};
use super::state::*;

impl<'a> PdfViewerApp<'a> {
    pub(super) fn set_view_mode(&mut self, mode: ViewMode) {
        if self.compare_mode.is_some() {
            match mode {
                ViewMode::FitPage => {
                    self.set_compare_fit_mode(super::compare_mode::CompareFitMode::FitPage);
                    return;
                }
                ViewMode::FitWidth | ViewMode::FitTextWidth => {
                    self.set_compare_fit_mode(super::compare_mode::CompareFitMode::FitWidth);
                    return;
                }
                _ => {}
            }
        }
        if self.view_mode == mode {
            return;
        }
        if mode == ViewMode::ThumbnailGrid {
            if self.view_mode.is_page_view() {
                self.last_page_view_mode = self.view_mode;
            }
            self.invalidate_page_caches();
            self.spare_pixel_buf = Vec::new();
            self.view_mode = ViewMode::ThumbnailGrid;
            self.status_notice = Some(
                "Thumbnail Grid View (Click page, Drag reorder, [BW]/[R] badges, Tab/Enter to view)"
                    .to_string(),
            );
        } else if mode == ViewMode::TextView {
            if self.view_mode.is_page_view() {
                self.last_page_view_mode = self.view_mode;
            }
            self.thumbnail_cache.clear();
            self.thumbnail_cache.shrink_to_fit();
            self.invalidate_page_caches();
            self.view_mode = ViewMode::TextView;
            self.scroll_y = 0.0;
            self.pan_x = 0.0;
            self.status_notice = Some(
                "Reflowable Text View (T) — Click A-/A+ to resize, Copy Text, or Export .md/.tex"
                    .to_string(),
            );
        } else {
            // Free thumbnail & text-view caches immediately when returning to Page View
            self.thumbnail_cache.clear();
            self.thumbnail_cache.shrink_to_fit();
            self.cached_text_view = None;
            self.grid_drag_page = None;
            self.hovered_thumb = None;
            self.view_mode = mode;
            self.last_page_view_mode = mode;
            if mode == ViewMode::ContinuousScroll {
                self.scroll_mode = ScrollMode::ContinuousVertical;
            }
            self.scroll_y = 0.0;
            self.scroll_x = 0.0;
            self.pan_x = 0.0;
            self.invalidate_page_caches();
            let flow_suffix = match self.effective_scroll_mode() {
                ScrollMode::SinglePage => "",
                ScrollMode::ContinuousVertical => " + Continuous Vertical",
                ScrollMode::ContinuousHorizontal => " + Book Horizontal Scroll",
            };
            self.status_notice = Some(match mode {
                ViewMode::FitPage => format!("View Mode: Fit Full Page (1){flow_suffix}"),
                ViewMode::FitWidth => format!("View Mode: Fit Page Width (2){flow_suffix}"),
                ViewMode::FitTextWidth => {
                    format!("View Mode: Fit Text Width (3) — Auto-trimmed margins{flow_suffix}")
                }
                ViewMode::TwoPageSpread => {
                    format!("View Mode: Two-Page Side-by-Side Spread (4){flow_suffix}")
                }
                ViewMode::ContinuousScroll => {
                    "View Mode: Continuous Vertical Scroll (5)".to_string()
                }
                ViewMode::ThumbnailGrid => "Thumbnail Grid View".to_string(),
                ViewMode::TextView => "Reflowable Text View (T)".to_string(),
            });
        }
        self.update_title_bar();
        self.request_redraw();
    }

    pub(super) fn toggle_view_mode(&mut self) {
        if self.view_mode == ViewMode::ThumbnailGrid {
            let restore = if self.last_page_view_mode.is_page_view() {
                self.last_page_view_mode
            } else {
                ViewMode::FitPage
            };
            self.set_view_mode(restore);
        } else {
            self.set_view_mode(ViewMode::ThumbnailGrid);
        }
    }

    /// Resolves which page (`current_page_index` vs `current_page_index + 1` in `TwoPageSpread`,
    /// or the hit page in `ContinuousVertical` / `ContinuousHorizontal`) and its corresponding screen `draw_rect` contains `(sx, sy)`.
    pub(super) fn resolve_target_page_and_draw_rect(&self, sx: f64, sy: f64) -> (u16, (i32, i32, u32, u32)) {
        if self.is_continuous_scroll() {
            let ix = sx.round() as i32;
            let iy = sy.round() as i32;
            for &(p_idx, rect) in &self.continuous_draw_rects {
                let (rx, ry, rw, rh) = rect;
                if ix >= rx && ix <= rx + (rw as i32) && iy >= ry && iy <= ry + (rh as i32) {
                    return (p_idx, rect);
                }
            }
            if let Some(&(p_idx, rect)) = self.continuous_draw_rects.iter().min_by_key(|&&(_, (rx, ry, rw, rh))| {
                let cx = rx + (rw as i32) / 2;
                let cy = ry + (rh as i32) / 2;
                (ix - cx).abs() + (iy - cy).abs()
            }) {
                return (p_idx, rect);
            }
        }
        if self.view_mode == ViewMode::TwoPageSpread {
            if let (Some(second_cache), Some(second_rect)) =
                (&self.cached_second_page, self.second_page_draw_rect)
            {
                let (rx, _ry, _rw, _rh) = second_rect;
                if (sx.round() as i32) >= rx - (SPREAD_GAP_PX as i32 / 2) {
                    return (second_cache.page_index, second_rect);
                }
            }
        }
        (self.current_page_index, self.page_draw_rect)
    }

    /// Converts screen drag start `(sx1, sy1)` and end `(sx2, sy2)` into `(page_idx, PdfRect)`.
    /// If `allow_single_click` is true (used for Stamp Image mode), a simple click produces a
    /// default-sized stamp box centered at `(sx1, sy1)`.
    pub(super) fn screen_drag_to_pdf_rect(
        &self,
        sx1: f64,
        sy1: f64,
        sx2: f64,
        sy2: f64,
        allow_single_click: bool,
    ) -> Option<(u16, PdfRect)> {
        let doc = self.doc.as_ref()?;
        let (page_idx, (px, py, pw, ph)) = self.resolve_target_page_and_draw_rect(sx1, sy1);
        let page = doc.pages().get(page_idx).ok()?;
        if pw == 0 || ph == 0 {
            return None;
        }

        let mut bx1 = (sx1 - px as f64).clamp(0.0, pw as f64) as i32;
        let mut by1 = (sy1 - py as f64).clamp(0.0, ph as f64) as i32;
        let mut bx2 = (sx2 - px as f64).clamp(0.0, pw as f64) as i32;
        let mut by2 = (sy2 - py as f64).clamp(0.0, ph as f64) as i32;

        if (bx2 - bx1).abs() < 4 && (by2 - by1).abs() < 4 {
            if !allow_single_click {
                return None;
            }
            // Default stamp size (~22% of page width, preserving stamp aspect ratio)
            let aspect = self
                .pending_stamp_image
                .as_ref()
                .map(|s| (s.height as f32 / (s.width.max(1) as f32)).clamp(0.15, 4.0))
                .unwrap_or(0.45);
            let default_w = ((pw as f32) * 0.22).max(40.0) as i32;
            let default_h = ((default_w as f32) * aspect).max(20.0) as i32;
            let cx = bx1;
            let cy = by1;
            bx1 = (cx - default_w / 2).clamp(0, pw as i32);
            bx2 = (cx + default_w / 2).clamp(0, pw as i32);
            by1 = (cy - default_h / 2).clamp(0, ph as i32);
            by2 = (cy + default_h / 2).clamp(0, ph as i32);
        }

        let config = PdfRenderConfig::new()
            .set_fixed_width(pw as i32)
            .set_fixed_height(ph as i32);

        let (pt1_x, pt1_y, pt2_x, pt2_y) = match (
            page.pixels_to_points(bx1, by1, &config),
            page.pixels_to_points(bx2, by2, &config),
        ) {
            (Ok((x1, y1)), Ok((x2, y2))) => (x1.value, y1.value, x2.value, y2.value),
            _ => {
                let page_w = page.width().value;
                let page_h = page.height().value;
                let u1 = (bx1 as f32) / (pw as f32);
                let v1 = (by1 as f32) / (ph as f32);
                let u2 = (bx2 as f32) / (pw as f32);
                let v2 = (by2 as f32) / (ph as f32);
                (
                    u1 * page_w,
                    (1.0 - v1) * page_h,
                    u2 * page_w,
                    (1.0 - v2) * page_h,
                )
            }
        };

        let bottom = pt1_y.min(pt2_y);
        let top = pt1_y.max(pt2_y);
        let left = pt1_x.min(pt2_x);
        let right = pt1_x.max(pt2_x);

        Some((page_idx, PdfRect::new_from_values(bottom, left, top, right)))
    }

    /// Converts screen coordinates `(sx, sy)` into `(page_idx, x_pts, y_pts)` if `(sx, sy)` lies inside a rendered page.
    pub(super) fn screen_point_to_pdf_point(&self, sx: f64, sy: f64) -> Option<(u16, PdfPoints, PdfPoints)> {
        let doc = self.doc.as_ref()?;
        let (page_idx, (px, py, pw, ph)) = self.resolve_target_page_and_draw_rect(sx, sy);
        if pw == 0 || ph == 0 {
            return None;
        }
        let ix = sx.round() as i32;
        let iy = sy.round() as i32;
        if ix < px || ix > px + (pw as i32) || iy < py || iy > py + (ph as i32) {
            return None;
        }
        let bx = (ix - px).clamp(0, pw as i32);
        let by = (iy - py).clamp(0, ph as i32);
        let page = doc.pages().get(page_idx).ok()?;
        let config = PdfRenderConfig::new()
            .set_fixed_width(pw as i32)
            .set_fixed_height(ph as i32);
        match page.pixels_to_points(bx, by, &config) {
            Ok((pt_x, pt_y)) => Some((page_idx, pt_x, pt_y)),
            Err(_) => {
                let page_w = page.width().value;
                let page_h = page.height().value;
                let u = (bx as f32) / (pw as f32);
                let v = (by as f32) / (ph as f32);
                Some((
                    page_idx,
                    PdfPoints::new(u * page_w),
                    PdfPoints::new((1.0 - v) * page_h),
                ))
            }
        }
    }

    /// Resolves any internal destination or external URI link at screen coordinates `(sx, sy)`.
    pub(super) fn resolve_link_at_screen_pos(&self, sx: f64, sy: f64) -> Option<ResolvedLinkTarget> {
        let (page_idx, pt_x, pt_y) = self.screen_point_to_pdf_point(sx, sy)?;
        let doc = self.doc.as_ref()?;
        let page = doc.pages().get(page_idx).ok()?;

        let extract_target = |link: &PdfLink<'_>| -> Option<ResolvedLinkTarget> {
            if let Some(dest) = link.destination() {
                if let Ok(target_p) = dest.page_index() {
                    return Some(ResolvedLinkTarget::InternalPage(target_p));
                }
            }
            if let Some(action) = link.action() {
                match action {
                    PdfAction::LocalDestination(local) => {
                        if let Ok(dest) = local.destination() {
                            if let Ok(target_p) = dest.page_index() {
                                return Some(ResolvedLinkTarget::InternalPage(target_p));
                            }
                        }
                    }
                    PdfAction::Uri(uri_act) => {
                        if let Ok(url) = uri_act.uri() {
                            let trimmed = url.trim().to_string();
                            if !trimmed.is_empty() {
                                return Some(ResolvedLinkTarget::ExternalUrl(trimmed));
                            }
                        }
                    }
                    _ => {}
                }
            }
            None
        };

        if let Some(link) = page.links().link_at_point(pt_x, pt_y) {
            if let Some(t) = extract_target(&link) {
                return Some(t);
            }
        }

        for link in page.links().iter() {
            if let Ok(r) = link.rect() {
                let left = r.left().value.min(r.right().value);
                let right = r.left().value.max(r.right().value);
                let bottom = r.bottom().value.min(r.top().value);
                let top = r.bottom().value.max(r.top().value);
                if pt_x.value >= left
                    && pt_x.value <= right
                    && pt_y.value >= bottom
                    && pt_y.value <= top
                {
                    if let Some(t) = extract_target(&link) {
                        return Some(t);
                    }
                }
            }
        }
        None
    }

    /// Pushes `(self.current_page_index, self.scroll_y)` onto `self.nav_back_stack`,
    /// clears `self.nav_forward_stack`, and enforces the hard 50-entry history cap.
    pub(super) fn push_nav_history(&mut self) {
        let entry = (self.current_page_index, self.scroll_y);
        if self.nav_back_stack.back() != Some(&entry) {
            while self.nav_back_stack.len() >= MAX_NAV_HISTORY {
                self.nav_back_stack.pop_front();
            }
            self.nav_back_stack.push_back(entry);
        }
        self.nav_forward_stack.clear();
    }

    /// Navigates back in the `(page, scroll)` history (`Alt+Left`).
    pub(super) fn navigate_back(&mut self) {
        let Some((prev_page, prev_scroll)) = self.nav_back_stack.pop_back() else {
            self.status_notice = Some("No previous location in navigation history".to_string());
            self.update_title_bar();
            self.request_redraw();
            return;
        };
        while self.nav_back_stack.len() + self.nav_forward_stack.len() >= MAX_NAV_HISTORY {
            if !self.nav_forward_stack.is_empty() {
                self.nav_forward_stack.remove(0);
            } else {
                break;
            }
        }
        self.nav_forward_stack
            .push((self.current_page_index, self.scroll_y));
        let total = self.total_pages();
        if total > 0 {
            self.current_page_index = prev_page.min(total - 1);
            self.scroll_y = prev_scroll;
            self.invalidate_page_caches();
            self.status_notice = Some(format!(
                "Navigated Back to Page {} (Alt+Right to go Forward)",
                self.current_page_index + 1
            ));
            self.update_title_bar();
            self.request_redraw();
        }
    }

    /// Navigates forward in the `(page, scroll)` history (`Alt+Right`).
    pub(super) fn navigate_forward(&mut self) {
        let Some((next_page, next_scroll)) = self.nav_forward_stack.pop() else {
            self.status_notice = Some("No forward location in navigation history".to_string());
            self.update_title_bar();
            self.request_redraw();
            return;
        };
        while self.nav_back_stack.len() + self.nav_forward_stack.len() >= MAX_NAV_HISTORY {
            if self.nav_back_stack.pop_front().is_none() {
                break;
            }
        }
        self.nav_back_stack
            .push_back((self.current_page_index, self.scroll_y));
        let total = self.total_pages();
        if total > 0 {
            self.current_page_index = next_page.min(total - 1);
            self.scroll_y = next_scroll;
            self.invalidate_page_caches();
            self.status_notice = Some(format!(
                "Navigated Forward to Page {}",
                self.current_page_index + 1
            ));
            self.update_title_bar();
            self.request_redraw();
        }
    }

    /// Opens an external HTTP/HTTPS/mailto URL in the OS default browser after user confirmation.
    pub(super) fn open_external_url_in_browser(url: &str) -> anyhow::Result<()> {
        let trimmed = url.trim();
        let lower = trimmed.to_ascii_lowercase();
        if !(lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("mailto:"))
            || trimmed.chars().any(|c| c.is_control())
        {
            anyhow::bail!("Refused unsafe URL scheme (only http://, https://, and mailto: allowed)");
        }

        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open").arg(trimmed).spawn()?;
        }
        #[cfg(target_os = "windows")]
        {
            std::process::Command::new("rundll32")
                .args(["url.dll,FileProtocolHandler", trimmed])
                .spawn()?;
        }
        #[cfg(target_os = "linux")]
        {
            std::process::Command::new("xdg-open").arg(trimmed).spawn()?;
        }
        Ok(())
    }

    /// Populates `self.cached_char_boxes` for `page_idx` ONLY, immediately dropping any previous page's character boxes.
    pub(super) fn ensure_page_char_boxes(&mut self, page_idx: u16) {
        if self
            .cached_char_boxes
            .as_ref()
            .map_or(false, |(cached_p, _)| *cached_p == page_idx)
        {
            return;
        }
        self.cached_char_boxes = None;
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let Ok(page) = doc.pages().get(page_idx) else {
            return;
        };
        let Ok(text_page) = page.text() else {
            return;
        };

        let mut boxes = Vec::new();
        for ch in text_page.chars().iter() {
            let Some(uc) = ch.unicode_char() else {
                continue;
            };
            if uc == '\0' {
                continue;
            }
            let rect_opt = ch
                .loose_bounds()
                .or_else(|_| ch.tight_bounds())
                .ok()
                .and_then(|b| {
                    let bottom = b.bottom().value.min(b.top().value);
                    let top = b.bottom().value.max(b.top().value);
                    let left = b.left().value.min(b.right().value);
                    let right = b.left().value.max(b.right().value);
                    if (right - left) >= 0.05 && (top - bottom) >= 0.05 {
                        Some(PdfRect::new_from_values(bottom, left, top, right))
                    } else {
                        None
                    }
                });
            boxes.push(PageCharBox {
                ch: uc,
                rect: rect_opt,
            });
        }
        self.cached_char_boxes = Some((page_idx, boxes));
    }

    pub(super) fn clear_text_selection(&mut self) {
        self.selected_text.clear();
        self.selected_text_page = None;
        self.selected_text_rects.clear();
    }

    /// Selects all characters on `page_idx` whose bounding boxes intersect `pdf_rect`
    /// and populates `self.selected_text` and `self.selected_text_rects`.
    pub(super) fn select_text_in_pdf_rect(&mut self, page_idx: u16, pdf_rect: PdfRect) -> usize {
        self.ensure_page_char_boxes(page_idx);
        let Some((cached_p, ref chars)) = self.cached_char_boxes else {
            self.clear_text_selection();
            return 0;
        };
        if cached_p != page_idx || chars.is_empty() {
            self.clear_text_selection();
            return 0;
        }

        let sel_bottom = pdf_rect.bottom().value.min(pdf_rect.top().value);
        let sel_top = pdf_rect.bottom().value.max(pdf_rect.top().value);
        let sel_left = pdf_rect.left().value.min(pdf_rect.right().value);
        let sel_right = pdf_rect.left().value.max(pdf_rect.right().value);

        let mut text_out = String::new();
        let mut line_rects: Vec<PdfRect> = Vec::new();
        let mut prev_rect: Option<PdfRect> = None;

        for item in chars.iter() {
            let Some(cb) = item.rect else {
                continue;
            };
            let cx = (cb.left().value + cb.right().value) * 0.5;
            let cy = (cb.bottom().value + cb.top().value) * 0.5;
            let overlaps = cb.left().value <= sel_right
                && cb.right().value >= sel_left
                && cb.bottom().value <= sel_top
                && cb.top().value >= sel_bottom;
            let center_in =
                cx >= sel_left && cx <= sel_right && cy >= sel_bottom && cy <= sel_top;

            if !(center_in || overlaps) {
                continue;
            }

            if item.ch == '\r' {
                continue;
            }

            if let Some(pr) = prev_rect {
                let pr_mid_y = (pr.bottom().value + pr.top().value) * 0.5;
                let cb_mid_y = (cb.bottom().value + cb.top().value) * 0.5;
                let lh = (pr.top().value - pr.bottom().value).max(4.0);
                if (pr_mid_y - cb_mid_y).abs() >= lh * 0.65 {
                    if !text_out.ends_with('\n') && !text_out.is_empty() {
                        text_out.push('\n');
                    }
                } else {
                    let gap_x = cb.left().value - pr.right().value;
                    let cw = (pr.right().value - pr.left().value).max(2.0);
                    if gap_x > cw * 0.45
                        && !text_out.ends_with(' ')
                        && !text_out.ends_with('\n')
                        && !item.ch.is_whitespace()
                    {
                        text_out.push(' ');
                    }
                }
            }

            text_out.push(item.ch);
            if !item.ch.is_whitespace() {
                prev_rect = Some(cb);
            }

            if let Some(last) = line_rects.last_mut() {
                let last_mid_y = (last.bottom().value + last.top().value) * 0.5;
                let cb_mid_y = (cb.bottom().value + cb.top().value) * 0.5;
                let lh = (last.top().value - last.bottom().value).max(4.0);
                if (last_mid_y - cb_mid_y).abs() < lh * 0.65 {
                    *last = PdfRect::new_from_values(
                        last.bottom().value.min(cb.bottom().value),
                        last.left().value.min(cb.left().value),
                        last.top().value.max(cb.top().value),
                        last.right().value.max(cb.right().value),
                    );
                    continue;
                }
            }
            line_rects.push(cb);
        }

        let trimmed = text_out.trim().to_string();
        if trimmed.is_empty() || line_rects.is_empty() {
            self.clear_text_selection();
            return 0;
        }

        let char_count = trimmed.chars().count();
        self.selected_text = trimmed;
        self.selected_text_page = Some(page_idx);
        self.selected_text_rects = line_rects;
        char_count
    }

    /// Selects the contiguous word under `(pt_x, pt_y)` on `page_idx` (triggered by double-click in `MouseMode::Select`).
    pub(super) fn select_word_at_pdf_point(
        &mut self,
        page_idx: u16,
        pt_x: PdfPoints,
        pt_y: PdfPoints,
    ) -> bool {
        self.ensure_page_char_boxes(page_idx);
        let Some((cached_p, ref chars)) = self.cached_char_boxes else {
            return false;
        };
        if cached_p != page_idx || chars.is_empty() {
            return false;
        }

        let is_word_char = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
        let x = pt_x.value;
        let y = pt_y.value;

        let mut hit_idx: Option<usize> = None;
        let mut best_dist_sq = 64.0_f32; // Within 8pt tolerance
        for (idx, item) in chars.iter().enumerate() {
            if !is_word_char(item.ch) {
                continue;
            }
            let Some(r) = item.rect else {
                continue;
            };
            let dx = if x < r.left().value {
                r.left().value - x
            } else if x > r.right().value {
                x - r.right().value
            } else {
                0.0
            };
            let dy = if y < r.bottom().value {
                r.bottom().value - y
            } else if y > r.top().value {
                y - r.top().value
            } else {
                0.0
            };
            let d2 = dx * dx + dy * dy;
            if d2 <= best_dist_sq {
                best_dist_sq = d2;
                hit_idx = Some(idx);
                if d2 == 0.0 {
                    break;
                }
            }
        }

        let Some(center_idx) = hit_idx else {
            return false;
        };

        let mut start = center_idx;
        while start > 0 && is_word_char(chars[start - 1].ch) {
            if let (Some(r_cur), Some(r_prev)) = (chars[start].rect, chars[start - 1].rect) {
                let lh = (r_cur.top().value - r_cur.bottom().value).max(4.0);
                if ((r_cur.bottom().value - r_prev.bottom().value).abs() > lh * 0.8)
                    || ((r_cur.left().value - r_prev.right().value) > lh * 0.6)
                {
                    break;
                }
            }
            start -= 1;
        }

        let mut end = center_idx;
        while end + 1 < chars.len() && is_word_char(chars[end + 1].ch) {
            if let (Some(r_cur), Some(r_next)) = (chars[end].rect, chars[end + 1].rect) {
                let lh = (r_cur.top().value - r_cur.bottom().value).max(4.0);
                if ((r_cur.bottom().value - r_next.bottom().value).abs() > lh * 0.8)
                    || ((r_next.left().value - r_cur.right().value) > lh * 0.6)
                {
                    break;
                }
            }
            end += 1;
        }

        let mut word = String::new();
        let mut union_rect: Option<PdfRect> = None;
        for item in &chars[start..=end] {
            word.push(item.ch);
            if let Some(cb) = item.rect {
                union_rect = Some(match union_rect {
                    Some(u) => PdfRect::new_from_values(
                        u.bottom().value.min(cb.bottom().value),
                        u.left().value.min(cb.left().value),
                        u.top().value.max(cb.top().value),
                        u.right().value.max(cb.right().value),
                    ),
                    None => cb,
                });
            }
        }

        if word.is_empty() {
            return false;
        }
        self.selected_text = word;
        self.selected_text_page = Some(page_idx);
        self.selected_text_rects = union_rect.into_iter().collect();
        true
    }

    /// Copies `text` to the OS system clipboard without external crate dependencies.
    pub(super) fn copy_text_to_os_clipboard(text: &str) -> anyhow::Result<()> {
        use std::io::Write;
        use std::process::{Command, Stdio};

        #[cfg(target_os = "macos")]
        let mut child = Command::new("pbcopy").stdin(Stdio::piped()).spawn()?;

        #[cfg(target_os = "windows")]
        let mut child = Command::new("clip").stdin(Stdio::piped()).spawn()?;

        #[cfg(target_os = "linux")]
        let mut child = Command::new("wl-copy")
            .stdin(Stdio::piped())
            .spawn()
            .or_else(|_| {
                Command::new("xclip")
                    .args(["-selection", "clipboard"])
                    .stdin(Stdio::piped())
                    .spawn()
            })?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes())?;
        }
        let status = child.wait()?;
        if !status.success() {
            anyhow::bail!("Clipboard command exited with status {status}");
        }
        Ok(())
    }

    /// Reads plain UTF-8 text from the OS system clipboard without external crate dependencies.
    pub(super) fn read_text_from_os_clipboard() -> anyhow::Result<String> {
        use std::process::Command;

        #[cfg(target_os = "macos")]
        let out = Command::new("pbpaste").output()?;

        #[cfg(target_os = "windows")]
        let out = {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            Command::new("powershell")
                .args(["-NoProfile", "-Command", "Get-Clipboard"])
                .creation_flags(CREATE_NO_WINDOW)
                .output()?
        };

        #[cfg(target_os = "linux")]
        let out = Command::new("wl-paste")
            .arg("-n")
            .output()
            .or_else(|_| {
                Command::new("xclip")
                    .args(["-selection", "clipboard", "-o"])
                    .output()
            })?;

        if !out.status.success() {
            anyhow::bail!("Clipboard read failed with status {}", out.status);
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Copies `self.selected_text` to the OS clipboard (`Ctrl/Cmd+C`).
    pub(super) fn copy_selected_text_to_clipboard(&mut self) {
        if self.selected_text.is_empty() {
            self.status_notice = Some(
                "No text selected — Drag or double-click text in Select mode first".to_string(),
            );
            self.update_title_bar();
            self.request_redraw();
            return;
        }
        let count = self.selected_text.chars().count();
        match Self::copy_text_to_os_clipboard(&self.selected_text) {
            Ok(()) => {
                self.status_notice = Some(format!("Copied {count} character(s) to clipboard"));
            }
            Err(e) => {
                self.status_notice = Some(format!("Clipboard copy failed: {e}"));
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    pub(super) fn navigate_to_page(&mut self, new_index: u16) {
        let total = self.total_pages();
        if total == 0 {
            return;
        }
        let clamped = new_index.min(total - 1);
        if clamped != self.current_page_index {
            self.current_page_index = clamped;
            self.scroll_y = 0.0;
            self.scroll_x = 0.0;
            self.invalidate_page_caches();
            self.status_notice = None;
            self.update_title_bar();
            self.request_redraw();
        }
    }

    pub(super) fn step_pages_forward(&mut self) {
        let total = self.total_pages();
        if total == 0 {
            return;
        }
        let step = if self.view_mode == ViewMode::TwoPageSpread {
            2
        } else {
            1
        };
        if self.current_page_index + step < total {
            self.navigate_to_page(self.current_page_index + step);
        } else if self.current_page_index + 1 < total {
            self.navigate_to_page(total - 1);
        }
    }

    pub(super) fn step_pages_backward(&mut self) {
        let step = if self.view_mode == ViewMode::TwoPageSpread {
            2
        } else {
            1
        };
        if self.current_page_index > 0 {
            self.navigate_to_page(self.current_page_index.saturating_sub(step));
        }
    }

    /// Sets `self.zoom_level` (clamped to `25%..=800%`) while adjusting `self.pan_x` and `self.scroll_y`
    /// so the document point under `(anchor_x, anchor_y)` stays fixed on screen.
    pub(super) fn set_zoom_around_cursor(&mut self, new_zoom: f32, anchor_x: f64, anchor_y: f64) {
        if self.compare_mode.is_some() {
            let clamped = new_zoom.clamp(MIN_ZOOM_LEVEL, 4.0);
            if (clamped - self.zoom_level).abs() < 0.001 {
                return;
            }
            self.zoom_level = clamped;
            if let Some(ref mut cmp) = self.compare_mode {
                cmp.invalidate_caches();
            }
            self.status_notice = Some(format!("Compare Zoom: {:.0}%", self.zoom_level * 100.0));
            self.update_title_bar();
            self.request_redraw();
            return;
        }
        let clamped = new_zoom.clamp(MIN_ZOOM_LEVEL, MAX_ZOOM_LEVEL);
        let old_zoom = self.zoom_level.clamp(MIN_ZOOM_LEVEL, MAX_ZOOM_LEVEL);
        if (clamped - old_zoom).abs() < 0.001 {
            return;
        }
        let ratio = clamped / old_zoom;

        let (sidebar_w, top_bars_h, content_w, content_h) = if let Some(w) = self.window.as_ref() {
            let sz = w.inner_size();
            let sf = self.effective_scale_factor(w.scale_factor());
            let sw = self.sidebar_width_px(sf).min(sz.width.saturating_sub(64));
            let th = self.top_bars_height_px(sf).min(sz.height.saturating_sub(32));
            let sb = self
                .status_bar_height_px(sf)
                .min(sz.height.saturating_sub(th + 16));
            (
                sw,
                th,
                sz.width.saturating_sub(sw).max(1),
                sz.height.saturating_sub(th + sb).max(1),
            )
        } else {
            (0, 52, 1024, 1006)
        };

        let (_, (draw_x, draw_y, old_w, old_h)) =
            self.resolve_target_page_and_draw_rect(anchor_x, anchor_y);

        if old_w > 0 && old_h > 0 {
            let in_page_x = anchor_x >= draw_x as f64 && anchor_x <= (draw_x + old_w as i32) as f64;
            let in_page_y = anchor_y >= draw_y as f64 && anchor_y <= (draw_y + old_h as i32) as f64;

            let (ax, u) = if in_page_x {
                (
                    anchor_x as f32,
                    ((anchor_x as f32 - draw_x as f32) / (old_w as f32)).clamp(0.0, 1.0),
                )
            } else {
                (
                    (sidebar_w as f32) + (content_w as f32) * 0.5,
                    0.5,
                )
            };
            let (ay, v) = if in_page_y {
                (
                    anchor_y as f32,
                    ((anchor_y as f32 - draw_y as f32) / (old_h as f32)).clamp(0.0, 1.0),
                )
            } else {
                (
                    (top_bars_h as f32) + (content_h as f32) * 0.5,
                    0.5,
                )
            };

            let new_w = (old_w as f32) * ratio;
            let new_h = (old_h as f32) * ratio;
            let new_draw_x = ax - u * new_w;
            let new_draw_y = ay - v * new_h;

            match self.effective_scroll_mode() {
                ScrollMode::ContinuousHorizontal => {
                    self.pan_x = 0.0;
                    self.scroll_x = ((sidebar_w + PAGE_MARGIN_PX) as f32) - new_draw_x;
                    if new_h > content_h as f32 {
                        self.scroll_y = ((top_bars_h as f32) - new_draw_y)
                            .clamp(0.0, (new_h - content_h as f32).max(0.0));
                    } else {
                        self.scroll_y = 0.0;
                    }
                }
                ScrollMode::ContinuousVertical => {
                    if new_w > content_w as f32 {
                        self.pan_x = ((sidebar_w as f32) - new_draw_x)
                            .clamp(0.0, (new_w - content_w as f32).max(0.0));
                    } else {
                        self.pan_x = 0.0;
                    }
                    self.scroll_y = ((top_bars_h + PAGE_MARGIN_PX) as f32) - new_draw_y;
                }
                ScrollMode::SinglePage => {
                    if new_w > content_w as f32 {
                        self.pan_x = ((sidebar_w as f32) - new_draw_x)
                            .clamp(0.0, (new_w - content_w as f32).max(0.0));
                    } else {
                        self.pan_x = 0.0;
                    }
                    if new_h > content_h as f32 {
                        self.scroll_y = ((top_bars_h as f32) - new_draw_y)
                            .clamp(0.0, (new_h - content_h as f32).max(0.0));
                    } else {
                        self.scroll_y = 0.0;
                    }
                }
            }
        }

        self.zoom_level = clamped;
        for (_, old_tile) in self.tile_cache.drain() {
            if old_tile.pixels.capacity() > self.spare_pixel_buf.capacity() {
                self.spare_pixel_buf = old_tile.pixels;
            }
        }
        let pct = (self.zoom_level * 100.0).round() as u32;
        self.status_notice = Some(format!(
            "Zoom: {pct}% ('+'/'-' or Ctrl+Wheel to zoom, '0' to reset to 100%)"
        ));
        self.update_title_bar();
        self.request_redraw();
    }

    pub(super) fn zoom_by_step(&mut self, factor: f32) {
        if self.is_markdown_tab() {
            self.zoom_level = (self.zoom_level * factor).clamp(0.6, 2.2);
            let pct = (self.zoom_level * 100.0).round() as u32;
            self.status_notice = Some(format!("Markdown Zoom: {pct}%"));
            self.update_title_bar();
            self.request_redraw();
            return;
        }
        let target = (self.zoom_level * factor).clamp(MIN_ZOOM_LEVEL, MAX_ZOOM_LEVEL);
        let (cx, cy) = self.cursor_pos;
        self.set_zoom_around_cursor(target, cx, cy);
    }

    pub(super) fn reset_zoom(&mut self) {
        self.zoom_level = 1.0;
        self.pan_x = 0.0;
        self.scroll_y = 0.0;
        self.scroll_x = 0.0;
        if self.is_markdown_tab() {
            self.status_notice = Some("Markdown Zoom reset to 100%".to_string());
            self.update_title_bar();
            self.request_redraw();
            return;
        }
        if let Some(ref mut cmp) = self.compare_mode {
            cmp.fit_mode = super::compare_mode::CompareFitMode::ActualSize;
            cmp.invalidate_caches();
        }
        for (_, old_tile) in self.tile_cache.drain() {
            if old_tile.pixels.capacity() > self.spare_pixel_buf.capacity() {
                self.spare_pixel_buf = old_tile.pixels;
            }
        }
        self.status_notice = Some("Zoom reset to 100%".to_string());
        self.update_title_bar();
        self.request_redraw();
    }

    pub(super) fn pan_horizontal(&mut self, delta_x: f32) -> bool {
        if !self.view_mode.is_page_view() {
            return false;
        }
        if self.effective_scroll_mode() == ScrollMode::ContinuousHorizontal {
            self.scroll_x += delta_x;
            self.request_redraw();
            return true;
        }
        let (_, _, bmp_w, _) = self.page_draw_rect;
        let content_w = self
            .cached_page
            .as_ref()
            .map(|c| c.viewport_width)
            .or_else(|| {
                self.continuous_page_cache
                    .get(&self.current_page_index)
                    .map(|c| c.viewport_width)
            })
            .unwrap_or(1024);
        if bmp_w > content_w {
            let max_pan = (bmp_w - content_w) as f32;
            let new_pan = (self.pan_x + delta_x).clamp(0.0, max_pan);
            if (new_pan - self.pan_x).abs() > 0.5 {
                self.pan_x = new_pan;
                self.request_redraw();
                return true;
            }
        }
        false
    }

    /// Scrolls vertically in `ContinuousVertical`, horizontally in `ContinuousHorizontal` (Book mode),
    /// or in any page view when the page bitmap exceeds the viewport height,
    /// seamlessly transitioning to the next/previous page when scrolling past the bottom/top!
    pub(super) fn scroll_or_turn_page_vertical(&mut self, scroll_delta_px: f32) -> bool {
        if self.is_markdown_tab() {
            if let Some(w) = self.window.as_ref() {
                let sz = w.inner_size();
                let sf = self.effective_scale_factor(w.scale_factor());
                let top_bars_h = self.top_bars_height_px(sf);
                let status_bar_h = self.status_bar_height_px(sf);
                let canvas_h = sz.height.saturating_sub(top_bars_h + status_bar_h);
                let max_s = self.md_max_scroll_y(canvas_h, sf);
                self.scroll_y = (self.scroll_y + scroll_delta_px).clamp(0.0, max_s);
                self.request_redraw();
            }
            return true;
        }
        if self.view_mode.is_page_view() {
            match self.effective_scroll_mode() {
                ScrollMode::ContinuousVertical => {
                    self.scroll_y += scroll_delta_px;
                    self.request_redraw();
                    return true;
                }
                ScrollMode::ContinuousHorizontal => {
                    self.scroll_x += scroll_delta_px;
                    self.request_redraw();
                    return true;
                }
                ScrollMode::SinglePage => {}
            }
        }
        if self.view_mode == ViewMode::TextView {
            let max_s = self.text_view_max_scroll.max(0.0);
            if max_s > 1.0 {
                if scroll_delta_px > 0.0 && self.scroll_y < max_s - 1.0 {
                    self.scroll_y = (self.scroll_y + scroll_delta_px).min(max_s);
                    self.request_redraw();
                    return true;
                } else if scroll_delta_px < 0.0 && self.scroll_y > 1.0 {
                    self.scroll_y = (self.scroll_y + scroll_delta_px).max(0.0);
                    self.request_redraw();
                    return true;
                }
            }
            return false;
        }
        if self.view_mode.is_page_view() {
            if let Some(cache) = &self.cached_page {
                let bmp_h = cache.bitmap_height;
                let content_h = cache.viewport_height;
                if bmp_h > content_h {
                    let max_scroll = (bmp_h - content_h) as f32;
                    let new_scroll = self.scroll_y + scroll_delta_px;
                    if scroll_delta_px > 0.0 {
                        if self.scroll_y < max_scroll - 1.0 {
                            self.scroll_y = new_scroll.min(max_scroll);
                            self.request_redraw();
                            return true;
                        } else if matches!(self.view_mode, ViewMode::FitWidth | ViewMode::FitTextWidth)
                            && self.current_page_index + 1 < self.total_pages()
                        {
                            self.current_page_index += 1;
                            self.scroll_y = 0.0;
                            self.invalidate_page_caches();
                            self.update_title_bar();
                            self.request_redraw();
                            return true;
                        }
                    } else if scroll_delta_px < 0.0 {
                        if self.scroll_y > 1.0 {
                            self.scroll_y = new_scroll.max(0.0);
                            self.request_redraw();
                            return true;
                        } else if matches!(self.view_mode, ViewMode::FitWidth | ViewMode::FitTextWidth)
                            && self.current_page_index > 0
                        {
                            self.current_page_index -= 1;
                            self.scroll_y = 99999.0; // Clamped to bottom of previous page on render
                            self.invalidate_page_caches();
                            self.update_title_bar();
                            self.request_redraw();
                            return true;
                        }
                    }
                    return true;
                }
            }
        }
        false
    }

    pub(super) fn rotate_page_clockwise(&mut self, page_idx: u16) {
        self.push_undo_snapshot();
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let Ok(mut page) = doc.pages_mut().get(page_idx) else {
            return;
        };
        let next_rot = match page.rotation().unwrap_or(PdfPageRenderRotation::None) {
            PdfPageRenderRotation::None => PdfPageRenderRotation::Degrees90,
            PdfPageRenderRotation::Degrees90 => PdfPageRenderRotation::Degrees180,
            PdfPageRenderRotation::Degrees180 => PdfPageRenderRotation::Degrees270,
            PdfPageRenderRotation::Degrees270 => PdfPageRenderRotation::None,
        };
        page.set_rotation(next_rot);
        self.invalidate_page_caches();
        self.thumbnail_cache.remove(&page_idx);
        self.status_notice = Some(format!("Rotated page {} by 90°", page_idx + 1));
        self.update_title_bar();
        self.request_redraw();
    }

    pub(super) fn convert_page_to_bw_in_place(&mut self, page_idx: u16) {
        self.push_undo_snapshot();
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let bytes = match doc.save_to_bytes() {
            Ok(b) => b,
            Err(e) => {
                self.status_notice = Some(format!("BW serialize error: {e}"));
                self.update_title_bar();
                return;
            }
        };
        match self.engine.convert_page_to_bw(&bytes, page_idx) {
            Ok(bw_bytes) => match self.engine.load_pdf_from_bytes_safe(bw_bytes) {
                Ok(new_doc) => {
                    self.doc = Some(new_doc);
                    self.bw_pages.insert(page_idx);
                    self.invalidate_page_caches();
                    self.thumbnail_cache.remove(&page_idx);
                    self.status_notice = Some(format!(
                        "Converted Page {} to Grayscale [BW] (Cmd/Ctrl+Z to undo)",
                        page_idx + 1
                    ));
                    self.update_title_bar();
                    self.request_redraw();
                }
                Err(e) => {
                    self.status_notice = Some(format!("BW reload error: {e}"));
                    self.update_title_bar();
                }
            },
            Err(e) => {
                self.status_notice = Some(format!("BW conversion error: {e}"));
                self.update_title_bar();
            }
        }
    }

    pub(super) fn reset_current_page_crop(&mut self) {
        self.push_undo_snapshot();
        let page_idx = self.current_page_index;
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        match self.engine.reset_page_crop(doc, page_idx) {
            Ok(bytes) => match self.engine.load_pdf_from_bytes_safe(bytes) {
                Ok(new_doc) => {
                    self.doc = Some(new_doc);
                    self.invalidate_page_caches();
                    self.thumbnail_cache.remove(&page_idx);
                    self.status_notice = Some(format!(
                        "Reset crop on Page {} to full MediaBox",
                        page_idx + 1
                    ));
                    self.update_title_bar();
                    self.request_redraw();
                }
                Err(e) => {
                    self.status_notice = Some(format!("Reset crop reload error: {e}"));
                    self.update_title_bar();
                }
            },
            Err(e) => {
                let _ = self.undo_stack.pop_back();
                self.status_notice = Some(format!("Reset crop failed: {e}"));
                self.update_title_bar();
            }
        }
    }

    pub(super) fn flatten_and_unlock_current_pdf(&mut self) {
        // v1.1: signed documents need a second press, because flattening and removing
        // /Perms invalidates digital signatures.
        let sig_count = self.doc.as_ref().map(|d| d.signatures().len()).unwrap_or(0);
        if sig_count > 0 {
            let confirmed = self
                .flatten_confirm_at
                .is_some_and(|t| t.elapsed() <= FLATTEN_CONFIRM_WINDOW);
            if !confirmed {
                self.flatten_confirm_at = Some(Instant::now());
                self.status_notice = Some(format!(
                    "This PDF has {sig_count} digital signature(s). Flatten & Unlock will INVALIDATE them. Press U again within 5 s to continue."
                ));
                self.update_title_bar();
                self.request_redraw();
                return;
            }
        }
        self.flatten_confirm_at = None;

        self.push_undo_snapshot();
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        match self.engine.flatten_and_unlock_doc(doc) {
            Ok(report) => match self.engine.load_pdf_from_bytes_safe(report.bytes) {
                Ok(new_doc) => {
                    self.doc = Some(new_doc);
                    self.invalidate_page_caches();
                    self.thumbnail_cache.clear();
                    let mut notice = String::from("Flattened annotations/forms");
                    if report.permissions_removed {
                        notice.push_str(" & removed permission restrictions");
                    } else {
                        notice.push_str("; WARNING: permission restrictions could NOT be removed");
                    }
                    if !report.failed_pages.is_empty() {
                        notice.push_str(&format!(
                            "; WARNING: {} page(s) failed to flatten (first: p.{})",
                            report.failed_pages.len(),
                            report.failed_pages[0]
                        ));
                    }
                    notice.push_str(" (Press 'S' to save)");
                    self.status_notice = Some(notice);
                    self.update_title_bar();
                    self.request_redraw();
                }
                Err(e) => {
                    self.status_notice = Some(format!("Flatten reload error: {e}"));
                    self.update_title_bar();
                }
            },
            Err(e) => {
                let _ = self.undo_stack.pop_back();
                self.status_notice = Some(format!("Flatten failed: {e}"));
                self.update_title_bar();
            }
        }
    }

    pub(super) fn delete_current_page(&mut self) {
        let total = self.total_pages();
        if total <= 1 {
            self.status_notice = Some("Cannot delete the only remaining page".to_string());
            self.update_title_bar();
            return;
        }
        let deleted_idx = self.current_page_index;
        self.push_undo_snapshot();
        let delete_ok = self
            .doc
            .as_ref()
            .and_then(|doc| doc.pages().get(deleted_idx).ok())
            .is_some_and(|page| page.delete().is_ok());
        if delete_ok {
            let new_total = self.total_pages();
            if self.current_page_index >= new_total && new_total > 0 {
                self.current_page_index = new_total - 1;
            }
            PdfEngine::remap_bookmarks_after_delete(
                &mut self.bookmarks,
                deleted_idx as usize,
                new_total as usize,
                self.skip_pages,
                self.roman_end,
            );
            let next_bw: HashSet<u16> = self
                .bw_pages
                .iter()
                .filter_map(|&p| {
                    if p == deleted_idx {
                        None
                    } else if p > deleted_idx {
                        Some(p - 1)
                    } else {
                        Some(p)
                    }
                })
                .collect();
            self.bw_pages = next_bw;
            self.search_matches.clear();
            self.current_search_match = 0;
            self.invalidate_page_caches();
            self.thumbnail_cache.clear();
            self.status_notice = Some("Deleted page (Cmd/Ctrl+Z to undo)".to_string());
            self.update_title_bar();
            self.request_redraw();
        } else {
            let _ = self.undo_stack.pop_back();
        }
    }

    pub(super) fn insert_blank_pages_at(&mut self, insert_at: u16, count: u16, before: bool) {
        let count = count.clamp(1, 50);
        if self.doc.is_none() {
            return;
        }
        self.push_undo_snapshot();
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let clamped_idx = insert_at.min(doc.pages().len());
        match PdfEngine::insert_blank_pages_in_doc(doc, clamped_idx, count) {
            Ok(inserted_idx) => {
                PdfEngine::remap_bookmarks_after_insert(
                    &mut self.bookmarks,
                    clamped_idx as usize,
                    count as usize,
                    self.skip_pages,
                    self.roman_end,
                );
                let next_bw: HashSet<u16> = self
                    .bw_pages
                    .iter()
                    .map(|&p| if p >= clamped_idx { p.saturating_add(count) } else { p })
                    .collect();
                self.bw_pages = next_bw;
                self.search_matches.clear();
                self.current_search_match = 0;
                self.current_page_index = inserted_idx;
                self.has_unsaved_edits = true;
                self.invalidate_page_caches();
                self.thumbnail_cache.clear();
                let plural = if count == 1 { "page" } else { "pages" };
                let where_str = if before {
                    format!("before Page {}", inserted_idx + 1)
                } else {
                    format!("at Page {}", inserted_idx + 1)
                };
                self.status_notice = Some(format!("Inserted {count} blank {plural} {where_str}"));
                self.update_title_bar();
                self.request_redraw();
            }
            Err(e) => {
                let _ = self.undo_stack.pop_back();
                self.status_notice = Some(format!("Insert blank pages failed: {e}"));
                self.update_title_bar();
                self.request_redraw();
            }
        }
    }

    pub(super) fn insert_blank_page_after_current(&mut self) {
        let target = if self.view_mode == ViewMode::ThumbnailGrid {
            self.hovered_thumb.unwrap_or(self.current_page_index)
        } else {
            self.current_page_index
        };
        self.insert_blank_pages_at(target.saturating_add(1), 1, false);
    }

    pub(super) fn insert_blank_page_before_current(&mut self) {
        let target = if self.view_mode == ViewMode::ThumbnailGrid {
            self.hovered_thumb.unwrap_or(self.current_page_index)
        } else {
            self.current_page_index
        };
        self.insert_blank_pages_at(target, 1, true);
    }

    pub(super) fn merge_pdf_dialog(&mut self) {
        let Some(second_path) = rfd::FileDialog::new()
            .set_title("Select PDF to Merge")
            .add_filter("PDF Document", &["pdf"])
            .pick_file()
        else {
            return;
        };

        let second_doc = match self.engine.load_pdf_safe(&second_path) {
            Ok(d) => d,
            Err(e) => {
                self.status_notice = Some(format!("Merge load failed: {e}"));
                self.update_title_bar();
                return;
            }
        };

        self.push_undo_snapshot();
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        match doc.pages_mut().append(&second_doc) {
            Ok(()) => {
                self.invalidate_page_caches();
                self.thumbnail_cache.clear();
                self.status_notice = Some("Merged successfully!".to_string());
                self.update_title_bar();
                self.request_redraw();
            }
            Err(e) => {
                self.status_notice = Some(format!("Merge failed: {e}"));
                self.update_title_bar();
            }
        }
    }

    /// Legacy direct-picker Batch Combine Multiple PDFs (superseded by Interactive Combine Modal).
    #[allow(dead_code)]
    pub(super) fn combine_multiple_pdfs_dialog(&mut self) {
        let Some(paths) = rfd::FileDialog::new()
            .set_title("Select 2 or More PDFs to Combine in Order")
            .add_filter("PDF", &["pdf"])
            .pick_files()
        else {
            return;
        };

        if paths.len() < 2 {
            self.status_notice =
                Some("Select at least 2 PDF files to combine (Shift+M)".to_string());
            self.update_title_bar();
            self.request_redraw();
            return;
        }

        let num_files = paths.len();
        match self.engine.combine_multiple_pdfs(&paths) {
            Ok((combined_bytes, total_pages)) => {
                let virtual_path = match paths[0].parent() {
                    Some(p) if !p.as_os_str().is_empty() => p.join("Combined.pdf"),
                    _ => PathBuf::from("Combined.pdf"),
                };
                match self.open_bytes_in_new_tab(combined_bytes, virtual_path) {
                    Ok(()) => {
                        self.status_notice = Some(format!(
                            "Combined {num_files} PDFs ({total_pages} pages) into 'Combined.pdf' (Press 'S' to save)"
                        ));
                    }
                    Err(e) => {
                        self.status_notice = Some(format!("Failed to open Combined.pdf tab: {e}"));
                    }
                }
            }
            Err(e) => {
                self.status_notice = Some(format!("Combine PDFs failed: {e}"));
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    /// Opens native file dialog to choose a PNG/JPG signature or image stamp (`P` key).
    pub(super) fn pick_stamp_image_dialog(&mut self) {
        let Some(img_path) = rfd::FileDialog::new()
            .set_title("Select Signature or Image Stamp (PNG with transparency or JPG)")
            .add_filter("Images", &["png", "jpg", "jpeg"])
            .pick_file()
        else {
            return;
        };

        let fname = img_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("stamp.png")
            .to_string();

        match PdfEngine::load_stamp_image(&img_path) {
            Ok(buf) => {
                let w = buf.width;
                let h = buf.height;
                self.pending_stamp_image = Some(buf);
                self.mouse_mode = MouseMode::StampImage;
                self.status_notice = Some(format!(
                    "Loaded '{fname}' ({w}x{h}) — Click or Drag on page to stamp signature/image!"
                ));
            }
            Err(e) => {
                self.status_notice = Some(format!("Failed to load image '{fname}': {e}"));
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    pub(super) fn execute_text_search(&mut self, query: &str) {
        let trimmed = query.trim().to_string();
        let case_sensitive = self.search_case_sensitive;
        let whole_word = self.search_whole_word;
        let q_chars: Vec<char> = if case_sensitive {
            trimmed.chars().collect()
        } else {
            trimmed.chars().flat_map(|c| c.to_lowercase()).collect()
        };
        if q_chars.is_empty() {
            self.search_query.clear();
            self.search_matches.clear();
            self.current_search_match = 0;
            self.incremental_search = None;
            self.status_notice = Some("Cleared text search highlights".to_string());
            self.update_title_bar();
            self.request_redraw();
            return;
        }

        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let total_pages = doc.pages().len();
        if total_pages == 0 {
            return;
        }

        self.search_query = trimmed.clone();
        self.search_matches.clear();
        self.current_search_match = 0;
        self.incremental_search = Some(IncrementalSearchState {
            query: trimmed,
            q_chars,
            case_sensitive,
            whole_word,
            next_page: 0,
            total_pages,
            origin_page: self.current_page_index,
            jumped_to_first: false,
        });

        // Immediately run the first ~8 ms time-slice so small/medium PDFs finish in a single frame
        // while 1,000+ page PDFs stay responsive and continue across frames in `about_to_wait`.
        self.step_incremental_search(Duration::from_millis(8));
    }

    /// Advances an active incremental search by searching pages until `budget` (~8 ms) elapses
    /// (or at least `max_pages_override` if specified for deterministic testing).
    /// Keeps ONLY the `SearchMatch` list (`self.search_matches`) in memory, never a per-page text cache.
    /// Returns `true` if incremental search is still in progress after this slice.
    pub(super) fn step_incremental_search(&mut self, budget: Duration) -> bool {
        self.step_incremental_search_with_limit(budget, usize::MAX)
    }

    pub(super) fn step_incremental_search_with_limit(
        &mut self,
        budget: Duration,
        max_pages: usize,
    ) -> bool {
        let Some(mut state) = self.incremental_search.take() else {
            return false;
        };
        if self.doc.is_none() {
            return false;
        }

        let slice_start = Instant::now();
        let mut pages_scanned_in_slice = 0usize;

        while state.next_page < state.total_pages && pages_scanned_in_slice < max_pages {
            let page_idx = state.next_page;
            let page_matches = match self.doc.as_ref() {
                Some(doc) => PdfEngine::search_text_on_single_page_opts(
                    doc,
                    page_idx,
                    &state.q_chars,
                    state.case_sensitive,
                    state.whole_word,
                ),
                None => break,
            };
            if !page_matches.is_empty() {
                self.search_matches.extend(page_matches);
                if !state.jumped_to_first {
                    if let Some(first_on_or_after) = self
                        .search_matches
                        .iter()
                        .position(|m| m.page_index >= state.origin_page)
                    {
                        state.jumped_to_first = true;
                        self.current_search_match = first_on_or_after;
                        let target_page = self.search_matches[first_on_or_after].page_index;
                        if target_page != self.current_page_index {
                            self.current_page_index = target_page;
                            self.scroll_y = 0.0;
                            self.invalidate_page_caches();
                        }
                    }
                }
            }

            state.next_page += 1;
            pages_scanned_in_slice += 1;

            if slice_start.elapsed() >= budget {
                break;
            }
        }

        if state.next_page < state.total_pages {
            let scanned = state.next_page;
            let total = state.total_pages;
            let found = self.search_matches.len();
            let q = state.query.clone();
            self.incremental_search = Some(state);
            self.status_notice = Some(format!(
                "Searching '{}' (Page {}/{}, {} match(es) so far) — Press Esc to cancel",
                q, scanned, total, found
            ));
            self.update_title_bar();
            self.request_redraw();
            true
        } else {
            // Search finished across all pages
            if self.search_matches.is_empty() {
                self.current_search_match = 0;
                self.status_notice = Some(format!("No matches found for '{}'", state.query));
            } else {
                if !state.jumped_to_first {
                    self.current_search_match = 0;
                    let target_page = self.search_matches[0].page_index;
                    if target_page != self.current_page_index {
                        self.current_page_index = target_page;
                        self.scroll_y = 0.0;
                        self.invalidate_page_caches();
                    }
                }
                let idx = self.current_search_match.min(self.search_matches.len() - 1);
                let target_page = self.search_matches[idx].page_index;
                self.status_notice = Some(format!(
                    "Match {}/{} for '{}' on Page {} (F3 next / Shift+F3 prev)",
                    idx + 1,
                    self.search_matches.len(),
                    state.query,
                    target_page + 1
                ));
            }
            self.update_title_bar();
            self.request_redraw();
            false
        }
    }

    pub(super) fn cancel_incremental_search(&mut self) -> bool {
        if let Some(state) = self.incremental_search.take() {
            self.status_notice = Some(format!(
                "Search cancelled at Page {}/{} ({} match(es) found for '{}')",
                state.next_page,
                state.total_pages,
                self.search_matches.len(),
                state.query
            ));
            self.update_title_bar();
            self.request_redraw();
            true
        } else {
            false
        }
    }

    pub(super) fn jump_to_next_search_match(&mut self) {
        if self.search_matches.is_empty() {
            self.input_mode = InputMode::SearchText {
                query_input: self.search_query.clone(),
            };
            self.update_title_bar();
            self.request_redraw();
            return;
        }
        self.current_search_match = (self.current_search_match + 1) % self.search_matches.len();
        let target_page = self.search_matches[self.current_search_match].page_index;
        if target_page != self.current_page_index {
            self.current_page_index = target_page;
            self.scroll_y = 0.0;
            self.invalidate_page_caches();
        }
        self.status_notice = Some(format!(
            "Match {}/{} for '{}' on Page {}",
            self.current_search_match + 1,
            self.search_matches.len(),
            self.search_query,
            target_page + 1
        ));
        self.update_title_bar();
        self.request_redraw();
    }

    pub(super) fn jump_to_prev_search_match(&mut self) {
        if self.search_matches.is_empty() {
            self.input_mode = InputMode::SearchText {
                query_input: self.search_query.clone(),
            };
            self.update_title_bar();
            self.request_redraw();
            return;
        }
        let len = self.search_matches.len();
        self.current_search_match = if self.current_search_match == 0 {
            len - 1
        } else {
            self.current_search_match - 1
        };
        let target_page = self.search_matches[self.current_search_match].page_index;
        if target_page != self.current_page_index {
            self.current_page_index = target_page;
            self.scroll_y = 0.0;
            self.invalidate_page_caches();
        }
        self.status_notice = Some(format!(
            "Match {}/{} for '{}' on Page {}",
            self.current_search_match + 1,
            self.search_matches.len(),
            self.search_query,
            target_page + 1
        ));
        self.update_title_bar();
        self.request_redraw();
    }

    pub(super) fn execute_split_range(&mut self, range_spec: &str) {
        let trimmed = range_spec.trim();
        if trimmed.is_empty() {
            self.status_notice = Some("Split cancelled (empty range)".to_string());
            self.update_title_bar();
            return;
        }

        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let stem = self
            .file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document");
        let default_name = format!("{stem}_split.pdf");

        let mut dialog = rfd::FileDialog::new()
            .set_title("Save Extracted/Split PDF")
            .add_filter("PDF Document", &["pdf"])
            .set_file_name(&default_name);
        if let Some(parent) = self.file_path.parent() {
            if !parent.as_os_str().is_empty() && parent.exists() {
                dialog = dialog.set_directory(parent);
            }
        }
        let Some(save_path) = dialog.save_file() else {
            self.status_notice = Some("Split cancelled".to_string());
            self.update_title_bar();
            self.request_redraw();
            return;
        };

        match self
            .engine
            .extract_page_spec_from_doc(doc, trimmed, &save_path)
        {
            Ok(count) => {
                let saved_name = save_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&default_name);
                self.status_notice =
                    Some(format!("Extracted {count} page(s) to '{saved_name}'"));
            }
            Err(e) => {
                self.status_notice = Some(format!("Split error: {e}"));
            }
        }
        self.update_title_bar();
    }

    pub(super) fn open_file_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Supported Documents (PDF, Markdown, LaTeX, EPUB)", &["pdf", "md", "markdown", "tex", "epub"])
            .add_filter("PDF Document", &["pdf"])
            .add_filter("EPUB E-Book", &["epub"])
            .add_filter("Markdown Document", &["md", "markdown"])
            .add_filter("LaTeX Document", &["tex"])
            .pick_file()
        {
            let fname = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("file.pdf")
                .to_string();
            match self.open_document_in_new_tab(path) {
                Ok(()) => {
                    self.status_notice = Some(format!(
                        "Opened '{fname}' in Tab {}",
                        self.active_tab + 1
                    ));
                }
                Err(e) => {
                    self.status_notice = Some(format!("ERROR opening '{fname}': {e}"));
                }
            }
            self.update_title_bar();
            self.request_redraw();
        }
    }

    fn handle_markdown_editor_key(&mut self, key: &winit::keyboard::Key, text: Option<&str>) -> bool {
        use winit::keyboard::{Key, NamedKey};
        self.ensure_md_editor_initialized();
        let sf = self
            .window
            .as_ref()
            .map(|w| self.effective_scale_factor(w.scale_factor()))
            .unwrap_or(1.0);
        let (ew, view_h) = self
            .window
            .as_ref()
            .and_then(|w| {
                let sz = w.inner_size();
                let top_bars_h = self.top_bars_height_px(sf);
                let status_bar_h = self.status_bar_height_px(sf);
                let sidebar_w = self.sidebar_width_px(sf).min(sz.width.saturating_sub(64));
                self.md_editor_rect(sidebar_w, top_bars_h, sz.width, sz.height, status_bar_h, sf)
                    .map(|(_, _, w_px, h_px)| (w_px, h_px as f32))
            })
            .unwrap_or((600, 500.0));
        let zoom = self.zoom_level;

        if self.cmd_or_ctrl_pressed {
            if let Key::Character(ch) = key {
                let s = ch.as_str();
                if s.eq_ignore_ascii_case("s") {
                    self.save_active_markdown();
                    return true;
                } else if s.eq_ignore_ascii_case("a") {
                    if let Some(ref mut ed) = self.md_editor {
                        ed.select_all();
                    }
                    self.request_redraw();
                    return true;
                } else if s.eq_ignore_ascii_case("c") {
                    if let Some(sel) = self.md_editor.as_ref().and_then(|e| e.selected_text()) {
                        let _ = Self::copy_text_to_os_clipboard(&sel);
                    }
                    return true;
                } else if s.eq_ignore_ascii_case("x") {
                    let mut changed = false;
                    if let Some(ref mut ed) = self.md_editor {
                        if let Some(sel) = ed.selected_text() {
                            let _ = Self::copy_text_to_os_clipboard(&sel);
                            changed = ed.delete_selection();
                            ed.ensure_cursor_visible(ew, view_h, sf, zoom);
                        }
                    }
                    if changed {
                        self.sync_markdown_from_editor();
                    }
                    return true;
                } else if s.eq_ignore_ascii_case("v") {
                    if let Ok(clip) = Self::read_text_from_os_clipboard() {
                        if !clip.is_empty() {
                            if let Some(ref mut ed) = self.md_editor {
                                ed.insert_str(&clip);
                                ed.ensure_cursor_visible(ew, view_h, sf, zoom);
                            }
                            self.sync_markdown_from_editor();
                        }
                    }
                    return true;
                } else if s.eq_ignore_ascii_case("z") {
                    let changed = if let Some(ref mut ed) = self.md_editor {
                        let ok = if self.shift_pressed || s == "Z" { ed.redo() } else { ed.undo() };
                        ed.ensure_cursor_visible(ew, view_h, sf, zoom);
                        ok
                    } else {
                        false
                    };
                    if changed {
                        self.sync_markdown_from_editor();
                    }
                    return true;
                } else if s.eq_ignore_ascii_case("y") {
                    let changed = if let Some(ref mut ed) = self.md_editor {
                        let ok = ed.redo();
                        ed.ensure_cursor_visible(ew, view_h, sf, zoom);
                        ok
                    } else {
                        false
                    };
                    if changed {
                        self.sync_markdown_from_editor();
                    }
                    return true;
                }
            }
            return false;
        }

        if self.alt_pressed {
            return false;
        }

        let shift = self.shift_pressed;
        match key {
            Key::Named(NamedKey::Escape)
            | Key::Named(NamedKey::F1)
            | Key::Named(NamedKey::F3)
            | Key::Named(NamedKey::F5)
            | Key::Named(NamedKey::F11) => return false,
            Key::Named(NamedKey::ArrowUp) => {
                if let Some(ref mut ed) = self.md_editor {
                    ed.move_cursor(-1, 0, shift);
                    ed.ensure_cursor_visible(ew, view_h, sf, zoom);
                }
                self.request_redraw();
                return true;
            }
            Key::Named(NamedKey::ArrowDown) => {
                if let Some(ref mut ed) = self.md_editor {
                    ed.move_cursor(1, 0, shift);
                    ed.ensure_cursor_visible(ew, view_h, sf, zoom);
                }
                self.request_redraw();
                return true;
            }
            Key::Named(NamedKey::ArrowLeft) => {
                if let Some(ref mut ed) = self.md_editor {
                    ed.move_cursor(0, -1, shift);
                    ed.ensure_cursor_visible(ew, view_h, sf, zoom);
                }
                self.request_redraw();
                return true;
            }
            Key::Named(NamedKey::ArrowRight) => {
                if let Some(ref mut ed) = self.md_editor {
                    ed.move_cursor(0, 1, shift);
                    ed.ensure_cursor_visible(ew, view_h, sf, zoom);
                }
                self.request_redraw();
                return true;
            }
            Key::Named(NamedKey::Home) => {
                if let Some(ref mut ed) = self.md_editor {
                    ed.move_home(shift);
                    ed.ensure_cursor_visible(ew, view_h, sf, zoom);
                }
                self.request_redraw();
                return true;
            }
            Key::Named(NamedKey::End) => {
                if let Some(ref mut ed) = self.md_editor {
                    ed.move_end(shift);
                    ed.ensure_cursor_visible(ew, view_h, sf, zoom);
                }
                self.request_redraw();
                return true;
            }
            Key::Named(NamedKey::PageUp) => {
                if let Some(ref mut ed) = self.md_editor {
                    ed.move_cursor(-15, 0, shift);
                    ed.ensure_cursor_visible(ew, view_h, sf, zoom);
                }
                self.request_redraw();
                return true;
            }
            Key::Named(NamedKey::PageDown) => {
                if let Some(ref mut ed) = self.md_editor {
                    ed.move_cursor(15, 0, shift);
                    ed.ensure_cursor_visible(ew, view_h, sf, zoom);
                }
                self.request_redraw();
                return true;
            }
            Key::Named(NamedKey::Enter) => {
                if let Some(ref mut ed) = self.md_editor {
                    ed.insert_str("\n");
                    ed.ensure_cursor_visible(ew, view_h, sf, zoom);
                }
                self.sync_markdown_from_editor();
                return true;
            }
            Key::Named(NamedKey::Tab) => {
                if let Some(ref mut ed) = self.md_editor {
                    ed.insert_str("  ");
                    ed.ensure_cursor_visible(ew, view_h, sf, zoom);
                }
                self.sync_markdown_from_editor();
                return true;
            }
            Key::Named(NamedKey::Backspace) => {
                if let Some(ref mut ed) = self.md_editor {
                    ed.backspace();
                    ed.ensure_cursor_visible(ew, view_h, sf, zoom);
                }
                self.sync_markdown_from_editor();
                return true;
            }
            Key::Named(NamedKey::Delete) => {
                if let Some(ref mut ed) = self.md_editor {
                    ed.delete_forward();
                    ed.ensure_cursor_visible(ew, view_h, sf, zoom);
                }
                self.sync_markdown_from_editor();
                return true;
            }
            Key::Named(NamedKey::Space) => {
                if let Some(ref mut ed) = self.md_editor {
                    ed.insert_str(" ");
                    ed.ensure_cursor_visible(ew, view_h, sf, zoom);
                }
                self.sync_markdown_from_editor();
                return true;
            }
            _ => {
                if let Some(t) = text {
                    let clean: String = t.chars().filter(|c| !c.is_control()).collect();
                    if !clean.is_empty() {
                        if let Some(ref mut ed) = self.md_editor {
                            ed.insert_str(&clean);
                            ed.ensure_cursor_visible(ew, view_h, sf, zoom);
                        }
                        self.sync_markdown_from_editor();
                        return true;
                    }
                }
            }
        }
        false
    }

    pub(super) fn save_file_dialog(&mut self) {
        if self.is_markdown_tab() {
            self.save_active_markdown();
            return;
        }
        if self.doc.is_none() {
            return;
        }
        let stem = self
            .file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document");
        let default_name = format!("{stem}_edited.pdf");

        let mut dialog = rfd::FileDialog::new()
            .add_filter("PDF Document", &["pdf"])
            .set_file_name(&default_name);
        if let Some(parent) = self.file_path.parent() {
            if !parent.as_os_str().is_empty() && parent.exists() {
                dialog = dialog.set_directory(parent);
            }
        }
        let target_path = dialog.save_file();

        if let Some(save_path) = target_path {
            let _ = self.save_active_doc_to(save_path);
        }
    }

    /// Opens a Save As dialog so an annotated or edited PDF can be saved under a new name
    /// while leaving the original file completely untouched (`Ctrl/Cmd+Shift+S`).
    pub(super) fn save_as_file_dialog(&mut self) {
        if self.doc.is_none() {
            return;
        }
        let stem = self
            .file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document");
        let default_name = format!("{stem}_copy.pdf");
        let mut dialog = rfd::FileDialog::new()
            .set_title("Save PDF As (Original File Remains Intact)")
            .add_filter("PDF Document", &["pdf"])
            .set_file_name(&default_name);
        if let Some(parent) = self.file_path.parent() {
            if !parent.as_os_str().is_empty() && parent.exists() {
                dialog = dialog.set_directory(parent);
            }
        }
        if let Some(save_path) = dialog.save_file() {
            let orig_path = self.file_path.clone();
            if self.save_active_doc_to(save_path.clone()) && !same_file(&orig_path, &save_path) {
                let fname = save_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("copy.pdf");
                self.status_notice = Some(format!(
                    "Saved As '{fname}' — original file left intact"
                ));
                self.update_title_bar();
                self.request_redraw();
            }
        }
    }

    /// Opens a multi-file PDF picker to add files into the Interactive Combine Modal.
    pub(super) fn combine_add_files_dialog(&mut self) {
        let Some(paths) = rfd::FileDialog::new()
            .set_title("Add PDF Files to Combine")
            .add_filter("PDF Document", &["pdf"])
            .pick_files()
        else {
            return;
        };
        self.add_files_to_combine_modal(paths);
    }

    /// Exports the currently open PDF to a structured Markdown (`.md`) file using the pure-Rust extractor.
    pub(super) fn export_markdown_dialog(&mut self) {
        if self.doc.is_none() {
            self.status_notice = Some("Open a PDF first to export as Markdown".to_string());
            self.request_redraw();
            return;
        }
        let stem = self
            .file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document");
        let default_name = format!("{stem}.md");
        let mut dialog = rfd::FileDialog::new()
            .set_title("Export PDF as Markdown (.md)")
            .add_filter("Markdown", &["md"])
            .set_file_name(&default_name);
        if let Some(parent) = self.file_path.parent() {
            if !parent.as_os_str().is_empty() && parent.exists() {
                dialog = dialog.set_directory(parent);
            }
        }
        let Some(save_path) = dialog.save_file() else {
            return;
        };
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let md = crate::export::export_document_to_markdown(doc);
        match std::fs::write(&save_path, md.as_bytes()) {
            Ok(()) => {
                let fname = save_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("export.md");
                self.status_notice = Some(format!(
                    "Exported Markdown to '{fname}' ({} KB)",
                    (md.len() + 1023) / 1024
                ));
            }
            Err(e) => {
                self.status_notice = Some(format!("Markdown export failed: {e}"));
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    /// Exports the currently open PDF to a clean, compilable LaTeX (`.tex`) document.
    pub(super) fn export_latex_dialog(&mut self) {
        if self.doc.is_none() {
            self.status_notice = Some("Open a PDF first to export as LaTeX".to_string());
            self.request_redraw();
            return;
        }
        let stem = self
            .file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document")
            .to_string();
        let default_name = format!("{stem}.tex");
        let mut dialog = rfd::FileDialog::new()
            .set_title("Export PDF as LaTeX (.tex)")
            .add_filter("LaTeX Document", &["tex"])
            .set_file_name(&default_name);
        if let Some(parent) = self.file_path.parent() {
            if !parent.as_os_str().is_empty() && parent.exists() {
                dialog = dialog.set_directory(parent);
            }
        }
        let Some(save_path) = dialog.save_file() else {
            return;
        };
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let tex = crate::export::export_document_to_latex(doc, &stem);
        match std::fs::write(&save_path, tex.as_bytes()) {
            Ok(()) => {
                let fname = save_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("export.tex");
                self.status_notice = Some(format!(
                    "Exported LaTeX to '{fname}' ({} KB)",
                    (tex.len() + 1023) / 1024
                ));
            }
            Err(e) => {
                self.status_notice = Some(format!("LaTeX export failed: {e}"));
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    /// Exports the currently open PDF (or Markdown/LaTeX tab) to an EPUB 3 (`.epub`) e-book.
    pub(super) fn export_epub_dialog(&mut self) {
        if self.is_markdown_tab() {
            self.run_markdown_epub_export();
            return;
        }
        let Some(doc) = self.doc.as_ref() else {
            self.status_notice = Some("Open a PDF first to export as EPUB (.epub)".to_string());
            self.request_redraw();
            return;
        };
        let stem = self
            .file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document")
            .to_string();
        let default_name = format!("{stem}.epub");
        let mut dialog = rfd::FileDialog::new()
            .set_title("Export PDF as EPUB E-Book (.epub)")
            .add_filter("EPUB E-Book", &["epub"])
            .set_file_name(&default_name);
        if let Some(parent) = self.file_path.parent() {
            if !parent.as_os_str().is_empty() && parent.exists() {
                dialog = dialog.set_directory(parent);
            }
        }
        let Some(save_path) = dialog.save_file() else {
            return;
        };
        let md = crate::export::export_document_to_markdown(doc);
        let parsed = crate::markdown::parse_markdown(&md, &stem);
        match crate::epub_export::export_markdown_doc_to_epub(&parsed, &save_path) {
            Ok(()) => {
                let fname = save_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("export.epub");
                self.status_notice = Some(format!("Exported EPUB to '{fname}'"));
            }
            Err(e) => {
                self.status_notice = Some(format!("EPUB export failed: {e}"));
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    /// Exports the currently open PDF (or Markdown tab) to a native Microsoft Word (`.docx`) document using pure Rust.
    pub(super) fn export_docx_dialog(&mut self) {
        if self.is_markdown_tab() {
            self.run_markdown_docx_export();
            return;
        }
        if self.doc.is_none() {
            self.status_notice = Some("Open a PDF first to export to Word (.docx)".to_string());
            self.request_redraw();
            return;
        }
        let stem = self
            .file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document")
            .to_string();
        let default_name = format!("{stem}.docx");
        let mut dialog = rfd::FileDialog::new()
            .set_title("Export PDF as Word Document (.docx)")
            .add_filter("Word Document", &["docx"])
            .set_file_name(&default_name);
        if let Some(parent) = self.file_path.parent() {
            if !parent.as_os_str().is_empty() && parent.exists() {
                dialog = dialog.set_directory(parent);
            }
        }
        let Some(save_path) = dialog.save_file() else {
            return;
        };
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let bytes = crate::export::export_document_to_docx(doc, &stem);
        match std::fs::write(&save_path, &bytes) {
            Ok(()) => {
                let fname = save_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("export.docx");
                self.status_notice = Some(format!(
                    "Exported Word document to '{fname}' ({} KB)",
                    (bytes.len() + 1023) / 1024
                ));
            }
            Err(e) => {
                self.status_notice = Some(format!("Word (.docx) export failed: {e}"));
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    /// Exports tables and page text from the currently open PDF to an Excel (`.xlsx`) workbook using pure Rust.
    pub(super) fn export_xlsx_dialog(&mut self) {
        if self.doc.is_none() {
            self.status_notice = Some("Open a PDF first to export to Excel (.xlsx)".to_string());
            self.request_redraw();
            return;
        }
        let stem = self
            .file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document")
            .to_string();
        let default_name = format!("{stem}.xlsx");
        let mut dialog = rfd::FileDialog::new()
            .set_title("Export PDF as Excel Workbook (.xlsx)")
            .add_filter("Excel Workbook", &["xlsx"])
            .set_file_name(&default_name);
        if let Some(parent) = self.file_path.parent() {
            if !parent.as_os_str().is_empty() && parent.exists() {
                dialog = dialog.set_directory(parent);
            }
        }
        let Some(save_path) = dialog.save_file() else {
            return;
        };
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let bytes = crate::export::export_document_to_xlsx_with_options(doc, self.excel_export_opts);
        match std::fs::write(&save_path, &bytes) {
            Ok(()) => {
                let fname = save_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("export.xlsx");
                self.status_notice = Some(format!(
                    "Exported Excel workbook to '{fname}' ({} KB)",
                    (bytes.len() + 1023) / 1024
                ));
            }
            Err(e) => {
                self.status_notice = Some(format!("Excel (.xlsx) export failed: {e}"));
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    /// Saves a ready-to-edit sample bookmark `.txt` template (`Help -> Bookmark Import Format (.txt)`).
    pub(super) fn save_bookmark_sample_txt_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Save Sample Bookmark Import File (.txt)")
            .add_filter("Text File", &["txt"])
            .set_file_name("bookmarks_sample.txt");
        if let Some(parent) = self.file_path.parent() {
            if !parent.as_os_str().is_empty() && parent.exists() {
                dialog = dialog.set_directory(parent);
            }
        }
        let Some(save_path) = dialog.save_file() else {
            return;
        };
        match std::fs::write(&save_path, super::overlays::BOOKMARK_SAMPLE_TXT.as_bytes()) {
            Ok(()) => {
                let fname = save_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("bookmarks_sample.txt");
                self.show_bookmark_format_overlay = false;
                self.status_notice = Some(format!(
                    "Saved sample bookmark template to '{fname}' — edit and press L to import"
                ));
            }
            Err(e) => {
                self.status_notice = Some(format!("Failed to save sample .txt: {e}"));
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    /// Saves the active document to `save_path` (which may be the open file itself), then
    /// reopens it from there. Shows the outcome in the status bar.
    /// Saves the active document to `save_path` (which may be the open file itself), then
    /// reopens it from there. Shows the outcome in the status bar.
    ///
    /// The bytes go to a temp file next to `save_path` first, which then replaces `save_path` by
    /// rename, so the original is never rewritten in place. If the rename fails (FAT32/exFAT and
    /// some network shares refuse to replace a file that is open), the documents this app has
    /// open on `save_path` are closed and the rename is retried. If it still fails, the original
    /// is left untouched, the temp file stays next to it as the saved copy, and the active tab
    /// keeps working from that copy with its edits still marked unsaved.
    pub(super) fn save_active_doc_to(&mut self, save_path: std::path::PathBuf) -> bool {
        let Some(doc) = self.doc.as_ref() else {
            return false;
        };
        let serialized = PdfEngine::serialize_doc_with_metadata_and_skip(
            doc,
            self.skip_pages,
            self.roman_end,
            self.skip_pages > 0 || self.roman_end > 0,
            &self.bookmarks,
            self.redacted,
        );
        let temp = serialized.and_then(|(bytes, warning)| {
            PdfEngine::write_temp_next_to(&save_path, &bytes).map(|t| (t, warning))
        });
        let saved_ok = match temp {
            Err(e) => {
                self.status_notice = Some(format!("Save error: {e:#}"));
                false
            }
            Ok((temp, warning)) => match self.replace_with_saved_temp(&temp, &save_path) {
                Ok(()) => {
                    self.finish_successful_save(save_path, warning);
                    true
                }
                Err(e) => {
                    let fname = save_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    self.status_notice = Some(format!(
                        "Could not replace '{fname}' ({e}). The original is unchanged; your saved copy is {}",
                        temp.display()
                    ));
                    false
                }
            },
        };
        self.update_title_bar();
        self.request_redraw();
        saved_ok
    }

    /// Renames the saved `temp` file over `target`, closing and reopening this app's documents
    /// on `target` if the first attempt fails. On success the active document is reopened by
    /// `finish_successful_save`; on failure it is reopened from `temp` (it holds the edits).
    fn replace_with_saved_temp(&mut self, temp: &Path, target: &Path) -> std::io::Result<()> {
        let first_err = match (self.rename_for_save)(temp, target) {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };

        let active_holds = self.doc.is_some() && same_file(&self.file_path, target);
        // Other tabs on the same file are closed too, unless they have unsaved edits of their
        // own: those are never dropped (the retry may then fail and is reported).
        let other_tabs: Vec<usize> = (0..self.tabs.len())
            .filter(|&i| {
                i != self.active_tab
                    && self.tabs[i].doc.is_some()
                    && !self.tabs[i].has_unsaved_edits
                    && same_file(&self.tabs[i].file_path, target)
            })
            .collect();
        if !active_holds && other_tabs.is_empty() {
            return Err(first_err);
        }

        if active_holds {
            self.invalidate_page_caches();
            self.thumbnail_cache.clear();
            self.doc = None;
        }
        for &i in &other_tabs {
            self.tabs[i].doc = None;
        }

        let result = (self.rename_for_save)(temp, target);

        for &i in &other_tabs {
            self.tabs[i].doc = self.engine.load_pdf_safe(target).ok();
            if result.is_ok() {
                self.tabs[i].last_file_mtime = Self::read_file_mtime(target);
            }
        }
        if active_holds && result.is_err() {
            self.doc = self
                .engine
                .load_pdf_safe(temp)
                .or_else(|_| self.engine.load_pdf_safe(target))
                .ok();
        }
        result
    }

    fn finish_successful_save(&mut self, save_path: std::path::PathBuf, warning: Option<String>) {
        let mut reopen_error = None;
        match self.engine.load_pdf_safe(&save_path) {
            Ok(reloaded) => {
                self.doc = Some(reloaded);
                self.invalidate_page_caches();
            }
            // Keep the document already open, unless it was closed for the rename.
            Err(e) if self.doc.is_none() => reopen_error = Some(e),
            Err(_) => {}
        }
        let saved_mtime = Self::read_file_mtime(&save_path);
        self.record_recent_file(&save_path);
        self.file_path = save_path.clone();
        self.saved_undo_generation = self.undo_generation;
        self.has_unsaved_edits = false;
        self.last_file_mtime = saved_mtime;
        if self.active_tab < self.tabs.len() {
            self.tabs[self.active_tab].file_path = save_path;
            self.tabs[self.active_tab].saved_undo_generation = self.saved_undo_generation;
            self.tabs[self.active_tab].has_unsaved_edits = false;
            self.tabs[self.active_tab].last_file_mtime = saved_mtime;
        }
        self.persist_bookmarks();
        self.status_notice = Some(match (reopen_error, warning) {
            (Some(e), _) => format!("Saved PDF, but could not reopen it: {e:#}"),
            (None, None) if self.redacted => {
                "Saved PDF (redaction finalized: orphaned objects, /Info & thumbnails removed)".to_string()
            }
            (None, None) => "Saved PDF (sanitized metadata + embedded /PageLabels & /Outlines)".to_string(),
            (None, Some(w)) => format!("Saved PDF, but WARNING: {w}"),
        });
    }

    fn pick_images_for_pdf_dialog(&mut self) {
        if let Some(paths) = rfd::FileDialog::new()
            .set_title("Select Images to Convert into PDF")
            .add_filter(
                "Images (PNG, JPG, BMP, WebP, GIF, TIFF)",
                &["png", "jpg", "jpeg", "bmp", "webp", "gif", "tif", "tiff"],
            )
            .pick_files()
        {
            self.add_images_to_create_modal(paths);
        }
    }

    fn export_pages_as_images_dir_dialog(&mut self) {
        let Some(st) = self.export_images_modal.clone() else {
            return;
        };
        let Ok(pages) = st.resolved_pages(self.current_page_index, self.total_pages()) else {
            return;
        };
        let Some(dir) = rfd::FileDialog::new()
            .set_title("Choose Destination Folder for Exported Page Images")
            .pick_folder()
        else {
            return;
        };
        let stem = self
            .file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document")
            .to_string();
        self.export_images_modal = None;
        if let Some(doc) = self.doc.as_ref() {
            match PdfEngine::export_pages_as_images(doc, &pages, st.dpi, st.format, &dir, &stem) {
                Ok(n) => {
                    self.status_notice = Some(format!(
                        "Exported {n} page(s) as {} ({} DPI) to '{}'",
                        st.format.extension().to_uppercase(),
                        st.dpi,
                        dir.display()
                    ));
                }
                Err(e) => {
                    self.status_notice = Some(format!("Image export failed: {e}"));
                }
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    fn extract_selected_pages_dialog(&mut self, pages: &[u16]) {
        if pages.is_empty() || self.doc.is_none() {
            return;
        }
        let stem = self
            .file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document");
        let default_name = format!("{stem}_extracted.pdf");
        let mut dlg = rfd::FileDialog::new()
            .set_title(format!("Extract {} Selected Page(s) to New PDF", pages.len()))
            .add_filter("PDF Document", &["pdf"])
            .set_file_name(&default_name);
        if let Some(parent) = self.file_path.parent().filter(|p| !p.as_os_str().is_empty()) {
            dlg = dlg.set_directory(parent);
        }
        let Some(out_path) = dlg.save_file() else {
            return;
        };
        if let Some(doc) = self.doc.as_ref() {
            let one_based: Vec<u16> = pages.iter().map(|&p| p + 1).collect();
            let spec: String = one_based
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join(",");
            match self.engine.extract_page_spec_from_doc(doc, &spec, &out_path) {
                Ok(n) => {
                    let fname = out_path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("extracted.pdf");
                    self.status_notice = Some(format!("Extracted {n} selected page(s) to '{fname}'"));
                }
                Err(e) => {
                    self.status_notice = Some(format!("Extract selected pages failed: {e}"));
                }
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    fn export_bookmarks_txt_dialog(&mut self) {
        if self.bookmarks.is_empty() {
            self.status_notice = Some("No bookmarks to export".to_string());
            self.request_redraw();
            return;
        }
        let stem = self
            .file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document");
        let default_name = format!("{stem}_bookmarks.txt");
        let mut dlg = rfd::FileDialog::new()
            .set_title("Export Bookmarks as .txt")
            .add_filter("Text File", &["txt"])
            .set_file_name(&default_name);
        if let Some(parent) = self.file_path.parent().filter(|p| !p.as_os_str().is_empty()) {
            dlg = dlg.set_directory(parent);
        }
        if let Some(path) = dlg.save_file() {
            self.export_bookmarks_to_path(&path);
        }
    }

    /// Opens the interactive Optimize menu in the bottom command bar (`Tools -> Optimize...`).
    pub(super) fn open_optimize_prompt(&mut self) {
        if self.doc.is_none() {
            return;
        }
        self.input_mode = InputMode::OptimizePrompt {
            preset: crate::optimize::OptimizePreset::Balanced,
            mode: crate::optimize::ColorMode::Color,
            clean_scans: true,
            previewing: false,
        };
        self.status_notice = None;
        self.update_title_bar();
        self.request_redraw();
    }

    /// `Z`: optimize images (Balanced preset: 200 dpi, JPEG 70, scan cleanup) and write
    /// `<name>_optimized.pdf` next to the original. The open document and the original file
    /// are never modified. Nothing is written when optimizing would not make the file smaller.
    pub(super) fn compress_current_pdf(&mut self) {
        self.optimize_current_pdf(crate::optimize::OptimizeOptions::default());
    }

    pub(super) fn optimize_current_pdf(&mut self, opts: crate::optimize::OptimizeOptions) {
        self.start_optimize_job(opts);
        if self.window.is_none() {
            while self.optimize_job.is_some() {
                self.step_optimize_job();
            }
        }
    }

    /// Starts an incremental `OptimizeJob` to be stepped one image per event-loop iteration.
    pub(super) fn start_optimize_job(&mut self, opts: crate::optimize::OptimizeOptions) {
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let stem = self
            .file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document")
            .to_string();
        let dir = self
            .file_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        let mut out_name = format!("{stem}_optimized.pdf");
        let mut out_path = dir.join(&out_name);
        let mut n = 2;
        while out_path == self.file_path {
            out_name = format!("{stem}_optimized-{n}.pdf");
            out_path = dir.join(&out_name);
            n += 1;
        }

        let disk_size = std::fs::metadata(&self.file_path)
            .map(|m| m.len() as usize)
            .unwrap_or(0);

        let raw_bytes = match doc.save_to_bytes() {
            Ok(b) => b,
            Err(e) => {
                self.status_notice = Some(format!("Optimize error: {e:#}"));
                self.update_title_bar();
                self.request_redraw();
                return;
            }
        };

        let job = match crate::optimize::OptimizeJob::new(raw_bytes, opts) {
            Ok(j) => j,
            Err(e) => {
                self.status_notice = Some(format!("Optimize error: {e:#}"));
                self.update_title_bar();
                self.request_redraw();
                return;
            }
        };

        let total = job.total();
        let is_done = job.is_done();
        self.optimize_job = Some(ActiveOptimizeJob {
            job,
            out_path,
            out_name,
            disk_size,
            skip_pages: self.skip_pages,
            roman_end: self.roman_end,
            bookmarks: self.bookmarks.clone(),
            redacted: self.redacted,
        });

        if is_done {
            self.finish_active_optimize_job();
        } else {
            self.status_notice = Some(format!(
                "Optimizing image 0 / {} (Esc to cancel)",
                total
            ));
            self.update_title_bar();
            self.request_redraw();
        }
    }

    /// Runs a single `OptimizeJob::step()` per event-loop iteration. Returns `true` if more steps remain.
    pub(super) fn step_optimize_job(&mut self) -> bool {
        let Some(active) = self.optimize_job.as_mut() else {
            return false;
        };
        if !active.job.is_done() {
            active.job.step();
        }
        let processed = active.job.processed();
        let total = active.job.total();
        let done = active.job.is_done();
        if !done {
            self.status_notice = Some(format!(
                "Optimizing image {} / {} (Esc to cancel)",
                processed, total
            ));
            self.update_title_bar();
            self.request_redraw();
            true
        } else {
            self.finish_active_optimize_job();
            false
        }
    }

    /// Cancels an active `OptimizeJob` (`Esc` key), dropping the job and writing nothing.
    pub(super) fn cancel_optimize_job(&mut self) {
        if let Some(active) = self.optimize_job.take() {
            self.status_notice = Some(format!(
                "Optimize cancelled at image {} / {} (nothing written)",
                active.job.processed(),
                active.job.total()
            ));
            self.update_title_bar();
            self.request_redraw();
        }
    }

    pub(super) fn finish_active_optimize_job(&mut self) {
        let Some(active) = self.optimize_job.take() else {
            return;
        };
        let report = match active.job.finish() {
            Ok(r) => r,
            Err(e) => {
                self.status_notice = Some(format!("Optimize error: {e:#}"));
                self.update_title_bar();
                self.request_redraw();
                return;
            }
        };
        let orig_len = active.disk_size.max(report.original_len);
        if report.kept_original {
            self.status_notice = Some(if report.images_seen == 0 && report.deduped_streams == 0 {
                "Optimize: PDF is already compact (text/vector only — no images to compress); nothing written".to_string()
            } else {
                format!(
                    "Optimize: already compact ({} image(s) checked, {} stream(s) deduped — no smaller output); nothing written",
                    report.images_seen, report.deduped_streams
                )
            });
            self.update_title_bar();
            self.request_redraw();
            return;
        }

        let (final_bytes, warning) = match PdfEngine::embed_metadata_in_pdf_bytes_with_skip(
            &report.bytes,
            active.skip_pages,
            active.roman_end,
            active.skip_pages > 0 || active.roman_end > 0,
            &active.bookmarks,
            active.redacted,
        ) {
            Ok(b) => (b, None),
            Err(e) if active.redacted => {
                self.status_notice = Some(format!(
                    "Optimize refused: post-redaction cleanup failed ({e}). Nothing was written."
                ));
                self.update_title_bar();
                self.request_redraw();
                return;
            }
            Err(e) => (
                report.bytes,
                Some(format!(" (WARNING: metadata not sanitized: {e})")),
            ),
        };
        let new_len = final_bytes.len();
        let fmt_size = |bytes: usize| -> String {
            if bytes >= 1024 * 1024 {
                format!("{:.2} MB", (bytes as f64) / (1024.0 * 1024.0))
            } else {
                format!("{:.0} KB", ((bytes as f64) / 1024.0).max(1.0))
            }
        };
        match PdfEngine::write_atomic(&active.out_path, &final_bytes) {
            Ok(()) => {
                let saved_pct = if orig_len > 0 && new_len < orig_len {
                    (((orig_len - new_len) as f64 / (orig_len as f64)) * 100.0).round() as u32
                } else {
                    0
                };
                let dedup_part = if report.deduped_streams > 0 {
                    format!(", {} deduped", report.deduped_streams)
                } else {
                    String::new()
                };
                let skip_part = if report.images_skipped > 0 {
                    format!(", {} kept", report.images_skipped)
                } else {
                    String::new()
                };
                self.status_notice = Some(format!(
                    "Optimized {} -> {} ({}% smaller; {} image(s) recompressed{}{}) -> '{}'{}",
                    fmt_size(orig_len),
                    fmt_size(new_len),
                    saved_pct,
                    report.images_rewritten,
                    skip_part,
                    dedup_part,
                    active.out_name,
                    warning.unwrap_or_default()
                ));
            }
            Err(e) => {
                self.status_notice = Some(format!("Failed to write '{}': {e}", active.out_name));
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    /// Renders a transient 1-page preview of the current page with the selected `OptimizeOptions`
    /// directly into `self.cached_page` and drops all temporary buffers immediately (zero permanent RAM).
    pub(super) fn render_optimize_preview_current_page(
        &mut self,
        preset: crate::optimize::OptimizePreset,
        mode: crate::optimize::ColorMode,
        clean_scans: bool,
    ) {
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let total = doc.pages().len();
        if total == 0 {
            return;
        }
        let page_idx = self.current_page_index.min(total - 1);

        let mut opts = crate::optimize::OptimizeOptions::preset(preset);
        opts.mode = mode;
        opts.clean_scans = clean_scans;

        let preview_bytes_res: anyhow::Result<Vec<u8>> = (|| {
            let mut temp_doc = self.engine.pdfium.create_new_pdf()?;
            temp_doc
                .pages_mut()
                .copy_page_from_document(doc, page_idx, 0)?;
            let raw_bytes = temp_doc.save_to_bytes()?;
            drop(temp_doc);

            let mut job = crate::optimize::OptimizeJob::new(raw_bytes.clone(), opts)?;
            while !job.is_done() {
                job.step();
            }
            let report = job.finish()?;
            if report.kept_original || report.bytes.is_empty() {
                Ok(raw_bytes)
            } else {
                Ok(report.bytes)
            }
        })();

        let preview_bytes = match preview_bytes_res {
            Ok(b) => b,
            Err(e) => {
                self.status_notice = Some(format!("Preview error: {e}"));
                return;
            }
        };

        let Ok(opt_doc) = self.engine.load_pdf_from_bytes_safe(preview_bytes) else {
            return;
        };

        let (win_w, win_h, sidebar_w, top_bars_h, status_bar_h) =
            if let Some(w) = self.window.as_ref() {
                let sz = w.inner_size();
                let sf = self.effective_scale_factor(w.scale_factor());
                (
                    sz.width.max(1),
                    sz.height.max(1),
                    self.sidebar_width_px(sf).min(sz.width.saturating_sub(64)),
                    self.top_bars_height_px(sf).min(sz.height.saturating_sub(32)),
                    self.status_bar_height_px(sf),
                )
            } else {
                (
                    1024,
                    1080,
                    0,
                    MENU_BAR_HEIGHT_LOGICAL + TAB_BAR_HEIGHT_LOGICAL,
                    STATUS_BAR_HEIGHT_LOGICAL,
                )
            };

        if !matches!(
            self.view_mode,
            ViewMode::FitPage | ViewMode::FitWidth | ViewMode::FitTextWidth
        ) {
            self.view_mode = ViewMode::FitPage;
        }
        let orig_doc = self.doc.replace(opt_doc);
        let orig_page = self.current_page_index;
        self.current_page_index = 0;
        self.invalidate_page_caches();
        let _ = self.ensure_current_page_rendered(
            win_w,
            win_h,
            sidebar_w,
            top_bars_h,
            status_bar_h,
        );
        if let Some(c) = self.cached_page.as_mut() {
            c.page_index = orig_page;
        }
        self.current_page_index = orig_page;
        self.doc = orig_doc;
    }

    /// Triggers Print Preview (`Ctrl/Cmd+P` or `File -> Print...`) of the current in-memory document.
    pub(super) fn trigger_print(&mut self) {
        if self.is_markdown_tab() {
            self.open_markdown_export_modal();
            return;
        }
        self.open_print_preview();
    }

    fn print_progress_notice(job: &crate::print::PagedPrintJob) -> String {
        let (sent, total) = job.progress();
        format!("Printing page {} / {total} (Esc to cancel)", (sent + 1).min(total))
    }

    /// Starts sending `job` to the printer, one page per event-loop iteration.
    #[cfg_attr(not(any(windows, test)), allow(dead_code))]
    pub(super) fn start_print_job(&mut self, job: crate::print::PagedPrintJob<'a>, title: String) {
        self.status_notice = Some(Self::print_progress_notice(&job));
        self.print_job = Some(ActivePrintJob { job, title });
        self.update_title_bar();
        self.request_redraw();
    }

    /// Renders and sends one page of the active print job. Returns `true` while pages remain.
    pub(super) fn step_print_job(&mut self) -> bool {
        let Some(active) = self.print_job.as_mut() else {
            return false;
        };
        let result = active.job.step(self.engine);
        let running = matches!(result, Ok(true));
        self.status_notice = Some(match result {
            Ok(true) => Self::print_progress_notice(&active.job),
            Ok(false) => format!(
                "Printed {} page(s) x {} copy/copies of '{}'",
                active.job.pages_per_copy(),
                active.job.copies(),
                active.title
            ),
            Err(e) => format!("Print error: {e:#} (printing aborted)"),
        });
        if !running {
            self.print_job = None;
        }
        self.update_title_bar();
        self.request_redraw();
        running
    }

    /// Cancels the active print job (`Esc`): the printer document is aborted (`AbortDoc`).
    pub(super) fn cancel_print_job(&mut self) {
        if let Some(mut active) = self.print_job.take() {
            let (sent, total) = active.job.progress();
            active.job.cancel();
            self.status_notice = Some(format!(
                "Printing cancelled after {sent} of {total} page(s)"
            ));
            self.update_title_bar();
            self.request_redraw();
        }
    }

    /// Checks the background `lp` submission without waiting. Returns `true` while it runs.
    pub(super) fn poll_lp_job(&mut self) -> bool {
        let Some(active) = self.lp_job.as_mut() else {
            return false;
        };
        let Some(result) = active.job.poll() else {
            return true;
        };
        let printer = active.printer.clone();
        self.lp_job = None;
        self.status_notice = Some(match result {
            Ok(out) => {
                let suffix = if out.job_info.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", out.job_info)
                };
                format!(
                    "Sent {} page(s) x {} copy/copies to printer '{printer}'{suffix}",
                    out.pages, out.copies
                )
            }
            Err(e) => format!("Print error: {e:#}"),
        });
        self.update_title_bar();
        self.request_redraw();
        false
    }

    pub(super) fn parse_page_range_input(&self, input: &str) -> Option<(u16, u16)> {
        let total = self.total_pages();
        if total == 0 {
            return None;
        }
        let s = input.trim();
        if s.is_empty() || s.eq_ignore_ascii_case("all") {
            return Some((1, total));
        }
        if let Some((a, b)) = s.split_once('-') {
            let start = a.trim().parse::<u16>().ok()?;
            let end = b.trim().parse::<u16>().ok()?;
            if start >= 1 && start <= end && end <= total {
                return Some((start, end));
            }
            return None;
        }
        let single = s.parse::<u16>().ok()?;
        if single >= 1 && single <= total {
            Some((single, single))
        } else {
            None
        }
    }

    pub(super) fn handle_prompt_submission(&mut self) {
        let mode = std::mem::replace(&mut self.input_mode, InputMode::Normal);
        match mode {
            InputMode::Normal => {}
            InputMode::SearchText { query_input } => {
                self.execute_text_search(&query_input);
                return;
            }
            InputMode::JumpToPage { input } => {
                let total = self.total_pages() as usize;
                if let Some(target_idx) = PdfEngine::resolve_logical_page_str_with_skip(
                    &input,
                    self.skip_pages,
                    self.roman_end,
                    total,
                ) {
                    self.push_nav_history();
                    self.current_page_index = target_idx as u16;
                    self.scroll_y = 0.0;
                    self.invalidate_page_caches();
                    let lbl = PdfEngine::format_logical_page_label_with_skip(
                        target_idx,
                        self.skip_pages,
                        self.roman_end,
                    );
                    self.status_notice = Some(format!(
                        "Jumped to Page {} (Physical {} of {})",
                        lbl,
                        target_idx + 1,
                        total
                    ));
                    self.request_redraw();
                } else {
                    self.status_notice = Some(format!(
                        "Invalid page '{}' (use logical#, Roman 'i..v', or '#physical')",
                        input.trim()
                    ));
                }
            }
            InputMode::PageNumbers { input } => {
                let trimmed = input.trim();
                let roman_end = if trimmed.is_empty() {
                    0
                } else {
                    match trimmed.parse::<u16>() {
                        Ok(n) => n,
                        Err(_) => {
                            self.status_notice =
                                Some("Invalid number for Roman page end".to_string());
                            self.update_title_bar();
                            self.request_redraw();
                            return;
                        }
                    }
                };
                self.push_undo_snapshot();
                let total = self.total_pages() as usize;
                if let Some(doc) = self.doc.as_mut() {
                    match PdfEngine::apply_page_numbers_to_doc(doc, roman_end) {
                        Ok(()) => {
                            self.roman_end = roman_end as usize;
                            PdfEngine::resync_bookmarks_logical_pages_with_skip(
                                &mut self.bookmarks,
                                self.skip_pages,
                                self.roman_end,
                                total,
                            );
                            self.persist_bookmarks();
                            self.invalidate_page_caches();
                            self.thumbnail_cache.clear();
                            self.status_notice = Some(format!(
                                "Stamped page numbers (Roman 1..={roman_end}) & synced bookmarks"
                            ));
                            self.request_redraw();
                        }
                        Err(e) => {
                            self.status_notice = Some(format!("Page numbering failed: {e}"));
                        }
                    }
                }
            }
            InputMode::HeaderFooterRange { range_input } => {
                if let Some((start_page, end_page)) = self.parse_page_range_input(&range_input) {
                    self.input_mode = InputMode::HeaderFooterPosition {
                        start_page,
                        end_page,
                        pos_input: String::new(),
                    };
                } else {
                    self.status_notice = Some(format!(
                        "Invalid page range '{}' (use e.g. '1-5' or 'all')",
                        range_input.trim()
                    ));
                }
            }
            InputMode::HeaderFooterPosition {
                start_page,
                end_page,
                pos_input,
            } => {
                if let Some(position) = MarginPosition::parse(&pos_input) {
                    self.input_mode = InputMode::HeaderFooterText {
                        start_page,
                        end_page,
                        position,
                        text_input: String::new(),
                    };
                } else {
                    self.status_notice = Some(format!(
                        "Invalid position '{}' (use HL, HC, HR, FL, FC, or FR)",
                        pos_input.trim()
                    ));
                }
            }
            InputMode::HeaderFooterText {
                start_page,
                end_page,
                position,
                text_input,
            } => {
                let raw = text_input.trim();
                self.push_undo_snapshot();
                if let Some(doc) = self.doc.as_mut() {
                    match PdfEngine::apply_aligned_header_footer_to_doc(
                        doc, position, raw, start_page, end_page,
                    ) {
                        Ok(()) => {
                            self.invalidate_page_caches();
                            self.thumbnail_cache.clear();
                            if raw.is_empty() {
                                self.status_notice = Some(format!(
                                    "Cleared margin zone on pages {start_page}..={end_page}"
                                ));
                            } else {
                                self.status_notice = Some(format!(
                                    "Stamped {:?} on pages {start_page}..={end_page}",
                                    position
                                ));
                            }
                            self.request_redraw();
                        }
                        Err(e) => {
                            self.status_notice = Some(format!("Header/Footer failed: {e}"));
                        }
                    }
                }
            }
            InputMode::ClearMargins { input } => {
                let mut parts = input.split_whitespace();
                let choice = parts.next().unwrap_or("");
                let range_str = parts.next().unwrap_or("all");
                if let Some((start_p, end_p)) = self.parse_page_range_input(range_str) {
                    self.push_undo_snapshot();
                    if let Some(doc) = self.doc.as_mut() {
                        let res = match choice {
                            "1" => PdfEngine::clear_margin_zone_in_doc(
                                doc,
                                MarginZone::Footer,
                                start_p,
                                end_p,
                            ),
                            "2" => PdfEngine::clear_margin_zone_in_doc(
                                doc,
                                MarginZone::Header,
                                start_p,
                                end_p,
                            ),
                            "3" => PdfEngine::clear_margin_zone_in_doc(
                                doc,
                                MarginZone::Header,
                                start_p,
                                end_p,
                            )
                            .and_then(|()| {
                                PdfEngine::clear_margin_zone_in_doc(
                                    doc,
                                    MarginZone::Footer,
                                    start_p,
                                    end_p,
                                )
                            }),
                            _ => Err(anyhow::anyhow!(
                                "Enter '1' (Footers/Page#), '2' (Headers), or '3' (Both)"
                            )),
                        };
                        match res {
                            Ok(()) => {
                                self.invalidate_page_caches();
                                self.thumbnail_cache.clear();
                                self.status_notice = Some(format!(
                                    "Cleared margins (option {choice}) on pages {start_p}..={end_p}"
                                ));
                                self.request_redraw();
                            }
                            Err(e) => {
                                self.status_notice = Some(format!("{e}"));
                            }
                        }
                    }
                } else {
                    self.status_notice = Some("Invalid page range for Clear Margins".to_string());
                }
            }
            InputMode::Watermark { input } => {
                let text = input.trim();
                if !text.is_empty() {
                    self.push_undo_snapshot();
                    if let Some(doc) = self.doc.as_mut() {
                        match PdfEngine::apply_watermark_to_doc(doc, text, true) {
                            Ok(()) => {
                                self.invalidate_page_caches();
                                self.thumbnail_cache.clear();
                                self.status_notice = Some(format!("Stamped watermark '{text}'"));
                                self.request_redraw();
                            }
                            Err(e) => {
                                self.status_notice = Some(format!("Watermark failed: {e}"));
                            }
                        }
                    }
                }
            }
            InputMode::SplitRange { input } => {
                self.pending_dialog = Some(PendingDialog::Split(input));
            }
            InputMode::AddBookmark { title_input } => {
                let trimmed = title_input.trim();
                let title = if trimmed.is_empty() {
                    format!("Page {}", self.current_page_index + 1)
                } else {
                    trimmed.to_string()
                };
                let page_idx = self.current_page_index as usize;
                let logical_page = PdfEngine::format_logical_page_label_with_skip(
                    page_idx,
                    self.skip_pages,
                    self.roman_end,
                );
                let next_serial = (self
                    .bookmarks
                    .iter()
                    .filter(|b| b.level == 0)
                    .count()
                    + 1)
                .to_string();
                self.push_metadata_undo_snapshot();
                self.bookmarks.push(Bookmark {
                    serial: next_serial,
                    title: title.clone(),
                    logical_page,
                    page_index: page_idx,
                    level: 0,
                });
                self.selected_bookmark = Some(self.bookmarks.len() - 1);
                self.show_bookmarks = true;
                self.persist_bookmarks();
                self.invalidate_page_caches();
                self.status_notice = Some(format!("Added bookmark '{title}' (press S to save into PDF)"));
                self.request_redraw();
            }
            InputMode::EditBookmark {
                bookmark_idx,
                title_input,
            } => {
                let trimmed = title_input.trim();
                if !trimmed.is_empty() && bookmark_idx < self.bookmarks.len() {
                    self.push_metadata_undo_snapshot();
                    if let Some(bm) = self.bookmarks.get_mut(bookmark_idx) {
                        bm.title = trimmed.to_string();
                        self.persist_bookmarks();
                        self.status_notice = Some("Updated bookmark title (press S to save)".to_string());
                        self.request_redraw();
                    }
                }
            }
            InputMode::ConfirmExternalUrl { url } => {
                match Self::open_external_url_in_browser(&url) {
                    Ok(()) => {
                        self.status_notice =
                            Some(format!("Opened external URL in browser: {url}"));
                    }
                    Err(e) => {
                        self.status_notice = Some(format!("Failed to open URL: {e}"));
                    }
                }
            }
            InputMode::PasswordPrompt {
                path,
                mut password_input,
                attempts_left,
                open_in_new_tab,
            } => {
                let fname = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("document.pdf")
                    .to_string();
                let res = if open_in_new_tab && !self.tabs.is_empty() {
                    self.open_document_in_new_tab_with_password(path.clone(), Some(&password_input))
                } else {
                    self.load_document_with_password(path.clone(), Some(&password_input))
                };
                // Never store the password after attempting to open the PDF
                password_input.clear();
                password_input.shrink_to_fit();

                match res {
                    Ok(()) => {
                        self.status_notice = Some(format!("Unlocked and opened '{fname}'"));
                    }
                    Err(e) if PdfEngine::is_password_error(&e) => {
                        let rem = attempts_left.saturating_sub(1);
                        if rem > 0 {
                            self.input_mode = InputMode::PasswordPrompt {
                                path,
                                password_input: String::new(),
                                attempts_left: rem,
                                open_in_new_tab,
                            };
                            self.status_notice = Some(format!(
                                "Incorrect password for '{fname}' ({rem} attempt(s) left)"
                            ));
                        } else {
                            self.status_notice = Some(format!(
                                "Failed to open '{fname}': maximum 3 password attempts exceeded"
                            ));
                        }
                    }
                    Err(e) => {
                        self.status_notice = Some(format!("ERROR opening '{fname}': {e}"));
                    }
                }
            }
            InputMode::RestoreSessionPrompt { .. } => {
                self.restore_pending_session();
                return;
            }
            InputMode::OptimizePrompt {
                preset,
                mode,
                clean_scans,
                previewing,
            } => {
                if previewing {
                    self.invalidate_page_caches();
                }
                let mut opts = crate::optimize::OptimizeOptions::preset(preset);
                opts.mode = mode;
                opts.clean_scans = clean_scans;
                self.start_optimize_job(opts);
                return;
            }
            InputMode::PrintPrompt {
                printers,
                selected_printer,
                copies,
                range_input,
                fit_to_page,
            } => {
                let Some(printer) = printers.get(selected_printer).cloned() else {
                    self.status_notice = Some(
                        "Print failed: no CUPS printers found via 'lpstat -p -d'".to_string(),
                    );
                    self.update_title_bar();
                    self.request_redraw();
                    return;
                };
                let title = self.filename_display();
                let Some(doc) = self.doc.as_ref() else {
                    return;
                };
                let req = crate::print::CupsPrintRequest {
                    printer: &printer,
                    copies,
                    range_input: &range_input,
                    fit_to_page,
                    title: &title,
                };
                match crate::print::start_cups_print_with(
                    self.engine,
                    doc,
                    &req,
                    self.redacted,
                    &self.lp_program,
                    &std::env::temp_dir(),
                ) {
                    Ok(job) => {
                        self.status_notice =
                            Some(format!("Sending to printer '{printer}'..."));
                        self.lp_job = Some(ActiveLpJob { job, printer });
                    }
                    Err(e) => {
                        self.status_notice = Some(format!("Print error: {e:#}"));
                    }
                }
            }
            InputMode::StickyNotePrompt {
                page_idx,
                pdf_x,
                pdf_y,
                text_input,
            } => {
                let note = text_input.trim().to_string();
                if !note.is_empty() {
                    let color = self.annotation_color;
                    self.push_undo_snapshot();
                    if let Some(doc) = self.doc.as_mut() {
                        match PdfEngine::add_sticky_note(doc, page_idx, pdf_x, pdf_y, &note, color)
                        {
                            Ok(()) => {
                                self.has_unsaved_edits = true;
                                self.invalidate_page_caches();
                                self.thumbnail_cache.remove(&page_idx);
                                self.status_notice = Some(format!(
                                    "Added sticky note on Page {} — click note in Select mode to read",
                                    page_idx + 1
                                ));
                            }
                            Err(e) => {
                                let _ = self.undo_stack.pop_back();
                                self.status_notice = Some(format!("Sticky note error: {e}"));
                            }
                        }
                    }
                }
            }
            InputMode::FreeTextPrompt {
                page_idx,
                pdf_x,
                pdf_y,
                text_input,
            } => {
                let text = text_input.trim().to_string();
                if !text.is_empty() {
                    let color = self.annotation_color;
                    self.push_undo_snapshot();
                    if let Some(doc) = self.doc.as_mut() {
                        match PdfEngine::add_freetext_box(
                            doc, page_idx, pdf_x, pdf_y, &text, 12.0, color,
                        ) {
                            Ok(()) => {
                                self.has_unsaved_edits = true;
                                self.invalidate_page_caches();
                                self.thumbnail_cache.remove(&page_idx);
                                self.status_notice = Some(format!(
                                    "Added text annotation on Page {} (Ctrl/Cmd+Z to undo)",
                                    page_idx + 1
                                ));
                            }
                            Err(e) => {
                                let _ = self.undo_stack.pop_back();
                                self.status_notice = Some(format!("Text box error: {e}"));
                            }
                        }
                    }
                }
            }
            InputMode::EditAnnotationText {
                page_idx,
                annot_idx,
                label,
                text_input,
            } => {
                let text = text_input.trim().to_string();
                if !text.is_empty() {
                    let fallback = self.annotation_color;
                    self.push_undo_snapshot();
                    if let Some(doc) = self.doc.as_mut() {
                        match PdfEngine::update_annotation_contents(
                            doc, page_idx, annot_idx, &text, fallback,
                        ) {
                            Ok(()) => {
                                self.invalidate_page_caches();
                                self.thumbnail_cache.remove(&page_idx);
                                self.status_notice = Some(format!(
                                    "Updated {label} text on Page {} (Ctrl+Z to undo)",
                                    page_idx + 1
                                ));
                            }
                            Err(e) => {
                                let _ = self.undo_stack.pop_back();
                                self.status_notice = Some(format!("Edit annotation error: {e}"));
                            }
                        }
                    }
                }
            }
            InputMode::EditFormField {
                page_idx,
                annot_idx,
                field_name,
                value_input,
            } => {
                self.push_undo_snapshot();
                if let Some(doc) = self.doc.as_mut() {
                    match PdfEngine::set_form_text_value_at(doc, page_idx, annot_idx, &value_input) {
                        Ok(()) => {
                            self.invalidate_page_caches();
                            self.thumbnail_cache.remove(&page_idx);
                            self.status_notice = Some(format!(
                                "Updated form field '{field_name}' on Page {} (Ctrl+Z to undo, S to save)",
                                page_idx + 1
                            ));
                        }
                        Err(e) => {
                            let _ = self.undo_stack.pop_back();
                            self.status_notice = Some(format!("Form field update failed: {e}"));
                        }
                    }
                }
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    pub(super) fn pick_compare_pdf_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Choose Second PDF to Compare Side-by-Side")
            .add_filter("PDF Document", &["pdf"]);
        if let Some(parent) = self.file_path.parent() {
            if !parent.as_os_str().is_empty() && parent.exists() {
                dialog = dialog.set_directory(parent);
            }
        }
        if let Some(path) = dialog.pick_file() {
            self.open_compare_with_path(path);
        }
    }

    pub(super) fn handle_key_input(&mut self, logical_key: Key, text: Option<&str>) {
        // 0. Unsaved Changes confirmation dialog takes absolute highest keyboard priority
        if self.unsaved_changes_modal.is_some() {
            match logical_key {
                Key::Named(NamedKey::Escape) => {
                    self.handle_unsaved_modal_click(super::doc_props_modal::UnsavedChangesHit::Cancel);
                }
                Key::Named(NamedKey::Enter) => {
                    self.handle_unsaved_modal_click(super::doc_props_modal::UnsavedChangesHit::Save);
                }
                Key::Character(ref c) if c.eq_ignore_ascii_case("s") => {
                    self.handle_unsaved_modal_click(super::doc_props_modal::UnsavedChangesHit::Save);
                }
                Key::Character(ref c) if c.eq_ignore_ascii_case("d") => {
                    self.handle_unsaved_modal_click(super::doc_props_modal::UnsavedChangesHit::Discard);
                }
                _ => {}
            }
            return;
        }
        // 0a. Guided Manual Duplex modal
        if self.manual_duplex_wait.is_some() {
            match logical_key {
                Key::Named(NamedKey::Escape) => {
                    self.handle_manual_duplex_click(
                        super::print_preview::ManualDuplexHit::Cancel,
                    );
                }
                Key::Named(NamedKey::Enter) => {
                    self.handle_manual_duplex_click(
                        super::print_preview::ManualDuplexHit::PrintPass2,
                    );
                }
                _ => {}
            }
            return;
        }
        // 0a2. Document Properties modal
        if self.doc_props_modal.is_some() {
            if matches!(logical_key, Key::Named(NamedKey::Escape)) {
                self.doc_props_modal = None;
                self.request_redraw();
            }
            return;
        }
        // 0a3. Create PDF from Images modal
        if self.create_pdf_images_modal.is_some() {
            match logical_key {
                Key::Named(NamedKey::Escape) => {
                    self.create_pdf_images_modal = None;
                    self.request_redraw();
                }
                Key::Named(NamedKey::Enter) => {
                    self.handle_create_pdf_images_click(
                        super::image_tools_modal::CreatePdfImagesHit::CreateNow,
                    );
                }
                _ => {}
            }
            return;
        }
        // 0a4. Export Pages as Images modal
        if self.export_images_modal.is_some() {
            self.handle_export_images_key(&logical_key, text);
            return;
        }
        // 0a5. Export PDF to Excel options modal
        if self.excel_export_modal.is_some() {
            match logical_key {
                Key::Named(NamedKey::Escape) => {
                    self.handle_excel_export_click(super::excel_export_modal::ExcelExportHit::Cancel);
                }
                Key::Named(NamedKey::Enter) => {
                    self.handle_excel_export_click(super::excel_export_modal::ExcelExportHit::ExportNow);
                }
                _ => {}
            }
            return;
        }
        // 0a5b. Export Markdown modal
        if self.markdown_export_modal.is_some() {
            match logical_key {
                Key::Named(NamedKey::Escape) => {
                    self.handle_markdown_export_click(super::markdown_export_modal::MarkdownExportHit::Cancel);
                }
                Key::Named(NamedKey::Enter) => {
                    self.handle_markdown_export_click(super::markdown_export_modal::MarkdownExportHit::ExportPdf);
                }
                _ => {}
            }
            return;
        }
        // 0a6. Split to Single-Page PDFs modal
        if self.split_pages_modal.is_some() {
            self.handle_split_pages_key(&logical_key, text);
            return;
        }
        // 0a7. Detailed Watermark modal
        if self.watermark_modal.is_some() {
            self.handle_watermark_key(&logical_key, text);
            return;
        }
        // 0a8. Portable Stamp & Signature Manager modal
        if self.stamp_manager_modal.is_some() {
            self.handle_stamp_manager_key(&logical_key, text);
            return;
        }
        // 0b. Print Preview modal takes top keyboard priority
        if self.print_preview.is_some() {
            self.handle_print_preview_key(logical_key, text);
            return;
        }
        // 0b2. Deskew control bar focused text fields (Angle / Range)
        if let Some(ref mut bar) = self.deskew_bar {
            use super::deskew::DeskewTextFocus;
            if bar.focus != DeskewTextFocus::None {
                match logical_key {
                    Key::Named(NamedKey::Escape) => {
                        bar.focus = DeskewTextFocus::None;
                        bar.angle_input = format!("{:.1}", bar.angle_deg);
                        self.request_redraw();
                        return;
                    }
                    Key::Named(NamedKey::Enter) => {
                        if bar.focus == DeskewTextFocus::Angle {
                            if let Ok(v) = bar.angle_input.trim().parse::<f32>() {
                                bar.set_angle(v);
                            } else {
                                bar.angle_input = format!("{:.1}", bar.angle_deg);
                            }
                            bar.focus = DeskewTextFocus::None;
                            self.request_redraw();
                            return;
                        } else {
                            bar.focus = DeskewTextFocus::None;
                            self.apply_deskew_from_bar();
                            return;
                        }
                    }
                    Key::Named(NamedKey::Backspace) | Key::Named(NamedKey::Delete) => {
                        match bar.focus {
                            DeskewTextFocus::Angle => {
                                bar.angle_input.pop();
                                if let Ok(v) = bar.angle_input.trim().parse::<f32>() {
                                    let snapped = (v.clamp(-15.0, 15.0) * 10.0).round() / 10.0;
                                    bar.angle_deg = if snapped.abs() < 0.05 { 0.0 } else { snapped };
                                    bar.preview_cache = None;
                                }
                            }
                            DeskewTextFocus::Range => {
                                bar.range_input.pop();
                            }
                            DeskewTextFocus::None => {}
                        }
                        self.request_redraw();
                        return;
                    }
                    _ => {
                        if let Some(t) = text {
                            match bar.focus {
                                DeskewTextFocus::Angle => {
                                    for ch in t.chars() {
                                        if (ch.is_ascii_digit() || ch == '.' || ch == '-' || ch == '+')
                                            && bar.angle_input.len() < 7
                                        {
                                            bar.angle_input.push(ch);
                                        }
                                    }
                                    if let Ok(v) = bar.angle_input.trim().parse::<f32>() {
                                        let snapped = (v.clamp(-15.0, 15.0) * 10.0).round() / 10.0;
                                        bar.angle_deg = if snapped.abs() < 0.05 { 0.0 } else { snapped };
                                        bar.preview_cache = None;
                                    }
                                }
                                DeskewTextFocus::Range => {
                                    for ch in t.chars() {
                                        if (ch.is_ascii_digit() || ch == '-' || ch == ',' || ch == ' ')
                                            && bar.range_input.len() < 24
                                        {
                                            bar.range_input.push(ch);
                                        }
                                    }
                                }
                                DeskewTextFocus::None => {}
                            }
                            self.request_redraw();
                            return;
                        }
                    }
                }
            }
        }
        // 0c. Combine Multiple PDFs modal
        if self.combine_modal.is_some() {
            self.handle_combine_key(logical_key, text);
            return;
        }
        // 0d. Header & Footer visual modal
        if self.header_footer_modal.is_some() {
            use super::doc_tools_modal::{HeaderFooterHit, HfField};
            // Ctrl/Cmd + A selects all text in the focused field; Ctrl/Cmd + V pastes clipboard text
            if self.cmd_or_ctrl_pressed {
                if let Key::Character(ch) = &logical_key {
                    let s = ch.as_str();
                    if s.eq_ignore_ascii_case("a") {
                        if let Some(m) = self.header_footer_modal.as_mut() {
                            m.select_all = true;
                        }
                        self.request_redraw();
                        return;
                    } else if s.eq_ignore_ascii_case("v") {
                        if let Ok(clip) = Self::read_text_from_os_clipboard() {
                            let clean: String = clip
                                .chars()
                                .map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c })
                                .filter(|c| !c.is_control())
                                .collect();
                            if !clean.is_empty() {
                                if let Some(m) = self.header_footer_modal.as_mut() {
                                    if m.select_all {
                                        m.focused_text_mut().clear();
                                        m.select_all = false;
                                    }
                                    match m.focused_field {
                                        HfField::CustomRange => {
                                            m.all_pages = false;
                                            for c in clean.chars() {
                                                if c.is_ascii_digit() || c == '-' || c == ',' || c == ' ' {
                                                    m.custom_range.push(c);
                                                }
                                            }
                                        }
                                        HfField::ProfileName => {
                                            for c in clean.chars() {
                                                if c != '|' {
                                                    m.profile.name.push(c);
                                                }
                                            }
                                        }
                                        _ => {
                                            m.focused_text_mut().push_str(&clean);
                                        }
                                    }
                                    m.status_msg = None;
                                }
                                self.request_redraw();
                            }
                        }
                        return;
                    }
                }
            }
            match logical_key {
                Key::Named(NamedKey::Escape) => {
                    if self.header_footer_modal.as_ref().is_some_and(|m| m.font_dropdown.open) {
                        if let Some(m) = self.header_footer_modal.as_mut() {
                            m.font_dropdown.close();
                        }
                        self.request_redraw();
                    } else {
                        self.handle_header_footer_click(HeaderFooterHit::Cancel);
                    }
                }
                Key::Named(NamedKey::Enter) => {
                    self.handle_header_footer_click(HeaderFooterHit::Apply);
                }
                Key::Named(NamedKey::Tab) => {
                    const ORDER: [HfField; 8] = [
                        HfField::ProfileName,
                        HfField::CustomRange,
                        HfField::HeaderLeft,
                        HfField::HeaderCenter,
                        HfField::HeaderRight,
                        HfField::FooterLeft,
                        HfField::FooterCenter,
                        HfField::FooterRight,
                    ];
                    if let Some(m) = self.header_footer_modal.as_mut() {
                        let idx = ORDER.iter().position(|&f| f == m.focused_field).unwrap_or(0);
                        let next_idx = if self.shift_pressed {
                            (idx + ORDER.len() - 1) % ORDER.len()
                        } else {
                            (idx + 1) % ORDER.len()
                        };
                        m.focused_field = ORDER[next_idx];
                        m.select_all = false;
                        if m.focused_field == HfField::CustomRange {
                            m.all_pages = false;
                        }
                    }
                    self.request_redraw();
                }
                Key::Named(NamedKey::Delete) => {
                    if let Some(m) = self.header_footer_modal.as_mut() {
                        m.focused_text_mut().clear();
                        m.select_all = false;
                        m.status_msg = None;
                    }
                    self.request_redraw();
                }
                Key::Named(NamedKey::Backspace) => {
                    if let Some(m) = self.header_footer_modal.as_mut() {
                        if m.select_all || self.cmd_or_ctrl_pressed {
                            m.focused_text_mut().clear();
                            m.select_all = false;
                        } else {
                            m.focused_text_mut().pop();
                        }
                        m.status_msg = None;
                    }
                    self.request_redraw();
                }
                _ => {
                    if self.cmd_or_ctrl_pressed {
                        return;
                    }
                    let mut typed = String::new();
                    if let Some(t) = text {
                        for c in t.chars() {
                            if !c.is_control() {
                                typed.push(c);
                            }
                        }
                    } else if let Key::Character(ch) = &logical_key {
                        for c in ch.chars() {
                            if !c.is_control() {
                                typed.push(c);
                            }
                        }
                    } else if logical_key == Key::Named(NamedKey::Space) {
                        typed.push(' ');
                    }
                    if !typed.is_empty() {
                        if let Some(m) = self.header_footer_modal.as_mut() {
                            if m.select_all {
                                m.focused_text_mut().clear();
                                m.select_all = false;
                            }
                            match m.focused_field {
                                HfField::CustomRange => {
                                    m.all_pages = false;
                                    for c in typed.chars() {
                                        if c.is_ascii_digit() || c == '-' || c == ',' || c == ' ' {
                                            m.custom_range.push(c);
                                        }
                                    }
                                }
                                HfField::ProfileName => {
                                    for c in typed.chars() {
                                        if c != '|' && c != '\n' && c != '\r' {
                                            m.profile.name.push(c);
                                        }
                                    }
                                }
                                _ => {
                                    m.focused_text_mut().push_str(&typed);
                                }
                            }
                            m.status_msg = None;
                        }
                        self.request_redraw();
                    }
                }
            }
            return;
        }
        // 0e. Page Numbering visual modal
        if self.page_number_modal.is_some() {
            use super::doc_tools_modal::{PageNumberHit, PnNumericFocus};
            let total = self.total_pages().max(1);
            match logical_key {
                Key::Named(NamedKey::Escape) => {
                    if self.page_number_modal.as_ref().is_some_and(|m| m.font_dropdown.open) {
                        if let Some(m) = self.page_number_modal.as_mut() {
                            m.font_dropdown.close();
                        }
                        self.request_redraw();
                    } else {
                        self.handle_page_number_click(PageNumberHit::Cancel);
                    }
                }
                Key::Named(NamedKey::Enter) => {
                    self.handle_page_number_click(PageNumberHit::Apply);
                }
                Key::Named(NamedKey::ArrowLeft) => {
                    self.handle_page_number_click(PageNumberHit::PrevPage);
                }
                Key::Named(NamedKey::ArrowRight) => {
                    self.handle_page_number_click(PageNumberHit::NextPage);
                }
                Key::Named(NamedKey::Tab) => {
                    if let Some(m) = self.page_number_modal.as_mut() {
                        m.focused_numeric = Some(match m.focused_numeric {
                            Some(PnNumericFocus::SkipPages) => PnNumericFocus::RomanPages,
                            _ => PnNumericFocus::SkipPages,
                        });
                    }
                    self.request_redraw();
                }
                Key::Named(NamedKey::Backspace) => {
                    if let Some(m) = self.page_number_modal.as_mut() {
                        match m.focused_numeric {
                            Some(PnNumericFocus::SkipPages) => {
                                m.config.skip_pages /= 10;
                                m.status_msg = None;
                            }
                            Some(PnNumericFocus::RomanPages) => {
                                m.config.roman_end /= 10;
                                m.status_msg = None;
                            }
                            None => {}
                        }
                    }
                    self.request_redraw();
                }
                Key::Named(NamedKey::Delete) => {
                    if let Some(m) = self.page_number_modal.as_mut() {
                        match m.focused_numeric {
                            Some(PnNumericFocus::SkipPages) => {
                                m.config.skip_pages = 0;
                                m.status_msg = None;
                            }
                            Some(PnNumericFocus::RomanPages) => {
                                m.config.roman_end = 0;
                                m.status_msg = None;
                            }
                            None => {}
                        }
                    }
                    self.request_redraw();
                }
                _ => {
                    let digit_char = text
                        .and_then(|t| t.chars().find(|c| c.is_ascii_digit()))
                        .or_else(|| {
                            if let Key::Character(ch) = &logical_key {
                                ch.chars().find(|c| c.is_ascii_digit())
                            } else {
                                None
                            }
                        });
                    if let Some(d_ch) = digit_char {
                        let d = (d_ch as u8 - b'0') as u16;
                        if let Some(m) = self.page_number_modal.as_mut() {
                            match m.focused_numeric {
                                Some(PnNumericFocus::SkipPages) => {
                                    let max_skip = total.saturating_sub(1);
                                    let next = m.config.skip_pages.saturating_mul(10).saturating_add(d);
                                    m.config.skip_pages = next.min(max_skip);
                                    let max_rom = total.saturating_sub(m.config.skip_pages);
                                    if m.config.roman_end > max_rom {
                                        m.config.roman_end = max_rom;
                                    }
                                    m.status_msg = None;
                                }
                                Some(PnNumericFocus::RomanPages) => {
                                    let max_rom = total.saturating_sub(m.config.skip_pages);
                                    let next = m.config.roman_end.saturating_mul(10).saturating_add(d);
                                    m.config.roman_end = next.min(max_rom);
                                    m.status_msg = None;
                                }
                                None => {}
                            }
                        }
                        self.request_redraw();
                    }
                }
            }
            return;
        }
        if self.favorites_modal.is_some() {
            let _ = self.handle_favorites_key(&logical_key, text);
            return;
        }
        if let Some(g) = self.user_guide_modal {
            match logical_key {
                Key::Named(NamedKey::Escape) => self.close_user_guide_modal(),
                Key::Named(NamedKey::ArrowDown) => self.scroll_user_guide(48.0),
                Key::Named(NamedKey::ArrowUp) => self.scroll_user_guide(-48.0),
                Key::Named(NamedKey::PageDown) | Key::Named(NamedKey::Space) => {
                    self.scroll_user_guide(220.0);
                }
                Key::Named(NamedKey::PageUp) => self.scroll_user_guide(-220.0),
                Key::Named(NamedKey::ArrowRight) => {
                    if g.chapter + 1 < super::user_guide::USER_GUIDE_CHAPTERS.len() {
                        self.select_user_guide_chapter(g.chapter + 1);
                    }
                }
                Key::Named(NamedKey::ArrowLeft) => {
                    if g.chapter > 0 {
                        self.select_user_guide_chapter(g.chapter - 1);
                    }
                }
                _ => {}
            }
            return;
        }
        if self.show_customize_toolbar || self.show_about_overlay || self.show_bookmark_format_overlay {
            if matches!(logical_key, Key::Named(NamedKey::Escape)) {
                self.show_customize_toolbar = false;
                self.show_about_overlay = false;
                self.show_bookmark_format_overlay = false;
                self.request_redraw();
            }
            return;
        }

        // 1. If in an interactive command/dialog prompt mode, handle typing, Backspace, Delete, Ctrl+A/V, F3, Enter, Esc
        if self.input_mode != InputMode::Normal {
            // Allow F3 / Shift+F3 / Ctrl+G / Ctrl+Shift+G to step through matches while SearchText pill is focused
            if matches!(&self.input_mode, InputMode::SearchText { .. }) {
                let is_find_step = matches!(logical_key, Key::Named(NamedKey::F3))
                    || (self.cmd_or_ctrl_pressed
                        && matches!(&logical_key, Key::Character(c) if c.eq_ignore_ascii_case("g")));
                if is_find_step {
                    let q = match &self.input_mode {
                        InputMode::SearchText { query_input } => query_input.clone(),
                        _ => String::new(),
                    };
                    if !q.trim().is_empty() && q.trim() != self.search_query {
                        self.execute_text_search(q.trim());
                    } else if self.shift_pressed {
                        self.jump_to_prev_search_match();
                    } else {
                        self.jump_to_next_search_match();
                    }
                    self.update_title_bar();
                    self.request_redraw();
                    return;
                }
            }

            // Handle Ctrl/Cmd+V (paste) and Ctrl/Cmd+A (clear/replace) inside active text prompts;
            // ignore all other Ctrl/Cmd combos so shortcuts don't inject stray letters.
            if self.cmd_or_ctrl_pressed {
                if let Key::Character(ch) = &logical_key {
                    let s = ch.as_str();
                    if s.eq_ignore_ascii_case("v") {
                        if let Ok(clip) = Self::read_text_from_os_clipboard() {
                            let clean: String = clip
                                .chars()
                                .filter(|c| !c.is_control() || *c == ' ')
                                .take(256)
                                .collect();
                            if !clean.is_empty() {
                                match &mut self.input_mode {
                                    InputMode::SearchText { query_input } => query_input.push_str(&clean),
                                    InputMode::JumpToPage { input } => {
                                        for c in clean.chars() {
                                            if c.is_ascii_alphanumeric() || c == '#' {
                                                input.push(c);
                                            }
                                        }
                                    }
                                    InputMode::PageNumbers { input } => {
                                        for c in clean.chars() {
                                            if c.is_ascii_digit() {
                                                input.push(c);
                                            }
                                        }
                                    }
                                    InputMode::HeaderFooterRange { range_input } => {
                                        for c in clean.chars() {
                                            if c.is_ascii_alphanumeric() || c == '-' {
                                                range_input.push(c);
                                            }
                                        }
                                    }
                                    InputMode::HeaderFooterPosition { pos_input, .. } => {
                                        for c in clean.chars() {
                                            if c.is_ascii_alphabetic() {
                                                pos_input.push(c.to_ascii_uppercase());
                                            }
                                        }
                                    }
                                    InputMode::SplitRange { input } => {
                                        for c in clean.chars() {
                                            if c.is_ascii_digit() || c == '-' || c == ',' || c == ' ' {
                                                input.push(c);
                                            }
                                        }
                                    }
                                    InputMode::ClearMargins { input } => {
                                        for c in clean.chars() {
                                            if c.is_ascii_alphanumeric() || c == '-' || c == ' ' {
                                                input.push(c);
                                            }
                                        }
                                    }
                                    InputMode::HeaderFooterText { text_input, .. }
                                    | InputMode::StickyNotePrompt { text_input, .. }
                                    | InputMode::FreeTextPrompt { text_input, .. }
                                    | InputMode::EditAnnotationText { text_input, .. }
                                    | InputMode::EditFormField { value_input: text_input, .. }
                                    | InputMode::AddBookmark { title_input: text_input }
                                    | InputMode::EditBookmark { title_input: text_input, .. } => {
                                        text_input.push_str(&clean);
                                    }
                                    InputMode::Watermark { input } => input.push_str(&clean),
                                    InputMode::PasswordPrompt { password_input, .. } => {
                                        password_input.push_str(&clean);
                                    }
                                    InputMode::PrintPrompt { range_input, .. } => {
                                        for c in clean.chars() {
                                            if c.is_ascii_digit() || c == '-' || c == ',' || c == ' ' {
                                                range_input.push(c);
                                            }
                                        }
                                    }
                                    InputMode::ConfirmExternalUrl { .. }
                                    | InputMode::RestoreSessionPrompt { .. }
                                    | InputMode::OptimizePrompt { .. }
                                    | InputMode::Normal => {}
                                }
                                self.update_title_bar();
                                self.request_redraw();
                            }
                        }
                    } else if s.eq_ignore_ascii_case("a") {
                        // Quick clear-to-replace for single-line prompts (no selection cursor needed)
                        match &mut self.input_mode {
                            InputMode::SearchText { query_input } => query_input.clear(),
                            InputMode::JumpToPage { input }
                            | InputMode::PageNumbers { input }
                            | InputMode::ClearMargins { input }
                            | InputMode::Watermark { input }
                            | InputMode::SplitRange { input } => input.clear(),
                            InputMode::HeaderFooterRange { range_input } => range_input.clear(),
                            InputMode::HeaderFooterPosition { pos_input, .. } => pos_input.clear(),
                            InputMode::HeaderFooterText { text_input, .. }
                            | InputMode::StickyNotePrompt { text_input, .. }
                            | InputMode::FreeTextPrompt { text_input, .. }
                            | InputMode::EditAnnotationText { text_input, .. }
                            | InputMode::EditFormField { value_input: text_input, .. }
                            | InputMode::AddBookmark { title_input: text_input }
                            | InputMode::EditBookmark { title_input: text_input, .. } => text_input.clear(),
                            InputMode::PasswordPrompt { password_input, .. } => password_input.clear(),
                            InputMode::PrintPrompt { range_input, .. } => range_input.clear(),
                            InputMode::ConfirmExternalUrl { .. }
                            | InputMode::RestoreSessionPrompt { .. }
                            | InputMode::OptimizePrompt { .. }
                            | InputMode::Normal => {}
                        }
                        self.update_title_bar();
                        self.request_redraw();
                    }
                }
                return;
            }

            match logical_key {
                Key::Named(NamedKey::Escape) => {
                    match std::mem::replace(&mut self.input_mode, InputMode::Normal) {
                        InputMode::PasswordPrompt {
                            mut password_input, ..
                        } => {
                            password_input.clear();
                            password_input.shrink_to_fit();
                        }
                        InputMode::OptimizePrompt {
                            previewing: true, ..
                        } => {
                            self.invalidate_page_caches();
                        }
                        _ => {}
                    }
                    self.pending_restore_tabs.clear();
                    self.status_notice = Some("Cancelled".to_string());
                    self.update_title_bar();
                    self.request_redraw();
                }
                Key::Named(NamedKey::Delete) => {
                    match &mut self.input_mode {
                        InputMode::SearchText { query_input } => query_input.clear(),
                        InputMode::JumpToPage { input }
                        | InputMode::PageNumbers { input }
                        | InputMode::ClearMargins { input }
                        | InputMode::Watermark { input }
                        | InputMode::SplitRange { input } => input.clear(),
                        InputMode::HeaderFooterRange { range_input } => range_input.clear(),
                        InputMode::HeaderFooterPosition { pos_input, .. } => pos_input.clear(),
                        InputMode::HeaderFooterText { text_input, .. }
                        | InputMode::StickyNotePrompt { text_input, .. }
                        | InputMode::FreeTextPrompt { text_input, .. }
                        | InputMode::EditAnnotationText { text_input, .. }
                        | InputMode::EditFormField { value_input: text_input, .. }
                        | InputMode::AddBookmark { title_input: text_input }
                        | InputMode::EditBookmark { title_input: text_input, .. } => text_input.clear(),
                        InputMode::PasswordPrompt { password_input, .. } => password_input.clear(),
                        InputMode::PrintPrompt { range_input, .. } => range_input.clear(),
                        InputMode::ConfirmExternalUrl { .. }
                        | InputMode::RestoreSessionPrompt { .. }
                        | InputMode::OptimizePrompt { .. }
                        | InputMode::Normal => {}
                    }
                    self.update_title_bar();
                    self.request_redraw();
                }
                Key::Named(NamedKey::Backspace) => {
                    match &mut self.input_mode {
                        InputMode::SearchText { query_input } => {
                            query_input.pop();
                        }
                        InputMode::JumpToPage { input }
                        | InputMode::PageNumbers { input }
                        | InputMode::ClearMargins { input }
                        | InputMode::Watermark { input }
                        | InputMode::SplitRange { input } => {
                            input.pop();
                        }
                        InputMode::HeaderFooterRange { range_input } => {
                            range_input.pop();
                        }
                        InputMode::HeaderFooterPosition { pos_input, .. } => {
                            pos_input.pop();
                        }
                        InputMode::HeaderFooterText { text_input, .. }
                        | InputMode::StickyNotePrompt { text_input, .. }
                        | InputMode::FreeTextPrompt { text_input, .. }
                        | InputMode::EditAnnotationText { text_input, .. }
                        | InputMode::EditFormField { value_input: text_input, .. } => {
                            text_input.pop();
                        }
                        InputMode::AddBookmark { title_input }
                        | InputMode::EditBookmark { title_input, .. } => {
                            title_input.pop();
                        }
                        InputMode::PasswordPrompt { password_input, .. } => {
                            password_input.pop();
                        }
                        InputMode::PrintPrompt { range_input, .. } => {
                            range_input.pop();
                        }
                        InputMode::ConfirmExternalUrl { .. }
                        | InputMode::RestoreSessionPrompt { .. }
                        | InputMode::OptimizePrompt { .. }
                        | InputMode::Normal => {}
                    }
                    self.update_title_bar();
                    self.request_redraw();
                }
                Key::Named(NamedKey::Tab) => {
                    if let InputMode::PrintPrompt {
                        printers,
                        selected_printer,
                        ..
                    } = &mut self.input_mode
                    {
                        if !printers.is_empty() {
                            *selected_printer = (*selected_printer + 1) % printers.len();
                        }
                        self.update_title_bar();
                        self.request_redraw();
                    }
                }
                Key::Named(NamedKey::ArrowUp) => {
                    if let InputMode::PrintPrompt { copies, .. } = &mut self.input_mode {
                        *copies = (*copies + 1).min(99);
                        self.update_title_bar();
                        self.request_redraw();
                    }
                }
                Key::Named(NamedKey::ArrowDown) => {
                    if let InputMode::PrintPrompt { copies, .. } = &mut self.input_mode {
                        *copies = copies.saturating_sub(1).max(1);
                        self.update_title_bar();
                        self.request_redraw();
                    }
                }
                Key::Named(NamedKey::F3) => {
                    if matches!(self.input_mode, InputMode::SearchText { .. }) {
                        if self.shift_pressed {
                            self.jump_to_prev_search_match();
                        } else {
                            self.jump_to_next_search_match();
                        }
                    }
                }
                Key::Named(NamedKey::Enter) => {
                    if let InputMode::SearchText { ref query_input } = self.input_mode {
                        let trimmed = query_input.trim();
                        if !trimmed.is_empty()
                            && trimmed == self.search_query
                            && !self.search_matches.is_empty()
                        {
                            if self.shift_pressed {
                                self.jump_to_prev_search_match();
                            } else {
                                self.jump_to_next_search_match();
                            }
                            return;
                        }
                    }
                    self.handle_prompt_submission();
                }
                _ => {
                    let mut typed = String::new();
                    if let Some(t) = text {
                        for c in t.chars() {
                            if !c.is_control() {
                                typed.push(c);
                            }
                        }
                    } else if let Key::Character(ch) = &logical_key {
                        for c in ch.chars() {
                            if !c.is_control() {
                                typed.push(c);
                            }
                        }
                    } else if logical_key == Key::Named(NamedKey::Space) {
                        typed.push(' ');
                    }

                    if !typed.is_empty() {
                        let mut auto_submit_clear = false;
                        let mut opt_preview_action: Option<Option<(
                            crate::optimize::OptimizePreset,
                            crate::optimize::ColorMode,
                            bool,
                        )>> = None;
                        match &mut self.input_mode {
                            InputMode::SearchText { query_input } => {
                                query_input.push_str(&typed);
                            }
                            InputMode::JumpToPage { input } => {
                                for c in typed.chars() {
                                    if c.is_ascii_alphanumeric() || c == '#' {
                                        input.push(c);
                                    }
                                }
                            }
                            InputMode::PageNumbers { input } => {
                                for c in typed.chars() {
                                    if c.is_ascii_digit() {
                                        input.push(c);
                                    }
                                }
                            }
                            InputMode::HeaderFooterRange { range_input } => {
                                for c in typed.chars() {
                                    if c.is_ascii_alphanumeric() || c == '-' {
                                        range_input.push(c);
                                    }
                                }
                            }
                            InputMode::HeaderFooterPosition { pos_input, .. } => {
                                for c in typed.chars() {
                                    if c.is_ascii_alphabetic() {
                                        pos_input.push(c.to_ascii_uppercase());
                                    }
                                }
                            }
                            InputMode::ClearMargins { input } => {
                                for c in typed.chars() {
                                    if c.is_ascii_alphanumeric() || c == '-' || c == ' ' {
                                        input.push(c);
                                    }
                                }
                                // If the user presses '1', '2', or '3' directly, execute immediately on all pages
                                if matches!(input.trim(), "1" | "2" | "3") {
                                    auto_submit_clear = true;
                                }
                            }
                            InputMode::SplitRange { input } => {
                                for c in typed.chars() {
                                    if c.is_ascii_digit() || c == '-' || c == ',' || c == ' ' {
                                        input.push(c);
                                    }
                                }
                            }
                            InputMode::HeaderFooterText { text_input, .. }
                            | InputMode::StickyNotePrompt { text_input, .. }
                            | InputMode::FreeTextPrompt { text_input, .. }
                            | InputMode::EditAnnotationText { text_input, .. }
                            | InputMode::EditFormField { value_input: text_input, .. }
                            | InputMode::AddBookmark {
                                title_input: text_input,
                            }
                            | InputMode::EditBookmark {
                                title_input: text_input,
                                ..
                            } => {
                                text_input.push_str(&typed);
                            }
                            InputMode::Watermark { input } => {
                                input.push_str(&typed);
                            }
                            InputMode::PasswordPrompt { password_input, .. } => {
                                password_input.push_str(&typed);
                            }
                            InputMode::RestoreSessionPrompt { .. } => {
                                if typed.eq_ignore_ascii_case("y") {
                                    auto_submit_clear = true;
                                } else if typed.eq_ignore_ascii_case("n") {
                                    self.input_mode = InputMode::Normal;
                                    self.pending_restore_tabs.clear();
                                }
                            }
                            InputMode::OptimizePrompt {
                                preset,
                                mode,
                                clean_scans,
                                previewing,
                            } => {
                                let mut changed_opts = false;
                                let mut toggled_preview = false;
                                for ch in typed.chars() {
                                    match ch {
                                        '1' => {
                                            *preset = crate::optimize::OptimizePreset::Smaller;
                                            changed_opts = true;
                                        }
                                        '2' => {
                                            *preset = crate::optimize::OptimizePreset::Balanced;
                                            changed_opts = true;
                                        }
                                        '3' => {
                                            *preset = crate::optimize::OptimizePreset::Print;
                                            changed_opts = true;
                                        }
                                        'c' | 'C' => {
                                            *mode = crate::optimize::ColorMode::Color;
                                            changed_opts = true;
                                        }
                                        'g' | 'G' => {
                                            *mode = crate::optimize::ColorMode::Gray;
                                            changed_opts = true;
                                        }
                                        'b' | 'B' => {
                                            *mode = crate::optimize::ColorMode::BlackWhite;
                                            changed_opts = true;
                                        }
                                        's' | 'S' | ' ' => {
                                            *clean_scans = !*clean_scans;
                                            changed_opts = true;
                                        }
                                        'p' | 'P' => {
                                            *previewing = !*previewing;
                                            toggled_preview = true;
                                        }
                                        _ => {}
                                    }
                                }
                                if toggled_preview || (changed_opts && *previewing) {
                                    if *previewing {
                                        opt_preview_action =
                                            Some(Some((*preset, *mode, *clean_scans)));
                                    } else {
                                        opt_preview_action = Some(None);
                                    }
                                }
                            }
                            InputMode::PrintPrompt {
                                copies,
                                range_input,
                                fit_to_page,
                                ..
                            } => {
                                for c in typed.chars() {
                                    match c {
                                        'f' | 'F' => {
                                            *fit_to_page = !*fit_to_page;
                                        }
                                        '+' | '=' | ']' => {
                                            *copies = (*copies + 1).min(99);
                                        }
                                        '[' => {
                                            *copies = copies.saturating_sub(1).max(1);
                                        }
                                        '-' => {
                                            if range_input.is_empty() {
                                                *copies = copies.saturating_sub(1).max(1);
                                            } else {
                                                range_input.push('-');
                                            }
                                        }
                                        _ if c.is_ascii_digit()
                                            || c == ','
                                            || c == ' '
                                            || c.eq_ignore_ascii_case(&'a')
                                            || c.eq_ignore_ascii_case(&'l') =>
                                        {
                                            range_input.push(c);
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            InputMode::ConfirmExternalUrl { .. } | InputMode::Normal => {}
                        }
                        if let Some(action) = opt_preview_action {
                            match action {
                                Some((p, m, cs)) => {
                                    self.render_optimize_preview_current_page(p, m, cs);
                                }
                                None => {
                                    self.invalidate_page_caches();
                                }
                            }
                        }
                        if auto_submit_clear {
                            self.handle_prompt_submission();
                        } else {
                            self.update_title_bar();
                            self.request_redraw();
                        }
                    }
                }
            }
            return;
        }

        // 2. If Help Overlay is open, ONLY `Esc`, `?`, `/`, or `F1` closes it — never leak keys beneath it!
        if self.show_help_overlay {
            if matches!(
                logical_key,
                Key::Named(NamedKey::Escape) | Key::Named(NamedKey::F1)
            ) {
                self.show_help_overlay = false;
                self.request_redraw();
            } else if let Key::Character(ch) = &logical_key {
                if ch.as_str() == "?" || ch.as_str() == "/" {
                    self.show_help_overlay = false;
                    self.request_redraw();
                }
            }
            return;
        }

        // 2b. Markdown / LaTeX live editor (Split or Edit mode)
        if self.is_markdown_tab()
            && self.md_editor_mode != super::text_editor::MdEditorMode::Preview
            && self.open_menu.is_none()
            && !self.show_help_overlay
        {
            if self.handle_markdown_editor_key(&logical_key, text) {
                return;
            }
        }

        // 3. OS-native modifier shortcuts: Cmd/Ctrl + Z/C/F/G/P/S/W/0/+/-/Tab/1..9.
        // Any unrecognized Ctrl/Cmd or Alt+Character combo returns early so e.g. Ctrl+D NEVER triggers bare 'D' (Delete Page)!
        if self.cmd_or_ctrl_pressed {
            match &logical_key {
                Key::Named(NamedKey::Tab) => {
                    if self.tabs.len() > 1 {
                        if self.shift_pressed {
                            let prev = if self.active_tab == 0 {
                                self.tabs.len() - 1
                            } else {
                                self.active_tab - 1
                            };
                            self.switch_to_tab(prev);
                        } else {
                            self.cycle_next_tab();
                        }
                    }
                    return;
                }
                Key::Character(ch) => {
                    let s = ch.as_str();
                    if s.eq_ignore_ascii_case("z") {
                        if self.shift_pressed || s == "Z" {
                            self.perform_redo();
                        } else {
                            self.perform_undo();
                        }
                    } else if s.eq_ignore_ascii_case("y") {
                        self.perform_redo();
                    } else if s.eq_ignore_ascii_case("c") {
                        self.copy_selected_text_to_clipboard();
                    } else if s.eq_ignore_ascii_case("f") {
                        self.input_mode = InputMode::SearchText {
                            query_input: self.search_query.clone(),
                        };
                        self.status_notice = None;
                        self.update_title_bar();
                        self.request_redraw();
                    } else if s.eq_ignore_ascii_case("g") {
                        if self.shift_pressed || s == "G" {
                            self.jump_to_prev_search_match();
                        } else {
                            self.jump_to_next_search_match();
                        }
                    } else if s.eq_ignore_ascii_case("p") {
                        self.trigger_print();
                    } else if s.eq_ignore_ascii_case("s") {
                        if self.shift_pressed || s == "S" {
                            self.pending_dialog = Some(PendingDialog::SaveAs);
                        } else {
                            self.pending_dialog = Some(PendingDialog::Save);
                        }
                    } else if s.eq_ignore_ascii_case("d") {
                        self.open_favorites_modal();
                    } else if s.eq_ignore_ascii_case("w") {
                        if !self.tabs.is_empty() {
                            let _ = self.request_close_action(
                                super::doc_props_modal::PendingCloseAction::CloseTab(self.active_tab),
                            );
                        }
                    } else if s == "0" {
                        self.reset_zoom();
                    } else if s == "+" || s == "=" {
                        self.zoom_by_step(1.25);
                    } else if s == "-" || s == "_" {
                        self.zoom_by_step(0.8);
                    } else if let Some(d) = s.chars().next().filter(|_| s.len() == 1).and_then(|c| c.to_digit(10)) {
                        if (1..=9).contains(&d) {
                            let target = (d as usize) - 1;
                            if target < self.tabs.len() {
                                self.switch_to_tab(target);
                            }
                        }
                    }
                    return;
                }
                _ => return,
            }
        }
        if self.alt_pressed && matches!(logical_key, Key::Character(_)) {
            return;
        }

        // 4. Normal Mode hotkeys
        match logical_key {
            Key::Named(NamedKey::Tab) => {
                if self.ctrl_pressed {
                    self.cycle_next_tab();
                } else {
                    self.toggle_view_mode();
                }
            }
            Key::Named(NamedKey::Enter) => {
                if self.view_mode == ViewMode::ThumbnailGrid {
                    self.toggle_view_mode();
                }
            }
            Key::Named(NamedKey::F1) => {
                self.show_help_overlay = !self.show_help_overlay;
                self.request_redraw();
            }
            Key::Named(NamedKey::F3) => {
                if self.shift_pressed {
                    self.jump_to_prev_search_match();
                } else {
                    self.jump_to_next_search_match();
                }
            }
            Key::Named(NamedKey::F5) => {
                if self.is_presentation {
                    self.exit_presentation_or_fullscreen();
                } else {
                    self.enter_presentation_mode();
                }
            }
            Key::Named(NamedKey::F11) => {
                self.toggle_fullscreen();
            }
            Key::Named(NamedKey::Escape) => {
                if self.open_menu.is_some() {
                    self.open_menu = None;
                    self.request_redraw();
                } else if self.tab_context_menu.is_some() {
                    self.tab_context_menu = None;
                    self.request_redraw();
                } else if self.show_tab_list_popover {
                    self.show_tab_list_popover = false;
                    self.request_redraw();
                } else if self.thumb_context_menu.is_some() {
                    self.thumb_context_menu = None;
                    self.request_redraw();
                } else if self.annot_context_menu.is_some() {
                    self.annot_context_menu = None;
                    self.request_redraw();
                } else if self.show_bookmark_format_overlay {
                    self.show_bookmark_format_overlay = false;
                    self.request_redraw();
                } else if self.show_reading_mode_popover {
                    self.show_reading_mode_popover = false;
                    self.request_redraw();
                } else if self.show_notifications {
                    self.show_notifications = false;
                    self.request_redraw();
                } else if self.dragging_annotation.is_some() {
                    self.dragging_annotation = None;
                    self.request_redraw();
                } else if self.active_note_popover.is_some() {
                    self.active_note_popover = None;
                    self.request_redraw();
                } else if !self.selected_thumbs.is_empty() {
                    self.selected_thumbs.clear();
                    self.request_redraw();
                } else if self.show_help_overlay {
                    self.show_help_overlay = false;
                    self.request_redraw();
                } else if self.compare_mode.is_some() {
                    self.close_compare_mode();
                } else if self.deskew_bar.is_some() {
                    self.close_deskew_bar();
                } else if self.incremental_search.is_some() {
                    self.cancel_incremental_search();
                } else if self.optimize_job.is_some() {
                    self.cancel_optimize_job();
                } else if self.print_job.is_some() {
                    self.cancel_print_job();
                } else if self.is_presentation || self.is_fullscreen {
                    self.exit_presentation_or_fullscreen();
                } else if !self.selected_text_rects.is_empty() {
                    self.clear_text_selection();
                    self.status_notice = Some("Cleared text selection".to_string());
                    self.update_title_bar();
                    self.request_redraw();
                } else if self.mouse_mode != MouseMode::Select {
                    self.mouse_mode = MouseMode::Select;
                    self.last_measurement = None;
                    self.pending_stamp_image = None;
                    self.ink_stroke_screen_pts.clear();
                    self.status_notice =
                        Some("Returned to Mouse Mode: Select Text".to_string());
                    self.update_title_bar();
                    self.request_redraw();
                } else if self.view_mode == ViewMode::ThumbnailGrid {
                    self.toggle_view_mode();
                } else if self.view_mode == ViewMode::TextView {
                    let restore = if self.last_page_view_mode.is_page_view() {
                        self.last_page_view_mode
                    } else {
                        ViewMode::FitPage
                    };
                    self.set_view_mode(restore);
                }
            }
            Key::Named(NamedKey::Delete) => {
                // If Bookmark Sidebar is open and a bookmark is selected, Delete removes it
                if self.show_bookmarks {
                    if let Some(sel) = self.selected_bookmark {
                        if sel < self.bookmarks.len() {
                            let removed = self.bookmarks.remove(sel);
                            self.has_unsaved_edits = true;
                            if self.bookmarks.is_empty() {
                                self.selected_bookmark = None;
                            } else if sel >= self.bookmarks.len() {
                                self.selected_bookmark = Some(self.bookmarks.len() - 1);
                            }
                            self.persist_bookmarks();
                            self.status_notice =
                                Some(format!("Deleted bookmark '{}' (press S to save)", removed.title));
                            self.update_title_bar();
                            self.request_redraw();
                        }
                    }
                }
            }
            Key::Named(NamedKey::ArrowDown) => {
                if !self.scroll_or_turn_page_vertical(64.0) {
                    self.step_pages_forward();
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                if !self.scroll_or_turn_page_vertical(-64.0) {
                    self.step_pages_backward();
                }
            }
            Key::Named(NamedKey::ArrowRight) => {
                if self.alt_pressed {
                    self.navigate_forward();
                } else if self.is_markdown_tab() {
                    let step = self.window.as_ref().map(|w| (w.inner_size().height as f32 * 0.80).max(240.0)).unwrap_or(480.0);
                    self.scroll_or_turn_page_vertical(step);
                } else if !self.pan_horizontal(64.0) {
                    self.step_pages_forward();
                }
            }
            Key::Named(NamedKey::PageDown) | Key::Named(NamedKey::Space) => {
                if self.is_markdown_tab() {
                    let step = self.window.as_ref().map(|w| (w.inner_size().height as f32 * 0.80).max(240.0)).unwrap_or(480.0);
                    self.scroll_or_turn_page_vertical(step);
                } else {
                    self.step_pages_forward();
                }
            }
            Key::Named(NamedKey::ArrowLeft) => {
                if self.alt_pressed {
                    self.navigate_back();
                } else if self.is_markdown_tab() {
                    let step = self.window.as_ref().map(|w| (w.inner_size().height as f32 * 0.80).max(240.0)).unwrap_or(480.0);
                    self.scroll_or_turn_page_vertical(-step);
                } else if !self.pan_horizontal(-64.0) {
                    self.step_pages_backward();
                }
            }
            Key::Named(NamedKey::PageUp) => {
                if self.is_markdown_tab() {
                    let step = self.window.as_ref().map(|w| (w.inner_size().height as f32 * 0.80).max(240.0)).unwrap_or(480.0);
                    self.scroll_or_turn_page_vertical(-step);
                } else {
                    self.step_pages_backward();
                }
            }
            Key::Named(NamedKey::Home) => {
                if self.is_markdown_tab() {
                    self.scroll_y = 0.0;
                    self.request_redraw();
                } else {
                    self.navigate_to_page(0);
                }
            }
            Key::Named(NamedKey::End) => {
                if self.is_markdown_tab() {
                    if let Some(w) = self.window.as_ref() {
                        let sz = w.inner_size();
                        let sf = self.effective_scale_factor(w.scale_factor());
                        let canvas_h = sz.height.saturating_sub(self.top_bars_height_px(sf) + self.status_bar_height_px(sf));
                        self.scroll_y = self.md_max_scroll_y(canvas_h, sf);
                        self.request_redraw();
                    }
                } else {
                    let total = self.total_pages();
                    if total > 0 {
                        self.navigate_to_page(total - 1);
                    }
                }
            }
            Key::Character(ch) => {
                let s = ch.as_str();
                if s == "?" || s == "/" {
                    self.show_help_overlay = !self.show_help_overlay;
                    self.request_redraw();
                } else if s == "1" {
                    self.set_view_mode(ViewMode::FitPage);
                } else if s == "2" {
                    self.set_view_mode(ViewMode::FitWidth);
                } else if s == "3" {
                    self.set_view_mode(ViewMode::FitTextWidth);
                } else if s == "4" {
                    self.set_view_mode(ViewMode::TwoPageSpread);
                } else if s == "5" {
                    self.set_scroll_mode(self.effective_scroll_mode().next());
                } else if s.eq_ignore_ascii_case("t") {
                    if self.view_mode == ViewMode::TextView {
                        let restore = if self.last_page_view_mode.is_page_view() {
                            self.last_page_view_mode
                        } else {
                            ViewMode::FitPage
                        };
                        self.set_view_mode(restore);
                    } else {
                        self.set_view_mode(ViewMode::TextView);
                    }
                } else if s.eq_ignore_ascii_case("v") {
                    self.set_reading_filter(self.reading_filter.next());
                } else if s.eq_ignore_ascii_case("p") {
                    self.open_stamp_manager_modal();
                } else if s.eq_ignore_ascii_case("u") {
                    self.flatten_and_unlock_current_pdf();
                } else if s.eq_ignore_ascii_case("b") {
                    self.show_bookmarks = !self.show_bookmarks;
                    self.invalidate_page_caches();
                    self.status_notice = Some(if self.show_bookmarks {
                        "Bookmarks Sidebar ON ('A' Add, 'L' Import .txt, 'E' Edit, Del Remove)"
                            .to_string()
                    } else {
                        "Bookmarks Sidebar OFF".to_string()
                    });
                    self.update_title_bar();
                    self.request_redraw();
                } else if s.eq_ignore_ascii_case("l") {
                    self.pending_dialog = Some(PendingDialog::ImportBookmarksTxt);
                } else if s == "+" || s == "=" {
                    self.zoom_by_step(1.25);
                } else if s == "-" || s == "_" {
                    self.zoom_by_step(0.8);
                } else if s == "0" {
                    self.reset_zoom();
                } else if s.eq_ignore_ascii_case("a") && self.show_bookmarks {
                    self.input_mode = InputMode::AddBookmark {
                        title_input: String::new(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                    self.request_redraw();
                } else if s.eq_ignore_ascii_case("e") && self.show_bookmarks {
                    if let Some(sel) = self.selected_bookmark {
                        if let Some(bm) = self.bookmarks.get(sel) {
                            self.input_mode = InputMode::EditBookmark {
                                bookmark_idx: sel,
                                title_input: bm.title.clone(),
                            };
                            self.status_notice = None;
                            self.update_title_bar();
                            self.request_redraw();
                        }
                    }
                } else if s.eq_ignore_ascii_case("g") {
                    self.input_mode = InputMode::JumpToPage {
                        input: String::new(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                    self.request_redraw();
                } else if s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty() {
                    self.input_mode = InputMode::JumpToPage {
                        input: s.to_string(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                    self.request_redraw();
                } else if s.eq_ignore_ascii_case("n") {
                    self.open_page_number_modal();
                } else if s.eq_ignore_ascii_case("h") || s.eq_ignore_ascii_case("c") {
                    self.open_header_footer_modal();
                } else if s.eq_ignore_ascii_case("w") {
                    self.open_watermark_modal();
                    self.status_notice = None;
                    self.update_title_bar();
                    self.request_redraw();
                } else if s.eq_ignore_ascii_case("x") {
                    self.input_mode = InputMode::SplitRange {
                        input: String::new(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                    self.request_redraw();
                } else if s.eq_ignore_ascii_case("m") {
                    if self.shift_pressed || s == "M" {
                        self.pending_dialog = Some(PendingDialog::CombineMultiple);
                    } else {
                        self.pending_dialog = Some(PendingDialog::Merge);
                    }
                } else if s.eq_ignore_ascii_case("i") {
                    self.insert_blank_page_after_current();
                } else if s.eq_ignore_ascii_case("r") {
                    let target_page = if self.view_mode == ViewMode::ThumbnailGrid {
                        self.hovered_thumb.unwrap_or(self.current_page_index)
                    } else {
                        self.current_page_index
                    };
                    self.rotate_page_clockwise(target_page);
                } else if s.eq_ignore_ascii_case("k") {
                    let target_page = if self.view_mode == ViewMode::ThumbnailGrid {
                        self.hovered_thumb.unwrap_or(self.current_page_index)
                    } else {
                        self.current_page_index
                    };
                    self.convert_page_to_bw_in_place(target_page);
                } else if s.eq_ignore_ascii_case("d") {
                    self.delete_current_page();
                } else if s.eq_ignore_ascii_case("o") {
                    self.pending_dialog = Some(PendingDialog::Open);
                } else if s.eq_ignore_ascii_case("s") {
                    self.pending_dialog = Some(PendingDialog::Save);
                } else if s.eq_ignore_ascii_case("z") {
                    self.compress_current_pdf();
                }
            }
            _ => {}
        }
    }
}

impl<'a> ApplicationHandler for PdfViewerApp<'a> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let icon = crate::app_icon::build_window_icon();
            #[allow(unused_mut)]
            let mut win_attrs = Window::default_attributes()
                .with_title("MicroPDF")
                .with_inner_size(LogicalSize::new(1024.0, 1080.0))
                .with_window_icon(icon.clone());
            #[cfg(target_os = "windows")]
            {
                use winit::platform::windows::WindowAttributesExtWindows;
                win_attrs = win_attrs.with_taskbar_icon(icon);
            }

            let window = Rc::new(
                event_loop
                    .create_window(win_attrs)
                    .expect("Failed to create winit window"),
            );
            let context =
                SoftContext::new(window.clone()).expect("Failed to create softbuffer context");
            let surface = Surface::new(&context, window.clone())
                .expect("Failed to create softbuffer surface");

            self.window = Some(window);
            self.context = Some(context);
            self.surface = Some(surface);
            self.update_title_bar();
            self.request_redraw();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.pending_app_exit {
            self.invalidate_page_caches();
            self.thumbnail_cache.clear();
            self.tabs.clear();
            self.doc = None;
            self.surface = None;
            self.context = None;
            self.window = None;
            event_loop.exit();
            return;
        }
        self.poll_single_instance_listener();
        self.poll_auto_reload_if_due();
        if self.optimize_job.is_some() {
            let still_running = self.step_optimize_job();
            if still_running {
                event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
            }
        }
        // One printed page per iteration while a print job runs.
        let printing = self.print_job.is_some() && self.step_print_job();
        if printing {
            event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
        }
        let sending_to_lp = self.lp_job.is_some() && self.poll_lp_job();
        if self.incremental_search.is_some() {
            let still_searching = self.step_incremental_search(Duration::from_millis(8));
            if still_searching {
                event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
            }
        } else if sending_to_lp && self.optimize_job.is_none() && !printing {
            // `lp` is still running: check it again soon.
            event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(50),
            ));
        } else if self.optimize_job.is_none()
            && !printing
            && (self.single_instance_listener.is_some()
                || (self.window_focused && self.doc.is_some()))
        {
            event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(120),
            ));
        }

        if let Some(dialog) = self.pending_dialog.take() {
            match dialog {
                PendingDialog::Open => self.open_file_dialog(),
                PendingDialog::Save => self.save_file_dialog(),
                PendingDialog::SaveAs => self.save_as_file_dialog(),
                PendingDialog::Merge => self.merge_pdf_dialog(),
                PendingDialog::CombineMultiple => self.open_combine_modal(),
                PendingDialog::CombineAddFiles => self.combine_add_files_dialog(),
                PendingDialog::PickImagesForPdf => self.pick_images_for_pdf_dialog(),
                PendingDialog::PickComparePdf => self.pick_compare_pdf_dialog(),
                PendingDialog::ExportPagesAsImagesDir => self.export_pages_as_images_dir_dialog(),
                PendingDialog::SplitSinglePagesDir => self.execute_split_single_pages_dialog(),
                PendingDialog::ExportMarkdown => self.export_markdown_dialog(),
                PendingDialog::ExportLatex => self.export_latex_dialog(),
                PendingDialog::ExportDocx => self.export_docx_dialog(),
                PendingDialog::ExportXlsx => self.export_xlsx_dialog(),
                PendingDialog::ExportMdPdf => self.run_markdown_pdf_export(),
                PendingDialog::ExportMdDocx => self.run_markdown_docx_export(),
                PendingDialog::ExportEpub => self.export_epub_dialog(),
                PendingDialog::ExportMdEpub => self.run_markdown_epub_export(),
                PendingDialog::PickStampImage => self.pick_stamp_image_dialog(),
                PendingDialog::ImportStampAssetPng => self.import_stamp_asset_png_dialog(),
                PendingDialog::Split(spec) => self.execute_split_range(&spec),
                PendingDialog::ExtractSelectedPages(pages) => {
                    self.extract_selected_pages_dialog(&pages)
                }
                PendingDialog::ImportBookmarksTxt => self.import_bookmarks_txt_dialog(),
                PendingDialog::ExportBookmarksTxt => self.export_bookmarks_txt_dialog(),
                PendingDialog::SaveBookmarkSampleTxt => self.save_bookmark_sample_txt_dialog(),
                PendingDialog::SaveUserGuideTxt => self.save_user_guide_txt_dialog(),
                PendingDialog::AddFavoriteFile => self.add_favorite_file_dialog(),
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                if self.request_close_action(super::doc_props_modal::PendingCloseAction::ExitApp) {
                    self.invalidate_page_caches();
                    self.thumbnail_cache.clear();
                    self.tabs.clear();
                    self.doc = None;
                    self.surface = None;
                    self.context = None;
                    self.window = None;
                    event_loop.exit();
                }
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.invalidate_page_caches();
                self.request_redraw();
            }
            WindowEvent::Focused(focused) => {
                self.window_focused = focused;
                if focused {
                    self.poll_auto_reload_if_due();
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                let st = modifiers.state();
                self.shift_pressed = st.shift_key();
                self.ctrl_pressed = st.control_key();
                self.alt_pressed = st.alt_key();
                self.cmd_or_ctrl_pressed = st.super_key() || st.control_key();
            }
            WindowEvent::DroppedFile(dropped_path) => {
                let ext = dropped_path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");
                if self.create_pdf_images_modal.is_some() {
                    if Self::is_supported_image_path(&dropped_path) {
                        self.add_images_to_create_modal(vec![dropped_path]);
                    }
                    return;
                }
                if self.combine_modal.is_some() {
                    if ext.eq_ignore_ascii_case("pdf") {
                        self.add_files_to_combine_modal(vec![dropped_path]);
                    }
                    return;
                }
                if self.has_blocking_modal() {
                    return;
                }
                self.close_all_popovers();
                if ext.eq_ignore_ascii_case("txt") {
                    self.import_bookmarks_from_path(&dropped_path);
                    return;
                }
                if ext.eq_ignore_ascii_case("png")
                    || ext.eq_ignore_ascii_case("jpg")
                    || ext.eq_ignore_ascii_case("jpeg")
                {
                    if let Ok(buf) = PdfEngine::load_stamp_image(&dropped_path) {
                        self.pending_stamp_image = Some(buf);
                        self.mouse_mode = MouseMode::StampImage;
                        self.status_notice = Some(
                            "Loaded dropped image — Click or Drag on page to stamp!".to_string(),
                        );
                        self.update_title_bar();
                        self.request_redraw();
                    }
                    return;
                }
                if Self::is_doc_reader_path(&dropped_path) {
                    let fname = dropped_path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("document.md")
                        .to_string();
                    match self.open_document_in_new_tab(dropped_path) {
                        Ok(()) => {
                            self.status_notice = Some(format!("Opened Markdown '{fname}'"));
                        }
                        Err(e) => {
                            self.status_notice = Some(format!("Error opening '{fname}': {e}"));
                        }
                    }
                    self.update_title_bar();
                    self.request_redraw();
                    return;
                }
                if !ext.eq_ignore_ascii_case("pdf") {
                    self.status_notice = Some(format!(
                        "Dropped file {:?} is not a .pdf, .md, .txt, or .png/.jpg",
                        dropped_path
                    ));
                    self.update_title_bar();
                    self.request_redraw();
                    return;
                }

                let fname = dropped_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("dropped.pdf")
                    .to_string();

                // In Page View (or Text View, or when holding Shift, or when no document is loaded):
                // Open the dropped PDF in a new tab!
                if self.view_mode.is_page_view()
                    || self.view_mode == ViewMode::TextView
                    || self.shift_pressed
                    || self.doc.is_none()
                {
                    match self.open_document_in_new_tab(dropped_path) {
                        Ok(()) => {
                            self.status_notice = Some(format!(
                                "Opened '{fname}' in Tab {} ({} pages)",
                                self.active_tab + 1,
                                self.total_pages()
                            ));
                        }
                        Err(e) => {
                            self.status_notice = Some(format!("ERROR opening '{fname}': {e}"));
                        }
                    }
                    self.update_title_bar();
                    self.request_redraw();
                    return;
                }

                // In Thumbnail Grid View (`Tab`): insert all pages of the dropped PDF right before target thumbnail
                let (win_w, sidebar_w, top_bars_h) = if let Some(w) = self.window.as_ref() {
                    let sz = w.inner_size();
                    let sf = self.effective_scale_factor(w.scale_factor());
                    (
                        sz.width.max(1),
                        self.sidebar_width_px(sf).min(sz.width.saturating_sub(64)),
                        self.top_bars_height_px(sf).min(sz.height.saturating_sub(32)),
                    )
                } else {
                    (1024, 0, MENU_BAR_HEIGHT_LOGICAL + TAB_BAR_HEIGHT_LOGICAL)
                };

                let target_idx = self.grid_drop_target_index(
                    self.cursor_pos.0,
                    self.cursor_pos.1,
                    win_w,
                    sidebar_w,
                    top_bars_h,
                );

                self.push_undo_snapshot();
                if let Some(doc) = self.doc.as_mut() {
                    match self
                        .engine
                        .insert_external_pdf_at_index(doc, &dropped_path, target_idx)
                    {
                        Ok(inserted_count) => {
                            PdfEngine::remap_bookmarks_after_insert(
                                &mut self.bookmarks,
                                target_idx as usize,
                                inserted_count as usize,
                                self.skip_pages,
                                self.roman_end,
                            );
                            let next_bw: HashSet<u16> = self
                                .bw_pages
                                .iter()
                                .map(|&p| {
                                    if p >= target_idx {
                                        p.saturating_add(inserted_count)
                                    } else {
                                        p
                                    }
                                })
                                .collect();
                            self.bw_pages = next_bw;
                            self.search_matches.clear();
                            self.current_search_match = 0;
                            self.has_unsaved_edits = true;
                            self.current_page_index = target_idx;
                            self.invalidate_page_caches();
                            self.thumbnail_cache.clear();
                            self.status_notice = Some(format!(
                                "Inserted {inserted_count} page(s) from '{fname}' before Page {}",
                                target_idx + 1
                            ));
                            self.update_title_bar();
                            self.request_redraw();
                        }
                        Err(e) => {
                            let _ = self.undo_stack.pop_back();
                            self.status_notice = Some(format!("Drop insert failed: {e}"));
                            self.update_title_bar();
                        }
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => ((x as f64) * 36.0, (y as f64) * 36.0),
                    MouseScrollDelta::PixelDelta(pos) => (pos.x, pos.y),
                };
                if let Some(ref mut fav_m) = self.favorites_modal {
                    fav_m.scroll_y = (fav_m.scroll_y - dy as f32).max(0.0);
                    self.request_redraw();
                    return;
                }
                if self.user_guide_modal.is_some() {
                    self.scroll_user_guide((-dy) as f32);
                    return;
                }
                if let Some(ref mut m) = self.header_footer_modal {
                    if m.font_dropdown.open {
                        if dy.abs() > 0.1 {
                            m.font_dropdown.scroll_by(if dy < 0.0 { 2 } else { -2 });
                            self.request_redraw();
                        }
                        return;
                    }
                }
                if let Some(ref mut m) = self.page_number_modal {
                    if m.font_dropdown.open {
                        if dy.abs() > 0.1 {
                            m.font_dropdown.scroll_by(if dy < 0.0 { 2 } else { -2 });
                            self.request_redraw();
                        }
                        return;
                    }
                }
                if let Some(ref mut m) = self.watermark_modal {
                    if m.font_dropdown.open {
                        if dy.abs() > 0.1 {
                            m.font_dropdown.scroll_by(if dy < 0.0 { 2 } else { -2 });
                            self.request_redraw();
                        }
                        return;
                    }
                }
                if self.has_blocking_modal()
                    || self.open_menu.is_some()
                    || self.tab_context_menu.is_some()
                    || self.thumb_context_menu.is_some()
                    || self.show_reading_mode_popover
                    || self.show_notifications
                {
                    return;
                }

                // Scroll inside the `▾` All-Tabs overflow dropdown popover if open and hovered
                if self.show_tab_list_popover {
                    if let Some(w) = self.window.as_ref() {
                        let sz = w.inner_size();
                        let sf = self.effective_scale_factor(w.scale_factor());
                        let top_bars_h = self.top_bars_height_px(sf).min(sz.height.saturating_sub(32));
                        if self
                            .hit_test_tab_list_popover(
                                self.cursor_pos.0,
                                self.cursor_pos.1,
                                sz.width.max(1),
                                sz.height.max(1),
                                top_bars_h,
                                sf,
                            )
                            .is_some()
                        {
                            if dy < -4.0 && self.tab_list_scroll + 1 < self.tabs.len() {
                                self.tab_list_scroll += 1;
                                self.request_redraw();
                            } else if dy > 4.0 && self.tab_list_scroll > 0 {
                                self.tab_list_scroll -= 1;
                                self.request_redraw();
                            }
                            return;
                        }
                    }
                }

                // Scroll horizontal tab strip when wheeling directly over the Tab Bar
                if let Some(w) = self.window.as_ref() {
                    let sz = w.inner_size();
                    let sf = self.effective_scale_factor(w.scale_factor());
                    let mb_h = self.menu_bar_height_px(sf).min(sz.height.saturating_sub(48));
                    let tb_h = self.tab_bar_height_px(sf).min(sz.height.saturating_sub(mb_h + 24));
                    let cy = self.cursor_pos.1 as i32;
                    if cy >= mb_h as i32 && cy < (mb_h + tb_h) as i32 {
                        let dir = if dy.abs() >= dx.abs() { dy } else { dx };
                        if dir < -4.0 {
                            self.scroll_tab_strip(1);
                        } else if dir > 4.0 {
                            self.scroll_tab_strip(-1);
                        }
                        return;
                    }
                }

                if self.is_markdown_tab() {
                    if self.cmd_or_ctrl_pressed {
                        if dy.abs() > 0.1 {
                            let factor = if dy > 0.0 { 1.1 } else { 1.0 / 1.1 };
                            self.zoom_level = (self.zoom_level * factor).clamp(0.6, 2.2);
                            let pct = (self.zoom_level * 100.0).round() as u32;
                            self.status_notice = Some(format!("Markdown Zoom: {pct}%"));
                            self.update_title_bar();
                            self.request_redraw();
                        }
                    } else if let Some(w) = self.window.as_ref() {
                        let sz = w.inner_size();
                        let sf = self.effective_scale_factor(w.scale_factor());
                        let top_bars_h = self.top_bars_height_px(sf);
                        let status_bar_h = self.status_bar_height_px(sf);
                        let sidebar_w = self.sidebar_width_px(sf).min(sz.width.saturating_sub(64));
                        let cx = self.cursor_pos.0.round() as i32;
                        let cy = self.cursor_pos.1.round() as i32;
                        if let Some((ex, ey, ew, eh)) = self.md_editor_rect(sidebar_w, top_bars_h, sz.width, sz.height, status_bar_h, sf) {
                            if cx >= ex && cx < ex + ew as i32 && cy >= ey && cy < ey + eh as i32 {
                                let zoom = self.zoom_level;
                                if let Some(ref mut ed) = self.md_editor {
                                    let max_s = ed.max_scroll_y(ew, eh as f32, sf, zoom);
                                    ed.scroll_y = (ed.scroll_y - dy as f32).clamp(0.0, max_s);
                                    self.request_redraw();
                                }
                                return;
                            }
                        }
                        let canvas_h = sz.height.saturating_sub(top_bars_h + status_bar_h);
                        let max_s = self.md_max_scroll_y(canvas_h, sf);
                        self.scroll_y = (self.scroll_y - dy as f32).clamp(0.0, max_s);
                        self.request_redraw();
                    }
                    return;
                }

                // Ctrl/Cmd + MouseWheel zooms around the cursor position (25%..=800%)
                if self.cmd_or_ctrl_pressed && (self.view_mode.is_page_view() || self.compare_mode.is_some()) {
                    if dy.abs() > 0.1 {
                        let factor = if dy > 0.0 { 1.15 } else { 1.0 / 1.15 };
                        if self.compare_mode.is_some() {
                            self.zoom_level = (self.zoom_level * factor).clamp(MIN_ZOOM_LEVEL, 4.0);
                            if let Some(ref mut cmp) = self.compare_mode {
                                cmp.invalidate_caches();
                            }
                            self.update_title_bar();
                            self.request_redraw();
                        } else {
                            let target = (self.zoom_level * factor).clamp(MIN_ZOOM_LEVEL, MAX_ZOOM_LEVEL);
                            let (cx, cy) = self.cursor_pos;
                            self.set_zoom_around_cursor(target, cx, cy);
                        }
                    }
                    return;
                }

                // Synchronized vertical/horizontal scrolling in Compare Mode
                if self.compare_mode.is_some() {
                    let (pan_dx, scroll_dy) = if self.shift_pressed && dx.abs() < 0.5 {
                        ((-dy) as f32, 0.0)
                    } else {
                        ((-dx) as f32, (-dy) as f32)
                    };
                    if self.scroll_compare_synced(pan_dx, scroll_dy) {
                        return;
                    }
                }

                // If cursor is over the Bookmark Sidebar, scroll bookmarks
                if let Some(w) = self.window.as_ref() {
                    let sidebar_w = self.sidebar_width_px(self.effective_scale_factor(w.scale_factor()));
                    if self.show_bookmarks && (self.cursor_pos.0 as u32) < sidebar_w {
                        if dy < -4.0 && self.bookmark_scroll_y + 1 < self.bookmarks.len() {
                            self.bookmark_scroll_y += 1;
                            self.request_redraw();
                        } else if dy > 4.0 && self.bookmark_scroll_y > 0 {
                            self.bookmark_scroll_y -= 1;
                            self.request_redraw();
                        }
                        return;
                    }
                }

                if self.view_mode == ViewMode::ThumbnailGrid {
                    // Scroll thumbnail grid vertically
                    self.grid_scroll_y = (self.grid_scroll_y - dy as f32).max(0.0);
                    self.request_redraw();
                    return;
                }

                let mut panned_h = false;
                if dx.abs() > 0.5 {
                    panned_h = self.pan_horizontal((-dx) as f32);
                }
                if panned_h && dy.abs() < 0.5 {
                    return;
                }

                // Smoothly scroll vertically (or horizontally in Book mode) when zoomed or in continuous / FitWidth / FitTextWidth
                if dy.abs() > 0.1 && self.scroll_or_turn_page_vertical((-dy) as f32) {
                    return;
                }

                // Otherwise turn page(s) cleanly with 120ms debounce
                let now = Instant::now();
                if let Some(last) = self.last_wheel_turn {
                    if now.duration_since(last) < Duration::from_millis(120) {
                        return;
                    }
                }
                self.wheel_accum_y += dy;
                if self.wheel_accum_y <= -18.0 {
                    self.wheel_accum_y = 0.0;
                    self.last_wheel_turn = Some(now);
                    self.step_pages_forward();
                } else if self.wheel_accum_y >= 18.0 {
                    self.wheel_accum_y = 0.0;
                    self.last_wheel_turn = Some(now);
                    self.step_pages_backward();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = (position.x, position.y);
                let (win_w, win_h, sf, sidebar_w, menu_bar_h, tab_bar_h, top_bars_h, status_bar_h) =
                    if let Some(w) = self.window.as_ref() {
                        let sz = w.inner_size();
                        let sf = self.effective_scale_factor(w.scale_factor());
                        let mb_h = self.menu_bar_height_px(sf).min(sz.height.saturating_sub(48));
                        let tb_h = self.tab_bar_height_px(sf).min(sz.height.saturating_sub(mb_h + 24));
                        let st_h = self.status_bar_height_px(sf).min(sz.height.saturating_sub(mb_h + tb_h + 16));
                        (
                            sz.width.max(1),
                            sz.height.max(1),
                            sf,
                            self.sidebar_width_px(sf).min(sz.width.saturating_sub(64)),
                            mb_h,
                            tb_h,
                            mb_h + tb_h,
                            st_h,
                        )
                    } else {
                        (1024, 1080, 1.0, 0, MENU_BAR_HEIGHT_LOGICAL, TAB_BAR_HEIGHT_LOGICAL, MENU_BAR_HEIGHT_LOGICAL + TAB_BAR_HEIGHT_LOGICAL, STATUS_BAR_HEIGHT_LOGICAL)
                    };

                if self.scrollbar_drag {
                    self.scroll_to_scrollbar_y(position.y, win_w, win_h, top_bars_h, status_bar_h, sf);
                    return;
                }
                if self.hscrollbar_drag {
                    self.scroll_to_hscrollbar_x(position.x, win_w, win_h, status_bar_h, sf);
                    return;
                }
                if self.md_editor.as_ref().is_some_and(|e| e.scrollbar_drag) {
                    if let Some((ex, ey, ew, eh)) = self.md_editor_rect(sidebar_w, top_bars_h, win_w, win_h, status_bar_h, sf) {
                        let zoom = self.zoom_level;
                        if let Some(ref mut ed) = self.md_editor {
                            ed.scroll_to_scrollbar_y(position.y, ex, ey, ew, eh, sf, zoom);
                            self.request_redraw();
                        }
                    }
                    return;
                }
                if self.md_editor.as_ref().is_some_and(|e| e.dragging) {
                    if let Some((ex, ey, ew, _)) = self.md_editor_rect(sidebar_w, top_bars_h, win_w, win_h, status_bar_h, sf) {
                        let zoom = self.zoom_level;
                        if let Some(ref mut ed) = self.md_editor {
                            ed.cursor = ed.hit_pos(position.x.round() as i32, position.y.round() as i32, ex, ey, ew, sf, zoom);
                            self.request_redraw();
                        }
                    }
                    return;
                }
                if let Some((lx, ly)) = self.hand_pan_drag_last {
                    let dx = (lx - position.x) as f32;
                    let dy = (ly - position.y) as f32;
                    self.hand_pan_drag_last = Some((position.x, position.y));
                    let mut changed = false;
                    if dx.abs() > 0.25 {
                        changed |= self.pan_horizontal(dx);
                    }
                    if dy.abs() > 0.25 {
                        if self.effective_scroll_mode() == ScrollMode::ContinuousVertical {
                            self.scroll_y += dy;
                            changed = true;
                        } else {
                            let content_h = win_h.saturating_sub(top_bars_h + status_bar_h);
                            let bmp_h = self.page_draw_rect.3;
                            if bmp_h > content_h {
                                let max_s = (bmp_h - content_h) as f32;
                                let next_y = (self.scroll_y + dy).clamp(0.0, max_s);
                                if (next_y - self.scroll_y).abs() > 0.25 {
                                    self.scroll_y = next_y;
                                    changed = true;
                                }
                            }
                        }
                    }
                    if changed {
                        self.request_redraw();
                    }
                    return;
                }
                if self.deskew_bar.as_ref().is_some_and(|d| d.dragging_slider) {
                    self.update_deskew_slider_from_x(position.x, win_w, sidebar_w, top_bars_h, sf);
                    return;
                }
                if self.stamp_manager_modal.as_ref().is_some_and(|s| s.pad_drawing) {
                    self.append_signature_pad_point(position.x, position.y, win_w, win_h, sf, false);
                    return;
                }
                if let Some(last) = self.compare_mode.as_ref().and_then(|c| c.pan_drag_last) {
                    let dx = (last.0 - position.x) as f32;
                    let dy = (last.1 - position.y) as f32;
                    if let Some(ref mut cmp) = self.compare_mode {
                        cmp.pan_drag_last = Some((position.x, position.y));
                    }
                    self.scroll_compare_synced(dx, dy);
                    return;
                }

                // Compute unified hover widget and only redraw when hover actually changes
                let mut new_hover: Option<UiHit> = None;
                let mut menu_switched = false;

                if self.unsaved_changes_modal.is_some() {
                    if let Some(h) = self.hit_test_unsaved_modal(position.x, position.y, win_w, win_h, sf) {
                        if h != super::doc_props_modal::UnsavedChangesHit::InsideModal {
                            new_hover = Some(UiHit::UnsavedChanges(h));
                        }
                    }
                } else if self.manual_duplex_wait.is_some() {
                    if let Some(h) = self.hit_test_manual_duplex_modal(position.x, position.y, win_w, win_h, sf) {
                        if h != super::print_preview::ManualDuplexHit::InsideModal {
                            new_hover = Some(UiHit::ManualDuplex(h));
                        }
                    }
                } else if self.print_preview.is_some() {
                    if let Some(h) = self.hit_test_print_preview(position.x, position.y, win_w, win_h, sf) {
                        if h != super::print_preview::PrintPreviewHit::InsideModal {
                            new_hover = Some(UiHit::PrintPreview(h));
                        }
                    }
                } else if self.doc_props_modal.is_some() {
                    if let Some(h) = self.hit_test_doc_props_modal(position.x, position.y, win_w, win_h, sf) {
                        if h != super::doc_props_modal::DocPropsHit::InsideModal {
                            new_hover = Some(UiHit::DocProps(h));
                        }
                    }
                } else if self.create_pdf_images_modal.is_some() {
                    if let Some(h) = self.hit_test_create_pdf_images_modal(position.x, position.y, win_w, win_h, sf) {
                        if h != super::image_tools_modal::CreatePdfImagesHit::InsideModal {
                            new_hover = Some(UiHit::CreatePdfImages(h));
                        }
                    }
                } else if self.export_images_modal.is_some() {
                    if let Some(h) = self.hit_test_export_images_modal(position.x, position.y, win_w, win_h, sf) {
                        if h != super::image_tools_modal::ExportImagesHit::InsideModal {
                            new_hover = Some(UiHit::ExportImages(h));
                        }
                    }
                } else if self.excel_export_modal.is_some() {
                    if let Some(h) = self.hit_test_excel_export_modal(position.x, position.y, win_w, win_h, sf) {
                        if h != super::excel_export_modal::ExcelExportHit::InsideModal {
                            new_hover = Some(UiHit::ExcelExport(h));
                        }
                    }
                } else if self.markdown_export_modal.is_some() {
                    if let Some(h) = self.hit_test_markdown_export_modal(
                        position.x.round() as i32,
                        position.y.round() as i32,
                        win_w,
                        win_h,
                        sf,
                    ) {
                        if h != super::markdown_export_modal::MarkdownExportHit::InsideModal {
                            new_hover = Some(UiHit::MarkdownExport(h));
                        }
                    }
                } else if self.split_pages_modal.is_some() {
                    if let Some(h) = self.hit_test_split_pages_modal(position.x, position.y, win_w, win_h, sf) {
                        if h != super::split_pages_modal::SplitPagesHit::InsideModal {
                            new_hover = Some(UiHit::SplitPages(h));
                        }
                    }
                } else if self.watermark_modal.is_some() {
                    if let Some(h) = self.hit_test_watermark_modal(position.x, position.y, win_w, win_h, sf) {
                        if let Some(m) = self.watermark_modal.as_mut() {
                            m.font_dropdown.hover_idx = match h {
                                super::watermark_modal::WatermarkHit::FontDropdownPopup(
                                    super::font_dropdown::FontDropdownHit::Row(idx),
                                ) => Some(idx),
                                _ => None,
                            };
                        }
                        if h != super::watermark_modal::WatermarkHit::InsideModal {
                            new_hover = Some(UiHit::Watermark(h));
                        }
                    }
                } else if self.stamp_manager_modal.is_some() {
                    if let Some(h) = self.hit_test_stamp_manager_modal(position.x, position.y, win_w, win_h, sf) {
                        if h != super::stamp_manager_modal::StampManagerHit::InsideModal {
                            new_hover = Some(UiHit::StampManager(h));
                        }
                    }
                } else if self.combine_modal.is_some() {
                    if let Some(h) = self.hit_test_combine_modal(position.x, position.y, win_w, win_h, sf) {
                        if h != super::combine_modal::CombineHit::InsideModal {
                            new_hover = Some(UiHit::Combine(h));
                        }
                    }
                } else if self.header_footer_modal.is_some() {
                    if let Some(h) = self.hit_test_header_footer_modal(
                        position.x.round() as i32,
                        position.y.round() as i32,
                        win_w,
                        win_h,
                        sf,
                    ) {
                        if let Some(m) = self.header_footer_modal.as_mut() {
                            m.font_dropdown.hover_idx = match h {
                                super::doc_tools_modal::HeaderFooterHit::FontDropdownPopup(
                                    super::font_dropdown::FontDropdownHit::Row(idx),
                                ) => Some(idx),
                                _ => None,
                            };
                        }
                        if h != super::doc_tools_modal::HeaderFooterHit::InsideModal {
                            new_hover = Some(UiHit::HeaderFooter(h));
                        }
                    }
                } else if self.page_number_modal.is_some() {
                    if let Some(h) = self.hit_test_page_number_modal(
                        position.x.round() as i32,
                        position.y.round() as i32,
                        win_w,
                        win_h,
                        sf,
                    ) {
                        if let Some(m) = self.page_number_modal.as_mut() {
                            m.font_dropdown.hover_idx = match h {
                                super::doc_tools_modal::PageNumberHit::FontDropdownPopup(
                                    super::font_dropdown::FontDropdownHit::Row(idx),
                                ) => Some(idx),
                                _ => None,
                            };
                        }
                        if h != super::doc_tools_modal::PageNumberHit::InsideModal {
                            new_hover = Some(UiHit::PageNumber(h));
                        }
                    }
                } else if self.user_guide_modal.is_some() {
                    if let Some(h) = self.hit_test_user_guide_modal(position.x, position.y, win_w, win_h, sf) {
                        if h != super::user_guide::UserGuideHit::InsideModal {
                            new_hover = Some(UiHit::UserGuide(h));
                        }
                    }
                } else if self.show_bookmark_format_overlay {
                    if let Some(h) = self.hit_test_bookmark_format_overlay(position.x, position.y, win_w, win_h, sf) {
                        new_hover = Some(h);
                    }
                } else if self.show_customize_toolbar {
                    if let Some(h) = self.hit_test_customize_toolbar_modal(position.x, position.y, win_w, win_h, sf) {
                        if h != super::toolbar::CustomizeHit::InsideModal {
                            new_hover = Some(UiHit::Customize(h));
                        }
                    }
                } else if self.open_menu.is_some() {
                    if let Some(hovered_cat) = self.hit_test_menu_bar_header(position.x, position.y, menu_bar_h, sf) {
                        if self.open_menu != Some(hovered_cat) {
                            self.open_menu = Some(hovered_cat);
                            menu_switched = true;
                        }
                        new_hover = Some(UiHit::MenuHeader(hovered_cat));
                    } else if let Some((_, idx)) = self.hit_test_open_menu_dropdown(position.x, position.y, win_w, menu_bar_h, sf) {
                        new_hover = Some(UiHit::MenuItem(idx));
                    }
                } else if self.tab_context_menu.is_some() {
                    if let Some(h) = self.hit_test_tab_context_menu(position.x, position.y, win_w, win_h, sf) {
                        new_hover = Some(UiHit::TabContext(h));
                    }
                } else if self.show_tab_list_popover {
                    if let Some(h) = self.hit_test_tab_list_popover(position.x, position.y, win_w, win_h, top_bars_h, sf) {
                        if h != TabListHit::InsidePopover {
                            new_hover = Some(UiHit::TabList(h));
                        }
                    }
                } else if self.thumb_context_menu.is_some() && self.view_mode == ViewMode::ThumbnailGrid {
                    if let Some(h) = self.hit_test_thumb_context_menu(position.x, position.y, win_w, win_h, sf) {
                        if h != ThumbContextHit::InsideMenu {
                            new_hover = Some(UiHit::ThumbContext(h));
                        }
                    }
                } else if self.annot_context_menu.is_some() {
                    if let Some(h) = self.hit_test_annot_context_menu(position.x, position.y, win_w, win_h, sf) {
                        if h != super::doc_props_modal::AnnotContextHit::InsideMenu {
                            new_hover = Some(UiHit::AnnotContext(h));
                        }
                    }
                } else {
                    if self.active_note_popover.is_some() {
                        if let Some(h) = self.hit_test_note_popover(position.x, position.y, win_w, win_h, sf) {
                            if h != super::doc_props_modal::NotePopoverHit::InsidePopover {
                                new_hover = Some(UiHit::NotePopover(h));
                            }
                        }
                    }
                    if new_hover.is_none() && self.view_mode == ViewMode::ThumbnailGrid && self.selected_thumbs.len() >= 2 {
                        if let Some(h) = self.hit_test_thumb_batch_bar(position.x, position.y, win_w, win_h, sidebar_w, status_bar_h, sf) {
                            if h != ThumbBatchHit::InsideBar {
                                new_hover = Some(UiHit::ThumbBatch(h));
                            }
                        }
                    }
                    if new_hover.is_none() && self.show_reading_mode_popover {
                        if let Some(Some(mode)) = self.hit_test_reading_mode_popover(
                            position.x,
                            position.y,
                            win_w,
                            win_h,
                            status_bar_h,
                            sf,
                        ) {
                            new_hover = Some(UiHit::ReadingModeOption(mode));
                        }
                    }
                    if new_hover.is_none() && self.show_notifications {
                        if let Some(on_clear) = self.hit_test_notification_board(position.x, position.y, win_w, win_h, status_bar_h, sf) {
                            if on_clear {
                                new_hover = Some(UiHit::NotificationClear);
                            }
                        }
                    }
                    if new_hover.is_none() && self.input_mode != InputMode::Normal {
                        if let Some(h) = self.hit_test_active_input_overlay(position.x, position.y, win_w, top_bars_h, sf) {
                            if h != super::overlays::OverlayHit::InsideModal {
                                new_hover = Some(UiHit::Overlay(h));
                            }
                        }
                    }
                    if new_hover.is_none() && self.compare_mode.is_some() {
                        if let Some(h) = self.hit_test_compare_bar(position.x, position.y, win_w, sidebar_w, top_bars_h, sf) {
                            if h != super::compare_mode::CompareHit::InsideBar {
                                new_hover = Some(UiHit::Compare(h));
                            }
                        }
                    }
                    if new_hover.is_none() && self.compare_mode.is_none() && self.deskew_bar.is_some() && self.view_mode.is_page_view() {
                        if let Some(h) = self.hit_test_deskew_bar(position.x, position.y, win_w, sidebar_w, top_bars_h, sf) {
                            if h != super::deskew::DeskewHit::InsideBar {
                                new_hover = Some(UiHit::Deskew(h));
                            }
                        }
                    }
                    if new_hover.is_none() && self.compare_mode.is_none() && self.mouse_mode == MouseMode::Measure && self.view_mode.is_page_view() {
                        if let Some(h) = self.hit_test_measure_bar(position.x, position.y, win_w, sidebar_w, top_bars_h, sf) {
                            if h != super::measure::MeasureBarHit::InsideBar {
                                new_hover = Some(UiHit::MeasureBar(h));
                            }
                        }
                    }
                    if new_hover.is_none() && (position.y as i32) < menu_bar_h as i32 {
                        if let Some(cat) = self.hit_test_menu_bar_header(position.x, position.y, menu_bar_h, sf) {
                            new_hover = Some(UiHit::MenuHeader(cat));
                        } else {
                            let tb_x = self.toolbar_start_x(sf);
                            if let Some(act) = self.hit_test_inline_toolbar(position.x, position.y, tb_x, win_w, menu_bar_h, sf) {
                                new_hover = Some(UiHit::Toolbar(act));
                            }
                        }
                    } else if new_hover.is_none() && (position.y as i32) < top_bars_h as i32 {
                        if let Some(th) = self.hit_test_tab_bar(position.x, position.y, win_w, menu_bar_h, tab_bar_h, sf) {
                            new_hover = Some(match th {
                                TabBarHit::ScrollLeft => UiHit::TabScrollLeft,
                                TabBarHit::ScrollRight => UiHit::TabScrollRight,
                                TabBarHit::SwitchTab(i) => UiHit::Tab(i),
                                TabBarHit::CloseTab(i) => UiHit::TabClose(i),
                                TabBarHit::NewTab => UiHit::TabNew,
                                TabBarHit::TabListToggle => UiHit::TabListToggle,
                            });
                        }
                    } else if new_hover.is_none()
                        && status_bar_h > 0
                        && (position.y as i32) >= win_h.saturating_sub(status_bar_h) as i32
                    {
                        if let Some(act) = self.hit_test_bottom_bar(position.x, position.y, win_w, win_h, status_bar_h, sf) {
                            new_hover = Some(UiHit::BottomBar(act));
                        }
                    } else if new_hover.is_none() && self.show_bookmarks && (position.x as u32) < sidebar_w {
                        if let Some(sh) = self.hit_test_bookmark_sidebar(position.x, position.y, sidebar_w, top_bars_h, sf) {
                            new_hover = Some(match sh {
                                SidebarHit::AddBookmarkButton => UiHit::SidebarAdd,
                                SidebarHit::ImportTxtButton => UiHit::SidebarImport,
                                SidebarHit::ExportTxtButton => UiHit::SidebarExport,
                                SidebarHit::AutoDetectButton => UiHit::SidebarAuto,
                                SidebarHit::MoveUpButton => UiHit::SidebarMoveUp,
                                SidebarHit::MoveDownButton => UiHit::SidebarMoveDown,
                                SidebarHit::OutdentButton => UiHit::SidebarOutdent,
                                SidebarHit::IndentButton => UiHit::SidebarIndent,
                                SidebarHit::BookmarkRow { index, on_delete: true } => UiHit::SidebarDelete(index),
                                SidebarHit::BookmarkRow { index, on_delete: false } => UiHit::SidebarRow(index),
                            });
                        }
                    } else if new_hover.is_none() && self.doc.is_none() && !self.is_markdown_tab() {
                        if let Some(oh) = self.hit_test_welcome(position.x, position.y, win_w, win_h, top_bars_h, status_bar_h, sf) {
                            new_hover = Some(UiHit::Overlay(oh));
                        }
                    } else if new_hover.is_none() && self.is_markdown_tab() {
                        if let Some(mh) = self.hit_test_markdown_view(position.x, position.y) {
                            new_hover = Some(UiHit::MarkdownView(mh));
                        }
                    } else if new_hover.is_none() && self.view_mode == ViewMode::TextView {
                        if let Some(th) = self.hit_test_text_view(position.x, position.y, sidebar_w, top_bars_h, win_w, sf) {
                            new_hover = Some(UiHit::TextView(th));
                        }
                    } else if new_hover.is_none() {
                        let ix = position.x.round() as i32;
                        let iy = position.y.round() as i32;
                        if let Some((tx, ty, tw, th, _, _)) = self.horizontal_scrollbar_geometry(win_w, win_h, status_bar_h, sf) {
                            if ix >= tx && ix < tx + tw as i32 && iy >= ty - 4 && iy <= ty + th as i32 + 4 {
                                new_hover = Some(UiHit::HScrollbar);
                            }
                        }
                        if new_hover.is_none() {
                            if let Some((tx, ty, tw, th, _, _)) = self.vertical_scrollbar_geometry(win_w, win_h, top_bars_h, status_bar_h, sf) {
                                if ix >= tx - 4 && ix <= tx + tw as i32 + 4 && iy >= ty && iy < ty + th as i32 {
                                    new_hover = Some(UiHit::Scrollbar);
                                }
                            }
                        }
                    }
                }

                let hover_changed = new_hover != self.hovered_widget;
                if hover_changed {
                    self.hovered_widget = new_hover;
                }
                if hover_changed || menu_switched {
                    if let Some(w) = self.window.as_ref() {
                        let pointer = new_hover.is_some()
                            || self.hovered_link.is_some()
                            || self.hovered_form_field.is_some();
                        let grab = !pointer
                            && (self.mouse_mode == MouseMode::Hand
                                || self.compare_mode.as_ref().is_some_and(|c| c.max_pan_x > 1.0 || c.max_scroll_y > 1.0));
                        w.set_cursor(if pointer {
                            CursorIcon::Pointer
                        } else if grab {
                            CursorIcon::Grab
                        } else {
                            CursorIcon::Default
                        });
                    }
                    self.request_redraw();
                }

                if self.has_blocking_modal() || self.has_open_popover() {
                    return;
                }

                if self.dragging_annotation.is_some() {
                    self.request_redraw();
                    return;
                }

                if self.drag_start.is_some() || self.grid_drag_page.is_some() {
                    if let Some((sx1, sy1)) = self.drag_start {
                        if self.mouse_mode == MouseMode::InkPen && self.view_mode.is_page_view() {
                            let px = position.x as f32;
                            let py = position.y as f32;
                            let should_push = self.ink_stroke_screen_pts.last().map_or(true, |&(lx, ly)| {
                                (px - lx).hypot(py - ly) >= 2.5
                            });
                            if should_push {
                                self.ink_stroke_screen_pts.push((px, py));
                            }
                        } else if self.mouse_mode == MouseMode::Select && self.view_mode.is_page_view() {
                            if let Some((page_idx, pdf_rect)) = self.screen_drag_to_pdf_rect(
                                sx1,
                                sy1,
                                position.x,
                                position.y,
                                false,
                            ) {
                                self.select_text_in_pdf_rect(page_idx, pdf_rect);
                            }
                        }
                    }
                    self.request_redraw();
                } else if self.compare_mode.is_none()
                    && self.view_mode.is_page_view()
                    && new_hover.is_none()
                    && self.deskew_bar.as_ref().is_some_and(|d| d.draw_guideline)
                {
                    if let Some(w) = self.window.as_ref() {
                        w.set_cursor(CursorIcon::Crosshair);
                    }
                    if self.deskew_bar.as_ref().is_some_and(|d| d.guide_start.is_some() && d.guide_end.is_none()) {
                        self.request_redraw();
                    }
                } else if self.compare_mode.is_none() && self.view_mode.is_page_view() && new_hover.is_none() {
                    let new_link = self.resolve_link_at_screen_pos(position.x, position.y);
                    let new_form = if new_link.is_none() && self.mouse_mode == MouseMode::Select {
                        self.screen_point_to_pdf_point(position.x, position.y).and_then(|(page_idx, pt_x, pt_y)| {
                            let doc = self.doc.as_ref()?;
                            let page = doc.pages().get(page_idx).ok()?;
                            PdfEngine::find_form_field_at_point(&page, pt_x.value, pt_y.value)
                                .map(|f| (page_idx, f.bounds))
                        })
                    } else {
                        None
                    };
                    let form_changed = new_form != self.hovered_form_field;
                    let link_changed = new_link != self.hovered_link;
                    if link_changed || form_changed {
                        let is_interactive = new_link.is_some() || new_form.is_some();
                        self.hovered_link = new_link;
                        self.hovered_form_field = new_form;
                        if let Some(w) = self.window.as_ref() {
                            w.set_cursor(if is_interactive {
                                CursorIcon::Pointer
                            } else if self.mouse_mode == MouseMode::Hand {
                                CursorIcon::Grab
                            } else {
                                CursorIcon::Default
                            });
                        }
                        if form_changed {
                            self.request_redraw();
                        }
                    }
                } else if self.view_mode == ViewMode::ThumbnailGrid {
                    let new_hover_thumb = self
                        .hit_test_thumbnail(position.x, position.y, win_w, sidebar_w, top_bars_h)
                        .map(|(idx, _)| idx);
                    if new_hover_thumb != self.hovered_thumb {
                        self.hovered_thumb = new_hover_thumb;
                        self.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if state == ElementState::Released && self.scrollbar_drag {
                    self.scrollbar_drag = false;
                    self.request_redraw();
                    return;
                }
                if state == ElementState::Released && self.hscrollbar_drag {
                    self.hscrollbar_drag = false;
                    self.request_redraw();
                    return;
                }
                if state == ElementState::Released && self.md_editor.as_ref().is_some_and(|e| e.dragging || e.scrollbar_drag) {
                    if let Some(ref mut ed) = self.md_editor {
                        ed.dragging = false;
                        ed.scrollbar_drag = false;
                    }
                    self.request_redraw();
                    return;
                }
                if state == ElementState::Released && self.hand_pan_drag_last.is_some() {
                    self.hand_pan_drag_last = None;
                    if let Some(w) = self.window.as_ref() {
                        w.set_cursor(if self.mouse_mode == MouseMode::Hand {
                            CursorIcon::Grab
                        } else {
                            CursorIcon::Default
                        });
                    }
                    self.request_redraw();
                    return;
                }
                if state == ElementState::Released && self.deskew_bar.as_ref().is_some_and(|d| d.dragging_slider) {
                    if let Some(ref mut d) = self.deskew_bar {
                        d.dragging_slider = false;
                    }
                    self.request_redraw();
                    return;
                }

                let (win_w, win_h, scale_factor, sidebar_w, menu_bar_h, tab_bar_h, top_bars_h, status_bar_h) =
                    if let Some(w) = self.window.as_ref() {
                        let sz = w.inner_size();
                        let sf = self.effective_scale_factor(w.scale_factor());
                        let mb_h = self.menu_bar_height_px(sf).min(sz.height.saturating_sub(48));
                        let tb_h = self
                            .tab_bar_height_px(sf)
                            .min(sz.height.saturating_sub(mb_h + 24));
                        let st_h = self
                            .status_bar_height_px(sf)
                            .min(sz.height.saturating_sub(mb_h + tb_h + 16));
                        (
                            sz.width.max(1),
                            sz.height.max(1),
                            sf,
                            self.sidebar_width_px(sf).min(sz.width.saturating_sub(64)),
                            mb_h,
                            tb_h,
                            mb_h + tb_h,
                            st_h,
                        )
                    } else {
                        (
                            1024,
                            1080,
                            1.0,
                            0,
                            MENU_BAR_HEIGHT_LOGICAL,
                            TAB_BAR_HEIGHT_LOGICAL,
                            MENU_BAR_HEIGHT_LOGICAL + TAB_BAR_HEIGHT_LOGICAL,
                            STATUS_BAR_HEIGHT_LOGICAL,
                        )
                    };

                // Highest-priority modal: Unsaved Changes Confirmation Dialog
                if self.unsaved_changes_modal.is_some() {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if let Some(hit) = self.hit_test_unsaved_modal(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            win_h,
                            scale_factor,
                        ) {
                            self.handle_unsaved_modal_click(hit);
                        }
                    }
                    return;
                }

                // Top-priority modal: Guided Manual Duplex Pass 2 Confirmation
                if self.manual_duplex_wait.is_some() {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if let Some(hit) = self.hit_test_manual_duplex_modal(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            win_h,
                            scale_factor,
                        ) {
                            self.handle_manual_duplex_click(hit);
                        }
                    }
                    return;
                }

                // Document Properties & Privacy Scrubber modal
                if self.doc_props_modal.is_some() {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        match self.hit_test_doc_props_modal(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            win_h,
                            scale_factor,
                        ) {
                            Some(hit) => self.handle_doc_props_click(hit),
                            None => {
                                self.doc_props_modal = None;
                                self.request_redraw();
                            }
                        }
                    }
                    return;
                }

                // Create PDF from Images modal
                if self.create_pdf_images_modal.is_some() {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if let Some(hit) = self.hit_test_create_pdf_images_modal(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            win_h,
                            scale_factor,
                        ) {
                            self.handle_create_pdf_images_click(hit);
                        }
                    }
                    return;
                }

                // Export Pages as Images modal
                if self.export_images_modal.is_some() {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if let Some(hit) = self.hit_test_export_images_modal(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            win_h,
                            scale_factor,
                        ) {
                            self.handle_export_images_click(hit);
                        }
                    }
                    return;
                }

                // Export PDF to Excel options modal
                if self.excel_export_modal.is_some() {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        match self.hit_test_excel_export_modal(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            win_h,
                            scale_factor,
                        ) {
                            Some(hit) => self.handle_excel_export_click(hit),
                            None => {
                                self.excel_export_modal = None;
                                self.request_redraw();
                            }
                        }
                    }
                    return;
                }

                // Export Markdown modal
                if self.markdown_export_modal.is_some() {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        match self.hit_test_markdown_export_modal(
                            self.cursor_pos.0.round() as i32,
                            self.cursor_pos.1.round() as i32,
                            win_w,
                            win_h,
                            scale_factor,
                        ) {
                            Some(hit) => self.handle_markdown_export_click(hit),
                            None => {
                                self.markdown_export_modal = None;
                                self.request_redraw();
                            }
                        }
                    }
                    return;
                }

                // Split to Single-Page PDFs modal
                if self.split_pages_modal.is_some() {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        match self.hit_test_split_pages_modal(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            win_h,
                            scale_factor,
                        ) {
                            Some(hit) => self.handle_split_pages_click(hit),
                            None => {
                                self.split_pages_modal = None;
                                self.request_redraw();
                            }
                        }
                    }
                    return;
                }

                // Detailed Watermark modal
                if self.watermark_modal.is_some() {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        match self.hit_test_watermark_modal(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            win_h,
                            scale_factor,
                        ) {
                            Some(hit) => self.handle_watermark_click(hit),
                            None => {
                                self.watermark_modal = None;
                                self.request_redraw();
                            }
                        }
                    }
                    return;
                }

                // Portable Stamp & Signature Manager modal (with Freehand Signature Pad drawing)
                if self.stamp_manager_modal.is_some() {
                    if state == ElementState::Released && button == MouseButton::Left {
                        if let Some(ref mut st) = self.stamp_manager_modal {
                            st.pad_drawing = false;
                        }
                        return;
                    }
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        match self.hit_test_stamp_manager_modal(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            win_h,
                            scale_factor,
                        ) {
                            Some(super::stamp_manager_modal::StampManagerHit::DrawPad) => {
                                self.append_signature_pad_point(
                                    self.cursor_pos.0,
                                    self.cursor_pos.1,
                                    win_w,
                                    win_h,
                                    scale_factor,
                                    true,
                                );
                            }
                            Some(hit) => self.handle_stamp_manager_click(hit),
                            None => {
                                self.stamp_manager_modal = None;
                                self.request_redraw();
                            }
                        }
                    }
                    return;
                }

                // Print Preview modal
                if self.print_preview.is_some() {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        match self.hit_test_print_preview(self.cursor_pos.0, self.cursor_pos.1, win_w, win_h, scale_factor) {
                            Some(hit) => self.handle_print_preview_click(hit),
                            None => self.close_print_preview(),
                        }
                    }
                    return;
                }

                // Combine Multiple PDFs modal
                if self.combine_modal.is_some() {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if let Some(hit) = self.hit_test_combine_modal(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            win_h,
                            scale_factor,
                        ) {
                            self.handle_combine_click(hit);
                        }
                    }
                    return;
                }

                // Header & Footer visual modal
                if self.header_footer_modal.is_some() {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if let Some(hit) = self.hit_test_header_footer_modal(
                            self.cursor_pos.0.round() as i32,
                            self.cursor_pos.1.round() as i32,
                            win_w,
                            win_h,
                            scale_factor,
                        ) {
                            self.handle_header_footer_click(hit);
                        }
                    }
                    return;
                }

                // Page Numbering visual modal
                if self.page_number_modal.is_some() {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if let Some(hit) = self.hit_test_page_number_modal(
                            self.cursor_pos.0.round() as i32,
                            self.cursor_pos.1.round() as i32,
                            win_w,
                            win_h,
                            scale_factor,
                        ) {
                            self.handle_page_number_click(hit);
                        }
                    }
                    return;
                }

                // Favorites Library modal (`File -> Favorites Library... (Ctrl+D)`)
                if self.favorites_modal.is_some() {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        self.handle_favorites_click(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            win_h,
                            scale_factor,
                        );
                    }
                    return;
                }

                // Built-in Step-by-Step User Guide modal (`Help -> User Guide (How to Use)`)
                if self.user_guide_modal.is_some() {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        match self.hit_test_user_guide_modal(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            win_h,
                            scale_factor,
                        ) {
                            Some(hit) => self.handle_user_guide_click(hit),
                            None => self.close_user_guide_modal(),
                        }
                    }
                    return;
                }

                // Bookmark Import Format Guide modal (`Help -> Bookmark Import Format (.txt)`)
                if self.show_bookmark_format_overlay {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        match self.hit_test_bookmark_format_overlay(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            win_h,
                            scale_factor,
                        ) {
                            Some(UiHit::BookmarkFormatSaveSample) => {
                                self.pending_dialog = Some(PendingDialog::SaveBookmarkSampleTxt);
                            }
                            Some(UiHit::BookmarkFormatClose) => {
                                self.show_bookmark_format_overlay = false;
                                self.request_redraw();
                            }
                            _ => {}
                        }
                    }
                    return;
                }

                // Help & About modals
                if self.show_help_overlay || self.show_about_overlay {
                    if state == ElementState::Pressed {
                        self.show_help_overlay = false;
                        self.show_about_overlay = false;
                        self.request_redraw();
                    }
                    return;
                }

                // Customize Toolbar modal
                if self.show_customize_toolbar {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        match self.hit_test_customize_toolbar_modal(self.cursor_pos.0, self.cursor_pos.1, win_w, win_h, scale_factor) {
                            Some(hit) => self.handle_customize_toolbar_click(hit),
                            None => {
                                self.show_customize_toolbar = false;
                                self.request_redraw();
                            }
                        }
                    }
                    return;
                }

                // 0a. If a Top Menu dropdown is open, check if the user clicked an item or header
                if self.open_menu.is_some() {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if let Some((cat, item_idx)) = self.hit_test_open_menu_dropdown(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            menu_bar_h,
                            scale_factor,
                        ) {
                            self.execute_menu_item(cat, item_idx);
                            return;
                        }
                        if let Some(clicked_cat) = self.hit_test_menu_bar_header(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            menu_bar_h,
                            scale_factor,
                        ) {
                            if self.open_menu == Some(clicked_cat) {
                                self.open_menu = None;
                            } else {
                                self.open_menu = Some(clicked_cat);
                            }
                            self.request_redraw();
                            return;
                        }
                        self.open_menu = None;
                        self.request_redraw();
                    }
                    return;
                }

                // 0a-tabctx. Right-click Tab Context Menu
                if self.tab_context_menu.is_some() {
                    if state == ElementState::Pressed {
                        if button == MouseButton::Left {
                            if let Some(hit) = self.hit_test_tab_context_menu(
                                self.cursor_pos.0,
                                self.cursor_pos.1,
                                win_w,
                                win_h,
                                scale_factor,
                            ) {
                                self.execute_tab_context_action(hit);
                            } else {
                                self.tab_context_menu = None;
                                self.request_redraw();
                            }
                            return;
                        } else if button == MouseButton::Right {
                            self.tab_context_menu = None;
                            self.request_redraw();
                        }
                    } else {
                        return;
                    }
                }

                // 0a-annotctx. Right-click Annotation Inspector Context Menu
                if self.annot_context_menu.is_some() {
                    if state == ElementState::Pressed {
                        if button == MouseButton::Left {
                            if let Some(hit) = self.hit_test_annot_context_menu(
                                self.cursor_pos.0,
                                self.cursor_pos.1,
                                win_w,
                                win_h,
                                scale_factor,
                            ) {
                                self.handle_annot_context_click(hit);
                            } else {
                                self.annot_context_menu = None;
                                self.request_redraw();
                            }
                            return;
                        } else if button == MouseButton::Right {
                            self.annot_context_menu = None;
                            self.request_redraw();
                        }
                    } else {
                        return;
                    }
                }

                // 0a-notepop. Multi-line Sticky Note Reader Popover (Edit / Delete / Close)
                if self.active_note_popover.is_some() && state == ElementState::Pressed && button == MouseButton::Left {
                    if let Some(hit) = self.hit_test_note_popover(
                        self.cursor_pos.0,
                        self.cursor_pos.1,
                        win_w,
                        win_h,
                        scale_factor,
                    ) {
                        self.handle_note_popover_click(hit);
                        return;
                    }
                }

                // 0a-tablist. All-Tabs Overflow Dropdown Popover (`▾`)
                if self.show_tab_list_popover && state == ElementState::Pressed && button == MouseButton::Left {
                    if let Some(hit) = self.hit_test_tab_list_popover(
                        self.cursor_pos.0,
                        self.cursor_pos.1,
                        win_w,
                        win_h,
                        top_bars_h,
                        scale_factor,
                    ) {
                        match hit {
                            TabListHit::SwitchTab(idx) => {
                                self.switch_to_tab(idx);
                            }
                            TabListHit::CloseTab(idx) => {
                                let _ = self.request_close_action(
                                    super::doc_props_modal::PendingCloseAction::CloseTab(idx),
                                );
                            }
                            TabListHit::CloseOthers => {
                                let _ = self.request_close_action(
                                    super::doc_props_modal::PendingCloseAction::CloseOtherTabs(
                                        self.active_tab,
                                    ),
                                );
                            }
                            TabListHit::CloseAll => {
                                let _ = self.request_close_action(
                                    super::doc_props_modal::PendingCloseAction::CloseAllTabs,
                                );
                            }
                            TabListHit::InsidePopover => {}
                        }
                        return;
                    }
                    let clicking_toggle = (self.cursor_pos.1 as i32) >= menu_bar_h as i32
                        && (self.cursor_pos.1 as i32) < top_bars_h as i32
                        && self.hit_test_tab_bar(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            menu_bar_h,
                            tab_bar_h,
                            scale_factor,
                        ) == Some(TabBarHit::TabListToggle);
                    if !clicking_toggle {
                        self.show_tab_list_popover = false;
                        self.request_redraw();
                        return;
                    }
                }

                // 0a-thumbctx. Thumbnail Grid Right-Click Context Menu
                if self.thumb_context_menu.is_some() && self.view_mode == ViewMode::ThumbnailGrid {
                    if state == ElementState::Pressed {
                        if button == MouseButton::Left {
                            if let Some(hit) = self.hit_test_thumb_context_menu(
                                self.cursor_pos.0,
                                self.cursor_pos.1,
                                win_w,
                                win_h,
                                scale_factor,
                            ) {
                                self.handle_thumb_context_click(hit);
                            } else {
                                self.thumb_context_menu = None;
                                self.request_redraw();
                            }
                            return;
                        } else if button == MouseButton::Right {
                            self.thumb_context_menu = None;
                            self.request_redraw();
                        }
                    } else {
                        return;
                    }
                }

                // 0a0. Reading Mode picker popover clicks
                if self.show_reading_mode_popover && state == ElementState::Pressed && button == MouseButton::Left {
                    if let Some(opt_mode) = self.hit_test_reading_mode_popover(
                        self.cursor_pos.0,
                        self.cursor_pos.1,
                        win_w,
                        win_h,
                        status_bar_h,
                        scale_factor,
                    ) {
                        if let Some(mode) = opt_mode {
                            self.show_reading_mode_popover = false;
                            self.set_reading_filter(mode);
                        }
                        return;
                    }
                    let clicking_toggle = status_bar_h > 0
                        && (self.cursor_pos.1 as i32) >= win_h.saturating_sub(status_bar_h) as i32
                        && self.hit_test_bottom_bar(self.cursor_pos.0, self.cursor_pos.1, win_w, win_h, status_bar_h, scale_factor)
                            == Some(super::bottom_bar::BottomBarAction::ReadingFilter);
                    if !clicking_toggle {
                        self.show_reading_mode_popover = false;
                        self.request_redraw();
                    }
                }

                // 0a1. Session Notification Board popover clicks
                if self.show_notifications && state == ElementState::Pressed && button == MouseButton::Left {
                    if let Some(on_clear) = self.hit_test_notification_board(
                        self.cursor_pos.0,
                        self.cursor_pos.1,
                        win_w,
                        win_h,
                        status_bar_h,
                        scale_factor,
                    ) {
                        if on_clear {
                            self.notifications.clear();
                            self.last_recorded_notice = None;
                            self.status_notice = None;
                            self.request_redraw();
                        }
                        return;
                    }
                    // Clicking outside closes the notification board unless clicking its toggle button itself
                    let clicking_toggle = status_bar_h > 0
                        && (self.cursor_pos.1 as i32) >= win_h.saturating_sub(status_bar_h) as i32
                        && self.hit_test_bottom_bar(self.cursor_pos.0, self.cursor_pos.1, win_w, win_h, status_bar_h, scale_factor)
                            == Some(super::bottom_bar::BottomBarAction::NotificationBoard);
                    if !clicking_toggle {
                        self.show_notifications = false;
                        self.request_redraw();
                    }
                }

                // 0a2. Active input overlay card / Floating Search pill clicks
                if self.input_mode != InputMode::Normal {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if let Some(oh) = self.hit_test_active_input_overlay(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            top_bars_h,
                            scale_factor,
                        ) {
                            self.handle_overlay_click(oh);
                            return;
                        }
                    }
                }

                // 0b. Unified Top Bar clicks (Menus on left + Inline Customizable Toolbar beside Help)
                if (self.cursor_pos.1 as i32) < menu_bar_h as i32 {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if let Some(cat) = self.hit_test_menu_bar_header(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            menu_bar_h,
                            scale_factor,
                        ) {
                            self.open_menu = Some(cat);
                            self.request_redraw();
                            return;
                        }
                        let tb_x = self.toolbar_start_x(scale_factor);
                        if let Some(action) = self.hit_test_inline_toolbar(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            tb_x,
                            win_w,
                            menu_bar_h,
                            scale_factor,
                        ) {
                            self.execute_toolbar_action(action);
                        }
                    }
                    return;
                }

                // 0c. Multi-Tab Bar interactions (`menu_bar_h .. top_bars_h`)
                if (self.cursor_pos.1 as i32) < top_bars_h as i32 {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if let Some(hit) = self.hit_test_tab_bar(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            menu_bar_h,
                            tab_bar_h,
                            scale_factor,
                        ) {
                            match hit {
                                TabBarHit::ScrollLeft => {
                                    self.scroll_tab_strip(-1);
                                }
                                TabBarHit::ScrollRight => {
                                    self.scroll_tab_strip(1);
                                }
                                TabBarHit::SwitchTab(idx) => {
                                    self.switch_to_tab(idx);
                                }
                                TabBarHit::CloseTab(idx) => {
                                    if !self.tabs.is_empty() {
                                        let _ = self.request_close_action(
                                            super::doc_props_modal::PendingCloseAction::CloseTab(idx),
                                        );
                                    }
                                }
                                TabBarHit::NewTab => {
                                    self.pending_dialog = Some(PendingDialog::Open);
                                }
                                TabBarHit::TabListToggle => {
                                    self.show_tab_list_popover = !self.show_tab_list_popover;
                                    self.tab_list_scroll = 0;
                                    self.tab_context_menu = None;
                                    self.request_redraw();
                                }
                            }
                        }
                    } else if state == ElementState::Pressed && button == MouseButton::Right {
                        if !self.tabs.is_empty() {
                            if let Some(TabBarHit::SwitchTab(idx) | TabBarHit::CloseTab(idx)) =
                                self.hit_test_tab_bar(
                                    self.cursor_pos.0,
                                    self.cursor_pos.1,
                                    win_w,
                                    menu_bar_h,
                                    tab_bar_h,
                                    scale_factor,
                                )
                            {
                                self.show_tab_list_popover = false;
                                self.tab_context_menu = Some((
                                    idx,
                                    self.cursor_pos.0.round() as i32,
                                    self.cursor_pos.1.round() as i32,
                                ));
                                self.request_redraw();
                            }
                        }
                    }
                    return;
                }

                // 0c2. Bottom View Bar clicks (`win_h - status_bar_h .. win_h`)
                if status_bar_h > 0
                    && (self.cursor_pos.1 as i32) >= win_h.saturating_sub(status_bar_h) as i32
                {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if let Some(action) = self.hit_test_bottom_bar(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            win_h,
                            status_bar_h,
                            scale_factor,
                        ) {
                            self.execute_bottom_bar_action(action);
                        }
                    }
                    return;
                }

                // 0d. Welcome Screen clicks when no document is open
                if self.doc.is_none() && !self.is_markdown_tab() {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if let Some(oh) = self.hit_test_welcome(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            win_h,
                            top_bars_h,
                            status_bar_h,
                            scale_factor,
                        ) {
                            self.handle_overlay_click(oh);
                        }
                    }
                    return;
                }

                // 0d0. Thin overlay Horizontal & Vertical Scrollbars (work in both Normal Window & Compare Window)
                if state == ElementState::Pressed && button == MouseButton::Left {
                    let ix = self.cursor_pos.0.round() as i32;
                    let iy = self.cursor_pos.1.round() as i32;
                    if let Some((tx, ty, tw, th, _, _)) = self.horizontal_scrollbar_geometry(
                        win_w,
                        win_h,
                        status_bar_h,
                        scale_factor,
                    ) {
                        if ix >= tx && ix < tx + tw as i32 && iy >= ty - 4 && iy <= ty + th as i32 + 4 {
                            self.hscrollbar_drag = true;
                            self.scroll_to_hscrollbar_x(
                                self.cursor_pos.0,
                                win_w,
                                win_h,
                                status_bar_h,
                                scale_factor,
                            );
                            return;
                        }
                    }
                    if let Some((tx, ty, tw, th, _, _)) = self.vertical_scrollbar_geometry(
                        win_w,
                        win_h,
                        top_bars_h,
                        status_bar_h,
                        scale_factor,
                    ) {
                        if ix >= tx - 4 && ix <= tx + tw as i32 + 4 && iy >= ty && iy < ty + th as i32 {
                            self.scrollbar_drag = true;
                            self.scroll_to_scrollbar_y(
                                self.cursor_pos.1,
                                win_w,
                                win_h,
                                top_bars_h,
                                status_bar_h,
                                scale_factor,
                            );
                            return;
                        }
                    }
                }

                // 0d1. Side-by-Side Compare Mode top control bar + synced drag-to-pan
                if self.compare_mode.is_some() {
                    if state == ElementState::Released && button == MouseButton::Left {
                        if let Some(ref mut cmp) = self.compare_mode {
                            cmp.pan_drag_last = None;
                        }
                        if let Some(w) = self.window.as_ref() {
                            let grab = self.mouse_mode == MouseMode::Hand
                                || self.compare_mode.as_ref().is_some_and(|c| c.max_pan_x > 1.0 || c.max_scroll_y > 1.0);
                            w.set_cursor(if grab { CursorIcon::Grab } else { CursorIcon::Default });
                        }
                        return;
                    }
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if let Some(hit) = self.hit_test_compare_bar(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            sidebar_w,
                            top_bars_h,
                            scale_factor,
                        ) {
                            self.handle_compare_bar_click(hit);
                            return;
                        }
                        if !(self.show_bookmarks && (self.cursor_pos.0 as u32) < sidebar_w) {
                            if let Some(ref mut cmp) = self.compare_mode {
                                cmp.pan_drag_last = Some(self.cursor_pos);
                            }
                            if let Some(w) = self.window.as_ref() {
                                w.set_cursor(CursorIcon::Grabbing);
                            }
                            return;
                        }
                    }
                    // Allow clicking Bookmark Sidebar during Compare Mode, otherwise ignore canvas clicks
                    if !(self.show_bookmarks && (self.cursor_pos.0 as u32) < sidebar_w) {
                        return;
                    }
                }

                // 0d1a. Lightweight Matrix Page Deskew bar + Draw Guideline canvas interaction
                if self.deskew_bar.is_some() && self.view_mode.is_page_view() {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if let Some(hit) = self.hit_test_deskew_bar(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            sidebar_w,
                            top_bars_h,
                            scale_factor,
                        ) {
                            self.handle_deskew_bar_click(
                                hit,
                                self.cursor_pos.0,
                                win_w,
                                sidebar_w,
                                top_bars_h,
                                scale_factor,
                            );
                            return;
                        }
                    }
                    let on_canvas = self.cursor_pos.0 >= sidebar_w as f64
                        && self.cursor_pos.1 >= (top_bars_h + self.deskew_bar_height_px(scale_factor)) as f64
                        && self.cursor_pos.1 < win_h.saturating_sub(status_bar_h) as f64;
                    if button == MouseButton::Left
                        && on_canvas
                        && self.deskew_bar.as_ref().is_some_and(|d| d.draw_guideline)
                    {
                        match state {
                            ElementState::Pressed => {
                                let second_click_p1 = self.deskew_bar.as_ref().and_then(|d| {
                                    if !d.guide_dragging && d.guide_end.is_none() {
                                        d.guide_start
                                    } else {
                                        None
                                    }
                                });
                                if let Some(p1) = second_click_p1 {
                                    self.commit_deskew_guideline(p1, self.cursor_pos);
                                } else if let Some(ref mut d) = self.deskew_bar {
                                    d.focus = super::deskew::DeskewTextFocus::None;
                                    d.guide_start = Some(self.cursor_pos);
                                    d.guide_end = None;
                                    d.guide_dragging = true;
                                    self.request_redraw();
                                }
                                return;
                            }
                            ElementState::Released => {
                                let drag_p1 = self.deskew_bar.as_ref().and_then(|d| {
                                    if d.guide_dragging { d.guide_start } else { None }
                                });
                                if let Some(p1) = drag_p1 {
                                    let dist = (self.cursor_pos.0 - p1.0).hypot(self.cursor_pos.1 - p1.1);
                                    if dist >= 6.0 {
                                        self.commit_deskew_guideline(p1, self.cursor_pos);
                                    } else if let Some(ref mut d) = self.deskew_bar {
                                        // Stay waiting for second click in two-click mode
                                        d.guide_dragging = false;
                                        self.request_redraw();
                                    }
                                    return;
                                }
                            }
                        }
                    }
                }

                // 0d1b. Quick Measure / Ruler floating bar clicks
                if self.mouse_mode == MouseMode::Measure && self.view_mode.is_page_view() {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if let Some(hit) = self.hit_test_measure_bar(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            sidebar_w,
                            top_bars_h,
                            scale_factor,
                        ) {
                            self.handle_measure_bar_click(hit);
                            return;
                        }
                    }
                }

                // 0d2. Markdown Reader View controls, links & live editor clicks
                if self.is_markdown_tab() {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if let Some(hit) = self.hit_test_markdown_view(self.cursor_pos.0, self.cursor_pos.1) {
                            self.handle_markdown_click(hit);
                            return;
                        }
                        if let Some((ex, ey, ew, eh)) = self.md_editor_rect(sidebar_w, top_bars_h, win_w, win_h, status_bar_h, scale_factor) {
                            let px = self.cursor_pos.0.round() as i32;
                            let py = self.cursor_pos.1.round() as i32;
                            if px >= ex && px < ex + ew as i32 && py >= ey && py < ey + eh as i32 {
                                self.ensure_md_editor_initialized();
                                let shift = self.shift_pressed;
                                let zoom = self.zoom_level;
                                let py_f = self.cursor_pos.1;
                                if let Some(ref mut ed) = self.md_editor {
                                    if ed.hit_scrollbar(px, py, ex, ey, ew, eh, scale_factor, zoom) {
                                        ed.scrollbar_drag = true;
                                        ed.scroll_to_scrollbar_y(py_f, ex, ey, ew, eh, scale_factor, zoom);
                                    } else {
                                        let old = ed.cursor;
                                        let pos = ed.hit_pos(px, py, ex, ey, ew, scale_factor, zoom);
                                        ed.cursor = pos;
                                        ed.sel_anchor = if shift { ed.sel_anchor.or(Some(old)) } else { Some(pos) };
                                        ed.dragging = true;
                                    }
                                }
                                self.request_redraw();
                                return;
                            }
                        }
                    }
                }

                // 0d2. Reflowable Text View header controls
                if self.view_mode == ViewMode::TextView {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if let Some(hit) = self.hit_test_text_view(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            sidebar_w,
                            top_bars_h,
                            win_w,
                            scale_factor,
                        ) {
                            self.handle_text_view_click(hit);
                            return;
                        }
                    }
                }

                // 0e. Thin right-edge overlay vertical scrollbar click/drag
                if state == ElementState::Pressed && button == MouseButton::Left {
                    if let Some((tx, ty, tw, th, _, _)) = self.vertical_scrollbar_geometry(
                        win_w,
                        win_h,
                        top_bars_h,
                        status_bar_h,
                        scale_factor,
                    ) {
                        let ix = self.cursor_pos.0.round() as i32;
                        let iy = self.cursor_pos.1.round() as i32;
                        if ix >= tx - 4 && ix <= tx + tw as i32 + 4 && iy >= ty && iy < ty + th as i32 {
                            self.scrollbar_drag = true;
                            self.scroll_to_scrollbar_y(
                                self.cursor_pos.1,
                                win_w,
                                win_h,
                                top_bars_h,
                                status_bar_h,
                                scale_factor,
                            );
                            return;
                        }
                    }
                }

                // 1. Check Left Bookmark Sidebar interactions
                if self.show_bookmarks && (self.cursor_pos.0 as u32) < sidebar_w {
                    if state == ElementState::Pressed {
                        if let Some(hit) = self.hit_test_bookmark_sidebar(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            sidebar_w,
                            top_bars_h,
                            scale_factor,
                        ) {
                            match hit {
                                SidebarHit::AddBookmarkButton => {
                                    if button == MouseButton::Left {
                                        self.input_mode = InputMode::AddBookmark {
                                            title_input: String::new(),
                                        };
                                        self.update_title_bar();
                                        self.request_redraw();
                                    }
                                }
                                SidebarHit::ImportTxtButton => {
                                    if button == MouseButton::Left {
                                        self.pending_dialog =
                                            Some(PendingDialog::ImportBookmarksTxt);
                                    }
                                }
                                SidebarHit::ExportTxtButton => {
                                    if button == MouseButton::Left {
                                        self.pending_dialog =
                                            Some(PendingDialog::ExportBookmarksTxt);
                                    }
                                }
                                SidebarHit::AutoDetectButton => {
                                    if button == MouseButton::Left {
                                        self.auto_detect_bookmarks_from_headings();
                                    }
                                }
                                SidebarHit::MoveUpButton => {
                                    if button == MouseButton::Left {
                                        self.move_selected_bookmark(-1);
                                    }
                                }
                                SidebarHit::MoveDownButton => {
                                    if button == MouseButton::Left {
                                        self.move_selected_bookmark(1);
                                    }
                                }
                                SidebarHit::OutdentButton => {
                                    if button == MouseButton::Left {
                                        self.adjust_selected_bookmark_level(-1);
                                    }
                                }
                                SidebarHit::IndentButton => {
                                    if button == MouseButton::Left {
                                        self.adjust_selected_bookmark_level(1);
                                    }
                                }
                                SidebarHit::BookmarkRow {
                                    index: bm_idx,
                                    on_delete,
                                } => match button {
                                    MouseButton::Left => {
                                        if on_delete {
                                            self.push_metadata_undo_snapshot();
                                            let removed = self.bookmarks.remove(bm_idx);
                                            if self.bookmarks.is_empty() {
                                                self.selected_bookmark = None;
                                            } else if bm_idx >= self.bookmarks.len() {
                                                self.selected_bookmark =
                                                    Some(self.bookmarks.len() - 1);
                                            }
                                            self.persist_bookmarks();
                                            self.status_notice = Some(format!(
                                                "Deleted bookmark '{}' (Ctrl+Z to undo, S to save)",
                                                removed.title
                                            ));
                                            self.update_title_bar();
                                            self.request_redraw();
                                        } else if self.is_markdown_tab() {
                                            self.selected_bookmark = Some(bm_idx);
                                            let block_idx = self.bookmarks[bm_idx].page_index;
                                            self.scroll_to_md_heading(block_idx);
                                        } else {
                                            self.selected_bookmark = Some(bm_idx);
                                            let target_page =
                                                self.bookmarks[bm_idx].page_index as u16;
                                            self.push_nav_history();
                                            self.current_page_index = target_page
                                                .min(self.total_pages().saturating_sub(1));
                                            self.scroll_y = 0.0;
                                            self.invalidate_page_caches();
                                            self.update_title_bar();
                                            self.request_redraw();
                                        }
                                    }
                                    MouseButton::Right => {
                                        // Right-click bookmark row to Edit Bookmark Title
                                        self.selected_bookmark = Some(bm_idx);
                                        self.input_mode = InputMode::EditBookmark {
                                            bookmark_idx: bm_idx,
                                            title_input: self.bookmarks[bm_idx].title.clone(),
                                        };
                                        self.update_title_bar();
                                        self.request_redraw();
                                    }
                                    MouseButton::Middle => {
                                        // Middle-click bookmark row to Delete Bookmark
                                        self.push_metadata_undo_snapshot();
                                        let removed = self.bookmarks.remove(bm_idx);
                                        if self.bookmarks.is_empty() {
                                            self.selected_bookmark = None;
                                        } else if bm_idx >= self.bookmarks.len() {
                                            self.selected_bookmark =
                                                Some(self.bookmarks.len() - 1);
                                        }
                                        self.persist_bookmarks();
                                        self.status_notice =
                                            Some(format!("Deleted bookmark '{}' (Ctrl+Z to undo)", removed.title));
                                        self.update_title_bar();
                                        self.request_redraw();
                                    }
                                    _ => {}
                                },
                            }
                        }
                    }
                    return;
                }

                // 2. Thumbnail Grid View mouse interactions
                if self.view_mode == ViewMode::ThumbnailGrid {
                    if button == MouseButton::Left {
                        match state {
                            ElementState::Pressed => {
                                // First check floating multi-select Batch Action Bar
                                if self.selected_thumbs.len() >= 2 {
                                    if let Some(hit) = self.hit_test_thumb_batch_bar(
                                        self.cursor_pos.0,
                                        self.cursor_pos.1,
                                        win_w,
                                        win_h,
                                        sidebar_w,
                                        status_bar_h,
                                        scale_factor,
                                    ) {
                                        self.handle_thumb_batch_click(hit);
                                        return;
                                    }
                                }
                                if let Some((clicked_idx, badge_hit)) = self.hit_test_thumbnail(
                                    self.cursor_pos.0,
                                    self.cursor_pos.1,
                                    win_w,
                                    sidebar_w,
                                    top_bars_h,
                                ) {
                                    match badge_hit {
                                        ThumbBadgeHit::BwConvert => {
                                            self.convert_page_to_bw_in_place(clicked_idx);
                                            return;
                                        }
                                        ThumbBadgeHit::Rotate => {
                                            self.rotate_page_clockwise(clicked_idx);
                                            return;
                                        }
                                        ThumbBadgeHit::None => {}
                                    }

                                    // Ctrl/Cmd+Click or Shift+Click multi-select
                                    if self.cmd_or_ctrl_pressed || self.shift_pressed {
                                        self.select_thumbnail_with_modifiers(
                                            clicked_idx,
                                            self.cmd_or_ctrl_pressed,
                                            self.shift_pressed,
                                        );
                                        self.last_thumb_click = None;
                                        self.grid_drag_page = None;
                                        let sel_n = self.selected_thumbs.len();
                                        if sel_n > 1 {
                                            self.status_notice = Some(format!(
                                                "{sel_n} pages selected — use the bottom bar or right-click for batch actions"
                                            ));
                                        }
                                        self.update_title_bar();
                                        self.request_redraw();
                                        return;
                                    }

                                    // Check for double-click to jump into Page View
                                    let now = Instant::now();
                                    if let Some((last_idx, last_time)) = self.last_thumb_click {
                                        if last_idx == clicked_idx
                                            && now.duration_since(last_time)
                                                < Duration::from_millis(350)
                                        {
                                            self.selected_thumbs.clear();
                                            self.current_page_index = clicked_idx;
                                            self.last_thumb_click = None;
                                            self.toggle_view_mode();
                                            return;
                                        }
                                    }
                                    self.select_thumbnail_with_modifiers(clicked_idx, false, false);
                                    self.last_thumb_click = Some((clicked_idx, now));
                                    self.grid_drag_page = Some((
                                        clicked_idx,
                                        self.cursor_pos.0,
                                        self.cursor_pos.1,
                                    ));
                                    self.update_title_bar();
                                    self.request_redraw();
                                }
                            }
                            ElementState::Released => {
                                if let Some((src_idx, sx, sy)) = self.grid_drag_page.take() {
                                    let (cx, cy) = self.cursor_pos;
                                    let dist = ((cx - sx).powi(2) + (cy - sy).powi(2)).sqrt();
                                    if dist > 10.0 {
                                        let dst_idx = self.grid_drop_target_index(
                                            cx, cy, win_w, sidebar_w, top_bars_h,
                                        );
                                        if dst_idx != src_idx {
                                            self.push_undo_snapshot();
                                            if let Some(doc) = self.doc.as_mut() {
                                                match self.engine.move_page_in_doc(
                                                    doc, src_idx, dst_idx,
                                                ) {
                                                    Ok(()) => {
                                                        PdfEngine::remap_bookmarks_after_move(
                                                            &mut self.bookmarks,
                                                            src_idx as usize,
                                                            dst_idx as usize,
                                                            self.skip_pages,
                                                            self.roman_end,
                                                        );
                                                        let next_bw: HashSet<u16> = self
                                                            .bw_pages
                                                            .iter()
                                                            .map(|&p| {
                                                                if p == src_idx {
                                                                    dst_idx
                                                                } else if src_idx < dst_idx
                                                                    && p > src_idx
                                                                    && p <= dst_idx
                                                                {
                                                                    p - 1
                                                                } else if dst_idx < src_idx
                                                                    && p >= dst_idx
                                                                    && p < src_idx
                                                                {
                                                                    p + 1
                                                                } else {
                                                                    p
                                                                }
                                                            })
                                                            .collect();
                                                        self.bw_pages = next_bw;
                                                        self.search_matches.clear();
                                                        self.current_search_match = 0;
                                                        self.has_unsaved_edits = true;
                                                        self.current_page_index = dst_idx;
                                                        self.invalidate_page_caches();
                                                        self.thumbnail_cache.clear();
                                                        self.status_notice = Some(format!(
                                                            "Moved Page {} to Page {}",
                                                            src_idx + 1,
                                                            dst_idx + 1
                                                        ));
                                                    }
                                                    Err(e) => {
                                                        let _ = self.undo_stack.pop_back();
                                                        self.status_notice = Some(format!(
                                                            "Move page failed: {e}"
                                                        ));
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    self.update_title_bar();
                                    self.request_redraw();
                                }
                            }
                        }
                    } else if button == MouseButton::Right && state == ElementState::Pressed {
                        if let Some((clicked_idx, _)) = self.hit_test_thumbnail(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            sidebar_w,
                            top_bars_h,
                        ) {
                            self.current_page_index = clicked_idx;
                            self.thumb_context_menu = Some(ThumbContextMenuState {
                                page_idx: clicked_idx,
                                x: self.cursor_pos.0.round() as i32,
                                y: self.cursor_pos.1.round() as i32,
                                blank_count: 1,
                            });
                            self.update_title_bar();
                            self.request_redraw();
                        }
                    }
                    return;
                }

                // In Presentation Mode (F5), Left-Click advances a page and Right-Click goes back
                if self.is_presentation {
                    if state == ElementState::Pressed {
                        match button {
                            MouseButton::Left => {
                                self.step_pages_forward();
                            }
                            MouseButton::Right => {
                                self.step_pages_backward();
                            }
                            _ => {}
                        }
                    }
                    return;
                }

                if self.view_mode == ViewMode::TextView {
                    return;
                }

                // 3. Page View mouse drag interactions: Select, Move Note/TextBox, Annotations (8 tools), Redact, Crop, StampImage
                if button == MouseButton::Left {
                    match state {
                        ElementState::Pressed => {
                            if self.active_note_popover.is_some() {
                                self.active_note_popover = None;
                                self.request_redraw();
                            }
                            if self.mouse_mode == MouseMode::Hand {
                                self.hand_pan_drag_last = Some(self.cursor_pos);
                                if let Some(w) = self.window.as_ref() {
                                    w.set_cursor(CursorIcon::Grabbing);
                                }
                                return;
                            }
                            // Check if grabbing an existing Sticky Note or Text Box to drag-reposition it
                            if matches!(
                                self.mouse_mode,
                                MouseMode::Select | MouseMode::StickyNote | MouseMode::FreeText
                            ) {
                                if let Some((page_idx, pt_x, pt_y)) =
                                    self.screen_point_to_pdf_point(self.cursor_pos.0, self.cursor_pos.1)
                                {
                                    let hit_movable = self.doc.as_ref().and_then(|doc| {
                                        let page = doc.pages().get(page_idx).ok()?;
                                        // Prefer topmost (last-created) matching Sticky Note or Text Box
                                        PdfEngine::find_movable_annotations_on_page(&page)
                                            .into_iter()
                                            .rev()
                                            .find(|m| {
                                                pt_x.value >= m.bounds.left().value - 6.0
                                                    && pt_x.value <= m.bounds.right().value + 6.0
                                                    && pt_y.value >= m.bounds.bottom().value - 6.0
                                                    && pt_y.value <= m.bounds.top().value + 6.0
                                            })
                                    });
                                    if let Some(m) = hit_movable {
                                        self.dragging_annotation = Some(DraggingAnnotation {
                                            page_index: page_idx,
                                            annot_index: m.index,
                                            is_sticky_note: m.is_sticky_note,
                                            orig_rect: m.bounds,
                                            start_pdf_x: pt_x.value,
                                            start_pdf_y: pt_y.value,
                                            start_screen_x: self.cursor_pos.0,
                                            start_screen_y: self.cursor_pos.1,
                                            note_text: if m.text.is_empty() { None } else { Some(m.text) },
                                        });
                                        return;
                                    }
                                }
                            }
                            self.drag_start = Some(self.cursor_pos);
                            if self.mouse_mode == MouseMode::InkPen {
                                self.ink_stroke_screen_pts =
                                    vec![(self.cursor_pos.0 as f32, self.cursor_pos.1 as f32)];
                            }
                        }
                        ElementState::Released => {
                            if let Some(drag) = self.dragging_annotation.take() {
                                let (sx2, sy2) = self.cursor_pos;
                                let moved = (sx2 - drag.start_screen_x).abs() >= 5.0
                                    || (sy2 - drag.start_screen_y).abs() >= 5.0;
                                if moved {
                                    if let Some((end_page, end_x, end_y)) =
                                        self.screen_point_to_pdf_point(sx2, sy2)
                                    {
                                        if end_page == drag.page_index {
                                            let dx = end_x.value - drag.start_pdf_x;
                                            let dy = end_y.value - drag.start_pdf_y;
                                            self.push_undo_snapshot();
                                            if let Some(doc) = self.doc.as_mut() {
                                                match PdfEngine::move_annotation_by(
                                                    doc,
                                                    drag.page_index,
                                                    drag.annot_index,
                                                    dx,
                                                    dy,
                                                ) {
                                                    Ok(()) => {
                                                        self.has_unsaved_edits = true;
                                                        self.invalidate_page_caches();
                                                        self.thumbnail_cache.remove(&drag.page_index);
                                                        let kind = if drag.is_sticky_note {
                                                            "Sticky Note"
                                                        } else {
                                                            "Text Box"
                                                        };
                                                        self.status_notice = Some(format!(
                                                            "Moved {kind} on Page {} (Ctrl/Cmd+Z to undo)",
                                                            drag.page_index + 1
                                                        ));
                                                    }
                                                    Err(e) => {
                                                        let _ = self.undo_stack.pop_back();
                                                        self.status_notice = Some(format!("Move annotation error: {e}"));
                                                    }
                                                }
                                            }
                                        }
                                    }
                                } else if drag.is_sticky_note {
                                    if let Some(txt) = drag.note_text {
                                        self.active_note_popover = Some(
                                            super::doc_props_modal::ActiveNotePopover {
                                                page_idx: drag.page_index,
                                                annot_idx: drag.annot_index,
                                                text: txt,
                                                x: sx2.round() as i32,
                                                y: sy2.round() as i32,
                                            },
                                        );
                                    }
                                }
                                self.update_title_bar();
                                self.request_redraw();
                                return;
                            }
                            if let Some((sx1, sy1)) = self.drag_start.take() {
                                let (sx2, sy2) = self.cursor_pos;
                                let is_click = (sx2 - sx1).abs() < 5.0 && (sy2 - sy1).abs() < 5.0;

                                // Freehand Ink Pen stroke commit
                                if self.mouse_mode == MouseMode::InkPen {
                                    let mut screen_pts = std::mem::take(&mut self.ink_stroke_screen_pts);
                                    screen_pts.push((sx2 as f32, sy2 as f32));
                                    if screen_pts.len() >= 2 {
                                        let mut target_page: Option<u16> = None;
                                        let mut pdf_pts: Vec<(f32, f32)> = Vec::with_capacity(screen_pts.len());
                                        for (px, py) in screen_pts {
                                            if let Some((p_idx, pt_x, pt_y)) =
                                                self.screen_point_to_pdf_point(px as f64, py as f64)
                                            {
                                                if target_page.is_none() {
                                                    target_page = Some(p_idx);
                                                }
                                                if target_page == Some(p_idx) {
                                                    pdf_pts.push((pt_x.value, pt_y.value));
                                                }
                                            }
                                        }
                                        if let Some(page_idx) = target_page {
                                            if pdf_pts.len() >= 2 {
                                                let color = self.annotation_color;
                                                self.push_undo_snapshot();
                                                if let Some(doc) = self.doc.as_mut() {
                                                    match PdfEngine::add_freehand_ink(
                                                        doc, page_idx, &pdf_pts, color, 2.0,
                                                    ) {
                                                        Ok(()) => {
                                                            self.has_unsaved_edits = true;
                                                            self.invalidate_page_caches();
                                                            self.thumbnail_cache.remove(&page_idx);
                                                            self.status_notice = Some(format!(
                                                                "Added {} ink stroke on Page {} (Ctrl/Cmd+Z to undo)",
                                                                color.label(),
                                                                page_idx + 1
                                                            ));
                                                        }
                                                        Err(e) => {
                                                            let _ = self.undo_stack.pop_back();
                                                            self.status_notice = Some(format!("Ink pen error: {e}"));
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    self.update_title_bar();
                                    self.request_redraw();
                                    return;
                                }

                                // Sticky Note placement click
                                if self.mouse_mode == MouseMode::StickyNote {
                                    if let Some((page_idx, pt_x, pt_y)) =
                                        self.screen_point_to_pdf_point(sx2, sy2)
                                    {
                                        self.current_page_index = page_idx;
                                        self.input_mode = InputMode::StickyNotePrompt {
                                            page_idx,
                                            pdf_x: pt_x.value,
                                            pdf_y: pt_y.value,
                                            text_input: String::new(),
                                        };
                                        self.status_notice = None;
                                    }
                                    self.update_title_bar();
                                    self.request_redraw();
                                    return;
                                }

                                // FreeText / Text Box placement click
                                if self.mouse_mode == MouseMode::FreeText {
                                    if let Some((page_idx, pt_x, pt_y)) =
                                        self.screen_point_to_pdf_point(sx2, sy2)
                                    {
                                        self.current_page_index = page_idx;
                                        self.input_mode = InputMode::FreeTextPrompt {
                                            page_idx,
                                            pdf_x: pt_x.value,
                                            pdf_y: pt_y.value,
                                            text_input: String::new(),
                                        };
                                        self.status_notice = None;
                                    }
                                    self.update_title_bar();
                                    self.request_redraw();
                                    return;
                                }

                                // Quick Measure / Ruler Tool commit
                                if self.mouse_mode == MouseMode::Measure {
                                    if !is_click {
                                        if let (
                                            Some((p1, x1, y1)),
                                            Some((p2, x2, y2)),
                                        ) = (
                                            self.screen_point_to_pdf_point(sx1, sy1),
                                            self.screen_point_to_pdf_point(sx2, sy2),
                                        ) {
                                            if p1 == p2 {
                                                let m = super::measure::MeasurementResult {
                                                    page_idx: p1,
                                                    x1_pt: x1.value,
                                                    y1_pt: y1.value,
                                                    x2_pt: x2.value,
                                                    y2_pt: y2.value,
                                                };
                                                let st = super::measure::compute_measurement_stats(
                                                    m.x1_pt,
                                                    m.y1_pt,
                                                    m.x2_pt,
                                                    m.y2_pt,
                                                    self.measure_unit,
                                                );
                                                let u = self.measure_unit.label();
                                                let au = self.measure_unit.area_label();
                                                self.last_measurement = Some(m);
                                                self.status_notice = Some(format!(
                                                    "Measured: {:.2} {u}  (ΔX {:.2} × ΔY {:.2} {u}, Area {:.2} {au}, {:.1}°)",
                                                    st.length, st.dx, st.dy, st.area, st.angle_deg
                                                ));
                                            }
                                        }
                                    }
                                    self.update_title_bar();
                                    self.request_redraw();
                                    return;
                                }

                                // Vector Arrow shape commit (directional drag from start -> end)
                                if self.mouse_mode == MouseMode::ArrowShape {
                                    if !is_click {
                                        if let (
                                            Some((p1, x1, y1)),
                                            Some((p2, x2, y2)),
                                        ) = (
                                            self.screen_point_to_pdf_point(sx1, sy1),
                                            self.screen_point_to_pdf_point(sx2, sy2),
                                        ) {
                                            if p1 == p2 {
                                                let color = self.annotation_color;
                                                self.push_undo_snapshot();
                                                if let Some(doc) = self.doc.as_mut() {
                                                    match PdfEngine::add_arrow_shape(
                                                        doc, p1, x1.value, y1.value, x2.value, y2.value, color, 2.0,
                                                    ) {
                                                        Ok(()) => {
                                                            self.has_unsaved_edits = true;
                                                            self.invalidate_page_caches();
                                                            self.thumbnail_cache.remove(&p1);
                                                            self.status_notice = Some(format!(
                                                                "Added {} arrow on Page {} (Ctrl/Cmd+Z to undo)",
                                                                color.label(),
                                                                p1 + 1
                                                            ));
                                                        }
                                                        Err(e) => {
                                                            let _ = self.undo_stack.pop_back();
                                                            self.status_notice = Some(format!("Arrow error: {e}"));
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    self.update_title_bar();
                                    self.request_redraw();
                                    return;
                                }

                                if is_click && self.mouse_mode != MouseMode::StampImage {
                                    if let Some(link_target) =
                                        self.resolve_link_at_screen_pos(sx2, sy2)
                                    {
                                        match link_target {
                                            ResolvedLinkTarget::InternalPage(target_p) => {
                                                let total = self.total_pages();
                                                if total > 0 {
                                                    self.push_nav_history();
                                                    self.current_page_index =
                                                        target_p.min(total - 1);
                                                    self.scroll_y = 0.0;
                                                    self.invalidate_page_caches();
                                                    self.status_notice = Some(format!(
                                                        "Followed link to Page {} (Alt+Left to go Back)",
                                                        self.current_page_index + 1
                                                    ));
                                                }
                                            }
                                            ResolvedLinkTarget::ExternalUrl(url) => {
                                                self.input_mode =
                                                    InputMode::ConfirmExternalUrl { url };
                                            }
                                        }
                                        self.update_title_bar();
                                        self.request_redraw();
                                        return;
                                    }

                                    if self.mouse_mode == MouseMode::Select {
                                        let now = Instant::now();
                                        if let Some((page_idx, pt_x, pt_y)) =
                                            self.screen_point_to_pdf_point(sx2, sy2)
                                        {
                                            // Check if user clicked on an existing Sticky Note annotation to read it
                                            let hit_note = self.doc.as_ref().and_then(|doc| {
                                                let page = doc.pages().get(page_idx).ok()?;
                                                PdfEngine::find_movable_annotations_on_page(&page)
                                                    .into_iter()
                                                    .rev()
                                                    .find(|m| {
                                                        m.is_sticky_note
                                                            && pt_x.value >= m.bounds.left().value - 6.0
                                                            && pt_x.value <= m.bounds.right().value + 6.0
                                                            && pt_y.value >= m.bounds.bottom().value - 6.0
                                                            && pt_y.value <= m.bounds.top().value + 6.0
                                                    })
                                            });
                                            if let Some(m) = hit_note {
                                                self.active_note_popover = Some(
                                                    super::doc_props_modal::ActiveNotePopover {
                                                        page_idx,
                                                        annot_idx: m.index,
                                                        text: m.text,
                                                        x: sx2.round() as i32,
                                                        y: sy2.round() as i32,
                                                    },
                                                );
                                                self.request_redraw();
                                                return;
                                            }

                                            // Check if user clicked on a fillable PDF Form Field (Text, Checkbox, Radio)
                                            let hit_form = self.doc.as_ref().and_then(|doc| {
                                                let page = doc.pages().get(page_idx).ok()?;
                                                PdfEngine::find_form_field_at_point(&page, pt_x.value, pt_y.value)
                                            });
                                            if let Some(field) = hit_form {
                                                match field.kind {
                                                    crate::engine::FormFieldKind::Checkbox
                                                    | crate::engine::FormFieldKind::Radio => {
                                                        self.push_undo_snapshot();
                                                        if let Some(doc) = self.doc.as_mut() {
                                                            match PdfEngine::toggle_form_button_at(
                                                                doc,
                                                                page_idx,
                                                                field.annot_index,
                                                            ) {
                                                                Ok(msg) => {
                                                                    self.invalidate_page_caches();
                                                                    self.thumbnail_cache.remove(&page_idx);
                                                                    self.status_notice = Some(format!(
                                                                        "{msg} on Page {} (Ctrl+Z to undo, S to save)",
                                                                        page_idx + 1
                                                                    ));
                                                                }
                                                                Err(e) => {
                                                                    let _ = self.undo_stack.pop_back();
                                                                    self.status_notice = Some(format!(
                                                                        "Form toggle failed: {e}"
                                                                    ));
                                                                }
                                                            }
                                                        }
                                                    }
                                                    crate::engine::FormFieldKind::Text => {
                                                        self.input_mode = InputMode::EditFormField {
                                                            page_idx,
                                                            annot_idx: field.annot_index,
                                                            field_name: field.name,
                                                            value_input: field.value,
                                                        };
                                                        self.status_notice = None;
                                                    }
                                                }
                                                self.update_title_bar();
                                                self.request_redraw();
                                                return;
                                            }

                                            let is_double_click = self
                                                .last_select_click
                                                .map_or(false, |(lp, lx, ly, lt)| {
                                                    lp == page_idx
                                                        && now.duration_since(lt)
                                                            < Duration::from_millis(380)
                                                        && (sx2 - lx).abs() < 8.0
                                                        && (sy2 - ly).abs() < 8.0
                                                });
                                            if is_double_click {
                                                self.last_select_click = None;
                                                if self
                                                    .select_word_at_pdf_point(page_idx, pt_x, pt_y)
                                                {
                                                    self.status_notice = Some(format!(
                                                        "Selected word \"{}\" ({} chars) — Ctrl/Cmd+C to copy",
                                                        self.selected_text,
                                                        self.selected_text.chars().count()
                                                    ));
                                                } else {
                                                    self.clear_text_selection();
                                                }
                                            } else {
                                                self.last_select_click =
                                                    Some((page_idx, sx2, sy2, now));
                                                self.clear_text_selection();
                                            }
                                        } else {
                                            self.clear_text_selection();
                                        }
                                        self.update_title_bar();
                                        self.request_redraw();
                                        return;
                                    }
                                }
                                let allow_click = self.mouse_mode == MouseMode::StampImage;
                                if let Some((page_idx, pdf_rect)) = self.screen_drag_to_pdf_rect(
                                    sx1,
                                    sy1,
                                    sx2,
                                    sy2,
                                    allow_click,
                                ) {
                                    match self.mouse_mode {
                                        MouseMode::Hand => {}
                                        MouseMode::Select => {
                                            let count =
                                                self.select_text_in_pdf_rect(page_idx, pdf_rect);
                                            if count > 0 {
                                                self.status_notice = Some(format!(
                                                    "Selected {count} character(s) on Page {} — Ctrl/Cmd+C to copy",
                                                    page_idx + 1
                                                ));
                                            } else {
                                                self.clear_text_selection();
                                            }
                                        }
                                        MouseMode::Highlight => {
                                            let color = self.annotation_color;
                                            self.push_undo_snapshot();
                                            if let Some(doc) = self.doc.as_mut() {
                                                match PdfEngine::highlight_text_in_rect_with_color(
                                                    doc, page_idx, pdf_rect, color,
                                                ) {
                                                    Ok(count) if count > 0 => {
                                                        self.has_unsaved_edits = true;
                                                        self.invalidate_page_caches();
                                                        self.thumbnail_cache.remove(&page_idx);
                                                        self.status_notice = Some(format!(
                                                            "Highlighted {count} region(s) ({}) on Page {}",
                                                            color.label(),
                                                            page_idx + 1
                                                        ));
                                                    }
                                                    Ok(_) => {
                                                        let _ = self.undo_stack.pop_back();
                                                    }
                                                    Err(e) => {
                                                        let _ = self.undo_stack.pop_back();
                                                        self.status_notice =
                                                            Some(format!("Highlight error: {e}"));
                                                    }
                                                }
                                            }
                                        }
                                        MouseMode::Underline => {
                                            let color = self.annotation_color;
                                            self.push_undo_snapshot();
                                            if let Some(doc) = self.doc.as_mut() {
                                                match PdfEngine::underline_text_in_rect(
                                                    doc, page_idx, pdf_rect, color,
                                                ) {
                                                    Ok(count) if count > 0 => {
                                                        self.has_unsaved_edits = true;
                                                        self.invalidate_page_caches();
                                                        self.thumbnail_cache.remove(&page_idx);
                                                        self.status_notice = Some(format!(
                                                            "Underlined {count} region(s) ({}) on Page {}",
                                                            color.label(),
                                                            page_idx + 1
                                                        ));
                                                    }
                                                    Ok(_) => {
                                                        let _ = self.undo_stack.pop_back();
                                                    }
                                                    Err(e) => {
                                                        let _ = self.undo_stack.pop_back();
                                                        self.status_notice =
                                                            Some(format!("Underline error: {e}"));
                                                    }
                                                }
                                            }
                                        }
                                        MouseMode::Strikethrough => {
                                            let color = self.annotation_color;
                                            self.push_undo_snapshot();
                                            if let Some(doc) = self.doc.as_mut() {
                                                match PdfEngine::strikeout_text_in_rect(
                                                    doc, page_idx, pdf_rect, color,
                                                ) {
                                                    Ok(count) if count > 0 => {
                                                        self.has_unsaved_edits = true;
                                                        self.invalidate_page_caches();
                                                        self.thumbnail_cache.remove(&page_idx);
                                                        self.status_notice = Some(format!(
                                                            "Strikethrough {count} region(s) ({}) on Page {}",
                                                            color.label(),
                                                            page_idx + 1
                                                        ));
                                                    }
                                                    Ok(_) => {
                                                        let _ = self.undo_stack.pop_back();
                                                    }
                                                    Err(e) => {
                                                        let _ = self.undo_stack.pop_back();
                                                        self.status_notice =
                                                            Some(format!("Strikethrough error: {e}"));
                                                    }
                                                }
                                            }
                                        }
                                        MouseMode::RectShape => {
                                            let color = self.annotation_color;
                                            self.push_undo_snapshot();
                                            if let Some(doc) = self.doc.as_mut() {
                                                match PdfEngine::add_rectangle_shape(
                                                    doc, page_idx, pdf_rect, color, 1.8,
                                                ) {
                                                    Ok(()) => {
                                                        self.has_unsaved_edits = true;
                                                        self.invalidate_page_caches();
                                                        self.thumbnail_cache.remove(&page_idx);
                                                        self.status_notice = Some(format!(
                                                            "Added {} rectangle on Page {} (Ctrl/Cmd+Z to undo)",
                                                            color.label(),
                                                            page_idx + 1
                                                        ));
                                                    }
                                                    Err(e) => {
                                                        let _ = self.undo_stack.pop_back();
                                                        self.status_notice =
                                                            Some(format!("Rectangle error: {e}"));
                                                    }
                                                }
                                            }
                                        }
                                        MouseMode::StickyNote
                                        | MouseMode::InkPen
                                        | MouseMode::FreeText
                                        | MouseMode::ArrowShape
                                        | MouseMode::Measure => {}
                                        MouseMode::Redact => {
                                            self.push_undo_snapshot();
                                            if let Some(doc) = self.doc.as_mut() {
                                                match PdfEngine::redact_rect_on_page(
                                                    doc, page_idx, pdf_rect,
                                                ) {
                                                    Ok(removed) => {
                                                        self.redacted = true;
                                                        self.invalidate_page_caches();
                                                        self.thumbnail_cache.remove(&page_idx);
                                                        self.status_notice = Some(format!(
                                                            "REDACTED area on Page {} ({} object(s)/annotation(s) removed or masked). Save to make it permanent.",
                                                            page_idx + 1,
                                                            removed
                                                        ));
                                                    }
                                                    Err(e) => {
                                                        let _ = self.undo_stack.pop_back();
                                                        self.status_notice =
                                                            Some(format!("Redaction error: {e}"));
                                                    }
                                                }
                                            }
                                        }
                                        MouseMode::Crop => {
                                            self.push_undo_snapshot();
                                            if let Some(doc) = self.doc.as_mut() {
                                                match self
                                                    .engine
                                                    .crop_page_to_rect(doc, page_idx, pdf_rect)
                                                {
                                                    Ok(cropped_bytes) => {
                                                        match self
                                                            .engine
                                                            .load_pdf_from_bytes_safe(cropped_bytes)
                                                        {
                                                            Ok(new_doc) => {
                                                                self.doc = Some(new_doc);
                                                                self.mouse_mode =
                                                                    MouseMode::Select;
                                                                self.scroll_y = 0.0;
                                                                self.invalidate_page_caches();
                                                                self.thumbnail_cache
                                                                    .remove(&page_idx);
                                                                self.status_notice = Some(format!(
                                                                    "Cropped Page {} (Use Page -> Reset Page Crop or Cmd/Ctrl+Z to restore)",
                                                                    page_idx + 1
                                                                ));
                                                            }
                                                            Err(e) => {
                                                                let _ = self.undo_stack.pop_back();
                                                                self.status_notice = Some(format!(
                                                                    "Crop reload error: {e}"
                                                                ));
                                                            }
                                                        }
                                                    }
                                                    Err(e) => {
                                                        let _ = self.undo_stack.pop_back();
                                                        self.status_notice =
                                                            Some(format!("Crop error: {e}"));
                                                    }
                                                }
                                            }
                                        }
                                        MouseMode::StampImage => {
                                            if let Some(stamp) = self.pending_stamp_image.take() {
                                                self.push_undo_snapshot();
                                                if let Some(doc) = self.doc.as_mut() {
                                                    match self.engine.stamp_image_on_page(
                                                        doc, page_idx, &stamp, pdf_rect,
                                                    ) {
                                                        Ok(()) => {
                                                            self.mouse_mode = MouseMode::Select;
                                                            self.invalidate_page_caches();
                                                            self.thumbnail_cache.remove(&page_idx);
                                                            self.status_notice = Some(format!(
                                                                "Stamped image/signature onto Page {}! (Cmd/Ctrl+Z to undo)",
                                                                page_idx + 1
                                                            ));
                                                        }
                                                        Err(e) => {
                                                            self.pending_stamp_image = Some(stamp);
                                                            let _ = self.undo_stack.pop_back();
                                                            self.status_notice = Some(format!(
                                                                "Image stamp error: {e}"
                                                            ));
                                                        }
                                                    }
                                                }
                                            } else {
                                                self.mouse_mode = MouseMode::Select;
                                            }
                                        }
                                    }
                                }
                                self.update_title_bar();
                                self.request_redraw();
                            }
                        }
                    }
                } else if button == MouseButton::Right && state == ElementState::Pressed {
                    if let Some((page_idx, pt_x, pt_y)) =
                        self.screen_point_to_pdf_point(self.cursor_pos.0, self.cursor_pos.1)
                    {
                        let found = self.doc.as_ref().and_then(|doc| {
                            let page = doc.pages().get(page_idx).ok()?;
                            PdfEngine::find_annotation_at_point(&page, pt_x.value, pt_y.value)
                        });
                        if let Some(info) = found {
                            self.active_note_popover = None;
                            self.annot_context_menu = Some(
                                super::doc_props_modal::AnnotationContextMenuState {
                                    page_idx,
                                    annot_idx: info.index,
                                    label: info.label,
                                    is_text_editable: info.is_text_editable,
                                    current_text: info.contents,
                                    x: self.cursor_pos.0.round() as i32,
                                    y: self.cursor_pos.1.round() as i32,
                                },
                            );
                            self.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    let text_ref = event.text.as_deref();
                    self.handle_key_input(event.logical_key, text_ref);
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.render_to_window() {
                    eprintln!("Render error: {e:#}");
                }
            }
            _ => {}
        }
    }
}

/// Whether `a` and `b` name the same file (compared after resolving links and `..`).
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

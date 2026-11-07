use crate::engine::{Bookmark, MarginPosition, MarginZone, PdfEngine};
use pdfium_render::prelude::*;
use softbuffer::{Context as SoftContext, Surface};
use std::path::PathBuf;
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
        if self.view_mode == mode {
            return;
        }
        if mode == ViewMode::ThumbnailGrid {
            if self.view_mode.is_page_view() {
                self.last_page_view_mode = self.view_mode;
            }
            self.invalidate_page_caches();
            self.view_mode = ViewMode::ThumbnailGrid;
            self.status_notice = Some(
                "Thumbnail Grid View (Click page, Drag reorder, [BW]/[R] badges, Tab/Enter to view)"
                    .to_string(),
            );
        } else {
            // Free thumbnail cache immediately when returning to Page View
            self.thumbnail_cache.clear();
            self.thumbnail_cache.shrink_to_fit();
            self.grid_drag_page = None;
            self.hovered_thumb = None;
            self.view_mode = mode;
            self.last_page_view_mode = mode;
            self.scroll_y = 0.0;
            self.invalidate_page_caches();
            self.status_notice = Some(match mode {
                ViewMode::FitPage => "View Mode: Fit Full Page (1)".to_string(),
                ViewMode::FitWidth => "View Mode: Fit Page Width (2) — Scroll vertically".to_string(),
                ViewMode::FitTextWidth => {
                    "View Mode: Fit Text Width (3) — Auto-trimmed margins".to_string()
                }
                ViewMode::TwoPageSpread => {
                    "View Mode: Two-Page Side-by-Side Spread (4)".to_string()
                }
                ViewMode::ContinuousScroll => {
                    "View Mode: Continuous Vertical Scroll (5)".to_string()
                }
                ViewMode::ThumbnailGrid => "Thumbnail Grid View".to_string(),
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
    /// or the vertically hit page in `ContinuousScroll`) and its corresponding screen `draw_rect` contains `(sx, sy)`.
    pub(super) fn resolve_target_page_and_draw_rect(&self, sx: f64, sy: f64) -> (u16, (i32, i32, u32, u32)) {
        if self.view_mode == ViewMode::ContinuousScroll {
            let iy = sy.round() as i32;
            for &(p_idx, rect) in &self.continuous_draw_rects {
                let (_rx, ry, _rw, rh) = rect;
                if iy >= ry && iy <= ry + (rh as i32) {
                    return (p_idx, rect);
                }
            }
            if let Some(&(p_idx, rect)) = self.continuous_draw_rects.iter().min_by_key(|&&(_, (_, ry, _, rh))| {
                let center = ry + (rh as i32) / 2;
                (iy - center).abs()
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
        let clamped = new_zoom.clamp(MIN_ZOOM_LEVEL, MAX_ZOOM_LEVEL);
        let old_zoom = self.zoom_level.clamp(MIN_ZOOM_LEVEL, MAX_ZOOM_LEVEL);
        if (clamped - old_zoom).abs() < 0.001 {
            return;
        }
        let ratio = clamped / old_zoom;

        let (sidebar_w, top_bars_h, content_w, content_h) = if let Some(w) = self.window.as_ref() {
            let sz = w.inner_size();
            let sf = w.scale_factor();
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

            if new_w > content_w as f32 {
                self.pan_x =
                    ((sidebar_w as f32) - new_draw_x).clamp(0.0, (new_w - content_w as f32).max(0.0));
            } else {
                self.pan_x = 0.0;
            }

            if self.view_mode == ViewMode::ContinuousScroll {
                self.scroll_y = ((top_bars_h + PAGE_MARGIN_PX) as f32) - new_draw_y;
            } else if new_h > content_h as f32 {
                self.scroll_y = ((top_bars_h as f32) - new_draw_y)
                    .clamp(0.0, (new_h - content_h as f32).max(0.0));
            } else {
                self.scroll_y = 0.0;
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
        let target = (self.zoom_level * factor).clamp(MIN_ZOOM_LEVEL, MAX_ZOOM_LEVEL);
        let (cx, cy) = self.cursor_pos;
        self.set_zoom_around_cursor(target, cx, cy);
    }

    pub(super) fn reset_zoom(&mut self) {
        self.zoom_level = 1.0;
        self.pan_x = 0.0;
        self.scroll_y = 0.0;
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

    /// Scrolls vertically in `ContinuousScroll`, or in any page view when the page bitmap exceeds the viewport height,
    /// seamlessly transitioning to the next/previous page when scrolling past the bottom/top!
    pub(super) fn scroll_or_turn_page_vertical(&mut self, scroll_delta_px: f32) -> bool {
        if self.view_mode == ViewMode::ContinuousScroll {
            self.scroll_y += scroll_delta_px;
            self.request_redraw();
            return true;
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
        self.push_undo_snapshot();
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        if let Ok(page) = doc.pages().get(self.current_page_index) {
            if page.delete().is_ok() {
                let new_total = doc.pages().len();
                if self.current_page_index >= new_total && new_total > 0 {
                    self.current_page_index = new_total - 1;
                }
                self.invalidate_page_caches();
                self.thumbnail_cache.clear();
                self.status_notice = Some("Deleted page (Cmd/Ctrl+Z to undo)".to_string());
                self.update_title_bar();
                self.request_redraw();
            }
        }
    }

    pub(super) fn insert_blank_page_after_current(&mut self) {
        self.push_undo_snapshot();
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let target_idx = (self.current_page_index + 1).min(doc.pages().len());
        match PdfEngine::insert_blank_page_in_doc(doc, target_idx) {
            Ok(inserted_idx) => {
                self.current_page_index = inserted_idx;
                self.invalidate_page_caches();
                self.thumbnail_cache.clear();
                self.status_notice =
                    Some(format!("Inserted blank A4 page at #{}", inserted_idx + 1));
                self.update_title_bar();
                self.request_redraw();
            }
            Err(e) => {
                self.status_notice = Some(format!("Insert failed: {e}"));
                self.update_title_bar();
            }
        }
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

    /// Batch Combine Multiple PDFs (`Shift+M` or `File -> Combine Multiple PDFs...`).
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
        if trimmed.is_empty() {
            self.search_query.clear();
            self.search_matches.clear();
            self.current_search_match = 0;
            self.status_notice = Some("Cleared text search highlights".to_string());
            self.update_title_bar();
            self.request_redraw();
            return;
        }

        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let matches = PdfEngine::search_text_in_doc(doc, &trimmed);
        self.search_query = trimmed.clone();
        if matches.is_empty() {
            self.search_matches.clear();
            self.current_search_match = 0;
            self.status_notice = Some(format!("No matches found for '{trimmed}'"));
            self.update_title_bar();
            self.request_redraw();
            return;
        }

        // Start from the first match on or after `self.current_page_index`
        let start_idx = matches
            .iter()
            .position(|m| m.page_index >= self.current_page_index)
            .unwrap_or(0);
        let target_page = matches[start_idx].page_index;
        let total_matches = matches.len();
        self.search_matches = matches;
        self.current_search_match = start_idx;

        if target_page != self.current_page_index {
            self.current_page_index = target_page;
            self.scroll_y = 0.0;
            self.invalidate_page_caches();
        }
        self.status_notice = Some(format!(
            "Match {}/{} for '{}' on Page {} (Press F3 or Cmd/Ctrl+G for next)",
            start_idx + 1,
            total_matches,
            trimmed,
            target_page + 1
        ));
        self.update_title_bar();
        self.request_redraw();
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

        let save_path = rfd::FileDialog::new()
            .set_title("Save Extracted/Split PDF")
            .add_filter("PDF Document", &["pdf"])
            .set_file_name(&default_name)
            .save_file()
            .unwrap_or_else(|| PathBuf::from(&default_name));

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
            .add_filter("PDF Document", &["pdf"])
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

    pub(super) fn save_file_dialog(&mut self) {
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let stem = self
            .file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document");
        let default_name = format!("{stem}_edited.pdf");

        let target_path = rfd::FileDialog::new()
            .add_filter("PDF Document", &["pdf"])
            .set_file_name(&default_name)
            .save_file();

        if let Some(save_path) = target_path {
            match PdfEngine::save_doc_with_metadata(
                doc,
                &save_path,
                self.roman_end,
                &self.bookmarks,
                self.redacted,
            ) {
                Ok(warning) => {
                    let saved_mtime = Self::read_file_mtime(&save_path);
                    self.record_recent_file(&save_path);
                    self.file_path = save_path.clone();
                    self.has_unsaved_edits = false;
                    self.last_file_mtime = saved_mtime;
                    if self.active_tab < self.tabs.len() {
                        self.tabs[self.active_tab].file_path = save_path;
                        self.tabs[self.active_tab].has_unsaved_edits = false;
                        self.tabs[self.active_tab].last_file_mtime = saved_mtime;
                    }
                    self.persist_bookmarks();
                    self.status_notice = Some(match warning {
                        None if self.redacted => {
                            "Saved PDF (redaction finalized: orphaned objects, /Info & thumbnails removed)".to_string()
                        }
                        None => "Saved PDF (sanitized metadata + embedded /PageLabels & /Outlines)".to_string(),
                        Some(w) => format!("Saved PDF, but WARNING: {w}"),
                    });
                }
                Err(e) => {
                    self.status_notice = Some(format!("Save error: {e}"));
                }
            }
            self.update_title_bar();
            self.request_redraw();
        }
    }

    pub(super) fn compress_current_pdf(&mut self) {
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let stem = self
            .file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document");
        let out_name = format!("{stem}_compressed.pdf");
        let out_path = match self.file_path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.join(&out_name),
            _ => PathBuf::from(&out_name),
        };

        let disk_size = std::fs::metadata(&self.file_path)
            .map(|m| m.len() as usize)
            .unwrap_or(0);

        match self.engine.compress_doc(doc) {
            Ok((compressed_bytes, serialized_len, _)) => {
                let (final_bytes, warning) = match PdfEngine::embed_metadata_in_pdf_bytes(
                    &compressed_bytes,
                    self.roman_end,
                    &self.bookmarks,
                    self.redacted,
                ) {
                    Ok(b) => (b, None),
                    Err(e) if self.redacted => {
                        self.status_notice = Some(format!(
                            "Compression refused: post-redaction cleanup failed ({e}). Nothing was written."
                        ));
                        self.update_title_bar();
                        self.request_redraw();
                        return;
                    }
                    Err(e) => (compressed_bytes, Some(format!(" (WARNING: metadata not sanitized: {e})"))),
                };
                let new_len = final_bytes.len();
                let orig_len = disk_size.max(serialized_len);
                match PdfEngine::write_atomic(&out_path, &final_bytes) {
                    Ok(()) => {
                        let orig_mb = (orig_len as f64) / (1024.0 * 1024.0);
                        let new_mb = (new_len as f64) / (1024.0 * 1024.0);
                        let saved_pct = if orig_len > 0 && new_len < orig_len {
                            (((orig_len - new_len) as f64 / (orig_len as f64)) * 100.0).round()
                                as u32
                        } else {
                            0
                        };
                        self.status_notice = Some(format!(
                            "Compressed! {:.2} MB -> {:.2} MB (Saved {}%) -> '{}'{}",
                            orig_mb, new_mb, saved_pct, out_name, warning.unwrap_or_default()
                        ));
                    }
                    Err(e) => {
                        self.status_notice =
                            Some(format!("Failed to write compressed PDF '{}': {e}", out_name));
                    }
                }
            }
            Err(e) => {
                self.status_notice = Some(format!("Compression error: {e}"));
            }
        }
        self.update_title_bar();
        self.request_redraw();
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
                if let Some(target_idx) =
                    PdfEngine::resolve_logical_page_str(&input, self.roman_end, total)
                {
                    self.push_nav_history();
                    self.current_page_index = target_idx as u16;
                    self.scroll_y = 0.0;
                    self.invalidate_page_caches();
                    let lbl =
                        PdfEngine::format_logical_page_label(target_idx, self.roman_end);
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
                            PdfEngine::resync_bookmarks_logical_pages(
                                &mut self.bookmarks,
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
                let logical_page =
                    PdfEngine::format_logical_page_label(page_idx, self.roman_end);
                let next_serial = (self
                    .bookmarks
                    .iter()
                    .filter(|b| b.level == 0)
                    .count()
                    + 1)
                .to_string();
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
                self.status_notice = Some(format!("Added bookmark '{title}'"));
                self.request_redraw();
            }
            InputMode::EditBookmark {
                bookmark_idx,
                title_input,
            } => {
                let trimmed = title_input.trim();
                if !trimmed.is_empty() {
                    if let Some(bm) = self.bookmarks.get_mut(bookmark_idx) {
                        bm.title = trimmed.to_string();
                        self.persist_bookmarks();
                        self.status_notice = Some("Updated bookmark title".to_string());
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
        }
        self.update_title_bar();
        self.request_redraw();
    }

    pub(super) fn handle_key_input(&mut self, logical_key: Key, text: Option<&str>) {
        // 1. If in an interactive Bottom Command Bar / Title-Bar prompt mode, handle typing, Backspace, Enter, Esc
        if self.input_mode != InputMode::Normal {
            match logical_key {
                Key::Named(NamedKey::Escape) => {
                    if let InputMode::PasswordPrompt {
                        mut password_input, ..
                    } = std::mem::replace(&mut self.input_mode, InputMode::Normal)
                    {
                        password_input.clear();
                        password_input.shrink_to_fit();
                    }
                    self.pending_restore_tabs.clear();
                    self.status_notice = Some("Cancelled".to_string());
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
                        InputMode::HeaderFooterText { text_input, .. } => {
                            text_input.pop();
                        }
                        InputMode::AddBookmark { title_input }
                        | InputMode::EditBookmark { title_input, .. } => {
                            title_input.pop();
                        }
                        InputMode::PasswordPrompt { password_input, .. } => {
                            password_input.pop();
                        }
                        InputMode::ConfirmExternalUrl { .. }
                        | InputMode::RestoreSessionPrompt { .. }
                        | InputMode::Normal => {}
                    }
                    self.update_title_bar();
                    self.request_redraw();
                }
                Key::Named(NamedKey::Enter) => {
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
                            InputMode::ConfirmExternalUrl { .. } | InputMode::Normal => {}
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

        // 2. OS-native modifier shortcuts: Cmd+Z / Ctrl+Z (Undo), Cmd+C / Ctrl+C (Copy), Cmd+F / Ctrl+F (Find), Cmd+G / Ctrl+G (Next Match), Cmd+W / Ctrl+W (Close Tab)
        if self.cmd_or_ctrl_pressed {
            if let Key::Character(ch) = &logical_key {
                let s = ch.as_str();
                if s.eq_ignore_ascii_case("z") {
                    self.perform_undo();
                    return;
                } else if s.eq_ignore_ascii_case("c") {
                    self.copy_selected_text_to_clipboard();
                    return;
                } else if s.eq_ignore_ascii_case("f") {
                    self.input_mode = InputMode::SearchText {
                        query_input: self.search_query.clone(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                    self.request_redraw();
                    return;
                } else if s.eq_ignore_ascii_case("g") {
                    self.jump_to_next_search_match();
                    return;
                } else if s.eq_ignore_ascii_case("w") {
                    if self.tabs.len() > 1 {
                        let _ = self.close_tab(self.active_tab);
                    } else {
                        self.status_notice =
                            Some("Cannot close the last remaining tab".to_string());
                        self.update_title_bar();
                        self.request_redraw();
                    }
                    return;
                }
            }
        }

        // 3. If Help Overlay is open, `Esc`, `?`, or `F1` closes it
        if self.show_help_overlay {
            if matches!(
                logical_key,
                Key::Named(NamedKey::Escape) | Key::Named(NamedKey::F1)
            ) {
                self.show_help_overlay = false;
                self.request_redraw();
                return;
            }
            if let Key::Character(ch) = &logical_key {
                if ch.as_str() == "?" || ch.as_str() == "/" {
                    self.show_help_overlay = false;
                    self.request_redraw();
                    return;
                }
            }
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
                self.jump_to_next_search_match();
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
                } else if self.show_help_overlay {
                    self.show_help_overlay = false;
                    self.request_redraw();
                } else if self.is_presentation || self.is_fullscreen {
                    self.exit_presentation_or_fullscreen();
                } else if !self.selected_text_rects.is_empty() {
                    self.clear_text_selection();
                    self.status_notice = Some("Cleared text selection".to_string());
                    self.update_title_bar();
                    self.request_redraw();
                } else if self.mouse_mode != MouseMode::Select {
                    self.mouse_mode = MouseMode::Select;
                    self.pending_stamp_image = None;
                    self.status_notice =
                        Some("Returned to Mouse Mode: Select Text".to_string());
                    self.update_title_bar();
                    self.request_redraw();
                } else if self.view_mode == ViewMode::ThumbnailGrid {
                    self.toggle_view_mode();
                }
            }
            Key::Named(NamedKey::Delete) | Key::Named(NamedKey::Backspace) => {
                // If Bookmark Sidebar is open and a bookmark is selected, Delete/Backspace deletes it
                if self.show_bookmarks {
                    if let Some(sel) = self.selected_bookmark {
                        if sel < self.bookmarks.len() {
                            let removed = self.bookmarks.remove(sel);
                            if self.bookmarks.is_empty() {
                                self.selected_bookmark = None;
                            } else if sel >= self.bookmarks.len() {
                                self.selected_bookmark = Some(self.bookmarks.len() - 1);
                            }
                            self.persist_bookmarks();
                            self.status_notice =
                                Some(format!("Deleted bookmark '{}'", removed.title));
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
                } else if !self.pan_horizontal(64.0) {
                    self.step_pages_forward();
                }
            }
            Key::Named(NamedKey::PageDown) | Key::Named(NamedKey::Space) => {
                self.step_pages_forward();
            }
            Key::Named(NamedKey::ArrowLeft) => {
                if self.alt_pressed {
                    self.navigate_back();
                } else if !self.pan_horizontal(-64.0) {
                    self.step_pages_backward();
                }
            }
            Key::Named(NamedKey::PageUp) => {
                self.step_pages_backward();
            }
            Key::Named(NamedKey::Home) => {
                self.navigate_to_page(0);
            }
            Key::Named(NamedKey::End) => {
                let total = self.total_pages();
                if total > 0 {
                    self.navigate_to_page(total - 1);
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
                    self.set_view_mode(ViewMode::ContinuousScroll);
                } else if s.eq_ignore_ascii_case("v") {
                    self.reading_filter = self.reading_filter.next();
                    self.status_notice = Some(format!(
                        "Reading Filter: {} (Zero-RAM CPU Blit Filter)",
                        self.reading_filter.label()
                    ));
                    self.update_title_bar();
                    self.request_redraw();
                } else if s.eq_ignore_ascii_case("p") {
                    self.pending_dialog = Some(PendingDialog::PickStampImage);
                } else if s.eq_ignore_ascii_case("u") {
                    self.flatten_and_unlock_current_pdf();
                } else if s.eq_ignore_ascii_case("b") {
                    self.show_bookmarks = !self.show_bookmarks;
                    self.invalidate_page_caches();
                    self.status_notice = Some(if self.show_bookmarks {
                        "Bookmarks Sidebar ON ('+'/'A' Add, 'L' Import .txt, 'E' Edit, Del Remove)"
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
                    self.input_mode = InputMode::PageNumbers {
                        input: String::new(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                    self.request_redraw();
                } else if s.eq_ignore_ascii_case("h") {
                    self.input_mode = InputMode::HeaderFooterRange {
                        range_input: String::new(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                    self.request_redraw();
                } else if s.eq_ignore_ascii_case("c") {
                    self.input_mode = InputMode::ClearMargins {
                        input: String::new(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                    self.request_redraw();
                } else if s.eq_ignore_ascii_case("w") {
                    self.input_mode = InputMode::Watermark {
                        input: String::new(),
                    };
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
            let win_attrs = Window::default_attributes()
                .with_title("MicroPDF")
                .with_inner_size(LogicalSize::new(1024.0, 1080.0));

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
        self.poll_single_instance_listener();
        self.poll_auto_reload_if_due();
        if self.single_instance_listener.is_some()
            || (self.window_focused && self.doc.is_some())
        {
            event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(120),
            ));
        }

        if let Some(dialog) = self.pending_dialog.take() {
            match dialog {
                PendingDialog::Open => self.open_file_dialog(),
                PendingDialog::Save => self.save_file_dialog(),
                PendingDialog::Merge => self.merge_pdf_dialog(),
                PendingDialog::CombineMultiple => self.combine_multiple_pdfs_dialog(),
                PendingDialog::PickStampImage => self.pick_stamp_image_dialog(),
                PendingDialog::Split(spec) => self.execute_split_range(&spec),
                PendingDialog::ImportBookmarksTxt => self.import_bookmarks_txt_dialog(),
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
                self.stop_single_instance_listener();
                self.save_session_to_ini();
                self.invalidate_page_caches();
                self.thumbnail_cache.clear();
                self.tabs.clear();
                self.doc = None;
                self.surface = None;
                self.context = None;
                self.window = None;
                event_loop.exit();
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
                if !ext.eq_ignore_ascii_case("pdf") {
                    self.status_notice = Some(format!(
                        "Dropped file {:?} is not a .pdf, .txt, or .png/.jpg",
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

                // In Page View (or when holding Shift, or when no document is loaded):
                // Open the dropped PDF in a new tab!
                if self.view_mode.is_page_view() || self.shift_pressed || self.doc.is_none() {
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
                    let sf = w.scale_factor();
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

                // Ctrl/Cmd + MouseWheel zooms around the cursor position (25%..=800%)
                if self.cmd_or_ctrl_pressed && self.view_mode.is_page_view() {
                    if dy.abs() > 0.1 {
                        let factor = if dy > 0.0 { 1.15 } else { 1.0 / 1.15 };
                        let target = (self.zoom_level * factor).clamp(MIN_ZOOM_LEVEL, MAX_ZOOM_LEVEL);
                        let (cx, cy) = self.cursor_pos;
                        self.set_zoom_around_cursor(target, cx, cy);
                    }
                    return;
                }

                // If cursor is over the Bookmark Sidebar, scroll bookmarks
                if let Some(w) = self.window.as_ref() {
                    let sidebar_w = self.sidebar_width_px(w.scale_factor());
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

                if dx.abs() > 0.5 {
                    let _ = self.pan_horizontal((-dx) as f32);
                }

                // Smoothly scroll `scroll_y` vertically when zoomed or in ContinuousScroll / FitWidth / FitTextWidth
                if self.scroll_or_turn_page_vertical((-dy) as f32) {
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
                if let Some(w) = self.window.as_ref() {
                    let sf = w.scale_factor();
                    let menu_bar_h = self.menu_bar_height_px(sf);
                    if self.open_menu.is_some() {
                        // Hover-switch between top-level menus when a dropdown is already open
                        if let Some(hovered_cat) =
                            self.hit_test_menu_bar_header(position.x, position.y, menu_bar_h, sf)
                        {
                            if self.open_menu != Some(hovered_cat) {
                                self.open_menu = Some(hovered_cat);
                            }
                        }
                        self.request_redraw();
                        return;
                    } else if (position.y as u32) < menu_bar_h {
                        self.request_redraw();
                    }
                }

                if self.drag_start.is_some() || self.grid_drag_page.is_some() {
                    if let Some((sx1, sy1)) = self.drag_start {
                        if self.mouse_mode == MouseMode::Select && self.view_mode.is_page_view() {
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
                } else if self.view_mode.is_page_view() {
                    let new_link = self.resolve_link_at_screen_pos(position.x, position.y);
                    if new_link != self.hovered_link {
                        let is_over_link = new_link.is_some();
                        self.hovered_link = new_link;
                        if let Some(w) = self.window.as_ref() {
                            w.set_cursor(if is_over_link {
                                CursorIcon::Pointer
                            } else {
                                CursorIcon::Default
                            });
                        }
                    }
                } else if self.view_mode == ViewMode::ThumbnailGrid {
                    if let Some(w) = self.window.as_ref() {
                        let sz = w.inner_size();
                        let sf = w.scale_factor();
                        let sidebar_w = self.sidebar_width_px(sf);
                        let top_bars_h = self.top_bars_height_px(sf);
                        let new_hover = self
                            .hit_test_thumbnail(
                                position.x,
                                position.y,
                                sz.width.max(1),
                                sidebar_w,
                                top_bars_h,
                            )
                            .map(|(idx, _)| idx);
                        if new_hover != self.hovered_thumb {
                            self.hovered_thumb = new_hover;
                            self.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if self.show_help_overlay {
                    if state == ElementState::Pressed {
                        self.show_help_overlay = false;
                        self.request_redraw();
                    }
                    return;
                }
                let (win_w, scale_factor, sidebar_w, menu_bar_h, tab_bar_h, top_bars_h) =
                    if let Some(w) = self.window.as_ref() {
                        let sz = w.inner_size();
                        let sf = w.scale_factor();
                        let mb_h = self.menu_bar_height_px(sf).min(sz.height.saturating_sub(48));
                        let tb_h = self
                            .tab_bar_height_px(sf)
                            .min(sz.height.saturating_sub(mb_h + 24));
                        (
                            sz.width.max(1),
                            sf,
                            self.sidebar_width_px(sf).min(sz.width.saturating_sub(64)),
                            mb_h,
                            tb_h,
                            mb_h + tb_h,
                        )
                    } else {
                        (
                            1024,
                            1.0,
                            0,
                            MENU_BAR_HEIGHT_LOGICAL,
                            TAB_BAR_HEIGHT_LOGICAL,
                            MENU_BAR_HEIGHT_LOGICAL + TAB_BAR_HEIGHT_LOGICAL,
                        )
                    };

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
                        // Clicked outside menu dropdown -> close dropdown
                        self.open_menu = None;
                        self.request_redraw();
                    }
                    return;
                }

                // 0b. Check Top 24px Menu Bar header clicks
                if (self.cursor_pos.1 as u32) < menu_bar_h {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if let Some(cat) = self.hit_test_menu_bar_header(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            menu_bar_h,
                            scale_factor,
                        ) {
                            self.open_menu = Some(cat);
                            self.request_redraw();
                        }
                    }
                    return;
                }

                // 0c. Check 28px Multi-Tab Bar interactions (`menu_bar_h .. top_bars_h`)
                if (self.cursor_pos.1 as u32) < top_bars_h {
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
                                TabBarHit::SwitchTab(idx) => {
                                    self.switch_to_tab(idx);
                                }
                                TabBarHit::CloseTab(idx) => {
                                    if self.tabs.len() > 1 {
                                        let _ = self.close_tab(idx);
                                    } else {
                                        self.status_notice = Some(
                                            "Cannot close the last remaining tab".to_string(),
                                        );
                                        self.update_title_bar();
                                        self.request_redraw();
                                    }
                                }
                                TabBarHit::NewTab => {
                                    self.pending_dialog = Some(PendingDialog::Open);
                                }
                            }
                        }
                    }
                    return;
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
                                SidebarHit::ImportTxtButton => {
                                    if button == MouseButton::Left {
                                        self.pending_dialog =
                                            Some(PendingDialog::ImportBookmarksTxt);
                                    }
                                }
                                SidebarHit::BookmarkRow {
                                    index: bm_idx,
                                    on_delete,
                                } => match button {
                                    MouseButton::Left => {
                                        if on_delete {
                                            let removed = self.bookmarks.remove(bm_idx);
                                            if self.bookmarks.is_empty() {
                                                self.selected_bookmark = None;
                                            } else if bm_idx >= self.bookmarks.len() {
                                                self.selected_bookmark =
                                                    Some(self.bookmarks.len() - 1);
                                            }
                                            self.persist_bookmarks();
                                            self.status_notice = Some(format!(
                                                "Deleted bookmark '{}'",
                                                removed.title
                                            ));
                                            self.update_title_bar();
                                            self.request_redraw();
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
                                        let removed = self.bookmarks.remove(bm_idx);
                                        if self.bookmarks.is_empty() {
                                            self.selected_bookmark = None;
                                        } else if bm_idx >= self.bookmarks.len() {
                                            self.selected_bookmark =
                                                Some(self.bookmarks.len() - 1);
                                        }
                                        self.persist_bookmarks();
                                        self.status_notice =
                                            Some(format!("Deleted bookmark '{}'", removed.title));
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

                                    // Check for double-click to jump into Page View
                                    let now = Instant::now();
                                    if let Some((last_idx, last_time)) = self.last_thumb_click {
                                        if last_idx == clicked_idx
                                            && now.duration_since(last_time)
                                                < Duration::from_millis(350)
                                        {
                                            self.current_page_index = clicked_idx;
                                            self.last_thumb_click = None;
                                            self.toggle_view_mode();
                                            return;
                                        }
                                    }
                                    self.last_thumb_click = Some((clicked_idx, now));
                                    self.current_page_index = clicked_idx;
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
                                                        self.current_page_index = dst_idx;
                                                        self.thumbnail_cache.clear();
                                                        self.status_notice = Some(format!(
                                                            "Moved Page {} to Page {}",
                                                            src_idx + 1,
                                                            dst_idx + 1
                                                        ));
                                                    }
                                                    Err(e) => {
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

                // 3. Page View mouse drag interactions: Highlight, Redact, Crop, StampImage
                if button == MouseButton::Left {
                    match state {
                        ElementState::Pressed => {
                            self.drag_start = Some(self.cursor_pos);
                        }
                        ElementState::Released => {
                            if let Some((sx1, sy1)) = self.drag_start.take() {
                                let (sx2, sy2) = self.cursor_pos;
                                let is_click = (sx2 - sx1).abs() < 5.0 && (sy2 - sy1).abs() < 5.0;
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
                                            self.push_undo_snapshot();
                                            if let Some(doc) = self.doc.as_mut() {
                                                match PdfEngine::highlight_text_in_rect(
                                                    doc, page_idx, pdf_rect,
                                                ) {
                                                    Ok(count) if count > 0 => {
                                                        self.invalidate_page_caches();
                                                        self.status_notice = Some(format!(
                                                            "Highlighted {count} text region(s) on Page {}",
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

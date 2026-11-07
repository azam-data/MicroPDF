use crate::engine::PdfEngine;
use anyhow::{Context, Result};
use pdfium_render::prelude::*;
use std::num::NonZeroU32;
use super::state::*;

impl<'a> PdfViewerApp<'a> {
    /// Renders `page` directly into a reusable `Vec<u32>` buffer via `PdfBitmap::from_bytes`,
    /// masking the high byte in place (`0x00RRGGBB` on little-endian) so only 1 copy of the page
    /// bitmap exists in memory at any point.
    pub(super) fn render_page_into_u32_vec(
        &self,
        page: &PdfPage,
        target_w: i32,
        target_h: i32,
        lcd_text: bool,
        mut pixels: Vec<u32>,
    ) -> Result<Vec<u32>> {
        let pixel_count = (target_w.max(1) as usize) * (target_h.max(1) as usize);
        pixels.clear();
        pixels.resize(pixel_count, 0x00FFFFFF);

        let bindings = self.engine.pdfium.bindings();
        let config = PdfRenderConfig::new()
            .set_fixed_width(target_w)
            .set_fixed_height(target_h)
            .render_form_data(true)
            .render_annotations(true)
            .use_lcd_text_rendering(lcd_text)
            .set_reverse_byte_order(false);

        {
            let byte_slice: &mut [u8] = unsafe {
                std::slice::from_raw_parts_mut(pixels.as_mut_ptr() as *mut u8, pixel_count * 4)
            };
            let mut bitmap = unsafe {
                PdfBitmap::from_bytes(
                    target_w,
                    target_h,
                    PdfBitmapFormat::BGRx,
                    byte_slice,
                    bindings,
                )
            }
            .context("Failed to wrap reusable buffer in PdfBitmap")?;

            page.render_into_bitmap_with_config(&mut bitmap, &config)
                .context("Failed to render PDF page into bitmap")?;
        }

        // On little-endian, BGRx bytes [B, G, R, X] read as u32 are 0xXXRRGGBB; mask high byte in place.
        for px in &mut pixels {
            #[cfg(target_endian = "little")]
            {
                *px &= 0x00FFFFFF;
            }
            #[cfg(target_endian = "big")]
            {
                let [b, g, r, _] = px.to_be_bytes();
                *px = ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
            }
        }
        Ok(pixels)
    }

    /// Rasterizes a single `page_idx` for the specified `mode` and available `(avail_w, avail_h)`,
    /// reusing `reused_pixels` when provided.
    pub(super) fn rasterize_page_for_mode(
        &self,
        page_idx: u16,
        mode: ViewMode,
        content_w: u32,
        content_h: u32,
        avail_w: f32,
        avail_h: f32,
        reused_pixels: Vec<u32>,
    ) -> Result<CachedPageBitmap> {
        let doc = self
            .doc
            .as_ref()
            .context("No active PDF document to render")?;
        let page = doc
            .pages()
            .get(page_idx)
            .context("Page index out of bounds")?;
        let rotation_deg = self.page_rotation_degrees(page_idx);

        let page_w_pts = page.width().value.max(1.0);
        let page_h_pts = page.height().value.max(1.0);

        let (content_min_x, content_max_x) = if mode == ViewMode::FitTextWidth {
            let (min_x, _min_y, max_x, _max_y) = PdfEngine::compute_page_content_bounds(&page);
            (min_x, max_x)
        } else {
            (0.0, page_w_pts)
        };

        let scale = match mode {
            ViewMode::FitPage
            | ViewMode::TwoPageSpread
            | ViewMode::ContinuousScroll
            | ViewMode::ThumbnailGrid => (avail_w / page_w_pts).min(avail_h / page_h_pts),
            ViewMode::FitWidth => avail_w / page_w_pts,
            ViewMode::FitTextWidth => {
                let text_w = (content_max_x - content_min_x).max(page_w_pts * 0.25);
                avail_w / text_w
            }
        };

        // Cap maximum rasterized dimension to 3600px to keep RAM strictly bounded
        let max_dim = 3600.0_f32;
        let clamped_scale = scale
            .min(max_dim / page_w_pts)
            .min(max_dim / page_h_pts)
            .max(0.05);

        let target_w = ((page_w_pts * clamped_scale).round() as i32).max(1);
        let target_h = ((page_h_pts * clamped_scale).round() as i32).max(1);

        let pixels = self.render_page_into_u32_vec(&page, target_w, target_h, true, reused_pixels)?;

        Ok(CachedPageBitmap {
            page_index: page_idx,
            view_mode: mode,
            viewport_width: content_w,
            viewport_height: content_h,
            bitmap_width: target_w as u32,
            bitmap_height: target_h as u32,
            content_min_x,
            content_max_x,
            rotation_deg,
            pixels,
        })
    }

    /// Computes `(draw_x, draw_y, bmp_w, bmp_h)` for a single-page view (`FitPage`, `FitWidth`, `FitTextWidth`)
    /// and clamps `self.scroll_y` to valid vertical bounds.
    pub(super) fn compute_single_page_draw_rect(
        &mut self,
        sidebar_w: u32,
        top_bars_h: u32,
        content_w: u32,
        content_h: u32,
        bmp_w: u32,
        bmp_h: u32,
        content_min_x: f32,
        content_max_x: f32,
        page_w_pts: f32,
    ) -> (i32, i32, u32, u32) {
        let draw_x = if self.view_mode == ViewMode::FitTextWidth && page_w_pts > 1.0 {
            let scale = (bmp_w as f32) / page_w_pts;
            let text_center_bx = ((content_min_x + content_max_x) * 0.5 * scale).round() as i32;
            let viewport_center_x = (sidebar_w as i32) + (content_w as i32) / 2;
            viewport_center_x - text_center_bx
        } else {
            (sidebar_w as i32) + ((content_w as i32 - bmp_w as i32) / 2)
        };

        let draw_y = if bmp_h > content_h {
            let max_scroll = (bmp_h - content_h) as f32;
            self.scroll_y = self.scroll_y.clamp(0.0, max_scroll);
            (top_bars_h as i32) - (self.scroll_y.round() as i32)
        } else {
            self.scroll_y = 0.0;
            (top_bars_h as i32) + ((content_h as i32 - bmp_h as i32) / 2)
        };

        (draw_x, draw_y, bmp_w, bmp_h)
    }

    /// Ensures `self.cached_page` (and `self.cached_second_page` in `TwoPageSpread`) holds the rasterized
    /// bitmap(s) for `self.current_page_index` inside `(sidebar_w..viewport_w, top_bars_h..viewport_h - status_bar_h)`.
    pub(super) fn ensure_current_page_rendered(
        &mut self,
        viewport_w: u32,
        viewport_h: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        status_bar_h: u32,
    ) -> Result<()> {
        let content_w = viewport_w.saturating_sub(sidebar_w).max(1);
        let content_h = viewport_h
            .saturating_sub(top_bars_h + status_bar_h)
            .max(1);
        let total = self.total_pages();
        if total == 0 || self.doc.is_none() {
            return Ok(());
        }

        if self.view_mode == ViewMode::ContinuousScroll {
            if let Some(old) = self.cached_page.take() {
                if old.pixels.capacity() > self.spare_pixel_buf.capacity() {
                    self.spare_pixel_buf = old.pixels;
                }
            }
            self.cached_second_page = None;
            self.second_page_draw_rect = None;

            let avail_w = content_w.saturating_sub(PAGE_MARGIN_PX * 2).max(1) as f32;
            let avail_h = content_h.saturating_sub(PAGE_MARGIN_PX * 2).max(1) as f32;
            let gap = SPREAD_GAP_PX as f32;
            let min_screen_y = top_bars_h as f32;
            let max_screen_y = (viewport_h.saturating_sub(status_bar_h)) as f32;
            let center_y = min_screen_y + (content_h as f32) * 0.5;

            let ensure_page_cached = |app: &mut Self, p: u16| -> Result<(u32, u32)> {
                let rot = app.page_rotation_degrees(p);
                let need = app.continuous_page_cache.get(&p).map_or(true, |c| {
                    c.view_mode != ViewMode::ContinuousScroll
                        || c.viewport_width != content_w
                        || c.viewport_height != content_h
                        || c.rotation_deg != rot
                });
                if need {
                    let reused = app
                        .continuous_page_cache
                        .remove(&p)
                        .map(|c| c.pixels)
                        .unwrap_or_else(|| std::mem::take(&mut app.spare_pixel_buf));
                    let bmp = app.rasterize_page_for_mode(
                        p,
                        ViewMode::ContinuousScroll,
                        content_w,
                        content_h,
                        avail_w,
                        avail_h,
                        reused,
                    )?;
                    let dims = (bmp.bitmap_width, bmp.bitmap_height);
                    app.continuous_page_cache.insert(p, bmp);
                    Ok(dims)
                } else {
                    let c = &app.continuous_page_cache[&p];
                    Ok((c.bitmap_width, c.bitmap_height))
                }
            };

            // Normalize `(current_page_index, scroll_y)` so `current_page_index` follows the page at the viewport centre.
            for _ in 0..total {
                let (_, cur_h_u32) = ensure_page_cached(self, self.current_page_index)?;
                let cur_h = cur_h_u32 as f32;
                let cur_top = min_screen_y + (PAGE_MARGIN_PX as f32) - self.scroll_y;
                let cur_bottom = cur_top + cur_h;

                if center_y > cur_bottom + gap * 0.5 && self.current_page_index + 1 < total {
                    self.current_page_index += 1;
                    self.scroll_y -= cur_h + gap;
                } else if center_y < cur_top - gap * 0.5 && self.current_page_index > 0 {
                    let prev_idx = self.current_page_index - 1;
                    let (_, prev_h_u32) = ensure_page_cached(self, prev_idx)?;
                    self.current_page_index = prev_idx;
                    self.scroll_y += (prev_h_u32 as f32) + gap;
                } else {
                    break;
                }
            }

            // Clamp scroll at document top/bottom boundaries.
            if self.current_page_index == 0 && self.scroll_y < 0.0 {
                self.scroll_y = 0.0;
            }
            if self.current_page_index + 1 == total {
                let (_, last_h_u32) = ensure_page_cached(self, self.current_page_index)?;
                let max_s = (last_h_u32 as f32 - avail_h).max(0.0);
                if self.scroll_y > max_s {
                    self.scroll_y = max_s;
                }
            }

            // Collect visible pages above and below `current_page_index`.
            self.continuous_draw_rects.clear();
            let anchor_top = min_screen_y + (PAGE_MARGIN_PX as f32) - self.scroll_y;

            // Walk upward from `current_page_index - 1` while visible.
            let mut up_bottom = anchor_top - gap;
            let mut p_up = self.current_page_index;
            let mut upper_pages = Vec::new();
            while p_up > 0 && up_bottom > min_screen_y {
                p_up -= 1;
                let (bw, bh) = ensure_page_cached(self, p_up)?;
                let py = (up_bottom - bh as f32).round() as i32;
                let px = (sidebar_w as i32) + ((content_w as i32 - bw as i32) / 2);
                upper_pages.push((p_up, (px, py, bw, bh)));
                up_bottom -= (bh as f32) + gap;
            }
            for item in upper_pages.into_iter().rev() {
                self.continuous_draw_rects.push(item);
            }

            // Walk downward from `current_page_index` while visible.
            let mut down_top = anchor_top;
            let mut p_down = self.current_page_index;
            while p_down < total && down_top < max_screen_y {
                let (bw, bh) = ensure_page_cached(self, p_down)?;
                let py = down_top.round() as i32;
                let px = (sidebar_w as i32) + ((content_w as i32 - bw as i32) / 2);
                let rect = (px, py, bw, bh);
                if p_down == self.current_page_index {
                    self.page_draw_rect = rect;
                }
                self.continuous_draw_rects.push((p_down, rect));
                down_top += (bh as f32) + gap;
                p_down += 1;
            }

            let first_vis = self
                .continuous_draw_rects
                .first()
                .map(|&(idx, _)| idx)
                .unwrap_or(self.current_page_index);
            let last_vis = self
                .continuous_draw_rects
                .last()
                .map(|&(idx, _)| idx)
                .unwrap_or(self.current_page_index);

            // RAM rule: Cache only the visible pages plus 1 above and below. Evict everything else immediately.
            let keep_start = first_vis.saturating_sub(1);
            let keep_end = (last_vis + 1).min(total - 1);

            // Evict pages outside `[keep_start..=keep_end]` first so their buffer can be reused by neighbor prefetch.
            let evicted_keys: Vec<u16> = self
                .continuous_page_cache
                .keys()
                .copied()
                .filter(|&k| k < keep_start || k > keep_end)
                .collect();
            for k in evicted_keys {
                if let Some(old) = self.continuous_page_cache.remove(&k) {
                    if old.pixels.capacity() > self.spare_pixel_buf.capacity() {
                        self.spare_pixel_buf = old.pixels;
                    }
                }
            }

            if keep_start < first_vis {
                let _ = ensure_page_cached(self, keep_start);
            }
            if keep_end > last_vis {
                let _ = ensure_page_cached(self, keep_end);
            }

            return Ok(());
        }

        if self.view_mode == ViewMode::TwoPageSpread {
            self.scroll_y = 0.0;
            let avail_w = (content_w
                .saturating_sub(PAGE_MARGIN_PX * 2 + SPREAD_GAP_PX)
                / 2)
            .max(1) as f32;
            let avail_h = content_h.saturating_sub(PAGE_MARGIN_PX * 2).max(1) as f32;

            let left_idx = self.current_page_index;
            let left_rot = self.page_rotation_degrees(left_idx);
            let need_left = self.cached_page.as_ref().map_or(true, |c| {
                c.page_index != left_idx
                    || c.view_mode != ViewMode::TwoPageSpread
                    || c.viewport_width != content_w
                    || c.viewport_height != content_h
                    || c.rotation_deg != left_rot
            });
            if need_left {
                let reused = self
                    .cached_page
                    .take()
                    .map(|c| c.pixels)
                    .unwrap_or_else(|| std::mem::take(&mut self.spare_pixel_buf));
                self.cached_page = Some(self.rasterize_page_for_mode(
                    left_idx,
                    ViewMode::TwoPageSpread,
                    content_w,
                    content_h,
                    avail_w,
                    avail_h,
                    reused,
                )?);
            }

            let right_idx_opt = if left_idx + 1 < total {
                Some(left_idx + 1)
            } else {
                None
            };
            if let Some(right_idx) = right_idx_opt {
                let right_rot = self.page_rotation_degrees(right_idx);
                let need_right = self.cached_second_page.as_ref().map_or(true, |c| {
                    c.page_index != right_idx
                        || c.view_mode != ViewMode::TwoPageSpread
                        || c.viewport_width != content_w
                        || c.viewport_height != content_h
                        || c.rotation_deg != right_rot
                });
                if need_right {
                    let reused = self
                        .cached_second_page
                        .take()
                        .map(|c| c.pixels)
                        .unwrap_or_default();
                    self.cached_second_page = Some(self.rasterize_page_for_mode(
                        right_idx,
                        ViewMode::TwoPageSpread,
                        content_w,
                        content_h,
                        avail_w,
                        avail_h,
                        reused,
                    )?);
                }
            } else {
                self.cached_second_page = None;
            }

            let center_x = (sidebar_w as i32) + (content_w as i32) / 2;
            if let Some(left_bmp) = &self.cached_page {
                let lw = left_bmp.bitmap_width;
                let lh = left_bmp.bitmap_height;
                let lx = if self.cached_second_page.is_some() {
                    center_x - (SPREAD_GAP_PX as i32 / 2) - (lw as i32)
                } else {
                    center_x - (lw as i32 / 2)
                };
                let ly = (top_bars_h as i32) + ((content_h as i32 - lh as i32) / 2);
                self.page_draw_rect = (lx, ly, lw, lh);
            }
            if let Some(right_bmp) = &self.cached_second_page {
                let rw = right_bmp.bitmap_width;
                let rh = right_bmp.bitmap_height;
                let rx = center_x + (SPREAD_GAP_PX as i32 / 2);
                let ry = (top_bars_h as i32) + ((content_h as i32 - rh as i32) / 2);
                self.second_page_draw_rect = Some((rx, ry, rw, rh));
            } else {
                self.second_page_draw_rect = None;
            }

            return Ok(());
        }

        // Single-page view modes: FitPage, FitWidth, FitTextWidth
        self.cached_second_page = None;
        self.second_page_draw_rect = None;

        let rotation_deg = self.current_page_rotation_degrees();
        let page_w_pts = self
            .doc
            .as_ref()
            .and_then(|d| d.pages().get(self.current_page_index).ok())
            .map(|p| p.width().value.max(1.0))
            .unwrap_or(595.0);

        let need_render = self.cached_page.as_ref().map_or(true, |cache| {
            cache.page_index != self.current_page_index
                || cache.view_mode != self.view_mode
                || cache.viewport_width != content_w
                || cache.viewport_height != content_h
                || cache.rotation_deg != rotation_deg
        });

        if need_render {
            let reused = self
                .cached_page
                .take()
                .map(|c| c.pixels)
                .unwrap_or_else(|| std::mem::take(&mut self.spare_pixel_buf));
            let avail_w = match self.view_mode {
                ViewMode::FitWidth | ViewMode::FitTextWidth => {
                    content_w.saturating_sub(24).max(1) as f32
                }
                _ => content_w.saturating_sub(PAGE_MARGIN_PX * 2).max(1) as f32,
            };
            let avail_h = content_h.saturating_sub(PAGE_MARGIN_PX * 2).max(1) as f32;
            let rendered = self.rasterize_page_for_mode(
                self.current_page_index,
                self.view_mode,
                content_w,
                content_h,
                avail_w,
                avail_h,
                reused,
            )?;
            self.cached_page = Some(rendered);
        }

        if let Some(cache) = &self.cached_page {
            let bmp_w = cache.bitmap_width;
            let bmp_h = cache.bitmap_height;
            let c_min_x = cache.content_min_x;
            let c_max_x = cache.content_max_x;
            self.page_draw_rect = self.compute_single_page_draw_rect(
                sidebar_w,
                top_bars_h,
                content_w,
                content_h,
                bmp_w,
                bmp_h,
                c_min_x,
                c_max_x,
                page_w_pts,
            );
        }

        Ok(())
    }

    /// Blits a `CachedPageBitmap` into `buffer` clipped to the content area
    /// `(sidebar_w..buf_w, top_bars_h..buf_h - status_bar_h)` while applying `self.reading_filter` on the fly.
    pub(super) fn blit_cached_page(
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        status_bar_h: u32,
        draw_rect: (i32, i32, u32, u32),
        pixels: &[u32],
        filter: ReadingFilter,
    ) {
        let (draw_x, draw_y, bmp_w, bmp_h) = draw_rect;
        let min_screen_x = sidebar_w as i32;
        let max_screen_x = buf_w as i32;
        let min_screen_y = top_bars_h as i32;
        let max_screen_y = (buf_h.saturating_sub(status_bar_h)) as i32;

        let start_row = (min_screen_y - draw_y).max(0) as u32;
        let end_row = ((max_screen_y - draw_y).max(0) as u32).min(bmp_h);
        let start_col = (min_screen_x - draw_x).max(0) as u32;
        let end_col = ((max_screen_x - draw_x).max(0) as u32).min(bmp_w);

        if start_row >= end_row || start_col >= end_col {
            return;
        }

        let copy_len = (end_col - start_col) as usize;
        for row in start_row..end_row {
            let dst_y = (draw_y + row as i32) as u32;
            let dst_x = (draw_x + start_col as i32) as u32;
            let src_idx = (row * bmp_w + start_col) as usize;
            let dst_idx = (dst_y * buf_w + dst_x) as usize;

            match filter {
                ReadingFilter::Normal => {
                    buffer[dst_idx..dst_idx + copy_len]
                        .copy_from_slice(&pixels[src_idx..src_idx + copy_len]);
                }
                ReadingFilter::DarkInvert => {
                    for i in 0..copy_len {
                        let px = pixels[src_idx + i];
                        buffer[dst_idx + i] = px ^ 0x00FFFFFF;
                    }
                }
                ReadingFilter::WarmSepia => {
                    for i in 0..copy_len {
                        let px = pixels[src_idx + i];
                        let r = (px >> 16) & 0xFF;
                        let g = (((px >> 8) & 0xFF) * 95) / 100;
                        let b = ((px & 0xFF) * 83) / 100;
                        buffer[dst_idx + i] = (r << 16) | (g << 8) | b;
                    }
                }
            }
        }
    }

    /// Draws bright cyan/orange search highlight boxes for any `SearchMatch` on `page_idx`.
    pub(super) fn draw_search_overlays_on_page(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        top_bars_h: u32,
        status_bar_h: u32,
        page_idx: u16,
        draw_rect: (i32, i32, u32, u32),
    ) {
        if self.search_matches.is_empty() {
            return;
        }
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let Ok(page) = doc.pages().get(page_idx) else {
            return;
        };
        let (draw_x, draw_y, bmp_w, bmp_h) = draw_rect;
        if bmp_w == 0 || bmp_h == 0 {
            return;
        }
        let page_w = page.width().value.max(1.0);
        let page_h = page.height().value.max(1.0);
        let config = PdfRenderConfig::new()
            .set_fixed_width(bmp_w as i32)
            .set_fixed_height(bmp_h as i32);

        let min_y_bound = top_bars_h as i32;
        let max_y_bound = (buf_h.saturating_sub(status_bar_h)) as i32;

        for (m_idx, m) in self.search_matches.iter().enumerate() {
            if m.page_index != page_idx {
                continue;
            }
            let is_active = m_idx == self.current_search_match;
            let fill_rgba = if is_active {
                (255, 145, 0, 120) // Bright orange for active match
            } else {
                (0, 229, 255, 95) // Bright cyan for other matches on page
            };
            let border_col = if is_active { 0x00FF6D00 } else { 0x0000B8D4 };

            for r in &m.rects {
                let (bx1, by1, bx2, by2) = match (
                    page.points_to_pixels(r.left(), r.top(), &config),
                    page.points_to_pixels(r.right(), r.bottom(), &config),
                ) {
                    (Ok((x1, y1)), Ok((x2, y2))) => (x1, y1, x2, y2),
                    _ => {
                        let x1 = ((r.left().value / page_w) * (bmp_w as f32)).round() as i32;
                        let y1 =
                            ((1.0 - (r.top().value / page_h)) * (bmp_h as f32)).round() as i32;
                        let x2 = ((r.right().value / page_w) * (bmp_w as f32)).round() as i32;
                        let y2 =
                            ((1.0 - (r.bottom().value / page_h)) * (bmp_h as f32)).round() as i32;
                        (x1, y1, x2, y2)
                    }
                };

                let sx1 = (draw_x + bx1.min(bx2) - 2).clamp(0, buf_w as i32) as u32;
                let sx2 = (draw_x + bx1.max(bx2) + 2).clamp(0, buf_w as i32) as u32;
                let sy1 = (draw_y + by1.min(by2) - 2).clamp(min_y_bound, max_y_bound) as u32;
                let sy2 = (draw_y + by1.max(by2) + 2).clamp(min_y_bound, max_y_bound) as u32;

                if sx2 > sx1 && sy2 > sy1 {
                    blend_rect_rgba(
                        buffer,
                        buf_w,
                        buf_h,
                        sx1,
                        sy1,
                        sx2 - sx1,
                        sy2 - sy1,
                        fill_rgba,
                    );
                    draw_rect_border(
                        buffer,
                        buf_w,
                        buf_h,
                        sx1,
                        sy1,
                        sx2 - sx1,
                        sy2 - sy1,
                        border_col,
                    );
                }
            }
        }
    }

    pub(super) fn render_to_window(&mut self) -> Result<()> {
        let Some(window) = self.window.as_ref().cloned() else {
            return Ok(());
        };
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);
        let scale_factor = window.scale_factor();
        let sidebar_w = self
            .sidebar_width_px(scale_factor)
            .min(width.saturating_sub(64));
        let menu_bar_h = self
            .menu_bar_height_px(scale_factor)
            .min(height.saturating_sub(48));
        let tab_bar_h = self
            .tab_bar_height_px(scale_factor)
            .min(height.saturating_sub(menu_bar_h + 24));
        let top_bars_h = menu_bar_h + tab_bar_h;
        let status_bar_h = self
            .status_bar_height_px(scale_factor)
            .min(height.saturating_sub(top_bars_h + 16));

        if self.view_mode.is_page_view() {
            self.ensure_current_page_rendered(width, height, sidebar_w, top_bars_h, status_bar_h)?;
        } else {
            self.update_visible_thumbnails(width, height, sidebar_w, top_bars_h, status_bar_h);
        }

        let Some(mut surface) = self.surface.take() else {
            return Ok(());
        };
        let render_res = (|| -> Result<()> {
            surface
                .resize(
                    NonZeroU32::new(width).unwrap(),
                    NonZeroU32::new(height).unwrap(),
                )
                .map_err(|e| anyhow::anyhow!("Failed to resize softbuffer surface: {e}"))?;

            let mut buffer = surface
                .buffer_mut()
                .map_err(|e| anyhow::anyhow!("Failed to acquire softbuffer frame: {e}"))?;

            // Fill background with #2B2B2B
            buffer.fill(BG_COLOR_U32);

            if self.doc.is_none() {
                let scale = (scale_factor.round() as u32).clamp(1, 2);
                let hint1 = "Open PDF (O)";
                let hint2 = "Press 'O' or File -> Open PDF, or drag and drop a PDF file here";
                let cy = (top_bars_h + height.saturating_sub(status_bar_h)) / 2;
                let w1 = (hint1.len() as u32) * 8 * (scale * 2);
                let w2 = (hint2.len() as u32) * 8 * scale;
                let x1 = width.saturating_sub(w1) / 2;
                let x2 = width.saturating_sub(w2) / 2;
                draw_bitmap_str_8x8(
                    &mut buffer,
                    width,
                    height,
                    x1,
                    cy.saturating_sub(20 * scale),
                    hint1,
                    0x0064B5F6,
                    scale * 2,
                );
                draw_bitmap_str_8x8(
                    &mut buffer,
                    width,
                    height,
                    x2,
                    cy + 8 * scale,
                    hint2,
                    0x009E9E9E,
                    scale,
                );
            } else if self.view_mode.is_page_view() {
                if self.view_mode == ViewMode::ContinuousScroll {
                    for &(page_idx, draw_rect) in &self.continuous_draw_rects {
                        if let Some(cache) = self.continuous_page_cache.get(&page_idx) {
                            Self::blit_cached_page(
                                &mut buffer,
                                width,
                                height,
                                sidebar_w,
                                top_bars_h,
                                status_bar_h,
                                draw_rect,
                                &cache.pixels,
                                self.reading_filter,
                            );
                            self.draw_search_overlays_on_page(
                                &mut buffer,
                                width,
                                height,
                                top_bars_h,
                                status_bar_h,
                                page_idx,
                                draw_rect,
                            );
                        }
                    }
                } else {
                    // 1. Blit primary page bitmap (with zero-RAM ReadingFilter applied inline)
                    if let Some(cache) = &self.cached_page {
                        Self::blit_cached_page(
                            &mut buffer,
                            width,
                            height,
                            sidebar_w,
                            top_bars_h,
                            status_bar_h,
                            self.page_draw_rect,
                            &cache.pixels,
                            self.reading_filter,
                        );
                        self.draw_search_overlays_on_page(
                            &mut buffer,
                            width,
                            height,
                            top_bars_h,
                            status_bar_h,
                            cache.page_index,
                            self.page_draw_rect,
                        );
                    }

                    // 1b. If in TwoPageSpread mode, blit right-hand page bitmap as well
                    if self.view_mode == ViewMode::TwoPageSpread {
                        if let (Some(second_cache), Some(second_rect)) =
                            (&self.cached_second_page, self.second_page_draw_rect)
                        {
                            Self::blit_cached_page(
                                &mut buffer,
                                width,
                                height,
                                sidebar_w,
                                top_bars_h,
                                status_bar_h,
                                second_rect,
                                &second_cache.pixels,
                                self.reading_filter,
                            );
                            self.draw_search_overlays_on_page(
                                &mut buffer,
                                width,
                                height,
                                top_bars_h,
                                status_bar_h,
                                second_cache.page_index,
                                second_rect,
                            );
                        }
                    }
                }

                // 2. If user is holding Left-Click and dragging, draw live mode-colored selection box
                if let Some((sx1, sy1)) = self.drag_start {
                    let (sx2, sy2) = self.cursor_pos;
                    let (_, target_rect) = self.resolve_target_page_and_draw_rect(sx1, sy1);
                    let (px, py, pw, ph) = target_rect;
                    let page_left = px.max(sidebar_w as i32);
                    let page_right = (px + pw as i32).min(width as i32);
                    let page_top = py.max(top_bars_h as i32);
                    let page_bottom =
                        (py + ph as i32).min(height.saturating_sub(status_bar_h) as i32);

                    if page_right > page_left && page_bottom > page_top {
                        let min_x = (sx1.min(sx2).round() as i32)
                            .clamp(page_left, page_right) as u32;
                        let max_x = (sx1.max(sx2).round() as i32)
                            .clamp(page_left, page_right) as u32;
                        let min_y = (sy1.min(sy2).round() as i32)
                            .clamp(page_top, page_bottom) as u32;
                        let max_y = (sy1.max(sy2).round() as i32)
                            .clamp(page_top, page_bottom) as u32;

                        if max_x > min_x && max_y > min_y {
                            let (fill_rgba, border_col) = match self.mouse_mode {
                                MouseMode::Highlight => ((255, 235, 59, 90), 0x00FBC02D),
                                MouseMode::Redact => ((229, 57, 53, 110), 0x00FF1744),
                                MouseMode::Crop => ((0, 230, 118, 85), 0x0000E676),
                                MouseMode::StampImage => ((41, 182, 246, 95), 0x0000B0FF),
                            };
                            blend_rect_rgba(
                                &mut buffer,
                                width,
                                height,
                                min_x,
                                min_y,
                                max_x - min_x,
                                max_y - min_y,
                                fill_rgba,
                            );
                            draw_rect_border(
                                &mut buffer,
                                width,
                                height,
                                min_x,
                                min_y,
                                max_x - min_x,
                                max_y - min_y,
                                border_col,
                            );
                        }
                    }
                }
            } else {
                self.draw_thumbnail_grid(
                    &mut buffer,
                    width,
                    height,
                    sidebar_w,
                    top_bars_h,
                    status_bar_h,
                );
            }

            // 3. If Left Bookmark Sidebar (`B`) is active, draw the 240px sidebar panel between top_bars_h and status_bar_h
            if self.show_bookmarks && sidebar_w > 0 {
                self.draw_bookmark_sidebar(
                    &mut buffer,
                    width,
                    height.saturating_sub(status_bar_h),
                    sidebar_w,
                    top_bars_h,
                    scale_factor,
                );
            }

            // 4. Draw the 28px Multi-Document Tab Bar (`y = menu_bar_h .. top_bars_h`)
            self.draw_tab_bar(
                &mut buffer,
                width,
                height,
                menu_bar_h,
                tab_bar_h,
                scale_factor,
            );

            // 5. Draw the 24px Top Menu Bar (`y = 0 .. menu_bar_h`)
            self.draw_menu_bar(&mut buffer, width, height, menu_bar_h, scale_factor);

            // 6. Draw the 22px Bottom Command & Status Bar (`y = height - status_bar_h .. height`)
            self.draw_status_bar(&mut buffer, width, height, status_bar_h, scale_factor);

            // 7. If a Top Menu dropdown is open, draw it over the viewport
            if let Some(cat) = self.open_menu {
                self.draw_open_menu_dropdown(
                    &mut buffer,
                    width,
                    height,
                    menu_bar_h,
                    cat,
                    scale_factor,
                );
            }

            // 8. If Help Overlay (`?` or `F1`) is active, draw the modal overlay
            if self.show_help_overlay {
                draw_help_overlay(&mut buffer, width, height, scale_factor);
            }

            buffer
                .present()
                .map_err(|e| anyhow::anyhow!("Failed to present softbuffer frame: {e}"))?;

            Ok(())
        })();

        self.surface = Some(surface);
        render_res?;

        self.update_title_bar();
        Ok(())
    }
}

/// Fills a clipped rectangle in the `0x00RRGGBB` pixel buffer.
pub(super) fn fill_rect_clipped(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    rx: i32,
    ry: i32,
    rw: u32,
    rh: u32,
    color: u32,
) {
    let x0 = rx.max(0) as u32;
    let y0 = ry.max(0) as u32;
    let x1 = (rx + rw as i32).clamp(0, buf_w as i32) as u32;
    let y1 = (ry + rh as i32).clamp(0, buf_h as i32) as u32;
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    for y in y0..y1 {
        let start = (y * buf_w + x0) as usize;
        let end = (y * buf_w + x1) as usize;
        buffer[start..end].fill(color);
    }
}

/// Alpha-blends an `(r, g, b, a)` rectangle onto the `0x00RRGGBB` pixel buffer.
pub(super) fn blend_rect_rgba(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    rx: u32,
    ry: u32,
    rw: u32,
    rh: u32,
    rgba: (u32, u32, u32, u32),
) {
    let (sr, sg, sb, sa) = rgba;
    let inv_a = 255 - sa;
    let x_end = (rx + rw).min(buf_w);
    let y_end = (ry + rh).min(buf_h);

    for y in ry..y_end {
        let row_start = (y * buf_w) as usize;
        for x in rx..x_end {
            let idx = row_start + (x as usize);
            let dst = buffer[idx];
            let dr = (dst >> 16) & 0xFF;
            let dg = (dst >> 8) & 0xFF;
            let db = dst & 0xFF;

            let out_r = (sr * sa + dr * inv_a) / 255;
            let out_g = (sg * sa + dg * inv_a) / 255;
            let out_b = (sb * sa + db * inv_a) / 255;

            buffer[idx] = (out_r << 16) | (out_g << 8) | out_b;
        }
    }
}

/// Draws a 1px solid border around a rectangle in the `0x00RRGGBB` buffer.
pub(super) fn draw_rect_border(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    rx: u32,
    ry: u32,
    rw: u32,
    rh: u32,
    color: u32,
) {
    if rw == 0 || rh == 0 {
        return;
    }
    let x_end = (rx + rw).min(buf_w);
    let y_end = (ry + rh).min(buf_h);

    for x in rx..x_end {
        if ry < buf_h {
            buffer[(ry * buf_w + x) as usize] = color;
        }
        if y_end > 0 && y_end - 1 < buf_h {
            buffer[((y_end - 1) * buf_w + x) as usize] = color;
        }
    }
    for y in ry..y_end {
        if rx < buf_w {
            buffer[(y * buf_w + rx) as usize] = color;
        }
        if x_end > 0 && x_end - 1 < buf_w {
            buffer[(y * buf_w + (x_end - 1)) as usize] = color;
        }
    }
}

/// Renders a lightweight CPU bitmap Help Overlay (`?` / `F1`) directly into the window buffer.
pub(super) fn draw_help_overlay(buffer: &mut [u32], buf_w: u32, buf_h: u32, scale_factor: f64) {
    let scale = (scale_factor.round() as u32).clamp(1, 2);
    let char_w = 8 * scale;
    let line_h = 14 * scale;

    let lines = [
        "=== MICRO_PDF PHASE 5: MENU BAR, VIEW MODES & POWER TOOLS ===",
        "",
        "TOP MENU BAR : Click [File] [View] [Edit] [Page] [Tools] [Help]",
        "TOP TAB BAR  : Click Tab to Switch, [x] or Cmd/Ctrl+W Close, [+] Open",
        "VIEW MODES   : '1' Fit Page | '2' Fit Width | '3' Fit Text | '4' 2-Page",
        "GRID & BKMKS : 'Tab' Thumbnail Grid | 'B' Bookmark Sidebar | 'L' Import",
        "FIND TEXT    : Cmd/Ctrl+F Search Text in PDF | F3 / Cmd+G Next Match",
        "MOUSE MODES  : Highlight (default) | Redact (Edit menu) | Crop (Edit)",
        "STAMP IMAGE  : 'P' Pick PNG/JPG Signature -> Click/Drag on Page to Stamp",
        "READ FILTER  : 'V' Cycle Normal / Dark Invert / Warm Sepia (0-RAM Blit)",
        "FLATTEN PDF  : 'U' Flatten Annotations/Forms & Unlock Permissions",
        "BATCH COMBINE: 'Shift+M' Combine 2+ PDFs in Order into 'Combined.pdf'",
        "SMART JUMP   : 'G' or 0-9 (Logical '5', Roman 'ii', or Physical '#5')",
        "MARGINS & #  : 'N' Roman/Arabic # | 'H' Header/Footer | 'C' Clear Zone",
        "PAGE TOOLS   : 'W' Watermark | 'K' Grayscale [BW] | 'I' Insert | 'R' Rot",
        "FILE & UNDO  : 'O' Open | 'S' Save | 'Z' Compress | 'X' Split | Cmd+Z Undo",
        "? / F1 / ESC : Toggle or Close this Help Overlay",
    ];

    let max_chars = lines.iter().map(|l| l.len()).max().unwrap_or(40) as u32;
    let pad_x = 18 * scale;
    let pad_y = 16 * scale;
    let box_w = (max_chars * char_w + pad_x * 2).min(buf_w.saturating_sub(16));
    let box_h = ((lines.len() as u32) * line_h + pad_y * 2).min(buf_h.saturating_sub(16));
    let box_x = buf_w.saturating_sub(box_w) / 2;
    let box_y = buf_h.saturating_sub(box_h) / 2;

    blend_rect_rgba(
        buffer,
        buf_w,
        buf_h,
        box_x,
        box_y,
        box_w,
        box_h,
        (20, 24, 33, 238),
    );
    draw_rect_border(buffer, buf_w, buf_h, box_x, box_y, box_w, box_h, 0x0064B5F6);

    for (idx, line) in lines.iter().enumerate() {
        let color = if idx == 0 {
            0x00FFEB3B
        } else {
            0x00F5F5F5
        };
        let tx = box_x + pad_x;
        let ty = box_y + pad_y + (idx as u32) * line_h;
        draw_bitmap_str_8x8(buffer, buf_w, buf_h, tx, ty, line, color, scale);
    }
}

/// Built-in 8x8 bitmap font rasterizer for Bookmark Sidebar, Thumbnail Grid labels, and Help Overlay.
pub(super) fn draw_bitmap_str_8x8(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    x: u32,
    y: u32,
    text: &str,
    color: u32,
    scale: u32,
) {
    let mut cx = x;
    for ch in text.chars() {
        let glyph = glyph_8x8(ch);
        for (row_idx, &row_bits) in glyph.iter().enumerate() {
            for col_idx in 0..8u32 {
                if (row_bits >> (7 - col_idx)) & 1 == 1 {
                    let px = cx + col_idx * scale;
                    let py = y + (row_idx as u32) * scale;
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let gx = px + dx;
                            let gy = py + dy;
                            if gx < buf_w && gy < buf_h {
                                buffer[(gy * buf_w + gx) as usize] = color;
                            }
                        }
                    }
                }
            }
        }
        cx += 8 * scale;
    }
}

/// Crisp 8x8 ASCII bitmap font glyph generator (embeds standard 5x7 glyphs centered in an 8x8 cell).
pub(super) fn glyph_8x8(c: char) -> [u8; 8] {
    let g5x7: [u8; 7] = match c.to_ascii_uppercase() {
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'D' => [0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        ':' => [0b00000, 0b00100, 0b00100, 0b00000, 0b00100, 0b00100, 0b00000],
        '-' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        '+' => [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000],
        '/' => [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
        '(' => [0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010],
        ')' => [0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000],
        '[' => [0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110],
        ']' => [0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110],
        '&' => [0b01100, 0b10010, 0b10100, 0b01000, 0b10101, 0b10010, 0b01101],
        '?' => [0b01110, 0b10001, 0b00010, 0b00100, 0b00100, 0b00000, 0b00100],
        '=' => [0b00000, 0b00000, 0b11111, 0b00000, 0b11111, 0b00000, 0b00000],
        '_' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b11111],
        '#' => [0b01010, 0b11111, 0b01010, 0b01010, 0b11111, 0b01010, 0b00000],
        '\'' => [0b00100, 0b00100, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000],
        '.' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00100, 0b00100],
        ',' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00100, 0b00100, 0b01000],
        '|' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        '!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100],
        '%' => [0b11001, 0b11010, 0b00010, 0b00100, 0b01000, 0b01011, 0b10011],
        '<' => [0b00010, 0b00100, 0b01000, 0b10000, 0b01000, 0b00100, 0b00010],
        '>' => [0b01000, 0b00100, 0b00010, 0b00001, 0b00010, 0b00100, 0b01000],
        '°' => [0b00110, 0b01001, 0b01001, 0b00110, 0b00000, 0b00000, 0b00000],
        '—' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        _ => [0; 7],
    };
    [
        g5x7[0] << 2,
        g5x7[1] << 2,
        g5x7[2] << 2,
        g5x7[3] << 2,
        g5x7[4] << 2,
        g5x7[5] << 2,
        g5x7[6] << 2,
        0,
    ]
}

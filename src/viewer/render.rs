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
        if pixels.capacity() > pixel_count.saturating_mul(2) {
            pixels.shrink_to_fit();
        }

        let bindings = self.engine.pdfium.bindings();
        let config = PdfRenderConfig::new()
            .set_fixed_width(target_w)
            .set_fixed_height(target_h)
            .render_form_data(true)
            .render_annotations(true)
            .use_lcd_text_rendering(lcd_text)
            .limit_render_image_cache_size(true)
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

    /// Returns true if `(virtual_w, virtual_h)` exceeds 2x the viewport size `(content_w, content_h)`.
    /// Above 2x the screen size, MicroPDF never allocates a full-page bitmap and renders only
    /// visible 512x512 tiles on demand into the 16 MB LRU `tile_cache`.
    pub(super) fn is_tiled_for_viewport(
        virtual_w: u32,
        virtual_h: u32,
        content_w: u32,
        content_h: u32,
    ) -> bool {
        virtual_w > content_w.max(1).saturating_mul(2)
            || virtual_h > content_h.max(1).saturating_mul(2)
    }

    /// Renders a single 512x512 tile `key` into `self.tile_cache`, evicting the least-recently-used
    /// tile (and reusing its pixel buffer) if `tile_cache` reaches `MAX_TILE_CACHE_ENTRIES` (16 MB).
    pub(super) fn ensure_tile_rendered(&mut self, key: TileKey) -> Result<()> {
        self.tile_lru_tick = self.tile_lru_tick.wrapping_add(1);
        let tick = self.tile_lru_tick;
        if let Some(entry) = self.tile_cache.get_mut(&key) {
            entry.last_used = tick;
            return Ok(());
        }

        let mut reused_pixels = if self.tile_cache.len() >= MAX_TILE_CACHE_ENTRIES {
            let lru_key = self
                .tile_cache
                .iter()
                .min_by_key(|(_, v)| v.last_used)
                .map(|(&k, _)| k);
            if let Some(evict_k) = lru_key {
                self.tile_cache
                    .remove(&evict_k)
                    .map(|e| e.pixels)
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        } else {
            std::mem::take(&mut self.spare_pixel_buf)
        };

        let has_form_widgets = {
            let doc = self
                .doc
                .as_ref()
                .context("No active PDF document to render tile")?;
            let page = doc
                .pages()
                .get(key.page_index)
                .context("Page index out of bounds for tile")?;
            PdfEngine::page_has_form_widgets(&page)
        };
        // Tiles are matrix renders, which PDFium draws without form fields. A page with form
        // fields is tiled from a flattened one-page copy (the document itself is untouched),
        // kept while that page is zoomed and dropped by `invalidate_page_caches`.
        if has_form_widgets
            && self
                .tile_form_copy
                .as_ref()
                .is_none_or(|(idx, _)| *idx != key.page_index)
        {
            self.tile_form_copy = None;
            let doc = self.doc.as_ref().context("No active PDF document to render tile")?;
            let copy = self.engine.flattened_page_copy(doc, key.page_index)?;
            self.tile_form_copy = Some((key.page_index, copy));
        }
        let page = match (&self.tile_form_copy, has_form_widgets) {
            (Some((_, copy)), true) => copy.pages().get(0).context("Form page copy is empty")?,
            _ => self
                .doc
                .as_ref()
                .context("No active PDF document to render tile")?
                .pages()
                .get(key.page_index)
                .context("Page index out of bounds for tile")?,
        };

        let page_w_pts = page.width().value.max(1.0);
        let page_h_pts = page.height().value.max(1.0);
        let sx = (key.virtual_w as f32) / page_w_pts;
        let sy = (key.virtual_h as f32) / page_h_pts;

        let tile_x = (key.tile_col as u32) * TILE_SIZE_PX;
        let tile_y = (key.tile_row as u32) * TILE_SIZE_PX;
        let tile_w = (key.virtual_w.saturating_sub(tile_x)).min(TILE_SIZE_PX).max(1) as i32;
        let tile_h = (key.virtual_h.saturating_sub(tile_y)).min(TILE_SIZE_PX).max(1) as i32;

        let pixel_count = (tile_w as usize) * (tile_h as usize);
        let max_tile_pixels = (TILE_SIZE_PX as usize) * (TILE_SIZE_PX as usize);
        reused_pixels.clear();
        reused_pixels.resize(pixel_count, 0x00FFFFFF);
        if reused_pixels.capacity() > max_tile_pixels {
            reused_pixels.shrink_to_fit();
        }

        let bindings = self.engine.pdfium.bindings();
        let config = PdfRenderConfig::new()
            .set_fixed_width(tile_w)
            .set_fixed_height(tile_h)
            .render_form_data(false)
            .render_annotations(true)
            .use_lcd_text_rendering(true)
            .limit_render_image_cache_size(true)
            .set_reverse_byte_order(false)
            .reset_matrix(PdfMatrix::new(
                sx,
                0.0,
                0.0,
                sy,
                -(tile_x as f32),
                -(tile_y as f32),
            ))
            .context("Invalid tile transformation matrix")?;

        {
            let byte_slice: &mut [u8] = unsafe {
                std::slice::from_raw_parts_mut(
                    reused_pixels.as_mut_ptr() as *mut u8,
                    pixel_count * 4,
                )
            };
            let mut bitmap = unsafe {
                PdfBitmap::from_bytes(
                    tile_w,
                    tile_h,
                    PdfBitmapFormat::BGRx,
                    byte_slice,
                    bindings,
                )
            }
            .context("Failed to wrap tile buffer in PdfBitmap")?;

            page.render_into_bitmap_with_config(&mut bitmap, &config)
                .context("Failed to render PDF tile into bitmap")?;
        }
        drop(page);

        for px in &mut reused_pixels {
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

        self.tile_cache.insert(
            key,
            TileEntry {
                width: tile_w as u32,
                height: tile_h as u32,
                last_used: tick,
                pixels: reused_pixels,
            },
        );
        Ok(())
    }

    /// Renders all visible 512x512 tiles for `page_idx` at `draw_rect` within the viewport.
    pub(super) fn ensure_visible_tiles_for_page(
        &mut self,
        page_idx: u16,
        draw_rect: (i32, i32, u32, u32),
        rotation_deg: i32,
        buf_w: u32,
        buf_h: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        status_bar_h: u32,
    ) -> Result<()> {
        let (draw_x, draw_y, virtual_w, virtual_h) = draw_rect;
        let min_screen_x = sidebar_w as i32;
        let max_screen_x = buf_w as i32;
        let min_screen_y = top_bars_h as i32;
        let max_screen_y = (buf_h.saturating_sub(status_bar_h)) as i32;

        let vis_x0 = (min_screen_x - draw_x).max(0) as u32;
        let vis_x1 = ((max_screen_x - draw_x).max(0) as u32).min(virtual_w);
        let vis_y0 = (min_screen_y - draw_y).max(0) as u32;
        let vis_y1 = ((max_screen_y - draw_y).max(0) as u32).min(virtual_h);

        if vis_x0 >= vis_x1 || vis_y0 >= vis_y1 {
            return Ok(());
        }

        let col_start = (vis_x0 / TILE_SIZE_PX) as u16;
        let col_end = ((vis_x1 + TILE_SIZE_PX - 1) / TILE_SIZE_PX) as u16;
        let row_start = (vis_y0 / TILE_SIZE_PX) as u16;
        let row_end = ((vis_y1 + TILE_SIZE_PX - 1) / TILE_SIZE_PX) as u16;

        // Evict off-screen tiles for this page first so their 512x512 buffers can be reused.
        let mut evicted_bufs: Vec<Vec<u32>> = Vec::new();
        if !self.is_continuous_scroll() && self.view_mode != ViewMode::TwoPageSpread {
            let max_tile_pixels = (TILE_SIZE_PX as usize) * (TILE_SIZE_PX as usize);
            let stale_keys: Vec<TileKey> = self
                .tile_cache
                .keys()
                .copied()
                .filter(|k| {
                    k.page_index != page_idx
                        || k.virtual_w != virtual_w
                        || k.virtual_h != virtual_h
                        || k.rotation_deg != rotation_deg
                        || k.tile_col < col_start
                        || k.tile_col >= col_end
                        || k.tile_row < row_start
                        || k.tile_row >= row_end
                })
                .collect();
            for sk in stale_keys {
                if let Some(old_tile) = self.tile_cache.remove(&sk) {
                    if old_tile.pixels.capacity() <= max_tile_pixels {
                        evicted_bufs.push(old_tile.pixels);
                    }
                }
            }
        }

        for tile_row in row_start..row_end {
            for tile_col in col_start..col_end {
                let key = TileKey {
                    page_index: page_idx,
                    virtual_w,
                    virtual_h,
                    rotation_deg,
                    tile_col,
                    tile_row,
                };
                if !self.tile_cache.contains_key(&key) && self.spare_pixel_buf.is_empty() {
                    if let Some(buf) = evicted_bufs.pop() {
                        self.spare_pixel_buf = buf;
                    }
                }
                self.ensure_tile_rendered(key)?;
            }
        }
        Ok(())
    }

    /// Rasterizes a single `page_idx` for the specified `mode` and available `(avail_w, avail_h)`,
    /// reusing `reused_pixels` when provided. Above 2x the screen size, returns an empty `pixels`
    /// Vec and delegates rendering to `ensure_visible_tiles_for_page` (512x512 LRU tile cache).
    pub(super) fn rasterize_page_for_mode(
        &mut self,
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
        let rotation_deg = Self::rotation_degrees_of_page(&page);

        let page_w_pts = page.width().value.max(1.0);
        let page_h_pts = page.height().value.max(1.0);

        let (content_min_x, content_min_y, content_max_x, content_max_y) = if mode == ViewMode::FitTextWidth {
            PdfEngine::compute_page_content_bounds_with_scan(&page)
        } else {
            (0.0, 0.0, page_w_pts, page_h_pts)
        };

        let base_scale = match mode {
            ViewMode::FitPage
            | ViewMode::TwoPageSpread
            | ViewMode::ContinuousScroll
            | ViewMode::TextView
            | ViewMode::ThumbnailGrid => (avail_w / page_w_pts).min(avail_h / page_h_pts),
            ViewMode::FitWidth => avail_w / page_w_pts,
            ViewMode::FitTextWidth => {
                let text_w = (content_max_x - content_min_x).max(page_w_pts * 0.25);
                avail_w / text_w
            }
        };

        let zoom = self.zoom_level.clamp(MIN_ZOOM_LEVEL, MAX_ZOOM_LEVEL);
        let scale = (base_scale * zoom).max(0.05);

        let target_w = ((page_w_pts * scale).round() as i32).max(1);
        let target_h = ((page_h_pts * scale).round() as i32).max(1);
        let virtual_w = target_w as u32;
        let virtual_h = target_h as u32;

        // RAM rule: Above 2x the screen size, never render the whole page bitmap; use 512x512 tiles.
        let pixels = if Self::is_tiled_for_viewport(virtual_w, virtual_h, content_w, content_h) {
            drop(page);
            let max_tile_pixels = (TILE_SIZE_PX as usize) * (TILE_SIZE_PX as usize);
            if reused_pixels.capacity() > self.spare_pixel_buf.capacity()
                && reused_pixels.capacity() <= max_tile_pixels
            {
                self.spare_pixel_buf = reused_pixels;
            }
            Vec::new()
        } else {
            let rendered =
                self.render_page_into_u32_vec(&page, target_w, target_h, true, reused_pixels)?;
            drop(page);
            rendered
        };

        Ok(CachedPageBitmap {
            page_index: page_idx,
            view_mode: mode,
            zoom_bits: zoom.to_bits(),
            viewport_width: content_w,
            viewport_height: content_h,
            bitmap_width: virtual_w,
            bitmap_height: virtual_h,
            content_min_x,
            content_max_x,
            content_min_y,
            content_max_y,
            rotation_deg,
            pixels,
        })
    }

    /// Computes `(draw_x, draw_y, bmp_w, bmp_h)` for a single-page view (`FitPage`, `FitWidth`, `FitTextWidth`)
    /// and clamps `self.pan_x` and `self.scroll_y` to valid bounds.
    #[allow(clippy::too_many_arguments)]
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
        content_min_y: f32,
        content_max_y: f32,
        page_w_pts: f32,
        page_h_pts: f32,
    ) -> (i32, i32, u32, u32) {
        let draw_x = if self.view_mode == ViewMode::FitTextWidth && page_w_pts > 1.0 {
            let scale_x = (bmp_w as f32) / page_w_pts;
            let text_w_px = ((content_max_x - content_min_x).max(1.0) * scale_x).round();
            if text_w_px > content_w as f32 + 2.0 {
                // User zoomed in beyond text width: pan across the text column cleanly
                let left_margin_px = (content_min_x * scale_x).round();
                let max_extra_pan = (text_w_px - content_w as f32).max(0.0);
                self.pan_x = self.pan_x.clamp(0.0, max_extra_pan);
                (sidebar_w as i32) - (left_margin_px as i32) - (self.pan_x.round() as i32)
            } else {
                // Center strictly on the detected text column, pushing left/right white margins off-screen!
                self.pan_x = 0.0;
                let text_center_bx = ((content_min_x + content_max_x) * 0.5 * scale_x).round() as i32;
                let viewport_center_x = (sidebar_w as i32) + (content_w as i32) / 2;
                viewport_center_x - text_center_bx
            }
        } else if bmp_w > content_w {
            let max_pan = (bmp_w - content_w) as f32;
            self.pan_x = self.pan_x.clamp(0.0, max_pan);
            (sidebar_w as i32) - (self.pan_x.round() as i32)
        } else {
            self.pan_x = 0.0;
            (sidebar_w as i32) + ((content_w as i32 - bmp_w as i32) / 2)
        };

        let draw_y = if self.view_mode == ViewMode::FitTextWidth && page_h_pts > 1.0 {
            let scale_y = (bmp_h as f32) / page_h_pts;
            // Crop top & bottom white margins cleanly so text starts right near top of viewport!
            let top_crop_px = (((page_h_pts - content_max_y).max(0.0) * scale_y) - 8.0).max(0.0);
            let bot_crop_px = ((content_min_y.max(0.0) * scale_y) - 12.0).max(0.0);
            let eff_h_px = (bmp_h as f32 - top_crop_px - bot_crop_px).max(1.0);
            if eff_h_px > content_h as f32 {
                let max_scroll = (eff_h_px - content_h as f32).max(0.0);
                self.scroll_y = self.scroll_y.clamp(0.0, max_scroll);
                (top_bars_h as i32) - (top_crop_px.round() as i32) - (self.scroll_y.round() as i32)
            } else {
                self.scroll_y = 0.0;
                let top_pad = ((content_h as f32 - eff_h_px) * 0.5).round() as i32;
                (top_bars_h as i32) + top_pad - (top_crop_px.round() as i32)
            }
        } else if bmp_h > content_h {
            let max_scroll = (bmp_h - content_h) as f32;
            self.scroll_y = self.scroll_y.clamp(0.0, max_scroll);
            (top_bars_h as i32) - (self.scroll_y.round() as i32)
        } else {
            self.scroll_y = 0.0;
            (top_bars_h as i32) + ((content_h as i32 - bmp_h as i32) / 2)
        };

        (draw_x, draw_y, bmp_w, bmp_h)
    }

    /// Renders the page view for this frame. A page PDFium cannot read (for example because the
    /// open file was overwritten or truncated on disk) leaves that page blank and puts the error
    /// in the status bar, so the rest of the window is still drawn. Returns true on success.
    pub(super) fn render_page_view_or_notice(
        &mut self,
        viewport_w: u32,
        viewport_h: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        status_bar_h: u32,
    ) -> bool {
        match self.ensure_current_page_rendered(
            viewport_w,
            viewport_h,
            sidebar_w,
            top_bars_h,
            status_bar_h,
        ) {
            Ok(()) => true,
            Err(e) => {
                self.status_notice = Some(format!(
                    "Cannot render page {}: {e:#}",
                    self.current_page_index + 1
                ));
                false
            }
        }
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

        let zoom_bits = self.zoom_level.clamp(MIN_ZOOM_LEVEL, MAX_ZOOM_LEVEL).to_bits();

        if self.is_continuous_scroll() {
            if let Some(old) = self.cached_page.take() {
                if old.pixels.capacity() > self.spare_pixel_buf.capacity() {
                    self.spare_pixel_buf = old.pixels;
                }
            }
            self.cached_second_page = None;
            self.second_page_draw_rect = None;

            let scroll_mode = self.effective_scroll_mode();
            let layout_mode = self.effective_layout_mode();
            let is_spread = layout_mode == ViewMode::TwoPageSpread;

            let avail_w = if is_spread {
                (content_w.saturating_sub(PAGE_MARGIN_PX * 2 + SPREAD_GAP_PX) / 2).max(1) as f32
            } else {
                content_w.saturating_sub(PAGE_MARGIN_PX * 2).max(1) as f32
            };
            let avail_h = content_h.saturating_sub(PAGE_MARGIN_PX * 2).max(1) as f32;
            let gap = SPREAD_GAP_PX as f32;
            let min_screen_y = top_bars_h as f32;
            let max_screen_y = (viewport_h.saturating_sub(status_bar_h)) as f32;
            let center_y = min_screen_y + (content_h as f32) * 0.5;
            let min_screen_x = sidebar_w as f32;
            let max_screen_x = viewport_w as f32;
            let center_x = min_screen_x + (content_w as f32) * 0.5;

            let ensure_page_cached = |app: &mut Self, p: u16| -> Result<(u32, u32)> {
                let need = app.continuous_page_cache.get(&p).map_or(true, |c| {
                    c.view_mode != layout_mode
                        || c.zoom_bits != zoom_bits
                        || c.viewport_width != content_w
                        || c.viewport_height != content_h
                });
                if need {
                    let reused = app
                        .continuous_page_cache
                        .remove(&p)
                        .map(|c| c.pixels)
                        .unwrap_or_else(|| std::mem::take(&mut app.spare_pixel_buf));
                    let bmp = app.rasterize_page_for_mode(
                        p,
                        layout_mode,
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

            let compute_single_px = |app: &mut Self, p: u16, bw: u32| -> i32 {
                if layout_mode == ViewMode::FitTextWidth {
                    if let Some(c) = app.continuous_page_cache.get(&p) {
                        if let Some(doc) = app.doc.as_ref() {
                            if let Ok(page) = doc.pages().get(p) {
                                let pw_pts = page.width().value.max(1.0);
                                let text_mid_px = ((c.content_min_x + c.content_max_x) * 0.5 / pw_pts) * (bw as f32);
                                let ideal_x = (sidebar_w as f32) + (content_w as f32) * 0.5 - text_mid_px;
                                let min_x = (sidebar_w as f32) + (content_w as f32) - (bw as f32);
                                let max_x = sidebar_w as f32;
                                let base = if min_x < max_x {
                                    ideal_x.clamp(min_x, max_x)
                                } else {
                                    (sidebar_w as f32) + ((content_w as f32 - bw as f32) * 0.5)
                                };
                                return (base - app.pan_x).round() as i32;
                            }
                        }
                    }
                }
                if bw > content_w {
                    let max_pan = (bw - content_w) as f32;
                    app.pan_x = app.pan_x.clamp(0.0, max_pan);
                    (sidebar_w as i32) - (app.pan_x.round() as i32)
                } else {
                    (sidebar_w as i32) + ((content_w as i32 - bw as i32) / 2)
                }
            };

            self.continuous_draw_rects.clear();

            if scroll_mode == ScrollMode::ContinuousHorizontal {
                // Continuous Horizontal "Book Ribbon" Scroll: pages flow smoothly left-to-right!
                for _ in 0..total {
                    let (cur_w_u32, _) = ensure_page_cached(self, self.current_page_index)?;
                    let cur_w = cur_w_u32 as f32;
                    let cur_left = min_screen_x + (PAGE_MARGIN_PX as f32) - self.scroll_x;
                    let cur_right = cur_left + cur_w;
                    if center_x > cur_right + gap * 0.5 && self.current_page_index + 1 < total {
                        self.current_page_index += 1;
                        self.scroll_x -= cur_w + gap;
                        self.cached_char_boxes = None;
                    } else if center_x < cur_left - gap * 0.5 && self.current_page_index > 0 {
                        let prev_idx = self.current_page_index - 1;
                        let (prev_w_u32, _) = ensure_page_cached(self, prev_idx)?;
                        self.current_page_index = prev_idx;
                        self.scroll_x += (prev_w_u32 as f32) + gap;
                        self.cached_char_boxes = None;
                    } else {
                        break;
                    }
                }

                if self.current_page_index == 0 && self.scroll_x < 0.0 {
                    self.scroll_x = 0.0;
                }
                if self.current_page_index + 1 == total {
                    let (last_w_u32, _) = ensure_page_cached(self, self.current_page_index)?;
                    let max_sx = (last_w_u32 as f32 - avail_w).max(0.0);
                    if self.scroll_x > max_sx {
                        self.scroll_x = max_sx;
                    }
                }

                let (_, cur_h_u32) = ensure_page_cached(self, self.current_page_index)?;
                let cur_max_sy = (cur_h_u32.saturating_sub(content_h)) as f32;
                self.scroll_y = self.scroll_y.clamp(0.0, cur_max_sy);

                let anchor_left = min_screen_x + (PAGE_MARGIN_PX as f32) - self.scroll_x;
                let compute_hy = |app: &Self, bh: u32| -> i32 {
                    if bh > content_h {
                        let max_sy = (bh - content_h) as f32;
                        let sy = app.scroll_y.clamp(0.0, max_sy);
                        (top_bars_h as i32) - (sy.round() as i32)
                    } else {
                        (top_bars_h as i32) + ((content_h as i32 - bh as i32) / 2)
                    }
                };

                // Walk left from current_page_index - 1
                let mut left_edge = anchor_left - gap;
                let mut p_l = self.current_page_index;
                let mut left_pages = Vec::new();
                while p_l > 0 && left_edge > min_screen_x {
                    p_l -= 1;
                    let (bw, bh) = ensure_page_cached(self, p_l)?;
                    let px = (left_edge - bw as f32).round() as i32;
                    let py = compute_hy(self, bh);
                    left_pages.push((p_l, (px, py, bw, bh)));
                    left_edge -= (bw as f32) + gap;
                }
                for item in left_pages.into_iter().rev() {
                    self.continuous_draw_rects.push(item);
                }

                // Walk right from current_page_index
                let mut right_edge = anchor_left;
                let mut p_r = self.current_page_index;
                while p_r < total && right_edge < max_screen_x {
                    let (bw, bh) = ensure_page_cached(self, p_r)?;
                    let px = right_edge.round() as i32;
                    let py = compute_hy(self, bh);
                    let rect = (px, py, bw, bh);
                    if p_r == self.current_page_index {
                        self.page_draw_rect = rect;
                    }
                    self.continuous_draw_rects.push((p_r, rect));
                    right_edge += (bw as f32) + gap;
                    p_r += 1;
                }
            } else if is_spread {
                // Continuous Vertical + Two-Page Spread rows: (0,1), (2,3), (4,5)...
                let step = 2u16;
                let row_start = (self.current_page_index / step) * step;
                self.current_page_index = row_start;

                let row_height = |app: &mut Self, rs: u16| -> Result<u32> {
                    let (_, h0) = ensure_page_cached(app, rs)?;
                    let h1 = if rs + 1 < total {
                        ensure_page_cached(app, rs + 1)?.1
                    } else {
                        0
                    };
                    Ok(h0.max(h1))
                };

                for _ in 0..((total + 1) / 2) {
                    let cur_h = row_height(self, self.current_page_index)? as f32;
                    let cur_top = min_screen_y + (PAGE_MARGIN_PX as f32) - self.scroll_y;
                    let cur_bottom = cur_top + cur_h;
                    if center_y > cur_bottom + gap * 0.5 && self.current_page_index + step < total {
                        self.current_page_index += step;
                        self.scroll_y -= cur_h + gap;
                        self.cached_char_boxes = None;
                    } else if center_y < cur_top - gap * 0.5 && self.current_page_index >= step {
                        let prev_rs = self.current_page_index - step;
                        let prev_h = row_height(self, prev_rs)? as f32;
                        self.current_page_index = prev_rs;
                        self.scroll_y += prev_h + gap;
                        self.cached_char_boxes = None;
                    } else {
                        break;
                    }
                }

                if self.current_page_index == 0 && self.scroll_y < 0.0 {
                    self.scroll_y = 0.0;
                }
                if self.current_page_index + step >= total {
                    let last_h = row_height(self, self.current_page_index)? as f32;
                    let max_s = (last_h - avail_h).max(0.0);
                    if self.scroll_y > max_s {
                        self.scroll_y = max_s;
                    }
                }

                let anchor_top = min_screen_y + (PAGE_MARGIN_PX as f32) - self.scroll_y;
                let push_spread_row = |app: &mut Self, rs: u16, row_top: i32, dst: &mut Vec<(u16, (i32, i32, u32, u32))>| -> Result<u32> {
                    let (w0, h0) = ensure_page_cached(app, rs)?;
                    if rs + 1 < total {
                        let (w1, h1) = ensure_page_cached(app, rs + 1)?;
                        let rh = h0.max(h1);
                        let combined_w = w0 + SPREAD_GAP_PX + w1;
                        let left_x = (sidebar_w as i32) + ((content_w as i32 - combined_w as i32) / 2);
                        let r0 = (left_x, row_top + ((rh - h0) / 2) as i32, w0, h0);
                        let r1 = (left_x + w0 as i32 + SPREAD_GAP_PX as i32, row_top + ((rh - h1) / 2) as i32, w1, h1);
                        dst.push((rs, r0));
                        dst.push((rs + 1, r1));
                        Ok(rh)
                    } else {
                        let left_x = (sidebar_w as i32) + ((content_w as i32 - w0 as i32) / 2);
                        dst.push((rs, (left_x, row_top, w0, h0)));
                        Ok(h0)
                    }
                };

                // Walk upward
                let mut up_bottom = anchor_top - gap;
                let mut rs_up = self.current_page_index;
                let mut upper_rows = Vec::new();
                while rs_up >= step && up_bottom > min_screen_y {
                    rs_up -= step;
                    let rh = row_height(self, rs_up)?;
                    let py = (up_bottom - rh as f32).round() as i32;
                    let mut row_items = Vec::with_capacity(2);
                    let _ = push_spread_row(self, rs_up, py, &mut row_items)?;
                    upper_rows.push(row_items);
                    up_bottom -= (rh as f32) + gap;
                }
                for row_items in upper_rows.into_iter().rev() {
                    self.continuous_draw_rects.extend(row_items);
                }

                // Walk downward
                let mut down_top = anchor_top;
                let mut rs_down = self.current_page_index;
                while rs_down < total && down_top < max_screen_y {
                    let py = down_top.round() as i32;
                    let start_len = self.continuous_draw_rects.len();
                    let mut tmp = Vec::with_capacity(2);
                    let rh = push_spread_row(self, rs_down, py, &mut tmp)?;
                    if rs_down == self.current_page_index && !tmp.is_empty() {
                        self.page_draw_rect = tmp[0].1;
                        self.second_page_draw_rect = tmp.get(1).map(|x| x.1);
                    }
                    let _ = start_len;
                    self.continuous_draw_rects.extend(tmp);
                    down_top += (rh as f32) + gap;
                    rs_down += step;
                }
            } else {
                // Continuous Vertical Single-Column (FitPage, FitWidth, FitTextWidth)
                for _ in 0..total {
                    let (_, cur_h_u32) = ensure_page_cached(self, self.current_page_index)?;
                    let cur_h = cur_h_u32 as f32;
                    let cur_top = min_screen_y + (PAGE_MARGIN_PX as f32) - self.scroll_y;
                    let cur_bottom = cur_top + cur_h;

                    if center_y > cur_bottom + gap * 0.5 && self.current_page_index + 1 < total {
                        self.current_page_index += 1;
                        self.scroll_y -= cur_h + gap;
                        self.cached_char_boxes = None;
                    } else if center_y < cur_top - gap * 0.5 && self.current_page_index > 0 {
                        let prev_idx = self.current_page_index - 1;
                        let (_, prev_h_u32) = ensure_page_cached(self, prev_idx)?;
                        self.current_page_index = prev_idx;
                        self.scroll_y += (prev_h_u32 as f32) + gap;
                        self.cached_char_boxes = None;
                    } else {
                        break;
                    }
                }

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

                let anchor_top = min_screen_y + (PAGE_MARGIN_PX as f32) - self.scroll_y;

                // Walk upward
                let mut up_bottom = anchor_top - gap;
                let mut p_up = self.current_page_index;
                let mut upper_pages = Vec::new();
                while p_up > 0 && up_bottom > min_screen_y {
                    p_up -= 1;
                    let (bw, bh) = ensure_page_cached(self, p_up)?;
                    let py = (up_bottom - bh as f32).round() as i32;
                    let px = compute_single_px(self, p_up, bw);
                    upper_pages.push((p_up, (px, py, bw, bh)));
                    up_bottom -= (bh as f32) + gap;
                }
                for item in upper_pages.into_iter().rev() {
                    self.continuous_draw_rects.push(item);
                }

                // Walk downward
                let mut down_top = anchor_top;
                let mut p_down = self.current_page_index;
                while p_down < total && down_top < max_screen_y {
                    let (bw, bh) = ensure_page_cached(self, p_down)?;
                    let py = down_top.round() as i32;
                    let px = compute_single_px(self, p_down, bw);
                    let rect = (px, py, bw, bh);
                    if p_down == self.current_page_index {
                        self.page_draw_rect = rect;
                    }
                    self.continuous_draw_rects.push((p_down, rect));
                    down_top += (bh as f32) + gap;
                    p_down += 1;
                }
            }

            let first_vis = self
                .continuous_draw_rects
                .iter()
                .map(|&(idx, _)| idx)
                .min()
                .unwrap_or(self.current_page_index);
            let last_vis = self
                .continuous_draw_rects
                .iter()
                .map(|&(idx, _)| idx)
                .max()
                .unwrap_or(self.current_page_index);

            // RAM rule: Cache only visible pages plus 1 neighbor on each side. Evict everything else immediately.
            let keep_start = first_vis.saturating_sub(1);
            let keep_end = (last_vis + 1).min(total - 1);

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

            let vis_rects = self.continuous_draw_rects.clone();
            for (p_idx, rect) in vis_rects {
                let is_tiled = self
                    .continuous_page_cache
                    .get(&p_idx)
                    .map_or(false, |c| c.pixels.is_empty());
                if is_tiled {
                    let rot = self.page_rotation_degrees(p_idx);
                    self.ensure_visible_tiles_for_page(
                        p_idx,
                        rect,
                        rot,
                        viewport_w,
                        viewport_h,
                        sidebar_w,
                        top_bars_h,
                        status_bar_h,
                    )?;
                }
            }

            return Ok(());
        }

        let is_magazine = self.effective_scroll_mode() == ScrollMode::MagazineBook;
        if self.view_mode == ViewMode::TwoPageSpread || is_magazine {
            let spread_gap = if is_magazine { 0 } else { SPREAD_GAP_PX };
            let avail_w = (content_w
                .saturating_sub(PAGE_MARGIN_PX * 2 + spread_gap)
                / 2)
            .max(1) as f32;
            let avail_h = content_h.saturating_sub(PAGE_MARGIN_PX * 2).max(1) as f32;

            let left_idx = if is_magazine {
                if self.current_page_index == 0 {
                    0
                } else if self.current_page_index % 2 == 0 {
                    self.current_page_index - 1
                } else {
                    self.current_page_index
                }
            } else {
                self.current_page_index
            };
            let left_rot = self.page_rotation_degrees(left_idx);
            let need_left = self.cached_page.as_ref().map_or(true, |c| {
                c.page_index != left_idx
                    || c.view_mode != ViewMode::TwoPageSpread
                    || c.zoom_bits != zoom_bits
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

            let right_idx_opt = if is_magazine && left_idx == 0 {
                None
            } else if left_idx + 1 < total {
                Some(left_idx + 1)
            } else {
                None
            };
            if let Some(right_idx) = right_idx_opt {
                let right_rot = self.page_rotation_degrees(right_idx);
                let need_right = self.cached_second_page.as_ref().map_or(true, |c| {
                    c.page_index != right_idx
                        || c.view_mode != ViewMode::TwoPageSpread
                        || c.zoom_bits != zoom_bits
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
                    center_x - (spread_gap as i32 / 2) - (lw as i32) - (self.pan_x.round() as i32)
                } else {
                    center_x - (lw as i32 / 2) - (self.pan_x.round() as i32)
                };
                let ly = if lh > content_h {
                    let max_s = (lh - content_h) as f32;
                    self.scroll_y = self.scroll_y.clamp(0.0, max_s);
                    (top_bars_h as i32) - (self.scroll_y.round() as i32)
                } else {
                    self.scroll_y = 0.0;
                    (top_bars_h as i32) + ((content_h as i32 - lh as i32) / 2)
                };
                self.page_draw_rect = (lx, ly, lw, lh);
            }
            if let Some(right_bmp) = &self.cached_second_page {
                let rw = right_bmp.bitmap_width;
                let rh = right_bmp.bitmap_height;
                let rx = center_x + (spread_gap as i32 / 2) - (self.pan_x.round() as i32);
                let ry = if rh > content_h {
                    (top_bars_h as i32) - (self.scroll_y.round() as i32)
                } else {
                    (top_bars_h as i32) + ((content_h as i32 - rh as i32) / 2)
                };
                self.second_page_draw_rect = Some((rx, ry, rw, rh));
            } else {
                self.second_page_draw_rect = None;
            }

            if self
                .cached_page
                .as_ref()
                .map_or(false, |c| c.pixels.is_empty())
            {
                self.ensure_visible_tiles_for_page(
                    left_idx,
                    self.page_draw_rect,
                    left_rot,
                    viewport_w,
                    viewport_h,
                    sidebar_w,
                    top_bars_h,
                    status_bar_h,
                )?;
            }
            if let (Some(right_idx), Some(right_rect)) = (right_idx_opt, self.second_page_draw_rect)
            {
                if self
                    .cached_second_page
                    .as_ref()
                    .map_or(false, |c| c.pixels.is_empty())
                {
                    let right_rot = self.page_rotation_degrees(right_idx);
                    self.ensure_visible_tiles_for_page(
                        right_idx,
                        right_rect,
                        right_rot,
                        viewport_w,
                        viewport_h,
                        sidebar_w,
                        top_bars_h,
                        status_bar_h,
                    )?;
                }
            }

            return Ok(());
        }

        // Single-page view modes: FitPage, FitWidth, FitTextWidth
        self.cached_second_page = None;
        self.second_page_draw_rect = None;

        let need_render = self.cached_page.as_ref().map_or(true, |cache| {
            cache.page_index != self.current_page_index
                || cache.view_mode != self.view_mode
                || cache.zoom_bits != zoom_bits
                || cache.viewport_width != content_w
                || cache.viewport_height != content_h
        });

        if need_render {
            let reused = self
                .cached_page
                .take()
                .map(|c| c.pixels)
                .unwrap_or_else(|| std::mem::take(&mut self.spare_pixel_buf));
            let (avail_w, avail_h) = if self.is_presentation {
                (content_w.max(1) as f32, content_h.max(1) as f32)
            } else {
                let aw = match self.view_mode {
                    ViewMode::FitWidth | ViewMode::FitTextWidth => {
                        content_w.saturating_sub(24).max(1) as f32
                    }
                    _ => content_w.saturating_sub(PAGE_MARGIN_PX * 2).max(1) as f32,
                };
                let ah = content_h.saturating_sub(PAGE_MARGIN_PX * 2).max(1) as f32;
                (aw, ah)
            };
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
            let c_min_y = cache.content_min_y;
            let c_max_y = cache.content_max_y;
            let rotation_deg = cache.rotation_deg;
            let is_tiled = cache.pixels.is_empty();
            let (page_w_pts, page_h_pts) = self
                .doc
                .as_ref()
                .and_then(|d| d.pages().page_size(self.current_page_index).ok())
                .map(|r| (r.width().value.max(1.0), r.height().value.max(1.0)))
                .unwrap_or((595.0, 842.0));
            self.page_draw_rect = self.compute_single_page_draw_rect(
                sidebar_w,
                top_bars_h,
                content_w,
                content_h,
                bmp_w,
                bmp_h,
                c_min_x,
                c_max_x,
                c_min_y,
                c_max_y,
                page_w_pts,
                page_h_pts,
            );
            if is_tiled {
                self.ensure_visible_tiles_for_page(
                    self.current_page_index,
                    self.page_draw_rect,
                    rotation_deg,
                    viewport_w,
                    viewport_h,
                    sidebar_w,
                    top_bars_h,
                    status_bar_h,
                )?;
            }
        }

        Ok(())
    }

    /// Blits either a full `CachedPageBitmap` or its visible 512x512 tiles from `self.tile_cache`
    /// when `cache.pixels` is empty (zoomed above 2x screen size).
    pub(super) fn blit_page_or_tiles(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        status_bar_h: u32,
        draw_rect: (i32, i32, u32, u32),
        cache: &CachedPageBitmap,
    ) {
        if !cache.pixels.is_empty() {
            Self::blit_cached_page(
                buffer,
                buf_w,
                buf_h,
                sidebar_w,
                top_bars_h,
                status_bar_h,
                draw_rect,
                &cache.pixels,
                self.reading_filter,
            );
            return;
        }

        let (draw_x, draw_y, virtual_w, virtual_h) = draw_rect;
        let min_screen_x = sidebar_w as i32;
        let max_screen_x = buf_w as i32;
        let min_screen_y = top_bars_h as i32;
        let max_screen_y = (buf_h.saturating_sub(status_bar_h)) as i32;

        let vis_x0 = (min_screen_x - draw_x).max(0) as u32;
        let vis_x1 = ((max_screen_x - draw_x).max(0) as u32).min(virtual_w);
        let vis_y0 = (min_screen_y - draw_y).max(0) as u32;
        let vis_y1 = ((max_screen_y - draw_y).max(0) as u32).min(virtual_h);

        if vis_x0 >= vis_x1 || vis_y0 >= vis_y1 {
            return;
        }

        let col_start = (vis_x0 / TILE_SIZE_PX) as u16;
        let col_end = ((vis_x1 + TILE_SIZE_PX - 1) / TILE_SIZE_PX) as u16;
        let row_start = (vis_y0 / TILE_SIZE_PX) as u16;
        let row_end = ((vis_y1 + TILE_SIZE_PX - 1) / TILE_SIZE_PX) as u16;

        for tile_row in row_start..row_end {
            for tile_col in col_start..col_end {
                let key = TileKey {
                    page_index: cache.page_index,
                    virtual_w,
                    virtual_h,
                    rotation_deg: cache.rotation_deg,
                    tile_col,
                    tile_row,
                };
                if let Some(entry) = self.tile_cache.get(&key) {
                    let tile_draw_x = draw_x + (tile_col as i32) * (TILE_SIZE_PX as i32);
                    let tile_draw_y = draw_y + (tile_row as i32) * (TILE_SIZE_PX as i32);
                    Self::blit_cached_page(
                        buffer,
                        buf_w,
                        buf_h,
                        sidebar_w,
                        top_bars_h,
                        status_bar_h,
                        (tile_draw_x, tile_draw_y, entry.width, entry.height),
                        &entry.pixels,
                        self.reading_filter,
                    );
                }
            }
        }
    }

    /// Transforms a single `0x00RRGGBB` pixel through the selected eye-care `ReadingFilter`.
    #[inline]
    pub(super) fn apply_reading_filter_pixel(px: u32, filter: ReadingFilter) -> u32 {
        match filter {
            ReadingFilter::Normal => px,
            ReadingFilter::PaperEInk => {
                let r = (px >> 16) & 0xFF;
                let g = (px >> 8) & 0xFF;
                let b = px & 0xFF;
                let lum = (77 * r + 150 * g + 29 * b) >> 8;
                let dr = (lum * 4 + r) / 5;
                let dg = (lum * 4 + g) / 5;
                let db = (lum * 4 + b) / 5;
                let nr = 30 + ((dr * 204) >> 8);
                let ng = 31 + ((dg * 204) >> 8);
                let nb = 29 + ((db * 201) >> 8);
                (nr << 16) | (ng << 8) | nb
            }
            ReadingFilter::WarmSepia => {
                let r = (px >> 16) & 0xFF;
                let g = (px >> 8) & 0xFF;
                let b = px & 0xFF;
                let nr = 43 + ((r * 204) >> 8);
                let ng = 35 + ((g * 205) >> 8);
                let nb = 24 + ((b * 199) >> 8);
                (nr << 16) | (ng << 8) | nb
            }
            ReadingFilter::SageEyeCare => {
                let r = (px >> 16) & 0xFF;
                let g = (px >> 8) & 0xFF;
                let b = px & 0xFF;
                let nr = 29 + ((r * 204) >> 8);
                let ng = 39 + ((g * 203) >> 8);
                let nb = 31 + ((b * 203) >> 8);
                (nr << 16) | (ng << 8) | nb
            }
            ReadingFilter::DarkInvert => {
                let ir = 255 - ((px >> 16) & 0xFF);
                let ig = 255 - ((px >> 8) & 0xFF);
                let ib = 255 - (px & 0xFF);
                let nr = 30 + ((ir * 186) >> 8);
                let ng = 32 + ((ig * 188) >> 8);
                let nb = 36 + ((ib * 190) >> 8);
                (nr << 16) | (ng << 8) | nb
            }
            ReadingFilter::NightAmber => {
                let ir = 255 - ((px >> 16) & 0xFF);
                let ig = 255 - ((px >> 8) & 0xFF);
                let ib = 255 - (px & 0xFF);
                let nr = 26 + ((ir * 203) >> 8);
                let ng = 23 + ((ig * 184) >> 8);
                let nb = 19 + ((ib * 149) >> 8);
                (nr << 16) | (ng << 8) | nb
            }
        }
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

            if filter == ReadingFilter::Normal {
                buffer[dst_idx..dst_idx + copy_len]
                    .copy_from_slice(&pixels[src_idx..src_idx + copy_len]);
            } else {
                for i in 0..copy_len {
                    buffer[dst_idx + i] = Self::apply_reading_filter_pixel(pixels[src_idx + i], filter);
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

    /// Draws translucent blue text selection overlays for `self.selected_text_rects` on `page_idx`.
    pub(super) fn draw_text_selection_overlays_on_page(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        top_bars_h: u32,
        status_bar_h: u32,
        page_idx: u16,
        draw_rect: (i32, i32, u32, u32),
    ) {
        if self.selected_text_page != Some(page_idx) || self.selected_text_rects.is_empty() {
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

        for r in &self.selected_text_rects {
            let (bx1, by1, bx2, by2) = match (
                page.points_to_pixels(r.left(), r.top(), &config),
                page.points_to_pixels(r.right(), r.bottom(), &config),
            ) {
                (Ok((x1, y1)), Ok((x2, y2))) => (x1, y1, x2, y2),
                _ => {
                    let x1 = ((r.left().value / page_w) * (bmp_w as f32)).round() as i32;
                    let y1 = ((1.0 - (r.top().value / page_h)) * (bmp_h as f32)).round() as i32;
                    let x2 = ((r.right().value / page_w) * (bmp_w as f32)).round() as i32;
                    let y2 = ((1.0 - (r.bottom().value / page_h)) * (bmp_h as f32)).round() as i32;
                    (x1, y1, x2, y2)
                }
            };

            let sx1 = (draw_x + bx1.min(bx2) - 1).clamp(0, buf_w as i32) as u32;
            let sx2 = (draw_x + bx1.max(bx2) + 1).clamp(0, buf_w as i32) as u32;
            let sy1 = (draw_y + by1.min(by2) - 1).clamp(min_y_bound, max_y_bound) as u32;
            let sy2 = (draw_y + by1.max(by2) + 1).clamp(min_y_bound, max_y_bound) as u32;

            if sx2 > sx1 && sy2 > sy1 {
                blend_rect_rgba(
                    buffer,
                    buf_w,
                    buf_h,
                    sx1,
                    sy1,
                    sx2 - sx1,
                    sy2 - sy1,
                    (33, 150, 243, 95),
                );
                draw_rect_border(
                    buffer,
                    buf_w,
                    buf_h,
                    sx1,
                    sy1,
                    sx2 - sx1,
                    sy2 - sy1,
                    0x001E88E5,
                );
            }
        }
    }

    /// Computes `(track_x, track_y, track_w, track_h, thumb_y, thumb_h)` for the thin right-edge
    /// overlay vertical scrollbar, or `None` when the current view has nothing to scroll.
    pub(super) fn vertical_scrollbar_geometry(
        &self,
        buf_w: u32,
        buf_h: u32,
        top_bars_h: u32,
        status_bar_h: u32,
        scale_factor: f64,
    ) -> Option<(i32, i32, u32, u32, i32, u32)> {
        if !self.has_any_document() || self.is_presentation {
            return None;
        }
        let s = scale_factor as f32;
        let track_w = (7.0 * s).round().clamp(6.0, 12.0) as u32;
        let pad_r = (4.0 * s).round() as i32;
        let pad_v = (6.0 * s).round() as i32;
        let content_h = buf_h.saturating_sub(top_bars_h + status_bar_h);
        if content_h < 40 {
            return None;
        }
        if self.is_comic_tab() {
            let canvas_w = buf_w.saturating_sub(self.sidebar_width_px(scale_factor));
            let max_scroll = self.comic_max_scroll_y(canvas_w, content_h, scale_factor);
            if max_scroll <= 1.0 {
                return None;
            }
            let track_x = buf_w as i32 - track_w as i32 - pad_r;
            let track_y = top_bars_h as i32 + pad_v;
            let track_h = content_h.saturating_sub((pad_v * 2) as u32);
            if track_h < 24 {
                return None;
            }
            let total_h = content_h as f32 + max_scroll;
            let visible_ratio = (content_h as f32 / total_h).clamp(0.08, 0.92);
            let fraction = (self.scroll_y / max_scroll).clamp(0.0, 1.0);
            let thumb_h = ((track_h as f32 * visible_ratio).round() as u32).clamp(24, track_h);
            let thumb_y = track_y + ((track_h.saturating_sub(thumb_h)) as f32 * fraction).round() as i32;
            return Some((track_x, track_y, track_w, track_h, thumb_y, thumb_h));
        }
        if self.is_markdown_tab() {
            if self.md_editor_mode == super::text_editor::MdEditorMode::Edit {
                return None;
            }
            let max_scroll = self.md_max_scroll_y(content_h, scale_factor);
            if max_scroll <= 1.0 {
                return None;
            }
            let hdr_h = self.md_header_bar_height_px(scale_factor) as i32;
            let track_x = buf_w as i32 - track_w as i32 - pad_r;
            let track_y = top_bars_h as i32 + hdr_h + pad_v;
            let track_h = content_h.saturating_sub((hdr_h + pad_v * 2) as u32);
            if track_h < 24 {
                return None;
            }
            let total_h = (content_h as f32 - hdr_h as f32) + max_scroll;
            let visible_ratio = ((content_h as f32 - hdr_h as f32) / total_h).clamp(0.08, 0.92);
            let fraction = (self.scroll_y / max_scroll).clamp(0.0, 1.0);
            let thumb_h = ((track_h as f32 * visible_ratio).round() as u32).clamp(24, track_h);
            let thumb_y = track_y + ((track_h.saturating_sub(thumb_h)) as f32 * fraction).round() as i32;
            return Some((track_x, track_y, track_w, track_h, thumb_y, thumb_h));
        }
        let total = self.total_pages();
        if total == 0 {
            return None;
        }
        let track_x = buf_w as i32 - track_w as i32 - pad_r;
        let track_y = top_bars_h as i32 + pad_v;
        let track_h = content_h.saturating_sub((pad_v * 2) as u32);
        if track_h < 24 {
            return None;
        }

        let (fraction, visible_ratio) = if let Some(cmp) = self.compare_mode.as_ref() {
            if cmp.max_scroll_y > 1.0 {
                let total_h = content_h as f32 + cmp.max_scroll_y;
                (
                    (self.scroll_y / cmp.max_scroll_y).clamp(0.0, 1.0),
                    (content_h as f32 / total_h).clamp(0.10, 0.92),
                )
            } else if total > 1 {
                let denom = (total - 1) as f32;
                ((self.current_page_index as f32 / denom).clamp(0.0, 1.0), (1.0 / total as f32).clamp(0.08, 0.5))
            } else {
                return None;
            }
        } else if self.view_mode == ViewMode::ThumbnailGrid {
            let sidebar_w = self.sidebar_width_px(scale_factor).min(buf_w.saturating_sub(64));
            let (cols, _cw, cell_h, _sx, _sy) = self.grid_metrics(buf_w, sidebar_w, top_bars_h);
            let rows = (total as u32).div_ceil(cols.max(1));
            let total_h = (rows * cell_h + 40) as f32;
            if total_h <= content_h as f32 {
                return None;
            }
            let max_scroll = (total_h - content_h as f32).max(1.0);
            ((self.grid_scroll_y / max_scroll).clamp(0.0, 1.0), (content_h as f32 / total_h).clamp(0.08, 1.0))
        } else if self.view_mode == ViewMode::TextView {
            if self.text_view_max_scroll <= 1.0 {
                return None;
            }
            let total_h = content_h as f32 + self.text_view_max_scroll;
            (
                (self.scroll_y / self.text_view_max_scroll).clamp(0.0, 1.0),
                (content_h as f32 / total_h).clamp(0.08, 0.9),
            )
        } else if total > 1 {
            let denom = (total - 1) as f32;
            ((self.current_page_index as f32 / denom).clamp(0.0, 1.0), (1.0 / total as f32).clamp(0.08, 0.5))
        } else {
            let bmp_h = self.page_draw_rect.3;
            if bmp_h <= content_h {
                return None;
            }
            let max_s = (bmp_h - content_h) as f32;
            ((self.scroll_y / max_s).clamp(0.0, 1.0), (content_h as f32 / bmp_h as f32).clamp(0.10, 0.9))
        };

        let min_thumb = (28.0 * s).round().min(track_h as f32);
        let thumb_h = ((track_h as f32) * visible_ratio)
            .round()
            .clamp(min_thumb, track_h as f32) as u32;
        let travel = track_h.saturating_sub(thumb_h);
        let thumb_y = track_y + ((travel as f32) * fraction).round() as i32;
        Some((track_x, track_y, track_w, track_h, thumb_y, thumb_h))
    }

    pub(super) fn scroll_to_scrollbar_y(
        &mut self,
        mouse_y: f64,
        buf_w: u32,
        buf_h: u32,
        top_bars_h: u32,
        status_bar_h: u32,
        scale_factor: f64,
    ) {
        let Some((_tx, ty, _tw, th, _tby, tbh)) =
            self.vertical_scrollbar_geometry(buf_w, buf_h, top_bars_h, status_bar_h, scale_factor)
        else {
            return;
        };
        let travel = th.saturating_sub(tbh).max(1) as f32;
        let frac = (((mouse_y as f32) - (ty as f32) - (tbh as f32) * 0.5) / travel).clamp(0.0, 1.0);
        let total = self.total_pages();
        let content_h = buf_h.saturating_sub(top_bars_h + status_bar_h);

        if self.is_markdown_tab() {
            let max_s = self.md_max_scroll_y(content_h, scale_factor);
            self.scroll_y = (frac * max_s).clamp(0.0, max_s);
            self.request_redraw();
            return;
        }

        if let Some(cmp_max_y) = self.compare_mode.as_ref().map(|c| c.max_scroll_y) {
            if cmp_max_y > 1.0 {
                self.scroll_y = frac * cmp_max_y;
            } else if total > 1 {
                let target = (frac * ((total - 1) as f32)).round() as u16;
                if target != self.current_page_index {
                    self.current_page_index = target.min(total - 1);
                    self.scroll_y = 0.0;
                    self.invalidate_page_caches();
                }
            }
        } else if self.view_mode == ViewMode::ThumbnailGrid {
            let sidebar_w = self.sidebar_width_px(scale_factor).min(buf_w.saturating_sub(64));
            let (cols, _cw, cell_h, _sx, _sy) = self.grid_metrics(buf_w, sidebar_w, top_bars_h);
            let rows = (total as u32).div_ceil(cols.max(1));
            let total_h = (rows * cell_h + 40) as f32;
            let max_scroll = (total_h - content_h as f32).max(0.0);
            self.grid_scroll_y = frac * max_scroll;
        } else if self.view_mode == ViewMode::TextView {
            self.scroll_y = frac * self.text_view_max_scroll.max(0.0);
        } else if total > 1 {
            let target = (frac * ((total - 1) as f32)).round() as u16;
            if target != self.current_page_index {
                self.current_page_index = target.min(total - 1);
                self.scroll_y = 0.0;
                self.invalidate_page_caches();
            }
        } else {
            let bmp_h = self.page_draw_rect.3;
            let max_s = (bmp_h.saturating_sub(content_h)) as f32;
            self.scroll_y = frac * max_s;
        }
        self.update_title_bar();
        self.request_redraw();
    }

    /// Computes `(max_pan_x, visible_ratio)` for horizontal panning in both Normal Window and Compare Window.
    pub(super) fn horizontal_pan_metrics(&self, buf_w: u32, scale_factor: f64) -> Option<(f32, f32)> {
        if self.doc.is_none() || self.is_presentation {
            return None;
        }
        let sidebar_w = self.sidebar_width_px(scale_factor).min(buf_w.saturating_sub(64));
        let content_w = buf_w.saturating_sub(sidebar_w).max(1);
        if let Some(cmp) = self.compare_mode.as_ref() {
            if cmp.max_pan_x <= 1.0 {
                return None;
            }
            let half_w = (content_w / 2).max(1) as f32;
            let total_w = half_w + cmp.max_pan_x;
            return Some((cmp.max_pan_x, (half_w / total_w).clamp(0.10, 0.92)));
        }
        if !self.view_mode.is_page_view() || self.effective_scroll_mode() == ScrollMode::ContinuousHorizontal {
            return None;
        }
        let bmp_w = self.page_draw_rect.2;
        if bmp_w <= content_w {
            return None;
        }
        let max_pan = (bmp_w - content_w) as f32;
        if max_pan <= 1.0 {
            return None;
        }
        Some((max_pan, (content_w as f32 / bmp_w as f32).clamp(0.10, 0.92)))
    }

    /// Computes `(track_x, track_y, track_w, track_h, thumb_x, thumb_w)` for the thin bottom-edge
    /// overlay horizontal scrollbar in both Normal Window and Compare Window, or `None` when not needed.
    pub(super) fn horizontal_scrollbar_geometry(
        &self,
        buf_w: u32,
        buf_h: u32,
        status_bar_h: u32,
        scale_factor: f64,
    ) -> Option<(i32, i32, u32, u32, i32, u32)> {
        let (max_pan, visible_ratio) = self.horizontal_pan_metrics(buf_w, scale_factor)?;
        let s = scale_factor as f32;
        let sidebar_w = self.sidebar_width_px(scale_factor).min(buf_w.saturating_sub(64));
        let track_h = (8.0 * s).round().clamp(7.0, 12.0) as u32;
        let pad_h = (10.0 * s).round() as i32;
        let pad_b = (4.0 * s).round() as i32;
        let vscroll_clear = (18.0 * s).round() as u32;
        let content_w = buf_w.saturating_sub(sidebar_w);
        if content_w < 80 {
            return None;
        }
        let track_x = sidebar_w as i32 + pad_h;
        let track_w = content_w.saturating_sub((pad_h as u32) + vscroll_clear);
        if track_w < 40 {
            return None;
        }
        let track_y = buf_h.saturating_sub(status_bar_h) as i32 - track_h as i32 - pad_b;
        let fraction = (self.pan_x / max_pan).clamp(0.0, 1.0);
        let min_thumb = (36.0 * s).round().min(track_w as f32);
        let thumb_w = ((track_w as f32) * visible_ratio)
            .round()
            .clamp(min_thumb, track_w as f32) as u32;
        let travel = track_w.saturating_sub(thumb_w);
        let thumb_x = track_x + ((travel as f32) * fraction).round() as i32;
        Some((track_x, track_y, track_w, track_h, thumb_x, thumb_w))
    }

    pub(super) fn scroll_to_hscrollbar_x(
        &mut self,
        mouse_x: f64,
        buf_w: u32,
        buf_h: u32,
        status_bar_h: u32,
        scale_factor: f64,
    ) {
        let Some((tx, _ty, tw, _th, _tbx, tbw)) =
            self.horizontal_scrollbar_geometry(buf_w, buf_h, status_bar_h, scale_factor)
        else {
            return;
        };
        let Some((max_pan, _)) = self.horizontal_pan_metrics(buf_w, scale_factor) else {
            return;
        };
        let travel = tw.saturating_sub(tbw).max(1) as f32;
        let frac = (((mouse_x as f32) - (tx as f32) - (tbw as f32) * 0.5) / travel).clamp(0.0, 1.0);
        self.pan_x = (frac * max_pan).clamp(0.0, max_pan);
        self.request_redraw();
    }

    pub(super) fn draw_page_frame_chrome(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        status_bar_h: u32,
        draw_rect: (i32, i32, u32, u32),
        sf: f64,
    ) {
        if self.is_presentation {
            return;
        }
        let theme = self.ui_theme();
        let (dx, dy, bw, bh) = draw_rect;
        if bw == 0 || bh == 0 {
            return;
        }
        let bottom_limit = buf_h.saturating_sub(status_bar_h);
        let spread = (7.0 * sf as f32).round() as i32;
        let off_y = (3.0 * sf as f32).round() as i32;
        if dx >= sidebar_w as i32 && dy >= top_bars_h as i32 {
            super::ui_draw::draw_soft_shadow(
                buffer,
                buf_w,
                bottom_limit,
                dx,
                dy,
                bw,
                bh,
                2.0,
                spread,
                off_y,
                theme.shadow_rgb,
                theme.shadow_max_alpha,
            );
        }
        // Subtle 1px hairline paper border clipped to viewport
        let x0 = dx.max(sidebar_w as i32);
        let x1 = (dx + bw as i32).min(buf_w as i32);
        let y0 = dy.max(top_bars_h as i32);
        let y1 = (dy + bh as i32).min(bottom_limit as i32);
        if x1 > x0 && y1 > y0 {
            if dy >= top_bars_h as i32 && dy < bottom_limit as i32 {
                fill_rect_clipped(buffer, buf_w, bottom_limit, x0, dy, (x1 - x0) as u32, 1, theme.page_border);
            }
            let by = dy + bh as i32 - 1;
            if by >= top_bars_h as i32 && by < bottom_limit as i32 {
                fill_rect_clipped(buffer, buf_w, bottom_limit, x0, by, (x1 - x0) as u32, 1, theme.page_border);
            }
            if dx >= sidebar_w as i32 && dx < buf_w as i32 {
                fill_rect_clipped(buffer, buf_w, bottom_limit, dx, y0, 1, (y1 - y0) as u32, theme.page_border);
            }
            let rx = dx + bw as i32 - 1;
            if rx >= sidebar_w as i32 && rx < buf_w as i32 {
                fill_rect_clipped(buffer, buf_w, bottom_limit, rx, y0, 1, (y1 - y0) as u32, theme.page_border);
            }
        }
    }

    /// Renders the complete UI and active PDF content into `buffer` (`width * height` `0x00RRGGBB`).
    /// Shared by `render_to_window` and headless verification tests.
    pub(super) fn render_frame_into_buffer(
        &mut self,
        buffer: &mut [u32],
        width: u32,
        height: u32,
        scale_factor: f64,
    ) {
        self.record_status_notice_if_new();
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

        if self.compare_mode.is_none() && !self.is_markdown_tab() && !self.is_comic_tab() {
            if self.view_mode.is_page_view() {
                self.render_page_view_or_notice(width, height, sidebar_w, top_bars_h, status_bar_h);
                self.ensure_deskew_preview_pixels();
            } else if self.view_mode == ViewMode::ThumbnailGrid {
                self.update_visible_thumbnails(width, height, sidebar_w, top_bars_h, status_bar_h);
            }
        }

        if self.print_preview.is_some() {
            self.ensure_print_preview_sheet_rendered(width, height, scale_factor);
        }
        if self.combine_modal.is_some() {
            self.ensure_combine_preview_rendered(width, height, scale_factor);
        }
        if self.watermark_modal.is_some() {
            self.ensure_watermark_preview_rendered(width, height, scale_factor);
        }

        let theme = self.ui_theme();
        buffer.fill(if self.is_presentation {
            0x0000_0000
        } else {
            theme.canvas_bg
        });

        if self.is_markdown_tab() {
            self.draw_markdown_view(
                buffer, width, height, sidebar_w, top_bars_h, status_bar_h, scale_factor,
            );
        } else if self.is_comic_tab() {
            self.draw_comic_view(
                buffer, width, height, sidebar_w, top_bars_h, status_bar_h, scale_factor,
            );
        } else if self.doc.is_none() {
            self.draw_welcome_screen(buffer, width, height, top_bars_h, status_bar_h, scale_factor);
        } else if self.compare_mode.is_some() {
            self.render_and_draw_compare_mode(
                buffer, width, height, sidebar_w, top_bars_h, status_bar_h, scale_factor,
            );
        } else if self.view_mode == ViewMode::TextView {
            self.draw_text_view(
                buffer, width, height, sidebar_w, top_bars_h, status_bar_h, scale_factor,
            );
        } else if self.view_mode.is_page_view() {
            if self.is_continuous_scroll() {
                for &(page_idx, draw_rect) in &self.continuous_draw_rects {
                    if let Some(cache) = self.continuous_page_cache.get(&page_idx) {
                        self.draw_page_frame_chrome(
                            buffer, width, height, sidebar_w, top_bars_h, status_bar_h, draw_rect, scale_factor,
                        );
                        self.blit_page_or_tiles(
                            buffer, width, height, sidebar_w, top_bars_h, status_bar_h, draw_rect, cache,
                        );
                        self.draw_search_overlays_on_page(
                            buffer, width, height, top_bars_h, status_bar_h, page_idx, draw_rect,
                        );
                        self.draw_text_selection_overlays_on_page(
                            buffer, width, height, top_bars_h, status_bar_h, page_idx, draw_rect,
                        );
                    }
                }
            } else {
                if let Some(cache) = &self.cached_page {
                    self.draw_page_frame_chrome(
                        buffer, width, height, sidebar_w, top_bars_h, status_bar_h, self.page_draw_rect, scale_factor,
                    );
                    let deskew_preview = self
                        .deskew_bar
                        .as_ref()
                        .and_then(|d| d.preview_cache.as_ref())
                        .filter(|(p, _, w, h, _)| {
                            *p == cache.page_index && *w == cache.bitmap_width && *h == cache.bitmap_height
                        })
                        .map(|(_, _, _, _, px)| px.as_slice());
                    if let Some(rotated_pixels) = deskew_preview {
                        Self::blit_cached_page(
                            buffer,
                            width,
                            height,
                            sidebar_w,
                            top_bars_h,
                            status_bar_h,
                            self.page_draw_rect,
                            rotated_pixels,
                            self.reading_filter,
                        );
                    } else {
                        self.blit_page_or_tiles(
                            buffer, width, height, sidebar_w, top_bars_h, status_bar_h, self.page_draw_rect, cache,
                        );
                    }
                    self.draw_search_overlays_on_page(
                        buffer, width, height, top_bars_h, status_bar_h, cache.page_index, self.page_draw_rect,
                    );
                    self.draw_text_selection_overlays_on_page(
                        buffer, width, height, top_bars_h, status_bar_h, cache.page_index, self.page_draw_rect,
                    );
                }

                let is_magazine = self.effective_scroll_mode() == ScrollMode::MagazineBook;
                if self.view_mode == ViewMode::TwoPageSpread || is_magazine {
                    if let (Some(second_cache), Some(second_rect)) =
                        (&self.cached_second_page, self.second_page_draw_rect)
                    {
                        self.draw_page_frame_chrome(
                            buffer, width, height, sidebar_w, top_bars_h, status_bar_h, second_rect, scale_factor,
                        );
                        self.blit_page_or_tiles(
                            buffer, width, height, sidebar_w, top_bars_h, status_bar_h, second_rect, second_cache,
                        );
                        self.draw_search_overlays_on_page(
                            buffer, width, height, top_bars_h, status_bar_h, second_cache.page_index, second_rect,
                        );
                        self.draw_text_selection_overlays_on_page(
                            buffer, width, height, top_bars_h, status_bar_h, second_cache.page_index, second_rect,
                        );
                    }
                }

                if is_magazine {
                    let s = scale_factor as f32;
                    let bottom_limit = height.saturating_sub(status_bar_h);
                    let r1 = self.page_draw_rect;
                    let (spread_x, spread_y, spread_w, spread_h) = if let Some(r2) = self.second_page_draw_rect {
                        let spine_x = r2.0;
                        let top_y = r1.1.min(r2.1).max(top_bars_h as i32);
                        let bot_y = (r1.1 + r1.3 as i32).max(r2.1 + r2.3 as i32).min(bottom_limit as i32);
                        let sh = (bot_y - top_y).max(0) as u32;
                        super::ui_draw::draw_book_spine_crease(
                            buffer,
                            width,
                            bottom_limit,
                            spine_x,
                            top_y,
                            sh,
                            (14.0 * s).round().max(6.0) as i32,
                        );
                        (r1.0, top_y, (r2.0 + r2.2 as i32 - r1.0).max(0) as u32, sh)
                    } else {
                        let top_y = r1.1.max(top_bars_h as i32);
                        let bot_y = (r1.1 + r1.3 as i32).min(bottom_limit as i32);
                        (r1.0, top_y, r1.2, (bot_y - top_y).max(0) as u32)
                    };
                    let canvas_rect = (
                        sidebar_w as i32,
                        top_bars_h as i32,
                        width.saturating_sub(sidebar_w),
                        bottom_limit.saturating_sub(top_bars_h),
                    );
                    let cur_rect = (
                        spread_x.max(canvas_rect.0),
                        spread_y.max(canvas_rect.1),
                        spread_w.min(canvas_rect.2),
                        spread_h.min(canvas_rect.3),
                    );
                    let canvas_bg = match self.reading_filter {
                        ReadingFilter::DarkInvert => 0x0014_161A,
                        ReadingFilter::NightAmber => 0x0014_0F0B,
                        _ => theme.canvas_bg,
                    };
                    let paper_bg = self.curl_paper_bg_color(&theme);
                    self.render_active_spread_curl(
                        buffer,
                        width,
                        bottom_limit,
                        cur_rect,
                        canvas_rect,
                        canvas_bg,
                        paper_bg,
                        s,
                    );
                }
            }

            // Subtle interactive highlight frame on hovered fillable PDF form field
            if self.mouse_mode == MouseMode::Select {
                if let Some((fp, frect)) = self.hovered_form_field {
                    if let Some((fx, fy, fw, fh)) = self.pdf_rect_to_screen_rect(fp, frect) {
                        let bottom_limit = height.saturating_sub(status_bar_h);
                        super::ui_draw::fill_rounded_rect_alpha(
                            buffer, width, bottom_limit, fx, fy, fw, fh, 3.0, theme.accent, 34,
                        );
                        super::ui_draw::stroke_rounded_rect(
                            buffer, width, bottom_limit, fx, fy, fw, fh, 3.0, theme.accent,
                        );
                    }
                }
            }

            // Active drag preview: Move Annotation / Ink / Arrow / Select / Shapes / Redact / Crop / Stamp
            if let Some(ref drag) = self.dragging_annotation {
                let ddx = (self.cursor_pos.0 - drag.start_screen_x).round() as i32;
                let ddy = (self.cursor_pos.1 - drag.start_screen_y).round() as i32;
                if ddx.abs() >= 3 || ddy.abs() >= 3 {
                    if let Some((orig_x, orig_y, orig_w, orig_h)) =
                        self.pdf_rect_to_screen_rect(drag.page_index, drag.orig_rect)
                    {
                        let gx = orig_x + ddx;
                        let gy = orig_y + ddy;
                        let gw = orig_w.max(20);
                        let gh = orig_h.max(18);
                        super::ui_draw::fill_rounded_rect_alpha(
                            buffer, width, height, gx, gy, gw, gh, 4.0, theme.accent, 42,
                        );
                        super::ui_draw::draw_dashed_rect(
                            buffer, width, height, gx, gy, gw, gh, 5, 3, theme.accent,
                        );
                    }
                }
            } else if self.mouse_mode == MouseMode::InkPen && self.ink_stroke_screen_pts.len() >= 2 {
                let col = self.annotation_color.u32_rgb();
                let thick = (2.2 * scale_factor as f32).clamp(1.6, 5.0);
                for win in self.ink_stroke_screen_pts.windows(2) {
                    super::ui_draw::draw_line_aa(
                        buffer, width, height, win[0].0, win[0].1, win[1].0, win[1].1, thick, col,
                    );
                }
            } else if self.mouse_mode != MouseMode::Measure {
                if let Some((sx1, sy1)) = self.drag_start {
                    let (sx2, sy2) = self.cursor_pos;
                    if self.mouse_mode == MouseMode::ArrowShape {
                        let col = self.annotation_color.u32_rgb();
                        let thick = (2.2 * scale_factor as f32).clamp(1.6, 5.0);
                        let dx = (sx2 - sx1) as f32;
                        let dy = (sy2 - sy1) as f32;
                        let len = (dx * dx + dy * dy).sqrt();
                        if len > 4.0 {
                            super::ui_draw::draw_line_aa(
                                buffer, width, height, sx1 as f32, sy1 as f32, sx2 as f32, sy2 as f32, thick, col,
                            );
                            let ux = dx / len;
                            let uy = dy / len;
                            let px = -uy;
                            let py = ux;
                            let hlen = (thick * 5.0).min(len * 0.45).max(3.0);
                            let hw = hlen * 0.45;
                            let bx = sx2 as f32 - ux * hlen;
                            let by = sy2 as f32 - uy * hlen;
                            super::ui_draw::draw_line_aa(buffer, width, height, sx2 as f32, sy2 as f32, bx + px * hw, by + py * hw, thick, col);
                            super::ui_draw::draw_line_aa(buffer, width, height, sx2 as f32, sy2 as f32, bx - px * hw, by - py * hw, thick, col);
                        }
                    } else {
                        let (_, target_rect) = self.resolve_target_page_and_draw_rect(sx1, sy1);
                        let (px, py, pw, ph) = target_rect;
                        let page_left = px.max(sidebar_w as i32);
                        let page_right = (px + pw as i32).min(width as i32);
                        let page_top = py.max(top_bars_h as i32);
                        let page_bottom =
                            (py + ph as i32).min(height.saturating_sub(status_bar_h) as i32);

                        if page_right > page_left && page_bottom > page_top {
                            let min_x = (sx1.min(sx2).round() as i32).clamp(page_left, page_right) as u32;
                            let max_x = (sx1.max(sx2).round() as i32).clamp(page_left, page_right) as u32;
                            let min_y = (sy1.min(sy2).round() as i32).clamp(page_top, page_bottom) as u32;
                            let max_y = (sy1.max(sy2).round() as i32).clamp(page_top, page_bottom) as u32;

                            if max_x > min_x && max_y > min_y {
                                let (ar, ag, ab) = self.annotation_color.rgb();
                                let ann_col = self.annotation_color.u32_rgb();
                                let (fill_rgba, border_col) = match self.mouse_mode {
                                    MouseMode::Select => ((37, 99, 235, 42), 0x002563EB),
                                    MouseMode::Highlight => ((ar as u32, ag as u32, ab as u32, 95), ann_col),
                                    MouseMode::Underline | MouseMode::Strikethrough => ((ar as u32, ag as u32, ab as u32, 48), ann_col),
                                    MouseMode::RectShape => ((ar as u32, ag as u32, ab as u32, 22), ann_col),
                                    MouseMode::Redact => ((220, 38, 38, 105), 0x00DC2626),
                                    MouseMode::Crop => ((5, 150, 105, 85), 0x00059669),
                                    MouseMode::StampImage => ((124, 58, 237, 85), 0x007C3AED),
                                    _ => ((ar as u32, ag as u32, ab as u32, 40), ann_col),
                                };
                                blend_rect_rgba(buffer, width, height, min_x, min_y, max_x - min_x, max_y - min_y, fill_rgba);
                                draw_rect_border(buffer, width, height, min_x, min_y, max_x - min_x, max_y - min_y, border_col);
                            }
                        }
                    }
                }
            }

            // Quick Measure / Ruler Tool HUD & live vector guides
            if self.mouse_mode == MouseMode::Measure {
                self.draw_measure_overlay(
                    buffer, width, height, sidebar_w, top_bars_h, status_bar_h, scale_factor,
                );
            }

            // Multi-line Sticky Note reader popover with Edit & Delete controls
            if self.active_note_popover.is_some() {
                self.draw_active_note_popover(buffer, width, height, scale_factor);
            }

            // Right-click Annotation Inspector Context Menu
            if self.annot_context_menu.is_some() {
                self.draw_annot_context_menu(buffer, width, height, scale_factor);
            }

            // Lightweight Matrix Page Deskew bar, alignment grid, and Draw Guideline overlay
            if self.deskew_bar.is_some() {
                self.draw_deskew_overlay(
                    buffer, width, height, sidebar_w, top_bars_h, status_bar_h, scale_factor,
                );
            }
        } else {
            self.draw_thumbnail_grid(
                buffer, width, height, sidebar_w, top_bars_h, status_bar_h, scale_factor,
            );
        }

        // Thin right-edge overlay vertical scrollbar
        if let Some((tx, ty, tw, th, tby, tbh)) =
            self.vertical_scrollbar_geometry(width, height, top_bars_h, status_bar_h, scale_factor)
        {
            let tr = theme.scrollbar_track;
            let tr_col = (tr.0 << 16) | (tr.1 << 8) | tr.2;
            super::ui_draw::fill_rounded_rect_alpha(
                buffer, width, height, tx, ty, tw, th, (tw as f32) * 0.5, tr_col, tr.3,
            );
            let thm = if self.scrollbar_drag || self.hovered_widget == Some(UiHit::Scrollbar) {
                theme.scrollbar_thumb_hover
            } else {
                theme.scrollbar_thumb
            };
            let th_col = (thm.0 << 16) | (thm.1 << 8) | thm.2;
            super::ui_draw::fill_rounded_rect_alpha(
                buffer, width, height, tx, tby, tw, tbh, (tw as f32) * 0.5, th_col, thm.3,
            );
        }

        // Thin bottom-edge overlay horizontal scrollbar (Normal & Compare windows when zoomed)
        if let Some((tx, ty, tw, th, tbx, tbw)) =
            self.horizontal_scrollbar_geometry(width, height, status_bar_h, scale_factor)
        {
            let tr = theme.scrollbar_track;
            let tr_col = (tr.0 << 16) | (tr.1 << 8) | tr.2;
            super::ui_draw::fill_rounded_rect_alpha(
                buffer, width, height, tx, ty, tw, th, (th as f32) * 0.5, tr_col, tr.3,
            );
            let thm = if self.hscrollbar_drag || self.hovered_widget == Some(UiHit::HScrollbar) {
                theme.scrollbar_thumb_hover
            } else {
                theme.scrollbar_thumb
            };
            let th_col = (thm.0 << 16) | (thm.1 << 8) | thm.2;
            super::ui_draw::fill_rounded_rect_alpha(
                buffer, width, height, tbx, ty, tbw, th, (th as f32) * 0.5, th_col, thm.3,
            );
        }

        // Leftmost Format Drawer Rail (`All` + open document format drawers)
        let format_rail_bot = height.saturating_sub(status_bar_h);
        if self.format_rail_width_px(scale_factor) > 0 && format_rail_bot > top_bars_h {
            self.draw_format_drawer_rail(
                buffer,
                width,
                height,
                top_bars_h,
                format_rail_bot,
                scale_factor,
            );
        }

        // Left Bookmark / Distinct Book / Outline Sidebar
        if self.show_bookmarks && sidebar_w > 0 {
            if self.is_distinct_epub_mode() {
                self.draw_distinct_epub_sidebar(
                    buffer,
                    width,
                    height.saturating_sub(status_bar_h),
                    sidebar_w,
                    top_bars_h,
                    scale_factor,
                );
            } else if self.is_distinct_comic_mode() {
                self.draw_distinct_comic_sidebar(
                    buffer,
                    width,
                    height.saturating_sub(status_bar_h),
                    sidebar_w,
                    top_bars_h,
                    scale_factor,
                );
            } else if self.is_distinct_word_mode() {
                self.draw_distinct_word_sidebar(
                    buffer,
                    width,
                    height.saturating_sub(status_bar_h),
                    sidebar_w,
                    top_bars_h,
                    scale_factor,
                );
            } else if self.is_distinct_outline_mode() {
                self.draw_distinct_outline_sidebar(
                    buffer,
                    width,
                    height.saturating_sub(status_bar_h),
                    sidebar_w,
                    top_bars_h,
                    scale_factor,
                );
            } else {
                self.draw_bookmark_sidebar(
                    buffer,
                    width,
                    height.saturating_sub(status_bar_h),
                    sidebar_w,
                    top_bars_h,
                    scale_factor,
                );
            }
        }

        // Multi-Document Tab Bar
        self.draw_tab_bar(buffer, width, height, menu_bar_h, tab_bar_h, scale_factor);

        // Unified Top Menu + Inline Customizable Toolbar Bar
        self.draw_menu_bar(buffer, width, height, menu_bar_h, scale_factor);

        // Centered Bottom Page View & Status Bar
        self.draw_status_bar(buffer, width, height, status_bar_h, scale_factor);

        let chrome_visible = !self.is_fullscreen && !self.is_presentation;

        // Distinct EPUB Floating Selection Action Pill (Highlight | Add Note | Copy)
        if self.is_distinct_epub_mode()
            && self.epub_selection_drag.is_some()
            && self.drag_start.is_none()
            && !self.has_blocking_modal()
            && !self.show_aa_popover
            && !self.book_info_popover
        {
            self.recompute_epub_selection_pill(width, top_bars_h, scale_factor);
            self.draw_epub_selection_pill(buffer, width, height, scale_factor);
        } else if self.drag_start.is_some() || !self.is_distinct_epub_mode() {
            self.epub_selection_pill_rect = None;
        }

        // Session Notification Board Popover (bottom-left)
        if chrome_visible && self.show_notifications {
            self.draw_notification_board_popover(buffer, width, height, status_bar_h, scale_factor);
        }

        // Reading Color Mode Picker Popover (6 eye-care palettes)
        if chrome_visible && self.show_reading_mode_popover {
            self.draw_reading_mode_popover(buffer, width, height, status_bar_h, scale_factor);
        }

        // Scroll Mode Picker Popover (Single / Vert / Book / Magazine)
        if chrome_visible && self.show_scroll_mode_popover {
            self.draw_scroll_mode_popover(buffer, width, height, status_bar_h, scale_factor);
        }

        // Distinct EPUB Book Metadata Info Popover & Aa Typography Popover (EPUB / MD / LaTeX)
        if chrome_visible && self.is_markdown_tab() {
            if self.is_distinct_epub_mode() && self.book_info_popover && self.show_bookmarks && sidebar_w > 0 {
                self.draw_book_info_popover(buffer, width, height, sidebar_w, top_bars_h, scale_factor);
            }
            if self.show_aa_popover {
                self.draw_aa_popover(buffer, width, height, status_bar_h, scale_factor);
            }
        }

        // Floating Hover Tooltip Pill for Top Toolbar & Bottom Bar buttons (only when no modal/menu/prompt is open)
        if chrome_visible
            && !self.has_blocking_modal()
            && !self.has_open_popover()
        {
            self.draw_floating_hover_tooltip(buffer, width, height, menu_bar_h, status_bar_h, scale_factor);
        }

        // Thumbnail Grid Right-Click Context Menu
        if self.thumb_context_menu.is_some() && self.view_mode == ViewMode::ThumbnailGrid {
            self.draw_thumb_context_menu(buffer, width, height, scale_factor);
        }

        // All-Tabs Overflow Dropdown Popover (`▾`)
        if chrome_visible && self.show_tab_list_popover {
            self.draw_tab_list_popover(buffer, width, height, top_bars_h, scale_factor);
        }

        // Tab Bar Right-Click Context Menu
        if chrome_visible && self.tab_context_menu.is_some() {
            self.draw_tab_context_menu(buffer, width, height, scale_factor);
        }

        // Floating Search Bar / Optimize / Prompt Card
        if self.input_mode != InputMode::Normal {
            self.draw_active_input_overlay(buffer, width, height, top_bars_h, scale_factor);
        }

        // Open Menu Dropdown
        if chrome_visible {
            if let Some(cat) = self.open_menu {
                self.draw_open_menu_dropdown(buffer, width, height, menu_bar_h, cat, scale_factor);
            }
        }

        // Customize Toolbar Modal
        if self.show_customize_toolbar {
            self.draw_customize_toolbar_modal(buffer, width, height, scale_factor);
        }

        // Built-in Step-by-Step User Guide Modal (`Help -> User Guide (How to Use)`)
        if self.user_guide_modal.is_some() {
            self.draw_user_guide_modal(buffer, width, height, scale_factor);
        }

        // Favorites Library Modal (`File -> Favorites Library... (Ctrl+D)`)
        if self.favorites_modal.is_some() {
            self.draw_favorites_modal(buffer, width, height, scale_factor);
        }

        // Help Overlay (`?` / `F1`)
        if self.show_help_overlay {
            self.draw_themed_help_overlay(buffer, width, height, scale_factor);
        }

        // Bookmark Import Format Guide Modal (`Help -> Bookmark Import Format (.txt)`)
        if self.show_bookmark_format_overlay {
            self.draw_bookmark_format_overlay(buffer, width, height, scale_factor);
        }

        // About MicroPDF Modal (`Help -> About MicroPDF`)
        if self.show_about_overlay {
            self.draw_about_overlay(buffer, width, height, scale_factor);
        }

        // Interactive Combine / Insert Multiple PDFs Modal (`Shift+M` / `Page -> Insert PDF(s)...`)
        if self.combine_modal.is_some() {
            self.draw_combine_modal(buffer, width, height, scale_factor);
        }

        // Header & Footer Modal (`H`)
        if self.header_footer_modal.is_some() {
            self.draw_header_footer_modal(buffer, width, height, scale_factor);
        }

        // Page Numbering Modal (`N`)
        if self.page_number_modal.is_some() {
            self.draw_page_number_modal(buffer, width, height, scale_factor);
        }

        // Create PDF from Images Modal
        if self.create_pdf_images_modal.is_some() {
            self.draw_create_pdf_images_modal(buffer, width, height, scale_factor);
        }

        // Export Pages as Images Modal
        if self.export_images_modal.is_some() {
            self.draw_export_images_modal(buffer, width, height, scale_factor);
        }

        // Export to Excel Options Modal
        if self.excel_export_modal.is_some() {
            self.draw_excel_export_modal(buffer, width, height, scale_factor);
        }

        // Export Markdown Modal
        if self.markdown_export_modal.is_some() {
            self.draw_markdown_export_modal(buffer, width, height, scale_factor);
        }

        // Split to Single-Page PDFs Modal
        if self.split_pages_modal.is_some() {
            self.draw_split_pages_modal(buffer, width, height, scale_factor);
        }

        // Detailed Watermark Modal with Live Preview
        if self.watermark_modal.is_some() {
            self.draw_watermark_modal(buffer, width, height, scale_factor);
        }

        // Portable Stamp & Signature Manager Modal
        if self.stamp_manager_modal.is_some() {
            self.draw_stamp_manager_modal(buffer, width, height, scale_factor);
        }

        // Document Properties & Privacy Metadata Scrubber Modal
        if self.doc_props_modal.is_some() {
            self.draw_doc_props_modal(buffer, width, height, scale_factor);
        }

        // Print Preview Modal (`Ctrl/Cmd+P`)
        if self.print_preview.is_some() {
            self.draw_print_preview_modal(buffer, width, height, scale_factor);
        }

        // Guided Manual Duplex (Simplex Printer) Pass 2 Modal
        if self.manual_duplex_wait.is_some() {
            self.draw_manual_duplex_modal(buffer, width, height, scale_factor);
        }

        // Unsaved Changes Confirmation Dialog (top-most modal)
        if self.unsaved_changes_modal.is_some() {
            self.draw_unsaved_changes_modal(buffer, width, height, scale_factor);
        }
    }

    pub(super) fn curl_paper_bg_color(&self, theme: &super::theme::UiTheme) -> u32 {
        match self.reading_filter {
            ReadingFilter::DarkInvert => 0x001E_222B,
            ReadingFilter::NightAmber => 0x0024_1A12,
            ReadingFilter::WarmSepia => 0x00F4_ECD8,
            ReadingFilter::SageEyeCare => 0x00DF_ECE2,
            ReadingFilter::PaperEInk => 0x00F5_F5F2,
            ReadingFilter::Normal => {
                if self.ui_theme_mode == super::theme::UiThemeMode::Dark {
                    theme.surface_bg
                } else {
                    0x00F8_F9FB
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_active_spread_curl(
        &mut self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        cur_rect: (i32, i32, u32, u32),
        canvas_rect: (i32, i32, u32, u32),
        canvas_bg: u32,
        paper_bg: u32,
        s: f32,
    ) {
        let Some((linear_t, dir)) = self.page_turn_anim_linear() else {
            return;
        };
        let Some(ref mut anim) = self.page_turn_anim else {
            return;
        };
        if anim.prev_rect != Some(cur_rect) && cur_rect.2 >= 8 && cur_rect.3 >= 8 {
            if let (Some((ox, oy, _, _)), Some((ow, oh, old_px))) =
                (anim.prev_rect, anim.prev_spread.take())
            {
                let cx0 = canvas_rect.0;
                let cy0 = canvas_rect.1;
                let cx1 = cx0 + canvas_rect.2 as i32;
                let cy1 = cy0 + canvas_rect.3 as i32;
                let ux = ox.min(cur_rect.0).clamp(cx0, cx1);
                let uy = oy.min(cur_rect.1).clamp(cy0, cy1);
                let ur = (ox + ow as i32).max(cur_rect.0 + cur_rect.2 as i32).clamp(ux, cx1);
                let ub = (oy + oh as i32).max(cur_rect.1 + cur_rect.3 as i32).clamp(uy, cy1);
                let uw = (ur - ux) as u32;
                let uh = (ub - uy) as u32;
                if uw >= 8 && uh >= 8 {
                    let mut merged = vec![canvas_bg; (uw as usize).saturating_mul(uh as usize)];
                    let src_w = ow as usize;
                    let dst_w = uw as usize;
                    for sy in 0..oh as usize {
                        let dy = (oy + sy as i32) - uy;
                        if dy < 0 || dy >= uh as i32 {
                            continue;
                        }
                        let dst_row = (dy as usize) * dst_w;
                        let src_row = sy * src_w;
                        for sx in 0..ow as usize {
                            let dx = (ox + sx as i32) - ux;
                            if dx >= 0 && dx < uw as i32 {
                                merged[dst_row + dx as usize] = old_px[src_row + sx];
                            }
                        }
                    }
                    anim.prev_spread = Some((uw, uh, merged));
                    anim.prev_rect = Some((ux, uy, uw, uh));
                } else {
                    anim.prev_spread = Some((ow, oh, old_px));
                }
            }
        }
        if let (Some((aw, ah, prev_px)), Some((ax, ay, _, _))) =
            (&anim.prev_spread, anim.prev_rect)
        {
            super::ui_draw::draw_apple_books_page_curl(
                buffer,
                buf_w,
                buf_h,
                prev_px,
                ax,
                ay,
                *aw,
                *ah,
                linear_t,
                dir,
                paper_bg,
                s,
            );
        }
    }

    pub(super) fn capture_pdf_spread_snapshot(&self) -> Option<((i32, i32, u32, u32), Vec<u32>)> {
        let win = self.window.as_ref()?;
        let sz = win.inner_size();
        let width = sz.width;
        let height = sz.height;
        if width < 80 || height < 80 {
            return None;
        }
        let scale_factor = self.effective_scale_factor(win.scale_factor());
        let sidebar_w = self.sidebar_width_px(scale_factor).min(width.saturating_sub(64));
        let top_bars_h = self.top_bars_height_px(scale_factor).min(height.saturating_sub(48));
        let status_bar_h = self.status_bar_height_px(scale_factor).min(height.saturating_sub(top_bars_h + 16));
        let bottom_limit = height.saturating_sub(status_bar_h);

        let r1 = self.page_draw_rect;
        if r1.2 == 0 || r1.3 == 0 {
            return None;
        }
        let (raw_x, raw_y, raw_r, raw_b) = if let Some(r2) = self.second_page_draw_rect {
            (
                r1.0.min(r2.0),
                r1.1.min(r2.1),
                (r1.0 + r1.2 as i32).max(r2.0 + r2.2 as i32),
                (r1.1 + r1.3 as i32).max(r2.1 + r2.3 as i32),
            )
        } else {
            (r1.0, r1.1, r1.0 + r1.2 as i32, r1.1 + r1.3 as i32)
        };
        let sx = raw_x.max(sidebar_w as i32);
        let sy = raw_y.max(top_bars_h as i32);
        let sr = raw_r.min(width as i32);
        let sb = raw_b.min(bottom_limit as i32);
        if sr <= sx + 8 || sb <= sy + 8 {
            return None;
        }
        let sw = (sr - sx) as u32;
        let sh = (sb - sy) as u32;

        let theme = self.ui_theme();
        let canvas_bg = match self.reading_filter {
            ReadingFilter::DarkInvert => 0x0014_161A,
            ReadingFilter::NightAmber => 0x0014_0F0B,
            _ => theme.canvas_bg,
        };
        let mut snap = vec![canvas_bg; (sw as usize).saturating_mul(sh as usize)];
        if let Some(ref cache) = self.cached_page {
            let local_r1 = (r1.0 - sx, r1.1 - sy, r1.2, r1.3);
            self.draw_page_frame_chrome(&mut snap, sw, sh, 0, 0, 0, local_r1, scale_factor);
            self.blit_page_or_tiles(&mut snap, sw, sh, 0, 0, 0, local_r1, cache);
        }
        if let (Some(second_cache), Some(r2)) = (&self.cached_second_page, self.second_page_draw_rect) {
            if r2.2 > 0 && r2.3 > 0 {
                let local_r2 = (r2.0 - sx, r2.1 - sy, r2.2, r2.3);
                self.draw_page_frame_chrome(&mut snap, sw, sh, 0, 0, 0, local_r2, scale_factor);
                self.blit_page_or_tiles(&mut snap, sw, sh, 0, 0, 0, local_r2, second_cache);
                let s = scale_factor as f32;
                let top_y = (r1.1 - sy).min(local_r2.1).max(0);
                let bot_y = (r1.1 - sy + r1.3 as i32).max(local_r2.1 + local_r2.3 as i32).min(sh as i32);
                super::ui_draw::draw_book_spine_crease(
                    &mut snap,
                    sw,
                    sh,
                    local_r2.0,
                    top_y,
                    (bot_y - top_y).max(0) as u32,
                    (14.0 * s).round().max(6.0) as i32,
                );
            }
        }
        Some(((sx, sy, sw, sh), snap))
    }

    /// Converts a PDF-coordinate rectangle on `page_idx` into `(screen_x, screen_y, width, height)`
    /// for rendering live move/selection ghosts over the currently visible page(s).
    /// Handles 0°/90°/180°/270° rotated pages via PDFium's `points_to_pixels`.
    pub(super) fn pdf_rect_to_screen_rect(&self, page_idx: u16, rect: PdfRect) -> Option<(i32, i32, u32, u32)> {
        let draw_rect = if self.is_continuous_scroll() {
            let &(_, dr) = self.continuous_draw_rects.iter().find(|&&(p, _)| p == page_idx)?;
            dr
        } else if self.cached_page.as_ref().is_some_and(|c| c.page_index == page_idx)
            || self.current_page_index == page_idx
        {
            self.page_draw_rect
        } else if self.cached_second_page.as_ref().is_some_and(|c| c.page_index == page_idx) {
            self.second_page_draw_rect?
        } else {
            return None;
        };
        let page = self.doc.as_ref()?.pages().get(page_idx).ok()?;
        let pw_pts = page.width().value;
        let ph_pts = page.height().value;
        if pw_pts <= 0.0 || ph_pts <= 0.0 || draw_rect.2 == 0 || draw_rect.3 == 0 {
            return None;
        }
        let cfg = PdfRenderConfig::new()
            .set_fixed_width(draw_rect.2 as i32)
            .set_fixed_height(draw_rect.3 as i32);
        let (bx1, by1, bx2, by2) = match (
            page.points_to_pixels(rect.left(), rect.top(), &cfg),
            page.points_to_pixels(rect.right(), rect.bottom(), &cfg),
        ) {
            (Ok((x1, y1)), Ok((x2, y2))) => (x1, y1, x2, y2),
            _ => {
                let sx = draw_rect.2 as f32 / pw_pts;
                let sy = draw_rect.3 as f32 / ph_pts;
                (
                    (rect.left().value * sx).round() as i32,
                    ((ph_pts - rect.top().value) * sy).round() as i32,
                    (rect.right().value * sx).round() as i32,
                    ((ph_pts - rect.bottom().value) * sy).round() as i32,
                )
            }
        };
        let x0 = draw_rect.0 + bx1;
        let x1 = draw_rect.0 + bx2;
        let y0 = draw_rect.1 + by1;
        let y1 = draw_rect.1 + by2;
        let left = x0.min(x1);
        let top = y0.min(y1);
        let rw = (x1 - x0).unsigned_abs().max(4);
        let rh = (y1 - y0).unsigned_abs().max(4);
        Some((left, top, rw, rh))
    }

    /// Draws a crisp floating tooltip pill directly below hovered top-toolbar buttons
    /// or directly above hovered bottom-bar buttons.
    fn draw_floating_hover_tooltip(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        menu_bar_h: u32,
        status_bar_h: u32,
        sf: f64,
    ) {
        let (label, anchor_rect, below, beside_right): (String, _, _, _) = match self.hovered_widget {
            Some(UiHit::Toolbar(act)) => {
                let tb_x = self.toolbar_start_x(sf);
                let Some(rect) = self.toolbar_widget_rect(act, tb_x, buf_w, menu_bar_h, sf) else {
                    return;
                };
                (act.tooltip().to_string(), rect, true, false)
            }
            Some(UiHit::BottomBar(act)) => {
                if self.show_notifications && act == super::bottom_bar::BottomBarAction::NotificationBoard {
                    return;
                }
                if self.show_reading_mode_popover && act == super::bottom_bar::BottomBarAction::ReadingFilter {
                    return;
                }
                if self.show_scroll_mode_popover && act == super::bottom_bar::BottomBarAction::Continuous {
                    return;
                }
                let Some(rect) = self.bottom_bar_widget_rect(act, buf_w, buf_h, status_bar_h, sf) else {
                    return;
                };
                (act.tooltip().to_string(), rect, false, false)
            }
            Some(UiHit::ComicBottom(cb)) => {
                if matches!(cb, ComicBottomHit::Scrubber(_)) {
                    return;
                }
                let Some(rect) = self.comic_bottom_widget_rect(cb, buf_w, buf_h, status_bar_h, sf) else {
                    return;
                };
                (cb.tooltip().to_string(), rect, false, false)
            }
            Some(UiHit::FormatDrawer(opt_kind)) => {
                let top_bars_h = self.top_bars_height_px(sf).min(buf_h.saturating_sub(48));
                let Some(rect) = self.format_drawer_rect_for(opt_kind, buf_h, top_bars_h, status_bar_h, sf) else {
                    return;
                };
                (self.format_drawer_tooltip(opt_kind), rect, false, true)
            }
            _ => return,
        };

        let s = sf as f32;
        let font_sz = (11.0 * s).clamp(9.5, 20.0);
        let pad_x = (9.0 * s).round() as u32;
        let tip_h = (22.0 * s).round().clamp(18.0, 38.0) as u32;
        let tw = super::ui_draw::measure_text(&label, font_sz, false).ceil() as u32;
        let tip_w = (tw + pad_x * 2).min(buf_w.saturating_sub(16));
        let gap = (6.0 * s).round() as i32;
        let (tx, ty) = if beside_right {
            let tx = (anchor_rect.0 + anchor_rect.2 as i32 + (8.0 * s).round() as i32)
                .min((buf_w as i32 - tip_w as i32 - 8).max(8));
            let ty = (anchor_rect.1 + (anchor_rect.3 as i32 - tip_h as i32) / 2)
                .clamp(6, (buf_h as i32 - tip_h as i32 - 6).max(6));
            (tx, ty)
        } else {
            let cx = anchor_rect.0 + (anchor_rect.2 as i32) / 2;
            let max_tx = (buf_w as i32 - tip_w as i32 - 8).max(8);
            let tx = (cx - (tip_w as i32) / 2).clamp(8, max_tx);
            let ty = if below {
                (anchor_rect.1 + anchor_rect.3 as i32 + gap).min(buf_h as i32 - tip_h as i32 - 6)
            } else {
                (anchor_rect.1 - tip_h as i32 - gap).max(6)
            };
            (tx, ty)
        };
        let rad = (5.0 * s).clamp(4.0, 9.0);
        // Dark crisp tooltip pill in Light mode, elevated slate in Dark mode (high contrast in both!)
        let (bg, border, fg) = match self.ui_theme_mode {
            super::theme::UiThemeMode::Light => (0x001E293B, 0x00334155, 0x00F8FAFC),
            super::theme::UiThemeMode::Dark => (0x00334155, 0x00475569, 0x00F8FAFC),
        };
        super::ui_draw::draw_soft_shadow(
            buffer, buf_w, buf_h, tx, ty, tip_w, tip_h, rad,
            (6.0 * s).round() as i32, (2.0 * s).round() as i32,
            (0, 0, 0), 56,
        );
        super::ui_draw::fill_rounded_rect(buffer, buf_w, buf_h, tx, ty, tip_w, tip_h, rad, bg);
        super::ui_draw::stroke_rounded_rect(buffer, buf_w, buf_h, tx, ty, tip_w, tip_h, rad, border);
        super::ui_draw::draw_text_vcenter(
            buffer, buf_w, buf_h,
            tx + pad_x as i32, ty, tip_h, tx + tip_w as i32 - 4,
            &label, font_sz, fg, false,
        );
    }

    pub(super) fn render_to_window(&mut self) -> Result<()> {
        let Some(window) = self.window.as_ref().cloned() else {
            return Ok(());
        };
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);
        let scale_factor = self.effective_scale_factor(window.scale_factor());

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

            self.render_frame_into_buffer(&mut buffer, width, height, scale_factor);

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

    pub(super) fn render_editor_popout(&mut self) -> Result<()> {
        let Some(mut pop) = self.editor_popout.take() else {
            return Ok(());
        };
        let size = pop.window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);
        let sf = self.effective_scale_factor(pop.window.scale_factor());

        let theme = self.ui_theme();
        let (page_bg, content_theme) = super::markdown_view::md_reading_theme(
            theme,
            self.reading_filter,
            self.ui_theme_mode,
        );
        let syntax = self.active_editor_syntax();
        let mode = self.md_editor_mode;
        let zoom = self.zoom_level;
        self.ensure_md_editor_initialized();

        let render_res = if let Some(mut surface) = pop.surface.take() {
            let res = (|| -> Result<()> {
                surface
                    .resize(
                        NonZeroU32::new(width).unwrap(),
                        NonZeroU32::new(height).unwrap(),
                    )
                    .map_err(|e| anyhow::anyhow!("Failed to resize popout surface: {e}"))?;
                let mut buffer = surface
                    .buffer_mut()
                    .map_err(|e| anyhow::anyhow!("Failed to acquire popout buffer: {e}"))?;

                let hdr_h = super::text_editor::editor_header_height_px(sf).min(height.saturating_sub(24));
                super::text_editor::draw_editor_header(
                    &mut buffer,
                    width,
                    height,
                    0,
                    0,
                    width,
                    hdr_h,
                    syntax,
                    mode,
                    true,
                    pop.hovered_action,
                    &theme,
                    sf,
                );

                let eh = height.saturating_sub(hdr_h);
                if eh > 0 {
                    if let Some(ref mut ed) = self.md_editor {
                        ed.draw(
                            &mut buffer,
                            width,
                            height,
                            0,
                            hdr_h as i32,
                            width,
                            eh,
                            page_bg,
                            &content_theme,
                            sf,
                            zoom,
                            syntax,
                        );
                    }
                }

                buffer
                    .present()
                    .map_err(|e| anyhow::anyhow!("Failed to present popout buffer: {e}"))?;
                Ok(())
            })();
            pop.surface = Some(surface);
            res
        } else {
            Ok(())
        };

        self.editor_popout = Some(pop);
        render_res
    }
}

/// Fills a clipped rectangle in the `0x00RRGGBB` pixel buffer.
pub(crate) fn fill_rect_clipped(
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

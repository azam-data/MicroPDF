//! Comic Book (`.cbz`) Canvas Renderer & Bounded LRU Cache Manager.
//!
//! Renders comic pages in Single Page, 2-Page Spread (with Cover-First & LTR/RTL Manga pairing),
//! and Vertical Strip (Webtoon) modes directly into the `softbuffer` `0x00RRGGBB` pixel buffer.

use crate::comic::ComicPageBitmap;

use super::render::fill_rect_clipped;
use super::state::*;
use super::theme::UiSkin;
use super::ui_draw::{
    UiIcon, draw_icon, draw_text_clipped, draw_text_vcenter, fill_circle_aa, fill_rounded_rect,
    measure_text, stroke_rounded_rect, truncate_ellipsis,
};

pub(super) const COMIC_PAGE_CACHE_MAX_COUNT: usize = 4;
pub(super) const COMIC_PAGE_CACHE_MAX_BYTES: usize = 32 * 1024 * 1024;
pub(super) const COMIC_THUMB_CACHE_MAX_COUNT: usize = 40;
pub(super) const COMIC_THUMB_CACHE_MAX_BYTES: usize = 8 * 1024 * 1024;

impl<'a> PdfViewerApp<'a> {
    fn comic_page_cache_bytes(&self) -> usize {
        self.comic_page_cache
            .values()
            .map(|e| e.bitmap.byte_len())
            .sum()
    }

    fn comic_thumb_cache_bytes(&self) -> usize {
        self.comic_thumb_cache
            .values()
            .map(|e| e.bitmap.byte_len())
            .sum()
    }

    fn evict_comic_page_cache_if_needed(&mut self, protect_idx: usize) {
        while self.comic_page_cache.len() > COMIC_PAGE_CACHE_MAX_COUNT
            || self.comic_page_cache_bytes() > COMIC_PAGE_CACHE_MAX_BYTES
        {
            let evict_key = self
                .comic_page_cache
                .iter()
                .filter(|(k, _)| **k != protect_idx || self.comic_page_cache.len() > 1)
                .min_by_key(|(_, v)| v.last_tick)
                .map(|(k, _)| *k);
            let Some(k) = evict_key else {
                break;
            };
            self.comic_page_cache.remove(&k);
        }
    }

    fn evict_comic_thumb_cache_if_needed(&mut self, protect_idx: usize) {
        while self.comic_thumb_cache.len() > COMIC_THUMB_CACHE_MAX_COUNT
            || self.comic_thumb_cache_bytes() > COMIC_THUMB_CACHE_MAX_BYTES
        {
            let evict_key = self
                .comic_thumb_cache
                .iter()
                .filter(|(k, _)| **k != protect_idx || self.comic_thumb_cache.len() > 1)
                .min_by_key(|(_, v)| v.last_tick)
                .map(|(k, _)| *k);
            let Some(k) = evict_key else {
                break;
            };
            self.comic_thumb_cache.remove(&k);
        }
    }

    pub(super) fn ensure_comic_page_cached(&mut self, page_idx: usize) -> Option<&ComicPageBitmap> {
        self.comic_lru_tick = self.comic_lru_tick.wrapping_add(1);
        let tick = self.comic_lru_tick;
        if let Some(entry) = self.comic_page_cache.get_mut(&page_idx) {
            entry.last_tick = tick;
            return self.comic_page_cache.get(&page_idx).map(|e| &e.bitmap);
        }
        let bmp = self.comic_doc.as_ref()?.decode_page(page_idx).ok()?;
        self.comic_page_dims
            .insert(page_idx, (bmp.width.max(1), bmp.height.max(1)));
        self.comic_page_cache.insert(
            page_idx,
            ComicCachedEntry {
                bitmap: bmp,
                last_tick: tick,
            },
        );
        self.evict_comic_page_cache_if_needed(page_idx);
        self.comic_page_cache.get(&page_idx).map(|e| &e.bitmap)
    }

    pub(super) fn ensure_comic_thumb_cached(
        &mut self,
        page_idx: usize,
        max_w: u32,
        max_h: u32,
    ) -> Option<&ComicPageBitmap> {
        self.comic_lru_tick = self.comic_lru_tick.wrapping_add(1);
        let tick = self.comic_lru_tick;
        if let Some(entry) = self.comic_thumb_cache.get_mut(&page_idx) {
            entry.last_tick = tick;
            return self.comic_thumb_cache.get(&page_idx).map(|e| &e.bitmap);
        }
        let bmp = self
            .comic_doc
            .as_ref()?
            .decode_thumbnail(page_idx, max_w, max_h)
            .ok()?;
        self.comic_thumb_cache.insert(
            page_idx,
            ComicCachedEntry {
                bitmap: bmp,
                last_tick: tick,
            },
        );
        self.evict_comic_thumb_cache_if_needed(page_idx);
        self.comic_thumb_cache.get(&page_idx).map(|e| &e.bitmap)
    }

    pub(super) fn comic_spread_visual_pages_for(
        total: usize,
        cur_idx: usize,
        cover_first: bool,
        rtl_manga: bool,
    ) -> (Option<usize>, Option<usize>) {
        if total == 0 {
            return (None, None);
        }
        let cur = cur_idx.min(total - 1);
        let (first, second) = if cover_first {
            if cur == 0 {
                return (Some(0), None);
            }
            let left_logical = if cur % 2 == 1 { cur } else { cur - 1 };
            let right_logical = if left_logical + 1 < total {
                Some(left_logical + 1)
            } else {
                None
            };
            (left_logical, right_logical)
        } else {
            let left_logical = if cur % 2 == 0 { cur } else { cur - 1 };
            let right_logical = if left_logical + 1 < total {
                Some(left_logical + 1)
            } else {
                None
            };
            (left_logical, right_logical)
        };

        if rtl_manga {
            match second {
                Some(s_idx) => (Some(s_idx), Some(first)),
                None => (Some(first), None),
            }
        } else {
            (Some(first), second)
        }
    }

    pub(super) fn comic_spread_logical_range_for(
        total: usize,
        cur_idx: usize,
        layout_mode: ComicLayoutMode,
        cover_first: bool,
    ) -> (usize, Option<usize>) {
        if total == 0 {
            return (0, None);
        }
        let cur = cur_idx.min(total - 1);
        if layout_mode != ComicLayoutMode::TwoPageSpread {
            return (cur, None);
        }
        if cover_first {
            if cur == 0 {
                return (0, None);
            }
            let a = if cur % 2 == 1 { cur } else { cur - 1 };
            let b = if a + 1 < total { Some(a + 1) } else { None };
            (a, b)
        } else {
            let a = if cur % 2 == 0 { cur } else { cur - 1 };
            let b = if a + 1 < total { Some(a + 1) } else { None };
            (a, b)
        }
    }

    pub(super) fn comic_step_index(
        total: usize,
        cur_idx: usize,
        layout_mode: ComicLayoutMode,
        cover_first: bool,
        logical_delta: isize,
    ) -> usize {
        if total == 0 {
            return 0;
        }
        let cur = cur_idx.min(total - 1);
        if layout_mode == ComicLayoutMode::TwoPageSpread {
            if logical_delta >= 0 {
                if cover_first && cur == 0 {
                    1usize.min(total - 1)
                } else {
                    let a = if cover_first {
                        if cur % 2 == 1 { cur } else { cur - 1 }
                    } else if cur % 2 == 0 {
                        cur
                    } else {
                        cur - 1
                    };
                    (a + 2).min(total - 1)
                }
            } else if cover_first && cur <= 1 {
                0
            } else {
                let a = if cover_first {
                    if cur % 2 == 1 { cur } else { cur - 1 }
                } else if cur % 2 == 0 {
                    cur
                } else {
                    cur - 1
                };
                a.saturating_sub(2)
            }
        } else if logical_delta >= 0 {
            (cur + logical_delta as usize).min(total - 1)
        } else {
            cur.saturating_sub((-logical_delta) as usize)
        }
    }

    /// Returns `(left_page_opt, right_page_opt)` in visual screen order for 2-Page Spread,
    /// honoring `cover_first` and `rtl_manga`.
    pub(super) fn comic_spread_visual_pages(&self) -> (Option<usize>, Option<usize>) {
        let total = self.comic_doc.as_ref().map(|c| c.page_count()).unwrap_or(0);
        Self::comic_spread_visual_pages_for(
            total,
            self.current_page_index as usize,
            self.comic_prefs.cover_first,
            self.comic_prefs.rtl_manga,
        )
    }

    /// Returns 0-based logical `(first_idx, optional_second_idx)` for page counter pills.
    pub(super) fn comic_spread_logical_range(&self) -> (usize, Option<usize>) {
        let total = self.comic_doc.as_ref().map(|c| c.page_count()).unwrap_or(0);
        Self::comic_spread_logical_range_for(
            total,
            self.current_page_index as usize,
            self.comic_prefs.layout_mode,
            self.comic_prefs.cover_first,
        )
    }

    /// Steps forward (`logical_delta > 0`) or backward (`logical_delta < 0`) through comic pages.
    pub(super) fn comic_step_page(&mut self, logical_delta: isize) {
        let total = self.comic_doc.as_ref().map(|c| c.page_count()).unwrap_or(0);
        if total == 0 {
            return;
        }
        let next = Self::comic_step_index(
            total,
            self.current_page_index as usize,
            self.comic_prefs.layout_mode,
            self.comic_prefs.cover_first,
            logical_delta,
        );
        let next_u16 = next.min(u16::MAX as usize) as u16;
        if next_u16 != self.current_page_index {
            self.current_page_index = next_u16;
            self.scroll_y = 0.0;
            self.pan_x = 0.0;
            self.update_title_bar();
            self.request_redraw();
        }
    }

    fn compute_comic_fitted_size(
        src_w: u32,
        src_h: u32,
        avail_w: f32,
        avail_h: f32,
        fit_mode: ComicFitMode,
        zoom: f32,
    ) -> (u32, u32) {
        let sw = src_w.max(1) as f32;
        let sh = src_h.max(1) as f32;
        let aw = avail_w.max(32.0);
        let ah = avail_h.max(32.0);
        let base_scale = match fit_mode {
            ComicFitMode::FitPage => (aw / sw).min(ah / sh),
            ComicFitMode::FitWidth => aw / sw,
            ComicFitMode::FitHeight => ah / sh,
        };
        let scale = (base_scale * zoom).max(0.05);
        (
            (sw * scale).round().max(1.0) as u32,
            (sh * scale).round().max(1.0) as u32,
        )
    }

    fn comic_strip_page_size(
        &self,
        page_idx: usize,
        avail_w: f32,
        zoom: f32,
    ) -> (u32, u32) {
        let (sw, sh) = self
            .comic_page_dims
            .get(&page_idx)
            .copied()
            .or_else(|| {
                self.comic_page_cache
                    .get(&page_idx)
                    .map(|e| (e.bitmap.width.max(1), e.bitmap.height.max(1)))
            })
            .unwrap_or((1000, 1450));
        let target_w = (avail_w.max(64.0) * zoom).round().max(32.0) as u32;
        let target_h = ((target_w as f32) * (sh as f32 / sw as f32))
            .round()
            .max(32.0) as u32;
        (target_w, target_h)
    }

    pub(super) fn comic_max_scroll_y(&self, canvas_w: u32, canvas_h: u32, sf: f64) -> f32 {
        let Some(ref archive) = self.comic_doc else {
            return 0.0;
        };
        let total = archive.page_count();
        if total == 0 {
            return 0.0;
        }
        let s = sf as f32;
        let pad = (18.0 * s).round();
        let avail_w = (canvas_w as f32 - pad * 2.0).max(32.0);
        let avail_h = (canvas_h as f32 - pad * 2.0).max(32.0);

        match self.comic_prefs.layout_mode {
            ComicLayoutMode::VerticalStrip => {
                let gap = (12.0 * s).round();
                let mut total_h = pad;
                for idx in 0..total {
                    let (_, ph) = self.comic_strip_page_size(idx, avail_w, self.zoom_level);
                    total_h += ph as f32 + gap;
                }
                total_h += pad;
                (total_h - canvas_h as f32).max(0.0)
            }
            ComicLayoutMode::SinglePage => {
                let (_, bh) = self.page_draw_rect.2.max(1).pipe_dims(self.page_draw_rect.3);
                if self.page_draw_rect.3 > 0 {
                    ((bh as f32 + pad * 2.0) - canvas_h as f32).max(0.0)
                } else {
                    let _ = avail_h;
                    0.0
                }
            }
            ComicLayoutMode::TwoPageSpread => {
                let h1 = self.page_draw_rect.3;
                let h2 = self.second_page_draw_rect.map(|r| r.3).unwrap_or(0);
                let bh = h1.max(h2);
                if bh > 0 {
                    ((bh as f32 + pad * 2.0) - canvas_h as f32).max(0.0)
                } else {
                    0.0
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn blit_comic_bitmap(
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        bmp: &ComicPageBitmap,
        dst_x: i32,
        dst_y: i32,
        dst_w: u32,
        dst_h: u32,
        clip_x0: i32,
        clip_y0: i32,
        clip_x1: i32,
        clip_y1: i32,
        filter: ReadingFilter,
    ) {
        if dst_w == 0 || dst_h == 0 || bmp.width == 0 || bmp.height == 0 {
            return;
        }
        let vx0 = dst_x.max(clip_x0).max(0);
        let vy0 = dst_y.max(clip_y0).max(0);
        let vx1 = (dst_x + dst_w as i32).min(clip_x1).min(buf_w as i32);
        let vy1 = (dst_y + dst_h as i32).min(clip_y1).min(buf_h as i32);
        if vx0 >= vx1 || vy0 >= vy1 {
            return;
        }

        let sw = bmp.width;
        let sh = bmp.height;
        let scale_x = sw as f32 / dst_w as f32;
        let scale_y = sh as f32 / dst_h as f32;
        let fast_1to1 = dst_w == sw && dst_h == sh;
        let heavy_downscale = scale_x >= 2.0 || scale_y >= 2.0;

        for py in vy0..vy1 {
            let row_off = (py as usize) * (buf_w as usize);
            let rel_y = (py - dst_y) as f32;

            if fast_1to1 {
                let sy = (py - dst_y) as usize;
                let src_row = sy * sw as usize;
                for px in vx0..vx1 {
                    let sx = (px - dst_x) as usize;
                    let c = bmp.pixels[src_row + sx];
                    buffer[row_off + px as usize] = Self::apply_reading_filter_pixel(c, filter);
                }
            } else if heavy_downscale {
                // Crisp 2x2 / area-box sampling for sharp line art & screen-tones
                let sy0 = ((rel_y * scale_y).floor() as u32).min(sh - 1);
                let sy1 = (((rel_y + 1.0) * scale_y).ceil() as u32).clamp(sy0 + 1, sh);
                let sy_mid = (sy0 + sy1) / 2;
                for px in vx0..vx1 {
                    let rel_x = (px - dst_x) as f32;
                    let sx0 = ((rel_x * scale_x).floor() as u32).min(sw - 1);
                    let sx1 = (((rel_x + 1.0) * scale_x).ceil() as u32).clamp(sx0 + 1, sw);
                    let sx_mid = (sx0 + sx1) / 2;
                    // Fast 4-tap area sample inside the footprint (exact for 2x, anti-aliased for 4x+)
                    let c0 = bmp.pixels[(sy0 * sw + sx0) as usize];
                    let c1 = bmp.pixels[(sy0 * sw + sx_mid.min(sw - 1)) as usize];
                    let c2 = bmp.pixels[(sy_mid.min(sh - 1) * sw + sx0) as usize];
                    let c3 = bmp.pixels[(sy_mid.min(sh - 1) * sw + sx_mid.min(sw - 1)) as usize];
                    let r = (((c0 >> 16) & 0xFF)
                        + ((c1 >> 16) & 0xFF)
                        + ((c2 >> 16) & 0xFF)
                        + ((c3 >> 16) & 0xFF))
                        >> 2;
                    let g = (((c0 >> 8) & 0xFF)
                        + ((c1 >> 8) & 0xFF)
                        + ((c2 >> 8) & 0xFF)
                        + ((c3 >> 8) & 0xFF))
                        >> 2;
                    let b = ((c0 & 0xFF) + (c1 & 0xFF) + (c2 & 0xFF) + (c3 & 0xFF)) >> 2;
                    let col = (r << 16) | (g << 8) | b;
                    buffer[row_off + px as usize] = Self::apply_reading_filter_pixel(col, filter);
                }
            } else {
                // Bilinear interpolation for smooth scaling
                let fy = ((rel_y + 0.5) * scale_y - 0.5).clamp(0.0, (sh - 1) as f32);
                let y0 = fy.floor() as u32;
                let y1 = (y0 + 1).min(sh - 1);
                let wy_fp = ((fy - y0 as f32) * 256.0).round() as u32;
                let iwy_fp = 256 - wy_fp;

                for px in vx0..vx1 {
                    let rel_x = (px - dst_x) as f32;
                    let fx = ((rel_x + 0.5) * scale_x - 0.5).clamp(0.0, (sw - 1) as f32);
                    let x0 = fx.floor() as u32;
                    let x1 = (x0 + 1).min(sw - 1);
                    let wx_fp = ((fx - x0 as f32) * 256.0).round() as u32;
                    let iwx_fp = 256 - wx_fp;

                    let c00 = bmp.pixels[(y0 * sw + x0) as usize];
                    let c10 = bmp.pixels[(y0 * sw + x1) as usize];
                    let c01 = bmp.pixels[(y1 * sw + x0) as usize];
                    let c11 = bmp.pixels[(y1 * sw + x1) as usize];

                    let w00 = iwx_fp * iwy_fp;
                    let w10 = wx_fp * iwy_fp;
                    let w01 = iwx_fp * wy_fp;
                    let w11 = wx_fp * wy_fp;

                    let r = (((c00 >> 16) & 0xFF) * w00
                        + ((c10 >> 16) & 0xFF) * w10
                        + ((c01 >> 16) & 0xFF) * w01
                        + ((c11 >> 16) & 0xFF) * w11)
                        >> 16;
                    let g = (((c00 >> 8) & 0xFF) * w00
                        + ((c10 >> 8) & 0xFF) * w10
                        + ((c01 >> 8) & 0xFF) * w01
                        + ((c11 >> 8) & 0xFF) * w11)
                        >> 16;
                    let b = ((c00 & 0xFF) * w00
                        + (c10 & 0xFF) * w10
                        + (c01 & 0xFF) * w01
                        + (c11 & 0xFF) * w11)
                        >> 16;

                    let col = (r << 16) | (g << 8) | b;
                    buffer[row_off + px as usize] = Self::apply_reading_filter_pixel(col, filter);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_comic_view(
        &mut self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        status_bar_h: u32,
        scale_factor: f64,
    ) {
        let total = self.comic_doc.as_ref().map(|c| c.page_count()).unwrap_or(0);
        if total == 0 {
            self.page_draw_rect = (0, 0, 0, 0);
            self.second_page_draw_rect = None;
            self.comic_strip_rects.clear();
            return;
        }

        let s = scale_factor as f32;
        let canvas_x0 = sidebar_w as i32;
        let canvas_y0 = top_bars_h as i32;
        let canvas_x1 = buf_w as i32;
        let canvas_y1 = buf_h.saturating_sub(status_bar_h) as i32;
        let canvas_w = (canvas_x1 - canvas_x0).max(0) as u32;
        let canvas_h = (canvas_y1 - canvas_y0).max(0) as u32;
        if canvas_w == 0 || canvas_h == 0 {
            return;
        }

        let pad = if self.is_presentation || self.is_fullscreen {
            0.0
        } else {
            (18.0 * s).round()
        };
        let avail_w = (canvas_w as f32 - pad * 2.0).max(32.0);
        let avail_h = (canvas_h as f32 - pad * 2.0).max(32.0);
        let filter = self.reading_filter;

        self.comic_strip_rects.clear();

        match self.comic_prefs.layout_mode {
            ComicLayoutMode::SinglePage => {
                let page_idx = (self.current_page_index as usize).min(total - 1);
                let _ = self.ensure_comic_page_cached(page_idx);
                let Some(entry) = self.comic_page_cache.get(&page_idx) else {
                    return;
                };
                let (dw, dh) = Self::compute_comic_fitted_size(
                    entry.bitmap.width,
                    entry.bitmap.height,
                    avail_w,
                    avail_h,
                    self.comic_prefs.fit_mode,
                    self.zoom_level,
                );
                let max_pan_x = ((dw as f32 + pad * 2.0) - canvas_w as f32).max(0.0) * 0.5;
                self.pan_x = self.pan_x.clamp(-max_pan_x, max_pan_x);
                let max_sy = ((dh as f32 + pad * 2.0) - canvas_h as f32).max(0.0);
                self.scroll_y = self.scroll_y.clamp(0.0, max_sy);

                let dx = canvas_x0 + ((canvas_w as f32 - dw as f32) * 0.5 + self.pan_x).round() as i32;
                let dy = if max_sy > 0.0 {
                    canvas_y0 + pad.round() as i32 - self.scroll_y.round() as i32
                } else {
                    canvas_y0 + ((canvas_h as f32 - dh as f32) * 0.5).round() as i32
                };
                let rect = (dx, dy, dw, dh);
                self.page_draw_rect = rect;
                self.second_page_draw_rect = None;

                self.draw_page_frame_chrome(
                    buffer,
                    buf_w,
                    buf_h,
                    sidebar_w,
                    top_bars_h,
                    status_bar_h,
                    rect,
                    scale_factor,
                );
                if let Some(entry) = self.comic_page_cache.get(&page_idx) {
                    Self::blit_comic_bitmap(
                        buffer, buf_w, buf_h, &entry.bitmap, dx, dy, dw, dh,
                        canvas_x0, canvas_y0, canvas_x1, canvas_y1, filter,
                    );
                }
            }
            ComicLayoutMode::TwoPageSpread => {
                let (left_opt, right_opt) = self.comic_spread_visual_pages();
                let Some(l_idx) = left_opt else {
                    return;
                };
                let _ = self.ensure_comic_page_cached(l_idx);
                if let Some(r_idx) = right_opt {
                    let _ = self.ensure_comic_page_cached(r_idx);
                }

                let l_dims = self
                    .comic_page_cache
                    .get(&l_idx)
                    .map(|e| (e.bitmap.width, e.bitmap.height));
                let r_dims = right_opt.and_then(|ri| {
                    self.comic_page_cache
                        .get(&ri)
                        .map(|e| (e.bitmap.width, e.bitmap.height))
                });

                let Some((lw_src, lh_src)) = l_dims else {
                    return;
                };
                if let (Some(r_idx), Some((rw_src, rh_src))) = (right_opt, r_dims) {
                    let gutter = (4.0 * s).round().max(2.0);
                    let half_avail_w = ((avail_w - gutter) * 0.5).max(32.0);
                    let (ldw, ldh) = Self::compute_comic_fitted_size(
                        lw_src,
                        lh_src,
                        half_avail_w,
                        avail_h,
                        self.comic_prefs.fit_mode,
                        self.zoom_level,
                    );
                    let (rdw, rdh) = Self::compute_comic_fitted_size(
                        rw_src,
                        rh_src,
                        half_avail_w,
                        avail_h,
                        self.comic_prefs.fit_mode,
                        self.zoom_level,
                    );
                    let total_w = ldw as f32 + gutter + rdw as f32;
                    let max_h = ldh.max(rdh);
                    let max_pan_x = ((total_w + pad * 2.0) - canvas_w as f32).max(0.0) * 0.5;
                    self.pan_x = self.pan_x.clamp(-max_pan_x, max_pan_x);
                    let max_sy = ((max_h as f32 + pad * 2.0) - canvas_h as f32).max(0.0);
                    self.scroll_y = self.scroll_y.clamp(0.0, max_sy);

                    let start_x =
                        canvas_x0 + ((canvas_w as f32 - total_w) * 0.5 + self.pan_x).round() as i32;
                    let base_y = if max_sy > 0.0 {
                        canvas_y0 + pad.round() as i32 - self.scroll_y.round() as i32
                    } else {
                        canvas_y0 + ((canvas_h as f32 - max_h as f32) * 0.5).round() as i32
                    };
                    let l_y = base_y + ((max_h.saturating_sub(ldh)) / 2) as i32;
                    let r_x = start_x + ldw as i32 + gutter as i32;
                    let r_y = base_y + ((max_h.saturating_sub(rdh)) / 2) as i32;

                    let l_rect = (start_x, l_y, ldw, ldh);
                    let r_rect = (r_x, r_y, rdw, rdh);
                    self.page_draw_rect = l_rect;
                    self.second_page_draw_rect = Some(r_rect);

                    self.draw_page_frame_chrome(
                        buffer, buf_w, buf_h, sidebar_w, top_bars_h, status_bar_h, l_rect, scale_factor,
                    );
                    self.draw_page_frame_chrome(
                        buffer, buf_w, buf_h, sidebar_w, top_bars_h, status_bar_h, r_rect, scale_factor,
                    );
                    if let Some(le) = self.comic_page_cache.get(&l_idx) {
                        Self::blit_comic_bitmap(
                            buffer, buf_w, buf_h, &le.bitmap, l_rect.0, l_rect.1, l_rect.2, l_rect.3,
                            canvas_x0, canvas_y0, canvas_x1, canvas_y1, filter,
                        );
                    }
                    if let Some(re) = self.comic_page_cache.get(&r_idx) {
                        Self::blit_comic_bitmap(
                            buffer, buf_w, buf_h, &re.bitmap, r_rect.0, r_rect.1, r_rect.2, r_rect.3,
                            canvas_x0, canvas_y0, canvas_x1, canvas_y1, filter,
                        );
                    }
                    let spine_x = start_x + ldw as i32 + (gutter as i32) / 2;
                    super::ui_draw::draw_book_spine_crease(
                        buffer,
                        buf_w,
                        buf_h,
                        spine_x,
                        base_y.max(canvas_y0),
                        max_h.min((canvas_y1 - base_y.max(canvas_y0)).max(0) as u32),
                        (6.0 * s).round().max(3.0) as i32,
                    );
                } else {
                    let (dw, dh) = Self::compute_comic_fitted_size(
                        lw_src,
                        lh_src,
                        avail_w,
                        avail_h,
                        self.comic_prefs.fit_mode,
                        self.zoom_level,
                    );
                    let max_pan_x = ((dw as f32 + pad * 2.0) - canvas_w as f32).max(0.0) * 0.5;
                    self.pan_x = self.pan_x.clamp(-max_pan_x, max_pan_x);
                    let max_sy = ((dh as f32 + pad * 2.0) - canvas_h as f32).max(0.0);
                    self.scroll_y = self.scroll_y.clamp(0.0, max_sy);

                    let dx = canvas_x0 + ((canvas_w as f32 - dw as f32) * 0.5 + self.pan_x).round() as i32;
                    let dy = if max_sy > 0.0 {
                        canvas_y0 + pad.round() as i32 - self.scroll_y.round() as i32
                    } else {
                        canvas_y0 + ((canvas_h as f32 - dh as f32) * 0.5).round() as i32
                    };
                    let rect = (dx, dy, dw, dh);
                    self.page_draw_rect = rect;
                    self.second_page_draw_rect = None;
                    self.draw_page_frame_chrome(
                        buffer, buf_w, buf_h, sidebar_w, top_bars_h, status_bar_h, rect, scale_factor,
                    );
                    if let Some(le) = self.comic_page_cache.get(&l_idx) {
                        Self::blit_comic_bitmap(
                            buffer, buf_w, buf_h, &le.bitmap, dx, dy, dw, dh,
                            canvas_x0, canvas_y0, canvas_x1, canvas_y1, filter,
                        );
                    }
                }
            }
            ComicLayoutMode::VerticalStrip => {
                let gap = (12.0 * s).round();
                let max_sy = self.comic_max_scroll_y(canvas_w, canvas_h, scale_factor);
                self.scroll_y = self.scroll_y.clamp(0.0, max_sy);
                let mut cur_y = canvas_y0 as f32 + pad - self.scroll_y;
                let center_mid_y = (canvas_y0 + canvas_y1) as f32 * 0.5;
                let mut center_page = self.current_page_index as usize;
                let mut best_dist = f32::MAX;
                let mut max_w_seen = 0u32;

                let mut visible_indices: Vec<(usize, (i32, i32, u32, u32))> = Vec::new();
                for idx in 0..total {
                    let (pw, ph) = self.comic_strip_page_size(idx, avail_w, self.zoom_level);
                    max_w_seen = max_w_seen.max(pw);
                    let max_pan_x = ((pw as f32 + pad * 2.0) - canvas_w as f32).max(0.0) * 0.5;
                    let clamped_pan = self.pan_x.clamp(-max_pan_x, max_pan_x);
                    let dx = canvas_x0 + ((canvas_w as f32 - pw as f32) * 0.5 + clamped_pan).round() as i32;
                    let dy = cur_y.round() as i32;
                    let y_bot = dy + ph as i32;

                    if y_bot >= canvas_y0 && dy <= canvas_y1 {
                        visible_indices.push((idx, (dx, dy, pw, ph)));
                        let mid = dy as f32 + ph as f32 * 0.5;
                        let dist = (mid - center_mid_y).abs();
                        if dist < best_dist {
                            best_dist = dist;
                            center_page = idx;
                        }
                    }
                    cur_y += ph as f32 + gap;
                }
                let max_pan_x = ((max_w_seen as f32 + pad * 2.0) - canvas_w as f32).max(0.0) * 0.5;
                self.pan_x = self.pan_x.clamp(-max_pan_x, max_pan_x);
                self.current_page_index = center_page.min(u16::MAX as usize) as u16;
                self.second_page_draw_rect = None;
                self.page_draw_rect = visible_indices
                    .first()
                    .map(|(_, r)| *r)
                    .unwrap_or((0, 0, 0, 0));

                for (idx, rect) in visible_indices {
                    self.comic_strip_rects.push((idx, rect));
                    let _ = self.ensure_comic_page_cached(idx);
                    self.draw_page_frame_chrome(
                        buffer, buf_w, buf_h, sidebar_w, top_bars_h, status_bar_h, rect, scale_factor,
                    );
                    if let Some(entry) = self.comic_page_cache.get(&idx) {
                        Self::blit_comic_bitmap(
                            buffer, buf_w, buf_h, &entry.bitmap, rect.0, rect.1, rect.2, rect.3,
                            canvas_x0, canvas_y0, canvas_x1, canvas_y1, filter,
                        );
                    }
                }
            }
        }
    }

    pub(super) fn is_distinct_comic_mode(&self) -> bool {
        self.ui_skin == UiSkin::Distinct && self.is_comic_tab()
    }

    pub(super) fn comic_page_badge_text_for(
        total: usize,
        cur_idx: usize,
        layout_mode: ComicLayoutMode,
        cover_first: bool,
    ) -> String {
        if total == 0 {
            return "— / —".to_string();
        }
        let (r0, r1_opt) = Self::comic_spread_logical_range_for(total, cur_idx, layout_mode, cover_first);
        if let Some(r1) = r1_opt {
            format!("Pages {}–{} / {total}", r0 + 1, r1 + 1)
        } else {
            format!("Page {} / {total}", r0 + 1)
        }
    }

    pub(super) fn comic_page_badge_text(&self) -> String {
        let total = self.comic_doc.as_ref().map(|c| c.page_count()).unwrap_or(0);
        Self::comic_page_badge_text_for(
            total,
            self.current_page_index as usize,
            self.comic_prefs.layout_mode,
            self.comic_prefs.cover_first,
        )
    }

    fn comic_sidebar_metrics_for(
        has_meta: bool,
        total_pages: usize,
        content_bot: u32,
        top_bars_h: u32,
        sf: f64,
    ) -> (u32, u32, u32, i32, usize, usize) {
        let s = sf as f32;
        let header_h = if has_meta {
            (74.0 * s).round() as u32
        } else {
            (50.0 * s).round() as u32
        };
        let list_top_y = top_bars_h as i32 + header_h as i32 + (6.0 * s).round() as i32;
        let list_bot_y = content_bot as i32 - (6.0 * s).round() as i32;
        let thumb_card_h = (138.0 * s).round().max(96.0) as u32;
        let gap = (8.0 * s).round().max(4.0) as u32;
        let step_h = thumb_card_h + gap;
        let visible_rows = (((list_bot_y - list_top_y).max(0) as u32) / step_h.max(1)).max(1) as usize;
        (header_h, thumb_card_h, step_h, list_top_y, visible_rows, total_pages)
    }

    fn comic_sidebar_metrics(
        &self,
        content_bot: u32,
        top_bars_h: u32,
        sf: f64,
    ) -> (u32, u32, u32, i32, usize, usize) {
        let has_meta = self.comic_doc.as_ref().and_then(|c| c.info.as_ref()).is_some_and(|info| {
            info.series.is_some()
                || info.title.is_some()
                || info.writer.is_some()
                || info.publisher.is_some()
        });
        let total_pages = self.comic_doc.as_ref().map(|c| c.page_count()).unwrap_or(0);
        Self::comic_sidebar_metrics_for(has_meta, total_pages, content_bot, top_bars_h, sf)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_distinct_comic_sidebar(
        &mut self,
        buffer: &mut [u32],
        buf_w: u32,
        content_bot: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        sf: f64,
    ) {
        let buf_h = (buffer.len() / (buf_w.max(1) as usize)) as u32;
        if sidebar_w < 48 || content_bot <= top_bars_h {
            return;
        }
        let s = sf as f32;
        let theme = self.ui_theme();
        let (header_h, thumb_card_h, step_h, list_top_y, visible_rows, total_pages) =
            self.comic_sidebar_metrics(content_bot, top_bars_h, sf);

        // Sidebar panel background + right divider
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            0,
            top_bars_h as i32,
            sidebar_w.saturating_sub(1),
            content_bot.saturating_sub(top_bars_h),
            theme.sidebar_bg,
        );
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            sidebar_w.saturating_sub(1) as i32,
            top_bars_h as i32,
            1,
            content_bot.saturating_sub(top_bars_h),
            theme.divider,
        );

        // Header card with ComicInfo metadata or filename
        let hdr_y = top_bars_h as i32;
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            0,
            hdr_y,
            sidebar_w.saturating_sub(1),
            header_h,
            theme.sidebar_header_bg,
        );
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            0,
            hdr_y + header_h as i32 - 1,
            sidebar_w.saturating_sub(1),
            1,
            theme.divider,
        );

        let pad_x = (10.0 * s).round() as i32;
        let max_tx = sidebar_w as i32 - pad_x;
        let title_sz = (12.0 * s).clamp(10.0, 22.0);
        let sub_sz = (10.0 * s).clamp(8.5, 18.0);

        let dir_lbl = if self.comic_prefs.rtl_manga { "RTL" } else { "LTR" };
        let (primary_title, issue_line, creator_line) = if let Some(ref archive) = self.comic_doc {
            if let Some(ref info) = archive.info {
                let main_t = info
                    .series
                    .as_deref()
                    .or(info.title.as_deref())
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| self.filename_display());
                let mut issue_parts: Vec<String> = Vec::new();
                if let Some(ref num) = info.number {
                    issue_parts.push(format!("#{num}"));
                }
                if let Some(ref vol) = info.volume {
                    issue_parts.push(format!("Vol {vol}"));
                }
                if let Some(yr) = info.year {
                    issue_parts.push(yr.to_string());
                }
                issue_parts.push(format!("{total_pages}p"));
                issue_parts.push(dir_lbl.to_string());

                let creator = match (info.writer.as_deref(), info.publisher.as_deref()) {
                    (Some(w), Some(p)) => Some(format!("{w} · {p}")),
                    (Some(w), None) => Some(w.to_string()),
                    (None, Some(p)) => Some(p.to_string()),
                    (None, None) => info
                        .title
                        .as_deref()
                        .filter(|t| info.series.as_deref() != Some(*t))
                        .map(|t| t.to_string()),
                };
                (main_t, issue_parts.join(" · "), creator)
            } else {
                (
                    self.filename_display(),
                    format!("{total_pages} pages · {dir_lbl}"),
                    None,
                )
            }
        } else {
            ("Comic Archive".to_string(), "0p".to_string(), None)
        };

        let avail_text_w = (max_tx - pad_x).max(20) as f32;
        let trunc_title = truncate_ellipsis(&primary_title, avail_text_w, title_sz, true);
        draw_text_clipped(
            buffer,
            buf_w,
            buf_h,
            pad_x,
            hdr_y + (8.0 * s).round() as i32,
            max_tx,
            &trunc_title,
            title_sz,
            theme.text_primary,
            true,
        );
        let trunc_issue = truncate_ellipsis(&issue_line, avail_text_w, sub_sz, false);
        draw_text_clipped(
            buffer,
            buf_w,
            buf_h,
            pad_x,
            hdr_y + (26.0 * s).round() as i32,
            max_tx,
            &trunc_issue,
            sub_sz,
            theme.accent_soft_fg,
            false,
        );
        if let Some(creator) = creator_line {
            let trunc_c = truncate_ellipsis(&creator, avail_text_w, sub_sz, false);
            draw_text_clipped(
                buffer,
                buf_w,
                buf_h,
                pad_x,
                hdr_y + (44.0 * s).round() as i32,
                max_tx,
                &trunc_c,
                sub_sz,
                theme.text_muted,
                false,
            );
        }

        if total_pages == 0 {
            return;
        }

        let max_scroll = total_pages.saturating_sub(visible_rows);
        self.comic_sidebar_scroll = self.comic_sidebar_scroll.min(max_scroll);
        let start_idx = self.comic_sidebar_scroll;
        let end_idx = (start_idx + visible_rows + 1).min(total_pages);
        let (active_a, active_b) = self.comic_spread_logical_range();

        let scrollbar_reserve = if total_pages > visible_rows {
            (8.0 * s).round() as i32
        } else {
            0
        };
        let card_x = (10.0 * s).round() as i32;
        let card_w = (sidebar_w as i32 - card_x * 2 - scrollbar_reserve).max(40) as u32;
        let radius = (6.0 * s).clamp(3.0, 10.0);
        let footer_h = (22.0 * s).round().max(16.0) as u32;
        let img_pad = (6.0 * s).round().max(3.0) as u32;
        let img_box_w = card_w.saturating_sub(img_pad * 2).max(16);
        let img_box_h = thumb_card_h.saturating_sub(footer_h + img_pad * 2).max(16);
        let clip_y0 = list_top_y.max(top_bars_h as i32);
        let clip_y1 = content_bot as i32;
        let lbl_sz = (10.5 * s).clamp(8.5, 18.0);
        let filter = self.reading_filter;

        for idx in start_idx..end_idx {
            let rel = idx - start_idx;
            let cy = list_top_y + (rel as u32 * step_h) as i32;
            if cy >= clip_y1 || cy + (thumb_card_h as i32) <= clip_y0 {
                continue;
            }
            let is_active = idx == active_a || active_b == Some(idx);
            let is_hov = self.hovered_widget == Some(UiHit::ComicSidebar(ComicSidebarHit::PageThumb(idx)));
            let card_bg = if is_active {
                theme.active_row_bg
            } else if is_hov {
                theme.hover_bg
            } else {
                theme.surface_bg
            };

            fill_rounded_rect(buffer, buf_w, buf_h, card_x, cy, card_w, thumb_card_h, radius, card_bg);

            // Draw lazily decoded thumbnail scaled inside clipped card bounds
            let _ = self.ensure_comic_thumb_cached(idx, img_box_w.max(120), img_box_h.max(160));
            if let Some(entry) = self.comic_thumb_cache.get(&idx) {
                let sw = entry.bitmap.width.max(1) as f32;
                let sh = entry.bitmap.height.max(1) as f32;
                let fit = (img_box_w as f32 / sw).min(img_box_h as f32 / sh);
                let dw = (sw * fit).round().max(1.0) as u32;
                let dh = (sh * fit).round().max(1.0) as u32;
                let dx = card_x + img_pad as i32 + ((img_box_w.saturating_sub(dw)) / 2) as i32;
                let dy = cy + img_pad as i32 + ((img_box_h.saturating_sub(dh)) / 2) as i32;
                Self::blit_comic_bitmap(
                    buffer,
                    buf_w,
                    buf_h,
                    &entry.bitmap,
                    dx,
                    dy,
                    dw,
                    dh,
                    card_x + 1,
                    clip_y0.max(cy + 1),
                    card_x + card_w as i32 - 1,
                    clip_y1.min(cy + thumb_card_h as i32 - footer_h as i32),
                    filter,
                );
            }

            let page_lbl = format!("Page {}", idx + 1);
            let lbl_w = measure_text(&page_lbl, lbl_sz, is_active);
            let lbl_x = card_x + ((card_w as f32 - lbl_w) * 0.5).round().max(4.0) as i32;
            let lbl_y = cy + thumb_card_h as i32 - footer_h as i32;
            if lbl_y >= clip_y0 && lbl_y + footer_h as i32 <= clip_y1 {
                draw_text_vcenter(
                    buffer,
                    buf_w,
                    buf_h,
                    lbl_x,
                    lbl_y,
                    footer_h,
                    card_x + card_w as i32 - 4,
                    &page_lbl,
                    lbl_sz,
                    if is_active {
                        theme.accent_soft_fg
                    } else {
                        theme.text_secondary
                    },
                    is_active,
                );
            }

            if is_active {
                stroke_rounded_rect(buffer, buf_w, buf_h, card_x, cy, card_w, thumb_card_h, radius, theme.accent);
                stroke_rounded_rect(
                    buffer,
                    buf_w,
                    buf_h,
                    card_x + 1,
                    cy + 1,
                    card_w.saturating_sub(2),
                    thumb_card_h.saturating_sub(2),
                    (radius - 1.0).max(1.0),
                    theme.accent,
                );
            } else {
                stroke_rounded_rect(
                    buffer,
                    buf_w,
                    buf_h,
                    card_x,
                    cy,
                    card_w,
                    thumb_card_h,
                    radius,
                    if is_hov { theme.accent } else { theme.border_subtle },
                );
            }
        }

        // Thin vertical scrollbar when thumbnails overflow
        if total_pages > visible_rows {
            let sb_x = sidebar_w as i32 - (6.0 * s).round().max(4.0) as i32;
            let sb_w = (4.0 * s).round().max(3.0) as u32;
            let track_h = (clip_y1 - clip_y0).max(20) as u32;
            let thumb_h = ((visible_rows as f32 / total_pages as f32) * track_h as f32)
                .round()
                .clamp(20.0, track_h as f32) as u32;
            let ratio = if max_scroll > 0 {
                start_idx as f32 / max_scroll as f32
            } else {
                0.0
            };
            let thumb_y = clip_y0 + ((track_h - thumb_h) as f32 * ratio).round() as i32;
            fill_rounded_rect(buffer, buf_w, buf_h, sb_x, thumb_y, sb_w, thumb_h, 2.0, theme.divider);
        }
    }

    pub(super) fn hit_test_comic_sidebar_at(
        mx: f64,
        my: f64,
        sidebar_w: u32,
        content_bot: u32,
        top_bars_h: u32,
        sf: f64,
        has_meta: bool,
        total_pages: usize,
        sidebar_scroll: usize,
    ) -> Option<ComicSidebarHit> {
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix < 0 || ix >= sidebar_w as i32 || iy < top_bars_h as i32 || iy >= content_bot as i32 {
            return None;
        }
        let (_, thumb_card_h, step_h, list_top_y, visible_rows, total) =
            Self::comic_sidebar_metrics_for(has_meta, total_pages, content_bot, top_bars_h, sf);
        if total == 0 || iy < list_top_y {
            return None;
        }
        let max_scroll = total.saturating_sub(visible_rows);
        let start_idx = sidebar_scroll.min(max_scroll);
        let dy = (iy - list_top_y) as u32;
        let rel = (dy / step_h.max(1)) as usize;
        let within_card = (dy % step_h.max(1)) <= thumb_card_h;
        let idx = start_idx + rel;
        if within_card && idx < total {
            Some(ComicSidebarHit::PageThumb(idx))
        } else {
            None
        }
    }

    pub(super) fn hit_test_distinct_comic_sidebar(
        &self,
        mx: f64,
        my: f64,
        sidebar_w: u32,
        content_bot: u32,
        top_bars_h: u32,
        sf: f64,
    ) -> Option<ComicSidebarHit> {
        let has_meta = self.comic_doc.as_ref().and_then(|c| c.info.as_ref()).is_some_and(|info| {
            info.series.is_some()
                || info.title.is_some()
                || info.writer.is_some()
                || info.publisher.is_some()
        });
        let total_pages = self.comic_doc.as_ref().map(|c| c.page_count()).unwrap_or(0);
        Self::hit_test_comic_sidebar_at(
            mx,
            my,
            sidebar_w,
            content_bot,
            top_bars_h,
            sf,
            has_meta,
            total_pages,
            self.comic_sidebar_scroll,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn comic_bottom_bar_rects_for(
        buf_w: u32,
        buf_h: u32,
        status_bar_h: u32,
        sf: f64,
        show_sidebar: bool,
        is_fullscreen: bool,
        jump_active: bool,
        page_txt: String,
        prefs: &ComicPrefs,
        reading_filter: ReadingFilter,
    ) -> Vec<(ComicBottomHit, i32, i32, u32, u32, Option<UiIcon>, Option<String>, bool)> {
        let mut items = Vec::new();
        if status_bar_h == 0 || buf_w < 160 {
            return items;
        }
        let s = sf as f32;
        let bar_y = buf_h.saturating_sub(status_bar_h) as i32;
        let pad_x = (10.0 * s).round().max(6.0) as i32;
        let btn_h = (22.0 * s).round().clamp(16.0, (status_bar_h.saturating_sub(4) as f32).max(16.0)) as u32;
        let btn_y = bar_y + ((status_bar_h.saturating_sub(btn_h)) / 2) as i32;
        let sq_w = btn_h + (4.0 * s).round() as u32;
        let gap = (6.0 * s).round().max(4.0) as i32;
        let font_sz = (10.5 * s).clamp(8.5, 19.0);

        let pill_w = |txt: &str, min_w: f32| -> u32 {
            (measure_text(txt, font_sz, false).ceil() as u32 + (14.0 * s).round() as u32)
                .max((min_w * s).round() as u32)
        };

        // Left side: Sidebar + Page pill
        let mut lx = pad_x;
        items.push((
            ComicBottomHit::ToggleSidebar,
            lx,
            btn_y,
            sq_w,
            btn_h,
            Some(UiIcon::Sidebar),
            None,
            show_sidebar,
        ));
        lx += sq_w as i32 + gap;

        let page_w = pill_w(&page_txt, 74.0);
        items.push((
            ComicBottomHit::PageJump,
            lx,
            btn_y,
            page_w,
            btn_h,
            None,
            Some(page_txt),
            jump_active,
        ));
        lx += page_w as i32 + gap * 2;

        // Right side: RTL, Layout, Fit, Filter, Fullscreen
        let rtl_txt = if prefs.rtl_manga { "RTL" } else { "LTR" }.to_string();
        let layout_txt = prefs.layout_mode.short_label().to_string();
        let fit_txt = prefs.fit_mode.label().to_string();
        let filter_txt = reading_filter.label().to_string();

        let rtl_w = pill_w(&rtl_txt, 38.0);
        let layout_w = pill_w(&layout_txt, 54.0);
        let fit_w = pill_w(&fit_txt, 56.0);
        let filter_w = pill_w(&filter_txt, 54.0);

        let right_total = rtl_w as i32
            + gap
            + layout_w as i32
            + gap
            + fit_w as i32
            + gap
            + filter_w as i32
            + gap
            + sq_w as i32
            + pad_x;

        let mut rx = (buf_w as i32 - right_total).max(lx + (70.0 * s).round() as i32);
        let track_x0 = lx;
        let track_x1 = (rx - gap * 2).max(track_x0 + 24);

        // Progress scrubber rect
        items.push((
            ComicBottomHit::Scrubber(0.0),
            track_x0,
            bar_y,
            (track_x1 - track_x0).max(1) as u32,
            status_bar_h,
            None,
            None,
            false,
        ));

        let right_defs = [
            (ComicBottomHit::ToggleRtl, rtl_w, None, Some(rtl_txt), prefs.rtl_manga),
            (
                ComicBottomHit::CycleLayout,
                layout_w,
                None,
                Some(layout_txt),
                prefs.layout_mode != ComicLayoutMode::SinglePage,
            ),
            (ComicBottomHit::CycleFit, fit_w, None, Some(fit_txt), false),
            (
                ComicBottomHit::ToggleFilter,
                filter_w,
                None,
                Some(filter_txt),
                reading_filter != ReadingFilter::Normal,
            ),
            (ComicBottomHit::ToggleFullscreen, sq_w, Some(UiIcon::Fullscreen), None, is_fullscreen),
        ];
        for (hit, w, icon, lbl, active) in right_defs {
            if rx + w as i32 <= buf_w as i32 {
                items.push((hit, rx, btn_y, w, btn_h, icon, lbl, active));
            }
            rx += w as i32 + gap;
        }
        items
    }

    fn comic_bottom_bar_rects(
        &self,
        buf_w: u32,
        buf_h: u32,
        status_bar_h: u32,
        sf: f64,
    ) -> Vec<(ComicBottomHit, i32, i32, u32, u32, Option<UiIcon>, Option<String>, bool)> {
        Self::comic_bottom_bar_rects_for(
            buf_w,
            buf_h,
            status_bar_h,
            sf,
            self.show_bookmarks,
            self.is_fullscreen,
            matches!(self.input_mode, InputMode::JumpToPage { .. }),
            self.comic_page_badge_text(),
            &self.comic_prefs,
            self.reading_filter,
        )
    }

    pub(super) fn comic_scrubber_track_x(&self, buf_w: u32, status_bar_h: u32, sf: f64) -> Option<(i32, i32)> {
        let rects = self.comic_bottom_bar_rects(buf_w, status_bar_h + 100, status_bar_h, sf);
        rects
            .into_iter()
            .find(|(hit, ..)| matches!(hit, ComicBottomHit::Scrubber(_)))
            .map(|(_, x, _, w, ..)| (x, x + w as i32))
    }

    pub(super) fn comic_bottom_widget_rect(
        &self,
        target: ComicBottomHit,
        buf_w: u32,
        buf_h: u32,
        status_bar_h: u32,
        sf: f64,
    ) -> Option<(i32, i32, u32, u32)> {
        let rects = self.comic_bottom_bar_rects(buf_w, buf_h, status_bar_h, sf);
        rects
            .into_iter()
            .find(|(hit, ..)| std::mem::discriminant(hit) == std::mem::discriminant(&target))
            .map(|(_, x, y, w, h, ..)| (x, y, w, h))
    }

    pub(super) fn draw_distinct_comic_bottom_bar(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        status_bar_h: u32,
        sf: f64,
    ) {
        if status_bar_h == 0 || buf_w == 0 || buf_h < status_bar_h {
            return;
        }
        let s = sf as f32;
        let theme = self.ui_theme();
        let bar_y = buf_h.saturating_sub(status_bar_h) as i32;
        fill_rect_clipped(buffer, buf_w, buf_h, 0, bar_y, buf_w, status_bar_h, theme.statusbar_bg);
        fill_rect_clipped(buffer, buf_w, buf_h, 0, bar_y, buf_w, 1, theme.divider);

        let items = self.comic_bottom_bar_rects(buf_w, buf_h, status_bar_h, sf);
        let rad = (5.0 * s).clamp(3.0, 9.0);
        let font_sz = (10.5 * s).clamp(8.5, 19.0);
        let ic_sz = (13.5 * s).round().clamp(11.0, 22.0) as u32;

        let total = self.comic_doc.as_ref().map(|c| c.page_count()).unwrap_or(0);
        let progress_norm = if total <= 1 {
            0.0
        } else {
            (self.current_page_index as f32 / (total - 1) as f32).clamp(0.0, 1.0)
        };

        for (hit, x, y, w, h, icon_opt, lbl_opt, active) in items {
            if matches!(hit, ComicBottomHit::Scrubber(_)) {
                let track_h = (5.0 * s).round().clamp(4.0, 8.0) as u32;
                let track_y = y + ((status_bar_h.saturating_sub(track_h)) / 2) as i32;
                let track_rad = (track_h as f32) * 0.5;
                fill_rounded_rect(buffer, buf_w, buf_h, x, track_y, w, track_h, track_rad, theme.divider);

                let visual_ratio = if self.comic_prefs.rtl_manga {
                    1.0 - progress_norm
                } else {
                    progress_norm
                };
                let knob_x = x as f32 + (w as f32) * visual_ratio;
                if self.comic_prefs.rtl_manga {
                    let fx = knob_x.round() as i32;
                    let fw = (x + w as i32 - fx).max(0) as u32;
                    if fw > 0 {
                        fill_rounded_rect(buffer, buf_w, buf_h, fx, track_y, fw, track_h, track_rad, theme.accent);
                    }
                } else {
                    let fw = (knob_x - x as f32).round().max(0.0) as u32;
                    if fw > 0 {
                        fill_rounded_rect(buffer, buf_w, buf_h, x, track_y, fw, track_h, track_rad, theme.accent);
                    }
                }
                let hovered_scrub = self.comic_scrubber_drag
                    || matches!(self.hovered_widget, Some(UiHit::ComicBottom(ComicBottomHit::Scrubber(_))));
                let knob_r = if hovered_scrub {
                    (6.5 * s).clamp(5.0, 11.0)
                } else {
                    (5.0 * s).clamp(4.0, 9.0)
                };
                let knob_cy = track_y as f32 + (track_h as f32) * 0.5;
                fill_circle_aa(buffer, buf_w, buf_h, knob_x, knob_cy, knob_r + 1.2, theme.surface_bg);
                fill_circle_aa(buffer, buf_w, buf_h, knob_x, knob_cy, knob_r, theme.accent);
                continue;
            }

            let hovered = self.hovered_widget == Some(UiHit::ComicBottom(hit));
            let bg = if active {
                theme.accent_soft_bg
            } else if hovered {
                theme.hover_bg
            } else {
                theme.surface_bg
            };
            let fg = if active {
                theme.accent_soft_fg
            } else {
                theme.text_primary
            };
            fill_rounded_rect(buffer, buf_w, buf_h, x, y, w, h, rad, bg);
            stroke_rounded_rect(
                buffer,
                buf_w,
                buf_h,
                x,
                y,
                w,
                h,
                rad,
                if active { theme.accent } else { theme.border_subtle },
            );
            if let Some(ic) = icon_opt {
                let ix = x + ((w.saturating_sub(ic_sz)) / 2) as i32;
                let iy = y + ((h.saturating_sub(ic_sz)) / 2) as i32;
                draw_icon(buffer, buf_w, buf_h, ic, ix, iy, ic_sz, fg);
            } else if let Some(lbl) = lbl_opt {
                let tw = measure_text(&lbl, font_sz, active);
                let tx = x + ((w as f32 - tw) * 0.5).round().max(4.0) as i32;
                draw_text_vcenter(buffer, buf_w, buf_h, tx, y, h, x + w as i32 - 3, &lbl, font_sz, fg, active);
            }
        }
    }

    fn hit_test_comic_bottom_rects(
        mx: f64,
        my: f64,
        items: &[(ComicBottomHit, i32, i32, u32, u32, Option<UiIcon>, Option<String>, bool)],
    ) -> Option<ComicBottomHit> {
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        for (hit, x, y, w, h, ..) in items {
            if ix >= *x && ix < *x + *w as i32 && iy >= *y && iy < *y + *h as i32 {
                if matches!(hit, ComicBottomHit::Scrubber(_)) {
                    let ratio = ((mx as f32 - *x as f32) / ((*w).max(1) as f32)).clamp(0.0, 1.0);
                    return Some(ComicBottomHit::Scrubber(ratio));
                }
                return Some(*hit);
            }
        }
        None
    }

    pub(super) fn hit_test_distinct_comic_bottom_bar(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        status_bar_h: u32,
        sf: f64,
    ) -> Option<ComicBottomHit> {
        let items = self.comic_bottom_bar_rects(buf_w, buf_h, status_bar_h, sf);
        Self::hit_test_comic_bottom_rects(mx, my, &items)
    }

    pub(super) fn jump_comic_to_progress_ratio(
        &mut self,
        visual_ratio: f32,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) {
        let total = self.comic_doc.as_ref().map(|c| c.page_count()).unwrap_or(0);
        if total == 0 {
            return;
        }
        let r = visual_ratio.clamp(0.0, 1.0);
        let norm = if self.comic_prefs.rtl_manga { 1.0 - r } else { r };
        let target_idx = ((total - 1) as f32 * norm).round() as u16;
        self.current_page_index = target_idx;
        if self.comic_prefs.layout_mode == ComicLayoutMode::VerticalStrip {
            let sidebar_w = self.sidebar_width_px(sf);
            let top_bars_h = self.top_bars_height_px(sf);
            let status_bar_h = self.status_bar_height_px(sf);
            let canvas_w = buf_w.saturating_sub(sidebar_w).max(1);
            let canvas_h = buf_h.saturating_sub(top_bars_h + status_bar_h).max(1);
            let max_s = self.comic_max_scroll_y(canvas_w, canvas_h, sf);
            self.scroll_y = max_s * norm;
        } else {
            self.scroll_y = 0.0;
        }
        self.pan_x = 0.0;
        self.update_title_bar();
        self.request_redraw();
    }

    pub(super) fn handle_distinct_comic_bottom_click(
        &mut self,
        hit: ComicBottomHit,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) {
        match hit {
            ComicBottomHit::ToggleSidebar => {
                self.show_bookmarks = !self.show_bookmarks;
                self.status_notice = Some(if self.show_bookmarks {
                    "Page Thumbnails Sidebar ON".to_string()
                } else {
                    "Page Thumbnails Sidebar OFF".to_string()
                });
            }
            ComicBottomHit::PageJump => {
                if self.is_comic_tab() {
                    self.input_mode = InputMode::JumpToPage { input: String::new() };
                }
            }
            ComicBottomHit::ToggleRtl => {
                self.comic_prefs.rtl_manga = !self.comic_prefs.rtl_manga;
                self.save_session_to_ini();
                self.status_notice = Some(if self.comic_prefs.rtl_manga {
                    "Comic Reading Direction: Right-to-Left (Manga)".to_string()
                } else {
                    "Comic Reading Direction: Left-to-Right".to_string()
                });
            }
            ComicBottomHit::CycleLayout => {
                self.comic_prefs.layout_mode = self.comic_prefs.layout_mode.next();
                self.scroll_y = 0.0;
                self.pan_x = 0.0;
                self.save_session_to_ini();
                self.status_notice = Some(format!("Comic Layout: {}", self.comic_prefs.layout_mode.label()));
            }
            ComicBottomHit::CycleFit => {
                self.comic_prefs.fit_mode = self.comic_prefs.fit_mode.next();
                self.zoom_level = 1.0;
                self.scroll_y = 0.0;
                self.pan_x = 0.0;
                self.save_session_to_ini();
                self.status_notice = Some(format!("Comic Fit: {}", self.comic_prefs.fit_mode.label()));
            }
            ComicBottomHit::ToggleFilter => {
                self.set_reading_filter(self.reading_filter.next());
                return;
            }
            ComicBottomHit::ToggleFullscreen => {
                self.toggle_fullscreen();
            }
            ComicBottomHit::Scrubber(ratio) => {
                self.jump_comic_to_progress_ratio(ratio, buf_w, buf_h, sf);
                return;
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }
}

trait PipeDims {
    fn pipe_dims(self, h: u32) -> (u32, u32);
}

impl PipeDims for u32 {
    fn pipe_dims(self, h: u32) -> (u32, u32) {
        (self, h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_comic_prefs_ini_roundtrip_and_blitter() {
        let mut cfg = SessionConfig::default();
        cfg.comic_prefs = ComicPrefs {
            rtl_manga: true,
            layout_mode: ComicLayoutMode::TwoPageSpread,
            cover_first: false,
            fit_mode: ComicFitMode::FitWidth,
        };
        let ini = PdfViewerApp::serialize_session_ini(&cfg);
        assert!(ini.contains("comic_rtl=1"));
        assert!(ini.contains("comic_layout=spread"));
        assert!(ini.contains("comic_cover_first=0"));
        assert!(ini.contains("comic_fit=width"));

        let parsed = PdfViewerApp::parse_session_ini(&ini);
        assert_eq!(parsed.comic_prefs, cfg.comic_prefs);

        // Blitter scaling + reading filter + clip bounds
        let bmp = ComicPageBitmap {
            width: 4,
            height: 4,
            pixels: vec![0x00FF_0000; 16],
        };
        let mut buf = vec![0x0000_0000u32; 20 * 20];
        PdfViewerApp::blit_comic_bitmap(
            &mut buf, 20, 20, &bmp, 2, 2, 8, 8, 4, 4, 10, 10, ReadingFilter::Normal,
        );
        // Inside clip [4..10, 4..10] should be red; outside should remain black
        assert_eq!(buf[5 * 20 + 5], 0x00FF_0000);
        assert_eq!(buf[2 * 20 + 2], 0x0000_0000);
    }

    #[test]
    fn test_comic_spread_rtl_step_and_hit_testing() {
        // Cover-first on page 0 shows solo cover
        assert_eq!(PdfViewerApp::comic_spread_visual_pages_for(6, 0, true, false), (Some(0), None));
        assert_eq!(
            PdfViewerApp::comic_spread_logical_range_for(6, 0, ComicLayoutMode::TwoPageSpread, true),
            (0, None)
        );
        assert_eq!(
            PdfViewerApp::comic_page_badge_text_for(6, 0, ComicLayoutMode::TwoPageSpread, true),
            "Page 1 / 6"
        );

        // Stepping forward lands on spread (1, 2)
        let next = PdfViewerApp::comic_step_index(6, 0, ComicLayoutMode::TwoPageSpread, true, 1);
        assert_eq!(next, 1);
        assert_eq!(PdfViewerApp::comic_spread_visual_pages_for(6, next, true, false), (Some(1), Some(2)));
        assert_eq!(
            PdfViewerApp::comic_page_badge_text_for(6, next, ComicLayoutMode::TwoPageSpread, true),
            "Pages 2–3 / 6"
        );

        // RTL swaps visual order to (right=2, left=1)
        assert_eq!(PdfViewerApp::comic_spread_visual_pages_for(6, next, true, true), (Some(2), Some(1)));

        // Stepping backward from spread (1, 2) returns to cover 0
        assert_eq!(PdfViewerApp::comic_step_index(6, 1, ComicLayoutMode::TwoPageSpread, true, -1), 0);

        // Bottom bar hit testing (Sidebar, Page pill, Scrubber, RTL toggle)
        let rects = PdfViewerApp::comic_bottom_bar_rects_for(
            1000,
            700,
            28,
            1.0,
            true,
            false,
            false,
            "Pages 2–3 / 6".to_string(),
            &ComicPrefs {
                rtl_manga: true,
                layout_mode: ComicLayoutMode::TwoPageSpread,
                cover_first: true,
                fit_mode: ComicFitMode::FitPage,
            },
            ReadingFilter::Normal,
        );
        let scrubber_hit = PdfViewerApp::hit_test_comic_bottom_rects(300.0, 686.0, &rects);
        assert!(matches!(scrubber_hit, Some(ComicBottomHit::Scrubber(_))));
        let sb_btn_hit = PdfViewerApp::hit_test_comic_bottom_rects(20.0, 686.0, &rects);
        assert_eq!(sb_btn_hit, Some(ComicBottomHit::ToggleSidebar));

        // Sidebar thumbnail hit testing on first card
        let sb_hit = PdfViewerApp::hit_test_comic_sidebar_at(60.0, 140.0, 220, 672, 56, 1.0, true, 6, 0);
        assert_eq!(sb_hit, Some(ComicSidebarHit::PageThumb(0)));
    }
}

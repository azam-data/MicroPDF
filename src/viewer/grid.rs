use crate::engine::PdfEngine;
use anyhow::Result;
use super::state::*;
use super::render::{draw_bitmap_str_8x8, draw_rect_border, fill_rect_clipped};

impl<'a> PdfViewerApp<'a> {
    /// Renders a single 140x198 thumbnail for `page_idx` into `self.thumbnail_cache` if not already cached.
    pub(super) fn ensure_thumbnail_rendered(&mut self, page_idx: u16) -> Result<()> {
        if self.thumbnail_cache.contains_key(&page_idx) {
            return Ok(());
        }

        let Some(doc) = self.doc.as_ref() else {
            return Ok(());
        };
        let page = doc.pages().get(page_idx)?;
        let pw = page.width().value.max(1.0);
        let ph = page.height().value.max(1.0);

        let scale = (THUMB_W as f32 / pw).min(THUMB_H as f32 / ph);
        let tw = ((pw * scale).round() as i32).max(1);
        let th = ((ph * scale).round() as i32).max(1);

        let reused = std::mem::take(&mut self.spare_pixel_buf);
        let mut pixels = self.render_page_into_u32_vec(&page, tw, th, false, reused)?;
        drop(page);
        let max_thumb_pixels = (THUMB_W as usize) * (THUMB_H as usize);
        if pixels.capacity() > max_thumb_pixels {
            pixels.shrink_to_fit();
        }

        self.thumbnail_cache.insert(
            page_idx,
            ThumbnailEntry {
                width: tw as u32,
                height: th as u32,
                pixels,
            },
        );
        Ok(())
    }

    /// Evicts thumbnails outside the visible grid rows plus one screen above and below,
    /// enforcing a hard cap of `MAX_THUMB_CACHE_ENTRIES` (300 entries).
    pub(super) fn update_visible_thumbnails(
        &mut self,
        viewport_w: u32,
        viewport_h: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        status_bar_h: u32,
    ) {
        let total = self.total_pages();
        if total == 0 {
            self.thumbnail_cache.clear();
            return;
        }
        let screen_h = viewport_h.saturating_sub(top_bars_h + status_bar_h).max(1) as i32;
        let vis_top = top_bars_h as i32;
        let vis_bot = viewport_h.saturating_sub(status_bar_h) as i32;
        let keep_top = vis_top - screen_h;
        let keep_bot = vis_bot + screen_h;

        let mut keep_min = total;
        let mut keep_max = 0u16;
        let mut visible_indices = Vec::new();

        for idx in 0..total {
            let (_, ty, _, th) = self.thumbnail_screen_rect(idx, viewport_w, sidebar_w, top_bars_h);
            let cell_bot = ty + (th as i32) + 30;
            if cell_bot >= keep_top && ty <= keep_bot {
                keep_min = keep_min.min(idx);
                keep_max = keep_max.max(idx);
            }
            if cell_bot >= vis_top && ty <= vis_bot {
                visible_indices.push(idx);
            }
        }

        let mut evicted_bufs: Vec<Vec<u32>> = Vec::new();
        if keep_min <= keep_max {
            let count = (keep_max - keep_min + 1) as usize;
            if count > MAX_THUMB_CACHE_ENTRIES {
                let mid = if !visible_indices.is_empty() {
                    visible_indices[visible_indices.len() / 2]
                } else {
                    keep_min + (keep_max - keep_min) / 2
                };
                let half = (MAX_THUMB_CACHE_ENTRIES / 2) as u16;
                keep_min = mid.saturating_sub(half);
                keep_max = (keep_min + (MAX_THUMB_CACHE_ENTRIES as u16) - 1).min(total - 1);
            }
            let out_of_range: Vec<u16> = self
                .thumbnail_cache
                .keys()
                .copied()
                .filter(|&idx| idx < keep_min || idx > keep_max)
                .collect();
            for k in out_of_range {
                if let Some(entry) = self.thumbnail_cache.remove(&k) {
                    evicted_bufs.push(entry.pixels);
                }
            }
        } else {
            self.thumbnail_cache.clear();
        }

        for idx in visible_indices {
            if idx >= keep_min && idx <= keep_max {
                if !self.thumbnail_cache.contains_key(&idx) {
                    if self.spare_pixel_buf.is_empty() {
                        if let Some(buf) = evicted_bufs.pop() {
                            self.spare_pixel_buf = buf;
                        }
                    }
                    let _ = self.ensure_thumbnail_rendered(idx);
                }
            }
        }

        while self.thumbnail_cache.len() > MAX_THUMB_CACHE_ENTRIES {
            let center = self.current_page_index as i32;
            if let Some(&farthest) = self
                .thumbnail_cache
                .keys()
                .max_by_key(|&&k| (k as i32 - center).abs())
            {
                self.thumbnail_cache.remove(&farthest);
            } else {
                break;
            }
        }
    }

    /// Computes grid layout metrics for Thumbnail Grid View.
    pub(super) fn grid_metrics(
        &self,
        viewport_w: u32,
        sidebar_w: u32,
        top_bars_h: u32,
    ) -> (u32, u32, u32, u32, u32) {
        let content_w = viewport_w.saturating_sub(sidebar_w).max(1);
        let pad_x = 28u32;
        let pad_y = 36u32;
        let cell_w = THUMB_W + pad_x;
        let cell_h = THUMB_H + pad_y;
        let cols = ((content_w.saturating_sub(pad_x)) / cell_w).max(1);
        let grid_total_w = cols * cell_w;
        let start_x = sidebar_w + content_w.saturating_sub(grid_total_w) / 2 + pad_x / 2;
        let start_y = top_bars_h + 20u32;
        (cols, cell_w, cell_h, start_x, start_y)
    }

    /// Returns the screen rectangle `(x, y, THUMB_W, THUMB_H)` for `page_idx` in Thumbnail Grid View.
    pub(super) fn thumbnail_screen_rect(
        &self,
        page_idx: u16,
        viewport_w: u32,
        sidebar_w: u32,
        top_bars_h: u32,
    ) -> (i32, i32, u32, u32) {
        let (cols, cell_w, cell_h, start_x, start_y) =
            self.grid_metrics(viewport_w, sidebar_w, top_bars_h);
        let col = (page_idx as u32) % cols;
        let row = (page_idx as u32) / cols;
        let x = (start_x + col * cell_w) as i32;
        let y = (start_y + row * cell_h) as i32 - (self.grid_scroll_y.round() as i32);
        (x, y, THUMB_W, THUMB_H)
    }

    /// Hit-tests which thumbnail (and whether its top-left `[BW]` or top-right `[R]` badge was clicked) is at `(mx, my)`.
    pub(super) fn hit_test_thumbnail(
        &self,
        mx: f64,
        my: f64,
        viewport_w: u32,
        sidebar_w: u32,
        top_bars_h: u32,
    ) -> Option<(u16, ThumbBadgeHit)> {
        if (mx as u32) < sidebar_w || (my as u32) < top_bars_h {
            return None;
        }
        let total = self.total_pages();
        let ix = mx.round() as i32;
        let iy = my.round() as i32;

        for idx in 0..total {
            let (tx, ty, tw, th) =
                self.thumbnail_screen_rect(idx, viewport_w, sidebar_w, top_bars_h);
            if ix >= tx && ix < tx + (tw as i32) && iy >= ty && iy < ty + (th as i32) + 22 {
                // Top-left `[BW]` badge is 28x18 at `(tx, ty)`
                let on_bw_badge = ix >= tx && ix <= tx + 28 && iy >= ty && iy <= ty + 18;
                if on_bw_badge {
                    return Some((idx, ThumbBadgeHit::BwConvert));
                }
                // Top-right `[R]` badge is 24x18 at `(tx + tw - 24, ty)`
                let badge_x = tx + (tw as i32) - 24;
                let badge_y = ty;
                let on_rotate_badge =
                    ix >= badge_x && ix <= tx + (tw as i32) && iy >= badge_y && iy <= badge_y + 18;
                if on_rotate_badge {
                    return Some((idx, ThumbBadgeHit::Rotate));
                }
                return Some((idx, ThumbBadgeHit::None));
            }
        }
        None
    }

    /// Determines the target page index when dropping an item (reorder or external file drop) at `(mx, my)` in Grid View.
    pub(super) fn grid_drop_target_index(
        &self,
        mx: f64,
        my: f64,
        viewport_w: u32,
        sidebar_w: u32,
        top_bars_h: u32,
    ) -> u16 {
        let total = self.total_pages();
        if total == 0 {
            return 0;
        }
        if let Some((idx, _)) = self.hit_test_thumbnail(mx, my, viewport_w, sidebar_w, top_bars_h) {
            return idx;
        }
        let (cols, cell_w, cell_h, start_x, start_y) =
            self.grid_metrics(viewport_w, sidebar_w, top_bars_h);
        let rel_x = (mx as i32 - start_x as i32).max(0) as u32;
        let rel_y = (my as i32 + self.grid_scroll_y.round() as i32 - start_y as i32).max(0) as u32;
        let col = (rel_x / cell_w).min(cols - 1);
        let row = rel_y / cell_h;
        let computed = (row * cols + col) as u16;
        computed.min(total.saturating_sub(1))
    }

    pub(super) fn draw_thumbnail_grid(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        status_bar_h: u32,
    ) {
        let bottom_limit = buf_h.saturating_sub(status_bar_h);
        let total = self.total_pages();
        for idx in 0..total {
            let (tx, ty, tw, th) = self.thumbnail_screen_rect(idx, buf_w, sidebar_w, top_bars_h);
            if ty + (th as i32) + 28 < (top_bars_h as i32) || ty >= (bottom_limit as i32) {
                continue;
            }

            // Fill cell card backdrop (white paper background inside THUMB_W x THUMB_H)
            fill_rect_clipped(
                buffer,
                buf_w,
                bottom_limit,
                tx,
                ty,
                tw,
                th,
                0x003A3A3A,
            );

            if let Some(entry) = self.thumbnail_cache.get(&idx) {
                let ox = tx + ((tw.saturating_sub(entry.width)) / 2) as i32;
                let oy = ty + ((th.saturating_sub(entry.height)) / 2) as i32;

                for r in 0..entry.height {
                    let py = oy + (r as i32);
                    if py < (top_bars_h as i32) || py >= (bottom_limit as i32) {
                        continue;
                    }
                    for c in 0..entry.width {
                        let px = ox + (c as i32);
                        if px < (sidebar_w as i32) || px >= (buf_w as i32) {
                            continue;
                        }
                        buffer[(py as u32 * buf_w + px as u32) as usize] =
                            entry.pixels[(r * entry.width + c) as usize];
                    }
                }
            }

            // Border highlight for selected page vs hovered page vs normal page
            let border_color = if idx == self.current_page_index {
                0x0042A5F5 // Bright blue for selected page
            } else if self.hovered_thumb == Some(idx) {
                0x00FFCA28 // Amber for hovered page
            } else {
                0x00555555 // Subtle border
            };

            if tx >= 0 && ty >= (top_bars_h as i32) {
                draw_rect_border(
                    buffer,
                    buf_w,
                    bottom_limit,
                    tx as u32,
                    ty as u32,
                    tw,
                    th,
                    border_color,
                );
                if idx == self.current_page_index && tw > 2 && th > 2 {
                    draw_rect_border(
                        buffer,
                        buf_w,
                        bottom_limit,
                        (tx as u32) + 1,
                        (ty as u32) + 1,
                        tw - 2,
                        th - 2,
                        border_color,
                    );
                }
            }

            // Top-left `[BW]` Grayscale conversion badge on each thumbnail
            let bw_badge_w = 26u32;
            let bw_badge_h = 16u32;
            let bwx = tx;
            let bwy = ty;
            let is_bw = self.bw_pages.contains(&idx);
            if bwx >= (sidebar_w as i32)
                && bwy >= (top_bars_h as i32)
                && (bwy as u32 + bw_badge_h) < bottom_limit
            {
                let bg = if is_bw { 0x00455A64 } else { 0x00263238 };
                let fg = if is_bw { 0x0080DEEA } else { 0x00E0E0E0 };
                fill_rect_clipped(
                    buffer,
                    buf_w,
                    bottom_limit,
                    bwx,
                    bwy,
                    bw_badge_w,
                    bw_badge_h,
                    bg,
                );
                draw_rect_border(
                    buffer,
                    buf_w,
                    bottom_limit,
                    bwx as u32,
                    bwy as u32,
                    bw_badge_w,
                    bw_badge_h,
                    0x0090A4AE,
                );
                draw_bitmap_str_8x8(
                    buffer,
                    buf_w,
                    bottom_limit,
                    (bwx as u32) + 5,
                    (bwy as u32) + 4,
                    "BW",
                    fg,
                    1,
                );
            }

            // Top-right `[R]` rotate badge on each thumbnail
            let badge_w = 24u32;
            let badge_h = 16u32;
            let bx = tx + (tw as i32) - (badge_w as i32);
            let by = ty;
            if bx >= (sidebar_w as i32)
                && by >= (top_bars_h as i32)
                && (by as u32 + badge_h) < bottom_limit
            {
                fill_rect_clipped(
                    buffer,
                    buf_w,
                    bottom_limit,
                    bx,
                    by,
                    badge_w,
                    badge_h,
                    0x00263238,
                );
                draw_rect_border(
                    buffer,
                    buf_w,
                    bottom_limit,
                    bx as u32,
                    by as u32,
                    badge_w,
                    badge_h,
                    0x0090A4AE,
                );
                draw_bitmap_str_8x8(
                    buffer,
                    buf_w,
                    bottom_limit,
                    (bx as u32) + 4,
                    (by as u32) + 4,
                    "R",
                    0x00FFEB3B,
                    1,
                );
            }

            // Page number label centered beneath thumbnail (shows Logical + Physical when roman_end > 0)
            let label = if self.roman_end > 0 {
                let logical =
                    PdfEngine::format_logical_page_label(idx as usize, self.roman_end);
                if is_bw {
                    format!("P.{} ({}) BW", logical, idx + 1)
                } else {
                    format!("Page {} ({})", logical, idx + 1)
                }
            } else if is_bw {
                format!("Page {} [BW]", idx + 1)
            } else {
                format!("Page {}", idx + 1)
            };
            let label_w = (label.len() as u32) * 8;
            let lx = (tx + ((tw as i32 - label_w as i32) / 2)).max(sidebar_w as i32);
            let ly = ty + (th as i32) + 6;
            if ly >= (top_bars_h as i32) && (ly as u32 + 10) < bottom_limit {
                let text_color = if idx == self.current_page_index {
                    0x0064B5F6
                } else {
                    0x00E0E0E0
                };
                draw_bitmap_str_8x8(
                    buffer,
                    buf_w,
                    bottom_limit,
                    lx as u32,
                    ly as u32,
                    &label,
                    text_color,
                    1,
                );
            }
        }

        // If user is dragging a page thumbnail to reorder, draw a bright blue vertical insertion bar
        if let Some((src_idx, sx, sy)) = self.grid_drag_page {
            let (cx, cy) = self.cursor_pos;
            let dist = ((cx - sx).powi(2) + (cy - sy).powi(2)).sqrt();
            if dist > 6.0 {
                let target_idx = self.grid_drop_target_index(cx, cy, buf_w, sidebar_w, top_bars_h);
                if target_idx != src_idx {
                    let (tx, ty, _tw, th) =
                        self.thumbnail_screen_rect(target_idx, buf_w, sidebar_w, top_bars_h);
                    let bar_x = (tx - 8).max(sidebar_w as i32);
                    fill_rect_clipped(
                        buffer,
                        buf_w,
                        bottom_limit,
                        bar_x,
                        ty - 4,
                        4,
                        th + 8,
                        0x0000E5FF,
                    );
                }
            }
        }
    }
}

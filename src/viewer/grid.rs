use crate::engine::PdfEngine;
use anyhow::Result;
use super::state::*;
use super::render::fill_rect_clipped;

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
        scale_factor: f64,
    ) {
        let theme = self.ui_theme();
        let s = scale_factor as f32;
        let bottom_limit = buf_h.saturating_sub(status_bar_h);
        let total = self.total_pages();
        let label_sz = (11.0 * s).clamp(10.0, 22.0);
        let badge_sz = (9.5 * s).clamp(8.5, 18.0);

        for idx in 0..total {
            let (tx, ty, tw, th) = self.thumbnail_screen_rect(idx, buf_w, sidebar_w, top_bars_h);
            if ty + (th as i32) + 32 < (top_bars_h as i32) || ty >= (bottom_limit as i32) {
                continue;
            }

            let is_selected = idx == self.current_page_index;
            let is_hovered = self.hovered_thumb == Some(idx);
            let is_bw = self.bw_pages.contains(&idx);

            // Soft drop shadow behind page card (clipped to content region)
            if ty >= top_bars_h as i32 && (ty + th as i32) < bottom_limit as i32 {
                super::ui_draw::draw_soft_shadow(
                    buffer,
                    buf_w,
                    bottom_limit,
                    tx,
                    ty,
                    tw,
                    th,
                    3.0,
                    6,
                    2,
                    theme.shadow_rgb,
                    theme.shadow_max_alpha,
                );
            }

            // Paper card backdrop
            fill_rect_clipped(buffer, buf_w, bottom_limit, tx, ty, tw, th, theme.surface_bg);

            if let Some(entry) = self.thumbnail_cache.get(&idx) {
                let ox = tx + ((tw.saturating_sub(entry.width)) / 2) as i32;
                let oy = ty + ((th.saturating_sub(entry.height)) / 2) as i32;

                for r in 0..entry.height {
                    let py = oy + (r as i32);
                    if py < (top_bars_h as i32) || py >= (bottom_limit as i32) {
                        continue;
                    }
                    let dst_row = (py as u32 * buf_w) as usize;
                    let src_row = (r * entry.width) as usize;
                    for c in 0..entry.width {
                        let px = ox + (c as i32);
                        if px < (sidebar_w as i32) || px >= (buf_w as i32) {
                            continue;
                        }
                        buffer[dst_row + px as usize] = entry.pixels[src_row + c as usize];
                    }
                }
            }

            // Clean accent ring for selected/hovered page, hairline border otherwise
            let border_color = if is_selected {
                theme.accent
            } else if is_hovered {
                theme.accent_hover
            } else {
                theme.page_border
            };

            if tx >= sidebar_w as i32 && ty >= (top_bars_h as i32) {
                super::ui_draw::stroke_rounded_rect(buffer, buf_w, bottom_limit, tx, ty, tw, th, 2.5, border_color);
                if is_selected && tw > 4 && th > 4 {
                    super::ui_draw::stroke_rounded_rect(buffer, buf_w, bottom_limit, tx - 1, ty - 1, tw + 2, th + 2, 3.5, theme.accent);
                }
            }

            // Clean corner action pills: show on hover (and keep BW badge visible if page is BW)
            let show_badges = is_hovered || is_selected;
            if (show_badges || is_bw)
                && tx >= (sidebar_w as i32)
                && ty >= (top_bars_h as i32)
                && (ty as u32 + 22) < bottom_limit
            {
                let bw_w = 28u32;
                let bw_h = 18u32;
                let bg = if is_bw { theme.accent } else { theme.surface_bg };
                let fg = if is_bw { theme.text_on_accent } else { theme.text_secondary };
                super::ui_draw::fill_rounded_rect(buffer, buf_w, bottom_limit, tx + 3, ty + 3, bw_w, bw_h, 4.0, bg);
                super::ui_draw::stroke_rounded_rect(buffer, buf_w, bottom_limit, tx + 3, ty + 3, bw_w, bw_h, 4.0, if is_bw { theme.accent } else { theme.border_subtle });
                let btw = super::ui_draw::measure_text("BW", badge_sz, true);
                super::ui_draw::draw_text_vcenter(buffer, buf_w, bottom_limit, tx + 3 + ((bw_w as f32 - btw) * 0.5).round() as i32, ty + 3, bw_h, tx + 3 + bw_w as i32, "BW", badge_sz, fg, true);
            }

            if show_badges
                && (tx + tw as i32 - 24) >= (sidebar_w as i32)
                && ty >= (top_bars_h as i32)
                && (ty as u32 + 22) < bottom_limit
            {
                let rw = 22u32;
                let rh = 18u32;
                let rx = tx + tw as i32 - rw as i32 - 3;
                let ry = ty + 3;
                super::ui_draw::fill_rounded_rect(buffer, buf_w, bottom_limit, rx, ry, rw, rh, 4.0, theme.surface_bg);
                super::ui_draw::stroke_rounded_rect(buffer, buf_w, bottom_limit, rx, ry, rw, rh, 4.0, theme.border_subtle);
                super::ui_draw::draw_icon(buffer, buf_w, bottom_limit, super::ui_draw::UiIcon::Rotate, rx + 4, ry + 2, 13, theme.accent);
            }

            // Crisp proportional page number centered below card
            let label = if self.roman_end > 0 || self.skip_pages > 0 {
                let logical = PdfEngine::format_logical_page_label_with_skip(
                    idx as usize,
                    self.skip_pages,
                    self.roman_end,
                );
                format!("{} ({})", logical, idx + 1)
            } else {
                format!("Page {}", idx + 1)
            };
            let lw = super::ui_draw::measure_text(&label, label_sz, is_selected);
            let lx = (tx + ((tw as f32 - lw) * 0.5).round() as i32).max(sidebar_w as i32);
            let ly = ty + (th as i32) + 6;
            if ly >= (top_bars_h as i32) && (ly as u32 + 16) < bottom_limit {
                let text_color = if is_selected { theme.accent } else { theme.text_secondary };
                super::ui_draw::draw_text_clipped(buffer, buf_w, bottom_limit, lx, ly, buf_w as i32 - 4, &label, label_sz, text_color, is_selected);
            }
        }

        // Drag-to-reorder insertion indicator bar
        if let Some((src_idx, sx, sy)) = self.grid_drag_page {
            let (cx, cy) = self.cursor_pos;
            let dist = ((cx - sx).powi(2) + (cy - sy).powi(2)).sqrt();
            if dist > 6.0 {
                let target_idx = self.grid_drop_target_index(cx, cy, buf_w, sidebar_w, top_bars_h);
                if target_idx != src_idx {
                    let (tx, ty, _tw, th) =
                        self.thumbnail_screen_rect(target_idx, buf_w, sidebar_w, top_bars_h);
                    let bar_x = (tx - 10).max(sidebar_w as i32);
                    super::ui_draw::fill_rounded_rect(buffer, buf_w, bottom_limit, bar_x, ty - 4, 4, th + 8, 2.0, theme.accent);
                }
            }
        }
    }

    // =========================================================================
    // Thumbnail Grid Right-Click Page Context Menu (#4)
    // =========================================================================

    fn thumb_context_menu_layout(
        &self,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<(ThumbContextMenuState, i32, i32, u32, u32, Vec<(i32, i32, u32, u32, ThumbContextHit)>)> {
        let st = self.thumb_context_menu?;
        let s = sf as f32;
        let mw = (242.0 * s).round() as u32;
        let pad = (8.0 * s).round() as i32;
        let hdr_h = (26.0 * s).round() as u32;
        let row_h = (28.0 * s).round() as u32;
        let gap = (4.0 * s).round() as i32;
        let iw = mw.saturating_sub((pad * 2) as u32);
        // Header + blank count row + Before/After blank pills + divider + 5 actions + padding
        let mh = hdr_h + row_h * 7 + (gap as u32) * 3 + (14.0 * s).round() as u32;
        let mx = st.x.clamp(8, (buf_w as i32 - mw as i32 - 8).max(8));
        let my = st.y.clamp(8, (buf_h as i32 - mh as i32 - 8).max(8));

        let mut boxes = Vec::with_capacity(10);
        let ix = mx + pad;
        let mut cy = my + hdr_h as i32 + (5.0 * s).round() as i32;

        // Row 1: Blank Pages count stepper: "Blank Pages:" [ - ] N [ + ]
        let sm_w = (24.0 * s).round() as u32;
        let stepper_w = (82.0 * s).round() as u32;
        let plus_x = ix + iw as i32 - sm_w as i32;
        let minus_x = ix + iw as i32 - stepper_w as i32;
        let bh = (22.0 * s).round() as u32;
        let by = cy + ((row_h.saturating_sub(bh)) / 2) as i32;
        boxes.push((minus_x, by, sm_w, bh, ThumbContextHit::BlankCountMinus));
        boxes.push((plus_x, by, sm_w, bh, ThumbContextHit::BlankCountPlus));
        cy += row_h as i32 + gap;

        // Row 2: [ Insert Before ] [ Insert After ]
        let half_w = (iw.saturating_sub(gap as u32)) / 2;
        boxes.push((ix, cy, half_w, bh + 2, ThumbContextHit::InsertBlankBefore));
        boxes.push((ix + half_w as i32 + gap, cy, iw.saturating_sub(half_w + gap as u32), bh + 2, ThumbContextHit::InsertBlankAfter));
        cy += row_h as i32 + gap * 2;

        // Rows 3..7: Standard page operations
        for hit in [
            ThumbContextHit::InsertPdfBefore,
            ThumbContextHit::InsertPdfAfter,
            ThumbContextHit::RotateCw,
            ThumbContextHit::ConvertBw,
            ThumbContextHit::DeletePage,
        ] {
            boxes.push((ix, cy, iw, row_h, hit));
            cy += row_h as i32;
        }

        Some((st, mx, my, mw, mh, boxes))
    }

    pub(super) fn draw_thumb_context_menu(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) {
        let Some((st, mx, my, mw, mh, boxes)) = self.thumb_context_menu_layout(buf_w, buf_h, sf) else {
            return;
        };
        let theme = self.ui_theme();
        let s = sf as f32;
        let rad = (8.0 * s).clamp(5.0, 12.0);
        let pad = (8.0 * s).round() as i32;
        let hdr_h = (26.0 * s).round() as u32;
        let row_h = (28.0 * s).round() as u32;
        let lbl_sz = (10.5 * s).clamp(9.0, 18.0);
        let body_sz = (11.5 * s).clamp(10.0, 20.0);
        let p_rad = (4.5 * s).max(3.0);

        super::ui_draw::draw_soft_shadow(
            buffer, buf_w, buf_h, mx, my, mw, mh, rad,
            (12.0 * s).round() as i32, (4.0 * s).round() as i32,
            theme.shadow_rgb, theme.shadow_max_alpha,
        );
        super::ui_draw::fill_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.surface_bg);
        super::ui_draw::stroke_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.border_subtle);

        let pnum = st.page_idx + 1;
        let hdr = format!("Page {pnum} Actions");
        super::ui_draw::draw_text_vcenter(
            buffer, buf_w, buf_h, mx + pad + 2, my, hdr_h, mx + mw as i32 - pad,
            &hdr, lbl_sz, theme.text_muted, true,
        );
        fill_rect_clipped(buffer, buf_w, buf_h, mx + 1, my + hdr_h as i32, mw.saturating_sub(2), 1, theme.divider);

        let find = |hit: ThumbContextHit| boxes.iter().copied().find(|b| b.4 == hit);

        // Blank count row
        if let (Some((mx1, my1, mw1, mh1, _)), Some((px1, py1, pw1, ph1, _))) = (
            find(ThumbContextHit::BlankCountMinus),
            find(ThumbContextHit::BlankCountPlus),
        ) {
            super::ui_draw::draw_text_vcenter(
                buffer, buf_w, buf_h, mx + pad + 2, my1, mh1, mx1 - 4,
                "Blank Pages:", body_sz, theme.text_primary, false,
            );
            for (rx, ry, rw, rh, hit, txt) in [
                (mx1, my1, mw1, mh1, ThumbContextHit::BlankCountMinus, "-"),
                (px1, py1, pw1, ph1, ThumbContextHit::BlankCountPlus, "+"),
            ] {
                let hov = self.hovered_widget == Some(UiHit::ThumbContext(hit));
                super::ui_draw::fill_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, p_rad, if hov { theme.hover_bg } else { theme.surface_elevated });
                super::ui_draw::stroke_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, p_rad, theme.border_subtle);
                let tw = super::ui_draw::measure_text(txt, body_sz, true);
                super::ui_draw::draw_text_vcenter(buffer, buf_w, buf_h, rx + ((rw as f32 - tw) * 0.5).round() as i32, ry, rh, rx + rw as i32, txt, body_sz, theme.text_primary, true);
            }
            let cnt_txt = format!("{}", st.blank_count);
            let cw = super::ui_draw::measure_text(&cnt_txt, body_sz, true);
            let cx = mx1 + mw1 as i32 + (((px1 - (mx1 + mw1 as i32)) as f32 - cw) * 0.5).round() as i32;
            super::ui_draw::draw_text_vcenter(buffer, buf_w, buf_h, cx, my1, mh1, px1, &cnt_txt, body_sz, theme.accent, true);
        }

        // Insert Blank Before / After pill buttons
        for (hit, txt) in [
            (ThumbContextHit::InsertBlankBefore, format!("+ {} Before", st.blank_count)),
            (ThumbContextHit::InsertBlankAfter, format!("+ {} After", st.blank_count)),
        ] {
            if let Some((rx, ry, rw, rh, _)) = find(hit) {
                let hov = self.hovered_widget == Some(UiHit::ThumbContext(hit));
                super::ui_draw::fill_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, p_rad, if hov { theme.accent_soft_bg } else { theme.surface_elevated });
                super::ui_draw::stroke_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, p_rad, if hov { theme.accent } else { theme.border_subtle });
                let tw = super::ui_draw::measure_text(&txt, lbl_sz, true);
                let fg = if hov { theme.accent_soft_fg } else { theme.text_primary };
                super::ui_draw::draw_text_vcenter(buffer, buf_w, buf_h, rx + ((rw as f32 - tw) * 0.5).round() as i32, ry, rh, rx + rw as i32 - 2, &txt, lbl_sz, fg, true);
            }
        }

        // Divider above action list
        if let Some((_, first_y, _, _, _)) = find(ThumbContextHit::InsertPdfBefore) {
            fill_rect_clipped(buffer, buf_w, buf_h, mx + 6, first_y - (4.0 * s).round() as i32, mw.saturating_sub(12), 1, theme.divider);
        }

        let can_delete = self.total_pages() > 1;
        let rows: [(ThumbContextHit, String, bool, bool); 5] = [
            (ThumbContextHit::InsertPdfBefore, format!("Insert PDF(s) Before Page {pnum}..."), true, false),
            (ThumbContextHit::InsertPdfAfter, format!("Insert PDF(s) After Page {pnum}..."), true, false),
            (ThumbContextHit::RotateCw, "Rotate 90° Clockwise".to_string(), true, false),
            (ThumbContextHit::ConvertBw, "Convert Page to Black & White".to_string(), true, false),
            (ThumbContextHit::DeletePage, format!("Delete Page {pnum}"), can_delete, true),
        ];

        for (hit, label, enabled, danger) in rows {
            if let Some((rx, ry, rw, _rh, _)) = find(hit) {
                let hov = enabled && self.hovered_widget == Some(UiHit::ThumbContext(hit));
                if hov {
                    let bg = if danger { theme.danger_soft_bg } else { theme.hover_bg };
                    super::ui_draw::fill_rounded_rect(buffer, buf_w, buf_h, rx, ry + 1, rw, row_h.saturating_sub(2), p_rad, bg);
                }
                let fg = if !enabled {
                    theme.text_muted
                } else if danger {
                    theme.danger
                } else {
                    theme.text_primary
                };
                super::ui_draw::draw_text_vcenter(buffer, buf_w, buf_h, rx + 6, ry, row_h, rx + rw as i32 - 6, &label, body_sz, fg, danger && enabled);
            }
        }
    }

    pub(super) fn hit_test_thumb_context_menu(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<ThumbContextHit> {
        let (_, x, y, mw, mh, boxes) = self.thumb_context_menu_layout(buf_w, buf_h, sf)?;
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix < x || ix >= x + mw as i32 || iy < y || iy >= y + mh as i32 {
            return None;
        }
        for (bx, by, bw, bh, hit) in boxes {
            if ix >= bx && ix < bx + bw as i32 && iy >= by && iy < by + bh as i32 {
                return Some(hit);
            }
        }
        Some(ThumbContextHit::InsideMenu)
    }

    pub(super) fn handle_thumb_context_click(&mut self, hit: ThumbContextHit) {
        let Some(st) = self.thumb_context_menu else { return };
        match hit {
            ThumbContextHit::InsideMenu => return,
            ThumbContextHit::BlankCountMinus => {
                if let Some(m) = self.thumb_context_menu.as_mut() {
                    m.blank_count = m.blank_count.saturating_sub(1).max(1);
                }
                self.request_redraw();
                return;
            }
            ThumbContextHit::BlankCountPlus => {
                if let Some(m) = self.thumb_context_menu.as_mut() {
                    m.blank_count = (m.blank_count + 1).min(50);
                }
                self.request_redraw();
                return;
            }
            ThumbContextHit::InsertBlankBefore => {
                self.thumb_context_menu = None;
                self.insert_blank_pages_at(st.page_idx, st.blank_count, true);
            }
            ThumbContextHit::InsertBlankAfter => {
                self.thumb_context_menu = None;
                self.insert_blank_pages_at(st.page_idx + 1, st.blank_count, false);
            }
            ThumbContextHit::InsertPdfBefore => {
                self.thumb_context_menu = None;
                self.open_insert_pdf_modal(true, st.page_idx);
            }
            ThumbContextHit::InsertPdfAfter => {
                self.thumb_context_menu = None;
                self.open_insert_pdf_modal(false, st.page_idx);
            }
            ThumbContextHit::RotateCw => {
                self.thumb_context_menu = None;
                self.rotate_page_clockwise(st.page_idx);
            }
            ThumbContextHit::ConvertBw => {
                self.thumb_context_menu = None;
                self.convert_page_to_bw_in_place(st.page_idx);
            }
            ThumbContextHit::DeletePage => {
                self.thumb_context_menu = None;
                self.current_page_index = st.page_idx;
                self.delete_current_page();
            }
        }
    }
}

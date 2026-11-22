//! Milestone 4: Compact 720p PDF-to-Excel Export Options Modal (`Export -> Export as Excel (.xlsx)...`).
//!
//! Lets users choose between Auto (Hybrid), Lattice, and Stream table extraction modes,
//! multi-table sheet layout (Stack on One Sheet vs Separate Sheet per Table), and financial
//! comma-decimal normalization. Zero GPU, zero idle RAM when closed.

use crate::export::{ExcelExportMode, ExcelExportOptions, ExcelTableLayout};
use super::render::{blend_rect_rgba, fill_rect_clipped};
use super::state::*;
use super::ui_draw::*;

#[derive(Clone, Debug)]
pub(super) struct ExcelExportModalState {
    pub(super) opts: ExcelExportOptions,
}

impl ExcelExportModalState {
    pub(super) fn new(opts: ExcelExportOptions) -> Self {
        Self { opts }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum ExcelExportHit {
    Close,
    ModeAuto,
    ModeLattice,
    ModeStream,
    LayoutStack,
    LayoutSeparate,
    ToggleCommaDecimals,
    Cancel,
    ExportNow,
    InsideModal,
}

#[derive(Clone, Copy)]
struct ExcelBox {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    hit: ExcelExportHit,
}

impl ExcelBox {
    fn contains(self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w as i32 && py >= self.y && py < self.y + self.h as i32
    }
}

impl<'a> PdfViewerApp<'a> {
    pub(super) fn open_excel_export_modal(&mut self) {
        if self.doc.is_none() {
            self.status_notice = Some("Open a PDF first to export to Excel (.xlsx)".to_string());
            self.request_redraw();
            return;
        }
        self.open_menu = None;
        self.excel_export_modal = Some(ExcelExportModalState::new(self.excel_export_opts));
        self.request_redraw();
    }

    fn excel_export_modal_layout(
        &self,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<(i32, i32, u32, u32, Vec<ExcelBox>)> {
        let _st = self.excel_export_modal.as_ref()?;
        let s = sf as f32;
        let mw = ((476.0 * s).round() as u32).min(buf_w.saturating_sub(24));
        let mh = ((318.0 * s).round() as u32).min(buf_h.saturating_sub(24));
        let mx = ((buf_w.saturating_sub(mw)) / 2) as i32;
        let my = ((buf_h.saturating_sub(mh)) / 2) as i32;
        let pad = (18.0 * s).round() as i32;
        let inner_w = mw.saturating_sub((pad as u32) * 2);

        let mut hits = Vec::with_capacity(10);

        // Close X button
        let close_sz = (24.0 * s).round() as u32;
        hits.push(ExcelBox {
            x: mx + mw as i32 - pad - close_sz as i32,
            y: my + (12.0 * s).round() as i32,
            w: close_sz,
            h: close_sz,
            hit: ExcelExportHit::Close,
        });

        // Extraction Mode row (3 equal pills)
        let mode_y = my + (74.0 * s).round() as i32;
        let pill_h = (30.0 * s).round() as u32;
        let gap = (8.0 * s).round() as i32;
        let mode_w = (inner_w.saturating_sub((gap as u32) * 2)) / 3;
        hits.push(ExcelBox {
            x: mx + pad,
            y: mode_y,
            w: mode_w,
            h: pill_h,
            hit: ExcelExportHit::ModeAuto,
        });
        hits.push(ExcelBox {
            x: mx + pad + mode_w as i32 + gap,
            y: mode_y,
            w: mode_w,
            h: pill_h,
            hit: ExcelExportHit::ModeLattice,
        });
        hits.push(ExcelBox {
            x: mx + pad + (mode_w as i32 + gap) * 2,
            y: mode_y,
            w: inner_w.saturating_sub((mode_w + gap as u32) * 2),
            h: pill_h,
            hit: ExcelExportHit::ModeStream,
        });

        // Multi-Table Layout row (2 equal pills)
        let lay_y = my + (152.0 * s).round() as i32;
        let lay_w = (inner_w.saturating_sub(gap as u32)) / 2;
        hits.push(ExcelBox {
            x: mx + pad,
            y: lay_y,
            w: lay_w,
            h: pill_h,
            hit: ExcelExportHit::LayoutStack,
        });
        hits.push(ExcelBox {
            x: mx + pad + lay_w as i32 + gap,
            y: lay_y,
            w: inner_w.saturating_sub(lay_w + gap as u32),
            h: pill_h,
            hit: ExcelExportHit::LayoutSeparate,
        });

        // Comma-decimal checkbox row
        let chk_y = my + (206.0 * s).round() as i32;
        let chk_h = (32.0 * s).round() as u32;
        hits.push(ExcelBox {
            x: mx + pad,
            y: chk_y,
            w: inner_w,
            h: chk_h,
            hit: ExcelExportHit::ToggleCommaDecimals,
        });

        // Footer buttons: Cancel + Export Excel (.xlsx)...
        let btn_h = (32.0 * s).round() as u32;
        let btn_y = my + mh as i32 - (16.0 * s).round() as i32 - btn_h as i32;
        let export_w = (168.0 * s).round() as u32;
        let cancel_w = (82.0 * s).round() as u32;
        let export_x = mx + mw as i32 - pad - export_w as i32;
        let cancel_x = export_x - gap - cancel_w as i32;
        hits.push(ExcelBox {
            x: cancel_x,
            y: btn_y,
            w: cancel_w,
            h: btn_h,
            hit: ExcelExportHit::Cancel,
        });
        hits.push(ExcelBox {
            x: export_x,
            y: btn_y,
            w: export_w,
            h: btn_h,
            hit: ExcelExportHit::ExportNow,
        });

        Some((mx, my, mw, mh, hits))
    }

    pub(super) fn draw_excel_export_modal(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) {
        let Some(st) = self.excel_export_modal.as_ref() else {
            return;
        };
        let Some((mx, my, mw, mh, hits)) = self.excel_export_modal_layout(buf_w, buf_h, sf) else {
            return;
        };
        let theme = self.ui_theme();
        let s = sf as f32;
        let pad = (18.0 * s).round() as i32;
        let hdr_h = (46.0 * s).round() as u32;
        let rad = (10.0 * s).max(6.0);
        let btn_rad = (6.0 * s).max(4.0);
        let opts = st.opts;

        // Scrim + card shadow + modal surface
        blend_rect_rgba(buffer, buf_w, buf_h, 0, 0, buf_w, buf_h, theme.modal_scrim);
        draw_soft_shadow(
            buffer,
            buf_w,
            buf_h,
            mx,
            my,
            mw,
            mh,
            rad,
            18,
            6,
            theme.shadow_rgb,
            theme.shadow_max_alpha,
        );
        fill_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.border_subtle);

        // Header
        draw_text_vcenter(
            buffer,
            buf_w,
            buf_h,
            mx + pad,
            my,
            hdr_h,
            mx + mw as i32 - (48.0 * s).round() as i32,
            "Export PDF to Excel (.xlsx)",
            14.5 * s,
            theme.text_primary,
            true,
        );
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            mx + 1,
            my + hdr_h as i32,
            mw.saturating_sub(2),
            1,
            theme.divider,
        );

        // Section 1: Extraction Mode
        draw_text(
            buffer,
            buf_w,
            buf_h,
            mx + pad,
            my + (55.0 * s).round() as i32,
            "Table Detection Engine",
            11.5 * s,
            theme.text_secondary,
            true,
        );
        let mode_hint = match opts.mode {
            ExcelExportMode::AutoHybrid => {
                "Auto: extracts ruled vector tables first, then falls back to whitespace columns."
            }
            ExcelExportMode::Lattice => {
                "Lattice: strictly uses drawn lines/borders & preserves true merged cells."
            }
            ExcelExportMode::Stream => {
                "Stream: ignores ruling lines and aligns text using vertical whitespace gutters."
            }
        };
        draw_text(
            buffer,
            buf_w,
            buf_h,
            mx + pad,
            my + (110.0 * s).round() as i32,
            mode_hint,
            10.5 * s,
            theme.text_muted,
            false,
        );

        // Section 2: Multi-Table Layout
        draw_text(
            buffer,
            buf_w,
            buf_h,
            mx + pad,
            my + (133.0 * s).round() as i32,
            "Multi-Table Worksheet Layout",
            11.5 * s,
            theme.text_secondary,
            true,
        );

        // Footer divider
        let foot_line_y = my + mh as i32 - (58.0 * s).round() as i32;
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            mx + 1,
            foot_line_y,
            mw.saturating_sub(2),
            1,
            theme.divider,
        );

        // Draw interactive controls
        for b in &hits {
            let hovered = self.hovered_widget == Some(UiHit::ExcelExport(b.hit));
            match b.hit {
                ExcelExportHit::Close => {
                    if hovered {
                        fill_rounded_rect(
                            buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, 5.0 * s, theme.tab_hover_bg,
                        );
                    }
                    let tw = measure_text("x", 13.0 * s, true).round() as i32;
                    draw_text_vcenter(
                        buffer,
                        buf_w,
                        buf_h,
                        b.x + (b.w as i32 - tw) / 2,
                        b.y,
                        b.h,
                        b.x + b.w as i32,
                        "x",
                        13.0 * s,
                        theme.text_secondary,
                        true,
                    );
                }
                ExcelExportHit::ModeAuto
                | ExcelExportHit::ModeLattice
                | ExcelExportHit::ModeStream => {
                    let (label, active) = match b.hit {
                        ExcelExportHit::ModeAuto => {
                            ("Auto (Hybrid)", opts.mode == ExcelExportMode::AutoHybrid)
                        }
                        ExcelExportHit::ModeLattice => {
                            ("Lattice", opts.mode == ExcelExportMode::Lattice)
                        }
                        _ => ("Stream", opts.mode == ExcelExportMode::Stream),
                    };
                    let bg = if active {
                        theme.accent
                    } else if hovered {
                        theme.tab_hover_bg
                    } else {
                        theme.surface_elevated
                    };
                    let fg = if active { 0x00FF_FFFF } else { theme.text_primary };
                    fill_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, btn_rad, bg);
                    stroke_rounded_rect(
                        buffer,
                        buf_w,
                        buf_h,
                        b.x,
                        b.y,
                        b.w,
                        b.h,
                        btn_rad,
                        if active { theme.accent } else { theme.border_subtle },
                    );
                    let tw = measure_text(label, 11.5 * s, active).round() as i32;
                    draw_text_vcenter(
                        buffer,
                        buf_w,
                        buf_h,
                        b.x + (b.w as i32 - tw) / 2,
                        b.y,
                        b.h,
                        b.x + b.w as i32,
                        label,
                        11.5 * s,
                        fg,
                        active,
                    );
                }
                ExcelExportHit::LayoutStack | ExcelExportHit::LayoutSeparate => {
                    let (label, active) = match b.hit {
                        ExcelExportHit::LayoutStack => (
                            "Stack on One Sheet",
                            opts.layout == ExcelTableLayout::StackOnOneSheet,
                        ),
                        _ => (
                            "Separate Sheet per Table",
                            opts.layout == ExcelTableLayout::SeparateSheets,
                        ),
                    };
                    let bg = if active {
                        theme.accent
                    } else if hovered {
                        theme.tab_hover_bg
                    } else {
                        theme.surface_elevated
                    };
                    let fg = if active { 0x00FF_FFFF } else { theme.text_primary };
                    fill_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, btn_rad, bg);
                    stroke_rounded_rect(
                        buffer,
                        buf_w,
                        buf_h,
                        b.x,
                        b.y,
                        b.w,
                        b.h,
                        btn_rad,
                        if active { theme.accent } else { theme.border_subtle },
                    );
                    let tw = measure_text(label, 11.5 * s, active).round() as i32;
                    draw_text_vcenter(
                        buffer,
                        buf_w,
                        buf_h,
                        b.x + (b.w as i32 - tw) / 2,
                        b.y,
                        b.h,
                        b.x + b.w as i32,
                        label,
                        11.5 * s,
                        fg,
                        active,
                    );
                }
                ExcelExportHit::ToggleCommaDecimals => {
                    let bg = if hovered {
                        theme.tab_hover_bg
                    } else {
                        theme.surface_elevated
                    };
                    fill_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, btn_rad, bg);
                    stroke_rounded_rect(
                        buffer,
                        buf_w,
                        buf_h,
                        b.x,
                        b.y,
                        b.w,
                        b.h,
                        btn_rad,
                        theme.border_subtle,
                    );
                    let cb_sz = (16.0 * s).round() as u32;
                    let cb_x = b.x + (10.0 * s).round() as i32;
                    let cb_y = b.y + (b.h as i32 - cb_sz as i32) / 2;
                    fill_rounded_rect(
                        buffer,
                        buf_w,
                        buf_h,
                        cb_x,
                        cb_y,
                        cb_sz,
                        cb_sz,
                        3.0 * s,
                        if opts.normalize_comma_decimals {
                            theme.accent
                        } else {
                            theme.surface_bg
                        },
                    );
                    stroke_rounded_rect(
                        buffer,
                        buf_w,
                        buf_h,
                        cb_x,
                        cb_y,
                        cb_sz,
                        cb_sz,
                        3.0 * s,
                        if opts.normalize_comma_decimals {
                            theme.accent
                        } else {
                            theme.text_muted
                        },
                    );
                    if opts.normalize_comma_decimals {
                        let x0 = cb_x as f32 + cb_sz as f32 * 0.22;
                        let y0 = cb_y as f32 + cb_sz as f32 * 0.54;
                        let x1 = cb_x as f32 + cb_sz as f32 * 0.44;
                        let y1 = cb_y as f32 + cb_sz as f32 * 0.74;
                        let x2 = cb_x as f32 + cb_sz as f32 * 0.80;
                        let y2 = cb_y as f32 + cb_sz as f32 * 0.28;
                        let thick = (1.9 * s).max(1.4);
                        draw_line_aa(buffer, buf_w, buf_h, x0, y0, x1, y1, thick, 0x00FF_FFFF);
                        draw_line_aa(buffer, buf_w, buf_h, x1, y1, x2, y2, thick, 0x00FF_FFFF);
                    }
                    draw_text_vcenter(
                        buffer,
                        buf_w,
                        buf_h,
                        cb_x + cb_sz as i32 + (9.0 * s).round() as i32,
                        b.y,
                        b.h,
                        b.x + b.w as i32 - (8.0 * s).round() as i32,
                        "Normalize comma-decimal numbers (e.g. 123,9500 -> 123.95)",
                        11.2 * s,
                        theme.text_primary,
                        false,
                    );
                }
                ExcelExportHit::Cancel => {
                    let bg = if hovered {
                        theme.tab_hover_bg
                    } else {
                        theme.surface_elevated
                    };
                    fill_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, btn_rad, bg);
                    stroke_rounded_rect(
                        buffer,
                        buf_w,
                        buf_h,
                        b.x,
                        b.y,
                        b.w,
                        b.h,
                        btn_rad,
                        theme.border_subtle,
                    );
                    let tw = measure_text("Cancel", 11.5 * s, false).round() as i32;
                    draw_text_vcenter(
                        buffer,
                        buf_w,
                        buf_h,
                        b.x + (b.w as i32 - tw) / 2,
                        b.y,
                        b.h,
                        b.x + b.w as i32,
                        "Cancel",
                        11.5 * s,
                        theme.text_primary,
                        false,
                    );
                }
                ExcelExportHit::ExportNow => {
                    let bg = if hovered {
                        theme.accent_hover
                    } else {
                        theme.accent
                    };
                    fill_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, btn_rad, bg);
                    let label = "Export Excel (.xlsx)...";
                    let tw = measure_text(label, 11.5 * s, true).round() as i32;
                    draw_text_vcenter(
                        buffer,
                        buf_w,
                        buf_h,
                        b.x + (b.w as i32 - tw) / 2,
                        b.y,
                        b.h,
                        b.x + b.w as i32,
                        label,
                        11.5 * s,
                        0x00FF_FFFF,
                        true,
                    );
                }
                ExcelExportHit::InsideModal => {}
            }
        }
    }

    pub(super) fn hit_test_excel_export_modal(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<ExcelExportHit> {
        let (x, y, w, h, hits) = self.excel_export_modal_layout(buf_w, buf_h, sf)?;
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix < x || ix >= x + w as i32 || iy < y || iy >= y + h as i32 {
            return None;
        }
        hits.into_iter()
            .find(|b| b.contains(ix, iy))
            .map(|b| b.hit)
            .or(Some(ExcelExportHit::InsideModal))
    }

    pub(super) fn handle_excel_export_click(&mut self, hit: ExcelExportHit) {
        match hit {
            ExcelExportHit::Close | ExcelExportHit::Cancel => {
                self.excel_export_modal = None;
                self.request_redraw();
            }
            ExcelExportHit::ModeAuto => {
                if let Some(ref mut st) = self.excel_export_modal {
                    st.opts.mode = ExcelExportMode::AutoHybrid;
                }
                self.request_redraw();
            }
            ExcelExportHit::ModeLattice => {
                if let Some(ref mut st) = self.excel_export_modal {
                    st.opts.mode = ExcelExportMode::Lattice;
                }
                self.request_redraw();
            }
            ExcelExportHit::ModeStream => {
                if let Some(ref mut st) = self.excel_export_modal {
                    st.opts.mode = ExcelExportMode::Stream;
                }
                self.request_redraw();
            }
            ExcelExportHit::LayoutStack => {
                if let Some(ref mut st) = self.excel_export_modal {
                    st.opts.layout = ExcelTableLayout::StackOnOneSheet;
                }
                self.request_redraw();
            }
            ExcelExportHit::LayoutSeparate => {
                if let Some(ref mut st) = self.excel_export_modal {
                    st.opts.layout = ExcelTableLayout::SeparateSheets;
                }
                self.request_redraw();
            }
            ExcelExportHit::ToggleCommaDecimals => {
                if let Some(ref mut st) = self.excel_export_modal {
                    st.opts.normalize_comma_decimals = !st.opts.normalize_comma_decimals;
                }
                self.request_redraw();
            }
            ExcelExportHit::ExportNow => {
                if let Some(st) = self.excel_export_modal.take() {
                    self.excel_export_opts = st.opts;
                    self.pending_dialog = Some(PendingDialog::ExportXlsx);
                }
                self.request_redraw();
            }
            ExcelExportHit::InsideModal => {}
        }
    }
}

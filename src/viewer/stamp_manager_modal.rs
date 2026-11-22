//! Milestone 2.3: Portable Stamp & Signature Manager Window (`Tools -> Stamps & Signatures... (P)`).
//!
//! Compact 720p modal with **Signatures** and **Stamps** tabs, portable asset storage in
//! `./micropdf_assets/signatures/` and `./micropdf_assets/stamps/` (OS user config fallback only
//! if read-only), embedded freehand signature drawing pad (Black/Blue ink, Clear, Save PNG),
//! pre-populated customizable stamps (`APPROVED`, `CONFIDENTIAL`, `DRAFT`, `REVIEWED`, `COPY`),
//! custom text/PNG import, live preview, rename/delete, and direct interactive page placement.

use crate::engine::StampImageBuffer;
use std::path::{Path, PathBuf};

use super::render::{blend_rect_rgba, fill_rect_clipped};
use super::state::*;
use super::ui_draw::*;

const PRESET_STAMPS: [&str; 5] = ["APPROVED", "CONFIDENTIAL", "DRAFT", "REVIEWED", "COPY"];
const STAMP_COLORS: [(&str, (u8, u8, u8)); 5] = [
    ("Red", (204, 34, 42)),
    ("Green", (22, 138, 68)),
    ("Navy", (29, 78, 216)),
    ("Amber", (194, 102, 12)),
    ("Black", (28, 32, 38)),
];

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum StampManagerTab {
    Signatures,
    Stamps,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum SignatureInkColor {
    Black,
    Blue,
}

impl SignatureInkColor {
    fn rgb(self) -> (u8, u8, u8) {
        match self {
            SignatureInkColor::Black => (18, 22, 28),
            SignatureInkColor::Blue => (22, 66, 178),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum StampBorderStyle {
    Boxed,
    Double,
    Plain,
}

#[derive(Clone, Debug)]
pub(super) struct SavedAssetItem {
    pub(super) name: String,
    pub(super) path: Option<PathBuf>,
    /// `true` for built-in dynamic text stamp presets (`APPROVED`, etc.)
    pub(super) is_builtin_preset: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum StampManagerFocus {
    None,
    SaveName,
    RenameInput,
    CustomStampText,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum StampManagerHit {
    Close,
    TabSignatures,
    TabStamps,
    AssetRow(usize),
    ImportPng,
    RenameStart,
    RenameConfirm,
    DeleteAsset,
    PlaceSelected,
    // Signatures pad
    DrawPad,
    InkBlack,
    InkBlue,
    PadClear,
    FocusSaveName,
    PadSave,
    // Stamps customization
    StampColor(usize),
    StampBorder(StampBorderStyle),
    FocusCustomText,
    SaveCustomStamp,
    FocusRename,
    InsideModal,
}

#[derive(Clone, Copy)]
struct SmBox {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    hit: StampManagerHit,
}

impl SmBox {
    fn contains(self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w as i32 && py >= self.y && py < self.y + self.h as i32
    }
}

/// Resolves the portable `./micropdf_assets/` directory next to the executable (falling back to
/// OS user config dir ONLY if the executable directory is read-only).
pub fn resolve_portable_assets_root() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("micropdf_assets");
            if std::fs::create_dir_all(&candidate).is_ok() {
                let probe = candidate.join(".write_probe");
                if std::fs::write(&probe, b"ok").is_ok() {
                    let _ = std::fs::remove_file(&probe);
                    return candidate;
                }
            }
        }
    }
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("Library").join("Application Support"))
        .unwrap_or_else(std::env::temp_dir);
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir);
    let fallback = base.join("MicroPDF").join("micropdf_assets");
    let _ = std::fs::create_dir_all(&fallback);
    fallback
}

/// Ensures `signatures/` and `stamps/` exist under `root` and returns `(signatures_dir, stamps_dir)`.
pub fn ensure_asset_subdirs(root: &Path) -> (PathBuf, PathBuf) {
    let sigs = root.join("signatures");
    let stamps = root.join("stamps");
    let _ = std::fs::create_dir_all(&sigs);
    let _ = std::fs::create_dir_all(&stamps);
    (sigs, stamps)
}

/// Renders a crisp transparent-background RGBA stamp image for any text label, color, and border style.
pub fn rasterize_text_stamp_rgba(
    text: &str,
    rgb: (u8, u8, u8),
    border: StampBorderStyle,
) -> StampImageBuffer {
    let label = if text.trim().is_empty() { "STAMP" } else { text.trim() };
    let font_sz = 34.0f32;
    let tw = measure_text(label, font_sz, true).ceil() as u32;
    let pad_x = 24u32;
    let pad_y = 14u32;
    let w = (tw + pad_x * 2).clamp(140, 720);
    let h = ((font_sz * 1.25).ceil() as u32 + pad_y * 2).clamp(64, 160);

    // Render onto black scratch mask so white channel = alpha coverage
    let mut mask = vec![0u32; (w * h) as usize];
    let text_x = ((w.saturating_sub(tw)) / 2) as i32;
    draw_text_vcenter(
        &mut mask,
        w,
        h,
        text_x,
        0,
        h,
        w as i32 - 8,
        label,
        font_sz,
        0x00FF_FFFF,
        true,
    );
    match border {
        StampBorderStyle::Boxed => {
            stroke_rounded_rect(&mut mask, w, h, 3, 3, w.saturating_sub(6), h.saturating_sub(6), 8.0, 0x00FF_FFFF);
            stroke_rounded_rect(&mut mask, w, h, 4, 4, w.saturating_sub(8), h.saturating_sub(8), 7.0, 0x00FF_FFFF);
            stroke_rounded_rect(&mut mask, w, h, 5, 5, w.saturating_sub(10), h.saturating_sub(10), 6.0, 0x00FF_FFFF);
        }
        StampBorderStyle::Double => {
            stroke_rounded_rect(&mut mask, w, h, 2, 2, w.saturating_sub(4), h.saturating_sub(4), 8.0, 0x00FF_FFFF);
            stroke_rounded_rect(&mut mask, w, h, 3, 3, w.saturating_sub(6), h.saturating_sub(6), 7.0, 0x00FF_FFFF);
            stroke_rounded_rect(&mut mask, w, h, 7, 7, w.saturating_sub(14), h.saturating_sub(14), 5.0, 0x00FF_FFFF);
            stroke_rounded_rect(&mut mask, w, h, 8, 8, w.saturating_sub(16), h.saturating_sub(16), 4.0, 0x00FF_FFFF);
        }
        StampBorderStyle::Plain => {}
    }

    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for (i, &px) in mask.iter().enumerate() {
        let a = (px & 0xFF) as u8;
        if a > 0 {
            let off = i * 4;
            rgba[off] = rgb.0;
            rgba[off + 1] = rgb.1;
            rgba[off + 2] = rgb.2;
            rgba[off + 3] = a;
        }
    }
    let img = image::RgbaImage::from_raw(w, h, rgba)
        .map(image::DynamicImage::ImageRgba8)
        .unwrap_or_else(|| image::DynamicImage::new_rgba8(w, h));
    StampImageBuffer { width: w, height: h, image: img }
}

/// Rasterizes normalized freehand signature strokes (`0.0..=1.0`) into a crisp transparent RGBA PNG buffer.
pub fn rasterize_signature_pad_rgba(
    strokes: &[Vec<(f32, f32)>],
    ink: SignatureInkColor,
) -> Option<StampImageBuffer> {
    if strokes.iter().all(|s| s.is_empty()) {
        return None;
    }
    let w = 480u32;
    let h = 180u32;
    let mut mask = vec![0u32; (w * h) as usize];
    for stroke in strokes {
        if stroke.len() == 1 {
            let (nx, ny) = stroke[0];
            let x = nx * w as f32;
            let y = ny * h as f32;
            draw_line_aa(&mut mask, w, h, x, y, x + 0.8, y + 0.8, 3.6, 0x00FF_FFFF);
        } else {
            for win in stroke.windows(2) {
                let x0 = win[0].0 * w as f32;
                let y0 = win[0].1 * h as f32;
                let x1 = win[1].0 * w as f32;
                let y1 = win[1].1 * h as f32;
                draw_line_aa(&mut mask, w, h, x0, y0, x1, y1, 3.4, 0x00FF_FFFF);
            }
        }
    }
    let rgb = ink.rgb();
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let mut any = false;
    for (i, &px) in mask.iter().enumerate() {
        let a = (px & 0xFF) as u8;
        if a > 0 {
            any = true;
            let off = i * 4;
            rgba[off] = rgb.0;
            rgba[off + 1] = rgb.1;
            rgba[off + 2] = rgb.2;
            rgba[off + 3] = a;
        }
    }
    if !any {
        return None;
    }
    let img = image::RgbaImage::from_raw(w, h, rgba).map(image::DynamicImage::ImageRgba8)?;
    Some(StampImageBuffer { width: w, height: h, image: img })
}

fn save_rgba_as_png(path: &Path, buf: &StampImageBuffer) -> anyhow::Result<()> {
    buf.image.to_rgba8().save_with_format(path, image::ImageFormat::Png)?;
    Ok(())
}

fn load_png_as_stamp_buffer(path: &Path) -> anyhow::Result<StampImageBuffer> {
    let img = image::open(path)?.to_rgba8();
    let (w, h) = img.dimensions();
    Ok(StampImageBuffer {
        width: w,
        height: h,
        image: image::DynamicImage::ImageRgba8(img),
    })
}

fn sanitize_asset_filename(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let t = cleaned.trim();
    if t.is_empty() {
        "asset".to_string()
    } else {
        t.to_string()
    }
}

pub(super) struct StampManagerModalState {
    pub(super) tab: StampManagerTab,
    pub(super) assets_root: PathBuf,
    pub(super) items: Vec<SavedAssetItem>,
    pub(super) selected_idx: usize,
    pub(super) preview_rgba: Option<StampImageBuffer>,
    pub(super) focus: StampManagerFocus,
    pub(super) renaming: bool,
    pub(super) rename_input: String,
    // Freehand signature pad
    pub(super) pad_strokes: Vec<Vec<(f32, f32)>>,
    pub(super) pad_drawing: bool,
    pub(super) ink_color: SignatureInkColor,
    pub(super) save_name: String,
    // Stamp customizer
    pub(super) stamp_color_idx: usize,
    pub(super) stamp_border: StampBorderStyle,
    pub(super) custom_stamp_text: String,
    pub(super) status_msg: Option<(String, bool)>,
}

impl StampManagerModalState {
    pub(super) fn new() -> Self {
        let root = resolve_portable_assets_root();
        let _ = ensure_asset_subdirs(&root);
        let mut st = Self {
            tab: StampManagerTab::Signatures,
            assets_root: root,
            items: Vec::new(),
            selected_idx: 0,
            preview_rgba: None,
            focus: StampManagerFocus::None,
            renaming: false,
            rename_input: String::new(),
            pad_strokes: Vec::new(),
            pad_drawing: false,
            ink_color: SignatureInkColor::Black,
            save_name: "My Signature".to_string(),
            stamp_color_idx: 0,
            stamp_border: StampBorderStyle::Boxed,
            custom_stamp_text: "URGENT".to_string(),
            status_msg: None,
        };
        st.reload_items();
        st
    }

    pub(super) fn active_dir(&self) -> PathBuf {
        let (sigs, stamps) = ensure_asset_subdirs(&self.assets_root);
        match self.tab {
            StampManagerTab::Signatures => sigs,
            StampManagerTab::Stamps => stamps,
        }
    }

    pub(super) fn reload_items(&mut self) {
        self.items.clear();
        if self.tab == StampManagerTab::Stamps {
            for &preset in &PRESET_STAMPS {
                self.items.push(SavedAssetItem {
                    name: preset.to_string(),
                    path: None,
                    is_builtin_preset: true,
                });
            }
        }
        let dir = self.active_dir();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            let mut files: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case("png") || e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"))
                })
                .collect();
            files.sort();
            for p in files {
                let name = p
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Asset")
                    .to_string();
                self.items.push(SavedAssetItem {
                    name,
                    path: Some(p),
                    is_builtin_preset: false,
                });
            }
        }
        if self.selected_idx >= self.items.len() {
            self.selected_idx = 0;
        }
        self.refresh_preview();
    }

    pub(super) fn refresh_preview(&mut self) {
        self.preview_rgba = None;
        let Some(item) = self.items.get(self.selected_idx) else {
            return;
        };
        if item.is_builtin_preset {
            let rgb = STAMP_COLORS[self.stamp_color_idx % STAMP_COLORS.len()].1;
            self.preview_rgba = Some(rasterize_text_stamp_rgba(&item.name, rgb, self.stamp_border));
        } else if let Some(ref p) = item.path {
            if let Ok(buf) = load_png_as_stamp_buffer(p) {
                self.preview_rgba = Some(buf);
            }
        }
    }
}

/// Blits a transparent RGBA `StampImageBuffer` centered & aspect-fitted inside `(rx, ry, rw, rh)`.
fn blit_rgba_fitted(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    rx: i32,
    ry: i32,
    rw: u32,
    rh: u32,
    img: &StampImageBuffer,
) {
    if img.width == 0 || img.height == 0 || rw < 4 || rh < 4 {
        return;
    }
    let scale = ((rw - 8) as f32 / img.width as f32)
        .min((rh - 8) as f32 / img.height as f32)
        .min(1.5);
    let dw = (img.width as f32 * scale).round().max(1.0) as u32;
    let dh = (img.height as f32 * scale).round().max(1.0) as u32;
    let ox = rx + ((rw.saturating_sub(dw)) / 2) as i32;
    let oy = ry + ((rh.saturating_sub(dh)) / 2) as i32;

    let rgba_img = img.image.to_rgba8();
    let raw = rgba_img.as_raw();
    for y in 0..dh {
        let py = oy + y as i32;
        if py < 0 || py >= buf_h as i32 {
            continue;
        }
        let sy = ((y as u64 * img.height as u64) / dh as u64).min(img.height as u64 - 1) as u32;
        let dst_row = (py as u32 * buf_w) as usize;
        for x in 0..dw {
            let px = ox + x as i32;
            if px < 0 || px >= buf_w as i32 {
                continue;
            }
            let sx = ((x as u64 * img.width as u64) / dw as u64).min(img.width as u64 - 1) as u32;
            let s_off = ((sy * img.width + sx) * 4) as usize;
            let a = raw[s_off + 3] as u32;
            if a == 0 {
                continue;
            }
            let sr = raw[s_off] as u32;
            let sg = raw[s_off + 1] as u32;
            let sb = raw[s_off + 2] as u32;
            let inv = 255 - a;
            let dst = buffer[dst_row + px as usize];
            let dr = (dst >> 16) & 0xFF;
            let dg = (dst >> 8) & 0xFF;
            let db = dst & 0xFF;
            buffer[dst_row + px as usize] =
                (((sr * a + dr * inv) / 255) << 16)
                    | (((sg * a + dg * inv) / 255) << 8)
                    | ((sb * a + db * inv) / 255);
        }
    }
}

impl<'a> PdfViewerApp<'a> {
    pub(super) fn open_stamp_manager_modal(&mut self) {
        if self.doc.is_none() {
            self.status_notice = Some("Open a PDF first to place a stamp or signature".to_string());
            self.request_redraw();
            return;
        }
        self.close_all_popovers();
        self.stamp_manager_modal = Some(StampManagerModalState::new());
        self.request_redraw();
    }

    fn stamp_manager_layout(
        &self,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<(i32, i32, u32, u32, u32, Vec<SmBox>)> {
        let st = self.stamp_manager_modal.as_ref()?;
        let s = sf as f32;
        let mw = ((760.0 * s).round() as u32).min(buf_w.saturating_sub(24)).max(560);
        let mh = ((510.0 * s).round() as u32).min(buf_h.saturating_sub(24)).max(430);
        let mx = ((buf_w.saturating_sub(mw)) / 2) as i32;
        let my = ((buf_h.saturating_sub(mh)) / 2) as i32;
        let left_w = ((mw as f32) * 0.37).round() as u32;
        let panel_x = mx + left_w as i32;
        let panel_w = mw.saturating_sub(left_w);
        let p = (14.0 * s).round() as i32;
        let hdr_h = (42.0 * s).round() as u32;
        let row_h = (28.0 * s).round() as u32;
        let gap = (6.0 * s).round() as i32;
        let sm = (24.0 * s).round() as u32;

        let mut hits = Vec::with_capacity(32);

        // Close X
        hits.push(SmBox {
            x: mx + mw as i32 - p - sm as i32,
            y: my + ((hdr_h.saturating_sub(sm)) / 2) as i32,
            w: sm,
            h: sm,
            hit: StampManagerHit::Close,
        });

        // Tabs in header: Signatures | Stamps
        let tab_w = (96.0 * s).round() as u32;
        let tab_y = my + ((hdr_h.saturating_sub(row_h)) / 2) as i32;
        let tab_x = mx + (205.0 * s).round() as i32;
        hits.push(SmBox { x: tab_x, y: tab_y, w: tab_w, h: row_h, hit: StampManagerHit::TabSignatures });
        hits.push(SmBox { x: tab_x + tab_w as i32 + 6, y: tab_y, w: tab_w, h: row_h, hit: StampManagerHit::TabStamps });

        // Left list of assets
        let list_x = mx + p;
        let list_w = left_w.saturating_sub((p * 2) as u32);
        let list_y = my + hdr_h as i32 + (12.0 * s).round() as i32;
        let item_h = (30.0 * s).round() as u32;
        let max_rows = ((mh.saturating_sub(hdr_h + (128.0 * s).round() as u32)) / (item_h + 4)).min(10) as usize;
        for i in 0..st.items.len().min(max_rows) {
            hits.push(SmBox {
                x: list_x,
                y: list_y + (i as i32) * (item_h as i32 + 4),
                w: list_w,
                h: item_h,
                hit: StampManagerHit::AssetRow(i),
            });
        }

        // Left bottom action bar: Import PNG / Rename / Delete
        let act_y = my + mh as i32 - p - row_h as i32;
        let btn3 = (list_w.saturating_sub((gap * 2) as u32)) / 3;
        hits.push(SmBox { x: list_x, y: act_y, w: btn3, h: row_h, hit: StampManagerHit::ImportPng });
        hits.push(SmBox { x: list_x + btn3 as i32 + gap, y: act_y, w: btn3, h: row_h, hit: StampManagerHit::RenameStart });
        hits.push(SmBox { x: list_x + (btn3 as i32 + gap) * 2, y: act_y, w: list_w.saturating_sub(btn3 * 2 + (gap * 2) as u32), h: row_h, hit: StampManagerHit::DeleteAsset });

        if st.renaming {
            let ren_y = act_y - row_h as i32 - 6;
            let ok_w = (46.0 * s).round() as u32;
            hits.push(SmBox { x: list_x, y: ren_y, w: list_w.saturating_sub(ok_w + 6), h: row_h, hit: StampManagerHit::FocusRename });
            hits.push(SmBox { x: list_x + list_w as i32 - ok_w as i32, y: ren_y, w: ok_w, h: row_h, hit: StampManagerHit::RenameConfirm });
        }

        // Right pane
        let rx = panel_x + p;
        let rw = panel_w.saturating_sub((p * 2) as u32);
        let mut cy = my + hdr_h as i32 + (12.0 * s).round() as i32;

        // Live Preview box (top of right pane: 120px)
        let prev_h = (120.0 * s).round() as u32;
        cy += prev_h as i32 + (12.0 * s).round() as i32;

        match st.tab {
            StampManagerTab::Signatures => {
                // Freehand Drawing Pad
                let pad_h = (158.0 * s).round() as u32;
                hits.push(SmBox { x: rx, y: cy + (16.0 * s).round() as i32, w: rw, h: pad_h, hit: StampManagerHit::DrawPad });
                cy += pad_h as i32 + (24.0 * s).round() as i32;
                // Ink color + Clear row
                let ink_w = (82.0 * s).round() as u32;
                let clr_w = (70.0 * s).round() as u32;
                hits.push(SmBox { x: rx, y: cy, w: ink_w, h: row_h, hit: StampManagerHit::InkBlack });
                hits.push(SmBox { x: rx + ink_w as i32 + gap, y: cy, w: ink_w, h: row_h, hit: StampManagerHit::InkBlue });
                hits.push(SmBox { x: rx + rw as i32 - clr_w as i32, y: cy, w: clr_w, h: row_h, hit: StampManagerHit::PadClear });
                cy += row_h as i32 + (8.0 * s).round() as i32;
                // Name + Save Signature row
                let save_w = (126.0 * s).round() as u32;
                hits.push(SmBox { x: rx, y: cy, w: rw.saturating_sub(save_w + gap as u32), h: row_h, hit: StampManagerHit::FocusSaveName });
                hits.push(SmBox { x: rx + rw as i32 - save_w as i32, y: cy, w: save_w, h: row_h, hit: StampManagerHit::PadSave });
            }
            StampManagerTab::Stamps => {
                cy += (16.0 * s).round() as i32;
                // 5 Color Swatches
                let sw_w = (rw.saturating_sub((gap * 4) as u32)) / 5;
                for i in 0..STAMP_COLORS.len() {
                    hits.push(SmBox {
                        x: rx + (i as i32) * (sw_w as i32 + gap),
                        y: cy,
                        w: sw_w,
                        h: row_h,
                        hit: StampManagerHit::StampColor(i),
                    });
                }
                cy += row_h as i32 + (24.0 * s).round() as i32;
                // 3 Border styles
                let b_w = (rw.saturating_sub((gap * 2) as u32)) / 3;
                for (i, bs) in [StampBorderStyle::Boxed, StampBorderStyle::Double, StampBorderStyle::Plain].iter().enumerate() {
                    hits.push(SmBox {
                        x: rx + (i as i32) * (b_w as i32 + gap),
                        y: cy,
                        w: b_w,
                        h: row_h,
                        hit: StampManagerHit::StampBorder(*bs),
                    });
                }
                cy += row_h as i32 + (28.0 * s).round() as i32;
                // Custom text stamp creator
                let save_w = (132.0 * s).round() as u32;
                hits.push(SmBox { x: rx, y: cy, w: rw.saturating_sub(save_w + gap as u32), h: row_h, hit: StampManagerHit::FocusCustomText });
                hits.push(SmBox { x: rx + rw as i32 - save_w as i32, y: cy, w: save_w, h: row_h, hit: StampManagerHit::SaveCustomStamp });
            }
        }

        // Primary Place on Page button (right footer)
        let place_h = (34.0 * s).round() as u32;
        let place_y = my + mh as i32 - p - place_h as i32;
        hits.push(SmBox { x: rx, y: place_y, w: rw, h: place_h, hit: StampManagerHit::PlaceSelected });

        Some((mx, my, mw, mh, left_w, hits))
    }

    pub(super) fn draw_stamp_manager_modal(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) {
        let Some(st) = self.stamp_manager_modal.as_ref() else {
            return;
        };
        let Some((mx, my, mw, mh, left_w, hits)) = self.stamp_manager_layout(buf_w, buf_h, sf) else {
            return;
        };
        let theme = self.ui_theme();
        let s = sf as f32;
        let p = (14.0 * s).round() as i32;
        let hdr_h = (42.0 * s).round() as u32;
        let rad = (10.0 * s).max(6.0);
        let btn_rad = (5.0 * s).max(3.0);
        let title_sz = (14.5 * s).clamp(12.0, 26.0);
        let body_sz = (11.2 * s).clamp(9.5, 20.0);
        let lbl_sz = (10.2 * s).clamp(9.0, 18.0);

        blend_rect_rgba(buffer, buf_w, buf_h, 0, 0, buf_w, buf_h, theme.modal_scrim);
        draw_soft_shadow(buffer, buf_w, buf_h, mx, my, mw, mh, rad, 18, 6, theme.shadow_rgb, theme.shadow_max_alpha);
        fill_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.border_subtle);

        draw_text_vcenter(buffer, buf_w, buf_h, mx + p, my, hdr_h, mx + (200.0 * s).round() as i32, "Stamps & Signatures", title_sz, theme.text_primary, true);
        fill_rect_clipped(buffer, buf_w, buf_h, mx, my + hdr_h as i32, mw, 1, theme.divider);
        fill_rect_clipped(buffer, buf_w, buf_h, mx + left_w as i32, my + hdr_h as i32, 1, mh.saturating_sub(hdr_h), theme.divider);

        let find = |target: StampManagerHit| hits.iter().copied().find(|b| b.hit == target);
        let draw_pill = |buf: &mut [u32], b: SmBox, lbl: &str, active: bool| {
            let hov = self.hovered_widget == Some(UiHit::StampManager(b.hit));
            let bg = if active { theme.accent } else if hov { theme.hover_bg } else { theme.surface_elevated };
            let fg = if active { theme.text_on_accent } else { theme.text_primary };
            let bdr = if active { theme.accent } else { theme.border_subtle };
            fill_rounded_rect(buf, buf_w, buf_h, b.x, b.y, b.w, b.h, btn_rad, bg);
            stroke_rounded_rect(buf, buf_w, buf_h, b.x, b.y, b.w, b.h, btn_rad, bdr);
            let tw = measure_text(lbl, body_sz, active);
            draw_text_vcenter(buf, buf_w, buf_h, b.x + ((b.w as f32 - tw) * 0.5).round() as i32, b.y, b.h, b.x + b.w as i32, lbl, body_sz, fg, active);
        };

        if let Some(cb) = find(StampManagerHit::Close) {
            draw_pill(buffer, cb, "x", false);
        }
        if let Some(ts) = find(StampManagerHit::TabSignatures) {
            draw_pill(buffer, ts, "Signatures", st.tab == StampManagerTab::Signatures);
        }
        if let Some(tst) = find(StampManagerHit::TabStamps) {
            draw_pill(buffer, tst, "Stamps", st.tab == StampManagerTab::Stamps);
        }

        // Left saved items list
        if st.items.is_empty() {
            let msg = "No saved signatures yet.\nDraw on the pad or Import PNG.";
            draw_text(buffer, buf_w, buf_h, mx + p, my + hdr_h as i32 + 24, msg, lbl_sz, theme.text_muted, false);
        } else {
            for (i, item) in st.items.iter().enumerate() {
                if let Some(rb) = find(StampManagerHit::AssetRow(i)) {
                    let sel = i == st.selected_idx;
                    let hov = self.hovered_widget == Some(UiHit::StampManager(rb.hit));
                    let bg = if sel { theme.accent_soft_bg } else if hov { theme.hover_bg } else { theme.surface_elevated };
                    let bdr = if sel { theme.accent } else { theme.border_subtle };
                    fill_rounded_rect(buffer, buf_w, buf_h, rb.x, rb.y, rb.w, rb.h, btn_rad, bg);
                    stroke_rounded_rect(buffer, buf_w, buf_h, rb.x, rb.y, rb.w, rb.h, btn_rad, bdr);
                    let badge = if item.is_builtin_preset { "[Preset] " } else { "" };
                    let lbl = format!("{badge}{}", item.name);
                    let clipped = truncate_ellipsis(&lbl, (rb.w.saturating_sub(14)) as f32, body_sz, sel);
                    draw_text_vcenter(buffer, buf_w, buf_h, rb.x + 8, rb.y, rb.h, rb.x + rb.w as i32 - 6, &clipped, body_sz, if sel { theme.accent } else { theme.text_primary }, sel);
                }
            }
        }

        // Left action buttons
        if let Some(ib) = find(StampManagerHit::ImportPng) {
            draw_pill(buffer, ib, "Import PNG", false);
        }
        if let Some(rb) = find(StampManagerHit::RenameStart) {
            draw_pill(buffer, rb, "Rename", st.renaming);
        }
        if let Some(db) = find(StampManagerHit::DeleteAsset) {
            draw_pill(buffer, db, "Delete", false);
        }
        if let (Some(rf), Some(rok)) = (find(StampManagerHit::FocusRename), find(StampManagerHit::RenameConfirm)) {
            let focused = st.focus == StampManagerFocus::RenameInput;
            fill_rounded_rect(buffer, buf_w, buf_h, rf.x, rf.y, rf.w, rf.h, btn_rad, theme.input_bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, rf.x, rf.y, rf.w, rf.h, btn_rad, if focused { theme.accent } else { theme.border_subtle });
            let disp = if focused { format!("{}|", st.rename_input) } else { st.rename_input.clone() };
            draw_text_vcenter(buffer, buf_w, buf_h, rf.x + 6, rf.y, rf.h, rf.x + rf.w as i32 - 4, &disp, body_sz, theme.text_primary, false);
            draw_pill(buffer, rok, "OK", true);
        }

        // Right pane: Live Preview box
        let rx = mx + left_w as i32 + p;
        let rw = mw.saturating_sub(left_w).saturating_sub((p * 2) as u32);
        let prev_y = my + hdr_h as i32 + (12.0 * s).round() as i32;
        let prev_h = (120.0 * s).round() as u32;
        fill_rounded_rect(buffer, buf_w, buf_h, rx, prev_y, rw, prev_h, 6.0 * s, 0x00FF_FFFF);
        stroke_rounded_rect(buffer, buf_w, buf_h, rx, prev_y, rw, prev_h, 6.0 * s, theme.border_subtle);
        if let Some(ref img) = st.preview_rgba {
            blit_rgba_fitted(buffer, buf_w, buf_h, rx, prev_y, rw, prev_h, img);
        } else {
            let msg = "Selected preview appears here";
            let tw = measure_text(msg, lbl_sz, false);
            draw_text_vcenter(buffer, buf_w, buf_h, rx + ((rw as f32 - tw) * 0.5).round() as i32, prev_y, prev_h, rx + rw as i32, msg, lbl_sz, theme.text_muted, false);
        }

        match st.tab {
            StampManagerTab::Signatures => {
                if let Some(pad) = find(StampManagerHit::DrawPad) {
                    draw_text(buffer, buf_w, buf_h, pad.x, pad.y - (16.0 * s).round() as i32, "Freehand Signature Pad (Click & Drag to Sign)", lbl_sz, theme.text_muted, true);
                    fill_rounded_rect(buffer, buf_w, buf_h, pad.x, pad.y, pad.w, pad.h, 6.0 * s, 0x00FF_FFFF);
                    stroke_rounded_rect(buffer, buf_w, buf_h, pad.x, pad.y, pad.w, pad.h, 6.0 * s, theme.accent);
                    // Subtle signature baseline
                    let base_y = pad.y + (pad.h as f32 * 0.78).round() as i32;
                    fill_rect_clipped(buffer, buf_w, buf_h, pad.x + 18, base_y, pad.w.saturating_sub(36), 1, 0x00E2_E8F0);
                    let rgb = st.ink_color.rgb();
                    let col = ((rgb.0 as u32) << 16) | ((rgb.1 as u32) << 8) | (rgb.2 as u32);
                    for stroke in &st.pad_strokes {
                        for win in stroke.windows(2) {
                            let x0 = pad.x as f32 + win[0].0 * pad.w as f32;
                            let y0 = pad.y as f32 + win[0].1 * pad.h as f32;
                            let x1 = pad.x as f32 + win[1].0 * pad.w as f32;
                            let y1 = pad.y as f32 + win[1].1 * pad.h as f32;
                            draw_line_aa(buffer, buf_w, buf_h, x0, y0, x1, y1, (2.6 * s).max(1.8), col);
                        }
                    }
                }
                if let Some(ib) = find(StampManagerHit::InkBlack) {
                    draw_pill(buffer, ib, "Black Ink", st.ink_color == SignatureInkColor::Black);
                }
                if let Some(ibl) = find(StampManagerHit::InkBlue) {
                    draw_pill(buffer, ibl, "Blue Ink", st.ink_color == SignatureInkColor::Blue);
                }
                if let Some(clr) = find(StampManagerHit::PadClear) {
                    draw_pill(buffer, clr, "Clear", false);
                }
                if let Some(sn) = find(StampManagerHit::FocusSaveName) {
                    let focused = st.focus == StampManagerFocus::SaveName;
                    fill_rounded_rect(buffer, buf_w, buf_h, sn.x, sn.y, sn.w, sn.h, btn_rad, theme.input_bg);
                    stroke_rounded_rect(buffer, buf_w, buf_h, sn.x, sn.y, sn.w, sn.h, btn_rad, if focused { theme.accent } else { theme.border_subtle });
                    let disp = if focused { format!("{}|", st.save_name) } else { st.save_name.clone() };
                    draw_text_vcenter(buffer, buf_w, buf_h, sn.x + 8, sn.y, sn.h, sn.x + sn.w as i32 - 4, &disp, body_sz, theme.text_primary, false);
                }
                if let Some(sv) = find(StampManagerHit::PadSave) {
                    draw_pill(buffer, sv, "Save Signature", false);
                }
            }
            StampManagerTab::Stamps => {
                for (i, &(_, rgb)) in STAMP_COLORS.iter().enumerate() {
                    if let Some(sb) = find(StampManagerHit::StampColor(i)) {
                        if i == 0 {
                            draw_text(buffer, buf_w, buf_h, sb.x, sb.y - (16.0 * s).round() as i32, "Stamp Color", lbl_sz, theme.text_muted, true);
                        }
                        let active = st.stamp_color_idx == i;
                        let col = ((rgb.0 as u32) << 16) | ((rgb.1 as u32) << 8) | (rgb.2 as u32);
                        fill_rounded_rect(buffer, buf_w, buf_h, sb.x, sb.y, sb.w, sb.h, btn_rad, col);
                        stroke_rounded_rect(buffer, buf_w, buf_h, sb.x, sb.y, sb.w, sb.h, btn_rad, if active { theme.accent } else { theme.border_subtle });
                        if active {
                            stroke_rounded_rect(buffer, buf_w, buf_h, sb.x + 2, sb.y + 2, sb.w.saturating_sub(4), sb.h.saturating_sub(4), 3.0, 0x00FF_FFFF);
                        }
                    }
                }
                for (bs, lbl) in [
                    (StampBorderStyle::Boxed, "Boxed Border"),
                    (StampBorderStyle::Double, "Double Border"),
                    (StampBorderStyle::Plain, "No Border"),
                ] {
                    if let Some(bb) = find(StampManagerHit::StampBorder(bs)) {
                        if bs == StampBorderStyle::Boxed {
                            draw_text(buffer, buf_w, buf_h, bb.x, bb.y - (16.0 * s).round() as i32, "Border Style", lbl_sz, theme.text_muted, true);
                        }
                        draw_pill(buffer, bb, lbl, st.stamp_border == bs);
                    }
                }
                if let Some(ct) = find(StampManagerHit::FocusCustomText) {
                    draw_text(buffer, buf_w, buf_h, ct.x, ct.y - (16.0 * s).round() as i32, "Create Custom Text Stamp", lbl_sz, theme.text_muted, true);
                    let focused = st.focus == StampManagerFocus::CustomStampText;
                    fill_rounded_rect(buffer, buf_w, buf_h, ct.x, ct.y, ct.w, ct.h, btn_rad, theme.input_bg);
                    stroke_rounded_rect(buffer, buf_w, buf_h, ct.x, ct.y, ct.w, ct.h, btn_rad, if focused { theme.accent } else { theme.border_subtle });
                    let disp = if focused { format!("{}|", st.custom_stamp_text) } else { st.custom_stamp_text.clone() };
                    draw_text_vcenter(buffer, buf_w, buf_h, ct.x + 8, ct.y, ct.h, ct.x + ct.w as i32 - 4, &disp, body_sz, theme.text_primary, false);
                }
                if let Some(sv) = find(StampManagerHit::SaveCustomStamp) {
                    draw_pill(buffer, sv, "Save Text Stamp", false);
                }
            }
        }

        if let Some((ref msg, is_err)) = st.status_msg {
            if let Some(pl) = find(StampManagerHit::PlaceSelected) {
                draw_text(buffer, buf_w, buf_h, pl.x, pl.y - (18.0 * s).round() as i32, msg, lbl_sz, if is_err { theme.danger } else { theme.accent }, true);
            }
        }

        if let Some(pl) = find(StampManagerHit::PlaceSelected) {
            let lbl = match st.tab {
                StampManagerTab::Signatures => "Place Selected Signature on Page",
                StampManagerTab::Stamps => "Place Selected Stamp on Page",
            };
            draw_pill(buffer, pl, lbl, true);
        }
    }

    pub(super) fn hit_test_stamp_manager_modal(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<StampManagerHit> {
        let (x, y, w, h, _, hits) = self.stamp_manager_layout(buf_w, buf_h, sf)?;
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix < x || ix >= x + w as i32 || iy < y || iy >= y + h as i32 {
            return None;
        }
        hits.into_iter()
            .find(|b| b.contains(ix, iy))
            .map(|b| b.hit)
            .or(Some(StampManagerHit::InsideModal))
    }

    pub(super) fn append_signature_pad_point(
        &mut self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
        start_new_stroke: bool,
    ) {
        let Some((_, _, _, _, _, hits)) = self.stamp_manager_layout(buf_w, buf_h, sf) else {
            return;
        };
        let Some(pad) = hits.into_iter().find(|b| b.hit == StampManagerHit::DrawPad) else {
            return;
        };
        let nx = ((mx as f32 - pad.x as f32) / pad.w.max(1) as f32).clamp(0.0, 1.0);
        let ny = ((my as f32 - pad.y as f32) / pad.h.max(1) as f32).clamp(0.0, 1.0);
        if let Some(st) = self.stamp_manager_modal.as_mut() {
            if start_new_stroke {
                st.pad_drawing = true;
                st.focus = StampManagerFocus::None;
                st.pad_strokes.push(vec![(nx, ny)]);
            } else if st.pad_drawing {
                if let Some(last) = st.pad_strokes.last_mut() {
                    last.push((nx, ny));
                }
            }
        }
        self.request_redraw();
    }

    pub(super) fn handle_stamp_manager_click(&mut self, hit: StampManagerHit) {
        match hit {
            StampManagerHit::Close => {
                self.stamp_manager_modal = None;
            }
            StampManagerHit::TabSignatures => {
                if let Some(st) = self.stamp_manager_modal.as_mut() {
                    st.tab = StampManagerTab::Signatures;
                    st.renaming = false;
                    st.status_msg = None;
                    st.reload_items();
                }
            }
            StampManagerHit::TabStamps => {
                if let Some(st) = self.stamp_manager_modal.as_mut() {
                    st.tab = StampManagerTab::Stamps;
                    st.renaming = false;
                    st.status_msg = None;
                    st.reload_items();
                }
            }
            StampManagerHit::AssetRow(i) => {
                if let Some(st) = self.stamp_manager_modal.as_mut() {
                    if i < st.items.len() {
                        st.selected_idx = i;
                        st.renaming = false;
                        st.refresh_preview();
                    }
                }
            }
            StampManagerHit::ImportPng => {
                self.pending_dialog = Some(PendingDialog::ImportStampAssetPng);
            }
            StampManagerHit::RenameStart => {
                if let Some(st) = self.stamp_manager_modal.as_mut() {
                    if let Some(item) = st.items.get(st.selected_idx) {
                        if item.is_builtin_preset {
                            st.status_msg = Some(("Built-in presets cannot be renamed".to_string(), true));
                        } else {
                            st.renaming = true;
                            st.rename_input = item.name.clone();
                            st.focus = StampManagerFocus::RenameInput;
                        }
                    }
                }
            }
            StampManagerHit::FocusRename => {
                if let Some(st) = self.stamp_manager_modal.as_mut() {
                    st.focus = StampManagerFocus::RenameInput;
                }
            }
            StampManagerHit::RenameConfirm => {
                if let Some(st) = self.stamp_manager_modal.as_mut() {
                    let new_stem = sanitize_asset_filename(&st.rename_input);
                    if let Some(old_path) = st.items.get(st.selected_idx).and_then(|it| it.path.clone()) {
                        let ext = old_path.extension().and_then(|e| e.to_str()).unwrap_or("png");
                        let new_path = old_path.with_file_name(format!("{new_stem}.{ext}"));
                        match std::fs::rename(&old_path, &new_path) {
                            Ok(()) => {
                                st.renaming = false;
                                st.focus = StampManagerFocus::None;
                                st.status_msg = Some((format!("Renamed to '{new_stem}'"), false));
                                st.reload_items();
                            }
                            Err(e) => {
                                st.status_msg = Some((format!("Rename failed: {e}"), true));
                            }
                        }
                    }
                }
            }
            StampManagerHit::DeleteAsset => {
                if let Some(st) = self.stamp_manager_modal.as_mut() {
                    if let Some(item) = st.items.get(st.selected_idx).cloned() {
                        if item.is_builtin_preset {
                            st.status_msg = Some(("Built-in presets cannot be deleted".to_string(), true));
                        } else if let Some(ref p) = item.path {
                            match std::fs::remove_file(p) {
                                Ok(()) => {
                                    st.status_msg = Some((format!("Deleted '{}'", item.name), false));
                                    st.reload_items();
                                }
                                Err(e) => {
                                    st.status_msg = Some((format!("Delete failed: {e}"), true));
                                }
                            }
                        }
                    }
                }
            }
            StampManagerHit::InkBlack => {
                if let Some(st) = self.stamp_manager_modal.as_mut() {
                    st.ink_color = SignatureInkColor::Black;
                }
            }
            StampManagerHit::InkBlue => {
                if let Some(st) = self.stamp_manager_modal.as_mut() {
                    st.ink_color = SignatureInkColor::Blue;
                }
            }
            StampManagerHit::PadClear => {
                if let Some(st) = self.stamp_manager_modal.as_mut() {
                    st.pad_strokes.clear();
                    st.status_msg = None;
                }
            }
            StampManagerHit::FocusSaveName => {
                if let Some(st) = self.stamp_manager_modal.as_mut() {
                    st.focus = StampManagerFocus::SaveName;
                }
            }
            StampManagerHit::PadSave => {
                if let Some(st) = self.stamp_manager_modal.as_mut() {
                    match rasterize_signature_pad_rgba(&st.pad_strokes, st.ink_color) {
                        Some(buf) => {
                            let stem = sanitize_asset_filename(&st.save_name);
                            let out = st.active_dir().join(format!("{stem}.png"));
                            match save_rgba_as_png(&out, &buf) {
                                Ok(()) => {
                                    st.pad_strokes.clear();
                                    st.status_msg = Some((format!("Saved signature '{stem}.png'"), false));
                                    st.reload_items();
                                    if let Some(pos) = st.items.iter().position(|it| it.name == stem) {
                                        st.selected_idx = pos;
                                        st.refresh_preview();
                                    }
                                }
                                Err(e) => st.status_msg = Some((format!("Save failed: {e}"), true)),
                            }
                        }
                        None => {
                            st.status_msg = Some(("Draw a signature on the pad first".to_string(), true));
                        }
                    }
                }
            }
            StampManagerHit::StampColor(i) => {
                if let Some(st) = self.stamp_manager_modal.as_mut() {
                    st.stamp_color_idx = i % STAMP_COLORS.len();
                    st.refresh_preview();
                }
            }
            StampManagerHit::StampBorder(bs) => {
                if let Some(st) = self.stamp_manager_modal.as_mut() {
                    st.stamp_border = bs;
                    st.refresh_preview();
                }
            }
            StampManagerHit::FocusCustomText => {
                if let Some(st) = self.stamp_manager_modal.as_mut() {
                    st.focus = StampManagerFocus::CustomStampText;
                }
            }
            StampManagerHit::SaveCustomStamp => {
                if let Some(st) = self.stamp_manager_modal.as_mut() {
                    let txt = st.custom_stamp_text.trim().to_string();
                    if txt.is_empty() {
                        st.status_msg = Some(("Enter custom stamp text first".to_string(), true));
                    } else {
                        let rgb = STAMP_COLORS[st.stamp_color_idx % STAMP_COLORS.len()].1;
                        let buf = rasterize_text_stamp_rgba(&txt, rgb, st.stamp_border);
                        let stem = sanitize_asset_filename(&txt);
                        let out = st.active_dir().join(format!("{stem}.png"));
                        match save_rgba_as_png(&out, &buf) {
                            Ok(()) => {
                                st.status_msg = Some((format!("Saved stamp '{stem}.png'"), false));
                                st.reload_items();
                                if let Some(pos) = st.items.iter().position(|it| it.name == stem) {
                                    st.selected_idx = pos;
                                    st.refresh_preview();
                                }
                            }
                            Err(e) => st.status_msg = Some((format!("Save failed: {e}"), true)),
                        }
                    }
                }
            }
            StampManagerHit::PlaceSelected => {
                let placement_buf = self.stamp_manager_modal.as_ref().and_then(|st| {
                    if let Some(ref p) = st.preview_rgba {
                        Some(p.clone())
                    } else if st.tab == StampManagerTab::Signatures {
                        rasterize_signature_pad_rgba(&st.pad_strokes, st.ink_color)
                    } else {
                        None
                    }
                });
                if let Some(buf) = placement_buf {
                    self.stamp_manager_modal = None;
                    self.pending_stamp_image = Some(buf);
                    self.mouse_mode = MouseMode::StampImage;
                    self.status_notice = Some(
                        "Click or drag on the page to place & size your stamp/signature (Esc cancels)"
                            .to_string(),
                    );
                    self.update_title_bar();
                } else if let Some(st) = self.stamp_manager_modal.as_mut() {
                    st.status_msg = Some(("Select, draw, or import an item first".to_string(), true));
                }
            }
            StampManagerHit::DrawPad | StampManagerHit::InsideModal => {
                if let Some(st) = self.stamp_manager_modal.as_mut() {
                    st.focus = StampManagerFocus::None;
                }
            }
        }
        self.request_redraw();
    }

    pub(super) fn handle_stamp_manager_key(&mut self, key: &winit::keyboard::Key, text: Option<&str>) {
        use winit::keyboard::{Key, NamedKey};
        match key {
            Key::Named(NamedKey::Escape) => {
                self.stamp_manager_modal = None;
                self.request_redraw();
            }
            Key::Named(NamedKey::Enter) => {
                if self.stamp_manager_modal.as_ref().is_some_and(|s| s.renaming) {
                    self.handle_stamp_manager_click(StampManagerHit::RenameConfirm);
                } else {
                    self.handle_stamp_manager_click(StampManagerHit::PlaceSelected);
                }
            }
            Key::Named(NamedKey::Backspace) => {
                if let Some(st) = self.stamp_manager_modal.as_mut() {
                    match st.focus {
                        StampManagerFocus::SaveName => {
                            st.save_name.pop();
                        }
                        StampManagerFocus::RenameInput => {
                            st.rename_input.pop();
                        }
                        StampManagerFocus::CustomStampText => {
                            st.custom_stamp_text.pop();
                        }
                        StampManagerFocus::None => {}
                    }
                }
                self.request_redraw();
            }
            _ => {
                if let (Some(t), Some(st)) = (text, self.stamp_manager_modal.as_mut()) {
                    match st.focus {
                        StampManagerFocus::SaveName => {
                            for ch in t.chars() {
                                if !ch.is_control() && st.save_name.len() < 32 {
                                    st.save_name.push(ch);
                                }
                            }
                        }
                        StampManagerFocus::RenameInput => {
                            for ch in t.chars() {
                                if !ch.is_control() && st.rename_input.len() < 32 {
                                    st.rename_input.push(ch);
                                }
                            }
                        }
                        StampManagerFocus::CustomStampText => {
                            for ch in t.chars() {
                                if !ch.is_control() && st.custom_stamp_text.len() < 32 {
                                    st.custom_stamp_text.push(ch);
                                }
                            }
                        }
                        StampManagerFocus::None => {}
                    }
                    self.request_redraw();
                }
            }
        }
    }

    pub(super) fn import_stamp_asset_png_dialog(&mut self) {
        let Some(target_dir) = self.stamp_manager_modal.as_ref().map(|s| s.active_dir()) else {
            return;
        };
        let Some(src_path) = rfd::FileDialog::new()
            .set_title("Import Transparent PNG Signature / Stamp")
            .add_filter("Images (PNG, JPG)", &["png", "jpg", "jpeg"])
            .pick_file()
        else {
            return;
        };
        match load_png_as_stamp_buffer(&src_path) {
            Ok(buf) => {
                let stem = src_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(sanitize_asset_filename)
                    .unwrap_or_else(|| "imported".to_string());
                let dst = target_dir.join(format!("{stem}.png"));
                if let Some(st) = self.stamp_manager_modal.as_mut() {
                    match save_rgba_as_png(&dst, &buf) {
                        Ok(()) => {
                            st.status_msg = Some((format!("Imported '{stem}.png'"), false));
                            st.reload_items();
                            if let Some(pos) = st.items.iter().position(|it| it.name == stem) {
                                st.selected_idx = pos;
                                st.refresh_preview();
                            }
                        }
                        Err(e) => st.status_msg = Some((format!("Import save failed: {e}"), true)),
                    }
                }
            }
            Err(e) => {
                if let Some(st) = self.stamp_manager_modal.as_mut() {
                    st.status_msg = Some((format!("Failed to load image: {e}"), true));
                }
            }
        }
        self.request_redraw();
    }
}

//! Lightweight CPU 2D UI primitives, crisp procedural icons, and antialiased proportional
//! typography (powered by `ab_glyph` + bounded alpha-only glyph cache).
//! Draws directly into the existing `softbuffer` `0x00RRGGBB` pixel slice with zero GPU overhead.

use ab_glyph::{Font, FontVec, GlyphId, PxScale, ScaleFont};
use std::collections::HashMap;
use std::io::Read;
use std::sync::{Mutex, OnceLock};

/// Maximum memory allowed for cached alpha-only glyph bitmaps (512 KB).
pub const MAX_GLYPH_CACHE_BYTES: usize = 512 * 1024;

static REGULAR_FONT_Z: &[u8] = include_bytes!("../../assets/ui_regular.ttf.z");
static BOLD_FONT_Z: &[u8] = include_bytes!("../../assets/ui_bold.ttf.z");
static BOOK_SERIF_REGULAR_Z: &[u8] = include_bytes!("../../assets/book_serif_regular.ttf.z");
static BOOK_SERIF_BOLD_Z: &[u8] = include_bytes!("../../assets/book_serif_bold.ttf.z");
static BOOK_SERIF_ITALIC_Z: &[u8] = include_bytes!("../../assets/book_serif_italic.ttf.z");
static CODE_MONO_REGULAR_Z: &[u8] = include_bytes!("../../assets/code_mono_regular.ttf.z");

/// Supported typography families for MicroPDF's CPU text engine.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum FontFamily {
    UiSans,
    BookSerif,
    CodeMono,
    CustomOs,
}

/// Complete styled text parameters for `measure_text_styled` and `draw_text_styled_clipped`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct TextStyle {
    pub family: FontFamily,
    pub size_px: f32,
    pub bold: bool,
    pub italic: bool,
}

impl TextStyle {
    pub const fn ui(size_px: f32, bold: bool) -> Self {
        Self {
            family: FontFamily::UiSans,
            size_px,
            bold,
            italic: false,
        }
    }

    #[allow(dead_code)]
    pub const fn book(size_px: f32, bold: bool, italic: bool) -> Self {
        Self {
            family: FontFamily::BookSerif,
            size_px,
            bold,
            italic,
        }
    }

    #[allow(dead_code)]
    pub const fn mono(size_px: f32) -> Self {
        Self {
            family: FontFamily::CodeMono,
            size_px,
            bold: false,
            italic: false,
        }
    }
}

impl Default for TextStyle {
    fn default() -> Self {
        Self::ui(12.0, false)
    }
}

struct CustomOsFontSlot {
    requested_name: Option<String>,
    font: Option<FontVec>,
    is_native_italic: bool,
}

impl CustomOsFontSlot {
    fn empty() -> Self {
        Self {
            requested_name: None,
            font: None,
            is_native_italic: false,
        }
    }
}

struct FontBank {
    regular: Option<FontVec>,
    bold: Option<FontVec>,
    book_regular: OnceLock<Option<FontVec>>,
    book_bold: OnceLock<Option<FontVec>>,
    book_italic: OnceLock<Option<FontVec>>,
    code_mono: OnceLock<Option<FontVec>>,
    custom_os: Mutex<CustomOsFontSlot>,
    fallback: Mutex<Option<Option<FontVec>>>,
    cache: Mutex<GlyphCache>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
struct GlyphKey {
    ch: char,
    size_q4: u16,
    family: FontFamily,
    bold: bool,
    italic: bool,
}

struct CachedGlyph {
    width: u32,
    height: u32,
    left: i32,
    top: i32,
    advance: f32,
    gid: u16,
    from_primary: bool,
    alpha: Vec<u8>,
}

struct GlyphCache {
    entries: HashMap<GlyphKey, CachedGlyph>,
    total_bytes: usize,
}

impl GlyphCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            total_bytes: 0,
        }
    }

    #[cfg(test)]
    fn byte_len(&self) -> usize {
        self.total_bytes
    }
}

fn decompress_font(compressed: &[u8]) -> Option<FontVec> {
    let mut decoder = flate2::read::ZlibDecoder::new(compressed);
    let mut raw = Vec::new();
    decoder.read_to_end(&mut raw).ok()?;
    FontVec::try_from_vec(raw).ok()
}

fn load_os_fallback_font() -> Option<FontVec> {
    let candidates: &[&str] = &[
        #[cfg(target_os = "windows")]
        r"C:\Windows\Fonts\Nirmala.ttf",
        #[cfg(target_os = "windows")]
        r"C:\Windows\Fonts\segoeui.ttf",
        #[cfg(target_os = "windows")]
        r"C:\Windows\Fonts\arial.ttf",
        #[cfg(target_os = "macos")]
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        #[cfg(target_os = "macos")]
        "/System/Library/Fonts/KohinoorBangla.ttc",
        #[cfg(target_os = "macos")]
        "/System/Library/Fonts/Helvetica.ttc",
        #[cfg(target_os = "linux")]
        "/usr/share/fonts/truetype/freefont/FreeSans.ttf",
        #[cfg(target_os = "linux")]
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        #[cfg(target_os = "linux")]
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    ];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(font) = FontVec::try_from_vec(bytes) {
                return Some(font);
            }
        }
    }
    None
}

fn font_bank() -> &'static FontBank {
    static BANK: OnceLock<FontBank> = OnceLock::new();
    BANK.get_or_init(|| FontBank {
        regular: decompress_font(REGULAR_FONT_Z),
        bold: decompress_font(BOLD_FONT_Z),
        book_regular: OnceLock::new(),
        book_bold: OnceLock::new(),
        book_italic: OnceLock::new(),
        code_mono: OnceLock::new(),
        custom_os: Mutex::new(CustomOsFontSlot::empty()),
        fallback: Mutex::new(None),
        cache: Mutex::new(GlyphCache::new()),
    })
}

/// Returns `(font_ref, is_native_italic)` for a built-in embedded font family.
fn resolve_builtin_font(
    family: FontFamily,
    bold: bool,
    italic: bool,
) -> (Option<&'static FontVec>, bool) {
    let bank = font_bank();
    match family {
        FontFamily::UiSans => {
            let f = if bold {
                bank.bold.as_ref().or(bank.regular.as_ref())
            } else {
                bank.regular.as_ref()
            };
            (f, false)
        }
        FontFamily::BookSerif | FontFamily::CustomOs => {
            if italic && !bold {
                let f = bank
                    .book_italic
                    .get_or_init(|| decompress_font(BOOK_SERIF_ITALIC_Z))
                    .as_ref();
                if f.is_some() {
                    return (f, true);
                }
            }
            if bold {
                let f = bank
                    .book_bold
                    .get_or_init(|| decompress_font(BOOK_SERIF_BOLD_Z))
                    .as_ref();
                if f.is_some() {
                    return (f, false);
                }
            }
            let f = bank
                .book_regular
                .get_or_init(|| decompress_font(BOOK_SERIF_REGULAR_Z))
                .as_ref();
            (f, false)
        }
        FontFamily::CodeMono => {
            let f = bank
                .code_mono
                .get_or_init(|| decompress_font(CODE_MONO_REGULAR_Z))
                .as_ref();
            (f, false)
        }
    }
}

/// Lazily loads (or clears when `None`) an OS TrueType/OpenType font into the `CustomOs` slot
/// using `crate::fonts::global_system_fonts()`. Uses 0 bytes of RAM until called with `Some(...)`.
#[allow(dead_code)]
pub fn set_custom_os_font(font_name: Option<&str>) -> bool {
    let bank = font_bank();
    let trimmed = font_name.map(|s| s.trim()).filter(|s| !s.is_empty());

    let Some(name) = trimmed else {
        if let Ok(mut guard) = bank.custom_os.lock() {
            *guard = CustomOsFontSlot::empty();
        }
        evict_custom_os_glyphs();
        return true;
    };

    if let Ok(guard) = bank.custom_os.lock() {
        if guard
            .requested_name
            .as_deref()
            .is_some_and(|n| n.eq_ignore_ascii_case(name))
            && guard.font.is_some()
        {
            return true;
        }
    }

    let catalog = crate::fonts::global_system_fonts();
    let needle = name.to_ascii_lowercase();
    let matched = catalog
        .iter()
        .find(|e| e.path.is_some() && e.display_name.eq_ignore_ascii_case(name))
        .or_else(|| {
            catalog.iter().find(|e| {
                e.path.is_some() && e.display_name.to_ascii_lowercase().starts_with(&needle)
            })
        })
        .or_else(|| {
            catalog.iter().find(|e| {
                e.path.is_some() && e.display_name.to_ascii_lowercase().contains(&needle)
            })
        });

    let loaded = matched.and_then(|entry| {
        let path = entry.path.as_ref()?;
        let bytes = std::fs::read(path).ok()?;
        let font = FontVec::try_from_vec(bytes).ok()?;
        let lower = entry.display_name.to_ascii_lowercase();
        let is_italic = lower.contains("italic") || lower.contains("oblique");
        Some((entry.display_name.clone(), font, is_italic))
    });

    let ok = loaded.is_some();
    if let Ok(mut guard) = bank.custom_os.lock() {
        if let Some((display_name, font, is_native_italic)) = loaded {
            *guard = CustomOsFontSlot {
                requested_name: Some(display_name),
                font: Some(font),
                is_native_italic,
            };
        } else {
            *guard = CustomOsFontSlot::empty();
        }
    }
    evict_custom_os_glyphs();
    ok
}

#[allow(dead_code)]
pub fn active_custom_os_font_name() -> Option<String> {
    font_bank()
        .custom_os
        .lock()
        .ok()
        .and_then(|g| g.requested_name.clone())
}

fn evict_custom_os_glyphs() {
    let bank = font_bank();
    let mut cache = bank.cache.lock().unwrap_or_else(|e| e.into_inner());
    let mut removed_bytes = 0usize;
    cache.entries.retain(|k, v| {
        if k.family == FontFamily::CustomOs {
            removed_bytes += v.alpha.len()
                + std::mem::size_of::<CachedGlyph>()
                + std::mem::size_of::<GlyphKey>()
                + 16;
            false
        } else {
            true
        }
    });
    cache.total_bytes = cache.total_bytes.saturating_sub(removed_bytes);
}

fn apply_synthetic_italic(
    w: u32,
    h: u32,
    left: i32,
    top: i32,
    ascent: f32,
    alpha: Vec<u8>,
) -> (u32, i32, Vec<u8>) {
    if w == 0 || h == 0 {
        return (w, left, alpha);
    }
    let shear = 0.21f32;
    let y_top_rel = (top as f32) - ascent;
    let y_bot_rel = ((top + h as i32) as f32) - ascent;
    let shift_top = -y_top_rel * shear;
    let shift_bot = -y_bot_rel * shear;
    let min_shift = shift_top.min(shift_bot).floor() as i32;
    let max_shift = shift_top.max(shift_bot).ceil() as i32;
    let extra_w = (max_shift - min_shift).max(0) as u32;
    let new_w = w + extra_w;
    let mut out = vec![0u8; (new_w * h) as usize];

    for row in 0..h {
        let y_rel = (top + row as i32) as f32 - ascent;
        let dx = -y_rel * shear - (min_shift as f32);
        let dx_i = dx.floor() as i32;
        let frac = dx - (dx_i as f32);
        let src_row = (row * w) as usize;
        let dst_row = (row * new_w) as usize;
        for col in 0..w {
            let a = alpha[src_row + col as usize] as f32;
            if a == 0.0 {
                continue;
            }
            let c0 = col as i32 + dx_i;
            let c1 = c0 + 1;
            if c0 >= 0 && (c0 as u32) < new_w {
                let idx = dst_row + c0 as usize;
                let v = (out[idx] as f32 + a * (1.0 - frac)).round().clamp(0.0, 255.0) as u8;
                out[idx] = v;
            }
            if c1 >= 0 && (c1 as u32) < new_w {
                let idx = dst_row + c1 as usize;
                let v = (out[idx] as f32 + a * frac).round().clamp(0.0, 255.0) as u8;
                out[idx] = v;
            }
        }
    }
    (new_w, left + min_shift, out)
}

fn rasterize_glyph_with_font(
    font: &FontVec,
    ch: char,
    scale: PxScale,
    synthetic_italic: bool,
    from_primary: bool,
) -> Option<CachedGlyph> {
    let gid = font.glyph_id(ch);
    if gid.0 == 0 && ch != ' ' && ch != '\t' {
        return None;
    }
    let scaled = font.as_scaled(scale);
    let advance = scaled.h_advance(gid);
    let ascent = scaled.ascent();
    let glyph = gid.with_scale_and_position(scale, ab_glyph::point(0.0, ascent));
    if let Some(outlined) = font.outline_glyph(glyph) {
        let bounds = outlined.px_bounds();
        let w = (bounds.max.x - bounds.min.x).ceil().max(0.0) as u32;
        let h = (bounds.max.y - bounds.min.y).ceil().max(0.0) as u32;
        let mut alpha = vec![0u8; (w * h) as usize];
        outlined.draw(|gx, gy, cov| {
            if gx < w && gy < h {
                let a = (cov.clamp(0.0, 1.0) * 255.0).round() as u8;
                alpha[(gy * w + gx) as usize] = a;
            }
        });
        let left = bounds.min.x.floor() as i32;
        let top = bounds.min.y.floor() as i32;
        let (final_w, final_left, final_alpha) = if synthetic_italic {
            apply_synthetic_italic(w, h, left, top, ascent, alpha)
        } else {
            (w, left, alpha)
        };
        Some(CachedGlyph {
            width: final_w,
            height: h,
            left: final_left,
            top,
            advance,
            gid: gid.0,
            from_primary,
            alpha: final_alpha,
        })
    } else {
        Some(CachedGlyph {
            width: 0,
            height: 0,
            left: 0,
            top: 0,
            advance,
            gid: gid.0,
            from_primary,
            alpha: Vec::new(),
        })
    }
}

fn rasterize_glyph_styled(ch: char, size_px: f32, style: TextStyle) -> CachedGlyph {
    let bank = font_bank();
    let scale = PxScale::from(size_px.max(6.0));

    // Try CustomOs font slot first if requested
    if style.family == FontFamily::CustomOs {
        if let Ok(guard) = bank.custom_os.lock() {
            if let Some(ref custom_font) = guard.font {
                let synth_italic = style.italic && !guard.is_native_italic;
                if let Some(g) = rasterize_glyph_with_font(custom_font, ch, scale, synth_italic, true) {
                    return g;
                }
            }
        }
    }

    let (primary, is_native_italic) = resolve_builtin_font(style.family, style.bold, style.italic);
    let synth_italic = style.italic && !is_native_italic;

    if let Some(font) = primary {
        if let Some(g) = rasterize_glyph_with_font(font, ch, scale, synth_italic, true) {
            return g;
        }
    }

    // Lazy OS fallback only when a non-Latin character is missing from the embedded font.
    if !ch.is_ascii() {
        if let Ok(mut fb_guard) = bank.fallback.lock() {
            let fb_ref = fb_guard.get_or_insert_with(load_os_fallback_font);
            if let Some(font) = fb_ref.as_ref() {
                if let Some(g) = rasterize_glyph_with_font(font, ch, scale, synth_italic, false) {
                    return g;
                }
            }
        }
    }

    // Final fallback: space or missing-glyph box using primary font's space advance
    let adv = primary
        .map(|f| f.as_scaled(scale).h_advance(f.glyph_id(' ')))
        .unwrap_or(size_px * 0.5);
    CachedGlyph {
        width: 0,
        height: 0,
        left: 0,
        top: 0,
        advance: adv,
        gid: 0,
        from_primary: false,
        alpha: Vec::new(),
    }
}

fn snap_size_q4(size_px: f32) -> (u16, f32) {
    let q4 = (size_px.clamp(6.0, 96.0) * 4.0).round() as u16;
    (q4, (q4 as f32) / 4.0)
}

fn with_cached_glyph_styled<R>(
    ch: char,
    style: TextStyle,
    f: impl FnOnce(&CachedGlyph) -> R,
) -> R {
    let (size_q4, snapped_px) = snap_size_q4(style.size_px);
    let key = GlyphKey {
        ch,
        size_q4,
        family: style.family,
        bold: style.bold,
        italic: style.italic,
    };
    let bank = font_bank();
    let mut cache = bank.cache.lock().unwrap_or_else(|e| e.into_inner());
    if !cache.entries.contains_key(&key) {
        let glyph = rasterize_glyph_styled(ch, snapped_px, style);
        let bytes = glyph.alpha.len()
            + std::mem::size_of::<CachedGlyph>()
            + std::mem::size_of::<GlyphKey>()
            + 16;
        if cache.total_bytes + bytes > MAX_GLYPH_CACHE_BYTES {
            cache.entries.clear();
            cache.total_bytes = 0;
        }
        cache.total_bytes += bytes;
        cache.entries.insert(key, glyph);
    }
    f(&cache.entries[&key])
}

fn kern_pair_for_style(style: TextStyle, scale: PxScale, prev_gid: u16, cur_gid: u16) -> f32 {
    if prev_gid == 0 || cur_gid == 0 {
        return 0.0;
    }
    let g1 = GlyphId(prev_gid);
    let g2 = GlyphId(cur_gid);
    if style.family == FontFamily::CustomOs {
        if let Ok(guard) = font_bank().custom_os.lock() {
            if let Some(ref custom_font) = guard.font {
                return custom_font.as_scaled(scale).kern(g1, g2);
            }
        }
    }
    let (primary, _) = resolve_builtin_font(style.family, style.bold, style.italic);
    primary
        .map(|f| f.as_scaled(scale).kern(g1, g2))
        .unwrap_or(0.0)
}

#[cfg(test)]
pub fn glyph_cache_bytes() -> usize {
    font_bank()
        .cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .byte_len()
}

/// Measures the horizontal advance width of `text` using `style` (including real kerning pairs).
pub fn measure_text_styled(text: &str, style: TextStyle) -> f32 {
    let (_, snapped_px) = snap_size_q4(style.size_px);
    let scale = PxScale::from(snapped_px);
    let mut w = 0.0f32;
    let mut prev_gid: Option<u16> = None;
    for ch in text.chars() {
        let (adv, gid, from_primary) =
            with_cached_glyph_styled(ch, style, |g| (g.advance, g.gid, g.from_primary));
        if from_primary {
            if let Some(pg) = prev_gid {
                w += kern_pair_for_style(style, scale, pg, gid);
            }
            prev_gid = Some(gid);
        } else {
            prev_gid = None;
        }
        w += adv;
    }
    w
}

/// Measures the horizontal advance width of `text` at `size_px` using `UiSans`.
pub fn measure_text(text: &str, size_px: f32, bold: bool) -> f32 {
    measure_text_styled(text, TextStyle::ui(size_px, bold))
}

/// Truncates `text` with `"…"` so its styled measured width does not exceed `max_w`.
#[allow(dead_code)]
pub fn truncate_ellipsis_styled(text: &str, max_w: f32, style: TextStyle) -> String {
    if max_w <= 4.0 {
        return String::new();
    }
    if measure_text_styled(text, style) <= max_w {
        return text.to_string();
    }
    let (_, snapped_px) = snap_size_q4(style.size_px);
    let scale = PxScale::from(snapped_px);
    let ell = "…";
    let ell_w = measure_text_styled(ell, style);
    let budget = (max_w - ell_w).max(0.0);
    let mut out = String::new();
    let mut cur_w = 0.0f32;
    let mut prev_gid: Option<u16> = None;
    for ch in text.chars() {
        let (adv, gid, from_primary) =
            with_cached_glyph_styled(ch, style, |g| (g.advance, g.gid, g.from_primary));
        let k = if from_primary {
            prev_gid
                .map(|pg| kern_pair_for_style(style, scale, pg, gid))
                .unwrap_or(0.0)
        } else {
            0.0
        };
        if cur_w + k + adv > budget {
            break;
        }
        cur_w += k + adv;
        prev_gid = if from_primary { Some(gid) } else { None };
        out.push(ch);
    }
    out.push_str(ell);
    out
}

/// Truncates `text` with `"..."` so its measured width does not exceed `max_w`.
pub fn truncate_ellipsis(text: &str, max_w: f32, size_px: f32, bold: bool) -> String {
    truncate_ellipsis_styled(text, max_w, TextStyle::ui(size_px, bold))
}

/// Draws antialiased proportional styled text (with real kerning pairs) starting at `(x, y_top)`
/// and clipped horizontally to `max_x`. Returns the x-coordinate after the last drawn glyph.
#[allow(clippy::too_many_arguments)]
pub fn draw_text_styled_clipped(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    x: i32,
    y_top: i32,
    max_x: i32,
    text: &str,
    style: TextStyle,
    color: u32,
) -> i32 {
    let sr = (color >> 16) & 0xFF;
    let sg = (color >> 8) & 0xFF;
    let sb = color & 0xFF;
    let clip_right = max_x.min(buf_w as i32);
    let (_, snapped_px) = snap_size_q4(style.size_px);
    let scale = PxScale::from(snapped_px);
    let mut pen_x = x as f32;
    let mut prev_gid: Option<u16> = None;

    for ch in text.chars() {
        if pen_x.floor() as i32 >= clip_right {
            break;
        }
        let (gid, from_primary) =
            with_cached_glyph_styled(ch, style, |g| (g.gid, g.from_primary));
        if from_primary {
            if let Some(pg) = prev_gid {
                pen_x += kern_pair_for_style(style, scale, pg, gid);
            }
            prev_gid = Some(gid);
        } else {
            prev_gid = None;
        }
        if pen_x.floor() as i32 >= clip_right {
            break;
        }
        with_cached_glyph_styled(ch, style, |g| {
            let gx0 = pen_x.round() as i32 + g.left;
            let gy0 = y_top + g.top;
            for row in 0..g.height {
                let py = gy0 + row as i32;
                if py < 0 || py >= buf_h as i32 {
                    continue;
                }
                let dst_row = (py as u32 * buf_w) as usize;
                let src_row = (row * g.width) as usize;
                for col in 0..g.width {
                    let px = gx0 + col as i32;
                    if px < 0 || px >= clip_right {
                        continue;
                    }
                    let a = g.alpha[src_row + col as usize] as u32;
                    if a == 0 {
                        continue;
                    }
                    let idx = dst_row + px as usize;
                    if a == 255 {
                        buffer[idx] = color;
                    } else {
                        let dst = buffer[idx];
                        let dr = (dst >> 16) & 0xFF;
                        let dg = (dst >> 8) & 0xFF;
                        let db = dst & 0xFF;
                        let inv = 255 - a;
                        let out_r = (sr * a + dr * inv + 127) / 255;
                        let out_g = (sg * a + dg * inv + 127) / 255;
                        let out_b = (sb * a + db * inv + 127) / 255;
                        buffer[idx] = (out_r << 16) | (out_g << 8) | out_b;
                    }
                }
            }
            pen_x += g.advance;
        });
    }
    pen_x.round() as i32
}

/// Draws antialiased proportional text starting at `(x, y_top)` and clipped horizontally to `max_x`.
/// Returns the x-coordinate after the last drawn glyph.
#[allow(clippy::too_many_arguments)]
pub fn draw_text_clipped(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    x: i32,
    y_top: i32,
    max_x: i32,
    text: &str,
    size_px: f32,
    color: u32,
    bold: bool,
) -> i32 {
    draw_text_styled_clipped(
        buffer,
        buf_w,
        buf_h,
        x,
        y_top,
        max_x,
        text,
        TextStyle::ui(size_px, bold),
        color,
    )
}

/// Convenience wrapper without an explicit right clip beyond `buf_w`.
#[allow(clippy::too_many_arguments)]
pub fn draw_text(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    x: i32,
    y_top: i32,
    text: &str,
    size_px: f32,
    color: u32,
    bold: bool,
) -> i32 {
    draw_text_clipped(buffer, buf_w, buf_h, x, y_top, buf_w as i32, text, size_px, color, bold)
}

/// Draws styled text vertically centered inside `(box_y, box_y + box_h)`.
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
pub fn draw_text_vcenter_styled(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    x: i32,
    box_y: i32,
    box_h: u32,
    max_x: i32,
    text: &str,
    style: TextStyle,
    color: u32,
) -> i32 {
    let y_top = box_y + ((box_h as f32 - style.size_px * 1.15) * 0.5).round() as i32;
    draw_text_styled_clipped(buffer, buf_w, buf_h, x, y_top, max_x, text, style, color)
}

/// Draws text vertically centered inside `(y, y + h)`.
#[allow(clippy::too_many_arguments)]
pub fn draw_text_vcenter(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    x: i32,
    box_y: i32,
    box_h: u32,
    max_x: i32,
    text: &str,
    size_px: f32,
    color: u32,
    bold: bool,
) -> i32 {
    let y_top = box_y + ((box_h as f32 - size_px * 1.15) * 0.5).round() as i32;
    draw_text_clipped(buffer, buf_w, buf_h, x, y_top, max_x, text, size_px, color, bold)
}

#[inline]
pub fn blend_pixel(dst: u32, color: u32, alpha: u32) -> u32 {
    if alpha == 0 {
        return dst;
    }
    if alpha >= 255 {
        return color;
    }
    let inv = 255 - alpha;
    let sr = (color >> 16) & 0xFF;
    let sg = (color >> 8) & 0xFF;
    let sb = color & 0xFF;
    let dr = (dst >> 16) & 0xFF;
    let dg = (dst >> 8) & 0xFF;
    let db = dst & 0xFF;
    let r = (sr * alpha + dr * inv + 127) / 255;
    let g = (sg * alpha + dg * inv + 127) / 255;
    let b = (sb * alpha + db * inv + 127) / 255;
    (r << 16) | (g << 8) | b
}

#[inline]
pub fn put_pixel_alpha(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    x: i32,
    y: i32,
    color: u32,
    alpha: u32,
) {
    if x < 0 || y < 0 || x >= buf_w as i32 || y >= buf_h as i32 || alpha == 0 {
        return;
    }
    let idx = (y as u32 * buf_w + x as u32) as usize;
    buffer[idx] = blend_pixel(buffer[idx], color, alpha);
}

/// Antialiased rounded rectangle fill.
#[allow(clippy::too_many_arguments)]
pub fn fill_rounded_rect(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    rx: i32,
    ry: i32,
    rw: u32,
    rh: u32,
    radius: f32,
    color: u32,
) {
    fill_rounded_rect_alpha(buffer, buf_w, buf_h, rx, ry, rw, rh, radius, color, 255);
}

/// Antialiased rounded rectangle fill with opacity `alpha` (`0..=255`).
#[allow(clippy::too_many_arguments)]
pub fn fill_rounded_rect_alpha(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    rx: i32,
    ry: i32,
    rw: u32,
    rh: u32,
    radius: f32,
    color: u32,
    alpha: u32,
) {
    if rw == 0 || rh == 0 || alpha == 0 {
        return;
    }
    let r = radius.clamp(0.0, (rw.min(rh) as f32) * 0.5);
    let x0 = rx.max(0);
    let y0 = ry.max(0);
    let x1 = (rx + rw as i32).min(buf_w as i32);
    let y1 = (ry + rh as i32).min(buf_h as i32);
    if x1 <= x0 || y1 <= y0 {
        return;
    }

    let left = rx as f32;
    let top = ry as f32;
    let right = (rx + rw as i32) as f32;
    let bottom = (ry + rh as i32) as f32;
    let r_ceil = r.ceil() as i32;

    for y in y0..y1 {
        let in_top_band = y < ry + r_ceil;
        let in_bot_band = y >= ry + rh as i32 - r_ceil;
        let row_start = (y as u32 * buf_w) as usize;

        if !in_top_band && !in_bot_band {
            if alpha == 255 {
                buffer[row_start + x0 as usize..row_start + x1 as usize].fill(color);
            } else {
                for x in x0..x1 {
                    let idx = row_start + x as usize;
                    buffer[idx] = blend_pixel(buffer[idx], color, alpha);
                }
            }
            continue;
        }

        let py = y as f32 + 0.5;
        let cy = if in_top_band { top + r } else { bottom - r };
        let dy = py - cy;

        for x in x0..x1 {
            let px = x as f32 + 0.5;
            let cov = if px < left + r {
                let dx = px - (left + r);
                (r + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0)
            } else if px > right - r {
                let dx = px - (right - r);
                (r + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0)
            } else {
                1.0
            };
            if cov > 0.0 {
                let a = ((cov * alpha as f32).round() as u32).min(255);
                let idx = row_start + x as usize;
                buffer[idx] = blend_pixel(buffer[idx], color, a);
            }
        }
    }
}

/// Antialiased 1px rounded rectangle border.
#[allow(clippy::too_many_arguments)]
pub fn stroke_rounded_rect(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    rx: i32,
    ry: i32,
    rw: u32,
    rh: u32,
    radius: f32,
    color: u32,
) {
    if rw < 2 || rh < 2 {
        return;
    }
    let r = radius.clamp(0.0, (rw.min(rh) as f32) * 0.5);
    let x0 = rx.max(0);
    let y0 = ry.max(0);
    let x1 = (rx + rw as i32).min(buf_w as i32);
    let y1 = (ry + rh as i32).min(buf_h as i32);
    if x1 <= x0 || y1 <= y0 {
        return;
    }

    let left = rx as f32;
    let top = ry as f32;
    let right = (rx + rw as i32) as f32;
    let bottom = (ry + rh as i32) as f32;
    let r_ceil = r.ceil() as i32 + 1;

    // Straight top & bottom edges between corners
    let sx0 = (rx + r.ceil() as i32).max(0);
    let sx1 = (rx + rw as i32 - r.ceil() as i32).min(buf_w as i32);
    if sx1 > sx0 {
        if ry >= 0 && ry < buf_h as i32 {
            let row = (ry as u32 * buf_w) as usize;
            buffer[row + sx0 as usize..row + sx1 as usize].fill(color);
        }
        let by = ry + rh as i32 - 1;
        if by >= 0 && by < buf_h as i32 {
            let row = (by as u32 * buf_w) as usize;
            buffer[row + sx0 as usize..row + sx1 as usize].fill(color);
        }
    }

    // Straight left & right edges between corners
    let sy0 = (ry + r.ceil() as i32).max(0);
    let sy1 = (ry + rh as i32 - r.ceil() as i32).min(buf_h as i32);
    for y in sy0..sy1 {
        let row = (y as u32 * buf_w) as usize;
        if rx >= 0 && rx < buf_w as i32 {
            buffer[row + rx as usize] = color;
        }
        let bx = rx + rw as i32 - 1;
        if bx >= 0 && bx < buf_w as i32 {
            buffer[row + bx as usize] = color;
        }
    }

    if r <= 0.5 {
        return;
    }

    // Antialiased 4 corner arcs
    let corners = [
        (left + r, top + r, x0.max(rx), (rx + r_ceil).min(x1), y0.max(ry), (ry + r_ceil).min(y1)),
        (right - r, top + r, (rx + rw as i32 - r_ceil).max(x0), x1, y0.max(ry), (ry + r_ceil).min(y1)),
        (left + r, bottom - r, x0.max(rx), (rx + r_ceil).min(x1), (ry + rh as i32 - r_ceil).max(y0), y1),
        (right - r, bottom - r, (rx + rw as i32 - r_ceil).max(x0), x1, (ry + rh as i32 - r_ceil).max(y0), y1),
    ];
    for (cx, cy, cx0, cx1, cy0, cy1) in corners {
        for y in cy0..cy1 {
            let py = y as f32 + 0.5;
            let dy = py - cy;
            for x in cx0..cx1 {
                let px = x as f32 + 0.5;
                let dx = px - cx;
                // Only pixels inside the corner quadrant
                if (cx == left + r && px > cx)
                    || (cx == right - r && px < cx)
                    || (cy == top + r && py > cy)
                    || (cy == bottom - r && py < cy)
                {
                    continue;
                }
                let dist = (dx * dx + dy * dy).sqrt();
                let ring = (1.0 - (dist - (r - 0.5)).abs()).clamp(0.0, 1.0);
                if ring > 0.0 {
                    let a = (ring * 255.0).round() as u32;
                    put_pixel_alpha(buffer, buf_w, buf_h, x, y, color, a);
                }
            }
        }
    }
}

/// Top-rounded-only rectangle fill (for browser/folder style tabs).
#[allow(clippy::too_many_arguments)]
pub fn fill_top_rounded_rect(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    rx: i32,
    ry: i32,
    rw: u32,
    rh: u32,
    radius: f32,
    color: u32,
) {
    if rw == 0 || rh == 0 {
        return;
    }
    let r = radius.clamp(0.0, (rw as f32 * 0.5).min(rh as f32));
    let x0 = rx.max(0);
    let y0 = ry.max(0);
    let x1 = (rx + rw as i32).min(buf_w as i32);
    let y1 = (ry + rh as i32).min(buf_h as i32);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let left = rx as f32;
    let top = ry as f32;
    let right = (rx + rw as i32) as f32;
    let r_ceil = r.ceil() as i32;

    for y in y0..y1 {
        let row_start = (y as u32 * buf_w) as usize;
        if y >= ry + r_ceil {
            buffer[row_start + x0 as usize..row_start + x1 as usize].fill(color);
            continue;
        }
        let py = y as f32 + 0.5;
        let dy = py - (top + r);
        for x in x0..x1 {
            let px = x as f32 + 0.5;
            let cov = if px < left + r {
                let dx = px - (left + r);
                (r + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0)
            } else if px > right - r {
                let dx = px - (right - r);
                (r + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0)
            } else {
                1.0
            };
            if cov > 0.0 {
                let idx = row_start + x as usize;
                buffer[idx] = blend_pixel(buffer[idx], color, (cov * 255.0).round() as u32);
            }
        }
    }
}

/// Draws a soft multi-step rectangular drop shadow behind pages, dropdowns, and cards.
#[allow(clippy::too_many_arguments)]
pub fn draw_soft_shadow(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    rx: i32,
    ry: i32,
    rw: u32,
    rh: u32,
    radius: f32,
    spread: i32,
    offset_y: i32,
    rgb: (u32, u32, u32),
    max_alpha: u32,
) {
    if spread <= 0 || max_alpha == 0 || rw == 0 || rh == 0 {
        return;
    }
    let color = (rgb.0 << 16) | (rgb.1 << 8) | rgb.2;
    let steps = spread.clamp(2, 10);
    // Paint only the outer perimeter bands around the box to avoid over-darkening the interior.
    let x0 = (rx - spread).max(0);
    let y0 = (ry + offset_y - spread).max(0);
    let x1 = (rx + rw as i32 + spread).min(buf_w as i32);
    let y1 = (ry + offset_y + rh as i32 + spread).min(buf_h as i32);
    if x1 <= x0 || y1 <= y0 {
        return;
    }

    let inner_left = rx as f32 + radius;
    let inner_right = (rx + rw as i32) as f32 - radius;
    let inner_top = (ry + offset_y) as f32 + radius;
    let inner_bottom = (ry + offset_y + rh as i32) as f32 - radius;
    let inv_spread = 1.0 / (steps as f32);

    let r_inset = radius.ceil() as i32;
    let skip_x0 = rx + r_inset;
    let skip_x1 = rx + rw as i32 - r_inset;
    let skip_y0 = ry + r_inset;
    let skip_y1 = ry + rh as i32 - r_inset;

    for y in y0..y1 {
        let py = y as f32 + 0.5;
        let row_start = (y as u32 * buf_w) as usize;
        let inside_y = y >= skip_y0 && y < skip_y1;
        for x in x0..x1 {
            if inside_y && x >= skip_x0 && x < skip_x1 {
                continue;
            }
            let px = x as f32 + 0.5;
            let dx = if px < inner_left {
                inner_left - px
            } else if px > inner_right {
                px - inner_right
            } else {
                0.0
            };
            let dy = if py < inner_top {
                inner_top - py
            } else if py > inner_bottom {
                py - inner_bottom
            } else {
                0.0
            };
            let dist = (dx * dx + dy * dy).sqrt() - radius;
            if dist >= 0.0 && dist < steps as f32 {
                let t = 1.0 - dist * inv_spread;
                let a = ((t * t) * (max_alpha as f32)).round() as u32;
                if a > 0 {
                    let idx = row_start + x as usize;
                    buffer[idx] = blend_pixel(buffer[idx], color, a);
                }
            }
        }
    }
}

/// Draws a dashed 1px rectangle border (for drop zones and printable margin guides).
#[allow(clippy::too_many_arguments)]
pub fn draw_dashed_rect(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    rx: i32,
    ry: i32,
    rw: u32,
    rh: u32,
    dash: i32,
    gap: i32,
    color: u32,
) {
    if rw < 2 || rh < 2 {
        return;
    }
    let period = (dash + gap).max(2);
    let x1 = rx + rw as i32 - 1;
    let y1 = ry + rh as i32 - 1;
    for dx in 0..rw as i32 {
        if dx % period < dash {
            put_pixel_alpha(buffer, buf_w, buf_h, rx + dx, ry, color, 255);
            put_pixel_alpha(buffer, buf_w, buf_h, rx + dx, y1, color, 255);
        }
    }
    for dy in 0..rh as i32 {
        if dy % period < dash {
            put_pixel_alpha(buffer, buf_w, buf_h, rx, ry + dy, color, 255);
            put_pixel_alpha(buffer, buf_w, buf_h, x1, ry + dy, color, 255);
        }
    }
}

/// Antialiased filled circle.
#[allow(clippy::too_many_arguments)]
pub fn fill_circle_aa(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    cx: f32,
    cy: f32,
    radius: f32,
    color: u32,
) {
    let x0 = ((cx - radius - 1.0).floor() as i32).max(0);
    let y0 = ((cy - radius - 1.0).floor() as i32).max(0);
    let x1 = ((cx + radius + 1.0).ceil() as i32).min(buf_w as i32);
    let y1 = ((cy + radius + 1.0).ceil() as i32).min(buf_h as i32);
    for y in y0..y1 {
        let dy = (y as f32 + 0.5) - cy;
        for x in x0..x1 {
            let dx = (x as f32 + 0.5) - cx;
            let cov = (radius + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
            if cov > 0.0 {
                put_pixel_alpha(buffer, buf_w, buf_h, x, y, color, (cov * 255.0).round() as u32);
            }
        }
    }
}

/// Antialiased circle outline.
#[allow(clippy::too_many_arguments)]
pub fn stroke_circle_aa(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    cx: f32,
    cy: f32,
    radius: f32,
    thickness: f32,
    color: u32,
) {
    let half_t = (thickness * 0.5).max(0.5);
    let outer = radius + half_t + 1.0;
    let x0 = ((cx - outer).floor() as i32).max(0);
    let y0 = ((cy - outer).floor() as i32).max(0);
    let x1 = ((cx + outer).ceil() as i32).min(buf_w as i32);
    let y1 = ((cy + outer).ceil() as i32).min(buf_h as i32);
    for y in y0..y1 {
        let dy = (y as f32 + 0.5) - cy;
        for x in x0..x1 {
            let dx = (x as f32 + 0.5) - cx;
            let dist = ((dx * dx + dy * dy).sqrt() - radius).abs();
            let cov = (half_t + 0.5 - dist).clamp(0.0, 1.0);
            if cov > 0.0 {
                put_pixel_alpha(buffer, buf_w, buf_h, x, y, color, (cov * 255.0).round() as u32);
            }
        }
    }
}

/// Antialiased thick line segment (capsule SDF).
#[allow(clippy::too_many_arguments)]
pub fn draw_line_aa(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    thickness: f32,
    color: u32,
) {
    let half_t = (thickness * 0.5).max(0.5);
    let pad = half_t + 1.2;
    let bx0 = ((x0.min(x1) - pad).floor() as i32).max(0);
    let by0 = ((y0.min(y1) - pad).floor() as i32).max(0);
    let bx1 = ((x0.max(x1) + pad).ceil() as i32).min(buf_w as i32);
    let by1 = ((y0.max(y1) + pad).ceil() as i32).min(buf_h as i32);

    let vx = x1 - x0;
    let vy = y1 - y0;
    let len_sq = (vx * vx + vy * vy).max(1e-6);

    for y in by0..by1 {
        let py = y as f32 + 0.5;
        for x in bx0..bx1 {
            let px = x as f32 + 0.5;
            let t = (((px - x0) * vx + (py - y0) * vy) / len_sq).clamp(0.0, 1.0);
            let proj_x = x0 + t * vx;
            let proj_y = y0 + t * vy;
            let dist = ((px - proj_x).powi(2) + (py - proj_y).powi(2)).sqrt();
            let cov = (half_t + 0.5 - dist).clamp(0.0, 1.0);
            if cov > 0.0 {
                put_pixel_alpha(buffer, buf_w, buf_h, x, y, color, (cov * 255.0).round() as u32);
            }
        }
    }
}

/// Crisp resolution-independent procedural UI icons.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UiIcon {
    Sidebar,
    Grid,
    Open,
    Save,
    SaveAs,
    Print,
    ChevronLeft,
    ChevronRight,
    ChevronDown,
    MoveUp,
    MoveDown,
    ZoomOut,
    ZoomIn,
    FitPage,
    FitWidth,
    FitText,
    TwoPage,
    Continuous,
    TextView,
    Fullscreen,
    Presentation,
    Select,
    Hand,
    Highlight,
    Underline,
    Strikethrough,
    Note,
    Pen,
    TextBox,
    RectShape,
    ArrowShape,
    Redact,
    Crop,
    Stamp,
    Rotate,
    Search,
    Filter,
    Sun,
    Moon,
    Close,
    Plus,
    Check,
    Document,
    Star,
    #[allow(dead_code)]
    DocPdf,
    #[allow(dead_code)]
    DocEpub,
    #[allow(dead_code)]
    DocMarkdown,
    #[allow(dead_code)]
    DocLatex,
}

/// Draws `icon` inside the square `(x, y, size, size)` with stroke scaled proportionally.
#[allow(clippy::too_many_arguments)]
pub fn draw_icon(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    icon: UiIcon,
    x: i32,
    y: i32,
    size: u32,
    color: u32,
) {
    let s = size as f32;
    let fx = x as f32;
    let fy = y as f32;
    let t = (s / 11.5).clamp(1.15, 2.4);
    let p = |u: f32, v: f32| -> (f32, f32) { (fx + u * s, fy + v * s) };
    let line = |buf: &mut [u32], u0: f32, v0: f32, u1: f32, v1: f32| {
        let (ax, ay) = p(u0, v0);
        let (bx, by) = p(u1, v1);
        draw_line_aa(buf, buf_w, buf_h, ax, ay, bx, by, t, color);
    };

    match icon {
        UiIcon::Sidebar => {
            stroke_rounded_rect(buffer, buf_w, buf_h, x + (s * 0.12) as i32, y + (s * 0.16) as i32, (s * 0.76) as u32, (s * 0.68) as u32, (s * 0.12).max(1.5), color);
            line(buffer, 0.38, 0.20, 0.38, 0.80);
        }
        UiIcon::Grid => {
            let g = (s * 0.32).round() as u32;
            let o1 = (s * 0.13).round() as i32;
            let o2 = (s * 0.55).round() as i32;
            let rad = (s * 0.08).max(1.0);
            stroke_rounded_rect(buffer, buf_w, buf_h, x + o1, y + o1, g, g, rad, color);
            stroke_rounded_rect(buffer, buf_w, buf_h, x + o2, y + o1, g, g, rad, color);
            stroke_rounded_rect(buffer, buf_w, buf_h, x + o1, y + o2, g, g, rad, color);
            stroke_rounded_rect(buffer, buf_w, buf_h, x + o2, y + o2, g, g, rad, color);
        }
        UiIcon::Open => {
            // Folder outline
            line(buffer, 0.14, 0.24, 0.42, 0.24);
            line(buffer, 0.42, 0.24, 0.52, 0.35);
            line(buffer, 0.52, 0.35, 0.86, 0.35);
            line(buffer, 0.86, 0.35, 0.86, 0.78);
            line(buffer, 0.86, 0.78, 0.14, 0.78);
            line(buffer, 0.14, 0.78, 0.14, 0.24);
        }
        UiIcon::Save => {
            // Floppy outline + inner label
            line(buffer, 0.18, 0.16, 0.70, 0.16);
            line(buffer, 0.70, 0.16, 0.84, 0.30);
            line(buffer, 0.84, 0.30, 0.84, 0.84);
            line(buffer, 0.84, 0.84, 0.18, 0.84);
            line(buffer, 0.18, 0.84, 0.18, 0.16);
            line(buffer, 0.34, 0.16, 0.34, 0.38);
            line(buffer, 0.34, 0.38, 0.64, 0.38);
            line(buffer, 0.64, 0.38, 0.64, 0.16);
            line(buffer, 0.30, 0.84, 0.30, 0.56);
            line(buffer, 0.30, 0.56, 0.72, 0.56);
            line(buffer, 0.72, 0.56, 0.72, 0.84);
        }
        UiIcon::Print => {
            // Top paper feed
            line(buffer, 0.30, 0.38, 0.30, 0.16);
            line(buffer, 0.30, 0.16, 0.70, 0.16);
            line(buffer, 0.70, 0.16, 0.70, 0.38);
            // Printer body
            stroke_rounded_rect(buffer, buf_w, buf_h, x + (s * 0.14) as i32, y + (s * 0.36) as i32, (s * 0.72) as u32, (s * 0.34) as u32, (s * 0.1).max(1.5), color);
            // Bottom paper sheet
            line(buffer, 0.30, 0.62, 0.70, 0.62);
            line(buffer, 0.30, 0.62, 0.30, 0.86);
            line(buffer, 0.30, 0.86, 0.70, 0.86);
            line(buffer, 0.70, 0.86, 0.70, 0.62);
        }
        UiIcon::ChevronLeft => {
            line(buffer, 0.62, 0.22, 0.34, 0.50);
            line(buffer, 0.34, 0.50, 0.62, 0.78);
        }
        UiIcon::ChevronRight => {
            line(buffer, 0.38, 0.22, 0.66, 0.50);
            line(buffer, 0.66, 0.50, 0.38, 0.78);
        }
        UiIcon::ChevronDown => {
            line(buffer, 0.22, 0.38, 0.50, 0.66);
            line(buffer, 0.50, 0.66, 0.78, 0.38);
        }
        UiIcon::ZoomOut => {
            line(buffer, 0.22, 0.50, 0.78, 0.50);
        }
        UiIcon::ZoomIn | UiIcon::Plus => {
            line(buffer, 0.22, 0.50, 0.78, 0.50);
            line(buffer, 0.50, 0.22, 0.50, 0.78);
        }
        UiIcon::FitPage => {
            stroke_rounded_rect(buffer, buf_w, buf_h, x + (s * 0.22) as i32, y + (s * 0.14) as i32, (s * 0.56) as u32, (s * 0.72) as u32, (s * 0.08).max(1.0), color);
            line(buffer, 0.34, 0.36, 0.66, 0.36);
            line(buffer, 0.34, 0.52, 0.66, 0.52);
            line(buffer, 0.34, 0.68, 0.54, 0.68);
        }
        UiIcon::FitWidth => {
            line(buffer, 0.14, 0.22, 0.14, 0.78);
            line(buffer, 0.86, 0.22, 0.86, 0.78);
            line(buffer, 0.24, 0.50, 0.76, 0.50);
            line(buffer, 0.36, 0.38, 0.24, 0.50);
            line(buffer, 0.36, 0.62, 0.24, 0.50);
            line(buffer, 0.64, 0.38, 0.76, 0.50);
            line(buffer, 0.64, 0.62, 0.76, 0.50);
        }
        UiIcon::TwoPage => {
            let pw = (s * 0.36) as u32;
            let ph = (s * 0.66) as u32;
            stroke_rounded_rect(buffer, buf_w, buf_h, x + (s * 0.10) as i32, y + (s * 0.17) as i32, pw, ph, 1.5, color);
            stroke_rounded_rect(buffer, buf_w, buf_h, x + (s * 0.54) as i32, y + (s * 0.17) as i32, pw, ph, 1.5, color);
        }
        UiIcon::Continuous => {
            stroke_rounded_rect(buffer, buf_w, buf_h, x + (s * 0.24) as i32, y + (s * 0.10) as i32, (s * 0.52) as u32, (s * 0.36) as u32, 1.5, color);
            stroke_rounded_rect(buffer, buf_w, buf_h, x + (s * 0.24) as i32, y + (s * 0.54) as i32, (s * 0.52) as u32, (s * 0.36) as u32, 1.5, color);
        }
        UiIcon::Select => {
            // I-beam text cursor
            line(buffer, 0.34, 0.18, 0.66, 0.18);
            line(buffer, 0.50, 0.18, 0.50, 0.82);
            line(buffer, 0.34, 0.82, 0.66, 0.82);
        }
        UiIcon::Hand => {
            // Open-palm pan / grab hand icon
            // Four fingers
            line(buffer, 0.36, 0.48, 0.36, 0.22);
            line(buffer, 0.48, 0.46, 0.48, 0.16);
            line(buffer, 0.60, 0.48, 0.60, 0.20);
            line(buffer, 0.72, 0.52, 0.72, 0.30);
            // Thumb on left
            line(buffer, 0.32, 0.64, 0.20, 0.48);
            // Palm outline & wrist base
            line(buffer, 0.32, 0.64, 0.36, 0.82);
            line(buffer, 0.36, 0.82, 0.68, 0.82);
            line(buffer, 0.68, 0.82, 0.72, 0.52);
        }
        UiIcon::Highlight => {
            // Diagonal highlighter pen + underline
            line(buffer, 0.22, 0.82, 0.78, 0.82);
            line(buffer, 0.30, 0.68, 0.66, 0.24);
            line(buffer, 0.66, 0.24, 0.78, 0.34);
            line(buffer, 0.78, 0.34, 0.42, 0.74);
            line(buffer, 0.42, 0.74, 0.30, 0.68);
        }
        UiIcon::Redact => {
            // Solid bar inside frame
            stroke_rounded_rect(buffer, buf_w, buf_h, x + (s * 0.14) as i32, y + (s * 0.20) as i32, (s * 0.72) as u32, (s * 0.60) as u32, 2.0, color);
            fill_rounded_rect(buffer, buf_w, buf_h, x + (s * 0.25) as i32, y + (s * 0.40) as i32, (s * 0.50) as u32, (s * 0.22) as u32, 1.0, color);
        }
        UiIcon::Crop => {
            line(buffer, 0.28, 0.14, 0.28, 0.72);
            line(buffer, 0.28, 0.72, 0.86, 0.72);
            line(buffer, 0.14, 0.28, 0.72, 0.28);
            line(buffer, 0.72, 0.28, 0.72, 0.86);
        }
        UiIcon::Stamp => {
            // Framed image mountain/sun
            stroke_rounded_rect(buffer, buf_w, buf_h, x + (s * 0.14) as i32, y + (s * 0.18) as i32, (s * 0.72) as u32, (s * 0.64) as u32, 2.0, color);
            line(buffer, 0.22, 0.70, 0.44, 0.48);
            line(buffer, 0.44, 0.48, 0.60, 0.62);
            line(buffer, 0.56, 0.58, 0.70, 0.44);
            line(buffer, 0.70, 0.44, 0.80, 0.68);
        }
        UiIcon::Rotate => {
            let (cx, cy) = p(0.50, 0.52);
            stroke_circle_aa(buffer, buf_w, buf_h, cx, cy, s * 0.30, t, color);
            // Arrow head on top-right
            line(buffer, 0.66, 0.16, 0.80, 0.30);
            line(buffer, 0.80, 0.30, 0.60, 0.34);
        }
        UiIcon::Search => {
            let (cx, cy) = p(0.43, 0.43);
            stroke_circle_aa(buffer, buf_w, buf_h, cx, cy, s * 0.26, t, color);
            line(buffer, 0.63, 0.63, 0.85, 0.85);
        }
        UiIcon::Filter => {
            let (cx, cy) = p(0.50, 0.50);
            stroke_circle_aa(buffer, buf_w, buf_h, cx, cy, s * 0.33, t, color);
            line(buffer, 0.50, 0.19, 0.50, 0.81);
            line(buffer, 0.50, 0.34, 0.76, 0.34);
            line(buffer, 0.50, 0.50, 0.80, 0.50);
            line(buffer, 0.50, 0.66, 0.76, 0.66);
        }
        UiIcon::Sun => {
            let (cx, cy) = p(0.50, 0.50);
            stroke_circle_aa(buffer, buf_w, buf_h, cx, cy, s * 0.19, t, color);
            line(buffer, 0.50, 0.12, 0.50, 0.22);
            line(buffer, 0.50, 0.78, 0.50, 0.88);
            line(buffer, 0.12, 0.50, 0.22, 0.50);
            line(buffer, 0.78, 0.50, 0.88, 0.50);
            line(buffer, 0.23, 0.23, 0.30, 0.30);
            line(buffer, 0.70, 0.70, 0.77, 0.77);
            line(buffer, 0.77, 0.23, 0.70, 0.30);
            line(buffer, 0.30, 0.70, 0.23, 0.77);
        }
        UiIcon::Moon => {
            // Crescent moon drawn via two offset arcs
            let (cx, cy) = p(0.52, 0.50);
            let r = s * 0.32;
            let cut_x = cx + s * 0.16;
            let cut_y = cy - s * 0.12;
            let cut_r = s * 0.27;
            let x0 = ((cx - r - 1.0).floor() as i32).max(0);
            let y0 = ((cy - r - 1.0).floor() as i32).max(0);
            let x1 = ((cx + r + 1.0).ceil() as i32).min(buf_w as i32);
            let y1 = ((cy + r + 1.0).ceil() as i32).min(buf_h as i32);
            for py_i in y0..y1 {
                let py = py_i as f32 + 0.5;
                for px_i in x0..x1 {
                    let px = px_i as f32 + 0.5;
                    let d_outer = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
                    let d_cut = ((px - cut_x).powi(2) + (py - cut_y).powi(2)).sqrt();
                    let cov_in = (r + 0.5 - d_outer).clamp(0.0, 1.0);
                    let cov_out = (d_cut - cut_r + 0.5).clamp(0.0, 1.0);
                    let cov = cov_in.min(cov_out);
                    if cov > 0.0 {
                        put_pixel_alpha(buffer, buf_w, buf_h, px_i, py_i, color, (cov * 255.0).round() as u32);
                    }
                }
            }
        }
        UiIcon::Close => {
            line(buffer, 0.25, 0.25, 0.75, 0.75);
            line(buffer, 0.75, 0.25, 0.25, 0.75);
        }
        UiIcon::Check => {
            line(buffer, 0.18, 0.54, 0.40, 0.76);
            line(buffer, 0.40, 0.76, 0.82, 0.26);
        }
        UiIcon::Document => {
            line(buffer, 0.22, 0.12, 0.60, 0.12);
            line(buffer, 0.60, 0.12, 0.78, 0.30);
            line(buffer, 0.78, 0.30, 0.78, 0.88);
            line(buffer, 0.78, 0.88, 0.22, 0.88);
            line(buffer, 0.22, 0.88, 0.22, 0.12);
            line(buffer, 0.60, 0.12, 0.60, 0.30);
            line(buffer, 0.60, 0.30, 0.78, 0.30);
            line(buffer, 0.34, 0.48, 0.66, 0.48);
            line(buffer, 0.34, 0.64, 0.66, 0.64);
        }
        UiIcon::Star => {
            let pts = [
                (0.50f32, 0.12f32),
                (0.61, 0.37),
                (0.88, 0.40),
                (0.68, 0.59),
                (0.73, 0.86),
                (0.50, 0.72),
                (0.27, 0.86),
                (0.32, 0.59),
                (0.12, 0.40),
                (0.39, 0.37),
            ];
            for i in 0..pts.len() {
                let (x0, y0) = pts[i];
                let (x1, y1) = pts[(i + 1) % pts.len()];
                line(buffer, x0, y0, x1, y1);
            }
        }
        UiIcon::SaveAs => {
            // Floppy outline + small '+' badge on bottom-right
            line(buffer, 0.14, 0.14, 0.66, 0.14);
            line(buffer, 0.66, 0.14, 0.80, 0.28);
            line(buffer, 0.80, 0.28, 0.80, 0.56);
            line(buffer, 0.14, 0.14, 0.14, 0.84);
            line(buffer, 0.14, 0.84, 0.54, 0.84);
            line(buffer, 0.28, 0.14, 0.28, 0.38);
            line(buffer, 0.28, 0.38, 0.60, 0.38);
            line(buffer, 0.60, 0.38, 0.60, 0.14);
            // '+' badge
            line(buffer, 0.62, 0.76, 0.90, 0.76);
            line(buffer, 0.76, 0.62, 0.76, 0.90);
        }
        UiIcon::MoveUp => {
            line(buffer, 0.50, 0.20, 0.50, 0.82);
            line(buffer, 0.26, 0.44, 0.50, 0.20);
            line(buffer, 0.74, 0.44, 0.50, 0.20);
        }
        UiIcon::MoveDown => {
            line(buffer, 0.50, 0.18, 0.50, 0.80);
            line(buffer, 0.26, 0.56, 0.50, 0.80);
            line(buffer, 0.74, 0.56, 0.50, 0.80);
        }
        UiIcon::FitText => {
            // Left & right inward crop brackets + text lines between them
            line(buffer, 0.14, 0.22, 0.14, 0.78);
            line(buffer, 0.86, 0.22, 0.86, 0.78);
            line(buffer, 0.28, 0.32, 0.72, 0.32);
            line(buffer, 0.28, 0.50, 0.72, 0.50);
            line(buffer, 0.28, 0.68, 0.62, 0.68);
        }
        UiIcon::TextView => {
            line(buffer, 0.16, 0.24, 0.84, 0.24);
            line(buffer, 0.16, 0.42, 0.84, 0.42);
            line(buffer, 0.16, 0.60, 0.76, 0.60);
            line(buffer, 0.16, 0.78, 0.58, 0.78);
        }
        UiIcon::Fullscreen => {
            // Four outward corners
            line(buffer, 0.16, 0.36, 0.16, 0.16);
            line(buffer, 0.16, 0.16, 0.36, 0.16);
            line(buffer, 0.64, 0.16, 0.84, 0.16);
            line(buffer, 0.84, 0.16, 0.84, 0.36);
            line(buffer, 0.84, 0.64, 0.84, 0.84);
            line(buffer, 0.84, 0.84, 0.64, 0.84);
            line(buffer, 0.36, 0.84, 0.16, 0.84);
            line(buffer, 0.16, 0.84, 0.16, 0.64);
        }
        UiIcon::Presentation => {
            // Screen frame + play triangle
            line(buffer, 0.14, 0.18, 0.86, 0.18);
            line(buffer, 0.86, 0.18, 0.86, 0.72);
            line(buffer, 0.86, 0.72, 0.14, 0.72);
            line(buffer, 0.14, 0.72, 0.14, 0.18);
            line(buffer, 0.50, 0.72, 0.50, 0.86);
            line(buffer, 0.34, 0.86, 0.66, 0.86);
            line(buffer, 0.42, 0.33, 0.62, 0.45);
            line(buffer, 0.62, 0.45, 0.42, 0.57);
            line(buffer, 0.42, 0.57, 0.42, 0.33);
        }
        UiIcon::Underline => {
            // 'U' + bottom bar
            line(buffer, 0.28, 0.18, 0.28, 0.54);
            line(buffer, 0.28, 0.54, 0.38, 0.66);
            line(buffer, 0.38, 0.66, 0.62, 0.66);
            line(buffer, 0.62, 0.66, 0.72, 0.54);
            line(buffer, 0.72, 0.54, 0.72, 0.18);
            line(buffer, 0.20, 0.84, 0.80, 0.84);
        }
        UiIcon::Strikethrough => {
            // Stylized 'S' + center strike line
            line(buffer, 0.68, 0.24, 0.34, 0.24);
            line(buffer, 0.34, 0.24, 0.30, 0.38);
            line(buffer, 0.30, 0.38, 0.68, 0.58);
            line(buffer, 0.68, 0.58, 0.66, 0.76);
            line(buffer, 0.66, 0.76, 0.30, 0.76);
            line(buffer, 0.14, 0.50, 0.86, 0.50);
        }
        UiIcon::Note => {
            // Sticky note square with folded bottom-right corner
            line(buffer, 0.18, 0.16, 0.82, 0.16);
            line(buffer, 0.82, 0.16, 0.82, 0.62);
            line(buffer, 0.82, 0.62, 0.62, 0.82);
            line(buffer, 0.62, 0.82, 0.18, 0.82);
            line(buffer, 0.18, 0.82, 0.18, 0.16);
            line(buffer, 0.62, 0.82, 0.62, 0.62);
            line(buffer, 0.62, 0.62, 0.82, 0.62);
            line(buffer, 0.30, 0.34, 0.68, 0.34);
            line(buffer, 0.30, 0.50, 0.58, 0.50);
        }
        UiIcon::Pen => {
            // Diagonal fountain/ink pen + freehand wave
            line(buffer, 0.22, 0.78, 0.74, 0.22);
            line(buffer, 0.62, 0.16, 0.82, 0.34);
            line(buffer, 0.22, 0.78, 0.34, 0.74);
            line(buffer, 0.42, 0.84, 0.60, 0.74);
            line(buffer, 0.60, 0.74, 0.82, 0.84);
        }
        UiIcon::TextBox => {
            // Outer box + 'T' inside
            line(buffer, 0.14, 0.18, 0.86, 0.18);
            line(buffer, 0.86, 0.18, 0.86, 0.82);
            line(buffer, 0.86, 0.82, 0.14, 0.82);
            line(buffer, 0.14, 0.82, 0.14, 0.18);
            line(buffer, 0.32, 0.36, 0.68, 0.36);
            line(buffer, 0.50, 0.36, 0.50, 0.68);
        }
        UiIcon::RectShape => {
            line(buffer, 0.16, 0.24, 0.84, 0.24);
            line(buffer, 0.84, 0.24, 0.84, 0.76);
            line(buffer, 0.84, 0.76, 0.16, 0.76);
            line(buffer, 0.16, 0.76, 0.16, 0.24);
        }
        UiIcon::ArrowShape => {
            line(buffer, 0.20, 0.80, 0.78, 0.22);
            line(buffer, 0.46, 0.22, 0.78, 0.22);
            line(buffer, 0.78, 0.22, 0.78, 0.54);
        }
        UiIcon::DocPdf => {
            // Folded document outline + accent ribbon bars
            line(buffer, 0.20, 0.12, 0.60, 0.12);
            line(buffer, 0.60, 0.12, 0.80, 0.32);
            line(buffer, 0.80, 0.32, 0.80, 0.88);
            line(buffer, 0.80, 0.88, 0.20, 0.88);
            line(buffer, 0.20, 0.88, 0.20, 0.12);
            line(buffer, 0.60, 0.12, 0.60, 0.32);
            line(buffer, 0.60, 0.32, 0.80, 0.32);
            line(buffer, 0.32, 0.52, 0.68, 0.52);
            line(buffer, 0.32, 0.68, 0.58, 0.68);
        }
        UiIcon::DocEpub => {
            // Open-book silhouette with central spine
            line(buffer, 0.10, 0.22, 0.50, 0.28);
            line(buffer, 0.50, 0.28, 0.90, 0.22);
            line(buffer, 0.10, 0.22, 0.10, 0.74);
            line(buffer, 0.90, 0.22, 0.90, 0.74);
            line(buffer, 0.10, 0.74, 0.50, 0.80);
            line(buffer, 0.50, 0.80, 0.90, 0.74);
            line(buffer, 0.50, 0.28, 0.50, 0.80);
            line(buffer, 0.21, 0.44, 0.39, 0.47);
            line(buffer, 0.61, 0.47, 0.79, 0.44);
        }
        UiIcon::DocMarkdown => {
            // Rounded badge + 'M↓'
            stroke_rounded_rect(
                buffer,
                buf_w,
                buf_h,
                x + (s * 0.06).round() as i32,
                y + (s * 0.12).round() as i32,
                (s * 0.88).round().max(4.0) as u32,
                (s * 0.76).round().max(4.0) as u32,
                (s * 0.18).max(1.5),
                color,
            );
            line(buffer, 0.20, 0.68, 0.20, 0.32);
            line(buffer, 0.20, 0.32, 0.34, 0.52);
            line(buffer, 0.34, 0.52, 0.48, 0.32);
            line(buffer, 0.48, 0.32, 0.48, 0.68);
            line(buffer, 0.71, 0.30, 0.71, 0.68);
            line(buffer, 0.60, 0.55, 0.71, 0.68);
            line(buffer, 0.82, 0.55, 0.71, 0.68);
        }
        UiIcon::DocLatex => {
            // Rounded math-doc badge + 'Σ'
            stroke_rounded_rect(
                buffer,
                buf_w,
                buf_h,
                x + (s * 0.06).round() as i32,
                y + (s * 0.12).round() as i32,
                (s * 0.88).round().max(4.0) as u32,
                (s * 0.76).round().max(4.0) as u32,
                (s * 0.18).max(1.5),
                color,
            );
            line(buffer, 0.68, 0.30, 0.32, 0.30);
            line(buffer, 0.32, 0.30, 0.54, 0.50);
            line(buffer, 0.54, 0.50, 0.32, 0.70);
            line(buffer, 0.32, 0.70, 0.68, 0.70);
        }
    }
}

/// Draws a crisp color-coded procedural file-type icon/mini-badge inside `(x, y, size, size)`:
/// - **PDF**: red document badge
/// - **EPUB**: warm amber/brown open-book icon
/// - **Markdown**: indigo/blue `M↓` badge
/// - **LaTeX**: emerald/teal `Σ` math-doc badge
#[allow(clippy::too_many_arguments)]
pub fn draw_doc_type_icon(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    kind: super::theme::DocKind,
    x: i32,
    y: i32,
    size: u32,
    dark_ui: bool,
) {
    if size < 4 {
        return;
    }
    let s = size as f32;
    let fx = x as f32;
    let fy = y as f32;
    let stroke_t = (s / 10.5).clamp(1.15, 2.2);
    let rad = (s * 0.18).max(1.5);
    let p = |u: f32, v: f32| -> (f32, f32) { (fx + u * s, fy + v * s) };
    let line = |buf: &mut [u32], u0: f32, v0: f32, u1: f32, v1: f32, col: u32| {
        let (ax, ay) = p(u0, v0);
        let (bx, by) = p(u1, v1);
        draw_line_aa(buf, buf_w, buf_h, ax, ay, bx, by, stroke_t, col);
    };

    match kind {
        super::theme::DocKind::Pdf => {
            let bg = if dark_ui { 0x0045_1A1D } else { 0x00FE_E2E2 };
            let fg = if dark_ui { 0x00F8_7171 } else { 0x00DC_2626 };
            let bx = x + (s * 0.12).round() as i32;
            let by = y + (s * 0.06).round() as i32;
            let bw = (s * 0.76).round().max(4.0) as u32;
            let bh = (s * 0.88).round().max(4.0) as u32;
            fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, rad, bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, rad, fg);
            // Folded corner accent + red document bar lines
            line(buffer, 0.58, 0.10, 0.58, 0.34, fg);
            line(buffer, 0.58, 0.34, 0.84, 0.34, fg);
            fill_rounded_rect(
                buffer,
                buf_w,
                buf_h,
                x + (s * 0.25).round() as i32,
                y + (s * 0.44).round() as i32,
                (s * 0.50).round().max(2.0) as u32,
                (s * 0.14).round().max(1.5) as u32,
                0.8,
                fg,
            );
            line(buffer, 0.28, 0.72, 0.64, 0.72, fg);
        }
        super::theme::DocKind::Epub => {
            let bg = if dark_ui { 0x003B_2A16 } else { 0x00FE_F3C7 };
            let fg = if dark_ui { 0x00FB_BF24 } else { 0x00B4_5309 };
            let spine = if dark_ui { 0x00F5_9E0B } else { 0x0078_350F };
            // Warm open-book cover + pages
            fill_rounded_rect(
                buffer,
                buf_w,
                buf_h,
                x + (s * 0.08).round() as i32,
                y + (s * 0.22).round() as i32,
                (s * 0.84).round().max(4.0) as u32,
                (s * 0.58).round().max(4.0) as u32,
                rad,
                bg,
            );
            line(buffer, 0.10, 0.22, 0.50, 0.29, fg);
            line(buffer, 0.50, 0.29, 0.90, 0.22, fg);
            line(buffer, 0.10, 0.22, 0.10, 0.74, fg);
            line(buffer, 0.90, 0.22, 0.90, 0.74, fg);
            line(buffer, 0.10, 0.74, 0.50, 0.81, fg);
            line(buffer, 0.50, 0.81, 0.90, 0.74, fg);
            line(buffer, 0.50, 0.29, 0.50, 0.81, spine);
            line(buffer, 0.21, 0.44, 0.39, 0.48, spine);
            line(buffer, 0.61, 0.48, 0.79, 0.44, spine);
        }
        super::theme::DocKind::Markdown => {
            let bg = if dark_ui { 0x001E_2952 } else { 0x00E0_E7FF };
            let fg = if dark_ui { 0x0081_8CF8 } else { 0x0043_38CA };
            let bx = x + (s * 0.05).round() as i32;
            let by = y + (s * 0.11).round() as i32;
            let bw = (s * 0.90).round().max(4.0) as u32;
            let bh = (s * 0.78).round().max(4.0) as u32;
            fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, rad, bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, rad, fg);
            // Crisp 'M↓'
            line(buffer, 0.19, 0.68, 0.19, 0.32, fg);
            line(buffer, 0.19, 0.32, 0.34, 0.53, fg);
            line(buffer, 0.34, 0.53, 0.49, 0.32, fg);
            line(buffer, 0.49, 0.32, 0.49, 0.68, fg);
            line(buffer, 0.71, 0.29, 0.71, 0.68, fg);
            line(buffer, 0.60, 0.55, 0.71, 0.68, fg);
            line(buffer, 0.82, 0.55, 0.71, 0.68, fg);
        }
        super::theme::DocKind::LaTeX => {
            let bg = if dark_ui { 0x0013_362B } else { 0x00D1_FAE5 };
            let fg = if dark_ui { 0x0034_D399 } else { 0x0005_9669 };
            let bx = x + (s * 0.05).round() as i32;
            let by = y + (s * 0.11).round() as i32;
            let bw = (s * 0.90).round().max(4.0) as u32;
            let bh = (s * 0.78).round().max(4.0) as u32;
            fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, rad, bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, rad, fg);
            // Crisp 'Σ' math-doc emblem
            line(buffer, 0.69, 0.30, 0.31, 0.30, fg);
            line(buffer, 0.31, 0.30, 0.54, 0.50, fg);
            line(buffer, 0.54, 0.50, 0.31, 0.70, fg);
            line(buffer, 0.31, 0.70, 0.69, 0.70, fg);
            line(buffer, 0.69, 0.30, 0.69, 0.37, fg);
            line(buffer, 0.69, 0.70, 0.69, 0.63, fg);
        }
    }
}

/// Draws a subtle vertical book-spine crease shadow centered at `spine_x` over `[top_y, top_y + height)`.
pub fn draw_book_spine_crease(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    spine_x: i32,
    top_y: i32,
    height: u32,
    half_w: i32,
) {
    if height == 0 || half_w <= 0 {
        return;
    }
    let y0 = top_y.max(0);
    let y1 = (top_y + height as i32).min(buf_h as i32);
    if y1 <= y0 {
        return;
    }
    let x0 = (spine_x - half_w).max(0);
    let x1 = (spine_x + half_w).min(buf_w as i32);
    if x1 <= x0 {
        return;
    }
    for x in x0..x1 {
        let dist = (x - spine_x).abs() as f32 / half_w as f32;
        let alpha = ((1.0 - dist).powf(2.2) * 52.0).round() as u32;
        if alpha == 0 {
            continue;
        }
        let inv = 255 - alpha;
        for y in y0..y1 {
            let idx = y as usize * buf_w as usize + x as usize;
            let dst = buffer[idx];
            let r = (((dst >> 16) & 0xFF) * inv) / 255;
            let g = (((dst >> 8) & 0xFF) * inv) / 255;
            let b = ((dst & 0xFF) * inv) / 255;
            buffer[idx] = (r << 16) | (g << 8) | b;
        }
    }
}

/// Draws a 220 ms animated page-turn fold shadow & paper highlight sweeping across an open spread.
#[allow(clippy::too_many_arguments)]
pub fn draw_magazine_page_turn_overlay(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    spread_x: i32,
    spread_y: i32,
    spread_w: u32,
    spread_h: u32,
    eased_t: f32,
    direction: i8,
    s: f32,
) {
    if spread_w == 0 || spread_h == 0 {
        return;
    }
    let y0 = spread_y.max(0);
    let y1 = (spread_y + spread_h as i32).min(buf_h as i32);
    if y1 <= y0 {
        return;
    }
    let sweep_x = if direction >= 0 {
        spread_x as f32 + spread_w as f32 * (1.0 - eased_t)
    } else {
        spread_x as f32 + spread_w as f32 * eased_t
    };
    let fold_w = (48.0 * s).max(24.0);
    let intensity = (1.0 - (eased_t - 0.5).abs() * 1.25).clamp(0.30, 1.0);
    let x0 = ((sweep_x - fold_w).floor() as i32).max(spread_x.max(0));
    let x1 = ((sweep_x + fold_w).ceil() as i32).min((spread_x + spread_w as i32).min(buf_w as i32));
    if x1 <= x0 {
        return;
    }
    for x in x0..x1 {
        let dx = x as f32 - sweep_x;
        let abs_d = dx.abs();
        if abs_d <= 4.0 * s {
            // Bright specular paper-curl highlight along the turning crease
            let a = ((1.0 - abs_d / (4.0 * s)) * 110.0 * intensity).round() as u32;
            let inv = 255 - a;
            for y in y0..y1 {
                let idx = y as usize * buf_w as usize + x as usize;
                let dst = buffer[idx];
                let r = (((dst >> 16) & 0xFF) * inv + 255 * a) / 255;
                let g = (((dst >> 8) & 0xFF) * inv + 255 * a) / 255;
                let b = ((dst & 0xFF) * inv + 255 * a) / 255;
                buffer[idx] = (r << 16) | (g << 8) | b;
            }
        } else {
            // Trailing page-curl drop shadow
            let a = ((1.0 - abs_d / fold_w).powf(1.5) * 86.0 * intensity).round() as u32;
            if a == 0 {
                continue;
            }
            let inv = 255 - a;
            for y in y0..y1 {
                let idx = y as usize * buf_w as usize + x as usize;
                let dst = buffer[idx];
                let r = (((dst >> 16) & 0xFF) * inv) / 255;
                let g = (((dst >> 8) & 0xFF) * inv) / 255;
                let b = ((dst & 0xFF) * inv) / 255;
                buffer[idx] = (r << 16) | (g << 8) | b;
            }
        }
    }
}


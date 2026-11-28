//! Pure-Rust Comic Book Archive (`.cbz`) Reader.
//!
//! Indexes `.cbz` archives via seek-based `ZipFileReader` (zero full-file RAM load),
//! sorts page entries in natural alphanumeric order, parses optional `ComicInfo.xml`,
//! and decodes JPEG / PNG / WebP pages on demand into `0x00RRGGBB` pixel buffers.

use anyhow::{Context, Result, bail};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use crate::zip_mini::ZipFileReader;

#[allow(dead_code)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComicInfo {
    pub series: Option<String>,
    pub title: Option<String>,
    pub number: Option<String>,
    pub volume: Option<String>,
    pub writer: Option<String>,
    pub penciller: Option<String>,
    pub publisher: Option<String>,
    pub year: Option<i32>,
    pub summary: Option<String>,
    pub manga_rtl: bool,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct ComicPageBitmap {
    pub width: u32,
    pub height: u32,
    /// Pixels stored as `0x00RRGGBB` matching `softbuffer`.
    pub pixels: Vec<u32>,
}

impl ComicPageBitmap {
    #[allow(dead_code)]
    pub fn byte_len(&self) -> usize {
        self.pixels.len().saturating_mul(std::mem::size_of::<u32>())
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct ComicArchive {
    pub path: PathBuf,
    pub pages: Vec<String>,
    pub info: Option<ComicInfo>,
}

fn is_supported_comic_image(name: &str) -> bool {
    if name.ends_with('/') || name.ends_with('\\') {
        return false;
    }
    for seg in name.split(['/', '\\']) {
        if seg.is_empty() {
            continue;
        }
        if seg.eq_ignore_ascii_case("__MACOSX") || seg.starts_with('.') {
            return false;
        }
    }
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".png")
        || lower.ends_with(".webp")
}

/// Natural alphanumeric path comparison so `page2.jpg` comes before `page10.jpg`.
pub(crate) fn natural_cmp(a: &str, b: &str) -> Ordering {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    let mut ia = 0usize;
    let mut ib = 0usize;

    while ia < ab.len() && ib < bb.len() {
        let ca = ab[ia];
        let cb = bb[ib];

        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let start_a = ia;
            while ia < ab.len() && ab[ia].is_ascii_digit() {
                ia += 1;
            }
            let start_b = ib;
            while ib < bb.len() && bb[ib].is_ascii_digit() {
                ib += 1;
            }
            let num_a = &ab[start_a..ia];
            let num_b = &bb[start_b..ib];
            let trim_a = trim_leading_zeros(num_a);
            let trim_b = trim_leading_zeros(num_b);

            match trim_a.len().cmp(&trim_b.len()) {
                Ordering::Equal => match trim_a.cmp(trim_b) {
                    Ordering::Equal => {
                        // Prefer fewer leading zeros (e.g., "1" before "01") if identical numerically
                        match num_a.len().cmp(&num_b.len()) {
                            Ordering::Equal => {}
                            ord => return ord,
                        }
                    }
                    ord => return ord,
                },
                ord => return ord,
            }
            continue;
        }

        // Normalize path separators so `/` and `\` compare identically
        let na = if ca == b'\\' { b'/' } else { ca.to_ascii_lowercase() };
        let nb = if cb == b'\\' { b'/' } else { cb.to_ascii_lowercase() };
        match na.cmp(&nb) {
            Ordering::Equal => {
                ia += 1;
                ib += 1;
            }
            ord => return ord,
        }
    }

    ab.len().cmp(&bb.len())
}

fn trim_leading_zeros(digits: &[u8]) -> &[u8] {
    let mut i = 0;
    while i < digits.len() && digits[i] == b'0' {
        i += 1;
    }
    &digits[i..]
}

fn parse_comic_info_xml(xml: &str) -> Option<ComicInfo> {
    let clean = xml.strip_prefix('\u{FEFF}').unwrap_or(xml);
    let doc = roxmltree::Document::parse(clean).ok()?;
    let mut info = ComicInfo::default();
    let mut saw_any = false;

    for node in doc.descendants().filter(|n| n.is_element()) {
        let Some(val) = node.text().map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        let tag = node.tag_name().name();
        if tag.eq_ignore_ascii_case("Series") && info.series.is_none() {
            info.series = Some(val.to_string());
            saw_any = true;
        } else if tag.eq_ignore_ascii_case("Title") && info.title.is_none() {
            info.title = Some(val.to_string());
            saw_any = true;
        } else if tag.eq_ignore_ascii_case("Number") && info.number.is_none() {
            info.number = Some(val.to_string());
            saw_any = true;
        } else if tag.eq_ignore_ascii_case("Volume") && info.volume.is_none() {
            info.volume = Some(val.to_string());
            saw_any = true;
        } else if tag.eq_ignore_ascii_case("Writer") && info.writer.is_none() {
            info.writer = Some(val.to_string());
            saw_any = true;
        } else if tag.eq_ignore_ascii_case("Penciller") && info.penciller.is_none() {
            info.penciller = Some(val.to_string());
            saw_any = true;
        } else if tag.eq_ignore_ascii_case("Publisher") && info.publisher.is_none() {
            info.publisher = Some(val.to_string());
            saw_any = true;
        } else if tag.eq_ignore_ascii_case("Year") && info.year.is_none() {
            if let Ok(y) = val.parse::<i32>() {
                info.year = Some(y);
                saw_any = true;
            }
        } else if tag.eq_ignore_ascii_case("Summary") && info.summary.is_none() {
            info.summary = Some(val.to_string());
            saw_any = true;
        } else if tag.eq_ignore_ascii_case("Manga") {
            info.manga_rtl = val.eq_ignore_ascii_case("Yes")
                || val.eq_ignore_ascii_case("YesAndRightToLeft");
            saw_any = true;
        }
    }

    if saw_any { Some(info) } else { None }
}

fn rgba_to_xrgb_white_bg(rgba: &image::RgbaImage) -> ComicPageBitmap {
    let w = rgba.width();
    let h = rgba.height();
    let raw = rgba.as_raw();
    let mut pixels = Vec::with_capacity((w as usize).saturating_mul(h as usize));
    for px in raw.chunks_exact(4) {
        let a = px[3] as u32;
        let (r, g, b) = if a == 255 {
            (px[0] as u32, px[1] as u32, px[2] as u32)
        } else if a == 0 {
            (255, 255, 255)
        } else {
            let inv = 255 - a;
            (
                (px[0] as u32 * a + 255 * inv) / 255,
                (px[1] as u32 * a + 255 * inv) / 255,
                (px[2] as u32 * a + 255 * inv) / 255,
            )
        };
        pixels.push((r << 16) | (g << 8) | b);
    }
    ComicPageBitmap {
        width: w,
        height: h,
        pixels,
    }
}

fn downscale_rgba_to_xrgb(rgba: &image::RgbaImage, dst_w: u32, dst_h: u32) -> ComicPageBitmap {
    let src_w = rgba.width().max(1);
    let src_h = rgba.height().max(1);
    let tw = dst_w.max(1);
    let th = dst_h.max(1);
    if tw == src_w && th == src_h {
        return rgba_to_xrgb_white_bg(rgba);
    }
    let raw = rgba.as_raw();
    let mut pixels = vec![0u32; (tw as usize).saturating_mul(th as usize)];

    // Area box filter when shrinking by >= 2x; bilinear when gently scaling
    let scale_x = src_w as f32 / tw as f32;
    let scale_y = src_h as f32 / th as f32;
    if scale_x >= 2.0 || scale_y >= 2.0 {
        for dy in 0..th {
            let sy0 = ((dy as f32 * scale_y).floor() as u32).min(src_h - 1);
            let sy1 = (((dy + 1) as f32 * scale_y).ceil() as u32).clamp(sy0 + 1, src_h);
            for dx in 0..tw {
                let sx0 = ((dx as f32 * scale_x).floor() as u32).min(src_w - 1);
                let sx1 = (((dx + 1) as f32 * scale_x).ceil() as u32).clamp(sx0 + 1, src_w);
                let mut sr = 0u64;
                let mut sg = 0u64;
                let mut sb = 0u64;
                let mut cnt = 0u64;
                for sy in sy0..sy1 {
                    let row_off = (sy as usize * src_w as usize) * 4;
                    for sx in sx0..sx1 {
                        let idx = row_off + sx as usize * 4;
                        let a = raw[idx + 3] as u64;
                        if a == 255 {
                            sr += raw[idx] as u64;
                            sg += raw[idx + 1] as u64;
                            sb += raw[idx + 2] as u64;
                        } else {
                            let inv = 255 - a;
                            sr += (raw[idx] as u64 * a + 255 * inv) / 255;
                            sg += (raw[idx + 1] as u64 * a + 255 * inv) / 255;
                            sb += (raw[idx + 2] as u64 * a + 255 * inv) / 255;
                        }
                        cnt += 1;
                    }
                }
                let c = cnt.max(1);
                let r = (sr / c) as u32;
                let g = (sg / c) as u32;
                let b = (sb / c) as u32;
                pixels[(dy * tw + dx) as usize] = (r << 16) | (g << 8) | b;
            }
        }
    } else {
        for dy in 0..th {
            let fy = ((dy as f32 + 0.5) * scale_y - 0.5).clamp(0.0, (src_h - 1) as f32);
            let y0 = fy.floor() as u32;
            let y1 = (y0 + 1).min(src_h - 1);
            let wy = fy - y0 as f32;
            for dx in 0..tw {
                let fx = ((dx as f32 + 0.5) * scale_x - 0.5).clamp(0.0, (src_w - 1) as f32);
                let x0 = fx.floor() as u32;
                let x1 = (x0 + 1).min(src_w - 1);
                let wx = fx - x0 as f32;

                let sample = |sx: u32, sy: u32| -> (f32, f32, f32) {
                    let i = ((sy * src_w + sx) as usize) * 4;
                    let a = raw[i + 3] as f32 / 255.0;
                    let inv = 1.0 - a;
                    (
                        raw[i] as f32 * a + 255.0 * inv,
                        raw[i + 1] as f32 * a + 255.0 * inv,
                        raw[i + 2] as f32 * a + 255.0 * inv,
                    )
                };
                let c00 = sample(x0, y0);
                let c10 = sample(x1, y0);
                let c01 = sample(x0, y1);
                let c11 = sample(x1, y1);

                let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
                let r = lerp(lerp(c00.0, c10.0, wx), lerp(c01.0, c11.0, wx), wy).round().clamp(0.0, 255.0) as u32;
                let g = lerp(lerp(c00.1, c10.1, wx), lerp(c01.1, c11.1, wx), wy).round().clamp(0.0, 255.0) as u32;
                let b = lerp(lerp(c00.2, c10.2, wx), lerp(c01.2, c11.2, wx), wy).round().clamp(0.0, 255.0) as u32;
                pixels[(dy * tw + dx) as usize] = (r << 16) | (g << 8) | b;
            }
        }
    }

    ComicPageBitmap {
        width: tw,
        height: th,
        pixels,
    }
}

#[allow(dead_code)]
impl ComicArchive {
    pub fn open(path: &Path) -> Result<Self> {
        let mut reader = ZipFileReader::open(path)
            .with_context(|| format!("Failed to open CBZ archive {:?}", path))?;

        let mut pages: Vec<String> = Vec::new();
        let mut comic_info_entry: Option<String> = None;

        for name in reader.entry_names() {
            let trimmed = name.trim_start_matches('/');
            let base = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
            if comic_info_entry.is_none()
                && base.eq_ignore_ascii_case("ComicInfo.xml")
                && !trimmed
                    .split(['/', '\\'])
                    .any(|s| s.eq_ignore_ascii_case("__MACOSX") || s.starts_with('.'))
            {
                comic_info_entry = Some(name.clone());
            } else if is_supported_comic_image(name) {
                pages.push(name.clone());
            }
        }

        if pages.is_empty() {
            bail!("CBZ archive contains no supported images (JPG, PNG, or WebP)");
        }

        pages.sort_by(|a, b| natural_cmp(a, b));

        let info = if let Some(entry_name) = comic_info_entry {
            reader
                .read_entry(&entry_name)
                .ok()
                .and_then(|bytes| {
                    let text = String::from_utf8_lossy(&bytes);
                    parse_comic_info_xml(&text)
                })
        } else {
            None
        };

        Ok(Self {
            path: path.to_path_buf(),
            pages,
            info,
        })
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn decode_page(&self, index: usize) -> Result<ComicPageBitmap> {
        let entry = self
            .pages
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("Comic page index {index} out of bounds"))?;
        let mut reader = ZipFileReader::open(&self.path)?;
        let raw = reader.read_entry(entry)?;
        let dyn_img = image::load_from_memory(&raw)
            .with_context(|| format!("Failed to decode comic page '{entry}'"))?;
        let rgba = dyn_img.to_rgba8();
        Ok(rgba_to_xrgb_white_bg(&rgba))
    }

    pub fn decode_thumbnail(&self, index: usize, max_w: u32, max_h: u32) -> Result<ComicPageBitmap> {
        let entry = self
            .pages
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("Comic page index {index} out of bounds"))?;
        let mut reader = ZipFileReader::open(&self.path)?;
        let raw = reader.read_entry(entry)?;
        let dyn_img = image::load_from_memory(&raw)
            .with_context(|| format!("Failed to decode comic thumbnail '{entry}'"))?;
        let rgba = dyn_img.to_rgba8();
        let sw = rgba.width().max(1);
        let sh = rgba.height().max(1);
        let mw = max_w.max(1) as f32;
        let mh = max_h.max(1) as f32;
        let scale = (mw / sw as f32).min(mh / sh as f32).min(1.0);
        let tw = (sw as f32 * scale).round().max(1.0) as u32;
        let th = (sh as f32 * scale).round().max(1.0) as u32;
        Ok(downscale_rgba_to_xrgb(&rgba, tw, th))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zip_mini::ZipWriter;
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
    use std::io::Cursor;

    fn encode_test_image(r: u8, g: u8, b: u8, fmt: ImageFormat) -> Vec<u8> {
        let mut img = RgbaImage::new(8, 12);
        for px in img.pixels_mut() {
            *px = Rgba([r, g, b, 255]);
        }
        let mut out = Vec::new();
        let dyn_img = DynamicImage::ImageRgba8(img);
        dyn_img
            .write_to(&mut Cursor::new(&mut out), fmt)
            .expect("encode test image");
        out
    }

    #[test]
    fn test_comic_natural_sort_and_filtering() {
        let mut names = vec![
            "vol1/page10.jpg".to_string(),
            "vol1/page2.jpg".to_string(),
            "vol1/page1.jpg".to_string(),
            "vol1/page01.jpg".to_string(),
            "vol2/001.webp".to_string(),
            "__MACOSX/vol1/._page1.jpg".to_string(),
            "vol1/.DS_Store".to_string(),
            "vol1/notes.txt".to_string(),
            "vol1/".to_string(),
        ];
        names.retain(|n| is_supported_comic_image(n));
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            names,
            vec![
                "vol1/page1.jpg",
                "vol1/page01.jpg",
                "vol1/page2.jpg",
                "vol1/page10.jpg",
                "vol2/001.webp",
            ]
        );
    }

    #[test]
    fn test_comic_archive_png_jpeg_webp_and_comic_info() {
        let png_bytes = encode_test_image(220, 40, 60, ImageFormat::Png);
        let jpg_bytes = encode_test_image(30, 140, 230, ImageFormat::Jpeg);
        let webp_bytes = encode_test_image(40, 200, 90, ImageFormat::WebP);

        let comic_info_xml = br#"<?xml version="1.0" encoding="utf-8"?>
<ComicInfo>
  <Series>Starlight Odyssey</Series>
  <Title>Crossing the Rift</Title>
  <Number>12</Number>
  <Volume>2</Volume>
  <Writer>K. Sato</Writer>
  <Penciller>M. Tanaka</Penciller>
  <Publisher>Indie Press</Publisher>
  <Year>2025</Year>
  <Summary>Space explorers enter the nebula.</Summary>
  <Manga>YesAndRightToLeft</Manga>
</ComicInfo>"#;

        let mut zw = ZipWriter::new();
        zw.add_stored("__MACOSX/._001.png", b"junk");
        zw.add_stored("README.txt", b"ignored");
        zw.add_deflated("ComicInfo.xml", comic_info_xml);
        zw.add_stored("p10.webp", &webp_bytes);
        zw.add_stored("p2.jpg", &jpg_bytes);
        zw.add_stored("p1.png", &png_bytes);
        let cbz_bytes = zw.finish();

        let dir = std::env::temp_dir().join(format!(
            "micropdf_comic_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cbz_path = dir.join("issue12.cbz");
        std::fs::write(&cbz_path, &cbz_bytes).unwrap();

        let archive = ComicArchive::open(&cbz_path).expect("open ComicArchive");
        assert_eq!(archive.page_count(), 3);
        assert_eq!(archive.pages, vec!["p1.png", "p2.jpg", "p10.webp"]);

        let info = archive.info.as_ref().expect("ComicInfo parsed");
        assert_eq!(info.series.as_deref(), Some("Starlight Odyssey"));
        assert_eq!(info.title.as_deref(), Some("Crossing the Rift"));
        assert_eq!(info.number.as_deref(), Some("12"));
        assert_eq!(info.volume.as_deref(), Some("2"));
        assert_eq!(info.writer.as_deref(), Some("K. Sato"));
        assert_eq!(info.penciller.as_deref(), Some("M. Tanaka"));
        assert_eq!(info.publisher.as_deref(), Some("Indie Press"));
        assert_eq!(info.year, Some(2025));
        assert!(info.manga_rtl);

        // Decode PNG, JPEG, and WebP pages
        let b0 = archive.decode_page(0).expect("decode PNG");
        assert_eq!((b0.width, b0.height), (8, 12));
        assert_eq!(b0.pixels[0], (220 << 16) | (40 << 8) | 60);

        let b1 = archive.decode_page(1).expect("decode JPEG");
        assert_eq!((b1.width, b1.height), (8, 12));
        // JPEG compression is lossy, check dominant blue channel
        let p1 = b1.pixels[0];
        assert!((p1 & 0xFF) > ((p1 >> 16) & 0xFF));

        let b2 = archive.decode_page(2).expect("decode WebP");
        assert_eq!((b2.width, b2.height), (8, 12));
        assert_eq!(b2.pixels[0], (40 << 16) | (200 << 8) | 90);

        // Thumbnail downscale
        let thumb = archive.decode_thumbnail(2, 4, 6).expect("decode WebP thumb");
        assert_eq!((thumb.width, thumb.height), (4, 6));
        assert_eq!(thumb.pixels.len(), 24);

        let _ = std::fs::remove_dir_all(&dir);
    }
}

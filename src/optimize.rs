//! v1.2.1: native PDF optimizer and scan cleanup.
//!
//! Ports the ideas from the earlier `compress_pdfs.py` (Ghostscript) and `clean_scans.py`
//! (levels + sharpen) scripts into Rust, with no external tools:
//! * downsample images to a target DPI and re-encode them as JPEG;
//! * on scanned pages, push off-white paper to pure white and dark ink to black ("levels"),
//!   then apply mild sharpening, keeping ink colour (blue signatures stay blue);
//! * optional gray or 1-bit black-and-white output;
//! * drop orphaned objects and compress remaining streams.
//!
//! The optimizer edits image streams in place with lopdf: page content, stacking order and
//! images shared across pages are untouched, and nothing is left orphaned. Images are
//! processed one at a time (`OptimizeJob::step`), so peak RAM is the parsed document plus
//! one decoded image (about 26 MB for a 300-dpi A4 colour scan), and the UI can run one
//! step per frame instead of freezing.

use crate::engine::PdfEngine;
use anyhow::{Context, Result};
use pdfium_render::prelude::*;
use std::io::Cursor;

/// `BlackWhite` is selectable once the Optimize menu UI lands.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode {
    /// Keep colour (blue-ink signatures stay blue).
    Color,
    /// 8-bit grayscale JPEG.
    Gray,
    /// 1-bit black and white. Smallest for text scans; signatures become jagged.
    BlackWhite,
}

/// `Smaller` and `Print` are selectable once the Optimize menu UI lands (Z uses `Balanced`).
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptimizePreset {
    /// 150 dpi, JPEG quality 60.
    Smaller,
    /// 200 dpi, JPEG quality 70 (default).
    Balanced,
    /// 300 dpi, JPEG quality 80.
    Print,
}

#[derive(Clone, Debug)]
pub struct OptimizeOptions {
    pub target_dpi: f32,
    pub jpeg_quality: u8,
    pub mode: ColorMode,
    /// Apply levels + sharpening to images that cover most of a page (scans).
    pub clean_scans: bool,
    /// Channel values at or above this become pure white (default 200).
    pub white_point: u8,
    /// Channel values at or below this become pure black (default 40).
    pub black_point: u8,
    pub sharpen: bool,
}

impl OptimizeOptions {
    pub fn preset(preset: OptimizePreset) -> Self {
        let (target_dpi, jpeg_quality) = match preset {
            OptimizePreset::Smaller => (150.0, 60),
            OptimizePreset::Balanced => (200.0, 70),
            OptimizePreset::Print => (300.0, 80),
        };
        Self {
            target_dpi,
            jpeg_quality,
            mode: ColorMode::Color,
            clean_scans: true,
            white_point: 200,
            black_point: 40,
            sharpen: true,
        }
    }
}

impl Default for OptimizeOptions {
    fn default() -> Self {
        Self::preset(OptimizePreset::Balanced)
    }
}

/// Result of `PdfEngine::optimize_doc`.
pub struct OptimizeReport {
    /// Optimized PDF. Empty when `kept_original` is true.
    pub bytes: Vec<u8>,
    pub original_len: usize,
    pub images_seen: usize,
    pub images_rewritten: usize,
    /// Images left untouched (transparency, rotation/flip, already-efficient bilevel, or no gain).
    pub images_skipped: usize,
    /// True when the optimized file would not have been smaller; nothing should be written.
    pub kept_original: bool,
}

/// Images below this pixel count are left alone in colour mode (logos, icons). They save
/// little, and because PDFium cannot share one new image across pages, rewriting a logo
/// repeated on every page could make the file bigger.
const MIN_PIXELS_COLOR_MODE: f32 = 250_000.0;
/// Only downsample when the image is at least this much sharper than the target DPI.
const DOWNSAMPLE_THRESHOLD: f32 = 1.25;
/// An image covering at least this share of the page is treated as a scanned page.
const SCAN_AREA_SHARE: f32 = 0.85;

struct PageImageStats {
    seen: usize,
    rewritten: usize,
    skipped: usize,
}

enum Replacement {
    Jpeg(Vec<u8>),
    Raw(image::DynamicImage),
}

impl PdfEngine {
    /// Optimizes a copy of `doc` in one go; `doc` itself is not modified.
    /// For large files prefer `OptimizeJob` and call `step()` once per frame.
    pub fn optimize_doc(&self, doc: &PdfDocument, opts: &OptimizeOptions) -> Result<OptimizeReport> {
        let bytes = doc
            .save_to_bytes()
            .context("Failed to serialize document for optimizing")?;
        let mut job = OptimizeJob::new(bytes, opts.clone())?;
        while !job.is_done() {
            job.step();
        }
        job.finish()
    }

    /// Converts text, vector paths, annotations AND images on one page to grayscale.
    ///
    /// v1.2.1 fix: v1.0–1.2 converted images only when they were uncompressed or
    /// Flate-compressed `/DeviceRGB`, so JPEG photos and ICC-profiled images (most colour
    /// pictures) were silently skipped and the button appeared to do nothing.
    pub fn convert_page_to_bw(&self, doc_bytes: &[u8], page_index: u16) -> Result<Vec<u8>> {
        let mut doc = self.load_pdf_from_bytes_safe(doc_bytes.to_vec())?;
        let to_gray = |c: PdfColor| -> PdfColor {
            let y = (0.299 * (c.red() as f32) + 0.587 * (c.green() as f32) + 0.114 * (c.blue() as f32))
                .round()
                .clamp(0.0, 255.0) as u8;
            PdfColor::new(y, y, y, c.alpha())
        };

        {
            let mut page = doc
                .pages_mut()
                .get(page_index)
                .context("Page index out of bounds for BW conversion")?;
            let prev_strategy = page.content_regeneration_strategy();
            page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
            for i in 0..page.objects().len() {
                if let Ok(mut obj) = page.objects_mut().get(i) {
                    if let Ok(fc) = obj.fill_color() {
                        let _ = obj.set_fill_color(to_gray(fc));
                    }
                    if let Ok(sc) = obj.stroke_color() {
                        let _ = obj.set_stroke_color(to_gray(sc));
                    }
                }
            }
            for a_idx in 0..page.annotations().len() {
                if let Ok(mut annot) = page.annotations_mut().get(a_idx) {
                    if let Ok(fc) = annot.fill_color() {
                        let _ = annot.set_fill_color(to_gray(fc));
                    }
                    if let Ok(sc) = annot.stroke_color() {
                        let _ = annot.set_stroke_color(to_gray(sc));
                    }
                }
            }
            page.regenerate_content()
                .context("Failed to regenerate page after recolouring")?;
            page.set_content_regeneration_strategy(prev_strategy);
        }

        let gray = OptimizeOptions {
            target_dpi: f32::MAX, // never downsample
            jpeg_quality: 85,
            mode: ColorMode::Gray,
            clean_scans: false,
            white_point: 255,
            black_point: 0,
            sharpen: false,
        };
        Self::process_page_images(&mut doc, page_index, &gray, true)?;

        let raw = doc
            .save_to_bytes()
            .context("Failed to serialize PDF after BW conversion")?;
        Self::finish_streams(&raw, false)
    }

    /// Rewrites the images on one page according to `opts`.
    /// `force` processes every image (used by the B/W button), not only those worth shrinking.
    fn process_page_images(
        doc: &mut PdfDocument,
        page_index: u16,
        opts: &OptimizeOptions,
        force: bool,
    ) -> Result<PageImageStats> {
        let mut stats = PageImageStats { seen: 0, rewritten: 0, skipped: 0 };
        // (object index, replacement, original matrix)
        let mut jobs: Vec<(PdfPageObjectIndex, Replacement, PdfMatrix)> = Vec::new();

        // Phase 1: read-only pass. Decode and re-encode one image at a time.
        {
            let page = doc.pages().get(page_index)?;
            let page_area = (page.width().value * page.height().value).max(1.0);
            for i in 0..page.objects().len() {
                let Ok(obj) = page.objects().get(i) else { continue };
                let Some(io) = obj.as_image_object() else { continue };
                stats.seen += 1;
                match Self::plan_image(doc, io, page_area, opts, force) {
                    Ok(Some((replacement, matrix))) => jobs.push((i, replacement, matrix)),
                    _ => stats.skipped += 1,
                }
            }
        }

        if jobs.is_empty() {
            return Ok(stats);
        }

        // Phase 2: build the new image objects, then swap them in at the same z-order index.
        let mut built = Vec::with_capacity(jobs.len());
        for (idx, replacement, matrix) in jobs {
            let new_obj = match replacement {
                Replacement::Jpeg(bytes) => {
                    PdfPageImageObject::new_from_jpeg_reader(doc, Cursor::new(bytes))
                }
                Replacement::Raw(img) => PdfPageImageObject::new(doc, &img),
            };
            match new_obj.and_then(|mut o| o.apply_matrix(matrix).map(|_| o)) {
                Ok(o) => built.push((idx, o)),
                Err(_) => stats.skipped += 1,
            }
        }

        let mut page = doc.pages_mut().get(page_index)?;
        let prev_strategy = page.content_regeneration_strategy();
        page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);

        // pdfium-render 0.8.37 only exposes insert-at-index behind a PDFium version feature, so
        // to keep the original stacking order we detach every object from the first replaced
        // image onward and re-append them in order, swapping in the new images as we go.
        let first_idx = built[0].0;
        let mut tail = Vec::new();
        while page.objects().len() > first_idx {
            match page.objects_mut().remove_object_at_index(first_idx) {
                Ok(o) => tail.push(o),
                Err(e) => {
                    page.set_content_regeneration_strategy(prev_strategy);
                    return Err(anyhow::anyhow!("Failed to detach page object: {e:?}"));
                }
            }
        }
        let mut replacements = built.into_iter().peekable();
        for (offset, old_obj) in tail.into_iter().enumerate() {
            let orig_idx = first_idx + offset;
            let to_add = if replacements.peek().map(|(i, _)| *i) == Some(orig_idx) {
                let (_, new_obj) = replacements.next().unwrap();
                // Dropping removed objects segfaults with pdfium-render 0.8.37 (see redaction).
                std::mem::forget(old_obj);
                stats.rewritten += 1;
                PdfPageObject::from(new_obj)
            } else {
                old_obj
            };
            if let Err(e) = page.objects_mut().add_object(to_add) {
                page.set_content_regeneration_strategy(prev_strategy);
                return Err(anyhow::anyhow!("Failed to re-attach page object: {e:?}"));
            }
        }
        let regen = page.regenerate_content();
        page.set_content_regeneration_strategy(prev_strategy);
        regen.context("Failed to regenerate page content after rewriting images")?;
        Ok(stats)
    }

    /// Decides what to do with one image. Returns `None` to leave it untouched.
    fn plan_image(
        doc: &PdfDocument,
        io: &PdfPageImageObject,
        page_area: f32,
        opts: &OptimizeOptions,
        force: bool,
    ) -> Result<Option<(Replacement, PdfMatrix)>> {
        let m = io.matrix()?;
        // Rotated, skewed or mirrored placements are skipped: re-rendering them upright and
        // reusing the matrix would change their orientation.
        if m.b().abs() > 1e-3 || m.c().abs() > 1e-3 || m.a() <= 0.0 || m.d() <= 0.0 {
            return Ok(None);
        }
        let (w, h) = (io.width()? as f32, io.height()? as f32);
        if w < 1.0 || h < 1.0 {
            return Ok(None);
        }

        let filters = io.filters();
        let filter_names: Vec<String> = (0..filters.len())
            .filter_map(|k| filters.get(k).ok().map(|f| f.name().to_string()))
            .collect();
        let has = |name: &str| filter_names.iter().any(|f| f == name);
        // Already efficient bilevel encodings; recompressing would only make them bigger.
        if has("JBIG2Decode") || has("CCITTFaxDecode") {
            return Ok(None);
        }

        let (placed_w, placed_h) = (m.a(), m.d());
        let is_scan = placed_w * placed_h >= SCAN_AREA_SHARE * page_area;
        let target_w = (placed_w / 72.0 * opts.target_dpi).min(w);
        let target_h = (placed_h / 72.0 * opts.target_dpi).min(h);
        let downsample = w > target_w * DOWNSAMPLE_THRESHOLD && h > target_h * DOWNSAMPLE_THRESHOLD;
        let clean = opts.clean_scans && is_scan;
        let recolor = opts.mode != ColorMode::Color;
        let is_jpeg = has("DCTDecode");
        let small = w * h < MIN_PIXELS_COLOR_MODE;

        if !force {
            if small && !recolor {
                return Ok(None);
            }
            if !(downsample || clean || recolor || !is_jpeg) {
                return Ok(None);
            }
        }

        let (out_w, out_h) = if downsample {
            (target_w.round().max(1.0) as Pixels, target_h.round().max(1.0) as Pixels)
        } else {
            (w as Pixels, h as Pixels)
        };
        // Decoded with filters, /Decode arrays, colour spaces and masks applied by PDFium,
        // scaled by PDFium to the output size.
        let rendered = io
            .get_processed_image_with_size(doc, out_w, out_h)
            .context("PDFium could not decode the image")?
            .to_rgba8();
        // JPEG has no alpha channel: leave transparent images alone.
        if rendered.pixels().any(|p| p[3] < 250) {
            return Ok(None);
        }
        let mut rgb = image::DynamicImage::ImageRgba8(rendered).to_rgb8();

        if clean {
            let lut = levels_lut(opts.black_point, opts.white_point);
            for p in rgb.pixels_mut() {
                p[0] = lut[p[0] as usize];
                p[1] = lut[p[1] as usize];
                p[2] = lut[p[2] as usize];
            }
            if opts.sharpen {
                rgb = image::imageops::unsharpen(&rgb, 0.8, 3);
            }
        }

        let replacement = match opts.mode {
            ColorMode::Color => Replacement::Jpeg(encode_jpeg(
                image::DynamicImage::ImageRgb8(rgb),
                opts.jpeg_quality,
            )?),
            ColorMode::Gray => Replacement::Jpeg(encode_jpeg(
                image::DynamicImage::ImageLuma8(image::DynamicImage::ImageRgb8(rgb).to_luma8()),
                opts.jpeg_quality,
            )?),
            ColorMode::BlackWhite => {
                // Stored raw here; `finish_streams` repacks pure black/white images to 1 bit.
                let mut luma = image::DynamicImage::ImageRgb8(rgb).to_luma8();
                for p in luma.pixels_mut() {
                    p[0] = if p[0] < 128 { 0 } else { 255 };
                }
                Replacement::Raw(image::DynamicImage::ImageLuma8(luma))
            }
        };
        Ok(Some((replacement, m)))
    }

    /// Final lopdf pass: repack pure black/white images to 1 bit (optional), drop orphaned
    /// objects (the replaced originals), and Flate-compress uncompressed streams.
    fn finish_streams(pdf: &[u8], repack_bilevel: bool) -> Result<Vec<u8>> {
        let mut ldoc = lopdf::Document::load_mem(pdf).context("lopdf could not parse the output")?;
        drop_opaque_smasks(&mut ldoc);
        if repack_bilevel {
            for obj in ldoc.objects.values_mut() {
                if let lopdf::Object::Stream(stream) = obj {
                    repack_to_one_bit(stream);
                }
            }
        }
        ldoc.prune_objects();
        ldoc.compress();
        let mut out = Vec::with_capacity(pdf.len() / 2);
        ldoc.save_to(&mut out).context("lopdf could not write the output")?;
        Ok(out)
    }
}

/// Incremental optimizer. `new` parses the PDF and plans the work, `step` rewrites one
/// image, `finish` writes the result. Each step takes roughly 0.1-0.5 s for a 300-dpi page.
pub struct OptimizeJob {
    ldoc: lopdf::Document,
    queue: Vec<ImagePlan>,
    next: usize,
    opts: OptimizeOptions,
    original_len: usize,
    rewritten: usize,
    skipped: usize,
}

struct ImagePlan {
    id: lopdf::ObjectId,
    /// Colour channels resolved at planning time (ICC profiles live in other objects).
    channels: Option<usize>,
    /// Largest size (in points) at which the image is drawn anywhere in the document.
    placed_w: f32,
    placed_h: f32,
    is_scan: bool,
}

impl OptimizeJob {
    pub fn new(pdf: Vec<u8>, opts: OptimizeOptions) -> Result<Self> {
        let original_len = pdf.len();
        let ldoc = lopdf::Document::load_mem(&pdf).context("Could not parse the PDF for optimizing")?;
        drop(pdf);
        let queue = plan_images(&ldoc);
        Ok(Self { ldoc, queue, next: 0, opts, original_len, rewritten: 0, skipped: 0 })
    }

    /// Number of images found on pages.
    #[allow(dead_code)]
    pub fn total(&self) -> usize {
        self.queue.len()
    }

    /// Images processed so far (for a progress display).
    #[allow(dead_code)]
    pub fn processed(&self) -> usize {
        self.next
    }

    pub fn is_done(&self) -> bool {
        self.next >= self.queue.len()
    }

    /// Rewrites the next image. Failures on one image only skip that image.
    pub fn step(&mut self) {
        let Some(plan) = self.queue.get(self.next) else { return };
        let (id, pw, ph, scan, channels) = (plan.id, plan.placed_w, plan.placed_h, plan.is_scan, plan.channels);
        self.next += 1;
        let done = match self.ldoc.get_object_mut(id) {
            Ok(lopdf::Object::Stream(stream)) => match channels {
                Some(ch) => rewrite_image_stream(stream, pw, ph, scan, ch, &self.opts).unwrap_or(false),
                None => false,
            },
            _ => false,
        };
        if done {
            self.rewritten += 1;
        } else {
            self.skipped += 1;
        }
    }

    pub fn finish(mut self) -> Result<OptimizeReport> {
        let images_seen = self.queue.len();
        self.ldoc.prune_objects();
        self.ldoc.compress();
        let mut out = Vec::with_capacity(self.original_len / 2);
        self.ldoc
            .save_to(&mut out)
            .context("Could not write the optimized PDF")?;
        let kept_original = out.len() >= self.original_len;
        Ok(OptimizeReport {
            bytes: if kept_original { Vec::new() } else { out },
            original_len: self.original_len,
            images_seen,
            images_rewritten: self.rewritten,
            images_skipped: self.skipped,
            kept_original,
        })
    }
}

/// Finds every image XObject drawn directly by a page and the largest size it is drawn at,
/// by following `q`/`Q`/`cm`/`Do` in each page's content stream. Images inside form
/// XObjects and inline images are not touched.
fn plan_images(ldoc: &lopdf::Document) -> Vec<ImagePlan> {
    use std::collections::BTreeMap;
    let mut plans: BTreeMap<lopdf::ObjectId, ImagePlan> = BTreeMap::new();
    for (_num, page_id) in ldoc.get_pages() {
        let (pw, ph) = page_size(ldoc, page_id);
        let page_area = (pw * ph).max(1.0);
        let xobjects = page_xobjects(ldoc, page_id);
        if xobjects.is_empty() {
            continue;
        }
        let Ok(content) = ldoc.get_page_content(page_id) else { continue };
        let Ok(ops) = lopdf::content::Content::decode(&content) else { continue };
        let mut ctm = [1.0f32, 0.0, 0.0, 1.0, 0.0, 0.0];
        let mut stack: Vec<[f32; 6]> = Vec::new();
        for op in ops.operations {
            match op.operator.as_str() {
                "q" => stack.push(ctm),
                "Q" => ctm = stack.pop().unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
                "cm" if op.operands.len() == 6 => {
                    let v: Vec<f32> = op.operands.iter().map(|o| o.as_float().unwrap_or(0.0)).collect();
                    ctm = [
                        v[0] * ctm[0] + v[1] * ctm[2],
                        v[0] * ctm[1] + v[1] * ctm[3],
                        v[2] * ctm[0] + v[3] * ctm[2],
                        v[2] * ctm[1] + v[3] * ctm[3],
                        v[4] * ctm[0] + v[5] * ctm[2] + ctm[4],
                        v[4] * ctm[1] + v[5] * ctm[3] + ctm[5],
                    ];
                }
                "Do" => {
                    let Some(name) = op.operands.first().and_then(|o| o.as_name().ok()) else { continue };
                    let Some(&id) = xobjects.get(name) else { continue };
                    let is_image = ldoc
                        .get_object(id)
                        .and_then(|o| o.as_stream())
                        .map(|st| st.dict.get(b"Subtype").and_then(|v| v.as_name()).ok() == Some(b"Image".as_slice()))
                        .unwrap_or(false);
                    if !is_image {
                        continue;
                    }
                    let w = (ctm[0] * ctm[0] + ctm[1] * ctm[1]).sqrt();
                    let h = (ctm[2] * ctm[2] + ctm[3] * ctm[3]).sqrt();
                    let entry = plans.entry(id).or_insert_with(|| ImagePlan {
                        id,
                        channels: ldoc
                            .get_object(id)
                            .and_then(|o| o.as_stream())
                            .ok()
                            .and_then(|st| image_channels(ldoc, &st.dict)),
                        placed_w: 0.0,
                        placed_h: 0.0,
                        is_scan: false,
                    });
                    entry.placed_w = entry.placed_w.max(w);
                    entry.placed_h = entry.placed_h.max(h);
                    entry.is_scan |= w * h >= SCAN_AREA_SHARE * page_area;
                }
                _ => {}
            }
        }
    }
    plans.into_values().collect()
}

/// Page width and height in points from the (possibly inherited) /MediaBox; A4 if missing.
fn page_size(ldoc: &lopdf::Document, page_id: lopdf::ObjectId) -> (f32, f32) {
    let mut node = ldoc.get_dictionary(page_id).ok();
    for _ in 0..32 {
        let Some(dict) = node else { break };
        if let Ok(mb) = dict.get(b"MediaBox") {
            let arr = match mb {
                lopdf::Object::Reference(r) => ldoc.get_object(*r).and_then(|o| o.as_array()).ok(),
                lopdf::Object::Array(a) => Some(a),
                _ => None,
            };
            if let Some(a) = arr.filter(|a| a.len() == 4) {
                let v: Vec<f32> = a.iter().map(|o| o.as_float().unwrap_or(0.0)).collect();
                return ((v[2] - v[0]).abs(), (v[3] - v[1]).abs());
            }
        }
        node = dict
            .get(b"Parent")
            .and_then(|p| p.as_reference())
            .ok()
            .and_then(|id| ldoc.get_dictionary(id).ok());
    }
    (595.0, 842.0)
}

/// XObject name -> object id for a page, including resources inherited from parents.
fn page_xobjects(ldoc: &lopdf::Document, page_id: lopdf::ObjectId) -> std::collections::HashMap<Vec<u8>, lopdf::ObjectId> {
    let mut map = std::collections::HashMap::new();
    let Ok((direct, inherited)) = ldoc.get_page_resources(page_id) else { return map };
    let mut dicts: Vec<&lopdf::Dictionary> = Vec::new();
    if let Some(d) = direct {
        dicts.push(d);
    }
    for id in inherited {
        if let Ok(d) = ldoc.get_dictionary(id) {
            dicts.push(d);
        }
    }
    // Earlier (more specific) dictionaries win.
    for res in dicts.into_iter().rev() {
        let xo = match res.get(b"XObject") {
            Ok(lopdf::Object::Reference(r)) => ldoc.get_dictionary(*r).ok(),
            Ok(lopdf::Object::Dictionary(d)) => Some(d),
            _ => None,
        };
        if let Some(xo) = xo {
            for (name, val) in xo.iter() {
                if let Ok(id) = val.as_reference() {
                    map.insert(name.clone(), id);
                }
            }
        }
    }
    map
}

/// Decodes, cleans, resizes and re-encodes one image stream in place.
/// Returns Ok(true) if the stream was replaced with a smaller version.
fn rewrite_image_stream(
    stream: &mut lopdf::Stream,
    placed_w: f32,
    placed_h: f32,
    is_scan: bool,
    channels: usize,
    opts: &OptimizeOptions,
) -> Result<bool> {
    let d = &stream.dict;
    let int = |k: &[u8]| d.get(k).and_then(|v| v.as_i64()).ok();
    // Leave stencil masks, colour-key masks, Decode arrays and non-8-bit images alone.
    if d.get(b"ImageMask").and_then(|v| v.as_bool()).unwrap_or(false)
        || matches!(d.get(b"Mask"), Ok(lopdf::Object::Array(_)))
        || d.has(b"Decode")
        || int(b"BitsPerComponent") != Some(8)
    {
        return Ok(false);
    }
    let (Some(w), Some(h)) = (int(b"Width"), int(b"Height")) else { return Ok(false) };
    if w <= 0 || h <= 0 || placed_w < 1.0 || placed_h < 1.0 {
        return Ok(false);
    }
    let (w, h) = (w as u32, h as u32);
    let filters: Vec<Vec<u8>> = match d.get(b"Filter") {
        Err(_) => Vec::new(),
        Ok(lopdf::Object::Name(n)) => vec![n.clone()],
        Ok(lopdf::Object::Array(a)) => a.iter().filter_map(|o| o.as_name().ok().map(|n| n.to_vec())).collect(),
        _ => return Ok(false),
    };
    let is_jpeg = filters.len() == 1 && filters[0] == b"DCTDecode";
    let is_flate_or_raw = filters.is_empty() || (filters.len() == 1 && filters[0] == b"FlateDecode");
    if !is_jpeg && !is_flate_or_raw {
        return Ok(false); // JPX, JBIG2, CCITT, LZW, ...
    }

    let (wf, hf) = (w as f32, h as f32);
    let target_w = (placed_w / 72.0 * opts.target_dpi).min(wf);
    let target_h = (placed_h / 72.0 * opts.target_dpi).min(hf);
    let downsample = wf > target_w * DOWNSAMPLE_THRESHOLD && hf > target_h * DOWNSAMPLE_THRESHOLD;
    let clean = opts.clean_scans && is_scan;
    let recolor = opts.mode != ColorMode::Color && !(opts.mode == ColorMode::Gray && channels == 1 && is_jpeg && !downsample && !clean);
    let small = wf * hf < MIN_PIXELS_COLOR_MODE;
    if small && !recolor {
        return Ok(false);
    }
    if !(downsample || clean || recolor || !is_jpeg) {
        return Ok(false);
    }

    // Decode.
    let img = if is_jpeg {
        match image::load_from_memory_with_format(&stream.content, image::ImageFormat::Jpeg) {
            Ok(i) => i,
            Err(_) => return Ok(false),
        }
    } else {
        let Some(raw) = stream_data(stream) else { return Ok(false) };
        if raw.len() != (w as usize) * (h as usize) * channels {
            return Ok(false);
        }
        let built = if channels == 1 {
            image::GrayImage::from_raw(w, h, raw).map(image::DynamicImage::ImageLuma8)
        } else {
            image::RgbImage::from_raw(w, h, raw).map(image::DynamicImage::ImageRgb8)
        };
        match built {
            Some(i) => i,
            None => return Ok(false),
        }
    };
    if img.width() != w || img.height() != h {
        return Ok(false);
    }

    let img = if downsample {
        // Area-averaging downscale: about 4x faster than a Triangle filter on 300-dpi pages
        // and needs no floating-point intermediate buffer.
        img.thumbnail_exact(target_w.round().max(1.0) as u32, target_h.round().max(1.0) as u32)
    } else {
        img
    };
    let mut rgb = img.to_rgb8();
    drop(img);
    if clean {
        let lut = levels_lut(opts.black_point, opts.white_point);
        for p in rgb.pixels_mut() {
            p[0] = lut[p[0] as usize];
            p[1] = lut[p[1] as usize];
            p[2] = lut[p[2] as usize];
        }
        if opts.sharpen {
            rgb = sharpen_rgb(&rgb);
        }
    }
    let (ow, oh) = rgb.dimensions();

    let (data, color_space, bpc, filter) = match opts.mode {
        ColorMode::Color if channels == 3 => (
            encode_jpeg(image::DynamicImage::ImageRgb8(rgb), opts.jpeg_quality)?,
            "DeviceRGB",
            8,
            Some("DCTDecode"),
        ),
        ColorMode::Color | ColorMode::Gray => (
            encode_jpeg(
                image::DynamicImage::ImageLuma8(image::DynamicImage::ImageRgb8(rgb).to_luma8()),
                opts.jpeg_quality,
            )?,
            "DeviceGray",
            8,
            Some("DCTDecode"),
        ),
        ColorMode::BlackWhite => {
            let luma = image::DynamicImage::ImageRgb8(rgb).to_luma8();
            let row_bytes = (ow as usize).div_ceil(8);
            let mut packed = vec![0u8; row_bytes * oh as usize];
            for (x, y, p) in luma.enumerate_pixels() {
                if p[0] >= 128 {
                    packed[y as usize * row_bytes + x as usize / 8] |= 0x80 >> (x % 8);
                }
            }
            (packed, "DeviceGray", 1, None)
        }
    };

    let mut new_stream = lopdf::Stream::new(stream.dict.clone(), data);
    for k in [b"Filter".as_slice(), b"DecodeParms", b"Width", b"Height", b"ColorSpace", b"BitsPerComponent"] {
        new_stream.dict.remove(k);
    }
    new_stream.dict.set("Width", ow as i64);
    new_stream.dict.set("Height", oh as i64);
    new_stream.dict.set("ColorSpace", lopdf::Object::Name(color_space.as_bytes().to_vec()));
    new_stream.dict.set("BitsPerComponent", bpc as i64);
    match filter {
        Some(f) => {
            new_stream.dict.set("Filter", lopdf::Object::Name(f.as_bytes().to_vec()));
            new_stream = new_stream.with_compression(false);
        }
        None => {
            let _ = new_stream.compress();
        }
    }
    // Keep the original if the new encoding is not smaller.
    if new_stream.content.len() >= stream.content.len() {
        return Ok(false);
    }
    *stream = new_stream;
    Ok(true)
}

/// 1 for gray, 3 for RGB (Device, Cal or ICC-based with /N 1 or 3); None for anything else
/// (CMYK, Indexed, Lab, Separation, ...).
fn image_channels(ldoc: &lopdf::Document, d: &lopdf::Dictionary) -> Option<usize> {
    let cs = match d.get(b"ColorSpace").ok()? {
        lopdf::Object::Reference(r) => ldoc.get_object(*r).ok()?,
        other => other,
    };
    match cs {
        lopdf::Object::Name(n) => match n.as_slice() {
            b"DeviceGray" | b"CalGray" => Some(1),
            b"DeviceRGB" | b"CalRGB" => Some(3),
            _ => None,
        },
        lopdf::Object::Array(a) => match a.first()?.as_name().ok()? {
            b"CalGray" => Some(1),
            b"CalRGB" => Some(3),
            b"ICCBased" => {
                let profile = match a.get(1)? {
                    lopdf::Object::Reference(r) => ldoc.get_object(*r).ok()?.as_stream().ok()?,
                    lopdf::Object::Stream(st) => st,
                    _ => return None,
                };
                match profile.dict.get(b"N").and_then(|v| v.as_i64()).ok()? {
                    1 => Some(1),
                    3 => Some(3),
                    _ => None,
                }
            }
            _ => None,
        },
        _ => None,
    }
}

/// Mild integer sharpen: out = (9*centre - up - down - left - right) / 5, i.e. the kernel
/// [0 -0.2 0; -0.2 1.8 -0.2; 0 -0.2 0]. About 20x faster than `imageops::filter3x3`.
fn sharpen_rgb(src: &image::RgbImage) -> image::RgbImage {
    let (w, h) = src.dimensions();
    let mut out = src.clone();
    if w < 3 || h < 3 {
        return out;
    }
    let stride = w as usize * 3;
    let s = src.as_raw();
    let o: &mut [u8] = &mut out;
    for y in 1..(h as usize - 1) {
        let row = y * stride;
        for i in (row + 3)..(row + stride - 3) {
            let v = 9 * s[i] as i32 - s[i - 3] as i32 - s[i + 3] as i32 - s[i - stride] as i32
                - s[i + stride] as i32;
            o[i] = (v / 5).clamp(0, 255) as u8;
        }
    }
    out
}

/// Lookup table for a levels adjustment: `<= black` -> 0, `>= white` -> 255, linear between.
fn levels_lut(black: u8, white: u8) -> [u8; 256] {
    let (b, w) = (black as f32, (white.max(black.saturating_add(1))) as f32);
    let mut lut = [0u8; 256];
    for (v, slot) in lut.iter_mut().enumerate() {
        *slot = (((v as f32) - b) * 255.0 / (w - b)).round().clamp(0.0, 255.0) as u8;
    }
    lut
}

fn encode_jpeg(img: image::DynamicImage, quality: u8) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100))
        .encode_image(&img)
        .context("JPEG encoding failed")?;
    Ok(out)
}

/// Decompresses a stream's data. lopdf 0.34's `decompressed_content()` refuses every
/// `/Subtype /Image` stream (returns `Error::Type`), which is also why the v1.0 B/W image
/// conversion never ran, so the check is bypassed on a copy of the dictionary.
fn stream_data(stream: &lopdf::Stream) -> Option<Vec<u8>> {
    if !stream.dict.has(b"Filter") {
        return Some(stream.content.clone());
    }
    let mut dict = stream.dict.clone();
    dict.remove(b"Subtype");
    let probe = lopdf::Stream::new(dict, stream.content.clone());
    probe.decompressed_content().ok()
}

/// PDFium attaches an `/SMask` to every image it creates from a bitmap, even when the bitmap
/// is fully opaque. Removing masks whose every value is 255 is lossless and lets those
/// images be repacked and stored without a second stream.
fn drop_opaque_smasks(ldoc: &mut lopdf::Document) {
    let mut opaque = std::collections::HashSet::new();
    for (id, obj) in &ldoc.objects {
        let lopdf::Object::Stream(s) = obj else { continue };
        let gray8 = s.dict.get(b"ColorSpace").and_then(|c| c.as_name()).ok() == Some(b"DeviceGray".as_slice())
            && s.dict.get(b"BitsPerComponent").and_then(|b| b.as_i64()).ok() == Some(8)
            && s.dict.get(b"Subtype").and_then(|v| v.as_name()).ok() == Some(b"Image".as_slice());
        if !gray8 || s.dict.has(b"Decode") {
            continue;
        }
        let Some(data) = stream_data(s) else { continue };
        if !data.is_empty() && data.iter().all(|&v| v == 255) {
            opaque.insert(*id);
        }
    }
    if opaque.is_empty() {
        return;
    }
    for obj in ldoc.objects.values_mut() {
        if let lopdf::Object::Stream(s) = obj {
            let target = s.dict.get(b"SMask").and_then(|v| v.as_reference()).ok();
            if target.is_some_and(|r| opaque.contains(&r)) {
                s.dict.remove(b"SMask");
            }
        }
    }
}

/// If `stream` is an 8-bit Gray or RGB image whose pixels are all pure black or pure white,
/// rewrite it as a 1-bit DeviceGray image. Lossless; leaves anything else untouched.
fn repack_to_one_bit(stream: &mut lopdf::Stream) {
    let d = &stream.dict;
    let is_image = d
        .get(b"Subtype")
        .and_then(|s| s.as_name())
        .map(|n| n == b"Image")
        .unwrap_or(false);
    if !is_image || d.has(b"SMask") || d.has(b"Mask") || d.has(b"Decode") {
        return;
    }
    let channels = match d.get(b"ColorSpace").and_then(|c| c.as_name()) {
        Ok(b"DeviceGray") => 1usize,
        Ok(b"DeviceRGB") => 3usize,
        _ => return,
    };
    if d.get(b"BitsPerComponent").and_then(|b| b.as_i64()).unwrap_or(0) != 8 {
        return;
    }
    let (Ok(width), Ok(height)) = (
        d.get(b"Width").and_then(|v| v.as_i64()),
        d.get(b"Height").and_then(|v| v.as_i64()),
    ) else {
        return;
    };
    let (width, height) = (width.max(0) as usize, height.max(0) as usize);
    let filter_ok = match d.get(b"Filter") {
        Err(_) => true,
        Ok(lopdf::Object::Name(n)) => n == b"FlateDecode",
        Ok(lopdf::Object::Array(a)) => {
            a.len() == 1 && matches!(&a[0], lopdf::Object::Name(n) if n == b"FlateDecode")
        }
        _ => false,
    };
    if !filter_ok || width == 0 || height == 0 {
        return;
    }
    let Some(data) = stream_data(stream) else { return };
    if data.len() != width * height * channels {
        return;
    }
    let row_bytes = width.div_ceil(8);
    let mut packed = vec![0u8; row_bytes * height];
    for y in 0..height {
        for x in 0..width {
            let px = &data[(y * width + x) * channels..(y * width + x + 1) * channels];
            let v = px[0];
            if (v != 0 && v != 255) || px.iter().any(|&c| c != v) {
                return; // not a pure bilevel image
            }
            if v == 255 {
                packed[y * row_bytes + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    stream.dict.remove(b"Filter");
    stream.dict.remove(b"DecodeParms");
    stream.dict.set("ColorSpace", lopdf::Object::Name(b"DeviceGray".to_vec()));
    stream.dict.set("BitsPerComponent", 1);
    stream.set_plain_content(packed);
    let _ = stream.compress();
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    /// A 300-dpi A4 "scan": noisy off-white paper, dark text blocks, a blue signature at the
    /// bottom right and a red marker at the top left (to catch orientation errors). Real text
    /// is drawn ON TOP of the scan in a blank area to catch stacking-order errors.
    fn make_scan_pdf(engine: &PdfEngine) -> Result<Vec<u8>> {
        let (w, h) = (2480u32, 3508u32);
        let mut seed: u32 = 7;
        let mut rnd = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            seed >> 16
        };
        let mut img = image::RgbImage::new(w, h);
        for p in img.pixels_mut() {
            let n = (rnd() % 13) as u8;
            *p = image::Rgb([232 + n, 229 + n, 222 + n]);
        }
        for row in (400..2600).step_by(70) {
            let mut x = 250;
            while x < w - 400 {
                let word = 60 + rnd() % 180;
                for yy in row..row + 36 {
                    for xx in x..(x + word).min(w - 250) {
                        if rnd() % 3 != 0 {
                            img.put_pixel(xx, yy, image::Rgb([38, 38, 44]));
                        }
                    }
                }
                x += word + 40;
            }
        }
        for yy in 3000..3200 {
            for xx in 1600..2200 {
                img.put_pixel(xx, yy, image::Rgb([35, 70, 190])); // blue signature
            }
        }
        for yy in 100..250 {
            for xx in 100..500 {
                img.put_pixel(xx, yy, image::Rgb([200, 30, 30])); // red top-left marker
            }
        }
        let jpg = encode_jpeg(image::DynamicImage::ImageRgb8(img), 90)?;

        let mut doc = engine.pdfium.create_new_pdf()?;
        let font = doc.fonts_mut().helvetica();
        let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::a4())?;
        let mut io = PdfPageImageObject::new_from_jpeg_reader(&doc, Cursor::new(jpg))?;
        io.scale(595.0, 842.0)?;
        page.objects_mut().add_image_object(io)?;
        // Blank paper area of the scan: x 300..550 pt, y 70..110 pt (from bottom).
        let mut t = PdfPageTextObject::new(&doc, "OVERLAY", font, PdfPoints::new(28.0))?;
        t.translate(PdfPoints::new(320.0), PdfPoints::new(80.0))?;
        page.objects_mut().add_text_object(t)?;
        Ok(doc.save_to_bytes()?)
    }

    fn only_image(page: &PdfPage) -> image::RgbaImage {
        let imgs: Vec<_> = page.objects().iter().filter(|o| o.as_image_object().is_some()).collect();
        assert_eq!(imgs.len(), 1, "expected exactly one image object");
        imgs[0].as_image_object().unwrap().get_raw_image().unwrap().to_rgba8()
    }

    #[test]
    fn optimize_scan_whitens_keeps_signature_orientation_and_text_on_top() -> Result<()> {
        let engine = PdfEngine::new()?;
        let src = make_scan_pdf(&engine)?;
        let doc = engine.load_pdf_from_bytes_safe(src)?;
        let report = engine.optimize_doc(&doc, &OptimizeOptions::default())?;
        eprintln!(
            "scan: {:.2} MB -> {:.2} MB, rewritten {}/{}",
            report.original_len as f64 / 1048576.0,
            report.bytes.len() as f64 / 1048576.0,
            report.images_rewritten,
            report.images_seen
        );
        assert!(!report.kept_original);
        assert_eq!(report.images_rewritten, 1);
        assert!(report.bytes.len() * 10 < report.original_len * 6, "expected >= 40% smaller");

        let out = engine.load_pdf_from_bytes_safe(report.bytes)?;
        let page = out.pages().get(0)?;
        let img = only_image(&page);
        // 595 pt at 200 dpi = 1653 px
        assert!((img.width() as i32 - 1653).abs() <= 2, "width {}", img.width());

        let (iw, ih) = (img.width() as f32, img.height() as f32);
        let at = |fx: f32, fy: f32| *img.get_pixel((fx * iw) as u32, (fy * ih) as u32);
        let paper = at(0.05, 0.92);
        assert!(paper[0] >= 250 && paper[1] >= 250 && paper[2] >= 250, "paper not white: {paper:?}");
        let sig = at(1900.0 / 2480.0, 3100.0 / 3508.0);
        assert!(sig[2] as i32 > sig[0] as i32 + 80, "signature lost its blue: {sig:?}");
        let marker = at(300.0 / 2480.0, 175.0 / 3508.0);
        assert!(marker[0] > 150 && marker[1] < 90, "orientation changed: {marker:?}");

        assert!(page.text()?.all().contains("OVERLAY"), "text object lost");
        // Render and look for dark text pixels in the blank area: proves text is above the image.
        let bmp = page.render_with_config(&PdfRenderConfig::new().set_target_width(1190))?;
        let r = bmp.as_image().to_rgba8();
        let scale = r.width() as f32 / 595.0;
        let (x0, x1) = ((320.0 * scale) as u32, (480.0 * scale) as u32);
        let (y0, y1) = (((842.0 - 110.0) * scale) as u32, ((842.0 - 78.0) * scale) as u32);
        let dark = (y0..y1)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .filter(|&(x, y)| r.get_pixel(x, y)[0] < 80)
            .count();
        assert!(dark > 200, "text hidden under the image (dark px: {dark})");
        Ok(())
    }

    #[test]
    fn optimize_black_white_mode_writes_one_bit_image() -> Result<()> {
        let engine = PdfEngine::new()?;
        let doc = engine.load_pdf_from_bytes_safe(make_scan_pdf(&engine)?)?;
        let opts = OptimizeOptions { mode: ColorMode::BlackWhite, ..Default::default() };
        let report = engine.optimize_doc(&doc, &opts)?;
        assert!(!report.kept_original);
        let ldoc = lopdf::Document::load_mem(&report.bytes)?;
        let one_bit = ldoc.objects.values().any(|o| match o {
            lopdf::Object::Stream(s) => {
                s.dict.get(b"Subtype").and_then(|v| v.as_name()).ok() == Some(b"Image".as_slice())
                    && s.dict.get(b"BitsPerComponent").and_then(|v| v.as_i64()).ok() == Some(1)
            }
            _ => false,
        });
        assert!(one_bit, "no 1-bit image in B/W output");
        eprintln!("bw: {} -> {} bytes", report.original_len, report.bytes.len());
        Ok(())
    }

    #[test]
    fn optimize_leaves_text_only_pdf_images_alone() -> Result<()> {
        let engine = PdfEngine::new()?;
        let mut doc = engine.pdfium.create_new_pdf()?;
        let font = doc.fonts_mut().helvetica();
        let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::a4())?;
        page.objects_mut().add_text_object(PdfPageTextObject::new(&doc, "Hello", font, PdfPoints::new(12.0))?)?;
        drop(page);
        let report = engine.optimize_doc(&doc, &OptimizeOptions::default())?;
        assert_eq!(report.images_seen, 0);
        assert_eq!(report.images_rewritten, 0);
        Ok(())
    }

    #[test]
    fn bw_button_converts_jpeg_color_photo() -> Result<()> {
        let engine = PdfEngine::new()?;
        let mut photo = image::RgbImage::new(600, 400);
        for (x, _y, p) in photo.enumerate_pixels_mut() {
            *p = match x / 200 {
                0 => image::Rgb([220, 30, 30]),
                1 => image::Rgb([30, 200, 40]),
                _ => image::Rgb([30, 60, 220]),
            };
        }
        let jpg = encode_jpeg(image::DynamicImage::ImageRgb8(photo), 90)?;
        let mut doc = engine.pdfium.create_new_pdf()?;
        let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::a4())?;
        let mut io = PdfPageImageObject::new_from_jpeg_reader(&doc, Cursor::new(jpg))?;
        io.scale(300.0, 200.0)?;
        io.translate(PdfPoints::new(100.0), PdfPoints::new(500.0))?;
        page.objects_mut().add_image_object(io)?;
        drop(page);
        let src = doc.save_to_bytes()?;

        let out_bytes = engine.convert_page_to_bw(&src, 0)?;
        let out = engine.load_pdf_from_bytes_safe(out_bytes)?;
        let img = only_image(&out.pages().get(0)?);
        let colored = img
            .pixels()
            .filter(|p| (p[0] as i32 - p[1] as i32).abs() > 6 || (p[1] as i32 - p[2] as i32).abs() > 6)
            .count();
        assert_eq!(colored, 0, "{colored} coloured pixels survived B/W");
        Ok(())
    }

    #[test]
    fn one_bit_repack_is_lossless_and_rejects_gray() {
        let mut s = lopdf::Stream::new(
            dictionary! {
                "Subtype" => "Image", "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8, "Width" => 10, "Height" => 2,
            },
            vec![0, 255, 255, 0, 0, 0, 255, 255, 255, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0],
        );
        repack_to_one_bit(&mut s);
        assert_eq!(s.dict.get(b"BitsPerComponent").unwrap().as_i64().unwrap(), 1);
        let data = stream_data(&s).unwrap();
        assert_eq!(data, vec![0b0110_0011, 0b1000_0000, 0b1111_1111, 0b1000_0000]);

        let mut g = lopdf::Stream::new(
            dictionary! {
                "Subtype" => "Image", "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8, "Width" => 2, "Height" => 1,
            },
            vec![0, 128],
        );
        repack_to_one_bit(&mut g);
        assert_eq!(g.dict.get(b"BitsPerComponent").unwrap().as_i64().unwrap(), 8);
    }
}

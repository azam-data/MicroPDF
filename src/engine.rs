use anyhow::{Context, Result};
use pdfium_render::prelude::*;
use std::path::{Path, PathBuf};

/// Margin zone to clear when replacing or deleting existing headers, footers, or page numbers.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MarginZone {
    /// Top 55 points of the page.
    Header,
    /// Bottom 55 points of the page (full width).
    Footer,
    /// Bottom-center 55 points of the page.
    PageNumber,
}

/// Horizontal alignment for headers and footers (and 6-position page numbers).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MarginPosition {
    HeaderLeft,
    HeaderCenter,
    HeaderRight,
    FooterLeft,
    FooterCenter,
    FooterRight,
}

impl MarginPosition {
    #[allow(dead_code)]
    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_uppercase().as_str() {
            "HL" => Some(Self::HeaderLeft),
            "HC" | "H" => Some(Self::HeaderCenter),
            "HR" => Some(Self::HeaderRight),
            "FL" => Some(Self::FooterLeft),
            "FC" | "F" => Some(Self::FooterCenter),
            "FR" => Some(Self::FooterRight),
            _ => None,
        }
    }

    pub fn is_header(self) -> bool {
        matches!(
            self,
            Self::HeaderLeft | Self::HeaderCenter | Self::HeaderRight
        )
    }

    pub fn zone(self) -> MarginZone {
        if self.is_header() {
            MarginZone::Header
        } else {
            MarginZone::Footer
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::HeaderLeft => "Top Left",
            Self::HeaderCenter => "Top Center",
            Self::HeaderRight => "Top Right",
            Self::FooterLeft => "Bottom Left",
            Self::FooterCenter => "Bottom Center",
            Self::FooterRight => "Bottom Right",
        }
    }
}

/// Text color preset for Header/Footer & Page Numbering stamps.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum HeaderFooterColor {
    Gray,
    Black,
    Navy,
}

impl HeaderFooterColor {
    pub fn label(self) -> &'static str {
        match self {
            Self::Gray => "Slate Gray",
            Self::Black => "Black",
            Self::Navy => "Navy",
        }
    }

    pub fn pdf_color(self) -> PdfColor {
        match self {
            Self::Gray => PdfColor::new(90, 90, 90, 255),
            Self::Black => PdfColor::new(20, 20, 20, 255),
            Self::Navy => PdfColor::new(30, 58, 138, 255),
        }
    }

    pub fn ui_rgb(self) -> u32 {
        match self {
            Self::Gray => 0x005A_5A5A,
            Self::Black => 0x0014_1414,
            Self::Navy => 0x001E_3A8A,
        }
    }

    pub fn to_ini_str(self) -> &'static str {
        match self {
            Self::Gray => "gray",
            Self::Black => "black",
            Self::Navy => "navy",
        }
    }

    pub fn from_ini_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "black" => Self::Black,
            "navy" => Self::Navy,
            _ => Self::Gray,
        }
    }
}

/// Reusable Header & Footer style profile persisted in portable `micropdf.ini`.
#[derive(Clone, Debug, PartialEq)]
pub struct HeaderFooterProfile {
    pub name: String,
    pub header_left: String,
    pub header_center: String,
    pub header_right: String,
    pub footer_left: String,
    pub footer_center: String,
    pub footer_right: String,
    pub font_size: f32,
    pub margin_pt: f32,
    pub color: HeaderFooterColor,
}

impl Default for HeaderFooterProfile {
    fn default() -> Self {
        Self {
            name: "Default".to_string(),
            header_left: String::new(),
            header_center: String::new(),
            header_right: String::new(),
            footer_left: String::new(),
            footer_center: String::new(),
            footer_right: String::new(),
            font_size: 10.0,
            margin_pt: 28.0,
            color: HeaderFooterColor::Gray,
        }
    }
}

impl HeaderFooterProfile {
    pub fn has_header(&self) -> bool {
        !self.header_left.trim().is_empty()
            || !self.header_center.trim().is_empty()
            || !self.header_right.trim().is_empty()
    }

    pub fn has_footer(&self) -> bool {
        !self.footer_left.trim().is_empty()
            || !self.footer_center.trim().is_empty()
            || !self.footer_right.trim().is_empty()
    }

    fn esc(s: &str) -> String {
        s.replace('\\', "\\\\")
            .replace('|', "\\p")
            .replace('\n', " ")
            .replace('\r', "")
    }

    fn unesc(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('p') => out.push('|'),
                    Some('\\') => out.push('\\'),
                    Some(other) => out.push(other),
                    None => {}
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    pub fn to_ini_str(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{:.1}|{:.1}|{}",
            Self::esc(&self.name),
            Self::esc(&self.header_left),
            Self::esc(&self.header_center),
            Self::esc(&self.header_right),
            Self::esc(&self.footer_left),
            Self::esc(&self.footer_center),
            Self::esc(&self.footer_right),
            self.font_size.clamp(6.0, 24.0),
            self.margin_pt.clamp(12.0, 72.0),
            self.color.to_ini_str(),
        )
    }

    pub fn from_ini_str(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split('|').collect();
        if parts.len() < 10 {
            return None;
        }
        let name = Self::unesc(parts[0]).trim().to_string();
        if name.is_empty() {
            return None;
        }
        Some(Self {
            name,
            header_left: Self::unesc(parts[1]),
            header_center: Self::unesc(parts[2]),
            header_right: Self::unesc(parts[3]),
            footer_left: Self::unesc(parts[4]),
            footer_center: Self::unesc(parts[5]),
            footer_right: Self::unesc(parts[6]),
            font_size: parts[7].trim().parse::<f32>().unwrap_or(10.0).clamp(6.0, 24.0),
            margin_pt: parts[8].trim().parse::<f32>().unwrap_or(28.0).clamp(12.0, 72.0),
            color: HeaderFooterColor::from_ini_str(parts[9]),
        })
    }
}

/// Formatting template for Page Numbering stamps.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PageNumberFormat {
    Plain,
    PageN,
    PageNOfTotal,
    NSlashTotal,
    Dashed,
}

impl PageNumberFormat {
    pub const ALL: [Self; 5] = [
        Self::Plain,
        Self::PageN,
        Self::PageNOfTotal,
        Self::NSlashTotal,
        Self::Dashed,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Plain => "1",
            Self::PageN => "Page 1",
            Self::PageNOfTotal => "Page 1 of N",
            Self::NSlashTotal => "1 / N",
            Self::Dashed => "- 1 -",
        }
    }
}

/// Configuration for the visual Page Numbering modal.
#[derive(Clone, Debug, PartialEq)]
pub struct PageNumberConfig {
    pub position: MarginPosition,
    pub format: PageNumberFormat,
    pub roman_end: u16,
    /// Number of leading cover/picture pages to leave completely unnumbered (0 = none).
    pub skip_pages: u16,
    pub font_size: f32,
    pub margin_pt: f32,
    pub color: HeaderFooterColor,
}

impl Default for PageNumberConfig {
    fn default() -> Self {
        Self {
            position: MarginPosition::FooterCenter,
            format: PageNumberFormat::Plain,
            roman_end: 0,
            skip_pages: 0,
            font_size: 10.0,
            margin_pt: 28.0,
            color: HeaderFooterColor::Gray,
        }
    }
}

/// In-memory Bookmark item for the Left Bookmark Sidebar (`B` key) and PDF `/Outlines`.
#[derive(Clone, Debug, PartialEq)]
pub struct Bookmark {
    pub serial: String,
    pub title: String,
    pub logical_page: String,
    pub page_index: usize,
    pub level: u8,
}

/// A single text search match on a specific 0-based page, holding one or more line-level bounding boxes.
#[derive(Clone, Debug)]
pub struct SearchMatch {
    pub page_index: u16,
    pub rects: Vec<PdfRect>,
}

/// Decoded RGBA image buffer for stamping signatures or images onto a PDF page (`P` key).
#[derive(Clone, Debug)]
pub struct StampImageBuffer {
    pub width: u32,
    pub height: u32,
    pub image: image::DynamicImage,
}

/// Color swatch for PDF annotations (Highlight, Underline, Strikethrough, Sticky Note, Ink, Text, Shapes).
/// A Sticky Note or Text Box annotation on a page that can be clicked to inspect or dragged to reposition.
#[derive(Clone, Debug)]
pub struct MovableAnnotation {
    pub index: usize,
    pub is_sticky_note: bool,
    pub bounds: PdfRect,
    pub text: String,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AnnotationColor {
    Yellow,
    Green,
    Cyan,
    Pink,
    Red,
    Blue,
    Black,
}

impl Default for AnnotationColor {
    fn default() -> Self {
        Self::Yellow
    }
}

impl AnnotationColor {
    pub const ALL: [AnnotationColor; 7] = [
        Self::Yellow,
        Self::Green,
        Self::Cyan,
        Self::Pink,
        Self::Red,
        Self::Blue,
        Self::Black,
    ];

    pub fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|&c| c == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Yellow => "Yellow",
            Self::Green => "Green",
            Self::Cyan => "Cyan",
            Self::Pink => "Pink",
            Self::Red => "Red",
            Self::Blue => "Blue",
            Self::Black => "Black",
        }
    }

    pub fn rgb(self) -> (u8, u8, u8) {
        match self {
            Self::Yellow => (255, 224, 51),
            Self::Green => (52, 211, 153),
            Self::Cyan => (56, 189, 248),
            Self::Pink => (244, 114, 182),
            Self::Red => (220, 38, 38),
            Self::Blue => (37, 99, 235),
            Self::Black => (24, 24, 27),
        }
    }

    /// Solid UI swatch color (`0x00RRGGBB`).
    pub fn u32_rgb(self) -> u32 {
        let (r, g, b) = self.rgb();
        ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
    }

    /// Stroke / ink / text color (slightly deeper for Yellow so lines/text stay legible on white paper).
    pub fn stroke_pdf_color(self) -> PdfColor {
        match self {
            Self::Yellow => PdfColor::new(202, 138, 4, 255),
            _ => {
                let (r, g, b) = self.rgb();
                PdfColor::new(r, g, b, 255)
            }
        }
    }

    /// Translucent Multiply-blend fill for text highlights.
    pub fn highlight_pdf_color(self) -> PdfColor {
        let (r, g, b) = self.rgb();
        PdfColor::new(r, g, b, 115)
    }
}

/// Information about a clickable/right-clickable markup annotation found on a page.
#[derive(Clone, Debug)]
pub struct FoundAnnotationInfo {
    pub index: usize,
    pub label: &'static str,
    pub is_text_editable: bool,
    pub contents: String,
}

/// Interactive AcroForm field kind supported by Tier 2 native form filling.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FormFieldKind {
    Text,
    Checkbox,
    Radio,
}

/// Fillable PDF form field found under a page coordinate.
#[derive(Clone, Debug)]
pub struct FoundFormFieldInfo {
    pub annot_index: usize,
    pub kind: FormFieldKind,
    pub name: String,
    pub value: String,
    pub bounds: PdfRect,
}

/// Page sizing mode when converting PNG/JPEG images into a new PDF.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ImagePdfFitMode {
    FitA4,
    FitLetter,
    MatchImageSize,
}

impl ImagePdfFitMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::FitA4 => "Fit A4 (Auto Orient)",
            Self::FitLetter => "Fit Letter (Auto Orient)",
            Self::MatchImageSize => "Match Image Size",
        }
    }
}

/// Image output format for `Export -> Export Pages as Images`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ImageExportFormat {
    Png,
    Jpeg,
}

impl ImageExportFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG (Lossless)",
            Self::Jpeg => "JPEG (Compact)",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }
}

/// Extracted metadata & properties shown in `File -> Document Properties...`.
#[derive(Clone, Debug, Default)]
pub struct DocumentPropertiesInfo {
    pub file_name: String,
    pub file_size_bytes: u64,
    pub pdf_version: String,
    pub page_count: u16,
    pub current_page_size_pt: (f32, f32),
    pub is_encrypted: bool,
    pub has_xmp_metadata: bool,
    pub has_info_dict: bool,
    pub fonts: Vec<String>,
    pub title: String,
    pub author: String,
    pub subject: String,
    pub keywords: String,
    pub creator: String,
    pub producer: String,
    pub creation_date: String,
    pub mod_date: String,
}

/// Result of `flatten_and_unlock_doc`.
pub struct FlattenReport {
    pub bytes: Vec<u8>,
    /// 1-based page numbers whose annotations/forms could not be flattened.
    pub failed_pages: Vec<u16>,
    /// False when the encryption/permission dictionaries could not be stripped.
    pub permissions_removed: bool,
}

/// Error returned when a PDF file is encrypted and requires a user/owner password.
#[derive(Debug)]
pub struct PasswordRequiredError;

impl std::fmt::Display for PasswordRequiredError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "This PDF requires a password to open")
    }
}

impl std::error::Error for PasswordRequiredError {}

pub struct PdfEngine {
    pub pdfium: Pdfium,
}

/// The open PDF file that PDFium reads blocks from on demand (see `load_pdf_with_password`).
///
/// PDFium keeps the byte offsets it parsed at open time and asks for blocks at those offsets for
/// the lifetime of the document. If another program rewrites the file in place (truncate and
/// write, as `cp` does), those offsets point into a different file: a same-size replacement
/// would make old pages silently show the new file's content. So every block read is checked
/// against the length and modification time recorded at open, and refused once either changes.
/// pdfium-render turns the error into a failed read, PDFium fails that page load, the viewer
/// reports it in the status bar, and auto-reload then opens the new file.
///
/// Each block is read completely or not at all: pdfium-render calls `read` once and would treat
/// a short read (a truncated file) as success with a partly filled buffer.
///
/// A file replaced by rename (an atomic save) or deleted is unaffected: the handle still refers
/// to the original file, whose bytes stay readable and unchanged until the handle is closed.
pub(crate) struct ChangeGuardedFile {
    file: std::fs::File,
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(test)]
    path: PathBuf,
}

impl ChangeGuardedFile {
    pub(crate) fn new(file: std::fs::File, _path: &Path) -> std::io::Result<Self> {
        let meta = file.metadata()?;
        #[cfg(test)]
        let path = open_file_registry::opened(_path);
        Ok(Self {
            len: meta.len(),
            modified: meta.modified().ok(),
            file,
            #[cfg(test)]
            path,
        })
    }

    fn check_unchanged(&self) -> std::io::Result<()> {
        let meta = self.file.metadata()?;
        if meta.len() != self.len || meta.modified().ok() != self.modified {
            return Err(std::io::Error::other(
                "the PDF file was changed on disk after it was opened",
            ));
        }
        Ok(())
    }
}

impl std::io::Read for ChangeGuardedFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.file.read_exact(buf)?;
        // Checked after the read: if nothing changed by now, the bytes just read are the
        // original ones. A change during the read moves the length or mtime and is caught here.
        self.check_unchanged()?;
        Ok(buf.len())
    }
}

#[cfg(test)]
impl Drop for ChangeGuardedFile {
    fn drop(&mut self) {
        open_file_registry::closed(&self.path);
    }
}

/// Test-only record of which files this process holds open through `ChangeGuardedFile`, so
/// tests can simulate file systems that refuse to replace a file while it is open (FAT32,
/// exFAT, some network shares, Windows without POSIX rename semantics).
#[cfg(test)]
pub(crate) mod open_file_registry {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    static OPEN: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    fn key(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    /// Records `path` as open; returns the key to pass to `closed`.
    pub(crate) fn opened(path: &Path) -> PathBuf {
        let k = key(path);
        OPEN.lock().unwrap_or_else(|e| e.into_inner()).push(k.clone());
        k
    }

    /// `opened_key` as returned by `opened` (the file may have been renamed or deleted since).
    pub(crate) fn closed(opened_key: &Path) {
        let mut open = OPEN.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = open.iter().position(|p| p == opened_key) {
            open.swap_remove(i);
        }
    }

    pub(crate) fn is_open(path: &Path) -> bool {
        let k = key(path);
        OPEN.lock().unwrap_or_else(|e| e.into_inner()).contains(&k)
    }
}

impl std::io::Seek for ChangeGuardedFile {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.file.seek(pos)
    }
}

#[allow(dead_code)]
impl PdfEngine {
    /// Initializes PDFium from the executable's own directory only.
    ///
    /// v1.1: the current working directory, `./lib` and system library paths are no longer
    /// searched. A PDF viewer is often launched with the working directory set to a folder of
    /// untrusted downloads, and loading `pdfium.dll` from there would be a DLL-planting risk.
    /// Developers can point at a different folder with the `MICROPDF_PDFIUM_DIR` env var.
    pub fn new() -> Result<Self> {
        let bindings = Self::bind_pdfium()?;
        Ok(Self {
            pdfium: Pdfium::new(bindings),
        })
    }

    fn bind_pdfium() -> Result<Box<dyn PdfiumLibraryBindings>> {
        let mut tried: Vec<PathBuf> = Vec::new();

        if let Some(dir) = std::env::var_os("MICROPDF_PDFIUM_DIR") {
            let lib = Pdfium::pdfium_platform_library_name_at_path(&PathBuf::from(dir));
            match Pdfium::bind_to_library(&lib) {
                Ok(b) => return Ok(b),
                Err(_) => tried.push(lib),
            }
        }

        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                #[allow(unused_mut)]
                let mut candidates = vec![exe_dir.to_path_buf()];
                // macOS .app bundle layout: Contents/MacOS/micro_pdf + Contents/Frameworks/
                #[cfg(target_os = "macos")]
                candidates.push(exe_dir.join("../Frameworks"));
                for dir in candidates {
                    let lib = Pdfium::pdfium_platform_library_name_at_path(&dir);
                    match Pdfium::bind_to_library(&lib) {
                        Ok(b) => return Ok(b),
                        Err(_) => tried.push(lib),
                    }
                }
            }
        }

        anyhow::bail!(
            "PDFium library not found. Place it next to the MicroPDF executable. Looked in: {:?}",
            tried
        )
    }

    /// Returns true if `err` (or any cause in its chain) is a `PasswordRequiredError`.
    pub fn is_password_error(err: &anyhow::Error) -> bool {
        err.downcast_ref::<PasswordRequiredError>().is_some()
    }

    /// Unified, safe PDF loader that never passes file paths to PDFium's C loader.
    /// 1. Cleans/canonicalizes `path` (stripping any trailing newlines or surrounding quotes).
    /// 2. Normal case: keeps the file open and lets PDFium read blocks on demand
    ///    (`FPDF_LoadCustomDocument` via `load_pdf_from_reader`) through a `ChangeGuardedFile`,
    ///    so the file is never held in RAM.
    /// 3. Leading junk/BOM before `%PDF-`, or a password: reads the whole file, strips the junk
    ///    in place and hands that single buffer to PDFium.
    pub fn load_pdf_safe<'a>(&'a self, path: &Path) -> Result<PdfDocument<'a>> {
        self.load_pdf_with_password(path, None)
    }

    /// Loads `path` with an optional password (`None` tries the empty password first).
    /// Never stores `password`.
    pub fn load_pdf_with_password<'a>(
        &'a self,
        path: &Path,
        password: Option<&str>,
    ) -> Result<PdfDocument<'a>> {
        use std::io::{Read, Seek, SeekFrom};

        let cleaned_path = if let Some(raw_str) = path.to_str() {
            let trimmed = raw_str
                .trim_matches(|c: char| c == '\n' || c == '\r' || c == '"' || c == '\'');
            PathBuf::from(trimmed)
        } else {
            path.to_path_buf()
        };

        let resolved_path = std::fs::canonicalize(&cleaned_path).unwrap_or(cleaned_path);

        let mut file = std::fs::File::open(&resolved_path)
            .with_context(|| format!("FS Error reading {:?}", resolved_path))?;
        let file_len = file
            .metadata()
            .with_context(|| format!("FS Error reading metadata {:?}", resolved_path))?
            .len();
        if file_len == 0 {
            anyhow::bail!("FS Error: file {:?} is empty (0 bytes)", resolved_path);
        }

        let mut head = [0u8; 1024];
        let n = file
            .read(&mut head)
            .with_context(|| format!("FS Error reading {:?}", resolved_path))?;
        let junk_offset = head[..n]
            .windows(5)
            .position(|w| w == b"%PDF-")
            .unwrap_or(0);

        if junk_offset == 0 && password.is_none() {
            file.seek(SeekFrom::Start(0))
                .with_context(|| format!("FS Error seeking {:?}", resolved_path))?;
            let reader = ChangeGuardedFile::new(file, &resolved_path)
                .with_context(|| format!("FS Error reading metadata {:?}", resolved_path))?;
            match self.pdfium.load_pdf_from_reader(reader, None) {
                Ok(doc) => return Ok(doc),
                Err(PdfiumError::PdfiumLibraryInternalError(
                    PdfiumInternalError::PasswordError,
                )) => {
                    return Err(anyhow::Error::new(PasswordRequiredError))
                        .with_context(|| format!("Could not open {:?}", resolved_path));
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("PDFium could not parse the document: {e:?}"))
                        .with_context(|| format!("Could not open {:?}", resolved_path));
                }
            }
        }

        let mut bytes = std::fs::read(&resolved_path)
            .with_context(|| format!("FS Error reading {:?}", resolved_path))?;
        Self::strip_leading_junk(&mut bytes);
        self.load_pdf_from_bytes_with_password(bytes, password)
            .with_context(|| format!("Could not open {:?}", resolved_path))
    }

    /// Alias for `load_pdf_safe` so all callers use the safe in-memory byte vector loader.
    pub fn load_pdf_file<'a>(&'a self, path: &Path) -> Result<PdfDocument<'a>> {
        self.load_pdf_safe(path)
    }

    /// Removes bytes before `%PDF-` when the header appears within the first 1 KB (BOMs, mail junk).
    fn strip_leading_junk(bytes: &mut Vec<u8>) {
        let window = &bytes[..bytes.len().min(1024)];
        if let Some(pos) = window.windows(5).position(|w| w == b"%PDF-") {
            if pos > 0 {
                bytes.drain(..pos);
            }
        }
    }

    /// Loads a `PdfDocument` from an in-memory buffer without copying it.
    /// PDFium already tries the empty user password, so no second attempt is needed.
    /// Password-protected files return `PasswordRequiredError`.
    pub fn load_pdf_from_bytes_safe<'a>(&'a self, bytes: Vec<u8>) -> Result<PdfDocument<'a>> {
        self.load_pdf_from_bytes_with_password(bytes, None)
    }

    /// Loads a `PdfDocument` from an in-memory buffer with an optional password.
    pub fn load_pdf_from_bytes_with_password<'a>(
        &'a self,
        bytes: Vec<u8>,
        password: Option<&str>,
    ) -> Result<PdfDocument<'a>> {
        match self.pdfium.load_pdf_from_byte_vec(bytes, password) {
            Ok(doc) => Ok(doc),
            Err(PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError)) => {
                Err(anyhow::Error::new(PasswordRequiredError))
            }
            Err(e) => Err(anyhow::anyhow!("PDFium could not parse the document: {e:?}")),
        }
    }

    /// Whether `page` has form-field widgets. PDFium draws those only through `FPDF_FFLDraw`
    /// (which the full-page render path uses); matrix renders such as zoom tiles and print
    /// strips skip them, so those render from `flattened_page_copy` instead.
    pub fn page_has_form_widgets(page: &PdfPage) -> bool {
        page.annotations()
            .iter()
            .any(|a| a.annotation_type() == PdfPageAnnotationType::Widget)
    }

    /// A new one-page document holding a copy of page `page_idx` of `doc` with its form fields
    /// and annotations flattened into the page content, so matrix renders show them as they
    /// appear on screen. `doc` itself is never modified.
    pub fn flattened_page_copy<'e>(&'e self, doc: &PdfDocument, page_idx: u16) -> Result<PdfDocument<'e>> {
        self.flattened_page_copy_with_opts(doc, page_idx, true)
    }

    /// Like `flattened_page_copy`, but when `include_annotations` is `false`, strips all non-Widget
    /// annotations from the temporary copy before flattening so filled form widgets remain visible
    /// while markup annotations (highlights, notes, shapes) are omitted.
    pub fn flattened_page_copy_with_opts<'e>(
        &'e self,
        doc: &PdfDocument,
        page_idx: u16,
        include_annotations: bool,
    ) -> Result<PdfDocument<'e>> {
        let mut copy = self
            .pdfium
            .create_new_pdf()
            .context("Failed to create a copy of a form page")?;
        copy.pages_mut()
            .copy_page_from_document(doc, page_idx, 0)
            .with_context(|| format!("Failed to copy page {}", page_idx + 1))?;
        let mut page = copy.pages().get(0).context("Page copy is empty")?;
        if !include_annotations {
            let count = page.annotations().len();
            for i in (0..count).rev() {
                if let Ok(annot) = page.annotations().get(i) {
                    if annot.annotation_type() != PdfPageAnnotationType::Widget {
                        let _ = page.annotations_mut().delete_annotation(annot);
                    }
                }
            }
        }
        page.flatten()
            .with_context(|| format!("Failed to flatten form fields of page {}", page_idx + 1))?;
        drop(page);
        Ok(copy)
    }

    /// The temporary file a save of `path` is written to before it replaces `path`:
    /// `<stem>.micropdf-save.pdf` in the same folder (same drive, so the rename is atomic).
    /// Visible and ending in `.pdf`, because when the final rename fails it is left in place as
    /// the user's saved copy.
    pub fn save_temp_path(path: &Path) -> PathBuf {
        let stem = path
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "output".to_string());
        path.with_file_name(format!("{stem}.micropdf-save.pdf"))
    }

    /// Writes `bytes` to `save_temp_path(path)` and flushes it to disk. On failure the partial
    /// temp file is removed. Returns the temp path.
    pub fn write_temp_next_to(path: &Path, bytes: &[u8]) -> Result<PathBuf> {
        use std::io::Write;
        let tmp = Self::save_temp_path(path);
        let mut f = std::fs::File::create(&tmp)
            .with_context(|| format!("Failed to create temp file {:?}", tmp))?;
        if let Err(e) = f.write_all(bytes).and_then(|_| f.sync_all()) {
            drop(f);
            let _ = std::fs::remove_file(&tmp);
            return Err(e).with_context(|| format!("Failed to write temp file {:?}", tmp));
        }
        Ok(tmp)
    }

    /// Writes `bytes` to `path` atomically: write a sibling temp file, flush it to disk, then
    /// rename over the target. A crash or full disk mid-save can never leave a truncated file:
    /// if the rename fails, the temp file is removed and `path` is left exactly as it was.
    /// (Saving over a document the viewer has open goes through `save_active_doc_to`, which
    /// closes the open handle and retries before giving up.)
    pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
        let tmp = Self::write_temp_next_to(path, bytes)?;
        if let Err(e) = std::fs::rename(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e).with_context(|| format!("Failed to replace {:?}", path));
        }
        Ok(())
    }

    /// Parses a valid Roman numeral (`i`, `ii`, `iii`, `iv`, `v`... case-insensitive) into a 1-based `usize`.
    pub fn from_roman(input: &str) -> Option<usize> {
        let s = input.trim().to_ascii_lowercase();
        if s.is_empty()
            || !s
                .chars()
                .all(|c| matches!(c, 'i' | 'v' | 'x' | 'l' | 'c' | 'd' | 'm'))
        {
            return None;
        }
        let val_of = |c: char| -> i32 {
            match c {
                'i' => 1,
                'v' => 5,
                'x' => 10,
                'l' => 50,
                'c' => 100,
                'd' => 500,
                'm' => 1000,
                _ => 0,
            }
        };
        let chars: Vec<char> = s.chars().collect();
        let mut total = 0i32;
        for idx in 0..chars.len() {
            let v = val_of(chars[idx]);
            if idx + 1 < chars.len() && v < val_of(chars[idx + 1]) {
                total -= v;
            } else {
                total += v;
            }
        }
        if total >= 1 && total <= 3999 && Self::to_roman(total as u16) == s {
            Some(total as usize)
        } else {
            None
        }
    }

    /// Resolves a logical page string (`input`) into a 0-based physical `page_index`:
    /// - If `input` starts with `#` (e.g. `#5`), parses as 1-based physical page `5 -> index 4`.
    /// - If `input` is a valid Roman numeral (`i`, `ii`, `iii`, `iv`, `v`...), converts to `r` (`1..=total_pages`) -> `Some(r - 1)`.
    /// - If `input` is an Arabic integer `n` (`1, 2, 3...`), maps to logical Arabic page `n`, which is
    ///   1-based physical page `roman_end + n` (0-based index `roman_end + n - 1`), validated against `total_pages`.
    pub fn resolve_logical_page_str(
        input: &str,
        roman_end: usize,
        total_pages: usize,
    ) -> Option<usize> {
        Self::resolve_logical_page_str_with_skip(input, 0, roman_end, total_pages)
    }

    /// Resolves a logical page string (`input`) taking into account `skip_pages` unnumbered
    /// leading cover/picture pages followed by `roman_end` lowercase Roman front-matter pages.
    pub fn resolve_logical_page_str_with_skip(
        input: &str,
        skip_pages: usize,
        roman_end: usize,
        total_pages: usize,
    ) -> Option<usize> {
        let trimmed = input.trim();
        if trimmed.is_empty() || total_pages == 0 {
            return None;
        }

        if let Some(phys_str) = trimmed.strip_prefix('#') {
            let p = phys_str.trim().parse::<usize>().ok()?;
            if p >= 1 && p <= total_pages {
                return Some(p - 1);
            }
            return None;
        }

        if let Some(r) = Self::from_roman(trimmed) {
            if roman_end > 0 && r > roman_end {
                return None;
            }
            let phys_1based = skip_pages + r;
            if phys_1based >= 1 && phys_1based <= total_pages {
                return Some(phys_1based - 1);
            }
            return None;
        }

        if let Ok(n) = trimmed.parse::<usize>() {
            if n >= 1 {
                let phys_1based = skip_pages + roman_end + n;
                if phys_1based >= 1 && phys_1based <= total_pages {
                    return Some(phys_1based - 1);
                }
            }
        }

        None
    }

    /// Formats the logical label for 0-based `page_idx` given `roman_end`.
    pub fn format_logical_page_label(page_idx: usize, roman_end: usize) -> String {
        Self::format_logical_page_label_with_skip(page_idx, 0, roman_end)
    }

    /// Formats the logical label for 0-based `page_idx` given `skip_pages` unnumbered front pages
    /// followed by `roman_end` Roman pages. Skipped front pages return `"—"`.
    pub fn format_logical_page_label_with_skip(
        page_idx: usize,
        skip_pages: usize,
        roman_end: usize,
    ) -> String {
        let phys_1based = page_idx + 1;
        if skip_pages > 0 && phys_1based <= skip_pages {
            return "—".to_string();
        }
        let eff = phys_1based.saturating_sub(skip_pages);
        if roman_end > 0 && eff <= roman_end {
            Self::to_roman(eff as u16)
        } else {
            format!("{}", eff.saturating_sub(roman_end))
        }
    }

    /// Reads `/PageLabels` from `pdf_bytes` via `lopdf` and returns `(skip_pages, roman_end)`.
    pub fn read_page_labels_skip_and_roman(pdf_bytes: &[u8]) -> (usize, usize) {
        let Ok(ldoc) = lopdf::Document::load_mem(pdf_bytes) else {
            return (0, 0);
        };
        let Ok(catalog) = ldoc.catalog() else {
            return (0, 0);
        };
        let Ok(pl_obj) = catalog.get(b"PageLabels") else {
            return (0, 0);
        };
        let pl_dict = match pl_obj {
            lopdf::Object::Reference(id) => ldoc.get_object(*id).and_then(|o| o.as_dict()).ok(),
            lopdf::Object::Dictionary(d) => Some(d),
            _ => None,
        };
        let Some(pl_dict) = pl_dict else {
            return (0, 0);
        };
        let Ok(nums_obj) = pl_dict.get(b"Nums") else {
            return (0, 0);
        };
        let nums_arr = match nums_obj {
            lopdf::Object::Reference(id) => ldoc.get_object(*id).and_then(|o| o.as_array()).ok(),
            lopdf::Object::Array(a) => Some(a),
            _ => None,
        };
        let Some(nums) = nums_arr else {
            return (0, 0);
        };

        let style_of = |d: &lopdf::Dictionary| -> Option<String> {
            d.get(b"S")
                .ok()
                .and_then(|s| s.as_name_str().ok())
                .map(|s| s.to_string())
        };

        let mut entries: Vec<(usize, Option<String>)> = Vec::new();
        let mut i = 0usize;
        while i + 1 < nums.len() {
            if let Ok(idx) = nums[i].as_i64() {
                if idx >= 0 {
                    let dict_opt = match &nums[i + 1] {
                        lopdf::Object::Reference(id) => {
                            ldoc.get_object(*id).and_then(|o| o.as_dict()).ok()
                        }
                        lopdf::Object::Dictionary(d) => Some(d),
                        _ => None,
                    };
                    let st = dict_opt.and_then(style_of);
                    entries.push((idx as usize, st));
                }
            }
            i += 2;
        }
        if entries.is_empty() {
            return (0, 0);
        }

        let mut skip_pages = 0usize;
        let mut roman_end = 0usize;
        let mut cursor = 0usize;
        if entries[0].0 == 0 && entries[0].1.is_none() && entries.len() >= 2 {
            // Leading unnumbered cover/picture pages: `0 << >>`
            skip_pages = entries[1].0;
            cursor = 1;
        }
        if cursor < entries.len()
            && entries[cursor]
                .1
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("r"))
        {
            if cursor + 1 < entries.len() {
                roman_end = entries[cursor + 1].0.saturating_sub(entries[cursor].0);
            }
        }
        (skip_pages, roman_end)
    }

    /// Reads `/PageLabels` from `pdf_bytes` via `lopdf` and returns `roman_end` (`0` if all Arabic).
    pub fn read_page_labels_roman_end(pdf_bytes: &[u8]) -> usize {
        Self::read_page_labels_skip_and_roman(pdf_bytes).1
    }

    /// Detects `(skip_pages, roman_end)` through PDFium from the already-open document without
    /// re-parsing the file with `lopdf`.
    pub fn read_page_labels_skip_and_roman_from_doc(doc: &PdfDocument) -> (usize, usize) {
        let total = doc.pages().len();
        if total == 0 {
            return (0, 0);
        }
        let mut skip: u16 = 0;
        while skip < total {
            let Ok(page) = doc.pages().get(skip) else { break };
            let lbl = page.label().unwrap_or("").trim();
            if !lbl.is_empty() {
                break;
            }
            skip += 1;
        }
        if skip == total {
            return (0, 0);
        }
        let mut roman: u16 = 0;
        while skip + roman < total {
            let Ok(page) = doc.pages().get(skip + roman) else { break };
            let expected = Self::to_roman(roman + 1);
            if page.label().map(str::trim) != Some(expected.as_str()) {
                break;
            }
            roman += 1;
        }
        if skip + roman == total && skip == 0 {
            return (0, 0);
        }
        (skip as usize, roman as usize)
    }

    /// Number of leading pages labelled i, ii, iii, ... read through PDFium from the already
    /// open document.
    pub fn read_page_labels_roman_end_from_doc(doc: &PdfDocument) -> usize {
        Self::read_page_labels_skip_and_roman_from_doc(doc).1
    }

    /// Parses hierarchical bookmarks from a `--` delimited `.txt` string (`Serial -- Title -- LogicalPage`),
    /// as well as tab-delimited sidecar lines.
    pub fn parse_bookmarks_txt(
        content: &str,
        roman_end: usize,
        total_pages: usize,
    ) -> Vec<Bookmark> {
        Self::parse_bookmarks_txt_with_skip(content, 0, roman_end, total_pages)
    }

    /// Parses hierarchical bookmarks taking `skip_pages` + `roman_end` into account.
    pub fn parse_bookmarks_txt_with_skip(
        content: &str,
        skip_pages: usize,
        roman_end: usize,
        total_pages: usize,
    ) -> Vec<Bookmark> {
        let mut list = Vec::new();
        for raw_line in content.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with("//") {
                continue;
            }

            if line.contains("--") {
                let parts: Vec<&str> = line.split("--").map(|s| s.trim()).collect();
                if parts.len() >= 3 {
                    let serial = parts[0].to_string();
                    let page_str = parts[parts.len() - 1].to_string();
                    let title = parts[1..parts.len() - 1].join(" -- ");
                    let first_char = serial.chars().next().unwrap_or('1');
                    let level = if first_char.is_ascii_alphabetic() || serial.contains('.') {
                        1
                    } else {
                        0
                    };
                    let effective_total = total_pages.max(9999);
                    let page_index = Self::resolve_logical_page_str_with_skip(
                        &page_str,
                        skip_pages,
                        roman_end,
                        total_pages,
                    )
                    .or_else(|| {
                        Self::resolve_logical_page_str_with_skip(
                            &page_str,
                            skip_pages,
                            roman_end,
                            effective_total,
                        )
                        .map(|idx| {
                            if total_pages > 0 {
                                idx.min(total_pages - 1)
                            } else {
                                idx
                            }
                        })
                    })
                    .unwrap_or(0);

                    list.push(Bookmark {
                        serial,
                        title,
                        logical_page: page_str,
                        page_index,
                        level,
                    });
                    continue;
                }
            }

            // Fallback for legacy tab-delimited sidecar format (`page_index\ttitle`)
            if let Some((idx_str, title)) = line.split_once('\t') {
                if let Ok(page_idx) = idx_str.trim().parse::<usize>() {
                    if total_pages == 0 || page_idx < total_pages {
                        let logical_page = Self::format_logical_page_label_with_skip(
                            page_idx,
                            skip_pages,
                            roman_end,
                        );
                        list.push(Bookmark {
                            serial: String::new(),
                            title: title.trim().to_string(),
                            logical_page,
                            page_index: page_idx,
                            level: 0,
                        });
                    }
                }
            }
        }
        list
    }

    /// Re-evaluates `page_index` for all bookmarks when `roman_end` changes in `N` Mode.
    pub fn resync_bookmarks_logical_pages(
        bookmarks: &mut [Bookmark],
        roman_end: usize,
        total_pages: usize,
    ) {
        Self::resync_bookmarks_logical_pages_with_skip(bookmarks, 0, roman_end, total_pages);
    }

    /// Re-evaluates `page_index` for all bookmarks when `skip_pages` or `roman_end` changes.
    pub fn resync_bookmarks_logical_pages_with_skip(
        bookmarks: &mut [Bookmark],
        skip_pages: usize,
        roman_end: usize,
        total_pages: usize,
    ) {
        if total_pages == 0 {
            return;
        }
        for bm in bookmarks.iter_mut() {
            if let Some(new_idx) = Self::resolve_logical_page_str_with_skip(
                &bm.logical_page,
                skip_pages,
                roman_end,
                total_pages,
            ) {
                bm.page_index = new_idx;
            } else {
                bm.page_index = bm.page_index.min(total_pages - 1);
            }
        }
    }

    /// Shifts bookmark `page_index` values after inserting `count` pages at 0-based `insert_idx`.
    pub fn remap_bookmarks_after_insert(
        bookmarks: &mut [Bookmark],
        insert_idx: usize,
        count: usize,
        skip_pages: usize,
        roman_end: usize,
    ) {
        if count == 0 {
            return;
        }
        for bm in bookmarks.iter_mut() {
            if bm.page_index >= insert_idx {
                bm.page_index = bm.page_index.saturating_add(count);
                bm.logical_page =
                    Self::format_logical_page_label_with_skip(bm.page_index, skip_pages, roman_end);
            }
        }
    }

    /// Shifts bookmark `page_index` values after deleting 0-based `deleted_idx`.
    pub fn remap_bookmarks_after_delete(
        bookmarks: &mut [Bookmark],
        deleted_idx: usize,
        new_total_pages: usize,
        skip_pages: usize,
        roman_end: usize,
    ) {
        for bm in bookmarks.iter_mut() {
            if bm.page_index > deleted_idx {
                bm.page_index = bm.page_index.saturating_sub(1);
            } else if new_total_pages > 0 && bm.page_index >= new_total_pages {
                bm.page_index = new_total_pages - 1;
            }
            bm.logical_page =
                Self::format_logical_page_label_with_skip(bm.page_index, skip_pages, roman_end);
        }
    }

    /// Shifts bookmark `page_index` values after moving page `src_idx` to `dst_idx`.
    pub fn remap_bookmarks_after_move(
        bookmarks: &mut [Bookmark],
        src_idx: usize,
        dst_idx: usize,
        skip_pages: usize,
        roman_end: usize,
    ) {
        if src_idx == dst_idx {
            return;
        }
        for bm in bookmarks.iter_mut() {
            if bm.page_index == src_idx {
                bm.page_index = dst_idx;
            } else if src_idx < dst_idx && bm.page_index > src_idx && bm.page_index <= dst_idx {
                bm.page_index -= 1;
            } else if dst_idx < src_idx && bm.page_index >= dst_idx && bm.page_index < src_idx {
                bm.page_index += 1;
            }
            bm.logical_page =
                Self::format_logical_page_label_with_skip(bm.page_index, skip_pages, roman_end);
        }
    }

    /// Embeds `/PageLabels` and hierarchical `/Outlines` into `pdf_bytes` using `lopdf`.
    /// Encodes a PDF text string: plain literal for ASCII, UTF-16BE with BOM otherwise, so
    /// Bengali and other non-Latin bookmark titles display correctly in every reader.
    fn pdf_text_string(text: String) -> lopdf::Object {
        if text.is_ascii() {
            lopdf::Object::string_literal(text)
        } else {
            let mut bytes = vec![0xFE, 0xFF];
            for unit in text.encode_utf16() {
                bytes.extend_from_slice(&unit.to_be_bytes());
            }
            lopdf::Object::String(bytes, lopdf::StringFormat::Hexadecimal)
        }
    }

    /// Embeds `/PageLabels` and `/Outlines`, strips XMP and identifying `/Info` keys.
    ///
    /// v1.1: returns an error instead of silently handing back the unsanitized input.
    /// With `deep_clean` (used after redaction) it also removes the whole `/Info` dictionary,
    /// embedded page thumbnails (`/Thumb`) and every object no longer reachable from the
    /// trailer, so orphaned pre-redaction content streams cannot survive in the saved file.
    pub fn embed_metadata_in_pdf_bytes(
        pdf_bytes: &[u8],
        roman_end: usize,
        bookmarks: &[Bookmark],
        deep_clean: bool,
    ) -> Result<Vec<u8>> {
        Self::embed_metadata_in_pdf_bytes_with_skip(
            pdf_bytes,
            0,
            roman_end,
            roman_end > 0,
            bookmarks,
            deep_clean,
        )
    }

    /// Embeds `/PageLabels` (with `skip_pages` + `roman_end`, preserving untouched custom labels when
    /// `skip_pages == 0 && roman_end == 0 && !update_page_labels`), updates or removes `/Outlines`,
    /// strips XMP/identifying `/Info` keys, and during `deep_clean` strips `/Info`, `/Thumb`,
    /// `/PieceInfo`, `/StructTreeRoot`, and orphaned redacted `/AcroForm /Fields`.
    pub fn embed_metadata_in_pdf_bytes_with_skip(
        pdf_bytes: &[u8],
        skip_pages: usize,
        roman_end: usize,
        update_page_labels: bool,
        bookmarks: &[Bookmark],
        deep_clean: bool,
    ) -> Result<Vec<u8>> {
        let mut ldoc = lopdf::Document::load_mem(pdf_bytes)
            .context("lopdf could not parse the document for metadata sanitizing")?;
        if ldoc.is_encrypted() {
            anyhow::bail!("Encrypted PDF cannot have plaintext metadata/outlines injected without decryption");
        }

        // 1. Build `/PageLabels` dictionary only when active or explicitly updated
        let page_labels_id_opt = if skip_pages > 0 || roman_end > 0 || update_page_labels {
            let mut nums_arr = Vec::new();
            if skip_pages > 0 {
                // Empty PageLabel dict = unnumbered cover/front-matter sheets (PDF 1.7 Table 159)
                nums_arr.push(lopdf::Object::Integer(0));
                nums_arr.push(lopdf::Object::Dictionary(lopdf::Dictionary::new()));
            }
            if roman_end > 0 {
                let mut roman_dict = lopdf::Dictionary::new();
                roman_dict.set("S", lopdf::Object::Name(b"r".to_vec()));
                nums_arr.push(lopdf::Object::Integer(skip_pages as i64));
                nums_arr.push(lopdf::Object::Dictionary(roman_dict));
            }
            let mut arabic_dict = lopdf::Dictionary::new();
            arabic_dict.set("S", lopdf::Object::Name(b"D".to_vec()));
            arabic_dict.set("St", lopdf::Object::Integer(1));
            nums_arr.push(lopdf::Object::Integer((skip_pages + roman_end) as i64));
            nums_arr.push(lopdf::Object::Dictionary(arabic_dict));

            let mut page_labels_dict = lopdf::Dictionary::new();
            page_labels_dict.set("Nums", lopdf::Object::Array(nums_arr));
            Some(ldoc.add_object(lopdf::Object::Dictionary(page_labels_dict)))
        } else {
            None
        };

        // 2. Build hierarchical `/Outlines` tree if bookmarks are present
        let outlines_id_opt = if !bookmarks.is_empty() {
            let pages_map = ldoc.get_pages();
            let page_ids: Vec<lopdf::ObjectId> = pages_map.values().copied().collect();
            if !page_ids.is_empty() {
                let outlines_root_id = ldoc.new_object_id();

                struct ChapterNode {
                    bm: Bookmark,
                    obj_id: lopdf::ObjectId,
                    children: Vec<(Bookmark, lopdf::ObjectId)>,
                }

                let mut chapters: Vec<ChapterNode> = Vec::new();
                for bm in bookmarks {
                    if bm.level == 0 || chapters.is_empty() {
                        chapters.push(ChapterNode {
                            bm: bm.clone(),
                            obj_id: ldoc.new_object_id(),
                            children: Vec::new(),
                        });
                    } else if let Some(last_chap) = chapters.last_mut() {
                        last_chap.children.push((bm.clone(), ldoc.new_object_id()));
                    }
                }

                let format_bm_title = |b: &Bookmark| -> String {
                    if b.serial.is_empty() {
                        b.title.clone()
                    } else {
                        format!("{}. {}", b.serial, b.title)
                    }
                };

                let mut total_outline_items = 0i64;
                for i in 0..chapters.len() {
                    total_outline_items += 1 + (chapters[i].children.len() as i64);
                    let chap_id = chapters[i].obj_id;
                    let page_obj_id = page_ids[chapters[i]
                        .bm
                        .page_index
                        .min(page_ids.len().saturating_sub(1))];

                    let mut chap_dict = lopdf::Dictionary::new();
                    chap_dict.set(
                        "Title",
                        Self::pdf_text_string(format_bm_title(&chapters[i].bm)),
                    );
                    chap_dict.set("Parent", lopdf::Object::Reference(outlines_root_id));
                    chap_dict.set(
                        "Dest",
                        lopdf::Object::Array(vec![
                            lopdf::Object::Reference(page_obj_id),
                            lopdf::Object::Name(b"XYZ".to_vec()),
                            lopdf::Object::Null,
                            lopdf::Object::Null,
                            lopdf::Object::Null,
                        ]),
                    );

                    if i > 0 {
                        chap_dict.set("Prev", lopdf::Object::Reference(chapters[i - 1].obj_id));
                    }
                    if i + 1 < chapters.len() {
                        chap_dict.set("Next", lopdf::Object::Reference(chapters[i + 1].obj_id));
                    }

                    let children = &chapters[i].children;
                    if !children.is_empty() {
                        chap_dict.set("First", lopdf::Object::Reference(children[0].1));
                        chap_dict.set(
                            "Last",
                            lopdf::Object::Reference(children[children.len() - 1].1),
                        );
                        chap_dict.set("Count", lopdf::Object::Integer(children.len() as i64));

                        for c_idx in 0..children.len() {
                            let (ref c_bm, c_id) = children[c_idx];
                            let c_page_id = page_ids
                                [c_bm.page_index.min(page_ids.len().saturating_sub(1))];
                            let mut c_dict = lopdf::Dictionary::new();
                            c_dict.set(
                                "Title",
                                Self::pdf_text_string(format_bm_title(c_bm)),
                            );
                            c_dict.set("Parent", lopdf::Object::Reference(chap_id));
                            c_dict.set(
                                "Dest",
                                lopdf::Object::Array(vec![
                                    lopdf::Object::Reference(c_page_id),
                                    lopdf::Object::Name(b"XYZ".to_vec()),
                                    lopdf::Object::Null,
                                    lopdf::Object::Null,
                                    lopdf::Object::Null,
                                ]),
                            );
                            if c_idx > 0 {
                                c_dict.set("Prev", lopdf::Object::Reference(children[c_idx - 1].1));
                            }
                            if c_idx + 1 < children.len() {
                                c_dict.set("Next", lopdf::Object::Reference(children[c_idx + 1].1));
                            }
                            ldoc.objects.insert(c_id, lopdf::Object::Dictionary(c_dict));
                        }
                    }

                    ldoc.objects
                        .insert(chap_id, lopdf::Object::Dictionary(chap_dict));
                }

                let mut root_outlines = lopdf::Dictionary::new();
                root_outlines.set("Type", lopdf::Object::Name(b"Outlines".to_vec()));
                root_outlines.set("First", lopdf::Object::Reference(chapters[0].obj_id));
                root_outlines.set(
                    "Last",
                    lopdf::Object::Reference(chapters[chapters.len() - 1].obj_id),
                );
                root_outlines.set("Count", lopdf::Object::Integer(total_outline_items));
                ldoc.objects
                    .insert(outlines_root_id, lopdf::Object::Dictionary(root_outlines));
                Some(outlines_root_id)
            } else {
                None
            }
        } else {
            None
        };

        if let Ok(catalog_mut) = ldoc.catalog_mut() {
            if let Some(pl_id) = page_labels_id_opt {
                catalog_mut.set("PageLabels", lopdf::Object::Reference(pl_id));
            }
            if let Some(outlines_id) = outlines_id_opt {
                catalog_mut.set("Outlines", lopdf::Object::Reference(outlines_id));
            } else {
                // Deleting all bookmarks must remove existing /Outlines so they don't reappear on reload
                catalog_mut.remove(b"Outlines");
            }
            catalog_mut.remove(b"Metadata");
        }

        // Sanitize document metadata: strip /Author, /Creator, /Producer, /Keywords from /Info dictionary
        if let Ok(info_obj) = ldoc.trailer.get(b"Info").cloned() {
            match info_obj {
                lopdf::Object::Reference(info_id) => {
                    if let Ok(lopdf::Object::Dictionary(info_dict)) = ldoc.get_object_mut(info_id) {
                        info_dict.remove(b"Author");
                        info_dict.remove(b"Creator");
                        info_dict.remove(b"Producer");
                        info_dict.remove(b"Keywords");
                    }
                }
                lopdf::Object::Dictionary(mut info_dict) => {
                    info_dict.remove(b"Author");
                    info_dict.remove(b"Creator");
                    info_dict.remove(b"Producer");
                    info_dict.remove(b"Keywords");
                    ldoc.trailer.set("Info", lopdf::Object::Dictionary(info_dict));
                }
                _ => {}
            }
        }

        if deep_clean {
            ldoc.trailer.remove(b"Info");
            let mut live_annots: std::collections::HashSet<lopdf::ObjectId> =
                std::collections::HashSet::new();
            let page_ids: Vec<lopdf::ObjectId> = ldoc.get_pages().values().copied().collect();
            for pid in &page_ids {
                if let Ok(lopdf::Object::Dictionary(page_dict)) = ldoc.get_object(*pid) {
                    if let Ok(annots_obj) = page_dict.get(b"Annots") {
                        let arr_opt = match annots_obj {
                            lopdf::Object::Array(a) => Some(a.clone()),
                            lopdf::Object::Reference(aid) => ldoc
                                .get_object(*aid)
                                .and_then(|o| o.as_array())
                                .ok()
                                .cloned(),
                            _ => None,
                        };
                        if let Some(arr) = arr_opt {
                            for item in arr {
                                if let lopdf::Object::Reference(rid) = item {
                                    live_annots.insert(rid);
                                }
                            }
                        }
                    }
                }
            }
            for pid in page_ids {
                if let Ok(lopdf::Object::Dictionary(page_dict)) = ldoc.get_object_mut(pid) {
                    page_dict.remove(b"Thumb");
                    page_dict.remove(b"PieceInfo");
                }
            }
            let acro_ref = ldoc
                .catalog()
                .ok()
                .and_then(|c| c.get(b"AcroForm").ok())
                .and_then(|o| o.as_reference().ok());
            if let Ok(cat_mut) = ldoc.catalog_mut() {
                cat_mut.remove(b"StructTreeRoot");
            }
            if let Some(acro_id) = acro_ref {
                let fields_arr_opt = ldoc
                    .get_object(acro_id)
                    .and_then(|o| o.as_dict())
                    .ok()
                    .and_then(|d| d.get(b"Fields").ok())
                    .and_then(|f| match f {
                        lopdf::Object::Array(a) => Some(a.clone()),
                        lopdf::Object::Reference(fid) => ldoc
                            .get_object(*fid)
                            .and_then(|o| o.as_array())
                            .ok()
                            .cloned(),
                        _ => None,
                    });
                if let Some(fields) = fields_arr_opt {
                    let filtered: Vec<lopdf::Object> = fields
                        .into_iter()
                        .filter(|item| {
                            if let lopdf::Object::Reference(rid) = item {
                                let is_merged_widget = ldoc
                                    .get_object(*rid)
                                    .and_then(|o| o.as_dict())
                                    .ok()
                                    .and_then(|d| d.get(b"Subtype").ok())
                                    .and_then(|s| s.as_name_str().ok())
                                    .map(|s| s == "Widget")
                                    .unwrap_or(false);
                                !is_merged_widget || live_annots.contains(rid)
                            } else {
                                true
                            }
                        })
                        .collect();
                    if let Ok(lopdf::Object::Dictionary(acro_dict)) = ldoc.get_object_mut(acro_id) {
                        acro_dict.set("Fields", lopdf::Object::Array(filtered));
                    }
                }
            }
            ldoc.prune_objects();
        }

        let mut out_bytes = Vec::with_capacity(pdf_bytes.len() + 1024);
        ldoc.save_to(&mut out_bytes)
            .context("lopdf could not write the sanitized document")?;
        if out_bytes.is_empty() {
            anyhow::bail!("lopdf produced an empty file while sanitizing");
        }
        Ok(out_bytes)
    }

    /// Safely saves a `PdfDocument` to disk by buffering into memory first, then writing
    /// atomically (temp file + rename).
    pub fn save_doc<P: AsRef<Path>>(doc: &PdfDocument, output_path: P) -> Result<()> {
        let bytes = doc
            .save_to_bytes()
            .context("Failed to serialize PDF document to bytes")?;
        Self::write_atomic(output_path.as_ref(), &bytes)
    }

    /// Serializes `doc` with `/PageLabels`, `/Outlines` and sanitized metadata (`skip_pages = 0`).
    pub fn serialize_doc_with_metadata(
        doc: &PdfDocument,
        roman_end: usize,
        bookmarks: &[Bookmark],
        redacted: bool,
    ) -> Result<(Vec<u8>, Option<String>)> {
        Self::serialize_doc_with_metadata_and_skip(
            doc,
            0,
            roman_end,
            roman_end > 0,
            bookmarks,
            redacted,
        )
    }

    /// Serializes `doc` with full `skip_pages` + `roman_end` `/PageLabels`, `/Outlines`, and metadata cleanup.
    pub fn serialize_doc_with_metadata_and_skip(
        doc: &PdfDocument,
        skip_pages: usize,
        roman_end: usize,
        update_page_labels: bool,
        bookmarks: &[Bookmark],
        redacted: bool,
    ) -> Result<(Vec<u8>, Option<String>)> {
        let raw_bytes = doc
            .save_to_bytes()
            .context("Failed to serialize PDF document to bytes")?;
        match Self::embed_metadata_in_pdf_bytes_with_skip(
            &raw_bytes,
            skip_pages,
            roman_end,
            update_page_labels,
            bookmarks,
            redacted,
        ) {
            Ok(final_bytes) => Ok((final_bytes, None)),
            Err(e) if redacted => Err(e.context(
                "Save refused: post-redaction cleanup failed, nothing was written",
            )),
            Err(e) => Ok((raw_bytes, Some(format!("metadata was NOT sanitized ({e})")))),
        }
    }

    /// Saves `doc` with `/PageLabels`, `/Outlines` and sanitized metadata (see
    /// `serialize_doc_with_metadata`), written with `write_atomic`.
    pub fn save_doc_with_metadata<P: AsRef<Path>>(
        doc: &PdfDocument,
        output_path: P,
        roman_end: usize,
        bookmarks: &[Bookmark],
        redacted: bool,
    ) -> Result<Option<String>> {
        let (bytes, warning) =
            Self::serialize_doc_with_metadata(doc, roman_end, bookmarks, redacted)?;
        Self::write_atomic(output_path.as_ref(), &bytes)?;
        Ok(warning)
    }

    /// Sidecar file path (`<pdf_filename>.bookmarks.txt`) used to persist user bookmark edits.
    pub fn bookmarks_sidecar_path(pdf_path: &Path) -> PathBuf {
        let mut s = pdf_path.as_os_str().to_os_string();
        s.push(".bookmarks.txt");
        PathBuf::from(s)
    }

    /// Formats `bookmarks` into MicroPDF's documented `Serial -- Title -- Page` `.txt` syntax.
    pub fn format_bookmarks_as_txt(bookmarks: &[Bookmark]) -> String {
        let mut out = String::from("// MicroPDF Bookmarks (Serial -- Title -- Page)\n");
        let mut chap_n = 0usize;
        let mut sub_n = 0usize;
        for bm in bookmarks {
            let serial = if !bm.serial.is_empty() {
                bm.serial.clone()
            } else if bm.level == 0 {
                chap_n += 1;
                sub_n = 0;
                chap_n.to_string()
            } else {
                let letter = (b'a' + (sub_n % 26) as u8) as char;
                sub_n += 1;
                letter.to_string()
            };
            let page_str = if bm.logical_page.is_empty() || bm.logical_page == "—" {
                format!("#{}", bm.page_index + 1)
            } else {
                bm.logical_page.clone()
            };
            out.push_str(&format!("{serial} -- {} -- {page_str}\n", bm.title));
        }
        out
    }

    /// Loads bookmarks for `doc` (`skip_pages = 0` legacy wrapper).
    pub fn load_bookmarks(
        doc: &PdfDocument,
        pdf_path: &Path,
        roman_end: usize,
    ) -> Vec<Bookmark> {
        Self::load_bookmarks_with_skip(doc, pdf_path, 0, roman_end)
    }

    /// Loads bookmarks from embedded PDF `/Outlines` first (falling back to legacy `.bookmarks.txt`
    /// sidecar only when the PDF has no embedded outlines), preserving original titles without
    /// injecting fabricated numeric prefixes.
    pub fn load_bookmarks_with_skip(
        doc: &PdfDocument,
        pdf_path: &Path,
        skip_pages: usize,
        roman_end: usize,
    ) -> Vec<Bookmark> {
        let total_pages = doc.pages().len() as usize;

        let mut list = Vec::new();
        for bm in doc.bookmarks().iter() {
            let raw_title = bm
                .title()
                .unwrap_or_else(|| "Untitled".to_string())
                .trim()
                .to_string();
            let page_opt = bm
                .destination()
                .and_then(|d| d.page_index().ok())
                .or_else(|| {
                    let act = bm.action()?;
                    let ld = act.as_local_destination_action()?;
                    let dest = ld.destination().ok()?;
                    dest.page_index().ok()
                });
            if let Some(page_idx) = page_opt {
                let idx = page_idx as usize;
                if total_pages == 0 || idx < total_pages {
                    let has_parent = bm.parent().is_some();
                    let (serial, title, level) =
                        if let Some((prefix, rest)) = raw_title.split_once(". ") {
                            let p_trim = prefix.trim();
                            let first_c = p_trim.chars().next().unwrap_or('1');
                            let valid_serial = !p_trim.is_empty()
                                && p_trim.len() <= 3
                                && (p_trim.chars().all(|c| c.is_ascii_digit())
                                    || (p_trim.len() == 1 && first_c.is_ascii_lowercase()));
                            if valid_serial {
                                let lvl = if has_parent || first_c.is_ascii_alphabetic() {
                                    1
                                } else {
                                    0
                                };
                                (p_trim.to_string(), rest.trim().to_string(), lvl)
                            } else {
                                (String::new(), raw_title, if has_parent { 1 } else { 0 })
                            }
                        } else {
                            (
                                String::new(),
                                if raw_title.is_empty() {
                                    format!("Page {}", idx + 1)
                                } else {
                                    raw_title
                                },
                                if has_parent { 1 } else { 0 },
                            )
                        };
                    let logical_page =
                        Self::format_logical_page_label_with_skip(idx, skip_pages, roman_end);
                    list.push(Bookmark {
                        serial,
                        title,
                        logical_page,
                        page_index: idx,
                        level,
                    });
                }
            }
        }
        if !list.is_empty() {
            return list;
        }
        let sidecar = Self::bookmarks_sidecar_path(pdf_path);
        if let Ok(content) = std::fs::read_to_string(&sidecar) {
            return Self::parse_bookmarks_txt_with_skip(
                &content,
                skip_pages,
                roman_end,
                total_pages,
            );
        }
        list
    }

    /// Saves bookmarks to `<pdf_filename>.bookmarks.txt` in `--` delimited hierarchical format
    /// (`Serial -- Title -- LogicalPage`) so user edits and hierarchy levels are preserved.
    pub fn save_bookmarks_sidecar(pdf_path: &Path, bookmarks: &[Bookmark]) -> Result<()> {
        let sidecar = Self::bookmarks_sidecar_path(pdf_path);
        if bookmarks.is_empty() {
            if sidecar.exists() {
                let _ = std::fs::remove_file(&sidecar);
            }
            return Ok(());
        }
        let mut out = String::new();
        for (i, bm) in bookmarks.iter().enumerate() {
            let clean_title: String = bm
                .title
                .chars()
                .map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c })
                .collect();
            let serial = if bm.serial.trim().is_empty() {
                if bm.level == 0 {
                    format!("{}", i + 1)
                } else {
                    "a".to_string()
                }
            } else {
                bm.serial.trim().to_string()
            };
            let logical = if bm.logical_page.trim().is_empty() {
                format!("#{}", bm.page_index + 1)
            } else {
                bm.logical_page.trim().to_string()
            };
            out.push_str(&format!("{serial} -- {} -- {logical}\n", clean_title.trim()));
        }
        std::fs::write(&sidecar, out)
            .with_context(|| format!("Failed to write bookmarks sidecar {:?}", sidecar))?;
        Ok(())
    }

    /// Clears existing headers, footers, or page numbers in the specified `MarginZone` on `page`:
    /// 1. Iterates over `page.objects_mut()` and removes any text/path objects whose bounding box
    ///    lies completely within that margin band.
    /// 2. Places an opaque white rectangle (`PdfPagePathObject::new_rect` filled with white `255, 255, 255`)
    ///    over that margin band so even flattened/scanned existing headers, footers, or page numbers are cleanly erased.
    pub fn clear_margin_zone(page: &mut PdfPage, zone: MarginZone) -> Result<usize> {
        Self::clear_margin_zone_with_height(page, zone, 55.0)
    }

    /// Clears `zone` using a configurable `band_height` in PDF points (55pt default, up to 96pt
    /// when larger user margins are configured).
    pub fn clear_margin_zone_with_height(
        page: &mut PdfPage,
        zone: MarginZone,
        band_height: f32,
    ) -> Result<usize> {
        let page_w = page.width().value;
        let page_h = page.height().value;
        let band_height = band_height.clamp(24.0, (page_h * 0.35).max(24.0));

        let (zone_bottom, zone_left, zone_top, zone_right) = match zone {
            MarginZone::Header => ((page_h - band_height).max(0.0), 0.0, page_h, page_w),
            MarginZone::Footer => (0.0, 0.0, band_height.min(page_h), page_w),
            MarginZone::PageNumber => {
                // Bottom-center band (centered horizontally across the middle 44% of the page)
                let center_x = page_w * 0.5;
                let half_span = (page_w * 0.22).max(70.0);
                (
                    0.0,
                    (center_x - half_span).max(0.0),
                    band_height.min(page_h),
                    (center_x + half_span).min(page_w),
                )
            }
        };

        // 1. Remove any text or path objects whose bounding box lies completely within the margin band
        let mut to_remove = Vec::new();
        let obj_count = page.objects().len();
        for i in 0..obj_count {
            if let Ok(obj) = page.objects().get(i) {
                let is_text_or_path = matches!(
                    obj.object_type(),
                    PdfPageObjectType::Text | PdfPageObjectType::Path
                );
                if is_text_or_path {
                    if let Ok(b) = obj.bounds() {
                        let b_bottom = b.bottom().value.min(b.top().value);
                        let b_top = b.bottom().value.max(b.top().value);
                        let b_left = b.left().value.min(b.right().value);
                        let b_right = b.left().value.max(b.right().value);

                        let eps = 2.0_f32;
                        let inside_band = b_bottom >= (zone_bottom - eps)
                            && b_top <= (zone_top + eps)
                            && b_left >= (zone_left - eps)
                            && b_right <= (zone_right + eps);
                        if inside_band {
                            to_remove.push(i);
                        }
                    }
                }
            }
        }

        let prev_strategy = page.content_regeneration_strategy();
        page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);

        let mut removed_count = 0;
        for &idx in to_remove.iter().rev() {
            if let Ok(obj) = page.objects_mut().remove_object_at_index(idx) {
                // Note: In PDFium, calling FPDFPageObj_Destroy after FPDFPage_RemoveObject +
                // FPDFPage_GenerateContent on reloaded page content stream objects can trigger
                // a double-free/segfault inside PDFium's CPDF_PageContentGenerator.
                std::mem::forget(obj);
                removed_count += 1;
            }
        }

        // 2. Place an opaque white rectangle over the margin band so flattened/scanned text is also cleanly erased
        let white_rect = PdfRect::new_from_values(zone_bottom, zone_left, zone_top, zone_right);
        let rect_res = page
            .objects_mut()
            .create_path_object_rect(white_rect, None, None, Some(PdfColor::WHITE))
            .context("Failed to create whiteout margin rectangle");

        let regen_res = page.regenerate_content();
        page.set_content_regeneration_strategy(prev_strategy);

        rect_res?;
        regen_res.context("Failed to regenerate page content after clearing margin zone")?;

        Ok(removed_count)
    }

    /// Clears the specified `MarginZone` across a 1-based inclusive page range `start_page..=end_page`.
    pub fn clear_margin_zone_in_doc(
        doc: &mut PdfDocument,
        zone: MarginZone,
        start_page: u16,
        end_page: u16,
    ) -> Result<()> {
        let total = doc.pages().len();
        if total == 0 {
            return Ok(());
        }
        let start_idx = start_page.saturating_sub(1);
        let end_idx = end_page.min(total).saturating_sub(1);
        if start_idx > end_idx {
            return Ok(());
        }
        for idx in start_idx..=end_idx {
            let mut page = doc.pages_mut().get(idx)?;
            Self::clear_margin_zone(&mut page, zone)?;
        }
        Ok(())
    }

    /// Generate a sample multi-page test PDF (`create_sample_pdf(output_path, num_pages)`)
    pub fn create_sample_pdf<P: AsRef<Path>>(&self, output_path: P, num_pages: u16) -> Result<()> {
        let mut doc = self
            .pdfium
            .create_new_pdf()
            .context("Failed to create new PDF document")?;

        let font = doc.fonts_mut().helvetica();
        let font_bold = doc.fonts_mut().helvetica_bold();

        for i in 1..=num_pages {
            let mut page = doc
                .pages_mut()
                .create_page_at_end(PdfPagePaperSize::a4())
                .with_context(|| format!("Failed to create page {i}"))?;

            let mut title = PdfPageTextObject::new(
                &doc,
                format!("Sample Document - Page {i} of {num_pages}"),
                font_bold,
                PdfPoints::new(24.0),
            )?;
            title.set_fill_color(PdfColor::new(30, 60, 120, 255))?;
            title.translate(PdfPoints::new(72.0), PdfPoints::new(720.0))?;
            page.objects_mut().add_text_object(title)?;

            let lines = [
                format!(
                    "This is auto-generated content for page {i}. Built with MicroPDF & PDFium."
                ),
                "Drag your mouse across any text on this page to highlight it interactively!".to_string(),
                "Press 'B' to toggle the Left Bookmark Sidebar, or 'Tab' for Thumbnail Grid View.".to_string(),
                "Use 'N' for Roman/Arabic page numbering, 'H' for aligned headers/footers, 'C' to clear margins.".to_string(),
                "Use 'W' for watermarks, 'I' to insert a page, 'X' to split, and 'M' or drag-drop to merge.".to_string(),
            ];

            for (line_idx, text_line) in lines.iter().enumerate() {
                let mut body = PdfPageTextObject::new(&doc, text_line, font, PdfPoints::new(12.0))?;
                body.set_fill_color(PdfColor::new(50, 50, 50, 255))?;
                let y = 680.0 - (line_idx as f32 * 24.0);
                body.translate(PdfPoints::new(72.0), PdfPoints::new(y))?;
                page.objects_mut().add_text_object(body)?;
            }
        }

        Self::save_doc(&doc, output_path).context("Failed to save sample PDF")
    }

    /// Collects merged line-level text rects overlapping `normalized_sel` on `page_index`.
    fn collect_line_rects_in_selection(
        doc: &PdfDocument,
        page_index: u16,
        normalized_sel: PdfRect,
    ) -> Result<Vec<PdfRect>> {
        let sel_bottom = normalized_sel.bottom().value;
        let sel_top = normalized_sel.top().value;
        let sel_left = normalized_sel.left().value;
        let sel_right = normalized_sel.right().value;

        let mut target_rects: Vec<PdfRect> = Vec::new();
        let page = doc
            .pages()
            .get(page_index)
            .context("Page index out of bounds for text markup")?;

        if let Ok(text_page) = page.text() {
            let mut char_boxes: Vec<PdfRect> = Vec::new();
            for ch in text_page.chars().iter() {
                if let Some(uc) = ch.unicode_char() {
                    if uc.is_control() {
                        continue;
                    }
                }
                let bounds = ch.loose_bounds().or_else(|_| ch.tight_bounds());
                if let Ok(b) = bounds {
                    let b_bottom = b.bottom().value.min(b.top().value);
                    let b_top = b.bottom().value.max(b.top().value);
                    let b_left = b.left().value.min(b.right().value);
                    let b_right = b.left().value.max(b.right().value);

                    if (b_right - b_left) < 0.1 || (b_top - b_bottom) < 0.1 {
                        continue;
                    }

                    let overlaps = b_left <= sel_right
                        && b_right >= sel_left
                        && b_bottom <= sel_top
                        && b_top >= sel_bottom;
                    if overlaps {
                        char_boxes.push(PdfRect::new_from_values(
                            b_bottom, b_left, b_top, b_right,
                        ));
                    }
                }
            }

            for cb in char_boxes {
                if let Some(last) = target_rects.last_mut() {
                    let last_mid_y = (last.bottom().value + last.top().value) * 0.5;
                    let cb_mid_y = (cb.bottom().value + cb.top().value) * 0.5;
                    let line_height = (last.top().value - last.bottom().value).max(4.0);

                    if (last_mid_y - cb_mid_y).abs() < line_height * 0.65 {
                        let merged_bottom = last.bottom().value.min(cb.bottom().value);
                        let merged_left = last.left().value.min(cb.left().value);
                        let merged_top = last.top().value.max(cb.top().value);
                        let merged_right = last.right().value.max(cb.right().value);
                        *last = PdfRect::new_from_values(
                            merged_bottom,
                            merged_left,
                            merged_top,
                            merged_right,
                        );
                        continue;
                    }
                }
                target_rects.push(cb);
            }

            if target_rects.is_empty() {
                for seg in text_page.segments().iter() {
                    let b = seg.bounds();
                    if b.does_overlap(&normalized_sel) {
                        let clip_left = b.left().value.max(sel_left);
                        let clip_right = b.right().value.min(sel_right);
                        if clip_right > clip_left {
                            target_rects.push(PdfRect::new_from_values(
                                b.bottom().value,
                                clip_left,
                                b.top().value,
                                clip_right,
                            ));
                        }
                    }
                }
            }
        }

        if target_rects.is_empty() {
            target_rects.push(normalized_sel);
        }
        Ok(target_rects)
    }

    fn normalize_sel_rect(pdf_rect: PdfRect) -> Option<PdfRect> {
        let sel_bottom = pdf_rect.bottom().value.min(pdf_rect.top().value);
        let sel_top = pdf_rect.bottom().value.max(pdf_rect.top().value);
        let sel_left = pdf_rect.left().value.min(pdf_rect.right().value);
        let sel_right = pdf_rect.left().value.max(pdf_rect.right().value);
        if (sel_right - sel_left) < 2.0 && (sel_top - sel_bottom) < 2.0 {
            return None;
        }
        Some(PdfRect::new_from_values(sel_bottom, sel_left, sel_top, sel_right))
    }

    /// Highlights text in `pdf_rect` on `page_index` (0-based) of `doc` in default Yellow.
    #[allow(dead_code)]
    pub fn highlight_text_in_rect(
        doc: &mut PdfDocument,
        page_index: u16,
        pdf_rect: PdfRect,
    ) -> Result<usize> {
        Self::highlight_text_in_rect_with_color(doc, page_index, pdf_rect, AnnotationColor::Yellow)
    }

    /// Highlights text in `pdf_rect` on `page_index` (0-based) of `doc` using `color`.
    /// Stored inside a PDF annotation so `Print Without Annotations` cleanly hides it.
    pub fn highlight_text_in_rect_with_color(
        doc: &mut PdfDocument,
        page_index: u16,
        pdf_rect: PdfRect,
        color: AnnotationColor,
    ) -> Result<usize> {
        let Some(normalized_sel) = Self::normalize_sel_rect(pdf_rect) else {
            return Ok(0);
        };
        let target_rects = Self::collect_line_rects_in_selection(doc, page_index, normalized_sel)?;
        if target_rects.is_empty() {
            return Ok(0);
        }
        let highlight_fill = color.highlight_pdf_color();

        let mut path_objects = Vec::with_capacity(target_rects.len());
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for rect in &target_rects {
            let padded = PdfRect::new_from_values(
                rect.bottom().value - 1.0,
                rect.left().value - 1.0,
                rect.top().value + 1.0,
                rect.right().value + 1.0,
            );
            min_x = min_x.min(padded.left().value);
            min_y = min_y.min(padded.bottom().value);
            max_x = max_x.max(padded.right().value);
            max_y = max_y.max(padded.top().value);
            let mut path_obj =
                PdfPagePathObject::new_rect(doc, padded, None, None, Some(highlight_fill))?;
            let _ = path_obj.set_blend_mode(PdfPageObjectBlendMode::Multiply);
            path_objects.push(path_obj);
        }

        let bounds = PdfRect::new_from_values(min_y, min_x, max_y, max_x);
        let mut page = doc
            .pages_mut()
            .get(page_index)
            .context("Page index out of bounds for mutable highlight")?;

        let mut annot = page.annotations_mut().create_stamp_annotation()?;
        let _ = annot.set_is_printed(true);
        let _ = annot.set_bounds(bounds);
        let _ = annot.set_creator("MicroPDF:Highlight");
        for path_obj in path_objects {
            annot.objects_mut().add_path_object(path_obj)?;
        }

        Ok(target_rects.len())
    }

    /// Underlines text in `pdf_rect` on `page_index` (0-based) using `color`.
    pub fn underline_text_in_rect(
        doc: &mut PdfDocument,
        page_index: u16,
        pdf_rect: PdfRect,
        color: AnnotationColor,
    ) -> Result<usize> {
        let Some(normalized_sel) = Self::normalize_sel_rect(pdf_rect) else {
            return Ok(0);
        };
        let target_rects = Self::collect_line_rects_in_selection(doc, page_index, normalized_sel)?;
        if target_rects.is_empty() {
            return Ok(0);
        }
        let stroke = color.stroke_pdf_color();

        let mut path_objects = Vec::with_capacity(target_rects.len());
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for rect in &target_rects {
            let y = (rect.bottom().value - 0.8).max(0.0);
            min_x = min_x.min(rect.left().value);
            max_x = max_x.max(rect.right().value);
            min_y = min_y.min(y - 2.0);
            max_y = max_y.max(y + 2.0);
            let line = PdfPagePathObject::new_line(
                doc,
                rect.left(),
                PdfPoints::new(y),
                rect.right(),
                PdfPoints::new(y),
                stroke,
                PdfPoints::new(1.3),
            )?;
            path_objects.push(line);
        }

        let bounds = PdfRect::new_from_values(min_y.max(0.0), min_x, max_y, max_x);
        let mut page = doc.pages_mut().get(page_index)?;
        let mut annot = page.annotations_mut().create_stamp_annotation()?;
        let _ = annot.set_is_printed(true);
        let _ = annot.set_bounds(bounds);
        let _ = annot.set_creator("MicroPDF:Underline");
        for obj in path_objects {
            annot.objects_mut().add_path_object(obj)?;
        }
        Ok(target_rects.len())
    }

    /// Strikes through text in `pdf_rect` on `page_index` (0-based) using `color`.
    pub fn strikeout_text_in_rect(
        doc: &mut PdfDocument,
        page_index: u16,
        pdf_rect: PdfRect,
        color: AnnotationColor,
    ) -> Result<usize> {
        let Some(normalized_sel) = Self::normalize_sel_rect(pdf_rect) else {
            return Ok(0);
        };
        let target_rects = Self::collect_line_rects_in_selection(doc, page_index, normalized_sel)?;
        if target_rects.is_empty() {
            return Ok(0);
        }
        let stroke = color.stroke_pdf_color();

        let mut path_objects = Vec::with_capacity(target_rects.len());
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for rect in &target_rects {
            let mid_y = (rect.bottom().value + rect.top().value) * 0.5;
            min_x = min_x.min(rect.left().value);
            max_x = max_x.max(rect.right().value);
            min_y = min_y.min(mid_y - 2.0);
            max_y = max_y.max(mid_y + 2.0);
            let line = PdfPagePathObject::new_line(
                doc,
                rect.left(),
                PdfPoints::new(mid_y),
                rect.right(),
                PdfPoints::new(mid_y),
                stroke,
                PdfPoints::new(1.3),
            )?;
            path_objects.push(line);
        }

        let bounds = PdfRect::new_from_values(min_y.max(0.0), min_x, max_y, max_x);
        let mut page = doc.pages_mut().get(page_index)?;
        let mut annot = page.annotations_mut().create_stamp_annotation()?;
        let _ = annot.set_is_printed(true);
        let _ = annot.set_bounds(bounds);
        let _ = annot.set_creator("MicroPDF:Strikeout");
        for obj in path_objects {
            annot.objects_mut().add_path_object(obj)?;
        }
        Ok(target_rects.len())
    }

    /// Adds a draggable PDF Sticky Note annotation with visible note icon badge at `(pdf_x, pdf_y)`.
    pub fn add_sticky_note(
        doc: &mut PdfDocument,
        page_index: u16,
        pdf_x: f32,
        pdf_y: f32,
        note_text: &str,
        color: AnnotationColor,
    ) -> Result<()> {
        let trimmed = note_text.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        let size = 18.0f32;
        let rect = PdfRect::new_from_values(
            (pdf_y - size * 0.5).max(2.0),
            (pdf_x - size * 0.5).max(2.0),
            pdf_y + size * 0.5,
            pdf_x + size * 0.5,
        );
        let (r, g, b) = color.rgb();
        let fill = PdfColor::new(r, g, b, 235);
        let border = color.stroke_pdf_color();

        let badge = PdfPagePathObject::new_rect(
            doc,
            rect,
            Some(border),
            Some(PdfPoints::new(1.1)),
            Some(fill),
        )?;
        let l1 = PdfPagePathObject::new_line(
            doc,
            PdfPoints::new(rect.left().value + 3.5),
            PdfPoints::new(rect.top().value - 6.0),
            PdfPoints::new(rect.right().value - 3.5),
            PdfPoints::new(rect.top().value - 6.0),
            PdfColor::new(30, 41, 59, 220),
            PdfPoints::new(1.0),
        )?;
        let l2 = PdfPagePathObject::new_line(
            doc,
            PdfPoints::new(rect.left().value + 3.5),
            PdfPoints::new(rect.top().value - 10.0),
            PdfPoints::new(rect.right().value - 5.5),
            PdfPoints::new(rect.top().value - 10.0),
            PdfColor::new(30, 41, 59, 220),
            PdfPoints::new(1.0),
        )?;

        let mut page = doc.pages_mut().get(page_index)?;
        let mut annot = page.annotations_mut().create_stamp_annotation()?;
        let _ = annot.set_is_printed(true);
        let _ = annot.set_bounds(rect);
        let _ = annot.set_creator("MicroPDF:StickyNote");
        let _ = annot.set_contents(trimmed);
        annot.objects_mut().add_path_object(badge)?;
        annot.objects_mut().add_path_object(l1)?;
        annot.objects_mut().add_path_object(l2)?;
        Ok(())
    }

    /// Reads all Sticky Note annotations (`/Text` or `MicroPDF:StickyNote` stamps) on `page`.
    #[allow(dead_code)]
    pub fn read_sticky_notes_on_page(page: &PdfPage) -> Vec<(PdfRect, String)> {
        Self::find_movable_annotations_on_page(page)
            .into_iter()
            .filter(|m| m.is_sticky_note && !m.text.is_empty())
            .map(|m| (m.bounds, m.text))
            .collect()
    }

    /// Finds all Sticky Note and Text Box annotations on `page` so the user can click to read
    /// or drag to reposition them cleanly.
    pub fn find_movable_annotations_on_page(page: &PdfPage) -> Vec<MovableAnnotation> {
        let mut out = Vec::new();
        for (idx, annot) in page.annotations().iter().enumerate() {
            let Ok(bounds) = annot.bounds() else {
                continue;
            };
            match annot.annotation_type() {
                PdfPageAnnotationType::Text => {
                    out.push(MovableAnnotation {
                        index: idx,
                        is_sticky_note: true,
                        bounds,
                        text: annot.contents().unwrap_or_default().trim().to_string(),
                    });
                }
                PdfPageAnnotationType::FreeText => {
                    out.push(MovableAnnotation {
                        index: idx,
                        is_sticky_note: false,
                        bounds,
                        text: annot.contents().unwrap_or_default().trim().to_string(),
                    });
                }
                PdfPageAnnotationType::Stamp => {
                    let creator = annot.creator().unwrap_or_default();
                    if creator == "MicroPDF:StickyNote" {
                        out.push(MovableAnnotation {
                            index: idx,
                            is_sticky_note: true,
                            bounds,
                            text: annot.contents().unwrap_or_default().trim().to_string(),
                        });
                    } else if creator == "MicroPDF:FreeText" {
                        out.push(MovableAnnotation {
                            index: idx,
                            is_sticky_note: false,
                            bounds,
                            text: annot.contents().unwrap_or_default().trim().to_string(),
                        });
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Translates an existing Sticky Note or Text Box annotation (and its inner appearance objects)
    /// by `(dx_pts, dy_pts)` in PDF point coordinates while preserving exact size inside page bounds.
    pub fn move_annotation_by(
        doc: &mut PdfDocument,
        page_index: u16,
        annot_index: usize,
        dx_pts: f32,
        dy_pts: f32,
    ) -> Result<()> {
        let (pw, ph) = {
            let page = doc.pages().get(page_index)?;
            (page.width().value.max(10.0), page.height().value.max(10.0))
        };
        let mut page = doc.pages_mut().get(page_index)?;
        let mut annot = page.annotations_mut().get(annot_index)?;
        let old = annot.bounds()?;
        let old_l = old.left().value.min(old.right().value);
        let old_r = old.left().value.max(old.right().value);
        let old_b = old.bottom().value.min(old.top().value);
        let old_t = old.bottom().value.max(old.top().value);
        let w = (old_r - old_l).max(4.0);
        let h = (old_t - old_b).max(4.0);

        let new_l = (old_l + dx_pts).clamp(0.0, (pw - w).max(0.0));
        let new_b = (old_b + dy_pts).clamp(0.0, (ph - h).max(0.0));
        let actual_dx = new_l - old_l;
        let actual_dy = new_b - old_b;
        let new_bounds = PdfRect::new_from_values(new_b, new_l, new_b + h, new_l + w);
        annot.set_bounds(new_bounds)?;
        if let Some(stamp) = annot.as_stamp_annotation_mut() {
            for mut obj in stamp.objects_mut().iter() {
                let _ = obj.translate(PdfPoints::new(actual_dx), PdfPoints::new(actual_dy));
            }
        }
        Ok(())
    }

    /// Adds a smooth vector freehand pen stroke stored inside a PDF annotation on `page_index`.
    pub fn add_freehand_ink(
        doc: &mut PdfDocument,
        page_index: u16,
        points: &[(f32, f32)],
        color: AnnotationColor,
        stroke_width: f32,
    ) -> Result<()> {
        if points.len() < 2 {
            return Ok(());
        }
        let stroke = color.stroke_pdf_color();
        let sw = stroke_width.clamp(0.8, 8.0);
        let mut path = PdfPagePathObject::new(
            doc,
            PdfPoints::new(points[0].0),
            PdfPoints::new(points[0].1),
            Some(stroke),
            Some(PdfPoints::new(sw)),
            None,
        )?;
        let mut min_x = points[0].0;
        let mut max_x = points[0].0;
        let mut min_y = points[0].1;
        let mut max_y = points[0].1;
        for &(x, y) in points.iter().skip(1) {
            path.line_to(PdfPoints::new(x), PdfPoints::new(y))?;
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }

        let bounds = PdfRect::new_from_values(
            (min_y - sw - 2.0).max(0.0),
            (min_x - sw - 2.0).max(0.0),
            max_y + sw + 2.0,
            max_x + sw + 2.0,
        );
        let mut page = doc.pages_mut().get(page_index)?;
        let mut annot = page.annotations_mut().create_stamp_annotation()?;
        let _ = annot.set_is_printed(true);
        let _ = annot.set_bounds(bounds);
        let _ = annot.set_creator("MicroPDF:Ink");
        annot.objects_mut().add_path_object(path)?;
        Ok(())
    }

    /// Adds a draggable FreeText / Text Box annotation at `(pdf_x, pdf_y)` (top-left anchor in PDF points).
    pub fn add_freetext_box(
        doc: &mut PdfDocument,
        page_index: u16,
        pdf_x: f32,
        pdf_y: f32,
        text: &str,
        font_size: f32,
        color: AnnotationColor,
    ) -> Result<()> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        let font = doc.fonts_mut().helvetica();
        let fsz = font_size.clamp(8.0, 36.0);
        let fill = color.stroke_pdf_color();
        let lines: Vec<&str> = trimmed.split('\n').collect();
        let line_h = fsz * 1.25;
        let mut text_objs = Vec::with_capacity(lines.len());
        let mut min_x = pdf_x.max(4.0);
        let mut max_x = min_x + 40.0;
        let mut min_y = (pdf_y - fsz).max(4.0);
        let mut max_y = pdf_y + fsz * 0.9;
        for (i, line) in lines.iter().enumerate() {
            let mut tobj = PdfPageTextObject::new(doc, *line, font, PdfPoints::new(fsz))?;
            let _ = tobj.set_fill_color(fill);
            let y = (pdf_y - (i as f32) * line_h).max(8.0);
            tobj.translate(PdfPoints::new(pdf_x.max(4.0)), PdfPoints::new(y))?;
            if let Ok(b) = tobj.bounds() {
                min_x = min_x.min(b.left().value);
                max_x = max_x.max(b.right().value);
                min_y = min_y.min(b.bottom().value);
                max_y = max_y.max(b.top().value);
            }
            text_objs.push(tobj);
        }
        let bounds = PdfRect::new_from_values(
            (min_y - 3.0).max(2.0),
            (min_x - 3.0).max(2.0),
            max_y + 3.0,
            max_x + 4.0,
        );

        let mut page = doc.pages_mut().get(page_index)?;
        let mut annot = page.annotations_mut().create_stamp_annotation()?;
        let _ = annot.set_is_printed(true);
        let _ = annot.set_bounds(bounds);
        let _ = annot.set_creator("MicroPDF:FreeText");
        let _ = annot.set_contents(trimmed);
        for tobj in text_objs {
            annot.objects_mut().add_text_object(tobj)?;
        }
        Ok(())
    }

    /// Adds a crisp stroked Rectangle shape stored inside a PDF annotation.
    pub fn add_rectangle_shape(
        doc: &mut PdfDocument,
        page_index: u16,
        pdf_rect: PdfRect,
        color: AnnotationColor,
        stroke_width: f32,
    ) -> Result<()> {
        let Some(norm) = Self::normalize_sel_rect(pdf_rect) else {
            return Ok(());
        };
        let sw = stroke_width.clamp(0.8, 8.0);
        let stroke = color.stroke_pdf_color();
        let rect_obj = PdfPagePathObject::new_rect(
            doc,
            norm,
            Some(stroke),
            Some(PdfPoints::new(sw)),
            None,
        )?;
        let bounds = PdfRect::new_from_values(
            (norm.bottom().value - sw - 1.0).max(0.0),
            (norm.left().value - sw - 1.0).max(0.0),
            norm.top().value + sw + 1.0,
            norm.right().value + sw + 1.0,
        );
        let mut page = doc.pages_mut().get(page_index)?;
        let mut annot = page.annotations_mut().create_stamp_annotation()?;
        let _ = annot.set_is_printed(true);
        let _ = annot.set_bounds(bounds);
        let _ = annot.set_creator("MicroPDF:Rect");
        annot.objects_mut().add_path_object(rect_obj)?;
        Ok(())
    }

    /// Adds a crisp vector Arrow shape from `(x1, y1)` to `(x2, y2)` with a filled arrowhead,
    /// stored inside a PDF annotation.
    pub fn add_arrow_shape(
        doc: &mut PdfDocument,
        page_index: u16,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        color: AnnotationColor,
        stroke_width: f32,
    ) -> Result<()> {
        let dx = x2 - x1;
        let dy = y2 - y1;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 4.0 {
            return Ok(());
        }
        let ux = dx / len;
        let uy = dy / len;
        let px = -uy;
        let py = ux;
        let head_len = (stroke_width * 5.2).min(len * 0.45).max(3.0);
        let head_half_w = head_len * 0.42;

        let base_x = x2 - ux * head_len;
        let base_y = y2 - uy * head_len;
        let left_x = base_x + px * head_half_w;
        let left_y = base_y + py * head_half_w;
        let right_x = base_x - px * head_half_w;
        let right_y = base_y - py * head_half_w;

        let sw = stroke_width.clamp(1.0, 8.0);
        let stroke = color.stroke_pdf_color();
        let shaft = PdfPagePathObject::new_line(
            doc,
            PdfPoints::new(x1),
            PdfPoints::new(y1),
            PdfPoints::new(base_x),
            PdfPoints::new(base_y),
            stroke,
            PdfPoints::new(sw),
        )?;
        let mut head = PdfPagePathObject::new(
            doc,
            PdfPoints::new(x2),
            PdfPoints::new(y2),
            Some(stroke),
            Some(PdfPoints::new(0.8)),
            Some(stroke),
        )?;
        head.line_to(PdfPoints::new(left_x), PdfPoints::new(left_y))?;
        head.line_to(PdfPoints::new(right_x), PdfPoints::new(right_y))?;
        head.close_path()?;

        let min_x = x1.min(x2).min(left_x).min(right_x) - sw - 2.0;
        let max_x = x1.max(x2).max(left_x).max(right_x) + sw + 2.0;
        let min_y = y1.min(y2).min(left_y).min(right_y) - sw - 2.0;
        let max_y = y1.max(y2).max(left_y).max(right_y) + sw + 2.0;
        let bounds = PdfRect::new_from_values(min_y.max(0.0), min_x.max(0.0), max_y, max_x);

        let mut page = doc.pages_mut().get(page_index)?;
        let mut annot = page.annotations_mut().create_stamp_annotation()?;
        let _ = annot.set_is_printed(true);
        let _ = annot.set_bounds(bounds);
        let _ = annot.set_creator("MicroPDF:Arrow");
        annot.objects_mut().add_path_object(shaft)?;
        annot.objects_mut().add_path_object(head)?;
        Ok(())
    }

    /// Marks visible annotations on all pages with the PDF PRINT flag (`/F` bit `0x04`) so
    /// older PDFs whose annotations lacked `/F` still render when `With Annots` printing is selected.
    pub fn ensure_annotations_printable(doc: &mut PdfDocument) {
        let total = doc.pages().len();
        for idx in 0..total {
            if let Ok(mut page) = doc.pages_mut().get(idx) {
                let count = page.annotations().len();
                for a_idx in 0..count {
                    if let Ok(mut annot) = page.annotations_mut().get(a_idx) {
                        if !annot.is_hidden() && !annot.is_printed() {
                            let _ = annot.set_is_printed(true);
                        }
                    }
                }
            }
        }
    }

    /// Expands `{page}` and `{total}` placeholders inside header/footer strings for 1-based `page_num`.
    pub fn expand_hf_tokens(template: &str, page_num: u16, total_pages: u16) -> String {
        template
            .replace("{page}", &page_num.to_string())
            .replace("{total}", &total_pages.to_string())
    }

    /// Formats a page number label for 1-based `page_num` out of `total_pages` using `cfg`.
    /// When `cfg.skip_pages > 0`, physical pages `1..=skip_pages` are left unnumbered (`None`),
    /// the next `cfg.roman_end` pages are numbered `i, ii, iii...`, and subsequent pages start at `1`.
    pub fn format_page_number_label(
        cfg: &PageNumberConfig,
        page_num: u16,
        total_pages: u16,
    ) -> Option<String> {
        if page_num <= cfg.skip_pages {
            return None;
        }
        let eff_num = page_num - cfg.skip_pages;
        let is_roman = cfg.roman_end > 0 && eff_num <= cfg.roman_end;
        let core = if is_roman {
            Self::to_roman(eff_num)
        } else {
            format!("{}", eff_num.saturating_sub(cfg.roman_end))
        };
        let arabic_total = total_pages
            .saturating_sub(cfg.skip_pages.saturating_add(cfg.roman_end))
            .max(1);
        let formatted = match cfg.format {
            PageNumberFormat::Plain => core,
            PageNumberFormat::PageN => format!("Page {core}"),
            PageNumberFormat::PageNOfTotal => {
                if is_roman {
                    format!("Page {core}")
                } else {
                    format!("Page {core} of {arabic_total}")
                }
            }
            PageNumberFormat::NSlashTotal => {
                if is_roman {
                    core
                } else {
                    format!("{core} / {arabic_total}")
                }
            }
            PageNumberFormat::Dashed => format!("- {core} -"),
        };
        Some(formatted)
    }

    /// Stamps up to 6 aligned Header/Footer slots (HL/HC/HR/FL/FC/FR) across 0-based `page_indices`
    /// with optional `{page}`/`{total}` tokens, font size, margin, color, and optional replace-existing clear.
    pub fn apply_header_footer_full(
        doc: &mut PdfDocument,
        profile: &HeaderFooterProfile,
        page_indices: &[u16],
        replace_existing: bool,
    ) -> Result<()> {
        let total_pages = doc.pages().len();
        if total_pages == 0 || page_indices.is_empty() {
            return Ok(());
        }
        let font = doc.fonts_mut().helvetica();
        let font_sz = profile.font_size.clamp(6.0, 24.0);
        let edge_y = profile.margin_pt.clamp(12.0, 72.0);
        let side_x = 36.0_f32;
        let color = profile.color.pdf_color();
        let has_hdr = profile.has_header();
        let has_ftr = profile.has_footer();

        let clear_h = (edge_y + font_sz + 14.0).clamp(55.0, 96.0);
        for &idx in page_indices {
            if idx >= total_pages {
                continue;
            }
            if replace_existing {
                let mut page = doc.pages_mut().get(idx)?;
                if has_hdr {
                    Self::clear_margin_zone_with_height(&mut page, MarginZone::Header, clear_h)?;
                }
                if has_ftr {
                    Self::clear_margin_zone_with_height(&mut page, MarginZone::Footer, clear_h)?;
                }
            }

            let (pw, ph) = {
                let page = doc.pages().get(idx)?;
                (page.width().value, page.height().value)
            };
            let pnum = idx + 1;

            let slots: [(MarginPosition, &str); 6] = [
                (MarginPosition::HeaderLeft, &profile.header_left),
                (MarginPosition::HeaderCenter, &profile.header_center),
                (MarginPosition::HeaderRight, &profile.header_right),
                (MarginPosition::FooterLeft, &profile.footer_left),
                (MarginPosition::FooterCenter, &profile.footer_center),
                (MarginPosition::FooterRight, &profile.footer_right),
            ];

            let mut objs = Vec::new();
            for (pos, raw_txt) in slots {
                let expanded = Self::expand_hf_tokens(raw_txt.trim(), pnum, total_pages);
                if expanded.is_empty() {
                    continue;
                }
                let mut obj = PdfPageTextObject::new(doc, &expanded, font, PdfPoints::new(font_sz))?;
                obj.set_fill_color(color)?;
                let tw = obj.width().unwrap_or(PdfPoints::new(40.0)).value;
                let x = match pos {
                    MarginPosition::HeaderLeft | MarginPosition::FooterLeft => side_x,
                    MarginPosition::HeaderCenter | MarginPosition::FooterCenter => {
                        ((pw - tw) * 0.5).max(side_x)
                    }
                    MarginPosition::HeaderRight | MarginPosition::FooterRight => {
                        (pw - tw - side_x).max(side_x)
                    }
                };
                let y = if pos.is_header() {
                    (ph - edge_y - font_sz * 0.75).max(0.0)
                } else {
                    edge_y
                };
                obj.translate(PdfPoints::new(x), PdfPoints::new(y))?;
                objs.push(obj);
            }

            if !objs.is_empty() {
                let mut page = doc.pages_mut().get(idx)?;
                for obj in objs {
                    page.objects_mut().add_text_object(obj)?;
                }
            }
        }
        Ok(())
    }

    /// Stamps formatted Page Numbers across the document using `PageNumberConfig`, clearing
    /// any existing page numbers in the target zone first (never touching skipped front pages).
    pub fn apply_page_numbers_full(
        doc: &mut PdfDocument,
        cfg: &PageNumberConfig,
    ) -> Result<()> {
        let total_pages = doc.pages().len();
        if total_pages == 0 {
            return Ok(());
        }
        let font = doc.fonts_mut().helvetica();
        let font_sz = cfg.font_size.clamp(6.0, 24.0);
        let edge_y = cfg.margin_pt.clamp(12.0, 72.0);
        let side_x = 36.0_f32;
        let color = cfg.color.pdf_color();
        let clear_h = (edge_y + font_sz + 14.0).clamp(55.0, 96.0);
        let clear_zone = if cfg.position == MarginPosition::FooterCenter {
            MarginZone::PageNumber
        } else {
            cfg.position.zone()
        };

        for idx in 0..total_pages {
            let pnum = idx + 1;
            let Some(label) = Self::format_page_number_label(cfg, pnum, total_pages) else {
                // Skipped cover/front-matter page: leave artwork and margins completely untouched!
                continue;
            };
            {
                let mut page = doc.pages_mut().get(idx)?;
                Self::clear_margin_zone_with_height(&mut page, clear_zone, clear_h)?;
            }
            let (pw, ph) = {
                let page = doc.pages().get(idx)?;
                (page.width().value, page.height().value)
            };
            let mut obj = PdfPageTextObject::new(doc, &label, font, PdfPoints::new(font_sz))?;
            obj.set_fill_color(color)?;
            let tw = obj.width().unwrap_or(PdfPoints::new(18.0)).value;
            let x = match cfg.position {
                MarginPosition::HeaderLeft | MarginPosition::FooterLeft => side_x,
                MarginPosition::HeaderCenter | MarginPosition::FooterCenter => {
                    ((pw - tw) * 0.5).max(side_x)
                }
                MarginPosition::HeaderRight | MarginPosition::FooterRight => {
                    (pw - tw - side_x).max(side_x)
                }
            };
            let y = if cfg.position.is_header() {
                (ph - edge_y - font_sz * 0.75).max(0.0)
            } else {
                edge_y
            };
            obj.translate(PdfPoints::new(x), PdfPoints::new(y))?;
            let mut page = doc.pages_mut().get(idx)?;
            page.objects_mut().add_text_object(obj)?;
        }
        Ok(())
    }

    /// In-memory page numbering on a `PdfDocument`: automatically clears existing `MarginZone::PageNumber`
    /// first so old page numbers are replaced cleanly instead of overlapping, then stamps Roman numerals
    /// (`i, ii, iii...`) on `1..=roman_end` and Arabic numerals (`1, 2, 3...`) after that at bottom-center.
    pub fn apply_page_numbers_to_doc(doc: &mut PdfDocument, roman_end: u16) -> Result<()> {
        let font = doc.fonts_mut().helvetica();
        let total_pages = doc.pages().len();

        for idx in 0..total_pages {
            // Clear existing page number zone first
            {
                let mut page = doc.pages_mut().get(idx)?;
                Self::clear_margin_zone(&mut page, MarginZone::PageNumber)?;
            }

            let page_num = idx + 1;
            let label = if page_num <= roman_end {
                Self::to_roman(page_num)
            } else {
                format!("{}", page_num - roman_end)
            };

            let page_width = doc.pages().get(idx)?.width();

            let mut text_obj = PdfPageTextObject::new(doc, &label, font, PdfPoints::new(10.0))?;
            text_obj.set_fill_color(PdfColor::new(80, 80, 80, 255))?;

            let text_width = text_obj.width().unwrap_or(PdfPoints::new(15.0));
            let x = (page_width - text_width) / 2.0;
            let y = PdfPoints::new(28.0);

            text_obj.translate(x, y)?;
            let mut page = doc.pages_mut().get(idx)?;
            page.objects_mut().add_text_object(text_obj)?;
        }
        Ok(())
    }

    /// Stamps or clears a header or footer with Left/Center/Right alignment (`MarginPosition`)
    /// across 1-based `start_page..=end_page`. Always calls `clear_margin_zone` first so old
    /// headers/footers are cleanly replaced (or deleted if `text` is empty).
    pub fn apply_aligned_header_footer_to_doc(
        doc: &mut PdfDocument,
        position: MarginPosition,
        text: &str,
        start_page: u16,
        end_page: u16,
    ) -> Result<()> {
        let font = doc.fonts_mut().helvetica();
        let total_pages = doc.pages().len();
        if total_pages == 0 {
            return Ok(());
        }

        let start_idx = start_page.saturating_sub(1);
        let end_idx = end_page.min(total_pages).saturating_sub(1);
        if start_idx > end_idx {
            return Ok(());
        }

        let trimmed = text.trim();
        let margin_pt = 36.0_f32;

        for idx in start_idx..=end_idx {
            // 1. Always clear the target margin zone first
            {
                let mut page = doc.pages_mut().get(idx)?;
                Self::clear_margin_zone(&mut page, position.zone())?;
            }

            if trimmed.is_empty() {
                continue;
            }

            let (page_width, page_height) = {
                let page = doc.pages().get(idx)?;
                (page.width().value, page.height().value)
            };

            let mut obj = PdfPageTextObject::new(doc, trimmed, font, PdfPoints::new(10.0))?;
            obj.set_fill_color(PdfColor::new(90, 90, 90, 255))?;
            let text_width = obj.width().unwrap_or(PdfPoints::new(60.0)).value;

            let x = match position {
                MarginPosition::HeaderLeft | MarginPosition::FooterLeft => margin_pt,
                MarginPosition::HeaderCenter | MarginPosition::FooterCenter => {
                    ((page_width - text_width) * 0.5).max(margin_pt)
                }
                MarginPosition::HeaderRight | MarginPosition::FooterRight => {
                    (page_width - text_width - margin_pt).max(margin_pt)
                }
            };

            let y = if position.is_header() {
                (page_height - 34.0).max(0.0)
            } else {
                26.0
            };

            obj.translate(PdfPoints::new(x), PdfPoints::new(y))?;
            let mut page = doc.pages_mut().get(idx)?;
            page.objects_mut().add_text_object(obj)?;
        }

        Ok(())
    }

    /// In-memory header/footer stamping on a `PdfDocument` across 1-based `start_page..=end_page`.
    pub fn apply_header_footer_to_doc(
        doc: &mut PdfDocument,
        header_text: Option<&str>,
        footer_text: Option<&str>,
        start_page: u16,
        end_page: u16,
    ) -> Result<()> {
        if let Some(h_text) = header_text {
            Self::apply_aligned_header_footer_to_doc(
                doc,
                MarginPosition::HeaderCenter,
                h_text,
                start_page,
                end_page,
            )?;
        }
        if let Some(f_text) = footer_text {
            Self::apply_aligned_header_footer_to_doc(
                doc,
                MarginPosition::FooterLeft,
                f_text,
                start_page,
                end_page,
            )?;
        }
        Ok(())
    }

    /// In-memory watermark stamping on a `PdfDocument` across all pages.
    pub fn apply_watermark_to_doc(
        doc: &mut PdfDocument,
        watermark_text: &str,
        diagonal: bool,
    ) -> Result<()> {
        let font = doc.fonts_mut().helvetica_bold();
        let total_pages = doc.pages().len();

        for idx in 0..total_pages {
            let (page_width, page_height) = {
                let page = doc.pages().get(idx)?;
                (page.width(), page.height())
            };

            let mut wm_obj =
                PdfPageTextObject::new(doc, watermark_text, font, PdfPoints::new(54.0))?;
            wm_obj.set_fill_color(PdfColor::new(180, 40, 40, 64))?;

            let text_width = wm_obj.width().unwrap_or(PdfPoints::new(250.0));
            let text_height = wm_obj.height().unwrap_or(PdfPoints::new(54.0));

            if diagonal {
                let angle_deg = 45.0_f32;
                let angle_rad = angle_deg.to_radians();
                wm_obj.rotate_counter_clockwise_degrees(angle_deg)?;

                let half_w = text_width.value / 2.0;
                let half_h = text_height.value / 2.0;
                let offset_x = half_w * angle_rad.cos() - half_h * angle_rad.sin();
                let offset_y = half_w * angle_rad.sin() + half_h * angle_rad.cos();

                let cx = page_width.value / 2.0 - offset_x;
                let cy = page_height.value / 2.0 - offset_y;
                wm_obj.translate(PdfPoints::new(cx), PdfPoints::new(cy))?;
            } else {
                let cx = (page_width - text_width) / 2.0;
                let cy = (page_height - text_height) / 2.0;
                wm_obj.translate(cx, cy)?;
            }

            let mut page = doc.pages_mut().get(idx)?;
            page.objects_mut().add_text_object(wm_obj)?;
        }
        Ok(())
    }

    /// Reorders a single page inside `doc` from `from_index` (0-based) to `to_index` (0-based)
    /// using PDFium page import/delete semantics.
    pub fn move_page_in_doc(&self, doc: &mut PdfDocument, from_index: u16, to_index: u16) -> Result<()> {
        let total = doc.pages().len();
        if from_index >= total || to_index >= total || from_index == to_index {
            return Ok(());
        }

        // Copy `from_index` into a temporary 1-page document, delete `from_index`, then import at `to_index`
        let mut temp_doc = self.pdfium.create_new_pdf()?;
        temp_doc
            .pages_mut()
            .copy_page_from_document(doc, from_index, 0)
            .context("Failed to copy page to temporary document for reordering")?;

        doc.pages()
            .get(from_index)
            .context("Source page out of bounds")?
            .delete()
            .context("Failed to delete original page during move")?;

        let dest_idx = to_index.min(doc.pages().len());
        doc.pages_mut()
            .copy_page_from_document(&temp_doc, 0, dest_idx)
            .context("Failed to re-insert moved page at target index")?;

        Ok(())
    }

    /// Imports all pages from an external PDF file (`external_path`) into `doc` right BEFORE
    /// `target_page_index` (0-based). Returns the number of imported pages.
    pub fn insert_external_pdf_at_index(
        &self,
        doc: &mut PdfDocument,
        external_path: &Path,
        target_page_index: u16,
    ) -> Result<u16> {
        self.insert_external_pdf_pages_at_index(doc, external_path, None, target_page_index)
    }

    /// Imports either all pages (`page_indices == None`) or specific 0-based `page_indices`
    /// from an external PDF file (`external_path`) into `doc` starting at 0-based
    /// `target_page_index`. Returns the number of imported pages.
    pub fn insert_external_pdf_pages_at_index(
        &self,
        doc: &mut PdfDocument,
        external_path: &Path,
        page_indices: Option<&[u16]>,
        target_page_index: u16,
    ) -> Result<u16> {
        let ext_doc = self.load_pdf_safe(external_path)?;
        let ext_count = ext_doc.pages().len();
        if ext_count == 0 {
            return Ok(0);
        }
        let mut dest_idx = target_page_index.min(doc.pages().len());
        match page_indices {
            None => {
                if dest_idx >= doc.pages().len() {
                    doc.pages_mut()
                        .append(&ext_doc)
                        .with_context(|| format!("Failed to append pages from {:?}", external_path))?;
                } else {
                    doc.pages_mut()
                        .copy_page_range_from_document(&ext_doc, 0..=(ext_count - 1), dest_idx)
                        .with_context(|| format!("Failed to import pages from {:?}", external_path))?;
                }
                Ok(ext_count)
            }
            Some(indices) => {
                let mut inserted: u16 = 0;
                for &src_idx in indices {
                    if src_idx < ext_count {
                        doc.pages_mut()
                            .copy_page_from_document(&ext_doc, src_idx, dest_idx)
                            .with_context(|| {
                                format!(
                                    "Failed to import page {} from {:?}",
                                    src_idx + 1,
                                    external_path
                                )
                            })?;
                        dest_idx = dest_idx.saturating_add(1);
                        inserted = inserted.saturating_add(1);
                    }
                }
                Ok(inserted)
            }
        }
    }

    /// Extracts a comma-separated 1-based page specification (e.g. `"1-3"` or `"2,4,6"`)
    /// from `src_doc` and saves it to `output_path`.
    pub fn extract_page_spec_from_doc<P: AsRef<Path>>(
        &self,
        src_doc: &PdfDocument,
        page_spec: &str,
        output_path: P,
    ) -> Result<u16> {
        let mut dest_doc = self
            .pdfium
            .create_new_pdf()
            .context("Failed to create destination PDF for split/extraction")?;

        let cleaned_spec: String = page_spec.chars().filter(|c| !c.is_whitespace()).collect();
        dest_doc
            .pages_mut()
            .copy_pages_from_document(src_doc, &cleaned_spec, 0)
            .with_context(|| {
                format!("Invalid or out-of-range page specification '{cleaned_spec}'")
            })?;

        let count = dest_doc.pages().len();
        Self::save_doc(&dest_doc, output_path)?;
        Ok(count)
    }

    /// Inserts a blank page into `doc` at `insert_index` (0-based), matching adjacent page size or A4.
    pub fn insert_blank_page_in_doc(doc: &mut PdfDocument, insert_index: u16) -> Result<u16> {
        Self::insert_blank_pages_in_doc(doc, insert_index, 1)
    }

    /// Inserts `count` (1..=50) blank pages into `doc` starting at `insert_index` (0-based),
    /// matching the adjacent page's paper dimensions (or A4 if empty).
    pub fn insert_blank_pages_in_doc(
        doc: &mut PdfDocument,
        insert_index: u16,
        count: u16,
    ) -> Result<u16> {
        let count = count.clamp(1, 50);
        let total = doc.pages().len();
        let clamped_idx = insert_index.min(total);
        let paper = if total > 0 {
            let ref_idx = if clamped_idx < total {
                clamped_idx
            } else {
                total - 1
            };
            doc.pages()
                .get(ref_idx)
                .ok()
                .map(|p| PdfPagePaperSize::Custom(p.width(), p.height()))
                .unwrap_or_else(PdfPagePaperSize::a4)
        } else {
            PdfPagePaperSize::a4()
        };
        for offset in 0..count {
            doc.pages_mut()
                .create_page_at_index(paper, clamped_idx + offset)
                .context("Failed to insert blank page")?;
        }
        Ok(clamped_idx)
    }

    /// Rotate specific pages (`rotate_pages`).
    /// `pages` uses 1-based page numbers. `degrees` can be 0, 90, 180, or 270 (clockwise).
    pub fn rotate_pages<P: AsRef<Path>>(
        &self,
        input_path: P,
        output_path: P,
        pages: &[u16],
        degrees: i32,
    ) -> Result<()> {
        let mut doc = self
            .load_pdf_file(input_path.as_ref())
            .context("Failed to load PDF for rotation")?;

        let rotation = match degrees.rem_euclid(360) {
            0 => PdfPageRenderRotation::None,
            90 => PdfPageRenderRotation::Degrees90,
            180 => PdfPageRenderRotation::Degrees180,
            270 => PdfPageRenderRotation::Degrees270,
            _ => anyhow::bail!("Rotation degrees must be a multiple of 90 (got {degrees})"),
        };

        for &page_num in pages {
            if page_num == 0 {
                anyhow::bail!("Page numbers are 1-based (got 0)");
            }
            let idx = page_num - 1;
            let mut page = doc
                .pages_mut()
                .get(idx)
                .with_context(|| format!("Page {page_num} out of bounds"))?;
            page.set_rotation(rotation);
        }

        let bytes = doc
            .save_to_bytes()
            .context("Failed to serialize rotated PDF")?;
        drop(doc);
        Self::write_atomic(output_path.as_ref(), &bytes).context("Failed to save rotated PDF")
    }

    /// Delete specific pages (`delete_pages`).
    /// `pages` uses 1-based page numbers. Deletes in descending order so indices don't shift.
    pub fn delete_pages<P: AsRef<Path>>(
        &self,
        input_path: P,
        output_path: P,
        pages: &[u16],
    ) -> Result<()> {
        for &p in pages {
            if p == 0 {
                anyhow::bail!("Page numbers are 1-based (got 0)");
            }
        }
        let doc = self
            .load_pdf_file(input_path.as_ref())
            .context("Failed to load PDF for deleting pages")?;

        let mut sorted_pages: Vec<u16> = pages.to_vec();
        sorted_pages.sort_unstable();
        sorted_pages.dedup();

        for &page_num in sorted_pages.iter().rev() {
            let idx = page_num - 1;
            let page = doc
                .pages()
                .get(idx)
                .with_context(|| format!("Page {page_num} out of bounds"))?;
            page.delete()
                .with_context(|| format!("Failed to delete page {page_num}"))?;
        }

        let bytes = doc
            .save_to_bytes()
            .context("Failed to serialize PDF after deleting pages")?;
        drop(doc);
        Self::write_atomic(output_path.as_ref(), &bytes)
            .context("Failed to save PDF after deleting pages")
    }

    /// Merge two PDFs (`merge_pdfs`).
    pub fn merge_pdfs<P: AsRef<Path>>(
        &self,
        first_path: P,
        second_path: P,
        output_path: P,
    ) -> Result<()> {
        let mut first_doc = self
            .load_pdf_file(first_path.as_ref())
            .context("Failed to load first PDF for merge")?;
        let second_doc = self
            .load_pdf_file(second_path.as_ref())
            .context("Failed to load second PDF for merge")?;

        first_doc
            .pages_mut()
            .append(&second_doc)
            .context("Failed to append second PDF into first PDF")?;

        let bytes = first_doc
            .save_to_bytes()
            .context("Failed to serialize merged PDF")?;
        drop(second_doc);
        drop(first_doc);
        Self::write_atomic(output_path.as_ref(), &bytes).context("Failed to save merged PDF")
    }

    /// Split/extract a page range into a new PDF (`extract_pages`).
    /// `start_page` and `end_page` are 1-based inclusive page numbers.
    pub fn extract_pages<P: AsRef<Path>>(
        &self,
        input_path: P,
        output_path: P,
        start_page: u16,
        end_page: u16,
    ) -> Result<()> {
        let src_doc = self
            .load_pdf_file(input_path.as_ref())
            .context("Failed to load source PDF for extraction")?;
        let mut dest_doc = self
            .pdfium
            .create_new_pdf()
            .context("Failed to create destination PDF for extraction")?;

        let start_idx = start_page.saturating_sub(1);
        let end_idx = end_page.saturating_sub(1);
        dest_doc
            .pages_mut()
            .copy_page_range_from_document(&src_doc, start_idx..=end_idx, 0)
            .with_context(|| format!("Failed to extract page range {start_page}-{end_page}"))?;

        let bytes = dest_doc
            .save_to_bytes()
            .context("Failed to serialize extracted PDF")?;
        drop(dest_doc);
        drop(src_doc);
        Self::write_atomic(output_path.as_ref(), &bytes).context("Failed to save extracted PDF")
    }

    /// Formats a zero-padded single-page PDF filename (e.g., `report_page_001.pdf`).
    pub fn format_single_page_pdf_name(prefix: &str, page_num_1based: u16, total_pages: u16) -> String {
        let clean: String = prefix
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
        let clean = clean.trim();
        let stem = if clean.is_empty() { "page" } else { clean };
        let width = total_pages.max(1).to_string().len().max(3);
        format!("{stem}_page_{page_num_1based:0width$}.pdf")
    }

    /// Losslessly splits selected 0-based `page_indices` of `doc` into individual single-page
    /// `.pdf` files inside `out_dir` via PDFium page-import/export APIs (zero rasterization).
    pub fn split_doc_to_single_page_pdfs<P: AsRef<Path>>(
        &self,
        doc: &PdfDocument,
        page_indices: &[u16],
        out_dir: P,
        prefix: &str,
        mut on_progress: impl FnMut(usize, usize),
    ) -> Result<usize> {
        let total_doc_pages = doc.pages().len();
        if total_doc_pages == 0 || page_indices.is_empty() {
            anyhow::bail!("No pages selected to split");
        }
        let dir = out_dir.as_ref();
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Failed to create output folder {}", dir.display()))?;

        let total_to_export = page_indices.len();
        let mut written = 0usize;
        for (step, &page_idx) in page_indices.iter().enumerate() {
            if page_idx >= total_doc_pages {
                continue;
            }
            let mut single = self
                .pdfium
                .create_new_pdf()
                .context("Failed to create single-page destination PDF")?;
            single
                .pages_mut()
                .copy_page_from_document(doc, page_idx, 0)
                .with_context(|| format!("Failed to copy page {} losslessly", page_idx + 1))?;
            let bytes = single
                .save_to_bytes()
                .with_context(|| format!("Failed to serialize page {}", page_idx + 1))?;
            drop(single);

            let fname = Self::format_single_page_pdf_name(prefix, page_idx + 1, total_doc_pages);
            let out_path = dir.join(fname);
            Self::write_atomic(&out_path, &bytes)
                .with_context(|| format!("Failed to write {}", out_path.display()))?;
            written += 1;
            on_progress(step + 1, total_to_export);
        }
        Ok(written)
    }

    /// Stamp Page Numbers (`add_page_numbers`) supporting Roman numerals (e.g., i, ii, iii, iv, v)
    /// for pages `1..=roman_end` and standard Arabic numerals (1, 2, 3...) after that,
    /// positioned at bottom-center.
    pub fn add_page_numbers<P: AsRef<Path>>(
        &self,
        input_path: P,
        output_path: P,
        roman_end: u16,
    ) -> Result<()> {
        let mut doc = self
            .load_pdf_file(input_path.as_ref())
            .context("Failed to load PDF for adding page numbers")?;
        Self::apply_page_numbers_to_doc(&mut doc, roman_end)?;
        let bytes = doc
            .save_to_bytes()
            .context("Failed to serialize PDF with page numbers")?;
        drop(doc);
        Self::write_atomic(output_path.as_ref(), &bytes)
            .context("Failed to save PDF with page numbers")
    }

    /// Stamp Headers and Footers (`add_header_footer`) for a given 1-based inclusive page range.
    pub fn add_header_footer<P: AsRef<Path>>(
        &self,
        input_path: P,
        output_path: P,
        header_text: Option<&str>,
        footer_text: Option<&str>,
        start_page: u16,
        end_page: u16,
    ) -> Result<()> {
        let mut doc = self
            .load_pdf_file(input_path.as_ref())
            .context("Failed to load PDF for header/footer")?;
        Self::apply_header_footer_to_doc(&mut doc, header_text, footer_text, start_page, end_page)?;
        let bytes = doc
            .save_to_bytes()
            .context("Failed to serialize PDF with header/footer")?;
        drop(doc);
        Self::write_atomic(output_path.as_ref(), &bytes)
            .context("Failed to save PDF with header/footer")
    }

    /// Stamp a diagonal or centered semi-transparent Text Watermark (`add_watermark`) across pages.
    pub fn add_watermark<P: AsRef<Path>>(
        &self,
        input_path: P,
        output_path: P,
        watermark_text: &str,
        diagonal: bool,
    ) -> Result<()> {
        let mut doc = self
            .load_pdf_file(input_path.as_ref())
            .context("Failed to load PDF for watermark")?;
        Self::apply_watermark_to_doc(&mut doc, watermark_text, diagonal)?;
        let bytes = doc
            .save_to_bytes()
            .context("Failed to serialize watermarked PDF")?;
        drop(doc);
        Self::write_atomic(output_path.as_ref(), &bytes)
            .context("Failed to save watermarked PDF")
    }

    /// Compresses and optimizes a PDF byte buffer (`compress_pdf(input_bytes) -> Result<Vec<u8>>`):
    /// - Parses the PDF structure via `lopdf::Document::load_mem`
    /// - Refuses encrypted PDFs so per-object RC4/AES keys are never invalidated by renumbering
    /// - Prunes unused/orphaned objects (`prune_objects`)
    /// - Deletes zero-length streams (`delete_zero_length_streams`)
    /// - Renumbers objects compactly (`renumber_objects`)
    /// - Compresses uncompressed streams (`compress`)
    pub fn compress_pdf(input_bytes: &[u8]) -> Result<Vec<u8>> {
        let mut ldoc = lopdf::Document::load_mem(input_bytes)
            .context("Failed to parse PDF structure with lopdf for compression")?;
        if ldoc.is_encrypted() {
            anyhow::bail!("Cannot structurally compress an encrypted PDF without decrypting it first");
        }
        ldoc.prune_objects();
        ldoc.delete_zero_length_streams();
        ldoc.renumber_objects();
        ldoc.compress();

        let mut compressed = Vec::with_capacity(input_bytes.len());
        ldoc.save_to(&mut compressed)
            .context("Failed to serialize compressed PDF via lopdf")?;
        Ok(compressed)
    }

    /// Compresses the active `PdfDocument` using `lopdf` object pruning and stream compression.
    ///
    /// v1.2: The `clean_doc.pages_mut().append(doc)` rebuild branch was removed because copying
    /// only pages into a blank PDFium document drops catalog-level `/Outlines` (bookmarks),
    /// `/PageLabels`, and `/AcroForm` (form fields).
    /// Returns `(compressed_bytes, original_size_bytes, compressed_size_bytes)`.
    pub fn compress_doc(&self, doc: &PdfDocument) -> Result<(Vec<u8>, usize, usize)> {
        let raw_bytes = doc
            .save_to_bytes()
            .context("Failed to serialize active PDF document")?;
        let orig_len = raw_bytes.len();

        let mut best_bytes = raw_bytes.clone();

        // Try lopdf structural pruning + deflate stream compression on raw_bytes
        if let Ok(lopdf_out) = Self::compress_pdf(&raw_bytes) {
            if !lopdf_out.is_empty() && lopdf_out.len() < best_bytes.len() {
                if self.pdfium.load_pdf_from_byte_vec(lopdf_out.clone(), None).is_ok() {
                    best_bytes = lopdf_out;
                }
            }
        }

        let new_len = best_bytes.len();
        Ok((best_bytes, orig_len, new_len))
    }

    /// Computes the union bounding box `(min_x, min_y, max_x, max_y)` in PDF points of all
    /// content objects (`page.objects().iter()`) on `page` for `FitTextWidth` mode (`3` key).
    /// Falls back to a 12% left/right margin trim if the page has no detectable content bounds.
    pub fn compute_page_content_bounds(page: &PdfPage) -> (f32, f32, f32, f32) {
        let page_w = page.width().value.max(1.0);
        let page_h = page.height().value.max(1.0);

        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        let mut found = false;

        for obj in page.objects().iter() {
            if let Ok(b) = obj.bounds() {
                let left = b.left().value.min(b.right().value).max(0.0);
                let right = b.left().value.max(b.right().value).min(page_w);
                let bottom = b.bottom().value.min(b.top().value).max(0.0);
                let top = b.bottom().value.max(b.top().value).min(page_h);

                let w = right - left;
                let h = top - bottom;
                if w < 1.0 || h < 1.0 {
                    continue;
                }
                // Skip full-page background paths/whiteout boxes that span >95% of the page width
                if obj.object_type() == PdfPageObjectType::Path && w >= page_w * 0.95 {
                    continue;
                }
                min_x = min_x.min(left);
                min_y = min_y.min(bottom);
                max_x = max_x.max(right);
                max_y = max_y.max(top);
                found = true;
            }
        }

        if found && max_x > min_x && (max_x - min_x) >= page_w * 0.22 {
            // Add a small 8pt breathing room on left/right/top/bottom of the detected content
            let pad = 8.0_f32;
            let cl_min_x = (min_x - pad).max(0.0);
            let cl_max_x = (max_x + pad).min(page_w);
            let cl_min_y = (min_y - pad).clamp(0.0, page_h);
            let cl_max_y = (max_y + pad).clamp(cl_min_y + 1.0, page_h);
            (cl_min_x, cl_min_y, cl_max_x, cl_max_y)
        } else {
            (page_w * 0.12, page_h * 0.08, page_w * 0.88, page_h * 0.92)
        }
    }

    /// Enhanced content bounds for `FitTextWidth`: uses vector object bounds first, and falls back
    /// to a fast low-res (~220px wide) non-white pixel scan when the page is a full-page scan image.
    pub fn compute_page_content_bounds_with_scan(page: &PdfPage) -> (f32, f32, f32, f32) {
        let page_w = page.width().value.max(1.0);
        let page_h = page.height().value.max(1.0);
        let (min_x, min_y, max_x, max_y) = Self::compute_page_content_bounds(page);

        let looks_full_page = (max_x - min_x) > page_w * 0.90 || (max_y - min_y) > page_h * 0.92;
        if !looks_full_page {
            return (min_x, min_y, max_x, max_y);
        }

        let scan_w = 220i32;
        let scan_h = ((scan_w as f32 * (page_h / page_w)).round() as i32).clamp(32, 440);
        let cfg = PdfRenderConfig::new()
            .set_target_width(scan_w)
            .set_maximum_height(scan_h)
            .render_form_data(true)
            .render_annotations(true);
        let Ok(bmp) = page.render_with_config(&cfg) else {
            return (min_x, min_y, max_x, max_y);
        };
        let img = bmp.as_image().to_rgba8();
        let (iw, ih) = img.dimensions();
        if iw < 8 || ih < 8 {
            return (min_x, min_y, max_x, max_y);
        }

        let mut p_min_x = iw;
        let mut p_max_x = 0u32;
        let mut p_min_y = ih;
        let mut p_max_y = 0u32;
        let mut ink = 0usize;
        for y in 0..ih {
            for x in 0..iw {
                let px = img.get_pixel(x, y);
                if px[0] < 242 || px[1] < 242 || px[2] < 242 {
                    p_min_x = p_min_x.min(x);
                    p_max_x = p_max_x.max(x);
                    p_min_y = p_min_y.min(y);
                    p_max_y = p_max_y.max(y);
                    ink += 1;
                }
            }
        }
        if ink < 16 || p_max_x <= p_min_x || p_max_y <= p_min_y {
            return (min_x, min_y, max_x, max_y);
        }

        let pad = 8.0f32;
        let sx = page_w / iw as f32;
        let sy = page_h / ih as f32;
        let bx0 = (p_min_x as f32 * sx - pad).max(0.0);
        let bx1 = ((p_max_x + 1) as f32 * sx + pad).min(page_w);
        // Image y=0 is top of page (PDF y = page_h)
        let by_top = (page_h - p_min_y as f32 * sy + pad).min(page_h);
        let by_bot = (page_h - (p_max_y + 1) as f32 * sy - pad).max(0.0);
        if (bx1 - bx0) >= page_w * 0.20 && by_top > by_bot {
            (bx0, by_bot, bx1, by_top)
        } else {
            (min_x, min_y, max_x, max_y)
        }
    }

    /// Combines 2 or more PDF files in order into a single in-memory `Vec<u8>` buffer (`Shift+M`).
    /// Returns `(combined_pdf_bytes, total_pages)`.
    #[allow(dead_code)]
    pub fn combine_multiple_pdfs(&self, paths: &[PathBuf]) -> Result<(Vec<u8>, u16)> {
        if paths.is_empty() {
            anyhow::bail!("No PDF files provided to combine");
        }
        let mut combined_doc = self
            .pdfium
            .create_new_pdf()
            .context("Failed to create combined PDF document")?;

        for path in paths {
            let src_doc = self.load_pdf_safe(path)?;
            combined_doc
                .pages_mut()
                .append(&src_doc)
                .with_context(|| format!("Failed to append pages from {:?}", path))?;
        }

        let total_pages = combined_doc.pages().len();
        let bytes = combined_doc
            .save_to_bytes()
            .context("Failed to serialize combined PDF document")?;
        Ok((bytes, total_pages))
    }

    /// Combines PDF files in exact user order, selecting only the specified 0-based page indices
    /// for each file (`None` or empty means all pages of that file).
    pub fn combine_pdfs_with_ranges(
        &self,
        entries: &[(PathBuf, Option<Vec<u16>>)],
    ) -> Result<(Vec<u8>, u16)> {
        if entries.is_empty() {
            anyhow::bail!("No PDF files queued to combine");
        }
        let mut combined_doc = self
            .pdfium
            .create_new_pdf()
            .context("Failed to create combined PDF document")?;

        for (path, custom_pages) in entries {
            let src_doc = self.load_pdf_safe(path)?;
            let src_len = src_doc.pages().len();
            match custom_pages {
                None => {
                    combined_doc
                        .pages_mut()
                        .append(&src_doc)
                        .with_context(|| format!("Failed to append pages from {:?}", path))?;
                }
                Some(indices) => {
                    for &p_idx in indices {
                        if p_idx < src_len {
                            let dest_idx = combined_doc.pages().len();
                            combined_doc
                                .pages_mut()
                                .copy_page_from_document(&src_doc, p_idx, dest_idx)
                                .with_context(|| {
                                    format!("Failed to copy page {} from {:?}", p_idx + 1, path)
                                })?;
                        }
                    }
                }
            }
        }

        let total_pages = combined_doc.pages().len();
        if total_pages == 0 {
            anyhow::bail!("Combined PDF has 0 pages");
        }
        let bytes = combined_doc
            .save_to_bytes()
            .context("Failed to serialize combined PDF document")?;
        Ok((bytes, total_pages))
    }

    /// Searches `page.text()` on a single page `page_idx` (case-insensitive) against pre-lowercased
    /// `q_chars` and returns all `SearchMatch` entries found on that page.
    /// Does not retain any per-page character buffer after returning.
    pub fn search_text_on_single_page(
        doc: &PdfDocument,
        page_idx: u16,
        q_chars: &[char],
    ) -> Vec<SearchMatch> {
        if q_chars.is_empty() {
            return Vec::new();
        }
        Self::search_text_on_single_page_opts(doc, page_idx, q_chars, false, false)
    }

    /// Pure-Rust character-stream matcher supporting optional `case_sensitive` and `whole_word`
    /// matching (with whitespace equivalence). Returns non-overlapping `(start, end_excl)` ranges
    /// in `haystack`.
    pub fn find_text_match_ranges(
        haystack: &[char],
        query: &[char],
        whole_word: bool,
    ) -> Vec<(usize, usize)> {
        if query.is_empty() || haystack.len() < query.len() {
            return Vec::new();
        }
        let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
        let mut ranges = Vec::new();
        let mut i = 0;
        while i + query.len() <= haystack.len() {
            let mut matched = true;
            for j in 0..query.len() {
                let pc = haystack[i + j];
                let qc = query[j];
                if pc != qc && !(pc.is_whitespace() && qc.is_whitespace()) {
                    matched = false;
                    break;
                }
            }
            if matched && whole_word {
                let before_ok = i == 0 || !is_word_char(haystack[i - 1]);
                let after_idx = i + query.len();
                let after_ok = after_idx >= haystack.len() || !is_word_char(haystack[after_idx]);
                if !(before_ok && after_ok) {
                    matched = false;
                }
            }
            if matched {
                ranges.push((i, i + query.len()));
                i += query.len();
            } else {
                i += 1;
            }
        }
        ranges
    }

    /// Searches a single page for `q_chars`, collapsing `\r\n` / consecutive line breaks into a
    /// single space so phrases wrapped across lines match cleanly. Supports optional
    /// `case_sensitive` and `whole_word` matching.
    pub fn search_text_on_single_page_opts(
        doc: &PdfDocument,
        page_idx: u16,
        q_chars: &[char],
        case_sensitive: bool,
        whole_word: bool,
    ) -> Vec<SearchMatch> {
        if q_chars.is_empty() {
            return Vec::new();
        }
        let Ok(page) = doc.pages().get(page_idx) else {
            return Vec::new();
        };
        let Ok(text_page) = page.text() else {
            return Vec::new();
        };

        let mut page_chars: Vec<(char, Option<PdfRect>)> = Vec::new();
        for ch in text_page.chars().iter() {
            let Some(uc) = ch.unicode_char() else {
                continue;
            };
            if uc == '\r' || uc == '\n' {
                // Collapse `\r\n` and consecutive line breaks into a single space
                if page_chars.last().map(|(c, _)| *c) != Some(' ') {
                    page_chars.push((' ', None));
                }
                continue;
            }
            if uc.is_control() {
                continue;
            }
            let rect_opt = ch
                .loose_bounds()
                .or_else(|_| ch.tight_bounds())
                .ok()
                .and_then(|b| {
                    let bottom = b.bottom().value.min(b.top().value);
                    let top = b.bottom().value.max(b.top().value);
                    let left = b.left().value.min(b.right().value);
                    let right = b.left().value.max(b.right().value);
                    if (right - left) >= 0.1 && (top - bottom) >= 0.1 {
                        Some(PdfRect::new_from_values(bottom, left, top, right))
                    } else {
                        None
                    }
                });
            if case_sensitive {
                page_chars.push((uc, rect_opt));
            } else {
                for lc in uc.to_lowercase() {
                    page_chars.push((lc, rect_opt));
                }
            }
        }

        let norm_query: Vec<char> = if case_sensitive {
            q_chars.to_vec()
        } else {
            q_chars.iter().flat_map(|c| c.to_lowercase()).collect()
        };
        let plain: Vec<char> = page_chars.iter().map(|(c, _)| *c).collect();
        let ranges = Self::find_text_match_ranges(&plain, &norm_query, whole_word);

        let mut matches = Vec::new();
        for (start, end) in ranges {
            let mut line_rects: Vec<PdfRect> = Vec::new();
            for (_, rect_opt) in &page_chars[start..end] {
                if let Some(cb) = *rect_opt {
                    if let Some(last) = line_rects.last_mut() {
                        let last_mid_y = (last.bottom().value + last.top().value) * 0.5;
                        let cb_mid_y = (cb.bottom().value + cb.top().value) * 0.5;
                        let lh = (last.top().value - last.bottom().value).max(4.0);
                        if (last_mid_y - cb_mid_y).abs() < lh * 0.65 {
                            *last = PdfRect::new_from_values(
                                last.bottom().value.min(cb.bottom().value),
                                last.left().value.min(cb.left().value),
                                last.top().value.max(cb.top().value),
                                last.right().value.max(cb.right().value),
                            );
                            continue;
                        }
                    }
                    line_rects.push(cb);
                }
            }

            if !line_rects.is_empty() {
                matches.push(SearchMatch {
                    page_index: page_idx,
                    rects: line_rects,
                });
            }
        }

        matches
    }

    /// Searches `page.text()` across all pages in `doc` (case-insensitive) and returns all matches
    /// with their line-level character bounding boxes (`SearchMatch`).
    pub fn search_text_in_doc(doc: &PdfDocument, query: &str) -> Vec<SearchMatch> {
        let q_chars: Vec<char> = query
            .trim()
            .chars()
            .flat_map(|c| c.to_lowercase())
            .collect();
        if q_chars.is_empty() {
            return Vec::new();
        }

        let mut matches = Vec::new();
        let total = doc.pages().len();

        for page_idx in 0..total {
            matches.extend(Self::search_text_on_single_page(doc, page_idx, &q_chars));
        }

        matches
    }

    /// Black-box redaction (`Mouse Mode: Redact`), v1.1.
    ///
    /// Everything that overlaps the selection is dealt with, failing closed:
    /// * text, shading, form XObjects and unsupported objects: removed;
    /// * images: pixels under the box are painted black inside the image itself; if the image
    ///   is rotated/skewed or cannot be decoded, the whole image is removed instead;
    /// * paths: removed, except simple shapes (<= 5 segments) that fully enclose the box, such
    ///   as page or table backgrounds, which carry no hidden content;
    /// * annotations (links, comments, form widgets) whose rectangle overlaps: deleted.
    /// A solid black rectangle is then drawn over the area.
    ///
    /// Save with `redacted = true` afterwards so orphaned objects are pruned from the file.
    /// Returns the number of objects/annotations removed or modified.
    pub fn redact_rect_on_page(
        doc: &mut PdfDocument,
        page_index: u16,
        pdf_rect: PdfRect,
    ) -> Result<usize> {
        let sel_bottom = pdf_rect.bottom().value.min(pdf_rect.top().value);
        let sel_top = pdf_rect.bottom().value.max(pdf_rect.top().value);
        let sel_left = pdf_rect.left().value.min(pdf_rect.right().value);
        let sel_right = pdf_rect.left().value.max(pdf_rect.right().value);

        if (sel_right - sel_left) < 2.0 && (sel_top - sel_bottom) < 2.0 {
            return Ok(0);
        }

        let normalized_sel = PdfRect::new_from_values(sel_bottom, sel_left, sel_top, sel_right);
        let overlaps = |l: f32, b: f32, r: f32, t: f32| {
            l <= sel_right && r >= sel_left && b <= sel_top && t >= sel_bottom
        };

        let mut page = doc
            .pages_mut()
            .get(page_index)
            .context("Page index out of bounds for redaction")?;

        let prev_strategy = page.content_regeneration_strategy();
        page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);

        // 1. Classify every overlapping page object.
        let mut to_remove: Vec<usize> = Vec::new();
        let mut to_mask: Vec<(usize, (u32, u32, u32, u32))> = Vec::new();
        let obj_count = page.objects().len();
        for i in 0..obj_count {
            let Ok(obj) = page.objects().get(i) else {
                continue;
            };
            let Ok(q) = obj.bounds() else {
                // Unknown geometry: cannot prove it is outside the box, so remove it.
                to_remove.push(i);
                continue;
            };
            let (l, r) = (q.left().value.min(q.right().value), q.left().value.max(q.right().value));
            let (b, t) = (q.bottom().value.min(q.top().value), q.bottom().value.max(q.top().value));
            if !overlaps(l, b, r, t) {
                continue;
            }
            match obj.object_type() {
                PdfPageObjectType::Image => {
                    match obj
                        .as_image_object()
                        .and_then(|io| Self::image_mask_region(io, sel_left, sel_bottom, sel_right, sel_top))
                    {
                        Some(region) => to_mask.push((i, region)),
                        None => to_remove.push(i),
                    }
                }
                PdfPageObjectType::Path => {
                    let simple = obj
                        .as_path_object()
                        .map(|p| p.segments().len() <= 5)
                        .unwrap_or(false);
                    let encloses = l <= sel_left && r >= sel_right && b <= sel_bottom && t >= sel_top;
                    if !(simple && encloses) {
                        to_remove.push(i);
                    }
                }
                _ => to_remove.push(i),
            }
        }

        // 2. Burn black pixels into overlapping images (indices are still valid here).
        let mut touched = 0usize;
        for (idx, (x0, y0, x1, y1)) in &to_mask {
            let mut masked = false;
            if let Ok(mut obj) = page.objects().get(*idx) {
                if let Some(io) = obj.as_image_object_mut() {
                    if let Ok(img) = io.get_raw_image() {
                        let mut rgba = img.to_rgba8();
                        for y in *y0..(*y1).min(rgba.height()) {
                            for x in *x0..(*x1).min(rgba.width()) {
                                rgba.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
                            }
                        }
                        masked = io
                            .set_image(&image::DynamicImage::ImageRgba8(rgba))
                            .is_ok();
                    }
                }
            }
            if masked {
                touched += 1;
            } else {
                to_remove.push(*idx);
            }
        }

        // 3. Remove objects in descending index order so earlier indices stay valid.
        to_remove.sort_unstable();
        to_remove.dedup();
        for &idx in to_remove.iter().rev() {
            if let Ok(obj) = page.objects_mut().remove_object_at_index(idx) {
                // Dropping a detached object here segfaults with pdfium-render 0.8.37 (verified
                // in testing), so it is intentionally leaked. Revisit when upgrading the crate.
                std::mem::forget(obj);
                touched += 1;
            }
        }

        // 4. Delete overlapping annotations (links, notes, form widgets).
        let annot_count = page.annotations().len();
        for i in (0..annot_count).rev() {
            let Ok(annot) = page.annotations().get(i) else {
                continue;
            };
            let hit = annot
                .bounds()
                .map(|rb| {
                    let (l, r) = (rb.left().value.min(rb.right().value), rb.left().value.max(rb.right().value));
                    let (b, t) = (rb.bottom().value.min(rb.top().value), rb.bottom().value.max(rb.top().value));
                    overlaps(l, b, r, t)
                })
                .unwrap_or(true);
            if hit && page.annotations_mut().delete_annotation(annot).is_ok() {
                touched += 1;
            }
        }

        let black_rect_res = page.objects_mut().create_path_object_rect(
            normalized_sel,
            None,
            None,
            Some(PdfColor::BLACK),
        );

        let regen_res = page.regenerate_content();
        page.set_content_regeneration_strategy(prev_strategy);

        black_rect_res.context("Failed to draw black redaction rectangle")?;
        regen_res.context("Failed to regenerate page content after redaction")?;

        Ok(touched)
    }

    /// Maps the selection into an image's own pixel grid, returning `(x0, y0, x1, y1)` with a
    /// 1-pixel safety margin. Returns `None` (caller removes the image) when the image is
    /// rotated or skewed, or its size cannot be read.
    fn image_mask_region(
        io: &PdfPageImageObject,
        sel_left: f32,
        sel_bottom: f32,
        sel_right: f32,
        sel_top: f32,
    ) -> Option<(u32, u32, u32, u32)> {
        let m = io.matrix().ok()?;
        let (a, b, c, d, e, f) = (m.a(), m.b(), m.c(), m.d(), m.e(), m.f());
        if b.abs() > 1e-3 || c.abs() > 1e-3 || a.abs() < 1e-6 || d.abs() < 1e-6 {
            return None;
        }
        let w = io.width().ok()? as f32;
        let h = io.height().ok()? as f32;
        if w < 1.0 || h < 1.0 {
            return None;
        }
        // Image space: u,v in [0,1]; x = a*u + e, y = d*v + f; pixel row 0 is the top (v = 1).
        let (u_a, u_b) = ((sel_left - e) / a, (sel_right - e) / a);
        let (v_a, v_b) = ((sel_bottom - f) / d, (sel_top - f) / d);
        let (u0, u1) = (u_a.min(u_b).clamp(0.0, 1.0), u_a.max(u_b).clamp(0.0, 1.0));
        let (v0, v1) = (v_a.min(v_b).clamp(0.0, 1.0), v_a.max(v_b).clamp(0.0, 1.0));
        let x0 = ((u0 * w).floor() - 1.0).max(0.0) as u32;
        let x1 = ((u1 * w).ceil() + 1.0).min(w) as u32;
        let y0 = (((1.0 - v1) * h).floor() - 1.0).max(0.0) as u32;
        let y1 = (((1.0 - v0) * h).ceil() + 1.0).min(h) as u32;
        Some((x0, y0, x1, y1))
    }

    /// Crops `page_index` (0-based) to `crop_rect` by setting `/CropBox` (while preserving `/MediaBox`
    /// so `reset_page_crop` can restore the original full page bounds).
    /// Returns serialized PDF bytes so reloading updates PDFium's cached page dimensions immediately.
    pub fn crop_page_to_rect(
        &self,
        doc: &mut PdfDocument,
        page_index: u16,
        crop_rect: PdfRect,
    ) -> Result<Vec<u8>> {
        let mut bottom = crop_rect.bottom().value.min(crop_rect.top().value);
        let mut top = crop_rect.bottom().value.max(crop_rect.top().value);
        let mut left = crop_rect.left().value.min(crop_rect.right().value);
        let mut right = crop_rect.left().value.max(crop_rect.right().value);

        if (right - left) < 10.0 || (top - bottom) < 10.0 {
            anyhow::bail!("Crop area too small (minimum 10x10 pt)");
        }

        {
            let mut page = doc
                .pages_mut()
                .get(page_index)
                .context("Page index out of bounds for cropping")?;

            // Ensure MediaBox is explicitly set if not already present so Reset Page Crop always knows the full bounds
            if page.boundaries().media().is_err() {
                let full_rect = PdfRect::new_from_values(
                    0.0,
                    0.0,
                    page.height().value,
                    page.width().value,
                );
                let _ = page.boundaries_mut().set_media(full_rect);
            }

            // If the page already has a CropBox offset, `pixels_to_points` in PDFium is relative to
            // the current CropBox origin `(cur_left, cur_bottom)` when `left >= 0 && right <= page.width()`.
            if let Ok(existing_crop) = page.boundaries().crop() {
                let c_left = existing_crop.bounds.left().value;
                let c_bottom = existing_crop.bounds.bottom().value;
                if (c_left.abs() > 0.5 || c_bottom.abs() > 0.5)
                    && right <= page.width().value + 1.0
                    && top <= page.height().value + 1.0
                {
                    left += c_left;
                    right += c_left;
                    bottom += c_bottom;
                    top += c_bottom;
                }
            }

            let normalized = PdfRect::new_from_values(bottom, left, top, right);
            page.boundaries_mut()
                .set_crop(normalized)
                .context("Failed to set page CropBox")?;
        }

        doc.save_to_bytes()
            .context("Failed to serialize PDF after setting CropBox")
    }

    /// Resets the `/CropBox` on `page_index` (0-based) back to the page's full `/MediaBox`.
    pub fn reset_page_crop(&self, doc: &mut PdfDocument, page_index: u16) -> Result<Vec<u8>> {
        {
            let mut page = doc
                .pages_mut()
                .get(page_index)
                .context("Page index out of bounds for resetting crop")?;
            let media_rect = page
                .boundaries()
                .media()
                .map(|b| b.bounds)
                .unwrap_or_else(|_| {
                    PdfRect::new_from_values(
                        0.0,
                        0.0,
                        page.height().value.max(10.0),
                        page.width().value.max(10.0),
                    )
                });
            page.boundaries_mut()
                .set_crop(media_rect)
                .context("Failed to reset page CropBox to MediaBox")?;
        }
        doc.save_to_bytes()
            .context("Failed to serialize PDF after resetting CropBox")
    }

    /// Decodes a PNG or JPEG image file from `path` into an RGBA `StampImageBuffer`, preserving PNG alpha transparency.
    /// Downscales any image over 2,000 px on its longest side so a large photo stamp never explodes RAM.
    pub fn load_stamp_image(path: &Path) -> Result<StampImageBuffer> {
        let mut dyn_img = image::open(path)
            .with_context(|| format!("Failed to decode image file {:?}", path))?;
        let (orig_w, orig_h) = (dyn_img.width(), dyn_img.height());
        if orig_w == 0 || orig_h == 0 {
            anyhow::bail!("Image has zero width or height");
        }
        const MAX_STAMP_SIDE: u32 = 2000;
        if orig_w.max(orig_h) > MAX_STAMP_SIDE {
            dyn_img = dyn_img.resize(
                MAX_STAMP_SIDE,
                MAX_STAMP_SIDE,
                image::imageops::FilterType::Triangle,
            );
        }
        let rgba_img = dyn_img.into_rgba8();
        let (width, height) = rgba_img.dimensions();
        Ok(StampImageBuffer {
            width,
            height,
            image: image::DynamicImage::ImageRgba8(rgba_img),
        })
    }

    /// Stamps `stamp` (preserving PNG alpha transparency via `image::DynamicImage` + `PdfPageImageObject`)
    /// onto `page_index` inside `target_rect` in PDF coordinates without copying the decoded buffer.
    pub fn stamp_image_on_page(
        &self,
        doc: &mut PdfDocument,
        page_index: u16,
        stamp: &StampImageBuffer,
        target_rect: PdfRect,
    ) -> Result<()> {
        let left = target_rect.left().value.min(target_rect.right().value);
        let right = target_rect.left().value.max(target_rect.right().value);
        let bottom = target_rect.bottom().value.min(target_rect.top().value);
        let top = target_rect.bottom().value.max(target_rect.top().value);
        let rect_w = (right - left).max(8.0);
        let rect_h = (top - bottom).max(8.0);

        let mut img_obj = PdfPageImageObject::new_with_size(
            doc,
            &stamp.image,
            PdfPoints::new(rect_w),
            PdfPoints::new(rect_h),
        )
        .context("Failed to create PdfPageImageObject from DynamicImage")?;

        img_obj
            .translate(PdfPoints::new(left), PdfPoints::new(bottom))
            .context("Failed to translate PdfPageImageObject")?;

        let mut page = doc
            .pages_mut()
            .get(page_index)
            .context("Page index out of bounds for stamping image")?;
        page.objects_mut()
            .add_image_object(img_obj)
            .context("Failed to add image object to PDF page")?;
        page.regenerate_content()
            .context("Failed to regenerate page content after image stamp")?;

        Ok(())
    }

    /// Flattens annotations/form fields on every page and strips encryption/permission
    /// dictionaries safely. Only removes `/Encrypt` after verifying streams are decrypted
    /// (attempting empty user password for owner-restricted PDFs) so encrypted streams are
    /// never silently corrupted into blank pages.
    pub fn flatten_and_unlock_doc(&self, doc: &mut PdfDocument) -> Result<FlattenReport> {
        let total = doc.pages().len();
        let mut failed_pages = Vec::new();
        for idx in 0..total {
            let ok = match doc.pages_mut().get(idx) {
                Ok(mut page) => page.flatten().is_ok(),
                Err(_) => false,
            };
            if !ok {
                failed_pages.push(idx + 1);
            }
        }

        let flattened_bytes = doc
            .save_to_bytes()
            .context("Failed to serialize flattened PDF")?;

        let unlocked = (|| -> Result<Vec<u8>> {
            let mut ldoc = lopdf::Document::load_mem(&flattened_bytes)?;
            if ldoc.is_encrypted() {
                // Only strip /Encrypt if lopdf actually decrypts all streams with empty user password
                ldoc.decrypt("")
                    .context("Document requires password decryption before removing /Encrypt")?;
                if ldoc.is_encrypted() {
                    anyhow::bail!("Document remained encrypted after decrypt attempt");
                }
            }
            ldoc.trailer.remove(b"Encrypt");
            if let Ok(cat) = ldoc.catalog_mut() {
                cat.remove(b"AcroForm");
                cat.remove(b"Perms");
            }
            let mut out = Vec::with_capacity(flattened_bytes.len());
            ldoc.save_to(&mut out)?;
            if out.is_empty() {
                anyhow::bail!("empty output");
            }
            // Verify PDFium can still open the result before trusting it.
            self.load_pdf_from_bytes_safe(out.clone())?;
            Ok(out)
        })();

        Ok(match unlocked {
            Ok(bytes) => FlattenReport { bytes, failed_pages, permissions_removed: true },
            Err(_) => FlattenReport {
                bytes: flattened_bytes,
                failed_pages,
                permissions_removed: false,
            },
        })
    }

    /// Converts a positive integer into lowercase Roman numerals (e.g. 1 -> "i", 4 -> "iv").
    pub fn to_roman(mut num: u16) -> String {
        let numerals = [
            (1000, "m"),
            (900, "cm"),
            (500, "d"),
            (400, "cd"),
            (100, "c"),
            (90, "xc"),
            (50, "l"),
            (40, "xl"),
            (10, "x"),
            (9, "ix"),
            (5, "v"),
            (4, "iv"),
            (1, "i"),
        ];
        let mut result = String::new();
        for &(val, sym) in &numerals {
            while num >= val {
                result.push_str(sym);
                num -= val;
            }
        }
        result
    }

    // =========================================================================
    // Tier 1.2: Interactive Annotation Inspector & Editor Helpers
    // =========================================================================

    /// Finds the top-most editable/deletable markup annotation at `(pdf_x, pdf_y)` on `page`.
    /// Skips interactive form `Widget` and `Link` annotations so form filling and links stay untouched.
    pub fn find_annotation_at_point(
        page: &PdfPage,
        pdf_x: f32,
        pdf_y: f32,
    ) -> Option<FoundAnnotationInfo> {
        let annots: Vec<_> = page.annotations().iter().enumerate().collect();
        for (idx, annot) in annots.into_iter().rev() {
            let atype = annot.annotation_type();
            if matches!(
                atype,
                PdfPageAnnotationType::Link
                    | PdfPageAnnotationType::Widget
                    | PdfPageAnnotationType::Popup
            ) {
                continue;
            }
            let Ok(bounds) = annot.bounds() else {
                continue;
            };
            let min_x = bounds.left().value.min(bounds.right().value) - 4.0;
            let max_x = bounds.left().value.max(bounds.right().value) + 4.0;
            let min_y = bounds.bottom().value.min(bounds.top().value) - 4.0;
            let max_y = bounds.bottom().value.max(bounds.top().value) + 4.0;
            if pdf_x < min_x || pdf_x > max_x || pdf_y < min_y || pdf_y > max_y {
                continue;
            }
            let creator = annot.creator().unwrap_or_default();
            let (label, is_text_editable) = match atype {
                PdfPageAnnotationType::Text => ("Sticky Note", true),
                PdfPageAnnotationType::FreeText => ("Text Box", true),
                PdfPageAnnotationType::Highlight => ("Highlight", false),
                PdfPageAnnotationType::Underline => ("Underline", false),
                PdfPageAnnotationType::Strikeout => ("Strikethrough", false),
                PdfPageAnnotationType::Ink => ("Ink Pen", false),
                PdfPageAnnotationType::Square => ("Rectangle", false),
                PdfPageAnnotationType::Circle => ("Circle", false),
                PdfPageAnnotationType::Line => ("Line", false),
                PdfPageAnnotationType::Stamp => match creator.as_str() {
                    "MicroPDF:StickyNote" => ("Sticky Note", true),
                    "MicroPDF:FreeText" => ("Text Box", true),
                    "MicroPDF:Highlight" => ("Highlight", false),
                    "MicroPDF:Underline" => ("Underline", false),
                    "MicroPDF:Strikeout" => ("Strikethrough", false),
                    "MicroPDF:Ink" => ("Ink Pen", false),
                    "MicroPDF:Rect" => ("Rectangle", false),
                    "MicroPDF:Arrow" => ("Arrow", false),
                    _ => ("Stamp", false),
                },
                _ => ("Annotation", false),
            };
            return Some(FoundAnnotationInfo {
                index: idx,
                label,
                is_text_editable,
                contents: annot.contents().unwrap_or_default().trim().to_string(),
            });
        }
        None
    }

    /// Deletes the annotation at `annot_index` on `page_index`.
    pub fn delete_annotation_at(
        doc: &mut PdfDocument,
        page_index: u16,
        annot_index: usize,
    ) -> Result<()> {
        let mut page = doc.pages_mut().get(page_index)?;
        let annot = page.annotations_mut().get(annot_index)?;
        page.annotations_mut().delete_annotation(annot)?;
        Ok(())
    }

    /// Updates the text contents of a Sticky Note or Text Box annotation at `annot_index`.
    /// For `MicroPDF:FreeText` stamps, rebuilds the visible text box at the same anchor so width/height reflows cleanly.
    pub fn update_annotation_contents(
        doc: &mut PdfDocument,
        page_index: u16,
        annot_index: usize,
        new_text: &str,
        fallback_color: AnnotationColor,
    ) -> Result<()> {
        let trimmed = new_text.trim();
        if trimmed.is_empty() {
            anyhow::bail!("Annotation text cannot be empty");
        }
        let recreate_freetext: Option<(f32, f32, PdfColor)> = {
            let page = doc.pages().get(page_index)?;
            let annot = page.annotations().get(annot_index)?;
            let creator = annot.creator().unwrap_or_default();
            if annot.annotation_type() == PdfPageAnnotationType::Stamp
                && creator == "MicroPDF:FreeText"
            {
                let bounds = annot.bounds()?;
                let anchor_x = bounds.left().value.min(bounds.right().value) + 3.0;
                let anchor_y = (bounds.top().value.max(bounds.bottom().value) - 14.0).max(8.0);
                let color = annot
                    .as_stamp_annotation()
                    .and_then(|s| {
                        s.objects()
                            .iter()
                            .find_map(|o| o.fill_color().ok())
                    })
                    .unwrap_or_else(|| fallback_color.stroke_pdf_color());
                Some((anchor_x, anchor_y, color))
            } else {
                None
            }
        };

        if let Some((ax, ay, color)) = recreate_freetext {
            Self::delete_annotation_at(doc, page_index, annot_index)?;
            let font = doc.fonts_mut().helvetica();
            let fsz = 12.0f32;
            let lines: Vec<&str> = trimmed.split('\n').collect();
            let line_h = fsz * 1.25;
            let mut text_objs = Vec::with_capacity(lines.len());
            let mut min_x = ax.max(4.0);
            let mut max_x = min_x + 40.0;
            let mut min_y = (ay - fsz).max(4.0);
            let mut max_y = ay + fsz * 0.9;
            for (i, line) in lines.iter().enumerate() {
                let mut tobj = PdfPageTextObject::new(doc, *line, font, PdfPoints::new(fsz))?;
                let _ = tobj.set_fill_color(color);
                let y = (ay - (i as f32) * line_h).max(8.0);
                tobj.translate(PdfPoints::new(ax.max(4.0)), PdfPoints::new(y))?;
                if let Ok(b) = tobj.bounds() {
                    min_x = min_x.min(b.left().value);
                    max_x = max_x.max(b.right().value);
                    min_y = min_y.min(b.bottom().value);
                    max_y = max_y.max(b.top().value);
                }
                text_objs.push(tobj);
            }
            let bounds = PdfRect::new_from_values(
                (min_y - 3.0).max(2.0),
                (min_x - 3.0).max(2.0),
                max_y + 3.0,
                max_x + 4.0,
            );
            let mut page = doc.pages_mut().get(page_index)?;
            let mut annot = page.annotations_mut().create_stamp_annotation()?;
            let _ = annot.set_is_printed(true);
            let _ = annot.set_bounds(bounds);
            let _ = annot.set_creator("MicroPDF:FreeText");
            let _ = annot.set_contents(trimmed);
            for tobj in text_objs {
                annot.objects_mut().add_text_object(tobj)?;
            }
            return Ok(());
        }

        let mut page = doc.pages_mut().get(page_index)?;
        let mut annot = page.annotations_mut().get(annot_index)?;
        annot.set_contents(trimmed)?;
        Ok(())
    }

    /// Recolors an annotation at `annot_index` on `page_index` to `color`.
    pub fn recolor_annotation_at(
        doc: &mut PdfDocument,
        page_index: u16,
        annot_index: usize,
        color: AnnotationColor,
    ) -> Result<()> {
        let mut page = doc.pages_mut().get(page_index)?;
        let mut annot = page.annotations_mut().get(annot_index)?;
        let creator = annot.creator().unwrap_or_default();
        let (r, g, b) = color.rgb();
        let stroke = color.stroke_pdf_color();
        let highlight = color.highlight_pdf_color();

        if let Some(stamp) = annot.as_stamp_annotation_mut() {
            match creator.as_str() {
                "MicroPDF:StickyNote" => {
                    let fill = PdfColor::new(r, g, b, 235);
                    if let Some(mut first) = stamp.objects_mut().iter().next() {
                        let _ = first.set_fill_color(fill);
                        let _ = first.set_stroke_color(stroke);
                    }
                }
                "MicroPDF:Highlight" => {
                    for mut obj in stamp.objects_mut().iter() {
                        let _ = obj.set_fill_color(highlight);
                    }
                }
                "MicroPDF:FreeText" => {
                    for mut obj in stamp.objects_mut().iter() {
                        let _ = obj.set_fill_color(stroke);
                    }
                }
                "MicroPDF:Underline" | "MicroPDF:Strikeout" | "MicroPDF:Ink" | "MicroPDF:Rect" => {
                    for mut obj in stamp.objects_mut().iter() {
                        let _ = obj.set_stroke_color(stroke);
                    }
                }
                "MicroPDF:Arrow" => {
                    for (i, mut obj) in stamp.objects_mut().iter().enumerate() {
                        let _ = obj.set_stroke_color(stroke);
                        if i > 0 {
                            let _ = obj.set_fill_color(stroke);
                        }
                    }
                }
                _ => {
                    for mut obj in stamp.objects_mut().iter() {
                        let _ = obj.set_stroke_color(stroke);
                    }
                }
            }
            return Ok(());
        }

        match annot.annotation_type() {
            PdfPageAnnotationType::Highlight => {
                let _ = annot.set_stroke_color(highlight);
                let _ = annot.set_fill_color(highlight);
            }
            _ => {
                let _ = annot.set_stroke_color(stroke);
                let _ = annot.set_fill_color(PdfColor::new(r, g, b, 235));
            }
        }
        Ok(())
    }

    // =========================================================================
    // Tier 1.3: Create PDF from Images & Export Pages as PNG/JPEG
    // =========================================================================

    /// Builds a clean multi-page PDF document from `images` (PNG, JPEG, BMP, WebP, GIF, TIFF)
    /// using pure Rust (`image` + `lopdf`) with zero new dependencies.
    pub fn create_pdf_from_images(
        images: &[PathBuf],
        fit_mode: ImagePdfFitMode,
    ) -> Result<Vec<u8>> {
        use flate2::Compression;
        use flate2::write::ZlibEncoder;
        use std::io::Write;

        if images.is_empty() {
            anyhow::bail!("Select at least one image to create a PDF");
        }

        let mut doc = lopdf::Document::with_version("1.4");
        let pages_id = doc.new_object_id();
        let mut page_kids: Vec<lopdf::Object> = Vec::with_capacity(images.len());

        for path in images {
            let dyn_img = image::open(path)
                .with_context(|| format!("Failed to decode image '{}'", path.display()))?;
            let rgba = dyn_img.to_rgba8();
            let (iw, ih) = rgba.dimensions();
            if iw == 0 || ih == 0 {
                anyhow::bail!("Image '{}' has zero dimensions", path.display());
            }

            // Composite any alpha over white paper so transparent PNGs never turn black in PDF.
            let mut rgb = Vec::with_capacity((iw as usize) * (ih as usize) * 3);
            for p in rgba.pixels() {
                let a = p[3] as u32;
                if a == 255 {
                    rgb.push(p[0]);
                    rgb.push(p[1]);
                    rgb.push(p[2]);
                } else {
                    let inv = 255 - a;
                    rgb.push(((p[0] as u32 * a + 255 * inv + 127) / 255) as u8);
                    rgb.push(((p[1] as u32 * a + 255 * inv + 127) / 255) as u8);
                    rgb.push(((p[2] as u32 * a + 255 * inv + 127) / 255) as u8);
                }
            }

            let is_jpeg_ext = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"));

            let (encoded_bytes, filter_name) = if is_jpeg_ext {
                let mut jpg = Vec::new();
                let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 90);
                enc.encode(&rgb, iw, ih, image::ExtendedColorType::Rgb8)
                    .context("Failed to encode JPEG stream for PDF")?;
                (jpg, b"DCTDecode".as_slice())
            } else {
                let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
                z.write_all(&rgb)?;
                let compressed = z.finish()?;
                if compressed.len() > 4 * 1024 * 1024 {
                    let mut jpg = Vec::new();
                    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 92);
                    if enc
                        .encode(&rgb, iw, ih, image::ExtendedColorType::Rgb8)
                        .is_ok()
                        && jpg.len() < compressed.len()
                    {
                        (jpg, b"DCTDecode".as_slice())
                    } else {
                        (compressed, b"FlateDecode".as_slice())
                    }
                } else {
                    (compressed, b"FlateDecode".as_slice())
                }
            };

            let mut img_dict = lopdf::Dictionary::new();
            img_dict.set("Type", lopdf::Object::Name(b"XObject".to_vec()));
            img_dict.set("Subtype", lopdf::Object::Name(b"Image".to_vec()));
            img_dict.set("Width", lopdf::Object::Integer(iw as i64));
            img_dict.set("Height", lopdf::Object::Integer(ih as i64));
            img_dict.set("ColorSpace", lopdf::Object::Name(b"DeviceRGB".to_vec()));
            img_dict.set("BitsPerComponent", lopdf::Object::Integer(8));
            img_dict.set("Filter", lopdf::Object::Name(filter_name.to_vec()));
            img_dict.set("Length", lopdf::Object::Integer(encoded_bytes.len() as i64));
            let img_id = doc.add_object(lopdf::Object::Stream(lopdf::Stream::new(
                img_dict,
                encoded_bytes,
            )));

            let (pw, ph, dx, dy, dw, dh) = match fit_mode {
                ImagePdfFitMode::FitA4 => {
                    let (w, h) = if iw > ih { (841.89f32, 595.28f32) } else { (595.28f32, 841.89f32) };
                    let margin = 24.0f32;
                    let avail_w = (w - margin * 2.0).max(72.0);
                    let avail_h = (h - margin * 2.0).max(72.0);
                    let scale = (avail_w / iw as f32).min(avail_h / ih as f32);
                    let dw = iw as f32 * scale;
                    let dh = ih as f32 * scale;
                    (w, h, (w - dw) * 0.5, (h - dh) * 0.5, dw, dh)
                }
                ImagePdfFitMode::FitLetter => {
                    let (w, h) = if iw > ih { (792.0f32, 612.0f32) } else { (612.0f32, 792.0f32) };
                    let margin = 24.0f32;
                    let avail_w = (w - margin * 2.0).max(72.0);
                    let avail_h = (h - margin * 2.0).max(72.0);
                    let scale = (avail_w / iw as f32).min(avail_h / ih as f32);
                    let dw = iw as f32 * scale;
                    let dh = ih as f32 * scale;
                    (w, h, (w - dw) * 0.5, (h - dh) * 0.5, dw, dh)
                }
                ImagePdfFitMode::MatchImageSize => {
                    let mut w = (iw as f32) * (72.0 / 96.0);
                    let mut h = (ih as f32) * (72.0 / 96.0);
                    let max_dim = 4000.0f32;
                    if w > max_dim || h > max_dim {
                        let s = max_dim / w.max(h);
                        w *= s;
                        h *= s;
                    }
                    let w = w.max(72.0);
                    let h = h.max(72.0);
                    (w, h, 0.0f32, 0.0f32, w, h)
                }
            };

            let content_ops = format!("q {dw:.2} 0 0 {dh:.2} {dx:.2} {dy:.2} cm /Im0 Do Q\n");
            let content_id = doc.add_object(lopdf::Object::Stream(lopdf::Stream::new(
                lopdf::Dictionary::new(),
                content_ops.into_bytes(),
            )));

            let mut xobj_dict = lopdf::Dictionary::new();
            xobj_dict.set("Im0", lopdf::Object::Reference(img_id));
            let mut resources = lopdf::Dictionary::new();
            resources.set("XObject", lopdf::Object::Dictionary(xobj_dict));

            let mut page_dict = lopdf::Dictionary::new();
            page_dict.set("Type", lopdf::Object::Name(b"Page".to_vec()));
            page_dict.set("Parent", lopdf::Object::Reference(pages_id));
            page_dict.set(
                "MediaBox",
                lopdf::Object::Array(vec![
                    lopdf::Object::Real(0.0),
                    lopdf::Object::Real(0.0),
                    lopdf::Object::Real(pw),
                    lopdf::Object::Real(ph),
                ]),
            );
            page_dict.set("Resources", lopdf::Object::Dictionary(resources));
            page_dict.set("Contents", lopdf::Object::Reference(content_id));
            let page_id = doc.add_object(lopdf::Object::Dictionary(page_dict));
            page_kids.push(lopdf::Object::Reference(page_id));
        }

        let mut pages_dict = lopdf::Dictionary::new();
        pages_dict.set("Type", lopdf::Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", lopdf::Object::Integer(page_kids.len() as i64));
        pages_dict.set("Kids", lopdf::Object::Array(page_kids));
        doc.objects
            .insert(pages_id, lopdf::Object::Dictionary(pages_dict));

        let mut catalog = lopdf::Dictionary::new();
        catalog.set("Type", lopdf::Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", lopdf::Object::Reference(pages_id));
        let catalog_id = doc.add_object(lopdf::Object::Dictionary(catalog));
        doc.trailer
            .set("Root", lopdf::Object::Reference(catalog_id));

        let mut out = Vec::new();
        doc.save_to(&mut out)?;
        Ok(out)
    }

    /// Exports the specified 0-based `pages` of `doc` as PNG or JPEG images into `out_dir`.
    /// Returns the number of exported image files.
    pub fn export_pages_as_images(
        doc: &PdfDocument,
        pages: &[u16],
        dpi: u32,
        format: ImageExportFormat,
        out_dir: &Path,
        stem: &str,
    ) -> Result<usize> {
        let total = doc.pages().len();
        if total == 0 || pages.is_empty() {
            anyhow::bail!("No pages selected to export");
        }
        std::fs::create_dir_all(out_dir)?;
        let safe_stem: String = stem
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let safe_stem = if safe_stem.is_empty() {
            "page".to_string()
        } else {
            safe_stem
        };
        let dpi_f = (dpi.clamp(72, 600)) as f32;
        let width_digits = if total >= 1000 { 4 } else if total >= 100 { 3 } else { 2 };
        let mut exported = 0usize;

        for &p_idx in pages {
            if p_idx >= total {
                continue;
            }
            let page = doc.pages().get(p_idx)?;
            let pw = page.width().value.max(1.0);
            let ph = page.height().value.max(1.0);
            let px_w = ((pw / 72.0) * dpi_f).round().clamp(16.0, 9600.0) as i32;
            let px_h = ((ph / 72.0) * dpi_f).round().clamp(16.0, 9600.0) as i32;
            let render_cfg = PdfRenderConfig::new()
                .set_target_width(px_w)
                .set_target_height(px_h)
                .render_form_data(true)
                .render_annotations(true);
            let bmp = page.render_with_config(&render_cfg)?;
            let img = bmp.as_image().to_rgb8();
            let out_name = format!(
                "{safe_stem}_p{:0width_digits$}.{}",
                p_idx + 1,
                format.extension()
            );
            let out_path = out_dir.join(out_name);
            match format {
                ImageExportFormat::Png => {
                    img.save_with_format(&out_path, image::ImageFormat::Png)
                        .with_context(|| format!("Failed to write '{}'", out_path.display()))?;
                }
                ImageExportFormat::Jpeg => {
                    let mut f = std::fs::File::create(&out_path)
                        .with_context(|| format!("Failed to create '{}'", out_path.display()))?;
                    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut f, 90);
                    enc.encode(
                        img.as_raw(),
                        img.width(),
                        img.height(),
                        image::ExtendedColorType::Rgb8,
                    )
                    .with_context(|| format!("Failed to encode '{}'", out_path.display()))?;
                }
            }
            exported += 1;
        }
        Ok(exported)
    }

    // =========================================================================
    // Tier 1.4: Document Properties Inspector & Privacy Metadata Scrubber
    // =========================================================================

    fn decode_pdf_string_bytes(bytes: &[u8]) -> String {
        if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
            let units: Vec<u16> = bytes[2..]
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            return String::from_utf16_lossy(&units)
                .trim_matches('\0')
                .trim()
                .to_string();
        }
        if bytes.len() >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF {
            return String::from_utf8_lossy(&bytes[3..]).trim().to_string();
        }
        match std::str::from_utf8(bytes) {
            Ok(s) => s.trim().to_string(),
            Err(_) => bytes.iter().map(|&b| b as char).collect::<String>().trim().to_string(),
        }
    }

    fn format_pdf_date(raw: &str) -> String {
        let s = raw.trim().strip_prefix("D:").unwrap_or(raw.trim());
        let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.len() >= 12 {
            format!(
                "{}-{}-{} {}:{}",
                &digits[0..4],
                &digits[4..6],
                &digits[6..8],
                &digits[8..10],
                &digits[10..12]
            )
        } else if digits.len() >= 8 {
            format!("{}-{}-{}", &digits[0..4], &digits[4..6], &digits[6..8])
        } else {
            raw.trim().to_string()
        }
    }

    /// Inspects document metadata, version, encryption, fonts, and XMP status from serialized PDF bytes.
    pub fn inspect_document_properties_from_bytes(
        bytes: &[u8],
        file_name: &str,
        page_count: u16,
        current_page_size_pt: (f32, f32),
    ) -> DocumentPropertiesInfo {
        let mut info = DocumentPropertiesInfo {
            file_name: file_name.to_string(),
            file_size_bytes: bytes.len() as u64,
            pdf_version: "1.4".to_string(),
            page_count,
            current_page_size_pt,
            ..Default::default()
        };

        let Ok(ldoc) = lopdf::Document::load_mem(bytes) else {
            return info;
        };
        info.pdf_version = ldoc.version.clone();
        info.is_encrypted = ldoc.is_encrypted();
        if info.page_count == 0 {
            info.page_count = ldoc.get_pages().len() as u16;
        }
        if let Ok(cat) = ldoc.catalog() {
            info.has_xmp_metadata = cat.get(b"Metadata").is_ok();
        }

        // Read trailer /Info dictionary fields
        let info_dict = ldoc.trailer.get(b"Info").ok().and_then(|obj| match obj {
            lopdf::Object::Reference(id) => ldoc.get_dictionary(*id).ok(),
            lopdf::Object::Dictionary(d) => Some(d),
            _ => None,
        });
        if let Some(d) = info_dict {
            info.has_info_dict = !d.is_empty();
            let read_str = |key: &[u8]| -> String {
                d.get(key)
                    .ok()
                    .and_then(|o| match o {
                        lopdf::Object::String(b, _) => Some(Self::decode_pdf_string_bytes(b)),
                        lopdf::Object::Reference(id) => match ldoc.get_object(*id) {
                            Ok(lopdf::Object::String(b, _)) => {
                                Some(Self::decode_pdf_string_bytes(b))
                            }
                            _ => None,
                        },
                        _ => None,
                    })
                    .unwrap_or_default()
            };
            info.title = read_str(b"Title");
            info.author = read_str(b"Author");
            info.subject = read_str(b"Subject");
            info.keywords = read_str(b"Keywords");
            info.creator = read_str(b"Creator");
            info.producer = read_str(b"Producer");
            info.creation_date = Self::format_pdf_date(&read_str(b"CreationDate"));
            info.mod_date = Self::format_pdf_date(&read_str(b"ModDate"));
        }

        // Collect unique font BaseFont names (bounded to first 16 for clean display)
        let mut fonts = Vec::new();
        for obj in ldoc.objects.values() {
            if let lopdf::Object::Dictionary(d) = obj {
                let is_font = matches!(d.get(b"Type"), Ok(lopdf::Object::Name(n)) if n == b"Font");
                if is_font {
                    if let Ok(lopdf::Object::Name(base)) = d.get(b"BaseFont") {
                        let raw_name = String::from_utf8_lossy(base);
                        let clean = raw_name
                            .split_once('+')
                            .map(|(_, r)| r)
                            .unwrap_or(&raw_name)
                            .to_string();
                        if !clean.is_empty() && !fonts.contains(&clean) {
                            fonts.push(clean);
                            if fonts.len() >= 16 {
                                break;
                            }
                        }
                    }
                }
            }
        }
        fonts.sort();
        info.fonts = fonts;
        info
    }

    /// Convenience wrapper inspecting the active in-memory `PdfDocument`.
    pub fn inspect_document_properties(
        doc: &PdfDocument,
        path: &Path,
        current_page_idx: u16,
    ) -> DocumentPropertiesInfo {
        let fname = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("document.pdf")
            .to_string();
        let page_count = doc.pages().len();
        let page_size = doc
            .pages()
            .get(current_page_idx.min(page_count.saturating_sub(1)))
            .map(|p| (p.width().value, p.height().value))
            .unwrap_or((595.28, 841.89));
        let bytes = doc.save_to_bytes().unwrap_or_default();
        let mut props = Self::inspect_document_properties_from_bytes(
            &bytes,
            &fname,
            page_count,
            page_size,
        );
        if let Ok(meta) = std::fs::metadata(path) {
            props.file_size_bytes = meta.len();
        }
        props
    }

    /// Strips all personal metadata (`/Info`, Catalog `/Metadata` XMP, `/PieceInfo`, and page-level
    /// `/Metadata` & `/PieceInfo`) from serialized PDF bytes while preserving all content, bookmarks, and page labels.
    pub fn scrub_document_metadata_bytes(bytes: &[u8]) -> Result<Vec<u8>> {
        let mut ldoc = lopdf::Document::load_mem(bytes)?;
        if ldoc.is_encrypted() {
            anyhow::bail!("Cannot scrub metadata on an encrypted PDF (use Flatten & Unlock first)");
        }
        if let Ok(lopdf::Object::Reference(info_id)) = ldoc.trailer.get(b"Info").cloned() {
            ldoc.objects.remove(&info_id);
        }
        ldoc.trailer.remove(b"Info");

        let mut orphan_ids = Vec::new();
        if let Ok(cat) = ldoc.catalog_mut() {
            for key in [b"Metadata".as_slice(), b"PieceInfo".as_slice()] {
                if let Some(lopdf::Object::Reference(id)) = cat.remove(key) {
                    orphan_ids.push(id);
                }
            }
        }
        let page_ids: Vec<lopdf::ObjectId> = ldoc.get_pages().values().copied().collect();
        for pid in page_ids {
            if let Ok(lopdf::Object::Dictionary(pdict)) = ldoc.get_object_mut(pid) {
                for key in [b"Metadata".as_slice(), b"PieceInfo".as_slice()] {
                    if let Some(lopdf::Object::Reference(id)) = pdict.remove(key) {
                        orphan_ids.push(id);
                    }
                }
            }
        }
        for id in orphan_ids {
            ldoc.objects.remove(&id);
        }

        let mut out = Vec::new();
        ldoc.save_to(&mut out)?;
        Ok(out)
    }

    // =========================================================================
    // Tier 2.2: Native Interactive Form Filling (Text, Checkbox, Radio)
    // =========================================================================

    /// Finds a fillable PDF form widget (Text Field, Checkbox, or Radio Button) at `(pdf_x, pdf_y)` on `page`.
    pub fn find_form_field_at_point(
        page: &PdfPage,
        pdf_x: f32,
        pdf_y: f32,
    ) -> Option<FoundFormFieldInfo> {
        let annots: Vec<_> = page.annotations().iter().enumerate().collect();
        for (idx, annot) in annots.into_iter().rev() {
            let Some(widget) = annot.as_widget_annotation() else {
                continue;
            };
            let Ok(bounds) = annot.bounds() else {
                continue;
            };
            let min_x = bounds.left().value.min(bounds.right().value) - 2.0;
            let max_x = bounds.left().value.max(bounds.right().value) + 2.0;
            let min_y = bounds.bottom().value.min(bounds.top().value) - 2.0;
            let max_y = bounds.bottom().value.max(bounds.top().value) + 2.0;
            if pdf_x < min_x || pdf_x > max_x || pdf_y < min_y || pdf_y > max_y {
                continue;
            }
            let Some(field) = widget.form_field() else {
                continue;
            };
            let raw_name = field.name().unwrap_or_default().trim().to_string();
            if let Some(tf) = field.as_text_field() {
                return Some(FoundFormFieldInfo {
                    annot_index: idx,
                    kind: FormFieldKind::Text,
                    name: if raw_name.is_empty() { "Text Field".to_string() } else { raw_name },
                    value: tf.value().unwrap_or_default(),
                    bounds,
                });
            }
            if field.as_checkbox_field().is_some() {
                return Some(FoundFormFieldInfo {
                    annot_index: idx,
                    kind: FormFieldKind::Checkbox,
                    name: if raw_name.is_empty() { "Checkbox".to_string() } else { raw_name },
                    value: String::new(),
                    bounds,
                });
            }
            if field.as_radio_button_field().is_some() {
                return Some(FoundFormFieldInfo {
                    annot_index: idx,
                    kind: FormFieldKind::Radio,
                    name: if raw_name.is_empty() { "Radio Button".to_string() } else { raw_name },
                    value: String::new(),
                    bounds,
                });
            }
        }
        None
    }

    /// Toggles a Checkbox or selects a Radio Button widget at `annot_index` on `page_index`.
    /// Returns a user-friendly status label describing the new state.
    pub fn toggle_form_button_at(
        doc: &mut PdfDocument,
        page_index: u16,
        annot_index: usize,
    ) -> Result<String> {
        let mut page = doc.pages_mut().get(page_index)?;
        let mut annot = page.annotations_mut().get(annot_index)?;
        let widget = annot
            .as_widget_annotation_mut()
            .context("Annotation is not a form widget")?;
        let field = widget
            .form_field_mut()
            .context("Widget has no associated form field")?;
        let name = field.name().unwrap_or_default().trim().to_string();
        if let Some(cb) = field.as_checkbox_field_mut() {
            let cur = cb.is_checked().unwrap_or(false);
            cb.set_checked(!cur)?;
            let label = if name.is_empty() { "Checkbox" } else { &name };
            return Ok(format!(
                "{} '{}'",
                if !cur { "Checked" } else { "Unchecked" },
                label
            ));
        }
        if let Some(rb) = field.as_radio_button_field_mut() {
            rb.set_checked()?;
            let label = if name.is_empty() { "Radio option" } else { &name };
            return Ok(format!("Selected '{label}'"));
        }
        anyhow::bail!("Form field is not a checkbox or radio button")
    }

    /// Sets the value of a PDF Text Form Field widget at `annot_index` on `page_index`.
    pub fn set_form_text_value_at(
        doc: &mut PdfDocument,
        page_index: u16,
        annot_index: usize,
        new_value: &str,
    ) -> Result<()> {
        let mut page = doc.pages_mut().get(page_index)?;
        let mut annot = page.annotations_mut().get(annot_index)?;
        let widget = annot
            .as_widget_annotation_mut()
            .context("Annotation is not a form widget")?;
        let field = widget
            .form_field_mut()
            .context("Widget has no associated form field")?;
        let tf = field
            .as_text_field_mut()
            .context("Form field is not a text input")?;
        tf.set_value(new_value)?;
        Ok(())
    }

    // =========================================================================
    // Milestone 3: Strictly Matrix-Based Page Deskew (Zero Rasterization)
    // =========================================================================

    /// Applies a centered 2D affine deskew rotation (`cw_deg` in `-15.0..=15.0`, positive = CW)
    /// directly to every PDFium page object on each page in `page_indices`, preserving MediaBox
    /// and CropBox (white corners) and regenerating content streams once per page.
    pub fn deskew_pages_in_doc(
        doc: &mut PdfDocument,
        page_indices: &[u16],
        cw_deg: f32,
    ) -> Result<usize> {
        let clamped = cw_deg.clamp(-15.0, 15.0);
        if clamped.abs() < 0.01 {
            return Ok(0);
        }
        let total = doc.pages().len();
        let mut applied = 0usize;
        for &p_idx in page_indices {
            if p_idx >= total {
                continue;
            }
            let mut page = doc.pages_mut().get(p_idx)?;
            let (cx, cy) = page
                .boundaries()
                .media()
                .map(|mb| {
                    let b = mb.bounds;
                    (
                        (b.left().value + b.right().value) * 0.5,
                        (b.bottom().value + b.top().value) * 0.5,
                    )
                })
                .unwrap_or_else(|_| (page.width().value * 0.5, page.height().value * 0.5));
            let m = compute_deskew_affine_matrix_around(cx, cy, clamped);

            let prev_strategy = page.content_regeneration_strategy();
            page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
            for mut obj in page.objects_mut().iter() {
                let _ = obj.transform(m.a, m.b, m.c, m.d, m.e, m.f);
            }
            page.regenerate_content()?;
            page.set_content_regeneration_strategy(prev_strategy);
            applied += 1;
        }
        Ok(applied)
    }
}

/// Auto-detected guideline snap axis for Milestone 3 Page Deskew.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DeskewAxis {
    Horizontal,
    Vertical,
}

/// 6-element PDF 2D affine matrix (`x' = a*x + c*y + e`, `y' = b*x + d*y + f`, Y-up PDF coords).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DeskewMatrix {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl DeskewMatrix {
    /// Applies this affine matrix to a PDF-space point `(x, y)`.
    pub fn apply_point(self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }
}

/// Computes the centered PDFium 2D affine matrix for `Translate(cx, cy) -> Rotate(cw_deg) -> Translate(-cx, -cy)`
/// around `(page_w / 2, page_h / 2)` in PDF Y-up coordinates (`positive = CW`, `negative = CCW`).
pub fn compute_deskew_affine_matrix(page_w: f32, page_h: f32, cw_deg: f32) -> DeskewMatrix {
    compute_deskew_affine_matrix_around(page_w * 0.5, page_h * 0.5, cw_deg)
}

/// Computes the centered PDFium 2D affine matrix around arbitrary center `(cx, cy)` in PDF Y-up coords.
pub fn compute_deskew_affine_matrix_around(cx: f32, cy: f32, cw_deg: f32) -> DeskewMatrix {
    // In PDF Y-up coordinates, CCW by phi uses (cos(phi), sin(phi), -sin(phi), cos(phi)).
    // Positive cw_deg = Clockwise -> phi = -cw_deg.to_radians().
    let phi = (-cw_deg as f64).to_radians();
    let cos_p = phi.cos() as f32;
    let sin_p = phi.sin() as f32;
    let a = cos_p;
    let b = sin_p;
    let c = -sin_p;
    let d = cos_p;
    let cx64 = cx as f64;
    let cy64 = cy as f64;
    let e = (cx64 - (a as f64) * cx64 - (c as f64) * cy64) as f32;
    let f = (cy64 - (b as f64) * cx64 - (d as f64) * cy64) as f32;
    DeskewMatrix { a, b, c, d, e, f }
}

/// Converts a user-drawn screen guideline delta `(dx_screen, dy_screen)` (screen space: +X right, +Y down)
/// into `(snapped_axis, correction_cw_deg)` clamped to `-15.0..=15.0` (positive = CW, negative = CCW).
/// - If `|dy| >= |dx|`: snaps to Vertical (`90°`).
/// - Otherwise: snaps to Horizontal (`0°`).
/// Returns `None` if the drawn guideline is too short (`< 4.0 px`).
pub fn compute_guideline_deskew_angle(
    dx_screen: f32,
    dy_screen: f32,
) -> Option<(DeskewAxis, f32)> {
    let len = dx_screen.hypot(dy_screen);
    if len < 4.0 {
        return None;
    }
    if dy_screen.abs() >= dx_screen.abs() {
        // Vertical margin: normalize top-to-bottom (ny > 0)
        let (nx, ny) = if dy_screen < 0.0 {
            (-dx_screen, -dy_screen)
        } else {
            (dx_screen, dy_screen)
        };
        // If line leans right going down (`\` shape, nx > 0), page is tilted CCW -> rotate CW (+)
        let raw_deg = (nx as f64).atan2(ny as f64).to_degrees() as f32;
        let clamped = (raw_deg.clamp(-15.0, 15.0) * 10.0).round() / 10.0;
        Some((DeskewAxis::Vertical, clamped))
    } else {
        // Horizontal line: normalize left-to-right (nx > 0)
        let (nx, ny) = if dx_screen < 0.0 {
            (-dx_screen, -dy_screen)
        } else {
            (dx_screen, dy_screen)
        };
        // If line slopes downward to the right (ny > 0 in screen Y-down), page is tilted CW -> rotate CCW (-)
        let raw_deg = -((ny as f64).atan2(nx as f64).to_degrees() as f32);
        let clamped = (raw_deg.clamp(-15.0, 15.0) * 10.0).round() / 10.0;
        Some((DeskewAxis::Horizontal, clamped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phase2_engine_upgrades() -> Result<()> {
        let engine = PdfEngine::new()?;
        let test_pdf = Path::new("test_unit_temp.pdf");
        engine.create_sample_pdf(test_pdf, 5)?;

        let mut doc = engine.load_pdf_file(test_pdf)?;
        assert_eq!(doc.pages().len(), 5);

        // 1. Stamp page numbers twice (verifies clear_margin_zone removes previous numbers without crashing)
        PdfEngine::apply_page_numbers_to_doc(&mut doc, 2)?;
        PdfEngine::apply_page_numbers_to_doc(&mut doc, 3)?;

        // 2. Stamp aligned headers/footers (Left, Center, Right) and clear them
        for pos in [
            MarginPosition::HeaderLeft,
            MarginPosition::HeaderCenter,
            MarginPosition::HeaderRight,
            MarginPosition::FooterLeft,
            MarginPosition::FooterCenter,
            MarginPosition::FooterRight,
        ] {
            PdfEngine::apply_aligned_header_footer_to_doc(&mut doc, pos, "Aligned Text", 1, 5)?;
        }
        PdfEngine::clear_margin_zone_in_doc(&mut doc, MarginZone::Header, 1, 5)?;
        PdfEngine::clear_margin_zone_in_doc(&mut doc, MarginZone::Footer, 1, 5)?;

        // Verify that the title text on page 1 is still near the top of the page (y > 500)
        {
            let page = doc.pages().get(0)?;
            let mut found_top_title = false;
            for obj in page.objects().iter() {
                if let Some(txt) = obj.as_text_object() {
                    if txt.text().contains("Sample Document") {
                        let b = obj.bounds()?;
                        assert!(b.bottom().value > 500.0);
                        found_top_title = true;
                    }
                }
            }
            assert!(found_top_title);
        }

        // 3. Reorder pages via move_page_in_doc
        engine.move_page_in_doc(&mut doc, 0, 3)?;
        assert_eq!(doc.pages().len(), 5);

        // 4. Insert external PDF at index
        let inserted = engine.insert_external_pdf_at_index(&mut doc, test_pdf, 2)?;
        assert_eq!(inserted, 5);
        assert_eq!(doc.pages().len(), 10);

        // 5. Hierarchical Bookmarks `.txt` parse, sidecar round-trip, and logical page sync
        let txt_input = "1 -- Introduction -- i\na -- Overview -- ii\n2 -- Core Analytics -- 1\nb -- SQL Basics -- 2\n";
        let mut bms = PdfEngine::parse_bookmarks_txt(txt_input, 2, 10);
        assert_eq!(bms.len(), 4);
        assert_eq!(bms[0].level, 0);
        assert_eq!(bms[0].page_index, 0); // "i" -> index 0
        assert_eq!(bms[1].level, 1);
        assert_eq!(bms[1].page_index, 1); // "ii" -> index 1
        assert_eq!(bms[2].level, 0);
        assert_eq!(bms[2].page_index, 2); // logical "1" with roman_end=2 -> physical page 3 (index 2)
        assert_eq!(bms[3].level, 1);
        assert_eq!(bms[3].page_index, 3); // logical "2" with roman_end=2 -> physical page 4 (index 3)

        // Re-sync when roman_end changes from 2 to 4
        PdfEngine::resync_bookmarks_logical_pages(&mut bms, 4, 10);
        assert_eq!(bms[2].page_index, 4); // logical "1" with roman_end=4 -> physical page 5 (index 4)

        PdfEngine::save_bookmarks_sidecar(test_pdf, &bms)?;
        let loaded_bms = PdfEngine::load_bookmarks(&doc, test_pdf, 4);
        assert_eq!(loaded_bms, bms);

        // 6. Test /PageLabels and /Outlines metadata embedding + BW page conversion
        let raw_bytes = doc.save_to_bytes()?;
        let embedded_bytes = PdfEngine::embed_metadata_in_pdf_bytes(&raw_bytes, 4, &bms, false)?;
        assert_eq!(PdfEngine::read_page_labels_roman_end(&embedded_bytes), 4);
        // v1.2.1: the PDFium-based reader used on open must agree with the lopdf reader.
        let labelled = engine.load_pdf_from_bytes_safe(embedded_bytes.clone())?;
        assert_eq!(PdfEngine::read_page_labels_roman_end_from_doc(&labelled), 4);
        let bw_bytes = engine.convert_page_to_bw(&embedded_bytes, 0)?;
        let bw_doc = engine.load_pdf_from_bytes_safe(bw_bytes)?;
        assert_eq!(bw_doc.pages().len(), 10);

        // 7. Compress PDF and verify compressed bytes load cleanly with 10 pages
        let (compressed_bytes, orig_len, new_len) = engine.compress_doc(&doc)?;
        assert!(!compressed_bytes.is_empty());
        assert!(new_len <= orig_len);
        let compressed_doc = engine
            .pdfium
            .load_pdf_from_byte_vec(compressed_bytes, None)?;
        assert_eq!(compressed_doc.pages().len(), 10);

        let _ = std::fs::remove_file(PdfEngine::bookmarks_sidecar_path(test_pdf));
        let _ = std::fs::remove_file(test_pdf);

        Ok(())
    }

    #[test]
    fn test_phase5_power_editor_tools() -> Result<()> {
        let engine = PdfEngine::new()?;
        let pdf_a = PathBuf::from("test_p5_a.pdf");
        let pdf_b = PathBuf::from("test_p5_b.pdf");
        engine.create_sample_pdf(&pdf_a, 3)?;
        engine.create_sample_pdf(&pdf_b, 2)?;

        // 1. Batch Combine Multiple PDFs
        let (combined_bytes, total_pages) =
            engine.combine_multiple_pdfs(&[pdf_a.clone(), pdf_b.clone()])?;
        assert_eq!(total_pages, 5);
        let mut doc = engine.load_pdf_from_bytes_safe(combined_bytes)?;
        assert_eq!(doc.pages().len(), 5);

        // 2. Compute Page Content Bounds (FitTextWidth)
        {
            let page0 = doc.pages().get(0)?;
            let (min_x, _min_y, max_x, _max_y) = PdfEngine::compute_page_content_bounds(&page0);
            assert!(max_x > min_x);
            assert!(min_x >= 0.0 && max_x <= page0.width().value);
        }

        // 3. Text Search across pages
        let matches = PdfEngine::search_text_in_doc(&doc, "Sample Document");
        assert_eq!(matches.len(), 5);
        assert!(!matches[0].rects.is_empty());

        // 4. True Black-Box Redaction
        let redact_rect = PdfRect::new_from_values(700.0, 60.0, 760.0, 500.0);
        let removed = PdfEngine::redact_rect_on_page(&mut doc, 0, redact_rect)?;
        assert!(removed >= 1);
        let matches_after_redact = PdfEngine::search_text_in_doc(&doc, "Sample Document");
        assert_eq!(matches_after_redact.len(), 4);

        // 5. Visual Page Cropping & Reset Page Crop
        let crop_rect = PdfRect::new_from_values(200.0, 50.0, 600.0, 450.0);
        let cropped_bytes = engine.crop_page_to_rect(&mut doc, 0, crop_rect)?;
        doc = engine.load_pdf_from_bytes_safe(cropped_bytes)?;
        {
            let p0 = doc.pages().get(0)?;
            assert!((p0.width().value - 400.0).abs() < 1.0);
            assert!((p0.height().value - 400.0).abs() < 1.0);
        }
        let uncropped_bytes = engine.reset_page_crop(&mut doc, 0)?;
        doc = engine.load_pdf_from_bytes_safe(uncropped_bytes)?;
        {
            let p0 = doc.pages().get(0)?;
            assert!(p0.width().value > 550.0);
            assert!(p0.height().value > 800.0);
        }

        // 6. Signature / Image Stamp (with RGBA alpha transparency & >2000px downscale cap)
        let big_img_path = std::env::temp_dir().join("test_big_stamp.png");
        let big_rgba = image::RgbaImage::from_pixel(2500, 1250, image::Rgba([20, 80, 200, 180]));
        big_rgba.save(&big_img_path)?;
        let stamp = PdfEngine::load_stamp_image(&big_img_path)?;
        let _ = std::fs::remove_file(&big_img_path);
        assert!(stamp.width <= 2000 && stamp.height <= 2000);
        assert_eq!(stamp.width, 2000);
        let stamp_rect = PdfRect::new_from_values(100.0, 100.0, 140.0, 180.0);
        engine.stamp_image_on_page(&mut doc, 0, &stamp, stamp_rect)?;

        // 7. Flatten & Unlock PDF
        let report = engine.flatten_and_unlock_doc(&mut doc)?;
        assert!(report.failed_pages.is_empty());
        let flat_doc = engine.load_pdf_from_bytes_safe(report.bytes)?;
        assert_eq!(flat_doc.pages().len(), 5);

        let _ = std::fs::remove_file(&pdf_a);
        let _ = std::fs::remove_file(&pdf_b);
        Ok(())
    }

    /// v1.1 regression test: redaction must remove text AND image pixels under the box,
    /// keep unrelated text, and leave no trace of the secret anywhere in the saved bytes.
    #[test]
    fn test_redaction_leaves_no_trace() -> Result<()> {
        const SECRET: &str = "TOPSECRET42";
        let engine = PdfEngine::new()?;

        let src = {
            let mut doc = engine.pdfium.create_new_pdf()?;
            let font = doc.fonts_mut().helvetica();
            let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::a4())?;
            let mut t = PdfPageTextObject::new(&doc, SECRET, font, PdfPoints::new(14.0))?;
            t.translate(PdfPoints::new(100.0), PdfPoints::new(700.0))?;
            page.objects_mut().add_text_object(t)?;
            let mut keep = PdfPageTextObject::new(&doc, "Public line stays", font, PdfPoints::new(14.0))?;
            keep.translate(PdfPoints::new(100.0), PdfPoints::new(500.0))?;
            page.objects_mut().add_text_object(keep)?;
            // Solid red 200x200 px image covering x 100..300, y 550..750 (overlaps the secret).
            let red = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                200, 200, image::Rgba([255, 0, 0, 255]),
            ));
            let mut io = PdfPageImageObject::new_with_size(&doc, &red, PdfPoints::new(200.0), PdfPoints::new(200.0))?;
            io.translate(PdfPoints::new(100.0), PdfPoints::new(550.0))?;
            page.objects_mut().add_image_object(io)?;
            doc.save_to_bytes()?
        };

        let mut doc = engine.load_pdf_from_bytes_safe(src)?;
        let box_rect = PdfRect::new_from_values(690.0, 90.0, 720.0, 320.0);
        assert!(PdfEngine::redact_rect_on_page(&mut doc, 0, box_rect)? >= 2);

        let out = std::env::temp_dir().join("micropdf_redaction_test.pdf");
        assert!(PdfEngine::save_doc_with_metadata(&doc, &out, 0, &[], true)?.is_none());
        let saved = std::fs::read(&out)?;
        let _ = std::fs::remove_file(&out);

        // (a) no secret in any decompressed stream or object
        let mut ldoc = lopdf::Document::load_mem(&saved)?;
        ldoc.decompress();
        let mut flat = Vec::new();
        ldoc.save_to(&mut flat)?;
        let hay = String::from_utf8_lossy(&flat).to_uppercase();
        let hex: String = SECRET.bytes().map(|b| format!("{b:02X}")).collect();
        assert!(!hay.contains(SECRET) && !hay.contains(&hex), "secret survives in file bytes");

        // (b) no secret in extracted text, unrelated text intact
        let reloaded = engine.load_pdf_from_bytes_safe(saved)?;
        let page = reloaded.pages().get(0)?;
        let text = page.text()?.all();
        assert!(!text.contains(SECRET));
        assert!(text.contains("Public line stays"));

        // (c) image survives outside the box, but no red pixels remain under it
        let mut red_outside = 0usize;
        for obj in page.objects().iter() {
            if let Some(io) = obj.as_image_object() {
                let px = io.get_raw_image()?.to_rgba8();
                for y in 0..px.height() {
                    for x in 0..px.width() {
                        let p = px.get_pixel(x, y);
                        let is_red = p[0] > 200 && p[1] < 50;
                        let under_box = (32..58).contains(&y) && (2..198).contains(&x);
                        assert!(!(is_red && under_box), "image pixel under the box survived");
                        if is_red && !under_box {
                            red_outside += 1;
                        }
                    }
                }
            }
        }
        assert!(red_outside > 20_000, "image outside the box should be preserved");
        Ok(())
    }

    /// v1.2 regression test: bookmarks (`/Outlines`), `/PageLabels`, and `/AcroForm` form fields
    /// must survive `compress_doc`.
    #[test]
    fn test_compress_doc_preserves_bookmarks_pagelabels_and_forms() -> Result<()> {
        let engine = PdfEngine::new()?;
        let tmp = std::env::temp_dir().join("test_compress_preserve.pdf");
        engine.create_sample_pdf(&tmp, 4)?;
        let raw = std::fs::read(&tmp)?;
        let _ = std::fs::remove_file(&tmp);

        let bms = vec![
            Bookmark {
                serial: "1".to_string(),
                title: "Preface".to_string(),
                logical_page: "i".to_string(),
                page_index: 0,
                level: 0,
            },
            Bookmark {
                serial: "2".to_string(),
                title: "Main Chapter".to_string(),
                logical_page: "1".to_string(),
                page_index: 2,
                level: 0,
            },
        ];

        // Embed /PageLabels (roman_end = 2) and /Outlines (bookmarks), then add an /AcroForm field
        let with_meta = PdfEngine::embed_metadata_in_pdf_bytes(&raw, 2, &bms, false)?;
        let mut ldoc = lopdf::Document::load_mem(&with_meta)?;
        let page1_id = *ldoc.get_pages().get(&1).unwrap();
        let mut field = lopdf::Dictionary::new();
        field.set("Type", lopdf::Object::Name(b"Annot".to_vec()));
        field.set("Subtype", lopdf::Object::Name(b"Widget".to_vec()));
        field.set("FT", lopdf::Object::Name(b"Tx".to_vec()));
        field.set("T", lopdf::Object::string_literal("CustomerName"));
        field.set("V", lopdf::Object::string_literal("Alice"));
        field.set(
            "Rect",
            lopdf::Object::Array(vec![
                lopdf::Object::Integer(50),
                lopdf::Object::Integer(50),
                lopdf::Object::Integer(200),
                lopdf::Object::Integer(80),
            ]),
        );
        field.set("P", lopdf::Object::Reference(page1_id));
        let field_id = ldoc.add_object(lopdf::Object::Dictionary(field));
        if let Ok(lopdf::Object::Dictionary(pdict)) = ldoc.get_object_mut(page1_id) {
            pdict.set(
                "Annots",
                lopdf::Object::Array(vec![lopdf::Object::Reference(field_id)]),
            );
        }
        let mut acro = lopdf::Dictionary::new();
        acro.set(
            "Fields",
            lopdf::Object::Array(vec![lopdf::Object::Reference(field_id)]),
        );
        let acro_id = ldoc.add_object(lopdf::Object::Dictionary(acro));
        if let Ok(cat) = ldoc.catalog_mut() {
            cat.set("AcroForm", lopdf::Object::Reference(acro_id));
        }
        // Add an unreferenced dead object so compress_doc actually shrinks the file
        ldoc.add_object(lopdf::Object::Stream(lopdf::Stream::new(
            lopdf::Dictionary::new(),
            vec![b'X'; 4096],
        )));
        let mut input_bytes = Vec::new();
        ldoc.save_to(&mut input_bytes)?;

        let doc = engine.load_pdf_from_bytes_safe(input_bytes)?;
        let (compressed_bytes, _orig, _new) = engine.compress_doc(&doc)?;

        // Verify /PageLabels survived
        assert_eq!(PdfEngine::read_page_labels_roman_end(&compressed_bytes), 2);

        // Verify /Outlines (bookmarks) survived
        let reloaded = engine.load_pdf_from_bytes_safe(compressed_bytes.clone())?;
        let dummy_path = Path::new("nonexistent_for_outline_check.pdf");
        let loaded_bms = PdfEngine::load_bookmarks(&reloaded, dummy_path, 2);
        assert_eq!(loaded_bms.len(), 2);
        assert_eq!(loaded_bms[0].title, "Preface");
        assert_eq!(loaded_bms[1].title, "Main Chapter");

        // Verify /AcroForm and widget annotation survived
        let check_ldoc = lopdf::Document::load_mem(&compressed_bytes)?;
        let cat = check_ldoc.catalog()?;
        assert!(cat.has(b"AcroForm"), "/AcroForm must survive compress_doc");
        assert!(
            reloaded.pages().get(0)?.annotations().len() >= 1,
            "Form widget annotation must survive compress_doc"
        );
        Ok(())
    }

    #[test]
    fn test_tier0_from_roman_subtractive_and_skip_bounds() {
        assert_eq!(PdfEngine::from_roman("i"), Some(1));
        assert_eq!(PdfEngine::from_roman("iv"), Some(4));
        assert_eq!(PdfEngine::from_roman("ix"), Some(9));
        assert_eq!(PdfEngine::from_roman("xiv"), Some(14));
        assert_eq!(PdfEngine::from_roman("xl"), Some(40));
        assert_eq!(PdfEngine::from_roman("xc"), Some(90));
        assert_eq!(PdfEngine::from_roman("cd"), Some(400));
        assert_eq!(PdfEngine::from_roman("cm"), Some(900));
        assert_eq!(PdfEngine::from_roman("mcmxcix"), Some(1999));

        // skip_pages = 2 (physical 0,1 unnumbered covers), roman_end = 3 (physical 2,3,4 -> i,ii,iii),
        // Arabic starts at physical 5 -> "1".
        assert_eq!(
            PdfEngine::resolve_logical_page_str_with_skip("i", 2, 3, 10),
            Some(2)
        );
        assert_eq!(
            PdfEngine::resolve_logical_page_str_with_skip("iii", 2, 3, 10),
            Some(4)
        );
        // "iv" exceeds roman_end == 3, so it must NOT jump past front matter
        assert_eq!(
            PdfEngine::resolve_logical_page_str_with_skip("iv", 2, 3, 10),
            None
        );
        assert_eq!(
            PdfEngine::resolve_logical_page_str_with_skip("1", 2, 3, 10),
            Some(5)
        );
        assert_eq!(
            PdfEngine::resolve_logical_page_str_with_skip("#1", 2, 3, 10),
            Some(0)
        );
    }

    #[test]
    fn test_tier0_page_labels_skip_roundtrip_outlines_removal_and_encrypted_guard() -> Result<()> {
        // Build a minimal 6-page PDF with pure lopdf (no libpdfium required)
        let mut doc = lopdf::Document::with_version("1.4");
        let pages_id = doc.new_object_id();
        let mut kids = Vec::new();
        for _ in 0..6 {
            let mut p = lopdf::Dictionary::new();
            p.set("Type", lopdf::Object::Name(b"Page".to_vec()));
            p.set("Parent", lopdf::Object::Reference(pages_id));
            p.set(
                "MediaBox",
                lopdf::Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]),
            );
            let pid = doc.add_object(lopdf::Object::Dictionary(p));
            kids.push(lopdf::Object::Reference(pid));
        }
        let mut pages_dict = lopdf::Dictionary::new();
        pages_dict.set("Type", lopdf::Object::Name(b"Pages".to_vec()));
        pages_dict.set("Kids", lopdf::Object::Array(kids));
        pages_dict.set("Count", lopdf::Object::Integer(6));
        doc.objects
            .insert(pages_id, lopdf::Object::Dictionary(pages_dict));
        let mut cat = lopdf::Dictionary::new();
        cat.set("Type", lopdf::Object::Name(b"Catalog".to_vec()));
        cat.set("Pages", lopdf::Object::Reference(pages_id));
        let cat_id = doc.add_object(lopdf::Object::Dictionary(cat));
        doc.trailer.set("Root", lopdf::Object::Reference(cat_id));
        let mut raw = Vec::new();
        doc.save_to(&mut raw)?;

        let bms = vec![Bookmark {
            serial: "1".to_string(),
            title: "Intro".to_string(),
            logical_page: "i".to_string(),
            page_index: 2,
            level: 0,
        }];
        let embedded = PdfEngine::embed_metadata_in_pdf_bytes_with_skip(
            &raw, 2, 3, true, &bms, false,
        )?;
        assert_eq!(PdfEngine::read_page_labels_skip_and_roman(&embedded), (2, 3));

        // Clearing all bookmarks must remove /Outlines from Catalog
        let cleared = PdfEngine::embed_metadata_in_pdf_bytes_with_skip(
            &embedded, 2, 3, true, &[], false,
        )?;
        let cleared_doc = lopdf::Document::load_mem(&cleared)?;
        assert!(
            !cleared_doc.catalog()?.has(b"Outlines"),
            "Catalog /Outlines should be removed when bookmarks are empty"
        );

        // Encrypted PDF must be refused cleanly rather than corrupted
        let mut enc_doc = lopdf::Document::load_mem(&raw)?;
        let enc_dict_id = enc_doc.add_object(lopdf::Object::Dictionary(lopdf::Dictionary::new()));
        enc_doc
            .trailer
            .set("Encrypt", lopdf::Object::Reference(enc_dict_id));
        let mut enc_raw = Vec::new();
        enc_doc.save_to(&mut enc_raw)?;
        assert!(
            PdfEngine::embed_metadata_in_pdf_bytes_with_skip(&enc_raw, 0, 0, false, &[], false)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn test_tier0_bookmark_remap_and_txt_roundtrip() {
        let mut bms = vec![
            Bookmark {
                serial: "1".to_string(),
                title: "Cover Note".to_string(),
                logical_page: "1".to_string(),
                page_index: 0,
                level: 0,
            },
            Bookmark {
                serial: "2".to_string(),
                title: "Chapter 1".to_string(),
                logical_page: "4".to_string(),
                page_index: 3,
                level: 0,
            },
            Bookmark {
                serial: "2.1".to_string(),
                title: "Section A".to_string(),
                logical_page: "6".to_string(),
                page_index: 5,
                level: 1,
            },
        ];

        // Insert 2 pages at index 2 -> page 0 stays 0, 3->5, 5->7
        PdfEngine::remap_bookmarks_after_insert(&mut bms, 2, 2, 0, 0);
        assert_eq!(bms[0].page_index, 0);
        assert_eq!(bms[1].page_index, 5);
        assert_eq!(bms[2].page_index, 7);

        // Delete page 1 -> 0 stays 0, 5->4, 7->6
        PdfEngine::remap_bookmarks_after_delete(&mut bms, 1, 8, 0, 0);
        assert_eq!(bms[0].page_index, 0);
        assert_eq!(bms[1].page_index, 4);
        assert_eq!(bms[2].page_index, 6);

        // Move page 4 to page 1 -> bms[1] follows page 4 -> 1
        PdfEngine::remap_bookmarks_after_move(&mut bms, 4, 1, 0, 0);
        assert_eq!(bms[1].page_index, 1);
        assert_eq!(bms[2].page_index, 6);

        // Export to .txt and parse back cleanly
        let txt = PdfEngine::format_bookmarks_as_txt(&bms);
        let parsed = PdfEngine::parse_bookmarks_txt_with_skip(&txt, 0, 0, 10);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[1].title, "Chapter 1");
        assert_eq!(parsed[1].page_index, 1);
        assert_eq!(parsed[2].level, 1);
    }

    #[test]
    fn test_tier1_create_pdf_from_images_all_fit_modes() -> Result<()> {
        let tmp = std::env::temp_dir().join("micropdf_tier1_img_test");
        let _ = std::fs::create_dir_all(&tmp);
        let png_path = tmp.join("portrait.png");
        let jpg_path = tmp.join("landscape.jpg");

        // Transparent portrait PNG (120x240)
        let png_img = image::RgbaImage::from_pixel(120, 240, image::Rgba([20, 120, 220, 128]));
        png_img.save_with_format(&png_path, image::ImageFormat::Png)?;

        // Opaque landscape JPEG (320x160)
        let jpg_img = image::RgbImage::from_pixel(320, 160, image::Rgb([220, 80, 40]));
        jpg_img.save_with_format(&jpg_path, image::ImageFormat::Jpeg)?;

        for fit in [
            ImagePdfFitMode::FitA4,
            ImagePdfFitMode::FitLetter,
            ImagePdfFitMode::MatchImageSize,
        ] {
            let bytes = PdfEngine::create_pdf_from_images(
                &[png_path.clone(), jpg_path.clone()],
                fit,
            )?;
            let doc = lopdf::Document::load_mem(&bytes)?;
            let pages = doc.get_pages();
            assert_eq!(pages.len(), 2, "Expected 2 pages for fit mode {:?}", fit);

            if fit == ImagePdfFitMode::FitA4 {
                // Page 1 (portrait PNG) -> portrait A4 (w < h); Page 2 (landscape JPG) -> landscape A4 (w > h)
                let p1 = doc.get_dictionary(pages[&1])?;
                let p2 = doc.get_dictionary(pages[&2])?;
                let mb1 = p1.get(b"MediaBox")?.as_array()?;
                let mb2 = p2.get(b"MediaBox")?.as_array()?;
                assert!(mb1[2].as_float()? < mb1[3].as_float()?);
                assert!(mb2[2].as_float()? > mb2[3].as_float()?);
            }
        }

        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[test]
    fn test_tier1_document_properties_and_privacy_metadata_scrubber() -> Result<()> {
        // Build a minimal PDF containing /Info, XMP /Metadata, /PieceInfo, /Outlines, and /PageLabels
        let mut doc = lopdf::Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let mut pdict = lopdf::Dictionary::new();
        pdict.set("Type", lopdf::Object::Name(b"Page".to_vec()));
        pdict.set("Parent", lopdf::Object::Reference(pages_id));
        pdict.set(
            "MediaBox",
            lopdf::Object::Array(vec![0.into(), 0.into(), 595.into(), 842.into()]),
        );
        let page_id = doc.add_object(lopdf::Object::Dictionary(pdict));
        let mut pages_dict = lopdf::Dictionary::new();
        pages_dict.set("Type", lopdf::Object::Name(b"Pages".to_vec()));
        pages_dict.set("Kids", lopdf::Object::Array(vec![page_id.into()]));
        pages_dict.set("Count", lopdf::Object::Integer(1));
        doc.objects
            .insert(pages_id, lopdf::Object::Dictionary(pages_dict));

        let xmp_id = doc.add_object(lopdf::Object::Stream(lopdf::Stream::new(
            lopdf::Dictionary::new(),
            b"<x:xmpmeta>SecretAuthor</x:xmpmeta>".to_vec(),
        )));
        let piece_id = doc.add_object(lopdf::Object::Dictionary(lopdf::Dictionary::new()));

        let mut cat = lopdf::Dictionary::new();
        cat.set("Type", lopdf::Object::Name(b"Catalog".to_vec()));
        cat.set("Pages", lopdf::Object::Reference(pages_id));
        cat.set("Metadata", lopdf::Object::Reference(xmp_id));
        cat.set("PieceInfo", lopdf::Object::Reference(piece_id));
        let cat_id = doc.add_object(lopdf::Object::Dictionary(cat));
        doc.trailer.set("Root", lopdf::Object::Reference(cat_id));

        let mut info_dict = lopdf::Dictionary::new();
        info_dict.set("Title", lopdf::Object::string_literal("Quarterly Report"));
        info_dict.set("Author", lopdf::Object::string_literal("Jane Doe"));
        info_dict.set("Creator", lopdf::Object::string_literal("Writer Pro"));
        info_dict.set("CreationDate", lopdf::Object::string_literal("D:20261121143000Z"));
        let info_id = doc.add_object(lopdf::Object::Dictionary(info_dict));
        doc.trailer.set("Info", lopdf::Object::Reference(info_id));

        let mut raw = Vec::new();
        doc.save_to(&mut raw)?;

        let inspected = PdfEngine::inspect_document_properties_from_bytes(
            &raw,
            "report.pdf",
            1,
            (595.0, 842.0),
        );
        assert_eq!(inspected.pdf_version, "1.7");
        assert_eq!(inspected.title, "Quarterly Report");
        assert_eq!(inspected.author, "Jane Doe");
        assert_eq!(inspected.creator, "Writer Pro");
        assert_eq!(inspected.creation_date, "2026-11-21 14:30");
        assert!(inspected.has_xmp_metadata);
        assert!(inspected.has_info_dict);

        // Scrub metadata and verify /Info, XMP, and /PieceInfo are completely stripped
        let scrubbed = PdfEngine::scrub_document_metadata_bytes(&raw)?;
        let after = PdfEngine::inspect_document_properties_from_bytes(
            &scrubbed,
            "report.pdf",
            1,
            (595.0, 842.0),
        );
        assert!(!after.has_info_dict);
        assert!(!after.has_xmp_metadata);
        assert!(after.title.is_empty());
        assert!(after.author.is_empty());

        let scrubbed_doc = lopdf::Document::load_mem(&scrubbed)?;
        assert!(!scrubbed_doc.catalog()?.has(b"PieceInfo"));
        assert!(!scrubbed_doc.catalog()?.has(b"Metadata"));
        assert!(scrubbed_doc.trailer.get(b"Info").is_err());

        // Encrypted PDF must be refused cleanly
        let mut enc_doc = lopdf::Document::load_mem(&raw)?;
        let enc_id = enc_doc.add_object(lopdf::Object::Dictionary(lopdf::Dictionary::new()));
        enc_doc.trailer.set("Encrypt", lopdf::Object::Reference(enc_id));
        let mut enc_raw = Vec::new();
        enc_doc.save_to(&mut enc_raw)?;
        assert!(PdfEngine::scrub_document_metadata_bytes(&enc_raw).is_err());
        Ok(())
    }

    #[test]
    fn test_milestone3_deskew_guideline_all_directions() {
        // Horizontal line sloping DOWN to the right (+dx, +dy in screen Y-down) => page tilted CW => needs CCW (-) correction
        let (ax1, deg1) = compute_guideline_deskew_angle(200.0, 20.0).unwrap();
        assert_eq!(ax1, DeskewAxis::Horizontal);
        assert!((deg1 - (-5.7)).abs() < 0.15, "got {deg1}");

        // Same line drawn right-to-left (-dx, -dy) => identical angle!
        let (ax2, deg2) = compute_guideline_deskew_angle(-200.0, -20.0).unwrap();
        assert_eq!(ax2, DeskewAxis::Horizontal);
        assert!((deg2 - deg1).abs() < 1e-4);

        // Horizontal line sloping UP to the right (+dx, -dy) => page tilted CCW => needs CW (+) correction
        let (ax3, deg3) = compute_guideline_deskew_angle(200.0, -20.0).unwrap();
        assert_eq!(ax3, DeskewAxis::Horizontal);
        assert!((deg3 - 5.7).abs() < 0.15, "got {deg3}");

        // Vertical margin leaning RIGHT going down (`\` shape: +dx, +dy) => tilted CCW => needs CW (+) correction
        let (ax4, deg4) = compute_guideline_deskew_angle(15.0, 300.0).unwrap();
        assert_eq!(ax4, DeskewAxis::Vertical);
        assert!((deg4 - 2.9).abs() < 0.15, "got {deg4}");

        // Same vertical margin drawn bottom-to-top (-dx, -dy) => identical angle!
        let (ax5, deg5) = compute_guideline_deskew_angle(-15.0, -300.0).unwrap();
        assert_eq!(ax5, DeskewAxis::Vertical);
        assert!((deg5 - deg4).abs() < 1e-4);

        // Vertical margin leaning LEFT going down (`/` shape: -dx, +dy) => tilted CW => needs CCW (-) correction
        let (ax6, deg6) = compute_guideline_deskew_angle(-15.0, 300.0).unwrap();
        assert_eq!(ax6, DeskewAxis::Vertical);
        assert!((deg6 - (-2.9)).abs() < 0.15, "got {deg6}");

        // Clamping to ±15.0°
        let (_, clamped_pos) = compute_guideline_deskew_angle(100.0, -80.0).unwrap();
        assert!((clamped_pos - 15.0).abs() < 1e-4);
        let (_, clamped_neg) = compute_guideline_deskew_angle(100.0, 80.0).unwrap();
        assert!((clamped_neg - (-15.0)).abs() < 1e-4);

        // Too short (< 4 px) returns None
        assert!(compute_guideline_deskew_angle(1.5, 1.5).is_none());
    }

    #[test]
    fn test_milestone3_deskew_affine_matrix_centered() {
        let w = 600.0f32;
        let h = 800.0f32;
        let cx = w * 0.5;
        let cy = h * 0.5;

        // 0° is exact identity
        let id = compute_deskew_affine_matrix(w, h, 0.0);
        assert!((id.a - 1.0).abs() < 1e-5);
        assert!(id.b.abs() < 1e-5);
        assert!(id.c.abs() < 1e-5);
        assert!((id.d - 1.0).abs() < 1e-5);
        assert!(id.e.abs() < 1e-4);
        assert!(id.f.abs() < 1e-4);

        // +5.0° CW rotation fixes center point (cx, cy) exactly
        let m5 = compute_deskew_affine_matrix(w, h, 5.0);
        let (rcx, rcy) = m5.apply_point(cx, cy);
        assert!((rcx - cx).abs() < 1e-3);
        assert!((rcy - cy).abs() < 1e-3);

        // Determinant = a*d - b*c = 1.0 (pure rigid rotation, no scaling/shearing)
        let det = m5.a * m5.d - m5.b * m5.c;
        assert!((det - 1.0).abs() < 1e-5);

        // Top-center point (cx, cy + 100) rotated +5° CW in PDF Y-up moves RIGHT (+X) and slightly DOWN (-Y)
        let (tx, ty) = m5.apply_point(cx, cy + 100.0);
        assert!(tx > cx + 8.0 && tx < cx + 9.5, "expected +X shift, got {tx}");
        assert!(ty < cy + 100.0 && ty > cy + 99.0, "expected slight -Y drop, got {ty}");

        // Applying -5° CW afterwards restores original point
        let inv = compute_deskew_affine_matrix(w, h, -5.0);
        let (ox, oy) = inv.apply_point(tx, ty);
        assert!((ox - cx).abs() < 1e-2);
        assert!((oy - (cy + 100.0)).abs() < 1e-2);
    }
}


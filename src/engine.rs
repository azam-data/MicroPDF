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

/// Horizontal alignment for headers and footers.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MarginPosition {
    HeaderLeft,
    HeaderCenter,
    HeaderRight,
    FooterLeft,
    FooterCenter,
    FooterRight,
}

impl MarginPosition {
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
        let mut copy = self
            .pdfium
            .create_new_pdf()
            .context("Failed to create a copy of a form page")?;
        copy.pages_mut()
            .copy_page_from_document(doc, page_idx, 0)
            .with_context(|| format!("Failed to copy page {}", page_idx + 1))?;
        let mut page = copy.pages().get(0).context("Page copy is empty")?;
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
        let val_of = |c: char| -> usize {
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
        let mut total = 0usize;
        for idx in 0..chars.len() {
            let v = val_of(chars[idx]);
            if idx + 1 < chars.len() && v < val_of(chars[idx + 1]) {
                total = total.saturating_sub(v);
            } else {
                total += v;
            }
        }
        if total >= 1 && total <= 3999 && Self::to_roman(total as u16) == s {
            Some(total)
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
            if r >= 1 && r <= total_pages {
                return Some(r - 1);
            }
            return None;
        }

        if let Ok(n) = trimmed.parse::<usize>() {
            if n >= 1 {
                let phys_1based = roman_end + n;
                if phys_1based >= 1 && phys_1based <= total_pages {
                    return Some(phys_1based - 1);
                }
            }
        }

        None
    }

    /// Formats the logical label for 0-based `page_idx` given `roman_end`.
    pub fn format_logical_page_label(page_idx: usize, roman_end: usize) -> String {
        let phys_1based = page_idx + 1;
        if roman_end > 0 && phys_1based <= roman_end {
            Self::to_roman(phys_1based as u16)
        } else {
            format!("{}", phys_1based.saturating_sub(roman_end))
        }
    }

    /// Reads `/PageLabels` from `pdf_bytes` via `lopdf` and returns `roman_end` (`0` if all Arabic).
    pub fn read_page_labels_roman_end(pdf_bytes: &[u8]) -> usize {
        let Ok(ldoc) = lopdf::Document::load_mem(pdf_bytes) else {
            return 0;
        };
        let Ok(catalog) = ldoc.catalog() else {
            return 0;
        };
        let Ok(pl_obj) = catalog.get(b"PageLabels") else {
            return 0;
        };
        let pl_dict = match pl_obj {
            lopdf::Object::Reference(id) => ldoc.get_object(*id).and_then(|o| o.as_dict()).ok(),
            lopdf::Object::Dictionary(d) => Some(d),
            _ => None,
        };
        let Some(pl_dict) = pl_dict else {
            return 0;
        };
        let Ok(nums_obj) = pl_dict.get(b"Nums") else {
            return 0;
        };
        let nums_arr = match nums_obj {
            lopdf::Object::Reference(id) => ldoc.get_object(*id).and_then(|o| o.as_array()).ok(),
            lopdf::Object::Array(a) => Some(a),
            _ => None,
        };
        let Some(nums) = nums_arr else {
            return 0;
        };

        // Expect [ 0 << /S /r >> roman_end << /S /D ... >> ]
        if nums.len() >= 4 {
            let first_idx = nums[0].as_i64().unwrap_or(-1);
            let first_dict = match &nums[1] {
                lopdf::Object::Reference(id) => ldoc.get_object(*id).and_then(|o| o.as_dict()).ok(),
                lopdf::Object::Dictionary(d) => Some(d),
                _ => None,
            };
            let is_roman = first_dict
                .and_then(|d| d.get(b"S").ok())
                .and_then(|s| s.as_name_str().ok())
                .map(|n| n.eq_ignore_ascii_case("r"))
                .unwrap_or(false);
            if first_idx == 0 && is_roman {
                if let Ok(second_idx) = nums[2].as_i64() {
                    if second_idx > 0 {
                        return second_idx as usize;
                    }
                }
            }
        }
        0
    }

    /// Number of leading pages labelled i, ii, iii, ... read through PDFium from the already
    /// open document. v1.2.1: replaces `read_page_labels_roman_end_from_path`, which read and
    /// fully parsed the whole file a second time with lopdf on every open and auto-reload.
    /// Like the lopdf version, returns 0 when there is no roman front matter or when the whole
    /// document is roman-numbered.
    pub fn read_page_labels_roman_end_from_doc(doc: &PdfDocument) -> usize {
        let total = doc.pages().len();
        let mut count: u16 = 0;
        while count < total {
            let Ok(page) = doc.pages().get(count) else { break };
            let expected = Self::to_roman(count + 1);
            if page.label() != Some(expected.as_str()) {
                break;
            }
            count += 1;
        }
        if count == total { 0 } else { count as usize }
    }

    /// Parses hierarchical bookmarks from a `--` delimited `.txt` string (`Serial -- Title -- LogicalPage`),
    /// as well as tab-delimited sidecar lines.
    pub fn parse_bookmarks_txt(
        content: &str,
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
                    let level = if first_char.is_ascii_alphabetic() {
                        1
                    } else {
                        0
                    };
                    let effective_total = total_pages.max(9999);
                    let page_index = Self::resolve_logical_page_str(&page_str, roman_end, total_pages)
                        .or_else(|| {
                            Self::resolve_logical_page_str(&page_str, roman_end, effective_total)
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
                        let logical_page = Self::format_logical_page_label(page_idx, roman_end);
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
        if total_pages == 0 {
            return;
        }
        for bm in bookmarks.iter_mut() {
            if let Some(new_idx) =
                Self::resolve_logical_page_str(&bm.logical_page, roman_end, total_pages)
            {
                bm.page_index = new_idx;
            }
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
        let mut ldoc = lopdf::Document::load_mem(pdf_bytes)
            .context("lopdf could not parse the document for metadata sanitizing")?;

        // 1. Build `/PageLabels` dictionary
        let mut nums_arr = Vec::new();
        if roman_end > 0 {
            let mut roman_dict = lopdf::Dictionary::new();
            roman_dict.set("S", lopdf::Object::Name(b"r".to_vec()));
            nums_arr.push(lopdf::Object::Integer(0));
            nums_arr.push(lopdf::Object::Dictionary(roman_dict));

            let mut arabic_dict = lopdf::Dictionary::new();
            arabic_dict.set("S", lopdf::Object::Name(b"D".to_vec()));
            arabic_dict.set("St", lopdf::Object::Integer(1));
            nums_arr.push(lopdf::Object::Integer(roman_end as i64));
            nums_arr.push(lopdf::Object::Dictionary(arabic_dict));
        } else {
            let mut arabic_dict = lopdf::Dictionary::new();
            arabic_dict.set("S", lopdf::Object::Name(b"D".to_vec()));
            arabic_dict.set("St", lopdf::Object::Integer(1));
            nums_arr.push(lopdf::Object::Integer(0));
            nums_arr.push(lopdf::Object::Dictionary(arabic_dict));
        }

        let mut page_labels_dict = lopdf::Dictionary::new();
        page_labels_dict.set("Nums", lopdf::Object::Array(nums_arr));
        let page_labels_id = ldoc.add_object(lopdf::Object::Dictionary(page_labels_dict));

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
            catalog_mut.set("PageLabels", lopdf::Object::Reference(page_labels_id));
            if let Some(outlines_id) = outlines_id_opt {
                catalog_mut.set("Outlines", lopdf::Object::Reference(outlines_id));
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
            let page_ids: Vec<lopdf::ObjectId> = ldoc.get_pages().values().copied().collect();
            for pid in page_ids {
                if let Ok(lopdf::Object::Dictionary(page_dict)) = ldoc.get_object_mut(pid) {
                    page_dict.remove(b"Thumb");
                    page_dict.remove(b"PieceInfo");
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

    /// Serializes `doc` with `/PageLabels`, `/Outlines` and sanitized metadata.
    ///
    /// Returns the bytes plus `Some(warning)` when metadata could not be sanitized (the raw
    /// bytes are returned then). When `redacted` is true that case is an error instead, so a file
    /// that might still contain redacted content is never produced.
    pub fn serialize_doc_with_metadata(
        doc: &PdfDocument,
        roman_end: usize,
        bookmarks: &[Bookmark],
        redacted: bool,
    ) -> Result<(Vec<u8>, Option<String>)> {
        let raw_bytes = doc
            .save_to_bytes()
            .context("Failed to serialize PDF document to bytes")?;
        match Self::embed_metadata_in_pdf_bytes(&raw_bytes, roman_end, bookmarks, redacted) {
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

    /// Loads bookmarks for `doc`, checking `<pdf_filename>.bookmarks.txt` first so user edits
    /// are preserved, and falling back to `doc.bookmarks().iter()` from the PDF outline tree.
    pub fn load_bookmarks(
        doc: &PdfDocument,
        pdf_path: &Path,
        roman_end: usize,
    ) -> Vec<Bookmark> {
        let sidecar = Self::bookmarks_sidecar_path(pdf_path);
        let total_pages = doc.pages().len() as usize;

        if let Ok(content) = std::fs::read_to_string(&sidecar) {
            return Self::parse_bookmarks_txt(&content, roman_end, total_pages);
        }

        let mut list = Vec::new();
        let mut chap_counter = 0usize;
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
                    let (serial, title, level) =
                        if let Some((prefix, rest)) = raw_title.split_once(". ") {
                            let p_trim = prefix.trim();
                            let first_c = p_trim.chars().next().unwrap_or('1');
                            if p_trim.len() <= 4 && first_c.is_ascii_alphanumeric() {
                                let lvl = if first_c.is_ascii_alphabetic() { 1 } else { 0 };
                                (p_trim.to_string(), rest.trim().to_string(), lvl)
                            } else {
                                chap_counter += 1;
                                (chap_counter.to_string(), raw_title, 0)
                            }
                        } else {
                            chap_counter += 1;
                            (
                                chap_counter.to_string(),
                                if raw_title.is_empty() {
                                    format!("Page {}", idx + 1)
                                } else {
                                    raw_title
                                },
                                0,
                            )
                        };
                    let logical_page = Self::format_logical_page_label(idx, roman_end);
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
        let page_w = page.width().value;
        let page_h = page.height().value;
        let band_height = 55.0_f32;

        let (zone_bottom, zone_left, zone_top, zone_right) = match zone {
            MarginZone::Header => ((page_h - band_height).max(0.0), 0.0, page_h, page_w),
            MarginZone::Footer => (0.0, 0.0, band_height.min(page_h), page_w),
            MarginZone::PageNumber => {
                // Bottom-center 55pt band (centered horizontally across the middle 40% of the page)
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

    /// Highlights text in `pdf_rect` on `page_index` (0-based) of `doc`.
    pub fn highlight_text_in_rect(
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

        let mut target_rects: Vec<PdfRect> = Vec::new();
        {
            let page = doc
                .pages()
                .get(page_index)
                .context("Page index out of bounds for highlighting")?;

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
        }

        if target_rects.is_empty() {
            target_rects.push(normalized_sel);
        }

        let highlight_fill = PdfColor::new(255, 235, 59, 115);

        let mut path_objects = Vec::with_capacity(target_rects.len());
        for rect in &target_rects {
            let padded = PdfRect::new_from_values(
                rect.bottom().value - 1.0,
                rect.left().value - 1.0,
                rect.top().value + 1.0,
                rect.right().value + 1.0,
            );
            let mut path_obj =
                PdfPagePathObject::new_rect(doc, padded, None, None, Some(highlight_fill))?;
            let _ = path_obj.set_blend_mode(PdfPageObjectBlendMode::Multiply);
            path_objects.push(path_obj);
        }

        let mut page = doc
            .pages_mut()
            .get(page_index)
            .context("Page index out of bounds for mutable highlight")?;

        if let Ok(mut annot) = page.annotations_mut().create_highlight_annotation() {
            let _ = annot.set_stroke_color(PdfColor::new(255, 235, 59, 160));
            let _ = annot.set_bounds(normalized_sel);
            for rect in &target_rects {
                let _ = annot
                    .attachment_points_mut()
                    .create_attachment_point_at_end(rect.to_quad_points());
            }
        }

        for path_obj in path_objects {
            page.objects_mut().add_path_object(path_obj)?;
        }

        Ok(target_rects.len())
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
        let ext_doc = self.load_pdf_safe(external_path)?;
        let ext_count = ext_doc.pages().len();
        if ext_count == 0 {
            return Ok(0);
        }
        let dest_idx = target_page_index.min(doc.pages().len());
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

    /// Inserts a blank A4 page into `doc` at `insert_index` (0-based).
    pub fn insert_blank_page_in_doc(doc: &mut PdfDocument, insert_index: u16) -> Result<u16> {
        let clamped_idx = insert_index.min(doc.pages().len());
        doc.pages_mut()
            .create_page_at_index(PdfPagePaperSize::a4(), clamped_idx)
            .context("Failed to insert blank A4 page")?;
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
            let idx = page_num.saturating_sub(1);
            let mut page = doc
                .pages_mut()
                .get(idx)
                .with_context(|| format!("Page {page_num} out of bounds"))?;
            page.set_rotation(rotation);
        }

        Self::save_doc(&doc, output_path).context("Failed to save rotated PDF")
    }

    /// Delete specific pages (`delete_pages`).
    /// `pages` uses 1-based page numbers. Deletes in descending order so indices don't shift.
    pub fn delete_pages<P: AsRef<Path>>(
        &self,
        input_path: P,
        output_path: P,
        pages: &[u16],
    ) -> Result<()> {
        let doc = self
            .load_pdf_file(input_path.as_ref())
            .context("Failed to load PDF for deleting pages")?;

        let mut sorted_pages: Vec<u16> = pages.to_vec();
        sorted_pages.sort_unstable();
        sorted_pages.dedup();

        for &page_num in sorted_pages.iter().rev() {
            let idx = page_num.saturating_sub(1);
            let page = doc
                .pages()
                .get(idx)
                .with_context(|| format!("Page {page_num} out of bounds"))?;
            page.delete()
                .with_context(|| format!("Failed to delete page {page_num}"))?;
        }

        Self::save_doc(&doc, output_path).context("Failed to save PDF after deleting pages")
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

        Self::save_doc(&first_doc, output_path).context("Failed to save merged PDF")
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

        Self::save_doc(&dest_doc, output_path).context("Failed to save extracted PDF")
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
        Self::save_doc(&doc, output_path).context("Failed to save PDF with page numbers")
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
        Self::save_doc(&doc, output_path).context("Failed to save PDF with header/footer")
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
        Self::save_doc(&doc, output_path).context("Failed to save watermarked PDF")
    }

    /// Compresses and optimizes a PDF byte buffer (`compress_pdf(input_bytes) -> Result<Vec<u8>>`):
    /// - Parses the PDF structure via `lopdf::Document::load_mem`
    /// - Prunes unused/orphaned objects (`prune_objects`)
    /// - Deletes zero-length streams (`delete_zero_length_streams`)
    /// - Renumbers objects compactly (`renumber_objects`)
    /// - Compresses uncompressed streams (`compress`)
    pub fn compress_pdf(input_bytes: &[u8]) -> Result<Vec<u8>> {
        let mut ldoc = lopdf::Document::load_mem(input_bytes)
            .context("Failed to parse PDF structure with lopdf for compression")?;
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
            // Add a small 8pt breathing room on left/right of the detected text column
            let pad = 8.0_f32;
            let cl_min_x = (min_x - pad).max(0.0);
            let cl_max_x = (max_x + pad).min(page_w);
            let cl_min_y = min_y.clamp(0.0, page_h);
            let cl_max_y = max_y.clamp(cl_min_y + 1.0, page_h);
            (cl_min_x, cl_min_y, cl_max_x, cl_max_y)
        } else {
            (page_w * 0.12, page_h * 0.08, page_w * 0.88, page_h * 0.92)
        }
    }

    /// Combines 2 or more PDF files in order into a single in-memory `Vec<u8>` buffer (`Shift+M`).
    /// Returns `(combined_pdf_bytes, total_pages)`.
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
                page_chars.push((' ', None));
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
            for lc in uc.to_lowercase() {
                page_chars.push((lc, rect_opt));
            }
        }

        if page_chars.len() < q_chars.len() {
            return Vec::new();
        }

        let mut matches = Vec::new();
        let mut i = 0;
        while i + q_chars.len() <= page_chars.len() {
            let mut matched = true;
            for j in 0..q_chars.len() {
                let pc = page_chars[i + j].0;
                let qc = q_chars[j];
                if pc != qc && !(pc.is_whitespace() && qc.is_whitespace()) {
                    matched = false;
                    break;
                }
            }

            if matched {
                let mut line_rects: Vec<PdfRect> = Vec::new();
                for j in 0..q_chars.len() {
                    if let Some(cb) = page_chars[i + j].1 {
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
                i += q_chars.len();
            } else {
                i += 1;
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
                .unwrap_or_else(|_| PdfRect::new_from_values(0.0, 0.0, 841.89, 595.28));
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
    /// dictionaries. v1.1: reports pages that failed to flatten and whether the permission
    /// removal step actually succeeded, instead of swallowing both.
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
}

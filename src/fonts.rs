//! Milestone 2.1: Lightweight Shared System Font Scanner & PDFium TrueType Embedder.
//!
//! Scans standard OS font directories (`C:\Windows\Fonts`, `/System/Library/Fonts`,
//! `/Library/Fonts`, `/usr/share/fonts`, etc.) using `std::fs`, extracts human-readable
//! family & style names via `ttf-parser` (already transitive in `Cargo.lock` via `ab_glyph`),
//! caches the name-to-path mapping once per session, and embeds selected `.ttf`/`.otf` fonts
//! into PDFium documents via `load_true_type_from_file` with built-in PDF fonts as fallbacks.

use pdfium_render::prelude::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BuiltinPdfFont {
    Helvetica,
    HelveticaBold,
    TimesRoman,
    TimesBold,
    Courier,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemFontEntry {
    pub display_name: String,
    pub path: Option<PathBuf>,
    pub builtin: Option<BuiltinPdfFont>,
}

static FONT_CATALOG: OnceLock<Vec<SystemFontEntry>> = OnceLock::new();

/// Extracts a clean, human-readable font name (e.g. `"Times New Roman Bold"` instead of `timesbd.ttf`)
/// from raw `.ttf` / `.otf` bytes using `ttf-parser`.
pub fn parse_ttf_display_name(bytes: &[u8]) -> Option<String> {
    let face = ttf_parser::Face::parse(bytes, 0).ok()?;
    let mut full_name: Option<String> = None;
    let mut family: Option<String> = None;
    let mut subfamily: Option<String> = None;

    for name in face.names() {
        if !name.is_unicode() {
            continue;
        }
        let Some(text) = name.to_string() else {
            continue;
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Prefer English (0x0409) or default Unicode records
        let is_en = name.language_id == 0x0409 || name.language_id == 0;
        match name.name_id {
            ttf_parser::name_id::FULL_NAME => {
                if full_name.is_none() || is_en {
                    full_name = Some(trimmed.to_string());
                }
            }
            ttf_parser::name_id::TYPOGRAPHIC_FAMILY | ttf_parser::name_id::FAMILY => {
                if family.is_none() || (is_en && name.name_id == ttf_parser::name_id::TYPOGRAPHIC_FAMILY) {
                    family = Some(trimmed.to_string());
                }
            }
            ttf_parser::name_id::TYPOGRAPHIC_SUBFAMILY | ttf_parser::name_id::SUBFAMILY => {
                if subfamily.is_none() || (is_en && name.name_id == ttf_parser::name_id::TYPOGRAPHIC_SUBFAMILY) {
                    subfamily = Some(trimmed.to_string());
                }
            }
            _ => {}
        }
    }

    if let Some(full) = full_name {
        let cleaned = full
            .strip_suffix(" Regular")
            .unwrap_or(&full)
            .trim()
            .to_string();
        if !cleaned.is_empty() {
            return Some(cleaned);
        }
    }
    if let Some(fam) = family {
        let sub = subfamily.unwrap_or_default();
        let combined = if sub.is_empty() || sub.eq_ignore_ascii_case("regular") {
            fam
        } else if fam.to_ascii_lowercase().ends_with(&sub.to_ascii_lowercase()) {
            fam
        } else {
            format!("{fam} {sub}")
        };
        let cleaned = combined.trim().to_string();
        if !cleaned.is_empty() {
            return Some(cleaned);
        }
    }
    None
}

fn os_font_directories() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(target_os = "windows")]
    {
        dirs.push(PathBuf::from(r"C:\Windows\Fonts"));
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Microsoft").join("Windows").join("Fonts"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/System/Library/Fonts"));
        dirs.push(PathBuf::from("/Library/Fonts"));
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(PathBuf::from(home).join("Library").join("Fonts"));
        }
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        dirs.push(PathBuf::from("/usr/share/fonts"));
        dirs.push(PathBuf::from("/usr/local/share/fonts"));
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(PathBuf::from(&home).join(".local").join("share").join("fonts"));
            dirs.push(PathBuf::from(home).join(".fonts"));
        }
    }
    dirs
}

fn collect_font_files(dir: &Path, depth: u8, out: &mut Vec<PathBuf>, limit: usize) {
    if depth > 3 || out.len() >= limit {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= limit {
            break;
        }
        let path = entry.path();
        if path.is_dir() {
            collect_font_files(&path, depth + 1, out, limit);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if ext.eq_ignore_ascii_case("ttf") || ext.eq_ignore_ascii_case("otf") {
                out.push(path);
            }
        }
    }
}

/// Scans standard OS font directories and returns built-in PDF fallback fonts followed by
/// deduplicated, alphabetically sorted OS TrueType/OpenType fonts.
pub fn scan_system_fonts() -> Vec<SystemFontEntry> {
    let mut entries = vec![
        SystemFontEntry {
            display_name: "Helvetica (Built-in)".to_string(),
            path: None,
            builtin: Some(BuiltinPdfFont::Helvetica),
        },
        SystemFontEntry {
            display_name: "Helvetica Bold (Built-in)".to_string(),
            path: None,
            builtin: Some(BuiltinPdfFont::HelveticaBold),
        },
        SystemFontEntry {
            display_name: "Times Roman (Built-in)".to_string(),
            path: None,
            builtin: Some(BuiltinPdfFont::TimesRoman),
        },
        SystemFontEntry {
            display_name: "Times Bold (Built-in)".to_string(),
            path: None,
            builtin: Some(BuiltinPdfFont::TimesBold),
        },
        SystemFontEntry {
            display_name: "Courier (Built-in)".to_string(),
            path: None,
            builtin: Some(BuiltinPdfFont::Courier),
        },
    ];

    let mut files = Vec::new();
    for dir in os_font_directories() {
        if dir.is_dir() {
            collect_font_files(&dir, 0, &mut files, 450);
        }
    }

    let mut seen: HashSet<String> = entries
        .iter()
        .map(|e| e.display_name.to_ascii_lowercase())
        .collect();
    let mut os_entries: Vec<SystemFontEntry> = Vec::new();

    for path in files {
        // Skip very large CJK mega-fonts (> 12 MB) during quick metadata scan to keep startup instant
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > 12 * 1024 * 1024 {
                continue;
            }
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if let Some(name) = parse_ttf_display_name(&bytes) {
            let key = name.to_ascii_lowercase();
            if seen.insert(key) {
                os_entries.push(SystemFontEntry {
                    display_name: name,
                    path: Some(path),
                    builtin: None,
                });
            }
        }
    }

    os_entries.sort_by(|a, b| {
        a.display_name
            .to_ascii_lowercase()
            .cmp(&b.display_name.to_ascii_lowercase())
    });
    entries.extend(os_entries);
    entries
}

/// Returns the lazily cached list of built-in + OS fonts for the current session.
pub fn global_system_fonts() -> &'static [SystemFontEntry] {
    FONT_CATALOG.get_or_init(scan_system_fonts).as_slice()
}

/// Steps to the previous (`delta = -1`) or next (`delta = +1`) font name in the shared catalog.
pub fn step_font_name(current: &str, delta: isize) -> String {
    let catalog = global_system_fonts();
    if catalog.is_empty() {
        return "Helvetica (Built-in)".to_string();
    }
    let idx = catalog
        .iter()
        .position(|e| e.display_name.eq_ignore_ascii_case(current))
        .unwrap_or(0);
    let len = catalog.len() as isize;
    let next = (idx as isize + delta).rem_euclid(len) as usize;
    catalog[next].display_name.clone()
}

/// Loads the requested built-in or OS TrueType font into `doc` via PDFium
/// (`load_true_type_from_file`), falling back safely to Helvetica if unavailable.
pub fn load_pdf_font_by_name<'a>(doc: &mut PdfDocument<'a>, font_name: &str) -> PdfFontToken {
    let catalog = global_system_fonts();
    if let Some(entry) = catalog
        .iter()
        .find(|e| e.display_name.eq_ignore_ascii_case(font_name))
    {
        if let Some(ref path) = entry.path {
            if let Ok(tok) = doc.fonts_mut().load_true_type_from_file(path, true) {
                return tok;
            }
            if let Ok(tok) = doc.fonts_mut().load_true_type_from_file(path, false) {
                return tok;
            }
        }
        if let Some(builtin) = entry.builtin {
            return match builtin {
                BuiltinPdfFont::Helvetica => doc.fonts_mut().helvetica(),
                BuiltinPdfFont::HelveticaBold => doc.fonts_mut().helvetica_bold(),
                BuiltinPdfFont::TimesRoman => doc.fonts_mut().times_roman(),
                BuiltinPdfFont::TimesBold => doc.fonts_mut().times_bold(),
                BuiltinPdfFont::Courier => doc.fonts_mut().courier(),
            };
        }
    }
    if font_name.to_ascii_lowercase().contains("bold") {
        doc.fonts_mut().helvetica_bold()
    } else if font_name.to_ascii_lowercase().contains("times") {
        doc.fonts_mut().times_roman()
    } else if font_name.to_ascii_lowercase().contains("courier") {
        doc.fonts_mut().courier()
    } else {
        doc.fonts_mut().helvetica()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_milestone2_system_font_catalog_and_stepping() {
        let fonts = global_system_fonts();
        assert!(fonts.len() >= 5, "Must always contain at least the 5 built-in PDF fallback fonts");
        assert_eq!(fonts[0].display_name, "Helvetica (Built-in)");
        assert_eq!(fonts[1].display_name, "Helvetica Bold (Built-in)");

        let next = step_font_name("Helvetica (Built-in)", 1);
        assert_eq!(next, "Helvetica Bold (Built-in)");
        let prev = step_font_name(&next, -1);
        assert_eq!(prev, "Helvetica (Built-in)");

        // Verify any discovered OS font has a clean human-readable name (not raw ".ttf" filename)
        for f in fonts.iter().skip(5).take(20) {
            assert!(!f.display_name.to_ascii_lowercase().ends_with(".ttf"));
            assert!(!f.display_name.to_ascii_lowercase().ends_with(".otf"));
            assert!(f.path.is_some());
        }
    }
}

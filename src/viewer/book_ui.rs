//! Phase 2: Apple Books / Foliate-inspired Distinct EPUB UI components.
//!
//! Includes:
//! - Structured `EpubReaderPrefs` (persisted in `micropdf.ini`)
//! - Portable EPUB highlights, notes, and bookmarks (`micropdf_books.ini`, never mutating `.epub`)
//! - Apple Books / Foliate left drawer (cover card, metadata `(i)` popover, collapsible TOC tree,
//!   bottom `Contents | Annotations | Bookmarks` segmented switcher)
//! - `Aa` Typography Popover (font family + curated OS serif stepper, size, spacing, margins,
//!   justification, hyphenation, 2-Page / 1-Page spread, 6 paper theme swatches)
//! - Floating selection pill (`Highlight` swatches | `Add Note` | `Copy`)
//! - Distraction-free EPUB bottom strip (progress scrubber, chapter title, `Loc. X of Y`, `Aa`, Bookmark, Fullscreen)

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::markdown::TocEntry;

use super::render::fill_rect_clipped;
use super::state::*;
use super::theme::{DocKind, UiSkin, UiThemeMode};
use super::ui_draw::*;

pub(super) const EPUB_HIGHLIGHT_COLORS: [(u32, &str); 4] = [
    (0x00FB_BF24, "Amber"),
    (0x0034_D399, "Mint"),
    (0x0060_A5FA, "Sky"),
    (0x00F4_72B6, "Rose"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum EpubFontChoice {
    BookSerif,
    UiSans,
    OsSerif(String),
}

impl Default for EpubFontChoice {
    fn default() -> Self {
        Self::BookSerif
    }
}

impl EpubFontChoice {
    pub(super) fn to_ini_str(&self) -> String {
        match self {
            Self::BookSerif => "book_serif".to_string(),
            Self::UiSans => "ui_sans".to_string(),
            Self::OsSerif(name) => format!("os:{}", name.replace(['\n', '\r'], "")),
        }
    }

    pub(super) fn from_ini_str(s: &str) -> Self {
        let t = s.trim();
        if t.eq_ignore_ascii_case("ui_sans") {
            Self::UiSans
        } else if let Some(name) = t.strip_prefix("os:") {
            let clean = name.trim();
            if clean.is_empty() {
                Self::BookSerif
            } else {
                Self::OsSerif(clean.to_string())
            }
        } else {
            Self::BookSerif
        }
    }

    pub(super) fn resolve_font_family(&self) -> FontFamily {
        match self {
            Self::BookSerif => FontFamily::BookSerif,
            Self::UiSans => FontFamily::UiSans,
            Self::OsSerif(name) => {
                if set_custom_os_font(Some(name)) {
                    FontFamily::CustomOs
                } else {
                    FontFamily::BookSerif
                }
            }
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum LineSpacingPreset {
    Compact,
    Normal,
    Relaxed,
}

impl Default for LineSpacingPreset {
    fn default() -> Self {
        Self::Normal
    }
}

impl LineSpacingPreset {
    pub(super) fn multiplier(self) -> f32 {
        match self {
            Self::Compact => 1.38,
            Self::Normal => 1.56,
            Self::Relaxed => 1.78,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Compact => "Compact",
            Self::Normal => "Normal",
            Self::Relaxed => "Relaxed",
        }
    }

    pub(super) fn to_ini_str(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Normal => "normal",
            Self::Relaxed => "relaxed",
        }
    }

    pub(super) fn from_ini_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "compact" => Self::Compact,
            "relaxed" => Self::Relaxed,
            _ => Self::Normal,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum MarginPreset {
    Narrow,
    Normal,
    Wide,
}

impl Default for MarginPreset {
    fn default() -> Self {
        Self::Normal
    }
}

impl MarginPreset {
    pub(super) fn pad_ratio(self) -> f32 {
        match self {
            Self::Narrow => 0.08,
            Self::Normal => 0.13,
            Self::Wide => 0.19,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Narrow => "Narrow",
            Self::Normal => "Normal",
            Self::Wide => "Wide",
        }
    }

    pub(super) fn to_ini_str(self) -> &'static str {
        match self {
            Self::Narrow => "narrow",
            Self::Normal => "normal",
            Self::Wide => "wide",
        }
    }

    pub(super) fn from_ini_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "narrow" => Self::Narrow,
            "wide" => Self::Wide,
            _ => Self::Normal,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct EpubReaderPrefs {
    pub(super) font_choice: EpubFontChoice,
    pub(super) font_size_px: f32,
    pub(super) line_spacing: LineSpacingPreset,
    pub(super) line_spacing_mult: f32,
    pub(super) word_spacing_px: f32,
    pub(super) paragraph_spacing_mult: f32,
    pub(super) margins: MarginPreset,
    pub(super) margin_ratio: f32,
    pub(super) margin_top: f32,
    pub(super) margin_bottom: f32,
    pub(super) margin_outer: f32,
    pub(super) margin_inner: f32,
    pub(super) justify: bool,
    pub(super) hyphenate: bool,
    pub(super) two_page_spread: bool,
    pub(super) page_curl_anim: bool,
}

impl Default for EpubReaderPrefs {
    fn default() -> Self {
        Self {
            font_choice: EpubFontChoice::BookSerif,
            font_size_px: 16.5,
            line_spacing: LineSpacingPreset::Normal,
            line_spacing_mult: LineSpacingPreset::Normal.multiplier(),
            word_spacing_px: 0.0,
            paragraph_spacing_mult: 1.0,
            margins: MarginPreset::Normal,
            margin_ratio: MarginPreset::Normal.pad_ratio(),
            margin_top: 0.075,
            margin_bottom: 0.060,
            margin_outer: 0.130,
            margin_inner: 0.080,
            justify: true,
            hyphenate: true,
            two_page_spread: true,
            page_curl_anim: true,
        }
    }
}

impl EpubReaderPrefs {
    pub(super) const MARGIN_MIN: f32 = 0.06;
    pub(super) const MARGIN_MAX: f32 = 0.24;

    pub(super) fn margin_part_bounds(part: u8) -> (f32, f32) {
        match part {
            0 => (0.04, 0.20), // Top
            1 => (0.04, 0.20), // Bottom
            2 => (0.04, 0.24), // Outer
            _ => (0.03, 0.20), // Inner / Spine
        }
    }

    pub(super) fn margin_part(&self, part: u8) -> f32 {
        let (lo, hi) = Self::margin_part_bounds(part);
        match part {
            0 => self.margin_top.clamp(lo, hi),
            1 => self.margin_bottom.clamp(lo, hi),
            2 => self.margin_outer.clamp(lo, hi),
            _ => self.margin_inner.clamp(lo, hi),
        }
    }

    pub(super) fn margin_part_norm(&self, part: u8) -> f32 {
        let (lo, hi) = Self::margin_part_bounds(part);
        ((self.margin_part(part) - lo) / (hi - lo)).clamp(0.0, 1.0)
    }

    pub(super) fn set_margin_part(&mut self, part: u8, val: f32) {
        let (lo, hi) = Self::margin_part_bounds(part);
        let c = val.clamp(lo, hi);
        match part {
            0 => self.margin_top = c,
            1 => self.margin_bottom = c,
            2 => {
                self.margin_outer = c;
                self.margin_ratio = c.clamp(Self::MARGIN_MIN, Self::MARGIN_MAX);
                self.margins = if c < 0.105 {
                    MarginPreset::Narrow
                } else if c <= 0.16 {
                    MarginPreset::Normal
                } else {
                    MarginPreset::Wide
                };
            }
            _ => self.margin_inner = c,
        }
    }

    pub(super) fn set_margin_part_norm(&mut self, part: u8, norm: f32) {
        let (lo, hi) = Self::margin_part_bounds(part);
        self.set_margin_part(part, lo + norm.clamp(0.0, 1.0) * (hi - lo));
    }

    pub(super) fn effective_margin_ratio(&self) -> f32 {
        self.margin_outer.clamp(0.04, 0.24)
    }

    pub(super) fn effective_inner_margin_ratio(&self) -> f32 {
        self.margin_inner.clamp(0.03, 0.20)
    }

    pub(super) fn set_margin_ratio(&mut self, ratio: f32) {
        let clamped = ratio.clamp(Self::MARGIN_MIN, Self::MARGIN_MAX);
        self.margin_ratio = clamped;
        self.margin_outer = clamped;
        self.margin_inner = (clamped * 0.62).clamp(0.03, 0.14);
        self.margin_top = (0.045 + ((clamped - Self::MARGIN_MIN) / (Self::MARGIN_MAX - Self::MARGIN_MIN)) * 0.085).clamp(0.04, 0.20);
        self.margin_bottom = (self.margin_top * 0.80).clamp(0.04, 0.20);
        self.margins = if clamped < 0.105 {
            MarginPreset::Narrow
        } else if clamped <= 0.16 {
            MarginPreset::Normal
        } else {
            MarginPreset::Wide
        };
    }

    #[cfg(test)]
    pub(super) fn set_margin_slider_norm(&mut self, norm: f32) {
        let r = Self::MARGIN_MIN + norm.clamp(0.0, 1.0) * (Self::MARGIN_MAX - Self::MARGIN_MIN);
        self.set_margin_ratio(r);
    }

    pub(super) fn set_margin_preset(&mut self, preset: MarginPreset) {
        self.margins = preset;
        self.margin_ratio = preset.pad_ratio();
        match preset {
            MarginPreset::Narrow => {
                self.margin_top = 0.050;
                self.margin_bottom = 0.045;
                self.margin_outer = 0.080;
                self.margin_inner = 0.055;
            }
            MarginPreset::Normal => {
                self.margin_top = 0.075;
                self.margin_bottom = 0.060;
                self.margin_outer = 0.130;
                self.margin_inner = 0.080;
            }
            MarginPreset::Wide => {
                self.margin_top = 0.115;
                self.margin_bottom = 0.095;
                self.margin_outer = 0.190;
                self.margin_inner = 0.110;
            }
        }
    }

    /// Computes independent `(top_pad_px, bottom_pad_px)` from `margin_top` and `margin_bottom`.
    /// Adjusting top margin NEVER widens the inner spine gap or bottom margin.
    pub(super) fn vertical_pad_px(&self, spread_h: f32, s: f32) -> (f32, f32) {
        let top_pad = (spread_h * self.margin_part(0))
            .clamp(22.0 * s, (spread_h * 0.24).max(22.0 * s));
        let bottom_pad = (spread_h * self.margin_part(1))
            .clamp(20.0 * s, (spread_h * 0.22).max(20.0 * s));
        (top_pad, bottom_pad)
    }

    pub(super) fn effective_line_mult(&self) -> f32 {
        self.line_spacing_mult.clamp(1.20, 2.20)
    }

    pub(super) fn set_line_spacing_preset(&mut self, preset: LineSpacingPreset) {
        self.line_spacing = preset;
        self.line_spacing_mult = preset.multiplier();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct EpubHighlight {
    pub(super) id: u64,
    pub(super) block_idx: usize,
    pub(super) start_word: usize,
    pub(super) end_word: usize,
    pub(super) quote: String,
    pub(super) color_idx: u8,
    pub(super) note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct EpubBookmark {
    pub(super) block_idx: usize,
    pub(super) page_num: usize,
    pub(super) label: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct EpubBookData {
    pub(super) highlights: Vec<EpubHighlight>,
    pub(super) bookmarks: Vec<EpubBookmark>,
    pub(super) next_id: u64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub(super) enum BookSidebarTab {
    #[default]
    Contents,
    Annotations,
    Bookmarks,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) enum BookSidebarHit {
    ToggleSearch,
    ToggleExpandAll,
    CollapseSidebar,
    BookInfo,
    TocToggle(usize),
    TocRow(usize),
    AnnotRow(usize),
    AnnotDelete(usize),
    BookmarkRow(usize),
    BookmarkDelete(usize),
    ScrollbarTrack(f32),
    Tab(BookSidebarTab),
}

impl Eq for BookSidebarHit {}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) enum AaPopoverHit {
    FontBookSerif,
    FontUiSans,
    FontOsPrev,
    FontOsNext,
    SizeMinus,
    SizePlus,
    CodeSizeMinus,
    CodeSizePlus,
    Spacing(LineSpacingPreset),
    LineSpacingMinus,
    LineSpacingPlus,
    WordSpacingMinus,
    WordSpacingPlus,
    ParaSpacingMinus,
    ParaSpacingPlus,
    OpenMarginsSubmenu,
    CloseMarginsSubmenu,
    ResetMargins,
    Margin(MarginPreset),
    MarginPartSlider(u8, f32),
    ToggleJustify,
    ToggleHyphenate,
    SpreadTwoPage,
    SpreadOnePage,
    TogglePageCurl,
    ThemeFilter(ReadingFilter),
    InsidePopover,
}

impl Eq for AaPopoverHit {}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum BookSelectionPillHit {
    HighlightColor(u8),
    AddNote,
    Copy,
    InsidePill,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum BookInfoPopoverHit {
    Close,
    Inside,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) enum EpubBottomHit {
    ToggleSidebar,
    Scrubber(f32),
    FindInDoc,
    ToggleAa,
    ToggleBookmark,
    ToggleFullscreen,
}

impl Eq for EpubBottomHit {}

#[derive(Clone, Debug)]
pub(super) struct EpubWordHit {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) w: u32,
    pub(super) h: u32,
    pub(super) block_idx: usize,
    pub(super) word_idx: usize,
    pub(super) text: String,
}

impl EpubWordHit {
    pub(super) fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w as i32 && py >= self.y && py < self.y + self.h as i32
    }
}

#[derive(Clone, Debug)]
struct FlatTocRow<'a> {
    node_id: usize,
    entry: &'a TocEntry,
    has_children: bool,
    is_collapsed: bool,
}

// ---------------------------------------------------------------------------
// Portable `micropdf_books.ini` persistence (never mutates .epub files)
// ---------------------------------------------------------------------------

fn escape_ini_field(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('|', "\\p")
        .replace('\n', "\\n")
        .replace('\r', "")
}

fn unescape_ini_field(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('p') => out.push('|'),
                Some('n') => out.push('\n'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub(super) fn serialize_books_ini(all_books: &BTreeMap<String, EpubBookData>) -> String {
    let mut out = String::from("# MicroPDF Portable EPUB Highlights, Notes & Bookmarks\n");
    for (book_key, data) in all_books {
        if data.highlights.is_empty() && data.bookmarks.is_empty() {
            continue;
        }
        out.push_str(&format!("book={}\n", escape_ini_field(book_key)));
        for h in &data.highlights {
            let note_str = h.note.as_deref().unwrap_or("");
            out.push_str(&format!(
                "hl={}|{}|{}|{}|{}|{}|{}\n",
                h.id,
                h.block_idx,
                h.start_word,
                h.end_word,
                h.color_idx,
                escape_ini_field(&h.quote),
                escape_ini_field(note_str),
            ));
        }
        for b in &data.bookmarks {
            out.push_str(&format!(
                "bm={}|{}|{}\n",
                b.block_idx,
                b.page_num,
                escape_ini_field(&b.label),
            ));
        }
    }
    out
}

pub(super) fn parse_books_ini(content: &str) -> BTreeMap<String, EpubBookData> {
    let mut map: BTreeMap<String, EpubBookData> = BTreeMap::new();
    let mut current_key: Option<String> = None;

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(val) = line.strip_prefix("book=") {
            let key = unescape_ini_field(val);
            if !key.is_empty() {
                map.entry(key.clone()).or_default();
                current_key = Some(key);
            }
            continue;
        }
        let Some(ref bkey) = current_key else {
            continue;
        };
        let Some(entry) = map.get_mut(bkey) else {
            continue;
        };
        if let Some(val) = line.strip_prefix("hl=") {
            let parts: Vec<&str> = val.splitn(7, '|').collect();
            if parts.len() == 7 {
                let id = parts[0].parse::<u64>().unwrap_or(entry.next_id + 1);
                let block_idx = parts[1].parse::<usize>().unwrap_or(0);
                let start_word = parts[2].parse::<usize>().unwrap_or(0);
                let end_word = parts[3].parse::<usize>().unwrap_or(start_word);
                let color_idx = parts[4].parse::<u8>().unwrap_or(0).min(3);
                let quote = unescape_ini_field(parts[5]);
                let note_raw = unescape_ini_field(parts[6]);
                let note = if note_raw.is_empty() {
                    None
                } else {
                    Some(note_raw)
                };
                entry.next_id = entry.next_id.max(id + 1);
                entry.highlights.push(EpubHighlight {
                    id,
                    block_idx,
                    start_word,
                    end_word,
                    quote,
                    color_idx,
                    note,
                });
            }
        } else if let Some(val) = line.strip_prefix("bm=") {
            let parts: Vec<&str> = val.splitn(3, '|').collect();
            if parts.len() == 3 {
                let block_idx = parts[0].parse::<usize>().unwrap_or(0);
                let page_num = parts[1].parse::<usize>().unwrap_or(1);
                let label = unescape_ini_field(parts[2]);
                entry.bookmarks.push(EpubBookmark {
                    block_idx,
                    page_num,
                    label,
                });
            }
        }
    }
    map
}

pub(super) fn curated_os_book_fonts() -> Vec<String> {
    let preferred = [
        "georgia",
        "palatino",
        "charter",
        "garamond",
        "baskerville",
        "bookerly",
        "literata",
        "iowan",
        "hoefler",
        "times new roman",
        "liberation serif",
        "dejavu serif",
        "noto serif",
        "cambria",
    ];
    let catalog = crate::fonts::global_system_fonts();
    let mut out = Vec::new();
    for entry in catalog {
        if entry.path.is_none() {
            continue;
        }
        let lower = entry.display_name.to_ascii_lowercase();
        if lower.contains("bold") || lower.contains("italic") || lower.contains("oblique") {
            continue;
        }
        if preferred.iter().any(|p| lower.contains(p)) {
            out.push(entry.display_name.clone());
        }
    }
    if out.is_empty() {
        for entry in catalog.iter().filter(|e| e.path.is_some()).take(8) {
            out.push(entry.display_name.clone());
        }
    }
    out
}

fn flatten_toc<'a>(
    entries: &'a [TocEntry],
    collapsed: &std::collections::HashSet<usize>,
    filter: Option<&str>,
    next_id: &mut usize,
    out: &mut Vec<FlatTocRow<'a>>,
) {
    let needle = filter.map(|f| f.trim().to_ascii_lowercase()).filter(|f| !f.is_empty());
    for entry in entries {
        let id = *next_id;
        *next_id += 1;
        let is_collapsed = collapsed.contains(&id);
        let has_children = !entry.children.is_empty();
        if let Some(ref q) = needle {
            if entry.title.to_ascii_lowercase().contains(q) {
                out.push(FlatTocRow {
                    node_id: id,
                    entry,
                    has_children,
                    is_collapsed: false,
                });
            }
            flatten_toc(&entry.children, collapsed, filter, next_id, out);
        } else {
            out.push(FlatTocRow {
                node_id: id,
                entry,
                has_children,
                is_collapsed,
            });
            if has_children && !is_collapsed {
                flatten_toc(&entry.children, collapsed, filter, next_id, out);
            } else {
                // Keep deterministic node IDs even when parent is collapsed
                advance_toc_ids(&entry.children, next_id);
            }
        }
    }
}

fn advance_toc_ids(entries: &[TocEntry], next_id: &mut usize) {
    for e in entries {
        *next_id += 1;
        advance_toc_ids(&e.children, next_id);
    }
}

impl<'a> PdfViewerApp<'a> {
    pub(super) fn is_distinct_epub_mode(&self) -> bool {
        self.ui_skin == UiSkin::Distinct && self.active_doc_kind() == DocKind::Epub
    }

    pub(super) fn portable_books_ini_path(&self) -> PathBuf {
        let mut p = self.portable_session_ini_path();
        p.set_file_name("micropdf_books.ini");
        p
    }

    fn fallback_os_books_ini_path() -> PathBuf {
        let mut p = Self::fallback_os_config_ini_path();
        p.set_file_name("micropdf_books.ini");
        p
    }

    fn active_book_storage_key(&self) -> Option<String> {
        if self.file_path.as_os_str().is_empty() || self.active_doc_kind() != DocKind::Epub {
            return None;
        }
        let resolved = std::fs::canonicalize(&self.file_path)
            .unwrap_or_else(|_| self.file_path.clone());
        Some(resolved.to_string_lossy().to_string())
    }

    pub(super) fn load_epub_book_data(&mut self) {
        self.epub_selection_drag = None;
        self.epub_selection_pill_rect = None;
        self.epub_word_rects.clear();
        self.epub_cover_thumb = None;
        self.book_toc_collapsed.clear();
        self.book_toc_search = None;
        self.book_info_popover = false;

        let Some(key) = self.active_book_storage_key() else {
            self.epub_book_data = EpubBookData::default();
            return;
        };
        let primary = self.portable_books_ini_path();
        let fallback = Self::fallback_os_books_ini_path();
        let raw = std::fs::read_to_string(&primary)
            .or_else(|_| std::fs::read_to_string(&fallback))
            .unwrap_or_default();
        let mut all = parse_books_ini(&raw);
        self.epub_book_data = all.remove(&key).unwrap_or_default();
    }

    pub(super) fn save_epub_book_data(&self) {
        let Some(key) = self.active_book_storage_key() else {
            return;
        };
        let primary = self.portable_books_ini_path();
        let fallback = Self::fallback_os_books_ini_path();
        let raw = std::fs::read_to_string(&primary)
            .or_else(|_| std::fs::read_to_string(&fallback))
            .unwrap_or_default();
        let mut all = parse_books_ini(&raw);
        if self.epub_book_data.highlights.is_empty() && self.epub_book_data.bookmarks.is_empty() {
            all.remove(&key);
        } else {
            all.insert(key, self.epub_book_data.clone());
        }
        let text = serialize_books_ini(&all);
        if std::fs::write(&primary, text.as_bytes()).is_err() {
            if let Some(parent) = fallback.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&fallback, text.as_bytes());
        }
    }

    pub(super) fn ensure_epub_cover_thumb(&mut self) {
        let Some(ref doc) = self.markdown_doc else {
            self.epub_cover_thumb = None;
            return;
        };
        let Some(key) = doc
            .book_meta
            .as_ref()
            .and_then(|m| m.cover_image_key.as_ref())
        else {
            self.epub_cover_thumb = None;
            return;
        };
        if self
            .epub_cover_thumb
            .as_ref()
            .is_some_and(|(k, _, _, _)| k == key)
        {
            return;
        }
        let Some(bytes) = doc.embedded_images.get(key) else {
            self.epub_cover_thumb = None;
            return;
        };
        let Ok(im) = image::load_from_memory(bytes) else {
            self.epub_cover_thumb = None;
            return;
        };
        let rgb = im.to_rgb8();
        let (tw, th) = (92u32, 132u32);
        let resized = image::imageops::resize(&rgb, tw, th, image::imageops::FilterType::Triangle);
        let mut px = Vec::with_capacity((tw * th) as usize);
        for p in resized.pixels() {
            let [r, g, b] = p.0;
            px.push(((r as u32) << 16) | ((g as u32) << 8) | (b as u32));
        }
        self.epub_cover_thumb = Some((key.clone(), tw, th, px));
    }

    pub(super) fn active_epub_chapter_title_and_block(&self) -> (String, usize) {
        let cur_block = self.current_visible_epub_block_idx();
        let Some(ref doc) = self.markdown_doc else {
            return (String::new(), cur_block);
        };
        if let Some(ref meta) = doc.book_meta {
            let mut best: Option<(usize, String)> = None;
            fn walk(entries: &[TocEntry], cur_block: usize, best: &mut Option<(usize, String)>) {
                for e in entries {
                    if e.block_idx <= cur_block {
                        if best.as_ref().is_none_or(|(b, _)| e.block_idx >= *b) {
                            *best = Some((e.block_idx, e.title.clone()));
                        }
                    }
                    walk(&e.children, cur_block, best);
                }
            }
            walk(&meta.toc, cur_block, &mut best);
            if let Some((_, t)) = best {
                return (t, cur_block);
            }
            return (meta.title.clone(), cur_block);
        }
        (doc.title.clone(), cur_block)
    }

    pub(super) fn toggle_current_epub_bookmark(&mut self) {
        let (chap_title, cur_block) = self.active_epub_chapter_title_and_block();
        let page_num = self.current_epub_loc_pair().0;
        if let Some(pos) = self
            .epub_book_data
            .bookmarks
            .iter()
            .position(|b| b.page_num == page_num || b.block_idx == cur_block)
        {
            let removed = self.epub_book_data.bookmarks.remove(pos);
            self.save_epub_book_data();
            self.status_notice = Some(format!("Removed bookmark ({})", removed.label));
        } else {
            let label = if chap_title.is_empty() {
                format!("Loc. {page_num}")
            } else {
                chap_title
            };
            self.epub_book_data.bookmarks.push(EpubBookmark {
                block_idx: cur_block,
                page_num,
                label: label.clone(),
            });
            self.save_epub_book_data();
            self.status_notice = Some(format!("Bookmarked Loc. {page_num} — {label}"));
        }
        self.request_redraw();
    }

    pub(super) fn current_epub_page_is_bookmarked(&self) -> bool {
        let (cur_loc, _) = self.current_epub_loc_pair();
        let cur_blk = self.current_visible_epub_block_idx();
        self.epub_book_data
            .bookmarks
            .iter()
            .any(|b| b.page_num == cur_loc || b.block_idx == cur_blk)
    }

    pub(super) fn current_epub_loc_pair(&self) -> (usize, usize) {
        if self.is_distinct_epub_mode() && self.epub_prefs.two_page_spread {
            if let Some((l, _, total)) = self.epub_book_page_info_cached() {
                return (l, total);
            }
        }
        let max_scroll = self.md_layout.as_ref().map(|c| c.total_h).unwrap_or(1.0).max(1.0);
        let total_locs = ((max_scroll / 420.0).ceil() as usize).max(1);
        let cur = ((self.scroll_y / 420.0).floor() as usize + 1).clamp(1, total_locs);
        (cur, total_locs)
    }

    pub(super) fn current_visible_epub_block_idx(&self) -> usize {
        if self.is_distinct_epub_mode() && self.epub_prefs.two_page_spread {
            if let Some(blk) = self.epub_book_current_first_block() {
                return blk;
            }
        }
        let Some(ref cache) = self.md_layout else {
            return 0;
        };
        let mut best_blk = 0usize;
        let mut best_y = -1.0f32;
        for (&ast_idx, &by) in &cache.block_y_by_ast_idx {
            if by <= self.scroll_y + 32.0 && by >= best_y {
                best_y = by;
                best_blk = ast_idx;
            }
        }
        best_blk
    }

    // -----------------------------------------------------------------------
    // Apple Books / Foliate Left Book Sidebar
    // -----------------------------------------------------------------------

    fn book_sidebar_layout(s: f32) -> (u32, u32, u32) {
        let util_h = (34.0 * s).round() as u32;
        let card_h = (92.0 * s).round() as u32;
        let seg_h = (40.0 * s).round() as u32;
        (util_h, card_h, seg_h)
    }

    pub(super) fn epub_sidebar_list_metrics(
        &self,
        buf_h: u32,
        top_bars_h: u32,
        sf: f64,
    ) -> (i32, i32, u32, usize, usize) {
        let s = sf as f32;
        let top_y = top_bars_h as i32;
        let total_h = buf_h.saturating_sub(top_bars_h) as i32;
        let (util_h, card_h, seg_h) = Self::book_sidebar_layout(s);
        let search_bar_h = if self.book_toc_search.is_some() && self.book_sidebar_tab == BookSidebarTab::Contents {
            (32.0 * s).round() as i32
        } else {
            0
        };
        let list_top_y = top_y + util_h as i32 + card_h as i32 + search_bar_h;
        let seg_y = top_y + total_h - seg_h as i32;
        let list_bot_y = (seg_y - 2).max(list_top_y + 1);
        let item_h = match self.book_sidebar_tab {
            BookSidebarTab::Annotations => (60.0 * s).round().max(36.0) as u32,
            BookSidebarTab::Contents | BookSidebarTab::Bookmarks => (38.0 * s).round().max(26.0) as u32,
        };
        let total_items = match self.book_sidebar_tab {
            BookSidebarTab::Contents => {
                let toc: &[TocEntry] = self
                    .markdown_doc
                    .as_ref()
                    .and_then(|d| d.book_meta.as_ref())
                    .map(|m| m.toc.as_slice())
                    .unwrap_or(&[]);
                let mut rows = Vec::new();
                let mut nid = 0usize;
                flatten_toc(toc, &self.book_toc_collapsed, self.book_toc_search.as_deref(), &mut nid, &mut rows);
                rows.len()
            }
            BookSidebarTab::Annotations => self.epub_book_data.highlights.len(),
            BookSidebarTab::Bookmarks => self.epub_book_data.bookmarks.len(),
        };
        let avail_h = (list_bot_y - list_top_y).max(0) as u32;
        let visible_items = ((avail_h / item_h.max(1)) as usize).max(1);
        (list_top_y, list_bot_y, item_h, total_items, visible_items)
    }

    pub(super) fn draw_distinct_epub_sidebar(
        &mut self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        scale_factor: f64,
    ) {
        self.ensure_epub_cover_thumb();
        let theme = self.ui_theme();
        let s = scale_factor as f32;
        let top_y = top_bars_h as i32;
        let total_h = buf_h.saturating_sub(top_bars_h);
        let (util_h, card_h, seg_h) = Self::book_sidebar_layout(s);
        let search_bar_h = if self.book_toc_search.is_some() && self.book_sidebar_tab == BookSidebarTab::Contents {
            (32.0 * s).round() as u32
        } else {
            0
        };
        let pad = (14.0 * s).round() as i32;
        let left_margin = (18.0 * s).round() as i32;
        let rad = (6.0 * s).max(4.0);

        // Background + right subtle border
        fill_rect_clipped(buffer, buf_w, buf_h, 0, top_y, sidebar_w, total_h, theme.sidebar_bg);
        fill_rect_clipped(buffer, buf_w, buf_h, sidebar_w.saturating_sub(1) as i32, top_y, 1, total_h, theme.divider);

        // 1. Top utility row: Search | Expand/Collapse | Pin/Close
        fill_rect_clipped(buffer, buf_w, buf_h, 0, top_y, sidebar_w.saturating_sub(1), util_h, theme.sidebar_header_bg);
        fill_rect_clipped(buffer, buf_w, buf_h, 0, top_y + util_h as i32 - 1, sidebar_w.saturating_sub(1), 1, theme.divider);

        let btn_sz = (22.0 * s).round() as u32;
        let ic_sz = (12.0 * s).round() as u32;
        let uy = top_y + ((util_h.saturating_sub(btn_sz)) / 2) as i32;

        let search_x = pad;
        let exp_x = search_x + btn_sz as i32 + (4.0 * s).round() as i32;
        let pin_x = sidebar_w as i32 - pad - btn_sz as i32;

        for (bx, ic, hit, active) in [
            (search_x, UiIcon::Search, BookSidebarHit::ToggleSearch, self.book_toc_search.is_some()),
            (exp_x, UiIcon::TextView, BookSidebarHit::ToggleExpandAll, !self.book_toc_collapsed.is_empty()),
            (pin_x, UiIcon::Sidebar, BookSidebarHit::CollapseSidebar, false),
        ] {
            let hov = self.hovered_widget == Some(UiHit::BookSidebar(hit));
            if active {
                fill_rounded_rect(buffer, buf_w, buf_h, bx, uy, btn_sz, btn_sz, 5.0 * s, theme.accent_soft_bg);
            } else if hov {
                fill_rounded_rect(buffer, buf_w, buf_h, bx, uy, btn_sz, btn_sz, 5.0 * s, theme.hover_bg);
            }
            draw_icon(
                buffer, buf_w, buf_h, ic,
                bx + ((btn_sz.saturating_sub(ic_sz)) / 2) as i32,
                uy + ((btn_sz.saturating_sub(ic_sz)) / 2) as i32,
                ic_sz,
                if active { theme.accent_soft_fg } else if hov { theme.text_primary } else { theme.text_secondary },
            );
        }

        // 2. Book Header Card (Cover thumbnail + Title + Author + Chapter X of Y + (i) info button)
        let card_y = top_y + util_h as i32;
        fill_rect_clipped(buffer, buf_w, buf_h, 0, card_y, sidebar_w.saturating_sub(1), card_h, theme.sidebar_header_bg);
        fill_rect_clipped(buffer, buf_w, buf_h, 0, card_y + card_h as i32 - 1, sidebar_w.saturating_sub(1), 1, theme.divider);

        let thumb_w = (46.0 * s).round() as u32;
        let thumb_h = (66.0 * s).round() as u32;
        let thumb_x = pad;
        let thumb_y = card_y + ((card_h.saturating_sub(thumb_h)) / 2) as i32;

        if let Some((_, sw, sh, ref px)) = self.epub_cover_thumb {
            draw_soft_shadow(buffer, buf_w, buf_h, thumb_x, thumb_y, thumb_w, thumb_h, 3.0 * s, (4.0 * s).round() as i32, 2, theme.shadow_rgb, 48);
            for dy in 0..thumb_h {
                let py = thumb_y + dy as i32;
                if py < 0 || py >= buf_h as i32 {
                    continue;
                }
                let sy = ((dy as u64 * sh as u64) / thumb_h as u64).min(sh as u64 - 1) as usize;
                let dst_row = py as usize * buf_w as usize;
                let src_row = sy * sw as usize;
                for dx in 0..thumb_w {
                    let px_x = thumb_x + dx as i32;
                    if px_x < 0 || px_x >= buf_w as i32 {
                        continue;
                    }
                    let sx = ((dx as u64 * sw as u64) / thumb_w as u64).min(sw as u64 - 1) as usize;
                    buffer[dst_row + px_x as usize] = px[src_row + sx];
                }
            }
            stroke_rounded_rect(buffer, buf_w, buf_h, thumb_x, thumb_y, thumb_w, thumb_h, 2.5, theme.border_subtle);
        } else {
            fill_rounded_rect(buffer, buf_w, buf_h, thumb_x, thumb_y, thumb_w, thumb_h, 4.0 * s, theme.accent_soft_bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, thumb_x, thumb_y, thumb_w, thumb_h, 4.0 * s, theme.border_subtle);
            draw_doc_type_icon(buffer, buf_w, buf_h, DocKind::Epub, thumb_x + (thumb_w as i32 - 20) / 2, thumb_y + (thumb_h as i32 - 20) / 2, 20, self.ui_theme_mode == UiThemeMode::Dark);
        }

        let (book_title, book_author) = if let Some(ref doc) = self.markdown_doc {
            if let Some(ref meta) = doc.book_meta {
                (meta.title.clone(), meta.author.clone())
            } else {
                (doc.title.clone(), None)
            }
        } else {
            (self.filename_display(), None)
        };

        let cur_block = self.current_visible_epub_block_idx();
        let (chap_cur, chap_total) = if let Some(toc) = self.markdown_doc.as_ref().and_then(|d| d.book_meta.as_ref()).map(|m| m.toc.as_slice()) {
            let mut flattened = Vec::new();
            let mut nid = 0usize;
            let empty = std::collections::HashSet::new();
            flatten_toc(toc, &empty, None, &mut nid, &mut flattened);
            let top_chapters: Vec<usize> = flattened.iter().filter(|r| r.entry.level == 0).map(|r| r.entry.block_idx).collect();
            let pool: Vec<usize> = if !top_chapters.is_empty() {
                top_chapters
            } else {
                flattened.iter().map(|r| r.entry.block_idx).collect()
            };
            let tot = pool.len();
            let idx = pool.iter().enumerate().filter(|&(_, &b)| b <= cur_block).map(|(i, _)| i + 1).last().unwrap_or(1).min(tot.max(1));
            (idx, tot)
        } else {
            (0, 0)
        };

        let info_sz = (18.0 * s).round() as u32;
        let info_x = sidebar_w as i32 - pad - info_sz as i32;
        let info_y = card_y + card_h as i32 - info_sz as i32 - (10.0 * s).round() as i32;
        let text_x = thumb_x + thumb_w as i32 + (10.0 * s).round() as i32;
        let text_max_x = sidebar_w as i32 - pad - 4;
        let title_sz = (13.5 * s).clamp(11.0, 22.0);
        let auth_sz = (11.5 * s).clamp(9.5, 19.0);
        let chap_sub_sz = (11.0 * s).clamp(9.0, 18.0);
        let title_budget = (text_max_x - text_x).max(20) as f32;
        let title_clipped = truncate_ellipsis_styled(
            &book_title,
            title_budget,
            TextStyle::book(title_sz, true, false),
        );
        draw_text_styled_clipped(
            buffer, buf_w, buf_h, text_x, thumb_y + (2.0 * s).round() as i32, text_max_x,
            &title_clipped, TextStyle::book(title_sz, true, false), theme.text_primary,
        );
        if let Some(ref auth) = book_author {
            let auth_clipped = truncate_ellipsis(auth, (info_x - text_x - 6).max(20) as f32, auth_sz, false);
            draw_text_clipped(
                buffer, buf_w, buf_h, text_x, thumb_y + (24.0 * s).round() as i32, info_x - 4,
                &auth_clipped, auth_sz, theme.text_muted, false,
            );
        }
        if chap_total > 0 {
            let chap_lbl = format!("• Chapter {chap_cur} of {chap_total}");
            let chap_clipped = truncate_ellipsis(&chap_lbl, (info_x - text_x - 6).max(20) as f32, chap_sub_sz, false);
            draw_text_clipped(
                buffer, buf_w, buf_h, text_x, thumb_y + (43.0 * s).round() as i32, info_x - 4,
                &chap_clipped, chap_sub_sz, theme.text_secondary, false,
            );
        }
        let info_hov = self.hovered_widget == Some(UiHit::BookSidebar(BookSidebarHit::BookInfo)) || self.book_info_popover;
        fill_circle_aa(
            buffer, buf_w, buf_h,
            info_x as f32 + info_sz as f32 * 0.5,
            info_y as f32 + info_sz as f32 * 0.5,
            info_sz as f32 * 0.44,
            if info_hov { theme.accent_soft_bg } else { theme.surface_elevated },
        );
        stroke_circle_aa(
            buffer, buf_w, buf_h,
            info_x as f32 + info_sz as f32 * 0.5,
            info_y as f32 + info_sz as f32 * 0.5,
            info_sz as f32 * 0.44,
            1.1,
            if info_hov { theme.accent } else { theme.border_subtle },
        );
        let i_sz = (10.5 * s).clamp(8.5, 16.0);
        let iw = measure_text("i", i_sz, true);
        draw_text_vcenter(
            buffer, buf_w, buf_h,
            info_x + ((info_sz as f32 - iw) * 0.5).round() as i32,
            info_y, info_sz, info_x + info_sz as i32,
            "i", i_sz, if info_hov { theme.accent_soft_fg } else { theme.text_secondary }, true,
        );

        // Optional inline TOC search filter bar
        let list_top_y = card_y + card_h as i32 + search_bar_h as i32;
        if let Some(ref q) = self.book_toc_search {
            if self.book_sidebar_tab == BookSidebarTab::Contents {
                let sy = card_y + card_h as i32 + (4.0 * s).round() as i32;
                let sh = search_bar_h.saturating_sub((8.0 * s).round() as u32);
                let sw_box = sidebar_w.saturating_sub((pad * 2 + 1) as u32);
                fill_rounded_rect(buffer, buf_w, buf_h, pad, sy, sw_box, sh, 5.0 * s, theme.input_bg);
                stroke_rounded_rect(buffer, buf_w, buf_h, pad, sy, sw_box, sh, 5.0 * s, theme.input_focus_border);
                let display = if q.is_empty() { "Filter contents..." } else { q.as_str() };
                let col = if q.is_empty() { theme.text_muted } else { theme.text_primary };
                draw_text_vcenter(buffer, buf_w, buf_h, pad + 8, sy, sh, pad + sw_box as i32 - 6, display, auth_sz, col, false);
            }
        }

        // Bottom 3-tab segmented switcher: Contents | Notes | Marks
        let seg_y = top_y + total_h as i32 - seg_h as i32;
        fill_rect_clipped(buffer, buf_w, buf_h, 0, seg_y, sidebar_w.saturating_sub(1), seg_h, theme.sidebar_header_bg);
        fill_rect_clipped(buffer, buf_w, buf_h, 0, seg_y, sidebar_w.saturating_sub(1), 1, theme.divider);

        let pill_pad = (8.0 * s).round() as i32;
        let track_x = pill_pad;
        let track_y = seg_y + (5.0 * s).round() as i32;
        let track_w = sidebar_w.saturating_sub((pill_pad * 2 + 1) as u32);
        let track_h = seg_h.saturating_sub((10.0 * s).round() as u32);
        fill_rounded_rect(buffer, buf_w, buf_h, track_x, track_y, track_w, track_h, rad, theme.surface_elevated);
        stroke_rounded_rect(buffer, buf_w, buf_h, track_x, track_y, track_w, track_h, rad, theme.border_subtle);

        let tabs = [
            (BookSidebarTab::Contents, "Contents"),
            (BookSidebarTab::Annotations, "Notes"),
            (BookSidebarTab::Bookmarks, "Marks"),
        ];
        let seg_w = track_w / 3;
        let seg_sz = (12.0 * s).clamp(9.5, 19.0);
        for (i, (tab, lbl)) in tabs.iter().enumerate() {
            let bx = track_x + (i as i32) * (seg_w as i32);
            let bw = if i == 2 { (track_x + track_w as i32 - bx).max(16) as u32 } else { seg_w };
            let is_active = self.book_sidebar_tab == *tab;
            let hov = self.hovered_widget == Some(UiHit::BookSidebar(BookSidebarHit::Tab(*tab)));
            if is_active {
                fill_rounded_rect(buffer, buf_w, buf_h, bx + 2, track_y + 2, bw.saturating_sub(4), track_h.saturating_sub(4), (rad - 1.5).max(3.0), theme.surface_bg);
            } else if hov {
                fill_rounded_rect(buffer, buf_w, buf_h, bx + 2, track_y + 2, bw.saturating_sub(4), track_h.saturating_sub(4), (rad - 1.5).max(3.0), theme.hover_bg);
            }
            let tw = measure_text(lbl, seg_sz, is_active);
            let fg = if is_active { theme.accent_soft_fg } else if hov { theme.text_primary } else { theme.text_secondary };
            draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round() as i32, track_y, track_h, bx + bw as i32 - 2, lbl, seg_sz, fg, is_active);
        }

        // Scrollable middle list area (Tweak 1 & Tweak 9)
        let list_bot_y = seg_y - 2;
        let row_h = (38.0 * s).round().max(26.0) as u32;
        let main_sz = (16.5 * s).clamp(13.0, 26.0);
        let sub_sz = (14.5 * s).clamp(11.5, 23.0);
        let loc_sz = (13.0 * s).clamp(10.5, 20.0);

        let list_total: usize;
        let mut list_item_h = row_h;

        let (_, _, _, metric_tot, metric_vis) = self.epub_sidebar_list_metrics(buf_h, top_bars_h, scale_factor);
        let has_scrollbar = metric_tot > metric_vis;
        let sb_w = (7.0 * s).round().max(5.0) as u32;
        let sb_reserve = if has_scrollbar { (14.0 * s).round() as i32 } else { (6.0 * s).round() as i32 };
        let content_right = sidebar_w as i32 - sb_reserve;

        match self.book_sidebar_tab {
            BookSidebarTab::Contents => {
                let toc: &[TocEntry] = self
                    .markdown_doc
                    .as_ref()
                    .and_then(|d| d.book_meta.as_ref())
                    .map(|m| m.toc.as_slice())
                    .unwrap_or(&[]);
                let mut rows = Vec::new();
                let mut nid = 0usize;
                flatten_toc(toc, &self.book_toc_collapsed, self.book_toc_search.as_deref(), &mut nid, &mut rows);
                list_total = rows.len();

                if rows.is_empty() {
                    draw_text(buffer, buf_w, buf_h, left_margin, list_top_y + 16, "No chapters found.", main_sz, theme.text_muted, false);
                } else {
                    let mut active_row_idx: Option<usize> = None;
                    for (ri, r) in rows.iter().enumerate() {
                        if r.entry.block_idx <= cur_block {
                            active_row_idx = Some(ri);
                        }
                    }

                    let start = self.bookmark_scroll_y.min(rows.len().saturating_sub(1));
                    for (vis_i, row) in rows.iter().enumerate().skip(start) {
                        let ry = list_top_y + (vis_i - start) as i32 * row_h as i32;
                        if ry + row_h as i32 > list_bot_y {
                            break;
                        }
                        let is_cur = active_row_idx == Some(vis_i);
                        let is_hov = matches!(
                            self.hovered_widget,
                            Some(UiHit::BookSidebar(BookSidebarHit::TocRow(i)))
                                | Some(UiHit::BookSidebar(BookSidebarHit::TocToggle(i))) if i == vis_i
                        );
                        let pill_x = (10.0 * s).round() as i32;
                        let pill_w = (content_right - pill_x - (4.0 * s).round() as i32).max(20) as u32;
                        if is_cur {
                            fill_rounded_rect(buffer, buf_w, buf_h, pill_x, ry + 2, pill_w, row_h.saturating_sub(4), 6.0 * s, theme.accent_soft_bg);
                            fill_circle_aa(buffer, buf_w, buf_h, 15.0 * s, ry as f32 + row_h as f32 * 0.5, 3.0 * s, theme.accent);
                        } else if is_hov {
                            fill_rounded_rect(buffer, buf_w, buf_h, pill_x, ry + 2, pill_w, row_h.saturating_sub(4), 6.0 * s, theme.hover_bg);
                        }

                        let loc_num = self.epub_loc_for_block(row.entry.block_idx);
                        let loc_txt = format!("Loc {loc_num}");
                        let loc_w = measure_text(&loc_txt, loc_sz, false).ceil() as i32;
                        let loc_x = content_right - (8.0 * s).round() as i32 - loc_w;

                        let indent = (row.entry.level.min(3) as i32) * (14.0 * s).round() as i32;
                        let base_x = left_margin + (4.0 * s).round() as i32 + indent;
                        if row.has_children {
                            let chev_sz = (11.0 * s).round() as u32;
                            let ic = if row.is_collapsed { UiIcon::ChevronRight } else { UiIcon::ChevronDown };
                            draw_icon(buffer, buf_w, buf_h, ic, base_x, ry + ((row_h.saturating_sub(chev_sz)) / 2) as i32, chev_sz, theme.text_muted);
                        }
                        let tx = base_x + if row.has_children { (16.0 * s).round() as i32 } else { 2 };
                        let title_max_x = (loc_x - (8.0 * s).round() as i32).max(tx + 24);
                        let row_sz = if row.entry.level == 0 { main_sz } else { sub_sz };
                        let clipped = truncate_ellipsis(&row.entry.title, (title_max_x - tx).max(20) as f32, row_sz, is_cur);
                        let fg = if is_cur { theme.accent_soft_fg } else if row.entry.level == 0 { theme.text_primary } else { theme.text_secondary };
                        draw_text_vcenter(buffer, buf_w, buf_h, tx, ry, row_h, title_max_x, &clipped, row_sz, fg, is_cur);
                        draw_text_vcenter(buffer, buf_w, buf_h, loc_x, ry, row_h, content_right - 4, &loc_txt, loc_sz, if is_cur { theme.accent_soft_fg } else { theme.text_muted }, false);
                    }
                }
            }
            BookSidebarTab::Annotations => {
                list_item_h = (60.0 * s).round().max(36.0) as u32;
                list_total = self.epub_book_data.highlights.len();
                if self.epub_book_data.highlights.is_empty() {
                    draw_text(buffer, buf_w, buf_h, left_margin, list_top_y + 16, "No highlights or notes yet.", main_sz, theme.text_muted, false);
                    draw_text(buffer, buf_w, buf_h, left_margin, list_top_y + 42, "Drag across words on the page", loc_sz, theme.text_muted, false);
                    draw_text(buffer, buf_w, buf_h, left_margin, list_top_y + 62, "to Highlight or Add Note.", loc_sz, theme.text_muted, false);
                } else {
                    let del_sz = (18.0 * s).round() as u32;
                    let del_ic = (10.0 * s).round() as u32;
                    let start = self.bookmark_scroll_y.min(self.epub_book_data.highlights.len().saturating_sub(1));
                    for (vis_i, hl) in self.epub_book_data.highlights.iter().enumerate().skip(start) {
                        let ry = list_top_y + (vis_i - start) as i32 * list_item_h as i32;
                        if ry + list_item_h as i32 > list_bot_y {
                            break;
                        }
                        let hov = matches!(
                            self.hovered_widget,
                            Some(UiHit::BookSidebar(BookSidebarHit::AnnotRow(idx)))
                                | Some(UiHit::BookSidebar(BookSidebarHit::AnnotDelete(idx))) if idx == vis_i
                        );
                        let pill_x = (10.0 * s).round() as i32;
                        let pill_w = (content_right - pill_x - (4.0 * s).round() as i32).max(20) as u32;
                        if hov {
                            fill_rounded_rect(buffer, buf_w, buf_h, pill_x, ry + 2, pill_w, list_item_h.saturating_sub(4), 6.0 * s, theme.hover_bg);
                        }
                        let swatch_col = EPUB_HIGHLIGHT_COLORS.get(hl.color_idx as usize).map(|c| c.0).unwrap_or(EPUB_HIGHLIGHT_COLORS[0].0);
                        fill_circle_aa(buffer, buf_w, buf_h, left_margin as f32 + 4.0 * s, ry as f32 + 18.0 * s, 4.5 * s, swatch_col);
                        let dx = content_right - del_sz as i32 - (6.0 * s).round() as i32;
                        let q_x = left_margin + (14.0 * s).round() as i32;
                        let q_clip = truncate_ellipsis(&hl.quote, (dx - q_x - 6).max(24) as f32, sub_sz, false);
                        draw_text_clipped(buffer, buf_w, buf_h, q_x, ry + (8.0 * s).round() as i32, dx - 2, &q_clip, sub_sz, theme.text_primary, false);
                        if let Some(ref note) = hl.note {
                            let n_clip = truncate_ellipsis(note, (dx - q_x - 6).max(24) as f32, loc_sz, false);
                            draw_text_clipped(buffer, buf_w, buf_h, q_x, ry + (31.0 * s).round() as i32, dx - 2, &n_clip, loc_sz, theme.accent_soft_fg, false);
                        }
                        if hov {
                            draw_icon(buffer, buf_w, buf_h, UiIcon::Close, dx + ((del_sz - del_ic) / 2) as i32, ry + ((list_item_h - del_ic) / 2) as i32, del_ic, theme.danger);
                        }
                    }
                }
            }
            BookSidebarTab::Bookmarks => {
                list_total = self.epub_book_data.bookmarks.len();
                if self.epub_book_data.bookmarks.is_empty() {
                    draw_text(buffer, buf_w, buf_h, left_margin, list_top_y + 16, "No page bookmarks yet.", main_sz, theme.text_muted, false);
                    draw_text(buffer, buf_w, buf_h, left_margin, list_top_y + 42, "Click the Bookmark icon on the", loc_sz, theme.text_muted, false);
                    draw_text(buffer, buf_w, buf_h, left_margin, list_top_y + 62, "bottom bar to save a page.", loc_sz, theme.text_muted, false);
                } else {
                    let del_sz = (18.0 * s).round() as u32;
                    let del_ic = (10.0 * s).round() as u32;
                    let start = self.bookmark_scroll_y.min(self.epub_book_data.bookmarks.len().saturating_sub(1));
                    for (vis_i, bm) in self.epub_book_data.bookmarks.iter().enumerate().skip(start) {
                        let ry = list_top_y + (vis_i - start) as i32 * row_h as i32;
                        if ry + row_h as i32 > list_bot_y {
                            break;
                        }
                        let hov = matches!(
                            self.hovered_widget,
                            Some(UiHit::BookSidebar(BookSidebarHit::BookmarkRow(idx)))
                                | Some(UiHit::BookSidebar(BookSidebarHit::BookmarkDelete(idx))) if idx == vis_i
                        );
                        let pill_x = (10.0 * s).round() as i32;
                        let pill_w = (content_right - pill_x - (4.0 * s).round() as i32).max(20) as u32;
                        if hov {
                            fill_rounded_rect(buffer, buf_w, buf_h, pill_x, ry + 2, pill_w, row_h.saturating_sub(4), 6.0 * s, theme.hover_bg);
                        }
                        let loc_txt = format!("Loc {}", bm.page_num);
                        let lw = measure_text(&loc_txt, loc_sz, true).ceil() as i32;
                        let dx = content_right - del_sz as i32 - (6.0 * s).round() as i32;
                        let max_lbl_x = if hov { dx - lw - 10 } else { content_right - lw - 10 };
                        let lbl = truncate_ellipsis(&bm.label, (max_lbl_x - left_margin).max(20) as f32, main_sz, false);
                        draw_text_vcenter(buffer, buf_w, buf_h, left_margin, ry, row_h, max_lbl_x, &lbl, main_sz, theme.text_primary, false);
                        draw_text_vcenter(buffer, buf_w, buf_h, max_lbl_x + 4, ry, row_h, if hov { dx - 2 } else { content_right - 4 }, &loc_txt, loc_sz, theme.accent_soft_fg, true);
                        if hov {
                            draw_icon(buffer, buf_w, buf_h, UiIcon::Close, dx + ((del_sz - del_ic) / 2) as i32, ry + ((row_h - del_ic) / 2) as i32, del_ic, theme.danger);
                        }
                    }
                }
            }
        }

        // Draw vertical scrollbar whenever items overflow (Tweak 1)
        let avail_h = (list_bot_y - list_top_y).max(0) as u32;
        let visible_rows = ((avail_h / list_item_h.max(1)) as usize).max(1);
        if list_total > visible_rows && avail_h > 20 {
            let track_x = sidebar_w as i32 - sb_w as i32 - (4.0 * s).round() as i32;
            let track_y = list_top_y + (4.0 * s).round() as i32;
            let track_h = avail_h.saturating_sub((8.0 * s).round() as u32).max(16);
            fill_rounded_rect(buffer, buf_w, buf_h, track_x, track_y, sb_w, track_h, sb_w as f32 * 0.5, theme.surface_elevated);
            let max_scroll = list_total.saturating_sub(visible_rows).max(1);
            let start = self.bookmark_scroll_y.min(max_scroll);
            let thumb_h = ((track_h as f32 * visible_rows as f32 / list_total as f32).round() as u32).clamp((22.0 * s).round() as u32, track_h);
            let travel = track_h.saturating_sub(thumb_h) as f32;
            let thumb_y = track_y + ((start as f32 / max_scroll as f32) * travel).round() as i32;
            let sb_active = self.epub_sidebar_scrollbar_drag
                || matches!(self.hovered_widget, Some(UiHit::BookSidebar(BookSidebarHit::ScrollbarTrack(_))));
            fill_rounded_rect(
                buffer, buf_w, buf_h,
                track_x, thumb_y, sb_w, thumb_h,
                sb_w as f32 * 0.5,
                if sb_active { theme.accent } else { theme.text_muted },
            );
        }
    }

    pub(super) fn hit_test_distinct_epub_sidebar(
        &self,
        mx: f64,
        my: f64,
        sidebar_w: u32,
        buf_h: u32,
        top_bars_h: u32,
        scale_factor: f64,
    ) -> Option<BookSidebarHit> {
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if !self.show_bookmarks || ix < 0 || ix >= sidebar_w as i32 || iy < top_bars_h as i32 {
            return None;
        }
        let s = scale_factor as f32;
        let top_y = top_bars_h as i32;
        let total_h = buf_h.saturating_sub(top_bars_h) as i32;
        let (util_h, card_h, seg_h) = Self::book_sidebar_layout(s);
        let pad = (14.0 * s).round() as i32;

        // Top utility row
        if iy < top_y + util_h as i32 {
            let btn_sz = (22.0 * s).round() as i32;
            let search_x = pad;
            let exp_x = search_x + btn_sz + (4.0 * s).round() as i32;
            let pin_x = sidebar_w as i32 - pad - btn_sz;
            if ix >= search_x && ix < search_x + btn_sz {
                return Some(BookSidebarHit::ToggleSearch);
            }
            if ix >= exp_x && ix < exp_x + btn_sz {
                return Some(BookSidebarHit::ToggleExpandAll);
            }
            if ix >= pin_x && ix < pin_x + btn_sz {
                return Some(BookSidebarHit::CollapseSidebar);
            }
            return None;
        }

        // Book header card -> (i) info button
        let card_y = top_y + util_h as i32;
        if iy < card_y + card_h as i32 {
            let info_sz = (18.0 * s).round() as i32;
            let info_x = sidebar_w as i32 - pad - info_sz - 4;
            let info_y = card_y + card_h as i32 - info_sz - (12.0 * s).round() as i32;
            if ix >= info_x && iy >= info_y {
                return Some(BookSidebarHit::BookInfo);
            }
            return None;
        }

        // Bottom 3-tab segmented switcher
        let seg_y = top_y + total_h - seg_h as i32;
        if iy >= seg_y {
            let pill_pad = (8.0 * s).round() as i32;
            let track_w = (sidebar_w as i32 - pill_pad * 2 - 1).max(30);
            let rel_x = (ix - pill_pad).clamp(0, track_w - 1);
            let seg = ((rel_x * 3) / track_w).clamp(0, 2);
            return Some(match seg {
                0 => BookSidebarHit::Tab(BookSidebarTab::Contents),
                1 => BookSidebarHit::Tab(BookSidebarTab::Annotations),
                _ => BookSidebarHit::Tab(BookSidebarTab::Bookmarks),
            });
        }

        let (list_top_y, list_bot_y, item_h_u, total_items, visible_items) =
            self.epub_sidebar_list_metrics(buf_h, top_bars_h, scale_factor);
        if iy < list_top_y || iy >= list_bot_y {
            return None;
        }
        let has_scrollbar = total_items > visible_items;
        let sb_hit_x = sidebar_w as i32 - (16.0 * s).round().max(12.0) as i32;
        if has_scrollbar && ix >= sb_hit_x {
            let ratio = ((iy - list_top_y) as f32 / (list_bot_y - list_top_y).max(1) as f32).clamp(0.0, 1.0);
            return Some(BookSidebarHit::ScrollbarTrack(ratio));
        }

        let content_right = sidebar_w as i32 - if has_scrollbar { (14.0 * s).round() as i32 } else { (6.0 * s).round() as i32 };
        let item_h = item_h_u as i32;
        let start = self.bookmark_scroll_y.min(total_items.saturating_sub(1));
        let vis_idx = ((iy - list_top_y) / item_h.max(1)) as usize;
        let row_idx = start + vis_idx;
        let left_margin = (18.0 * s).round() as i32;

        match self.book_sidebar_tab {
            BookSidebarTab::Contents => {
                let toc: &[TocEntry] = self
                    .markdown_doc
                    .as_ref()
                    .and_then(|d| d.book_meta.as_ref())
                    .map(|m| m.toc.as_slice())
                    .unwrap_or(&[]);
                let mut rows = Vec::new();
                let mut nid = 0usize;
                flatten_toc(toc, &self.book_toc_collapsed, self.book_toc_search.as_deref(), &mut nid, &mut rows);
                if let Some(row) = rows.get(row_idx) {
                    let indent = (row.entry.level.min(3) as i32) * (14.0 * s).round() as i32;
                    let base_x = left_margin + (4.0 * s).round() as i32 + indent;
                    if row.has_children && ix >= base_x - 4 && ix < base_x + (16.0 * s).round() as i32 {
                        return Some(BookSidebarHit::TocToggle(row_idx));
                    }
                    return Some(BookSidebarHit::TocRow(row_idx));
                }
            }
            BookSidebarTab::Annotations => {
                if row_idx < self.epub_book_data.highlights.len() {
                    let del_x = content_right - (24.0 * s).round() as i32;
                    if ix >= del_x {
                        return Some(BookSidebarHit::AnnotDelete(row_idx));
                    }
                    return Some(BookSidebarHit::AnnotRow(row_idx));
                }
            }
            BookSidebarTab::Bookmarks => {
                if row_idx < self.epub_book_data.bookmarks.len() {
                    let del_x = content_right - (24.0 * s).round() as i32;
                    if ix >= del_x {
                        return Some(BookSidebarHit::BookmarkDelete(row_idx));
                    }
                    return Some(BookSidebarHit::BookmarkRow(row_idx));
                }
            }
        }
        None
    }

    pub(super) fn handle_distinct_epub_sidebar_click(
        &mut self,
        hit: BookSidebarHit,
        buf_h: u32,
        top_bars_h: u32,
        scale_factor: f64,
    ) {
        match hit {
            BookSidebarHit::ToggleSearch => {
                self.book_toc_search = if self.book_toc_search.is_some() {
                    None
                } else {
                    Some(String::new())
                };
                self.book_sidebar_tab = BookSidebarTab::Contents;
                self.bookmark_scroll_y = 0;
            }
            BookSidebarHit::ToggleExpandAll => {
                if !self.book_toc_collapsed.is_empty() {
                    self.book_toc_collapsed.clear();
                } else if let Some(toc) = self
                    .markdown_doc
                    .as_ref()
                    .and_then(|d| d.book_meta.as_ref())
                    .map(|m| m.toc.clone())
                {
                    let mut rows = Vec::new();
                    let mut nid = 0usize;
                    let empty = std::collections::HashSet::new();
                    flatten_toc(&toc, &empty, None, &mut nid, &mut rows);
                    for r in rows {
                        if r.has_children {
                            self.book_toc_collapsed.insert(r.node_id);
                        }
                    }
                }
            }
            BookSidebarHit::CollapseSidebar => {
                self.show_bookmarks = false;
                self.book_info_popover = false;
            }
            BookSidebarHit::BookInfo => {
                self.book_info_popover = !self.book_info_popover;
            }
            BookSidebarHit::Tab(tab) => {
                self.book_sidebar_tab = tab;
                self.bookmark_scroll_y = 0;
            }
            BookSidebarHit::ScrollbarTrack(ratio) => {
                let (_, _, _, total_items, visible_items) =
                    self.epub_sidebar_list_metrics(buf_h, top_bars_h, scale_factor);
                let max_scroll = total_items.saturating_sub(visible_items);
                self.bookmark_scroll_y = ((max_scroll as f32) * ratio).round() as usize;
            }
            BookSidebarHit::TocToggle(row_idx) => {
                let toc: &[TocEntry] = self
                    .markdown_doc
                    .as_ref()
                    .and_then(|d| d.book_meta.as_ref())
                    .map(|m| m.toc.as_slice())
                    .unwrap_or(&[]);
                let mut rows = Vec::new();
                let mut nid = 0usize;
                flatten_toc(toc, &self.book_toc_collapsed, self.book_toc_search.as_deref(), &mut nid, &mut rows);
                if let Some(node_id) = rows.get(row_idx).map(|r| r.node_id) {
                    if !self.book_toc_collapsed.remove(&node_id) {
                        self.book_toc_collapsed.insert(node_id);
                    }
                }
            }
            _ => {}
        }
        // Handle row jumps/deletes separately so borrow checker stays tidy
        match hit {
            BookSidebarHit::TocRow(row_idx) => {
                let target_block = self
                    .markdown_doc
                    .as_ref()
                    .and_then(|d| d.book_meta.as_ref())
                    .and_then(|m| {
                        let mut rows = Vec::new();
                        let mut nid = 0usize;
                        flatten_toc(&m.toc, &self.book_toc_collapsed, self.book_toc_search.as_deref(), &mut nid, &mut rows);
                        rows.get(row_idx).map(|r| r.entry.block_idx)
                    });
                if let Some(b_idx) = target_block {
                    self.scroll_to_md_heading(b_idx);
                }
            }
            BookSidebarHit::AnnotRow(idx) => {
                if let Some(b_idx) = self.epub_book_data.highlights.get(idx).map(|h| h.block_idx) {
                    self.scroll_to_md_heading(b_idx);
                }
            }
            BookSidebarHit::AnnotDelete(idx) => {
                if idx < self.epub_book_data.highlights.len() {
                    self.epub_book_data.highlights.remove(idx);
                    self.save_epub_book_data();
                    self.status_notice = Some("Deleted EPUB highlight".to_string());
                }
            }
            BookSidebarHit::BookmarkRow(idx) => {
                if let Some(b_idx) = self.epub_book_data.bookmarks.get(idx).map(|b| b.block_idx) {
                    self.scroll_to_md_heading(b_idx);
                }
            }
            BookSidebarHit::BookmarkDelete(idx) => {
                if idx < self.epub_book_data.bookmarks.len() {
                    self.epub_book_data.bookmarks.remove(idx);
                    self.save_epub_book_data();
                    self.status_notice = Some("Deleted EPUB bookmark".to_string());
                }
            }
            _ => {}
        }
        self.request_redraw();
    }

    // -----------------------------------------------------------------------
    // Book Metadata `(i)` Popover
    // -----------------------------------------------------------------------

    fn book_info_popover_rect(sidebar_w: u32, top_bars_h: u32, buf_w: u32, buf_h: u32, sf: f64) -> (i32, i32, u32, u32) {
        let s = sf as f32;
        let pw = (320.0 * s).round().min((buf_w.saturating_sub(24)) as f32) as u32;
        let ph = (230.0 * s).round().min((buf_h.saturating_sub(80)) as f32) as u32;
        let px = (sidebar_w as i32 + (8.0 * s).round() as i32).min(buf_w as i32 - pw as i32 - 8).max(8);
        let py = top_bars_h as i32 + (40.0 * s).round() as i32;
        (px, py, pw, ph)
    }

    pub(super) fn draw_book_info_popover(&self, buffer: &mut [u32], buf_w: u32, buf_h: u32, sidebar_w: u32, top_bars_h: u32, sf: f64) {
        if !self.book_info_popover || !self.is_distinct_epub_mode() {
            return;
        }
        let Some(ref meta) = self.markdown_doc.as_ref().and_then(|d| d.book_meta.as_ref()) else {
            return;
        };
        let theme = self.ui_theme();
        let s = sf as f32;
        let (px, py, pw, ph) = Self::book_info_popover_rect(sidebar_w, top_bars_h, buf_w, buf_h, sf);
        let rad = (10.0 * s).clamp(6.0, 14.0);
        draw_soft_shadow(buffer, buf_w, buf_h, px, py, pw, ph, rad, (12.0 * s).round() as i32, 4, theme.shadow_rgb, theme.shadow_max_alpha);
        fill_rounded_rect(buffer, buf_w, buf_h, px, py, pw, ph, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, px, py, pw, ph, rad, theme.border_subtle);

        let pad = (14.0 * s).round() as i32;
        let hdr_sz = (12.5 * s).clamp(10.5, 22.0);
        let body_sz = (11.0 * s).clamp(9.5, 19.0);
        draw_text_styled_clipped(buffer, buf_w, buf_h, px + pad, py + pad, px + pw as i32 - 36, &truncate_ellipsis_styled(&meta.title, (pw as i32 - pad * 2 - 20) as f32, TextStyle::book(hdr_sz, true, false)), TextStyle::book(hdr_sz, true, false), theme.text_primary);
        let close_sz = (18.0 * s).round() as u32;
        let close_ic = (10.0 * s).round() as u32;
        let cx = px + pw as i32 - pad - close_sz as i32;
        let cy = py + pad - 2;
        draw_icon(buffer, buf_w, buf_h, UiIcon::Close, cx + ((close_sz - close_ic) / 2) as i32, cy + ((close_sz - close_ic) / 2) as i32, close_ic, theme.text_muted);

        let mut cur_y = py + pad + (26.0 * s).round() as i32;
        fill_rect_clipped(buffer, buf_w, buf_h, px + pad, cur_y - 6, pw.saturating_sub((pad * 2) as u32), 1, theme.divider);

        let fields = [
            ("Author", meta.author.as_deref()),
            ("Publisher", meta.publisher.as_deref()),
            ("Language", meta.language.as_deref()),
        ];
        for (lbl, val_opt) in fields {
            if let Some(val) = val_opt {
                let line = format!("{lbl}: {val}");
                let clipped = truncate_ellipsis(&line, (pw as i32 - pad * 2) as f32, body_sz, false);
                draw_text_clipped(buffer, buf_w, buf_h, px + pad, cur_y, px + pw as i32 - pad, &clipped, body_sz, theme.text_secondary, false);
                cur_y += (20.0 * s).round() as i32;
            }
        }
        if let Some(ref desc) = meta.description {
            cur_y += 4;
            let max_w = (pw as i32 - pad * 2).max(60) as f32;
            let mut rem = desc.as_str();
            let mut lines_drawn = 0;
            while !rem.is_empty() && cur_y + (16.0 * s).round() as i32 <= py + ph as i32 - pad && lines_drawn < 5 {
                let mut take = rem.len();
                while take > 0 && measure_text(&rem[..take], body_sz, false) > max_w {
                    if let Some(sp) = rem[..take].rfind(' ') {
                        take = sp;
                    } else {
                        break;
                    }
                }
                let line_str = rem[..take].trim();
                draw_text_clipped(buffer, buf_w, buf_h, px + pad, cur_y, px + pw as i32 - pad, line_str, body_sz, theme.text_muted, false);
                rem = rem[take..].trim_start();
                cur_y += (17.0 * s).round() as i32;
                lines_drawn += 1;
            }
        }
    }

    pub(super) fn hit_test_book_info_popover(&self, mx: f64, my: f64, sidebar_w: u32, top_bars_h: u32, buf_w: u32, buf_h: u32, sf: f64) -> Option<BookInfoPopoverHit> {
        if !self.book_info_popover || !self.is_distinct_epub_mode() {
            return None;
        }
        let s = sf as f32;
        let (px, py, pw, ph) = Self::book_info_popover_rect(sidebar_w, top_bars_h, buf_w, buf_h, sf);
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix < px || ix >= px + pw as i32 || iy < py || iy >= py + ph as i32 {
            return None;
        }
        let pad = (14.0 * s).round() as i32;
        let close_sz = (22.0 * s).round() as i32;
        if ix >= px + pw as i32 - pad - close_sz && iy <= py + pad + close_sz {
            Some(BookInfoPopoverHit::Close)
        } else {
            Some(BookInfoPopoverHit::Inside)
        }
    }

    // -----------------------------------------------------------------------
    // `Aa` Typography Popover
    // -----------------------------------------------------------------------

    fn aa_popover_rect(&self, buf_w: u32, buf_h: u32, status_bar_h: u32, sf: f64) -> (i32, i32, u32, u32) {
        let s = sf as f32;
        let is_epub = Self::is_epub_path(&self.file_path);
        let base_h = if !is_epub {
            264.0
        } else if self.aa_margins_submenu {
            320.0
        } else {
            428.0
        };
        let pw = (352.0 * s).round().min(buf_w.saturating_sub(24) as f32) as u32;
        let ph = (base_h * s).round().min(buf_h.saturating_sub(status_bar_h + 24) as f32).max(200.0) as u32;
        let px = (buf_w as i32 - pw as i32 - (14.0 * s).round() as i32).max(8);
        let py = (buf_h.saturating_sub(status_bar_h) as i32 - ph as i32 - (8.0 * s).round() as i32).max(8);
        (px, py, pw, ph)
    }

    fn aa_popover_row_metrics(ph: u32, row_count: i32, s: f32) -> (u32, i32, i32) {
        let count = row_count.max(1);
        let gap = (6.0 * s).round().max(3.0) as i32;
        let hdr_offset = (38.0 * s).round() as i32;
        let avail = (ph as i32 - hdr_offset - (14.0 * s).round() as i32 - gap * (count - 1) - (4.0 * s).round() as i32).max(100);
        let row_h = ((avail / count) as u32).clamp(18, (30.0 * s).round().max(18.0) as u32);
        (row_h, gap, hdr_offset)
    }

    pub(super) fn aa_4side_slider_track_x(&self, buf_w: u32, buf_h: u32, status_bar_h: u32, sf: f64) -> (i32, i32) {
        let s = sf as f32;
        let (px, _, pw, _) = self.aa_popover_rect(buf_w, buf_h, status_bar_h, sf);
        let pad = (14.0 * s).round() as i32;
        let inner_w = pw as i32 - pad * 2;
        let lbl_w = (76.0 * s).round() as i32;
        let val_w = (50.0 * s).round() as i32;
        let track_x0 = px + pad + lbl_w;
        let track_x1 = (px + pad + inner_w - val_w).max(track_x0 + 40);
        (track_x0, track_x1)
    }

    pub(super) fn draw_aa_popover(&self, buffer: &mut [u32], buf_w: u32, buf_h: u32, status_bar_h: u32, sf: f64) {
        if !self.show_aa_popover || !self.is_markdown_tab() {
            return;
        }
        let is_epub = Self::is_epub_path(&self.file_path);
        let theme = self.ui_theme();
        let s = sf as f32;
        let (px, py, pw, ph) = self.aa_popover_rect(buf_w, buf_h, status_bar_h, sf);
        let row_count = if !is_epub {
            6
        } else if self.aa_margins_submenu {
            6
        } else {
            10
        };
        let (row_h, gap, hdr_offset) = Self::aa_popover_row_metrics(ph, row_count, s);
        let rad = (10.0 * s).clamp(6.0, 14.0);
        let pad = (14.0 * s).round() as i32;
        let lbl_sz = (11.0 * s).clamp(9.0, 20.0);
        let small_sz = (10.2 * s).clamp(8.5, 18.0);
        let hdr_sz = (12.0 * s).clamp(10.0, 22.0);
        let inner_w = pw.saturating_sub((pad * 2) as u32);
        let btn_rad = (6.0 * s).max(4.0);

        draw_soft_shadow(buffer, buf_w, buf_h, px, py, pw, ph, rad, (14.0 * s).round() as i32, 4, theme.shadow_rgb, theme.shadow_max_alpha);
        fill_rounded_rect(buffer, buf_w, buf_h, px, py, pw, ph, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, px, py, pw, ph, rad, theme.border_subtle);

        let draw_chip = |buf: &mut [u32], x: i32, y: i32, w: u32, h: u32, txt: &str, active: bool, hit: AaPopoverHit| {
            let hov = self.hovered_widget == Some(UiHit::AaPopover(hit));
            let bg = if active { theme.accent_soft_bg } else if hov { theme.hover_bg } else { theme.surface_elevated };
            let border = if active { theme.accent } else { theme.border_subtle };
            let fg = if active { theme.accent_soft_fg } else { theme.text_primary };
            fill_rounded_rect(buf, buf_w, buf_h, x, y, w, h, btn_rad, bg);
            stroke_rounded_rect(buf, buf_w, buf_h, x, y, w, h, btn_rad, border);
            let clipped = truncate_ellipsis(txt, (w as f32 - 8.0).max(16.0), lbl_sz, active);
            let tw = measure_text(&clipped, lbl_sz, active);
            draw_text_vcenter(buf, buf_w, buf_h, x + ((w as f32 - tw) * 0.5).round().max(3.0) as i32, y, h, x + w as i32 - 2, &clipped, lbl_sz, fg, active);
        };

        let draw_stepper_row = |buf: &mut [u32], y: i32, label: &str, val_txt: &str, minus_lbl: &str, plus_lbl: &str, hit_minus: AaPopoverHit, hit_plus: AaPopoverHit| {
            let lbl_w = (128.0 * s).round() as u32;
            let btn_w = (36.0 * s).round() as u32;
            let val_w = inner_w.saturating_sub(lbl_w + btn_w * 2 + gap as u32 * 2);
            draw_text_vcenter(buf, buf_w, buf_h, px + pad + 2, y, row_h, px + pad + lbl_w as i32 - 4, label, lbl_sz, theme.text_secondary, false);
            let bx0 = px + pad + lbl_w as i32;
            draw_chip(buf, bx0, y, btn_w, row_h, minus_lbl, false, hit_minus);
            let vx = bx0 + btn_w as i32 + gap;
            let vw = measure_text(val_txt, lbl_sz, true);
            draw_text_vcenter(buf, buf_w, buf_h, vx + ((val_w as f32 - vw) * 0.5).round() as i32, y, row_h, vx + val_w as i32, val_txt, lbl_sz, theme.text_primary, true);
            let bx1 = vx + val_w as i32 + gap;
            draw_chip(buf, bx1, y, btn_w, row_h, plus_lbl, false, hit_plus);
        };

        let draw_toggle_row = |buf: &mut [u32], y: i32, label: &str, is_on: bool, hit: AaPopoverHit| {
            let hov = self.hovered_widget == Some(UiHit::AaPopover(hit));
            let row_bg = if hov { theme.hover_bg } else { theme.surface_elevated };
            fill_rounded_rect(buf, buf_w, buf_h, px + pad, y, inner_w, row_h, btn_rad, row_bg);
            stroke_rounded_rect(buf, buf_w, buf_h, px + pad, y, inner_w, row_h, btn_rad, theme.border_subtle);

            let sw_w = (36.0 * s).round().max(26.0) as u32;
            let sw_h = (18.0 * s).round().clamp(13.0, (row_h as f32 - 4.0).max(13.0)) as u32;
            let sw_x = px + pad + inner_w as i32 - sw_w as i32 - (8.0 * s).round() as i32;
            let sw_y = y + ((row_h.saturating_sub(sw_h)) / 2) as i32;
            let state_txt = if is_on { "On" } else { "Off" };
            let state_w = measure_text(state_txt, small_sz, is_on).ceil() as i32;
            let state_x = sw_x - state_w - (6.0 * s).round() as i32;

            let lbl_max = (state_x - (px + pad + (10.0 * s).round() as i32) - 4).max(40) as f32;
            let clipped = truncate_ellipsis(label, lbl_max, lbl_sz, false);
            draw_text_vcenter(buf, buf_w, buf_h, px + pad + (10.0 * s).round() as i32, y, row_h, state_x - 4, &clipped, lbl_sz, theme.text_primary, false);
            draw_text_vcenter(buf, buf_w, buf_h, state_x, y, row_h, sw_x - 2, state_txt, small_sz, if is_on { theme.accent } else { theme.text_muted }, is_on);

            let sw_bg = if is_on { theme.accent } else { theme.border_subtle };
            fill_rounded_rect(buf, buf_w, buf_h, sw_x, sw_y, sw_w, sw_h, sw_h as f32 * 0.5, sw_bg);
            let knob_r = (sw_h as f32 * 0.38).max(4.5);
            let knob_cx = if is_on {
                sw_x as f32 + sw_w as f32 - sw_h as f32 * 0.5
            } else {
                sw_x as f32 + sw_h as f32 * 0.5
            };
            let knob_cy = sw_y as f32 + sw_h as f32 * 0.5;
            fill_circle_aa(buf, buf_w, buf_h, knob_cx, knob_cy, knob_r, 0x00FF_FFFF);
        };

        let draw_swatches_row = |buf: &mut [u32], y: i32| {
            let sw_count = ReadingFilter::ALL.len() as u32;
            let sw_w = (inner_w.saturating_sub(gap as u32 * (sw_count - 1))) / sw_count;
            let sw_h = row_h;
            for (i, filter) in ReadingFilter::ALL.into_iter().enumerate() {
                let sx = px + pad + (i as i32) * (sw_w as i32 + gap);
                let is_cur = self.reading_filter == filter;
                let fill_col = match filter {
                    ReadingFilter::Normal => 0x00FF_FFFF,
                    ReadingFilter::PaperEInk => 0x00F2_F1EC,
                    ReadingFilter::WarmSepia => 0x00F6_ECD8,
                    ReadingFilter::SageEyeCare => 0x00E7_F0E7,
                    ReadingFilter::DarkInvert => 0x001B_1E24,
                    ReadingFilter::NightAmber => 0x0017_120E,
                };
                fill_rounded_rect(buf, buf_w, buf_h, sx, y, sw_w, sw_h, btn_rad, fill_col);
                stroke_rounded_rect(buf, buf_w, buf_h, sx, y, sw_w, sw_h, btn_rad, if is_cur { theme.accent } else { theme.border_subtle });
                if is_cur {
                    let ic_col = match filter {
                        ReadingFilter::DarkInvert | ReadingFilter::NightAmber => 0x00F8_FAFC,
                        _ => theme.accent,
                    };
                    let ic = (12.0 * s).round() as u32;
                    draw_icon(buf, buf_w, buf_h, UiIcon::Check, sx + ((sw_w - ic) / 2) as i32, y + ((sw_h - ic) / 2) as i32, ic, ic_col);
                }
            }
        };

        // Branch 1: Markdown / LaTeX Typography & Spacing Popover (Tweaks 7 & 8)
        if !is_epub {
            let hdr = if self.active_doc_kind() == DocKind::LaTeX {
                "LaTeX Typography & Spacing"
            } else {
                "Markdown Typography & Spacing"
            };
            draw_text(buffer, buf_w, buf_h, px + pad, py + (12.0 * s).round() as i32, hdr, hdr_sz, theme.text_primary, true);
            let mut cy = py + hdr_offset;
            fill_rect_clipped(buffer, buf_w, buf_h, px + pad, cy - 4, inner_w, 1, theme.divider);

            draw_stepper_row(buffer, cy, "Preview Font", &format!("{:.0} px", self.md_preview_font_px), "A-", "A+", AaPopoverHit::SizeMinus, AaPopoverHit::SizePlus);
            cy += row_h as i32 + gap;
            draw_stepper_row(buffer, cy, "Code Font", &format!("{:.0} px", self.md_code_font_px), "A-", "A+", AaPopoverHit::CodeSizeMinus, AaPopoverHit::CodeSizePlus);
            cy += row_h as i32 + gap;
            draw_stepper_row(buffer, cy, "Line Spacing", &format!("{:.2}×", self.md_line_spacing), "-", "+", AaPopoverHit::LineSpacingMinus, AaPopoverHit::LineSpacingPlus);
            cy += row_h as i32 + gap;
            draw_stepper_row(buffer, cy, "Word Spacing", &format!("{:.1} px", self.md_word_spacing_px), "-", "+", AaPopoverHit::WordSpacingMinus, AaPopoverHit::WordSpacingPlus);
            cy += row_h as i32 + gap;
            draw_stepper_row(buffer, cy, "Para Spacing", &format!("{:.2}×", self.md_para_spacing_mult), "-", "+", AaPopoverHit::ParaSpacingMinus, AaPopoverHit::ParaSpacingPlus);
            cy += row_h as i32 + gap;
            draw_swatches_row(buffer, cy);
            return;
        }

        // Branch 2: EPUB 4-Side Margins Sub-Menu (Tweak 2)
        if self.aa_margins_submenu {
            let back_w = (58.0 * s).round() as u32;
            let rst_w = (54.0 * s).round() as u32;
            let hdr_y = py + (7.0 * s).round() as i32;
            let hdr_btn_h = (24.0 * s).round() as u32;
            draw_chip(buffer, px + pad, hdr_y, back_w, hdr_btn_h, "‹ Back", false, AaPopoverHit::CloseMarginsSubmenu);
            let title = "Margins (4 Sides)";
            let tw = measure_text(title, hdr_sz, true);
            draw_text_vcenter(buffer, buf_w, buf_h, px + ((pw as f32 - tw) * 0.5).round() as i32, hdr_y, hdr_btn_h, px + pw as i32 - pad - rst_w as i32, title, hdr_sz, theme.text_primary, true);
            draw_chip(buffer, px + pad + inner_w as i32 - rst_w as i32, hdr_y, rst_w, hdr_btn_h, "Reset", false, AaPopoverHit::ResetMargins);

            let mut cy = py + hdr_offset;
            fill_rect_clipped(buffer, buf_w, buf_h, px + pad, cy - 4, inner_w, 1, theme.divider);

            // Row 1: Quick Presets (Narrow | Normal | Wide)
            let tri_w = (inner_w.saturating_sub(gap as u32 * 2)) / 3;
            for (i, mp) in [MarginPreset::Narrow, MarginPreset::Normal, MarginPreset::Wide].into_iter().enumerate() {
                let bx = px + pad + (i as i32) * (tri_w as i32 + gap);
                let bw = if i == 2 { inner_w - (tri_w + gap as u32) * 2 } else { tri_w };
                draw_chip(buffer, bx, cy, bw, row_h, mp.label(), self.epub_prefs.margins == mp, AaPopoverHit::Margin(mp));
            }
            cy += row_h as i32 + gap;

            // Rows 2..5: Top / Bottom / Outer / Inner sliders
            let (track_x0, track_x1) = self.aa_4side_slider_track_x(buf_w, buf_h, status_bar_h, sf);
            let track_w = (track_x1 - track_x0).max(20) as u32;
            let track_h = (4.0 * s).round().max(3.0) as u32;
            let knob_r = (6.0 * s).clamp(5.0, 9.0);
            let side_names = [(0u8, "Top Margin"), (1u8, "Bottom"), (2u8, "Outer Side"), (3u8, "Inner Spine")];
            for (part, label) in side_names {
                fill_rounded_rect(buffer, buf_w, buf_h, px + pad, cy, inner_w, row_h, btn_rad, theme.surface_elevated);
                stroke_rounded_rect(buffer, buf_w, buf_h, px + pad, cy, inner_w, row_h, btn_rad, theme.border_subtle);
                let norm = self.epub_prefs.margin_part_norm(part);
                let track_y = cy + ((row_h.saturating_sub(track_h)) / 2) as i32;
                draw_text_vcenter(buffer, buf_w, buf_h, px + pad + (8.0 * s).round() as i32, cy, row_h, track_x0 - 4, label, small_sz, theme.text_primary, false);
                fill_rounded_rect(buffer, buf_w, buf_h, track_x0, track_y, track_w, track_h, track_h as f32 * 0.5, theme.border_subtle);
                let fill_w = (track_w as f32 * norm).round() as u32;
                if fill_w > 0 {
                    fill_rounded_rect(buffer, buf_w, buf_h, track_x0, track_y, fill_w, track_h, track_h as f32 * 0.5, theme.accent);
                }
                let knob_x = track_x0 as f32 + track_w as f32 * norm;
                let knob_y = cy as f32 + row_h as f32 * 0.5;
                fill_circle_aa(buffer, buf_w, buf_h, knob_x, knob_y, knob_r + 1.0, theme.surface_bg);
                fill_circle_aa(buffer, buf_w, buf_h, knob_x, knob_y, knob_r, theme.accent);
                let pct_txt = format!("{:.0}%", self.epub_prefs.margin_part(part) * 100.0);
                let pw_t = measure_text(&pct_txt, small_sz, true);
                draw_text_vcenter(buffer, buf_w, buf_h, px + pad + inner_w as i32 - pw_t.ceil() as i32 - (8.0 * s).round() as i32, cy, row_h, px + pad + inner_w as i32 - 4, &pct_txt, small_sz, theme.text_secondary, true);
                cy += row_h as i32 + gap;
            }
            draw_text_vcenter(
                buffer, buf_w, buf_h, px + pad + 2, cy, row_h, px + pad + inner_w as i32,
                "Top/Bottom changes leave Spine untouched.",
                small_sz, theme.text_muted, false,
            );
            return;
        }

        // Branch 3: Main EPUB Aa Typography Popover
        let mut cy = py + (12.0 * s).round() as i32;
        draw_text(buffer, buf_w, buf_h, px + pad, cy, "Typography & Reading", hdr_sz, theme.text_primary, true);
        cy = py + hdr_offset;
        fill_rect_clipped(buffer, buf_w, buf_h, px + pad, cy - 4, inner_w, 1, theme.divider);

        // Row 1: Font Family (Book Serif | UI Sans)
        let half_w = (inner_w.saturating_sub(gap as u32)) / 2;
        draw_chip(buffer, px + pad, cy, half_w, row_h, "Book Serif", matches!(self.epub_prefs.font_choice, EpubFontChoice::BookSerif), AaPopoverHit::FontBookSerif);
        draw_chip(buffer, px + pad + half_w as i32 + gap, cy, inner_w - half_w - gap as u32, row_h, "UI Sans", matches!(self.epub_prefs.font_choice, EpubFontChoice::UiSans), AaPopoverHit::FontUiSans);
        cy += row_h as i32 + gap;

        // Row 2: Curated OS Font Stepper (`<  OS Font / Georgia  >`)
        let step_w = (32.0 * s).round() as u32;
        let os_lbl_w = inner_w.saturating_sub(step_w * 2 + gap as u32 * 2);
        draw_chip(buffer, px + pad, cy, step_w, row_h, "<", false, AaPopoverHit::FontOsPrev);
        let os_name = match &self.epub_prefs.font_choice {
            EpubFontChoice::OsSerif(n) => n.clone(),
            _ => "OS Serif Font...".to_string(),
        };
        let os_active = matches!(self.epub_prefs.font_choice, EpubFontChoice::OsSerif(_));
        fill_rounded_rect(buffer, buf_w, buf_h, px + pad + step_w as i32 + gap, cy, os_lbl_w, row_h, btn_rad, if os_active { theme.accent_soft_bg } else { theme.input_bg });
        stroke_rounded_rect(buffer, buf_w, buf_h, px + pad + step_w as i32 + gap, cy, os_lbl_w, row_h, btn_rad, if os_active { theme.accent } else { theme.border_subtle });
        let clipped_os = truncate_ellipsis(&os_name, (os_lbl_w as f32 - 12.0).max(20.0), lbl_sz, os_active);
        let osw = measure_text(&clipped_os, lbl_sz, os_active);
        draw_text_vcenter(buffer, buf_w, buf_h, px + pad + step_w as i32 + gap + ((os_lbl_w as f32 - osw) * 0.5).round() as i32, cy, row_h, px + pad + step_w as i32 + gap + os_lbl_w as i32 - 3, &clipped_os, lbl_sz, if os_active { theme.accent_soft_fg } else { theme.text_secondary }, os_active);
        draw_chip(buffer, px + pad + (step_w + os_lbl_w) as i32 + gap * 2, cy, step_w, row_h, ">", false, AaPopoverHit::FontOsNext);
        cy += row_h as i32 + gap;

        // Row 3: Font Size (`A-` | `17 px` | `A+`)
        let tri_w = (inner_w.saturating_sub(gap as u32 * 2)) / 3;
        draw_chip(buffer, px + pad, cy, tri_w, row_h, "A-", false, AaPopoverHit::SizeMinus);
        let sz_txt = format!("{:.0} px", self.epub_prefs.font_size_px);
        let szw = measure_text(&sz_txt, lbl_sz, true);
        draw_text_vcenter(buffer, buf_w, buf_h, px + pad + tri_w as i32 + gap + ((tri_w as f32 - szw) * 0.5).round() as i32, cy, row_h, px + pad + (tri_w * 2) as i32 + gap, &sz_txt, lbl_sz, theme.text_primary, true);
        draw_chip(buffer, px + pad + (tri_w as i32 + gap) * 2, cy, inner_w - (tri_w + gap as u32) * 2, row_h, "A+", false, AaPopoverHit::SizePlus);
        cy += row_h as i32 + gap;

        // Row 4: Line Spacing (Compact | Normal | Relaxed)
        for (i, sp) in [LineSpacingPreset::Compact, LineSpacingPreset::Normal, LineSpacingPreset::Relaxed].into_iter().enumerate() {
            let bx = px + pad + (i as i32) * (tri_w as i32 + gap);
            let bw = if i == 2 { inner_w - (tri_w + gap as u32) * 2 } else { tri_w };
            draw_chip(buffer, bx, cy, bw, row_h, sp.label(), self.epub_prefs.line_spacing == sp, AaPopoverHit::Spacing(sp));
        }
        cy += row_h as i32 + gap;

        // Row 5: Word Spacing & Paragraph Spacing compact steppers (Tweak 8)
        let left_half_w = half_w;
        let right_half_x = px + pad + left_half_w as i32 + gap;
        let right_half_w = inner_w - left_half_w - gap as u32;
        let mini_btn_w = (24.0 * s).round().max(18.0) as u32;
        let sub_gap = (4.0 * s).round() as i32;
        // Left half: Word spacing
        draw_chip(buffer, px + pad, cy, mini_btn_w, row_h, "-", false, AaPopoverHit::WordSpacingMinus);
        let w_lbl = format!("Word {:+.1}", self.epub_prefs.word_spacing_px);
        let wl_w = measure_text(&w_lbl, small_sz, false);
        let wl_x0 = px + pad + mini_btn_w as i32 + sub_gap;
        let wl_x1 = px + pad + left_half_w as i32 - mini_btn_w as i32 - sub_gap;
        draw_text_vcenter(buffer, buf_w, buf_h, wl_x0 + (((wl_x1 - wl_x0) as f32 - wl_w) * 0.5).round().max(0.0) as i32, cy, row_h, wl_x1, &w_lbl, small_sz, theme.text_secondary, false);
        draw_chip(buffer, px + pad + left_half_w as i32 - mini_btn_w as i32, cy, mini_btn_w, row_h, "+", false, AaPopoverHit::WordSpacingPlus);
        // Right half: Paragraph spacing
        draw_chip(buffer, right_half_x, cy, mini_btn_w, row_h, "-", false, AaPopoverHit::ParaSpacingMinus);
        let p_lbl = format!("Para {:.2}×", self.epub_prefs.paragraph_spacing_mult);
        let pl_w = measure_text(&p_lbl, small_sz, false);
        let pl_x0 = right_half_x + mini_btn_w as i32 + sub_gap;
        let pl_x1 = right_half_x + right_half_w as i32 - mini_btn_w as i32 - sub_gap;
        draw_text_vcenter(buffer, buf_w, buf_h, pl_x0 + (((pl_x1 - pl_x0) as f32 - pl_w) * 0.5).round().max(0.0) as i32, cy, row_h, pl_x1, &p_lbl, small_sz, theme.text_secondary, false);
        draw_chip(buffer, right_half_x + right_half_w as i32 - mini_btn_w as i32, cy, mini_btn_w, row_h, "+", false, AaPopoverHit::ParaSpacingPlus);
        cy += row_h as i32 + gap;

        // Row 6: Margins Quick Presets + 4-Side Sub-Menu button (Tweak 2)
        let quad_w = (inner_w.saturating_sub(gap as u32 * 3)) / 4;
        for (i, mp) in [MarginPreset::Narrow, MarginPreset::Normal, MarginPreset::Wide].into_iter().enumerate() {
            let bx = px + pad + (i as i32) * (quad_w as i32 + gap);
            draw_chip(buffer, bx, cy, quad_w, row_h, mp.label(), self.epub_prefs.margins == mp, AaPopoverHit::Margin(mp));
        }
        let custom_x = px + pad + (quad_w as i32 + gap) * 3;
        let custom_w = inner_w - (quad_w + gap as u32) * 3;
        draw_chip(buffer, custom_x, cy, custom_w, row_h, "4-Side ›", false, AaPopoverHit::OpenMarginsSubmenu);
        cy += row_h as i32 + gap;

        // Row 7: Justify Text toggle
        draw_toggle_row(buffer, cy, "Justify Text", self.epub_prefs.justify, AaPopoverHit::ToggleJustify);
        cy += row_h as i32 + gap;

        // Row 8: Avoid Breaking Words (-) toggle
        draw_toggle_row(buffer, cy, "Avoid Breaking Words (-)", !self.epub_prefs.hyphenate, AaPopoverHit::ToggleHyphenate);
        cy += row_h as i32 + gap;

        // Row 9: Spread Mode + Page Curl Animation
        draw_chip(buffer, px + pad, cy, tri_w, row_h, "2-Page Book", self.epub_prefs.two_page_spread, AaPopoverHit::SpreadTwoPage);
        draw_chip(buffer, px + pad + tri_w as i32 + gap, cy, tri_w, row_h, "1-Page Flow", !self.epub_prefs.two_page_spread, AaPopoverHit::SpreadOnePage);
        draw_chip(buffer, px + pad + (tri_w as i32 + gap) * 2, cy, inner_w - (tri_w + gap as u32) * 2, row_h, if self.epub_prefs.page_curl_anim { "Curl: On" } else { "Curl: Off" }, self.epub_prefs.page_curl_anim, AaPopoverHit::TogglePageCurl);
        cy += row_h as i32 + gap;

        // Row 10: 6 Paper Theme Swatches
        draw_swatches_row(buffer, cy);
    }

    pub(super) fn hit_test_aa_popover(&self, mx: f64, my: f64, buf_w: u32, buf_h: u32, status_bar_h: u32, sf: f64) -> Option<AaPopoverHit> {
        if !self.show_aa_popover || !self.is_markdown_tab() {
            return None;
        }
        let is_epub = Self::is_epub_path(&self.file_path);
        let s = sf as f32;
        let (px, py, pw, ph) = self.aa_popover_rect(buf_w, buf_h, status_bar_h, sf);
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix < px || ix >= px + pw as i32 || iy < py || iy >= py + ph as i32 {
            return None;
        }
        let row_count = if !is_epub {
            6
        } else if self.aa_margins_submenu {
            6
        } else {
            10
        };
        let pad = (14.0 * s).round() as i32;
        let (row_h_u, gap, hdr_offset) = Self::aa_popover_row_metrics(ph, row_count, s);
        let row_h = row_h_u as i32;
        let inner_w = pw as i32 - pad * 2;
        let mut cy = py + hdr_offset;

        let half_w = (inner_w - gap) / 2;
        let tri_w = (inner_w - gap * 2) / 3;

        let hit_swatches = |y: i32| -> Option<AaPopoverHit> {
            let sw_count = ReadingFilter::ALL.len() as i32;
            let sw_w = (inner_w - gap * (sw_count - 1)) / sw_count;
            if iy >= y && iy < y + row_h {
                for (i, filter) in ReadingFilter::ALL.into_iter().enumerate() {
                    let sx = px + pad + (i as i32) * (sw_w + gap);
                    if ix >= sx && ix < sx + sw_w {
                        return Some(AaPopoverHit::ThemeFilter(filter));
                    }
                }
            }
            None
        };

        if !is_epub {
            let lbl_w = (128.0 * s).round() as i32;
            let btn_w = (36.0 * s).round() as i32;
            let val_w = inner_w - lbl_w - btn_w * 2 - gap * 2;
            let bx0 = px + pad + lbl_w;
            let bx1 = bx0 + btn_w + gap + val_w + gap;
            let step_rows = [
                (AaPopoverHit::SizeMinus, AaPopoverHit::SizePlus),
                (AaPopoverHit::CodeSizeMinus, AaPopoverHit::CodeSizePlus),
                (AaPopoverHit::LineSpacingMinus, AaPopoverHit::LineSpacingPlus),
                (AaPopoverHit::WordSpacingMinus, AaPopoverHit::WordSpacingPlus),
                (AaPopoverHit::ParaSpacingMinus, AaPopoverHit::ParaSpacingPlus),
            ];
            for (h_minus, h_plus) in step_rows {
                if iy >= cy && iy < cy + row_h {
                    if ix >= bx0 && ix < bx0 + btn_w {
                        return Some(h_minus);
                    }
                    if ix >= bx1 && ix < bx1 + btn_w {
                        return Some(h_plus);
                    }
                    return Some(AaPopoverHit::InsidePopover);
                }
                cy += row_h + gap;
            }
            if let Some(h) = hit_swatches(cy) {
                return Some(h);
            }
            return Some(AaPopoverHit::InsidePopover);
        }

        if self.aa_margins_submenu {
            let back_w = (58.0 * s).round() as i32;
            let rst_w = (54.0 * s).round() as i32;
            let hdr_y = py + (7.0 * s).round() as i32;
            let hdr_btn_h = (24.0 * s).round() as i32;
            if iy >= hdr_y && iy < hdr_y + hdr_btn_h {
                if ix >= px + pad && ix < px + pad + back_w {
                    return Some(AaPopoverHit::CloseMarginsSubmenu);
                }
                if ix >= px + pad + inner_w - rst_w && ix < px + pad + inner_w {
                    return Some(AaPopoverHit::ResetMargins);
                }
            }
            if iy >= cy && iy < cy + row_h {
                if ix >= px + pad && ix < px + pad + tri_w {
                    return Some(AaPopoverHit::Margin(MarginPreset::Narrow));
                } else if ix >= px + pad + tri_w + gap && ix < px + pad + tri_w * 2 + gap {
                    return Some(AaPopoverHit::Margin(MarginPreset::Normal));
                } else if ix >= px + pad + (tri_w + gap) * 2 && ix < px + pad + inner_w {
                    return Some(AaPopoverHit::Margin(MarginPreset::Wide));
                }
            }
            cy += row_h + gap;
            let (tx0, tx1) = self.aa_4side_slider_track_x(buf_w, buf_h, status_bar_h, sf);
            for part in 0u8..4 {
                if iy >= cy && iy < cy + row_h {
                    if ix >= tx0 - 8 && ix <= tx1 + 8 {
                        let norm = ((ix - tx0) as f32 / (tx1 - tx0).max(1) as f32).clamp(0.0, 1.0);
                        return Some(AaPopoverHit::MarginPartSlider(part, norm));
                    }
                    return Some(AaPopoverHit::InsidePopover);
                }
                cy += row_h + gap;
            }
            return Some(AaPopoverHit::InsidePopover);
        }

        let step_w = (32.0 * s).round() as i32;
        let os_lbl_w = inner_w - step_w * 2 - gap * 2;

        // Row 1: Font family
        if iy >= cy && iy < cy + row_h {
            if ix >= px + pad && ix < px + pad + half_w {
                return Some(AaPopoverHit::FontBookSerif);
            } else if ix >= px + pad + half_w + gap && ix < px + pad + inner_w {
                return Some(AaPopoverHit::FontUiSans);
            }
        }
        cy += row_h + gap;

        // Row 2: OS font stepper
        if iy >= cy && iy < cy + row_h {
            if ix >= px + pad && ix < px + pad + step_w {
                return Some(AaPopoverHit::FontOsPrev);
            } else if ix >= px + pad + step_w + os_lbl_w + gap * 2 && ix < px + pad + inner_w {
                return Some(AaPopoverHit::FontOsNext);
            }
            return Some(AaPopoverHit::InsidePopover);
        }
        cy += row_h + gap;

        // Row 3: Size
        if iy >= cy && iy < cy + row_h {
            if ix >= px + pad && ix < px + pad + tri_w {
                return Some(AaPopoverHit::SizeMinus);
            } else if ix >= px + pad + (tri_w + gap) * 2 && ix < px + pad + inner_w {
                return Some(AaPopoverHit::SizePlus);
            }
            return Some(AaPopoverHit::InsidePopover);
        }
        cy += row_h + gap;

        // Row 4: Line Spacing
        if iy >= cy && iy < cy + row_h {
            if ix >= px + pad && ix < px + pad + tri_w {
                return Some(AaPopoverHit::Spacing(LineSpacingPreset::Compact));
            } else if ix >= px + pad + tri_w + gap && ix < px + pad + tri_w * 2 + gap {
                return Some(AaPopoverHit::Spacing(LineSpacingPreset::Normal));
            } else if ix >= px + pad + (tri_w + gap) * 2 && ix < px + pad + inner_w {
                return Some(AaPopoverHit::Spacing(LineSpacingPreset::Relaxed));
            }
        }
        cy += row_h + gap;

        // Row 5: Word Spacing & Paragraph Spacing
        if iy >= cy && iy < cy + row_h {
            let right_half_x = px + pad + half_w + gap;
            let right_half_w = inner_w - half_w - gap;
            let mini_btn_w = (24.0 * s).round().max(18.0) as i32;
            if ix >= px + pad && ix < px + pad + mini_btn_w {
                return Some(AaPopoverHit::WordSpacingMinus);
            } else if ix >= px + pad + half_w - mini_btn_w && ix < px + pad + half_w {
                return Some(AaPopoverHit::WordSpacingPlus);
            } else if ix >= right_half_x && ix < right_half_x + mini_btn_w {
                return Some(AaPopoverHit::ParaSpacingMinus);
            } else if ix >= right_half_x + right_half_w - mini_btn_w && ix < right_half_x + right_half_w {
                return Some(AaPopoverHit::ParaSpacingPlus);
            }
            return Some(AaPopoverHit::InsidePopover);
        }
        cy += row_h + gap;

        // Row 6: Margins presets + 4-Side Submenu
        if iy >= cy && iy < cy + row_h {
            let quad_w = (inner_w - gap * 3) / 4;
            if ix >= px + pad && ix < px + pad + quad_w {
                return Some(AaPopoverHit::Margin(MarginPreset::Narrow));
            } else if ix >= px + pad + quad_w + gap && ix < px + pad + quad_w * 2 + gap {
                return Some(AaPopoverHit::Margin(MarginPreset::Normal));
            } else if ix >= px + pad + (quad_w + gap) * 2 && ix < px + pad + quad_w * 3 + gap * 2 {
                return Some(AaPopoverHit::Margin(MarginPreset::Wide));
            } else if ix >= px + pad + (quad_w + gap) * 3 && ix < px + pad + inner_w {
                return Some(AaPopoverHit::OpenMarginsSubmenu);
            }
        }
        cy += row_h + gap;

        // Row 7: Justify Text toggle
        if iy >= cy && iy < cy + row_h && ix >= px + pad && ix < px + pad + inner_w {
            return Some(AaPopoverHit::ToggleJustify);
        }
        cy += row_h + gap;

        // Row 8: Avoid Breaking Words (-) toggle
        if iy >= cy && iy < cy + row_h && ix >= px + pad && ix < px + pad + inner_w {
            return Some(AaPopoverHit::ToggleHyphenate);
        }
        cy += row_h + gap;

        // Row 9: Spread Mode + Page Curl
        if iy >= cy && iy < cy + row_h {
            if ix >= px + pad && ix < px + pad + tri_w {
                return Some(AaPopoverHit::SpreadTwoPage);
            } else if ix >= px + pad + tri_w + gap && ix < px + pad + tri_w * 2 + gap {
                return Some(AaPopoverHit::SpreadOnePage);
            } else if ix >= px + pad + (tri_w + gap) * 2 && ix < px + pad + inner_w {
                return Some(AaPopoverHit::TogglePageCurl);
            }
        }
        cy += row_h + gap;

        // Row 10: 6 Paper Theme Swatches
        if let Some(h) = hit_swatches(cy) {
            return Some(h);
        }

        Some(AaPopoverHit::InsidePopover)
    }

    pub(super) fn handle_aa_popover_click(&mut self, hit: AaPopoverHit) {
        let is_epub = Self::is_epub_path(&self.file_path);
        let mut prefs_changed = false;
        match hit {
            AaPopoverHit::FontBookSerif => {
                self.epub_prefs.font_choice = EpubFontChoice::BookSerif;
                set_custom_os_font(None);
                prefs_changed = true;
            }
            AaPopoverHit::FontUiSans => {
                self.epub_prefs.font_choice = EpubFontChoice::UiSans;
                set_custom_os_font(None);
                prefs_changed = true;
            }
            AaPopoverHit::FontOsPrev | AaPopoverHit::FontOsNext => {
                let candidates = curated_os_book_fonts();
                if candidates.is_empty() {
                    self.status_notice = Some("No OS serif fonts found".to_string());
                } else {
                    let delta: isize = if hit == AaPopoverHit::FontOsNext { 1 } else { -1 };
                    let next_idx = match &self.epub_prefs.font_choice {
                        EpubFontChoice::OsSerif(cur) => {
                            let pos = candidates.iter().position(|n| n.eq_ignore_ascii_case(cur)).unwrap_or(0);
                            (pos as isize + delta).rem_euclid(candidates.len() as isize) as usize
                        }
                        _ => 0,
                    };
                    let chosen = candidates[next_idx].clone();
                    set_custom_os_font(Some(&chosen));
                    self.epub_prefs.font_choice = EpubFontChoice::OsSerif(chosen);
                    prefs_changed = true;
                }
            }
            AaPopoverHit::SizeMinus => {
                if is_epub {
                    self.epub_prefs.font_size_px = (self.epub_prefs.font_size_px - 1.0).clamp(11.0, 30.0);
                } else {
                    self.md_preview_font_px = (self.md_preview_font_px - 1.0).clamp(11.0, 28.0);
                }
                prefs_changed = true;
            }
            AaPopoverHit::SizePlus => {
                if is_epub {
                    self.epub_prefs.font_size_px = (self.epub_prefs.font_size_px + 1.0).clamp(11.0, 30.0);
                } else {
                    self.md_preview_font_px = (self.md_preview_font_px + 1.0).clamp(11.0, 28.0);
                }
                prefs_changed = true;
            }
            AaPopoverHit::CodeSizeMinus => {
                self.md_code_font_px = (self.md_code_font_px - 1.0).clamp(10.0, 24.0);
                if let Some(ref mut ed) = self.md_editor {
                    ed.font_size_px = self.md_code_font_px;
                }
                prefs_changed = true;
            }
            AaPopoverHit::CodeSizePlus => {
                self.md_code_font_px = (self.md_code_font_px + 1.0).clamp(10.0, 24.0);
                if let Some(ref mut ed) = self.md_editor {
                    ed.font_size_px = self.md_code_font_px;
                }
                prefs_changed = true;
            }
            AaPopoverHit::Spacing(sp) => {
                self.epub_prefs.set_line_spacing_preset(sp);
                prefs_changed = true;
            }
            AaPopoverHit::LineSpacingMinus => {
                if is_epub {
                    self.epub_prefs.line_spacing_mult = (self.epub_prefs.line_spacing_mult - 0.1).clamp(1.10, 2.20);
                } else {
                    self.md_line_spacing = (self.md_line_spacing - 0.1).clamp(1.10, 2.20);
                }
                prefs_changed = true;
            }
            AaPopoverHit::LineSpacingPlus => {
                if is_epub {
                    self.epub_prefs.line_spacing_mult = (self.epub_prefs.line_spacing_mult + 0.1).clamp(1.10, 2.20);
                } else {
                    self.md_line_spacing = (self.md_line_spacing + 0.1).clamp(1.10, 2.20);
                }
                prefs_changed = true;
            }
            AaPopoverHit::WordSpacingMinus => {
                if is_epub {
                    self.epub_prefs.word_spacing_px = (self.epub_prefs.word_spacing_px - 0.5).clamp(-1.0, 6.0);
                } else {
                    self.md_word_spacing_px = (self.md_word_spacing_px - 0.5).clamp(-1.0, 6.0);
                }
                prefs_changed = true;
            }
            AaPopoverHit::WordSpacingPlus => {
                if is_epub {
                    self.epub_prefs.word_spacing_px = (self.epub_prefs.word_spacing_px + 0.5).clamp(-1.0, 6.0);
                } else {
                    self.md_word_spacing_px = (self.md_word_spacing_px + 0.5).clamp(-1.0, 6.0);
                }
                prefs_changed = true;
            }
            AaPopoverHit::ParaSpacingMinus => {
                if is_epub {
                    self.epub_prefs.paragraph_spacing_mult = (self.epub_prefs.paragraph_spacing_mult - 0.15).clamp(0.50, 2.50);
                } else {
                    self.md_para_spacing_mult = (self.md_para_spacing_mult - 0.15).clamp(0.50, 2.50);
                }
                prefs_changed = true;
            }
            AaPopoverHit::ParaSpacingPlus => {
                if is_epub {
                    self.epub_prefs.paragraph_spacing_mult = (self.epub_prefs.paragraph_spacing_mult + 0.15).clamp(0.50, 2.50);
                } else {
                    self.md_para_spacing_mult = (self.md_para_spacing_mult + 0.15).clamp(0.50, 2.50);
                }
                prefs_changed = true;
            }
            AaPopoverHit::Margin(mp) => {
                self.epub_prefs.set_margin_preset(mp);
                prefs_changed = true;
            }
            AaPopoverHit::OpenMarginsSubmenu => {
                self.aa_margins_submenu = true;
                self.request_redraw();
                return;
            }
            AaPopoverHit::CloseMarginsSubmenu => {
                self.aa_margins_submenu = false;
                self.request_redraw();
                return;
            }
            AaPopoverHit::ResetMargins => {
                self.epub_prefs.set_margin_preset(MarginPreset::Normal);
                prefs_changed = true;
            }
            AaPopoverHit::MarginPartSlider(part, norm) => {
                self.epub_prefs.set_margin_part_norm(part, norm);
                prefs_changed = true;
            }
            AaPopoverHit::ToggleJustify => {
                self.epub_prefs.justify = !self.epub_prefs.justify;
                prefs_changed = true;
            }
            AaPopoverHit::ToggleHyphenate => {
                self.epub_prefs.hyphenate = !self.epub_prefs.hyphenate;
                prefs_changed = true;
            }
            AaPopoverHit::SpreadTwoPage => {
                self.epub_prefs.two_page_spread = true;
                prefs_changed = true;
            }
            AaPopoverHit::SpreadOnePage => {
                self.epub_prefs.two_page_spread = false;
                prefs_changed = true;
            }
            AaPopoverHit::TogglePageCurl => {
                self.epub_prefs.page_curl_anim = !self.epub_prefs.page_curl_anim;
                self.status_notice = Some(if self.epub_prefs.page_curl_anim {
                    "Page-turn curl animation: On".to_string()
                } else {
                    "Page-turn curl animation: Off".to_string()
                });
                self.save_session_to_ini();
                self.request_redraw();
                return;
            }
            AaPopoverHit::ThemeFilter(filter) => {
                self.set_reading_filter(filter);
            }
            AaPopoverHit::InsidePopover => {}
        }
        if prefs_changed {
            self.md_layout = None;
            self.save_session_to_ini();
            self.request_redraw();
        }
    }

    // -----------------------------------------------------------------------
    // Floating Selection Pill (Highlight swatches | Add Note | Copy)
    // -----------------------------------------------------------------------

    pub(super) fn epub_normalized_selection(&self) -> Option<((usize, usize), (usize, usize))> {
        let (a, b) = self.epub_selection_drag?;
        if a <= b { Some((a, b)) } else { Some((b, a)) }
    }

    pub(super) fn epub_selected_quote_text(&self) -> Option<(usize, usize, usize, String)> {
        let ((b0, w0), (b1, w1)) = self.epub_normalized_selection()?;
        let words: Vec<&EpubWordHit> = self
            .epub_word_rects
            .iter()
            .filter(|wh| {
                let pos = (wh.block_idx, wh.word_idx);
                pos >= (b0, w0) && pos <= (b1, w1)
            })
            .collect();
        if words.is_empty() {
            return None;
        }
        let quote = words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
        let first = words[0];
        let last = words.last().unwrap();
        // Store within first block if multi-block, or exact word span when single block
        let end_w = if first.block_idx == last.block_idx {
            last.word_idx
        } else {
            w0 + words.len().saturating_sub(1)
        };
        Some((first.block_idx, first.word_idx, end_w, quote))
    }

    pub(super) fn recompute_epub_selection_pill(&mut self, buf_w: u32, top_bars_h: u32, sf: f64) {
        let Some(((b0, w0), (b1, w1))) = self.epub_normalized_selection() else {
            self.epub_selection_pill_rect = None;
            return;
        };
        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        let mut max_x = i32::MIN;
        let mut max_y = i32::MIN;
        for wh in &self.epub_word_rects {
            let pos = (wh.block_idx, wh.word_idx);
            if pos >= (b0, w0) && pos <= (b1, w1) {
                min_x = min_x.min(wh.x);
                min_y = min_y.min(wh.y);
                max_x = max_x.max(wh.x + wh.w as i32);
                max_y = max_y.max(wh.y + wh.h as i32);
            }
        }
        if min_x == i32::MAX {
            self.epub_selection_pill_rect = None;
            return;
        }
        let s = sf as f32;
        let pw = (242.0 * s).round() as u32;
        let ph = (34.0 * s).round() as u32;
        let cx = (min_x + max_x) / 2;
        let px = (cx - pw as i32 / 2).clamp(12, buf_w as i32 - pw as i32 - 12);
        let above_y = min_y - ph as i32 - (8.0 * s).round() as i32;
        let py = if above_y >= top_bars_h as i32 + 6 {
            above_y
        } else {
            max_y + (8.0 * s).round() as i32
        };
        self.epub_selection_pill_rect = Some((px, py, pw, ph));
    }

    pub(super) fn draw_epub_selection_pill(&self, buffer: &mut [u32], buf_w: u32, buf_h: u32, sf: f64) {
        let Some((px, py, pw, ph)) = self.epub_selection_pill_rect else {
            return;
        };
        if !self.is_distinct_epub_mode() {
            return;
        }
        let theme = self.ui_theme();
        let s = sf as f32;
        let rad = (8.0 * s).max(6.0);
        draw_soft_shadow(buffer, buf_w, buf_h, px, py, pw, ph, rad, (10.0 * s).round() as i32, 3, theme.shadow_rgb, theme.shadow_max_alpha);
        fill_rounded_rect(buffer, buf_w, buf_h, px, py, pw, ph, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, px, py, pw, ph, rad, theme.border_subtle);

        let dot_r = (7.5 * s).clamp(5.0, 13.0);
        let mut cx = px + (16.0 * s).round() as i32;
        let cy = py as f32 + ph as f32 * 0.5;
        for (i, &(col, _)) in EPUB_HIGHLIGHT_COLORS.iter().enumerate() {
            let hov = self.hovered_widget == Some(UiHit::BookSelectionPill(BookSelectionPillHit::HighlightColor(i as u8)));
            if hov {
                stroke_circle_aa(buffer, buf_w, buf_h, cx as f32, cy, dot_r + 2.5, 1.5, theme.text_primary);
            }
            fill_circle_aa(buffer, buf_w, buf_h, cx as f32, cy, dot_r, col);
            cx += (22.0 * s).round() as i32;
        }

        fill_rect_clipped(buffer, buf_w, buf_h, cx, py + 6, 1, ph.saturating_sub(12), theme.divider);
        cx += (8.0 * s).round() as i32;

        let lbl_sz = (11.0 * s).clamp(9.0, 19.0);
        let note_w = (68.0 * s).round() as u32;
        let copy_w = (50.0 * s).round() as u32;
        let bh = ph.saturating_sub(8);
        let by = py + 4;

        for (bx, bw, lbl, hit) in [
            (cx, note_w, "Add Note", BookSelectionPillHit::AddNote),
            (cx + note_w as i32 + (4.0 * s).round() as i32, copy_w, "Copy", BookSelectionPillHit::Copy),
        ] {
            let hov = self.hovered_widget == Some(UiHit::BookSelectionPill(hit));
            if hov {
                fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, 5.0 * s, theme.hover_bg);
            }
            let tw = measure_text(lbl, lbl_sz, false);
            draw_text_vcenter(buffer, buf_w, buf_h, bx + ((bw as f32 - tw) * 0.5).round() as i32, by, bh, bx + bw as i32 - 2, lbl, lbl_sz, theme.text_primary, false);
        }
    }

    pub(super) fn hit_test_epub_selection_pill(&self, mx: f64, my: f64, sf: f64) -> Option<BookSelectionPillHit> {
        let (px, py, pw, ph) = self.epub_selection_pill_rect?;
        if !self.is_distinct_epub_mode() {
            return None;
        }
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix < px || ix >= px + pw as i32 || iy < py || iy >= py + ph as i32 {
            return None;
        }
        let s = sf as f32;
        let mut cx = px + (16.0 * s).round() as i32;
        let step = (22.0 * s).round() as i32;
        for i in 0..4u8 {
            if ix >= cx - 10 && ix <= cx + 10 {
                return Some(BookSelectionPillHit::HighlightColor(i));
            }
            cx += step;
        }
        cx += (8.0 * s).round() as i32;
        let note_w = (68.0 * s).round() as i32;
        let copy_w = (50.0 * s).round() as i32;
        if ix >= cx && ix < cx + note_w {
            return Some(BookSelectionPillHit::AddNote);
        }
        let copy_x = cx + note_w + (4.0 * s).round() as i32;
        if ix >= copy_x && ix < copy_x + copy_w {
            return Some(BookSelectionPillHit::Copy);
        }
        Some(BookSelectionPillHit::InsidePill)
    }

    pub(super) fn handle_epub_selection_pill_click(&mut self, hit: BookSelectionPillHit) {
        let Some((block_idx, start_word, end_word, quote)) = self.epub_selected_quote_text() else {
            self.epub_selection_drag = None;
            self.epub_selection_pill_rect = None;
            return;
        };
        match hit {
            BookSelectionPillHit::HighlightColor(color_idx) => {
                let id = self.epub_book_data.next_id.max(1);
                self.epub_book_data.next_id = id + 1;
                self.epub_book_data.highlights.push(EpubHighlight {
                    id,
                    block_idx,
                    start_word,
                    end_word,
                    quote,
                    color_idx,
                    note: None,
                });
                self.save_epub_book_data();
                self.epub_selection_drag = None;
                self.epub_selection_pill_rect = None;
                self.status_notice = Some("Highlighted EPUB passage".to_string());
            }
            BookSelectionPillHit::AddNote => {
                self.input_mode = InputMode::EpubNotePrompt {
                    block_idx,
                    start_word,
                    end_word,
                    quote,
                    color_idx: 0,
                    text_input: String::new(),
                };
                self.epub_selection_drag = None;
                self.epub_selection_pill_rect = None;
            }
            BookSelectionPillHit::Copy => {
                match Self::copy_text_to_os_clipboard(&quote) {
                    Ok(()) => self.status_notice = Some("Copied selected text".to_string()),
                    Err(e) => self.status_notice = Some(format!("Copy failed: {e}")),
                }
                self.epub_selection_drag = None;
                self.epub_selection_pill_rect = None;
            }
            BookSelectionPillHit::InsidePill => {}
        }
        self.request_redraw();
    }

    // -----------------------------------------------------------------------
    // Minimal Distraction-Free EPUB Bottom Bar (Distinct Skin)
    // -----------------------------------------------------------------------

    pub(super) fn draw_distinct_epub_bottom_bar(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        bar_h: u32,
        sf: f64,
    ) {
        if bar_h == 0 || buf_h < bar_h {
            return;
        }
        let theme = self.ui_theme();
        let s = sf as f32;
        let top_y = buf_h.saturating_sub(bar_h) as i32;
        fill_rect_clipped(buffer, buf_w, buf_h, 0, top_y, buf_w, bar_h, theme.statusbar_bg);
        fill_rect_clipped(buffer, buf_w, buf_h, 0, top_y, buf_w, 1, theme.divider);

        let pad = (12.0 * s).round() as i32;
        let btn_h = (22.0 * s).round().clamp(16.0, (bar_h as f32 - 4.0).max(16.0)) as u32;
        let btn_y = top_y + ((bar_h.saturating_sub(btn_h)) / 2) as i32;
        let rad = (5.0 * s).max(3.5);
        let font_sz = (11.0 * s).clamp(9.0, 20.0);
        let small_sz = (10.5 * s).clamp(8.5, 18.0);
        let ic_sz = (12.0 * s).round() as u32;
        let gap = (6.0 * s).round() as i32;

        // Left control: Book Drawer / Contents Sidebar button (mirrors the other 3 formats)
        let sb_w = (26.0 * s).round() as u32;
        let sb_x = pad;
        let sb_hov = self.hovered_widget == Some(UiHit::EpubBottom(EpubBottomHit::ToggleSidebar));
        if self.show_bookmarks {
            fill_rounded_rect(buffer, buf_w, buf_h, sb_x, btn_y, sb_w, btn_h, rad, theme.accent_soft_bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, sb_x, btn_y, sb_w, btn_h, rad, theme.accent);
        } else if sb_hov {
            fill_rounded_rect(buffer, buf_w, buf_h, sb_x, btn_y, sb_w, btn_h, rad, theme.hover_bg);
        } else {
            fill_rounded_rect(buffer, buf_w, buf_h, sb_x, btn_y, sb_w, btn_h, rad, theme.surface_elevated);
            stroke_rounded_rect(buffer, buf_w, buf_h, sb_x, btn_y, sb_w, btn_h, rad, theme.border_subtle);
        }
        draw_icon(
            buffer,
            buf_w,
            buf_h,
            UiIcon::Sidebar,
            sb_x + ((sb_w - ic_sz) / 2) as i32,
            btn_y + ((btn_h - ic_sz) / 2) as i32,
            ic_sz,
            if self.show_bookmarks { theme.accent_soft_fg } else { theme.text_secondary },
        );
        let left_text_x = sb_x + sb_w as i32 + (8.0 * s).round() as i32;

        // Right controls: Find | Aa | Bookmark | Fullscreen
        let fs_w = (26.0 * s).round() as u32;
        let bm_w = (26.0 * s).round() as u32;
        let aa_w = (34.0 * s).round() as u32;
        let find_w = (26.0 * s).round() as u32;

        let fs_x = buf_w as i32 - pad - fs_w as i32;
        let bm_x = fs_x - gap - bm_w as i32;
        let aa_x = bm_x - gap - aa_w as i32;
        let find_x = aa_x - gap - find_w as i32;

        let is_bookmarked = self.current_epub_page_is_bookmarked();
        let search_active = matches!(self.input_mode, InputMode::SearchText { .. });

        // Find button
        let find_hov = self.hovered_widget == Some(UiHit::EpubBottom(EpubBottomHit::FindInDoc));
        if search_active {
            fill_rounded_rect(buffer, buf_w, buf_h, find_x, btn_y, find_w, btn_h, rad, theme.accent_soft_bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, find_x, btn_y, find_w, btn_h, rad, theme.accent);
        } else if find_hov {
            fill_rounded_rect(buffer, buf_w, buf_h, find_x, btn_y, find_w, btn_h, rad, theme.hover_bg);
        } else {
            fill_rounded_rect(buffer, buf_w, buf_h, find_x, btn_y, find_w, btn_h, rad, theme.surface_elevated);
            stroke_rounded_rect(buffer, buf_w, buf_h, find_x, btn_y, find_w, btn_h, rad, theme.border_subtle);
        }
        draw_icon(
            buffer,
            buf_w,
            buf_h,
            UiIcon::Search,
            find_x + ((find_w - ic_sz) / 2) as i32,
            btn_y + ((btn_h - ic_sz) / 2) as i32,
            ic_sz,
            if search_active { theme.accent_soft_fg } else { theme.text_secondary },
        );

        // Aa button
        let aa_hov = self.hovered_widget == Some(UiHit::EpubBottom(EpubBottomHit::ToggleAa)) || self.show_aa_popover;
        if self.show_aa_popover {
            fill_rounded_rect(buffer, buf_w, buf_h, aa_x, btn_y, aa_w, btn_h, rad, theme.accent_soft_bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, aa_x, btn_y, aa_w, btn_h, rad, theme.accent);
        } else if aa_hov {
            fill_rounded_rect(buffer, buf_w, buf_h, aa_x, btn_y, aa_w, btn_h, rad, theme.hover_bg);
        } else {
            fill_rounded_rect(buffer, buf_w, buf_h, aa_x, btn_y, aa_w, btn_h, rad, theme.surface_elevated);
            stroke_rounded_rect(buffer, buf_w, buf_h, aa_x, btn_y, aa_w, btn_h, rad, theme.border_subtle);
        }
        let aaw = measure_text("Aa", font_sz, true);
        draw_text_vcenter(buffer, buf_w, buf_h, aa_x + ((aa_w as f32 - aaw) * 0.5).round() as i32, btn_y, btn_h, aa_x + aa_w as i32 - 2, "Aa", font_sz, if self.show_aa_popover { theme.accent_soft_fg } else { theme.text_primary }, true);

        // Bookmark toggle button (Star/Ribbon)
        let bm_hov = self.hovered_widget == Some(UiHit::EpubBottom(EpubBottomHit::ToggleBookmark));
        if is_bookmarked {
            fill_rounded_rect(buffer, buf_w, buf_h, bm_x, btn_y, bm_w, btn_h, rad, theme.accent_soft_bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, bm_x, btn_y, bm_w, btn_h, rad, theme.accent);
        } else if bm_hov {
            fill_rounded_rect(buffer, buf_w, buf_h, bm_x, btn_y, bm_w, btn_h, rad, theme.hover_bg);
        }
        draw_icon(
            buffer, buf_w, buf_h, UiIcon::Star,
            bm_x + ((bm_w - ic_sz) / 2) as i32,
            btn_y + ((btn_h - ic_sz) / 2) as i32,
            ic_sz,
            if is_bookmarked { 0x00F5_9E0B } else { theme.text_secondary },
        );

        // Fullscreen button
        let fs_hov = self.hovered_widget == Some(UiHit::EpubBottom(EpubBottomHit::ToggleFullscreen));
        if fs_hov {
            fill_rounded_rect(buffer, buf_w, buf_h, fs_x, btn_y, fs_w, btn_h, rad, theme.hover_bg);
        }
        draw_icon(
            buffer, buf_w, buf_h, UiIcon::Fullscreen,
            fs_x + ((fs_w - ic_sz) / 2) as i32,
            btn_y + ((btn_h - ic_sz) / 2) as i32,
            ic_sz,
            if self.is_fullscreen { theme.accent } else { theme.text_secondary },
        );

        // Loc. X of Y · Z min left label immediately left of Find button (Tweak 10)
        let (cur_loc, total_loc) = self.current_epub_loc_pair();
        let loc_txt = self.epub_loc_and_time_label(cur_loc, total_loc);
        let loc_w = measure_text(&loc_txt, small_sz, false).ceil() as i32;
        let loc_x = (find_x - (12.0 * s).round() as i32 - loc_w).max(left_text_x);
        draw_text_vcenter(buffer, buf_w, buf_h, loc_x, top_y, bar_h, find_x - 4, &loc_txt, small_sz, theme.text_muted, false);

        // Center Reading Progress Scrubber
        if let Some((scrub_x, scrub_end)) = self.epub_scrubber_track_x(buf_w, sf) {
            let actual_w = (scrub_end - scrub_x).max(1) as u32;
            let track_h = (4.0 * s).round().max(3.0) as u32;
            let track_y = top_y + ((bar_h.saturating_sub(track_h)) / 2) as i32;
            let ratio = if total_loc > 1 {
                ((cur_loc - 1) as f32 / (total_loc - 1) as f32).clamp(0.0, 1.0)
            } else {
                1.0
            };
            fill_rounded_rect(buffer, buf_w, buf_h, scrub_x, track_y, actual_w, track_h, track_h as f32 * 0.5, theme.border_subtle);
            let fill_w = ((actual_w as f32) * ratio).round() as u32;
            if fill_w > 0 {
                fill_rounded_rect(buffer, buf_w, buf_h, scrub_x, track_y, fill_w, track_h, track_h as f32 * 0.5, theme.accent);
            }
            let knob_x = scrub_x as f32 + (actual_w as f32) * ratio;
            let knob_y = top_y as f32 + bar_h as f32 * 0.5;
            fill_circle_aa(buffer, buf_w, buf_h, knob_x, knob_y, (5.5 * s).clamp(4.5, 8.5), theme.accent);

            // Left: Chapter title (or active status notice if recent)
            let (chap_title, _) = self.active_epub_chapter_title_and_block();
            let left_txt = self.status_notice.as_deref().unwrap_or(&chap_title);
            let left_avail = (scrub_x - left_text_x - 10).max(20) as f32;
            let clipped = truncate_ellipsis(left_txt, left_avail, small_sz, false);
            draw_text_vcenter(buffer, buf_w, buf_h, left_text_x, top_y, bar_h, scrub_x - 8, &clipped, small_sz, theme.text_secondary, false);
        } else {
            let (chap_title, _) = self.active_epub_chapter_title_and_block();
            let clipped = truncate_ellipsis(&chap_title, (loc_x - left_text_x - 8).max(20) as f32, small_sz, false);
            draw_text_vcenter(buffer, buf_w, buf_h, left_text_x, top_y, bar_h, loc_x - 4, &clipped, small_sz, theme.text_secondary, false);
        }
    }

    pub(super) fn epub_loc_and_time_label(&self, cur_loc: usize, total_loc: usize) -> String {
        if let Some(t_left) = self.epub_chapter_time_left_label() {
            format!("Loc. {cur_loc} of {total_loc} · {t_left}")
        } else {
            format!("Loc. {cur_loc} of {total_loc}")
        }
    }

    pub(super) fn epub_scrubber_track_x(&self, buf_w: u32, sf: f64) -> Option<(i32, i32)> {
        let s = sf as f32;
        let pad = (12.0 * s).round() as i32;
        let sb_w = (26.0 * s).round() as i32;
        let left_text_x = pad + sb_w + (8.0 * s).round() as i32;

        let fs_w = (26.0 * s).round() as i32;
        let bm_w = (26.0 * s).round() as i32;
        let aa_w = (34.0 * s).round() as i32;
        let find_w = (26.0 * s).round() as i32;
        let gap = (6.0 * s).round() as i32;
        let fs_x = buf_w as i32 - pad - fs_w;
        let bm_x = fs_x - gap - bm_w;
        let aa_x = bm_x - gap - aa_w;
        let find_x = aa_x - gap - find_w;

        let small_sz = (10.5 * s).clamp(8.5, 18.0);
        let (cur_loc, total_loc) = self.current_epub_loc_pair();
        let loc_txt = self.epub_loc_and_time_label(cur_loc, total_loc);
        let loc_w = measure_text(&loc_txt, small_sz, false).ceil() as i32;
        let loc_x = (find_x - (12.0 * s).round() as i32 - loc_w).max(left_text_x);

        let scrub_w = ((buf_w as f32 * 0.24).clamp(80.0 * s, 280.0 * s)).round() as i32;
        let scrub_right = loc_x - (12.0 * s).round() as i32;
        let scrub_x = ((buf_w as i32 - scrub_w) / 2).min(scrub_right - scrub_w).max(left_text_x + 60);
        if scrub_right - scrub_x >= (64.0 * s).round() as i32 {
            let actual_w = (scrub_right - scrub_x).min(scrub_w);
            Some((scrub_x, scrub_x + actual_w))
        } else {
            None
        }
    }

    pub(super) fn hit_test_distinct_epub_bottom_bar(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        bar_h: u32,
        sf: f64,
    ) -> Option<EpubBottomHit> {
        let top_y = buf_h.saturating_sub(bar_h) as i32;
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if iy < top_y || iy >= buf_h as i32 || ix < 0 || ix >= buf_w as i32 {
            return None;
        }
        let s = sf as f32;
        let pad = (12.0 * s).round() as i32;
        let sb_w = (26.0 * s).round() as i32;
        if ix >= pad && ix < pad + sb_w {
            return Some(EpubBottomHit::ToggleSidebar);
        }

        let fs_w = (26.0 * s).round() as i32;
        let bm_w = (26.0 * s).round() as i32;
        let aa_w = (34.0 * s).round() as i32;
        let find_w = (26.0 * s).round() as i32;
        let gap = (6.0 * s).round() as i32;

        let fs_x = buf_w as i32 - pad - fs_w;
        let bm_x = fs_x - gap - bm_w;
        let aa_x = bm_x - gap - aa_w;
        let find_x = aa_x - gap - find_w;

        if ix >= fs_x && ix < fs_x + fs_w {
            return Some(EpubBottomHit::ToggleFullscreen);
        }
        if ix >= bm_x && ix < bm_x + bm_w {
            return Some(EpubBottomHit::ToggleBookmark);
        }
        if ix >= aa_x && ix < aa_x + aa_w {
            return Some(EpubBottomHit::ToggleAa);
        }
        if ix >= find_x && ix < find_x + find_w {
            return Some(EpubBottomHit::FindInDoc);
        }

        if let Some((scrub_x, scrub_end)) = self.epub_scrubber_track_x(buf_w, sf) {
            if ix >= scrub_x - 6 && ix <= scrub_end + 6 {
                let ratio = ((ix - scrub_x) as f32 / (scrub_end - scrub_x).max(1) as f32).clamp(0.0, 1.0);
                return Some(EpubBottomHit::Scrubber(ratio));
            }
        }
        None
    }

    pub(super) fn handle_distinct_epub_bottom_click(&mut self, hit: EpubBottomHit, buf_w: u32, buf_h: u32, sf: f64) {
        match hit {
            EpubBottomHit::ToggleSidebar => {
                self.show_bookmarks = !self.show_bookmarks;
                self.invalidate_page_caches();
            }
            EpubBottomHit::FindInDoc => {
                self.show_aa_popover = false;
                self.input_mode = InputMode::SearchText {
                    query_input: self.search_query.clone(),
                };
                self.search_bar_focused = true;
                self.status_notice = None;
            }
            EpubBottomHit::ToggleAa => {
                self.show_aa_popover = !self.show_aa_popover;
                self.show_reading_mode_popover = false;
                self.show_scroll_mode_popover = false;
                self.show_notifications = false;
            }
            EpubBottomHit::ToggleBookmark => {
                self.toggle_current_epub_bookmark();
            }
            EpubBottomHit::ToggleFullscreen => {
                self.toggle_fullscreen();
            }
            EpubBottomHit::Scrubber(ratio) => {
                self.jump_epub_to_progress_ratio(ratio, buf_w, buf_h, sf);
            }
        }
        self.request_redraw();
    }

    // -----------------------------------------------------------------------
    // Distinct Phase 3: Markdown Outline & LaTeX Document Structure Sidebar
    // -----------------------------------------------------------------------

    pub(super) fn is_distinct_outline_mode(&self) -> bool {
        self.ui_skin == UiSkin::Distinct
            && matches!(self.active_doc_kind(), DocKind::Markdown | DocKind::LaTeX)
    }

    fn distinct_outline_metrics(s: f32) -> (u32, u32, i32) {
        let hdr_h = (34.0 * s).round().max(26.0) as u32;
        let row_h = (28.0 * s).round().max(22.0) as u32;
        let pad = (10.0 * s).round() as i32;
        (hdr_h, row_h, pad)
    }

    pub(super) fn draw_distinct_outline_sidebar(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        scale_factor: f64,
    ) {
        let theme = self.ui_theme();
        let s = scale_factor as f32;
        let top_y = top_bars_h as i32;
        let total_h = buf_h.saturating_sub(top_bars_h);
        if total_h == 0 || sidebar_w < 32 {
            return;
        }
        let (hdr_h, row_h, pad) = Self::distinct_outline_metrics(s);
        let is_tex = self.active_doc_kind() == DocKind::LaTeX;

        fill_rect_clipped(buffer, buf_w, buf_h, 0, top_y, sidebar_w, total_h, theme.sidebar_bg);
        fill_rect_clipped(buffer, buf_w, buf_h, sidebar_w.saturating_sub(1) as i32, top_y, 1, total_h, theme.divider);

        // Header strip
        fill_rect_clipped(buffer, buf_w, buf_h, 0, top_y, sidebar_w.saturating_sub(1), hdr_h, theme.sidebar_header_bg);
        fill_rect_clipped(buffer, buf_w, buf_h, 0, top_y + hdr_h as i32 - 1, sidebar_w.saturating_sub(1), 1, theme.divider);

        let title = if is_tex { "Document Structure" } else { "Outline" };
        let title_sz = (12.0 * s).clamp(10.0, 20.0);
        let badge_sz = (10.0 * s).clamp(8.5, 16.0);

        let badge_txt = if is_tex {
            let eq_cnt = self
                .markdown_doc
                .as_ref()
                .map(|d| {
                    d.blocks
                        .iter()
                        .filter(|b| matches!(b, crate::markdown::MdBlock::MathBlock(_)))
                        .count()
                })
                .unwrap_or(0);
            format!("{eq_cnt} eq")
        } else {
            format!("{} H", self.bookmarks.len())
        };
        let bw = (measure_text(&badge_txt, badge_sz, true).ceil() as u32 + (12.0 * s).round() as u32)
            .max((30.0 * s).round() as u32);
        let bh = (18.0 * s).round().clamp(14.0, (hdr_h as f32 - 6.0).max(14.0)) as u32;
        let bx = (sidebar_w as i32 - pad - bw as i32).max(pad);
        let by = top_y + ((hdr_h.saturating_sub(bh)) / 2) as i32;
        fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, (bh as f32) * 0.5, theme.accent_soft_bg);
        let btw = measure_text(&badge_txt, badge_sz, true);
        draw_text_vcenter(
            buffer,
            buf_w,
            buf_h,
            bx + ((bw as f32 - btw) * 0.5).round() as i32,
            by,
            bh,
            bx + bw as i32 - 2,
            &badge_txt,
            badge_sz,
            theme.accent_soft_fg,
            true,
        );

        draw_text_vcenter(
            buffer,
            buf_w,
            buf_h,
            pad,
            top_y,
            hdr_h,
            (bx - 6).max(pad + 16),
            title,
            title_sz,
            theme.text_primary,
            true,
        );

        let list_y = top_y + hdr_h as i32 + (4.0 * s).round() as i32;
        if self.bookmarks.is_empty() {
            let hint_sz = (11.0 * s).clamp(9.0, 18.0);
            let hint = if is_tex { "No \\section headings" } else { "No # headings yet" };
            draw_text_clipped(
                buffer,
                buf_w,
                buf_h,
                pad,
                list_y + (12.0 * s).round() as i32,
                sidebar_w as i32 - pad,
                hint,
                hint_sz,
                theme.text_muted,
                false,
            );
            return;
        }

        // Track active heading nearest to current scroll position
        let cur_blk = self.epub_book_current_first_block();
        let active_idx = cur_blk
            .and_then(|blk| {
                self.bookmarks
                    .iter()
                    .enumerate()
                    .filter(|(_, bm)| bm.page_index <= blk)
                    .max_by_key(|(_, bm)| bm.page_index)
                    .map(|(i, _)| i)
            })
            .or(self.selected_bookmark);

        let text_sz = (11.5 * s).clamp(9.5, 20.0);
        let tag_sz = (9.5 * s).clamp(8.0, 16.0);
        let rad = (5.0 * s).clamp(3.0, 8.0);
        let max_rows = ((buf_h as i32 - list_y).max(0) as u32 / row_h.max(1)) as usize;
        let start = self.bookmark_scroll_y.min(self.bookmarks.len().saturating_sub(1));

        for (vis_i, bm_idx) in (start..self.bookmarks.len()).take(max_rows).enumerate() {
            let bm = &self.bookmarks[bm_idx];
            let ry = list_y + (vis_i as u32 * row_h) as i32;
            let rx = (6.0 * s).round() as i32;
            let rw = sidebar_w.saturating_sub((14.0 * s).round() as u32);
            let rh = row_h.saturating_sub((3.0 * s).round().max(2.0) as u32);
            let is_active = active_idx == Some(bm_idx);
            let is_hov = self.hovered_widget == Some(UiHit::SidebarRow(bm_idx));

            if is_active {
                fill_rounded_rect(buffer, buf_w, buf_h, rx, ry + 1, rw, rh, rad, theme.active_row_bg);
                fill_rect_clipped(buffer, buf_w, buf_h, rx + 1, ry + 4, (3.0 * s).round().max(2.0) as u32, rh.saturating_sub(6), theme.accent);
            } else if is_hov {
                fill_rounded_rect(buffer, buf_w, buf_h, rx, ry + 1, rw, rh, rad, theme.hover_bg);
            }

            let indent = ((bm.level.saturating_sub(1).min(3) as f32) * 11.0 * s).round() as i32;
            let tag = if is_tex {
                match bm.level {
                    1 => "§",
                    2 => "§§",
                    _ => "•",
                }
            } else {
                match bm.level {
                    1 => "H1",
                    2 => "H2",
                    _ => "H3",
                }
            };
            let tx0 = rx + (8.0 * s).round() as i32 + indent;
            let tag_w = measure_text(tag, tag_sz, true).ceil() as i32 + (5.0 * s).round() as i32;
            let tag_col = if is_active { theme.accent_soft_fg } else { theme.text_muted };
            draw_text_vcenter(buffer, buf_w, buf_h, tx0, ry + 1, rh, rx + rw as i32 - 4, tag, tag_sz, tag_col, true);

            let lbl_x = tx0 + tag_w;
            let lbl_max = rx + rw as i32 - (6.0 * s).round() as i32;
            if lbl_x < lbl_max {
                let title_col = if is_active { theme.text_primary } else { theme.text_secondary };
                draw_text_vcenter(
                    buffer,
                    buf_w,
                    buf_h,
                    lbl_x,
                    ry + 1,
                    rh,
                    lbl_max,
                    &bm.title,
                    text_sz,
                    title_col,
                    is_active || bm.level <= 1,
                );
            }
        }
    }

    pub(super) fn hit_test_distinct_outline_sidebar(
        &self,
        mx: f64,
        my: f64,
        sidebar_w: u32,
        top_bars_h: u32,
        scale_factor: f64,
    ) -> Option<SidebarHit> {
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix < 0 || ix >= sidebar_w as i32 {
            return None;
        }
        let s = scale_factor as f32;
        let (hdr_h, row_h, _) = Self::distinct_outline_metrics(s);
        let list_y = top_bars_h as i32 + hdr_h as i32 + (4.0 * s).round() as i32;
        if iy < list_y || self.bookmarks.is_empty() {
            return None;
        }
        let vis_i = ((iy - list_y) as u32 / row_h.max(1)) as usize;
        let idx = self.bookmark_scroll_y + vis_i;
        if idx < self.bookmarks.len() {
            Some(SidebarHit::BookmarkRow {
                index: idx,
                on_delete: false,
            })
        } else {
            None
        }
    }
}

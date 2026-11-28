use crate::engine::{
    AnnotationColor, Bookmark, HeaderFooterProfile, MarginPosition, PdfEngine, SearchMatch,
    StampImageBuffer,
};
use anyhow::Result;
use pdfium_render::prelude::*;
use softbuffer::{Context as SoftContext, Surface};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};
use winit::window::Window;

use super::book_ui::{
    AaPopoverHit, BookInfoPopoverHit, BookSidebarHit, BookSidebarTab, BookSelectionPillHit,
    EpubBookData, EpubBottomHit, EpubFontChoice, EpubReaderPrefs, EpubWordHit, LineSpacingPreset,
    MarginPreset,
};
use super::bottom_bar::BottomBarAction;
use super::combine_modal::{CombineHit, CombineModalState};
use super::compare_mode::{CompareHit, CompareModeState};
use super::deskew::{DeskewBarState, DeskewHit};
use super::doc_props_modal::{
    ActiveNotePopover, AnnotContextHit, AnnotationContextMenuState, DocPropsHit,
    DocPropsModalState, NotePopoverHit, UnsavedChangesHit, UnsavedChangesModalState,
};
use super::doc_tools_modal::{
    HeaderFooterHit, HeaderFooterModalState, PageNumberHit, PageNumberModalState,
};
use super::excel_export_modal::{ExcelExportHit, ExcelExportModalState};
use super::image_tools_modal::{
    CreatePdfImagesHit, CreatePdfImagesModalState, ExportImagesHit, ExportImagesModalState,
};
use super::measure::{MeasureBarHit, MeasureUnit, MeasurementResult};
use super::overlays::OverlayHit;
use super::print_preview::{ManualDuplexHit, ManualDuplexWaitState, PrintPreviewHit, PrintPreviewState};
use super::split_pages_modal::{SplitPagesHit, SplitPagesModalState};
use super::stamp_manager_modal::{StampManagerHit, StampManagerModalState};
use super::text_view::{CachedTextViewPage, TextViewHit};
use super::theme::{DocKind, UiSkin, UiTheme, UiThemeMode};
use super::toolbar::{CustomizeHit, ToolbarAction, ToolbarConfig};
use super::user_guide::{UserGuideHit, UserGuideState};
use super::watermark_modal::{WatermarkHit, WatermarkModalState};

#[allow(dead_code)]
pub(super) const BG_COLOR_U32: u32 = 0x00E9EEF5;
#[allow(dead_code)]
pub(super) const SIDEBAR_BG_U32: u32 = 0x00F8FAFC;
pub(super) const SIDEBAR_WIDTH_LOGICAL: u32 = 248;
/// Unified top bar hosting both menus (`File..Help`) and the inline customizable icon toolbar.
pub(super) const MENU_BAR_HEIGHT_LOGICAL: u32 = 32;
pub(super) const TAB_BAR_HEIGHT_LOGICAL: u32 = 28;
/// Centered bottom page-view & status bar.
pub(super) const STATUS_BAR_HEIGHT_LOGICAL: u32 = 30;
pub(super) const PAGE_MARGIN_PX: u32 = 24;
pub(super) const SPREAD_GAP_PX: u32 = 12;
pub(super) const THUMB_W: u32 = 140;
pub(super) const THUMB_H: u32 = 198;
pub(super) const MAX_THUMB_CACHE_ENTRIES: usize = 300;
pub(super) const TILE_SIZE_PX: u32 = 512;
pub(super) const MAX_TILE_CACHE_BYTES: usize = 16 * 1024 * 1024;
pub(super) const MAX_TILE_CACHE_ENTRIES: usize =
    MAX_TILE_CACHE_BYTES / ((TILE_SIZE_PX as usize) * (TILE_SIZE_PX as usize) * 4);
pub(super) const MIN_ZOOM_LEVEL: f32 = 0.25;
pub(super) const MAX_ZOOM_LEVEL: f32 = 8.0;
pub(super) const MIN_UI_SCALE: f32 = 0.80;
pub(super) const MAX_UI_SCALE: f32 = 1.60;
pub(super) const MAX_NAV_HISTORY: usize = 50;
pub(super) const MAX_RECENT_FILES: usize = 10;
pub(super) const MAX_LAST_POSITIONS: usize = 50;
pub(super) const MAX_UNDO_STATES: usize = 8;
/// v1.1: total RAM allowed for undo snapshots per tab. The oldest snapshots are dropped first;
/// the most recent one is always kept even if it alone exceeds the budget.
pub(super) const UNDO_BYTES_BUDGET: usize = 64 * 1024 * 1024;
/// Window in which a second `U` press confirms Flatten & Unlock on a signed document.
pub(super) const FLATTEN_CONFIRM_WINDOW: Duration = Duration::from_secs(5);

/// Top Menu Bar categories on the unified top bar.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MenuCategory {
    File,
    View,
    Edit,
    Page,
    Tools,
    Export,
    Favorites,
    Help,
}

impl MenuCategory {
    pub const ALL: [MenuCategory; 8] = [
        MenuCategory::File,
        MenuCategory::View,
        MenuCategory::Edit,
        MenuCategory::Page,
        MenuCategory::Tools,
        MenuCategory::Export,
        MenuCategory::Favorites,
        MenuCategory::Help,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MenuCategory::File => "File",
            MenuCategory::View => "View",
            MenuCategory::Edit => "Edit",
            MenuCategory::Page => "Page",
            MenuCategory::Tools => "Tools",
            MenuCategory::Export => "Export",
            MenuCategory::Favorites => "Favorites",
            MenuCategory::Help => "Help",
        }
    }

    pub fn items(self) -> &'static [&'static str] {
        match self {
            MenuCategory::File => &[
                "Open Documents in Tab (O)",
                "Create PDF from Images...",
                "Combine Multiple PDFs... (Shift+M)",
                "Import Bookmarks .txt (L)",
                "Document Properties & Privacy...",
                "Save PDF (S)",
                "Save As... (Ctrl+Shift+S)",
                "Print... (Ctrl+P / Cmd+P)",
                "Close Tab (Ctrl+W)",
                "Close Other Tabs",
                "Close All Tabs",
                "Encrypt Portable Config (micropdf.ini)",
            ],
            MenuCategory::View => &[
                "Fit Full Page (1)",
                "Fit Page Width (2)",
                "Fit Text Width — Crop Margins (3)",
                "Two-Page Side-by-Side (4)",
                "Scroll: Single Page",
                "Scroll: Continuous Vertical (5)",
                "Scroll: Continuous Horizontal Book",
                "Scroll: Magazine / Book",
                "Magazine Page-Turn Speed...",
                "Text View — Reflowable Reader (T)",
                "Zoom In (+ / Ctrl+Wheel Up)",
                "Zoom Out (- / Ctrl+Wheel Down)",
                "Reset Zoom 100% (0)",
                "Toggle Thumbnail Grid (Tab)",
                "Toggle Bookmark Sidebar (B)",
                "Cycle Reading Mode (E-Ink / Sepia / Sage / Dark / Night) (V)",
                "Toggle Fullscreen (F11)",
                "Presentation Mode (F5)",
                "Show Toolbar",
                "Customize Toolbar...",
                "UI Scale: Larger (+10%)",
                "UI Scale: Smaller (-10%)",
                "UI Scale: Reset 100%",
                "Switch Light / Dark UI",
            ],
            MenuCategory::Edit => &[
                "Undo (Ctrl+Z / Cmd+Z)",
                "Redo (Ctrl+Y / Ctrl+Shift+Z)",
                "Copy Selected Text (Ctrl+C / Cmd+C)",
                "Find in the document... (Ctrl+F / Cmd+F)",
                "Mouse Mode: Select Text / Fill Forms",
                "Annotate: Highlight Text",
                "Annotate: Underline Text",
                "Annotate: Strikethrough Text",
                "Annotate: Sticky Note / Comment",
                "Annotate: Freehand Pen (Ink)",
                "Annotate: Text Box (FreeText)",
                "Annotate: Rectangle Shape",
                "Annotate: Arrow Shape",
                "Cycle Annotation Color",
                "Measure Distance & Area (Ruler)",
                "Mouse Mode: Black-Box Redact",
                "Mouse Mode: Crop Page Area",
            ],
            MenuCategory::Page => &[
                "Jump to Page... (G)",
                "Rotate Current Page 90° (R)",
                "Deskew Page(s)...",
                "Insert Blank Page After (I)",
                "Insert Blank Page Before",
                "Insert PDF(s) Before / After Page...",
                "Delete Current Page (D)",
                "Convert Page to Black & White (K)",
                "Reset Page Crop",
            ],
            MenuCategory::Tools => &[
                "Page Numbering... (N)",
                "Header & Footer / Clear Existing... (H)",
                "Add Watermark... (W)",
                "Stamps & Signatures... (P)",
                "Split / Extract Pages... (X)",
                "Split to Single-Page PDFs...",
                "Compare with Another PDF...",
                "Flatten & Unlock PDF (U)",
                "Optimize...",
            ],
            MenuCategory::Export => &[
                "Export as Word (.docx)...",
                "Export as Excel (.xlsx)...",
                "Export as Markdown (.md)...",
                "Export as LaTeX (.tex)...",
                "Export Pages as Images (PNG/JPG)...",
                "Export as EPUB (.epub)...",
            ],
            MenuCategory::Favorites => &[
                "Open Favorites Library... (Ctrl+D)",
                "Add Current Document to Favorites",
            ],
            MenuCategory::Help => &[
                "User Guide (How to Use)",
                "Keyboard & Mouse Shortcuts (? / F1)",
                "Bookmark Import Format (.txt)",
                "UI Skin: Classic / Distinct",
                "About MicroPDF",
            ],
        }
    }
}

/// Interactive Mouse Drag / Click Mode in Single-Page / Spread Views.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MouseMode {
    Select,
    Hand,
    Highlight,
    Underline,
    Strikethrough,
    StickyNote,
    InkPen,
    FreeText,
    RectShape,
    ArrowShape,
    Measure,
    Redact,
    Crop,
    StampImage,
}

impl MouseMode {
    pub fn short_label(self) -> &'static str {
        match self {
            MouseMode::Select => "Select",
            MouseMode::Hand => "Hand",
            MouseMode::Highlight => "Highlight",
            MouseMode::Underline => "Underline",
            MouseMode::Strikethrough => "Strike",
            MouseMode::StickyNote => "Note",
            MouseMode::InkPen => "Pen",
            MouseMode::FreeText => "TextBox",
            MouseMode::RectShape => "Rect",
            MouseMode::ArrowShape => "Arrow",
            MouseMode::Measure => "Ruler",
            MouseMode::Redact => "REDACT",
            MouseMode::Crop => "CROP",
            MouseMode::StampImage => "STAMP",
        }
    }

    pub fn uses_annotation_color(self) -> bool {
        matches!(
            self,
            MouseMode::Highlight
                | MouseMode::Underline
                | MouseMode::Strikethrough
                | MouseMode::StickyNote
                | MouseMode::InkPen
                | MouseMode::FreeText
                | MouseMode::RectShape
                | MouseMode::ArrowShape
        )
    }
}

/// Zero-RAM CPU-rendered Reading Filter (`V` key, Bottom Bar picker, or `View` menu).
/// Research-backed palettes tuned to reduce luminance glare in daytime and halation/blue-light at night.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ReadingFilter {
    Normal,
    /// Matte neutral low-glare paper (recommended for daytime reading).
    PaperEInk,
    /// Warm book parchment / cream.
    WarmSepia,
    /// Ultra-pale desaturated mint paper for long study sessions.
    SageEyeCare,
    /// Anti-halation soft charcoal dark mode for dim rooms.
    DarkInvert,
    /// Deep espresso dark with warm amber text (recommended for night reading).
    NightAmber,
}

impl ReadingFilter {
    pub const ALL: [Self; 6] = [
        Self::Normal,
        Self::PaperEInk,
        Self::WarmSepia,
        Self::SageEyeCare,
        Self::DarkInvert,
        Self::NightAmber,
    ];

    pub fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|&m| m == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    /// Short compact label for the Bottom Bar pill.
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::PaperEInk => "E-Ink",
            Self::WarmSepia => "Sepia",
            Self::SageEyeCare => "Sage",
            Self::DarkInvert => "Dark",
            Self::NightAmber => "Night",
        }
    }

    /// Descriptive title shown in the Reading Mode picker popover (including Best for day / night labels).
    pub fn picker_title(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::PaperEInk => "Paper (E-Ink) — Best for day",
            Self::WarmSepia => "Sepia (Cream)",
            Self::SageEyeCare => "Sage (Eye Care)",
            Self::DarkInvert => "Soft Dark",
            Self::NightAmber => "Night Amber — Best for night",
        }
    }

    /// Secondary guidance subtitle shown in the Reading Mode picker popover.
    pub fn picker_hint(self) -> &'static str {
        match self {
            Self::Normal => "Original PDF colors",
            Self::PaperEInk => "Matte low-glare neutral gray paper",
            Self::WarmSepia => "Warm book parchment · day & dusk",
            Self::SageEyeCare => "Soft pale mint · long study sessions",
            Self::DarkInvert => "Anti-halation charcoal · dim rooms",
            Self::NightAmber => "Warm amber on dark · lowest blue light",
        }
    }

    /// `(background_rgb, text_rgb)` preview swatch colors for the picker popover.
    pub fn swatch_colors(self) -> (u32, u32) {
        match self {
            Self::Normal => (0x00FF_FFFF, 0x001E_293B),
            Self::PaperEInk => (0x00EA_EBE6, 0x001E_1F1D),
            Self::WarmSepia => (0x00F7_F0DF, 0x002B_2318),
            Self::SageEyeCare => (0x00E9_F2EA, 0x001D_271F),
            Self::DarkInvert => (0x001E_2024, 0x00D8_DCE2),
            Self::NightAmber => (0x001A_1713, 0x00E5_CFA8),
        }
    }

    pub fn to_ini_str(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::PaperEInk => "PaperEInk",
            Self::WarmSepia => "WarmSepia",
            Self::SageEyeCare => "SageEyeCare",
            Self::DarkInvert => "DarkInvert",
            Self::NightAmber => "NightAmber",
        }
    }

    pub fn from_ini_str(s: &str) -> Self {
        match s.trim() {
            "PaperEInk" => Self::PaperEInk,
            "WarmSepia" => Self::WarmSepia,
            "SageEyeCare" => Self::SageEyeCare,
            "DarkInvert" => Self::DarkInvert,
            "NightAmber" => Self::NightAmber,
            _ => Self::Normal,
        }
    }
}

/// Independent page scrolling flow combined orthogonally with Fit/Layout (`ViewMode`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScrollMode {
    /// Classic discrete page turns (zero neighbor prefetch).
    SinglePage,
    /// Smooth continuous vertical scroll across all pages (works with FitPage, FitWidth, FitTextWidth, TwoPageSpread).
    ContinuousVertical,
    /// Smooth continuous horizontal "book ribbon" scroll across all pages left-to-right.
    ContinuousHorizontal,
    /// Two-page open Magazine / Book spread with spine crease and 220 ms page-turn animation (PDF, EPUB, MD, LaTeX).
    MagazineBook,
}

impl ScrollMode {
    pub fn next(self) -> Self {
        match self {
            Self::SinglePage => Self::ContinuousVertical,
            Self::ContinuousVertical => Self::ContinuousHorizontal,
            Self::ContinuousHorizontal => Self::MagazineBook,
            Self::MagazineBook => Self::SinglePage,
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::SinglePage => "Single",
            Self::ContinuousVertical => "Vert",
            Self::ContinuousHorizontal => "Book",
            Self::MagazineBook => "Mag",
        }
    }

    pub fn status_label(self) -> &'static str {
        match self {
            Self::SinglePage => "Single Page",
            Self::ContinuousVertical => "Continuous Vertical Scroll",
            Self::ContinuousHorizontal => "Continuous Horizontal Book Scroll",
            Self::MagazineBook => "Magazine / Book Mode",
        }
    }

    pub fn to_ini_str(self) -> &'static str {
        match self {
            Self::SinglePage => "SinglePage",
            Self::ContinuousVertical => "ContinuousVertical",
            Self::ContinuousHorizontal => "ContinuousHorizontal",
            Self::MagazineBook => "MagazineBook",
        }
    }

    pub const ALL: [Self; 4] = [
        Self::SinglePage,
        Self::ContinuousVertical,
        Self::ContinuousHorizontal,
        Self::MagazineBook,
    ];

    pub fn picker_title(self) -> &'static str {
        match self {
            Self::SinglePage => "Single Page",
            Self::ContinuousVertical => "Continuous Vertical",
            Self::ContinuousHorizontal => "Continuous Horizontal",
            Self::MagazineBook => "Magazine / Book",
        }
    }

    pub fn picker_hint(self) -> &'static str {
        match self {
            Self::SinglePage => "Discrete page-by-page turning",
            Self::ContinuousVertical => "Smooth top-to-bottom scroll (5)",
            Self::ContinuousHorizontal => "Side-by-side horizontal ribbon",
            Self::MagazineBook => "Cover page + 2-page spreads & flip",
        }
    }

    pub fn badge_label(self) -> &'static str {
        match self {
            Self::SinglePage => "1P",
            Self::ContinuousVertical => "↕",
            Self::ContinuousHorizontal => "↔",
            Self::MagazineBook => "2P",
        }
    }

    pub fn from_ini_str(s: &str) -> Self {
        match s.trim() {
            "ContinuousVertical" => Self::ContinuousVertical,
            "ContinuousHorizontal" => Self::ContinuousHorizontal,
            "MagazineBook" | "magazinebook" | "magazine" => Self::MagazineBook,
            _ => Self::SinglePage,
        }
    }
}

/// Page-turn animation state for Apple Books 3D curl & Magazine / Book mode.
/// `prev_spread` is allocated ONLY while the ~420 ms animation is running and dropped immediately on finish.
#[derive(Clone, Debug)]
pub(super) struct PageTurnAnim {
    pub(super) started_at: Instant,
    pub(super) duration_ms: u64,
    /// `+1` for turning forward (next page/spread), `-1` for turning backward.
    pub(super) direction: i8,
    /// Outgoing spread scroll offset so frame 0 can render the outgoing spread into `prev_spread`.
    pub(super) from_scroll_y: Option<f32>,
    /// Temporary outgoing spread pixel buffer `(w, h, pixels)` freed when animation ends.
    pub(super) prev_spread: Option<(u32, u32, Vec<u32>)>,
}

/// Full-text search hit inside an EPUB, Markdown, or LaTeX `MarkdownDoc`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MdSearchMatch {
    pub(super) ast_block_idx: usize,
    pub(super) word_start: usize,
    pub(super) word_end: usize,
    pub(super) snippet: String,
}

/// Bounded 24 MB on-demand decoded image cache for EPUB/Markdown/LaTeX so large image-heavy
/// books never freeze on UI zoom / layout rebuilds while keeping zero RAM on inactive tabs.
pub(super) const MAX_MD_IMAGE_CACHE_BYTES: usize = 24 * 1024 * 1024;

#[derive(Default)]
pub(super) struct MdDecodedImageCache {
    pub(super) entries: HashMap<(String, u32, u32), (u32, u32, Vec<u32>)>,
    pub(super) total_bytes: usize,
    pub(super) dims: HashMap<String, (u32, u32)>,
}

impl MdDecodedImageCache {
    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.total_bytes = 0;
        self.dims.clear();
    }

    pub(super) fn insert_decoded(&mut self, key: (String, u32, u32), w: u32, h: u32, px: Vec<u32>) {
        let bytes = px.len() * std::mem::size_of::<u32>() + 64;
        if self.total_bytes + bytes > MAX_MD_IMAGE_CACHE_BYTES {
            self.entries.clear();
            self.total_bytes = 0;
        }
        if bytes <= MAX_MD_IMAGE_CACHE_BYTES {
            self.total_bytes += bytes;
            self.entries.insert(key, (w, h, px));
        }
    }
}

/// Page, Text & Grid View Modes (`1`, `2`, `3`, `4`, `5`, `T`, and `Tab`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ViewMode {
    FitPage,
    FitWidth,
    FitTextWidth,
    TwoPageSpread,
    ContinuousScroll,
    TextView,
    ThumbnailGrid,
}

impl ViewMode {
    #[allow(dead_code)]
    pub fn short_label(self) -> &'static str {
        match self {
            ViewMode::FitPage => "FitPage",
            ViewMode::FitWidth => "FitWidth",
            ViewMode::FitTextWidth => "FitText",
            ViewMode::TwoPageSpread => "2-Page",
            ViewMode::ContinuousScroll => "Cont",
            ViewMode::TextView => "Text",
            ViewMode::ThumbnailGrid => "Grid",
        }
    }

    pub fn is_page_view(self) -> bool {
        !matches!(self, ViewMode::ThumbnailGrid | ViewMode::TextView)
    }

    pub fn to_ini_str(self) -> &'static str {
        match self {
            ViewMode::FitPage => "FitPage",
            ViewMode::FitWidth => "FitWidth",
            ViewMode::FitTextWidth => "FitTextWidth",
            ViewMode::TwoPageSpread => "TwoPageSpread",
            ViewMode::ContinuousScroll => "ContinuousScroll",
            ViewMode::TextView => "TextView",
            ViewMode::ThumbnailGrid => "ThumbnailGrid",
        }
    }

    pub fn from_ini_str(s: &str) -> Self {
        match s.trim() {
            "FitWidth" => ViewMode::FitWidth,
            "FitTextWidth" => ViewMode::FitTextWidth,
            "TwoPageSpread" => ViewMode::TwoPageSpread,
            "ContinuousScroll" => ViewMode::ContinuousScroll,
            "TextView" => ViewMode::TextView,
            "ThumbnailGrid" => ViewMode::ThumbnailGrid,
            _ => ViewMode::FitPage,
        }
    }
}

/// Comic Book (.cbz) page fitting mode.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub(super) enum ComicFitMode {
    #[default]
    FitPage,
    FitWidth,
    FitHeight,
}

#[allow(dead_code)]
impl ComicFitMode {
    pub(super) fn label(self) -> &'static str {
        match self {
            ComicFitMode::FitPage => "Fit Page",
            ComicFitMode::FitWidth => "Fit Width",
            ComicFitMode::FitHeight => "Fit Height",
        }
    }

    pub(super) fn short_label(self) -> &'static str {
        match self {
            ComicFitMode::FitPage => "Fit",
            ComicFitMode::FitWidth => "Width",
            ComicFitMode::FitHeight => "Height",
        }
    }

    pub(super) fn next(self) -> Self {
        match self {
            ComicFitMode::FitPage => ComicFitMode::FitWidth,
            ComicFitMode::FitWidth => ComicFitMode::FitHeight,
            ComicFitMode::FitHeight => ComicFitMode::FitPage,
        }
    }

    pub(super) fn to_ini_str(self) -> &'static str {
        match self {
            ComicFitMode::FitPage => "page",
            ComicFitMode::FitWidth => "width",
            ComicFitMode::FitHeight => "height",
        }
    }

    pub(super) fn from_ini_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "width" => ComicFitMode::FitWidth,
            "height" => ComicFitMode::FitHeight,
            _ => ComicFitMode::FitPage,
        }
    }
}

/// Comic Book (.cbz) layout mode: Single Page, 2-Page Spread, or Vertical Strip (Webtoon).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub(super) enum ComicLayoutMode {
    #[default]
    SinglePage,
    TwoPageSpread,
    VerticalStrip,
}

#[allow(dead_code)]
impl ComicLayoutMode {
    pub(super) fn label(self) -> &'static str {
        match self {
            ComicLayoutMode::SinglePage => "Single Page",
            ComicLayoutMode::TwoPageSpread => "2-Page Spread",
            ComicLayoutMode::VerticalStrip => "Vertical Strip",
        }
    }

    pub(super) fn short_label(self) -> &'static str {
        match self {
            ComicLayoutMode::SinglePage => "Single",
            ComicLayoutMode::TwoPageSpread => "2-Page",
            ComicLayoutMode::VerticalStrip => "Strip",
        }
    }

    pub(super) fn next(self) -> Self {
        match self {
            ComicLayoutMode::SinglePage => ComicLayoutMode::TwoPageSpread,
            ComicLayoutMode::TwoPageSpread => ComicLayoutMode::VerticalStrip,
            ComicLayoutMode::VerticalStrip => ComicLayoutMode::SinglePage,
        }
    }

    pub(super) fn to_ini_str(self) -> &'static str {
        match self {
            ComicLayoutMode::SinglePage => "single",
            ComicLayoutMode::TwoPageSpread => "spread",
            ComicLayoutMode::VerticalStrip => "strip",
        }
    }

    pub(super) fn from_ini_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "spread" | "twopage" | "2page" => ComicLayoutMode::TwoPageSpread,
            "strip" | "vertical" | "webtoon" => ComicLayoutMode::VerticalStrip,
            _ => ComicLayoutMode::SinglePage,
        }
    }
}

/// Persisted Comic Book (.cbz) reader preferences.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ComicPrefs {
    pub(super) rtl_manga: bool,
    pub(super) layout_mode: ComicLayoutMode,
    pub(super) cover_first: bool,
    pub(super) fit_mode: ComicFitMode,
}

impl Default for ComicPrefs {
    fn default() -> Self {
        Self {
            rtl_manga: false,
            layout_mode: ComicLayoutMode::SinglePage,
            cover_first: true,
            fit_mode: ComicFitMode::FitPage,
        }
    }
}

/// Bounded LRU cached decoded comic page or thumbnail bitmap.
pub(super) struct ComicCachedEntry {
    pub(super) bitmap: crate::comic::ComicPageBitmap,
    pub(super) last_tick: u64,
}

/// Saved tab state persisted in `micropdf.ini` on exit.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SavedSessionTab {
    pub(super) path: PathBuf,
    pub(super) page_index: u16,
    pub(super) view_mode: ViewMode,
}

/// Remembered reading position `(page_index, zoom_level)` for a file in the 50-entry LRU table.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct LastPositionEntry {
    pub(super) path: PathBuf,
    pub(super) page_index: u16,
    pub(super) zoom_level: f32,
}

/// Parsed contents of `micropdf.ini` (open tabs, active tab, recent files, UI preferences & saved profiles).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SessionConfig {
    pub(super) active_tab: usize,
    pub(super) tabs: Vec<SavedSessionTab>,
    pub(super) recent_files: Vec<PathBuf>,
    pub(super) last_positions: Vec<LastPositionEntry>,
    pub(super) ui_theme: UiThemeMode,
    pub(super) ui_skin: UiSkin,
    pub(super) ui_scale: f32,
    pub(super) scroll_mode: ScrollMode,
    pub(super) page_turn_anim_ms: u64,
    pub(super) reading_filter: ReadingFilter,
    pub(super) epub_prefs: EpubReaderPrefs,
    pub(super) comic_prefs: ComicPrefs,
    pub(super) md_code_font_px: f32,
    pub(super) md_preview_font_px: f32,
    pub(super) md_line_spacing: f32,
    pub(super) md_word_spacing_px: f32,
    pub(super) md_para_spacing_mult: f32,
    pub(super) show_toolbar: bool,
    pub(super) toolbar_config: ToolbarConfig,
    pub(super) hf_profiles: Vec<HeaderFooterProfile>,
    pub(super) encrypt_config: bool,
    pub(super) favorites: crate::favorites::FavoritesTree,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            active_tab: 0,
            tabs: Vec::new(),
            recent_files: Vec::new(),
            last_positions: Vec::new(),
            ui_theme: UiThemeMode::Light,
            ui_skin: UiSkin::Classic,
            ui_scale: 1.0,
            scroll_mode: ScrollMode::SinglePage,
            page_turn_anim_ms: 450,
            reading_filter: ReadingFilter::Normal,
            epub_prefs: EpubReaderPrefs::default(),
            comic_prefs: ComicPrefs::default(),
            md_code_font_px: 13.5,
            md_preview_font_px: 14.5,
            md_line_spacing: 1.56,
            md_word_spacing_px: 0.0,
            md_para_spacing_mult: 1.0,
            show_toolbar: true,
            toolbar_config: ToolbarConfig::default(),
            hf_profiles: Vec::new(),
            encrypt_config: false,
            favorites: crate::favorites::FavoritesTree::default(),
        }
    }
}

/// Holds a single visible page's rasterized 0x00RRGGBB pixels.
pub(super) struct CachedPageBitmap {
    pub(super) page_index: u16,
    pub(super) view_mode: ViewMode,
    pub(super) zoom_bits: u32,
    pub(super) viewport_width: u32,
    pub(super) viewport_height: u32,
    pub(super) bitmap_width: u32,
    pub(super) bitmap_height: u32,
    pub(super) content_min_x: f32,
    pub(super) content_max_x: f32,
    pub(super) content_min_y: f32,
    pub(super) content_max_y: f32,
    pub(super) rotation_deg: i32,
    pub(super) pixels: Vec<u32>,
}

/// Lightweight 140x198 thumbnail bitmap entry, allocated ONLY while Grid View is active
/// and freed immediately upon returning to Page View or switching tabs.
pub(super) struct ThumbnailEntry {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) pixels: Vec<u32>,
}

/// Key identifying a single 512x512 rendered page tile when zoomed above 2x screen size.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct TileKey {
    pub(super) page_index: u16,
    pub(super) virtual_w: u32,
    pub(super) virtual_h: u32,
    pub(super) rotation_deg: i32,
    pub(super) tile_col: u16,
    pub(super) tile_row: u16,
}

/// Cached 512x512 tile bitmap with LRU access tick (`tile_cache` max 16 MB).
pub(super) struct TileEntry {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) last_used: u64,
    pub(super) pixels: Vec<u32>,
}

/// Resolved target of a PDF link annotation under the cursor.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum ResolvedLinkTarget {
    InternalPage(u16),
    ExternalUrl(String),
}

/// Cached PDFium character box for the current page only (freed immediately on page change).
#[derive(Copy, Clone, Debug)]
pub(super) struct PageCharBox {
    pub(super) ch: char,
    pub(super) rect: Option<PdfRect>,
}

/// Metadata-aware Undo / Redo snapshot.
/// When `pdf_bytes` is `None`, the edit only mutated metadata (e.g. bookmarks), costing 0 bytes of PDF RAM.
#[derive(Clone, Debug)]
pub(super) struct UndoSnapshot {
    pub(super) pdf_bytes: Option<Vec<u8>>,
    pub(super) bookmarks: Vec<Bookmark>,
    pub(super) bw_pages: HashSet<u16>,
    pub(super) roman_end: usize,
    pub(super) skip_pages: usize,
    pub(super) redacted: bool,
    pub(super) generation: u64,
}

impl UndoSnapshot {
    pub(super) fn byte_len(&self) -> usize {
        self.pdf_bytes.as_ref().map_or(0, Vec::len)
    }
}

/// Zero-RAM inactive document tab slot (never stores cached bitmaps or thumbnails).
pub(super) struct DocumentTab<'a> {
    pub(super) doc: Option<PdfDocument<'a>>,
    pub(super) file_path: PathBuf,
    pub(super) current_page_index: u16,
    pub(super) view_mode: ViewMode,
    pub(super) last_page_view_mode: ViewMode,
    pub(super) scroll_mode: ScrollMode,
    pub(super) zoom_level: f32,
    pub(super) pan_x: f32,
    pub(super) scroll_y: f32,
    pub(super) scroll_x: f32,
    pub(super) nav_back_stack: VecDeque<(u16, f32)>,
    pub(super) nav_forward_stack: Vec<(u16, f32)>,
    pub(super) roman_end: usize,
    pub(super) skip_pages: usize,
    pub(super) bookmarks: Vec<Bookmark>,
    pub(super) selected_bookmark: Option<usize>,
    pub(super) bookmark_scroll_y: usize,
    pub(super) grid_scroll_y: f32,
    pub(super) bw_pages: HashSet<u16>,
    pub(super) undo_stack: VecDeque<UndoSnapshot>,
    pub(super) redo_stack: VecDeque<UndoSnapshot>,
    pub(super) undo_generation: u64,
    pub(super) saved_undo_generation: u64,
    /// True once any redaction has been applied; forces the strict save path.
    pub(super) redacted: bool,
    /// True once any unsaved edit has been made to this tab since load/save.
    pub(super) has_unsaved_edits: bool,
    /// Last observed disk modification time of `file_path` for 1s auto-reload polling.
    pub(super) last_file_mtime: Option<std::time::SystemTime>,
    /// Parsed Markdown AST when this tab is a `.md` / `.markdown` document (zero bitmaps).
    pub(super) markdown_doc: Option<crate::markdown::MarkdownDoc>,
    /// Raw Markdown/LaTeX source text when editing or inspecting source.
    pub(super) md_source: Option<String>,
    pub(super) md_editor_mode: super::text_editor::MdEditorMode,
    /// Lightweight `.cbz` archive index (zero decoded pixel buffers).
    pub(super) comic_doc: Option<crate::comic::ComicArchive>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum TabBarHit {
    ScrollLeft,
    ScrollRight,
    SwitchTab(usize),
    FavoriteTab(usize),
    CloseTab(usize),
    NewTab,
    TabListToggle,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum TabListHit {
    SwitchTab(usize),
    CloseTab(usize),
    CloseOthers,
    CloseAll,
    InsidePopover,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum TabContextHit {
    CloseThis,
    CloseOthers,
    CloseRight,
    CloseAll,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum ThumbContextHit {
    BlankCountMinus,
    BlankCountPlus,
    InsertBlankBefore,
    InsertBlankAfter,
    InsertPdfBefore,
    InsertPdfAfter,
    RotateCw,
    ConvertBw,
    ExtractSelected,
    DeletePage,
    InsideMenu,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum ThumbBatchHit {
    RotateSelected,
    BwSelected,
    ExtractSelected,
    DeleteSelected,
    ClearSelection,
    InsideBar,
}

#[derive(Copy, Clone, Debug)]
pub(super) struct ThumbContextMenuState {
    pub(super) page_idx: u16,
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) blank_count: u16,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) enum ThumbBadgeHit {
    None,
    Rotate,
    BwConvert,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) enum SidebarHit {
    AddBookmarkButton,
    ImportTxtButton,
    ExportTxtButton,
    AutoDetectButton,
    MoveUpButton,
    MoveDownButton,
    OutdentButton,
    IndentButton,
    BookmarkRow { index: usize, on_delete: bool },
}

/// Unified hovered UI widget identifier so `CursorMoved` only redraws when hover actually changes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum UiHit {
    MenuHeader(MenuCategory),
    MenuItem(usize),
    Toolbar(ToolbarAction),
    BottomBar(BottomBarAction),
    Tab(usize),
    TabFavorite(usize),
    TabClose(usize),
    TabScrollLeft,
    TabScrollRight,
    TabNew,
    TabListToggle,
    TabList(TabListHit),
    TabContext(TabContextHit),
    ThumbContext(ThumbContextHit),
    ThumbBatch(ThumbBatchHit),
    SidebarAdd,
    SidebarImport,
    SidebarExport,
    SidebarAuto,
    SidebarMoveUp,
    SidebarMoveDown,
    SidebarOutdent,
    SidebarIndent,
    SidebarRow(usize),
    SidebarDelete(usize),
    Scrollbar,
    HScrollbar,
    Customize(CustomizeHit),
    PrintPreview(PrintPreviewHit),
    ManualDuplex(ManualDuplexHit),
    Combine(CombineHit),
    HeaderFooter(HeaderFooterHit),
    PageNumber(PageNumberHit),
    CreatePdfImages(CreatePdfImagesHit),
    ExportImages(ExportImagesHit),
    ExcelExport(ExcelExportHit),
    SplitPages(SplitPagesHit),
    Watermark(WatermarkHit),
    StampManager(StampManagerHit),
    DocProps(DocPropsHit),
    UnsavedChanges(UnsavedChangesHit),
    AnnotContext(AnnotContextHit),
    NotePopover(NotePopoverHit),
    Compare(CompareHit),
    Deskew(DeskewHit),
    MeasureBar(MeasureBarHit),
    UserGuide(UserGuideHit),
    ReadingModeOption(ReadingFilter),
    ScrollModeOption(ScrollMode),
    TextView(TextViewHit),
    MarkdownView(super::markdown_view::MarkdownHit),
    MarkdownExport(super::markdown_export_modal::MarkdownExportHit),
    MdEditorBar(super::text_editor::EditorBarAction),
    MdSplitDivider,
    BookSidebar(BookSidebarHit),
    BookInfoPopover(BookInfoPopoverHit),
    AaPopover(AaPopoverHit),
    BookSelectionPill(BookSelectionPillHit),
    EpubBottom(EpubBottomHit),
    ComicBottom(ComicBottomHit),
    ComicSidebar(ComicSidebarHit),
    Overlay(OverlayHit),
    BookmarkFormatSaveSample,
    BookmarkFormatClose,
    NotificationClear,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) enum ComicBottomHit {
    ToggleSidebar,
    PageJump,
    ToggleRtl,
    CycleLayout,
    CycleFit,
    ToggleFilter,
    ToggleFullscreen,
    Scrubber(f32),
}

impl Eq for ComicBottomHit {}

impl ComicBottomHit {
    pub(super) fn tooltip(self) -> &'static str {
        match self {
            Self::ToggleSidebar => "Thumbnails Sidebar (B)",
            Self::PageJump => "Jump to Page (G)",
            Self::ToggleRtl => "Toggle LTR / RTL Manga Reading Direction (R)",
            Self::CycleLayout => "Cycle Layout: Single / 2-Page Spread / Vertical Strip (D)",
            Self::CycleFit => "Cycle Image Fit: Fit Page / Fit Width / Fit Height (F)",
            Self::ToggleFilter => "Cycle Eye-Care Reading Filter (V)",
            Self::ToggleFullscreen => "Toggle Fullscreen (F11)",
            Self::Scrubber(_) => "Drag or Click to Scrub Comic Pages",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum ComicSidebarHit {
    PageThumb(usize),
}

/// One timestamped session notification stored in the bottom-left Notification Board (capped at 50).
#[derive(Clone, Debug)]
pub(super) struct NotificationEntry {
    pub(super) timestamp: String,
    pub(super) message: String,
    pub(super) is_error: bool,
}

/// Active drag-to-move state for repositioning a Sticky Note or Text Box annotation on a page.
#[derive(Clone, Debug)]
pub(super) struct DraggingAnnotation {
    pub(super) page_index: u16,
    pub(super) annot_index: usize,
    pub(super) is_sticky_note: bool,
    pub(super) orig_rect: PdfRect,
    pub(super) start_pdf_x: f32,
    pub(super) start_pdf_y: f32,
    pub(super) start_screen_x: f64,
    pub(super) start_screen_y: f64,
    pub(super) note_text: Option<String>,
}

/// Interactive Command Prompt HUD state machine in the Bottom Command Bar & Window Title Bar.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum InputMode {
    Normal,
    SearchText {
        query_input: String,
    },
    JumpToPage {
        input: String,
    },
    PageTurnSpeed {
        input: String,
    },
    #[allow(dead_code)]
    PageNumbers {
        input: String,
    },
    #[allow(dead_code)]
    HeaderFooterRange {
        range_input: String,
    },
    #[allow(dead_code)]
    HeaderFooterPosition {
        start_page: u16,
        end_page: u16,
        pos_input: String,
    },
    #[allow(dead_code)]
    HeaderFooterText {
        start_page: u16,
        end_page: u16,
        position: MarginPosition,
        text_input: String,
    },
    #[allow(dead_code)]
    ClearMargins {
        input: String,
    },
    #[allow(dead_code)]
    Watermark {
        input: String,
    },
    SplitRange {
        input: String,
    },
    AddBookmark {
        title_input: String,
    },
    EditBookmark {
        bookmark_idx: usize,
        title_input: String,
    },
    EpubNotePrompt {
        block_idx: usize,
        start_word: usize,
        end_word: usize,
        quote: String,
        color_idx: u8,
        text_input: String,
    },
    StickyNotePrompt {
        page_idx: u16,
        pdf_x: f32,
        pdf_y: f32,
        text_input: String,
    },
    FreeTextPrompt {
        page_idx: u16,
        pdf_x: f32,
        pdf_y: f32,
        text_input: String,
    },
    EditAnnotationText {
        page_idx: u16,
        annot_idx: usize,
        label: &'static str,
        text_input: String,
    },
    EditFormField {
        page_idx: u16,
        annot_idx: usize,
        field_name: String,
        value_input: String,
    },
    ConfirmExternalUrl {
        url: String,
    },
    PasswordPrompt {
        path: PathBuf,
        password_input: String,
        attempts_left: u8,
        open_in_new_tab: bool,
    },
    RestoreSessionPrompt {
        tab_count: usize,
    },
    OptimizePrompt {
        preset: crate::optimize::OptimizePreset,
        mode: crate::optimize::ColorMode,
        clean_scans: bool,
        previewing: bool,
    },
    /// Legacy print prompt retained for existing tests; interactive printing uses `print_preview`.
    #[allow(dead_code)]
    PrintPrompt {
        printers: Vec<String>,
        selected_printer: usize,
        copies: u16,
        range_input: String,
        /// Scale each page to the paper (`lp -o fit-to-page`).
        fit_to_page: bool,
    },
}

/// A single-instance connection being read without blocking (see `poll_single_instance_listener`).
pub(super) struct PendingIpcConnection {
    pub(super) stream: std::net::TcpStream,
    pub(super) buf: Vec<u8>,
    pub(super) accepted_at: Instant,
    pub(super) polls: u32,
}

/// Longest accepted request line (`token|absolute path`); longer connections are dropped.
pub(super) const IPC_MAX_LINE_BYTES: usize = 8 * 1024;
/// At most this many connections are kept waiting for their request line at once.
pub(super) const IPC_MAX_PENDING: usize = 8;
/// One frame at 60 Hz: a connection that has not sent its whole request line by the poll after
/// the one that accepted it, and at least this long after being accepted, is dropped.
pub(super) const IPC_FRAME: Duration = Duration::from_millis(16);

/// 16 random bytes from the OS as 32 hex characters: `/dev/urandom` on macOS/Linux,
/// `BCryptGenRandom` (system-preferred RNG) on Windows.
pub(super) fn random_token_hex() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    #[cfg(unix)]
    {
        use std::io::Read;
        std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    }
    #[cfg(windows)]
    {
        const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
        #[link(name = "bcrypt")]
        unsafe extern "system" {
            fn BCryptGenRandom(
                h_algorithm: *mut std::ffi::c_void,
                pb_buffer: *mut u8,
                cb_buffer: u32,
                dw_flags: u32,
            ) -> i32;
        }
        let status = unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                bytes.as_mut_ptr(),
                bytes.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status != 0 {
            return Err(std::io::Error::other(format!(
                "BCryptGenRandom failed (NTSTATUS {status:#x})"
            )));
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        return Err(std::io::Error::other("no OS random source on this platform"));
    }
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Compares two tokens in time independent of where they first differ.
pub(super) fn tokens_match(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A forwarded path is opened only if it is an existing file ending in `.pdf` (any case).
pub(super) fn is_acceptable_forwarded_pdf(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
        && path.is_file()
}

/// A print job in progress, with the name shown in status messages.
pub(super) struct ActivePrintJob<'a> {
    pub(super) job: crate::print::PagedPrintJob<'a>,
    pub(super) title: String,
}

/// An `lp` submission in progress.
pub(super) struct ActiveLpJob {
    pub(super) job: crate::print::PendingLpJob,
    pub(super) printer: String,
}

pub(super) struct ActiveOptimizeJob {
    pub(super) job: crate::optimize::OptimizeJob,
    pub(super) out_path: PathBuf,
    pub(super) out_name: String,
    pub(super) disk_size: usize,
    pub(super) skip_pages: usize,
    pub(super) roman_end: usize,
    pub(super) bookmarks: Vec<Bookmark>,
    pub(super) redacted: bool,
}

#[derive(Clone, Debug)]
pub(super) struct IncrementalSearchState {
    pub(super) query: String,
    #[allow(dead_code)]
    pub(super) q_chars: Vec<char>,
    pub(super) case_sensitive: bool,
    pub(super) whole_word: bool,
    pub(super) next_page: u16,
    pub(super) total_pages: u16,
    pub(super) origin_page: u16,
    pub(super) jumped_to_first: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum PendingDialog {
    Open,
    Save,
    SaveAs,
    Merge,
    CombineMultiple,
    CombineAddFiles,
    PickImagesForPdf,
    PickComparePdf,
    ExportPagesAsImagesDir,
    SplitSinglePagesDir,
    ExportMarkdown,
    ExportLatex,
    ExportDocx,
    ExportXlsx,
    ExportMdPdf,
    ExportMdDocx,
    ExportEpub,
    ExportMdEpub,
    #[allow(dead_code)]
    PickStampImage,
    ImportStampAssetPng,
    Split(String),
    ExtractSelectedPages(Vec<u16>),
    ImportBookmarksTxt,
    ExportBookmarksTxt,
    SaveBookmarkSampleTxt,
    SaveUserGuideTxt,
    AddFavoriteFile,
}

/// On-demand secondary editor window for Markdown / LaTeX tabs (0 bytes when closed).
/// Strict drop order: `surface` drops before `window`.
pub(super) struct EditorPopoutWindow {
    pub(super) surface: Option<Surface<Rc<Window>, Rc<Window>>>,
    pub(super) window: Rc<Window>,
    pub(super) cursor_pos: (f64, f64),
    pub(super) hovered_action: Option<super::text_editor::EditorBarAction>,
    pub(super) hovered_scrollbar: bool,
    pub(super) shift_pressed: bool,
    pub(super) ctrl_pressed: bool,
    pub(super) cmd_or_ctrl_pressed: bool,
    pub(super) alt_pressed: bool,
}

pub struct PdfViewerApp<'a> {
    pub(super) engine: &'a PdfEngine,
    /// Open document tabs; inactive tabs hold zero rendered bitmaps.
    pub(super) tabs: Vec<DocumentTab<'a>>,
    pub(super) active_tab: usize,
    /// First visible tab index when tabs overflow the horizontal tab strip.
    pub(super) tab_scroll_offset: usize,
    /// True when the `▾` All-Tabs overflow dropdown is open.
    pub(super) show_tab_list_popover: bool,
    /// Scroll offset (first visible row) inside the `▾` All-Tabs dropdown.
    pub(super) tab_list_scroll: usize,
    /// Right-click Tab Context Menu: `Some((target_tab_idx, screen_x, screen_y))`.
    pub(super) tab_context_menu: Option<(usize, i32, i32)>,
    /// Active tab state mirrored directly for fast, conflict-free access.
    pub(super) doc: Option<PdfDocument<'a>>,
    pub(super) markdown_doc: Option<crate::markdown::MarkdownDoc>,
    pub(super) md_source: Option<String>,
    pub(super) md_editor: Option<super::text_editor::TextEditorState>,
    pub(super) md_editor_mode: super::text_editor::MdEditorMode,
    pub(super) md_split_ratio: f32,
    pub(super) md_split_drag: bool,
    pub(super) md_split_last_click: Option<Instant>,
    pub(super) pending_open_editor_popout: bool,
    pub(super) editor_popout: Option<EditorPopoutWindow>,
    pub(super) md_doc_gen: u64,
    pub(super) md_layout: Option<super::markdown_view::MdLayoutCache>,
    pub(super) md_image_cache: MdDecodedImageCache,
    pub(super) md_search_matches: Vec<MdSearchMatch>,
    pub(super) md_active_hits: Vec<super::markdown_view::MdHitRect>,
    /// True while floating Find bar has active keyboard focus; false renders at 50% opacity while keeping controls clickable.
    pub(super) search_bar_focused: bool,
    /// Separate Code & Preview font sizes + Line / Word / Paragraph spacing for Markdown & LaTeX.
    pub(super) md_code_font_px: f32,
    pub(super) md_preview_font_px: f32,
    pub(super) md_line_spacing: f32,
    pub(super) md_word_spacing_px: f32,
    pub(super) md_para_spacing_mult: f32,
    /// Phase 2 Distinct EPUB reader preferences & per-book annotations/bookmarks/UI state.
    pub(super) epub_prefs: EpubReaderPrefs,
    pub(super) epub_book_data: EpubBookData,
    pub(super) book_sidebar_tab: BookSidebarTab,
    pub(super) book_toc_collapsed: HashSet<usize>,
    pub(super) book_toc_search: Option<String>,
    pub(super) book_info_popover: bool,
    pub(super) show_aa_popover: bool,
    pub(super) aa_margins_submenu: bool,
    pub(super) epub_margin_slider_drag: Option<u8>,
    pub(super) epub_scrubber_drag: bool,
    pub(super) epub_sidebar_scrollbar_drag: bool,
    pub(super) epub_cover_thumb: Option<(String, u32, u32, Vec<u32>)>,
    pub(super) epub_word_rects: Vec<EpubWordHit>,
    pub(super) epub_selection_drag: Option<((usize, usize), (usize, usize))>,
    pub(super) epub_selection_pill_rect: Option<(i32, i32, u32, u32)>,
    /// Active `.cbz` comic archive and strictly bounded LRU pixel caches.
    pub(super) comic_doc: Option<crate::comic::ComicArchive>,
    pub(super) comic_prefs: ComicPrefs,
    pub(super) comic_page_cache: HashMap<usize, ComicCachedEntry>,
    pub(super) comic_thumb_cache: HashMap<usize, ComicCachedEntry>,
    pub(super) comic_page_dims: HashMap<usize, (u32, u32)>,
    pub(super) comic_lru_tick: u64,
    pub(super) comic_strip_rects: Vec<(usize, (i32, i32, u32, u32))>,
    pub(super) comic_scrubber_drag: bool,
    pub(super) comic_sidebar_scroll: usize,
    pub(super) file_path: PathBuf,
    pub(super) current_page_index: u16,
    pub(super) roman_end: usize,
    /// Number of leading unnumbered cover/picture pages set via Page Numbering.
    pub(super) skip_pages: usize,
    pub(super) bw_pages: HashSet<u16>,
    pub(super) undo_stack: VecDeque<UndoSnapshot>,
    pub(super) redo_stack: VecDeque<UndoSnapshot>,
    pub(super) undo_generation: u64,
    pub(super) saved_undo_generation: u64,
    /// True once any redaction has been applied to the active document.
    pub(super) redacted: bool,
    /// True once any unsaved edit has been made to the active document since load/save.
    pub(super) has_unsaved_edits: bool,
    /// Last observed disk modification time of `self.file_path` for 1s auto-reload polling.
    pub(super) last_file_mtime: Option<std::time::SystemTime>,
    /// True while the MicroPDF window is focused (auto-reload mtime polling runs only while focused).
    pub(super) window_focused: bool,
    /// Timestamp of the last 1s file mtime poll.
    pub(super) last_mtime_poll: Instant,
    /// Set by the first `U` press on a signed document; a second press confirms.
    pub(super) flatten_confirm_at: Option<Instant>,
    pub(super) cached_page: Option<CachedPageBitmap>,
    /// Second page bitmap used ONLY in `ViewMode::TwoPageSpread` (`4` key).
    pub(super) cached_second_page: Option<CachedPageBitmap>,
    /// Visible pages + 1 above/below cache used ONLY in `ViewMode::ContinuousScroll` (`5` key).
    pub(super) continuous_page_cache: HashMap<u16, CachedPageBitmap>,
    /// Screen draw rectangles `(page_index, (draw_x, draw_y, bmp_w, bmp_h))` for visible pages in `ContinuousScroll`.
    pub(super) continuous_draw_rects: Vec<(u16, (i32, i32, u32, u32))>,
    /// Signed screen draw rectangle `(draw_x, draw_y, bmp_w, bmp_h)` for the primary page.
    pub(super) page_draw_rect: (i32, i32, u32, u32),
    /// Signed screen draw rectangle `(draw_x, draw_y, bmp_w, bmp_h)` for the right page in `TwoPageSpread`.
    pub(super) second_page_draw_rect: Option<(i32, i32, u32, u32)>,
    /// Current mouse cursor coordinates in window physical pixels.
    pub(super) cursor_pos: (f64, f64),
    /// Active left-click drag state in Page View: `Some((start_x, start_y))`.
    pub(super) drag_start: Option<(f64, f64)>,
    /// Active View Mode (`FitPage`, `FitWidth`, `FitTextWidth`, `TwoPageSpread`, `ContinuousScroll`, `ThumbnailGrid`).
    pub(super) view_mode: ViewMode,
    pub(super) last_page_view_mode: ViewMode,
    /// Independent scroll flow (`SinglePage`, `ContinuousVertical`, `ContinuousHorizontal`).
    pub(super) scroll_mode: ScrollMode,
    /// Zoom factor (`0.25..=8.0`, i.e. 25%–800%, default `1.0`).
    pub(super) zoom_level: f32,
    /// Horizontal pan offset in physical pixels when the zoomed page width exceeds the viewport width.
    pub(super) pan_x: f32,
    /// Vertical scroll offset in physical pixels when zoomed or in vertical continuous scroll.
    pub(super) scroll_y: f32,
    /// Horizontal scroll offset in physical pixels when in `ScrollMode::ContinuousHorizontal` (book ribbon).
    pub(super) scroll_x: f32,
    /// LRU 512x512 tile cache used when a page's zoomed size exceeds 2x the viewport size (capped at 16 MB).
    pub(super) tile_cache: HashMap<TileKey, TileEntry>,
    pub(super) tile_lru_tick: u64,
    /// Navigation history of `(page_index, scroll_y)` entries (capped at `MAX_NAV_HISTORY = 50`).
    pub(super) nav_back_stack: VecDeque<(u16, f32)>,
    pub(super) nav_forward_stack: Vec<(u16, f32)>,
    /// Currently hovered link target on the page (triggers `CursorIcon::Pointer` hand cursor).
    pub(super) hovered_link: Option<ResolvedLinkTarget>,
    /// Currently hovered fillable form widget `(page_idx, pdf_bounds)` in `MouseMode::Select`.
    pub(super) hovered_form_field: Option<(u16, PdfRect)>,
    /// Cached character boxes `(page_index, chars)` for the current page only (freed on page change).
    pub(super) cached_char_boxes: Option<(u16, Vec<PageCharBox>)>,
    /// Currently selected text string and its page/line highlight rectangles in `MouseMode::Select`.
    pub(super) selected_text: String,
    pub(super) selected_text_page: Option<u16>,
    pub(super) selected_text_rects: Vec<PdfRect>,
    /// Tracks the last click `(page_idx, sx, sy, timestamp)` in `MouseMode::Select` to detect double-clicks.
    pub(super) last_select_click: Option<(u16, f64, f64, Instant)>,
    /// Last 10 recently opened/saved PDF file paths (shown in `File -> Recent` and persisted in `micropdf.ini`).
    pub(super) recent_files: Vec<PathBuf>,
    /// Compact 50-entry LRU remembering `(page_index, zoom_level)` per previously read PDF path.
    pub(super) last_positions: Vec<LastPositionEntry>,
    /// Saved session tabs loaded from `micropdf.ini` when starting with no file argument.
    pub(super) pending_restore_tabs: Vec<SavedSessionTab>,
    pub(super) pending_restore_active_tab: usize,
    /// Optional custom path override for `micropdf.ini` (used by unit tests).
    pub(super) custom_session_ini_path: Option<PathBuf>,
    /// When true, `micropdf.ini` is written using `config_crypto` authenticated encryption.
    pub(super) encrypt_config: bool,
    /// User's hierarchical Favorites Library (persisted in `micropdf.ini`).
    pub(super) favorites: crate::favorites::FavoritesTree,
    /// Open Favorites Library modal (0 KB RAM when closed).
    pub(super) favorites_modal: Option<super::favorites_modal::FavoritesModal>,
    /// Local loopback listener `(TcpListener, token, lock_file_path)` for single-instance tab forwarding.
    pub(super) single_instance_listener: Option<(std::net::TcpListener, String, PathBuf)>,
    /// Accepted single-instance connections whose request line has not fully arrived yet.
    pub(super) pending_ipc: Vec<PendingIpcConnection>,
    /// Verified forwarded PDF paths waiting until any blocking modal closes before switching tabs.
    pub(super) queued_ipc_paths: VecDeque<PathBuf>,
    /// Flattened one-page copy of the form page being tiled (see `ensure_tile_rendered`).
    pub(super) tile_form_copy: Option<(u16, PdfDocument<'a>)>,
    /// Paged print job sent one page per frame (Windows GDI; a fake printer in tests).
    pub(super) print_job: Option<ActivePrintJob<'a>>,
    /// `lp` submission running in the background (macOS/Linux).
    pub(super) lp_job: Option<ActiveLpJob>,
    /// The `lp` program to run; replaced by a fake in tests.
    pub(super) lp_program: std::ffi::OsString,
    /// Renames a finished save's temp file over its target. `std::fs::rename` except in tests,
    /// which substitute file-system behaviour (e.g. refusing to replace a file that is open).
    pub(super) rename_for_save: fn(&std::path::Path, &std::path::Path) -> std::io::Result<()>,
    /// Top Menu Bar open dropdown state (`[ File ] [ View ] [ Edit ] [ Page ] [ Tools ] [ Help ]`).
    pub(super) open_menu: Option<MenuCategory>,
    /// Active Mouse Mode (`Select`, `Highlight`, `Measure`, `Redact`, `Crop`, `StampImage`).
    pub(super) mouse_mode: MouseMode,
    /// Active unit & last completed measurement for `MouseMode::Measure`.
    pub(super) measure_unit: MeasureUnit,
    pub(super) last_measurement: Option<MeasurementResult>,
    /// Decoded RGBA image waiting to be stamped on the next page click/drag (`P` key).
    pub(super) pending_stamp_image: Option<StampImageBuffer>,
    /// Zero-RAM CPU-rendered Reading Filter (6 eye-care palettes).
    pub(super) reading_filter: ReadingFilter,
    /// True when the bottom-bar Reading Mode picker popover is open.
    pub(super) show_reading_mode_popover: bool,
    /// True when the bottom-bar Scroll Mode picker popover is open.
    pub(super) show_scroll_mode_popover: bool,
    /// Active text search state (`Ctrl+F` / `Cmd+F`, `F3` / `Shift+F3` / `Ctrl+G`).
    pub(super) search_query: String,
    pub(super) search_case_sensitive: bool,
    pub(super) search_whole_word: bool,
    pub(super) search_matches: Vec<SearchMatch>,
    pub(super) current_search_match: usize,
    /// Time-sliced (~8 ms/frame) incremental search state on the main thread (`Esc` cancels).
    pub(super) incremental_search: Option<IncrementalSearchState>,
    /// Active Side-by-Side Compare Mode state (0 bytes when closed).
    pub(super) compare_mode: Option<CompareModeState<'a>>,
    /// Active Page Deskew control bar state (0 bytes when closed).
    pub(super) deskew_bar: Option<DeskewBarState>,
    /// Incremental PDF optimization job driven one `step()` per event-loop iteration (`Esc` cancels).
    pub(super) optimize_job: Option<ActiveOptimizeJob>,
    /// Lightweight thumbnail cache populated ONLY when `view_mode == ViewMode::ThumbnailGrid`.
    pub(super) thumbnail_cache: HashMap<u16, ThumbnailEntry>,
    /// Vertical scroll offset in physical pixels for Thumbnail Grid View.
    pub(super) grid_scroll_y: f32,
    /// Currently hovered thumbnail index in Grid View.
    pub(super) hovered_thumb: Option<u16>,
    /// Active drag-and-drop page reorder state in Grid View: `Some((source_page_idx, start_x, start_y))`
    pub(super) grid_drag_page: Option<(u16, f64, f64)>,
    /// Last double-click tracking in Grid View: `(page_idx, timestamp)`.
    pub(super) last_thumb_click: Option<(u16, Instant)>,
    /// Multi-selected thumbnail page indices (`Ctrl+Click` / `Shift+Click`) for batch operations.
    pub(super) selected_thumbs: HashSet<u16>,
    /// Anchor page index for `Shift+Click` range selection in Thumbnail Grid.
    pub(super) last_thumb_anchor: Option<u16>,
    /// Right-click context menu on a thumbnail card in Grid View.
    pub(super) thumb_context_menu: Option<ThumbContextMenuState>,
    /// Left Bookmark Sidebar (`B` to toggle).
    pub(super) show_bookmarks: bool,
    pub(super) bookmarks: Vec<Bookmark>,
    pub(super) selected_bookmark: Option<usize>,
    pub(super) bookmark_scroll_y: usize,
    /// Debounce timer & accumulator for MouseWheel page turning.
    pub(super) last_wheel_turn: Option<Instant>,
    pub(super) wheel_accum_y: f64,
    /// Active page-turn animation for Magazine / Book mode.
    pub(super) page_turn_anim: Option<PageTurnAnim>,
    /// User-configurable Magazine / Book page-turn animation duration in ms (`0..=3000`, default `450`).
    pub(super) page_turn_anim_ms: u64,
    pub(super) shift_pressed: bool,
    pub(super) cmd_or_ctrl_pressed: bool,
    pub(super) ctrl_pressed: bool,
    pub(super) alt_pressed: bool,
    pub(super) input_mode: InputMode,
    pub(super) pending_dialog: Option<PendingDialog>,
    pub(super) show_help_overlay: bool,
    pub(super) show_bookmark_format_overlay: bool,
    pub(super) show_about_overlay: bool,
    pub(super) show_customize_toolbar: bool,
    /// Bright Light default UI theme with Dark Mode toggle.
    pub(super) ui_theme_mode: UiThemeMode,
    /// Visual skin mode (`Classic` default vs `Distinct`).
    pub(super) ui_skin: UiSkin,
    /// User-adjustable UI interface scale multiplier (`0.80..=1.60`, persisted in `micropdf.ini`).
    pub(super) ui_scale: f32,
    /// Inline icon toolbar beside the menus on the unified top bar (`View -> Show Toolbar`).
    pub(super) show_toolbar: bool,
    /// Per-button customization persisted in `micropdf.ini`.
    pub(super) toolbar_config: ToolbarConfig,
    /// Saved reusable Header/Footer style profiles persisted in `micropdf.ini`.
    pub(super) hf_profiles: Vec<HeaderFooterProfile>,
    /// Most recently edited Header/Footer settings so reopening the dialog lets you tweak & update cleanly.
    pub(super) last_hf_profile: HeaderFooterProfile,
    /// Active annotation color swatch (Highlight, Underline, Strikethrough, Note, Pen, TextBox, Shapes).
    pub(super) annotation_color: AnnotationColor,
    /// Live freehand pen stroke points in window physical pixels while dragging `MouseMode::InkPen`.
    pub(super) ink_stroke_screen_pts: Vec<(f32, f32)>,
    /// Floating multi-line sticky-note reader popover with Edit & Delete controls.
    pub(super) active_note_popover: Option<ActiveNotePopover>,
    /// Floating right-click annotation inspector menu (Edit Text / Color / Delete).
    pub(super) annot_context_menu: Option<AnnotationContextMenuState>,
    /// Active drag-to-move state when repositioning an existing Sticky Note or Text Box.
    pub(super) dragging_annotation: Option<DraggingAnnotation>,
    /// Active Print Preview modal state (0 bytes when closed).
    pub(super) print_preview: Option<PrintPreviewState>,
    /// Active Manual Duplex Pass 2 wait modal state (0 bytes when closed).
    pub(super) manual_duplex_wait: Option<ManualDuplexWaitState>,
    /// Active Interactive Combine PDFs modal state (0 bytes when closed).
    pub(super) combine_modal: Option<CombineModalState>,
    /// Active visual Header & Footer modal state (0 bytes when closed).
    pub(super) header_footer_modal: Option<HeaderFooterModalState>,
    /// Active visual Page Numbering modal state (0 bytes when closed).
    pub(super) page_number_modal: Option<PageNumberModalState>,
    /// Active `Create PDF from Images` modal state (0 bytes when closed).
    pub(super) create_pdf_images_modal: Option<CreatePdfImagesModalState>,
    /// Active `Export Pages as Images` modal state (0 bytes when closed).
    pub(super) export_images_modal: Option<ExportImagesModalState>,
    /// Active `Export PDF to Excel (.xlsx)` options modal state (0 bytes when closed).
    pub(super) excel_export_modal: Option<ExcelExportModalState>,
    pub(super) markdown_export_modal: Option<super::markdown_export_modal::MarkdownExportModalState>,
    /// Remembers last chosen Hybrid Lattice/Stream Excel export options for the session.
    pub(super) excel_export_opts: crate::export::ExcelExportOptions,
    /// Active `Split to Single-Page PDFs` modal state (0 bytes when closed).
    pub(super) split_pages_modal: Option<SplitPagesModalState>,
    /// Active Detailed Watermark modal with live preview (0 bytes when closed).
    pub(super) watermark_modal: Option<WatermarkModalState>,
    /// Active Portable Stamp & Signature Manager modal (0 bytes when closed).
    pub(super) stamp_manager_modal: Option<StampManagerModalState>,
    /// Active `Document Properties & Privacy` modal state (0 bytes when closed).
    pub(super) doc_props_modal: Option<DocPropsModalState>,
    /// Active `Unsaved Changes` confirmation dialog (0 bytes when closed).
    pub(super) unsaved_changes_modal: Option<UnsavedChangesModalState>,
    /// Active built-in Step-by-Step User Guide modal (`Help -> User Guide (How to Use)`).
    pub(super) user_guide_modal: Option<UserGuideState>,
    /// Set true once exit has been confirmed through the unsaved-changes guard.
    pub(super) pending_app_exit: bool,
    /// Cached extracted blocks for `ViewMode::TextView` (0 bytes when not in Text View).
    pub(super) cached_text_view: Option<CachedTextViewPage>,
    /// Reading font size in logical pixels for `ViewMode::TextView`.
    pub(super) text_view_font_size: f32,
    /// Last computed vertical max scroll in pixels for `ViewMode::TextView` (used by scrollbar & wheel).
    pub(super) text_view_max_scroll: f32,
    /// Unified hovered UI widget to coalesce redraws and show crisp hover feedback.
    pub(super) hovered_widget: Option<UiHit>,
    /// True while dragging the right-edge overlay vertical scrollbar.
    pub(super) scrollbar_drag: bool,
    /// True while dragging the bottom-edge overlay horizontal scrollbar.
    pub(super) hscrollbar_drag: bool,
    /// Active drag anchor `(x, y)` while panning the page with `MouseMode::Hand`.
    pub(super) hand_pan_drag_last: Option<(f64, f64)>,
    /// True when `F11` fullscreen (or `F5` presentation) is active with menu/tab/status bars hidden.
    pub(super) is_fullscreen: bool,
    /// True when `F5` presentation mode is active (fullscreen, one page fit to screen, black background).
    pub(super) is_presentation: bool,
    /// Saved `(view_mode, scroll_mode, zoom_level, pan_x, scroll_y)` restored when exiting `F5` presentation mode.
    pub(super) pre_presentation_state: Option<(ViewMode, ScrollMode, f32, f32, f32)>,
    pub(super) status_notice: Option<String>,
    /// Bounded session log of timestamped status notifications (max 50 entries).
    pub(super) notifications: VecDeque<NotificationEntry>,
    /// Last notice message recorded into `notifications` so duplicates are not double-logged.
    pub(super) last_recorded_notice: Option<String>,
    /// True when the bottom-left Session Notification Board popover is open.
    pub(super) show_notifications: bool,
    /// Reusable pixel buffer so page turns never reallocate when `cached_page` is invalidated.
    pub(super) spare_pixel_buf: Vec<u32>,
    /// Declared in strict drop order: `surface` drops first, then `context`, then `window`.
    pub(super) surface: Option<Surface<Rc<Window>, Rc<Window>>>,
    pub(super) context: Option<SoftContext<Rc<Window>>>,
    pub(super) window: Option<Rc<Window>>,
}


impl<'a> PdfViewerApp<'a> {
    pub(super) fn ui_theme(&self) -> UiTheme {
        self.ui_theme_mode
            .theme_for(self.ui_skin, self.active_doc_kind())
    }

    pub(super) fn active_doc_kind(&self) -> DocKind {
        let by_path = DocKind::from_path(&self.file_path);
        if self.comic_doc.is_some() && by_path == DocKind::Pdf {
            DocKind::Comic
        } else if self.markdown_doc.is_some() && by_path == DocKind::Pdf {
            DocKind::Markdown
        } else {
            by_path
        }
    }

    pub(super) fn tab_doc_kind(&self, tab_idx: usize) -> DocKind {
        if tab_idx == self.active_tab {
            self.active_doc_kind()
        } else if let Some(slot) = self.tabs.get(tab_idx) {
            let by_path = DocKind::from_path(&slot.file_path);
            if slot.comic_doc.is_some() && by_path == DocKind::Pdf {
                DocKind::Comic
            } else if slot.markdown_doc.is_some() && by_path == DocKind::Pdf {
                DocKind::Markdown
            } else {
                by_path
            }
        } else {
            DocKind::Pdf
        }
    }

    /// Effective UI scale combining OS display HiDPI `window_sf` with user's `ui_scale` preference.
    pub(super) fn effective_scale_factor(&self, window_sf: f64) -> f64 {
        (window_sf * (self.ui_scale as f64)).clamp(0.75, 4.0)
    }

    pub(super) fn adjust_ui_scale(&mut self, delta: f32) {
        let next = ((self.ui_scale + delta) * 20.0).round() / 20.0;
        self.ui_scale = next.clamp(MIN_UI_SCALE, MAX_UI_SCALE);
        self.status_notice = Some(format!("UI Scale: {}%", (self.ui_scale * 100.0).round() as i32));
        self.save_session_to_ini();
        self.update_title_bar();
        self.request_redraw();
    }

    pub(super) fn reset_ui_scale(&mut self) {
        self.ui_scale = 1.0;
        self.status_notice = Some("UI Scale reset to 100%".to_string());
        self.save_session_to_ini();
        self.update_title_bar();
        self.request_redraw();
    }

    /// Effective scroll mode (supports legacy `ViewMode::ContinuousScroll` seamlessly).
    pub(super) fn effective_scroll_mode(&self) -> ScrollMode {
        if self.view_mode == ViewMode::ContinuousScroll {
            ScrollMode::ContinuousVertical
        } else {
            self.scroll_mode
        }
    }

    /// Effective page layout/fit mode (`FitPage`, `FitWidth`, `FitTextWidth`, `TwoPageSpread`).
    pub(super) fn effective_layout_mode(&self) -> ViewMode {
        if self.view_mode == ViewMode::ContinuousScroll {
            ViewMode::FitWidth
        } else {
            self.view_mode
        }
    }

    /// True when rendering continuous vertical or continuous horizontal scrolling page flow.
    pub(super) fn is_continuous_scroll(&self) -> bool {
        self.view_mode.is_page_view()
            && matches!(
                self.effective_scroll_mode(),
                ScrollMode::ContinuousVertical | ScrollMode::ContinuousHorizontal
            )
    }

    pub(super) fn set_scroll_mode(&mut self, mode: ScrollMode) {
        self.show_scroll_mode_popover = false;
        if self.view_mode == ViewMode::ContinuousScroll {
            self.view_mode = ViewMode::FitWidth;
            self.last_page_view_mode = ViewMode::FitWidth;
        }
        if !self.view_mode.is_page_view() {
            self.view_mode = self.last_page_view_mode;
            self.thumbnail_cache.clear();
            self.cached_text_view = None;
        }
        self.scroll_mode = mode;
        self.scroll_y = 0.0;
        self.scroll_x = 0.0;
        self.pan_x = 0.0;
        self.page_turn_anim = None;
        self.invalidate_page_caches();
        self.status_notice = Some(format!("Scroll Mode: {}", mode.status_label()));
        self.save_session_to_ini();
        self.request_redraw();
    }

    pub(super) fn set_page_turn_anim_ms(&mut self, ms: u64) {
        let clamped = ms.min(3000);
        self.page_turn_anim_ms = clamped;
        self.page_turn_anim = None;
        self.status_notice = Some(if clamped == 0 {
            "Magazine Page-Turn Animation: Off (0 ms)".to_string()
        } else {
            format!("Magazine Page-Turn Speed: {clamped} ms")
        });
        self.save_session_to_ini();
        self.request_redraw();
    }

    pub(super) fn trigger_page_turn_anim(&mut self, direction: i8) {
        if self.effective_scroll_mode() == ScrollMode::MagazineBook && self.page_turn_anim_ms > 0 {
            self.page_turn_anim = Some(PageTurnAnim {
                started_at: Instant::now(),
                duration_ms: self.page_turn_anim_ms,
                direction,
                from_scroll_y: None,
                prev_spread: None,
            });
        }
    }

    pub(super) fn trigger_epub_page_curl(&mut self, direction: i8, from_scroll_y: f32) {
        if !self.epub_prefs.page_curl_anim {
            return;
        }
        let dur = if self.page_turn_anim_ms == 0 {
            420
        } else {
            self.page_turn_anim_ms.clamp(220, 1200)
        };
        self.page_turn_anim = Some(PageTurnAnim {
            started_at: Instant::now(),
            duration_ms: dur,
            direction,
            from_scroll_y: Some(from_scroll_y),
            prev_spread: None,
        });
    }

    /// Returns `Some((eased_t, direction))` while the page-turn animation is active,
    /// or `None` once finished.
    pub(super) fn page_turn_anim_progress(&mut self) -> Option<(f32, i8)> {
        let anim = self.page_turn_anim.as_ref()?;
        let elapsed_ms = anim.started_at.elapsed().as_secs_f32() * 1000.0;
        let t = (elapsed_ms / anim.duration_ms.max(1) as f32).clamp(0.0, 1.0);
        if t >= 1.0 {
            self.page_turn_anim = None;
            None
        } else {
            let eased = 1.0 - (1.0 - t).powi(3);
            Some((eased, anim.direction))
        }
    }

    pub(super) fn active_search_match_count(&self) -> usize {
        if self.is_markdown_tab() {
            self.md_search_matches.len()
        } else {
            self.search_matches.len()
        }
    }

    pub(super) fn set_reading_filter(&mut self, filter: ReadingFilter) {
        self.reading_filter = filter;
        self.show_reading_mode_popover = false;
        if let Some(ref mut cmp) = self.compare_mode {
            cmp.invalidate_caches();
        }
        self.status_notice = Some(format!("Reading Mode: {}", filter.picker_title()));
        self.save_session_to_ini();
        self.request_redraw();
    }

    pub fn new(engine: &'a PdfEngine, initial_path: Option<PathBuf>) -> Result<Self> {
        // Print temp files left behind by a previous run that was killed mid-print.
        crate::print::remove_stale_print_temp_files(&std::env::temp_dir());
        let mut app = Self {
            engine,
            tabs: Vec::new(),
            active_tab: 0,
            tab_scroll_offset: 0,
            show_tab_list_popover: false,
            tab_list_scroll: 0,
            tab_context_menu: None,
            doc: None,
            markdown_doc: None,
            md_source: None,
            md_editor: None,
            md_editor_mode: super::text_editor::MdEditorMode::Preview,
            md_split_ratio: 0.50,
            md_split_drag: false,
            md_split_last_click: None,
            pending_open_editor_popout: false,
            editor_popout: None,
            md_doc_gen: 0,
            md_layout: None,
            md_image_cache: MdDecodedImageCache::default(),
            md_search_matches: Vec::new(),
            md_active_hits: Vec::new(),
            search_bar_focused: false,
            md_code_font_px: 13.5,
            md_preview_font_px: 14.5,
            md_line_spacing: 1.56,
            md_word_spacing_px: 0.0,
            md_para_spacing_mult: 1.0,
            epub_prefs: EpubReaderPrefs::default(),
            epub_book_data: EpubBookData::default(),
            book_sidebar_tab: BookSidebarTab::Contents,
            book_toc_collapsed: HashSet::new(),
            book_toc_search: None,
            book_info_popover: false,
            show_aa_popover: false,
            aa_margins_submenu: false,
            epub_margin_slider_drag: None,
            epub_scrubber_drag: false,
            epub_sidebar_scrollbar_drag: false,
            epub_cover_thumb: None,
            epub_word_rects: Vec::new(),
            epub_selection_drag: None,
            epub_selection_pill_rect: None,
            comic_doc: None,
            comic_prefs: ComicPrefs::default(),
            comic_page_cache: HashMap::new(),
            comic_thumb_cache: HashMap::new(),
            comic_page_dims: HashMap::new(),
            comic_lru_tick: 0,
            comic_strip_rects: Vec::new(),
            comic_scrubber_drag: false,
            comic_sidebar_scroll: 0,
            file_path: initial_path.clone().unwrap_or_default(),
            current_page_index: 0,
            roman_end: 0,
            skip_pages: 0,
            bw_pages: HashSet::new(),
            undo_stack: VecDeque::with_capacity(MAX_UNDO_STATES),
            redo_stack: VecDeque::with_capacity(MAX_UNDO_STATES),
            undo_generation: 0,
            saved_undo_generation: 0,
            redacted: false,
            has_unsaved_edits: false,
            last_file_mtime: None,
            window_focused: true,
            last_mtime_poll: Instant::now(),
            flatten_confirm_at: None,
            cached_page: None,
            cached_second_page: None,
            continuous_page_cache: HashMap::new(),
            continuous_draw_rects: Vec::new(),
            page_draw_rect: (0, 0, 0, 0),
            second_page_draw_rect: None,
            cursor_pos: (0.0, 0.0),
            drag_start: None,
            view_mode: ViewMode::FitPage,
            last_page_view_mode: ViewMode::FitPage,
            scroll_mode: ScrollMode::SinglePage,
            page_turn_anim: None,
            page_turn_anim_ms: 450,
            zoom_level: 1.0,
            pan_x: 0.0,
            scroll_y: 0.0,
            scroll_x: 0.0,
            tile_cache: HashMap::new(),
            tile_lru_tick: 0,
            nav_back_stack: VecDeque::new(),
            nav_forward_stack: Vec::new(),
            hovered_link: None,
            hovered_form_field: None,
            cached_char_boxes: None,
            selected_text: String::new(),
            selected_text_page: None,
            selected_text_rects: Vec::new(),
            last_select_click: None,
            recent_files: Vec::new(),
            last_positions: Vec::new(),
            pending_restore_tabs: Vec::new(),
            pending_restore_active_tab: 0,
            custom_session_ini_path: None,
            encrypt_config: false,
            favorites: crate::favorites::FavoritesTree::default(),
            favorites_modal: None,
            single_instance_listener: None,
            pending_ipc: Vec::new(),
            queued_ipc_paths: VecDeque::new(),
            tile_form_copy: None,
            print_job: None,
            lp_job: None,
            lp_program: "lp".into(),
            rename_for_save: |from, to| std::fs::rename(from, to),
            open_menu: None,
            mouse_mode: MouseMode::Select,
            measure_unit: MeasureUnit::Mm,
            last_measurement: None,
            pending_stamp_image: None,
            reading_filter: ReadingFilter::Normal,
            show_reading_mode_popover: false,
            show_scroll_mode_popover: false,
            search_query: String::new(),
            search_case_sensitive: false,
            search_whole_word: false,
            search_matches: Vec::new(),
            current_search_match: 0,
            incremental_search: None,
            compare_mode: None,
            deskew_bar: None,
            optimize_job: None,
            thumbnail_cache: HashMap::new(),
            grid_scroll_y: 0.0,
            hovered_thumb: None,
            grid_drag_page: None,
            last_thumb_click: None,
            selected_thumbs: HashSet::new(),
            last_thumb_anchor: None,
            thumb_context_menu: None,
            show_bookmarks: false,
            bookmarks: Vec::new(),
            selected_bookmark: None,
            bookmark_scroll_y: 0,
            last_wheel_turn: None,
            wheel_accum_y: 0.0,
            shift_pressed: false,
            cmd_or_ctrl_pressed: false,
            ctrl_pressed: false,
            alt_pressed: false,
            input_mode: InputMode::Normal,
            pending_dialog: None,
            show_help_overlay: false,
            show_bookmark_format_overlay: false,
            show_about_overlay: false,
            show_customize_toolbar: false,
            ui_theme_mode: UiThemeMode::Light,
            ui_skin: UiSkin::Classic,
            ui_scale: 1.0,
            show_toolbar: true,
            toolbar_config: ToolbarConfig::default(),
            hf_profiles: Vec::new(),
            last_hf_profile: HeaderFooterProfile::default(),
            annotation_color: AnnotationColor::Yellow,
            ink_stroke_screen_pts: Vec::new(),
            active_note_popover: None,
            annot_context_menu: None,
            dragging_annotation: None,
            print_preview: None,
            manual_duplex_wait: None,
            combine_modal: None,
            header_footer_modal: None,
            page_number_modal: None,
            create_pdf_images_modal: None,
            export_images_modal: None,
            excel_export_modal: None,
            markdown_export_modal: None,
            excel_export_opts: crate::export::ExcelExportOptions::default(),
            split_pages_modal: None,
            watermark_modal: None,
            stamp_manager_modal: None,
            doc_props_modal: None,
            unsaved_changes_modal: None,
            user_guide_modal: None,
            pending_app_exit: false,
            cached_text_view: None,
            text_view_font_size: 14.5,
            text_view_max_scroll: 0.0,
            hovered_widget: None,
            scrollbar_drag: false,
            hscrollbar_drag: false,
            hand_pan_drag_last: None,
            is_fullscreen: false,
            is_presentation: false,
            pre_presentation_state: None,
            status_notice: if initial_path.is_none() {
                Some("Drop a document here or press O to open".to_string())
            } else {
                None
            },
            notifications: VecDeque::with_capacity(50),
            last_recorded_notice: None,
            show_notifications: false,
            spare_pixel_buf: Vec::new(),
            surface: None,
            context: None,
            window: None,
        };
        app.record_status_notice_if_new();
        app.load_session_from_ini(initial_path.is_none());
        if let Some(path) = initial_path {
            app.load_document(path)?;
        }
        Ok(app)
    }

    /// True while any modal dialog, full-window overlay, or interactive prompt owns input focus.
    pub(super) fn has_blocking_modal(&self) -> bool {
        self.unsaved_changes_modal.is_some()
            || self.print_preview.is_some()
            || self.manual_duplex_wait.is_some()
            || self.combine_modal.is_some()
            || self.header_footer_modal.is_some()
            || self.page_number_modal.is_some()
            || self.create_pdf_images_modal.is_some()
            || self.export_images_modal.is_some()
            || self.excel_export_modal.is_some()
            || self.markdown_export_modal.is_some()
            || self.split_pages_modal.is_some()
            || self.watermark_modal.is_some()
            || self.stamp_manager_modal.is_some()
            || self.doc_props_modal.is_some()
            || self.user_guide_modal.is_some()
            || self.favorites_modal.is_some()
            || self.show_customize_toolbar
            || self.show_help_overlay
            || self.show_bookmark_format_overlay
            || self.show_about_overlay
            || self.input_mode != InputMode::Normal
    }

    /// True while any dropdown menu or floating popover is open above the document canvas.
    pub(super) fn has_open_popover(&self) -> bool {
        self.open_menu.is_some()
            || self.show_tab_list_popover
            || self.tab_context_menu.is_some()
            || self.thumb_context_menu.is_some()
            || self.annot_context_menu.is_some()
            || self.show_reading_mode_popover
            || self.show_scroll_mode_popover
            || self.show_notifications
            || self.show_aa_popover
            || self.book_info_popover
    }

    /// Dismisses all transient menus, context menus, and floating popovers.
    pub(super) fn close_all_popovers(&mut self) {
        self.open_menu = None;
        self.show_tab_list_popover = false;
        self.tab_context_menu = None;
        self.thumb_context_menu = None;
        self.annot_context_menu = None;
        self.show_reading_mode_popover = false;
        self.show_scroll_mode_popover = false;
        self.show_notifications = false;
        self.show_aa_popover = false;
        self.aa_margins_submenu = false;
        self.book_info_popover = false;
        self.active_note_popover = None;
    }

    /// Formats the current local wall-clock time as `HH:MM:SS` using zero extra dependencies.
    fn local_hms_timestamp() -> String {
        #[cfg(target_os = "windows")]
        {
            #[repr(C)]
            struct SystemTimeWin {
                w_year: u16,
                w_month: u16,
                w_day_of_week: u16,
                w_day: u16,
                w_hour: u16,
                w_minute: u16,
                w_second: u16,
                w_milliseconds: u16,
            }
            unsafe extern "system" {
                fn GetLocalTime(lp_system_time: *mut SystemTimeWin);
            }
            let mut st = SystemTimeWin {
                w_year: 0,
                w_month: 0,
                w_day_of_week: 0,
                w_day: 0,
                w_hour: 0,
                w_minute: 0,
                w_second: 0,
                w_milliseconds: 0,
            };
            unsafe { GetLocalTime(&mut st) };
            return format!("{:02}:{:02}:{:02}", st.w_hour, st.w_minute, st.w_second);
        }
        #[cfg(unix)]
        {
            unsafe extern "C" {
                fn time(tloc: *mut i64) -> i64;
                fn localtime_r(timep: *const i64, result: *mut std::ffi::c_int) -> *mut std::ffi::c_void;
            }
            let now = unsafe { time(std::ptr::null_mut()) };
            // POSIX guarantees tm_sec, tm_min, tm_hour are the first three c_int fields of struct tm;
            // 16 c_ints (64 bytes) is safely larger than struct tm on Linux and macOS.
            let mut tm_buf = [0 as std::ffi::c_int; 16];
            let res = unsafe { localtime_r(&now, tm_buf.as_mut_ptr()) };
            if !res.is_null() {
                let s = tm_buf[0].clamp(0, 60);
                let m = tm_buf[1].clamp(0, 59);
                let h = tm_buf[2].clamp(0, 23);
                return format!("{h:02}:{m:02}:{s:02}");
            }
            let secs = now.max(0) as u64 % 86_400;
            format!("{:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
        }
        #[cfg(not(any(windows, unix)))]
        {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let day_secs = secs % 86_400;
            format!(
                "{:02}:{:02}:{:02}",
                day_secs / 3600,
                (day_secs % 3600) / 60,
                day_secs % 60
            )
        }
    }

    /// Captures the current `self.status_notice` into the bounded session Notification Board (max 50).
    pub(super) fn record_status_notice_if_new(&mut self) {
        let Some(ref msg) = self.status_notice else {
            return;
        };
        if self.last_recorded_notice.as_deref() == Some(msg.as_str()) {
            return;
        }
        let lower = msg.to_ascii_lowercase();
        let is_error = lower.contains("error")
            || lower.contains("failed")
            || lower.contains("invalid")
            || lower.contains("cannot")
            || lower.contains("refusing");
        if self.notifications.len() >= 50 {
            self.notifications.pop_front();
        }
        self.notifications.push_back(NotificationEntry {
            timestamp: Self::local_hms_timestamp(),
            message: msg.clone(),
            is_error,
        });
        self.last_recorded_notice = Some(msg.clone());
    }

    /// Returns the portable `micropdf.ini` path next to the executable (or `custom_session_ini_path` in tests).
    pub(super) fn portable_session_ini_path(&self) -> PathBuf {
        if let Some(ref custom) = self.custom_session_ini_path {
            return custom.clone();
        }
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("micropdf.ini")))
            .unwrap_or_else(|| PathBuf::from("micropdf.ini"))
    }

    /// Returns the fallback OS config folder `micropdf.ini` path when the executable folder is not writable.
    pub(super) fn fallback_os_config_ini_path() -> PathBuf {
        #[cfg(target_os = "windows")]
        {
            if let Some(appdata) = std::env::var_os("APPDATA") {
                return PathBuf::from(appdata).join("MicroPDF").join("micropdf.ini");
            }
            if let Some(profile) = std::env::var_os("USERPROFILE") {
                return PathBuf::from(profile)
                    .join(".config")
                    .join("MicroPDF")
                    .join("micropdf.ini");
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
                return PathBuf::from(xdg).join("micropdf").join("micropdf.ini");
            }
            if let Some(home) = std::env::var_os("HOME") {
                return PathBuf::from(home)
                    .join(".config")
                    .join("micropdf")
                    .join("micropdf.ini");
            }
        }
        std::env::temp_dir().join("micropdf.ini")
    }

    pub(super) fn serialize_session_ini(cfg: &SessionConfig) -> String {
        let mut out = String::from("# MicroPDF v1.4 Session, Recent Files, Last Positions & UI Preferences\n");
        out.push_str(&format!("ui_theme={}\n", cfg.ui_theme.to_ini_str()));
        out.push_str(&format!("ui_skin={}\n", cfg.ui_skin.to_ini_str()));
        out.push_str(&format!("ui_scale={:.2}\n", cfg.ui_scale.clamp(MIN_UI_SCALE, MAX_UI_SCALE)));
        out.push_str(&format!("scroll_mode={}\n", cfg.scroll_mode.to_ini_str()));
        out.push_str(&format!("page_turn_anim_ms={}\n", cfg.page_turn_anim_ms.min(3000)));
        out.push_str(&format!("reading_filter={}\n", cfg.reading_filter.to_ini_str()));
        out.push_str(&format!("show_toolbar={}\n", if cfg.show_toolbar { 1 } else { 0 }));
        out.push_str(&format!("encrypt_config={}\n", if cfg.encrypt_config { 1 } else { 0 }));
        out.push_str(&format!("epub_font={}\n", cfg.epub_prefs.font_choice.to_ini_str()));
        out.push_str(&format!("epub_font_size={:.1}\n", cfg.epub_prefs.font_size_px.clamp(11.0, 32.0)));
        out.push_str(&format!("epub_line_spacing={}\n", cfg.epub_prefs.line_spacing.to_ini_str()));
        out.push_str(&format!("epub_line_mult={:.2}\n", cfg.epub_prefs.effective_line_mult()));
        out.push_str(&format!("epub_word_spacing={:.1}\n", cfg.epub_prefs.word_spacing_px.clamp(0.0, 8.0)));
        out.push_str(&format!("epub_para_spacing={:.2}\n", cfg.epub_prefs.paragraph_spacing_mult.clamp(0.5, 2.5)));
        out.push_str(&format!("epub_margins={}\n", cfg.epub_prefs.margins.to_ini_str()));
        out.push_str(&format!("epub_margin_ratio={:.3}\n", cfg.epub_prefs.effective_margin_ratio()));
        out.push_str(&format!("epub_margin_top={:.3}\n", cfg.epub_prefs.margin_part(0)));
        out.push_str(&format!("epub_margin_bottom={:.3}\n", cfg.epub_prefs.margin_part(1)));
        out.push_str(&format!("epub_margin_outer={:.3}\n", cfg.epub_prefs.margin_part(2)));
        out.push_str(&format!("epub_margin_inner={:.3}\n", cfg.epub_prefs.margin_part(3)));
        out.push_str(&format!("epub_justify={}\n", if cfg.epub_prefs.justify { 1 } else { 0 }));
        out.push_str(&format!("epub_hyphenate={}\n", if cfg.epub_prefs.hyphenate { 1 } else { 0 }));
        out.push_str(&format!("epub_spread={}\n", if cfg.epub_prefs.two_page_spread { 1 } else { 0 }));
        out.push_str(&format!("epub_page_curl={}\n", if cfg.epub_prefs.page_curl_anim { 1 } else { 0 }));
        out.push_str(&format!("comic_rtl={}\n", if cfg.comic_prefs.rtl_manga { 1 } else { 0 }));
        out.push_str(&format!("comic_layout={}\n", cfg.comic_prefs.layout_mode.to_ini_str()));
        out.push_str(&format!("comic_cover_first={}\n", if cfg.comic_prefs.cover_first { 1 } else { 0 }));
        out.push_str(&format!("comic_fit={}\n", cfg.comic_prefs.fit_mode.to_ini_str()));
        out.push_str(&format!("md_code_font_size={:.1}\n", cfg.md_code_font_px.clamp(10.0, 28.0)));
        out.push_str(&format!("md_preview_font_size={:.1}\n", cfg.md_preview_font_px.clamp(10.0, 30.0)));
        out.push_str(&format!("md_line_spacing={:.2}\n", cfg.md_line_spacing.clamp(1.20, 2.20)));
        out.push_str(&format!("md_word_spacing={:.1}\n", cfg.md_word_spacing_px.clamp(0.0, 8.0)));
        out.push_str(&format!("md_para_spacing={:.2}\n", cfg.md_para_spacing_mult.clamp(0.5, 2.5)));
        out.push_str(&format!("toolbar_items={}\n", cfg.toolbar_config.to_ini_str()));
        for prof in &cfg.hf_profiles {
            out.push_str(&format!("hf_profile={}\n", prof.to_ini_str()));
        }
        out.push_str(&format!("active_tab={}\n", cfg.active_tab));
        for tab in &cfg.tabs {
            let p_str = tab.path.to_string_lossy().replace('\n', "").replace('\r', "");
            if !p_str.is_empty() {
                out.push_str(&format!(
                    "tab={}|{}|{}\n",
                    tab.page_index,
                    tab.view_mode.to_ini_str(),
                    p_str
                ));
            }
        }
        for rec in cfg.recent_files.iter().take(MAX_RECENT_FILES) {
            let r_str = rec.to_string_lossy().replace('\n', "").replace('\r', "");
            if !r_str.is_empty() {
                out.push_str(&format!("recent={}\n", r_str));
            }
        }
        for lp in cfg.last_positions.iter().take(MAX_LAST_POSITIONS) {
            let p_str = lp.path.to_string_lossy().replace('\n', "").replace('\r', "");
            if !p_str.is_empty() {
                out.push_str(&format!(
                    "last_pos={}|{:.2}|{}\n",
                    lp.page_index,
                    lp.zoom_level.clamp(MIN_ZOOM_LEVEL, MAX_ZOOM_LEVEL),
                    p_str
                ));
            }
        }
        for fl in cfg.favorites.to_ini_lines() {
            out.push_str(&fl);
            out.push('\n');
        }
        out
    }

    pub(super) fn parse_session_ini(content: &str) -> SessionConfig {
        let mut cfg = SessionConfig::default();
        let mut fav_lines: Vec<&str> = Vec::new();
        for raw_line in content.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with("fav_folder=") || line.starts_with("fav_doc=") {
                fav_lines.push(line);
                continue;
            }
            if let Some(val) = line.strip_prefix("ui_theme=") {
                cfg.ui_theme = UiThemeMode::from_ini_str(val);
            } else if let Some(val) = line.strip_prefix("ui_skin=") {
                cfg.ui_skin = UiSkin::from_ini_str(val);
            } else if let Some(val) = line.strip_prefix("ui_scale=") {
                if let Ok(sc) = val.trim().parse::<f32>() {
                    if sc.is_finite() {
                        cfg.ui_scale = sc.clamp(MIN_UI_SCALE, MAX_UI_SCALE);
                    }
                }
            } else if let Some(val) = line.strip_prefix("scroll_mode=") {
                cfg.scroll_mode = ScrollMode::from_ini_str(val);
            } else if let Some(val) = line.strip_prefix("page_turn_anim_ms=") {
                if let Ok(ms) = val.trim().parse::<u64>() {
                    cfg.page_turn_anim_ms = ms.min(3000);
                }
            } else if let Some(val) = line.strip_prefix("reading_filter=") {
                cfg.reading_filter = ReadingFilter::from_ini_str(val);
            } else if let Some(val) = line.strip_prefix("show_toolbar=") {
                let v = val.trim();
                cfg.show_toolbar = !(v == "0" || v.eq_ignore_ascii_case("false"));
            } else if let Some(val) = line.strip_prefix("encrypt_config=") {
                let v = val.trim();
                cfg.encrypt_config = v == "1" || v.eq_ignore_ascii_case("true");
            } else if let Some(val) = line.strip_prefix("epub_font=") {
                cfg.epub_prefs.font_choice = EpubFontChoice::from_ini_str(val);
            } else if let Some(val) = line.strip_prefix("epub_font_size=") {
                if let Ok(sz) = val.trim().parse::<f32>() {
                    if sz.is_finite() {
                        cfg.epub_prefs.font_size_px = sz.clamp(11.0, 32.0);
                    }
                }
            } else if let Some(val) = line.strip_prefix("epub_line_spacing=") {
                cfg.epub_prefs.set_line_spacing_preset(LineSpacingPreset::from_ini_str(val));
            } else if let Some(val) = line.strip_prefix("epub_line_mult=") {
                if let Ok(v) = val.trim().parse::<f32>() {
                    if v.is_finite() {
                        cfg.epub_prefs.line_spacing_mult = v.clamp(1.20, 2.20);
                    }
                }
            } else if let Some(val) = line.strip_prefix("epub_word_spacing=") {
                if let Ok(v) = val.trim().parse::<f32>() {
                    if v.is_finite() {
                        cfg.epub_prefs.word_spacing_px = v.clamp(0.0, 8.0);
                    }
                }
            } else if let Some(val) = line.strip_prefix("epub_para_spacing=") {
                if let Ok(v) = val.trim().parse::<f32>() {
                    if v.is_finite() {
                        cfg.epub_prefs.paragraph_spacing_mult = v.clamp(0.5, 2.5);
                    }
                }
            } else if let Some(val) = line.strip_prefix("epub_margins=") {
                cfg.epub_prefs.set_margin_preset(MarginPreset::from_ini_str(val));
            } else if let Some(val) = line.strip_prefix("epub_margin_ratio=") {
                if let Ok(r) = val.trim().parse::<f32>() {
                    if r.is_finite() {
                        cfg.epub_prefs.set_margin_ratio(r);
                    }
                }
            } else if let Some(val) = line.strip_prefix("epub_margin_top=") {
                if let Ok(r) = val.trim().parse::<f32>() {
                    if r.is_finite() {
                        cfg.epub_prefs.set_margin_part(0, r);
                    }
                }
            } else if let Some(val) = line.strip_prefix("epub_margin_bottom=") {
                if let Ok(r) = val.trim().parse::<f32>() {
                    if r.is_finite() {
                        cfg.epub_prefs.set_margin_part(1, r);
                    }
                }
            } else if let Some(val) = line.strip_prefix("epub_margin_outer=") {
                if let Ok(r) = val.trim().parse::<f32>() {
                    if r.is_finite() {
                        cfg.epub_prefs.set_margin_part(2, r);
                    }
                }
            } else if let Some(val) = line.strip_prefix("epub_margin_inner=") {
                if let Ok(r) = val.trim().parse::<f32>() {
                    if r.is_finite() {
                        cfg.epub_prefs.set_margin_part(3, r);
                    }
                }
            } else if let Some(val) = line.strip_prefix("epub_justify=") {
                let v = val.trim();
                cfg.epub_prefs.justify = !(v == "0" || v.eq_ignore_ascii_case("false"));
            } else if let Some(val) = line.strip_prefix("epub_hyphenate=") {
                let v = val.trim();
                cfg.epub_prefs.hyphenate = !(v == "0" || v.eq_ignore_ascii_case("false"));
            } else if let Some(val) = line.strip_prefix("epub_spread=") {
                let v = val.trim();
                cfg.epub_prefs.two_page_spread = !(v == "0" || v.eq_ignore_ascii_case("false"));
            } else if let Some(val) = line.strip_prefix("epub_page_curl=") {
                let v = val.trim();
                cfg.epub_prefs.page_curl_anim = !(v == "0" || v.eq_ignore_ascii_case("false"));
            } else if let Some(val) = line.strip_prefix("comic_rtl=") {
                let v = val.trim();
                cfg.comic_prefs.rtl_manga = v == "1" || v.eq_ignore_ascii_case("true");
            } else if let Some(val) = line.strip_prefix("comic_layout=") {
                cfg.comic_prefs.layout_mode = ComicLayoutMode::from_ini_str(val);
            } else if let Some(val) = line.strip_prefix("comic_cover_first=") {
                let v = val.trim();
                cfg.comic_prefs.cover_first = !(v == "0" || v.eq_ignore_ascii_case("false"));
            } else if let Some(val) = line.strip_prefix("comic_fit=") {
                cfg.comic_prefs.fit_mode = ComicFitMode::from_ini_str(val);
            } else if let Some(val) = line.strip_prefix("md_code_font_size=") {
                if let Ok(v) = val.trim().parse::<f32>() {
                    if v.is_finite() {
                        cfg.md_code_font_px = v.clamp(10.0, 28.0);
                    }
                }
            } else if let Some(val) = line.strip_prefix("md_preview_font_size=") {
                if let Ok(v) = val.trim().parse::<f32>() {
                    if v.is_finite() {
                        cfg.md_preview_font_px = v.clamp(10.0, 30.0);
                    }
                }
            } else if let Some(val) = line.strip_prefix("md_line_spacing=") {
                if let Ok(v) = val.trim().parse::<f32>() {
                    if v.is_finite() {
                        cfg.md_line_spacing = v.clamp(1.20, 2.20);
                    }
                }
            } else if let Some(val) = line.strip_prefix("md_word_spacing=") {
                if let Ok(v) = val.trim().parse::<f32>() {
                    if v.is_finite() {
                        cfg.md_word_spacing_px = v.clamp(0.0, 8.0);
                    }
                }
            } else if let Some(val) = line.strip_prefix("md_para_spacing=") {
                if let Ok(v) = val.trim().parse::<f32>() {
                    if v.is_finite() {
                        cfg.md_para_spacing_mult = v.clamp(0.5, 2.5);
                    }
                }
            } else if let Some(val) = line.strip_prefix("toolbar_items=") {
                cfg.toolbar_config = ToolbarConfig::from_ini_str(val);
            } else if let Some(val) = line.strip_prefix("hf_profile=") {
                if let Some(prof) = HeaderFooterProfile::from_ini_str(val) {
                    if cfg.hf_profiles.len() < 24 {
                        cfg.hf_profiles.push(prof);
                    }
                }
            } else if let Some(val) = line.strip_prefix("active_tab=") {
                if let Ok(idx) = val.trim().parse::<usize>() {
                    cfg.active_tab = idx;
                }
            } else if let Some(val) = line.strip_prefix("tab=") {
                let mut parts = val.splitn(3, '|');
                if let (Some(p_str), Some(vm_str), Some(path_str)) =
                    (parts.next(), parts.next(), parts.next())
                {
                    let page_index = p_str.trim().parse::<u16>().unwrap_or(0);
                    let view_mode = ViewMode::from_ini_str(vm_str);
                    let path = PathBuf::from(path_str.trim());
                    if !path.as_os_str().is_empty() {
                        cfg.tabs.push(SavedSessionTab {
                            path,
                            page_index,
                            view_mode,
                        });
                    }
                }
            } else if let Some(val) = line.strip_prefix("recent=") {
                let path = PathBuf::from(val.trim());
                if !path.as_os_str().is_empty()
                    && cfg.recent_files.len() < MAX_RECENT_FILES
                    && !cfg.recent_files.contains(&path)
                {
                    cfg.recent_files.push(path);
                }
            } else if let Some(val) = line.strip_prefix("last_pos=") {
                let mut parts = val.splitn(3, '|');
                if let (Some(p_str), Some(z_str), Some(path_str)) =
                    (parts.next(), parts.next(), parts.next())
                {
                    let page_index = p_str.trim().parse::<u16>().unwrap_or(0);
                    let zoom_level = z_str
                        .trim()
                        .parse::<f32>()
                        .ok()
                        .filter(|z| z.is_finite())
                        .unwrap_or(1.0)
                        .clamp(MIN_ZOOM_LEVEL, MAX_ZOOM_LEVEL);
                    let path = PathBuf::from(path_str.trim());
                    if !path.as_os_str().is_empty()
                        && cfg.last_positions.len() < MAX_LAST_POSITIONS
                        && !cfg.last_positions.iter().any(|e| e.path == path)
                    {
                        cfg.last_positions.push(LastPositionEntry {
                            path,
                            page_index,
                            zoom_level,
                        });
                    }
                }
            }
        }
        cfg.favorites = crate::favorites::FavoritesTree::from_ini_entries(&fav_lines);
        cfg
    }

    /// Records `(page_index, zoom_level)` for `path` at the front of the 50-entry LRU table.
    pub(super) fn record_last_position(
        &mut self,
        path: &std::path::Path,
        page_index: u16,
        zoom_level: f32,
    ) {
        if path.as_os_str().is_empty() || !path.exists() {
            return;
        }
        let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.last_positions
            .retain(|e| e.path != resolved && e.path != path);
        self.last_positions.insert(
            0,
            LastPositionEntry {
                path: resolved,
                page_index,
                zoom_level: zoom_level.clamp(MIN_ZOOM_LEVEL, MAX_ZOOM_LEVEL),
            },
        );
        self.last_positions.truncate(MAX_LAST_POSITIONS);
    }

    /// Looks up a previously saved `(page_index, zoom_level)` for `path` in the 50-entry LRU table.
    pub(super) fn lookup_last_position(&self, path: &std::path::Path) -> Option<(u16, f32)> {
        if path.as_os_str().is_empty() {
            return None;
        }
        let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.last_positions
            .iter()
            .find(|e| e.path == resolved || e.path == path)
            .map(|e| (e.page_index, e.zoom_level))
    }

    /// Adds `path` to the front of `self.recent_files` (deduplicated, capped at `MAX_RECENT_FILES = 10`).
    pub(super) fn record_recent_file(&mut self, path: &std::path::Path) {
        if path.as_os_str().is_empty() || !path.exists() {
            return;
        }
        let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.recent_files.retain(|p| p != &resolved && p != path);
        self.recent_files.insert(0, resolved);
        self.recent_files.truncate(MAX_RECENT_FILES);
    }

    /// Saves open tabs, current page, view mode, recent files, last positions, and UI preferences to `micropdf.ini`
    /// next to the executable, falling back to the OS config folder if not writable.
    ///
    /// IMPORTANT: Never calls `sync_active_tab_to_slot()` (which moves `self.doc` out into the slot),
    /// so saving preferences mid-session (e.g. toggling theme or UI scale) leaves the active PDF intact!
    pub(super) fn save_session_to_ini(&mut self) {
        // Record active and inactive tab reading positions into the 50-entry LRU table
        let encode_pos = |p: &std::path::Path, pg: u16, sy: f32| -> u16 {
            if DocKind::from_path(p).is_reflowable() {
                sy.round().clamp(0.0, 65535.0) as u16
            } else {
                pg
            }
        };
        let tab_positions: Vec<(PathBuf, u16, f32)> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| {
                if i == self.active_tab {
                    (
                        self.file_path.clone(),
                        encode_pos(&self.file_path, self.current_page_index, self.scroll_y),
                        self.zoom_level,
                    )
                } else {
                    (
                        tab.file_path.clone(),
                        encode_pos(&tab.file_path, tab.current_page_index, tab.scroll_y),
                        tab.zoom_level,
                    )
                }
            })
            .collect();
        for (p, pg, zm) in tab_positions {
            self.record_last_position(&p, pg, zm);
        }
        if self.tabs.is_empty() && !self.file_path.as_os_str().is_empty() {
            let p = self.file_path.clone();
            let pg = encode_pos(&p, self.current_page_index, self.scroll_y);
            self.record_last_position(&p, pg, self.zoom_level);
        }

        let mut saved_tabs = Vec::new();
        let mut mapped_active_tab: Option<usize> = None;
        for (i, tab) in self.tabs.iter().enumerate() {
            let (path, page_index, vm, last_vm) = if i == self.active_tab {
                (
                    &self.file_path,
                    self.current_page_index,
                    self.view_mode,
                    self.last_page_view_mode,
                )
            } else {
                (
                    &tab.file_path,
                    tab.current_page_index,
                    tab.view_mode,
                    tab.last_page_view_mode,
                )
            };
            if !path.as_os_str().is_empty() && path.exists() {
                if i == self.active_tab {
                    mapped_active_tab = Some(saved_tabs.len());
                }
                saved_tabs.push(SavedSessionTab {
                    path: path.clone(),
                    page_index,
                    view_mode: if vm.is_page_view() { vm } else { last_vm },
                });
            }
        }
        // If no tabs slice entry exists yet but a doc is loaded directly on self
        if self.tabs.is_empty() && !self.file_path.as_os_str().is_empty() && self.file_path.exists()
        {
            mapped_active_tab = Some(0);
            saved_tabs.push(SavedSessionTab {
                path: self.file_path.clone(),
                page_index: self.current_page_index,
                view_mode: if self.view_mode.is_page_view() {
                    self.view_mode
                } else {
                    self.last_page_view_mode
                },
            });
        }
        let cfg = SessionConfig {
            active_tab: mapped_active_tab
                .unwrap_or_else(|| self.active_tab.min(saved_tabs.len().saturating_sub(1))),
            tabs: saved_tabs,
            recent_files: self.recent_files.clone(),
            last_positions: self.last_positions.clone(),
            ui_theme: self.ui_theme_mode,
            ui_skin: self.ui_skin,
            ui_scale: self.ui_scale,
            scroll_mode: self.scroll_mode,
            page_turn_anim_ms: self.page_turn_anim_ms,
            reading_filter: self.reading_filter,
            show_toolbar: self.show_toolbar,
            toolbar_config: self.toolbar_config.clone(),
            hf_profiles: self.hf_profiles.clone(),
            encrypt_config: self.encrypt_config,
            favorites: self.favorites.clone(),
            epub_prefs: self.epub_prefs.clone(),
            comic_prefs: self.comic_prefs.clone(),
            md_code_font_px: self.md_code_font_px,
            md_preview_font_px: self.md_preview_font_px,
            md_line_spacing: self.md_line_spacing,
            md_word_spacing_px: self.md_word_spacing_px,
            md_para_spacing_mult: self.md_para_spacing_mult,
        };
        let ini_text = Self::serialize_session_ini(&cfg);
        let payload = if self.encrypt_config {
            crate::config_crypto::encrypt_config(&ini_text)
        } else {
            ini_text
        };
        let primary_path = self.portable_session_ini_path();

        if std::fs::write(&primary_path, payload.as_bytes()).is_ok() {
            return;
        }

        // Fallback to OS config folder when portable directory is not writable
        let fallback_path = Self::fallback_os_config_ini_path();
        if let Some(parent) = fallback_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(&fallback_path, payload.as_bytes()).is_ok() {
            self.status_notice = Some(format!(
                "Exe folder not writable — saved micropdf.ini to OS config folder ({})",
                fallback_path.display()
            ));
        }
    }

    /// Loads `micropdf.ini` (from next to executable or OS config fallback) and optionally offers
    /// to restore the saved session when starting with no file argument.
    pub(super) fn load_session_from_ini(&mut self, offer_restore: bool) {
        let primary_path = self.portable_session_ini_path();
        let fallback_path = Self::fallback_os_config_ini_path();

        let (raw_content, used_fallback) = if let Ok(s) = std::fs::read_to_string(&primary_path) {
            (s, false)
        } else if let Ok(s) = std::fs::read_to_string(&fallback_path) {
            (s, true)
        } else {
            return;
        };

        let was_encrypted = crate::config_crypto::is_encrypted_config(&raw_content);
        let content = if was_encrypted {
            match crate::config_crypto::decrypt_config(&raw_content) {
                Ok(plain) => plain,
                Err(e) => {
                    self.status_notice = Some(format!("Could not decrypt micropdf.ini: {e}"));
                    return;
                }
            }
        } else {
            raw_content
        };

        let cfg = Self::parse_session_ini(&content);
        self.ui_theme_mode = cfg.ui_theme;
        self.ui_skin = cfg.ui_skin;
        self.ui_scale = cfg.ui_scale;
        self.scroll_mode = cfg.scroll_mode;
        self.page_turn_anim_ms = cfg.page_turn_anim_ms;
        self.reading_filter = cfg.reading_filter;
        self.show_toolbar = cfg.show_toolbar;
        self.encrypt_config = was_encrypted || cfg.encrypt_config;
        self.favorites = cfg.favorites;
        self.epub_prefs = cfg.epub_prefs;
        self.comic_prefs = cfg.comic_prefs;
        self.md_code_font_px = cfg.md_code_font_px;
        self.md_preview_font_px = cfg.md_preview_font_px;
        self.md_line_spacing = cfg.md_line_spacing;
        self.md_word_spacing_px = cfg.md_word_spacing_px;
        self.md_para_spacing_mult = cfg.md_para_spacing_mult;
        self.toolbar_config = cfg.toolbar_config;
        self.hf_profiles = cfg.hf_profiles;
        self.last_positions = cfg.last_positions;
        // Saved Header/Footer profiles stay opt-in via < > in the dialog; never auto-fill every PDF.
        self.last_hf_profile = HeaderFooterProfile::default();
        self.recent_files = cfg
            .recent_files
            .into_iter()
            .filter(|p| p.exists())
            .take(MAX_RECENT_FILES)
            .collect();

        if used_fallback {
            self.status_notice = Some(format!(
                "Loaded micropdf.ini from OS config folder ({})",
                fallback_path.display()
            ));
        }

        if offer_restore {
            let valid_tabs: Vec<SavedSessionTab> =
                cfg.tabs.into_iter().filter(|t| t.path.exists()).collect();
            if !valid_tabs.is_empty() {
                let count = valid_tabs.len();
                self.pending_restore_tabs = valid_tabs;
                self.pending_restore_active_tab = cfg.active_tab.min(count.saturating_sub(1));
                self.input_mode = InputMode::RestoreSessionPrompt { tab_count: count };
            }
        }
    }

    /// Restores the saved session tabs from `self.pending_restore_tabs`.
    pub(super) fn restore_pending_session(&mut self) {
        let tabs_to_restore = std::mem::take(&mut self.pending_restore_tabs);
        let target_active = self.pending_restore_active_tab;
        if tabs_to_restore.is_empty() {
            return;
        }
        let mut restored_count = 0usize;
        for (idx, saved) in tabs_to_restore.into_iter().enumerate() {
            let res = if idx == 0 && self.tabs.is_empty() {
                self.load_document(saved.path)
            } else {
                self.open_document_in_new_tab(saved.path)
            };
            if res.is_ok() {
                let total = self.total_pages();
                if total > 0 {
                    self.current_page_index = saved.page_index.min(total - 1);
                }
                self.view_mode = saved.view_mode;
                self.last_page_view_mode = if saved.view_mode.is_page_view() {
                    saved.view_mode
                } else {
                    ViewMode::FitPage
                };
                if self.active_tab < self.tabs.len() {
                    self.tabs[self.active_tab].current_page_index = self.current_page_index;
                    self.tabs[self.active_tab].view_mode = self.view_mode;
                    self.tabs[self.active_tab].last_page_view_mode = self.last_page_view_mode;
                }
                restored_count += 1;
            }
        }
        if target_active < self.tabs.len() && target_active != self.active_tab {
            self.switch_to_tab(target_active);
        }
        self.invalidate_page_caches();
        self.status_notice = Some(format!(
            "Restored previous session ({restored_count} tab(s))"
        ));
        self.update_title_bar();
        self.request_redraw();
    }

    pub fn default_single_instance_lock_path() -> PathBuf {
        std::env::temp_dir().join("micropdf_instance.lock")
    }

    /// Starts the local loopback single-instance listener (`127.0.0.1:0`) and writes `port|token`
    /// to the lock file so a second process can forward a PDF path and exit within 200 ms.
    pub(super) fn start_single_instance_listener(
        &mut self,
        custom_lock_path: Option<PathBuf>,
    ) -> Result<()> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        // Random, so another local program cannot guess it and push files into the viewer.
        let token = random_token_hex()?;
        let lock_path =
            custom_lock_path.unwrap_or_else(Self::default_single_instance_lock_path);
        std::fs::write(&lock_path, format!("{port}|{token}\n").as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o600))?;
        }
        self.single_instance_listener = Some((listener, token, lock_path));
        Ok(())
    }

    pub(super) fn stop_single_instance_listener(&mut self) {
        self.pending_ipc.clear();
        if let Some((listener, token, lock_path)) = self.single_instance_listener.take() {
            if let Ok(addr) = listener.local_addr() {
                let expected = format!("{}|{}", addr.port(), token);
                if let Ok(existing) = std::fs::read_to_string(&lock_path) {
                    if existing.trim() == expected {
                        let _ = std::fs::remove_file(&lock_path);
                    }
                }
            }
        }
    }

    /// Polls the single-instance listener and opens forwarded PDF paths as new tabs. Never
    /// blocks: connections are accepted and read without blocking, a request line is at most
    /// `IPC_MAX_LINE_BYTES`, at most `IPC_MAX_PENDING` connections wait at once, and one that has
    /// not sent its whole line within one frame (`IPC_FRAME`, and at least one later poll) is
    /// dropped. Only lines with the right token naming an existing `.pdf` file are opened.
    pub(super) fn poll_single_instance_listener(&mut self) -> usize {
        use std::io::Read;

        let mut received_paths: Vec<PathBuf> = Vec::new();
        if let Some((ref listener, ref expected_token, _)) = self.single_instance_listener {
            while let Ok((stream, _peer)) = listener.accept() {
                if self.pending_ipc.len() >= IPC_MAX_PENDING || stream.set_nonblocking(true).is_err() {
                    continue; // dropped: closes the connection
                }
                self.pending_ipc.push(PendingIpcConnection {
                    stream,
                    buf: Vec::new(),
                    accepted_at: Instant::now(),
                    polls: 0,
                });
            }

            let mut chunk = [0u8; 1024];
            self.pending_ipc.retain_mut(|conn| {
                conn.polls += 1;
                let mut eof = false;
                loop {
                    match conn.stream.read(&mut chunk) {
                        Ok(0) => {
                            eof = true;
                            break;
                        }
                        Ok(n) => {
                            conn.buf.extend_from_slice(&chunk[..n]);
                            if conn.buf.len() > IPC_MAX_LINE_BYTES || conn.buf.contains(&b'\n') {
                                break;
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(_) => {
                            eof = true; // reset or other error: drop it
                            break;
                        }
                    }
                }
                if let Some(end) = conn.buf.iter().position(|&b| b == b'\n') {
                    if end <= IPC_MAX_LINE_BYTES {
                        let line = String::from_utf8_lossy(&conn.buf[..end]);
                        let line = line.trim_end_matches('\r');
                        if let Some((tok, path_str)) = line.split_once('|') {
                            if tokens_match(tok.as_bytes(), expected_token.as_bytes())
                                && !path_str.is_empty()
                            {
                                received_paths.push(PathBuf::from(path_str));
                            }
                        }
                    }
                    return false;
                }
                let expired = conn.polls >= 2 && conn.accepted_at.elapsed() >= IPC_FRAME;
                !(eof || conn.buf.len() > IPC_MAX_LINE_BYTES || expired)
            });
        } else {
            self.pending_ipc.clear();
        }

        for path in received_paths {
            if is_acceptable_forwarded_pdf(&path) {
                self.queued_ipc_paths.push_back(path);
            }
        }
        if self.has_blocking_modal() {
            return 0;
        }
        let mut opened = 0usize;
        while let Some(path) = self.queued_ipc_paths.pop_front() {
            if !is_acceptable_forwarded_pdf(&path) {
                continue;
            }
            self.close_all_popovers();
            let fname = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("document.pdf")
                .to_string();
            let res = if !self.has_any_document() {
                self.load_document(path)
            } else {
                self.open_document_in_new_tab(path)
            };
            if res.is_ok() {
                opened += 1;
                self.status_notice = Some(format!(
                    "Opened '{fname}' in Tab {} from external launch",
                    self.active_tab + 1
                ));
                if let Some(w) = self.window.as_ref() {
                    w.focus_window();
                }
                self.update_title_bar();
                self.request_redraw();
            }
        }
        opened
    }

    pub(super) fn read_file_mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
        std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
    }

    pub(super) fn is_markdown_tab(&self) -> bool {
        self.markdown_doc.is_some()
    }

    pub(super) fn is_comic_tab(&self) -> bool {
        self.comic_doc.is_some()
    }

    pub(super) fn has_any_document(&self) -> bool {
        self.doc.is_some() || self.is_markdown_tab() || self.is_comic_tab()
    }

    /// Checks if the active tab's file changed on disk (`mtime > self.last_file_mtime`).
    /// - If `self.has_unsaved_edits` is false, reloads the file from disk keeping `current_page_index` and `scroll_y`.
    /// - If `self.has_unsaved_edits` is true, shows a status-bar notice instead of reloading.
    pub(super) fn check_and_auto_reload_active_file(&mut self) -> bool {
        if !self.has_any_document() || self.file_path.as_os_str().is_empty() {
            return false;
        }
        let Some(disk_mtime) = Self::read_file_mtime(&self.file_path) else {
            return false;
        };
        let Some(prev_mtime) = self.last_file_mtime else {
            self.last_file_mtime = Some(disk_mtime);
            return false;
        };
        if disk_mtime <= prev_mtime {
            return false;
        }

        if self.markdown_doc.is_some() {
            return self.reload_active_markdown();
        }
        if self.comic_doc.is_some() {
            return self.reload_active_comic();
        }

        if self.has_unsaved_edits {
            self.last_file_mtime = Some(disk_mtime);
            if self.active_tab < self.tabs.len() {
                self.tabs[self.active_tab].last_file_mtime = Some(disk_mtime);
            }
            self.status_notice = Some(
                "File changed on disk — Not auto-reloaded because you have unsaved edits (Save 'S' first)"
                    .to_string(),
            );
            self.update_title_bar();
            self.request_redraw();
            return false;
        }

        let saved_page = self.current_page_index;
        let saved_scroll_y = self.scroll_y;
        let saved_pan_x = self.pan_x;
        let saved_zoom = self.zoom_level;
        let saved_view_mode = self.view_mode;

        // If the external writer (e.g. pdflatex) is mid-write, leave last_file_mtime unchanged so the next poll retries.
        let Ok(new_doc) = self.engine.load_pdf_safe(&self.file_path) else {
            return false;
        };
        let new_total = new_doc.pages().len();
        let (skip_pages, roman_end) = PdfEngine::read_page_labels_skip_and_roman_from_doc(&new_doc);
        let loaded_bookmarks =
            PdfEngine::load_bookmarks_with_skip(&new_doc, &self.file_path, skip_pages, roman_end);

        self.invalidate_page_caches();
        self.thumbnail_cache.clear();
        self.thumbnail_cache.shrink_to_fit();

        self.doc = Some(new_doc);
        // v1.2.1: snapshots from before the reload must not be restorable, otherwise Ctrl+Z
        // would silently bring back the old file content over the external change.
        self.undo_stack.clear();
        self.undo_stack.shrink_to_fit();
        self.redo_stack.clear();
        self.redo_stack.shrink_to_fit();
        self.undo_generation = 0;
        self.saved_undo_generation = 0;
        self.redacted = false;
        self.last_file_mtime = Some(disk_mtime);
        self.skip_pages = skip_pages;
        self.roman_end = roman_end;
        self.bookmarks = loaded_bookmarks;
        self.selected_bookmark = if self.bookmarks.is_empty() {
            None
        } else {
            Some(0)
        };
        self.current_page_index = if new_total > 0 {
            saved_page.min(new_total - 1)
        } else {
            0
        };
        self.scroll_y = saved_scroll_y;
        self.pan_x = saved_pan_x;
        self.zoom_level = saved_zoom;
        self.view_mode = saved_view_mode;

        if self.active_tab < self.tabs.len() {
            self.tabs[self.active_tab].last_file_mtime = Some(disk_mtime);
            self.tabs[self.active_tab].current_page_index = self.current_page_index;
            self.tabs[self.active_tab].scroll_y = self.scroll_y;
            self.tabs[self.active_tab].pan_x = self.pan_x;
            self.tabs[self.active_tab].zoom_level = self.zoom_level;
            self.tabs[self.active_tab].view_mode = self.view_mode;
            self.tabs[self.active_tab].skip_pages = self.skip_pages;
            self.tabs[self.active_tab].roman_end = self.roman_end;
            self.tabs[self.active_tab].bookmarks = self.bookmarks.clone();
            self.tabs[self.active_tab].undo_stack.clear();
            self.tabs[self.active_tab].redo_stack.clear();
            self.tabs[self.active_tab].undo_generation = 0;
            self.tabs[self.active_tab].saved_undo_generation = 0;
            self.tabs[self.active_tab].redacted = false;
        }

        let fname = self.filename_display();
        self.status_notice = Some(format!(
            "Auto-reloaded '{fname}' from disk (Page {}, scroll preserved)",
            self.current_page_index + 1
        ));
        self.update_title_bar();
        self.request_redraw();
        true
    }

    /// Polls the active file's modification time every 1 s while the window is focused.
    pub(super) fn poll_auto_reload_if_due(&mut self) {
        if !self.window_focused || !self.has_any_document() {
            return;
        }
        if self.last_mtime_poll.elapsed() >= Duration::from_secs(1) {
            self.last_mtime_poll = Instant::now();
            self.check_and_auto_reload_active_file();
        }
    }

    pub(super) fn invalidate_page_caches(&mut self) {
        if let Some(old) = self.cached_page.take() {
            if old.pixels.capacity() > self.spare_pixel_buf.capacity() {
                self.spare_pixel_buf = old.pixels;
            }
        }
        self.cached_second_page = None;
        self.second_page_draw_rect = None;
        for (_, old) in self.continuous_page_cache.drain() {
            if old.pixels.capacity() > self.spare_pixel_buf.capacity() {
                self.spare_pixel_buf = old.pixels;
            }
        }
        self.continuous_page_cache.shrink_to_fit();
        self.continuous_draw_rects.clear();
        for (_, old_tile) in self.tile_cache.drain() {
            if old_tile.pixels.capacity() > self.spare_pixel_buf.capacity() {
                self.spare_pixel_buf = old_tile.pixels;
            }
        }
        self.tile_cache.shrink_to_fit();
        self.tile_form_copy = None;
        self.comic_page_cache.clear();
        self.comic_page_cache.shrink_to_fit();
        self.comic_thumb_cache.clear();
        self.comic_thumb_cache.shrink_to_fit();
        self.comic_page_dims.clear();
        self.comic_page_dims.shrink_to_fit();
        self.comic_strip_rects.clear();
        self.comic_strip_rects.shrink_to_fit();
        // RAM rule: Character boxes for the current page only; free immediately on page/cache invalidation.
        self.cached_char_boxes = None;
        self.cached_text_view = None;
        self.active_note_popover = None;
        self.annot_context_menu = None;
        self.hovered_form_field = None;
        if let Some(ref mut cmp) = self.compare_mode {
            cmp.invalidate_caches();
        }
        if let Some(ref mut d) = self.deskew_bar {
            d.preview_cache = None;
        }
        self.selected_text.clear();
        self.selected_text_page = None;
        self.selected_text_rects.clear();
    }

    fn trim_snapshot_stack(stack: &mut VecDeque<UndoSnapshot>) {
        while stack.len() > MAX_UNDO_STATES {
            stack.pop_front();
        }
        let mut total: usize = stack.iter().map(UndoSnapshot::byte_len).sum();
        while stack.len() > 1 && total > UNDO_BYTES_BUDGET {
            if let Some(old) = stack.pop_front() {
                total = total.saturating_sub(old.byte_len());
            }
        }
        stack.shrink_to(MAX_UNDO_STATES);
    }

    fn capture_snapshot(&self, include_pdf_bytes: bool) -> Option<UndoSnapshot> {
        let pdf_bytes = if include_pdf_bytes {
            let doc = self.doc.as_ref()?;
            Some(doc.save_to_bytes().ok()?)
        } else {
            None
        };
        Some(UndoSnapshot {
            pdf_bytes,
            bookmarks: self.bookmarks.clone(),
            bw_pages: self.bw_pages.clone(),
            roman_end: self.roman_end,
            skip_pages: self.skip_pages,
            redacted: self.redacted,
            generation: self.undo_generation,
        })
    }

    /// Captures the current document's serialized PDF bytes AND metadata (`bookmarks`, `bw_pages`,
    /// `roman_end`, `skip_pages`, `redacted`) onto `self.undo_stack` (max 8 steps) before a mutating action.
    pub(super) fn push_undo_snapshot(&mut self) {
        if let Some(snap) = self.capture_snapshot(true) {
            self.redo_stack.clear();
            self.redo_stack.shrink_to_fit();
            self.undo_stack.push_back(snap);
            Self::trim_snapshot_stack(&mut self.undo_stack);
            self.undo_generation = self.undo_generation.wrapping_add(1);
            self.has_unsaved_edits = self.undo_generation != self.saved_undo_generation;
        }
    }

    /// Captures a 0-byte-PDF metadata-only snapshot (for bookmark add/edit/delete/reorder/indent/import)
    /// so metadata edits are also Undoable & Redoable without allocating PDF bytes.
    pub(super) fn push_metadata_undo_snapshot(&mut self) {
        if let Some(snap) = self.capture_snapshot(false) {
            self.redo_stack.clear();
            self.redo_stack.shrink_to_fit();
            self.undo_stack.push_back(snap);
            Self::trim_snapshot_stack(&mut self.undo_stack);
            self.undo_generation = self.undo_generation.wrapping_add(1);
            self.has_unsaved_edits = self.undo_generation != self.saved_undo_generation;
        }
    }

    fn apply_snapshot(&mut self, snap: UndoSnapshot, push_target_is_redo: bool) -> Result<(), String> {
        let needs_pdf_reload = snap.pdf_bytes.is_some();
        let current_snap = self.capture_snapshot(needs_pdf_reload);
        if let Some(bytes) = snap.pdf_bytes {
            let restored_doc = self
                .engine
                .load_pdf_from_bytes_safe(bytes)
                .map_err(|e| e.to_string())?;
            let new_total = restored_doc.pages().len();
            if self.current_page_index >= new_total && new_total > 0 {
                self.current_page_index = new_total - 1;
            }
            self.doc = Some(restored_doc);
            self.invalidate_page_caches();
            self.thumbnail_cache.clear();
        }
        self.bookmarks = snap.bookmarks;
        self.selected_bookmark = self
            .selected_bookmark
            .filter(|&i| i < self.bookmarks.len())
            .or_else(|| if self.bookmarks.is_empty() { None } else { Some(0) });
        self.bw_pages = snap.bw_pages;
        self.roman_end = snap.roman_end;
        self.skip_pages = snap.skip_pages;
        self.redacted = snap.redacted;
        self.undo_generation = snap.generation;
        self.has_unsaved_edits = self.undo_generation != self.saved_undo_generation;
        self.selected_thumbs.clear();

        if let Some(cur) = current_snap {
            let target_stack = if push_target_is_redo {
                &mut self.redo_stack
            } else {
                &mut self.undo_stack
            };
            target_stack.push_back(cur);
            Self::trim_snapshot_stack(target_stack);
        }
        Ok(())
    }

    /// Pops the latest snapshot from `self.undo_stack`, pushes the current state to `self.redo_stack`,
    /// and restores both PDF bytes (when modified) and all document metadata.
    pub(super) fn perform_undo(&mut self) {
        let Some(prev_snap) = self.undo_stack.pop_back() else {
            self.status_notice = Some("Nothing to undo (0 steps left)".to_string());
            self.update_title_bar();
            self.request_redraw();
            return;
        };
        match self.apply_snapshot(prev_snap, true) {
            Ok(()) => {
                let steps_left = self.undo_stack.len();
                self.status_notice = Some(format!(
                    "Undo successful ({steps_left} undo / {} redo)",
                    self.redo_stack.len()
                ));
            }
            Err(e) => {
                self.status_notice = Some(format!("Undo failed: {e}"));
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    /// Pops the latest snapshot from `self.redo_stack`, pushes the current state to `self.undo_stack`,
    /// and re-applies both PDF bytes (when modified) and all document metadata.
    pub(super) fn perform_redo(&mut self) {
        let Some(next_snap) = self.redo_stack.pop_back() else {
            self.status_notice = Some("Nothing to redo (0 steps left)".to_string());
            self.update_title_bar();
            self.request_redraw();
            return;
        };
        match self.apply_snapshot(next_snap, false) {
            Ok(()) => {
                let redo_left = self.redo_stack.len();
                self.status_notice = Some(format!(
                    "Redo successful ({} undo / {redo_left} redo)",
                    self.undo_stack.len()
                ));
            }
            Err(e) => {
                self.status_notice = Some(format!("Redo failed: {e}"));
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    pub(super) fn total_pages(&self) -> u16 {
        if let Some(ref c) = self.comic_doc {
            c.page_count().min(u16::MAX as usize) as u16
        } else {
            self.doc.as_ref().map(|d| d.pages().len()).unwrap_or(0)
        }
    }

    pub(super) fn rotation_degrees_of_page(page: &PdfPage<'_>) -> i32 {
        match page.rotation().unwrap_or(PdfPageRenderRotation::None) {
            PdfPageRenderRotation::None => 0,
            PdfPageRenderRotation::Degrees90 => 90,
            PdfPageRenderRotation::Degrees180 => 180,
            PdfPageRenderRotation::Degrees270 => 270,
        }
    }

    pub(super) fn page_rotation_degrees(&self, page_idx: u16) -> i32 {
        let Some(doc) = self.doc.as_ref() else {
            return 0;
        };
        let Ok(page) = doc.pages().get(page_idx) else {
            return 0;
        };
        Self::rotation_degrees_of_page(&page)
    }

    pub(super) fn filename_display(&self) -> String {
        if !self.has_any_document() && self.file_path.as_os_str().is_empty() {
            return "No PDF Open".to_string();
        }
        self.file_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("document.pdf")
            .to_string()
    }

    pub(super) fn sidebar_width_px(&self, scale_factor: f64) -> u32 {
        if self.is_fullscreen || self.is_presentation {
            return 0;
        }
        if self.show_bookmarks {
            let base_w = if self.is_distinct_epub_mode() {
                320.0
            } else if self.is_distinct_comic_mode() {
                220.0
            } else {
                SIDEBAR_WIDTH_LOGICAL as f64
            };
            (base_w * scale_factor).round() as u32
        } else {
            0
        }
    }

    pub(super) fn menu_bar_height_px(&self, scale_factor: f64) -> u32 {
        if self.is_fullscreen || self.is_presentation {
            return 0;
        }
        ((MENU_BAR_HEIGHT_LOGICAL as f64) * scale_factor).round() as u32
    }

    pub(super) fn tab_bar_height_px(&self, scale_factor: f64) -> u32 {
        if self.is_fullscreen || self.is_presentation {
            return 0;
        }
        ((TAB_BAR_HEIGHT_LOGICAL as f64) * scale_factor).round() as u32
    }

    pub(super) fn top_bars_height_px(&self, scale_factor: f64) -> u32 {
        self.menu_bar_height_px(scale_factor) + self.tab_bar_height_px(scale_factor)
    }

    pub(super) fn status_bar_height_px(&self, scale_factor: f64) -> u32 {
        if (self.is_fullscreen || self.is_presentation) && self.input_mode == InputMode::Normal {
            return 0;
        }
        ((STATUS_BAR_HEIGHT_LOGICAL as f64) * scale_factor).round() as u32
    }

    /// Toggles `F11` fullscreen mode with top/bottom/sidebar bars hidden.
    pub(super) fn toggle_fullscreen(&mut self) {
        if self.is_presentation {
            self.exit_presentation_or_fullscreen();
            return;
        }
        self.is_fullscreen = !self.is_fullscreen;
        self.close_all_popovers();
        if let Some(w) = self.window.as_ref() {
            w.set_fullscreen(if self.is_fullscreen {
                Some(winit::window::Fullscreen::Borderless(None))
            } else {
                None
            });
        }
        self.invalidate_page_caches();
        self.status_notice = Some(if self.is_fullscreen {
            "Fullscreen Mode ON (F11 or Esc to exit)".to_string()
        } else {
            "Fullscreen Mode OFF".to_string()
        });
        self.update_title_bar();
        self.request_redraw();
    }

    /// Enters `F5` presentation mode (fullscreen, one page fit to screen, black background, arrows/click to advance, Esc to exit).
    pub(super) fn enter_presentation_mode(&mut self) {
        if !self.is_presentation {
            self.pre_presentation_state = Some((
                self.view_mode,
                self.scroll_mode,
                self.zoom_level,
                self.pan_x,
                self.scroll_y,
            ));
        }
        self.is_presentation = true;
        self.is_fullscreen = true;
        self.close_all_popovers();
        self.view_mode = ViewMode::FitPage;
        self.scroll_mode = ScrollMode::SinglePage;
        self.zoom_level = 1.0;
        self.pan_x = 0.0;
        self.scroll_y = 0.0;
        self.scroll_x = 0.0;
        if let Some(w) = self.window.as_ref() {
            w.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
        }
        self.invalidate_page_caches();
        self.status_notice = Some(
            "Presentation Mode (Arrows/Click to advance, Esc to exit)".to_string(),
        );
        self.update_title_bar();
        self.request_redraw();
    }

    /// Exits `F5` presentation mode or `F11` fullscreen mode and restores previous view state.
    pub(super) fn exit_presentation_or_fullscreen(&mut self) {
        if !self.is_presentation && !self.is_fullscreen {
            return;
        }
        let was_presentation = self.is_presentation;
        self.is_presentation = false;
        self.is_fullscreen = false;
        if was_presentation {
            if let Some((vm, sm, zoom, px, sy)) = self.pre_presentation_state.take() {
                self.view_mode = vm;
                self.scroll_mode = sm;
                self.zoom_level = zoom;
                self.pan_x = px;
                self.scroll_y = sy;
            }
        }
        if let Some(w) = self.window.as_ref() {
            w.set_fullscreen(None);
        }
        self.invalidate_page_caches();
        self.status_notice = Some("Exited Fullscreen / Presentation Mode".to_string());
        self.update_title_bar();
        self.request_redraw();
    }


    pub(super) fn update_title_bar(&self) {
        let Some(window) = self.window.as_ref() else {
            return;
        };

        let title = if !self.has_any_document() {
            "MicroPDF".to_string()
        } else if self.doc.is_some() || self.comic_doc.is_some() {
            let filename = self.filename_display();
            let dirty = if self.has_unsaved_edits { "• " } else { "" };
            let total = self.total_pages();
            let cur = if total > 0 { self.current_page_index + 1 } else { 0 };
            format!("{dirty}{filename} ({cur}/{total}) — MicroPDF")
        } else {
            let filename = self.filename_display();
            let dirty = if self.has_unsaved_edits { "• " } else { "" };
            format!("{dirty}{filename} — MicroPDF")
        };

        window.set_title(&title);
    }

    pub(super) fn request_redraw(&self) {
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
        if let Some(pop) = self.editor_popout.as_ref() {
            pop.window.request_redraw();
        }
    }
}

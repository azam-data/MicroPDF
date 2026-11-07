use crate::engine::{
    Bookmark, MarginPosition, PdfEngine, SearchMatch, StampImageBuffer,
};
use anyhow::Result;
use pdfium_render::prelude::*;
use softbuffer::{Context as SoftContext, Surface};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};
use winit::window::Window;
use super::get_current_rss_mb;

pub(super) const BG_COLOR_U32: u32 = 0x002B2B2B; // #2B2B2B subtle dark gray margin background
pub(super) const SIDEBAR_BG_U32: u32 = 0x001E2229;
pub(super) const SIDEBAR_WIDTH_LOGICAL: u32 = 240;
pub(super) const MENU_BAR_HEIGHT_LOGICAL: u32 = 24;
pub(super) const TAB_BAR_HEIGHT_LOGICAL: u32 = 28;
pub(super) const STATUS_BAR_HEIGHT_LOGICAL: u32 = 22;
pub(super) const PAGE_MARGIN_PX: u32 = 24;
pub(super) const SPREAD_GAP_PX: u32 = 12;
pub(super) const THUMB_W: u32 = 140;
pub(super) const THUMB_H: u32 = 198;
pub(super) const MAX_THUMB_CACHE_ENTRIES: usize = 300;
pub(super) const TILE_SIZE_PX: u32 = 512;
pub(super) const MAX_TILE_CACHE_BYTES: usize = 48 * 1024 * 1024;
pub(super) const MAX_TILE_CACHE_ENTRIES: usize =
    MAX_TILE_CACHE_BYTES / ((TILE_SIZE_PX as usize) * (TILE_SIZE_PX as usize) * 4);
pub(super) const MIN_ZOOM_LEVEL: f32 = 0.25;
pub(super) const MAX_ZOOM_LEVEL: f32 = 8.0;
pub(super) const MAX_NAV_HISTORY: usize = 50;
pub(super) const MAX_RECENT_FILES: usize = 10;
pub(super) const MAX_UNDO_STATES: usize = 8;
/// v1.1: total RAM allowed for undo snapshots per tab. The oldest snapshots are dropped first;
/// the most recent one is always kept even if it alone exceeds the budget.
pub(super) const UNDO_BYTES_BUDGET: usize = 64 * 1024 * 1024;
/// Window in which a second `U` press confirms Flatten & Unlock on a signed document.
pub(super) const FLATTEN_CONFIRM_WINDOW: Duration = Duration::from_secs(5);

/// Top Menu Bar categories (`#1E1E1E` bar at the very top of the window).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MenuCategory {
    File,
    View,
    Edit,
    Page,
    Tools,
    Help,
}

impl MenuCategory {
    pub const ALL: [MenuCategory; 6] = [
        MenuCategory::File,
        MenuCategory::View,
        MenuCategory::Edit,
        MenuCategory::Page,
        MenuCategory::Tools,
        MenuCategory::Help,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MenuCategory::File => "[ File ]",
            MenuCategory::View => "[ View ]",
            MenuCategory::Edit => "[ Edit ]",
            MenuCategory::Page => "[ Page ]",
            MenuCategory::Tools => "[ Tools ]",
            MenuCategory::Help => "[ Help ]",
        }
    }

    pub fn items(self) -> &'static [&'static str] {
        match self {
            MenuCategory::File => &[
                "Open PDF in Tab (O)",
                "Combine Multiple PDFs... (Shift+M)",
                "Import Bookmarks .txt (L)",
                "Save PDF (S)",
                "Compress & Save PDF (Z)",
                "Close Tab (Ctrl+W)",
            ],
            MenuCategory::View => &[
                "Fit Full Page (1)",
                "Fit Page Width (2)",
                "Fit Text Width (3)",
                "Two-Page Side-by-Side (4)",
                "Continuous Vertical Scroll (5)",
                "Zoom In (+ / Ctrl+Wheel Up)",
                "Zoom Out (- / Ctrl+Wheel Down)",
                "Reset Zoom 100% (0)",
                "Toggle Thumbnail Grid (Tab)",
                "Toggle Bookmark Sidebar (B)",
                "Cycle Dark / Sepia / Normal Reading Mode (V)",
            ],
            MenuCategory::Edit => &[
                "Undo (Ctrl+Z / Cmd+Z)",
                "Copy Selected Text (Ctrl+C / Cmd+C)",
                "Find Text in PDF... (Ctrl+F / Cmd+F)",
                "Mouse Mode: Select Text (default)",
                "Mouse Mode: Highlight Text",
                "Mouse Mode: Black-Box Redact",
                "Mouse Mode: Crop Page Area",
            ],
            MenuCategory::Page => &[
                "Jump to Page... (G)",
                "Rotate Current Page 90° (R)",
                "Insert Blank Page (I)",
                "Delete Current Page (D)",
                "Convert Page to Black & White (K)",
                "Reset Page Crop",
            ],
            MenuCategory::Tools => &[
                "Page Numbering Roman/Arabic... (N)",
                "Add Header / Footer... (H)",
                "Clear Existing Margins... (C)",
                "Add Watermark... (W)",
                "Stamp Signature / Image... (P)",
                "Split / Extract Pages... (X)",
                "Flatten & Unlock PDF (U)",
            ],
            MenuCategory::Help => &["Toggle Shortcuts Overlay (? / F1)"],
        }
    }
}

/// Interactive Mouse Drag Mode in Single-Page / Spread Views.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MouseMode {
    Select,
    Highlight,
    Redact,
    Crop,
    StampImage,
}

impl MouseMode {
    pub fn short_label(self) -> &'static str {
        match self {
            MouseMode::Select => "Select",
            MouseMode::Highlight => "Highlight",
            MouseMode::Redact => "REDACT",
            MouseMode::Crop => "CROP",
            MouseMode::StampImage => "STAMP",
        }
    }
}

/// Zero-RAM CPU-rendered Reading Filter (`V` key or `View` menu).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ReadingFilter {
    Normal,
    DarkInvert,
    WarmSepia,
}

impl ReadingFilter {
    pub fn next(self) -> Self {
        match self {
            ReadingFilter::Normal => ReadingFilter::DarkInvert,
            ReadingFilter::DarkInvert => ReadingFilter::WarmSepia,
            ReadingFilter::WarmSepia => ReadingFilter::Normal,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ReadingFilter::Normal => "Normal",
            ReadingFilter::DarkInvert => "Dark",
            ReadingFilter::WarmSepia => "Sepia",
        }
    }
}

/// Page & Grid View Modes (`1`, `2`, `3`, `4`, `5`, and `Tab`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ViewMode {
    FitPage,
    FitWidth,
    FitTextWidth,
    TwoPageSpread,
    ContinuousScroll,
    ThumbnailGrid,
}

impl ViewMode {
    pub fn short_label(self) -> &'static str {
        match self {
            ViewMode::FitPage => "FitPage",
            ViewMode::FitWidth => "FitWidth",
            ViewMode::FitTextWidth => "FitText",
            ViewMode::TwoPageSpread => "2-Page",
            ViewMode::ContinuousScroll => "Cont",
            ViewMode::ThumbnailGrid => "Grid",
        }
    }

    pub fn is_page_view(self) -> bool {
        !matches!(self, ViewMode::ThumbnailGrid)
    }

    pub fn to_ini_str(self) -> &'static str {
        match self {
            ViewMode::FitPage => "FitPage",
            ViewMode::FitWidth => "FitWidth",
            ViewMode::FitTextWidth => "FitTextWidth",
            ViewMode::TwoPageSpread => "TwoPageSpread",
            ViewMode::ContinuousScroll => "ContinuousScroll",
            ViewMode::ThumbnailGrid => "ThumbnailGrid",
        }
    }

    pub fn from_ini_str(s: &str) -> Self {
        match s.trim() {
            "FitWidth" => ViewMode::FitWidth,
            "FitTextWidth" => ViewMode::FitTextWidth,
            "TwoPageSpread" => ViewMode::TwoPageSpread,
            "ContinuousScroll" => ViewMode::ContinuousScroll,
            "ThumbnailGrid" => ViewMode::ThumbnailGrid,
            _ => ViewMode::FitPage,
        }
    }
}

/// Saved tab state persisted in `micropdf.ini` on exit.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SavedSessionTab {
    pub(super) path: PathBuf,
    pub(super) page_index: u16,
    pub(super) view_mode: ViewMode,
}

/// Parsed contents of `micropdf.ini` (open tabs, active tab, and last 10 recent files).
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct SessionConfig {
    pub(super) active_tab: usize,
    pub(super) tabs: Vec<SavedSessionTab>,
    pub(super) recent_files: Vec<PathBuf>,
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
    pub(super) rotation_deg: i32,
    pub(super) pixels: Vec<u32>,
}

/// Lightweight 140x198 thumbnail bitmap entry, allocated ONLY while Grid View is active
/// and freed immediately upon returning to Page View or switching tabs.
pub(super) struct ThumbnailEntry {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) rotation_deg: i32,
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

/// Cached 512x512 tile bitmap with LRU access tick (`tile_cache` max 48 MB).
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

/// Zero-RAM inactive document tab slot (never stores cached bitmaps or thumbnails).
pub(super) struct DocumentTab<'a> {
    pub(super) doc: Option<PdfDocument<'a>>,
    pub(super) file_path: PathBuf,
    pub(super) current_page_index: u16,
    pub(super) view_mode: ViewMode,
    pub(super) last_page_view_mode: ViewMode,
    pub(super) zoom_level: f32,
    pub(super) pan_x: f32,
    pub(super) scroll_y: f32,
    pub(super) nav_back_stack: VecDeque<(u16, f32)>,
    pub(super) nav_forward_stack: Vec<(u16, f32)>,
    pub(super) roman_end: usize,
    pub(super) bookmarks: Vec<Bookmark>,
    pub(super) selected_bookmark: Option<usize>,
    pub(super) bookmark_scroll_y: usize,
    pub(super) grid_scroll_y: f32,
    pub(super) bw_pages: HashSet<u16>,
    pub(super) undo_stack: VecDeque<Vec<u8>>,
    /// True once any redaction has been applied; forces the strict save path.
    pub(super) redacted: bool,
    /// True once any unsaved edit has been made to this tab since load/save.
    pub(super) has_unsaved_edits: bool,
    /// Last observed disk modification time of `file_path` for 1s auto-reload polling.
    pub(super) last_file_mtime: Option<std::time::SystemTime>,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) enum TabBarHit {
    SwitchTab(usize),
    CloseTab(usize),
    NewTab,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) enum ThumbBadgeHit {
    None,
    Rotate,
    BwConvert,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) enum SidebarHit {
    ImportTxtButton,
    BookmarkRow { index: usize, on_delete: bool },
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
    PageNumbers {
        input: String,
    },
    HeaderFooterRange {
        range_input: String,
    },
    HeaderFooterPosition {
        start_page: u16,
        end_page: u16,
        pos_input: String,
    },
    HeaderFooterText {
        start_page: u16,
        end_page: u16,
        position: MarginPosition,
        text_input: String,
    },
    ClearMargins {
        input: String,
    },
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
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum PendingDialog {
    Open,
    Save,
    Merge,
    CombineMultiple,
    PickStampImage,
    Split(String),
    ImportBookmarksTxt,
}

pub struct PdfViewerApp<'a> {
    pub(super) engine: &'a PdfEngine,
    /// Open document tabs; inactive tabs hold zero rendered bitmaps.
    pub(super) tabs: Vec<DocumentTab<'a>>,
    pub(super) active_tab: usize,
    /// Active tab state mirrored directly for fast, conflict-free access.
    pub(super) doc: Option<PdfDocument<'a>>,
    pub(super) file_path: PathBuf,
    pub(super) current_page_index: u16,
    pub(super) roman_end: usize,
    pub(super) bw_pages: HashSet<u16>,
    pub(super) undo_stack: VecDeque<Vec<u8>>,
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
    /// Zoom factor (`0.25..=8.0`, i.e. 25%–800%, default `1.0`).
    pub(super) zoom_level: f32,
    /// Horizontal pan offset in physical pixels when the zoomed page width exceeds the viewport width.
    pub(super) pan_x: f32,
    /// Vertical scroll offset in physical pixels when zoomed or in `FitWidth` / `FitTextWidth` / `ContinuousScroll`.
    pub(super) scroll_y: f32,
    /// LRU 512x512 tile cache used when a page's zoomed size exceeds 2x the viewport size (capped at 48 MB).
    pub(super) tile_cache: HashMap<TileKey, TileEntry>,
    pub(super) tile_lru_tick: u64,
    /// Navigation history of `(page_index, scroll_y)` entries (capped at `MAX_NAV_HISTORY = 50`).
    pub(super) nav_back_stack: VecDeque<(u16, f32)>,
    pub(super) nav_forward_stack: Vec<(u16, f32)>,
    /// Currently hovered link target on the page (triggers `CursorIcon::Pointer` hand cursor).
    pub(super) hovered_link: Option<ResolvedLinkTarget>,
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
    /// Saved session tabs loaded from `micropdf.ini` when starting with no file argument.
    pub(super) pending_restore_tabs: Vec<SavedSessionTab>,
    pub(super) pending_restore_active_tab: usize,
    /// Optional custom path override for `micropdf.ini` (used by unit tests).
    pub(super) custom_session_ini_path: Option<PathBuf>,
    /// Local loopback listener `(TcpListener, token, lock_file_path)` for single-instance tab forwarding.
    pub(super) single_instance_listener: Option<(std::net::TcpListener, String, PathBuf)>,
    /// Top Menu Bar open dropdown state (`[ File ] [ View ] [ Edit ] [ Page ] [ Tools ] [ Help ]`).
    pub(super) open_menu: Option<MenuCategory>,
    /// Active Mouse Mode (`Select`, `Highlight`, `Redact`, `Crop`, `StampImage`).
    pub(super) mouse_mode: MouseMode,
    /// Decoded RGBA image waiting to be stamped on the next page click/drag (`P` key).
    pub(super) pending_stamp_image: Option<StampImageBuffer>,
    /// Zero-RAM CPU-rendered Reading Filter (`Normal`, `DarkInvert`, `WarmSepia`).
    pub(super) reading_filter: ReadingFilter,
    /// Active text search state (`Ctrl+F` / `Cmd+F`, `F3` / `Ctrl+G`).
    pub(super) search_query: String,
    pub(super) search_matches: Vec<SearchMatch>,
    pub(super) current_search_match: usize,
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
    /// Left Bookmark Sidebar (`B` to toggle).
    pub(super) show_bookmarks: bool,
    pub(super) bookmarks: Vec<Bookmark>,
    pub(super) selected_bookmark: Option<usize>,
    pub(super) bookmark_scroll_y: usize,
    /// Debounce timer & accumulator for MouseWheel page turning.
    pub(super) last_wheel_turn: Option<Instant>,
    pub(super) wheel_accum_y: f64,
    pub(super) shift_pressed: bool,
    pub(super) cmd_or_ctrl_pressed: bool,
    pub(super) ctrl_pressed: bool,
    pub(super) alt_pressed: bool,
    pub(super) input_mode: InputMode,
    pub(super) pending_dialog: Option<PendingDialog>,
    pub(super) show_help_overlay: bool,
    pub(super) status_notice: Option<String>,
    /// Reusable pixel buffer so page turns never reallocate when `cached_page` is invalidated.
    pub(super) spare_pixel_buf: Vec<u32>,
    /// Declared in strict drop order: `surface` drops first, then `context`, then `window`.
    pub(super) surface: Option<Surface<Rc<Window>, Rc<Window>>>,
    pub(super) context: Option<SoftContext<Rc<Window>>>,
    pub(super) window: Option<Rc<Window>>,
}


impl<'a> PdfViewerApp<'a> {
    pub fn new(engine: &'a PdfEngine, initial_path: Option<PathBuf>) -> Result<Self> {
        let mut app = Self {
            engine,
            tabs: Vec::new(),
            active_tab: 0,
            doc: None,
            file_path: initial_path.clone().unwrap_or_default(),
            current_page_index: 0,
            roman_end: 0,
            bw_pages: HashSet::new(),
            undo_stack: VecDeque::with_capacity(MAX_UNDO_STATES),
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
            zoom_level: 1.0,
            pan_x: 0.0,
            scroll_y: 0.0,
            tile_cache: HashMap::new(),
            tile_lru_tick: 0,
            nav_back_stack: VecDeque::new(),
            nav_forward_stack: Vec::new(),
            hovered_link: None,
            cached_char_boxes: None,
            selected_text: String::new(),
            selected_text_page: None,
            selected_text_rects: Vec::new(),
            last_select_click: None,
            recent_files: Vec::new(),
            pending_restore_tabs: Vec::new(),
            pending_restore_active_tab: 0,
            custom_session_ini_path: None,
            single_instance_listener: None,
            open_menu: None,
            mouse_mode: MouseMode::Select,
            pending_stamp_image: None,
            reading_filter: ReadingFilter::Normal,
            search_query: String::new(),
            search_matches: Vec::new(),
            current_search_match: 0,
            thumbnail_cache: HashMap::new(),
            grid_scroll_y: 0.0,
            hovered_thumb: None,
            grid_drag_page: None,
            last_thumb_click: None,
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
            status_notice: if initial_path.is_none() {
                Some("Open PDF (O) or drag and drop a file into this window".to_string())
            } else {
                None
            },
            spare_pixel_buf: Vec::new(),
            surface: None,
            context: None,
            window: None,
        };
        app.load_session_from_ini(initial_path.is_none());
        if let Some(path) = initial_path {
            app.load_document(path)?;
        }
        Ok(app)
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
        let mut out = String::from("# MicroPDF v1.2 Session & Recent Files\n");
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
        out
    }

    pub(super) fn parse_session_ini(content: &str) -> SessionConfig {
        let mut cfg = SessionConfig::default();
        for raw_line in content.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(val) = line.strip_prefix("active_tab=") {
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
            }
        }
        cfg
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

    /// Saves open tabs, current page, view mode, and recent files to `micropdf.ini` next to the executable,
    /// falling back to the OS config folder (and setting a status bar notice) if the exe folder isn't writable.
    pub(super) fn save_session_to_ini(&mut self) {
        self.sync_active_tab_to_slot();
        let mut saved_tabs = Vec::new();
        for tab in &self.tabs {
            if !tab.file_path.as_os_str().is_empty() && tab.file_path.exists() {
                saved_tabs.push(SavedSessionTab {
                    path: tab.file_path.clone(),
                    page_index: tab.current_page_index,
                    view_mode: if tab.view_mode.is_page_view() {
                        tab.view_mode
                    } else {
                        tab.last_page_view_mode
                    },
                });
            }
        }
        let cfg = SessionConfig {
            active_tab: self.active_tab.min(saved_tabs.len().saturating_sub(1)),
            tabs: saved_tabs,
            recent_files: self.recent_files.clone(),
        };
        let ini_text = Self::serialize_session_ini(&cfg);
        let primary_path = self.portable_session_ini_path();

        if std::fs::write(&primary_path, ini_text.as_bytes()).is_ok() {
            return;
        }

        // Fallback to OS config folder when portable directory is not writable
        let fallback_path = Self::fallback_os_config_ini_path();
        if let Some(parent) = fallback_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(&fallback_path, ini_text.as_bytes()).is_ok() {
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

        let (content, used_fallback) = if let Ok(s) = std::fs::read_to_string(&primary_path) {
            (s, false)
        } else if let Ok(s) = std::fs::read_to_string(&fallback_path) {
            (s, true)
        } else {
            return;
        };

        let cfg = Self::parse_session_ini(&content);
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
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let token = format!("{:x}_{}", nanos, std::process::id());
        let lock_path =
            custom_lock_path.unwrap_or_else(Self::default_single_instance_lock_path);
        std::fs::write(&lock_path, format!("{port}|{token}\n").as_bytes())?;
        self.single_instance_listener = Some((listener, token, lock_path));
        Ok(())
    }

    pub(super) fn stop_single_instance_listener(&mut self) {
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

    /// Polls the non-blocking single-instance listener and opens any forwarded PDF paths as new tabs.
    pub(super) fn poll_single_instance_listener(&mut self) -> usize {
        use std::io::{BufRead, BufReader};

        let mut received_paths: Vec<PathBuf> = Vec::new();
        if let Some((ref listener, ref expected_token, _)) = self.single_instance_listener {
            while let Ok((mut stream, _peer)) = listener.accept() {
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
                let mut line = String::new();
                {
                    let mut reader = BufReader::new(&mut stream);
                    let _ = reader.read_line(&mut line);
                }
                let trimmed = line.trim_end_matches(|c| c == '\r' || c == '\n');
                if let Some((tok, path_str)) = trimmed.split_once('|') {
                    if tok == expected_token && !path_str.is_empty() {
                        received_paths.push(PathBuf::from(path_str));
                    }
                }
            }
        }

        let mut opened = 0usize;
        for path in received_paths {
            if !path.exists() {
                continue;
            }
            let fname = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("document.pdf")
                .to_string();
            let res = if self.doc.is_none() {
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

    /// Checks if the active tab's file changed on disk (`mtime > self.last_file_mtime`).
    /// - If `self.has_unsaved_edits` is false, reloads the file from disk keeping `current_page_index` and `scroll_y`.
    /// - If `self.has_unsaved_edits` is true, shows a status-bar notice instead of reloading.
    pub(super) fn check_and_auto_reload_active_file(&mut self) -> bool {
        if self.doc.is_none() || self.file_path.as_os_str().is_empty() {
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
        let roman_end = PdfEngine::read_page_labels_roman_end_from_path(&self.file_path);
        let loaded_bookmarks = PdfEngine::load_bookmarks(&new_doc, &self.file_path, roman_end);

        self.invalidate_page_caches();
        self.thumbnail_cache.clear();
        self.thumbnail_cache.shrink_to_fit();

        self.doc = Some(new_doc);
        self.last_file_mtime = Some(disk_mtime);
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
            self.tabs[self.active_tab].roman_end = self.roman_end;
            self.tabs[self.active_tab].bookmarks = self.bookmarks.clone();
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
        if !self.window_focused || self.doc.is_none() {
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
        // RAM rule: Character boxes for the current page only; free immediately on page/cache invalidation.
        self.cached_char_boxes = None;
        self.selected_text.clear();
        self.selected_text_page = None;
        self.selected_text_rects.clear();
    }


    /// Captures the current document's serialized PDF bytes onto `self.undo_stack` (max 8 steps)
    /// before a mutating action.
    pub(super) fn push_undo_snapshot(&mut self) {
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        if let Ok(bytes) = doc.save_to_bytes() {
            self.has_unsaved_edits = true;
            if self.undo_stack.len() >= MAX_UNDO_STATES {
                self.undo_stack.pop_front();
            }
            self.undo_stack.push_back(bytes);
            // v1.1: cap total snapshot memory (large scans would otherwise cost 8x file size).
            let mut total: usize = self.undo_stack.iter().map(|b| b.len()).sum();
            while self.undo_stack.len() > 1 && total > UNDO_BYTES_BUDGET {
                if let Some(old) = self.undo_stack.pop_front() {
                    total -= old.len();
                }
            }
            self.undo_stack.shrink_to(MAX_UNDO_STATES);
        }
    }

    /// Pops the latest snapshot from `self.undo_stack` and reloads the active document in memory.
    pub(super) fn perform_undo(&mut self) {
        let Some(prev_bytes) = self.undo_stack.pop_back() else {
            self.status_notice = Some("Nothing to undo (0 steps left)".to_string());
            self.update_title_bar();
            self.request_redraw();
            return;
        };
        match self.engine.load_pdf_from_bytes_safe(prev_bytes) {
            Ok(restored_doc) => {
                let new_total = restored_doc.pages().len();
                if self.current_page_index >= new_total && new_total > 0 {
                    self.current_page_index = new_total - 1;
                }
                self.doc = Some(restored_doc);
                if self.undo_stack.is_empty() {
                    self.has_unsaved_edits = false;
                }
                self.invalidate_page_caches();
                self.thumbnail_cache.clear();
                let steps_left = self.undo_stack.len();
                self.status_notice =
                    Some(format!("Undo successful! ({steps_left} steps left)"));
                self.update_title_bar();
                self.request_redraw();
            }
            Err(e) => {
                self.status_notice = Some(format!("Undo failed: {e}"));
                self.update_title_bar();
                self.request_redraw();
            }
        }
    }

    pub(super) fn total_pages(&self) -> u16 {
        self.doc.as_ref().map(|d| d.pages().len()).unwrap_or(0)
    }

    pub(super) fn page_rotation_degrees(&self, page_idx: u16) -> i32 {
        let Some(doc) = self.doc.as_ref() else {
            return 0;
        };
        let Ok(page) = doc.pages().get(page_idx) else {
            return 0;
        };
        match page.rotation().unwrap_or(PdfPageRenderRotation::None) {
            PdfPageRenderRotation::None => 0,
            PdfPageRenderRotation::Degrees90 => 90,
            PdfPageRenderRotation::Degrees180 => 180,
            PdfPageRenderRotation::Degrees270 => 270,
        }
    }

    pub(super) fn current_page_rotation_degrees(&self) -> i32 {
        self.page_rotation_degrees(self.current_page_index)
    }

    pub(super) fn filename_display(&self) -> String {
        if self.doc.is_none() && self.file_path.as_os_str().is_empty() {
            return "No PDF Open".to_string();
        }
        self.file_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("document.pdf")
            .to_string()
    }

    pub(super) fn sidebar_width_px(&self, scale_factor: f64) -> u32 {
        if self.show_bookmarks {
            ((SIDEBAR_WIDTH_LOGICAL as f64) * scale_factor).round() as u32
        } else {
            0
        }
    }

    pub(super) fn menu_bar_height_px(&self, scale_factor: f64) -> u32 {
        ((MENU_BAR_HEIGHT_LOGICAL as f64) * scale_factor).round() as u32
    }

    pub(super) fn tab_bar_height_px(&self, scale_factor: f64) -> u32 {
        ((TAB_BAR_HEIGHT_LOGICAL as f64) * scale_factor).round() as u32
    }

    pub(super) fn top_bars_height_px(&self, scale_factor: f64) -> u32 {
        self.menu_bar_height_px(scale_factor) + self.tab_bar_height_px(scale_factor)
    }

    pub(super) fn status_bar_height_px(&self, scale_factor: f64) -> u32 {
        ((STATUS_BAR_HEIGHT_LOGICAL as f64) * scale_factor).round() as u32
    }


    pub(super) fn update_title_bar(&self) {
        let Some(window) = self.window.as_ref() else {
            return;
        };

        let title = if let Some(prompt) = self.format_active_prompt_text() {
            format!("MicroPDF | {prompt}")
        } else {
            let total = self.total_pages();
            let current_1based = if total > 0 {
                self.current_page_index + 1
            } else {
                0
            };
            let page_display = if total > 0 && self.roman_end > 0 {
                let logical_lbl = PdfEngine::format_logical_page_label(
                    self.current_page_index as usize,
                    self.roman_end,
                );
                format!("Page {} ({} of {})", logical_lbl, current_1based, total)
            } else if self.view_mode == ViewMode::TwoPageSpread && current_1based + 1 <= total {
                format!("Pages {}-{} / {}", current_1based, current_1based + 1, total)
            } else {
                format!("Page {} / {}", current_1based, total)
            };
            let rotation = self.current_page_rotation_degrees();
            let ram_mb = get_current_rss_mb().unwrap_or(0.0);
            let filename = self.filename_display();
            let mode_tag = self.view_mode.short_label();
            let mouse_tag = self.mouse_mode.short_label();
            let filter_tag = self.reading_filter.label();

            let suffix = if let Some(notice) = &self.status_notice {
                notice.clone()
            } else {
                "Use Top Menu Bar or press ? for Shortcuts".to_string()
            };

            format!(
                "MicroPDF [{mode_tag} | Mouse:{mouse_tag} | Filter:{filter_tag}] | {filename} | {page_display} | Rot:{rotation}° | RAM:{ram_mb:.1} MB | {suffix}"
            )
        };

        window.set_title(&title);
    }

    pub(super) fn request_redraw(&self) {
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }
}

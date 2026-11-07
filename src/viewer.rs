use crate::engine::{
    Bookmark, MarginPosition, MarginZone, PdfEngine, SearchMatch, StampImageBuffer,
};
use anyhow::{Context, Result};
use pdfium_render::prelude::*;
use softbuffer::{Context as SoftContext, Surface};
use std::collections::{HashMap, HashSet, VecDeque};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

const BG_COLOR_U32: u32 = 0x002B2B2B; // #2B2B2B subtle dark gray margin background
const SIDEBAR_BG_U32: u32 = 0x001E2229;
const SIDEBAR_WIDTH_LOGICAL: u32 = 240;
const MENU_BAR_HEIGHT_LOGICAL: u32 = 24;
const TAB_BAR_HEIGHT_LOGICAL: u32 = 28;
const STATUS_BAR_HEIGHT_LOGICAL: u32 = 22;
const PAGE_MARGIN_PX: u32 = 24;
const SPREAD_GAP_PX: u32 = 12;
const THUMB_W: u32 = 140;
const THUMB_H: u32 = 198;
const MAX_UNDO_STATES: usize = 8;
/// v1.1: total RAM allowed for undo snapshots per tab. The oldest snapshots are dropped first;
/// the most recent one is always kept even if it alone exceeds the budget.
const UNDO_BYTES_BUDGET: usize = 64 * 1024 * 1024;
/// Window in which a second `U` press confirms Flatten & Unlock on a signed document.
const FLATTEN_CONFIRM_WINDOW: Duration = Duration::from_secs(5);

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
                "Toggle Thumbnail Grid (Tab)",
                "Toggle Bookmark Sidebar (B)",
                "Cycle Dark / Sepia / Normal Reading Mode (V)",
            ],
            MenuCategory::Edit => &[
                "Undo (Ctrl+Z / Cmd+Z)",
                "Find Text in PDF... (Ctrl+F / Cmd+F)",
                "Mouse Mode: Highlight Text (default)",
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
    Highlight,
    Redact,
    Crop,
    StampImage,
}

impl MouseMode {
    pub fn short_label(self) -> &'static str {
        match self {
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

/// Page & Grid View Modes (`1`, `2`, `3`, `4`, and `Tab`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ViewMode {
    FitPage,
    FitWidth,
    FitTextWidth,
    TwoPageSpread,
    ThumbnailGrid,
}

impl ViewMode {
    pub fn short_label(self) -> &'static str {
        match self {
            ViewMode::FitPage => "FitPage",
            ViewMode::FitWidth => "FitWidth",
            ViewMode::FitTextWidth => "FitText",
            ViewMode::TwoPageSpread => "2-Page",
            ViewMode::ThumbnailGrid => "Grid",
        }
    }

    pub fn is_page_view(self) -> bool {
        !matches!(self, ViewMode::ThumbnailGrid)
    }
}

/// Holds a single visible page's rasterized 0x00RRGGBB pixels.
struct CachedPageBitmap {
    page_index: u16,
    view_mode: ViewMode,
    viewport_width: u32,
    viewport_height: u32,
    bitmap_width: u32,
    bitmap_height: u32,
    content_min_x: f32,
    content_max_x: f32,
    rotation_deg: i32,
    pixels: Vec<u32>,
}

/// Lightweight 140x198 thumbnail bitmap entry, allocated ONLY while Grid View is active
/// and freed immediately upon returning to Page View or switching tabs.
struct ThumbnailEntry {
    width: u32,
    height: u32,
    rotation_deg: i32,
    pixels: Vec<u32>,
}

/// Zero-RAM inactive document tab slot (never stores cached bitmaps or thumbnails).
struct DocumentTab<'a> {
    doc: Option<PdfDocument<'a>>,
    file_path: PathBuf,
    current_page_index: u16,
    view_mode: ViewMode,
    last_page_view_mode: ViewMode,
    scroll_y: f32,
    roman_end: usize,
    bookmarks: Vec<Bookmark>,
    selected_bookmark: Option<usize>,
    bookmark_scroll_y: usize,
    grid_scroll_y: f32,
    bw_pages: HashSet<u16>,
    undo_stack: VecDeque<Vec<u8>>,
    /// True once any redaction has been applied; forces the strict save path.
    redacted: bool,
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum TabBarHit {
    SwitchTab(usize),
    CloseTab(usize),
    NewTab,
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum ThumbBadgeHit {
    None,
    Rotate,
    BwConvert,
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum SidebarHit {
    ImportTxtButton,
    BookmarkRow { index: usize, on_delete: bool },
}

/// Interactive Command Prompt HUD state machine in the Bottom Command Bar & Window Title Bar.
#[derive(Clone, Debug, PartialEq)]
enum InputMode {
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
}

#[derive(Clone, Debug, PartialEq)]
enum PendingDialog {
    Open,
    Save,
    Merge,
    CombineMultiple,
    PickStampImage,
    Split(String),
    ImportBookmarksTxt,
}

pub struct PdfViewerApp<'a> {
    engine: &'a PdfEngine,
    /// Open document tabs; inactive tabs hold zero rendered bitmaps.
    tabs: Vec<DocumentTab<'a>>,
    active_tab: usize,
    /// Active tab state mirrored directly for fast, conflict-free access.
    doc: Option<PdfDocument<'a>>,
    file_path: PathBuf,
    current_page_index: u16,
    roman_end: usize,
    bw_pages: HashSet<u16>,
    undo_stack: VecDeque<Vec<u8>>,
    /// True once any redaction has been applied to the active document.
    redacted: bool,
    /// Set by the first `U` press on a signed document; a second press confirms.
    flatten_confirm_at: Option<Instant>,
    cached_page: Option<CachedPageBitmap>,
    /// Second page bitmap used ONLY in `ViewMode::TwoPageSpread` (`4` key).
    cached_second_page: Option<CachedPageBitmap>,
    /// Signed screen draw rectangle `(draw_x, draw_y, bmp_w, bmp_h)` for the primary page.
    page_draw_rect: (i32, i32, u32, u32),
    /// Signed screen draw rectangle `(draw_x, draw_y, bmp_w, bmp_h)` for the right page in `TwoPageSpread`.
    second_page_draw_rect: Option<(i32, i32, u32, u32)>,
    /// Current mouse cursor coordinates in window physical pixels.
    cursor_pos: (f64, f64),
    /// Active left-click drag state in Page View: `Some((start_x, start_y))`.
    drag_start: Option<(f64, f64)>,
    /// Active View Mode (`FitPage`, `FitWidth`, `FitTextWidth`, `TwoPageSpread`, `ThumbnailGrid`).
    view_mode: ViewMode,
    last_page_view_mode: ViewMode,
    /// Vertical scroll offset in physical pixels when zoomed in `FitWidth` or `FitTextWidth`.
    scroll_y: f32,
    /// Top Menu Bar open dropdown state (`[ File ] [ View ] [ Edit ] [ Page ] [ Tools ] [ Help ]`).
    open_menu: Option<MenuCategory>,
    /// Active Mouse Mode (`Highlight`, `Redact`, `Crop`, `StampImage`).
    mouse_mode: MouseMode,
    /// Decoded RGBA image waiting to be stamped on the next page click/drag (`P` key).
    pending_stamp_image: Option<StampImageBuffer>,
    /// Zero-RAM CPU-rendered Reading Filter (`Normal`, `DarkInvert`, `WarmSepia`).
    reading_filter: ReadingFilter,
    /// Active text search state (`Ctrl+F` / `Cmd+F`, `F3` / `Ctrl+G`).
    search_query: String,
    search_matches: Vec<SearchMatch>,
    current_search_match: usize,
    /// Lightweight thumbnail cache populated ONLY when `view_mode == ViewMode::ThumbnailGrid`.
    thumbnail_cache: HashMap<u16, ThumbnailEntry>,
    /// Vertical scroll offset in physical pixels for Thumbnail Grid View.
    grid_scroll_y: f32,
    /// Currently hovered thumbnail index in Grid View.
    hovered_thumb: Option<u16>,
    /// Active drag-and-drop page reorder state in Grid View: `Some((source_page_idx, start_x, start_y))`
    grid_drag_page: Option<(u16, f64, f64)>,
    /// Last double-click tracking in Grid View: `(page_idx, timestamp)`.
    last_thumb_click: Option<(u16, Instant)>,
    /// Left Bookmark Sidebar (`B` to toggle).
    show_bookmarks: bool,
    bookmarks: Vec<Bookmark>,
    selected_bookmark: Option<usize>,
    bookmark_scroll_y: usize,
    /// Debounce timer & accumulator for MouseWheel page turning.
    last_wheel_turn: Option<Instant>,
    wheel_accum_y: f64,
    shift_pressed: bool,
    cmd_or_ctrl_pressed: bool,
    ctrl_pressed: bool,
    input_mode: InputMode,
    pending_dialog: Option<PendingDialog>,
    show_help_overlay: bool,
    status_notice: Option<String>,
    /// Declared in strict drop order: `surface` drops first, then `context`, then `window`.
    surface: Option<Surface<Rc<Window>, Rc<Window>>>,
    context: Option<SoftContext<Rc<Window>>>,
    window: Option<Rc<Window>>,
}

impl<'a> PdfViewerApp<'a> {
    pub fn new(engine: &'a PdfEngine, initial_path: PathBuf) -> Result<Self> {
        let mut app = Self {
            engine,
            tabs: Vec::new(),
            active_tab: 0,
            doc: None,
            file_path: initial_path.clone(),
            current_page_index: 0,
            roman_end: 0,
            bw_pages: HashSet::new(),
            undo_stack: VecDeque::with_capacity(MAX_UNDO_STATES),
            redacted: false,
            flatten_confirm_at: None,
            cached_page: None,
            cached_second_page: None,
            page_draw_rect: (0, 0, 0, 0),
            second_page_draw_rect: None,
            cursor_pos: (0.0, 0.0),
            drag_start: None,
            view_mode: ViewMode::FitPage,
            last_page_view_mode: ViewMode::FitPage,
            scroll_y: 0.0,
            open_menu: None,
            mouse_mode: MouseMode::Highlight,
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
            input_mode: InputMode::Normal,
            pending_dialog: None,
            show_help_overlay: false,
            status_notice: None,
            surface: None,
            context: None,
            window: None,
        };
        app.load_document(initial_path)?;
        Ok(app)
    }

    fn invalidate_page_caches(&mut self) {
        self.cached_page = None;
        self.cached_second_page = None;
        self.second_page_draw_rect = None;
    }

    fn sync_active_tab_to_slot(&mut self) {
        if self.active_tab < self.tabs.len() {
            let slot = &mut self.tabs[self.active_tab];
            slot.doc = self.doc.take();
            slot.file_path = self.file_path.clone();
            slot.current_page_index = self.current_page_index;
            slot.view_mode = self.view_mode;
            slot.last_page_view_mode = self.last_page_view_mode;
            slot.scroll_y = self.scroll_y;
            slot.roman_end = self.roman_end;
            slot.bookmarks = self.bookmarks.clone();
            slot.selected_bookmark = self.selected_bookmark;
            slot.bookmark_scroll_y = self.bookmark_scroll_y;
            slot.grid_scroll_y = self.grid_scroll_y;
            slot.bw_pages = self.bw_pages.clone();
            // v1.1: move, don't deep-clone, up to 8 full-document snapshots on every tab switch.
            slot.undo_stack = std::mem::take(&mut self.undo_stack);
            slot.redacted = self.redacted;
        }
    }

    fn load_slot_into_active(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active_tab = index;
            let slot = &mut self.tabs[index];
            self.doc = slot.doc.take();
            self.file_path = slot.file_path.clone();
            self.current_page_index = slot.current_page_index;
            self.view_mode = slot.view_mode;
            self.last_page_view_mode = slot.last_page_view_mode;
            self.scroll_y = slot.scroll_y;
            self.roman_end = slot.roman_end;
            self.bookmarks = slot.bookmarks.clone();
            self.selected_bookmark = slot.selected_bookmark;
            self.bookmark_scroll_y = slot.bookmark_scroll_y;
            self.grid_scroll_y = slot.grid_scroll_y;
            self.bw_pages = slot.bw_pages.clone();
            self.undo_stack = std::mem::take(&mut slot.undo_stack);
            self.redacted = slot.redacted;
            self.flatten_confirm_at = None;
            self.search_query.clear();
            self.search_matches.clear();
            self.current_search_match = 0;
        }
    }

    /// Loads `path` into the current tab (or creates the first tab on startup).
    pub fn load_document(&mut self, path: PathBuf) -> Result<()> {
        let loaded = self.engine.load_pdf_safe(&path)?;
        let roman_end = PdfEngine::read_page_labels_roman_end_from_path(&path);
        let loaded_bookmarks = PdfEngine::load_bookmarks(&loaded, &path, roman_end);

        // Strictly drop previous cached page bitmaps, thumbnail cache, and previous document
        self.invalidate_page_caches();
        self.thumbnail_cache.clear();
        self.thumbnail_cache.shrink_to_fit();
        self.doc = None;

        self.file_path = path.clone();
        self.current_page_index = 0;
        self.scroll_y = 0.0;
        self.roman_end = roman_end;
        self.bw_pages.clear();
        self.undo_stack.clear();
        self.redacted = false;
        self.flatten_confirm_at = None;
        self.input_mode = InputMode::Normal;
        self.drag_start = None;
        self.grid_drag_page = None;
        self.grid_scroll_y = 0.0;
        self.search_query.clear();
        self.search_matches.clear();
        self.current_search_match = 0;
        self.bookmarks = loaded_bookmarks;
        self.selected_bookmark = if self.bookmarks.is_empty() {
            None
        } else {
            Some(0)
        };
        self.bookmark_scroll_y = 0;
        self.doc = Some(loaded);

        if self.tabs.is_empty() {
            self.tabs.push(DocumentTab {
                doc: None,
                file_path: path,
                current_page_index: 0,
                view_mode: self.view_mode,
                last_page_view_mode: self.last_page_view_mode,
                scroll_y: 0.0,
                roman_end,
                bookmarks: self.bookmarks.clone(),
                selected_bookmark: self.selected_bookmark,
                bookmark_scroll_y: 0,
                grid_scroll_y: 0.0,
                bw_pages: HashSet::new(),
                undo_stack: VecDeque::with_capacity(MAX_UNDO_STATES),
                redacted: false,
            });
            self.active_tab = 0;
        } else if self.active_tab < self.tabs.len() {
            self.tabs[self.active_tab].file_path = path;
            self.tabs[self.active_tab].current_page_index = 0;
            self.tabs[self.active_tab].scroll_y = 0.0;
            self.tabs[self.active_tab].roman_end = roman_end;
            self.tabs[self.active_tab].bookmarks = self.bookmarks.clone();
            self.tabs[self.active_tab].selected_bookmark = self.selected_bookmark;
            self.tabs[self.active_tab].bookmark_scroll_y = 0;
            self.tabs[self.active_tab].grid_scroll_y = 0.0;
            self.tabs[self.active_tab].bw_pages.clear();
            self.tabs[self.active_tab].undo_stack.clear();
            self.tabs[self.active_tab].redacted = false;
        }

        self.update_title_bar();
        self.request_redraw();
        Ok(())
    }

    /// Opens `path` in a brand-new tab and switches to it immediately, freeing the previous tab's bitmaps.
    pub fn open_document_in_new_tab(&mut self, path: PathBuf) -> Result<()> {
        if self.tabs.is_empty() {
            return self.load_document(path);
        }
        let loaded = self.engine.load_pdf_safe(&path)?;
        let roman_end = PdfEngine::read_page_labels_roman_end_from_path(&path);
        let loaded_bookmarks = PdfEngine::load_bookmarks(&loaded, &path, roman_end);
        let selected_bookmark = if loaded_bookmarks.is_empty() {
            None
        } else {
            Some(0)
        };

        // Save current active tab into its slot and release all cached bitmaps
        self.sync_active_tab_to_slot();
        self.invalidate_page_caches();
        self.thumbnail_cache.clear();
        self.thumbnail_cache.shrink_to_fit();

        let new_idx = self.tabs.len();
        self.tabs.push(DocumentTab {
            doc: None,
            file_path: path.clone(),
            current_page_index: 0,
            view_mode: ViewMode::FitPage,
            last_page_view_mode: ViewMode::FitPage,
            scroll_y: 0.0,
            roman_end,
            bookmarks: loaded_bookmarks.clone(),
            selected_bookmark,
            bookmark_scroll_y: 0,
            grid_scroll_y: 0.0,
            bw_pages: HashSet::new(),
            undo_stack: VecDeque::with_capacity(MAX_UNDO_STATES),
            redacted: false,
        });

        self.active_tab = new_idx;
        self.doc = Some(loaded);
        self.file_path = path;
        self.current_page_index = 0;
        self.view_mode = ViewMode::FitPage;
        self.last_page_view_mode = ViewMode::FitPage;
        self.scroll_y = 0.0;
        self.roman_end = roman_end;
        self.bookmarks = loaded_bookmarks;
        self.selected_bookmark = selected_bookmark;
        self.bookmark_scroll_y = 0;
        self.grid_scroll_y = 0.0;
        self.bw_pages.clear();
        self.undo_stack.clear();
        self.redacted = false;
        self.flatten_confirm_at = None;
        self.search_query.clear();
        self.search_matches.clear();
        self.current_search_match = 0;
        self.input_mode = InputMode::Normal;
        self.drag_start = None;
        self.grid_drag_page = None;

        self.update_title_bar();
        self.request_redraw();
        Ok(())
    }

    /// Opens an in-memory PDF byte vector in a brand-new tab named `virtual_path` (used by Combine Multiple PDFs).
    pub fn open_bytes_in_new_tab(&mut self, bytes: Vec<u8>, virtual_path: PathBuf) -> Result<()> {
        let loaded = self.engine.load_pdf_from_bytes_safe(bytes)?;
        self.sync_active_tab_to_slot();
        self.invalidate_page_caches();
        self.thumbnail_cache.clear();
        self.thumbnail_cache.shrink_to_fit();

        let new_idx = self.tabs.len();
        self.tabs.push(DocumentTab {
            doc: None,
            file_path: virtual_path.clone(),
            current_page_index: 0,
            view_mode: ViewMode::FitPage,
            last_page_view_mode: ViewMode::FitPage,
            scroll_y: 0.0,
            roman_end: 0,
            bookmarks: Vec::new(),
            selected_bookmark: None,
            bookmark_scroll_y: 0,
            grid_scroll_y: 0.0,
            bw_pages: HashSet::new(),
            undo_stack: VecDeque::with_capacity(MAX_UNDO_STATES),
            redacted: false,
        });

        self.active_tab = new_idx;
        self.doc = Some(loaded);
        self.file_path = virtual_path;
        self.current_page_index = 0;
        self.view_mode = ViewMode::FitPage;
        self.last_page_view_mode = ViewMode::FitPage;
        self.scroll_y = 0.0;
        self.roman_end = 0;
        self.bookmarks.clear();
        self.selected_bookmark = None;
        self.bookmark_scroll_y = 0;
        self.grid_scroll_y = 0.0;
        self.bw_pages.clear();
        self.undo_stack.clear();
        self.redacted = false;
        self.flatten_confirm_at = None;
        self.search_query.clear();
        self.search_matches.clear();
        self.current_search_match = 0;
        self.input_mode = InputMode::Normal;
        self.drag_start = None;
        self.grid_drag_page = None;

        self.update_title_bar();
        self.request_redraw();
        Ok(())
    }

    fn switch_to_tab(&mut self, target_idx: usize) {
        if target_idx >= self.tabs.len() || target_idx == self.active_tab {
            return;
        }
        self.sync_active_tab_to_slot();
        // Zero-RAM discipline for inactive tabs: immediately free cached page and thumbnail bitmaps
        self.invalidate_page_caches();
        self.thumbnail_cache.clear();
        self.thumbnail_cache.shrink_to_fit();
        self.drag_start = None;
        self.grid_drag_page = None;
        self.hovered_thumb = None;
        self.input_mode = InputMode::Normal;

        self.load_slot_into_active(target_idx);
        let fname = self.filename_display();
        self.status_notice = Some(format!(
            "Switched to tab {}: '{}'",
            target_idx + 1,
            fname
        ));
        self.update_title_bar();
        self.request_redraw();
    }

    fn cycle_next_tab(&mut self) {
        if self.tabs.len() <= 1 {
            return;
        }
        let next = (self.active_tab + 1) % self.tabs.len();
        self.switch_to_tab(next);
    }

    /// Closes the tab at `close_idx`. Returns `true` if the last remaining tab was closed.
    fn close_tab(&mut self, close_idx: usize) -> bool {
        if close_idx >= self.tabs.len() {
            return false;
        }
        self.sync_active_tab_to_slot();
        self.invalidate_page_caches();
        self.thumbnail_cache.clear();
        self.thumbnail_cache.shrink_to_fit();

        let closed_name = self.tabs[close_idx]
            .file_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("tab")
            .to_string();
        self.tabs.remove(close_idx);

        if self.tabs.is_empty() {
            self.doc = None;
            return true;
        }

        let next_active = if close_idx < self.active_tab {
            self.active_tab - 1
        } else {
            self.active_tab.min(self.tabs.len() - 1)
        };
        self.load_slot_into_active(next_active);
        self.status_notice = Some(format!("Closed tab '{closed_name}'"));
        self.update_title_bar();
        self.request_redraw();
        false
    }

    /// Captures the current document's serialized PDF bytes onto `self.undo_stack` (max 8 steps)
    /// before a mutating action.
    fn push_undo_snapshot(&mut self) {
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        if let Ok(bytes) = doc.save_to_bytes() {
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
    fn perform_undo(&mut self) {
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

    fn total_pages(&self) -> u16 {
        self.doc.as_ref().map(|d| d.pages().len()).unwrap_or(0)
    }

    fn page_rotation_degrees(&self, page_idx: u16) -> i32 {
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

    fn current_page_rotation_degrees(&self) -> i32 {
        self.page_rotation_degrees(self.current_page_index)
    }

    fn filename_display(&self) -> String {
        self.file_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("document.pdf")
            .to_string()
    }

    fn sidebar_width_px(&self, scale_factor: f64) -> u32 {
        if self.show_bookmarks {
            ((SIDEBAR_WIDTH_LOGICAL as f64) * scale_factor).round() as u32
        } else {
            0
        }
    }

    fn menu_bar_height_px(&self, scale_factor: f64) -> u32 {
        ((MENU_BAR_HEIGHT_LOGICAL as f64) * scale_factor).round() as u32
    }

    fn tab_bar_height_px(&self, scale_factor: f64) -> u32 {
        ((TAB_BAR_HEIGHT_LOGICAL as f64) * scale_factor).round() as u32
    }

    fn top_bars_height_px(&self, scale_factor: f64) -> u32 {
        self.menu_bar_height_px(scale_factor) + self.tab_bar_height_px(scale_factor)
    }

    fn status_bar_height_px(&self, scale_factor: f64) -> u32 {
        ((STATUS_BAR_HEIGHT_LOGICAL as f64) * scale_factor).round() as u32
    }

    /// Formats the active interactive prompt string (if any) for both the Bottom Command Bar and Title Bar.
    fn format_active_prompt_text(&self) -> Option<String> {
        match &self.input_mode {
            InputMode::Normal => None,
            InputMode::SearchText { query_input } => Some(format!(
                "FIND TEXT IN PDF: {}_  (Enter to search, Esc to cancel, F3 / Cmd+G next)",
                query_input
            )),
            InputMode::JumpToPage { input } => Some(format!(
                "JUMP TO PAGE (Logical '5', Roman 'ii', or Physical '#5'): {}_  (Enter confirm, Esc cancel)",
                input
            )),
            InputMode::PageNumbers { input } => Some(format!(
                "PAGE NUMBERS: Last Roman page # (e.g. '5' for i..v then 1..N, or '0' all Arabic): {}_",
                input
            )),
            InputMode::HeaderFooterRange { range_input } => Some(format!(
                "HEADER/FOOTER (1/3): Enter page range (e.g. '1-5' or 'all'): {}_",
                range_input
            )),
            InputMode::HeaderFooterPosition { pos_input, .. } => Some(format!(
                "HEADER/FOOTER (2/3): Position ('HL', 'HC', 'HR', 'FL', 'FC', 'FR'): {}_",
                pos_input
            )),
            InputMode::HeaderFooterText { text_input, .. } => Some(format!(
                "HEADER/FOOTER (3/3): Enter text (or empty + Enter to erase existing): {}_",
                text_input
            )),
            InputMode::ClearMargins { input } => Some(format!(
                "CLEAR MARGINS: '1' Footers/Page#, '2' Headers, '3' Both (optional range '1 1-5'): {}_",
                input
            )),
            InputMode::Watermark { input } => Some(format!(
                "WATERMARK: Type watermark text and press Enter: {}_",
                input
            )),
            InputMode::SplitRange { input } => Some(format!(
                "SPLIT PDF: Enter page range to extract (e.g. '1-3' or '2,4,6'): {}_",
                input
            )),
            InputMode::AddBookmark { title_input } => Some(format!(
                "ADD BOOKMARK (Page {}): Type title and press Enter: {}_",
                self.current_page_index + 1,
                title_input
            )),
            InputMode::EditBookmark {
                bookmark_idx,
                title_input,
            } => {
                let page_num = self
                    .bookmarks
                    .get(*bookmark_idx)
                    .map(|b| b.page_index + 1)
                    .unwrap_or(1);
                Some(format!(
                    "EDIT BOOKMARK (Page {}): Type new title and press Enter: {}_",
                    page_num, title_input
                ))
            }
        }
    }

    fn update_title_bar(&self) {
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

    fn request_redraw(&self) {
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    /// Rasterizes a single `page_idx` for the specified `mode` and available `(avail_w, avail_h)`.
    fn rasterize_page_for_mode(
        &self,
        page_idx: u16,
        mode: ViewMode,
        content_w: u32,
        content_h: u32,
        avail_w: f32,
        avail_h: f32,
    ) -> Result<CachedPageBitmap> {
        let doc = self
            .doc
            .as_ref()
            .context("No active PDF document to render")?;
        let page = doc
            .pages()
            .get(page_idx)
            .context("Page index out of bounds")?;
        let rotation_deg = self.page_rotation_degrees(page_idx);

        let page_w_pts = page.width().value.max(1.0);
        let page_h_pts = page.height().value.max(1.0);

        let (content_min_x, content_max_x) = if mode == ViewMode::FitTextWidth {
            let (min_x, _min_y, max_x, _max_y) = PdfEngine::compute_page_content_bounds(&page);
            (min_x, max_x)
        } else {
            (0.0, page_w_pts)
        };

        let scale = match mode {
            ViewMode::FitPage | ViewMode::TwoPageSpread | ViewMode::ThumbnailGrid => {
                (avail_w / page_w_pts).min(avail_h / page_h_pts)
            }
            ViewMode::FitWidth => avail_w / page_w_pts,
            ViewMode::FitTextWidth => {
                let text_w = (content_max_x - content_min_x).max(page_w_pts * 0.25);
                avail_w / text_w
            }
        };

        // Cap maximum rasterized dimension to 3600px to keep RAM strictly bounded
        let max_dim = 3600.0_f32;
        let clamped_scale = scale
            .min(max_dim / page_w_pts)
            .min(max_dim / page_h_pts)
            .max(0.05);

        let target_w = ((page_w_pts * clamped_scale).round() as i32).max(1);
        let target_h = ((page_h_pts * clamped_scale).round() as i32).max(1);

        let bindings = self.engine.pdfium.bindings();
        let mut bitmap = PdfBitmap::empty(target_w, target_h, PdfBitmapFormat::BGRx, bindings)
            .context("Failed to allocate PdfBitmap")?;

        let config = PdfRenderConfig::new()
            .set_fixed_width(target_w)
            .set_fixed_height(target_h)
            .render_form_data(true)
            .render_annotations(true)
            .use_lcd_text_rendering(true)
            .set_reverse_byte_order(false);

        page.render_into_bitmap_with_config(&mut bitmap, &config)
            .context("Failed to render PDF page into bitmap")?;

        let raw_bytes = bitmap.as_raw_bytes();
        drop(bitmap);

        let pixel_count = (target_w as usize) * (target_h as usize);
        let mut pixels = Vec::with_capacity(pixel_count);
        for chunk in raw_bytes.chunks_exact(4).take(pixel_count) {
            let b = chunk[0] as u32;
            let g = chunk[1] as u32;
            let r = chunk[2] as u32;
            pixels.push((r << 16) | (g << 8) | b);
        }
        drop(raw_bytes);

        Ok(CachedPageBitmap {
            page_index: page_idx,
            view_mode: mode,
            viewport_width: content_w,
            viewport_height: content_h,
            bitmap_width: target_w as u32,
            bitmap_height: target_h as u32,
            content_min_x,
            content_max_x,
            rotation_deg,
            pixels,
        })
    }

    /// Computes `(draw_x, draw_y, bmp_w, bmp_h)` for a single-page view (`FitPage`, `FitWidth`, `FitTextWidth`)
    /// and clamps `self.scroll_y` to valid vertical bounds.
    fn compute_single_page_draw_rect(
        &mut self,
        sidebar_w: u32,
        top_bars_h: u32,
        content_w: u32,
        content_h: u32,
        bmp_w: u32,
        bmp_h: u32,
        content_min_x: f32,
        content_max_x: f32,
        page_w_pts: f32,
    ) -> (i32, i32, u32, u32) {
        let draw_x = if self.view_mode == ViewMode::FitTextWidth && page_w_pts > 1.0 {
            let scale = (bmp_w as f32) / page_w_pts;
            let text_center_bx = ((content_min_x + content_max_x) * 0.5 * scale).round() as i32;
            let viewport_center_x = (sidebar_w as i32) + (content_w as i32) / 2;
            viewport_center_x - text_center_bx
        } else {
            (sidebar_w as i32) + ((content_w as i32 - bmp_w as i32) / 2)
        };

        let draw_y = if bmp_h > content_h {
            let max_scroll = (bmp_h - content_h) as f32;
            self.scroll_y = self.scroll_y.clamp(0.0, max_scroll);
            (top_bars_h as i32) - (self.scroll_y.round() as i32)
        } else {
            self.scroll_y = 0.0;
            (top_bars_h as i32) + ((content_h as i32 - bmp_h as i32) / 2)
        };

        (draw_x, draw_y, bmp_w, bmp_h)
    }

    /// Ensures `self.cached_page` (and `self.cached_second_page` in `TwoPageSpread`) holds the rasterized
    /// bitmap(s) for `self.current_page_index` inside `(sidebar_w..viewport_w, top_bars_h..viewport_h - status_bar_h)`.
    fn ensure_current_page_rendered(
        &mut self,
        viewport_w: u32,
        viewport_h: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        status_bar_h: u32,
    ) -> Result<()> {
        let content_w = viewport_w.saturating_sub(sidebar_w).max(1);
        let content_h = viewport_h
            .saturating_sub(top_bars_h + status_bar_h)
            .max(1);
        let total = self.total_pages();
        if total == 0 || self.doc.is_none() {
            return Ok(());
        }

        if self.view_mode == ViewMode::TwoPageSpread {
            self.scroll_y = 0.0;
            let avail_w = (content_w
                .saturating_sub(PAGE_MARGIN_PX * 2 + SPREAD_GAP_PX)
                / 2)
            .max(1) as f32;
            let avail_h = content_h.saturating_sub(PAGE_MARGIN_PX * 2).max(1) as f32;

            let left_idx = self.current_page_index;
            let left_rot = self.page_rotation_degrees(left_idx);
            let need_left = self.cached_page.as_ref().map_or(true, |c| {
                c.page_index != left_idx
                    || c.view_mode != ViewMode::TwoPageSpread
                    || c.viewport_width != content_w
                    || c.viewport_height != content_h
                    || c.rotation_deg != left_rot
            });
            if need_left {
                self.cached_page = None;
                self.cached_page = Some(self.rasterize_page_for_mode(
                    left_idx,
                    ViewMode::TwoPageSpread,
                    content_w,
                    content_h,
                    avail_w,
                    avail_h,
                )?);
            }

            let right_idx_opt = if left_idx + 1 < total {
                Some(left_idx + 1)
            } else {
                None
            };
            if let Some(right_idx) = right_idx_opt {
                let right_rot = self.page_rotation_degrees(right_idx);
                let need_right = self.cached_second_page.as_ref().map_or(true, |c| {
                    c.page_index != right_idx
                        || c.view_mode != ViewMode::TwoPageSpread
                        || c.viewport_width != content_w
                        || c.viewport_height != content_h
                        || c.rotation_deg != right_rot
                });
                if need_right {
                    self.cached_second_page = None;
                    self.cached_second_page = Some(self.rasterize_page_for_mode(
                        right_idx,
                        ViewMode::TwoPageSpread,
                        content_w,
                        content_h,
                        avail_w,
                        avail_h,
                    )?);
                }
            } else {
                self.cached_second_page = None;
            }

            let center_x = (sidebar_w as i32) + (content_w as i32) / 2;
            if let Some(left_bmp) = &self.cached_page {
                let lw = left_bmp.bitmap_width;
                let lh = left_bmp.bitmap_height;
                let lx = if self.cached_second_page.is_some() {
                    center_x - (SPREAD_GAP_PX as i32 / 2) - (lw as i32)
                } else {
                    center_x - (lw as i32 / 2)
                };
                let ly = (top_bars_h as i32) + ((content_h as i32 - lh as i32) / 2);
                self.page_draw_rect = (lx, ly, lw, lh);
            }
            if let Some(right_bmp) = &self.cached_second_page {
                let rw = right_bmp.bitmap_width;
                let rh = right_bmp.bitmap_height;
                let rx = center_x + (SPREAD_GAP_PX as i32 / 2);
                let ry = (top_bars_h as i32) + ((content_h as i32 - rh as i32) / 2);
                self.second_page_draw_rect = Some((rx, ry, rw, rh));
            } else {
                self.second_page_draw_rect = None;
            }

            return Ok(());
        }

        // Single-page view modes: FitPage, FitWidth, FitTextWidth
        self.cached_second_page = None;
        self.second_page_draw_rect = None;

        let rotation_deg = self.current_page_rotation_degrees();
        let page_w_pts = self
            .doc
            .as_ref()
            .and_then(|d| d.pages().get(self.current_page_index).ok())
            .map(|p| p.width().value.max(1.0))
            .unwrap_or(595.0);

        let need_render = self.cached_page.as_ref().map_or(true, |cache| {
            cache.page_index != self.current_page_index
                || cache.view_mode != self.view_mode
                || cache.viewport_width != content_w
                || cache.viewport_height != content_h
                || cache.rotation_deg != rotation_deg
        });

        if need_render {
            self.cached_page = None;
            let avail_w = match self.view_mode {
                ViewMode::FitWidth | ViewMode::FitTextWidth => {
                    content_w.saturating_sub(24).max(1) as f32
                }
                _ => content_w.saturating_sub(PAGE_MARGIN_PX * 2).max(1) as f32,
            };
            let avail_h = content_h.saturating_sub(PAGE_MARGIN_PX * 2).max(1) as f32;
            let rendered = self.rasterize_page_for_mode(
                self.current_page_index,
                self.view_mode,
                content_w,
                content_h,
                avail_w,
                avail_h,
            )?;
            self.cached_page = Some(rendered);
        }

        if let Some(cache) = &self.cached_page {
            let bmp_w = cache.bitmap_width;
            let bmp_h = cache.bitmap_height;
            let c_min_x = cache.content_min_x;
            let c_max_x = cache.content_max_x;
            self.page_draw_rect = self.compute_single_page_draw_rect(
                sidebar_w,
                top_bars_h,
                content_w,
                content_h,
                bmp_w,
                bmp_h,
                c_min_x,
                c_max_x,
                page_w_pts,
            );
        }

        Ok(())
    }

    /// Renders a single 140x198 thumbnail for `page_idx` into `self.thumbnail_cache` if not already cached.
    fn ensure_thumbnail_rendered(&mut self, page_idx: u16) -> Result<()> {
        let rot = self.page_rotation_degrees(page_idx);
        if let Some(existing) = self.thumbnail_cache.get(&page_idx) {
            if existing.rotation_deg == rot {
                return Ok(());
            }
        }

        let Some(doc) = self.doc.as_ref() else {
            return Ok(());
        };
        let page = doc.pages().get(page_idx)?;
        let pw = page.width().value.max(1.0);
        let ph = page.height().value.max(1.0);

        let scale = (THUMB_W as f32 / pw).min(THUMB_H as f32 / ph);
        let tw = ((pw * scale).round() as i32).max(1);
        let th = ((ph * scale).round() as i32).max(1);

        let bindings = self.engine.pdfium.bindings();
        let mut bitmap = PdfBitmap::empty(tw, th, PdfBitmapFormat::BGRx, bindings)?;
        let config = PdfRenderConfig::new()
            .set_fixed_width(tw)
            .set_fixed_height(th)
            .render_form_data(true)
            .render_annotations(true)
            .set_reverse_byte_order(false);

        page.render_into_bitmap_with_config(&mut bitmap, &config)?;
        let raw_bytes = bitmap.as_raw_bytes();
        drop(bitmap);

        let count = (tw as usize) * (th as usize);
        let mut pixels = Vec::with_capacity(count);
        for chunk in raw_bytes.chunks_exact(4).take(count) {
            let b = chunk[0] as u32;
            let g = chunk[1] as u32;
            let r = chunk[2] as u32;
            pixels.push((r << 16) | (g << 8) | b);
        }

        self.thumbnail_cache.insert(
            page_idx,
            ThumbnailEntry {
                width: tw as u32,
                height: th as u32,
                rotation_deg: rot,
                pixels,
            },
        );
        Ok(())
    }

    /// Computes grid layout metrics for Thumbnail Grid View.
    fn grid_metrics(
        &self,
        viewport_w: u32,
        sidebar_w: u32,
        top_bars_h: u32,
    ) -> (u32, u32, u32, u32, u32) {
        let content_w = viewport_w.saturating_sub(sidebar_w).max(1);
        let pad_x = 28u32;
        let pad_y = 36u32;
        let cell_w = THUMB_W + pad_x;
        let cell_h = THUMB_H + pad_y;
        let cols = ((content_w.saturating_sub(pad_x)) / cell_w).max(1);
        let grid_total_w = cols * cell_w;
        let start_x = sidebar_w + content_w.saturating_sub(grid_total_w) / 2 + pad_x / 2;
        let start_y = top_bars_h + 20u32;
        (cols, cell_w, cell_h, start_x, start_y)
    }

    /// Returns the screen rectangle `(x, y, THUMB_W, THUMB_H)` for `page_idx` in Thumbnail Grid View.
    fn thumbnail_screen_rect(
        &self,
        page_idx: u16,
        viewport_w: u32,
        sidebar_w: u32,
        top_bars_h: u32,
    ) -> (i32, i32, u32, u32) {
        let (cols, cell_w, cell_h, start_x, start_y) =
            self.grid_metrics(viewport_w, sidebar_w, top_bars_h);
        let col = (page_idx as u32) % cols;
        let row = (page_idx as u32) / cols;
        let x = (start_x + col * cell_w) as i32;
        let y = (start_y + row * cell_h) as i32 - (self.grid_scroll_y.round() as i32);
        (x, y, THUMB_W, THUMB_H)
    }

    /// Hit-tests which thumbnail (and whether its top-left `[BW]` or top-right `[R]` badge was clicked) is at `(mx, my)`.
    fn hit_test_thumbnail(
        &self,
        mx: f64,
        my: f64,
        viewport_w: u32,
        sidebar_w: u32,
        top_bars_h: u32,
    ) -> Option<(u16, ThumbBadgeHit)> {
        if (mx as u32) < sidebar_w || (my as u32) < top_bars_h {
            return None;
        }
        let total = self.total_pages();
        let ix = mx.round() as i32;
        let iy = my.round() as i32;

        for idx in 0..total {
            let (tx, ty, tw, th) =
                self.thumbnail_screen_rect(idx, viewport_w, sidebar_w, top_bars_h);
            if ix >= tx && ix < tx + (tw as i32) && iy >= ty && iy < ty + (th as i32) + 22 {
                // Top-left `[BW]` badge is 28x18 at `(tx, ty)`
                let on_bw_badge = ix >= tx && ix <= tx + 28 && iy >= ty && iy <= ty + 18;
                if on_bw_badge {
                    return Some((idx, ThumbBadgeHit::BwConvert));
                }
                // Top-right `[R]` badge is 24x18 at `(tx + tw - 24, ty)`
                let badge_x = tx + (tw as i32) - 24;
                let badge_y = ty;
                let on_rotate_badge =
                    ix >= badge_x && ix <= tx + (tw as i32) && iy >= badge_y && iy <= badge_y + 18;
                if on_rotate_badge {
                    return Some((idx, ThumbBadgeHit::Rotate));
                }
                return Some((idx, ThumbBadgeHit::None));
            }
        }
        None
    }

    /// Determines the target page index when dropping an item (reorder or external file drop) at `(mx, my)` in Grid View.
    fn grid_drop_target_index(
        &self,
        mx: f64,
        my: f64,
        viewport_w: u32,
        sidebar_w: u32,
        top_bars_h: u32,
    ) -> u16 {
        let total = self.total_pages();
        if total == 0 {
            return 0;
        }
        if let Some((idx, _)) = self.hit_test_thumbnail(mx, my, viewport_w, sidebar_w, top_bars_h) {
            return idx;
        }
        let (cols, cell_w, cell_h, start_x, start_y) =
            self.grid_metrics(viewport_w, sidebar_w, top_bars_h);
        let rel_x = (mx as i32 - start_x as i32).max(0) as u32;
        let rel_y = (my as i32 + self.grid_scroll_y.round() as i32 - start_y as i32).max(0) as u32;
        let col = (rel_x / cell_w).min(cols - 1);
        let row = rel_y / cell_h;
        let computed = (row * cols + col) as u16;
        computed.min(total.saturating_sub(1))
    }

    /// Blits a `CachedPageBitmap` into `buffer` clipped to the content area
    /// `(sidebar_w..buf_w, top_bars_h..buf_h - status_bar_h)` while applying `self.reading_filter` on the fly.
    fn blit_cached_page(
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        status_bar_h: u32,
        draw_rect: (i32, i32, u32, u32),
        pixels: &[u32],
        filter: ReadingFilter,
    ) {
        let (draw_x, draw_y, bmp_w, bmp_h) = draw_rect;
        let min_screen_x = sidebar_w as i32;
        let max_screen_x = buf_w as i32;
        let min_screen_y = top_bars_h as i32;
        let max_screen_y = (buf_h.saturating_sub(status_bar_h)) as i32;

        let start_row = (min_screen_y - draw_y).max(0) as u32;
        let end_row = ((max_screen_y - draw_y).max(0) as u32).min(bmp_h);
        let start_col = (min_screen_x - draw_x).max(0) as u32;
        let end_col = ((max_screen_x - draw_x).max(0) as u32).min(bmp_w);

        if start_row >= end_row || start_col >= end_col {
            return;
        }

        let copy_len = (end_col - start_col) as usize;
        for row in start_row..end_row {
            let dst_y = (draw_y + row as i32) as u32;
            let dst_x = (draw_x + start_col as i32) as u32;
            let src_idx = (row * bmp_w + start_col) as usize;
            let dst_idx = (dst_y * buf_w + dst_x) as usize;

            match filter {
                ReadingFilter::Normal => {
                    buffer[dst_idx..dst_idx + copy_len]
                        .copy_from_slice(&pixels[src_idx..src_idx + copy_len]);
                }
                ReadingFilter::DarkInvert => {
                    for i in 0..copy_len {
                        let px = pixels[src_idx + i];
                        buffer[dst_idx + i] = px ^ 0x00FFFFFF;
                    }
                }
                ReadingFilter::WarmSepia => {
                    for i in 0..copy_len {
                        let px = pixels[src_idx + i];
                        let r = (px >> 16) & 0xFF;
                        let g = (((px >> 8) & 0xFF) * 95) / 100;
                        let b = ((px & 0xFF) * 83) / 100;
                        buffer[dst_idx + i] = (r << 16) | (g << 8) | b;
                    }
                }
            }
        }
    }

    /// Draws bright cyan/orange search highlight boxes for any `SearchMatch` on `page_idx`.
    fn draw_search_overlays_on_page(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        top_bars_h: u32,
        status_bar_h: u32,
        page_idx: u16,
        draw_rect: (i32, i32, u32, u32),
    ) {
        if self.search_matches.is_empty() {
            return;
        }
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let Ok(page) = doc.pages().get(page_idx) else {
            return;
        };
        let (draw_x, draw_y, bmp_w, bmp_h) = draw_rect;
        if bmp_w == 0 || bmp_h == 0 {
            return;
        }
        let page_w = page.width().value.max(1.0);
        let page_h = page.height().value.max(1.0);
        let config = PdfRenderConfig::new()
            .set_fixed_width(bmp_w as i32)
            .set_fixed_height(bmp_h as i32);

        let min_y_bound = top_bars_h as i32;
        let max_y_bound = (buf_h.saturating_sub(status_bar_h)) as i32;

        for (m_idx, m) in self.search_matches.iter().enumerate() {
            if m.page_index != page_idx {
                continue;
            }
            let is_active = m_idx == self.current_search_match;
            let fill_rgba = if is_active {
                (255, 145, 0, 120) // Bright orange for active match
            } else {
                (0, 229, 255, 95) // Bright cyan for other matches on page
            };
            let border_col = if is_active { 0x00FF6D00 } else { 0x0000B8D4 };

            for r in &m.rects {
                let (bx1, by1, bx2, by2) = match (
                    page.points_to_pixels(r.left(), r.top(), &config),
                    page.points_to_pixels(r.right(), r.bottom(), &config),
                ) {
                    (Ok((x1, y1)), Ok((x2, y2))) => (x1, y1, x2, y2),
                    _ => {
                        let x1 = ((r.left().value / page_w) * (bmp_w as f32)).round() as i32;
                        let y1 =
                            ((1.0 - (r.top().value / page_h)) * (bmp_h as f32)).round() as i32;
                        let x2 = ((r.right().value / page_w) * (bmp_w as f32)).round() as i32;
                        let y2 =
                            ((1.0 - (r.bottom().value / page_h)) * (bmp_h as f32)).round() as i32;
                        (x1, y1, x2, y2)
                    }
                };

                let sx1 = (draw_x + bx1.min(bx2) - 2).clamp(0, buf_w as i32) as u32;
                let sx2 = (draw_x + bx1.max(bx2) + 2).clamp(0, buf_w as i32) as u32;
                let sy1 = (draw_y + by1.min(by2) - 2).clamp(min_y_bound, max_y_bound) as u32;
                let sy2 = (draw_y + by1.max(by2) + 2).clamp(min_y_bound, max_y_bound) as u32;

                if sx2 > sx1 && sy2 > sy1 {
                    blend_rect_rgba(
                        buffer,
                        buf_w,
                        buf_h,
                        sx1,
                        sy1,
                        sx2 - sx1,
                        sy2 - sy1,
                        fill_rgba,
                    );
                    draw_rect_border(
                        buffer,
                        buf_w,
                        buf_h,
                        sx1,
                        sy1,
                        sx2 - sx1,
                        sy2 - sy1,
                        border_col,
                    );
                }
            }
        }
    }

    fn render_to_window(&mut self) -> Result<()> {
        let Some(window) = self.window.as_ref().cloned() else {
            return Ok(());
        };
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);
        let scale_factor = window.scale_factor();
        let sidebar_w = self
            .sidebar_width_px(scale_factor)
            .min(width.saturating_sub(64));
        let menu_bar_h = self
            .menu_bar_height_px(scale_factor)
            .min(height.saturating_sub(48));
        let tab_bar_h = self
            .tab_bar_height_px(scale_factor)
            .min(height.saturating_sub(menu_bar_h + 24));
        let top_bars_h = menu_bar_h + tab_bar_h;
        let status_bar_h = self
            .status_bar_height_px(scale_factor)
            .min(height.saturating_sub(top_bars_h + 16));

        if self.view_mode.is_page_view() {
            self.ensure_current_page_rendered(width, height, sidebar_w, top_bars_h, status_bar_h)?;
        } else {
            // Render only the thumbnails currently visible in the scrolled viewport
            let total = self.total_pages();
            let mut visible_indices = Vec::new();
            for idx in 0..total {
                let (_, ty, _, th) =
                    self.thumbnail_screen_rect(idx, width, sidebar_w, top_bars_h);
                if ty + (th as i32) + 30 >= (top_bars_h as i32)
                    && ty <= (height.saturating_sub(status_bar_h) as i32)
                {
                    visible_indices.push(idx);
                }
            }
            for idx in visible_indices {
                let _ = self.ensure_thumbnail_rendered(idx);
            }
        }

        let Some(mut surface) = self.surface.take() else {
            return Ok(());
        };
        let render_res = (|| -> Result<()> {
            surface
                .resize(
                    NonZeroU32::new(width).unwrap(),
                    NonZeroU32::new(height).unwrap(),
                )
                .map_err(|e| anyhow::anyhow!("Failed to resize softbuffer surface: {e}"))?;

            let mut buffer = surface
                .buffer_mut()
                .map_err(|e| anyhow::anyhow!("Failed to acquire softbuffer frame: {e}"))?;

            // Fill background with #2B2B2B
            buffer.fill(BG_COLOR_U32);

            if self.view_mode.is_page_view() {
                // 1. Blit primary page bitmap (with zero-RAM ReadingFilter applied inline)
                if let Some(cache) = &self.cached_page {
                    Self::blit_cached_page(
                        &mut buffer,
                        width,
                        height,
                        sidebar_w,
                        top_bars_h,
                        status_bar_h,
                        self.page_draw_rect,
                        &cache.pixels,
                        self.reading_filter,
                    );
                    self.draw_search_overlays_on_page(
                        &mut buffer,
                        width,
                        height,
                        top_bars_h,
                        status_bar_h,
                        cache.page_index,
                        self.page_draw_rect,
                    );
                }

                // 1b. If in TwoPageSpread mode, blit right-hand page bitmap as well
                if self.view_mode == ViewMode::TwoPageSpread {
                    if let (Some(second_cache), Some(second_rect)) =
                        (&self.cached_second_page, self.second_page_draw_rect)
                    {
                        Self::blit_cached_page(
                            &mut buffer,
                            width,
                            height,
                            sidebar_w,
                            top_bars_h,
                            status_bar_h,
                            second_rect,
                            &second_cache.pixels,
                            self.reading_filter,
                        );
                        self.draw_search_overlays_on_page(
                            &mut buffer,
                            width,
                            height,
                            top_bars_h,
                            status_bar_h,
                            second_cache.page_index,
                            second_rect,
                        );
                    }
                }

                // 2. If user is holding Left-Click and dragging, draw live mode-colored selection box
                if let Some((sx1, sy1)) = self.drag_start {
                    let (sx2, sy2) = self.cursor_pos;
                    let (_, target_rect) = self.resolve_target_page_and_draw_rect(sx1, sy1);
                    let (px, py, pw, ph) = target_rect;
                    let page_left = px.max(sidebar_w as i32);
                    let page_right = (px + pw as i32).min(width as i32);
                    let page_top = py.max(top_bars_h as i32);
                    let page_bottom =
                        (py + ph as i32).min(height.saturating_sub(status_bar_h) as i32);

                    if page_right > page_left && page_bottom > page_top {
                        let min_x = (sx1.min(sx2).round() as i32)
                            .clamp(page_left, page_right) as u32;
                        let max_x = (sx1.max(sx2).round() as i32)
                            .clamp(page_left, page_right) as u32;
                        let min_y = (sy1.min(sy2).round() as i32)
                            .clamp(page_top, page_bottom) as u32;
                        let max_y = (sy1.max(sy2).round() as i32)
                            .clamp(page_top, page_bottom) as u32;

                        if max_x > min_x && max_y > min_y {
                            let (fill_rgba, border_col) = match self.mouse_mode {
                                MouseMode::Highlight => ((255, 235, 59, 90), 0x00FBC02D),
                                MouseMode::Redact => ((229, 57, 53, 110), 0x00FF1744),
                                MouseMode::Crop => ((0, 230, 118, 85), 0x0000E676),
                                MouseMode::StampImage => ((41, 182, 246, 95), 0x0000B0FF),
                            };
                            blend_rect_rgba(
                                &mut buffer,
                                width,
                                height,
                                min_x,
                                min_y,
                                max_x - min_x,
                                max_y - min_y,
                                fill_rgba,
                            );
                            draw_rect_border(
                                &mut buffer,
                                width,
                                height,
                                min_x,
                                min_y,
                                max_x - min_x,
                                max_y - min_y,
                                border_col,
                            );
                        }
                    }
                }
            } else {
                self.draw_thumbnail_grid(
                    &mut buffer,
                    width,
                    height,
                    sidebar_w,
                    top_bars_h,
                    status_bar_h,
                );
            }

            // 3. If Left Bookmark Sidebar (`B`) is active, draw the 240px sidebar panel between top_bars_h and status_bar_h
            if self.show_bookmarks && sidebar_w > 0 {
                self.draw_bookmark_sidebar(
                    &mut buffer,
                    width,
                    height.saturating_sub(status_bar_h),
                    sidebar_w,
                    top_bars_h,
                    scale_factor,
                );
            }

            // 4. Draw the 28px Multi-Document Tab Bar (`y = menu_bar_h .. top_bars_h`)
            self.draw_tab_bar(
                &mut buffer,
                width,
                height,
                menu_bar_h,
                tab_bar_h,
                scale_factor,
            );

            // 5. Draw the 24px Top Menu Bar (`y = 0 .. menu_bar_h`)
            self.draw_menu_bar(&mut buffer, width, height, menu_bar_h, scale_factor);

            // 6. Draw the 22px Bottom Command & Status Bar (`y = height - status_bar_h .. height`)
            self.draw_status_bar(&mut buffer, width, height, status_bar_h, scale_factor);

            // 7. If a Top Menu dropdown is open, draw it over the viewport
            if let Some(cat) = self.open_menu {
                self.draw_open_menu_dropdown(
                    &mut buffer,
                    width,
                    height,
                    menu_bar_h,
                    cat,
                    scale_factor,
                );
            }

            // 8. If Help Overlay (`?` or `F1`) is active, draw the modal overlay
            if self.show_help_overlay {
                draw_help_overlay(&mut buffer, width, height, scale_factor);
            }

            buffer
                .present()
                .map_err(|e| anyhow::anyhow!("Failed to present softbuffer frame: {e}"))?;

            Ok(())
        })();

        self.surface = Some(surface);
        render_res?;

        self.update_title_bar();
        Ok(())
    }

    fn menu_header_x_range(&self, cat: MenuCategory, scale: u32) -> (u32, u32) {
        let mut cur_x = 6 * scale;
        for c in MenuCategory::ALL {
            let w = (c.label().len() as u32) * 8 * scale + 12 * scale;
            if c == cat {
                return (cur_x, cur_x + w);
            }
            cur_x += w + 4 * scale;
        }
        (0, 0)
    }

    fn draw_menu_bar(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        menu_bar_h: u32,
        scale_factor: f64,
    ) {
        if menu_bar_h == 0 {
            return;
        }
        let scale = (scale_factor.round() as u32).clamp(1, 2);
        // #1E1E1E crisp dark Menu Bar background
        fill_rect_clipped(buffer, buf_w, buf_h, 0, 0, buf_w, menu_bar_h, 0x001E1E1E);
        // 1px bottom divider
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            0,
            (menu_bar_h.saturating_sub(1)) as i32,
            buf_w,
            1,
            0x00333333,
        );

        let text_y = menu_bar_h.saturating_sub(8 * scale) / 2;
        let mx = self.cursor_pos.0.round() as u32;
        let my = self.cursor_pos.1.round() as u32;

        for cat in MenuCategory::ALL {
            let (x0, x1) = self.menu_header_x_range(cat, scale);
            let is_open = self.open_menu == Some(cat);
            let is_hover = my < menu_bar_h && mx >= x0 && mx < x1;
            if is_open || is_hover {
                let bg = if is_open { 0x00007ACC } else { 0x00333337 };
                fill_rect_clipped(
                    buffer,
                    buf_w,
                    buf_h,
                    x0 as i32,
                    1,
                    x1.saturating_sub(x0),
                    menu_bar_h.saturating_sub(2),
                    bg,
                );
            }
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                x0 + 6 * scale,
                text_y,
                cat.label(),
                0x00F0F0F0,
                scale,
            );
        }

        // Right-aligned live status badges in Menu Bar (View Mode, Mouse Mode, Filter)
        let right_info = format!(
            "View:{} | Mouse:{} | Filter:{}",
            self.view_mode.short_label(),
            self.mouse_mode.short_label(),
            self.reading_filter.label()
        );
        let info_w = (right_info.len() as u32) * 8 * scale + 10 * scale;
        let (_, last_menu_x1) = self.menu_header_x_range(MenuCategory::Help, scale);
        if buf_w > last_menu_x1 + info_w + 12 * scale {
            let info_col = match self.mouse_mode {
                MouseMode::Highlight => 0x0080DEEA,
                MouseMode::Redact => 0x00FF8A80,
                MouseMode::Crop => 0x0069F0AE,
                MouseMode::StampImage => 0x00FFD740,
            };
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                buf_w - info_w,
                text_y,
                &right_info,
                info_col,
                scale,
            );
        }
    }

    fn draw_open_menu_dropdown(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        menu_bar_h: u32,
        cat: MenuCategory,
        scale_factor: f64,
    ) {
        let scale = (scale_factor.round() as u32).clamp(1, 2);
        let items = cat.items();
        if items.is_empty() {
            return;
        }
        let (hx0, _) = self.menu_header_x_range(cat, scale);
        let row_h = 22 * scale;
        let max_chars = items.iter().map(|s| s.chars().count()).max().unwrap_or(20) as u32;
        let drop_w = (max_chars * 8 * scale + 24 * scale).min(buf_w.saturating_sub(8));
        let drop_h = (items.len() as u32) * row_h + 4 * scale;
        let drop_x = hx0.min(buf_w.saturating_sub(drop_w + 4));
        let drop_y = menu_bar_h;

        // #252526 dropdown background + border
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            drop_x as i32,
            drop_y as i32,
            drop_w,
            drop_h,
            0x00252526,
        );
        draw_rect_border(
            buffer,
            buf_w,
            buf_h,
            drop_x,
            drop_y,
            drop_w,
            drop_h,
            0x00454545,
        );

        let mx = self.cursor_pos.0.round() as u32;
        let my = self.cursor_pos.1.round() as u32;

        for (idx, &item_str) in items.iter().enumerate() {
            let ry = drop_y + 2 * scale + (idx as u32) * row_h;
            let hovered =
                mx >= drop_x && mx < drop_x + drop_w && my >= ry && my < ry + row_h;
            if hovered {
                // #007ACC hover highlight
                fill_rect_clipped(
                    buffer,
                    buf_w,
                    buf_h,
                    (drop_x + 1) as i32,
                    ry as i32,
                    drop_w.saturating_sub(2),
                    row_h,
                    0x00007ACC,
                );
            }
            let text_y = ry + row_h.saturating_sub(8 * scale) / 2;
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                drop_x + 10 * scale,
                text_y,
                item_str,
                0x00F5F5F5,
                scale,
            );
        }
    }

    fn hit_test_menu_bar_header(
        &self,
        mx: f64,
        my: f64,
        menu_bar_h: u32,
        scale_factor: f64,
    ) -> Option<MenuCategory> {
        let ix = mx.round() as u32;
        let iy = my.round() as u32;
        if iy >= menu_bar_h {
            return None;
        }
        let scale = (scale_factor.round() as u32).clamp(1, 2);
        for cat in MenuCategory::ALL {
            let (x0, x1) = self.menu_header_x_range(cat, scale);
            if ix >= x0 && ix < x1 {
                return Some(cat);
            }
        }
        None
    }

    fn hit_test_open_menu_dropdown(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        menu_bar_h: u32,
        scale_factor: f64,
    ) -> Option<(MenuCategory, usize)> {
        let cat = self.open_menu?;
        let scale = (scale_factor.round() as u32).clamp(1, 2);
        let items = cat.items();
        let (hx0, _) = self.menu_header_x_range(cat, scale);
        let row_h = 22 * scale;
        let max_chars = items.iter().map(|s| s.chars().count()).max().unwrap_or(20) as u32;
        let drop_w = (max_chars * 8 * scale + 24 * scale).min(buf_w.saturating_sub(8));
        let drop_h = (items.len() as u32) * row_h + 4 * scale;
        let drop_x = hx0.min(buf_w.saturating_sub(drop_w + 4));
        let drop_y = menu_bar_h;

        let ix = mx.round() as u32;
        let iy = my.round() as u32;
        if ix < drop_x || ix >= drop_x + drop_w || iy < drop_y + 2 * scale || iy >= drop_y + drop_h
        {
            return None;
        }
        let idx = ((iy - (drop_y + 2 * scale)) / row_h) as usize;
        if idx < items.len() {
            Some((cat, idx))
        } else {
            None
        }
    }

    fn draw_status_bar(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        status_bar_h: u32,
        scale_factor: f64,
    ) {
        if status_bar_h == 0 || buf_h < status_bar_h {
            return;
        }
        let scale = (scale_factor.round() as u32).clamp(1, 2);
        let bar_y = buf_h - status_bar_h;

        if let Some(prompt) = self.format_active_prompt_text() {
            // Active interactive command prompt: bright #007ACC command bar at bottom of window
            fill_rect_clipped(
                buffer,
                buf_w,
                buf_h,
                0,
                bar_y as i32,
                buf_w,
                status_bar_h,
                0x000E639C,
            );
            fill_rect_clipped(buffer, buf_w, buf_h, 0, bar_y as i32, buf_w, 1, 0x004FC3F7);
            let text_y = bar_y + status_bar_h.saturating_sub(8 * scale) / 2;
            let max_chars = (buf_w.saturating_sub(16 * scale) / (8 * scale)).max(8) as usize;
            let display_str: String = if prompt.chars().count() > max_chars {
                let skip = prompt.chars().count() - max_chars;
                prompt.chars().skip(skip).collect()
            } else {
                prompt
            };
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                8 * scale,
                text_y,
                &display_str,
                0x00FFEB3B,
                scale,
            );
            return;
        }

        // Standard Status Bar (#1E1E1E background, 1px top border)
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            0,
            bar_y as i32,
            buf_w,
            status_bar_h,
            0x001E1E1E,
        );
        fill_rect_clipped(buffer, buf_w, buf_h, 0, bar_y as i32, buf_w, 1, 0x00333333);

        let total = self.total_pages();
        let cur = if total > 0 {
            self.current_page_index + 1
        } else {
            0
        };
        let ram_mb = get_current_rss_mb().unwrap_or(0.0);
        let notice = self
            .status_notice
            .as_deref()
            .unwrap_or("Ready — Click Top Menu or press 1-4 View, Ctrl+F Search, P Stamp, V Filter, U Flatten");

        let status_line = format!(
            "Page {cur}/{total} | Mode: {} | Mouse: {} | RAM: {ram_mb:.1}MB | {notice}",
            self.view_mode.short_label(),
            self.mouse_mode.short_label()
        );
        let max_chars = (buf_w.saturating_sub(16 * scale) / (8 * scale)).max(8) as usize;
        let clipped: String = status_line.chars().take(max_chars).collect();
        let text_y = bar_y + status_bar_h.saturating_sub(8 * scale) / 2;
        draw_bitmap_str_8x8(
            buffer,
            buf_w,
            buf_h,
            8 * scale,
            text_y,
            &clipped,
            0x00CCCCCC,
            scale,
        );
    }

    fn draw_tab_bar(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        menu_bar_h: u32,
        tab_bar_h: u32,
        scale_factor: f64,
    ) {
        if tab_bar_h == 0 {
            return;
        }
        let scale = (scale_factor.round() as u32).clamp(1, 2);
        let top_y = menu_bar_h;
        // Dark slate tab bar backdrop
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            0,
            top_y as i32,
            buf_w,
            tab_bar_h,
            0x0015181E,
        );
        // Bottom 1px divider
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            0,
            (top_y + tab_bar_h.saturating_sub(1)) as i32,
            buf_w,
            1,
            0x0037474F,
        );

        let num_tabs = self.tabs.len().max(1);
        let plus_btn_w = 34 * scale;
        let avail_for_tabs = buf_w.saturating_sub(plus_btn_w + 12 * scale);
        let tab_w = (avail_for_tabs / (num_tabs as u32)).clamp(90 * scale, 210 * scale);
        let text_y = top_y + tab_bar_h.saturating_sub(8 * scale) / 2;

        let mut cur_x = 4 * scale;
        for idx in 0..num_tabs {
            if cur_x + tab_w > buf_w.saturating_sub(plus_btn_w) {
                break;
            }
            let is_active = idx == self.active_tab;
            let tab_bg = if is_active { 0x00263238 } else { 0x001C2026 };
            let border_col = if is_active { 0x0042A5F5 } else { 0x0037474F };

            fill_rect_clipped(
                buffer,
                buf_w,
                buf_h,
                cur_x as i32,
                (top_y + 2) as i32,
                tab_w.saturating_sub(2),
                tab_bar_h.saturating_sub(3),
                tab_bg,
            );
            draw_rect_border(
                buffer,
                buf_w,
                buf_h,
                cur_x,
                top_y + 2,
                tab_w.saturating_sub(2),
                tab_bar_h.saturating_sub(3),
                border_col,
            );

            let fname = if is_active {
                self.filename_display()
            } else if let Some(slot) = self.tabs.get(idx) {
                slot.file_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("doc.pdf")
                    .to_string()
            } else {
                "doc.pdf".to_string()
            };

            let max_label_chars =
                ((tab_w.saturating_sub(36 * scale)) / (8 * scale)).max(4) as usize;
            let mut label = format!("{}: {}", idx + 1, fname);
            if label.chars().count() > max_label_chars {
                label = label.chars().take(max_label_chars).collect();
            }

            let text_col = if is_active { 0x00FFFFFF } else { 0x00B0BEC5 };
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                cur_x + 6 * scale,
                text_y,
                &label,
                text_col,
                scale,
            );

            // Close `[x]` button on right side of tab
            let close_x = cur_x + tab_w.saturating_sub(26 * scale);
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                close_x,
                text_y,
                "[x]",
                0x00EF5350,
                scale,
            );

            cur_x += tab_w;
        }

        // Draw `[ + ]` New Tab button
        let plus_x = cur_x + 4 * scale;
        if plus_x + plus_btn_w <= buf_w {
            fill_rect_clipped(
                buffer,
                buf_w,
                buf_h,
                plus_x as i32,
                (top_y + 2) as i32,
                plus_btn_w,
                tab_bar_h.saturating_sub(3),
                0x00263238,
            );
            draw_rect_border(
                buffer,
                buf_w,
                buf_h,
                plus_x,
                top_y + 2,
                plus_btn_w,
                tab_bar_h.saturating_sub(3),
                0x0064B5F6,
            );
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                plus_x + 5 * scale,
                text_y,
                "[+]",
                0x0064B5F6,
                scale,
            );
        }
    }

    fn hit_test_tab_bar(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        menu_bar_h: u32,
        tab_bar_h: u32,
        scale_factor: f64,
    ) -> Option<TabBarHit> {
        let ix = mx.round() as u32;
        let iy = my.round() as u32;
        if iy < menu_bar_h || iy >= menu_bar_h + tab_bar_h {
            return None;
        }
        let scale = (scale_factor.round() as u32).clamp(1, 2);
        let num_tabs = self.tabs.len().max(1);
        let plus_btn_w = 34 * scale;
        let avail_for_tabs = buf_w.saturating_sub(plus_btn_w + 12 * scale);
        let tab_w = (avail_for_tabs / (num_tabs as u32)).clamp(90 * scale, 210 * scale);

        let mut cur_x = 4 * scale;
        for idx in 0..num_tabs {
            if cur_x + tab_w > buf_w.saturating_sub(plus_btn_w) {
                break;
            }
            if ix >= cur_x && ix < cur_x + tab_w {
                let close_x = cur_x + tab_w.saturating_sub(28 * scale);
                if ix >= close_x {
                    return Some(TabBarHit::CloseTab(idx));
                } else {
                    return Some(TabBarHit::SwitchTab(idx));
                }
            }
            cur_x += tab_w;
        }

        let plus_x = cur_x + 4 * scale;
        if ix >= plus_x && ix <= plus_x + plus_btn_w {
            return Some(TabBarHit::NewTab);
        }
        None
    }

    fn draw_thumbnail_grid(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        status_bar_h: u32,
    ) {
        let bottom_limit = buf_h.saturating_sub(status_bar_h);
        let total = self.total_pages();
        for idx in 0..total {
            let (tx, ty, tw, th) = self.thumbnail_screen_rect(idx, buf_w, sidebar_w, top_bars_h);
            if ty + (th as i32) + 28 < (top_bars_h as i32) || ty >= (bottom_limit as i32) {
                continue;
            }

            // Fill cell card backdrop (white paper background inside THUMB_W x THUMB_H)
            fill_rect_clipped(
                buffer,
                buf_w,
                bottom_limit,
                tx,
                ty,
                tw,
                th,
                0x003A3A3A,
            );

            if let Some(entry) = self.thumbnail_cache.get(&idx) {
                let ox = tx + ((tw.saturating_sub(entry.width)) / 2) as i32;
                let oy = ty + ((th.saturating_sub(entry.height)) / 2) as i32;

                for r in 0..entry.height {
                    let py = oy + (r as i32);
                    if py < (top_bars_h as i32) || py >= (bottom_limit as i32) {
                        continue;
                    }
                    for c in 0..entry.width {
                        let px = ox + (c as i32);
                        if px < (sidebar_w as i32) || px >= (buf_w as i32) {
                            continue;
                        }
                        buffer[(py as u32 * buf_w + px as u32) as usize] =
                            entry.pixels[(r * entry.width + c) as usize];
                    }
                }
            }

            // Border highlight for selected page vs hovered page vs normal page
            let border_color = if idx == self.current_page_index {
                0x0042A5F5 // Bright blue for selected page
            } else if self.hovered_thumb == Some(idx) {
                0x00FFCA28 // Amber for hovered page
            } else {
                0x00555555 // Subtle border
            };

            if tx >= 0 && ty >= (top_bars_h as i32) {
                draw_rect_border(
                    buffer,
                    buf_w,
                    bottom_limit,
                    tx as u32,
                    ty as u32,
                    tw,
                    th,
                    border_color,
                );
                if idx == self.current_page_index && tw > 2 && th > 2 {
                    draw_rect_border(
                        buffer,
                        buf_w,
                        bottom_limit,
                        (tx as u32) + 1,
                        (ty as u32) + 1,
                        tw - 2,
                        th - 2,
                        border_color,
                    );
                }
            }

            // Top-left `[BW]` Grayscale conversion badge on each thumbnail
            let bw_badge_w = 26u32;
            let bw_badge_h = 16u32;
            let bwx = tx;
            let bwy = ty;
            let is_bw = self.bw_pages.contains(&idx);
            if bwx >= (sidebar_w as i32)
                && bwy >= (top_bars_h as i32)
                && (bwy as u32 + bw_badge_h) < bottom_limit
            {
                let bg = if is_bw { 0x00455A64 } else { 0x00263238 };
                let fg = if is_bw { 0x0080DEEA } else { 0x00E0E0E0 };
                fill_rect_clipped(
                    buffer,
                    buf_w,
                    bottom_limit,
                    bwx,
                    bwy,
                    bw_badge_w,
                    bw_badge_h,
                    bg,
                );
                draw_rect_border(
                    buffer,
                    buf_w,
                    bottom_limit,
                    bwx as u32,
                    bwy as u32,
                    bw_badge_w,
                    bw_badge_h,
                    0x0090A4AE,
                );
                draw_bitmap_str_8x8(
                    buffer,
                    buf_w,
                    bottom_limit,
                    (bwx as u32) + 5,
                    (bwy as u32) + 4,
                    "BW",
                    fg,
                    1,
                );
            }

            // Top-right `[R]` rotate badge on each thumbnail
            let badge_w = 24u32;
            let badge_h = 16u32;
            let bx = tx + (tw as i32) - (badge_w as i32);
            let by = ty;
            if bx >= (sidebar_w as i32)
                && by >= (top_bars_h as i32)
                && (by as u32 + badge_h) < bottom_limit
            {
                fill_rect_clipped(
                    buffer,
                    buf_w,
                    bottom_limit,
                    bx,
                    by,
                    badge_w,
                    badge_h,
                    0x00263238,
                );
                draw_rect_border(
                    buffer,
                    buf_w,
                    bottom_limit,
                    bx as u32,
                    by as u32,
                    badge_w,
                    badge_h,
                    0x0090A4AE,
                );
                draw_bitmap_str_8x8(
                    buffer,
                    buf_w,
                    bottom_limit,
                    (bx as u32) + 4,
                    (by as u32) + 4,
                    "R",
                    0x00FFEB3B,
                    1,
                );
            }

            // Page number label centered beneath thumbnail (shows Logical + Physical when roman_end > 0)
            let label = if self.roman_end > 0 {
                let logical =
                    PdfEngine::format_logical_page_label(idx as usize, self.roman_end);
                if is_bw {
                    format!("P.{} ({}) BW", logical, idx + 1)
                } else {
                    format!("Page {} ({})", logical, idx + 1)
                }
            } else if is_bw {
                format!("Page {} [BW]", idx + 1)
            } else {
                format!("Page {}", idx + 1)
            };
            let label_w = (label.len() as u32) * 8;
            let lx = (tx + ((tw as i32 - label_w as i32) / 2)).max(sidebar_w as i32);
            let ly = ty + (th as i32) + 6;
            if ly >= (top_bars_h as i32) && (ly as u32 + 10) < bottom_limit {
                let text_color = if idx == self.current_page_index {
                    0x0064B5F6
                } else {
                    0x00E0E0E0
                };
                draw_bitmap_str_8x8(
                    buffer,
                    buf_w,
                    bottom_limit,
                    lx as u32,
                    ly as u32,
                    &label,
                    text_color,
                    1,
                );
            }
        }

        // If user is dragging a page thumbnail to reorder, draw a bright blue vertical insertion bar
        if let Some((src_idx, sx, sy)) = self.grid_drag_page {
            let (cx, cy) = self.cursor_pos;
            let dist = ((cx - sx).powi(2) + (cy - sy).powi(2)).sqrt();
            if dist > 6.0 {
                let target_idx = self.grid_drop_target_index(cx, cy, buf_w, sidebar_w, top_bars_h);
                if target_idx != src_idx {
                    let (tx, ty, _tw, th) =
                        self.thumbnail_screen_rect(target_idx, buf_w, sidebar_w, top_bars_h);
                    let bar_x = (tx - 8).max(sidebar_w as i32);
                    fill_rect_clipped(
                        buffer,
                        buf_w,
                        bottom_limit,
                        bar_x,
                        ty - 4,
                        4,
                        th + 8,
                        0x0000E5FF,
                    );
                }
            }
        }
    }

    fn draw_bookmark_sidebar(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        scale_factor: f64,
    ) {
        let scale = (scale_factor.round() as u32).clamp(1, 2);
        let row_h = 24 * scale;
        let header_h = 54 * scale;
        let top_y = top_bars_h;

        // Fill sidebar background
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            0,
            top_y as i32,
            sidebar_w,
            buf_h.saturating_sub(top_y),
            SIDEBAR_BG_U32,
        );
        // Right separator line
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            (sidebar_w.saturating_sub(1)) as i32,
            top_y as i32,
            1,
            buf_h.saturating_sub(top_y),
            0x00455A64,
        );

        // Header bar + clickable `[Import .txt]` button
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            0,
            top_y as i32,
            sidebar_w,
            header_h,
            0x0014171C,
        );
        draw_bitmap_str_8x8(
            buffer,
            buf_w,
            buf_h,
            8 * scale,
            top_y + 7 * scale,
            "BOOKMARKS (+/E/Del)",
            0x0064B5F6,
            scale,
        );

        // `[Import .txt (L)]` button inside header
        let btn_x = 8 * scale;
        let btn_y = top_y + 24 * scale;
        let btn_w = sidebar_w.saturating_sub(16 * scale);
        let btn_h = 22 * scale;
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            btn_x as i32,
            btn_y as i32,
            btn_w,
            btn_h,
            0x00263238,
        );
        draw_rect_border(buffer, buf_w, buf_h, btn_x, btn_y, btn_w, btn_h, 0x004FC3F7);
        draw_bitmap_str_8x8(
            buffer,
            buf_w,
            buf_h,
            btn_x + 8 * scale,
            btn_y + 6 * scale,
            "[Import .txt] (L)",
            0x0080DEEA,
            scale,
        );

        if self.bookmarks.is_empty() {
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                10 * scale,
                top_y + header_h + 14 * scale,
                "No bookmarks yet.",
                0x009E9E9E,
                scale,
            );
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                10 * scale,
                top_y + header_h + 30 * scale,
                "Press '+' to add or",
                0x009E9E9E,
                scale,
            );
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                10 * scale,
                top_y + header_h + 46 * scale,
                "'L' to import .txt",
                0x009E9E9E,
                scale,
            );
            return;
        }

        for (vis_i, (bm_idx, bm)) in self
            .bookmarks
            .iter()
            .enumerate()
            .skip(self.bookmark_scroll_y)
            .enumerate()
        {
            let ry = top_y + header_h + (vis_i as u32) * row_h;
            if ry + row_h > buf_h {
                break;
            }

            let is_selected = self.selected_bookmark == Some(bm_idx);
            let is_current_page = bm.page_index == (self.current_page_index as usize);
            let row_bg = if is_selected {
                0x002C3E50
            } else if is_current_page {
                0x00263238
            } else {
                SIDEBAR_BG_U32
            };

            fill_rect_clipped(
                buffer,
                buf_w,
                buf_h,
                0,
                ry as i32,
                sidebar_w.saturating_sub(1),
                row_h,
                row_bg,
            );

            // Indent level == 1 sub-items by 16 * scale pixels
            let indent_px = if bm.level > 0 { 16 * scale } else { 6 * scale };
            let avail_px = sidebar_w.saturating_sub(indent_px + 30 * scale);
            let max_chars = (avail_px / (8 * scale)).max(5) as usize;

            let prefix = if bm.serial.is_empty() {
                String::new()
            } else {
                format!("{}. ", bm.serial)
            };
            let page_tag = if !bm.logical_page.is_empty() {
                format!("[p.{}]", bm.logical_page)
            } else {
                format!("[p.{}]", bm.page_index + 1)
            };

            let mut label = format!("{}{} {}", prefix, bm.title, page_tag);
            if label.chars().count() > max_chars {
                let title_budget =
                    max_chars.saturating_sub(prefix.chars().count() + page_tag.chars().count() + 1);
                let short_title: String = bm.title.chars().take(title_budget.max(3)).collect();
                label = format!("{}{} {}", prefix, short_title, page_tag);
                if label.chars().count() > max_chars {
                    label = label.chars().take(max_chars).collect();
                }
            }

            let text_color = if is_current_page {
                0x00FFEB3B
            } else if bm.level == 0 {
                0x00FFFFFF // Bright white for top-level chapters
            } else {
                0x0080DEEA // Soft cyan/gray for indented sub-items
            };
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                indent_px,
                ry + 7 * scale,
                &label,
                text_color,
                scale,
            );

            // Draw `[x]` delete button on right side of bookmark row
            let del_x = sidebar_w.saturating_sub(26 * scale);
            draw_bitmap_str_8x8(
                buffer,
                buf_w,
                buf_h,
                del_x,
                ry + 7 * scale,
                "[x]",
                0x00EF5350,
                scale,
            );
        }
    }

    /// Hit-tests a click at `(mx, my)` inside the Left Bookmark Sidebar.
    fn hit_test_bookmark_sidebar(
        &self,
        mx: f64,
        my: f64,
        sidebar_w: u32,
        top_bars_h: u32,
        scale_factor: f64,
    ) -> Option<SidebarHit> {
        if !self.show_bookmarks || (mx as u32) >= sidebar_w || (my as u32) < top_bars_h {
            return None;
        }
        let scale = (scale_factor.round() as u32).clamp(1, 2);
        let row_h = 24 * scale;
        let header_h = 54 * scale;
        let rel_y = (my.round() as u32).saturating_sub(top_bars_h);
        if rel_y < header_h {
            // Check if clicked inside `[Import .txt]` button (`22 * scale..48 * scale`)
            if rel_y >= 20 * scale && rel_y <= 50 * scale {
                return Some(SidebarHit::ImportTxtButton);
            }
            return None;
        }
        let vis_row = ((rel_y - header_h) / row_h) as usize;
        let bm_idx = self.bookmark_scroll_y + vis_row;
        if bm_idx < self.bookmarks.len() {
            let del_x = sidebar_w.saturating_sub(30 * scale);
            let on_delete = (mx.round() as u32) >= del_x;
            Some(SidebarHit::BookmarkRow {
                index: bm_idx,
                on_delete,
            })
        } else {
            None
        }
    }

    fn persist_bookmarks(&self) {
        let _ = PdfEngine::save_bookmarks_sidecar(&self.file_path, &self.bookmarks);
    }

    fn set_view_mode(&mut self, mode: ViewMode) {
        if self.view_mode == mode {
            return;
        }
        if mode == ViewMode::ThumbnailGrid {
            if self.view_mode.is_page_view() {
                self.last_page_view_mode = self.view_mode;
            }
            self.invalidate_page_caches();
            self.view_mode = ViewMode::ThumbnailGrid;
            self.status_notice = Some(
                "Thumbnail Grid View (Click page, Drag reorder, [BW]/[R] badges, Tab/Enter to view)"
                    .to_string(),
            );
        } else {
            // Free thumbnail cache immediately when returning to Page View
            self.thumbnail_cache.clear();
            self.thumbnail_cache.shrink_to_fit();
            self.grid_drag_page = None;
            self.hovered_thumb = None;
            self.view_mode = mode;
            self.last_page_view_mode = mode;
            self.scroll_y = 0.0;
            self.invalidate_page_caches();
            self.status_notice = Some(match mode {
                ViewMode::FitPage => "View Mode: Fit Full Page (1)".to_string(),
                ViewMode::FitWidth => "View Mode: Fit Page Width (2) — Scroll vertically".to_string(),
                ViewMode::FitTextWidth => {
                    "View Mode: Fit Text Width (3) — Auto-trimmed margins".to_string()
                }
                ViewMode::TwoPageSpread => {
                    "View Mode: Two-Page Side-by-Side Spread (4)".to_string()
                }
                ViewMode::ThumbnailGrid => "Thumbnail Grid View".to_string(),
            });
        }
        self.update_title_bar();
        self.request_redraw();
    }

    fn toggle_view_mode(&mut self) {
        if self.view_mode == ViewMode::ThumbnailGrid {
            let restore = if self.last_page_view_mode.is_page_view() {
                self.last_page_view_mode
            } else {
                ViewMode::FitPage
            };
            self.set_view_mode(restore);
        } else {
            self.set_view_mode(ViewMode::ThumbnailGrid);
        }
    }

    /// Resolves which page (`current_page_index` vs `current_page_index + 1` in `TwoPageSpread`)
    /// and its corresponding screen `draw_rect` contains `(sx, sy)`.
    fn resolve_target_page_and_draw_rect(&self, sx: f64, _sy: f64) -> (u16, (i32, i32, u32, u32)) {
        if self.view_mode == ViewMode::TwoPageSpread {
            if let (Some(second_cache), Some(second_rect)) =
                (&self.cached_second_page, self.second_page_draw_rect)
            {
                let (rx, _ry, _rw, _rh) = second_rect;
                if (sx.round() as i32) >= rx - (SPREAD_GAP_PX as i32 / 2) {
                    return (second_cache.page_index, second_rect);
                }
            }
        }
        (self.current_page_index, self.page_draw_rect)
    }

    /// Converts screen drag start `(sx1, sy1)` and end `(sx2, sy2)` into `(page_idx, PdfRect)`.
    /// If `allow_single_click` is true (used for Stamp Image mode), a simple click produces a
    /// default-sized stamp box centered at `(sx1, sy1)`.
    fn screen_drag_to_pdf_rect(
        &self,
        sx1: f64,
        sy1: f64,
        sx2: f64,
        sy2: f64,
        allow_single_click: bool,
    ) -> Option<(u16, PdfRect)> {
        let doc = self.doc.as_ref()?;
        let (page_idx, (px, py, pw, ph)) = self.resolve_target_page_and_draw_rect(sx1, sy1);
        let page = doc.pages().get(page_idx).ok()?;
        if pw == 0 || ph == 0 {
            return None;
        }

        let mut bx1 = (sx1 - px as f64).clamp(0.0, pw as f64) as i32;
        let mut by1 = (sy1 - py as f64).clamp(0.0, ph as f64) as i32;
        let mut bx2 = (sx2 - px as f64).clamp(0.0, pw as f64) as i32;
        let mut by2 = (sy2 - py as f64).clamp(0.0, ph as f64) as i32;

        if (bx2 - bx1).abs() < 4 && (by2 - by1).abs() < 4 {
            if !allow_single_click {
                return None;
            }
            // Default stamp size (~22% of page width, preserving stamp aspect ratio)
            let aspect = self
                .pending_stamp_image
                .as_ref()
                .map(|s| (s.height as f32 / (s.width.max(1) as f32)).clamp(0.15, 4.0))
                .unwrap_or(0.45);
            let default_w = ((pw as f32) * 0.22).max(40.0) as i32;
            let default_h = ((default_w as f32) * aspect).max(20.0) as i32;
            let cx = bx1;
            let cy = by1;
            bx1 = (cx - default_w / 2).clamp(0, pw as i32);
            bx2 = (cx + default_w / 2).clamp(0, pw as i32);
            by1 = (cy - default_h / 2).clamp(0, ph as i32);
            by2 = (cy + default_h / 2).clamp(0, ph as i32);
        }

        let config = PdfRenderConfig::new()
            .set_fixed_width(pw as i32)
            .set_fixed_height(ph as i32);

        let (pt1_x, pt1_y, pt2_x, pt2_y) = match (
            page.pixels_to_points(bx1, by1, &config),
            page.pixels_to_points(bx2, by2, &config),
        ) {
            (Ok((x1, y1)), Ok((x2, y2))) => (x1.value, y1.value, x2.value, y2.value),
            _ => {
                let page_w = page.width().value;
                let page_h = page.height().value;
                let u1 = (bx1 as f32) / (pw as f32);
                let v1 = (by1 as f32) / (ph as f32);
                let u2 = (bx2 as f32) / (pw as f32);
                let v2 = (by2 as f32) / (ph as f32);
                (
                    u1 * page_w,
                    (1.0 - v1) * page_h,
                    u2 * page_w,
                    (1.0 - v2) * page_h,
                )
            }
        };

        let bottom = pt1_y.min(pt2_y);
        let top = pt1_y.max(pt2_y);
        let left = pt1_x.min(pt2_x);
        let right = pt1_x.max(pt2_x);

        Some((page_idx, PdfRect::new_from_values(bottom, left, top, right)))
    }

    fn navigate_to_page(&mut self, new_index: u16) {
        let total = self.total_pages();
        if total == 0 {
            return;
        }
        let clamped = new_index.min(total - 1);
        if clamped != self.current_page_index {
            self.current_page_index = clamped;
            self.scroll_y = 0.0;
            self.invalidate_page_caches();
            self.status_notice = None;
            self.update_title_bar();
            self.request_redraw();
        }
    }

    fn step_pages_forward(&mut self) {
        let total = self.total_pages();
        if total == 0 {
            return;
        }
        let step = if self.view_mode == ViewMode::TwoPageSpread {
            2
        } else {
            1
        };
        if self.current_page_index + step < total {
            self.navigate_to_page(self.current_page_index + step);
        } else if self.current_page_index + 1 < total {
            self.navigate_to_page(total - 1);
        }
    }

    fn step_pages_backward(&mut self) {
        let step = if self.view_mode == ViewMode::TwoPageSpread {
            2
        } else {
            1
        };
        if self.current_page_index > 0 {
            self.navigate_to_page(self.current_page_index.saturating_sub(step));
        }
    }

    /// Scrolls vertically in `FitWidth` / `FitTextWidth` when the page bitmap exceeds the viewport height,
    /// seamlessly transitioning to the next/previous page when scrolling past the bottom/top!
    fn scroll_or_turn_page_vertical(&mut self, scroll_delta_px: f32) -> bool {
        if matches!(self.view_mode, ViewMode::FitWidth | ViewMode::FitTextWidth) {
            if let Some(cache) = &self.cached_page {
                let bmp_h = cache.bitmap_height;
                let content_h = cache.viewport_height;
                if bmp_h > content_h {
                    let max_scroll = (bmp_h - content_h) as f32;
                    let new_scroll = self.scroll_y + scroll_delta_px;
                    if scroll_delta_px > 0.0 {
                        if self.scroll_y < max_scroll - 1.0 {
                            self.scroll_y = new_scroll.min(max_scroll);
                            self.request_redraw();
                            return true;
                        } else if self.current_page_index + 1 < self.total_pages() {
                            self.current_page_index += 1;
                            self.scroll_y = 0.0;
                            self.invalidate_page_caches();
                            self.update_title_bar();
                            self.request_redraw();
                            return true;
                        }
                    } else if scroll_delta_px < 0.0 {
                        if self.scroll_y > 1.0 {
                            self.scroll_y = new_scroll.max(0.0);
                            self.request_redraw();
                            return true;
                        } else if self.current_page_index > 0 {
                            self.current_page_index -= 1;
                            self.scroll_y = 99999.0; // Clamped to bottom of previous page on render
                            self.invalidate_page_caches();
                            self.update_title_bar();
                            self.request_redraw();
                            return true;
                        }
                    }
                    return true;
                }
            }
        }
        false
    }

    fn rotate_page_clockwise(&mut self, page_idx: u16) {
        self.push_undo_snapshot();
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let Ok(mut page) = doc.pages_mut().get(page_idx) else {
            return;
        };
        let next_rot = match page.rotation().unwrap_or(PdfPageRenderRotation::None) {
            PdfPageRenderRotation::None => PdfPageRenderRotation::Degrees90,
            PdfPageRenderRotation::Degrees90 => PdfPageRenderRotation::Degrees180,
            PdfPageRenderRotation::Degrees180 => PdfPageRenderRotation::Degrees270,
            PdfPageRenderRotation::Degrees270 => PdfPageRenderRotation::None,
        };
        page.set_rotation(next_rot);
        self.invalidate_page_caches();
        self.thumbnail_cache.remove(&page_idx);
        self.status_notice = Some(format!("Rotated page {} by 90°", page_idx + 1));
        self.update_title_bar();
        self.request_redraw();
    }

    fn convert_page_to_bw_in_place(&mut self, page_idx: u16) {
        self.push_undo_snapshot();
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let bytes = match doc.save_to_bytes() {
            Ok(b) => b,
            Err(e) => {
                self.status_notice = Some(format!("BW serialize error: {e}"));
                self.update_title_bar();
                return;
            }
        };
        match self.engine.convert_page_to_bw(&bytes, page_idx) {
            Ok(bw_bytes) => match self.engine.load_pdf_from_bytes_safe(bw_bytes) {
                Ok(new_doc) => {
                    self.doc = Some(new_doc);
                    self.bw_pages.insert(page_idx);
                    self.invalidate_page_caches();
                    self.thumbnail_cache.remove(&page_idx);
                    self.status_notice = Some(format!(
                        "Converted Page {} to Grayscale [BW] (Cmd/Ctrl+Z to undo)",
                        page_idx + 1
                    ));
                    self.update_title_bar();
                    self.request_redraw();
                }
                Err(e) => {
                    self.status_notice = Some(format!("BW reload error: {e}"));
                    self.update_title_bar();
                }
            },
            Err(e) => {
                self.status_notice = Some(format!("BW conversion error: {e}"));
                self.update_title_bar();
            }
        }
    }

    fn reset_current_page_crop(&mut self) {
        self.push_undo_snapshot();
        let page_idx = self.current_page_index;
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        match self.engine.reset_page_crop(doc, page_idx) {
            Ok(bytes) => match self.engine.load_pdf_from_bytes_safe(bytes) {
                Ok(new_doc) => {
                    self.doc = Some(new_doc);
                    self.invalidate_page_caches();
                    self.thumbnail_cache.remove(&page_idx);
                    self.status_notice = Some(format!(
                        "Reset crop on Page {} to full MediaBox",
                        page_idx + 1
                    ));
                    self.update_title_bar();
                    self.request_redraw();
                }
                Err(e) => {
                    self.status_notice = Some(format!("Reset crop reload error: {e}"));
                    self.update_title_bar();
                }
            },
            Err(e) => {
                let _ = self.undo_stack.pop_back();
                self.status_notice = Some(format!("Reset crop failed: {e}"));
                self.update_title_bar();
            }
        }
    }

    fn flatten_and_unlock_current_pdf(&mut self) {
        // v1.1: signed documents need a second press, because flattening and removing
        // /Perms invalidates digital signatures.
        let sig_count = self.doc.as_ref().map(|d| d.signatures().len()).unwrap_or(0);
        if sig_count > 0 {
            let confirmed = self
                .flatten_confirm_at
                .is_some_and(|t| t.elapsed() <= FLATTEN_CONFIRM_WINDOW);
            if !confirmed {
                self.flatten_confirm_at = Some(Instant::now());
                self.status_notice = Some(format!(
                    "This PDF has {sig_count} digital signature(s). Flatten & Unlock will INVALIDATE them. Press U again within 5 s to continue."
                ));
                self.update_title_bar();
                self.request_redraw();
                return;
            }
        }
        self.flatten_confirm_at = None;

        self.push_undo_snapshot();
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        match self.engine.flatten_and_unlock_doc(doc) {
            Ok(report) => match self.engine.load_pdf_from_bytes_safe(report.bytes) {
                Ok(new_doc) => {
                    self.doc = Some(new_doc);
                    self.invalidate_page_caches();
                    self.thumbnail_cache.clear();
                    let mut notice = String::from("Flattened annotations/forms");
                    if report.permissions_removed {
                        notice.push_str(" & removed permission restrictions");
                    } else {
                        notice.push_str("; WARNING: permission restrictions could NOT be removed");
                    }
                    if !report.failed_pages.is_empty() {
                        notice.push_str(&format!(
                            "; WARNING: {} page(s) failed to flatten (first: p.{})",
                            report.failed_pages.len(),
                            report.failed_pages[0]
                        ));
                    }
                    notice.push_str(" (Press 'S' to save)");
                    self.status_notice = Some(notice);
                    self.update_title_bar();
                    self.request_redraw();
                }
                Err(e) => {
                    self.status_notice = Some(format!("Flatten reload error: {e}"));
                    self.update_title_bar();
                }
            },
            Err(e) => {
                let _ = self.undo_stack.pop_back();
                self.status_notice = Some(format!("Flatten failed: {e}"));
                self.update_title_bar();
            }
        }
    }

    fn delete_current_page(&mut self) {
        let total = self.total_pages();
        if total <= 1 {
            self.status_notice = Some("Cannot delete the only remaining page".to_string());
            self.update_title_bar();
            return;
        }
        self.push_undo_snapshot();
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        if let Ok(page) = doc.pages().get(self.current_page_index) {
            if page.delete().is_ok() {
                let new_total = doc.pages().len();
                if self.current_page_index >= new_total && new_total > 0 {
                    self.current_page_index = new_total - 1;
                }
                self.invalidate_page_caches();
                self.thumbnail_cache.clear();
                self.status_notice = Some("Deleted page (Cmd/Ctrl+Z to undo)".to_string());
                self.update_title_bar();
                self.request_redraw();
            }
        }
    }

    fn insert_blank_page_after_current(&mut self) {
        self.push_undo_snapshot();
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let target_idx = (self.current_page_index + 1).min(doc.pages().len());
        match PdfEngine::insert_blank_page_in_doc(doc, target_idx) {
            Ok(inserted_idx) => {
                self.current_page_index = inserted_idx;
                self.invalidate_page_caches();
                self.thumbnail_cache.clear();
                self.status_notice =
                    Some(format!("Inserted blank A4 page at #{}", inserted_idx + 1));
                self.update_title_bar();
                self.request_redraw();
            }
            Err(e) => {
                self.status_notice = Some(format!("Insert failed: {e}"));
                self.update_title_bar();
            }
        }
    }

    fn merge_pdf_dialog(&mut self) {
        let Some(second_path) = rfd::FileDialog::new()
            .set_title("Select PDF to Merge")
            .add_filter("PDF Document", &["pdf"])
            .pick_file()
        else {
            return;
        };

        let second_doc = match self.engine.load_pdf_safe(&second_path) {
            Ok(d) => d,
            Err(e) => {
                self.status_notice = Some(format!("Merge load failed: {e}"));
                self.update_title_bar();
                return;
            }
        };

        self.push_undo_snapshot();
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        match doc.pages_mut().append(&second_doc) {
            Ok(()) => {
                self.invalidate_page_caches();
                self.thumbnail_cache.clear();
                self.status_notice = Some("Merged successfully!".to_string());
                self.update_title_bar();
                self.request_redraw();
            }
            Err(e) => {
                self.status_notice = Some(format!("Merge failed: {e}"));
                self.update_title_bar();
            }
        }
    }

    /// Batch Combine Multiple PDFs (`Shift+M` or `File -> Combine Multiple PDFs...`).
    fn combine_multiple_pdfs_dialog(&mut self) {
        let Some(paths) = rfd::FileDialog::new()
            .set_title("Select 2 or More PDFs to Combine in Order")
            .add_filter("PDF", &["pdf"])
            .pick_files()
        else {
            return;
        };

        if paths.len() < 2 {
            self.status_notice =
                Some("Select at least 2 PDF files to combine (Shift+M)".to_string());
            self.update_title_bar();
            self.request_redraw();
            return;
        }

        let num_files = paths.len();
        match self.engine.combine_multiple_pdfs(&paths) {
            Ok((combined_bytes, total_pages)) => {
                let virtual_path = match paths[0].parent() {
                    Some(p) if !p.as_os_str().is_empty() => p.join("Combined.pdf"),
                    _ => PathBuf::from("Combined.pdf"),
                };
                match self.open_bytes_in_new_tab(combined_bytes, virtual_path) {
                    Ok(()) => {
                        self.status_notice = Some(format!(
                            "Combined {num_files} PDFs ({total_pages} pages) into 'Combined.pdf' (Press 'S' to save)"
                        ));
                    }
                    Err(e) => {
                        self.status_notice = Some(format!("Failed to open Combined.pdf tab: {e}"));
                    }
                }
            }
            Err(e) => {
                self.status_notice = Some(format!("Combine PDFs failed: {e}"));
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    /// Opens native file dialog to choose a PNG/JPG signature or image stamp (`P` key).
    fn pick_stamp_image_dialog(&mut self) {
        let Some(img_path) = rfd::FileDialog::new()
            .set_title("Select Signature or Image Stamp (PNG with transparency or JPG)")
            .add_filter("Images", &["png", "jpg", "jpeg"])
            .pick_file()
        else {
            return;
        };

        let fname = img_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("stamp.png")
            .to_string();

        match PdfEngine::load_stamp_image(&img_path) {
            Ok(buf) => {
                let w = buf.width;
                let h = buf.height;
                self.pending_stamp_image = Some(buf);
                self.mouse_mode = MouseMode::StampImage;
                self.status_notice = Some(format!(
                    "Loaded '{fname}' ({w}x{h}) — Click or Drag on page to stamp signature/image!"
                ));
            }
            Err(e) => {
                self.status_notice = Some(format!("Failed to load image '{fname}': {e}"));
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    fn execute_text_search(&mut self, query: &str) {
        let trimmed = query.trim().to_string();
        if trimmed.is_empty() {
            self.search_query.clear();
            self.search_matches.clear();
            self.current_search_match = 0;
            self.status_notice = Some("Cleared text search highlights".to_string());
            self.update_title_bar();
            self.request_redraw();
            return;
        }

        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let matches = PdfEngine::search_text_in_doc(doc, &trimmed);
        self.search_query = trimmed.clone();
        if matches.is_empty() {
            self.search_matches.clear();
            self.current_search_match = 0;
            self.status_notice = Some(format!("No matches found for '{trimmed}'"));
            self.update_title_bar();
            self.request_redraw();
            return;
        }

        // Start from the first match on or after `self.current_page_index`
        let start_idx = matches
            .iter()
            .position(|m| m.page_index >= self.current_page_index)
            .unwrap_or(0);
        let target_page = matches[start_idx].page_index;
        let total_matches = matches.len();
        self.search_matches = matches;
        self.current_search_match = start_idx;

        if target_page != self.current_page_index {
            self.current_page_index = target_page;
            self.scroll_y = 0.0;
            self.invalidate_page_caches();
        }
        self.status_notice = Some(format!(
            "Match {}/{} for '{}' on Page {} (Press F3 or Cmd/Ctrl+G for next)",
            start_idx + 1,
            total_matches,
            trimmed,
            target_page + 1
        ));
        self.update_title_bar();
        self.request_redraw();
    }

    fn jump_to_next_search_match(&mut self) {
        if self.search_matches.is_empty() {
            self.input_mode = InputMode::SearchText {
                query_input: self.search_query.clone(),
            };
            self.update_title_bar();
            self.request_redraw();
            return;
        }
        self.current_search_match = (self.current_search_match + 1) % self.search_matches.len();
        let target_page = self.search_matches[self.current_search_match].page_index;
        if target_page != self.current_page_index {
            self.current_page_index = target_page;
            self.scroll_y = 0.0;
            self.invalidate_page_caches();
        }
        self.status_notice = Some(format!(
            "Match {}/{} for '{}' on Page {}",
            self.current_search_match + 1,
            self.search_matches.len(),
            self.search_query,
            target_page + 1
        ));
        self.update_title_bar();
        self.request_redraw();
    }

    fn execute_menu_item(&mut self, cat: MenuCategory, item_idx: usize) {
        self.open_menu = None;
        match (cat, item_idx) {
            // File menu
            (MenuCategory::File, 0) => {
                self.pending_dialog = Some(PendingDialog::Open);
            }
            (MenuCategory::File, 1) => {
                self.pending_dialog = Some(PendingDialog::CombineMultiple);
            }
            (MenuCategory::File, 2) => {
                self.pending_dialog = Some(PendingDialog::ImportBookmarksTxt);
            }
            (MenuCategory::File, 3) => {
                self.pending_dialog = Some(PendingDialog::Save);
            }
            (MenuCategory::File, 4) => {
                self.compress_current_pdf();
            }
            (MenuCategory::File, 5) => {
                if self.tabs.len() > 1 {
                    let _ = self.close_tab(self.active_tab);
                } else {
                    self.status_notice = Some("Cannot close the last remaining tab".to_string());
                }
            }

            // View menu
            (MenuCategory::View, 0) => self.set_view_mode(ViewMode::FitPage),
            (MenuCategory::View, 1) => self.set_view_mode(ViewMode::FitWidth),
            (MenuCategory::View, 2) => self.set_view_mode(ViewMode::FitTextWidth),
            (MenuCategory::View, 3) => self.set_view_mode(ViewMode::TwoPageSpread),
            (MenuCategory::View, 4) => self.toggle_view_mode(),
            (MenuCategory::View, 5) => {
                self.show_bookmarks = !self.show_bookmarks;
                self.invalidate_page_caches();
                self.status_notice = Some(if self.show_bookmarks {
                    "Bookmarks Sidebar ON".to_string()
                } else {
                    "Bookmarks Sidebar OFF".to_string()
                });
            }
            (MenuCategory::View, 6) => {
                self.reading_filter = self.reading_filter.next();
                self.status_notice = Some(format!(
                    "Reading Filter: {} (Zero-RAM CPU Blit Filter)",
                    self.reading_filter.label()
                ));
            }

            // Edit menu
            (MenuCategory::Edit, 0) => self.perform_undo(),
            (MenuCategory::Edit, 1) => {
                self.input_mode = InputMode::SearchText {
                    query_input: self.search_query.clone(),
                };
            }
            (MenuCategory::Edit, 2) => {
                self.mouse_mode = MouseMode::Highlight;
                self.status_notice =
                    Some("Mouse Mode: Highlight Text (Drag across text)".to_string());
            }
            (MenuCategory::Edit, 3) => {
                self.mouse_mode = MouseMode::Redact;
                self.status_notice = Some(
                    "Mouse Mode: Black-Box Redact (Drag red box to permanently purge & black out)"
                        .to_string(),
                );
            }
            (MenuCategory::Edit, 4) => {
                self.mouse_mode = MouseMode::Crop;
                self.status_notice = Some(
                    "Mouse Mode: Crop Page Area (Drag green box to crop page)".to_string(),
                );
            }

            // Page menu
            (MenuCategory::Page, 0) => {
                self.input_mode = InputMode::JumpToPage {
                    input: String::new(),
                };
            }
            (MenuCategory::Page, 1) => {
                let target = if self.view_mode == ViewMode::ThumbnailGrid {
                    self.hovered_thumb.unwrap_or(self.current_page_index)
                } else {
                    self.current_page_index
                };
                self.rotate_page_clockwise(target);
            }
            (MenuCategory::Page, 2) => self.insert_blank_page_after_current(),
            (MenuCategory::Page, 3) => self.delete_current_page(),
            (MenuCategory::Page, 4) => {
                let target = if self.view_mode == ViewMode::ThumbnailGrid {
                    self.hovered_thumb.unwrap_or(self.current_page_index)
                } else {
                    self.current_page_index
                };
                self.convert_page_to_bw_in_place(target);
            }
            (MenuCategory::Page, 5) => self.reset_current_page_crop(),

            // Tools menu
            (MenuCategory::Tools, 0) => {
                self.input_mode = InputMode::PageNumbers {
                    input: String::new(),
                };
            }
            (MenuCategory::Tools, 1) => {
                self.input_mode = InputMode::HeaderFooterRange {
                    range_input: String::new(),
                };
            }
            (MenuCategory::Tools, 2) => {
                self.input_mode = InputMode::ClearMargins {
                    input: String::new(),
                };
            }
            (MenuCategory::Tools, 3) => {
                self.input_mode = InputMode::Watermark {
                    input: String::new(),
                };
            }
            (MenuCategory::Tools, 4) => {
                self.pending_dialog = Some(PendingDialog::PickStampImage);
            }
            (MenuCategory::Tools, 5) => {
                self.input_mode = InputMode::SplitRange {
                    input: String::new(),
                };
            }
            (MenuCategory::Tools, 6) => self.flatten_and_unlock_current_pdf(),

            // Help menu
            (MenuCategory::Help, 0) => {
                self.show_help_overlay = !self.show_help_overlay;
            }
            _ => {}
        }
        self.update_title_bar();
        self.request_redraw();
    }

    fn execute_split_range(&mut self, range_spec: &str) {
        let trimmed = range_spec.trim();
        if trimmed.is_empty() {
            self.status_notice = Some("Split cancelled (empty range)".to_string());
            self.update_title_bar();
            return;
        }

        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let stem = self
            .file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document");
        let default_name = format!("{stem}_split.pdf");

        let save_path = rfd::FileDialog::new()
            .set_title("Save Extracted/Split PDF")
            .add_filter("PDF Document", &["pdf"])
            .set_file_name(&default_name)
            .save_file()
            .unwrap_or_else(|| PathBuf::from(&default_name));

        match self
            .engine
            .extract_page_spec_from_doc(doc, trimmed, &save_path)
        {
            Ok(count) => {
                let saved_name = save_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&default_name);
                self.status_notice =
                    Some(format!("Extracted {count} page(s) to '{saved_name}'"));
            }
            Err(e) => {
                self.status_notice = Some(format!("Split error: {e}"));
            }
        }
        self.update_title_bar();
    }

    fn open_file_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("PDF Document", &["pdf"])
            .pick_file()
        {
            let fname = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("file.pdf")
                .to_string();
            match self.open_document_in_new_tab(path) {
                Ok(()) => {
                    self.status_notice = Some(format!(
                        "Opened '{fname}' in Tab {}",
                        self.active_tab + 1
                    ));
                }
                Err(e) => {
                    self.status_notice = Some(format!("ERROR opening '{fname}': {e}"));
                }
            }
            self.update_title_bar();
            self.request_redraw();
        }
    }

    fn import_bookmarks_from_path(&mut self, txt_path: &Path) {
        match std::fs::read_to_string(txt_path) {
            Ok(content) => {
                let total = self.total_pages() as usize;
                let parsed = PdfEngine::parse_bookmarks_txt(&content, self.roman_end, total);
                let count = parsed.len();
                self.bookmarks = parsed;
                self.selected_bookmark = if self.bookmarks.is_empty() {
                    None
                } else {
                    Some(0)
                };
                self.bookmark_scroll_y = 0;
                self.show_bookmarks = true;
                self.invalidate_page_caches();
                self.persist_bookmarks();
                self.status_notice = Some(format!("Imported {count} bookmarks!"));
                self.update_title_bar();
                self.request_redraw();
            }
            Err(e) => {
                self.status_notice = Some(format!("Failed to read bookmark .txt: {e}"));
                self.update_title_bar();
            }
        }
    }

    fn import_bookmarks_txt_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Import Bookmarks .txt (Serial -- Title -- LogicalPage)")
            .add_filter("Text File", &["txt"])
            .pick_file()
        {
            self.import_bookmarks_from_path(&path);
        }
    }

    fn save_file_dialog(&mut self) {
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let stem = self
            .file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document");
        let default_name = format!("{stem}_edited.pdf");

        let target_path = rfd::FileDialog::new()
            .add_filter("PDF Document", &["pdf"])
            .set_file_name(&default_name)
            .save_file();

        if let Some(save_path) = target_path {
            match PdfEngine::save_doc_with_metadata(
                doc,
                &save_path,
                self.roman_end,
                &self.bookmarks,
                self.redacted,
            ) {
                Ok(warning) => {
                    self.file_path = save_path.clone();
                    if self.active_tab < self.tabs.len() {
                        self.tabs[self.active_tab].file_path = save_path;
                    }
                    self.persist_bookmarks();
                    self.status_notice = Some(match warning {
                        None if self.redacted => {
                            "Saved PDF (redaction finalized: orphaned objects, /Info & thumbnails removed)".to_string()
                        }
                        None => "Saved PDF (sanitized metadata + embedded /PageLabels & /Outlines)".to_string(),
                        Some(w) => format!("Saved PDF, but WARNING: {w}"),
                    });
                }
                Err(e) => {
                    self.status_notice = Some(format!("Save error: {e}"));
                }
            }
            self.update_title_bar();
            self.request_redraw();
        }
    }

    fn compress_current_pdf(&mut self) {
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let stem = self
            .file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document");
        let out_name = format!("{stem}_compressed.pdf");
        let out_path = match self.file_path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.join(&out_name),
            _ => PathBuf::from(&out_name),
        };

        let disk_size = std::fs::metadata(&self.file_path)
            .map(|m| m.len() as usize)
            .unwrap_or(0);

        match self.engine.compress_doc(doc) {
            Ok((compressed_bytes, serialized_len, _)) => {
                let (final_bytes, warning) = match PdfEngine::embed_metadata_in_pdf_bytes(
                    &compressed_bytes,
                    self.roman_end,
                    &self.bookmarks,
                    self.redacted,
                ) {
                    Ok(b) => (b, None),
                    Err(e) if self.redacted => {
                        self.status_notice = Some(format!(
                            "Compression refused: post-redaction cleanup failed ({e}). Nothing was written."
                        ));
                        self.update_title_bar();
                        self.request_redraw();
                        return;
                    }
                    Err(e) => (compressed_bytes, Some(format!(" (WARNING: metadata not sanitized: {e})"))),
                };
                let new_len = final_bytes.len();
                let orig_len = disk_size.max(serialized_len);
                match PdfEngine::write_atomic(&out_path, &final_bytes) {
                    Ok(()) => {
                        let orig_mb = (orig_len as f64) / (1024.0 * 1024.0);
                        let new_mb = (new_len as f64) / (1024.0 * 1024.0);
                        let saved_pct = if orig_len > 0 && new_len < orig_len {
                            (((orig_len - new_len) as f64 / (orig_len as f64)) * 100.0).round()
                                as u32
                        } else {
                            0
                        };
                        self.status_notice = Some(format!(
                            "Compressed! {:.2} MB -> {:.2} MB (Saved {}%) -> '{}'{}",
                            orig_mb, new_mb, saved_pct, out_name, warning.unwrap_or_default()
                        ));
                    }
                    Err(e) => {
                        self.status_notice =
                            Some(format!("Failed to write compressed PDF '{}': {e}", out_name));
                    }
                }
            }
            Err(e) => {
                self.status_notice = Some(format!("Compression error: {e}"));
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    fn parse_page_range_input(&self, input: &str) -> Option<(u16, u16)> {
        let total = self.total_pages();
        if total == 0 {
            return None;
        }
        let s = input.trim();
        if s.is_empty() || s.eq_ignore_ascii_case("all") {
            return Some((1, total));
        }
        if let Some((a, b)) = s.split_once('-') {
            let start = a.trim().parse::<u16>().ok()?;
            let end = b.trim().parse::<u16>().ok()?;
            if start >= 1 && start <= end && end <= total {
                return Some((start, end));
            }
            return None;
        }
        let single = s.parse::<u16>().ok()?;
        if single >= 1 && single <= total {
            Some((single, single))
        } else {
            None
        }
    }

    fn handle_prompt_submission(&mut self) {
        let mode = std::mem::replace(&mut self.input_mode, InputMode::Normal);
        match mode {
            InputMode::Normal => {}
            InputMode::SearchText { query_input } => {
                self.execute_text_search(&query_input);
                return;
            }
            InputMode::JumpToPage { input } => {
                let total = self.total_pages() as usize;
                if let Some(target_idx) =
                    PdfEngine::resolve_logical_page_str(&input, self.roman_end, total)
                {
                    self.current_page_index = target_idx as u16;
                    self.scroll_y = 0.0;
                    self.invalidate_page_caches();
                    let lbl =
                        PdfEngine::format_logical_page_label(target_idx, self.roman_end);
                    self.status_notice = Some(format!(
                        "Jumped to Page {} (Physical {} of {})",
                        lbl,
                        target_idx + 1,
                        total
                    ));
                    self.request_redraw();
                } else {
                    self.status_notice = Some(format!(
                        "Invalid page '{}' (use logical#, Roman 'i..v', or '#physical')",
                        input.trim()
                    ));
                }
            }
            InputMode::PageNumbers { input } => {
                let trimmed = input.trim();
                let roman_end = if trimmed.is_empty() {
                    0
                } else {
                    match trimmed.parse::<u16>() {
                        Ok(n) => n,
                        Err(_) => {
                            self.status_notice =
                                Some("Invalid number for Roman page end".to_string());
                            self.update_title_bar();
                            self.request_redraw();
                            return;
                        }
                    }
                };
                self.push_undo_snapshot();
                let total = self.total_pages() as usize;
                if let Some(doc) = self.doc.as_mut() {
                    match PdfEngine::apply_page_numbers_to_doc(doc, roman_end) {
                        Ok(()) => {
                            self.roman_end = roman_end as usize;
                            PdfEngine::resync_bookmarks_logical_pages(
                                &mut self.bookmarks,
                                self.roman_end,
                                total,
                            );
                            self.persist_bookmarks();
                            self.invalidate_page_caches();
                            self.thumbnail_cache.clear();
                            self.status_notice = Some(format!(
                                "Stamped page numbers (Roman 1..={roman_end}) & synced bookmarks"
                            ));
                            self.request_redraw();
                        }
                        Err(e) => {
                            self.status_notice = Some(format!("Page numbering failed: {e}"));
                        }
                    }
                }
            }
            InputMode::HeaderFooterRange { range_input } => {
                if let Some((start_page, end_page)) = self.parse_page_range_input(&range_input) {
                    self.input_mode = InputMode::HeaderFooterPosition {
                        start_page,
                        end_page,
                        pos_input: String::new(),
                    };
                } else {
                    self.status_notice = Some(format!(
                        "Invalid page range '{}' (use e.g. '1-5' or 'all')",
                        range_input.trim()
                    ));
                }
            }
            InputMode::HeaderFooterPosition {
                start_page,
                end_page,
                pos_input,
            } => {
                if let Some(position) = MarginPosition::parse(&pos_input) {
                    self.input_mode = InputMode::HeaderFooterText {
                        start_page,
                        end_page,
                        position,
                        text_input: String::new(),
                    };
                } else {
                    self.status_notice = Some(format!(
                        "Invalid position '{}' (use HL, HC, HR, FL, FC, or FR)",
                        pos_input.trim()
                    ));
                }
            }
            InputMode::HeaderFooterText {
                start_page,
                end_page,
                position,
                text_input,
            } => {
                let raw = text_input.trim();
                self.push_undo_snapshot();
                if let Some(doc) = self.doc.as_mut() {
                    match PdfEngine::apply_aligned_header_footer_to_doc(
                        doc, position, raw, start_page, end_page,
                    ) {
                        Ok(()) => {
                            self.invalidate_page_caches();
                            self.thumbnail_cache.clear();
                            if raw.is_empty() {
                                self.status_notice = Some(format!(
                                    "Cleared margin zone on pages {start_page}..={end_page}"
                                ));
                            } else {
                                self.status_notice = Some(format!(
                                    "Stamped {:?} on pages {start_page}..={end_page}",
                                    position
                                ));
                            }
                            self.request_redraw();
                        }
                        Err(e) => {
                            self.status_notice = Some(format!("Header/Footer failed: {e}"));
                        }
                    }
                }
            }
            InputMode::ClearMargins { input } => {
                let mut parts = input.split_whitespace();
                let choice = parts.next().unwrap_or("");
                let range_str = parts.next().unwrap_or("all");
                if let Some((start_p, end_p)) = self.parse_page_range_input(range_str) {
                    self.push_undo_snapshot();
                    if let Some(doc) = self.doc.as_mut() {
                        let res = match choice {
                            "1" => PdfEngine::clear_margin_zone_in_doc(
                                doc,
                                MarginZone::Footer,
                                start_p,
                                end_p,
                            ),
                            "2" => PdfEngine::clear_margin_zone_in_doc(
                                doc,
                                MarginZone::Header,
                                start_p,
                                end_p,
                            ),
                            "3" => PdfEngine::clear_margin_zone_in_doc(
                                doc,
                                MarginZone::Header,
                                start_p,
                                end_p,
                            )
                            .and_then(|()| {
                                PdfEngine::clear_margin_zone_in_doc(
                                    doc,
                                    MarginZone::Footer,
                                    start_p,
                                    end_p,
                                )
                            }),
                            _ => Err(anyhow::anyhow!(
                                "Enter '1' (Footers/Page#), '2' (Headers), or '3' (Both)"
                            )),
                        };
                        match res {
                            Ok(()) => {
                                self.invalidate_page_caches();
                                self.thumbnail_cache.clear();
                                self.status_notice = Some(format!(
                                    "Cleared margins (option {choice}) on pages {start_p}..={end_p}"
                                ));
                                self.request_redraw();
                            }
                            Err(e) => {
                                self.status_notice = Some(format!("{e}"));
                            }
                        }
                    }
                } else {
                    self.status_notice = Some("Invalid page range for Clear Margins".to_string());
                }
            }
            InputMode::Watermark { input } => {
                let text = input.trim();
                if !text.is_empty() {
                    self.push_undo_snapshot();
                    if let Some(doc) = self.doc.as_mut() {
                        match PdfEngine::apply_watermark_to_doc(doc, text, true) {
                            Ok(()) => {
                                self.invalidate_page_caches();
                                self.thumbnail_cache.clear();
                                self.status_notice = Some(format!("Stamped watermark '{text}'"));
                                self.request_redraw();
                            }
                            Err(e) => {
                                self.status_notice = Some(format!("Watermark failed: {e}"));
                            }
                        }
                    }
                }
            }
            InputMode::SplitRange { input } => {
                self.pending_dialog = Some(PendingDialog::Split(input));
            }
            InputMode::AddBookmark { title_input } => {
                let trimmed = title_input.trim();
                let title = if trimmed.is_empty() {
                    format!("Page {}", self.current_page_index + 1)
                } else {
                    trimmed.to_string()
                };
                let page_idx = self.current_page_index as usize;
                let logical_page =
                    PdfEngine::format_logical_page_label(page_idx, self.roman_end);
                let next_serial = (self
                    .bookmarks
                    .iter()
                    .filter(|b| b.level == 0)
                    .count()
                    + 1)
                .to_string();
                self.bookmarks.push(Bookmark {
                    serial: next_serial,
                    title: title.clone(),
                    logical_page,
                    page_index: page_idx,
                    level: 0,
                });
                self.selected_bookmark = Some(self.bookmarks.len() - 1);
                self.show_bookmarks = true;
                self.persist_bookmarks();
                self.invalidate_page_caches();
                self.status_notice = Some(format!("Added bookmark '{title}'"));
                self.request_redraw();
            }
            InputMode::EditBookmark {
                bookmark_idx,
                title_input,
            } => {
                let trimmed = title_input.trim();
                if !trimmed.is_empty() {
                    if let Some(bm) = self.bookmarks.get_mut(bookmark_idx) {
                        bm.title = trimmed.to_string();
                        self.persist_bookmarks();
                        self.status_notice = Some("Updated bookmark title".to_string());
                        self.request_redraw();
                    }
                }
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    fn handle_key_input(&mut self, logical_key: Key, text: Option<&str>) {
        // 1. If in an interactive Bottom Command Bar / Title-Bar prompt mode, handle typing, Backspace, Enter, Esc
        if self.input_mode != InputMode::Normal {
            match logical_key {
                Key::Named(NamedKey::Escape) => {
                    self.input_mode = InputMode::Normal;
                    self.status_notice = Some("Cancelled".to_string());
                    self.update_title_bar();
                    self.request_redraw();
                }
                Key::Named(NamedKey::Backspace) => {
                    match &mut self.input_mode {
                        InputMode::SearchText { query_input } => {
                            query_input.pop();
                        }
                        InputMode::JumpToPage { input }
                        | InputMode::PageNumbers { input }
                        | InputMode::ClearMargins { input }
                        | InputMode::Watermark { input }
                        | InputMode::SplitRange { input } => {
                            input.pop();
                        }
                        InputMode::HeaderFooterRange { range_input } => {
                            range_input.pop();
                        }
                        InputMode::HeaderFooterPosition { pos_input, .. } => {
                            pos_input.pop();
                        }
                        InputMode::HeaderFooterText { text_input, .. } => {
                            text_input.pop();
                        }
                        InputMode::AddBookmark { title_input }
                        | InputMode::EditBookmark { title_input, .. } => {
                            title_input.pop();
                        }
                        InputMode::Normal => {}
                    }
                    self.update_title_bar();
                    self.request_redraw();
                }
                Key::Named(NamedKey::Enter) => {
                    self.handle_prompt_submission();
                }
                _ => {
                    let mut typed = String::new();
                    if let Some(t) = text {
                        for c in t.chars() {
                            if !c.is_control() {
                                typed.push(c);
                            }
                        }
                    } else if let Key::Character(ch) = &logical_key {
                        for c in ch.chars() {
                            if !c.is_control() {
                                typed.push(c);
                            }
                        }
                    } else if logical_key == Key::Named(NamedKey::Space) {
                        typed.push(' ');
                    }

                    if !typed.is_empty() {
                        let mut auto_submit_clear = false;
                        match &mut self.input_mode {
                            InputMode::SearchText { query_input } => {
                                query_input.push_str(&typed);
                            }
                            InputMode::JumpToPage { input } => {
                                for c in typed.chars() {
                                    if c.is_ascii_alphanumeric() || c == '#' {
                                        input.push(c);
                                    }
                                }
                            }
                            InputMode::PageNumbers { input } => {
                                for c in typed.chars() {
                                    if c.is_ascii_digit() {
                                        input.push(c);
                                    }
                                }
                            }
                            InputMode::HeaderFooterRange { range_input } => {
                                for c in typed.chars() {
                                    if c.is_ascii_alphanumeric() || c == '-' {
                                        range_input.push(c);
                                    }
                                }
                            }
                            InputMode::HeaderFooterPosition { pos_input, .. } => {
                                for c in typed.chars() {
                                    if c.is_ascii_alphabetic() {
                                        pos_input.push(c.to_ascii_uppercase());
                                    }
                                }
                            }
                            InputMode::ClearMargins { input } => {
                                for c in typed.chars() {
                                    if c.is_ascii_alphanumeric() || c == '-' || c == ' ' {
                                        input.push(c);
                                    }
                                }
                                // If the user presses '1', '2', or '3' directly, execute immediately on all pages
                                if matches!(input.trim(), "1" | "2" | "3") {
                                    auto_submit_clear = true;
                                }
                            }
                            InputMode::SplitRange { input } => {
                                for c in typed.chars() {
                                    if c.is_ascii_digit() || c == '-' || c == ',' || c == ' ' {
                                        input.push(c);
                                    }
                                }
                            }
                            InputMode::HeaderFooterText { text_input, .. }
                            | InputMode::AddBookmark {
                                title_input: text_input,
                            }
                            | InputMode::EditBookmark {
                                title_input: text_input,
                                ..
                            } => {
                                text_input.push_str(&typed);
                            }
                            InputMode::Watermark { input } => {
                                input.push_str(&typed);
                            }
                            InputMode::Normal => {}
                        }
                        if auto_submit_clear {
                            self.handle_prompt_submission();
                        } else {
                            self.update_title_bar();
                            self.request_redraw();
                        }
                    }
                }
            }
            return;
        }

        // 2. OS-native modifier shortcuts: Cmd+Z / Ctrl+Z (Undo), Cmd+F / Ctrl+F (Find), Cmd+G / Ctrl+G (Next Match), Cmd+W / Ctrl+W (Close Tab)
        if self.cmd_or_ctrl_pressed {
            if let Key::Character(ch) = &logical_key {
                let s = ch.as_str();
                if s.eq_ignore_ascii_case("z") {
                    self.perform_undo();
                    return;
                } else if s.eq_ignore_ascii_case("f") {
                    self.input_mode = InputMode::SearchText {
                        query_input: self.search_query.clone(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                    self.request_redraw();
                    return;
                } else if s.eq_ignore_ascii_case("g") {
                    self.jump_to_next_search_match();
                    return;
                } else if s.eq_ignore_ascii_case("w") {
                    if self.tabs.len() > 1 {
                        let _ = self.close_tab(self.active_tab);
                    } else {
                        self.status_notice =
                            Some("Cannot close the last remaining tab".to_string());
                        self.update_title_bar();
                        self.request_redraw();
                    }
                    return;
                }
            }
        }

        // 3. If Help Overlay is open, `Esc`, `?`, or `F1` closes it
        if self.show_help_overlay {
            if matches!(
                logical_key,
                Key::Named(NamedKey::Escape) | Key::Named(NamedKey::F1)
            ) {
                self.show_help_overlay = false;
                self.request_redraw();
                return;
            }
            if let Key::Character(ch) = &logical_key {
                if ch.as_str() == "?" || ch.as_str() == "/" {
                    self.show_help_overlay = false;
                    self.request_redraw();
                    return;
                }
            }
        }

        // 4. Normal Mode hotkeys
        match logical_key {
            Key::Named(NamedKey::Tab) => {
                if self.ctrl_pressed {
                    self.cycle_next_tab();
                } else {
                    self.toggle_view_mode();
                }
            }
            Key::Named(NamedKey::Enter) => {
                if self.view_mode == ViewMode::ThumbnailGrid {
                    self.toggle_view_mode();
                }
            }
            Key::Named(NamedKey::F1) => {
                self.show_help_overlay = !self.show_help_overlay;
                self.request_redraw();
            }
            Key::Named(NamedKey::F3) => {
                self.jump_to_next_search_match();
            }
            Key::Named(NamedKey::Escape) => {
                if self.open_menu.is_some() {
                    self.open_menu = None;
                    self.request_redraw();
                } else if self.show_help_overlay {
                    self.show_help_overlay = false;
                    self.request_redraw();
                } else if self.mouse_mode != MouseMode::Highlight {
                    self.mouse_mode = MouseMode::Highlight;
                    self.pending_stamp_image = None;
                    self.status_notice =
                        Some("Returned to Mouse Mode: Highlight Text".to_string());
                    self.update_title_bar();
                    self.request_redraw();
                } else if self.view_mode == ViewMode::ThumbnailGrid {
                    self.toggle_view_mode();
                }
            }
            Key::Named(NamedKey::Delete) | Key::Named(NamedKey::Backspace) => {
                // If Bookmark Sidebar is open and a bookmark is selected, Delete/Backspace deletes it
                if self.show_bookmarks {
                    if let Some(sel) = self.selected_bookmark {
                        if sel < self.bookmarks.len() {
                            let removed = self.bookmarks.remove(sel);
                            if self.bookmarks.is_empty() {
                                self.selected_bookmark = None;
                            } else if sel >= self.bookmarks.len() {
                                self.selected_bookmark = Some(self.bookmarks.len() - 1);
                            }
                            self.persist_bookmarks();
                            self.status_notice =
                                Some(format!("Deleted bookmark '{}'", removed.title));
                            self.update_title_bar();
                            self.request_redraw();
                        }
                    }
                }
            }
            Key::Named(NamedKey::ArrowDown) => {
                if !self.scroll_or_turn_page_vertical(64.0) {
                    self.step_pages_forward();
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                if !self.scroll_or_turn_page_vertical(-64.0) {
                    self.step_pages_backward();
                }
            }
            Key::Named(NamedKey::ArrowRight)
            | Key::Named(NamedKey::PageDown)
            | Key::Named(NamedKey::Space) => {
                self.step_pages_forward();
            }
            Key::Named(NamedKey::ArrowLeft) | Key::Named(NamedKey::PageUp) => {
                self.step_pages_backward();
            }
            Key::Named(NamedKey::Home) => {
                self.navigate_to_page(0);
            }
            Key::Named(NamedKey::End) => {
                let total = self.total_pages();
                if total > 0 {
                    self.navigate_to_page(total - 1);
                }
            }
            Key::Character(ch) => {
                let s = ch.as_str();
                if s == "?" || s == "/" {
                    self.show_help_overlay = !self.show_help_overlay;
                    self.request_redraw();
                } else if s == "1" {
                    self.set_view_mode(ViewMode::FitPage);
                } else if s == "2" {
                    self.set_view_mode(ViewMode::FitWidth);
                } else if s == "3" {
                    self.set_view_mode(ViewMode::FitTextWidth);
                } else if s == "4" {
                    self.set_view_mode(ViewMode::TwoPageSpread);
                } else if s.eq_ignore_ascii_case("v") {
                    self.reading_filter = self.reading_filter.next();
                    self.status_notice = Some(format!(
                        "Reading Filter: {} (Zero-RAM CPU Blit Filter)",
                        self.reading_filter.label()
                    ));
                    self.update_title_bar();
                    self.request_redraw();
                } else if s.eq_ignore_ascii_case("p") {
                    self.pending_dialog = Some(PendingDialog::PickStampImage);
                } else if s.eq_ignore_ascii_case("u") {
                    self.flatten_and_unlock_current_pdf();
                } else if s.eq_ignore_ascii_case("b") {
                    self.show_bookmarks = !self.show_bookmarks;
                    self.invalidate_page_caches();
                    self.status_notice = Some(if self.show_bookmarks {
                        "Bookmarks Sidebar ON ('+'/'A' Add, 'L' Import .txt, 'E' Edit, Del Remove)"
                            .to_string()
                    } else {
                        "Bookmarks Sidebar OFF".to_string()
                    });
                    self.update_title_bar();
                    self.request_redraw();
                } else if s.eq_ignore_ascii_case("l") {
                    self.pending_dialog = Some(PendingDialog::ImportBookmarksTxt);
                } else if (s == "+" || s == "=" || s.eq_ignore_ascii_case("a"))
                    && self.show_bookmarks
                {
                    self.input_mode = InputMode::AddBookmark {
                        title_input: String::new(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                    self.request_redraw();
                } else if s.eq_ignore_ascii_case("e") && self.show_bookmarks {
                    if let Some(sel) = self.selected_bookmark {
                        if let Some(bm) = self.bookmarks.get(sel) {
                            self.input_mode = InputMode::EditBookmark {
                                bookmark_idx: sel,
                                title_input: bm.title.clone(),
                            };
                            self.status_notice = None;
                            self.update_title_bar();
                            self.request_redraw();
                        }
                    }
                } else if s.eq_ignore_ascii_case("g") {
                    self.input_mode = InputMode::JumpToPage {
                        input: String::new(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                    self.request_redraw();
                } else if s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty() {
                    self.input_mode = InputMode::JumpToPage {
                        input: s.to_string(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                    self.request_redraw();
                } else if s.eq_ignore_ascii_case("n") {
                    self.input_mode = InputMode::PageNumbers {
                        input: String::new(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                    self.request_redraw();
                } else if s.eq_ignore_ascii_case("h") {
                    self.input_mode = InputMode::HeaderFooterRange {
                        range_input: String::new(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                    self.request_redraw();
                } else if s.eq_ignore_ascii_case("c") {
                    self.input_mode = InputMode::ClearMargins {
                        input: String::new(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                    self.request_redraw();
                } else if s.eq_ignore_ascii_case("w") {
                    self.input_mode = InputMode::Watermark {
                        input: String::new(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                    self.request_redraw();
                } else if s.eq_ignore_ascii_case("x") {
                    self.input_mode = InputMode::SplitRange {
                        input: String::new(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                    self.request_redraw();
                } else if s.eq_ignore_ascii_case("m") {
                    if self.shift_pressed || s == "M" {
                        self.pending_dialog = Some(PendingDialog::CombineMultiple);
                    } else {
                        self.pending_dialog = Some(PendingDialog::Merge);
                    }
                } else if s.eq_ignore_ascii_case("i") {
                    self.insert_blank_page_after_current();
                } else if s.eq_ignore_ascii_case("r") {
                    let target_page = if self.view_mode == ViewMode::ThumbnailGrid {
                        self.hovered_thumb.unwrap_or(self.current_page_index)
                    } else {
                        self.current_page_index
                    };
                    self.rotate_page_clockwise(target_page);
                } else if s.eq_ignore_ascii_case("k") {
                    let target_page = if self.view_mode == ViewMode::ThumbnailGrid {
                        self.hovered_thumb.unwrap_or(self.current_page_index)
                    } else {
                        self.current_page_index
                    };
                    self.convert_page_to_bw_in_place(target_page);
                } else if s.eq_ignore_ascii_case("d") {
                    self.delete_current_page();
                } else if s.eq_ignore_ascii_case("o") {
                    self.pending_dialog = Some(PendingDialog::Open);
                } else if s.eq_ignore_ascii_case("s") {
                    self.pending_dialog = Some(PendingDialog::Save);
                } else if s.eq_ignore_ascii_case("z") {
                    self.compress_current_pdf();
                }
            }
            _ => {}
        }
    }
}

impl<'a> ApplicationHandler for PdfViewerApp<'a> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let win_attrs = Window::default_attributes()
                .with_title("MicroPDF")
                .with_inner_size(LogicalSize::new(1024.0, 1080.0));

            let window = Rc::new(
                event_loop
                    .create_window(win_attrs)
                    .expect("Failed to create winit window"),
            );
            let context =
                SoftContext::new(window.clone()).expect("Failed to create softbuffer context");
            let surface = Surface::new(&context, window.clone())
                .expect("Failed to create softbuffer surface");

            self.window = Some(window);
            self.context = Some(context);
            self.surface = Some(surface);
            self.update_title_bar();
            self.request_redraw();
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(dialog) = self.pending_dialog.take() {
            match dialog {
                PendingDialog::Open => self.open_file_dialog(),
                PendingDialog::Save => self.save_file_dialog(),
                PendingDialog::Merge => self.merge_pdf_dialog(),
                PendingDialog::CombineMultiple => self.combine_multiple_pdfs_dialog(),
                PendingDialog::PickStampImage => self.pick_stamp_image_dialog(),
                PendingDialog::Split(spec) => self.execute_split_range(&spec),
                PendingDialog::ImportBookmarksTxt => self.import_bookmarks_txt_dialog(),
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                self.invalidate_page_caches();
                self.thumbnail_cache.clear();
                self.tabs.clear();
                self.doc = None;
                self.surface = None;
                self.context = None;
                self.window = None;
                event_loop.exit();
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.invalidate_page_caches();
                self.request_redraw();
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                let st = modifiers.state();
                self.shift_pressed = st.shift_key();
                self.ctrl_pressed = st.control_key();
                self.cmd_or_ctrl_pressed = st.super_key() || st.control_key();
            }
            WindowEvent::DroppedFile(dropped_path) => {
                let ext = dropped_path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");
                if ext.eq_ignore_ascii_case("txt") {
                    self.import_bookmarks_from_path(&dropped_path);
                    return;
                }
                if ext.eq_ignore_ascii_case("png")
                    || ext.eq_ignore_ascii_case("jpg")
                    || ext.eq_ignore_ascii_case("jpeg")
                {
                    if let Ok(buf) = PdfEngine::load_stamp_image(&dropped_path) {
                        self.pending_stamp_image = Some(buf);
                        self.mouse_mode = MouseMode::StampImage;
                        self.status_notice = Some(
                            "Loaded dropped image — Click or Drag on page to stamp!".to_string(),
                        );
                        self.update_title_bar();
                        self.request_redraw();
                    }
                    return;
                }
                if !ext.eq_ignore_ascii_case("pdf") {
                    self.status_notice = Some(format!(
                        "Dropped file {:?} is not a .pdf, .txt, or .png/.jpg",
                        dropped_path
                    ));
                    self.update_title_bar();
                    self.request_redraw();
                    return;
                }

                let fname = dropped_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("dropped.pdf")
                    .to_string();

                // In Page View (or when holding Shift, or when no document is loaded):
                // Open the dropped PDF in a new tab!
                if self.view_mode.is_page_view() || self.shift_pressed || self.doc.is_none() {
                    match self.open_document_in_new_tab(dropped_path) {
                        Ok(()) => {
                            self.status_notice = Some(format!(
                                "Opened '{fname}' in Tab {} ({} pages)",
                                self.active_tab + 1,
                                self.total_pages()
                            ));
                        }
                        Err(e) => {
                            self.status_notice = Some(format!("ERROR opening '{fname}': {e}"));
                        }
                    }
                    self.update_title_bar();
                    self.request_redraw();
                    return;
                }

                // In Thumbnail Grid View (`Tab`): insert all pages of the dropped PDF right before target thumbnail
                let (win_w, sidebar_w, top_bars_h) = if let Some(w) = self.window.as_ref() {
                    let sz = w.inner_size();
                    let sf = w.scale_factor();
                    (
                        sz.width.max(1),
                        self.sidebar_width_px(sf).min(sz.width.saturating_sub(64)),
                        self.top_bars_height_px(sf).min(sz.height.saturating_sub(32)),
                    )
                } else {
                    (1024, 0, MENU_BAR_HEIGHT_LOGICAL + TAB_BAR_HEIGHT_LOGICAL)
                };

                let target_idx = self.grid_drop_target_index(
                    self.cursor_pos.0,
                    self.cursor_pos.1,
                    win_w,
                    sidebar_w,
                    top_bars_h,
                );

                self.push_undo_snapshot();
                if let Some(doc) = self.doc.as_mut() {
                    match self
                        .engine
                        .insert_external_pdf_at_index(doc, &dropped_path, target_idx)
                    {
                        Ok(inserted_count) => {
                            self.current_page_index = target_idx;
                            self.invalidate_page_caches();
                            self.thumbnail_cache.clear();
                            self.status_notice = Some(format!(
                                "Inserted {inserted_count} page(s) from '{fname}' before Page {}",
                                target_idx + 1
                            ));
                            self.update_title_bar();
                            self.request_redraw();
                        }
                        Err(e) => {
                            self.status_notice = Some(format!("Drop insert failed: {e}"));
                            self.update_title_bar();
                        }
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => (y as f64) * 36.0,
                    MouseScrollDelta::PixelDelta(pos) => pos.y,
                };

                // If cursor is over the Bookmark Sidebar, scroll bookmarks
                if let Some(w) = self.window.as_ref() {
                    let sidebar_w = self.sidebar_width_px(w.scale_factor());
                    if self.show_bookmarks && (self.cursor_pos.0 as u32) < sidebar_w {
                        if dy < -4.0 && self.bookmark_scroll_y + 1 < self.bookmarks.len() {
                            self.bookmark_scroll_y += 1;
                            self.request_redraw();
                        } else if dy > 4.0 && self.bookmark_scroll_y > 0 {
                            self.bookmark_scroll_y -= 1;
                            self.request_redraw();
                        }
                        return;
                    }
                }

                if self.view_mode == ViewMode::ThumbnailGrid {
                    // Scroll thumbnail grid vertically
                    self.grid_scroll_y = (self.grid_scroll_y - dy as f32).max(0.0);
                    self.request_redraw();
                    return;
                }

                // In FitWidth or FitTextWidth, smoothly scroll `scroll_y` vertically first
                if self.scroll_or_turn_page_vertical((-dy) as f32) {
                    return;
                }

                // Otherwise turn page(s) cleanly with 120ms debounce
                let now = Instant::now();
                if let Some(last) = self.last_wheel_turn {
                    if now.duration_since(last) < Duration::from_millis(120) {
                        return;
                    }
                }
                self.wheel_accum_y += dy;
                if self.wheel_accum_y <= -18.0 {
                    self.wheel_accum_y = 0.0;
                    self.last_wheel_turn = Some(now);
                    self.step_pages_forward();
                } else if self.wheel_accum_y >= 18.0 {
                    self.wheel_accum_y = 0.0;
                    self.last_wheel_turn = Some(now);
                    self.step_pages_backward();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = (position.x, position.y);
                if let Some(w) = self.window.as_ref() {
                    let sf = w.scale_factor();
                    let menu_bar_h = self.menu_bar_height_px(sf);
                    if self.open_menu.is_some() {
                        // Hover-switch between top-level menus when a dropdown is already open
                        if let Some(hovered_cat) =
                            self.hit_test_menu_bar_header(position.x, position.y, menu_bar_h, sf)
                        {
                            if self.open_menu != Some(hovered_cat) {
                                self.open_menu = Some(hovered_cat);
                            }
                        }
                        self.request_redraw();
                        return;
                    } else if (position.y as u32) < menu_bar_h {
                        self.request_redraw();
                    }
                }

                if self.drag_start.is_some() || self.grid_drag_page.is_some() {
                    self.request_redraw();
                } else if self.view_mode == ViewMode::ThumbnailGrid {
                    if let Some(w) = self.window.as_ref() {
                        let sz = w.inner_size();
                        let sf = w.scale_factor();
                        let sidebar_w = self.sidebar_width_px(sf);
                        let top_bars_h = self.top_bars_height_px(sf);
                        let new_hover = self
                            .hit_test_thumbnail(
                                position.x,
                                position.y,
                                sz.width.max(1),
                                sidebar_w,
                                top_bars_h,
                            )
                            .map(|(idx, _)| idx);
                        if new_hover != self.hovered_thumb {
                            self.hovered_thumb = new_hover;
                            self.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if self.show_help_overlay {
                    if state == ElementState::Pressed {
                        self.show_help_overlay = false;
                        self.request_redraw();
                    }
                    return;
                }
                let (win_w, scale_factor, sidebar_w, menu_bar_h, tab_bar_h, top_bars_h) =
                    if let Some(w) = self.window.as_ref() {
                        let sz = w.inner_size();
                        let sf = w.scale_factor();
                        let mb_h = self.menu_bar_height_px(sf).min(sz.height.saturating_sub(48));
                        let tb_h = self
                            .tab_bar_height_px(sf)
                            .min(sz.height.saturating_sub(mb_h + 24));
                        (
                            sz.width.max(1),
                            sf,
                            self.sidebar_width_px(sf).min(sz.width.saturating_sub(64)),
                            mb_h,
                            tb_h,
                            mb_h + tb_h,
                        )
                    } else {
                        (
                            1024,
                            1.0,
                            0,
                            MENU_BAR_HEIGHT_LOGICAL,
                            TAB_BAR_HEIGHT_LOGICAL,
                            MENU_BAR_HEIGHT_LOGICAL + TAB_BAR_HEIGHT_LOGICAL,
                        )
                    };

                // 0a. If a Top Menu dropdown is open, check if the user clicked an item or header
                if self.open_menu.is_some() {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if let Some((cat, item_idx)) = self.hit_test_open_menu_dropdown(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            menu_bar_h,
                            scale_factor,
                        ) {
                            self.execute_menu_item(cat, item_idx);
                            return;
                        }
                        if let Some(clicked_cat) = self.hit_test_menu_bar_header(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            menu_bar_h,
                            scale_factor,
                        ) {
                            if self.open_menu == Some(clicked_cat) {
                                self.open_menu = None;
                            } else {
                                self.open_menu = Some(clicked_cat);
                            }
                            self.request_redraw();
                            return;
                        }
                        // Clicked outside menu dropdown -> close dropdown
                        self.open_menu = None;
                        self.request_redraw();
                    }
                    return;
                }

                // 0b. Check Top 24px Menu Bar header clicks
                if (self.cursor_pos.1 as u32) < menu_bar_h {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if let Some(cat) = self.hit_test_menu_bar_header(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            menu_bar_h,
                            scale_factor,
                        ) {
                            self.open_menu = Some(cat);
                            self.request_redraw();
                        }
                    }
                    return;
                }

                // 0c. Check 28px Multi-Tab Bar interactions (`menu_bar_h .. top_bars_h`)
                if (self.cursor_pos.1 as u32) < top_bars_h {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if let Some(hit) = self.hit_test_tab_bar(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
                            menu_bar_h,
                            tab_bar_h,
                            scale_factor,
                        ) {
                            match hit {
                                TabBarHit::SwitchTab(idx) => {
                                    self.switch_to_tab(idx);
                                }
                                TabBarHit::CloseTab(idx) => {
                                    if self.tabs.len() > 1 {
                                        let _ = self.close_tab(idx);
                                    } else {
                                        self.status_notice = Some(
                                            "Cannot close the last remaining tab".to_string(),
                                        );
                                        self.update_title_bar();
                                        self.request_redraw();
                                    }
                                }
                                TabBarHit::NewTab => {
                                    self.pending_dialog = Some(PendingDialog::Open);
                                }
                            }
                        }
                    }
                    return;
                }

                // 1. Check Left Bookmark Sidebar interactions
                if self.show_bookmarks && (self.cursor_pos.0 as u32) < sidebar_w {
                    if state == ElementState::Pressed {
                        if let Some(hit) = self.hit_test_bookmark_sidebar(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            sidebar_w,
                            top_bars_h,
                            scale_factor,
                        ) {
                            match hit {
                                SidebarHit::ImportTxtButton => {
                                    if button == MouseButton::Left {
                                        self.pending_dialog =
                                            Some(PendingDialog::ImportBookmarksTxt);
                                    }
                                }
                                SidebarHit::BookmarkRow {
                                    index: bm_idx,
                                    on_delete,
                                } => match button {
                                    MouseButton::Left => {
                                        if on_delete {
                                            let removed = self.bookmarks.remove(bm_idx);
                                            if self.bookmarks.is_empty() {
                                                self.selected_bookmark = None;
                                            } else if bm_idx >= self.bookmarks.len() {
                                                self.selected_bookmark =
                                                    Some(self.bookmarks.len() - 1);
                                            }
                                            self.persist_bookmarks();
                                            self.status_notice = Some(format!(
                                                "Deleted bookmark '{}'",
                                                removed.title
                                            ));
                                            self.update_title_bar();
                                            self.request_redraw();
                                        } else {
                                            self.selected_bookmark = Some(bm_idx);
                                            let target_page =
                                                self.bookmarks[bm_idx].page_index as u16;
                                            self.current_page_index = target_page
                                                .min(self.total_pages().saturating_sub(1));
                                            self.scroll_y = 0.0;
                                            self.invalidate_page_caches();
                                            self.update_title_bar();
                                            self.request_redraw();
                                        }
                                    }
                                    MouseButton::Right => {
                                        // Right-click bookmark row to Edit Bookmark Title
                                        self.selected_bookmark = Some(bm_idx);
                                        self.input_mode = InputMode::EditBookmark {
                                            bookmark_idx: bm_idx,
                                            title_input: self.bookmarks[bm_idx].title.clone(),
                                        };
                                        self.update_title_bar();
                                        self.request_redraw();
                                    }
                                    MouseButton::Middle => {
                                        // Middle-click bookmark row to Delete Bookmark
                                        let removed = self.bookmarks.remove(bm_idx);
                                        if self.bookmarks.is_empty() {
                                            self.selected_bookmark = None;
                                        } else if bm_idx >= self.bookmarks.len() {
                                            self.selected_bookmark =
                                                Some(self.bookmarks.len() - 1);
                                        }
                                        self.persist_bookmarks();
                                        self.status_notice =
                                            Some(format!("Deleted bookmark '{}'", removed.title));
                                        self.update_title_bar();
                                        self.request_redraw();
                                    }
                                    _ => {}
                                },
                            }
                        }
                    }
                    return;
                }

                // 2. Thumbnail Grid View mouse interactions
                if self.view_mode == ViewMode::ThumbnailGrid {
                    if button == MouseButton::Left {
                        match state {
                            ElementState::Pressed => {
                                if let Some((clicked_idx, badge_hit)) = self.hit_test_thumbnail(
                                    self.cursor_pos.0,
                                    self.cursor_pos.1,
                                    win_w,
                                    sidebar_w,
                                    top_bars_h,
                                ) {
                                    match badge_hit {
                                        ThumbBadgeHit::BwConvert => {
                                            self.convert_page_to_bw_in_place(clicked_idx);
                                            return;
                                        }
                                        ThumbBadgeHit::Rotate => {
                                            self.rotate_page_clockwise(clicked_idx);
                                            return;
                                        }
                                        ThumbBadgeHit::None => {}
                                    }

                                    // Check for double-click to jump into Page View
                                    let now = Instant::now();
                                    if let Some((last_idx, last_time)) = self.last_thumb_click {
                                        if last_idx == clicked_idx
                                            && now.duration_since(last_time)
                                                < Duration::from_millis(350)
                                        {
                                            self.current_page_index = clicked_idx;
                                            self.last_thumb_click = None;
                                            self.toggle_view_mode();
                                            return;
                                        }
                                    }
                                    self.last_thumb_click = Some((clicked_idx, now));
                                    self.current_page_index = clicked_idx;
                                    self.grid_drag_page = Some((
                                        clicked_idx,
                                        self.cursor_pos.0,
                                        self.cursor_pos.1,
                                    ));
                                    self.update_title_bar();
                                    self.request_redraw();
                                }
                            }
                            ElementState::Released => {
                                if let Some((src_idx, sx, sy)) = self.grid_drag_page.take() {
                                    let (cx, cy) = self.cursor_pos;
                                    let dist = ((cx - sx).powi(2) + (cy - sy).powi(2)).sqrt();
                                    if dist > 10.0 {
                                        let dst_idx = self.grid_drop_target_index(
                                            cx, cy, win_w, sidebar_w, top_bars_h,
                                        );
                                        if dst_idx != src_idx {
                                            self.push_undo_snapshot();
                                            if let Some(doc) = self.doc.as_mut() {
                                                match self.engine.move_page_in_doc(
                                                    doc, src_idx, dst_idx,
                                                ) {
                                                    Ok(()) => {
                                                        self.current_page_index = dst_idx;
                                                        self.thumbnail_cache.clear();
                                                        self.status_notice = Some(format!(
                                                            "Moved Page {} to Page {}",
                                                            src_idx + 1,
                                                            dst_idx + 1
                                                        ));
                                                    }
                                                    Err(e) => {
                                                        self.status_notice = Some(format!(
                                                            "Move page failed: {e}"
                                                        ));
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    self.update_title_bar();
                                    self.request_redraw();
                                }
                            }
                        }
                    }
                    return;
                }

                // 3. Page View mouse drag interactions: Highlight, Redact, Crop, StampImage
                if button == MouseButton::Left {
                    match state {
                        ElementState::Pressed => {
                            self.drag_start = Some(self.cursor_pos);
                        }
                        ElementState::Released => {
                            if let Some((sx1, sy1)) = self.drag_start.take() {
                                let (sx2, sy2) = self.cursor_pos;
                                let allow_click = self.mouse_mode == MouseMode::StampImage;
                                if let Some((page_idx, pdf_rect)) = self.screen_drag_to_pdf_rect(
                                    sx1,
                                    sy1,
                                    sx2,
                                    sy2,
                                    allow_click,
                                ) {
                                    match self.mouse_mode {
                                        MouseMode::Highlight => {
                                            self.push_undo_snapshot();
                                            if let Some(doc) = self.doc.as_mut() {
                                                match PdfEngine::highlight_text_in_rect(
                                                    doc, page_idx, pdf_rect,
                                                ) {
                                                    Ok(count) if count > 0 => {
                                                        self.invalidate_page_caches();
                                                        self.status_notice = Some(format!(
                                                            "Highlighted {count} text region(s) on Page {}",
                                                            page_idx + 1
                                                        ));
                                                    }
                                                    Ok(_) => {
                                                        let _ = self.undo_stack.pop_back();
                                                    }
                                                    Err(e) => {
                                                        let _ = self.undo_stack.pop_back();
                                                        self.status_notice =
                                                            Some(format!("Highlight error: {e}"));
                                                    }
                                                }
                                            }
                                        }
                                        MouseMode::Redact => {
                                            self.push_undo_snapshot();
                                            if let Some(doc) = self.doc.as_mut() {
                                                match PdfEngine::redact_rect_on_page(
                                                    doc, page_idx, pdf_rect,
                                                ) {
                                                    Ok(removed) => {
                                                        self.redacted = true;
                                                        self.invalidate_page_caches();
                                                        self.thumbnail_cache.remove(&page_idx);
                                                        self.status_notice = Some(format!(
                                                            "REDACTED area on Page {} ({} object(s)/annotation(s) removed or masked). Save to make it permanent.",
                                                            page_idx + 1,
                                                            removed
                                                        ));
                                                    }
                                                    Err(e) => {
                                                        let _ = self.undo_stack.pop_back();
                                                        self.status_notice =
                                                            Some(format!("Redaction error: {e}"));
                                                    }
                                                }
                                            }
                                        }
                                        MouseMode::Crop => {
                                            self.push_undo_snapshot();
                                            if let Some(doc) = self.doc.as_mut() {
                                                match self
                                                    .engine
                                                    .crop_page_to_rect(doc, page_idx, pdf_rect)
                                                {
                                                    Ok(cropped_bytes) => {
                                                        match self
                                                            .engine
                                                            .load_pdf_from_bytes_safe(cropped_bytes)
                                                        {
                                                            Ok(new_doc) => {
                                                                self.doc = Some(new_doc);
                                                                self.mouse_mode =
                                                                    MouseMode::Highlight;
                                                                self.scroll_y = 0.0;
                                                                self.invalidate_page_caches();
                                                                self.thumbnail_cache
                                                                    .remove(&page_idx);
                                                                self.status_notice = Some(format!(
                                                                    "Cropped Page {} (Use Page -> Reset Page Crop or Cmd/Ctrl+Z to restore)",
                                                                    page_idx + 1
                                                                ));
                                                            }
                                                            Err(e) => {
                                                                let _ = self.undo_stack.pop_back();
                                                                self.status_notice = Some(format!(
                                                                    "Crop reload error: {e}"
                                                                ));
                                                            }
                                                        }
                                                    }
                                                    Err(e) => {
                                                        let _ = self.undo_stack.pop_back();
                                                        self.status_notice =
                                                            Some(format!("Crop error: {e}"));
                                                    }
                                                }
                                            }
                                        }
                                        MouseMode::StampImage => {
                                            if let Some(stamp) = self.pending_stamp_image.clone() {
                                                self.push_undo_snapshot();
                                                if let Some(doc) = self.doc.as_mut() {
                                                    match self.engine.stamp_image_on_page(
                                                        doc, page_idx, &stamp, pdf_rect,
                                                    ) {
                                                        Ok(()) => {
                                                            self.mouse_mode = MouseMode::Highlight;
                                                            self.invalidate_page_caches();
                                                            self.thumbnail_cache.remove(&page_idx);
                                                            self.status_notice = Some(format!(
                                                                "Stamped image/signature onto Page {}! (Cmd/Ctrl+Z to undo)",
                                                                page_idx + 1
                                                            ));
                                                        }
                                                        Err(e) => {
                                                            let _ = self.undo_stack.pop_back();
                                                            self.status_notice = Some(format!(
                                                                "Image stamp error: {e}"
                                                            ));
                                                        }
                                                    }
                                                }
                                            } else {
                                                self.mouse_mode = MouseMode::Highlight;
                                            }
                                        }
                                    }
                                }
                                self.update_title_bar();
                                self.request_redraw();
                            }
                        }
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    let text_ref = event.text.as_deref();
                    self.handle_key_input(event.logical_key, text_ref);
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.render_to_window() {
                    eprintln!("Render error: {e:#}");
                }
            }
            _ => {}
        }
    }
}

/// Fills a clipped rectangle in the `0x00RRGGBB` pixel buffer.
fn fill_rect_clipped(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    rx: i32,
    ry: i32,
    rw: u32,
    rh: u32,
    color: u32,
) {
    let x0 = rx.max(0) as u32;
    let y0 = ry.max(0) as u32;
    let x1 = (rx + rw as i32).clamp(0, buf_w as i32) as u32;
    let y1 = (ry + rh as i32).clamp(0, buf_h as i32) as u32;
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    for y in y0..y1 {
        let start = (y * buf_w + x0) as usize;
        let end = (y * buf_w + x1) as usize;
        buffer[start..end].fill(color);
    }
}

/// Alpha-blends an `(r, g, b, a)` rectangle onto the `0x00RRGGBB` pixel buffer.
fn blend_rect_rgba(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    rx: u32,
    ry: u32,
    rw: u32,
    rh: u32,
    rgba: (u32, u32, u32, u32),
) {
    let (sr, sg, sb, sa) = rgba;
    let inv_a = 255 - sa;
    let x_end = (rx + rw).min(buf_w);
    let y_end = (ry + rh).min(buf_h);

    for y in ry..y_end {
        let row_start = (y * buf_w) as usize;
        for x in rx..x_end {
            let idx = row_start + (x as usize);
            let dst = buffer[idx];
            let dr = (dst >> 16) & 0xFF;
            let dg = (dst >> 8) & 0xFF;
            let db = dst & 0xFF;

            let out_r = (sr * sa + dr * inv_a) / 255;
            let out_g = (sg * sa + dg * inv_a) / 255;
            let out_b = (sb * sa + db * inv_a) / 255;

            buffer[idx] = (out_r << 16) | (out_g << 8) | out_b;
        }
    }
}

/// Draws a 1px solid border around a rectangle in the `0x00RRGGBB` buffer.
fn draw_rect_border(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    rx: u32,
    ry: u32,
    rw: u32,
    rh: u32,
    color: u32,
) {
    if rw == 0 || rh == 0 {
        return;
    }
    let x_end = (rx + rw).min(buf_w);
    let y_end = (ry + rh).min(buf_h);

    for x in rx..x_end {
        if ry < buf_h {
            buffer[(ry * buf_w + x) as usize] = color;
        }
        if y_end > 0 && y_end - 1 < buf_h {
            buffer[((y_end - 1) * buf_w + x) as usize] = color;
        }
    }
    for y in ry..y_end {
        if rx < buf_w {
            buffer[(y * buf_w + rx) as usize] = color;
        }
        if x_end > 0 && x_end - 1 < buf_w {
            buffer[(y * buf_w + (x_end - 1)) as usize] = color;
        }
    }
}

/// Renders a lightweight CPU bitmap Help Overlay (`?` / `F1`) directly into the window buffer.
fn draw_help_overlay(buffer: &mut [u32], buf_w: u32, buf_h: u32, scale_factor: f64) {
    let scale = (scale_factor.round() as u32).clamp(1, 2);
    let char_w = 8 * scale;
    let line_h = 14 * scale;

    let lines = [
        "=== MICRO_PDF PHASE 5: MENU BAR, VIEW MODES & POWER TOOLS ===",
        "",
        "TOP MENU BAR : Click [File] [View] [Edit] [Page] [Tools] [Help]",
        "TOP TAB BAR  : Click Tab to Switch, [x] or Cmd/Ctrl+W Close, [+] Open",
        "VIEW MODES   : '1' Fit Page | '2' Fit Width | '3' Fit Text | '4' 2-Page",
        "GRID & BKMKS : 'Tab' Thumbnail Grid | 'B' Bookmark Sidebar | 'L' Import",
        "FIND TEXT    : Cmd/Ctrl+F Search Text in PDF | F3 / Cmd+G Next Match",
        "MOUSE MODES  : Highlight (default) | Redact (Edit menu) | Crop (Edit)",
        "STAMP IMAGE  : 'P' Pick PNG/JPG Signature -> Click/Drag on Page to Stamp",
        "READ FILTER  : 'V' Cycle Normal / Dark Invert / Warm Sepia (0-RAM Blit)",
        "FLATTEN PDF  : 'U' Flatten Annotations/Forms & Unlock Permissions",
        "BATCH COMBINE: 'Shift+M' Combine 2+ PDFs in Order into 'Combined.pdf'",
        "SMART JUMP   : 'G' or 0-9 (Logical '5', Roman 'ii', or Physical '#5')",
        "MARGINS & #  : 'N' Roman/Arabic # | 'H' Header/Footer | 'C' Clear Zone",
        "PAGE TOOLS   : 'W' Watermark | 'K' Grayscale [BW] | 'I' Insert | 'R' Rot",
        "FILE & UNDO  : 'O' Open | 'S' Save | 'Z' Compress | 'X' Split | Cmd+Z Undo",
        "? / F1 / ESC : Toggle or Close this Help Overlay",
    ];

    let max_chars = lines.iter().map(|l| l.len()).max().unwrap_or(40) as u32;
    let pad_x = 18 * scale;
    let pad_y = 16 * scale;
    let box_w = (max_chars * char_w + pad_x * 2).min(buf_w.saturating_sub(16));
    let box_h = ((lines.len() as u32) * line_h + pad_y * 2).min(buf_h.saturating_sub(16));
    let box_x = buf_w.saturating_sub(box_w) / 2;
    let box_y = buf_h.saturating_sub(box_h) / 2;

    blend_rect_rgba(
        buffer,
        buf_w,
        buf_h,
        box_x,
        box_y,
        box_w,
        box_h,
        (20, 24, 33, 238),
    );
    draw_rect_border(buffer, buf_w, buf_h, box_x, box_y, box_w, box_h, 0x0064B5F6);

    for (idx, line) in lines.iter().enumerate() {
        let color = if idx == 0 {
            0x00FFEB3B
        } else {
            0x00F5F5F5
        };
        let tx = box_x + pad_x;
        let ty = box_y + pad_y + (idx as u32) * line_h;
        draw_bitmap_str_8x8(buffer, buf_w, buf_h, tx, ty, line, color, scale);
    }
}

/// Built-in 8x8 bitmap font rasterizer for Bookmark Sidebar, Thumbnail Grid labels, and Help Overlay.
fn draw_bitmap_str_8x8(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    x: u32,
    y: u32,
    text: &str,
    color: u32,
    scale: u32,
) {
    let mut cx = x;
    for ch in text.chars() {
        let glyph = glyph_8x8(ch);
        for (row_idx, &row_bits) in glyph.iter().enumerate() {
            for col_idx in 0..8u32 {
                if (row_bits >> (7 - col_idx)) & 1 == 1 {
                    let px = cx + col_idx * scale;
                    let py = y + (row_idx as u32) * scale;
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let gx = px + dx;
                            let gy = py + dy;
                            if gx < buf_w && gy < buf_h {
                                buffer[(gy * buf_w + gx) as usize] = color;
                            }
                        }
                    }
                }
            }
        }
        cx += 8 * scale;
    }
}

/// Crisp 8x8 ASCII bitmap font glyph generator (embeds standard 5x7 glyphs centered in an 8x8 cell).
fn glyph_8x8(c: char) -> [u8; 8] {
    let g5x7: [u8; 7] = match c.to_ascii_uppercase() {
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'D' => [0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        ':' => [0b00000, 0b00100, 0b00100, 0b00000, 0b00100, 0b00100, 0b00000],
        '-' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        '+' => [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000],
        '/' => [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
        '(' => [0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010],
        ')' => [0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000],
        '[' => [0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110],
        ']' => [0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110],
        '&' => [0b01100, 0b10010, 0b10100, 0b01000, 0b10101, 0b10010, 0b01101],
        '?' => [0b01110, 0b10001, 0b00010, 0b00100, 0b00100, 0b00000, 0b00100],
        '=' => [0b00000, 0b00000, 0b11111, 0b00000, 0b11111, 0b00000, 0b00000],
        '_' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b11111],
        '#' => [0b01010, 0b11111, 0b01010, 0b01010, 0b11111, 0b01010, 0b00000],
        '\'' => [0b00100, 0b00100, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000],
        '.' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00100, 0b00100],
        ',' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00100, 0b00100, 0b01000],
        '|' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        '!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100],
        '%' => [0b11001, 0b11010, 0b00010, 0b00100, 0b01000, 0b01011, 0b10011],
        '<' => [0b00010, 0b00100, 0b01000, 0b10000, 0b01000, 0b00100, 0b00010],
        '>' => [0b01000, 0b00100, 0b00010, 0b00001, 0b00010, 0b00100, 0b01000],
        '°' => [0b00110, 0b01001, 0b01001, 0b00110, 0b00000, 0b00000, 0b00000],
        '—' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        _ => [0; 7],
    };
    [
        g5x7[0] << 2,
        g5x7[1] << 2,
        g5x7[2] << 2,
        g5x7[3] << 2,
        g5x7[4] << 2,
        g5x7[5] << 2,
        g5x7[6] << 2,
        0,
    ]
}

/// Returns the current resident memory (RSS) of the process in megabytes (MB).
pub fn get_current_rss_mb() -> Option<f64> {
    #[cfg(target_os = "macos")]
    {
        #[repr(C)]
        struct MachTaskBasicInfo {
            virtual_size: u64,
            resident_size: u64,
            resident_size_max: u64,
            user_time: [u32; 2],
            system_time: [u32; 2],
            policy: i32,
            suspend_count: i32,
        }

        unsafe extern "C" {
            fn mach_task_self() -> u32;
            fn task_info(
                target_task: u32,
                flavor: u32,
                task_info_out: *mut MachTaskBasicInfo,
                task_info_outCnt: *mut u32,
            ) -> i32;
        }

        const MACH_TASK_BASIC_INFO: u32 = 20;
        const MACH_TASK_BASIC_INFO_COUNT: u32 =
            (std::mem::size_of::<MachTaskBasicInfo>() / std::mem::size_of::<u32>()) as u32;

        let mut info = unsafe { std::mem::zeroed::<MachTaskBasicInfo>() };
        let mut count = MACH_TASK_BASIC_INFO_COUNT;
        let kr = unsafe {
            task_info(
                mach_task_self(),
                MACH_TASK_BASIC_INFO,
                &mut info,
                &mut count,
            )
        };
        if kr == 0 {
            return Some(info.resident_size as f64 / (1024.0 * 1024.0));
        }
        None
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(statm) = std::fs::read_to_string("/proc/self/statm") {
            if let Some(rss_pages) = statm.split_whitespace().nth(1) {
                if let Ok(pages) = rss_pages.parse::<u64>() {
                    return Some((pages * 4096) as f64 / (1024.0 * 1024.0));
                }
            }
        }
        None
    }

    #[cfg(target_os = "windows")]
    {
        #[repr(C)]
        struct ProcessMemoryCounters {
            cb: u32,
            page_fault_count: u32,
            peak_working_set_size: usize,
            working_set_size: usize,
            quota_peak_paged_pool_usage: usize,
            quota_paged_pool_usage: usize,
            quota_peak_non_paged_pool_usage: usize,
            quota_non_paged_pool_usage: usize,
            pagefile_usage: usize,
            peak_pagefile_usage: usize,
        }

        #[link(name = "psapi")]
        unsafe extern "system" {
            fn K32GetProcessMemoryInfo(
                process: *mut std::ffi::c_void,
                ppsmemCounters: *mut ProcessMemoryCounters,
                cb: u32,
            ) -> i32;
        }
        unsafe extern "system" {
            fn GetCurrentProcess() -> *mut std::ffi::c_void;
        }

        let mut pmc = unsafe { std::mem::zeroed::<ProcessMemoryCounters>() };
        pmc.cb = std::mem::size_of::<ProcessMemoryCounters>() as u32;
        let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb) };
        if ok != 0 {
            return Some(pmc.working_set_size as f64 / (1024.0 * 1024.0));
        }
        None
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

pub fn run_viewer(engine: &PdfEngine, initial_pdf: &Path) -> Result<()> {
    let event_loop = EventLoop::new().context("Failed to create winit EventLoop")?;
    let mut app = PdfViewerApp::new(engine, initial_pdf.to_path_buf())?;
    event_loop
        .run_app(&mut app)
        .context("Error running winit application event loop")?;
    Ok(())
}

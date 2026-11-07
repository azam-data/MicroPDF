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
pub(super) struct CachedPageBitmap {
    pub(super) page_index: u16,
    pub(super) view_mode: ViewMode,
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

/// Zero-RAM inactive document tab slot (never stores cached bitmaps or thumbnails).
pub(super) struct DocumentTab<'a> {
    pub(super) doc: Option<PdfDocument<'a>>,
    pub(super) file_path: PathBuf,
    pub(super) current_page_index: u16,
    pub(super) view_mode: ViewMode,
    pub(super) last_page_view_mode: ViewMode,
    pub(super) scroll_y: f32,
    pub(super) roman_end: usize,
    pub(super) bookmarks: Vec<Bookmark>,
    pub(super) selected_bookmark: Option<usize>,
    pub(super) bookmark_scroll_y: usize,
    pub(super) grid_scroll_y: f32,
    pub(super) bw_pages: HashSet<u16>,
    pub(super) undo_stack: VecDeque<Vec<u8>>,
    /// True once any redaction has been applied; forces the strict save path.
    pub(super) redacted: bool,
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
    /// Set by the first `U` press on a signed document; a second press confirms.
    pub(super) flatten_confirm_at: Option<Instant>,
    pub(super) cached_page: Option<CachedPageBitmap>,
    /// Second page bitmap used ONLY in `ViewMode::TwoPageSpread` (`4` key).
    pub(super) cached_second_page: Option<CachedPageBitmap>,
    /// Signed screen draw rectangle `(draw_x, draw_y, bmp_w, bmp_h)` for the primary page.
    pub(super) page_draw_rect: (i32, i32, u32, u32),
    /// Signed screen draw rectangle `(draw_x, draw_y, bmp_w, bmp_h)` for the right page in `TwoPageSpread`.
    pub(super) second_page_draw_rect: Option<(i32, i32, u32, u32)>,
    /// Current mouse cursor coordinates in window physical pixels.
    pub(super) cursor_pos: (f64, f64),
    /// Active left-click drag state in Page View: `Some((start_x, start_y))`.
    pub(super) drag_start: Option<(f64, f64)>,
    /// Active View Mode (`FitPage`, `FitWidth`, `FitTextWidth`, `TwoPageSpread`, `ThumbnailGrid`).
    pub(super) view_mode: ViewMode,
    pub(super) last_page_view_mode: ViewMode,
    /// Vertical scroll offset in physical pixels when zoomed in `FitWidth` or `FitTextWidth`.
    pub(super) scroll_y: f32,
    /// Top Menu Bar open dropdown state (`[ File ] [ View ] [ Edit ] [ Page ] [ Tools ] [ Help ]`).
    pub(super) open_menu: Option<MenuCategory>,
    /// Active Mouse Mode (`Highlight`, `Redact`, `Crop`, `StampImage`).
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
            spare_pixel_buf: Vec::new(),
            surface: None,
            context: None,
            window: None,
        };
        app.load_document(initial_path)?;
        Ok(app)
    }

    pub(super) fn invalidate_page_caches(&mut self) {
        if let Some(old) = self.cached_page.take() {
            if old.pixels.capacity() > self.spare_pixel_buf.capacity() {
                self.spare_pixel_buf = old.pixels;
            }
        }
        self.cached_second_page = None;
        self.second_page_draw_rect = None;
    }


    /// Captures the current document's serialized PDF bytes onto `self.undo_stack` (max 8 steps)
    /// before a mutating action.
    pub(super) fn push_undo_snapshot(&mut self) {
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

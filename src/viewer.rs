use crate::engine::{Bookmark, MarginPosition, MarginZone, PdfEngine};
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
const TAB_BAR_HEIGHT_LOGICAL: u32 = 28;
const PAGE_MARGIN_PX: u32 = 24;
const THUMB_W: u32 = 140;
const THUMB_H: u32 = 198;
const MAX_UNDO_STATES: usize = 8;

/// Holds ONLY the single currently visible page's rasterized 0x00RRGGBB pixels.
/// Replaced immediately whenever `current_page_index` changes or the viewport resizes.
struct CachedPageBitmap {
    page_index: u16,
    viewport_width: u32,
    viewport_height: u32,
    bitmap_width: u32,
    bitmap_height: u32,
    rotation_deg: i32,
    pixels: Vec<u32>,
}

/// Lightweight 140x198 thumbnail bitmap entry, allocated ONLY while Grid View is active
/// and freed immediately upon returning to Single-Page View or switching tabs.
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
    roman_end: usize,
    bookmarks: Vec<Bookmark>,
    selected_bookmark: Option<usize>,
    bookmark_scroll_y: usize,
    grid_scroll_y: f32,
    bw_pages: HashSet<u16>,
    undo_stack: VecDeque<Vec<u8>>,
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

#[derive(Copy, Clone, Debug, PartialEq)]
enum ViewMode {
    SinglePage,
    ThumbnailGrid,
}

/// Interactive Command Prompt HUD state machine in the Window Title Bar.
#[derive(Clone, Debug, PartialEq)]
enum InputMode {
    Normal,
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
    cached_page: Option<CachedPageBitmap>,
    /// Exact screen rectangle `(x, y, width, height)` where the currently rendered PDF page bitmap
    /// is positioned inside the window buffer.
    page_rect: (u32, u32, u32, u32),
    /// Current mouse cursor coordinates in window physical pixels.
    cursor_pos: (f64, f64),
    /// Active left-click drag state in Single-Page View: `Some((start_x, start_y))` for text highlighting.
    drag_start: Option<(f64, f64)>,
    /// View mode: `SinglePage` vs `ThumbnailGrid` (`Tab` to toggle).
    view_mode: ViewMode,
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
    /// Debounce timer & accumulator for MouseWheel page turning in Single-Page View.
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
            cached_page: None,
            page_rect: (0, 0, 0, 0),
            cursor_pos: (0.0, 0.0),
            drag_start: None,
            view_mode: ViewMode::SinglePage,
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

    fn sync_active_tab_to_slot(&mut self) {
        if self.active_tab < self.tabs.len() {
            let slot = &mut self.tabs[self.active_tab];
            slot.doc = self.doc.take();
            slot.file_path = self.file_path.clone();
            slot.current_page_index = self.current_page_index;
            slot.roman_end = self.roman_end;
            slot.bookmarks = self.bookmarks.clone();
            slot.selected_bookmark = self.selected_bookmark;
            slot.bookmark_scroll_y = self.bookmark_scroll_y;
            slot.grid_scroll_y = self.grid_scroll_y;
            slot.bw_pages = self.bw_pages.clone();
            slot.undo_stack = self.undo_stack.clone();
        }
    }

    fn load_slot_into_active(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active_tab = index;
            let slot = &mut self.tabs[index];
            self.doc = slot.doc.take();
            self.file_path = slot.file_path.clone();
            self.current_page_index = slot.current_page_index;
            self.roman_end = slot.roman_end;
            self.bookmarks = slot.bookmarks.clone();
            self.selected_bookmark = slot.selected_bookmark;
            self.bookmark_scroll_y = slot.bookmark_scroll_y;
            self.grid_scroll_y = slot.grid_scroll_y;
            self.bw_pages = slot.bw_pages.clone();
            self.undo_stack = slot.undo_stack.clone();
        }
    }

    /// Loads `path` into the current tab (or creates the first tab on startup).
    pub fn load_document(&mut self, path: PathBuf) -> Result<()> {
        let loaded = self.engine.load_pdf_safe(&path)?;
        let roman_end = PdfEngine::read_page_labels_roman_end_from_path(&path);
        let loaded_bookmarks = PdfEngine::load_bookmarks(&loaded, &path, roman_end);

        // Strictly drop previous cached page bitmap, thumbnail cache, and previous document
        self.cached_page = None;
        self.thumbnail_cache.clear();
        self.thumbnail_cache.shrink_to_fit();
        self.doc = None;

        self.file_path = path.clone();
        self.current_page_index = 0;
        self.roman_end = roman_end;
        self.bw_pages.clear();
        self.undo_stack.clear();
        self.input_mode = InputMode::Normal;
        self.drag_start = None;
        self.grid_drag_page = None;
        self.grid_scroll_y = 0.0;
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
                roman_end,
                bookmarks: self.bookmarks.clone(),
                selected_bookmark: self.selected_bookmark,
                bookmark_scroll_y: 0,
                grid_scroll_y: 0.0,
                bw_pages: HashSet::new(),
                undo_stack: VecDeque::with_capacity(MAX_UNDO_STATES),
            });
            self.active_tab = 0;
        } else if self.active_tab < self.tabs.len() {
            self.tabs[self.active_tab].file_path = path;
            self.tabs[self.active_tab].current_page_index = 0;
            self.tabs[self.active_tab].roman_end = roman_end;
            self.tabs[self.active_tab].bookmarks = self.bookmarks.clone();
            self.tabs[self.active_tab].selected_bookmark = self.selected_bookmark;
            self.tabs[self.active_tab].bookmark_scroll_y = 0;
            self.tabs[self.active_tab].grid_scroll_y = 0.0;
            self.tabs[self.active_tab].bw_pages.clear();
            self.tabs[self.active_tab].undo_stack.clear();
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
        self.cached_page = None;
        self.thumbnail_cache.clear();
        self.thumbnail_cache.shrink_to_fit();

        let new_idx = self.tabs.len();
        self.tabs.push(DocumentTab {
            doc: None,
            file_path: path.clone(),
            current_page_index: 0,
            roman_end,
            bookmarks: loaded_bookmarks.clone(),
            selected_bookmark,
            bookmark_scroll_y: 0,
            grid_scroll_y: 0.0,
            bw_pages: HashSet::new(),
            undo_stack: VecDeque::with_capacity(MAX_UNDO_STATES),
        });

        self.active_tab = new_idx;
        self.doc = Some(loaded);
        self.file_path = path;
        self.current_page_index = 0;
        self.roman_end = roman_end;
        self.bookmarks = loaded_bookmarks;
        self.selected_bookmark = selected_bookmark;
        self.bookmark_scroll_y = 0;
        self.grid_scroll_y = 0.0;
        self.bw_pages.clear();
        self.undo_stack.clear();
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
        self.cached_page = None;
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
        self.cached_page = None;
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
        }
    }

    /// Pops the latest snapshot from `self.undo_stack` and reloads the active document in memory.
    fn perform_undo(&mut self) {
        let Some(prev_bytes) = self.undo_stack.pop_back() else {
            self.status_notice = Some("Nothing to undo (0 steps left)".to_string());
            self.update_title_bar();
            return;
        };
        match self.engine.load_pdf_from_bytes_safe(prev_bytes) {
            Ok(restored_doc) => {
                let new_total = restored_doc.pages().len();
                if self.current_page_index >= new_total && new_total > 0 {
                    self.current_page_index = new_total - 1;
                }
                self.doc = Some(restored_doc);
                self.cached_page = None;
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

    fn tab_bar_height_px(&self, scale_factor: f64) -> u32 {
        ((TAB_BAR_HEIGHT_LOGICAL as f64) * scale_factor).round() as u32
    }

    fn update_title_bar(&self) {
        let Some(window) = self.window.as_ref() else {
            return;
        };

        let title = match &self.input_mode {
            InputMode::JumpToPage { input } => format!(
                "MicroPDF | JUMP TO PAGE (Logical '5', Roman 'ii', or Physical '#5'): {}_ (Enter to confirm, Esc to cancel)",
                input
            ),
            InputMode::PageNumbers { input } => format!(
                "MicroPDF | PAGE NUMBERS: Enter last Roman page # (e.g. '5' for i..v then 1..N, or '0' for all Arabic): {}_",
                input
            ),
            InputMode::HeaderFooterRange { range_input } => format!(
                "MicroPDF | HEADER/FOOTER (1/3): Enter page range (e.g. '1-5' or 'all'): {}_",
                range_input
            ),
            InputMode::HeaderFooterPosition { pos_input, .. } => format!(
                "MicroPDF | HEADER/FOOTER (2/3): Position — type 'HL' (Header-Left), 'HC' (Header-Center), 'HR' (Header-Right), 'FL' (Footer-Left), 'FC' (Footer-Center), 'FR' (Footer-Right): {}_",
                pos_input
            ),
            InputMode::HeaderFooterText { text_input, .. } => format!(
                "MicroPDF | HEADER/FOOTER (3/3): Enter text (or leave empty and press Enter to delete existing): {}_",
                text_input
            ),
            InputMode::ClearMargins { input } => format!(
                "MicroPDF | CLEAR EXISTING: Press '1' Clear Page Numbers/Footers, '2' Clear Headers, '3' Clear Both (optional range e.g. '1 1-5'): {}_",
                input
            ),
            InputMode::Watermark { input } => format!(
                "MicroPDF | WATERMARK: Type watermark text and press Enter: {}_",
                input
            ),
            InputMode::SplitRange { input } => format!(
                "MicroPDF | SPLIT PDF: Enter page range to extract (e.g. '1-3' or '2,4,6'): {}_",
                input
            ),
            InputMode::AddBookmark { title_input } => format!(
                "MicroPDF | ADD BOOKMARK for Page {}: Type title and press Enter: {}_",
                self.current_page_index + 1,
                title_input
            ),
            InputMode::EditBookmark {
                bookmark_idx,
                title_input,
            } => {
                let page_num = self
                    .bookmarks
                    .get(*bookmark_idx)
                    .map(|b| b.page_index + 1)
                    .unwrap_or(1);
                format!(
                    "MicroPDF | EDIT BOOKMARK (Page {}): Type new title and press Enter: {}_",
                    page_num, title_input
                )
            }
            InputMode::Normal => {
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
                } else {
                    format!("Page {} / {}", current_1based, total)
                };
                let rotation = self.current_page_rotation_degrees();
                let ram_mb = get_current_rss_mb().unwrap_or(0.0);
                let filename = self.filename_display();
                let mode_tag = match self.view_mode {
                    ViewMode::SinglePage => "Single",
                    ViewMode::ThumbnailGrid => "Grid",
                };

                let shortcuts_hint =
                    "[Keys: Tab Grid, B Bmark, L Import .txt, G Jump, N Num, H Hdr/Ftr, C Clr, W Wmark, K BW, I Ins, R Rot, D Del, X Split, M Merge, Z Compress, O Open, S Save, ? Help]";
                let suffix = if let Some(notice) = &self.status_notice {
                    format!("{notice} | {shortcuts_hint}")
                } else {
                    shortcuts_hint.to_string()
                };

                format!(
                    "MicroPDF [{}] | {} | {} | Rotation: {}° | RAM: {:.1} MB | {}",
                    mode_tag, filename, page_display, rotation, ram_mb, suffix
                )
            }
        };

        window.set_title(&title);
    }

    fn request_redraw(&self) {
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    /// Ensures `self.cached_page` holds the rasterized bitmap for `self.current_page_index`
    /// fitted inside the content area `(sidebar_w..viewport_w, tab_bar_h..viewport_h)`.
    fn ensure_current_page_rendered(
        &mut self,
        viewport_w: u32,
        viewport_h: u32,
        sidebar_w: u32,
        tab_bar_h: u32,
    ) -> Result<()> {
        let content_w = viewport_w.saturating_sub(sidebar_w).max(1);
        let content_h = viewport_h.saturating_sub(tab_bar_h).max(1);
        let rotation_deg = self.current_page_rotation_degrees();

        if let Some(cache) = &self.cached_page {
            if cache.page_index == self.current_page_index
                && cache.viewport_width == content_w
                && cache.viewport_height == content_h
                && cache.rotation_deg == rotation_deg
            {
                let offset_x = sidebar_w + content_w.saturating_sub(cache.bitmap_width) / 2;
                let offset_y = tab_bar_h + content_h.saturating_sub(cache.bitmap_height) / 2;
                self.page_rect = (offset_x, offset_y, cache.bitmap_width, cache.bitmap_height);
                return Ok(());
            }
        }

        // Immediately drop old page buffer before allocating the new one (Single-Page Memory Discipline)
        self.cached_page = None;

        let Some(doc) = self.doc.as_ref() else {
            return Ok(());
        };
        let page = doc
            .pages()
            .get(self.current_page_index)
            .context("Current page index out of bounds")?;

        let avail_w = content_w.saturating_sub(PAGE_MARGIN_PX * 2).max(1) as f32;
        let avail_h = content_h.saturating_sub(PAGE_MARGIN_PX * 2).max(1) as f32;

        let page_w_pts = page.width().value.max(1.0);
        let page_h_pts = page.height().value.max(1.0);

        let scale = (avail_w / page_w_pts).min(avail_h / page_h_pts);
        let target_w = ((page_w_pts * scale).round() as i32).max(1);
        let target_h = ((page_h_pts * scale).round() as i32).max(1);

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

        let bmp_w = target_w as u32;
        let bmp_h = target_h as u32;
        let offset_x = sidebar_w + content_w.saturating_sub(bmp_w) / 2;
        let offset_y = tab_bar_h + content_h.saturating_sub(bmp_h) / 2;
        self.page_rect = (offset_x, offset_y, bmp_w, bmp_h);

        self.cached_page = Some(CachedPageBitmap {
            page_index: self.current_page_index,
            viewport_width: content_w,
            viewport_height: content_h,
            bitmap_width: bmp_w,
            bitmap_height: bmp_h,
            rotation_deg,
            pixels,
        });

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
        tab_bar_h: u32,
    ) -> (u32, u32, u32, u32, u32) {
        let content_w = viewport_w.saturating_sub(sidebar_w).max(1);
        let pad_x = 28u32;
        let pad_y = 36u32;
        let cell_w = THUMB_W + pad_x;
        let cell_h = THUMB_H + pad_y;
        let cols = ((content_w.saturating_sub(pad_x)) / cell_w).max(1);
        let grid_total_w = cols * cell_w;
        let start_x = sidebar_w + content_w.saturating_sub(grid_total_w) / 2 + pad_x / 2;
        let start_y = tab_bar_h + 20u32;
        (cols, cell_w, cell_h, start_x, start_y)
    }

    /// Returns the screen rectangle `(x, y, THUMB_W, THUMB_H)` for `page_idx` in Thumbnail Grid View.
    fn thumbnail_screen_rect(
        &self,
        page_idx: u16,
        viewport_w: u32,
        sidebar_w: u32,
        tab_bar_h: u32,
    ) -> (i32, i32, u32, u32) {
        let (cols, cell_w, cell_h, start_x, start_y) =
            self.grid_metrics(viewport_w, sidebar_w, tab_bar_h);
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
        tab_bar_h: u32,
    ) -> Option<(u16, ThumbBadgeHit)> {
        if (mx as u32) < sidebar_w || (my as u32) < tab_bar_h {
            return None;
        }
        let total = self.total_pages();
        let ix = mx.round() as i32;
        let iy = my.round() as i32;

        for idx in 0..total {
            let (tx, ty, tw, th) = self.thumbnail_screen_rect(idx, viewport_w, sidebar_w, tab_bar_h);
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
        tab_bar_h: u32,
    ) -> u16 {
        let total = self.total_pages();
        if total == 0 {
            return 0;
        }
        if let Some((idx, _)) = self.hit_test_thumbnail(mx, my, viewport_w, sidebar_w, tab_bar_h) {
            return idx;
        }
        let (cols, cell_w, cell_h, start_x, start_y) =
            self.grid_metrics(viewport_w, sidebar_w, tab_bar_h);
        let rel_x = (mx as i32 - start_x as i32).max(0) as u32;
        let rel_y = (my as i32 + self.grid_scroll_y.round() as i32 - start_y as i32).max(0) as u32;
        let col = (rel_x / cell_w).min(cols - 1);
        let row = rel_y / cell_h;
        let computed = (row * cols + col) as u16;
        computed.min(total.saturating_sub(1))
    }

    fn render_to_window(&mut self) -> Result<()> {
        let Some(window) = self.window.as_ref().cloned() else {
            return Ok(());
        };
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);
        let scale_factor = window.scale_factor();
        let sidebar_w = self.sidebar_width_px(scale_factor).min(width.saturating_sub(64));
        let tab_bar_h = self.tab_bar_height_px(scale_factor).min(height.saturating_sub(32));

        match self.view_mode {
            ViewMode::SinglePage => {
                self.ensure_current_page_rendered(width, height, sidebar_w, tab_bar_h)?;
            }
            ViewMode::ThumbnailGrid => {
                // Render only the thumbnails currently visible in the scrolled viewport
                let total = self.total_pages();
                let mut visible_indices = Vec::new();
                for idx in 0..total {
                    let (_, ty, _, th) = self.thumbnail_screen_rect(idx, width, sidebar_w, tab_bar_h);
                    if ty + (th as i32) + 30 >= (tab_bar_h as i32) && ty <= (height as i32) {
                        visible_indices.push(idx);
                    }
                }
                for idx in visible_indices {
                    let _ = self.ensure_thumbnail_rendered(idx);
                }
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

            match self.view_mode {
                ViewMode::SinglePage => {
                    // 1. Blit cached single-page bitmap
                    if let Some(cache) = &self.cached_page {
                        let (offset_x, offset_y, bmp_w, bmp_h) = self.page_rect;

                        for row in 0..bmp_h {
                            let dst_y = offset_y + row;
                            if dst_y >= height {
                                break;
                            }
                            let src_start = (row * bmp_w) as usize;
                            let copy_w = bmp_w.min(width.saturating_sub(offset_x)) as usize;
                            let dst_start = (dst_y * width + offset_x) as usize;
                            buffer[dst_start..dst_start + copy_w]
                                .copy_from_slice(&cache.pixels[src_start..src_start + copy_w]);
                        }
                    }

                    // 2. If user is holding Left-Click and dragging, draw live semi-transparent yellow selection box
                    if let Some((sx1, sy1)) = self.drag_start {
                        let (sx2, sy2) = self.cursor_pos;
                        let (px, py, pw, ph) = self.page_rect;
                        let page_right = px + pw;
                        let page_bottom = py + ph;

                        let min_x =
                            (sx1.min(sx2).round() as i64).clamp(px as i64, page_right as i64) as u32;
                        let max_x =
                            (sx1.max(sx2).round() as i64).clamp(px as i64, page_right as i64) as u32;
                        let min_y =
                            (sy1.min(sy2).round() as i64).clamp(py as i64, page_bottom as i64) as u32;
                        let max_y =
                            (sy1.max(sy2).round() as i64).clamp(py as i64, page_bottom as i64) as u32;

                        if max_x > min_x && max_y > min_y {
                            blend_rect_rgba(
                                &mut buffer,
                                width,
                                height,
                                min_x,
                                min_y,
                                max_x - min_x,
                                max_y - min_y,
                                (255, 235, 59, 90),
                            );
                            draw_rect_border(
                                &mut buffer,
                                width,
                                height,
                                min_x,
                                min_y,
                                max_x - min_x,
                                max_y - min_y,
                                0x00FBC02D,
                            );
                        }
                    }
                }
                ViewMode::ThumbnailGrid => {
                    self.draw_thumbnail_grid(&mut buffer, width, height, sidebar_w, tab_bar_h);
                }
            }

            // 3. If Left Bookmark Sidebar (`B`) is active, draw the 240px sidebar panel below tab bar
            if self.show_bookmarks && sidebar_w > 0 {
                self.draw_bookmark_sidebar(
                    &mut buffer,
                    width,
                    height,
                    sidebar_w,
                    tab_bar_h,
                    scale_factor,
                );
            }

            // 4. Draw the 28px Top Multi-Document Tab Bar (`[ 1: fileA.pdf [x] ]  [ + ]`)
            self.draw_tab_bar(&mut buffer, width, height, tab_bar_h, scale_factor);

            // 5. If Help Overlay (`?` or `F1`) is active, draw the modal overlay
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

    fn draw_tab_bar(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        tab_bar_h: u32,
        scale_factor: f64,
    ) {
        if tab_bar_h == 0 {
            return;
        }
        let scale = (scale_factor.round() as u32).clamp(1, 2);
        // Dark slate tab bar backdrop
        fill_rect_clipped(buffer, buf_w, buf_h, 0, 0, buf_w, tab_bar_h, 0x0015181E);
        // Bottom 1px divider
        fill_rect_clipped(
            buffer,
            buf_w,
            buf_h,
            0,
            (tab_bar_h.saturating_sub(1)) as i32,
            buf_w,
            1,
            0x0037474F,
        );

        let num_tabs = self.tabs.len().max(1);
        let plus_btn_w = 34 * scale;
        let avail_for_tabs = buf_w.saturating_sub(plus_btn_w + 12 * scale);
        let tab_w = (avail_for_tabs / (num_tabs as u32)).clamp(90 * scale, 210 * scale);
        let text_y = tab_bar_h.saturating_sub(8 * scale) / 2;

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
                2,
                tab_w.saturating_sub(2),
                tab_bar_h.saturating_sub(3),
                tab_bg,
            );
            draw_rect_border(
                buffer,
                buf_w,
                buf_h,
                cur_x,
                2,
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
                2,
                plus_btn_w,
                tab_bar_h.saturating_sub(3),
                0x00263238,
            );
            draw_rect_border(
                buffer,
                buf_w,
                buf_h,
                plus_x,
                2,
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
        tab_bar_h: u32,
        scale_factor: f64,
    ) -> Option<TabBarHit> {
        let ix = mx.round() as u32;
        let iy = my.round() as u32;
        if iy >= tab_bar_h {
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
        tab_bar_h: u32,
    ) {
        let total = self.total_pages();
        for idx in 0..total {
            let (tx, ty, tw, th) = self.thumbnail_screen_rect(idx, buf_w, sidebar_w, tab_bar_h);
            if ty + (th as i32) + 28 < (tab_bar_h as i32) || ty >= (buf_h as i32) {
                continue;
            }

            // Fill cell card backdrop (white paper background inside THUMB_W x THUMB_H)
            fill_rect_clipped(buffer, buf_w, buf_h, tx, ty, tw, th, 0x003A3A3A);

            if let Some(entry) = self.thumbnail_cache.get(&idx) {
                let ox = tx + ((tw.saturating_sub(entry.width)) / 2) as i32;
                let oy = ty + ((th.saturating_sub(entry.height)) / 2) as i32;

                for r in 0..entry.height {
                    let py = oy + (r as i32);
                    if py < (tab_bar_h as i32) || py >= (buf_h as i32) {
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

            if tx >= 0 && ty >= (tab_bar_h as i32) {
                draw_rect_border(
                    buffer,
                    buf_w,
                    buf_h,
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
                        buf_h,
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
                && bwy >= (tab_bar_h as i32)
                && (bwy as u32 + bw_badge_h) < buf_h
            {
                let bg = if is_bw { 0x00455A64 } else { 0x00263238 };
                let fg = if is_bw { 0x0080DEEA } else { 0x00E0E0E0 };
                fill_rect_clipped(buffer, buf_w, buf_h, bwx, bwy, bw_badge_w, bw_badge_h, bg);
                draw_rect_border(
                    buffer,
                    buf_w,
                    buf_h,
                    bwx as u32,
                    bwy as u32,
                    bw_badge_w,
                    bw_badge_h,
                    0x0090A4AE,
                );
                draw_bitmap_str_8x8(
                    buffer,
                    buf_w,
                    buf_h,
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
                && by >= (tab_bar_h as i32)
                && (by as u32 + badge_h) < buf_h
            {
                fill_rect_clipped(buffer, buf_w, buf_h, bx, by, badge_w, badge_h, 0x00263238);
                draw_rect_border(
                    buffer,
                    buf_w,
                    buf_h,
                    bx as u32,
                    by as u32,
                    badge_w,
                    badge_h,
                    0x0090A4AE,
                );
                draw_bitmap_str_8x8(
                    buffer,
                    buf_w,
                    buf_h,
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
            if ly >= (tab_bar_h as i32) && (ly as u32 + 10) < buf_h {
                let text_color = if idx == self.current_page_index {
                    0x0064B5F6
                } else {
                    0x00E0E0E0
                };
                draw_bitmap_str_8x8(
                    buffer,
                    buf_w,
                    buf_h,
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
                let target_idx = self.grid_drop_target_index(cx, cy, buf_w, sidebar_w, tab_bar_h);
                if target_idx != src_idx {
                    let (tx, ty, _tw, th) =
                        self.thumbnail_screen_rect(target_idx, buf_w, sidebar_w, tab_bar_h);
                    let bar_x = (tx - 8).max(sidebar_w as i32);
                    fill_rect_clipped(buffer, buf_w, buf_h, bar_x, ty - 4, 4, th + 8, 0x0000E5FF);
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
        tab_bar_h: u32,
        scale_factor: f64,
    ) {
        let scale = (scale_factor.round() as u32).clamp(1, 2);
        let row_h = 24 * scale;
        let header_h = 54 * scale;
        let top_y = tab_bar_h;

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
        tab_bar_h: u32,
        scale_factor: f64,
    ) -> Option<SidebarHit> {
        if !self.show_bookmarks || (mx as u32) >= sidebar_w || (my as u32) < tab_bar_h {
            return None;
        }
        let scale = (scale_factor.round() as u32).clamp(1, 2);
        let row_h = 24 * scale;
        let header_h = 54 * scale;
        let rel_y = (my.round() as u32).saturating_sub(tab_bar_h);
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

    fn toggle_view_mode(&mut self) {
        match self.view_mode {
            ViewMode::SinglePage => {
                // Switch to Thumbnail Grid View; free single-page buffer
                self.cached_page = None;
                self.view_mode = ViewMode::ThumbnailGrid;
                self.status_notice = Some(
                    "Thumbnail Grid View (Click page, Drag to reorder, 'R' rotate, Tab/Enter to view)"
                        .to_string(),
                );
            }
            ViewMode::ThumbnailGrid => {
                // Free thumbnail cache immediately when returning to Single-Page View
                self.thumbnail_cache.clear();
                self.thumbnail_cache.shrink_to_fit();
                self.grid_drag_page = None;
                self.hovered_thumb = None;
                self.view_mode = ViewMode::SinglePage;
                self.status_notice = Some("Single-Page View".to_string());
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    /// Converts screen drag start `(sx1, sy1)` and end `(sx2, sy2)` into PDF page coordinates.
    fn screen_drag_to_pdf_rect(
        &self,
        sx1: f64,
        sy1: f64,
        sx2: f64,
        sy2: f64,
    ) -> Option<PdfRect> {
        let doc = self.doc.as_ref()?;
        let page = doc.pages().get(self.current_page_index).ok()?;
        let (px, py, pw, ph) = self.page_rect;
        if pw == 0 || ph == 0 {
            return None;
        }

        let bx1 = (sx1 - px as f64).clamp(0.0, pw as f64) as i32;
        let by1 = (sy1 - py as f64).clamp(0.0, ph as f64) as i32;
        let bx2 = (sx2 - px as f64).clamp(0.0, pw as f64) as i32;
        let by2 = (sy2 - py as f64).clamp(0.0, ph as f64) as i32;

        if (bx2 - bx1).abs() < 3 && (by2 - by1).abs() < 3 {
            return None;
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

        Some(PdfRect::new_from_values(bottom, left, top, right))
    }

    fn navigate_to_page(&mut self, new_index: u16) {
        let total = self.total_pages();
        if total == 0 {
            return;
        }
        let clamped = new_index.min(total - 1);
        if clamped != self.current_page_index {
            self.current_page_index = clamped;
            self.cached_page = None;
            self.status_notice = None;
            self.update_title_bar();
            self.request_redraw();
        }
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
        self.cached_page = None;
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
                    self.cached_page = None;
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
                self.cached_page = None;
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
                self.cached_page = None;
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
                self.cached_page = None;
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
                self.cached_page = None;
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
            ) {
                Ok(()) => {
                    self.file_path = save_path.clone();
                    if self.active_tab < self.tabs.len() {
                        self.tabs[self.active_tab].file_path = save_path;
                    }
                    self.persist_bookmarks();
                    self.status_notice =
                        Some("Saved PDF with /PageLabels & /Outlines!".to_string());
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
                let final_bytes = PdfEngine::embed_metadata_in_pdf_bytes(
                    &compressed_bytes,
                    self.roman_end,
                    &self.bookmarks,
                )
                .unwrap_or(compressed_bytes);
                let new_len = final_bytes.len();
                let orig_len = disk_size.max(serialized_len);
                match std::fs::write(&out_path, &final_bytes) {
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
                            "Compressed! {:.2} MB -> {:.2} MB (Saved {}%) -> '{}'",
                            orig_mb, new_mb, saved_pct, out_name
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
            InputMode::JumpToPage { input } => {
                let total = self.total_pages() as usize;
                if let Some(target_idx) =
                    PdfEngine::resolve_logical_page_str(&input, self.roman_end, total)
                {
                    self.current_page_index = target_idx as u16;
                    self.cached_page = None;
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
                            self.cached_page = None;
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
                            self.cached_page = None;
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
                                self.cached_page = None;
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
                                self.cached_page = None;
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
                self.cached_page = None;
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
    }

    fn handle_key_input(&mut self, logical_key: Key, text: Option<&str>) {
        // 1. If in an interactive title-bar prompt mode, handle typing, Backspace, Enter, Esc
        if self.input_mode != InputMode::Normal {
            match logical_key {
                Key::Named(NamedKey::Escape) => {
                    self.input_mode = InputMode::Normal;
                    self.status_notice = Some("Cancelled".to_string());
                    self.update_title_bar();
                }
                Key::Named(NamedKey::Backspace) => {
                    match &mut self.input_mode {
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
                        }
                    }
                }
            }
            return;
        }

        // 2. OS-native modifier shortcuts: Cmd+Z / Ctrl+Z (Undo), Cmd+W / Ctrl+W (Close Tab), Ctrl+Tab (Cycle Tab)
        if self.cmd_or_ctrl_pressed {
            if let Key::Character(ch) = &logical_key {
                let s = ch.as_str();
                if s.eq_ignore_ascii_case("z") {
                    self.perform_undo();
                    return;
                } else if s.eq_ignore_ascii_case("w") {
                    if self.tabs.len() > 1 {
                        let _ = self.close_tab(self.active_tab);
                    } else {
                        self.status_notice =
                            Some("Cannot close the last remaining tab".to_string());
                        self.update_title_bar();
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
            Key::Named(NamedKey::Escape) => {
                if self.show_help_overlay {
                    self.show_help_overlay = false;
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
            Key::Named(NamedKey::ArrowRight)
            | Key::Named(NamedKey::ArrowDown)
            | Key::Named(NamedKey::PageDown)
            | Key::Named(NamedKey::Space) => {
                let total = self.total_pages();
                if self.current_page_index + 1 < total {
                    self.navigate_to_page(self.current_page_index + 1);
                }
            }
            Key::Named(NamedKey::ArrowLeft)
            | Key::Named(NamedKey::ArrowUp)
            | Key::Named(NamedKey::PageUp) => {
                if self.current_page_index > 0 {
                    self.navigate_to_page(self.current_page_index - 1);
                }
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
                } else if s.eq_ignore_ascii_case("b") {
                    self.show_bookmarks = !self.show_bookmarks;
                    self.cached_page = None;
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
                } else if s.eq_ignore_ascii_case("e") && self.show_bookmarks {
                    if let Some(sel) = self.selected_bookmark {
                        if let Some(bm) = self.bookmarks.get(sel) {
                            self.input_mode = InputMode::EditBookmark {
                                bookmark_idx: sel,
                                title_input: bm.title.clone(),
                            };
                            self.status_notice = None;
                            self.update_title_bar();
                        }
                    }
                } else if s.eq_ignore_ascii_case("g") {
                    self.input_mode = InputMode::JumpToPage {
                        input: String::new(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                } else if s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty() {
                    self.input_mode = InputMode::JumpToPage {
                        input: s.to_string(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                } else if s.eq_ignore_ascii_case("n") {
                    self.input_mode = InputMode::PageNumbers {
                        input: String::new(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                } else if s.eq_ignore_ascii_case("h") {
                    self.input_mode = InputMode::HeaderFooterRange {
                        range_input: String::new(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                } else if s.eq_ignore_ascii_case("c") {
                    self.input_mode = InputMode::ClearMargins {
                        input: String::new(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                } else if s.eq_ignore_ascii_case("w") {
                    self.input_mode = InputMode::Watermark {
                        input: String::new(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                } else if s.eq_ignore_ascii_case("x") {
                    self.input_mode = InputMode::SplitRange {
                        input: String::new(),
                    };
                    self.status_notice = None;
                    self.update_title_bar();
                } else if s.eq_ignore_ascii_case("m") {
                    self.pending_dialog = Some(PendingDialog::Merge);
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
                self.cached_page = None;
                self.thumbnail_cache.clear();
                self.tabs.clear();
                self.doc = None;
                self.surface = None;
                self.context = None;
                self.window = None;
                event_loop.exit();
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.cached_page = None;
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
                if !ext.eq_ignore_ascii_case("pdf") {
                    self.status_notice = Some(format!(
                        "Dropped file {:?} is not a .pdf or .txt",
                        dropped_path
                    ));
                    self.update_title_bar();
                    return;
                }

                let fname = dropped_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("dropped.pdf")
                    .to_string();

                // In Single-Page View (or when holding Shift, or when no document is loaded):
                // Open the dropped PDF in a new tab!
                if self.view_mode == ViewMode::SinglePage
                    || self.shift_pressed
                    || self.doc.is_none()
                {
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
                let (win_w, sidebar_w, tab_bar_h) = if let Some(w) = self.window.as_ref() {
                    let sz = w.inner_size();
                    let sf = w.scale_factor();
                    (
                        sz.width.max(1),
                        self.sidebar_width_px(sf).min(sz.width.saturating_sub(64)),
                        self.tab_bar_height_px(sf).min(sz.height.saturating_sub(32)),
                    )
                } else {
                    (1024, 0, TAB_BAR_HEIGHT_LOGICAL)
                };

                let target_idx = self.grid_drop_target_index(
                    self.cursor_pos.0,
                    self.cursor_pos.1,
                    win_w,
                    sidebar_w,
                    tab_bar_h,
                );

                self.push_undo_snapshot();
                if let Some(doc) = self.doc.as_mut() {
                    match self
                        .engine
                        .insert_external_pdf_at_index(doc, &dropped_path, target_idx)
                    {
                        Ok(inserted_count) => {
                            self.current_page_index = target_idx;
                            self.cached_page = None;
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

                match self.view_mode {
                    ViewMode::ThumbnailGrid => {
                        // Scroll thumbnail grid vertically
                        self.grid_scroll_y = (self.grid_scroll_y - dy as f32).max(0.0);
                        self.request_redraw();
                    }
                    ViewMode::SinglePage => {
                        // Turn 1 page cleanly with 120ms debounce
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
                            let total = self.total_pages();
                            if self.current_page_index + 1 < total {
                                self.navigate_to_page(self.current_page_index + 1);
                            }
                        } else if self.wheel_accum_y >= 18.0 {
                            self.wheel_accum_y = 0.0;
                            self.last_wheel_turn = Some(now);
                            if self.current_page_index > 0 {
                                self.navigate_to_page(self.current_page_index - 1);
                            }
                        }
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = (position.x, position.y);
                if self.drag_start.is_some() || self.grid_drag_page.is_some() {
                    self.request_redraw();
                } else if self.view_mode == ViewMode::ThumbnailGrid {
                    if let Some(w) = self.window.as_ref() {
                        let sz = w.inner_size();
                        let sf = w.scale_factor();
                        let sidebar_w = self.sidebar_width_px(sf);
                        let tab_bar_h = self.tab_bar_height_px(sf);
                        let new_hover = self
                            .hit_test_thumbnail(
                                position.x,
                                position.y,
                                sz.width.max(1),
                                sidebar_w,
                                tab_bar_h,
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
                    return;
                }
                let (win_w, scale_factor, sidebar_w, tab_bar_h) =
                    if let Some(w) = self.window.as_ref() {
                        let sz = w.inner_size();
                        let sf = w.scale_factor();
                        (
                            sz.width.max(1),
                            sf,
                            self.sidebar_width_px(sf).min(sz.width.saturating_sub(64)),
                            self.tab_bar_height_px(sf).min(sz.height.saturating_sub(32)),
                        )
                    } else {
                        (1024, 1.0, 0, TAB_BAR_HEIGHT_LOGICAL)
                    };

                // 0. Check Top 28px Multi-Tab Bar interactions first
                if (self.cursor_pos.1 as u32) < tab_bar_h {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if let Some(hit) = self.hit_test_tab_bar(
                            self.cursor_pos.0,
                            self.cursor_pos.1,
                            win_w,
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
                            tab_bar_h,
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
                                            self.cached_page = None;
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
                                    tab_bar_h,
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

                                    // Check for double-click to jump into Single-Page View
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
                                            cx, cy, win_w, sidebar_w, tab_bar_h,
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

                // 3. Single-Page View mouse drag text highlighting
                if button == MouseButton::Left {
                    match state {
                        ElementState::Pressed => {
                            self.drag_start = Some(self.cursor_pos);
                        }
                        ElementState::Released => {
                            if let Some((sx1, sy1)) = self.drag_start.take() {
                                let (sx2, sy2) = self.cursor_pos;
                                if let Some(pdf_rect) =
                                    self.screen_drag_to_pdf_rect(sx1, sy1, sx2, sy2)
                                {
                                    self.push_undo_snapshot();
                                    let page_idx = self.current_page_index;
                                    if let Some(doc) = self.doc.as_mut() {
                                        match PdfEngine::highlight_text_in_rect(
                                            doc, page_idx, pdf_rect,
                                        ) {
                                            Ok(count) if count > 0 => {
                                                self.cached_page = None;
                                                self.status_notice = Some(format!(
                                                    "Highlighted {count} text region(s)"
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
        "=== MICRO_PDF KEYBOARD & MOUSE CONTROLS ===",
        "",
        "TOP TAB BAR: Click Tab to Switch, [x] or Cmd/Ctrl+W Close, [+] Open",
        "MOUSE DRAG : Left-Click & Drag to Highlight Text (Single View)",
        "MOUSE WHEEL: Turn Page (Single) / Scroll (Grid & Bookmarks)",
        "TAB        : Toggle Thumbnail Grid Overview (Ctrl+Tab cycles tabs)",
        "B / L      : Toggle Bookmark Sidebar / 'L' Import .txt Bookmarks",
        "G or 0-9   : Smart Jump (Logical '5', Roman 'ii', or Physical '#5')",
        "N          : Stamp Roman (i..v) & Arabic Page Numbers + Sync Labels",
        "H / C      : Stamp Aligned Header/Footer / Clear Existing Margins",
        "W          : Stamp Diagonal Semi-Transparent Watermark",
        "K / [BW]   : Convert Current/Hovered Page to Grayscale (Black&White)",
        "I / R / D  : Insert Blank A4 / Rotate 90 deg / Delete Current Page",
        "CMD/CTRL+Z : Undo Last Structural or Page Edit (up to 8 steps)",
        "X / M / Z  : Split Range / Merge PDF / Compress & Optimize PDF",
        "O / S      : Open PDF in New Tab / Save PDF (embeds /Outlines)",
        "? / F1/ESC : Toggle or Close this Help Overlay",
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

//! CPU-rendered Favorites Library modal with nested folders & subfolders.
//!
//! Zero GPU, zero GUI toolkit, zero idle RAM (allocated only while open).

use std::path::PathBuf;
use winit::keyboard::{Key, NamedKey};

use super::render::blend_rect_rgba;
use super::state::{PdfViewerApp, PendingDialog};
use super::theme::UiTheme;
use super::ui_draw::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum FavEditKind {
    NewFolder(Option<u64>),
    Rename(u64),
}

#[derive(Clone, Debug)]
pub(super) struct FavInlineEdit {
    pub(super) kind: FavEditKind,
    pub(super) text: String,
}

pub(super) struct FavoritesModal {
    pub(super) selected_id: Option<u64>,
    pub(super) search_query: String,
    pub(super) inline_edit: Option<FavInlineEdit>,
    pub(super) move_mode_node: Option<u64>,
    pub(super) scroll_y: f32,
    last_click: Option<(u64, std::time::Instant)>,
}

impl FavoritesModal {
    pub(super) fn new() -> Self {
        Self {
            selected_id: None,
            search_query: String::new(),
            inline_edit: None,
            move_mode_node: None,
            scroll_y: 0.0,
            last_click: None,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum FavHit {
    Close,
    StarCurrent,
    AddFile,
    NewFolder,
    Rename,
    Move,
    Delete,
    OpenSelected,
    MoveToRoot,
    CancelMove,
    ClearSearch,
    RowToggle(u64),
    RowSelect(u64),
    ConfirmInline,
    CancelInline,
}

#[derive(Copy, Clone)]
struct FavHitRect {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    hit: FavHit,
}

impl FavHitRect {
    fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w as i32 && py >= self.y && py < self.y + self.h as i32
    }
}

impl<'a> PdfViewerApp<'a> {
    pub(super) fn open_favorites_modal(&mut self) {
        self.open_menu = None;
        self.favorites_modal = Some(FavoritesModal::new());
        self.request_redraw();
    }

    pub(super) fn star_current_document(&mut self, parent_folder_id: Option<u64>) {
        if self.file_path.as_os_str().is_empty() {
            self.status_notice = Some("Open a document first to add it to Favorites".to_string());
            self.request_redraw();
            return;
        }
        let title = self
            .file_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("Document")
            .to_string();
        let id = self.favorites.add_doc(parent_folder_id, title.clone(), self.file_path.clone());
        if let Some(ref mut m) = self.favorites_modal {
            m.selected_id = Some(id);
        }
        self.save_session_to_ini();
        self.status_notice = Some(format!("Added '{title}' to Favorites"));
        self.update_title_bar();
        self.request_redraw();
    }

    pub(super) fn add_favorite_file_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "Supported Documents (PDF, Markdown, LaTeX, EPUB, Word, Comic)",
                &["pdf", "md", "markdown", "tex", "epub", "docx", "cbz"],
            )
            .add_filter("PDF Document", &["pdf"])
            .add_filter("Word Document", &["docx"])
            .add_filter("Comic Book Archive", &["cbz"])
            .add_filter("EPUB E-Book", &["epub"])
            .add_filter("Markdown Document", &["md", "markdown"])
            .add_filter("LaTeX Document", &["tex"])
            .pick_file()
        {
            let parent = self
                .favorites_modal
                .as_ref()
                .and_then(|m| self.favorites.target_folder_for_selection(m.selected_id));
            let title = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("Document")
                .to_string();
            let id = self.favorites.add_doc(parent, title.clone(), path);
            if let Some(ref mut m) = self.favorites_modal {
                m.selected_id = Some(id);
            }
            self.save_session_to_ini();
            self.status_notice = Some(format!("Added '{title}' to Favorites"));
            self.request_redraw();
        }
    }

    fn open_favorite_path(&mut self, path: PathBuf) {
        if !path.exists() {
            self.status_notice = Some(format!("Favorite file not found: {}", path.display()));
            self.request_redraw();
            return;
        }
        self.favorites_modal = None;
        let fname = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("document")
            .to_string();
        let res = if !self.has_any_document() {
            self.load_document(path)
        } else {
            self.open_document_in_new_tab(path)
        };
        if let Err(e) = res {
            self.status_notice = Some(format!("Could not open '{fname}': {e}"));
        }
        self.update_title_bar();
        self.request_redraw();
    }

    fn layout_favorites_modal(
        &self,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> ((i32, i32, u32, u32), (i32, i32, u32, u32), Vec<FavHitRect>) {
        let s = sf as f32;
        let mw = ((680.0 * s).round() as u32).min(buf_w.saturating_sub(28));
        let mh = ((520.0 * s).round() as u32).min(buf_h.saturating_sub(28));
        let mx = ((buf_w.saturating_sub(mw)) / 2) as i32;
        let my = ((buf_h.saturating_sub(mh)) / 2) as i32;
        let pad = (16.0 * s).round() as i32;

        let mut hits = Vec::new();
        let close_sz = (24.0 * s).round() as u32;
        hits.push(FavHitRect {
            x: mx + mw as i32 - pad - close_sz as i32,
            y: my + (12.0 * s).round() as i32,
            w: close_sz,
            h: close_sz,
            hit: FavHit::Close,
        });

        // Search bar + top action buttons
        let bar_y = my + (48.0 * s).round() as i32;
        let bar_h = (28.0 * s).round() as u32;
        let search_w = ((210.0 * s).round() as u32).min(mw / 3);
        if self.favorites_modal.as_ref().is_some_and(|m| !m.search_query.is_empty()) {
            let cs = (18.0 * s).round() as u32;
            hits.push(FavHitRect {
                x: mx + pad + search_w as i32 - cs as i32 - 4,
                y: bar_y + ((bar_h - cs) / 2) as i32,
                w: cs,
                h: cs,
                hit: FavHit::ClearSearch,
            });
        }

        let gap = (6.0 * s).round() as i32;
        let mut bx = mx + pad + search_w as i32 + gap * 2;
        let btn_specs = [
            ("+ Star Current", FavHit::StarCurrent, 102.0),
            ("+ Add File", FavHit::AddFile, 84.0),
            ("+ Folder", FavHit::NewFolder, 76.0),
        ];
        for &(_, hit, w_base) in &btn_specs {
            let bw = (w_base * s).round() as u32;
            if bx + bw as i32 <= mx + mw as i32 - pad {
                hits.push(FavHitRect {
                    x: bx,
                    y: bar_y,
                    w: bw,
                    h: bar_h,
                    hit,
                });
                bx += bw as i32 + gap;
            }
        }

        let list_y = bar_y + bar_h as i32 + (12.0 * s).round() as i32;
        let footer_h = (52.0 * s).round() as u32;
        let extra_banner = if self
            .favorites_modal
            .as_ref()
            .is_some_and(|m| m.inline_edit.is_some() || m.move_mode_node.is_some())
        {
            (36.0 * s).round() as i32
        } else {
            0
        };
        let list_h = (my + mh as i32 - footer_h as i32 - list_y - extra_banner - (6.0 * s).round() as i32)
            .max(60) as u32;
        let list_rect = (mx + pad, list_y, mw.saturating_sub((pad * 2) as u32), list_h);

        // Inline edit / move banner buttons
        if let Some(m) = self.favorites_modal.as_ref() {
            let by = list_y + list_h as i32 + (6.0 * s).round() as i32;
            let bh = (28.0 * s).round() as u32;
            if m.inline_edit.is_some() {
                let cw = (64.0 * s).round() as u32;
                let ok_w = (58.0 * s).round() as u32;
                let cx_cancel = mx + mw as i32 - pad - cw as i32;
                let cx_ok = cx_cancel - ok_w as i32 - gap;
                hits.push(FavHitRect { x: cx_ok, y: by, w: ok_w, h: bh, hit: FavHit::ConfirmInline });
                hits.push(FavHitRect { x: cx_cancel, y: by, w: cw, h: bh, hit: FavHit::CancelInline });
            } else if m.move_mode_node.is_some() {
                let cw = (70.0 * s).round() as u32;
                let rw = (118.0 * s).round() as u32;
                let cx_cancel = mx + mw as i32 - pad - cw as i32;
                let cx_root = cx_cancel - rw as i32 - gap;
                hits.push(FavHitRect { x: cx_root, y: by, w: rw, h: bh, hit: FavHit::MoveToRoot });
                hits.push(FavHitRect { x: cx_cancel, y: by, w: cw, h: bh, hit: FavHit::CancelMove });
            }
        }

        // Visible rows hit boxes
        if let Some(m) = self.favorites_modal.as_ref() {
            let rows = self.favorites.flatten_visible(&m.search_query);
            let row_h = (28.0 * s).round() as u32;
            for (i, row) in rows.iter().enumerate() {
                let ry = list_rect.1 + (i as f32 * row_h as f32 - m.scroll_y).round() as i32;
                if ry + (row_h as i32) <= list_rect.1 || ry >= list_rect.1 + list_rect.3 as i32 {
                    continue;
                }
                let indent = (row.depth as f32 * 18.0 * s).round() as i32;
                if row.is_folder {
                    let tw = (22.0 * s).round() as u32;
                    hits.push(FavHitRect {
                        x: list_rect.0 + indent + (4.0 * s).round() as i32,
                        y: ry,
                        w: tw,
                        h: row_h,
                        hit: FavHit::RowToggle(row.id),
                    });
                }
                hits.push(FavHitRect {
                    x: list_rect.0,
                    y: ry,
                    w: list_rect.2,
                    h: row_h,
                    hit: FavHit::RowSelect(row.id),
                });
            }
        }

        // Footer buttons
        let fy = my + mh as i32 - (40.0 * s).round() as i32;
        let fh = (28.0 * s).round() as u32;
        let mut fx = mx + pad;
        for &(_, hit, w_base) in &[
            ("Rename", FavHit::Rename, 70.0),
            ("Move", FavHit::Move, 62.0),
            ("Delete", FavHit::Delete, 68.0),
        ] {
            let bw = (w_base * s).round() as u32;
            hits.push(FavHitRect { x: fx, y: fy, w: bw, h: fh, hit });
            fx += bw as i32 + gap;
        }
        let open_w = (88.0 * s).round() as u32;
        hits.push(FavHitRect {
            x: mx + mw as i32 - pad - open_w as i32,
            y: fy,
            w: open_w,
            h: fh,
            hit: FavHit::OpenSelected,
        });

        ((mx, my, mw, mh), list_rect, hits)
    }

    pub(super) fn draw_favorites_modal(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) {
        let Some(ref modal) = self.favorites_modal else {
            return;
        };
        let theme = self.ui_theme();
        let s = sf as f32;
        blend_rect_rgba(buffer, buf_w, buf_h, 0, 0, buf_w, buf_h, theme.modal_scrim);

        let ((mx, my, mw, mh), list_rect, hits) = self.layout_favorites_modal(buf_w, buf_h, sf);
        let rad = (10.0 * s).clamp(6.0, 14.0);
        draw_soft_shadow(buffer, buf_w, buf_h, mx, my, mw, mh, rad, (12.0 * s).round() as i32, (4.0 * s).round() as i32, theme.shadow_rgb, theme.shadow_max_alpha);
        fill_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.border_subtle);

        let pad = (16.0 * s).round() as i32;
        let title_sz = (15.0 * s).clamp(12.0, 24.0);
        let body_sz = (11.5 * s).clamp(9.5, 18.0);
        let small_sz = (10.5 * s).clamp(8.5, 16.0);

        draw_text(buffer, buf_w, buf_h, mx + pad, my + (14.0 * s).round() as i32, "Favorites Library (Folders & Documents)", title_sz, theme.text_primary, true);

        // Close X
        if let Some(ch) = hits.iter().find(|h| h.hit == FavHit::Close) {
            let ic = (12.0 * s).round() as u32;
            draw_icon(buffer, buf_w, buf_h, UiIcon::Close, ch.x + ((ch.w - ic) / 2) as i32, ch.y + ((ch.h - ic) / 2) as i32, ic, theme.text_muted);
        }

        // Search input box
        let bar_y = my + (48.0 * s).round() as i32;
        let bar_h = (28.0 * s).round() as u32;
        let search_w = ((210.0 * s).round() as u32).min(mw / 3);
        fill_rounded_rect(buffer, buf_w, buf_h, mx + pad, bar_y, search_w, bar_h, 5.0 * s, theme.input_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, mx + pad, bar_y, search_w, bar_h, 5.0 * s, theme.border_subtle);
        let (stxt, scol) = if modal.search_query.is_empty() {
            ("Type to filter favorites...", theme.text_muted)
        } else {
            (modal.search_query.as_str(), theme.text_primary)
        };
        let clipped_s = truncate_ellipsis(stxt, search_w as f32 - 26.0 * s, body_sz, false);
        draw_text_vcenter(buffer, buf_w, buf_h, mx + pad + (8.0 * s).round() as i32, bar_y, bar_h, mx + pad + search_w as i32 - 22, &clipped_s, body_sz, scol, false);

        // Top action buttons
        for (lbl, hit, primary) in [
            ("+ Star Current", FavHit::StarCurrent, true),
            ("+ Add File", FavHit::AddFile, false),
            ("+ Folder", FavHit::NewFolder, false),
        ] {
            if let Some(h) = hits.iter().find(|x| x.hit == hit) {
                Self::draw_fav_btn(buffer, buf_w, buf_h, h, lbl, primary, true, body_sz, &theme, s);
            }
        }

        // List background
        fill_rounded_rect(buffer, buf_w, buf_h, list_rect.0, list_rect.1, list_rect.2, list_rect.3, 6.0 * s, theme.canvas_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, list_rect.0, list_rect.1, list_rect.2, list_rect.3, 6.0 * s, theme.border_subtle);

        let rows = self.favorites.flatten_visible(&modal.search_query);
        if rows.is_empty() {
            let msg = if self.favorites.is_empty() {
                "No favorites yet. Click '+ Star Current' or '+ Folder' to organize your documents."
            } else {
                "No matching favorites."
            };
            draw_text_vcenter(buffer, buf_w, buf_h, list_rect.0 + pad, list_rect.1 + (16.0 * s).round() as i32, (26.0 * s).round() as u32, list_rect.0 + list_rect.2 as i32 - pad, msg, body_sz, theme.text_muted, false);
        } else {
            let row_h = (28.0 * s).round() as u32;
            let clip_top = list_rect.1 + 2;
            let clip_bot = list_rect.1 + list_rect.3 as i32 - 2;
            for (i, row) in rows.iter().enumerate() {
                let ry = list_rect.1 + (i as f32 * row_h as f32 - modal.scroll_y).round() as i32;
                if ry < clip_top || ry + row_h as i32 > clip_bot {
                    continue;
                }
                let is_sel = modal.selected_id == Some(row.id);
                if is_sel {
                    fill_rounded_rect(buffer, buf_w, buf_h, list_rect.0 + 4, ry + 1, list_rect.2.saturating_sub(8), row_h - 2, 4.0 * s, theme.accent_soft_bg);
                }
                let indent = (row.depth as f32 * 18.0 * s).round() as i32;
                let mut cx = list_rect.0 + indent + (8.0 * s).round() as i32;
                if row.is_folder {
                    let glyph = if row.expanded { "▾" } else { "▸" };
                    draw_text_vcenter(buffer, buf_w, buf_h, cx, ry, row_h, cx + 18, glyph, body_sz, theme.text_secondary, true);
                    cx += (16.0 * s).round() as i32;
                    let ic = (13.0 * s).round() as u32;
                    draw_icon(buffer, buf_w, buf_h, UiIcon::Open, cx, ry + ((row_h - ic) / 2) as i32, ic, theme.accent);
                    cx += ic as i32 + (6.0 * s).round() as i32;
                } else {
                    cx += (10.0 * s).round() as i32;
                    let ic = (13.0 * s).round() as u32;
                    draw_icon(buffer, buf_w, buf_h, UiIcon::Document, cx, ry + ((row_h - ic) / 2) as i32, ic, if is_sel { theme.accent } else { theme.text_secondary });
                    cx += ic as i32 + (6.0 * s).round() as i32;
                }
                let max_x = list_rect.0 + list_rect.2 as i32 - 10;
                let label = truncate_ellipsis(&row.title, (max_x - cx).max(30) as f32, body_sz, row.is_folder);
                let col = if is_sel { theme.accent_soft_fg } else { theme.text_primary };
                draw_text_vcenter(buffer, buf_w, buf_h, cx, ry, row_h, max_x, &label, body_sz, col, row.is_folder);
            }
        }

        // Inline edit or move-mode banner
        let banner_y = list_rect.1 + list_rect.3 as i32 + (6.0 * s).round() as i32;
        let banner_h = (28.0 * s).round() as u32;
        if let Some(ref ed) = modal.inline_edit {
            let lbl = match ed.kind {
                FavEditKind::NewFolder(_) => "Folder name:",
                FavEditKind::Rename(_) => "New name:",
            };
            draw_text_vcenter(buffer, buf_w, buf_h, mx + pad, banner_y, banner_h, mx + pad + 90, lbl, small_sz, theme.text_secondary, true);
            let ix = mx + pad + (84.0 * s).round() as i32;
            let iw = (mw as i32 - pad * 2 - (220.0 * s).round() as i32).max(80) as u32;
            fill_rounded_rect(buffer, buf_w, buf_h, ix, banner_y, iw, banner_h, 5.0 * s, theme.input_bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, ix, banner_y, iw, banner_h, 5.0 * s, theme.accent);
            let txt = format!("{}|", ed.text);
            draw_text_vcenter(buffer, buf_w, buf_h, ix + 8, banner_y, banner_h, ix + iw as i32 - 6, &txt, body_sz, theme.text_primary, false);
            if let Some(h) = hits.iter().find(|x| x.hit == FavHit::ConfirmInline) {
                Self::draw_fav_btn(buffer, buf_w, buf_h, h, "OK", true, true, body_sz, &theme, s);
            }
            if let Some(h) = hits.iter().find(|x| x.hit == FavHit::CancelInline) {
                Self::draw_fav_btn(buffer, buf_w, buf_h, h, "Cancel", false, true, body_sz, &theme, s);
            }
        } else if modal.move_mode_node.is_some() {
            draw_text_vcenter(
                buffer,
                buf_w,
                buf_h,
                mx + pad,
                banner_y,
                banner_h,
                mx + mw as i32 - (210.0 * s).round() as i32,
                "Move mode: click a destination folder above, or Top Level:",
                small_sz,
                theme.accent_soft_fg,
                true,
            );
            if let Some(h) = hits.iter().find(|x| x.hit == FavHit::MoveToRoot) {
                Self::draw_fav_btn(buffer, buf_w, buf_h, h, "Top Level (Root)", true, true, small_sz, &theme, s);
            }
            if let Some(h) = hits.iter().find(|x| x.hit == FavHit::CancelMove) {
                Self::draw_fav_btn(buffer, buf_w, buf_h, h, "Cancel", false, true, small_sz, &theme, s);
            }
        }

        // Footer buttons
        let has_sel = modal.selected_id.is_some();
        let is_doc_sel = modal
            .selected_id
            .and_then(|id| self.favorites.find_node(id))
            .is_some_and(|n| !n.is_folder());
        for (lbl, hit, primary, enabled) in [
            ("Rename", FavHit::Rename, false, has_sel),
            ("Move", FavHit::Move, false, has_sel),
            ("Delete", FavHit::Delete, false, has_sel),
            ("Open File", FavHit::OpenSelected, true, is_doc_sel),
        ] {
            if let Some(h) = hits.iter().find(|x| x.hit == hit) {
                Self::draw_fav_btn(buffer, buf_w, buf_h, h, lbl, primary, enabled, body_sz, &theme, s);
            }
        }
    }

    fn draw_fav_btn(
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        h: &FavHitRect,
        label: &str,
        primary: bool,
        enabled: bool,
        font_sz: f32,
        theme: &UiTheme,
        s: f32,
    ) {
        let bg = if !enabled {
            theme.surface_bg
        } else if primary {
            theme.accent
        } else {
            theme.surface_elevated
        };
        let fg = if !enabled {
            theme.text_muted
        } else if primary {
            theme.text_on_accent
        } else {
            theme.text_primary
        };
        fill_rounded_rect(buffer, buf_w, buf_h, h.x, h.y, h.w, h.h, 5.0 * s, bg);
        if !primary {
            stroke_rounded_rect(buffer, buf_w, buf_h, h.x, h.y, h.w, h.h, 5.0 * s, theme.border_subtle);
        }
        let tw = measure_text(label, font_sz, primary);
        let tx = h.x + ((h.w as f32 - tw) * 0.5).round() as i32;
        draw_text_vcenter(buffer, buf_w, buf_h, tx, h.y, h.h, h.x + h.w as i32 - 2, label, font_sz, fg, primary);
    }

    pub(super) fn handle_favorites_click(
        &mut self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> bool {
        if self.favorites_modal.is_none() {
            return false;
        }
        let ((bx, by, bw, bh), _, hits) = self.layout_favorites_modal(buf_w, buf_h, sf);
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix < bx || ix >= bx + bw as i32 || iy < by || iy >= by + bh as i32 {
            self.favorites_modal = None;
            self.request_redraw();
            return true;
        }
        let Some(hit) = hits.iter().find(|h| h.contains(ix, iy)).map(|h| h.hit) else {
            return true;
        };

        match hit {
            FavHit::Close => {
                self.favorites_modal = None;
            }
            FavHit::ClearSearch => {
                if let Some(ref mut m) = self.favorites_modal {
                    m.search_query.clear();
                    m.scroll_y = 0.0;
                }
            }
            FavHit::StarCurrent => {
                let target = self
                    .favorites_modal
                    .as_ref()
                    .and_then(|m| self.favorites.target_folder_for_selection(m.selected_id));
                self.star_current_document(target);
                return true;
            }
            FavHit::AddFile => {
                self.pending_dialog = Some(PendingDialog::AddFavoriteFile);
            }
            FavHit::NewFolder => {
                let target = self
                    .favorites_modal
                    .as_ref()
                    .and_then(|m| self.favorites.target_folder_for_selection(m.selected_id));
                if let Some(ref mut m) = self.favorites_modal {
                    m.move_mode_node = None;
                    m.inline_edit = Some(FavInlineEdit {
                        kind: FavEditKind::NewFolder(target),
                        text: String::new(),
                    });
                }
            }
            FavHit::Rename => {
                let sel = self.favorites_modal.as_ref().and_then(|m| m.selected_id);
                if let Some(id) = sel {
                    let cur = match self.favorites.find_node(id) {
                        Some(crate::favorites::FavNode::Folder { name, .. }) => name.clone(),
                        Some(crate::favorites::FavNode::Doc { title, .. }) => title.clone(),
                        None => String::new(),
                    };
                    if let Some(ref mut m) = self.favorites_modal {
                        m.move_mode_node = None;
                        m.inline_edit = Some(FavInlineEdit {
                            kind: FavEditKind::Rename(id),
                            text: cur,
                        });
                    }
                }
            }
            FavHit::Move => {
                if let Some(ref mut m) = self.favorites_modal {
                    if let Some(id) = m.selected_id {
                        m.inline_edit = None;
                        m.move_mode_node = Some(id);
                    }
                }
            }
            FavHit::MoveToRoot => {
                let node_id = self.favorites_modal.as_ref().and_then(|m| m.move_mode_node);
                if let Some(nid) = node_id {
                    self.favorites.move_node(nid, None);
                    self.save_session_to_ini();
                }
                if let Some(ref mut m) = self.favorites_modal {
                    m.move_mode_node = None;
                }
            }
            FavHit::CancelMove => {
                if let Some(ref mut m) = self.favorites_modal {
                    m.move_mode_node = None;
                }
            }
            FavHit::Delete => {
                let sel = self.favorites_modal.as_ref().and_then(|m| m.selected_id);
                if let Some(id) = sel {
                    self.favorites.delete_node(id);
                    self.save_session_to_ini();
                    if let Some(ref mut m) = self.favorites_modal {
                        m.selected_id = None;
                    }
                }
            }
            FavHit::OpenSelected => {
                let path = self
                    .favorites_modal
                    .as_ref()
                    .and_then(|m| m.selected_id)
                    .and_then(|id| match self.favorites.find_node(id) {
                        Some(crate::favorites::FavNode::Doc { path, .. }) => Some(path.clone()),
                        _ => None,
                    });
                if let Some(p) = path {
                    self.open_favorite_path(p);
                    return true;
                }
            }
            FavHit::ConfirmInline => {
                self.commit_favorites_inline_edit();
            }
            FavHit::CancelInline => {
                if let Some(ref mut m) = self.favorites_modal {
                    m.inline_edit = None;
                }
            }
            FavHit::RowToggle(fid) => {
                self.favorites.toggle_expand(fid);
                self.save_session_to_ini();
            }
            FavHit::RowSelect(id) => {
                // If in move mode and clicked a folder, move there
                let move_src = self.favorites_modal.as_ref().and_then(|m| m.move_mode_node);
                if let Some(src_id) = move_src {
                    if self.favorites.find_node(id).is_some_and(|n| n.is_folder()) {
                        self.favorites.move_node(src_id, Some(id));
                        self.save_session_to_ini();
                        if let Some(ref mut m) = self.favorites_modal {
                            m.move_mode_node = None;
                            m.selected_id = Some(src_id);
                        }
                        self.request_redraw();
                        return true;
                    }
                }
                let now = std::time::Instant::now();
                let is_double = self
                    .favorites_modal
                    .as_ref()
                    .and_then(|m| m.last_click)
                    .is_some_and(|(prev_id, t)| prev_id == id && now.duration_since(t).as_millis() < 450);
                if let Some(ref mut m) = self.favorites_modal {
                    m.selected_id = Some(id);
                    m.last_click = Some((id, now));
                }
                if is_double {
                    if let Some(crate::favorites::FavNode::Doc { path, .. }) = self.favorites.find_node(id) {
                        let p = path.clone();
                        self.open_favorite_path(p);
                        return true;
                    } else if self.favorites.find_node(id).is_some_and(|n| n.is_folder()) {
                        self.favorites.toggle_expand(id);
                        self.save_session_to_ini();
                    }
                }
            }
        }
        self.request_redraw();
        true
    }

    fn commit_favorites_inline_edit(&mut self) {
        let edit = self.favorites_modal.as_mut().and_then(|m| m.inline_edit.take());
        let Some(ed) = edit else { return };
        match ed.kind {
            FavEditKind::NewFolder(parent) => {
                let id = self.favorites.add_folder(parent, ed.text);
                if let Some(ref mut m) = self.favorites_modal {
                    m.selected_id = Some(id);
                }
            }
            FavEditKind::Rename(id) => {
                self.favorites.rename_node(id, ed.text);
            }
        }
        self.save_session_to_ini();
    }

    pub(super) fn handle_favorites_key(&mut self, key: &Key, text: Option<&str>) -> bool {
        let Some(ref mut modal) = self.favorites_modal else {
            return false;
        };
        if let Some(ref mut ed) = modal.inline_edit {
            match key {
                Key::Named(NamedKey::Escape) => {
                    modal.inline_edit = None;
                }
                Key::Named(NamedKey::Enter) => {
                    self.commit_favorites_inline_edit();
                }
                Key::Named(NamedKey::Backspace) => {
                    ed.text.pop();
                }
                _ => {
                    if let Some(t) = text {
                        for c in t.chars().filter(|c| !c.is_control()) {
                            ed.text.push(c);
                        }
                    }
                }
            }
            self.request_redraw();
            return true;
        }

        match key {
            Key::Named(NamedKey::Escape) => {
                if modal.move_mode_node.is_some() {
                    modal.move_mode_node = None;
                } else {
                    self.favorites_modal = None;
                }
            }
            Key::Named(NamedKey::Enter) => {
                if let Some(id) = modal.selected_id {
                    if let Some(crate::favorites::FavNode::Doc { path, .. }) = self.favorites.find_node(id) {
                        let p = path.clone();
                        self.open_favorite_path(p);
                        return true;
                    } else if self.favorites.find_node(id).is_some_and(|n| n.is_folder()) {
                        self.favorites.toggle_expand(id);
                        self.save_session_to_ini();
                    }
                }
            }
            Key::Named(NamedKey::Backspace) => {
                modal.search_query.pop();
                modal.scroll_y = 0.0;
            }
            Key::Named(NamedKey::ArrowDown) => {
                let rows = self.favorites.flatten_visible(&modal.search_query);
                if !rows.is_empty() {
                    let next = match modal.selected_id.and_then(|sid| rows.iter().position(|r| r.id == sid)) {
                        Some(idx) => (idx + 1).min(rows.len() - 1),
                        None => 0,
                    };
                    modal.selected_id = Some(rows[next].id);
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                let rows = self.favorites.flatten_visible(&modal.search_query);
                if !rows.is_empty() {
                    let prev = match modal.selected_id.and_then(|sid| rows.iter().position(|r| r.id == sid)) {
                        Some(idx) => idx.saturating_sub(1),
                        None => 0,
                    };
                    modal.selected_id = Some(rows[prev].id);
                }
            }
            _ => {
                if let Some(t) = text {
                    for c in t.chars().filter(|c| !c.is_control()) {
                        modal.search_query.push(c);
                        modal.scroll_y = 0.0;
                    }
                }
            }
        }
        self.request_redraw();
        true
    }
}

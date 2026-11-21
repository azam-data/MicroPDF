//! Customizable compact icon toolbar rendered inline beside the menus on the unified top bar,
//! plus the lightweight "Customize Toolbar..." checklist overlay.
//! Page navigation, page zoom, view modes, reading filter, fullscreen, and presentation live
//! centered on the Bottom View Bar (`bottom_bar.rs`).

use super::render::{blend_rect_rgba, fill_rect_clipped};
use super::state::*;
use super::ui_draw::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ToolbarItem {
    Sidebar = 0,
    Grid = 1,
    Open = 2,
    Save = 3,
    SaveAs = 4,
    Print = 5,
    Annotations = 6,
    EditTools = 7,
    Rotate = 8,
    Search = 9,
    UiScale = 10,
    ThemeToggle = 11,
}

impl ToolbarItem {
    pub const COUNT: usize = 12;

    pub const ALL: [ToolbarItem; Self::COUNT] = [
        ToolbarItem::Sidebar,
        ToolbarItem::Grid,
        ToolbarItem::Open,
        ToolbarItem::Save,
        ToolbarItem::SaveAs,
        ToolbarItem::Print,
        ToolbarItem::Annotations,
        ToolbarItem::EditTools,
        ToolbarItem::Rotate,
        ToolbarItem::Search,
        ToolbarItem::UiScale,
        ToolbarItem::ThemeToggle,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Sidebar => "sidebar",
            Self::Grid => "grid",
            Self::Open => "open",
            Self::Save => "save",
            Self::SaveAs => "save_as",
            Self::Print => "print",
            Self::Annotations => "annotations",
            Self::EditTools => "edit_tools",
            Self::Rotate => "rotate",
            Self::Search => "search",
            Self::UiScale => "ui_scale",
            Self::ThemeToggle => "theme",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Sidebar => "Bookmarks Sidebar (B)",
            Self::Grid => "Thumbnail Grid (Tab)",
            Self::Open => "Open PDF (O)",
            Self::Save => "Save PDF (S)",
            Self::SaveAs => "Save As... (Ctrl+Shift+S)",
            Self::Print => "Print Preview (Ctrl+P)",
            Self::Annotations => "Annotations (Highlight/Underline/Strike/Note/Pen/Text/Shapes)",
            Self::EditTools => "Edit Tools (Redact / Crop / Stamp)",
            Self::Rotate => "Rotate Page 90° (R)",
            Self::Search => "Find Text (Ctrl+F)",
            Self::UiScale => "Zoom Interface (UI Scale - / % / +)",
            Self::ThemeToggle => "Light / Dark UI Toggle",
        }
    }

    pub fn from_key(s: &str) -> Option<Self> {
        let k = s.trim();
        if k.eq_ignore_ascii_case("tools") {
            return Some(Self::Annotations);
        }
        Self::ALL.into_iter().find(|item| item.key().eq_ignore_ascii_case(k))
    }

    pub fn default_enabled(self) -> bool {
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolbarConfig {
    pub enabled: [bool; ToolbarItem::COUNT],
}

impl Default for ToolbarConfig {
    fn default() -> Self {
        let mut enabled = [false; ToolbarItem::COUNT];
        for item in ToolbarItem::ALL {
            enabled[item as usize] = item.default_enabled();
        }
        Self { enabled }
    }
}

impl ToolbarConfig {
    pub fn is_enabled(&self, item: ToolbarItem) -> bool {
        self.enabled[item as usize]
    }

    pub fn toggle(&mut self, item: ToolbarItem) {
        let idx = item as usize;
        self.enabled[idx] = !self.enabled[idx];
    }

    pub fn to_ini_str(&self) -> String {
        let keys: Vec<&str> = ToolbarItem::ALL
            .into_iter()
            .filter(|&it| self.is_enabled(it))
            .map(ToolbarItem::key)
            .collect();
        if keys.is_empty() {
            "none".to_string()
        } else {
            keys.join(",")
        }
    }

    pub fn from_ini_str(s: &str) -> Self {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Self::default();
        }
        if trimmed.eq_ignore_ascii_case("none") {
            return Self {
                enabled: [false; ToolbarItem::COUNT],
            };
        }
        let mut enabled = [false; ToolbarItem::COUNT];
        let mut any_matched = false;
        for part in trimmed.split(',') {
            if let Some(item) = ToolbarItem::from_key(part) {
                enabled[item as usize] = true;
                any_matched = true;
            }
        }
        if any_matched {
            Self { enabled }
        } else {
            Self::default()
        }
    }
}

/// Every clickable action exposed by the inline top toolbar.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ToolbarAction {
    ToggleSidebar,
    ToggleGrid,
    Open,
    Save,
    SaveAs,
    Print,
    ToolSelect,
    ToolHighlight,
    ToolUnderline,
    ToolStrikethrough,
    ToolStickyNote,
    ToolInkPen,
    ToolFreeText,
    ToolRectShape,
    ToolArrowShape,
    CycleAnnotationColor,
    ToolRedact,
    ToolCrop,
    ToolStamp,
    RotatePage,
    FindText,
    UiScaleDown,
    UiScaleReset,
    UiScaleUp,
    ToggleTheme,
}

impl ToolbarAction {
    pub fn tooltip(self) -> &'static str {
        match self {
            Self::ToggleSidebar => "Bookmarks Sidebar (B)",
            Self::ToggleGrid => "Thumbnail Grid (Tab)",
            Self::Open => "Open PDF (O)",
            Self::Save => "Save PDF (S)",
            Self::SaveAs => "Save As... (Ctrl+Shift+S)",
            Self::Print => "Print Preview (Ctrl+P)",
            Self::ToolSelect => "Select Text Tool (default)",
            Self::ToolHighlight => "Highlight Text (drag across text)",
            Self::ToolUnderline => "Underline Text (drag across text)",
            Self::ToolStrikethrough => "Strikethrough Text (drag across text)",
            Self::ToolStickyNote => "Sticky Note / Comment (click on page)",
            Self::ToolInkPen => "Freehand Pen / Ink (drag to draw)",
            Self::ToolFreeText => "Text Box / FreeText (click on page)",
            Self::ToolRectShape => "Rectangle Shape (drag box)",
            Self::ToolArrowShape => "Arrow Shape (drag start to tip)",
            Self::CycleAnnotationColor => "Click to Cycle Annotation Color",
            Self::ToolRedact => "Black-Box Redact (drag area)",
            Self::ToolCrop => "Crop Page Area (drag area)",
            Self::ToolStamp => "Stamp Signature / Image (P)",
            Self::RotatePage => "Rotate Page 90° (R)",
            Self::FindText => "Find Text (Ctrl+F)",
            Self::UiScaleDown => "Shrink Interface Scale (-10%)",
            Self::UiScaleReset => "Reset Interface Scale to 100%",
            Self::UiScaleUp => "Enlarge Interface Scale (+10%)",
            Self::ToggleTheme => "Switch Light / Dark UI Theme",
        }
    }
}

#[derive(Clone)]
pub(super) struct ToolbarWidgetRect {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) w: u32,
    pub(super) h: u32,
    pub(super) action: ToolbarAction,
    pub(super) icon: Option<UiIcon>,
    pub(super) label: Option<String>,
    pub(super) swatch_rgb: Option<u32>,
    pub(super) active: bool,
    pub(super) accent_dot: bool,
}

#[derive(Clone, Copy)]
pub(super) struct ToolbarSepX(pub(super) i32);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum CustomizeHit {
    ToggleItem(ToolbarItem),
    ResetDefaults,
    Done,
    InsideModal,
}

impl<'a> PdfViewerApp<'a> {
    pub(super) fn compute_inline_toolbar_layout(
        &self,
        start_x: i32,
        buf_w: u32,
        bar_h: u32,
        sf: f64,
    ) -> (Vec<ToolbarWidgetRect>, Vec<ToolbarSepX>) {
        if !self.show_toolbar || bar_h == 0 || buf_w < 200 {
            return (Vec::new(), Vec::new());
        }

        let s = sf as f32;
        let btn_h = (22.0 * s).round().clamp(16.0, (bar_h as f32) - 4.0) as u32;
        let btn_y = ((bar_h.saturating_sub(btn_h)) / 2) as i32;
        let sq_w = (23.0 * s).round().max(18.0) as u32;
        let gap = (2.0 * s).round().max(1.0) as i32;
        let sep_pad = (5.0 * s).round().max(3.0) as i32;
        let right_margin = (8.0 * s).round() as i32;
        let font_sz = (10.5 * s).clamp(8.5, 20.0);

        let mut widgets = Vec::with_capacity(24);
        let mut seps = Vec::with_capacity(6);
        let mut right_edge = (buf_w as i32) - right_margin;

        // Right-aligned Theme Toggle
        if self.toolbar_config.is_enabled(ToolbarItem::ThemeToggle) {
            let x = right_edge - (sq_w as i32);
            if x > start_x + 20 {
                let ic = match self.ui_theme_mode {
                    super::theme::UiThemeMode::Light => UiIcon::Moon,
                    super::theme::UiThemeMode::Dark => UiIcon::Sun,
                };
                widgets.push(ToolbarWidgetRect {
                    x,
                    y: btn_y,
                    w: sq_w,
                    h: btn_h,
                    action: ToolbarAction::ToggleTheme,
                    icon: Some(ic),
                    label: None,
                    swatch_rgb: None,
                    active: false,
                    accent_dot: false,
                });
                right_edge = x - gap * 2;
            }
        }

        // Right-aligned UI Scale (Zoom Interface: A- / 100% / A+)
        if self.toolbar_config.is_enabled(ToolbarItem::UiScale) {
            let ui_txt = format!("UI {}%", (self.ui_scale * 100.0).round() as i32);
            let lbl_w = (measure_text(&ui_txt, font_sz, false).ceil() as u32 + (8.0 * s).round() as u32)
                .max((46.0 * s).round() as u32);
            let mini_w = (20.0 * s).round().max(16.0) as u32;
            let total_ui_w = (mini_w * 2 + lbl_w) as i32 + gap * 2;
            if right_edge - total_ui_w > start_x + 40 {
                let x_plus = right_edge - mini_w as i32;
                let x_mid = x_plus - gap - lbl_w as i32;
                let x_minus = x_mid - gap - mini_w as i32;
                widgets.push(ToolbarWidgetRect {
                    x: x_minus,
                    y: btn_y,
                    w: mini_w,
                    h: btn_h,
                    action: ToolbarAction::UiScaleDown,
                    icon: Some(UiIcon::ZoomOut),
                    label: None,
                    swatch_rgb: None,
                    active: false,
                    accent_dot: false,
                });
                widgets.push(ToolbarWidgetRect {
                    x: x_mid,
                    y: btn_y,
                    w: lbl_w,
                    h: btn_h,
                    action: ToolbarAction::UiScaleReset,
                    icon: None,
                    label: Some(ui_txt),
                    swatch_rgb: None,
                    active: (self.ui_scale - 1.0).abs() > 0.01,
                    accent_dot: false,
                });
                widgets.push(ToolbarWidgetRect {
                    x: x_plus,
                    y: btn_y,
                    w: mini_w,
                    h: btn_h,
                    action: ToolbarAction::UiScaleUp,
                    icon: Some(UiIcon::ZoomIn),
                    label: None,
                    swatch_rgb: None,
                    active: false,
                    accent_dot: false,
                });
                right_edge = x_minus - gap * 2;
            }
        }

        let max_left_x = right_edge - (6.0 * s).round() as i32;
        let mut cur_x = start_x + (4.0 * s).round() as i32;

        if cur_x + (sq_w as i32) > max_left_x {
            return (widgets, seps);
        }

        // Initial separator between `Help` menu and toolbar
        seps.push(ToolbarSepX(cur_x));
        cur_x += sep_pad;

        let push_sep = |cur_x: &mut i32, seps: &mut Vec<ToolbarSepX>, count_before: usize| {
            if count_before > 0 && *cur_x + sep_pad * 2 < max_left_x {
                *cur_x += sep_pad / 2;
                seps.push(ToolbarSepX(*cur_x));
                *cur_x += sep_pad;
            }
        };

        let try_push = |w_item: ToolbarWidgetRect, cur_x: &mut i32, widgets: &mut Vec<ToolbarWidgetRect>| -> bool {
            if *cur_x + (w_item.w as i32) <= max_left_x {
                let mut item = w_item;
                item.x = *cur_x;
                *cur_x += (item.w as i32) + gap;
                widgets.push(item);
                true
            } else {
                false
            }
        };

        let mk_ic = |action: ToolbarAction, icon: UiIcon, active: bool, accent_dot: bool| ToolbarWidgetRect {
            x: 0,
            y: btn_y,
            w: sq_w,
            h: btn_h,
            action,
            icon: Some(icon),
            label: None,
            swatch_rgb: None,
            active,
            accent_dot,
        };

        // Group 1: Panels & File actions (Sidebar, Grid, Open, Save, SaveAs, Print)
        let mut g1 = 0usize;
        if self.toolbar_config.is_enabled(ToolbarItem::Sidebar) {
            if try_push(mk_ic(ToolbarAction::ToggleSidebar, UiIcon::Sidebar, self.show_bookmarks, false), &mut cur_x, &mut widgets) {
                g1 += 1;
            }
        }
        if self.toolbar_config.is_enabled(ToolbarItem::Grid) {
            if try_push(mk_ic(ToolbarAction::ToggleGrid, UiIcon::Grid, self.view_mode == ViewMode::ThumbnailGrid, false), &mut cur_x, &mut widgets) {
                g1 += 1;
            }
        }
        if self.toolbar_config.is_enabled(ToolbarItem::Open) {
            if try_push(mk_ic(ToolbarAction::Open, UiIcon::Open, false, false), &mut cur_x, &mut widgets) {
                g1 += 1;
            }
        }
        if self.toolbar_config.is_enabled(ToolbarItem::Save) {
            if try_push(mk_ic(ToolbarAction::Save, UiIcon::Save, self.has_unsaved_edits, self.has_unsaved_edits), &mut cur_x, &mut widgets) {
                g1 += 1;
            }
        }
        if self.toolbar_config.is_enabled(ToolbarItem::SaveAs) {
            if try_push(mk_ic(ToolbarAction::SaveAs, UiIcon::SaveAs, false, false), &mut cur_x, &mut widgets) {
                g1 += 1;
            }
        }
        if self.toolbar_config.is_enabled(ToolbarItem::Print) {
            if try_push(mk_ic(ToolbarAction::Print, UiIcon::Print, self.print_preview.is_some(), false), &mut cur_x, &mut widgets) {
                g1 += 1;
            }
        }

        // Group 2: Select + Full Annotations Suite + Color Swatch
        let mut g2 = 0usize;
        if self.toolbar_config.is_enabled(ToolbarItem::Annotations) {
            push_sep(&mut cur_x, &mut seps, g1);
            let ann_btns = [
                (ToolbarAction::ToolSelect, UiIcon::Select, self.mouse_mode == MouseMode::Select),
                (ToolbarAction::ToolHighlight, UiIcon::Highlight, self.mouse_mode == MouseMode::Highlight),
                (ToolbarAction::ToolUnderline, UiIcon::Underline, self.mouse_mode == MouseMode::Underline),
                (ToolbarAction::ToolStrikethrough, UiIcon::Strikethrough, self.mouse_mode == MouseMode::Strikethrough),
                (ToolbarAction::ToolStickyNote, UiIcon::Note, self.mouse_mode == MouseMode::StickyNote),
                (ToolbarAction::ToolInkPen, UiIcon::Pen, self.mouse_mode == MouseMode::InkPen),
                (ToolbarAction::ToolFreeText, UiIcon::TextBox, self.mouse_mode == MouseMode::FreeText),
                (ToolbarAction::ToolRectShape, UiIcon::RectShape, self.mouse_mode == MouseMode::RectShape),
                (ToolbarAction::ToolArrowShape, UiIcon::ArrowShape, self.mouse_mode == MouseMode::ArrowShape),
            ];
            for (act, ic, active) in ann_btns {
                if try_push(mk_ic(act, ic, active, false), &mut cur_x, &mut widgets) {
                    g2 += 1;
                }
            }
            // Clickable Annotation Color Swatch button
            if try_push(
                ToolbarWidgetRect {
                    x: 0,
                    y: btn_y,
                    w: sq_w,
                    h: btn_h,
                    action: ToolbarAction::CycleAnnotationColor,
                    icon: None,
                    label: None,
                    swatch_rgb: Some(self.annotation_color.u32_rgb()),
                    active: false,
                    accent_dot: false,
                },
                &mut cur_x,
                &mut widgets,
            ) {
                g2 += 1;
            }
        }

        // Group 3: Edit Tools (Redact, Crop, Stamp) + Rotate + Search
        let want_g3 = self.toolbar_config.is_enabled(ToolbarItem::EditTools)
            || self.toolbar_config.is_enabled(ToolbarItem::Rotate)
            || self.toolbar_config.is_enabled(ToolbarItem::Search);
        if want_g3 {
            push_sep(&mut cur_x, &mut seps, g1 + g2);
            if self.toolbar_config.is_enabled(ToolbarItem::EditTools) {
                for (act, ic, active) in [
                    (ToolbarAction::ToolRedact, UiIcon::Redact, self.mouse_mode == MouseMode::Redact),
                    (ToolbarAction::ToolCrop, UiIcon::Crop, self.mouse_mode == MouseMode::Crop),
                    (ToolbarAction::ToolStamp, UiIcon::Stamp, self.mouse_mode == MouseMode::StampImage),
                ] {
                    try_push(mk_ic(act, ic, active, false), &mut cur_x, &mut widgets);
                }
            }
            if self.toolbar_config.is_enabled(ToolbarItem::Rotate) {
                try_push(mk_ic(ToolbarAction::RotatePage, UiIcon::Rotate, false, false), &mut cur_x, &mut widgets);
            }
            if self.toolbar_config.is_enabled(ToolbarItem::Search) {
                try_push(
                    mk_ic(ToolbarAction::FindText, UiIcon::Search, matches!(self.input_mode, InputMode::SearchText { .. }), false),
                    &mut cur_x,
                    &mut widgets,
                );
            }
        }

        (widgets, seps)
    }

    pub(super) fn draw_inline_toolbar(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        start_x: i32,
        bar_h: u32,
        sf: f64,
    ) {
        let (widgets, seps) = self.compute_inline_toolbar_layout(start_x, buf_w, bar_h, sf);
        if widgets.is_empty() && seps.is_empty() {
            return;
        }
        let theme = self.ui_theme();
        let s = sf as f32;
        let icon_sz = (13.0 * s).round().clamp(10.0, 28.0) as u32;
        let font_sz = (10.5 * s).clamp(8.5, 20.0);
        let btn_rad = (4.5 * s).clamp(3.0, 8.0);

        let sep_h = (16.0 * s).round().max(10.0) as u32;
        let sep_y = ((bar_h.saturating_sub(sep_h)) / 2) as i32;
        for sep in &seps {
            fill_rect_clipped(buffer, buf_w, buf_h, sep.0, sep_y, 1, sep_h, theme.divider);
        }

        for w in &widgets {
            let is_hov = self.hovered_widget == Some(UiHit::Toolbar(w.action));
            if w.active {
                fill_rounded_rect(buffer, buf_w, buf_h, w.x, w.y, w.w, w.h, btn_rad, theme.accent_soft_bg);
                stroke_rounded_rect(buffer, buf_w, buf_h, w.x, w.y, w.w, w.h, btn_rad, theme.accent);
            } else if is_hov {
                fill_rounded_rect(buffer, buf_w, buf_h, w.x, w.y, w.w, w.h, btn_rad, theme.hover_bg);
            }

            if let Some(swatch) = w.swatch_rgb {
                let cx = w.x as f32 + w.w as f32 * 0.5;
                let cy = w.y as f32 + w.h as f32 * 0.5;
                let r = (5.5 * s).clamp(4.0, 10.0);
                fill_circle_aa(buffer, buf_w, buf_h, cx, cy, r + 1.2, theme.input_border);
                fill_circle_aa(buffer, buf_w, buf_h, cx, cy, r, swatch);
                continue;
            }

            let fg = if w.active {
                theme.accent_soft_fg
            } else if is_hov {
                theme.text_primary
            } else {
                theme.text_secondary
            };

            if let Some(ic) = w.icon {
                let ix = w.x + ((w.w.saturating_sub(icon_sz)) / 2) as i32;
                let iy = w.y + ((w.h.saturating_sub(icon_sz)) / 2) as i32;
                draw_icon(buffer, buf_w, buf_h, ic, ix, iy, icon_sz, fg);
            } else if let Some(ref lbl) = w.label {
                let lw = measure_text(lbl, font_sz, w.active);
                let tx = w.x + ((w.w as f32 - lw) * 0.5).round() as i32;
                draw_text_vcenter(buffer, buf_w, buf_h, tx, w.y, w.h, w.x + w.w as i32 - 2, lbl, font_sz, fg, w.active);
            }

            if w.accent_dot {
                let dr = (2.5 * s).max(2.0);
                fill_circle_aa(
                    buffer,
                    buf_w,
                    buf_h,
                    (w.x + w.w as i32) as f32 - dr - 2.0,
                    w.y as f32 + dr + 2.0,
                    dr,
                    theme.accent,
                );
            }
        }
    }

    pub(super) fn hit_test_inline_toolbar(
        &self,
        mx: f64,
        my: f64,
        start_x: i32,
        buf_w: u32,
        bar_h: u32,
        sf: f64,
    ) -> Option<ToolbarAction> {
        if !self.show_toolbar || my < 0.0 || my >= bar_h as f64 {
            return None;
        }
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        let (widgets, _) = self.compute_inline_toolbar_layout(start_x, buf_w, bar_h, sf);
        for w in widgets {
            if ix >= w.x && ix < w.x + w.w as i32 && iy >= w.y && iy < w.y + w.h as i32 {
                return Some(w.action);
            }
        }
        None
    }

    pub(super) fn execute_toolbar_action(&mut self, action: ToolbarAction) {
        self.open_menu = None;
        self.active_note_popover = None;
        match action {
            ToolbarAction::ToggleSidebar => {
                self.show_bookmarks = !self.show_bookmarks;
                self.invalidate_page_caches();
            }
            ToolbarAction::ToggleGrid => {
                self.toggle_view_mode();
            }
            ToolbarAction::Open => {
                self.pending_dialog = Some(PendingDialog::Open);
            }
            ToolbarAction::Save => {
                if self.doc.is_some() {
                    self.pending_dialog = Some(PendingDialog::Save);
                }
            }
            ToolbarAction::SaveAs => {
                if self.doc.is_some() {
                    self.pending_dialog = Some(PendingDialog::SaveAs);
                }
            }
            ToolbarAction::Print => {
                self.trigger_print();
                return;
            }
            ToolbarAction::ToolSelect => {
                self.mouse_mode = MouseMode::Select;
                self.status_notice = Some("Mouse mode: Select Text".to_string());
            }
            ToolbarAction::ToolHighlight => {
                self.mouse_mode = MouseMode::Highlight;
                self.status_notice = Some(format!(
                    "Annotate: Highlight ({}) — drag across text",
                    self.annotation_color.label()
                ));
            }
            ToolbarAction::ToolUnderline => {
                self.mouse_mode = MouseMode::Underline;
                self.status_notice = Some(format!(
                    "Annotate: Underline ({}) — drag across text",
                    self.annotation_color.label()
                ));
            }
            ToolbarAction::ToolStrikethrough => {
                self.mouse_mode = MouseMode::Strikethrough;
                self.status_notice = Some(format!(
                    "Annotate: Strikethrough ({}) — drag across text",
                    self.annotation_color.label()
                ));
            }
            ToolbarAction::ToolStickyNote => {
                self.mouse_mode = MouseMode::StickyNote;
                self.status_notice = Some(format!(
                    "Annotate: Sticky Note ({}) — click anywhere on page to place a note",
                    self.annotation_color.label()
                ));
            }
            ToolbarAction::ToolInkPen => {
                self.mouse_mode = MouseMode::InkPen;
                self.status_notice = Some(format!(
                    "Annotate: Freehand Pen ({}) — drag to draw",
                    self.annotation_color.label()
                ));
            }
            ToolbarAction::ToolFreeText => {
                self.mouse_mode = MouseMode::FreeText;
                self.status_notice = Some(format!(
                    "Annotate: Text Box ({}) — click anywhere on page to add text",
                    self.annotation_color.label()
                ));
            }
            ToolbarAction::ToolRectShape => {
                self.mouse_mode = MouseMode::RectShape;
                self.status_notice = Some(format!(
                    "Annotate: Rectangle ({}) — drag to draw box",
                    self.annotation_color.label()
                ));
            }
            ToolbarAction::ToolArrowShape => {
                self.mouse_mode = MouseMode::ArrowShape;
                self.status_notice = Some(format!(
                    "Annotate: Arrow ({}) — drag from tail to tip",
                    self.annotation_color.label()
                ));
            }
            ToolbarAction::CycleAnnotationColor => {
                self.annotation_color = self.annotation_color.next();
                self.status_notice = Some(format!(
                    "Annotation color: {}",
                    self.annotation_color.label()
                ));
            }
            ToolbarAction::ToolRedact => {
                self.mouse_mode = MouseMode::Redact;
                self.status_notice = Some("REDACT mode: drag across sensitive region".to_string());
            }
            ToolbarAction::ToolCrop => {
                self.mouse_mode = MouseMode::Crop;
                self.status_notice = Some("CROP mode: drag region to crop page".to_string());
            }
            ToolbarAction::ToolStamp => {
                if self.doc.is_some() {
                    self.pending_dialog = Some(PendingDialog::PickStampImage);
                }
            }
            ToolbarAction::RotatePage => {
                if self.doc.is_some() {
                    let p = self.current_page_index;
                    self.rotate_page_clockwise(p);
                }
            }
            ToolbarAction::FindText => {
                if self.doc.is_some() {
                    self.input_mode = InputMode::SearchText {
                        query_input: self.search_query.clone(),
                    };
                }
            }
            ToolbarAction::UiScaleDown => {
                self.adjust_ui_scale(-0.10);
                return;
            }
            ToolbarAction::UiScaleReset => {
                self.reset_ui_scale();
                return;
            }
            ToolbarAction::UiScaleUp => {
                self.adjust_ui_scale(0.10);
                return;
            }
            ToolbarAction::ToggleTheme => {
                self.ui_theme_mode = self.ui_theme_mode.toggle();
                self.status_notice = Some(format!("UI theme: {}", self.ui_theme_mode.label()));
                self.save_session_to_ini();
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    // -------------------------------------------------------------------------
    // "Customize Toolbar..." modal checklist card
    // -------------------------------------------------------------------------

    fn customize_modal_geometry(buf_w: u32, buf_h: u32, sf: f64) -> (i32, i32, u32, u32) {
        let s = sf as f32;
        let mw = ((420.0 * s).round() as u32).min(buf_w.saturating_sub(28));
        let row_h = (28.0 * s).round() as u32;
        let mh = ((108.0 * s).round() as u32 + row_h * ToolbarItem::COUNT as u32)
            .min(buf_h.saturating_sub(28));
        let mx = (buf_w.saturating_sub(mw) / 2) as i32;
        let my = (buf_h.saturating_sub(mh) / 2) as i32;
        (mx, my, mw, mh)
    }

    pub(super) fn draw_customize_toolbar_modal(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) {
        if !self.show_customize_toolbar {
            return;
        }
        let theme = self.ui_theme();
        let s = sf as f32;
        blend_rect_rgba(buffer, buf_w, buf_h, 0, 0, buf_w, buf_h, theme.modal_scrim);

        let (mx, my, mw, mh) = Self::customize_modal_geometry(buf_w, buf_h, sf);
        let rad = (10.0 * s).clamp(6.0, 16.0);
        draw_soft_shadow(
            buffer, buf_w, buf_h, mx, my, mw, mh, rad,
            (10.0 * s).round() as i32, (4.0 * s).round() as i32, theme.shadow_rgb, theme.shadow_max_alpha,
        );
        fill_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.border_subtle);

        let pad = (16.0 * s).round() as i32;
        let title_sz = (14.5 * s).clamp(11.0, 32.0);
        let row_sz = (12.0 * s).clamp(10.0, 26.0);
        let row_h = (28.0 * s).round() as i32;

        draw_text(
            buffer, buf_w, buf_h,
            mx + pad, my + (14.0 * s).round() as i32,
            "Customize Top Toolbar",
            title_sz, theme.text_primary, true,
        );
        fill_rect_clipped(buffer, buf_w, buf_h, mx + 1, my + (42.0 * s).round() as i32, mw.saturating_sub(2), 1, theme.divider);

        let list_top = my + (50.0 * s).round() as i32;
        let cb_sz = (15.0 * s).round().clamp(12.0, 28.0) as u32;
        let check_ic = (11.0 * s).round().max(8.0) as u32;

        for (i, item) in ToolbarItem::ALL.iter().enumerate() {
            let ry = list_top + (i as i32) * row_h;
            if ry + row_h > my + mh as i32 - (44.0 * s).round() as i32 {
                break;
            }
            let is_hov = self.hovered_widget == Some(UiHit::Customize(CustomizeHit::ToggleItem(*item)));
            if is_hov {
                fill_rounded_rect(
                    buffer, buf_w, buf_h,
                    mx + (8.0 * s).round() as i32,
                    ry + 1,
                    mw.saturating_sub((16.0 * s).round() as u32),
                    (row_h - 2).max(1) as u32,
                    (5.0 * s).max(3.0),
                    theme.hover_bg,
                );
            }
            let checked = self.toolbar_config.is_enabled(*item);
            let cbx = mx + pad;
            let cby = ry + (row_h - cb_sz as i32) / 2;
            let cb_rad = (3.5 * s).clamp(2.5, 6.0);
            if checked {
                fill_rounded_rect(buffer, buf_w, buf_h, cbx, cby, cb_sz, cb_sz, cb_rad, theme.accent);
                draw_icon(
                    buffer, buf_w, buf_h, UiIcon::Check,
                    cbx + ((cb_sz.saturating_sub(check_ic)) / 2) as i32,
                    cby + ((cb_sz.saturating_sub(check_ic)) / 2) as i32,
                    check_ic, theme.text_on_accent,
                );
            } else {
                fill_rounded_rect(buffer, buf_w, buf_h, cbx, cby, cb_sz, cb_sz, cb_rad, theme.surface_bg);
                stroke_rounded_rect(buffer, buf_w, buf_h, cbx, cby, cb_sz, cb_sz, cb_rad, theme.input_border);
            }
            draw_text_vcenter(
                buffer, buf_w, buf_h,
                cbx + cb_sz as i32 + (10.0 * s).round() as i32,
                ry, row_h as u32,
                mx + mw as i32 - pad,
                item.label(), row_sz, theme.text_primary, false,
            );
        }

        // Footer buttons: Reset Defaults | Done
        let btn_h = (28.0 * s).round() as u32;
        let btn_y = my + mh as i32 - (40.0 * s).round() as i32;
        let reset_w = (112.0 * s).round() as u32;
        let done_w = (78.0 * s).round() as u32;
        let reset_x = mx + pad;
        let done_x = mx + mw as i32 - pad - done_w as i32;
        let btn_rad = (6.0 * s).clamp(4.0, 10.0);

        let r_hov = self.hovered_widget == Some(UiHit::Customize(CustomizeHit::ResetDefaults));
        fill_rounded_rect(buffer, buf_w, buf_h, reset_x, btn_y, reset_w, btn_h, btn_rad, if r_hov { theme.hover_bg } else { theme.surface_elevated });
        stroke_rounded_rect(buffer, buf_w, buf_h, reset_x, btn_y, reset_w, btn_h, btn_rad, theme.border_subtle);
        let r_lbl = "Reset Defaults";
        let rw = measure_text(r_lbl, row_sz, false);
        draw_text_vcenter(buffer, buf_w, buf_h, reset_x + ((reset_w as f32 - rw) * 0.5).round() as i32, btn_y, btn_h, reset_x + reset_w as i32, r_lbl, row_sz, theme.text_primary, false);

        let d_hov = self.hovered_widget == Some(UiHit::Customize(CustomizeHit::Done));
        fill_rounded_rect(buffer, buf_w, buf_h, done_x, btn_y, done_w, btn_h, btn_rad, if d_hov { theme.accent_hover } else { theme.accent });
        let d_lbl = "Done";
        let dw = measure_text(d_lbl, row_sz, true);
        draw_text_vcenter(buffer, buf_w, buf_h, done_x + ((done_w as f32 - dw) * 0.5).round() as i32, btn_y, btn_h, done_x + done_w as i32, d_lbl, row_sz, theme.text_on_accent, true);
    }

    pub(super) fn hit_test_customize_toolbar_modal(
        &self,
        mx_px: f64,
        my_px: f64,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<CustomizeHit> {
        if !self.show_customize_toolbar {
            return None;
        }
        let s = sf as f32;
        let (mx, my, mw, mh) = Self::customize_modal_geometry(buf_w, buf_h, sf);
        let ix = mx_px.round() as i32;
        let iy = my_px.round() as i32;
        if ix < mx || ix >= mx + mw as i32 || iy < my || iy >= my + mh as i32 {
            return None;
        }
        let pad = (16.0 * s).round() as i32;
        let btn_h = (28.0 * s).round() as i32;
        let btn_y = my + mh as i32 - (40.0 * s).round() as i32;
        let reset_w = (112.0 * s).round() as i32;
        let done_w = (78.0 * s).round() as i32;
        let reset_x = mx + pad;
        let done_x = mx + mw as i32 - pad - done_w;

        if iy >= btn_y && iy < btn_y + btn_h {
            if ix >= reset_x && ix < reset_x + reset_w {
                return Some(CustomizeHit::ResetDefaults);
            }
            if ix >= done_x && ix < done_x + done_w {
                return Some(CustomizeHit::Done);
            }
        }

        let list_top = my + (50.0 * s).round() as i32;
        let row_h = (28.0 * s).round() as i32;
        if iy >= list_top && iy < btn_y - 4 {
            let idx = ((iy - list_top) / row_h.max(1)) as usize;
            if let Some(&item) = ToolbarItem::ALL.get(idx) {
                return Some(CustomizeHit::ToggleItem(item));
            }
        }

        Some(CustomizeHit::InsideModal)
    }

    pub(super) fn handle_customize_toolbar_click(&mut self, hit: CustomizeHit) {
        match hit {
            CustomizeHit::ToggleItem(item) => {
                self.toolbar_config.toggle(item);
                self.show_toolbar = true;
                self.save_session_to_ini();
            }
            CustomizeHit::ResetDefaults => {
                self.toolbar_config = ToolbarConfig::default();
                self.show_toolbar = true;
                self.save_session_to_ini();
                self.status_notice = Some("Toolbar reset to defaults".to_string());
            }
            CustomizeHit::Done => {
                self.show_customize_toolbar = false;
            }
            CustomizeHit::InsideModal => {}
        }
        self.request_redraw();
    }
}

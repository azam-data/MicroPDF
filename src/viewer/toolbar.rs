//! Customizable compact icon toolbar rendered inline beside the menus on the unified top bar,
//! plus the lightweight "Customize Toolbar..." checklist overlay.

use super::state::*;
use super::ui_draw::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ToolbarItem {
    Sidebar = 0,
    Grid = 1,
    Open = 2,
    Save = 3,
    Print = 4,
    PageNav = 5,
    Zoom = 6,
    ViewModes = 7,
    Tools = 8,
    Rotate = 9,
    Search = 10,
    ReadingFilter = 11,
    ThemeToggle = 12,
}

impl ToolbarItem {
    pub const COUNT: usize = 13;

    pub const ALL: [ToolbarItem; Self::COUNT] = [
        ToolbarItem::Sidebar,
        ToolbarItem::Grid,
        ToolbarItem::Open,
        ToolbarItem::Save,
        ToolbarItem::Print,
        ToolbarItem::PageNav,
        ToolbarItem::Zoom,
        ToolbarItem::ViewModes,
        ToolbarItem::Tools,
        ToolbarItem::Rotate,
        ToolbarItem::Search,
        ToolbarItem::ReadingFilter,
        ToolbarItem::ThemeToggle,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Sidebar => "sidebar",
            Self::Grid => "grid",
            Self::Open => "open",
            Self::Save => "save",
            Self::Print => "print",
            Self::PageNav => "pages",
            Self::Zoom => "zoom",
            Self::ViewModes => "view_modes",
            Self::Tools => "tools",
            Self::Rotate => "rotate",
            Self::Search => "search",
            Self::ReadingFilter => "filter",
            Self::ThemeToggle => "theme",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Sidebar => "Bookmarks Sidebar (B)",
            Self::Grid => "Thumbnail Grid (Tab)",
            Self::Open => "Open PDF (O)",
            Self::Save => "Save PDF (S)",
            Self::Print => "Print Preview (Ctrl+P)",
            Self::PageNav => "Page Navigation & Jump",
            Self::Zoom => "Zoom Controls (- / % / +)",
            Self::ViewModes => "View Mode Buttons (1-5)",
            Self::Tools => "Mouse Tools (Select/Highlight/Redact/Crop/Stamp)",
            Self::Rotate => "Rotate Page 90° (R)",
            Self::Search => "Find Text (Ctrl+F)",
            Self::ReadingFilter => "Reading Filter (Normal/Dark/Sepia)",
            Self::ThemeToggle => "Light / Dark UI Toggle",
        }
    }

    pub fn from_key(s: &str) -> Option<Self> {
        let k = s.trim();
        Self::ALL.into_iter().find(|item| item.key().eq_ignore_ascii_case(k))
    }

    pub fn default_enabled(self) -> bool {
        !matches!(self, Self::ViewModes | Self::ReadingFilter)
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
        if trimmed.eq_ignore_ascii_case("none") {
            return Self {
                enabled: [false; ToolbarItem::COUNT],
            };
        }
        let mut cfg = Self {
            enabled: [false; ToolbarItem::COUNT],
        };
        let mut any = false;
        for part in trimmed.split(',') {
            if let Some(item) = ToolbarItem::from_key(part) {
                cfg.enabled[item as usize] = true;
                any = true;
            }
        }
        if any { cfg } else { Self::default() }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ToolbarAction {
    ToggleSidebar,
    ToggleGrid,
    OpenPdf,
    SavePdf,
    PrintPreview,
    PrevPage,
    JumpPage,
    NextPage,
    ZoomOut,
    ResetZoom,
    ZoomIn,
    ViewFitPage,
    ViewFitWidth,
    ViewTwoPage,
    ViewContinuous,
    ToolSelect,
    ToolHighlight,
    ToolRedact,
    ToolCrop,
    ToolStamp,
    RotatePage,
    Search,
    CycleFilter,
    ToggleTheme,
}

impl ToolbarAction {
    pub fn tooltip(self) -> &'static str {
        match self {
            Self::ToggleSidebar => "Toggle Bookmarks Sidebar (B)",
            Self::ToggleGrid => "Toggle Thumbnail Grid (Tab)",
            Self::OpenPdf => "Open PDF in Tab (O)",
            Self::SavePdf => "Save PDF (S)",
            Self::PrintPreview => "Print Preview & Print (Ctrl/Cmd+P)",
            Self::PrevPage => "Previous Page (Left)",
            Self::JumpPage => "Jump to Page (G)",
            Self::NextPage => "Next Page (Right)",
            Self::ZoomOut => "Zoom Out (-)",
            Self::ResetZoom => "Reset Zoom to 100% (0)",
            Self::ZoomIn => "Zoom In (+)",
            Self::ViewFitPage => "Fit Full Page (1)",
            Self::ViewFitWidth => "Fit Page Width (2)",
            Self::ViewTwoPage => "Two-Page Spread (4)",
            Self::ViewContinuous => "Continuous Scroll (5)",
            Self::ToolSelect => "Mouse Mode: Select Text",
            Self::ToolHighlight => "Mouse Mode: Highlight Text",
            Self::ToolRedact => "Mouse Mode: Black-Box Redact",
            Self::ToolCrop => "Mouse Mode: Crop Page Area",
            Self::ToolStamp => "Stamp Signature / Image (P)",
            Self::RotatePage => "Rotate Current Page 90° (R)",
            Self::Search => "Find Text in PDF (Ctrl/Cmd+F)",
            Self::CycleFilter => "Cycle Reading Filter: Normal / Dark / Sepia (V)",
            Self::ToggleTheme => "Switch Light / Dark UI Theme",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct ToolbarWidgetRect {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) w: u32,
    pub(super) h: u32,
    pub(super) action: ToolbarAction,
    pub(super) icon: Option<UiIcon>,
    pub(super) text: Option<String>,
    pub(super) active: bool,
    pub(super) accent_kind: ToolAccent,
    pub(super) pill_outline: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum ToolAccent {
    Primary,
    Warning,
    Danger,
    Success,
    Violet,
}

#[derive(Copy, Clone, Debug)]
pub(super) struct ToolbarSepX(pub(super) i32);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum CustomizeHit {
    ToggleItem(ToolbarItem),
    ResetDefaults,
    Close,
    InsideCard,
}

impl<'a> PdfViewerApp<'a> {
    /// Computes responsive toolbar widget rectangles placed inline beside the menu headers
    /// on the unified top bar (`x_start .. buf_w - right_pad`).
    pub(super) fn layout_unified_toolbar(
        &self,
        start_x: i32,
        buf_w: u32,
        topbar_h: u32,
        sf: f64,
    ) -> (Vec<ToolbarWidgetRect>, Vec<ToolbarSepX>) {
        if !self.show_toolbar || topbar_h == 0 {
            return (Vec::new(), Vec::new());
        }
        let s = sf as f32;
        let btn_h = (22.0 * s).round().clamp(18.0, (topbar_h.saturating_sub(6)) as f32) as u32;
        let btn_y = ((topbar_h.saturating_sub(btn_h)) / 2) as i32;
        let icon_btn_w = (24.0 * s).round() as u32;
        let gap = (2.0 * s).round().max(1.0) as i32;
        let sep_pad = (6.0 * s).round().max(4.0) as i32;
        let font_sz = (11.5 * s).clamp(10.0, 26.0);
        let right_margin = (8.0 * s).round() as i32;

        // Reserve space on the far right for ThemeToggle (if enabled) so it always pins right cleanly.
        let pin_theme = self.toolbar_config.is_enabled(ToolbarItem::ThemeToggle);
        let theme_w = if pin_theme { icon_btn_w } else { 0 };
        let max_left_x = (buf_w as i32) - right_margin - if pin_theme { theme_w as i32 + sep_pad } else { 0 };

        let mut widgets: Vec<ToolbarWidgetRect> = Vec::new();
        let mut seps: Vec<ToolbarSepX> = Vec::new();
        let mut cur_x = start_x;

        if cur_x + (icon_btn_w as i32) >= max_left_x {
            // Only enough space for pinned theme button
            if pin_theme && (buf_w as i32) - right_margin - (theme_w as i32) > start_x {
                widgets.push(ToolbarWidgetRect {
                    x: (buf_w as i32) - right_margin - (theme_w as i32),
                    y: btn_y,
                    w: theme_w,
                    h: btn_h,
                    action: ToolbarAction::ToggleTheme,
                    icon: Some(match self.ui_theme_mode {
                        super::theme::UiThemeMode::Light => UiIcon::Moon,
                        super::theme::UiThemeMode::Dark => UiIcon::Sun,
                    }),
                    text: None,
                    active: false,
                    accent_kind: ToolAccent::Primary,
                    pill_outline: false,
                });
            }
            return (widgets, seps);
        }

        // Initial divider between `Help` and toolbar
        seps.push(ToolbarSepX(cur_x));
        cur_x += sep_pad;

        let push_sep = |cur_x: &mut i32, seps: &mut Vec<ToolbarSepX>, widgets: &[ToolbarWidgetRect]| {
            if !widgets.is_empty() && *cur_x + sep_pad * 2 < max_left_x {
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

        // Group 1: Navigation & File actions
        let mut g1_added = false;
        if self.toolbar_config.is_enabled(ToolbarItem::Sidebar) {
            g1_added |= try_push(
                ToolbarWidgetRect {
                    x: 0, y: btn_y, w: icon_btn_w, h: btn_h,
                    action: ToolbarAction::ToggleSidebar,
                    icon: Some(UiIcon::Sidebar), text: None,
                    active: self.show_bookmarks, accent_kind: ToolAccent::Primary, pill_outline: false,
                },
                &mut cur_x, &mut widgets,
            );
        }
        if self.toolbar_config.is_enabled(ToolbarItem::Grid) {
            g1_added |= try_push(
                ToolbarWidgetRect {
                    x: 0, y: btn_y, w: icon_btn_w, h: btn_h,
                    action: ToolbarAction::ToggleGrid,
                    icon: Some(UiIcon::Grid), text: None,
                    active: self.view_mode == ViewMode::ThumbnailGrid,
                    accent_kind: ToolAccent::Primary, pill_outline: false,
                },
                &mut cur_x, &mut widgets,
            );
        }
        if self.toolbar_config.is_enabled(ToolbarItem::Open) {
            g1_added |= try_push(
                ToolbarWidgetRect {
                    x: 0, y: btn_y, w: icon_btn_w, h: btn_h,
                    action: ToolbarAction::OpenPdf,
                    icon: Some(UiIcon::Open), text: None,
                    active: false, accent_kind: ToolAccent::Primary, pill_outline: false,
                },
                &mut cur_x, &mut widgets,
            );
        }
        if self.toolbar_config.is_enabled(ToolbarItem::Save) {
            g1_added |= try_push(
                ToolbarWidgetRect {
                    x: 0, y: btn_y, w: icon_btn_w, h: btn_h,
                    action: ToolbarAction::SavePdf,
                    icon: Some(UiIcon::Save), text: None,
                    active: self.has_unsaved_edits, accent_kind: ToolAccent::Primary, pill_outline: false,
                },
                &mut cur_x, &mut widgets,
            );
        }
        if self.toolbar_config.is_enabled(ToolbarItem::Print) {
            g1_added |= try_push(
                ToolbarWidgetRect {
                    x: 0, y: btn_y, w: icon_btn_w, h: btn_h,
                    action: ToolbarAction::PrintPreview,
                    icon: Some(UiIcon::Print), text: None,
                    active: self.print_preview.is_some(), accent_kind: ToolAccent::Primary, pill_outline: false,
                },
                &mut cur_x, &mut widgets,
            );
        }

        // Group 2: Page Navigation
        if self.toolbar_config.is_enabled(ToolbarItem::PageNav) {
            let total = self.total_pages();
            let cur = if total > 0 { self.current_page_index + 1 } else { 0 };
            let page_str = format!("{cur}/{total}");
            let pill_w = (measure_text(&page_str, font_sz, false).ceil() as u32 + (14.0 * s).round() as u32)
                .max((40.0 * s).round() as u32);
            let nav_btn_w = (20.0 * s).round() as u32;
            let group_w = nav_btn_w * 2 + pill_w + (gap as u32) * 2;
            if cur_x + (group_w as i32) + sep_pad <= max_left_x {
                if g1_added {
                    push_sep(&mut cur_x, &mut seps, &widgets);
                }
                try_push(
                    ToolbarWidgetRect {
                        x: 0, y: btn_y, w: nav_btn_w, h: btn_h,
                        action: ToolbarAction::PrevPage,
                        icon: Some(UiIcon::ChevronLeft), text: None,
                        active: false, accent_kind: ToolAccent::Primary, pill_outline: false,
                    },
                    &mut cur_x, &mut widgets,
                );
                try_push(
                    ToolbarWidgetRect {
                        x: 0, y: btn_y, w: pill_w, h: btn_h,
                        action: ToolbarAction::JumpPage,
                        icon: None, text: Some(page_str),
                        active: false, accent_kind: ToolAccent::Primary, pill_outline: true,
                    },
                    &mut cur_x, &mut widgets,
                );
                try_push(
                    ToolbarWidgetRect {
                        x: 0, y: btn_y, w: nav_btn_w, h: btn_h,
                        action: ToolbarAction::NextPage,
                        icon: Some(UiIcon::ChevronRight), text: None,
                        active: false, accent_kind: ToolAccent::Primary, pill_outline: false,
                    },
                    &mut cur_x, &mut widgets,
                );
                g1_added = true;
            }
        }

        // Group 3: Zoom
        if self.toolbar_config.is_enabled(ToolbarItem::Zoom) {
            let zoom_str = format!("{}%", (self.zoom_level * 100.0).round() as u32);
            let pill_w = (measure_text(&zoom_str, font_sz, false).ceil() as u32 + (12.0 * s).round() as u32)
                .max((40.0 * s).round() as u32);
            let z_btn_w = (20.0 * s).round() as u32;
            let group_w = z_btn_w * 2 + pill_w + (gap as u32) * 2;
            if cur_x + (group_w as i32) + sep_pad <= max_left_x {
                if g1_added {
                    push_sep(&mut cur_x, &mut seps, &widgets);
                }
                try_push(
                    ToolbarWidgetRect {
                        x: 0, y: btn_y, w: z_btn_w, h: btn_h,
                        action: ToolbarAction::ZoomOut,
                        icon: Some(UiIcon::ZoomOut), text: None,
                        active: false, accent_kind: ToolAccent::Primary, pill_outline: false,
                    },
                    &mut cur_x, &mut widgets,
                );
                try_push(
                    ToolbarWidgetRect {
                        x: 0, y: btn_y, w: pill_w, h: btn_h,
                        action: ToolbarAction::ResetZoom,
                        icon: None, text: Some(zoom_str),
                        active: false, accent_kind: ToolAccent::Primary, pill_outline: false,
                    },
                    &mut cur_x, &mut widgets,
                );
                try_push(
                    ToolbarWidgetRect {
                        x: 0, y: btn_y, w: z_btn_w, h: btn_h,
                        action: ToolbarAction::ZoomIn,
                        icon: Some(UiIcon::ZoomIn), text: None,
                        active: false, accent_kind: ToolAccent::Primary, pill_outline: false,
                    },
                    &mut cur_x, &mut widgets,
                );
                g1_added = true;
            }
        }

        // Group 4: View Modes (optional)
        if self.toolbar_config.is_enabled(ToolbarItem::ViewModes) {
            let group_w = icon_btn_w * 4 + (gap as u32) * 3;
            if cur_x + (group_w as i32) + sep_pad <= max_left_x {
                if g1_added {
                    push_sep(&mut cur_x, &mut seps, &widgets);
                }
                let modes = [
                    (ToolbarAction::ViewFitPage, UiIcon::FitPage, ViewMode::FitPage),
                    (ToolbarAction::ViewFitWidth, UiIcon::FitWidth, ViewMode::FitWidth),
                    (ToolbarAction::ViewTwoPage, UiIcon::TwoPage, ViewMode::TwoPageSpread),
                    (ToolbarAction::ViewContinuous, UiIcon::Continuous, ViewMode::ContinuousScroll),
                ];
                for (act, ic, vm) in modes {
                    try_push(
                        ToolbarWidgetRect {
                            x: 0, y: btn_y, w: icon_btn_w, h: btn_h,
                            action: act, icon: Some(ic), text: None,
                            active: self.view_mode == vm, accent_kind: ToolAccent::Primary, pill_outline: false,
                        },
                        &mut cur_x, &mut widgets,
                    );
                }
                g1_added = true;
            }
        }

        // Group 5: Mouse Tools
        if self.toolbar_config.is_enabled(ToolbarItem::Tools) {
            let group_w = icon_btn_w * 5 + (gap as u32) * 4;
            if cur_x + (group_w as i32) + sep_pad <= max_left_x {
                if g1_added {
                    push_sep(&mut cur_x, &mut seps, &widgets);
                }
                let tools = [
                    (ToolbarAction::ToolSelect, UiIcon::Select, MouseMode::Select, ToolAccent::Primary),
                    (ToolbarAction::ToolHighlight, UiIcon::Highlight, MouseMode::Highlight, ToolAccent::Warning),
                    (ToolbarAction::ToolRedact, UiIcon::Redact, MouseMode::Redact, ToolAccent::Danger),
                    (ToolbarAction::ToolCrop, UiIcon::Crop, MouseMode::Crop, ToolAccent::Success),
                    (ToolbarAction::ToolStamp, UiIcon::Stamp, MouseMode::StampImage, ToolAccent::Violet),
                ];
                for (act, ic, mm, acc) in tools {
                    try_push(
                        ToolbarWidgetRect {
                            x: 0, y: btn_y, w: icon_btn_w, h: btn_h,
                            action: act, icon: Some(ic), text: None,
                            active: self.mouse_mode == mm, accent_kind: acc, pill_outline: false,
                        },
                        &mut cur_x, &mut widgets,
                    );
                }
                g1_added = true;
            }
        }

        // Group 6: Quick Utilities (Rotate, Search, ReadingFilter)
        let want_util = self.toolbar_config.is_enabled(ToolbarItem::Rotate)
            || self.toolbar_config.is_enabled(ToolbarItem::Search)
            || self.toolbar_config.is_enabled(ToolbarItem::ReadingFilter);
        if want_util && g1_added && cur_x + (icon_btn_w as i32) + sep_pad <= max_left_x {
            push_sep(&mut cur_x, &mut seps, &widgets);
        }
        if self.toolbar_config.is_enabled(ToolbarItem::Rotate) {
            try_push(
                ToolbarWidgetRect {
                    x: 0, y: btn_y, w: icon_btn_w, h: btn_h,
                    action: ToolbarAction::RotatePage, icon: Some(UiIcon::Rotate), text: None,
                    active: false, accent_kind: ToolAccent::Primary, pill_outline: false,
                },
                &mut cur_x, &mut widgets,
            );
        }
        if self.toolbar_config.is_enabled(ToolbarItem::Search) {
            try_push(
                ToolbarWidgetRect {
                    x: 0, y: btn_y, w: icon_btn_w, h: btn_h,
                    action: ToolbarAction::Search, icon: Some(UiIcon::Search), text: None,
                    active: matches!(self.input_mode, InputMode::SearchText { .. }) || !self.search_matches.is_empty(),
                    accent_kind: ToolAccent::Primary, pill_outline: false,
                },
                &mut cur_x, &mut widgets,
            );
        }
        if self.toolbar_config.is_enabled(ToolbarItem::ReadingFilter) {
            try_push(
                ToolbarWidgetRect {
                    x: 0, y: btn_y, w: icon_btn_w, h: btn_h,
                    action: ToolbarAction::CycleFilter, icon: Some(UiIcon::Filter), text: None,
                    active: self.reading_filter != ReadingFilter::Normal,
                    accent_kind: ToolAccent::Warning, pill_outline: false,
                },
                &mut cur_x, &mut widgets,
            );
        }

        // Far-right pinned Light / Dark Theme toggle
        if pin_theme {
            let tx = (buf_w as i32) - right_margin - (theme_w as i32);
            if tx >= cur_x {
                widgets.push(ToolbarWidgetRect {
                    x: tx,
                    y: btn_y,
                    w: theme_w,
                    h: btn_h,
                    action: ToolbarAction::ToggleTheme,
                    icon: Some(match self.ui_theme_mode {
                        super::theme::UiThemeMode::Light => UiIcon::Moon,
                        super::theme::UiThemeMode::Dark => UiIcon::Sun,
                    }),
                    text: None,
                    active: false,
                    accent_kind: ToolAccent::Primary,
                    pill_outline: false,
                });
            }
        }

        (widgets, seps)
    }

    pub(super) fn draw_unified_toolbar(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        start_x: i32,
        topbar_h: u32,
        sf: f64,
    ) {
        let theme = self.ui_theme();
        let s = sf as f32;
        let (widgets, seps) = self.layout_unified_toolbar(start_x, buf_w, topbar_h, sf);
        let sep_h = (16.0 * s).round() as u32;
        let sep_y = ((topbar_h.saturating_sub(sep_h)) / 2) as i32;
        for ToolbarSepX(sx) in seps {
            super::render::fill_rect_clipped(buffer, buf_w, buf_h, sx, sep_y, 1, sep_h, theme.divider);
        }

        let font_sz = (11.5 * s).clamp(10.0, 26.0);
        let rad = (5.0 * s).clamp(3.0, 10.0);

        for w in &widgets {
            let is_hover = self.hovered_widget == Some(UiHit::Toolbar(w.action));
            let (active_bg, active_fg) = match w.accent_kind {
                ToolAccent::Primary => (theme.accent_soft_bg, theme.accent_soft_fg),
                ToolAccent::Warning => (theme.warning_soft_bg, theme.warning),
                ToolAccent::Danger => (theme.danger_soft_bg, theme.danger),
                ToolAccent::Success => (theme.success_soft_bg, theme.success),
                ToolAccent::Violet => (theme.violet_soft_bg, theme.violet),
            };

            if w.active {
                fill_rounded_rect(buffer, buf_w, buf_h, w.x, w.y, w.w, w.h, rad, active_bg);
            } else if is_hover {
                fill_rounded_rect(buffer, buf_w, buf_h, w.x, w.y, w.w, w.h, rad, theme.hover_bg);
            } else if w.pill_outline {
                fill_rounded_rect(buffer, buf_w, buf_h, w.x, w.y, w.w, w.h, rad, theme.surface_elevated);
            }

            if w.pill_outline {
                stroke_rounded_rect(buffer, buf_w, buf_h, w.x, w.y, w.w, w.h, rad, theme.border_subtle);
            }

            let fg = if w.active {
                active_fg
            } else if is_hover {
                theme.text_primary
            } else {
                theme.text_secondary
            };

            if let Some(icon) = w.icon {
                let isize = (14.0 * s).round().clamp(11.0, (w.h.saturating_sub(4)) as f32) as u32;
                let ix = w.x + ((w.w.saturating_sub(isize)) / 2) as i32;
                let iy = w.y + ((w.h.saturating_sub(isize)) / 2) as i32;
                draw_icon(buffer, buf_w, buf_h, icon, ix, iy, isize, fg);
            } else if let Some(ref txt) = w.text {
                let tw = measure_text(txt, font_sz, false);
                let tx = w.x + (((w.w as f32) - tw) * 0.5).round() as i32;
                draw_text_vcenter(
                    buffer, buf_w, buf_h, tx, w.y, w.h, w.x + w.w as i32 - 2, txt, font_sz, fg, false,
                );
            }
        }
    }

    pub(super) fn hit_test_unified_toolbar(
        &self,
        mx: f64,
        my: f64,
        start_x: i32,
        buf_w: u32,
        topbar_h: u32,
        sf: f64,
    ) -> Option<ToolbarAction> {
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if iy < 0 || iy >= topbar_h as i32 {
            return None;
        }
        let (widgets, _) = self.layout_unified_toolbar(start_x, buf_w, topbar_h, sf);
        for w in widgets {
            if ix >= w.x && ix < w.x + w.w as i32 && iy >= w.y && iy < w.y + w.h as i32 {
                return Some(w.action);
            }
        }
        None
    }

    pub(super) fn execute_toolbar_action(&mut self, action: ToolbarAction) {
        self.open_menu = None;
        match action {
            ToolbarAction::ToggleSidebar => {
                self.show_bookmarks = !self.show_bookmarks;
                self.invalidate_page_caches();
                self.status_notice = Some(if self.show_bookmarks {
                    "Bookmarks Sidebar ON".to_string()
                } else {
                    "Bookmarks Sidebar OFF".to_string()
                });
            }
            ToolbarAction::ToggleGrid => self.toggle_view_mode(),
            ToolbarAction::OpenPdf => self.pending_dialog = Some(PendingDialog::Open),
            ToolbarAction::SavePdf => self.pending_dialog = Some(PendingDialog::Save),
            ToolbarAction::PrintPreview => self.trigger_print(),
            ToolbarAction::PrevPage => self.step_pages_backward(),
            ToolbarAction::JumpPage => {
                self.input_mode = InputMode::JumpToPage { input: String::new() };
            }
            ToolbarAction::NextPage => self.step_pages_forward(),
            ToolbarAction::ZoomOut => self.zoom_by_step(0.8),
            ToolbarAction::ResetZoom => self.reset_zoom(),
            ToolbarAction::ZoomIn => self.zoom_by_step(1.25),
            ToolbarAction::ViewFitPage => self.set_view_mode(ViewMode::FitPage),
            ToolbarAction::ViewFitWidth => self.set_view_mode(ViewMode::FitWidth),
            ToolbarAction::ViewTwoPage => self.set_view_mode(ViewMode::TwoPageSpread),
            ToolbarAction::ViewContinuous => self.set_view_mode(ViewMode::ContinuousScroll),
            ToolbarAction::ToolSelect => {
                self.mouse_mode = MouseMode::Select;
                self.status_notice = Some("Mouse Mode: Select Text".to_string());
            }
            ToolbarAction::ToolHighlight => {
                self.mouse_mode = MouseMode::Highlight;
                self.status_notice = Some("Mouse Mode: Highlight Text (drag across text)".to_string());
            }
            ToolbarAction::ToolRedact => {
                self.mouse_mode = MouseMode::Redact;
                self.status_notice = Some("Mouse Mode: Redact (drag red box to permanently black out)".to_string());
            }
            ToolbarAction::ToolCrop => {
                self.mouse_mode = MouseMode::Crop;
                self.status_notice = Some("Mouse Mode: Crop Page Area (drag green box)".to_string());
            }
            ToolbarAction::ToolStamp => {
                self.pending_dialog = Some(PendingDialog::PickStampImage);
            }
            ToolbarAction::RotatePage => {
                let target = if self.view_mode == ViewMode::ThumbnailGrid {
                    self.hovered_thumb.unwrap_or(self.current_page_index)
                } else {
                    self.current_page_index
                };
                self.rotate_page_clockwise(target);
            }
            ToolbarAction::Search => {
                self.input_mode = InputMode::SearchText {
                    query_input: self.search_query.clone(),
                };
            }
            ToolbarAction::CycleFilter => {
                self.reading_filter = self.reading_filter.next();
                self.status_notice = Some(format!("Reading Filter: {}", self.reading_filter.label()));
            }
            ToolbarAction::ToggleTheme => {
                self.ui_theme_mode = self.ui_theme_mode.toggle();
                self.status_notice = Some(format!("UI Theme: {}", self.ui_theme_mode.label()));
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    /// Computes geometry of the "Customize Toolbar..." modal card.
    pub(super) fn customize_toolbar_card_rect(&self, buf_w: u32, buf_h: u32, sf: f64) -> (i32, i32, u32, u32) {
        let s = sf as f32;
        let card_w = ((390.0 * s).round() as u32).min(buf_w.saturating_sub(24));
        let header_h = (44.0 * s).round() as u32;
        let row_h = (27.0 * s).round() as u32;
        let footer_h = (48.0 * s).round() as u32;
        let card_h = (header_h + (ToolbarItem::COUNT as u32) * row_h + footer_h + (12.0 * s).round() as u32)
            .min(buf_h.saturating_sub(24));
        let card_x = (buf_w.saturating_sub(card_w) / 2) as i32;
        let card_y = (buf_h.saturating_sub(card_h) / 2) as i32;
        (card_x, card_y, card_w, card_h)
    }

    pub(super) fn draw_customize_toolbar_modal(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) {
        let theme = self.ui_theme();
        let s = sf as f32;
        super::render::blend_rect_rgba(buffer, buf_w, buf_h, 0, 0, buf_w, buf_h, theme.modal_scrim);

        let (cx, cy, cw, ch) = self.customize_toolbar_card_rect(buf_w, buf_h, sf);
        let rad = (10.0 * s).clamp(6.0, 16.0);
        draw_soft_shadow(buffer, buf_w, buf_h, cx, cy, cw, ch, rad, (10.0 * s).round() as i32, (4.0 * s).round() as i32, theme.shadow_rgb, theme.shadow_max_alpha);
        fill_rounded_rect(buffer, buf_w, buf_h, cx, cy, cw, ch, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, cx, cy, cw, ch, rad, theme.border_subtle);

        let pad = (16.0 * s).round() as i32;
        let header_h = (44.0 * s).round() as u32;
        let title_sz = (14.5 * s).clamp(12.0, 32.0);
        let body_sz = (12.0 * s).clamp(10.0, 26.0);

        draw_text_vcenter(buffer, buf_w, buf_h, cx + pad, cy, header_h, cx + cw as i32 - pad, "Customize Toolbar", title_sz, theme.text_primary, true);
        super::render::fill_rect_clipped(buffer, buf_w, buf_h, cx + 1, cy + header_h as i32, cw.saturating_sub(2), 1, theme.divider);

        let row_h = (27.0 * s).round() as u32;
        let list_y = cy + header_h as i32 + (6.0 * s).round() as i32;
        let cb_sz = (15.0 * s).round() as u32;

        for (idx, item) in ToolbarItem::ALL.iter().enumerate() {
            let ry = list_y + (idx as i32) * (row_h as i32);
            if ry + row_h as i32 > cy + ch as i32 - (46.0 * s).round() as i32 {
                break;
            }
            let hovered = self.hovered_widget == Some(UiHit::Customize(CustomizeHit::ToggleItem(*item)));
            if hovered {
                fill_rounded_rect(buffer, buf_w, buf_h, cx + pad / 2, ry, cw.saturating_sub(pad as u32), row_h, (5.0 * s).max(3.0), theme.hover_bg);
            }
            let cbx = cx + pad;
            let cby = ry + ((row_h.saturating_sub(cb_sz)) / 2) as i32;
            let checked = self.toolbar_config.is_enabled(*item);
            if checked {
                fill_rounded_rect(buffer, buf_w, buf_h, cbx, cby, cb_sz, cb_sz, (3.5 * s).max(2.0), theme.accent);
                let ic_pad = (2.0 * s).round() as i32;
                draw_icon(buffer, buf_w, buf_h, UiIcon::Check, cbx + ic_pad, cby + ic_pad, cb_sz.saturating_sub((ic_pad * 2) as u32), theme.text_on_accent);
            } else {
                fill_rounded_rect(buffer, buf_w, buf_h, cbx, cby, cb_sz, cb_sz, (3.5 * s).max(2.0), theme.input_bg);
                stroke_rounded_rect(buffer, buf_w, buf_h, cbx, cby, cb_sz, cb_sz, (3.5 * s).max(2.0), theme.input_border);
            }
            draw_text_vcenter(
                buffer, buf_w, buf_h, cbx + cb_sz as i32 + (10.0 * s).round() as i32, ry, row_h,
                cx + cw as i32 - pad, item.label(), body_sz, theme.text_primary, false,
            );
        }

        // Footer buttons: Reset Defaults (left) & Done (right)
        let footer_h = (46.0 * s).round() as u32;
        let fy = cy + ch as i32 - footer_h as i32;
        super::render::fill_rect_clipped(buffer, buf_w, buf_h, cx + 1, fy, cw.saturating_sub(2), 1, theme.divider);

        let btn_h = (28.0 * s).round() as u32;
        let by = fy + ((footer_h.saturating_sub(btn_h)) / 2) as i32;
        let reset_w = (112.0 * s).round() as u32;
        let done_w = (76.0 * s).round() as u32;
        let reset_x = cx + pad;
        let done_x = cx + cw as i32 - pad - done_w as i32;

        let reset_hov = self.hovered_widget == Some(UiHit::Customize(CustomizeHit::ResetDefaults));
        fill_rounded_rect(buffer, buf_w, buf_h, reset_x, by, reset_w, btn_h, (6.0 * s).max(4.0), if reset_hov { theme.hover_bg } else { theme.surface_elevated });
        stroke_rounded_rect(buffer, buf_w, buf_h, reset_x, by, reset_w, btn_h, (6.0 * s).max(4.0), theme.border_subtle);
        let rtw = measure_text("Reset Defaults", body_sz, false);
        draw_text_vcenter(buffer, buf_w, buf_h, reset_x + ((reset_w as f32 - rtw) * 0.5).round() as i32, by, btn_h, reset_x + reset_w as i32, "Reset Defaults", body_sz, theme.text_primary, false);

        let done_hov = self.hovered_widget == Some(UiHit::Customize(CustomizeHit::Close));
        fill_rounded_rect(buffer, buf_w, buf_h, done_x, by, done_w, btn_h, (6.0 * s).max(4.0), if done_hov { theme.accent_hover } else { theme.accent });
        let dtw = measure_text("Done", body_sz, true);
        draw_text_vcenter(buffer, buf_w, buf_h, done_x + ((done_w as f32 - dtw) * 0.5).round() as i32, by, btn_h, done_x + done_w as i32, "Done", body_sz, theme.text_on_accent, true);
    }

    pub(super) fn hit_test_customize_toolbar_modal(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<CustomizeHit> {
        let s = sf as f32;
        let (cx, cy, cw, ch) = self.customize_toolbar_card_rect(buf_w, buf_h, sf);
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix < cx || ix >= cx + cw as i32 || iy < cy || iy >= cy + ch as i32 {
            return None;
        }
        let pad = (16.0 * s).round() as i32;
        let header_h = (44.0 * s).round() as u32;
        let row_h = (27.0 * s).round() as u32;
        let list_y = cy + header_h as i32 + (6.0 * s).round() as i32;
        let footer_h = (46.0 * s).round() as u32;
        let fy = cy + ch as i32 - footer_h as i32;

        if iy >= list_y && iy < fy {
            let idx = ((iy - list_y) / row_h as i32) as usize;
            if idx < ToolbarItem::COUNT {
                return Some(CustomizeHit::ToggleItem(ToolbarItem::ALL[idx]));
            }
        }
        let btn_h = (28.0 * s).round() as u32;
        let by = fy + ((footer_h.saturating_sub(btn_h)) / 2) as i32;
        let reset_w = (112.0 * s).round() as u32;
        let done_w = (76.0 * s).round() as u32;
        let reset_x = cx + pad;
        let done_x = cx + cw as i32 - pad - done_w as i32;
        if iy >= by && iy < by + btn_h as i32 {
            if ix >= reset_x && ix < reset_x + reset_w as i32 {
                return Some(CustomizeHit::ResetDefaults);
            }
            if ix >= done_x && ix < done_x + done_w as i32 {
                return Some(CustomizeHit::Close);
            }
        }
        Some(CustomizeHit::InsideCard)
    }
}

//! Centralized Light (Bright Daylight default) and Dark UI themes for MicroPDF.
//! All colors are stored as `0x00RRGGBB` matching the `softbuffer` pixel layout.

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UiThemeMode {
    Light,
    Dark,
}

impl Default for UiThemeMode {
    fn default() -> Self {
        UiThemeMode::Light
    }
}

impl UiThemeMode {
    pub fn toggle(self) -> Self {
        match self {
            UiThemeMode::Light => UiThemeMode::Dark,
            UiThemeMode::Dark => UiThemeMode::Light,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            UiThemeMode::Light => "Light",
            UiThemeMode::Dark => "Dark",
        }
    }

    pub fn to_ini_str(self) -> &'static str {
        match self {
            UiThemeMode::Light => "light",
            UiThemeMode::Dark => "dark",
        }
    }

    pub fn from_ini_str(s: &str) -> Self {
        if s.trim().eq_ignore_ascii_case("dark") {
            UiThemeMode::Dark
        } else {
            UiThemeMode::Light
        }
    }

    pub fn theme(self) -> UiTheme {
        match self {
            UiThemeMode::Light => UiTheme::LIGHT,
            UiThemeMode::Dark => UiTheme::DARK,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct UiTheme {
    pub canvas_bg: u32,
    pub page_border: u32,
    pub topbar_bg: u32,
    pub topbar_border: u32,
    pub tabbar_bg: u32,
    pub tab_active_bg: u32,
    pub tab_active_border: u32,
    pub tab_hover_bg: u32,
    pub sidebar_bg: u32,
    pub sidebar_header_bg: u32,
    pub statusbar_bg: u32,
    pub surface_bg: u32,
    pub surface_elevated: u32,
    pub input_bg: u32,
    pub input_border: u32,
    pub input_focus_border: u32,
    pub divider: u32,
    pub border_subtle: u32,
    pub text_primary: u32,
    pub text_secondary: u32,
    pub text_muted: u32,
    pub text_on_accent: u32,
    pub accent: u32,
    pub accent_hover: u32,
    pub accent_soft_bg: u32,
    pub accent_soft_fg: u32,
    pub hover_bg: u32,
    pub active_row_bg: u32,
    pub danger: u32,
    pub danger_soft_bg: u32,
    pub warning: u32,
    pub warning_soft_bg: u32,
    pub success: u32,
    pub success_soft_bg: u32,
    pub violet: u32,
    pub violet_soft_bg: u32,
    pub scrollbar_track: (u32, u32, u32, u32),
    pub scrollbar_thumb: (u32, u32, u32, u32),
    pub scrollbar_thumb_hover: (u32, u32, u32, u32),
    pub modal_scrim: (u32, u32, u32, u32),
    pub shadow_rgb: (u32, u32, u32),
    pub shadow_max_alpha: u32,
}

impl UiTheme {
    pub const LIGHT: UiTheme = UiTheme {
        canvas_bg: 0x00E9EEF5,
        page_border: 0x00CBD5E1,
        topbar_bg: 0x00FFFFFF,
        topbar_border: 0x00E2E8F0,
        tabbar_bg: 0x00EEF2F6,
        tab_active_bg: 0x00FFFFFF,
        tab_active_border: 0x00CBD5E1,
        tab_hover_bg: 0x00E2E8F0,
        sidebar_bg: 0x00F8FAFC,
        sidebar_header_bg: 0x00FFFFFF,
        statusbar_bg: 0x00F8FAFC,
        surface_bg: 0x00FFFFFF,
        surface_elevated: 0x00F1F5F9,
        input_bg: 0x00FFFFFF,
        input_border: 0x00CBD5E1,
        input_focus_border: 0x002563EB,
        divider: 0x00E2E8F0,
        border_subtle: 0x00D5DCE5,
        text_primary: 0x000F172A,
        text_secondary: 0x00334155,
        text_muted: 0x0064748B,
        text_on_accent: 0x00FFFFFF,
        accent: 0x002563EB,
        accent_hover: 0x001D4ED8,
        accent_soft_bg: 0x00EFF6FF,
        accent_soft_fg: 0x001D4ED8,
        hover_bg: 0x00EEF2F7,
        active_row_bg: 0x00EFF6FF,
        danger: 0x00DC2626,
        danger_soft_bg: 0x00FEF2F2,
        warning: 0x00B45309,
        warning_soft_bg: 0x00FFFBEB,
        success: 0x00059669,
        success_soft_bg: 0x00ECFDF5,
        violet: 0x007C3AED,
        violet_soft_bg: 0x00F5F3FF,
        scrollbar_track: (100, 116, 139, 28),
        scrollbar_thumb: (100, 116, 139, 115),
        scrollbar_thumb_hover: (71, 85, 105, 175),
        modal_scrim: (15, 23, 42, 85),
        shadow_rgb: (15, 23, 42),
        shadow_max_alpha: 34,
    };

    pub const DARK: UiTheme = UiTheme {
        canvas_bg: 0x0012151A,
        page_border: 0x002D333E,
        topbar_bg: 0x001D2129,
        topbar_border: 0x002A303B,
        tabbar_bg: 0x0014171C,
        tab_active_bg: 0x00222730,
        tab_active_border: 0x00343B48,
        tab_hover_bg: 0x001C2028,
        sidebar_bg: 0x00181B22,
        sidebar_header_bg: 0x001D2129,
        statusbar_bg: 0x00181B20,
        surface_bg: 0x001F242D,
        surface_elevated: 0x00282E3A,
        input_bg: 0x0014171D,
        input_border: 0x00363E4D,
        input_focus_border: 0x003B82F6,
        divider: 0x002A303B,
        border_subtle: 0x00343B48,
        text_primary: 0x00F1F5F9,
        text_secondary: 0x00CBD5E1,
        text_muted: 0x0094A3B8,
        text_on_accent: 0x00FFFFFF,
        accent: 0x003B82F6,
        accent_hover: 0x002563EB,
        accent_soft_bg: 0x001E3A5F,
        accent_soft_fg: 0x0093C5FD,
        hover_bg: 0x002A303C,
        active_row_bg: 0x001E3A5F,
        danger: 0x00F87171,
        danger_soft_bg: 0x003F1D24,
        warning: 0x00FBBF24,
        warning_soft_bg: 0x003B2F15,
        success: 0x0034D399,
        success_soft_bg: 0x0013362B,
        violet: 0x00A78BFA,
        violet_soft_bg: 0x002B2246,
        scrollbar_track: (148, 163, 184, 24),
        scrollbar_thumb: (148, 163, 184, 105),
        scrollbar_thumb_hover: (203, 213, 225, 175),
        modal_scrim: (0, 0, 0, 145),
        shadow_rgb: (0, 0, 0),
        shadow_max_alpha: 68,
    };
}

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

    pub fn theme_for(self, skin: UiSkin, kind: DocKind) -> UiTheme {
        let base = self.theme();
        if skin == UiSkin::Classic {
            return base;
        }
        match (self, kind) {
            // Distinct PDF: Precision document-studio (crisp slate + cobalt blue)
            (UiThemeMode::Light, DocKind::Pdf) => UiTheme {
                canvas_bg: 0x00E8ECF2,
                page_border: 0x00C5CEDB,
                topbar_bg: 0x00F4F6FA,
                topbar_border: 0x00D9E0EA,
                tabbar_bg: 0x00E9EEF5,
                tab_active_bg: 0x00FFFFFF,
                tab_active_border: 0x002563EB,
                tab_hover_bg: 0x00DEE5F0,
                sidebar_bg: 0x00F2F5FA,
                sidebar_header_bg: 0x00E9EEF6,
                statusbar_bg: 0x00F4F6FA,
                surface_bg: 0x00FFFFFF,
                surface_elevated: 0x00FFFFFF,
                input_bg: 0x00FFFFFF,
                input_border: 0x00CBD4E1,
                input_focus_border: 0x002563EB,
                divider: 0x00D5DCE7,
                border_subtle: 0x00E3E8F0,
                accent: 0x002563EB,
                accent_hover: 0x001D4ED8,
                accent_soft_bg: 0x00DBEAFE,
                accent_soft_fg: 0x001E40AF,
                hover_bg: 0x00E5ECF6,
                active_row_bg: 0x00DBEAFE,
                ..base
            },
            (UiThemeMode::Dark, DocKind::Pdf) => UiTheme {
                canvas_bg: 0x00131720,
                page_border: 0x002B3445,
                topbar_bg: 0x001A202C,
                topbar_border: 0x002B3445,
                tabbar_bg: 0x00151A24,
                tab_active_bg: 0x00222A3A,
                tab_active_border: 0x003B82F6,
                tab_hover_bg: 0x001E2534,
                sidebar_bg: 0x00171D28,
                sidebar_header_bg: 0x001E2534,
                statusbar_bg: 0x001A202C,
                surface_bg: 0x001E2534,
                surface_elevated: 0x00262F42,
                input_bg: 0x0012161F,
                input_border: 0x00334155,
                input_focus_border: 0x003B82F6,
                divider: 0x002B3445,
                border_subtle: 0x00232B3B,
                accent: 0x003B82F6,
                accent_hover: 0x0060A5FA,
                accent_soft_bg: 0x001E3A5F,
                accent_soft_fg: 0x0093C5FD,
                hover_bg: 0x00242D3F,
                active_row_bg: 0x001E3A5F,
                ..base
            },
            // Distinct Markdown: Clean Writer/IDE (cool white/neutral charcoal + indigo)
            (UiThemeMode::Light, DocKind::Markdown) => UiTheme {
                canvas_bg: 0x00F3F4F8,
                page_border: 0x00D0D5E2,
                topbar_bg: 0x00F8F9FC,
                topbar_border: 0x00DCE0EC,
                tabbar_bg: 0x00ECEEF5,
                tab_active_bg: 0x00FFFFFF,
                tab_active_border: 0x004F46E5,
                tab_hover_bg: 0x00E3E6F0,
                sidebar_bg: 0x00F5F6FA,
                sidebar_header_bg: 0x00ECEEF6,
                statusbar_bg: 0x00F8F9FC,
                surface_bg: 0x00FFFFFF,
                surface_elevated: 0x00FFFFFF,
                input_bg: 0x00FFFFFF,
                input_border: 0x00CFD4E3,
                input_focus_border: 0x004F46E5,
                divider: 0x00D9DDEA,
                border_subtle: 0x00E6E9F2,
                accent: 0x004F46E5,
                accent_hover: 0x004338CA,
                accent_soft_bg: 0x00E0E7FF,
                accent_soft_fg: 0x003730A3,
                hover_bg: 0x00E8EAF6,
                active_row_bg: 0x00E0E7FF,
                ..base
            },
            (UiThemeMode::Dark, DocKind::Markdown) => UiTheme {
                canvas_bg: 0x0014151D,
                page_border: 0x002C2F40,
                topbar_bg: 0x001A1B26,
                topbar_border: 0x002C2F40,
                tabbar_bg: 0x00151620,
                tab_active_bg: 0x00232637,
                tab_active_border: 0x00818CF8,
                tab_hover_bg: 0x001F2130,
                sidebar_bg: 0x00171822,
                sidebar_header_bg: 0x001E202E,
                statusbar_bg: 0x001A1B26,
                surface_bg: 0x001E202E,
                surface_elevated: 0x00272A3C,
                input_bg: 0x0012131B,
                input_border: 0x00373B52,
                input_focus_border: 0x00818CF8,
                divider: 0x002C2F40,
                border_subtle: 0x00232636,
                text_on_accent: 0x000F172A,
                accent: 0x00818CF8,
                accent_hover: 0x00A5B4FC,
                accent_soft_bg: 0x002A2D54,
                accent_soft_fg: 0x00C7D2FE,
                hover_bg: 0x0024273A,
                active_row_bg: 0x002A2D54,
                ..base
            },
            // Distinct LaTeX: Academic Typesetting Studio (warm stone/slate + emerald/teal)
            (UiThemeMode::Light, DocKind::LaTeX) => UiTheme {
                canvas_bg: 0x00F2F3EF,
                page_border: 0x00CDD3CC,
                topbar_bg: 0x00F7F8F5,
                topbar_border: 0x00D9DFD8,
                tabbar_bg: 0x00EAECE7,
                tab_active_bg: 0x00FCFDFB,
                tab_active_border: 0x000D9488,
                tab_hover_bg: 0x00E0E4DD,
                sidebar_bg: 0x00F4F6F2,
                sidebar_header_bg: 0x00E9EDE7,
                statusbar_bg: 0x00F7F8F5,
                surface_bg: 0x00FCFDFB,
                surface_elevated: 0x00FFFFFF,
                input_bg: 0x00FFFFFF,
                input_border: 0x00C7D0C9,
                input_focus_border: 0x000D9488,
                divider: 0x00D4DBD4,
                border_subtle: 0x00E3E8E2,
                accent: 0x000D9488,
                accent_hover: 0x000F766E,
                accent_soft_bg: 0x00CCFBF1,
                accent_soft_fg: 0x00115E59,
                hover_bg: 0x00E2ECE8,
                active_row_bg: 0x00CCFBF1,
                ..base
            },
            (UiThemeMode::Dark, DocKind::LaTeX) => UiTheme {
                canvas_bg: 0x00131919,
                page_border: 0x00283736,
                topbar_bg: 0x00182020,
                topbar_border: 0x00283736,
                tabbar_bg: 0x00141B1B,
                tab_active_bg: 0x00202B2A,
                tab_active_border: 0x002DD4BF,
                tab_hover_bg: 0x001C2625,
                sidebar_bg: 0x00161D1D,
                sidebar_header_bg: 0x001C2625,
                statusbar_bg: 0x00182020,
                surface_bg: 0x001C2625,
                surface_elevated: 0x00243130,
                input_bg: 0x00111717,
                input_border: 0x00314442,
                input_focus_border: 0x002DD4BF,
                divider: 0x00283736,
                border_subtle: 0x00202B2A,
                text_on_accent: 0x000F172A,
                accent: 0x002DD4BF,
                accent_hover: 0x005EEAD4,
                accent_soft_bg: 0x00163D39,
                accent_soft_fg: 0x0099F6E4,
                hover_bg: 0x00212E2D,
                active_row_bg: 0x00163D39,
                ..base
            },
            // Distinct EPUB: Warm Apple Books / Foliate reading atmosphere
            (UiThemeMode::Light, DocKind::Epub) => UiTheme {
                canvas_bg: 0x00F4EFE6,
                page_border: 0x00DCD3C4,
                topbar_bg: 0x00F8F4EC,
                topbar_border: 0x00E3DACB,
                tabbar_bg: 0x00EEE8DC,
                tab_active_bg: 0x00FDFBF7,
                tab_active_border: 0x00B45309,
                tab_hover_bg: 0x00E6DEC9,
                sidebar_bg: 0x00F6F1E7,
                sidebar_header_bg: 0x00EDE6D8,
                statusbar_bg: 0x00F8F4EC,
                surface_bg: 0x00FDFBF7,
                surface_elevated: 0x00FFFFFF,
                input_bg: 0x00FFFFFF,
                input_border: 0x00D6CCB9,
                input_focus_border: 0x00B45309,
                divider: 0x00E0D7C6,
                border_subtle: 0x00EBE4D7,
                accent: 0x00B45309,
                accent_hover: 0x0092400E,
                accent_soft_bg: 0x00FEF3C7,
                accent_soft_fg: 0x0092400E,
                hover_bg: 0x00EFE6D5,
                active_row_bg: 0x00FEF3C7,
                ..base
            },
            (UiThemeMode::Dark, DocKind::Epub) => UiTheme {
                canvas_bg: 0x00181614,
                page_border: 0x00332D27,
                topbar_bg: 0x001F1C19,
                topbar_border: 0x00332D27,
                tabbar_bg: 0x001A1715,
                tab_active_bg: 0x0028231E,
                tab_active_border: 0x00F59E0B,
                tab_hover_bg: 0x00241F1B,
                sidebar_bg: 0x001C1916,
                sidebar_header_bg: 0x0025201B,
                statusbar_bg: 0x001F1C19,
                surface_bg: 0x00231F1B,
                surface_elevated: 0x002D2722,
                input_bg: 0x00151311,
                input_border: 0x00423930,
                input_focus_border: 0x00F59E0B,
                divider: 0x00332D27,
                border_subtle: 0x0029241F,
                text_on_accent: 0x001A140B,
                accent: 0x00F59E0B,
                accent_hover: 0x00FBBF24,
                accent_soft_bg: 0x00452E10,
                accent_soft_fg: 0x00FDE68A,
                hover_bg: 0x002B251F,
                active_row_bg: 0x00452E10,
                ..base
            },
        }
    }
}

/// Visual chrome skin mode. `Classic` preserves the original flat/crisp MicroPDF look;
/// `Distinct` enables the Phase 1–4 editorial/magazine UI skin.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum UiSkin {
    #[default]
    Classic,
    Distinct,
}

impl UiSkin {
    pub fn toggle(self) -> Self {
        match self {
            UiSkin::Classic => UiSkin::Distinct,
            UiSkin::Distinct => UiSkin::Classic,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            UiSkin::Classic => "Classic",
            UiSkin::Distinct => "Distinct",
        }
    }

    pub fn to_ini_str(self) -> &'static str {
        match self {
            UiSkin::Classic => "classic",
            UiSkin::Distinct => "distinct",
        }
    }

    pub fn from_ini_str(s: &str) -> Self {
        if s.trim().eq_ignore_ascii_case("distinct") {
            UiSkin::Distinct
        } else {
            UiSkin::Classic
        }
    }
}

/// Document format kind derived from a file path or active tab state.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DocKind {
    Pdf,
    Markdown,
    LaTeX,
    Epub,
}

impl DocKind {
    pub fn from_path(path: &std::path::Path) -> Self {
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            return DocKind::Pdf;
        };
        if ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown") {
            DocKind::Markdown
        } else if ext.eq_ignore_ascii_case("tex") {
            DocKind::LaTeX
        } else if ext.eq_ignore_ascii_case("epub") {
            DocKind::Epub
        } else {
            DocKind::Pdf
        }
    }

    pub fn is_reflowable(self) -> bool {
        matches!(self, DocKind::Markdown | DocKind::LaTeX | DocKind::Epub)
    }

    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            DocKind::Pdf => "PDF",
            DocKind::Markdown => "Markdown",
            DocKind::LaTeX => "LaTeX",
            DocKind::Epub => "EPUB",
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
    #[allow(dead_code)]
    pub warning_soft_bg: u32,
    #[allow(dead_code)]
    pub success: u32,
    #[allow(dead_code)]
    pub success_soft_bg: u32,
    #[allow(dead_code)]
    pub violet: u32,
    #[allow(dead_code)]
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

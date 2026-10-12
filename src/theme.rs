//! Design tokens: every colour and metric the custom widgets use, for a dark and a light theme.
//! Widgets read `Tokens` instead of hard-coding colours, so the whole window stays consistent.

use egui::{Color32, FontFamily, FontId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum ThemeKind {
    #[default]
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    pub kind: ThemeKind,
    /// Window body, top and bottom of the soft vertical gradient.
    pub bg_top: Color32,
    pub bg_bottom: Color32,
    pub border: Color32,
    pub divider: Color32,
    pub title: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub placeholder: Color32,
    pub icon: Color32,
    pub icon_faint: Color32,
    pub hover: Color32,
    pub accent: Color32,
    /// Encryption: its own colour, so a locked note never reads as merely hidden.
    pub secure: Color32,
    pub error: Color32,
    pub tab_selected: Color32,
    pub close_fill: Color32,
    pub close_glyph: Color32,
    pub popup: Color32,
    pub selection: Color32,
    pub radius: f32,
}

impl Tokens {
    pub const DARK: Tokens = Tokens {
        kind: ThemeKind::Dark,
        bg_top: Color32::from_rgb(0x2a, 0x2c, 0x30),
        bg_bottom: Color32::from_rgb(0x23, 0x25, 0x28),
        border: Color32::from_rgb(0x48, 0x4b, 0x50),
        divider: Color32::from_rgb(0x38, 0x3b, 0x40),
        title: Color32::from_rgb(0xc8, 0xca, 0xce),
        text: Color32::from_rgb(0xe6, 0xe7, 0xe9),
        text_muted: Color32::from_rgb(0xb4, 0xb7, 0xbc),
        placeholder: Color32::from_rgb(0x7a, 0x7e, 0x84),
        icon: Color32::from_rgb(0xd4, 0xd6, 0xd9),
        icon_faint: Color32::from_rgb(0x7c, 0x80, 0x86),
        hover: Color32::from_rgba_premultiplied(0x16, 0x16, 0x16, 0x16),
        accent: Color32::from_rgb(0x2f, 0x8c, 0xff),
        secure: Color32::from_rgb(0x3f, 0xc0, 0x6a),
        error: Color32::from_rgb(0xff, 0x6b, 0x6b),
        tab_selected: Color32::from_rgb(0x2b, 0x3b, 0x52),
        close_fill: Color32::from_rgb(0x8e, 0x91, 0x96),
        close_glyph: Color32::from_rgb(0x2a, 0x2c, 0x30),
        popup: Color32::from_rgb(0x31, 0x33, 0x37),
        selection: Color32::from_rgba_premultiplied(0x1c, 0x4f, 0x8f, 0xa0),
        radius: 12.0,
    };

    pub const LIGHT: Tokens = Tokens {
        kind: ThemeKind::Light,
        bg_top: Color32::from_rgb(0xfb, 0xfb, 0xfa),
        bg_bottom: Color32::from_rgb(0xf1, 0xf1, 0xef),
        border: Color32::from_rgb(0xcf, 0xd0, 0xd2),
        divider: Color32::from_rgb(0xe2, 0xe3, 0xe5),
        title: Color32::from_rgb(0x4a, 0x4d, 0x52),
        text: Color32::from_rgb(0x1f, 0x21, 0x24),
        text_muted: Color32::from_rgb(0x55, 0x58, 0x5e),
        placeholder: Color32::from_rgb(0x9a, 0x9d, 0xa2),
        icon: Color32::from_rgb(0x3a, 0x3d, 0x42),
        icon_faint: Color32::from_rgb(0x9a, 0x9d, 0xa2),
        hover: Color32::from_rgba_premultiplied(0x00, 0x00, 0x00, 0x10),
        accent: Color32::from_rgb(0x0a, 0x72, 0xf0),
        secure: Color32::from_rgb(0x1a, 0x8a, 0x40),
        error: Color32::from_rgb(0xd0, 0x30, 0x30),
        tab_selected: Color32::from_rgb(0xdd, 0xe8, 0xf8),
        close_fill: Color32::from_rgb(0xb4, 0xb6, 0xba),
        close_glyph: Color32::from_rgb(0xfb, 0xfb, 0xfa),
        popup: Color32::from_rgb(0xff, 0xff, 0xff),
        selection: Color32::from_rgba_premultiplied(0x60, 0x9c, 0xe8, 0x70),
        radius: 12.0,
    };

    pub fn of(kind: ThemeKind) -> Tokens {
        match kind {
            ThemeKind::Dark => Self::DARK,
            ThemeKind::Light => Self::LIGHT,
        }
    }

    /// Applies the tokens to egui's own visuals (text cursor, selection, tooltips).
    pub fn apply(&self, ctx: &egui::Context) {
        let mut v = match self.kind {
            ThemeKind::Dark => egui::Visuals::dark(),
            ThemeKind::Light => egui::Visuals::light(),
        };
        v.panel_fill = Color32::TRANSPARENT;
        v.window_fill = self.popup;
        v.extreme_bg_color = Color32::TRANSPARENT;
        v.selection.bg_fill = self.selection;
        v.selection.stroke.color = self.text;
        v.text_cursor.stroke = egui::Stroke::new(2.0, self.accent);
        v.override_text_color = Some(self.text);
        v.widgets.noninteractive.fg_stroke.color = self.text;
        v.window_corner_radius = 8.into();
        v.menu_corner_radius = 8.into();
        ctx.set_visuals(v);
    }
}

pub const SEMIBOLD: &str = "semibold";

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

pub fn regular(size: f32) -> FontId {
    FontId::proportional(size)
}

/// Embeds the fonts so text never depends on what is installed (all OFL).
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::empty();
    let add = |fonts: &mut egui::FontDefinitions, name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.to_owned(), egui::FontData::from_static(bytes).into());
    };
    add(&mut fonts, "inter", include_bytes!("../assets/fonts/Inter-Regular.ttf"));
    add(&mut fonts, "inter-semibold", include_bytes!("../assets/fonts/Inter-SemiBold.ttf"));
    add(&mut fonts, "mono", include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"));
    fonts.families.insert(FontFamily::Proportional, vec!["inter".into(), "mono".into()]);
    fonts.families.insert(FontFamily::Monospace, vec!["mono".into(), "inter".into()]);
    fonts.families.insert(FontFamily::Name(SEMIBOLD.into()), vec!["inter-semibold".into(), "inter".into()]);
    ctx.set_fonts(fonts);
}

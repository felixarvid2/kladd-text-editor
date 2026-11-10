//! Färger, typsnitt och egui-stil för Kladd.
//!
//! Två teman: "Skymning" (mörkt, blåtimme med varmt lampljus som accent)
//! och "Gryning" (ljust, svalt papper med bränd bärnsten).

use std::sync::Arc;

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke,
    Style, TextStyle, Theme, Visuals, vec2,
};

#[derive(Clone, Copy, Debug)]
pub struct Syntax {
    pub keyword: Color32,
    pub string: Color32,
    pub number: Color32,
    pub comment: Color32,
    pub function: Color32,
    pub ty: Color32,
    pub tag: Color32,
    pub attr: Color32,
    pub punct: Color32,
    pub heading: Color32,
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    /// Ramen runt editorn: flikrad och statusrad.
    pub chrome: Color32,
    /// Själva skrivytan.
    pub editor: Color32,
    /// Knappar, menyer, sökrutan.
    pub surface: Color32,
    pub surface_hover: Color32,
    /// Tunna linjer och kanter.
    pub line: Color32,
    pub text: Color32,
    pub muted: Color32,
    /// Radnummer.
    pub faint: Color32,
    /// Lampljuset: markör, aktiv flik, aktuell rad.
    pub accent: Color32,
    /// Text som ligger ovanpå accentfärg.
    pub on_accent: Color32,
    pub selection: Color32,
    pub current_line: Color32,
    pub match_bg: Color32,
    pub match_current_bg: Color32,
    pub error: Color32,
    pub syntax: Syntax,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

fn rgba(hex: u32, alpha: u8) -> Color32 {
    let c = rgb(hex);
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
}

pub fn skymning() -> Palette {
    Palette {
        dark: true,
        chrome: rgb(0x10161F),
        editor: rgb(0x161E2A),
        surface: rgb(0x1E2836),
        surface_hover: rgb(0x263244),
        line: rgb(0x243041),
        text: rgb(0xD9E0EA),
        muted: rgb(0x7D8BA0),
        faint: rgb(0x46546A),
        accent: rgb(0xF0B44C),
        on_accent: rgb(0x161E2A),
        selection: rgba(0xF0B44C, 54),
        current_line: rgba(0xFFFFFF, 7),
        match_bg: rgba(0x79B8E8, 46),
        match_current_bg: rgba(0xF0B44C, 110),
        error: rgb(0xEF7C7C),
        syntax: Syntax {
            keyword: rgb(0xC6A0F2),
            string: rgb(0xA3CF8F),
            number: rgb(0xEC9A6E),
            comment: rgb(0x5D6B80),
            function: rgb(0x7DBCEB),
            ty: rgb(0x7FD3C8),
            tag: rgb(0x7DBCEB),
            attr: rgb(0xE5C07B),
            punct: rgb(0x8794A8),
            heading: rgb(0xF0B44C),
        },
    }
}

pub fn gryning() -> Palette {
    Palette {
        dark: false,
        chrome: rgb(0xE4E9F0),
        editor: rgb(0xF7F9FB),
        surface: rgb(0xFFFFFF),
        surface_hover: rgb(0xD9E0EA),
        line: rgb(0xD0D8E3),
        text: rgb(0x1E2836),
        muted: rgb(0x66748A),
        faint: rgb(0xA7B2C2),
        accent: rgb(0xB8650F),
        on_accent: rgb(0xFFFFFF),
        selection: rgba(0xE8A23A, 70),
        current_line: rgba(0x1E2836, 9),
        match_bg: rgba(0x2F7BC4, 40),
        match_current_bg: rgba(0xE8A23A, 140),
        error: rgb(0xC0392B),
        syntax: Syntax {
            keyword: rgb(0x7A3FB8),
            string: rgb(0x3B7A2A),
            number: rgb(0xB4532A),
            comment: rgb(0x8C97A8),
            function: rgb(0x1F63A8),
            ty: rgb(0x16786D),
            tag: rgb(0x1F63A8),
            attr: rgb(0x8A6512),
            punct: rgb(0x6B7789),
            heading: rgb(0xB8650F),
        },
    }
}

pub fn palette(dark: bool) -> Palette {
    if dark { skymning() } else { gryning() }
}

pub const MEDIUM: &str = "plex-sans-medium";
pub const SEMIBOLD: &str = "plex-sans-semibold";
pub const MONO_MEDIUM: &str = "plex-mono-medium";

pub fn medium() -> FontFamily {
    FontFamily::Name(MEDIUM.into())
}
pub fn semibold() -> FontFamily {
    FontFamily::Name(SEMIBOLD.into())
}
pub fn mono_medium() -> FontFamily {
    FontFamily::Name(MONO_MEDIUM.into())
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let data = [
        ("plex-sans", &include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf")[..]),
        (MEDIUM, &include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf")[..]),
        (SEMIBOLD, &include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf")[..]),
        ("plex-mono", &include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf")[..]),
        (MONO_MEDIUM, &include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf")[..]),
    ];
    for (name, bytes) in data {
        fonts
            .font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    }

    let prop = fonts.families.entry(FontFamily::Proportional).or_default();
    prop.insert(0, "plex-sans".to_owned());
    let prop_fallback = prop.clone();

    let mono = fonts.families.entry(FontFamily::Monospace).or_default();
    mono.insert(0, "plex-mono".to_owned());
    let mono_fallback = mono.clone();

    let with_fallback = |first: &str, rest: &Vec<String>| {
        let mut v = vec![first.to_owned()];
        v.extend(rest.iter().cloned());
        v
    };
    fonts
        .families
        .insert(medium(), with_fallback(MEDIUM, &prop_fallback));
    fonts
        .families
        .insert(semibold(), with_fallback(SEMIBOLD, &prop_fallback));
    fonts
        .families
        .insert(mono_medium(), with_fallback(MONO_MEDIUM, &mono_fallback));

    ctx.set_fonts(fonts);
}

fn build_style(p: &Palette) -> Style {
    let mut style = Style::default();
    style.visuals = if p.dark { Visuals::dark() } else { Visuals::light() };

    style.text_styles = [
        (TextStyle::Heading, FontId::new(19.0, semibold())),
        (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(13.5, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(11.5, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
    ]
    .into();

    let s = &mut style.spacing;
    s.item_spacing = vec2(8.0, 6.0);
    s.button_padding = vec2(10.0, 5.0);
    s.interact_size.y = 26.0;
    s.menu_margin = Margin::same(6);
    s.window_margin = Margin::same(16);
    s.scroll.bar_width = 8.0;
    s.scroll.floating = true;

    let v = &mut style.visuals;
    v.panel_fill = p.editor;
    v.window_fill = p.surface;
    v.window_stroke = Stroke::new(1.0, p.line);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(10);
    let shadow_alpha = if p.dark { 110 } else { 38 };
    v.window_shadow = Shadow {
        offset: [0, 10],
        blur: 28,
        spread: 0,
        color: Color32::from_black_alpha(shadow_alpha),
    };
    v.popup_shadow = Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(shadow_alpha),
    };
    v.extreme_bg_color = p.editor;
    v.text_edit_bg_color = Some(p.editor);
    v.faint_bg_color = p.surface;
    v.code_bg_color = p.surface;
    v.hyperlink_color = p.syntax.function;
    v.warn_fg_color = p.accent;
    v.error_fg_color = p.error;
    v.selection.bg_fill = p.selection;
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.text_cursor.stroke = Stroke::new(2.0, p.accent);
    v.text_cursor.on_duration = 0.6;
    v.text_cursor.off_duration = 0.4;
    v.indent_has_left_vline = false;

    let radius = CornerRadius::same(7);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.surface;
    w.noninteractive.weak_bg_fill = p.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.line);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = radius;

    w.inactive.bg_fill = p.surface;
    w.inactive.weak_bg_fill = p.surface;
    w.inactive.bg_stroke = Stroke::new(1.0, p.line);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = radius;

    w.hovered.bg_fill = p.surface_hover;
    w.hovered.weak_bg_fill = p.surface_hover;
    w.hovered.bg_stroke = Stroke::new(1.0, p.line);
    w.hovered.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.corner_radius = radius;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = p.line;
    w.active.weak_bg_fill = p.line;
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(1.0, p.text);
    w.active.corner_radius = radius;
    w.active.expansion = 0.0;

    w.open = w.hovered;

    style
}

pub fn install_styles(ctx: &egui::Context) {
    ctx.set_style_of(Theme::Dark, build_style(&skymning()));
    ctx.set_style_of(Theme::Light, build_style(&gryning()));
}

pub fn set_dark(ctx: &egui::Context, dark: bool) {
    ctx.set_theme(if dark { Theme::Dark } else { Theme::Light });
}

//! Windows 11 / Fluent tokens shared by the library, inspector, and dialogs.
use super::Message;
use iced::{
    Border, Color, Element, Font, Theme,
    widget::{button, container, row, scrollable, text, text_input},
};

pub const TEXT_FONT: Font = Font::with_name("Source Han Sans CN");
pub const TEXT_FONT_REGULAR: &[u8] =
    include_bytes!("../../assets/fonts/SourceHanSansCN-Regular.otf");
pub const TEXT_FONT_BOLD: &[u8] = include_bytes!("../../assets/fonts/SourceHanSansCN-Bold.otf");

pub const BACKDROP: Color = Color::from_rgb8(239, 242, 248);
pub const SURFACE: Color = Color::WHITE;
pub const GLASS: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.58);
pub const SUBTLE: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.28);
pub const INK: Color = Color::from_rgb8(32, 32, 32);
pub const MUTED: Color = Color::from_rgb8(72, 81, 93);
pub const LINE: Color = Color::from_rgba(0.16, 0.20, 0.28, 0.10);
pub const ACCENT: Color = Color::from_rgb8(0, 95, 184);
pub const TINT: Color = Color::from_rgba(0.78, 0.88, 1.0, 0.62);
pub const SUCCESS: Color = Color::from_rgb8(15, 112, 62);
pub const DANGER: Color = Color::from_rgb8(179, 38, 30);
#[derive(Clone, Copy)]
pub struct Palette {
    pub backdrop: Color,
    pub surface: Color,
    pub glass: Color,
    pub subtle: Color,
    pub ink: Color,
    pub muted: Color,
    pub line: Color,
    pub accent: Color,
    pub tint: Color,
    pub success: Color,
    pub danger: Color,
    pub warning: Color,
    pub on_accent: Color,
    pub hover: Color,
    pub quiet_hover: Color,
    pub accent_hover: Color,
    pub accent_pressed: Color,
    pub disabled: Color,
    pub panel_edge: Color,
    pub error_surface: Color,
    pub shell: Color,
    pub modal: Color,
    pub scrim: Color,
}
impl Palette {
    pub fn new(dark: bool) -> Self {
        if dark {
            Self {
                backdrop: Color::from_rgb8(32, 34, 39),
                surface: Color::from_rgb8(43, 46, 52),
                glass: Color::from_rgba(0.16, 0.18, 0.21, 0.78),
                subtle: Color::from_rgba(1.0, 1.0, 1.0, 0.045),
                ink: Color::from_rgb8(245, 245, 245),
                muted: Color::from_rgb8(188, 197, 209),
                line: Color::from_rgba(1.0, 1.0, 1.0, 0.13),
                accent: Color::from_rgb8(96, 205, 255),
                tint: Color::from_rgba(0.12, 0.38, 0.55, 0.60),
                success: Color::from_rgb8(108, 216, 159),
                danger: Color::from_rgb8(255, 153, 164),
                warning: Color::from_rgb8(255, 201, 106),
                on_accent: Color::from_rgb8(0, 43, 64),
                hover: Color::from_rgba(1.0, 1.0, 1.0, 0.10),
                quiet_hover: Color::from_rgba(1.0, 1.0, 1.0, 0.075),
                accent_hover: Color::from_rgb8(125, 215, 255),
                accent_pressed: Color::from_rgb8(83, 184, 229),
                disabled: Color::from_rgb8(56, 60, 67),
                panel_edge: Color::from_rgba(1.0, 1.0, 1.0, 0.10),
                error_surface: Color::from_rgb8(66, 37, 44),
                shell: Color::from_rgba(0.10, 0.12, 0.15, 0.65),
                modal: Color::from_rgba(0.16, 0.18, 0.21, 0.98),
                scrim: Color::from_rgba(0.0, 0.0, 0.0, 0.40),
            }
        } else {
            Self {
                backdrop: BACKDROP,
                surface: SURFACE,
                glass: GLASS,
                subtle: SUBTLE,
                ink: INK,
                muted: MUTED,
                line: LINE,
                accent: ACCENT,
                tint: TINT,
                success: SUCCESS,
                danger: DANGER,
                warning: Color::from_rgb8(157, 93, 0),
                on_accent: SURFACE,
                hover: Color::from_rgba(1.0, 1.0, 1.0, 0.80),
                quiet_hover: Color::from_rgba(1.0, 1.0, 1.0, 0.44),
                accent_hover: Color::from_rgb8(25, 111, 191),
                accent_pressed: Color::from_rgb8(45, 120, 190),
                disabled: Color::from_rgb8(223, 227, 233),
                panel_edge: Color::from_rgba(1.0, 1.0, 1.0, 0.72),
                error_surface: Color::from_rgb8(253, 237, 237),
                shell: Color::from_rgba(0.95, 0.97, 1.0, 0.48),
                modal: Color::from_rgba(0.98, 0.985, 1.0, 0.97),
                scrim: Color::from_rgba(0.08, 0.12, 0.20, 0.22),
            }
        }
    }
}
pub fn palette(theme: &Theme) -> Palette {
    Palette::new(theme.extended_palette().is_dark)
}
// Fluent motion: short, interruptible transitions, with no fading of content.
pub const MOTION_HOVER: std::time::Duration = std::time::Duration::from_millis(120);
pub const MOTION_ENTER: std::time::Duration = std::time::Duration::from_millis(200);
pub const MOTION_PANEL: std::time::Duration = std::time::Duration::from_millis(240);
pub const MOTION_DISTANCE: f32 = 12.0;
pub fn icon_font() -> Font {
    Font::with_name("Segoe Fluent Icons")
}
pub const ADD: char = '\u{E710}';
pub const FOLDER: char = '\u{E8B7}';
pub const TAG: char = '\u{E8EC}';
pub const VIDEO: char = '\u{E714}';
pub const PLAY: char = '\u{E768}';
pub const LIBRARY: char = '\u{E8F1}';
pub const OPEN_WINDOW: char = '\u{E8A7}';
pub const COPY: char = '\u{E8C8}';
pub const INFO: char = '\u{E946}';
pub const SEARCH: char = '\u{E721}';
pub const CLOSE: char = '\u{E711}';
pub const REFRESH: char = '\u{E72C}';
pub const CHECK: char = '\u{E73E}';
pub const DELETE: char = '\u{E74D}';
pub const WARNING: char = '\u{E7BA}';
pub const UPDATE: char = '\u{E895}';
pub const SORT_ASCENDING: char = '\u{E70E}';
pub const SORT_DESCENDING: char = '\u{E70D}';
pub const LIST_SIZE_WIDTH: f32 = 64.0;
pub const LIST_DURATION_WIDTH: f32 = 64.0;
pub const LIST_COMPACT_WIDTH: f32 = 600.0;
pub const LIST_MODIFIED_WIDTH: f32 = 96.0;
pub const LIST_COLUMN_SPACING: f32 = 8.0;
pub const CONTENT_SCROLLBAR_SPACE: f32 = 12.0;
pub const DETAILS_DEFAULT_WIDTH: f32 = 300.0;
pub const DETAILS_MIN_WIDTH: f32 = 300.0;
pub const DETAILS_MAX_WIDTH: f32 = 640.0;
pub const DETAILS_RESIZE_WIDTH: f32 = 16.0;
pub const LIBRARY_MIN_WIDTH: f32 = 480.0;
pub const BUTTON_CONTENT_HEIGHT: f32 = 20.0;
pub const HEADER_COMPACT_WIDTH: f32 = 960.0;

pub fn theme(dark: bool) -> Theme {
    let p = Palette::new(dark);
    Theme::custom(
        if dark {
            "Windows Dark"
        } else {
            "Windows Light"
        },
        iced::theme::Palette {
            background: p.backdrop,
            text: p.ink,
            primary: p.accent,
            success: p.success,
            danger: p.danger,
            warning: p.warning,
        },
    )
}

pub fn icon<'a>(glyph: char, size: u32) -> iced::widget::Text<'a> {
    text(glyph.to_string()).font(icon_font()).size(size)
}

pub fn heading<'a>(value: impl Into<String>, size: u32) -> iced::widget::Text<'a> {
    text(value.into()).size(size).font(Font {
        weight: iced::font::Weight::Bold,
        ..TEXT_FONT
    })
}

pub fn label(glyph: char, value: &str) -> Element<'_, Message> {
    row![
        button_icon(glyph, 16).width(16),
        button_text(value).align_x(iced::alignment::Horizontal::Left)
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .into()
}

pub fn button_text<'a>(value: impl text::IntoFragment<'a>) -> iced::widget::Text<'a> {
    // Match icon line boxes even when a view uses a smaller caption size.
    // Keep shrink-to-fit widths and centered captions in fixed-width buttons.
    text(value)
        .size(13)
        .line_height(iced::Pixels(BUTTON_CONTENT_HEIGHT))
        .align_x(iced::alignment::Horizontal::Center)
        .align_y(iced::alignment::Vertical::Center)
}

pub fn button_icon<'a>(glyph: char, size: u32) -> iced::widget::Text<'a> {
    icon(glyph, size)
        .line_height(iced::Pixels(BUTTON_CONTENT_HEIGHT))
        .width(BUTTON_CONTENT_HEIGHT)
        .align_x(iced::alignment::Horizontal::Center)
        .align_y(iced::alignment::Vertical::Center)
}

pub fn content_scrollbar() -> scrollable::Scrollbar {
    scrollable::Scrollbar::new().width(4).scroller_width(4)
}

pub fn border(theme: &Theme, radius: f32) -> Border {
    Border {
        color: palette(theme).line,
        width: 1.0,
        radius: radius.into(),
    }
}

pub fn panel(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(palette(theme).glass.into()),
        border: iced::Border {
            color: palette(theme).panel_edge,
            ..border(theme, 12.0)
        },
        shadow: iced::Shadow {
            color: Color::from_rgba(0.12, 0.18, 0.30, 0.06),
            offset: iced::Vector::new(0.0, 2.0),
            blur_radius: 12.0,
        },
        ..Default::default()
    }
}

pub fn inset(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(palette(theme).subtle.into()),
        border: border(theme, 8.0),
        ..Default::default()
    }
}

pub fn primary(theme: &Theme, status: button::Status) -> button::Style {
    let (background, text_color) = match status {
        button::Status::Hovered => (palette(theme).accent_hover, palette(theme).on_accent),
        button::Status::Pressed => (palette(theme).accent_pressed, palette(theme).on_accent),
        button::Status::Disabled => (palette(theme).disabled, palette(theme).muted),
        _ => (palette(theme).accent, palette(theme).on_accent),
    };
    button::Style {
        background: Some(background.into()),
        text_color,
        border: iced::border::rounded(8),
        shadow: iced::Shadow {
            color: Color::from_rgba(0.0, 0.25, 0.55, 0.13),
            offset: iced::Vector::new(0.0, 2.0),
            blur_radius: 4.0,
        },
        ..Default::default()
    }
}

pub fn secondary(theme: &Theme, status: button::Status) -> button::Style {
    button::Style {
        background: Some(
            match status {
                button::Status::Hovered => palette(theme).hover,
                button::Status::Pressed | button::Status::Disabled => palette(theme).subtle,
                _ => palette(theme).glass,
            }
            .into(),
        ),
        text_color: if status == button::Status::Disabled {
            palette(theme).muted
        } else {
            palette(theme).ink
        },
        border: border(theme, 8.0),
        ..Default::default()
    }
}

pub fn quiet(theme: &Theme, status: button::Status) -> button::Style {
    button::Style {
        background: match status {
            button::Status::Hovered => Some(palette(theme).quiet_hover.into()),
            button::Status::Pressed => Some(palette(theme).line.into()),
            _ => None,
        },
        text_color: if status == button::Status::Disabled {
            palette(theme).muted
        } else {
            palette(theme).ink
        },
        border: iced::border::rounded(8),
        ..Default::default()
    }
}

pub fn danger(theme: &Theme, status: button::Status) -> button::Style {
    let mut style = secondary(theme, status);
    if status != button::Status::Disabled {
        style.text_color = palette(theme).danger;
    }
    style
}

pub fn item(theme: &Theme, active: bool, status: button::Status) -> button::Style {
    let mut style = quiet(theme, status);
    if active {
        style.background = Some(palette(theme).tint.into());
        style.text_color = palette(theme).ink;
    }
    style
}

pub fn sort_header(theme: &Theme, active: bool, status: button::Status) -> button::Style {
    let mut style = item(theme, active, status);
    if active && status != button::Status::Disabled {
        style.text_color = palette(theme).accent;
    }
    style
}

pub fn input(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let focused = matches!(status, text_input::Status::Focused { .. });
    text_input::Style {
        background: if focused {
            palette(theme).surface
        } else {
            palette(theme).glass
        }
        .into(),
        border: Border {
            color: if focused {
                palette(theme).accent
            } else {
                palette(theme).line
            },
            width: if focused { 2.0 } else { 1.0 },
            radius: 8.0.into(),
        },
        icon: palette(theme).muted,
        placeholder: palette(theme).muted,
        value: palette(theme).ink,
        selection: palette(theme).tint,
    }
}

pub fn select(
    theme: &Theme,
    status: iced::widget::pick_list::Status,
) -> iced::widget::pick_list::Style {
    iced::widget::pick_list::Style {
        text_color: palette(theme).ink,
        placeholder_color: palette(theme).muted,
        handle_color: palette(theme).muted,
        background: if matches!(status, iced::widget::pick_list::Status::Hovered) {
            palette(theme).subtle
        } else {
            palette(theme).glass
        }
        .into(),
        border: border(theme, 8.0),
    }
}

pub fn separator(theme: &Theme) -> iced::widget::rule::Style {
    iced::widget::rule::Style {
        color: palette(theme).line,
        radius: 0.0.into(),
        fill_mode: iced::widget::rule::FillMode::Full,
        snap: true,
    }
}

pub fn menu(theme: &Theme) -> iced::widget::overlay::menu::Style {
    iced::widget::overlay::menu::Style {
        background: palette(theme).surface.into(),
        border: border(theme, 8.0),
        text_color: palette(theme).ink,
        selected_text_color: palette(theme).ink,
        selected_background: palette(theme).tint.into(),
        shadow: iced::Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.12),
            offset: iced::Vector::new(0.0, 4.0),
            blur_radius: 12.0,
        },
    }
}

pub fn button_focus(
    theme: &Theme,
    status: button::Status,
    focused: bool,
    mut style: button::Style,
) -> button::Style {
    if focused && status == button::Status::Active {
        let colors = palette(theme);
        style.background = Some(
            if style.background == Some(colors.accent.into()) {
                colors.accent_hover
            } else if style.background == Some(colors.tint.into()) {
                Color {
                    a: 1.0,
                    ..colors.tint
                }
            } else {
                colors.tint
            }
            .into(),
        );
    }
    style
}

pub fn focus_surface(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(palette(theme).tint.into()),
        border: iced::border::rounded(8),
        ..Default::default()
    }
}

pub fn menu_surface(theme: &Theme) -> container::Style {
    let style = menu(theme);
    container::Style {
        background: Some(style.background),
        text_color: Some(style.text_color),
        border: style.border,
        shadow: style.shadow,
        ..Default::default()
    }
}

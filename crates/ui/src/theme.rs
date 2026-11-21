//! Shared desktop design system: tokens, typography, components and the
//! locally drawn vector icon set. Every page should build from these pieces
//! instead of hard-coding colours or sizes.
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, FontData, FontFamily, FontId, Frame, Margin, Painter, Pos2,
    Rect, Response, RichText, Rounding, Sense, Shape, Stroke, TextStyle, Ui, Vec2,
};

// ── Neutrals ────────────────────────────────────────────────────────────────
/// Window chrome and sidebar.
pub const BG: Color32 = Color32::from_rgb(245, 245, 247);
/// Main content and cards.
pub const SURFACE: Color32 = Color32::WHITE;
/// Recessed areas: inputs on cards, list rows, drop zones.
pub const SUNKEN: Color32 = Color32::from_rgb(249, 249, 251);
/// Hover fill for quiet controls.
pub const HOVER: Color32 = Color32::from_rgb(238, 238, 242);
pub const LINE: Color32 = Color32::from_rgb(230, 230, 235);
pub const LINE_STRONG: Color32 = Color32::from_rgb(212, 212, 220);
pub const TEXT: Color32 = Color32::from_rgb(17, 17, 24);
pub const TEXT_2: Color32 = Color32::from_rgb(63, 63, 74);
pub const MUTED: Color32 = Color32::from_rgb(110, 110, 122);
pub const FAINT: Color32 = Color32::from_rgb(160, 160, 171);

// ── Accent ──────────────────────────────────────────────────────────────────
pub const ACCENT: Color32 = Color32::from_rgb(91, 91, 214);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(76, 76, 196);
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(239, 239, 253);
pub const ACCENT_LINE: Color32 = Color32::from_rgb(201, 203, 245);
/// Kept for existing call sites; it is the accent.
pub const BLUE: Color32 = ACCENT;

// ── Status ──────────────────────────────────────────────────────────────────
pub const SUCCESS: Color32 = Color32::from_rgb(21, 128, 61);
pub const SUCCESS_SOFT: Color32 = Color32::from_rgb(236, 253, 243);
pub const WARNING: Color32 = Color32::from_rgb(180, 83, 9);
pub const WARNING_SOFT: Color32 = Color32::from_rgb(255, 247, 230);
pub const DANGER: Color32 = Color32::from_rgb(220, 38, 38);
pub const DANGER_SOFT: Color32 = Color32::from_rgb(254, 242, 242);

// ── Category hues (icons and tiles only, never body text) ───────────────────
pub const HUE_VIDEO: Color32 = ACCENT;
pub const HUE_AUDIO: Color32 = Color32::from_rgb(219, 39, 119);
pub const HUE_IMAGE: Color32 = Color32::from_rgb(5, 150, 105);
pub const HUE_PDF: Color32 = Color32::from_rgb(229, 72, 77);
pub const HUE_SUBTITLE: Color32 = Color32::from_rgb(217, 119, 6);
pub const HUE_DOCUMENT: Color32 = Color32::from_rgb(8, 145, 178);
pub const HUE_DOWNLOAD: Color32 = Color32::from_rgb(124, 58, 237);

// ── Shape ───────────────────────────────────────────────────────────────────
pub const RADIUS_SM: f32 = 6.0;
pub const RADIUS: f32 = 8.0;
pub const RADIUS_LG: f32 = 12.0;
pub const CONTROL_HEIGHT: f32 = 34.0;
/// Comfortable reading width for form pages.
pub const FORM_WIDTH: f32 = 880.0;

const SEMIBOLD: &str = "dm-sans-semibold";
const BOLD: &str = "dm-sans-bold";

#[derive(Clone, Copy)]
pub enum Icon {
    Home,
    Video,
    Audio,
    Image,
    Folder,
    Add,
    Check,
    Queue,
    Settings,
    Search,
    Upload,
    Arrow,
    Grid,
    Scissors,
    Pdf,
    Subtitles,
    Document,
    Download,
    ChevronRight,
    ChevronLeft,
    Close,
    Link,
    Sparkle,
    Play,
    Shield,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Tone {
    Neutral,
    Accent,
    Success,
    Warning,
    Danger,
}

impl Tone {
    fn colors(self) -> (Color32, Color32) {
        match self {
            Tone::Neutral => (HOVER, TEXT_2),
            Tone::Accent => (ACCENT_SOFT, ACCENT),
            Tone::Success => (SUCCESS_SOFT, SUCCESS),
            Tone::Warning => (WARNING_SOFT, WARNING),
            Tone::Danger => (DANGER_SOFT, DANGER),
        }
    }
}

/// Medium-weight UI text.
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

/// Display weight for page titles.
pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(BOLD.into()))
}

/// Mix `color` toward white; `amount` 0 keeps the colour, 1 is white.
pub fn tint(color: Color32, amount: f32) -> Color32 {
    let mix = |c: u8| (c as f32 + (255.0 - c as f32) * amount).round() as u8;
    Color32::from_rgb(mix(color.r()), mix(color.g()), mix(color.b()))
}

pub fn configure(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "dm-sans".into(),
        FontData::from_static(include_bytes!("../../../assets/fonts/DM-Sans.ttf")),
    );
    fonts.font_data.insert(
        SEMIBOLD.into(),
        FontData::from_static(include_bytes!("../../../assets/fonts/DM-Sans-SemiBold.ttf")),
    );
    fonts.font_data.insert(
        BOLD.into(),
        FontData::from_static(include_bytes!("../../../assets/fonts/DM-Sans-Bold.ttf")),
    );
    let fallbacks = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "dm-sans".into());
    for (family, face) in [(SEMIBOLD, SEMIBOLD), (BOLD, BOLD)] {
        let mut stack = vec![face.to_string()];
        stack.extend(fallbacks.iter().cloned());
        fonts
            .families
            .insert(FontFamily::Name(family.into()), stack);
    }
    #[cfg(target_os = "macos")]
    if let Ok(bytes) = std::fs::read("/System/Library/Fonts/STHeiti Medium.ttc") {
        fonts
            .font_data
            .insert("system-cjk".into(), FontData::from_owned(bytes));
        for family in [
            FontFamily::Proportional,
            FontFamily::Name(SEMIBOLD.into()),
            FontFamily::Name(BOLD.into()),
        ] {
            fonts
                .families
                .entry(family)
                .or_default()
                .insert(1, "system-cjk".into());
        }
    }
    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();
    style.text_styles = [
        (TextStyle::Small, FontId::proportional(12.0)),
        (TextStyle::Body, FontId::proportional(14.0)),
        (TextStyle::Button, FontId::proportional(13.5)),
        (TextStyle::Heading, semibold(16.0)),
        (TextStyle::Monospace, FontId::monospace(12.5)),
    ]
    .into();

    let spacing = &mut style.spacing;
    spacing.item_spacing = vec2(8.0, 10.0);
    spacing.button_padding = vec2(14.0, 7.0);
    spacing.interact_size = vec2(32.0, 30.0);
    spacing.text_edit_width = 320.0;
    spacing.combo_width = 180.0;
    spacing.slider_width = 200.0;
    spacing.indent = 18.0;
    spacing.icon_width = 16.0;
    spacing.icon_width_inner = 9.0;
    spacing.icon_spacing = 8.0;
    spacing.window_margin = Margin::same(14.0);
    spacing.menu_margin = Margin::same(6.0);
    spacing.combo_height = 280.0;
    spacing.scroll = egui::style::ScrollStyle::thin();

    let v = &mut style.visuals;
    *v = egui::Visuals::light();
    v.override_text_color = None;
    v.panel_fill = SURFACE;
    v.window_fill = SURFACE;
    v.extreme_bg_color = SURFACE;
    v.faint_bg_color = SUNKEN;
    v.code_bg_color = HOVER;
    v.hyperlink_color = ACCENT;
    v.warn_fg_color = WARNING;
    v.error_fg_color = DANGER;
    v.selection.bg_fill = Color32::from_rgb(224, 224, 250);
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    v.text_cursor = Stroke::new(1.5_f32, ACCENT);
    v.window_rounding = RADIUS_LG.into();
    v.menu_rounding = RADIUS.into();
    v.window_stroke = Stroke::new(1.0_f32, LINE);
    v.window_shadow = egui::epaint::Shadow {
        extrusion: 18.0,
        color: Color32::from_black_alpha(22),
    };
    v.popup_shadow = egui::epaint::Shadow {
        extrusion: 12.0,
        color: Color32::from_black_alpha(18),
    };
    v.slider_trailing_fill = true;
    v.indent_has_left_vline = false;
    v.collapsing_header_frame = false;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = SURFACE;
    w.noninteractive.weak_bg_fill = SURFACE;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    w.noninteractive.rounding = RADIUS.into();

    w.inactive.bg_fill = SUNKEN;
    w.inactive.weak_bg_fill = SURFACE;
    w.inactive.bg_stroke = Stroke::new(1.0_f32, LINE_STRONG);
    w.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT_2);
    w.inactive.rounding = RADIUS.into();
    w.inactive.expansion = 0.0;

    w.hovered.bg_fill = HOVER;
    w.hovered.weak_bg_fill = SUNKEN;
    w.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(190, 190, 202));
    w.hovered.fg_stroke = Stroke::new(1.5_f32, TEXT);
    w.hovered.rounding = RADIUS.into();
    w.hovered.expansion = 0.0;

    w.active.bg_fill = HOVER;
    w.active.weak_bg_fill = HOVER;
    w.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    w.active.fg_stroke = Stroke::new(1.5_f32, TEXT);
    w.active.rounding = RADIUS.into();
    w.active.expansion = 0.0;

    w.open.bg_fill = SURFACE;
    w.open.weak_bg_fill = SUNKEN;
    w.open.bg_stroke = Stroke::new(1.0_f32, ACCENT_LINE);
    w.open.fg_stroke = Stroke::new(1.0_f32, TEXT);
    w.open.rounding = RADIUS.into();

    ctx.set_style(style);
}

// ── Buttons ─────────────────────────────────────────────────────────────────

/// The one main action on a surface.
pub fn primary_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(
        RichText::new(text.into())
            .font(semibold(13.5))
            .color(Color32::WHITE),
    )
    .fill(ACCENT)
    .stroke(Stroke::NONE)
    .rounding(RADIUS)
    .min_size(vec2(0.0, CONTROL_HEIGHT))
}

/// Bordered neutral action.
pub fn secondary_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.into()).font(semibold(13.5)).color(TEXT))
        .fill(SURFACE)
        .stroke(Stroke::new(1.0_f32, LINE_STRONG))
        .rounding(RADIUS)
        .min_size(vec2(0.0, CONTROL_HEIGHT))
}

/// Compact bordered action for rows and toolbars.
pub fn small_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(
        RichText::new(text.into())
            .font(semibold(12.0))
            .color(TEXT_2),
    )
    .fill(SURFACE)
    .stroke(Stroke::new(1.0_f32, LINE))
    .rounding(RADIUS_SM)
    .min_size(vec2(0.0, 26.0))
}

/// Frameless text action.
pub fn ghost_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.into()).font(semibold(13.0)).color(MUTED))
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::NONE)
        .rounding(RADIUS_SM)
}

/// Destructive action.
pub fn danger_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(
        RichText::new(text.into())
            .font(semibold(13.5))
            .color(DANGER),
    )
    .fill(SURFACE)
    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(252, 202, 202)))
    .rounding(RADIUS)
    .min_size(vec2(0.0, CONTROL_HEIGHT))
}

/// A primary button that stretches across the available width.
pub fn wide_primary(ui: &mut Ui, enabled: bool, text: impl Into<String>) -> Response {
    let width = ui.available_width();
    ui.add_enabled(
        enabled,
        primary_button(text).min_size(vec2(width, CONTROL_HEIGHT + 8.0)),
    )
}

/// Segmented control; returns the newly picked index.
pub fn segmented(ui: &mut Ui, selected: usize, labels: &[&str]) -> Option<usize> {
    let mut picked = None;
    Frame::none()
        .fill(HOVER)
        .rounding(RADIUS + 2.0)
        .inner_margin(3.0)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for (index, label) in labels.iter().enumerate() {
                    let on = index == selected;
                    let button = egui::Button::new(
                        RichText::new(*label).font(semibold(13.0)).color(if on {
                            TEXT
                        } else {
                            MUTED
                        }),
                    )
                    .fill(if on { SURFACE } else { Color32::TRANSPARENT })
                    .stroke(if on {
                        Stroke::new(1.0_f32, LINE)
                    } else {
                        Stroke::NONE
                    })
                    .rounding(RADIUS)
                    .min_size(vec2(120.0, 30.0));
                    if ui.add(button).clicked() && !on {
                        picked = Some(index);
                    }
                }
            });
        });
    picked
}

/// Rounded filter chip.
pub fn chip(ui: &mut Ui, text: &str, selected: bool, dot: Option<Color32>) -> Response {
    let font = semibold(12.5);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font.clone(), TEXT);
    let dot_space = if dot.is_some() { 14.0 } else { 0.0 };
    let size = vec2(galley.size().x + 22.0 + dot_space, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let hovered = response.hovered();
    let (fill, stroke, color) = if selected {
        (TEXT, TEXT, Color32::WHITE)
    } else if hovered {
        (SUNKEN, LINE_STRONG, TEXT)
    } else {
        (SURFACE, LINE, TEXT_2)
    };
    let painter = ui.painter();
    painter.rect(rect, 14.0, fill, Stroke::new(1.0_f32, stroke));
    let mut x = rect.left() + 11.0;
    if let Some(dot) = dot {
        painter.circle_filled(pos2(x + 3.5, rect.center().y), 3.5, dot);
        x += dot_space;
    }
    painter.text(
        pos2(x, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        font,
        color,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ── Inputs ──────────────────────────────────────────────────────────────────

/// Single-line text input sized to match buttons.
pub fn text_input(ui: &mut Ui, value: &mut String) -> Response {
    ui.add(
        egui::TextEdit::singleline(value)
            .margin(vec2(10.0, 7.0))
            .desired_width(ui.spacing().text_edit_width),
    )
}

/// Full-width single-line input with placeholder text.
pub fn wide_input(ui: &mut Ui, value: &mut String, hint: &str) -> Response {
    ui.add(
        egui::TextEdit::singleline(value)
            .margin(vec2(10.0, 7.0))
            .hint_text(RichText::new(hint).color(FAINT))
            .desired_width(f32::INFINITY),
    )
}

/// Path field with a Browse button; returns true when Browse is clicked.
pub fn path_input(ui: &mut Ui, value: &mut String, hint: &str) -> bool {
    let mut browse = false;
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 96.0).max(120.0);
        ui.add(
            egui::TextEdit::singleline(value)
                .margin(vec2(10.0, 7.0))
                .hint_text(RichText::new(hint).color(FAINT))
                .desired_width(width),
        );
        browse = ui.add(secondary_button("Browse")).clicked();
    });
    browse
}

/// A labelled row: fixed-width label column, control on the right.
pub fn field<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(160.0, CONTROL_HEIGHT), Sense::hover());
        ui.painter().text(
            pos2(rect.left(), rect.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(13.5),
            TEXT_2,
        );
        add(ui)
    })
    .inner
}

/// Small helper text under a control.
pub fn hint(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).size(12.0).color(MUTED)).wrap(true));
}

// ── Surfaces ────────────────────────────────────────────────────────────────

/// Bordered white card.
pub fn card() -> Frame {
    Frame::none()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0_f32, LINE))
        .rounding(RADIUS_LG)
        .inner_margin(Margin::same(20.0))
}

/// Recessed panel used inside cards.
pub fn well() -> Frame {
    Frame::none()
        .fill(SUNKEN)
        .stroke(Stroke::new(1.0_f32, LINE))
        .rounding(RADIUS)
        .inner_margin(Margin::same(14.0))
}

/// Card with a title and optional description.
pub fn section<R>(
    ui: &mut Ui,
    title: &str,
    description: Option<&str>,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    let inner = card()
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).font(semibold(15.0)).color(TEXT));
            if let Some(description) = description {
                ui.add_space(-6.0);
                ui.label(RichText::new(description).size(13.0).color(MUTED));
            }
            ui.add_space(6.0);
            add(ui)
        })
        .inner;
    ui.add_space(12.0);
    inner
}

/// Upper-case group label.
pub fn overline(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text.to_uppercase())
            .font(semibold(11.0))
            .color(FAINT),
    );
}

/// Page title block. `trail` is an optional breadcrumb parent; returns true
/// when it is clicked.
pub fn page_header(ui: &mut Ui, trail: Option<&str>, title: &str, description: &str) -> bool {
    let mut back = false;
    if let Some(parent) = trail {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let response = ui
                .add(
                    egui::Label::new(RichText::new(parent).font(semibold(13.0)).color(MUTED))
                        .sense(Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.hovered() {
                ui.painter().line_segment(
                    [
                        response.rect.left_bottom() + vec2(0.0, 1.0),
                        response.rect.right_bottom() + vec2(0.0, 1.0),
                    ],
                    Stroke::new(1.0_f32, MUTED),
                );
            }
            back = response.clicked();
            icon(ui, Icon::ChevronRight, 12.0, FAINT);
            ui.label(RichText::new(title).font(semibold(13.0)).color(TEXT_2));
        });
        ui.add_space(2.0);
    }
    ui.label(RichText::new(title).font(bold(26.0)).color(TEXT));
    ui.add_space(-4.0);
    ui.label(RichText::new(description).size(14.0).color(MUTED));
    ui.add_space(20.0);
    back
}

/// Tinted pill label.
pub fn badge(ui: &mut Ui, text: &str, tone: Tone) -> Response {
    let (fill, color) = tone.colors();
    Frame::none()
        .fill(fill)
        .rounding(10.0)
        .inner_margin(Margin::symmetric(8.0, 3.0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).font(semibold(11.5)).color(color));
        })
        .response
}

/// Coloured status dot followed by text.
pub fn status_dot(ui: &mut Ui, color: Color32, text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (rect, _) = ui.allocate_exact_size(vec2(8.0, 16.0), Sense::hover());
        ui.painter()
            .circle_filled(rect.center(), 6.0, color.gamma_multiply(0.18));
        ui.painter().circle_filled(rect.center(), 3.5, color);
        ui.label(RichText::new(text).size(12.5).color(TEXT_2));
    });
}

/// Rounded square tile with a tinted fill and an icon.
pub fn paint_icon_tile(painter: &Painter, rect: Rect, kind: Icon, hue: Color32) {
    painter.rect_filled(rect, rect.width() * 0.26, tint(hue, 0.9));
    let inset = rect.width() * 0.27;
    paint_icon(painter, rect.shrink(inset), kind, hue);
}

pub fn icon_tile(ui: &mut Ui, kind: Icon, hue: Color32, size: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    paint_icon_tile(ui.painter(), rect, kind, hue);
    response
}

/// Rounded rectangle outline drawn with dashes, for drop targets.
pub fn dashed_rect(painter: &Painter, rect: Rect, radius: f32, stroke: Stroke) {
    let mut points: Vec<Pos2> = Vec::new();
    let r = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let corners = [
        (rect.right_top() + vec2(-r, r), -90.0_f32),
        (rect.right_bottom() + vec2(-r, -r), 0.0),
        (rect.left_bottom() + vec2(r, -r), 90.0),
        (rect.left_top() + vec2(r, r), 180.0),
    ];
    for (center, start) in corners {
        for step in 0..=8 {
            let angle = (start + step as f32 * 90.0 / 8.0).to_radians();
            points.push(center + r * Vec2::new(angle.cos(), angle.sin()));
        }
    }
    points.push(points[0]);
    painter.extend(Shape::dashed_line(&points, stroke, 6.0, 5.0));
}

/// Subtle lifted shadow behind a rect.
pub fn paint_shadow(painter: &Painter, rect: Rect, radius: f32) {
    painter.add(
        egui::epaint::Shadow {
            extrusion: 8.0,
            color: Color32::from_black_alpha(10),
        }
        .tessellate(rect.translate(vec2(0.0, 2.0)), Rounding::same(radius)),
    );
}

/// Empty-state block: tile, title, explanation.
pub fn empty_state(ui: &mut Ui, kind: Icon, title: &str, body: &str) {
    ui.vertical_centered(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(52.0, 52.0), Sense::hover());
        ui.painter()
            .rect(rect, 14.0, SUNKEN, Stroke::new(1.0_f32, LINE));
        paint_icon(ui.painter(), rect.shrink(15.0), kind, FAINT);
        ui.add_space(4.0);
        ui.label(RichText::new(title).font(semibold(14.0)).color(TEXT));
        ui.add_space(-6.0);
        ui.label(RichText::new(body).size(12.5).color(MUTED));
    });
}

/// Replace the home directory prefix with `~` for display.
pub fn display_path(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && text.starts_with(&home) => {
            format!("~{}", &text[home.len()..])
        }
        _ => text,
    }
}

/// Inverse of [`display_path`]: expand a leading `~` to the home directory.
pub fn expand_home(text: &str) -> std::path::PathBuf {
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty());
    match (text.strip_prefix('~'), home) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => {
            std::path::PathBuf::from(home).join(rest.trim_start_matches('/'))
        }
        _ => std::path::PathBuf::from(text),
    }
}

/// Restrict content to a comfortable reading width, left-aligned.
pub fn constrained<R>(ui: &mut Ui, max_width: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let width = ui.available_width().min(max_width);
    ui.allocate_ui_with_layout(
        vec2(width, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_width(width);
            add(ui)
        },
    )
    .inner
}

pub fn icon(ui: &mut Ui, kind: Icon, size: f32, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), egui::Sense::hover());
    paint_icon(ui.painter(), rect, kind, color);
    response
}

pub fn paint_icon(p: &Painter, r: Rect, kind: Icon, color: Color32) {
    let at = |x: f32, y: f32| pos2(r.left() + x * r.width(), r.top() + y * r.height());
    let stroke = Stroke::new((r.width() / 12.0).clamp(1.4, 2.0), color);
    let line = |a: (f32, f32), b: (f32, f32)| {
        p.line_segment([at(a.0, a.1), at(b.0, b.1)], stroke);
    };
    let box_at = |a: (f32, f32), b: (f32, f32), rounding: f32| {
        p.rect_stroke(
            Rect::from_min_max(at(a.0, a.1), at(b.0, b.1)),
            rounding,
            stroke,
        );
    };
    match kind {
        Icon::Home => {
            line((0.13, 0.45), (0.5, 0.13));
            line((0.5, 0.13), (0.87, 0.45));
            line((0.23, 0.39), (0.23, 0.86));
            line((0.23, 0.86), (0.77, 0.86));
            line((0.77, 0.86), (0.77, 0.39));
            line((0.42, 0.86), (0.42, 0.60));
            line((0.42, 0.60), (0.59, 0.60));
            line((0.59, 0.60), (0.59, 0.86));
        }
        Icon::Video => {
            box_at((0.1, 0.24), (0.67, 0.78), 3.0);
            line((0.68, 0.43), (0.9, 0.28));
            line((0.9, 0.28), (0.9, 0.74));
            line((0.9, 0.74), (0.68, 0.6));
        }
        Icon::Audio => {
            line((0.42, 0.72), (0.42, 0.2));
            line((0.42, 0.2), (0.8, 0.12));
            line((0.8, 0.12), (0.8, 0.63));
            p.circle_stroke(at(0.28, 0.75), r.width() * 0.14, stroke);
            p.circle_stroke(at(0.66, 0.66), r.width() * 0.14, stroke);
        }
        Icon::Image => {
            box_at((0.13, 0.13), (0.87, 0.87), 3.0);
            p.circle_stroke(at(0.36, 0.35), r.width() * 0.07, stroke);
            line((0.16, 0.76), (0.48, 0.46));
            line((0.48, 0.46), (0.67, 0.63));
            line((0.67, 0.63), (0.84, 0.48));
        }
        Icon::Folder => {
            for (a, b) in [
                ((0.12, 0.8), (0.12, 0.2)),
                ((0.12, 0.2), (0.4, 0.2)),
                ((0.4, 0.2), (0.5, 0.32)),
                ((0.5, 0.32), (0.88, 0.32)),
                ((0.88, 0.32), (0.88, 0.8)),
                ((0.88, 0.8), (0.12, 0.8)),
            ] {
                line(a, b);
            }
        }
        Icon::Add => {
            line((0.5, 0.18), (0.5, 0.82));
            line((0.18, 0.5), (0.82, 0.5));
        }
        Icon::Check => {
            line((0.18, 0.52), (0.4, 0.75));
            line((0.4, 0.75), (0.84, 0.24));
        }
        Icon::Queue => {
            line((0.1, 0.22), (0.83, 0.22));
            line((0.1, 0.47), (0.6, 0.47));
            line((0.1, 0.72), (0.4, 0.72));
            line((0.66, 0.5), (0.66, 0.86));
            line((0.66, 0.86), (0.91, 0.68));
            line((0.91, 0.68), (0.66, 0.5));
        }
        Icon::Settings => {
            p.circle_stroke(at(0.5, 0.5), r.width() * 0.29, stroke);
            p.circle_stroke(at(0.5, 0.5), r.width() * 0.10, stroke);
            for n in 0..8 {
                let t = n as f32 * std::f32::consts::TAU / 8.0;
                line(
                    (0.5 + 0.3 * t.cos(), 0.5 + 0.3 * t.sin()),
                    (0.5 + 0.43 * t.cos(), 0.5 + 0.43 * t.sin()),
                );
            }
        }
        Icon::Search => {
            p.circle_stroke(at(0.42, 0.42), r.width() * 0.26, stroke);
            line((0.61, 0.61), (0.88, 0.88));
        }
        Icon::Upload => {
            line((0.5, 0.12), (0.5, 0.64));
            line((0.5, 0.12), (0.28, 0.34));
            line((0.5, 0.12), (0.72, 0.34));
            line((0.13, 0.58), (0.13, 0.88));
            line((0.13, 0.88), (0.87, 0.88));
            line((0.87, 0.88), (0.87, 0.58));
        }
        Icon::Arrow => {
            line((0.16, 0.5), (0.86, 0.5));
            line((0.86, 0.5), (0.58, 0.23));
            line((0.86, 0.5), (0.58, 0.77));
        }
        Icon::Grid => {
            for x in [0.14, 0.57] {
                for y in [0.14, 0.57] {
                    box_at((x, y), (x + 0.28, y + 0.28), 2.0);
                }
            }
        }
        Icon::Scissors => {
            p.circle_stroke(at(0.24, 0.27), r.width() * 0.15, stroke);
            p.circle_stroke(at(0.24, 0.75), r.width() * 0.15, stroke);
            line((0.36, 0.36), (0.88, 0.84));
            line((0.36, 0.66), (0.88, 0.13));
        }
        Icon::Pdf | Icon::Document => {
            for (a, b) in [
                ((0.22, 0.1), (0.62, 0.1)),
                ((0.62, 0.1), (0.82, 0.31)),
                ((0.82, 0.31), (0.82, 0.9)),
                ((0.82, 0.9), (0.22, 0.9)),
                ((0.22, 0.9), (0.22, 0.1)),
                ((0.62, 0.1), (0.62, 0.31)),
                ((0.62, 0.31), (0.82, 0.31)),
            ] {
                line(a, b);
            }
            if matches!(kind, Icon::Pdf) {
                line((0.34, 0.7), (0.53, 0.43));
                line((0.53, 0.43), (0.68, 0.72));
                line((0.34, 0.7), (0.69, 0.63));
            } else {
                line((0.34, 0.48), (0.68, 0.48));
                line((0.34, 0.63), (0.68, 0.63));
                line((0.34, 0.77), (0.58, 0.77));
            }
        }
        Icon::Subtitles => {
            box_at((0.1, 0.2), (0.9, 0.8), 3.0);
            line((0.23, 0.43), (0.41, 0.43));
            line((0.53, 0.43), (0.76, 0.43));
            line((0.23, 0.61), (0.56, 0.61));
            line((0.68, 0.61), (0.77, 0.61));
        }
        Icon::Download => {
            line((0.5, 0.1), (0.5, 0.64));
            line((0.5, 0.64), (0.29, 0.43));
            line((0.5, 0.64), (0.71, 0.43));
            line((0.15, 0.66), (0.15, 0.87));
            line((0.15, 0.87), (0.85, 0.87));
            line((0.85, 0.87), (0.85, 0.66));
        }
        Icon::ChevronRight => {
            line((0.38, 0.2), (0.68, 0.5));
            line((0.68, 0.5), (0.38, 0.8));
        }
        Icon::ChevronLeft => {
            line((0.62, 0.2), (0.32, 0.5));
            line((0.32, 0.5), (0.62, 0.8));
        }
        Icon::Close => {
            line((0.25, 0.25), (0.75, 0.75));
            line((0.75, 0.25), (0.25, 0.75));
        }
        Icon::Link => {
            line((0.42, 0.58), (0.58, 0.42));
            p.circle_stroke(at(0.32, 0.68), r.width() * 0.17, stroke);
            p.circle_stroke(at(0.68, 0.32), r.width() * 0.17, stroke);
        }
        Icon::Sparkle => {
            let c = at(0.5, 0.5);
            let pts = [
                at(0.5, 0.08),
                at(0.6, 0.4),
                at(0.92, 0.5),
                at(0.6, 0.6),
                at(0.5, 0.92),
                at(0.4, 0.6),
                at(0.08, 0.5),
                at(0.4, 0.4),
            ];
            let _ = c;
            p.add(egui::Shape::closed_line(pts.to_vec(), stroke));
        }
        Icon::Play => {
            p.add(egui::Shape::convex_polygon(
                vec![at(0.3, 0.18), at(0.84, 0.5), at(0.3, 0.82)],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Shield => {
            for (a, b) in [
                ((0.5, 0.1), (0.85, 0.24)),
                ((0.85, 0.24), (0.8, 0.6)),
                ((0.8, 0.6), (0.5, 0.9)),
                ((0.5, 0.9), (0.2, 0.6)),
                ((0.2, 0.6), (0.15, 0.24)),
                ((0.15, 0.24), (0.5, 0.1)),
            ] {
                line(a, b);
            }
            line((0.35, 0.5), (0.47, 0.62));
            line((0.47, 0.62), (0.66, 0.4));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn heading_weights_resolve_to_bundled_faces() {
        let context = egui::Context::default();
        configure(&context);
        let _ = context.run(egui::RawInput::default(), |ctx| {
            for font in [semibold(14.0), bold(26.0)] {
                assert!(ctx.fonts(|fonts| fonts.has_glyphs(&font, "Video Toolkit")));
            }
        });
    }

    #[test]
    fn home_paths_round_trip_and_other_tildes_are_left_alone() {
        let home = std::env::var("HOME").unwrap();
        let path = std::path::Path::new(&home).join("Movies/task");
        assert_eq!(display_path(&path), "~/Movies/task");
        assert_eq!(expand_home("~/Movies/task"), path);
        assert_eq!(expand_home("~"), std::path::PathBuf::from(&home));
        assert_eq!(
            expand_home("~other/x"),
            std::path::PathBuf::from("~other/x")
        );
        assert_eq!(expand_home("/abs/x"), std::path::PathBuf::from("/abs/x"));
    }

    #[test]
    fn tint_moves_toward_white() {
        assert_eq!(
            tint(Color32::from_rgb(0, 100, 200), 0.0),
            Color32::from_rgb(0, 100, 200)
        );
        assert_eq!(tint(Color32::BLACK, 1.0), Color32::WHITE);
    }

    #[test]
    fn bundled_ui_font_produces_visible_letter_glyphs() {
        let context = egui::Context::default();
        configure(&context);
        let mut visible = 0;
        let _ = context.run(egui::RawInput::default(), |ctx| {
            #[cfg(target_os = "macos")]
            if std::path::Path::new("/System/Library/Fonts/STHeiti Medium.ttc").is_file() {
                assert!(ctx
                    .fonts(|fonts| fonts.has_glyphs(&FontId::proportional(14.0), "字幕文档图片")));
            }
            let galley = ctx.fonts(|fonts| {
                fonts.layout_no_wrap(
                    "What do you want to make?".into(),
                    FontId::proportional(30.0),
                    TEXT,
                )
            });
            visible = galley
                .rows
                .iter()
                .flat_map(|row| &row.glyphs)
                .filter(|glyph| !glyph.chr.is_whitespace() && !glyph.uv_rect.is_nothing())
                .count();
        });
        assert_eq!(
            visible,
            "What do you want to make?"
                .chars()
                .filter(|c| !c.is_whitespace())
                .count()
        );
    }
}

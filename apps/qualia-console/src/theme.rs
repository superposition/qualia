//! The console's one visual language: palette, 8 px grid, type scale and the
//! few row/banner/table primitives every panel draws with.
//!
//! Traceability (`docs/frontend-lessons.md`, source 3): presentation lives in
//! one place so the panels cannot drift into different greys, while the values
//! themselves stay in the views. The palette is the operator console's: a
//! near-black page, one raised panel colour for every floating window, one
//! accent for live, one amber for degraded, one red for failure, and no
//! gradients or shadows.
//!
//! One family, one scale. Every string is drawn in the bundled monospace face
//! — the console is a numeric surface, so tabular figures and a stable
//! character width are the point — at three sizes: 16 px window titles, 13 px
//! values and body, 11 px field labels, units and table headers. Weights are
//! regular only; emphasis is colour, never bold.

use egui::{
    Align, Color32, CornerRadius, FontFamily, FontId, Layout, Margin, RichText, Stroke, Ui,
};

/// Window and page background.
pub const BG: Color32 = Color32::from_rgb(0x0E, 0x11, 0x16);
/// The menu strip, table headers and control fills.
pub const PANEL: Color32 = Color32::from_rgb(0x15, 0x1A, 0x21);
/// 1 px separators and inactive control outlines.
pub const SEP: Color32 = Color32::from_rgb(0x23, 0x2A, 0x33);
/// Primary text and numbers.
pub const TEXT: Color32 = Color32::from_rgb(0xED, 0xF0, 0xF5);
/// Secondary text: field labels and descriptive values.
pub const TEXT_SECOND: Color32 = Color32::from_rgb(0x9A, 0xA3, 0xAE);
/// Muted text: units, absent cells and the never-written state.
pub const MUTED: Color32 = Color32::from_rgb(0x5C, 0x66, 0x73);
/// Live / healthy.
pub const ACCENT: Color32 = Color32::from_rgb(0x91, 0xDB, 0xBA);
/// Degraded / stale.
pub const WARN: Color32 = Color32::from_rgb(0xE8, 0xC0, 0x7D);
/// Failure / unreachable.
pub const FAIL: Color32 = Color32::from_rgb(0xE0, 0x7A, 0x7A);

/// The 8 px rhythm; every gap below is one of these.
pub const GAP_S: f32 = 8.0;
pub const GAP: f32 = 16.0;
/// The strip's and a window body's padding.
pub const PAD: f32 = 24.0;
/// The strip's own padding, kept slim so the strip is one line.
pub const STRIP_PAD: f32 = 8.0;
/// One label/value row and one table row.
pub const ROW_H: f32 = 20.0;
/// The preferred label column of a label/value row, and its floor.
pub const LABEL_W: f32 = 240.0;
const MIN_LABEL_W: f32 = 88.0;
/// The value column never shrinks below this, so a narrow window truncates the
/// label column rather than the reading.
const MIN_VALUE_W: f32 = 120.0;
/// The fixed unit column beside a value, so numbers stay in one column whether
/// or not they carry a unit.
pub const UNIT_W: f32 = 56.0;

/// Type sizes.
pub const SIZE_LABEL: f32 = 11.0;
pub const SIZE_BODY: f32 = 13.0;
pub const SIZE_TITLE: f32 = 16.0;

/// The empty reading: a datum that is absent is never a zero.
pub const DASH: &str = "—";

/// One floating panel's default place on the page: `[x, y, width, height]`.
///
/// The six panels are a 3×2 grid below the strip: two rows of three at 24 px
/// margins and 24 px gutters, which fits 1280x820 — the window the console opens
/// with — and leaves the same absolute gaps at 1920x1080. Every panel is open on
/// start-up and none overlaps another, so the operator reads the whole stack at
/// once instead of paging between tabs; each is still movable and resizable, and
/// the Brain panel (the 3D scene) sits bottom-right at [856, 432, 392, 364].
pub const PANEL_LAYOUT: [[f32; 4]; 6] = [
    // Mission
    [24.0, 44.0, 392.0, 364.0],
    // Belief
    [440.0, 44.0, 392.0, 364.0],
    // World
    [856.0, 44.0, 392.0, 364.0],
    // Evidence
    [24.0, 432.0, 392.0, 364.0],
    // Telemetry
    [440.0, 432.0, 392.0, 364.0],
    // Brain
    [856.0, 432.0, 392.0, 364.0],
];

/// Install the palette and type scale. Applied every frame: the console has one
/// style, and the snapshot harness drives the same code the window does.
pub fn apply(ctx: &egui::Context) {
    ctx.style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(GAP_S, GAP_S);
        style.spacing.button_padding = egui::vec2(GAP_S, 4.0);
        style.spacing.window_margin = Margin::same(8);
        style.visuals.panel_fill = BG;
        style.visuals.window_fill = PANEL;
        style.visuals.window_stroke = Stroke::new(1.0, SEP);
        style.visuals.window_corner_radius = CornerRadius::same(4);
        style.visuals.window_shadow = egui::epaint::Shadow::NONE;
        style.visuals.popup_shadow = egui::epaint::Shadow::NONE;
        style.visuals.menu_corner_radius = CornerRadius::same(4);
        style.visuals.extreme_bg_color = BG;
        style.visuals.faint_bg_color = PANEL;
        style.visuals.override_text_color = Some(TEXT);
        style.visuals.selection.bg_fill = SEP;
        style.visuals.selection.stroke = Stroke::new(1.0, ACCENT);
        style.visuals.widgets.noninteractive.bg_fill = PANEL;
        style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, SEP);
        style.visuals.widgets.inactive.weak_bg_fill = PANEL;
        style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, SEP);
        style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_SECOND);
        style.visuals.widgets.hovered.weak_bg_fill = SEP;
        style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, TEXT_SECOND);
        style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
        style.visuals.widgets.active.weak_bg_fill = SEP;
        style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
        style.visuals.widgets.active.fg_stroke = Stroke::new(1.0, ACCENT);
        for widgets in [
            &mut style.visuals.widgets.noninteractive,
            &mut style.visuals.widgets.inactive,
            &mut style.visuals.widgets.hovered,
            &mut style.visuals.widgets.active,
            &mut style.visuals.widgets.open,
        ] {
            widgets.corner_radius = CornerRadius::same(4);
        }
        let mono = |size: f32| FontId::new(size, FontFamily::Monospace);
        style.text_styles = [
            (egui::TextStyle::Small, mono(SIZE_LABEL)),
            (egui::TextStyle::Body, mono(SIZE_BODY)),
            (egui::TextStyle::Button, mono(SIZE_BODY)),
            (egui::TextStyle::Heading, mono(SIZE_TITLE)),
            (egui::TextStyle::Monospace, mono(SIZE_BODY)),
        ]
        .into();
    });
}

/// One styled string in the console's single family.
/// The one type helper: every string in the console is one of three sizes, so a
/// panel cannot invent a fourth. Public so the HUD panels share it.
pub fn text(value: &str, size: f32, color: Color32) -> RichText {
    RichText::new(value)
        .font(FontId::new(size, FontFamily::Monospace))
        .color(color)
}

/// A flat colour mixed `weight` of the way from the page background toward
/// `color`: the pill and banner fills, with no gradients.
fn tint(color: Color32, weight: f32) -> Color32 {
    let mix = |channel: u8, base: u8| {
        (channel as f32 * weight + base as f32 * (1.0 - weight)).round() as u8
    };
    Color32::from_rgb(
        mix(color.r(), BG.r()),
        mix(color.g(), BG.g()),
        mix(color.b(), BG.b()),
    )
}

/// The accent ramp `t` in 0..1: muted ink for an idle cell, the live accent for
/// a firing one. One function, so the scene and the matrix heatmaps cannot
/// disagree about what "bright" means.
pub fn ramp(intensity: f32) -> Color32 {
    let weight = intensity.clamp(0.0, 1.0);
    let mix = |from: u8, to: u8| {
        (from as f32 * (1.0 - weight) + to as f32 * weight).round() as u8
    };
    Color32::from_rgb(
        mix(MUTED.r(), ACCENT.r()),
        mix(MUTED.g(), ACCENT.g()),
        mix(MUTED.b(), ACCENT.b()),
    )
}

/// A full-width 1 px separator.
pub fn hairline(ui: &mut Ui) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, SEP);
}

/// The label column's width in a row `width` wide: the preferred 240 px when
/// there is room, never squeezing the value below 120 px.
fn label_width(width: f32) -> f32 {
    LABEL_W.min((width - UNIT_W - MIN_VALUE_W).max(MIN_LABEL_W))
}

/// The one error surface: a full-width band at the top of a window body, so a
/// failure sits in the same place in every panel.
pub fn error_banner(ui: &mut Ui, message: &str) {
    let width = ui.available_width();
    egui::Frame::NONE
        .fill(tint(FAIL, 0.15))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_min_width((width - 24.0).max(0.0));
            ui.label(text(message, SIZE_BODY, FAIL));
        });
    ui.add_space(GAP_S);
}

/// A named state or an absent reading: one line, in a colour that carries the
/// state. Never a box, never a zero.
pub fn state_line(ui: &mut Ui, message: &str, color: Color32) {
    ui.label(text(message, SIZE_BODY, color));
}

/// A muted line standing for something the console cannot show yet.
pub fn absent(ui: &mut Ui, message: &str) {
    state_line(ui, message, MUTED);
}

/// The status pill.
pub fn status_pill(ui: &mut Ui, label: &str, color: Color32) {
    egui::Frame::NONE
        .fill(tint(color, 0.18))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.label(text(label, SIZE_LABEL, color));
        });
}

/// A label/value row: the label in the label column, the value right-aligned in
/// the value column with its unit in the muted unit column.
pub fn field(ui: &mut Ui, label: &str, value: &str, unit: Option<&str>) {
    let width = ui.available_width();
    let label_w = label_width(width);
    let value_w = (width - label_w - UNIT_W).max(MIN_VALUE_W);
    ui.allocate_ui_with_layout(
        egui::vec2(width, ROW_H),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(label_w, ROW_H),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.label(text(label, SIZE_LABEL, TEXT_SECOND));
                },
            );
            ui.allocate_ui_with_layout(
                egui::vec2(value_w, ROW_H),
                Layout::right_to_left(Align::Center),
                |ui| {
                    ui.label(text(value, SIZE_BODY, TEXT));
                },
            );
            ui.allocate_ui_with_layout(
                egui::vec2(UNIT_W, ROW_H),
                Layout::left_to_right(Align::Center),
                |ui| {
                    if let Some(unit) = unit {
                        ui.label(text(unit, SIZE_LABEL, MUTED));
                    }
                },
            );
        },
    );
}

/// A label/value row whose value is text: left-aligned in the value column so a
/// sentence or an event name reads from its start.
pub fn field_text(ui: &mut Ui, label: &str, value: &str) {
    let width = ui.available_width();
    let label_w = LABEL_W.min((width - MIN_VALUE_W).max(MIN_LABEL_W));
    ui.allocate_ui_with_layout(
        egui::vec2(width, ROW_H),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(label_w, ROW_H),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.label(text(label, SIZE_LABEL, TEXT_SECOND));
                },
            );
            ui.allocate_ui_with_layout(
                egui::vec2((width - label_w).max(MIN_VALUE_W), ROW_H),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.label(text(value, SIZE_BODY, TEXT));
                },
            );
        },
    );
}

/// A label/value row whose value is a path or identifier: truncated with an
/// ellipsis rather than overflowing the column.
pub fn field_path(ui: &mut Ui, label: &str, value: &str) {
    let width = ui.available_width();
    let label_w = LABEL_W.min((width - MIN_VALUE_W).max(MIN_LABEL_W));
    ui.allocate_ui_with_layout(
        egui::vec2(width, ROW_H),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(label_w, ROW_H),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.label(text(label, SIZE_LABEL, TEXT_SECOND));
                },
            );
            ui.allocate_ui_with_layout(
                egui::vec2((width - label_w).max(MIN_VALUE_W), ROW_H),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.add(egui::Label::new(text(value, SIZE_BODY, TEXT)).truncate());
                },
            );
        },
    );
}

fn table_cell(ui: &mut Ui, width: f32, value: &str, color: Color32, right: bool, size: f32) {
    let layout = if right {
        Layout::right_to_left(Align::Center)
    } else {
        Layout::left_to_right(Align::Center)
    };
    ui.allocate_ui_with_layout(egui::vec2(width, ROW_H), layout, |ui| {
        ui.add(egui::Label::new(text(value, size, color)).truncate());
    });
}

/// A table header row in the label type, followed by a hairline. A cell too
/// narrow for its text shows an ellipsis rather than overflowing the column.
pub fn header(ui: &mut Ui, columns: &[(&str, f32, bool)]) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (label, width, right) in columns {
            table_cell(ui, *width, label, MUTED, *right, SIZE_LABEL);
        }
    });
    hairline(ui);
}

/// One table row. `right` aligns a numeric column; the caller picks the colour
/// so a state word can carry it.
pub fn cells(ui: &mut Ui, cells: &[(&str, f32, Color32, bool)]) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (value, width, color, right) in cells {
            table_cell(ui, *width, value, *color, *right, SIZE_BODY);
        }
    });
}

/// A relative age: the operator reads `238 ms ago`, not a raw timestamp.
pub fn age(observed_at_ns: u64, timestamp_ns: u64) -> String {
    if timestamp_ns == 0 {
        return "never".to_owned();
    }
    let ms = observed_at_ns.saturating_sub(timestamp_ns) / 1_000_000;
    if ms < 1_000 {
        format!("{ms} ms ago")
    } else if ms < 60_000 {
        format!("{:.1} s ago", ms as f64 / 1_000.0)
    } else if ms < 3_600_000 {
        format!("{} m ago", ms / 60_000)
    } else {
        format!("{} h ago", ms / 3_600_000)
    }
}

/// The one slim strip: the product name and the `Windows` menu that re-opens a
/// closed panel. Nothing else is chrome; the status and every reading live in
/// the panels.
pub fn menu_strip(ui: &mut Ui, state: &mut crate::ConsoleState) {
    let strip = egui::TopBottomPanel::top("qualia_console_strip")
        .frame(
            egui::Frame::NONE
                .fill(PANEL)
                .inner_margin(Margin::symmetric(STRIP_PAD as i8, 4)),
        )
        .show_inside(ui, |ui| {
            ui.set_min_width(ui.available_width());
            egui::MenuBar::new().ui(ui, |ui| {
                ui.label(text("Qualia Console", SIZE_BODY, TEXT));
                ui.add_space(GAP);
                ui.menu_button(text("Console", SIZE_BODY, TEXT_SECOND), |ui| {
                    for view in crate::VIEWS {
                        let mut open = state.windows.is_open(view);
                        if ui.checkbox(&mut open, view.label()).changed() {
                            state.windows.set(view, open);
                        }
                    }
                    ui.separator();
                    if ui.button("Poll now").clicked() {
                        state.refresh_requested = true;
                        ui.close();
                    }
                });
                ui.menu_button(text("HUD", SIZE_BODY, TEXT_SECOND), |ui| {
                    let mut on = state.hud_layout.on;
                    if ui
                        .checkbox(&mut on, "HUD panels (Ctrl+H)")
                        .changed()
                    {
                        state.hud_layout.set_on(on);
                    }
                    ui.separator();
                    if state.hud.rows.is_empty() {
                        // Nothing published: name the reason rather than draw an
                        // empty panel per runner the stack declares.
                        let reason = state
                            .hud
                            .error
                            .as_deref()
                            .unwrap_or("no runner has published a frame yet");
                        ui.label(text(reason, SIZE_LABEL, MUTED));
                    } else {
                        if ui.button("Show all").clicked() {
                            let runners: Vec<String> = state
                                .hud
                                .rows
                                .iter()
                                .map(|row| row.runner.clone())
                                .collect();
                            state.hud_layout.set_all(&runners, true);
                        }
                        if ui.button("Hide all").clicked() {
                            let runners: Vec<String> = state
                                .hud
                                .rows
                                .iter()
                                .map(|row| row.runner.clone())
                                .collect();
                            state.hud_layout.set_all(&runners, false);
                        }
                        ui.separator();
                        for row in &state.hud.rows {
                            let mut open = state.hud_layout.is_open(&row.runner);
                            if ui
                                .checkbox(&mut open, text(&row.runner, SIZE_BODY, TEXT))
                                .changed()
                            {
                                state.hud_layout.set(&row.runner, open);
                            }
                        }
                    }
                    if !state.hud.gaps.is_empty() {
                        ui.separator();
                        ui.label(text("no producer yet", SIZE_LABEL, TEXT_SECOND));
                        for gap in &state.hud.gaps {
                            ui.label(text(&format!("{gap} — no producer"), SIZE_LABEL, MUTED));
                        }
                    }
                });
            });
        });
    let rect = strip.response.rect;
    ui.painter().line_segment(
        [
            egui::pos2(rect.left(), rect.bottom()),
            egui::pos2(rect.right(), rect.bottom()),
        ],
        Stroke::new(1.0, SEP),
    );
}

/// One floating panel: a movable, resizable, collapsible, closable window at
/// its own default grid position, with a scrolling body.
pub fn panel(
    ctx: &egui::Context,
    view: crate::View,
    open: &mut bool,
    add_contents: impl FnOnce(&mut Ui),
) {
    let index = crate::VIEWS
        .iter()
        .position(|candidate| *candidate == view)
        .unwrap_or(0);
    let [x, y, width, height] = PANEL_LAYOUT[index];
    egui::Window::new(view.label())
        .id(egui::Id::new(("qualia_console_panel", view.label())))
        .default_pos(egui::pos2(x, y))
        .default_size(egui::vec2(width, height))
        .resizable(true)
        .collapsible(true)
        .open(open)
        .show(ctx, |ui| {
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| add_contents(ui));
        });
}

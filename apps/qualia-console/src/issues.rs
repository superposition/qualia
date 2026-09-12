//! Keep unavailable sources together without hiding usable readings.

use crate::{theme, views::coach::CoachView, ConsoleState, Connection, View};

fn show_panels_id() -> egui::Id {
    egui::Id::new("qualia_show_unavailable_panels")
}

pub(crate) fn show_unavailable_panels(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp(show_panels_id()).unwrap_or(false))
}

/// A partial failure must not take the remaining data off the screen.
pub(crate) fn unavailable(state: &ConsoleState, view: View) -> bool {
    match view {
        View::Mission => !state.connection.is_live(),
        View::Belief => state.belief.error.is_some() && state.belief.is_empty(),
        View::World => state.world.error.is_some() && state.world.is_empty(),
        View::Evidence => state.evidence.error.is_some() && state.evidence.is_empty(),
        View::Telemetry => state.telemetry.error.is_some() && state.telemetry.is_empty(),
        // The scene has independently loaded geometry even without live SHM.
        View::Brain => false,
    }
}

/// Equal reasons share one entry, with every affected source named beside it.
pub(crate) fn render(ui: &mut egui::Ui, state: &ConsoleState) {
    let mut groups: Vec<(String, Vec<&str>)> = Vec::new();
    let mut add = |source: &'static str, reason: Option<&str>| {
        if let Some(reason) = reason {
            if let Some((_, sources)) = groups.iter_mut().find(|(text, _)| text == reason) {
                sources.push(source);
            } else {
                groups.push((reason.to_owned(), vec![source]));
            }
        }
    };
    if let Connection::Unreachable { reason } = &state.connection {
        add("Mission", Some(reason));
    }
    add("Belief", state.belief.error.as_deref());
    add("World", state.world.error.as_deref());
    add("Evidence", state.evidence.error.as_deref());
    add("Telemetry", state.telemetry.error.as_deref());
    add("Brain", state.brain.error.as_deref());
    add("Brain assets", state.brain_assets.error.as_deref());
    add("HUD", state.hud.error.as_deref());
    let coach_reason = state.coach.degraded_line();
    add("Coach", coach_reason.as_deref());
    if groups.is_empty() {
        return;
    }

    egui::TopBottomPanel::bottom("qualia_issues")
        .frame(egui::Frame::NONE.fill(theme::PANEL).inner_margin(8))
        .show_inside(ui, |ui| {
            egui::CollapsingHeader::new(theme::text(
                &format!("Issues ({})", groups.len()),
                theme::SIZE_BODY,
                theme::WARN,
            ))
            .id_salt("qualia_issues_details")
            .default_open(false)
            .show(ui, |ui| {
                let mut show = show_unavailable_panels(ui.ctx());
                if ui.checkbox(&mut show, "Show unavailable panels").changed() {
                    ui.ctx().data_mut(|data| data.insert_temp(show_panels_id(), show));
                }
                egui::ScrollArea::vertical()
                    .max_height(200.0)
                    .show(ui, |ui| {
                        for (reason, sources) in &groups {
                            ui.label(theme::text(
                                &sources.join(", "),
                                theme::SIZE_BODY,
                                theme::WARN,
                            ));
                            ui.label(theme::text(reason, theme::SIZE_LABEL, theme::TEXT_SECOND));
                        }
                    });
            });
        });
}

pub(crate) fn coach_visible(ctx: &egui::Context, state: &ConsoleState) -> bool {
    matches!(state.coach, CoachView::Live(_)) || show_unavailable_panels(ctx)
}

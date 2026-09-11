//! Mission view: the braid state the local agent reports on `GET /braid`.
//!
//! Traceability (`docs/frontend-lessons.md`, "What this means for
//! `apps/qualia-console`"):
//!
//! - source 1 — mode and provenance in the panel: it says which agent, session
//!   and generation it is showing rather than implying it is the only one;
//! - source 2 — a dead service degrades a status rather than blanking the
//!   panel, so the degraded arm below renders the committed fixture instead of
//!   an empty window;
//! - source 3 — named degraded states, asserted by accessible label.

use egui::Ui;

use crate::{theme, ConsoleState, Connection};

pub fn render(ui: &mut Ui, state: &ConsoleState) {
    if let Connection::Unreachable { reason, .. } = &state.connection {
        theme::error_banner(ui, &format!("mission degraded: {reason}"));
        theme::state_line(
            ui,
            "rendering committed fixture braid-state.json",
            theme::TEXT_SECOND,
        );
        ui.add_space(theme::GAP_S);
    }

    let (label, color) = match state.connection {
        Connection::Live => ("connected", theme::ACCENT),
        Connection::Unreachable { .. } => ("degraded", theme::WARN),
    };
    theme::status_pill(ui, label, color);
    ui.add_space(theme::GAP_S);

    theme::field_path(ui, "agent", &state.agent_url);
    theme::field_text(ui, "session", &state.braid.session_id);
    theme::field(ui, "generation", &state.braid.generation.to_string(), None);
    theme::field(
        ui,
        "open missions",
        &state.braid.open_missions.to_string(),
        None,
    );
    theme::field(
        ui,
        "last promotion",
        &theme::age(state.observed_at_ns, state.braid.last_promotion_ns),
        None,
    );
    theme::field(
        ui,
        "last quarantine",
        &theme::age(
            state.observed_at_ns,
            state.braid.last_quarantine_ns.unwrap_or(0),
        ),
        None,
    );

    ui.add_space(theme::GAP_S);
    theme::hairline(ui);
    ui.add_space(theme::GAP_S);

    match &state.drift {
        Some(drift) if drift.sample_count > 0 => {
            theme::field(ui, "drift mahalanobis", &format!("{:.3}", drift.mahalanobis), None);
            theme::field(ui, "drift samples", &drift.sample_count.to_string(), None);
        }
        // `sample_count == 0` is the malformed-sample case the braid treats as
        // "no opinion"; the console must not turn it into a number.
        _ => theme::state_line(ui, "drift: no opinion", theme::TEXT_SECOND),
    }
}

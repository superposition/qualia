//! Mission view: the braid state the local agent reports on `GET /braid`.
//!
//! Traceability (`docs/frontend-lessons.md`, "What this means for
//! `apps/qualia-console`"):
//!
//! - source 1 — mode and provenance in the chrome: the panel says which agent,
//!   session and generation it is showing rather than implying it is the only
//!   one;
//! - source 2 — a dead service degrades a status rather than blanking the
//!   window, so the degraded arm below renders the committed fixture instead of
//!   an empty view;
//! - source 3 — named degraded states, asserted by accessible label.

use egui::Ui;

use crate::{format_age_ms, ConsoleState, Connection};

/// The view's own heading; distinct from the tab label in [`crate::VIEWS`].
pub const HEADING: &str = "Braid mission";

/// How long a promotion or a quarantine stays interesting in the header.
fn age_label(observed_at_ns: u64, timestamp_ns: u64, never: &str) -> String {
    if timestamp_ns == 0 {
        never.to_owned()
    } else {
        format!("{} ms", format_age_ms(observed_at_ns, timestamp_ns))
    }
}

pub fn render(ui: &mut Ui, state: &ConsoleState) {
    ui.heading(HEADING);

    if let Connection::Unreachable { reason, .. } = &state.connection {
        ui.colored_label(
            egui::Color32::from_rgb(224, 160, 138),
            format!("mission degraded: {reason}"),
        );
        ui.label("rendering committed fixture braid-state.json");
    }

    ui.label(format!("session: {}", state.braid.session_id));
    ui.label(format!("generation: {}", state.braid.generation));
    ui.label(format!("open missions: {}", state.braid.open_missions));
    ui.label(format!(
        "last promotion age: {}",
        age_label(state.observed_at_ns, state.braid.last_promotion_ns, "never")
    ));
    ui.label(format!(
        "last quarantine age: {}",
        age_label(state.observed_at_ns, state.braid.last_quarantine_ns.unwrap_or(0), "never")
    ));

    ui.separator();
    match &state.drift {
        Some(drift) if drift.sample_count > 0 => {
            ui.label(format!("drift mahalanobis: {:.3}", drift.mahalanobis));
            ui.label(format!("drift samples: {}", drift.sample_count));
        }
        // `sample_count == 0` is the malformed-sample case the braid treats as
        // "no opinion"; the console must not turn it into a number.
        _ => {
            ui.label("drift: no opinion");
        }
    }
}

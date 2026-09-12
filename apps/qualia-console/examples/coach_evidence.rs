//! Render the live Coach panel to a PNG, with no window on any desktop.
//!
//! The console's Coach panel reads the mission broker's status surface
//! (`GET /coach`, T63). This example renders the real `render_view` — six views
//! plus the panel — through `egui_kittest`'s wgpu backend over **live**
//! readings: the broker's own payload and the agent's `GET /braid`. Nothing is
//! a fixture; if the broker is not running, the frame shows the named degrade
//! line the window would show.
//!
//! The figure is written where `egui_kittest` keeps this crate's snapshots;
//! copy it into the ticket's `docs/evidence/` after the run.
//!
//! ```console
//! $ QUALIA_AGENT_URL=https://127.0.0.1:8080 QUALIA_AGENT_TLS_DIR=$HOME/.qualia_tls \
//!     UPDATE_SNAPSHOTS=1 cargo run -p qualia-console --example coach_evidence
//! ```

use egui_kittest::{Harness, SnapshotOptions};
use qualia_console::client::{agent_url, BraidSource, HttpSource};
use qualia_console::views::coach::{coach_url, sample as coach_sample, CoachView};
use qualia_console::views::evidence::EvidenceView;
use qualia_console::{shm_sample, stack, Connection, ConsoleState, Sample};

/// Wide enough for the six-view grid plus the Coach panel in the right margin,
/// which is where the panel places itself.
const VIEWPORT: [f32; 2] = [1920.0, 900.0];

fn main() {
    let region = shm_sample::region_name();
    let sensing = stack::load_sensing_set();
    let observed_at_ns = qualia_console::now_ns();
    let shm = shm_sample::sample(&region, &sensing);

    let url = agent_url();
    let (connection, braid, drift) = match HttpSource::new(url.clone())
        .and_then(|mut source| BraidSource::fetch(&mut source))
    {
        Ok(snapshot) => (Connection::Live, snapshot.braid, snapshot.drift),
        Err(reason) => {
            eprintln!("coach_evidence: agent did not answer: {reason}");
            let fixture = qualia_console::fixture();
            (
                Connection::Unreachable { reason },
                fixture.braid,
                fixture.drift,
            )
        }
    };

    let mut brain = shm.brain;
    brain.record_braid(&braid);
    brain.observe_coupling_scale(observed_at_ns);
    let broker = coach_url();
    let sample = Sample {
        observed_at_ns,
        connection,
        braid,
        drift,
        belief: shm.belief,
        world: shm.world,
        telemetry: shm.telemetry,
        evidence: EvidenceView::default(),
        brain,
        stats: shm.stats,
        coach: coach_sample(&broker),
    };

    let mut state = ConsoleState::from_sample(sample, url);
    // The figure is the six views plus the Coach panel; the per-runner HUD
    // panels are T60's figure, not this one.
    state.hud_layout.set_on(false);

    match &state.coach {
        CoachView::Live(snapshot) => {
            let model = snapshot
                .model
                .as_ref()
                .map(|model| format!("{} ({})", model.model_id, model.status))
                .unwrap_or_else(|| "no model state".to_string());
            println!(
                "coach_evidence: broker {broker} -> {model}, {} decision(s), {} mission(s)",
                snapshot.decisions.len(),
                snapshot.missions.len()
            );
            for decision in snapshot.decisions.iter().take(3) {
                println!(
                    "coach_evidence: decision {} {} -> {} · model {} · latency {} ms · tokens {}/{} · response {} · priors_ablated={}",
                    decision.decision_id,
                    decision.decision_kind,
                    decision.target_proposal_ids.join(","),
                    decision.model_id.as_deref().unwrap_or("none"),
                    decision
                        .latency_ms
                        .map(|millis| millis.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    decision
                        .prompt_tokens
                        .map(|tokens| tokens.to_string())
                        .unwrap_or_else(|| "no usage".to_string()),
                    decision
                        .completion_tokens
                        .map(|tokens| tokens.to_string())
                        .unwrap_or_else(|| "no usage".to_string()),
                    decision.response_id.as_deref().unwrap_or("-"),
                    decision.llm_priors_ablated,
                );
            }
            for mission in snapshot.missions.iter().take(3) {
                println!(
                    "coach_evidence: mission {} {} accepted={} ack={:?}",
                    mission.mission_id, mission.command, mission.accepted, mission.ack_http_status
                );
            }
            for line in snapshot.degradations.iter().take(3) {
                println!("coach_evidence: broker degradation: {line}");
            }
        }
        CoachView::Unreachable { .. } => {
            println!(
                "coach_evidence: {}",
                state.coach.degraded_line().unwrap_or_default()
            );
        }
    }

    let mut harness = Harness::builder()
        .with_size(egui::Vec2::new(VIEWPORT[0], VIEWPORT[1]))
        .wgpu()
        .build_ui_state(|ui, state| qualia_console::render_view(ui, state), state);
    harness.run();
    harness.run();
    harness.snapshot_options("coach_evidence", &SnapshotOptions::new());
    println!("coach_evidence: frame written");
}

//! Mission-panel tests: the wire contract of `GET /braid`, the rows the panel
//! draws with and without a mission, and the poller that feeds them.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use qualia_watch::braid::{
    agent_url_from, braid_line, BraidPoller, BraidState, Mission, MissionStatus, BRAID_STATE_SCHEMA,
};

/// `GET /braid`'s body, as the agent writes it: one mission open.
const BRAID_JSON: &str = concat!(
    r#"{"schema_version":"qualia.braid-state.v1","generation":12,"#,
    r#""session_id":"sess-2026-09-11-explore-frontier","open_missions":1,"#,
    r#""last_promotion_ns":1789084800000000000,"last_quarantine_ns":null}"#
);

/// How many requests the stub answers before it stops accepting.
const SERVE_LIMIT: usize = 16;

fn state(open_missions: u32) -> BraidState {
    BraidState {
        schema_version: BRAID_STATE_SCHEMA.to_string(),
        generation: 12,
        session_id: "sess-2026-09-11-explore-frontier".to_string(),
        open_missions,
        last_promotion_ns: 1_789_084_800_000_000_000,
        last_quarantine_ns: None,
    }
}

/// A one-endpoint HTTP server on a loopback port, answering every request with
/// `body` until the test process ends. The request line of each exchange is
/// reported back, so a test can pin the path the poller dials.
fn serve(body: &'static str) -> (String, Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
    let url = format!("http://{}", listener.local_addr().expect("local address"));
    let (lines, requests) = mpsc::channel();
    thread::spawn(move || {
        for _ in 0..SERVE_LIMIT {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = [0u8; 1024];
            let read = stream.read(&mut request).unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..read]);
            let line = request.lines().next().unwrap_or_default().to_string();
            let _ = lines.send(line);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (url, requests)
}

/// Wait for the poller's first answer, within a budget past one request.
fn take(poller: &BraidPoller) -> MissionStatus {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = poller.try_take() {
            return status;
        }
        assert!(Instant::now() < deadline, "the poller never answered");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn poller_takes_the_agents_view_off_the_wire() {
    let (url, requests) = serve(BRAID_JSON);
    let poller = BraidPoller::spawn(url);
    assert_eq!(take(&poller), MissionStatus::State(state(1)));
    // The route the agent serves. The stub answers every path, so only the
    // request line itself can pin `/braid`.
    assert_eq!(
        requests
            .recv_timeout(Duration::from_secs(5))
            .expect("the stub saw the poller's request"),
        "GET /braid HTTP/1.1"
    );
}

#[test]
fn the_default_agent_url_is_the_agents_tls_surface() {
    // The agent's only listener is rustls (`runners/agent`), so a plaintext
    // loopback default can never reach `GET /braid`. The URL resolved here is
    // the one handed to `Mission` and `BraidPoller`.
    assert_eq!(agent_url_from(None, None), "https://127.0.0.1:8080");
    assert_eq!(
        agent_url_from(None, Some("18081")),
        "https://127.0.0.1:18081"
    );
    assert_eq!(agent_url_from(None, Some("  ")), "https://127.0.0.1:8080");
}

#[test]
fn a_configured_agent_url_wins_over_the_loopback_default() {
    assert_eq!(
        agent_url_from(Some("http://192.0.2.10:9000/"), Some("18081")),
        "http://192.0.2.10:9000"
    );
    assert_eq!(
        agent_url_from(Some("  "), Some("18081")),
        "https://127.0.0.1:18081"
    );
}

#[test]
fn poller_reports_an_agent_that_is_not_there() {
    // Nothing listens on the discard port, so the first request cannot land.
    let poller = BraidPoller::spawn("http://127.0.0.1:9");
    match take(&poller) {
        MissionStatus::Unreachable(reason) => assert!(!reason.is_empty()),
        other => panic!("expected no answer, got {other:?}"),
    }
}

#[test]
fn panel_rows_carry_the_braid_and_an_open_mission() {
    let mut mission = Mission::pending("http://127.0.0.1:8080");
    assert!(!mission.degraded());
    mission.set_status(MissionStatus::State(state(1)));
    assert_eq!(
        mission.lines(),
        vec![
            ("agent".to_string(), "http://127.0.0.1:8080".to_string()),
            (
                "session".to_string(),
                "sess-2026-09-11-explore-frontier".to_string()
            ),
            ("generation".to_string(), "12".to_string()),
            ("missions".to_string(), "1 open".to_string()),
            ("promotion".to_string(), "00:00:00.000".to_string()),
            ("quarantine".to_string(), "never".to_string()),
            ("schema".to_string(), BRAID_STATE_SCHEMA.to_string()),
        ]
    );
}

#[test]
fn the_mission_row_spells_out_the_empty_case() {
    for (open, expected) in [(0, "none open"), (1, "1 open"), (3, "3 open")] {
        let mut mission = Mission::pending("http://127.0.0.1:8080");
        mission.set_status(MissionStatus::State(state(open)));
        let row = mission
            .lines()
            .into_iter()
            .find(|(label, _)| label == "missions")
            .expect("every braid draws a mission row");
        assert_eq!(row.1, expected, "{open} open missions");
    }
}

#[test]
fn panel_names_the_agent_and_the_reason_when_it_is_missing() {
    let mut mission = Mission::pending("http://127.0.0.1:8080");
    mission.set_status(MissionStatus::Unreachable("tcp timeout".to_string()));
    assert!(mission.degraded());
    assert_eq!(
        mission.lines(),
        vec![
            ("agent".to_string(), "http://127.0.0.1:8080".to_string()),
            ("state".to_string(), "unreachable: tcp timeout".to_string()),
        ]
    );
}

#[test]
fn the_braid_line_renders_whole_milliseconds_and_the_generation() {
    let status = MissionStatus::State(state(1));
    assert_eq!(braid_line(&status, 0), "braid gen 12 · belief lag: 0 ms");
    assert_eq!(
        braid_line(&status, 999_999),
        "braid gen 12 · belief lag: 0 ms"
    );
    assert_eq!(
        braid_line(&status, 1_000_000),
        "braid gen 12 · belief lag: 1 ms"
    );
    assert_eq!(
        braid_line(&status, 1_500_000_000),
        "braid gen 12 · belief lag: 1500 ms"
    );
    assert_eq!(
        braid_line(&MissionStatus::Pending, 42_000_000),
        "braid gen unknown · belief lag: 42 ms"
    );
}

//! The background poller: HTTP off the UI thread, a dead service into a status.
//!
//! Traceability (`docs/frontend-lessons.md`, source 2): the dashboard's
//! command/message channel pair is kept, with the named cadence constants, so a
//! slow or dead agent degrades a chip instead of freezing the window. The
//! source-3 lesson is behind the trait: the live agent and the committed fixture
//! are interchangeable, and the degrade path never asks the UI to invent data.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::client::BraidSource;
use crate::sample::Sample;
use crate::views::evidence::EvidenceScan;
use crate::{client, shm_sample, now_ns};

/// How often a healthy agent is polled.
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// How long to wait before re-trying an agent that did not answer.
pub const RETRY_INTERVAL: Duration = Duration::from_millis(1000);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollerCommand {
    RefreshNow,
    Shutdown,
}

/// Owns the worker thread and both halves of the channels.
pub struct Poller {
    commands: Sender<PollerCommand>,
    samples: Receiver<Sample>,
    handle: Option<JoinHandle<()>>,
}

impl Poller {
    /// Start polling `source`, degrading through `fallback` on any failure.
    pub fn spawn(mut source: Box<dyn BraidSource>, mut fallback: Box<dyn BraidSource>) -> Self {
        let (command_tx, command_rx) = mpsc::channel::<PollerCommand>();
        let (sample_tx, sample_rx) = mpsc::channel::<Sample>();

        let handle = thread::spawn(move || {
            worker(&mut *source, &mut *fallback, &command_rx, &sample_tx);
        });

        Self {
            commands: command_tx,
            samples: sample_rx,
            handle: Some(handle),
        }
    }

    /// Ask for a poll now rather than at the next interval.
    pub fn refresh_now(&self) {
        let _ = self.commands.send(PollerCommand::RefreshNow);
    }

    /// The newest sample, if the worker produced one since the last call.
    pub fn try_recv(&self) -> Option<Sample> {
        self.samples.try_recv().ok()
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        let _ = self.commands.send(PollerCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn worker(
    source: &mut dyn BraidSource,
    fallback: &mut dyn BraidSource,
    commands: &Receiver<PollerCommand>,
    samples: &Sender<Sample>,
) {
    let evidence_root = crate::views::evidence::evidence_root();
    // The stack's runner set is a deployment fact, not a per-poll one: read the
    // manifest once and hold it for the life of the poller.
    let sensing = crate::stack::load_sensing_runner_names();
    let mut evidence = EvidenceScan::default();

    loop {
        let observed_at_ns = now_ns();
        let region = shm_sample::region_name();
        let shm = shm_sample::sample(&region, &sensing);

        let sample = match source.fetch() {
            Ok(snapshot) => Sample {
                observed_at_ns,
                connection: crate::Connection::Live,
                braid: snapshot.braid,
                drift: snapshot.drift,
                belief: shm.belief,
                world: shm.world,
                telemetry: shm.telemetry,
                evidence: evidence.refresh(&evidence_root, shm.ledger),
            },
            Err(reason) => {
                // The fallback is the committed fixture source; if even that
                // cannot answer, the compiled-in fixture still can.
                let fixture = fallback.fetch().unwrap_or_else(|_| client::fixture());
                Sample::degraded(&fixture, &reason, observed_at_ns)
            }
        };

        let live = sample.connection == crate::Connection::Live;
        if samples.send(sample).is_err() {
            return;
        }

        let interval = if live { POLL_INTERVAL } else { RETRY_INTERVAL };
        match commands.recv_timeout(interval) {
            Ok(PollerCommand::RefreshNow) => {}
            Ok(PollerCommand::Shutdown) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

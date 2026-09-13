# Live model and wheel evidence in the console

This checkpoint restores visible, independently movable Brain, Camera, DeepSeek,
camera gimbal, lidar, CNS inputs/readout, and model-matrix dialogs. Other robot
views remain selectable from Dialogs. The console observes wheel transport; it
does not acquire wheel authority or launch a drive.

## What each model view means

- Brain reads actual flyvis model indices, retinal sample coordinates, continuous
  voltage, and signed update values. Brightness is rectified voltage on a labeled
  display scale. These are not spikes, Hz, or MaleCNS body IDs.
- Model matrices reads the actual 65-by-65 mean signed effective edge weights and
  edge counts. Each square aggregates one target/source cell-type pair; an absent
  pair is distinct from a zero-valued weight. These pretrained parameters remain
  frozen.
- Recurrence exposes the final actual 20 ms integration substep: before voltage,
  direct input, recurrent drive, bias, time constant, alpha, after voltage, and
  delta. Alpha is `dt / max(tau, dt)`. The producer preserves its float32 operation
  order. Its last-substep delta is different from the whole-output-interval delta
  in the Brain cell endpoint. Camera drive enters R1–R8; downstream T4a receives
  camera-conditioned recurrent input and correctly has zero direct input drive.
- Robot L2 shows the real 1024-component latent boundary and precision from Leash.
  Host beliefs shows separately labeled L3–L6 activation, weight-RMS, and weight
  delta sketches. The display distinguishes initialized sequence-zero state from
  stepped state. Neither view recasts latent values as probabilities.

The matrix panel places connectivity and selected-cell integration values side
by side. Views scroll and resize. The final visual inspection maximized the
console so the seven dialogs, their plots, and integration values were visible
together. UIA alone was insufficient to detect clipped contents at small window
sizes; the actual window capture was also inspected.

## Source and authority contracts

The visual `/status`, `/cells`, and `/matrices` feeds are polled independently.
Run/model identity, source-image hash, receipt time, output time, finite numeric
values, bounded shapes, and source freshness gate live color. Historical values
remain labeled and gray after loss of a source. The model-cell selector also
selects the matrix endpoint's cell type.

CNS inputs follows the active visual producer. Body rows prefer the exact
timestamps used by the engineered wheel readout, with a separately labeled
measured-context fallback. Camera enters the visual model; IMU, odometry, and
lidar condition the engineered downstream readout.

Wheel evidence separates shadow proposals, emitted JSONL frames, external
transport attachment, consumed trace ticks, and the global Leash action ledger.
The bridge reports either zero-only mode or explicitly enabled bounded mode.
Forwarding and attachment do not establish physical movement. The global ledger
is explicitly not attributed to the transport session. Reacquisition means no
new stdin writes; the existing transport owns its arrival-based deadman.

`QUALIA_WHEEL_PROBE_STATUS` reads the dated zero-drive integration result. Its
HTTP status, error, and verified-stop acknowledgement are historical evidence,
not a continuously refreshed robot health check. The final observed result was
zero-drive HTTP 200 / `ok: true`, followed by verified-stop HTTP 200 /
`acknowledged: true`.

Optional endpoint overrides are `QUALIA_FLYVIS_URL`,
`QUALIA_WHEEL_SHADOW_URL`, and `QUALIA_WHEEL_TRANSPORT_URL`. Defaults derive the
robot's visual port 8091 and shadow port 8092; external transport status defaults
to `http://127.0.0.1:8093/status`. Credentials are not recorded in this evidence.

## Verification

The combined cached console/coach build passed, followed by the final console
build after the bridge schema and viewport fixes. The last console build used:

```powershell
$env:CARGO_TARGET_DIR='C:/Users/ericm/qualia/target'
cargo build --offline --locked -p qualia-console --bin qualia-console -j 2
```

It passed in 26.91 seconds. No additional tests, GPU jobs, hardware commands, or
gimbal actions were run for this UI checkpoint. Verification used HTTP reads,
UIA reads, non-actuating matrix display selections, and console-window capture.

The deployed console SHA-256 was
`3c8027dd60e2d6c65ca2b4a07375daa62440008ed776299c66e62d409d861bc9`.
The dated machine-readable record is
[console-live-model-20260912.json](evidence/console-live-model-20260912.json).
It records actual UI text and source readings, with hashes and workstation paths
for the private window captures. Camera images are not committed.

The final check observed 721 real cells, live matrix substeps, actual DeepSeek
responses, increasing host L3–L6 sequences, and a connected **zero-only** bridge.
No nonzero motion, learned driving policy, optimizer update to the pretrained
model, or DeepSeek weight update is established by this checkpoint. The driving
demonstrations and broader console parity retain their separate acceptance work.


The Windows read-only observer supervisor (`scripts/console_observers.py`)
adopts the existing coach, visual frontend and status mirror, and replaces them
when their bounded runs finish while the console remains open. It starts no
physical transport and has no operator credential. DeepSeek credentials are
loaded from the existing credential tool directly into a replacement coach's
environment. Closing the console for 30 seconds stops these host observers.
A per-runtime lock prevents duplicate supervisors.

Live lifecycle integration at 21:34 EDT: supervisor 44336 adopted the three
existing observers. After the owned read-only mirror 66072 was stopped, it
started replacement 44688 and retained the robot's original publication times.
The coach and visual frontend were not restarted. The supervisor reports process
liveness separately from the application endpoints' actual data freshness.

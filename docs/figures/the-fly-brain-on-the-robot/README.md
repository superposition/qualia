# The fly brain on the robot

**The claim.** The released FlyEM male-CNS connectome — the fly's whole wiring diagram: 166,700
neurons, 25,582,938 neuron-level edges and 124,177,617 synapses — now steps as a spiking network on the
robot's own computer, from the public v1.0 tables under CC-BY 4.0 (#236;
`docs/evidence/T55/connectome-runner/README.md`). One **tick** is one step over those edges; the step
costs wall clock and the network keeps no model time of its own — the loop paces it: **11.840 ms on the
Orin NX iGPU of Pinkie**, the Jetson that carries the robot's software (D-010) = **84.5 ticks/s**, 1.41×
inside a 60 Hz frame, and **1.381 ms/tick on the development RTX 4090**; the operator's console draws the
same network from the committed artifact and the recorded firing (#237). The robot's own sensors reached
the dataset and the trainer: a 900 s capture through the **leash** (the board-side process that owns the
hardware), one **MCAP** log, gave 4,501 candidate transitions with **0 valid**, every one
`calibration_missing` (#239); the producers added in #250 took a 300 s session to
**`valid=1499 candidates=1500`**, and the trainer refused at its named floor — `dataset does not meet the
50k/12-session/3-condition/3-environment gate` (D-027). A DeepSeek call through the mission broker
produced a decision the agent accepted with **HTTP 202** (#244), and the review of that surface found
four characters of the live key; #251 removed prefix mode. The four Python gates are now one Rust binary,
its six output streams content-identical after folding line endings (#238), and the hardware audit found
why the drive path stops: the leash owns both serial ports (#240).

**Why a fly brain on a robot.** The project's bet is *structure as the controller*: the console's Brain
view draws the connectome the belief layers couple, and the prior the tree carried was deliberately
small — five types and nine edges in `assets/brain/prior/graph.bin`. The question this epic answers is
whether the release's own wiring, run as the network rather than a hand-authored stand-in, can drive a
controller we already own. The release carries the ventral nerve cord's motor neurons — `vnc_motor`,
alongside `descending_neuron`, is what the loop's decoder reads out — so a sensory frame can reach a
motor decision instead of stopping at the brain. Running it here closes the tree's gap at the only size
that is real: neuron for neuron, the release's own signs, nothing trained. It has to run on Pinkie
because the body carries its computer (D-010), and at frame rate because a controller that steers has to
decide inside a frame. What the wave demonstrated is that the released wiring drives a real controller
path — the free-running step holds one tick inside a 60 Hz frame (11.840 ms, 1.41× headroom on the
Orin), the host loop ran 200 ticks off the robot's real camera at 112.7 ticks/s, the board loop 31–32
ticks/s — and what it did not demonstrate is behaviour: the encoder and read-out are fixed constants, an
undriven network is silent, and the loop's commands never reached a motor (§What this does not
establish).

The wave worked ticket by ticket — T55, T56, T58, T59, T63 and T65 are its tickets, each with a
directory under `docs/evidence/`, and each step left a **braid** comment recording where it stood on the
ticket's issue. Every change was reviewed before merge — a review leaves numbered notes on the PR, which
is what `review note N5` below refers to — and the shared board is taken under a **lease**: an exclusive
window posted in the tracker. The references below (`#236, braid 2026-09-12T05:20Z`; `review note N5`)
point into that record.

## What we tried

An epic that lands in one stretch accumulates attempts that did not work. These are the ones worth
keeping, because each was caught by running the real thing.

**The first count of the release was 24× too small.** The first pass over the four flat tables built a
neuron bitmask with `bits[idx] |= value`; numpy applies that once per unique byte index, so most bits
were never set and the join came out with 1.07 M edges / 4.76 M synapses instead of the released
network's 25.58 M / 124.18 M. The Rust importer disagreed with it, which is how the error surfaced; a
corrected independent pass then agreed with the importer exactly (#236, braid 2026-09-12T05:20Z). The
import path stayed in Rust.

**The spike file's reader was wrong for a while.** A `spikes.bin` frame is
`tick u64 | t_ns u64 | count u32 | ids u32[]` behind an **8-byte** `QLSP` header (magic, `u16` version,
`u16` reserved), but the reader skipped the 12-byte base header the weights and positions files use. It
desynced on frame 1 and refused every stream; the writer had always been right, so only reading back
was broken. The failure is preserved in `docs/evidence/T55/connectome-runner/board-bench.log`
(`frame 83464099482600 ids are not strictly ascending`), fixed in `71c9900`, and the 300-tick board
loop had to be re-run under a lease to commit a transcript that reads back (#236).

**A CPU reference that read its own output.** `step_cpu` read `state.spike` at the gather and wrote it
in the same pass, so a neuron whose presynaptic partner had the lower index could fire one tick early.
The demonstration compiles the old and new files side by side: two neurons, one edge, tick 0 driven —
single-buffer fired `[0, 1]`, double-buffered `[0]`; free-running tick 1, single-buffer `[]`,
double-buffered `[1]`. Fixed by writing a second buffer and swapping at the end, the same pattern the
CUDA path uses; the device numbers were never affected (#236). The lesson is in the same log: a
free-running bench could not have caught it, because with no external current no neuron ever fires, so
`spikes on the last tick` is 0 and the two paths agree trivially — the demonstration has to drive the
network.

**A claim with no measurement behind it.** The first board loop was reported as "camera-bound" at
"~33 snapshots/s". Nothing had measured that. When the loop was re-run under a lease the endpoint was
probed directly: **200 fetches in 2,485 ms = 80.5 snapshots/s** on the board while the loop consumed
32.1 ticks/s, and the host loop against the same endpoint ran 112.7 ticks/s. The claim was withdrawn;
the entry quotes the observed loop rate with its conditions (#236; review note N5 on PR #246).

**A frame that did not exist.** In the console's spike reader the frame-header read checked its `bool`,
the ids read dropped it, so at EOF the buffer stayed zeroed. For `count == 1`, `[0]` is strictly
ascending and passed validation — a silent frame firing node 0. A scratch probe showed the failing arm
(`Some(SpikeFrame { tick: 7, t_ns: 0, ids: [0] })`) and the fixed one
(`truncated tick 7 ids: 0 of 4 bytes`); the guard now names the byte counts (#237).

**The highlight was one corner of the brain.** The firing ids arrive sorted by node index, so
`take(4096)` highlighted the lowest indices — a corner. It now strides, like the cloud decimation, and
the panel reports what it drew: 9,976 of 139,662 placed points, firing drawn 3,325–3,730 (#237).

**A guard that accepted torn data on any architecture.** `coherent_belief` read a belief layer with a
before/after pair over a parity bit and no fence. Two holes: no ordering fence (visible on aarch64,
hidden by x86_64's TSO), and — architecture-independent — two publishes inside one copy return the
index to where it started, so `before == after` held and a torn copy was accepted. In the reverted
pre-fix shape the board accepted 1,906–1,930 mixed copies out of 2,000; the fixed shape passed 20/20
repeats. The fix is a publication counter and an acquire-fenced re-read (#235 / PR #242).

**A gate that failed its own digest.** `qualia-jepa-train` refused the manifest `qualia-jepa-dataset`
had just written (`dataset manifest is unsupported or has an invalid digest`). It was not the writer:
`serde_json`'s default best-effort float parser read the audit's `mean_sensor_skew_ns` literal
`29192462.559039358` one ulp low, and `manifest_digest` re-serializes the parsed struct, so the
reader's recomputation was a different digest. `write_immutable_manifest` cannot catch that — it
verifies before any parse — and #239's all-zero audit carried no 17-digit literal, which is why it
passed. Fixed by enabling `serde_json/float_roundtrip` in `crates/jepa-dataset` and `crates/jepa-model`
(D-027, PR #257).

**Taking a side in a binary conflict.** In one hour two PRs conflicted on the console's committed
snapshot PNGs. Siding would have silently reverted the other ticket's render while the suite stayed
green, because the images *are* the expectation — the branch binaries differed from `main`'s by 132,009
pixels (`brain_fresh`) and 109,475 (`default_arrangement`) among others. D-026 makes the rule:
re-render on the rebased tree, headlessly, and say in pixels and in cause which images moved.

**The board is leased in comments, not in messages.** #240's first board window posted a lease after
checking only the process list, not the tracker, and started a release build at ~02:17 board-local
while #235 held an exclusive lease taken at 02:11. Main ordered the stop and the build was killed at
02:19:19 — nothing was produced, so the runner-level board rows had to wait for a properly posted
window (02:56–03:02). On the same ticket a host build ran as a second cargo job beside a live test.
#235's repeats taken while #240's build was running were discarded and the logs re-cut on a quiet
board, because a passing set taken under load is exactly what this session kept correcting (D-024).

**Four characters of a live key, in public.** The mission broker's status surface printed
`key=1448…(redacted, 35 chars)` — four characters of the real credential plus its length. #251 deleted
the prefix mode entirely and names a credential by presence and provenance only; against a stub
provider that echoes the credential back, the leak appears five times in the status JSON before the
fix and zero after (#244, #251). The tree carries no key-shaped literal (`git grep` for the marker
finds nothing).

**Three record corrections.** `/dev/video0` is **not** EBUSY: a second opener returns `fd=3`, and only
the two serial ports are exclusive (errno 16). `/dev/ttyACM0` is the `1a86:55d3` CDC device at USB
`1-2.1.2`, not a CH340 on `ttyUSB` — and there is no `/dev/ttyUSB*` at all, so `runners/lidar`'s
original default could never have opened here. And T60's inference that the loopback agent "admits the
route before any bearer check" was wrong: an undeserializable body answers 422 regardless of the
bearer because axum's `Json` extractor runs before `authorize`; well-formed envelopes measured
401/401/202 (#240, #241, #244).

**Two board-build findings.** `--offline` fails on Pinkie because `lz4_flex 0.11.6` is not in the
registry cache (the host CONNECT proxy is the route), and `nvcc` refuses a `__global__` function that
calls another `__global__` without dynamic parallelism — which NVRTC on the host had let pass, so the
LIF (leaky integrate-and-fire) kernel became a single kernel (#236). A board build of a *different*
package subset re-unifies features and recompiles the shared closure, which is why the T58 legs cost
~20 min rather than ~4 (#239).

## What we changed

* **`crates/connectome-cns`** (`qualia-connectome-cns`): an importer that writes a digest-checked
  artifact (`QLCN` weights, `QLCB` positions, types, nodes, per-section SHA-256 in a manifest), a CUDA
  kernel `kernels/cns_lif.cu` with a CPU oracle, and `bench` / `loop` / `replay` commands. The network
  artifact is committed at `assets/brain/connectome-cns/` with its attribution; `weights.bin`
  (180,414,198 B) stays off-tree by digest (#236).
* **`crates/connectome-stream`, `crates/connectome-view`, `apps/qualia-console/src/views/brain/`**: the
  console's Brain view draws the committed artifact, coloured by cell type, with the tick's firing set
  read off `QUALIA_CONNECTOME_SPIKES` (a recorded file on its own clock, or `tcp://`) on its own
  thread; the Rerun projection is the secondary path (#237).
* **`crates/gates`** (`qualia-gates`): the four gates — `provenance`, `figures`, `journal`, `mission` —
  as one Rust binary, with the Python scripts deleted and no shim; the parity evidence is six output
  streams content-identical to the scripts after folding line endings, plus matching exit codes (#238).
* **`runners/leash-sensors` and `runners/camera`'s MJPEG path**, and the default stack manifest: the
  deployed stack previously started neither, so nothing fed the arena from the robot. `runners/vision`
  now takes its frame from the arena camera preview instead of a file nothing writes, and the agent's
  inert `OrinConfig` / `QUALIA_ORIN_CAMERA_DEVICE` / `QUALIA_ORIN_SNAPSHOT_INTERVAL_MS` are gone (#240).
* **The HUD**: a fixed-width `RunnerStats` frame per runner in a stats region beside the arena, the
  console's floating panels, and T64's four honesty fixes — 12-byte labels fixed by renaming the
  publishers rather than widening the ABI, the sparkline sentence corrected to the real 30 s window,
  `started_at_ns` filled for `uptime`, and `PUBLISHING` cleared on drop so a stopped runner reads
  `stopped` (#241, #247).
* **`runners/calibration` and `runners/leash-transport`**: the calibration document whose
  `calibration_id` is a sha256 over measured identity fields (intrinsics and extrinsics recorded
  `unmeasured` with the reason, never invented), and a transport ledger that reaches the leash only
  over HTTP and records the leash's own acknowledgement of its zero-speed `stop` with
  `authority=leash` (#250).
* **`runners/mission-broker`** and the console's Coach panel: the first *mission/decision* model call
  in the tree — `runners/vision` already posts a camera frame to Gemini (`VISION_MODEL =
  "gemini-2.5-flash"`, `API_ROOT = generativelanguage.googleapis.com/...`; `runners/vision/src/main.rs:46,50`,
  a path T59 measured live) — with a `redact` module that names a credential by presence only
  (#244, #251).
* **`crates/shm`**: `LayerSlot`'s publish is a counter with a fenced reader, and `coherent_belief` is
  deleted (#235).
* **`docs/decisions.md` D-023–D-027**: the console is the operator's instrument and there is one agent
  instance per host; the board is leased; the leash owns the hardware and the stack subscribes;
  snapshot images are re-rendered, never sided; the promotion floor stands and what closes it is
  corpus.

## Evidence

| File | Kind | What it encodes |
| --- | --- | --- |
| `step-rates.svg` (+ `.png`) | chart | The same 25,582,938-edge step per device, on a log ticks/s axis: the Orin NX iGPU at 84.5 ticks/s (11.840 ms/tick, 182 MB resident), the RTX 4090 at 723.9, one host CPU thread at 48.5, the Orin CPU reference at 20.4, against the 60 Hz frame at 16.7 ms. |
| `dataset-gate.svg` (+ `.png`) | chart | Two real sessions through the dataset's transition gates: `t58-real-01` (900 s, 83,299,459 B) with 4,501 candidates and 0 valid at `calibration_missing`, and `t65-real-01` (300 s, 30,300,801 B) with 1,500 candidates and 1,499 valid; the promotion floor (50,000 valid, 12 sessions, 3 environments, 3 conditions — 33.4× the accepted session) is stated above both. |
| `brain-to-robot.svg` (+ `.png`) | diagram | The path the epic built: the four public release tables (1.89 GB in total; the weights table alone is 1.05 GB) into the importer's QLCN artifact (166,700 neurons / 25,582,938 neuron-level edges / 124,177,617 synapses), the LIF runner at 11.840 ms/tick, the console Brain view, the robot's sensors through the leash's HTTP surface into the encoder (8 columns, gain 0.5), the decoder (2,012 motor neurons) and the blocked drive path, with the leash's ownership boundary dashed, and below it MCAP → dataset gates → the 50k/12/3/3 floor. |
| `wave-numbers.json` | data | Every number the two charts draw, each with the committed evidence file it came from; a chart value the file does not carry is a refusal, not a guess. The diagram's labels are typed into `make_figures.py` and each traces to a source named in this table or in the figure captions. |

![The connectome's step per device](https://raw.githubusercontent.com/superposition/qualia/main/docs/figures/the-fly-brain-on-the-robot/step-rates.svg)

*`step-rates.svg`: one artifact, four meters. The bars are ticks/s (log scale) from
`docs/evidence/T55/connectome-runner/README.md` §Rates; the dashed line is a 60 Hz frame at 16.7 ms,
which the Orin sits 1.41× inside. The 4090's 1.381 ms/tick is 18,519.0 M synapses/s; the Orin's is
2,160.6 M.*

![Two real sessions through the dataset's gates](https://raw.githubusercontent.com/superposition/qualia/main/docs/figures/the-fly-brain-on-the-robot/dataset-gate.svg)

*`dataset-gate.svg`: what the two captures produced, from the dataset binary's own audit lines in
`docs/evidence/T58/real-sessions/README.md` and `docs/evidence/T65/calibration/README.md`. Slate is
candidate transitions, green is admitted valid; where nothing was admitted the bar has no height and
the series is read from its `0` label. The floor above them is `docs/decisions.md` D-027 and is off the
chart: 50,000 valid transitions is 33.4× the whole accepted session, before the 12-session /
3-environment / 3-condition diversity.*

![From the release tables to the leash's boundary](https://raw.githubusercontent.com/superposition/qualia/main/docs/figures/the-fly-brain-on-the-robot/brain-to-robot.svg)

*`brain-to-robot.svg`: the epic end to end. The four release tables are 1.89 GB together — the weights
table alone is 1.05 GB — from `docs/evidence/T55/connectome-runner/README.md` §What was run. The dashed
line is the leash's ownership boundary as measured in `docs/evidence/T59/devices/README.md` — both
serial ports are single-owner (errno 16), so `qualia-lidar` and `qualia-drive` measure a refusal and
exit 2; the camera node takes several readers, and the stack still subscribes to the leash's HTTP
surface rather than opening it. The decoder's `command`/`throttle` stopped at the loop's trace: no
command reached a wheel (§What this does not establish).*

### Data

`wave-numbers.json` carries the charts' numbers with the source file each was read from; a chart value
the file does not carry is a refusal rather than a guess, and the diagram's labels are typed into
`make_figures.py` beside the box they appear in. The quantities, with units and sources:

| Quantity | Value | Source |
| --- | --- | --- |
| Neurons / neuron-level edges / synapses | 166,700 / 25,582,938 / 124,177,617 | `docs/evidence/T55/connectome-runner/README.md`; `docs/evidence/T56/floating-brain/README.md` |
| Excitatory / inhibitory / unknown-sign edges | 94,542,746 / 26,402,637 / 3,232,234 | same |
| Segment-level source (for contrast) | 88,384,522 segments; 151,856,684 rows; Σweight 311,833,243 | `docs/evidence/T55/connectome-runner/README.md` |
| Artifact | `weights.bin` 180,414,198 B; `positions.bin` 3,334,012 B, sha256 `016a285b…afa6e`; placed 139,662 of 211,577 bodies; 11,752 type labels | same |
| Orin NX iGPU (sm_87 cubin) | 11.840 ms/tick, 84.5 ticks/s, 2,160.6 M synapses/s, 182,247,874 B resident | same |
| RTX 4090 (NVRTC, sm_89) | 1.381 ms/tick, 723.9 ticks/s, 18,519.0 M synapses/s | same |
| 60 Hz headroom | 16.7 ms frame vs 11.840 ms/tick = 1.41× (bench, not the loop) | same |
| Closed loop | host, 200 ticks in 1.775 s = 112.7 ticks/s, 10,048,960 spikes, 196 non-hold | same |
| Closed loop | Orin, 300 ticks in 9.358 s = 32.1 ticks/s, 15,085,686 spikes, 294 non-hold, 31.2 ms/tick | same |
| Leash snapshot probe on the board | 200 fetches in 2,485 ms = 80.5 snapshots/s | same |
| Leash surface | camera snapshot (`/camera/snapshot`) and MJPEG, LD06 scans at 9.999 Hz, 360 beams per scan (`waveshare-ugv-ld06`, 269 valid returns) | `docs/evidence/T58/real-sessions/leash-sensors.log`; `docs/decisions.md` D-025 |
| Encoder and decoder (chosen constants) | 8 luminance columns, gain 0.5, 95,494 visual-stage cells (`ol_intrinsic` + `R1-R6`/`R7*`/`R8*`); 2,012 descending/motor neurons, 1,011 L + 1,001 R | `docs/evidence/T55/connectome-runner/README.md` §The closed loop; `crates/connectome-cns/README.md` |
| Firing stream | 40,199,848 B, sha256 `a0a27acb…3b28`, 200 frames, 10,048,960 ids; first non-empty tick 3 fires 19,223 | `docs/evidence/T56/floating-brain/README.md` |
| Console Brain panel drew | 9,976 of 139,662 points; 3,728 of the captured tick's 36,480 firing nodes | same |
| Session 1 | 900 s; MCAP 83,299,459 B, sha256 `97612acb…3334`; camera 4,502, lidar 8,775 records; 4,501 candidates, 0 valid, all `calibration_missing` | `docs/evidence/T58/real-sessions/README.md` |
| Session 2 | 300 s; MCAP 30,300,801 B, sha256 `03ae6b0f…7110`; camera 1,501, lidar 2,917, pose 2,917, applied-action 2,997 records; `valid=1499 candidates=1500`; mean action coverage 1.0; mean sensor skew 29.19 ms; one candidate rejected at `action_coverage` | `docs/evidence/T65/calibration/README.md` |
| Calibration identity | `calibration_id = cac618af21db53843124b949d06e78c41c7b55d91a6cfdf2862813a61760f6e2`; camera intrinsics, camera↔lidar extrinsics, lidar mount transform `unmeasured` | same |
| Promotion floor | 50,000 valid transitions, 12 sessions, 3 environments, 3 conditions; shortfall 33.4× | `docs/decisions.md` D-027 |
| HUD live panels | `region /qualia_body, 3 live runner panel(s): qualia-camera 2.50 Hz, qualia-health 9.97 Hz, qualia-vision 4.99 Hz` | `docs/evidence/T60/live-hud/README.md` (the #241 close comment's run read 2.54 / 9.97 / 4.98) |
| Mission broker | `POST /mission-control/envelopes -> HTTP 202 accepted=true idempotent_replay=false`; requested `deepseek-chat`, provider reported `deepseek-flash`; latency 418 ms wall clock | `docs/evidence/T63/live-run.txt:4`; `docs/evidence/T63/README.md:36` |
| Gate parity | six output streams content-identical to the frozen Python after folding line endings (host Python writes CRLF; 38 B on provenance); `provenance: OK` | #238 braid; PR #253 |
| Leash ownership | `/dev/ttyACM0` and `/dev/ttyTHS1` errno 16; `qualia-lidar`/`qualia-drive` exit 2; `/dev/video0` second opener `fd=3` | `docs/evidence/T59/devices/README.md` |

**Sources disagree in three places, and all are quoted rather than resolved.** #236's first braid gives
the host CPU bench as 21.144 ms/tick, 47.3 ticks/s, 1,209.9 M synapses/s, while
`docs/evidence/T55/connectome-runner/README.md` gives 20.615 ms/tick, 48.5 ticks/s, 1,241.0 M — the
same bench, a later pass; the figures use the evidence file's numbers. That README's log list says the
import took "24.7 s" while its own transcript prints `-> artifact in 26.0445714s`; no figure draws
either. And `docs/evidence/T63/README.md`'s own note (`:117-119`) says the `--once` broker run was
`449 ms, 782/139 tokens, response 9f8bbc50-…`, while its committed transcript (`README.md:36`, copied
from `live-run.txt:4`) prints `latency_ms=418 usage=prompt_tokens=786 completion_tokens=143` for
response `70bb2dcb-…`; the entry quotes the transcript's 418 ms and no figure draws either run.

### Regenerating

```console
$ py -3.13 docs/figures/the-fly-brain-on-the-robot/make_figures.py
wrote step-rates, dataset-gate, brain-to-robot; ticks/s [20.4, 48.5, 84.5, 723.9]; sessions [('t58-real-01', 4501, 0), ('t65-real-01', 1500, 1499)]; 60 Hz frame 16.7 ms; floor 50000
```

No Blender step: this entry's subject is a system — a chart, a second chart and a schematic — not a
numeric field with a shape to turn, so the render band of `docs/figures/README.md` rule 2 is not added,
and the block above is the no-WebGL floor. The palette is the house one, defined once in
[`../_house.py`](../_house.py); two consecutive runs are byte-identical (`cmp` over each `.svg` and
`.png` under matplotlib 3.10.1), and every asset is under 400 KiB.

The gates, run in this tree:

```console
$ cargo run --quiet -p qualia-gates -- figures
figures: OK (11 entries, 55 figures, 400 KiB budget)
$ cargo run --quiet -p qualia-gates -- provenance
provenance: compared 152 authored file(s) (1 generated skipped); 0 identical, 27 EOL-identical, 7 code runs over 20, 0 prose runs over 2
provenance: highest identical-code-line share 1.000 (.cargo/config.toml)
provenance: OK
```

## What this does not establish

* **Nothing here learns.** The runner is a sparse leaky integrate-and-fire network — membrane
  potential and a refractory period, nothing else: no plasticity, no neuromodulation, no training. The
  monoamines and unknown-sign transmitters get sign 0 rather than an invented polarity, and a
  free-running network is silent because there is no noise term; the drive is what makes it spike.
* **The encoder and decoder are chosen constants, not a fit.** Eight luminance columns, gain 0.5,
  95,494 visual-stage cells feeding the network and 2,012 descending/motor neurons read out are fixed
  choices. The camera was pointed at a nearly uniform scene (luminance 0.4613–0.4679 over the run), so
  the frame contributed little and the network's own recurrent dynamics dominate. The loop is
  plumbing, not a behaviour claim.
* **60 Hz bounds the bench, not the loop.** 16.7 ms is a 60 Hz frame and the free-running step is
  11.840 ms; the closed loop as run was 31–32 ticks/s on the board and 112.7 ticks/s on the host, each
  with its own conditions. No 60 Hz claim is made for the loop.
* **The promotion floor is unmet, and it is corpus, not code.** 1,499 valid transitions in 1 session,
  1 environment and 1 condition against 50,000 / 12 / 3 / 3; the shortfall is 33.4× plus the diversity
  no threshold change can supply, and on this robot the drive belongs to the leash (D-027). The
  transport ledger's own record is a zero-speed stop, not motion.
* **The console brain figure is one host's wgpu render.** The committed panel is rendered headlessly by
  the console's snapshot surface on this workstation; the board's renderer is a separate leg and was
  not run. The cloud draws the 139,662 somata the release places of 211,577 bodies (the other 27,038
  CSR nodes are left out rather than placed at a made-up coordinate), decimated to 10,000 points, and
  the firing overlay is strided to 4,096 — so the figure shows 9,976 points and 3,325–3,730 firing
  nodes, not the whole network. The captured tick is a wall-clock property of the playback: the
  pre-rebase capture landed on tick 5 (44,431 firing), the re-capture on tick 6 (36,480), from the same
  byte-identical file.
* **The live `tcp://` spike path is unexercised.** The recorded file is the path the figures and the
  console exercise; the socket framing carries the same bytes but has only been checked over a local
  socket, never across the USB link to the board.
* **The firing stream the console draws is not in the tree.** `spikes.bin` — 40,199,848 B, sha256
  `a0a27acb…3b28` — is a recording, deliberately not committed (T55 §What was run); it lived on the board's
  run directory and on host scratch, and the committed console figure and the firing figures here were
  produced by pointing `QUALIA_CONNECTOME_SPIKES` at it (T56 §The stream this figure shows).
* **The lidar and drive success paths are unexercised on the robot.** The board rows measure the
  *refusal* — `Device or resource busy`, exit 2 — which is the documented contract for a port the
  leash owns; the scan-assembly and motor-write paths behind a successful open cannot be exercised
  without taking a port from the leash, which D-025 rules out. The connectome loop's own outputs fared
  the same: its `command` and `throttle` (196 non-hold commands in the host run, 294 on the board) went
  to its trace, `host-loop-gpu-trace.csv`, and nothing transported them to a motor, because the leash
  holds `/dev/ttyTHS1` — no wheel in this epic was turned (T55 §The closed loop).
* **No mission reached motion.** The broker's first decision was made over a committed
  `world.model.v1` envelope because the agent's proposal route is a `503` stub; the delivered mission
  parked at `awaiting_fresh_evidence` and closed at its own deadline. The provider returned
  `deepseek-flash` for a `deepseek-chat` request and the broker records what the provider said. The
  numeric envelope is the broker's, not the model's.
* **The hardware audit leaves gaps it names.** No IMU is on the robot's own I2C/IIO buses (the leash's
  `sensors.imu` exists, but the arena has no inertial slot and no runner consumes it); the leash's
  localizer never came up, so pose is our own lidar ICP on a stationary scene; `qualia-vslam`,
  `qualia-cli`, `qualia-jepa-runtime` and l0–l6 were not built or run in that pass, and `qualia-watch`
  and `qualia-console` were not launched windowed (D-023).
* **The charts round and encode single sessions.** Bar labels are to four significant figures and the
  floor is drawn as an annotation, not a bar, because it is 33.4× off the scale; the diagram's numbers
  are the two sessions' own, not a distribution.
* **The published page is a site step.** This README and its figures are the source; the site's front
  matter and live URL are produced from them when the entry moves to the site's `_posts/`.

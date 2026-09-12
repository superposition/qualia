# The fly brain on the robot

**The claim.** A connectome is a wiring diagram of a brain, and this one is a measurement, not a
design: 166,700 neurons and 124,177,617 synapses of a male fly's central nervous system, segmented
from an electron-microscopy volume and released as `male-cns:v1.0` (CC-BY 4.0, Berg et al., *Cell*,
2026) (#236). Our bet is that such a structure can itself be the controller, and I wanted to know whether
it could: until now our controller ran on a five-type, nine-edge stand-in, while this release is the
whole thing at full scale. We ran it as a spiking network on the robot's own computer: 84.5 ticks/s
with no input on its Jetson, 31–32 ticks/s driven by the robot's camera, and 112.7 ticks/s on the host
loop. Running the brain was the easy part; knowing whether what ran is what we thought ran took the
rest of the time, and gave us this entry's rule — a claim is only as good as the thing that can
contradict it. The robot's own camera and lidar reached it through the leash, and took a dataset from
0 usable transitions out of 4,501 to 1,499 of 1,500 (#239, #250).

A **neuron** is a cell that carries a signal, and a **synapse** is the junction where one neuron
passes it to the next. This release carries the ventral nerve cord's motor neurons, so a sensory frame
can reach a motor decision instead of stopping at the brain. The network runs as a spiking model: each
neuron holds a **membrane potential** (the voltage accumulated from its inputs), fires a **spike** (a
brief, all-or-nothing pulse) when it crosses a threshold, then stays quiet for a **refractory** period.
A **tick** is one step of that simulation, in which every neuron reads its inputs, updates and decides
whether to fire; it costs wall-clock time, because the network keeps no clock of its own and the loop
around it sets the pace. The **leash** is the board-side process that owns the robot's hardware, and
**MCAP** is the container format our recorder writes.

*(The parenthesised codes throughout are this project's own pointers: `#N` and `PR #N` are GitHub
issues and pull requests, `Txx` are tickets, and `D-0xx` are records in `docs/decisions.md`.)*

## What we tried

Almost nothing here failed by crashing. It failed by working and reporting something that was
not true, and what caught it was always a second opinion: another implementation of the
same measurement, a file read back, a number probed directly instead of assumed, a rule written on the
ticket where the other agent could see it. These are the ones worth keeping, because each cost us a
day and each ended in something a reader can use.

**Our first count of the brain was 24× too small.** The first pass over the release's four flat tables
built a neuron bitmask with `bits[idx] |= value`. That reads like “set one bit per neuron”, but numpy
applies it once per *unique byte index*, so most bits were never set and the join came out with 1.07 M
edges and 4.76 M synapses instead of the release's 25,582,938 and 124,177,617. A Rust importer, written
independently, disagreed with it — and that disagreement is the only reason we found the bug. We
rewrote the numpy version as a check and it then matched the importer exactly; the import path itself
stayed in Rust (#236). *The rule we took: a load-bearing measurement needs a second implementation,
written independently; when the two disagree, the disagreement is the finding, not a rounding error.*

**The reader skipped four bytes too many.** A `spikes.bin` frame is `tick u64 | t_ns u64 | count u32 |
ids u32[]` behind an **8-byte** `QLSP` header (magic, `u16` version, `u16` reserved), but our reader
skipped the 12-byte base header the weights and positions files use. It desynced on the first frame and
refused every stream; the writer had always been right, so only the reading was broken. The bench log
shows exactly how it looked — `frame 83464099482600 ids are not strictly ascending` — and after the fix
the 300-tick board loop had to be re-run under a lease to commit a transcript that reads back (#236).
*The rule we took: header layouts differ per file, so each format is parsed from its own description,
and what was written is read back — a correct writer is not evidence of a correct reader.*

**A CPU check that read its own output.** Our CPU version of the step, `step_cpu`, read `state.spike`
at the gather and wrote it back in the same pass, so a neuron whose presynaptic partner had a lower
index could fire one tick early. We caught it by driving the smallest network we could: two neurons,
one edge, tick 0 driven. The single-buffer version fired `[0, 1]`; the double-buffered version fired
`[0]`. Free-running at tick 1, the single-buffer version fired `[]` and the double-buffer `[1]`. We
fixed it by writing a second buffer and swapping at the end — the same pattern the CUDA path already
used, so the device numbers were never affected. The same log shows why the obvious bench would have
missed it: with no external current no neuron ever fires, so `spikes on the last tick` is 0 and the two
paths agree trivially (#236). *The rule we took: a bench that fires nothing cannot see an ordering
bug — a correctness check has to drive the system.*

**A guard that accepted torn data on every architecture.** Our belief-coupling guard `coherent_belief`
read a layer with a before/after pair over a parity bit and no memory fence. It had two holes. There
was no ordering fence, which aarch64 exposes and x86_64's stronger memory model hides; and, on any
architecture, two publishes inside one copy can return the parity to where it started, so `before ==
after` held and a torn copy passed as coherent. We measured both: in the reverted pre-fix shape the
board accepted 1,906–1,930 mixed copies out of 2,000, and the fixed shape — a publication counter and
an acquire-fenced re-read — passed 20/20 repeats (#235, PR #242). *The rule we took: parity is not a
version — a value that can return to its starting state cannot show that nothing changed, so count
publications.*

**A file that failed its own digest.** The trainer refused a manifest the dataset writer had just
written: `dataset manifest is unsupported or has an invalid digest`. Our first instinct was the writer.
It was the parser: `serde_json`'s default best-effort float reader read the audit's `mean_sensor_skew_ns`
literal `29192462.559039358` one ulp low, and the digest is taken over the re-serialized struct, so the
reader recomputed a different digest. The writer could not have caught it — it verifies before any
parse — and the earlier all-zero audit carried no 17-digit literal, which is why it had passed. The fix
was enabling `serde_json/float_roundtrip` (D-027, PR #257). *The rule we took: a digest over
re-serialized data tests the round trip, not the bytes; when a file fails its own digest, the parse is
a better suspect than the writer.*

**Four characters of a live key, in public.** The mission broker's status surface printed `key=1448…
(redacted, 35 chars)` — four characters of a real credential plus its length, in a public repository.
Against a stub provider that echoes the credential back, the leak appeared five times in the status
JSON before the fix and zero times after; we deleted the prefix mode entirely and now name a credential
by presence and provenance only (#244, #251). *The rule we took: “redacted” is a claim to verify, not
a format — never emit any part of a secret, and grep the tree for key-shaped literals.*

**The board is leased in writing, not in messages.** The first board window on one ticket posted its
lease after checking only the process list, not the tracker, and started a release build at ~02:17
board-local while another agent held an exclusive lease taken at 02:11. We were told to stop, and the
build was killed at 02:19:19; nothing was produced, so the board rows waited for a properly posted
window (02:56–03:02). On the same ticket a host build ran as a second cargo job beside a live test, and
the repeats another agent had taken under that load were discarded and re-cut on a quiet board — because
a passing set taken under load is exactly the kind of number we kept having to correct (D-024).
*The rule we took: a shared machine needs a written queue — check the tracker, not the process list —
and a measurement taken under someone else's load is not a measurement.*

**Three times the console showed us something that was not there.** The first board loop was reported
as “camera-bound” at “~33 snapshots/s”, and nothing had measured either claim; when we probed the
endpoint directly it answered 200 fetches in 2,485 ms — 80.5 snapshots/s — while the loop consumed
32.1 ticks/s, and the host loop against the same endpoint ran 112.7 ticks/s, so the claim was withdrawn
(#236; the review of PR #246). In the console's spike reader, the frame-header read checked its `bool`
and the ids read dropped it, so at end of file the buffer stayed zeroed; for a frame with `count == 1`,
`[0]` is strictly ascending and passed validation — a silent frame that fired node 0. A scratch probe
showed the failing arm (`Some(SpikeFrame { tick: 7, t_ns: 0, ids: [0] })`) and the fixed one
(`truncated tick 7 ids: 0 of 4 bytes`), and the guard now names the byte counts it read (#237). And the
“highlight” in the Brain panel was taking the first 4,096 of the firing ids, which arrive sorted by
node index, so it lit the lowest-numbered corner of the brain; it now strides, like the cloud
decimation, and the panel reports what it actually drew — 9,976 of 139,662 placed points, firing
3,325–3,730 (#237). *The rule we took: every rate and every picture has to say what it measured — a
prefix of sorted data is the lowest IDs, not a sample, and a bottleneck that was not probed is a guess.*

**Two PRs conflicted on the same PNGs.** In one hour, two branches hit a binary conflict on the
console's committed snapshot images. Taking either side would have silently reverted the other ticket's
render while the test suite stayed green, because the images *are* the expectation — the two branches
differed from `main` by 132,009 pixels (`brain_fresh`) and 109,475 (`default_arrangement`) among others.
So D-026 makes the rule: re-render the images on the rebased tree, headlessly, and say in pixels and in
cause which images moved. *The rule we took: a generated expectation is re-generated, never merged — a
green suite means nothing when the expectation is the thing that was overwritten.*

**What the machine told us about itself, when we finally asked it.** We had three things written down
wrong: `/dev/video0` is not busy (a second opener returns `fd=3`, and only the two serial ports are
exclusive, with errno 16); `/dev/ttyACM0` is a `1a86:55d3` CDC device at USB `1-2.1.2`, not a CH340 on
`ttyUSB` — and there is no `/dev/ttyUSB*` at all, so the lidar runner's original default could never
have opened here; and an inference that the loopback agent “admits the route before any bearer check”
was wrong, because a body that will not deserialize answers 422 regardless of the bearer (the JSON
extractor runs before authorization), while well-formed envelopes measured 401/401/202 (#240, #241,
#244). The target also disagreed with the host twice: `--offline` fails on Pinkie because `lz4_flex
0.11.6` is not in the registry cache, and `nvcc` refuses a `__global__` function that calls another
`__global__` without dynamic parallelism — which the host's NVRTC had let pass, so the LIF (leaky
integrate-and-fire) kernel became a single kernel; a board build of a different package subset re-unifies features and recompiles the
shared closure, which is why those legs cost ~20 minutes instead of ~4 (#236, #239). *The rule we took: build
for the target with the target's toolchain, and treat any guess about a device as unverified until it
has been opened.*

## What we changed

* **The importer and runner** (`crates/connectome-cns`, `qualia-connectome-cns`): reads the release's
  four public tables and writes a checkable artifact — `QLCN` weights, `QLCB` positions, cell types,
  nodes, and a per-section SHA-256 in a manifest — with a CUDA kernel (`kernels/cns_lif.cu`) and a CPU
  version to check it against, plus `bench`, `loop` and `replay` commands. The graph is committed at
  `assets/brain/connectome-cns/` with its attribution; the 180,414,198-byte `weights.bin` stays out of
  the tree by digest (#236).
* **The console's Brain view** (`crates/connectome-stream`, `crates/connectome-view`,
  `apps/qualia-console/src/views/brain/`): draws the committed graph, coloured by cell type, with the
  tick's firing set read off `QUALIA_CONNECTOME_SPIKES` — a recorded file on its own clock, or `tcp://`
  — on its own thread. The Rerun projection is the secondary path (#237).
* **The gates in one Rust binary** (`crates/gates`, `qualia-gates`): `provenance`, `figures`, `journal`
  and `mission` are one command now, with the four Python scripts deleted and no shim; the parity
  evidence is six output streams content-identical to the old scripts after folding line endings, plus
  matching exit codes (#238, PR #253).
* **The sensors and the arena** (`runners/leash-sensors`, `runners/camera`'s MJPEG path, and the
  default stack manifest): the deployed stack had started neither, so nothing fed the arena from the
  robot. `runners/vision` now takes its frame from the arena camera preview instead of a file nothing
  writes, and the inert `OrinConfig` / `QUALIA_ORIN_CAMERA_DEVICE` / `QUALIA_ORIN_SNAPSHOT_INTERVAL_MS`
  are gone (#240).
* **The HUD**: a fixed-width `RunnerStats` frame per runner in a stats region beside the arena, the
  console's floating panels, and four honesty fixes — 12-byte labels fixed by renaming the publishers
  rather than widening the interface, the sparkline sentence corrected to the real 30 s window,
  `started_at_ns` filled for `uptime`, and `PUBLISHING` cleared on drop so a stopped runner reads
  `stopped` (#241, #247).
* **Calibration and transport** (`runners/calibration`, `runners/leash-transport`): a calibration
  document whose `calibration_id` is a sha256 over measured identity fields (intrinsics and extrinsics
  recorded `unmeasured` with the reason, never invented), and a ledger that reaches the leash only over
  HTTP and records the leash's own acknowledgement of its zero-speed `stop` with `authority=leash`
  (#250).
* **The mission broker** (`runners/mission-broker`) and the console's Coach panel: the first
  mission/decision model call in the tree — `runners/vision` already posts a camera frame to Gemini
  (`VISION_MODEL = "gemini-2.5-flash"`, `API_ROOT = generativelanguage.googleapis.com/...`;
  `runners/vision/src/main.rs:46,50`, a path T59 measured live) — with a `redact` module that names a
  credential by presence only (#244, #251).
* **Shared memory** (`crates/shm`): `LayerSlot`'s publish is a counter with a fenced reader now, and
  `coherent_belief` is deleted (#235).
* **Five decisions** (`docs/decisions.md` D-023–D-027): the console is the operator's instrument and
  there is one agent instance per host; the board is leased; the leash owns the hardware and the stack
  subscribes; snapshot images are re-rendered, never sided; the promotion floor stands and what closes
  it is corpus.

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
| Reference prior (the stand-in, for contrast) | 5 cell types and 9 edges, in `assets/brain/prior/graph.bin` | `assets/brain/README.md` |
| Firing stream | 40,199,848 B, sha256 `a0a27acb…3b28`, 200 frames, 10,048,960 ids; first non-empty tick 3 fires 19,223 | `docs/evidence/T56/floating-brain/README.md` |
| Console Brain panel drew | 9,976 of 139,662 points; 3,728 of the captured tick's 36,480 firing nodes | same |
| Session 1 | 900 s; MCAP 83,299,459 B, sha256 `97612acb…3334`; camera 4,502, lidar 8,775 records; 4,501 candidates, 0 valid, all `calibration_missing` | `docs/evidence/T58/real-sessions/README.md` |
| Session 2 | 300 s; MCAP 30,300,801 B, sha256 `03ae6b0f…7110`; camera 1,501, lidar 2,917, pose 2,917, applied-action 2,997 records; `valid=1499 candidates=1500`; mean action coverage 1.0; mean sensor skew 29.19 ms; one candidate rejected at `action_coverage` | `docs/evidence/T65/calibration/README.md` |
| Calibration identity | `calibration_id = cac618af21db53843124b949d06e78c41c7b55d91a6cfdf2862813a61760f6e2`; camera intrinsics, camera↔lidar extrinsics, lidar mount transform `unmeasured` | same |
| Promotion floor | 50,000 valid transitions, 12 sessions, 3 environments, 3 conditions; shortfall 33.4× | `docs/decisions.md` D-027 |
| HUD live panels | `region /qualia_body, 3 live runner panel(s): qualia-camera 2.50 Hz, qualia-health 9.97 Hz, qualia-vision 4.99 Hz` | `docs/evidence/T60/live-hud/README.md` (the #241 close comment's run read 2.54 / 9.97 / 4.98) |
| Mission broker | `POST /mission-control/envelopes -> HTTP 202 accepted=true idempotent_replay=false`; requested `deepseek-chat`, provider reported `deepseek-flash`; latency 418 ms wall clock | `docs/evidence/T63/live-run.txt:4`; `docs/evidence/T63/README.md:36` |
| Gate parity | six output streams content-identical to the frozen Python after folding line endings (host Python writes CRLF; 38 B on provenance); `provenance: OK` | #238; PR #253 |
| Leash ownership | `/dev/ttyACM0` and `/dev/ttyTHS1` errno 16; `qualia-lidar`/`qualia-drive` exit 2; `/dev/video0` second opener `fd=3` | `docs/evidence/T59/devices/README.md` |

**Three places where our own sources disagree, and we quote both rather than pick.** The first ticket
comment gives the host CPU bench as 21.144 ms/tick, 47.3 ticks/s, 1,209.9 M synapses/s, while
`docs/evidence/T55/connectome-runner/README.md` gives 20.615 ms/tick, 48.5 ticks/s, 1,241.0 M — the
same bench, a later pass; the figures use the evidence file's numbers. That README's log list says the
import took “24.7 s” while its own transcript prints `-> artifact in 26.0445714s`; no figure draws
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

This is a wiring diagram running, not a fly. The honest conclusion is that the wiring is no longer the
bottleneck — the data is.

* **Nothing here learns.** The runner is a sparse leaky integrate-and-fire network — membrane potential
  and a refractory counter, nothing else: no plasticity, no neuromodulation, no training. The monoamines
  and unknown-sign transmitters get sign 0 rather than an invented polarity, and a free-running network
  is silent because there is no noise term; the drive is what makes it spike.
* **The encoder and decoder are chosen constants, not a fit.** Eight luminance columns, gain 0.5, 95,494
  visual-stage cells feeding the network and 2,012 descending/motor neurons read out are fixed choices.
  The camera was pointed at a nearly uniform scene (luminance 0.4613–0.4679 over the run), so the frame
  contributed little and the network's own recurrent dynamics dominate. The loop is plumbing, not a
  behaviour claim.
* **60 Hz bounds the bench, not the loop.** 16.7 ms is a 60 Hz frame and the free-running step is
  11.840 ms; the closed loop as run was 31–32 ticks/s on the board and 112.7 ticks/s on the host, each
  with its own conditions. No 60 Hz claim is made for the loop.
* **The promotion floor is unmet, and it is corpus, not code.** 1,499 valid transitions in 1 session,
  1 environment and 1 condition against 50,000 / 12 / 3 / 3; the shortfall is 33.4× plus the diversity
  no threshold change can supply, and on this robot the drive belongs to the leash (D-027). The
  transport ledger's own record is a zero-speed stop, not motion. This is the next bottleneck: more
  sessions in more places and conditions, not more kernels.
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
  `a0a27acb…3b28` — is a recording, deliberately not committed (T55 §What was run); it lived on the
  board's run directory and on host scratch, and the committed console figure and the firing figures
  here were produced by pointing `QUALIA_CONNECTOME_SPIKES` at it (T56 §The stream this figure shows).
* **The lidar and drive success paths are unexercised on the robot.** The board rows measure the
  *refusal* — `Device or resource busy`, exit 2 — which is the documented contract for a port the leash
  owns; the scan-assembly and motor-write paths behind a successful open cannot be exercised without
  taking a port from the leash, which D-025 rules out. The connectome loop's own outputs fared the same:
  its `command` and `throttle` (196 non-hold commands in the host run, 294 on the board) went to its
  trace, `host-loop-gpu-trace.csv`, and nothing transported them to a motor, because the leash holds
  `/dev/ttyTHS1` — no wheel in this epic was turned (T55 §The closed loop).
* **No mission reached motion.** The broker's first decision was made over a committed `world.model.v1`
  envelope because the agent's proposal route is a `503` stub; the delivered mission parked at
  `awaiting_fresh_evidence` and closed at its own deadline. The provider returned `deepseek-flash` for a
  `deepseek-chat` request and the broker records what the provider said. The numeric envelope is the
  broker's, not the model's.
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

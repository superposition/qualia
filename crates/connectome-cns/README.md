# `qualia-connectome-cns`

The released **Male CNS** connectome (Janelia FlyEM / Google Research, `male-cns:v1.0`) imported to a
verified binary artifact, and a sparse leaky integrate-and-fire runner that steps it.

This crate is the full-resolution counterpart of `qualia-connectome-prior`: the prior aggregates the
release to **5 cell types and 9 edges** for the belief layers, this one keeps the neurons and the
synapses. Nothing here learns, and nothing here is trained: the graph *is* the controller.

## What the release actually is

All four flat tables are public and unauthenticated under
`gs://flyem-male-cns/v1.0/connectome-data/flat-connectome/`
(`https://storage.googleapis.com/flyem-male-cns/...` also works), CC-BY 4.0. Measured on
2026-09-12:

| Table | Bytes | Rows | What it holds |
| --- | --- | --- | --- |
| `connectome-weights-male-cns-v1.0-minconf-0.5.feather` | 1,051,241,946 | 151,856,684 | `body_pre`, `body_post`, `weight` â€” the connection graph |
| `body-annotations-male-cns-v1.0-minconf-0.5.feather` | 14,483,314 | 211,577 | `bodyId`, `type`, `superclass`, `somaSide`, `instance`, `somaLocation`, â€¦ |
| `body-neurotransmitters-male-cns-v1.0.feather` | 43,282,834 | 1,835,518 | `body`, `predicted_nt`, `consensus_nt` |
| `body-stats-male-cns-v1.0-minconf-0.5.feather` | 778,062,826 | 88,384,522 | one row per segment: `pre`, `post`, `synweight`, `type`, `superclass`, â€¦ |

Two facts decide the artifact's shape, and both are measured, not assumed:

1. **The flat release is segment-level.** `body-stats` describes 88,384,522 segments with at least one
   synapse. Only 166,700 of them carry a `superclass` â€” the neurons the paper counts (the v2 preprint
   abstract says 166,691 neurons and 11,691 types; the v1.0 annotation table has 166,700 such rows and
   **11,751** distinct non-null `type` labels, while the importer interns **11,752** because it adds
   the empty label for the 2,194 neurons that have no type) â€” and only **164,506** carry a cell type.
2. **The two tables cross-check.** Î£`weight` over the weights table is **311,833,243**, and
   Î£`post` over `body-stats` is **311,833,243** â€” the release's own segment-resolution synapse total,
   over 88.1M unannotated fragments as well as neurons.

Mapping the weights table through the annotation table â€” an edge exists when **both** ends are a
released neuron, exactly the rule `connectome-prior` uses â€” keeps **25,582,938 of 151,856,684 rows**
and **124,177,617 synapses** over **166,700 nodes**, with **no duplicate `(pre, post)` pairs** (the
release is already aggregated per pair). The other 126,273,746 rows are counted in the manifest as
`dropped_rows`; nothing is silently discarded. **So the neuron-level connectome is 166,700 neurons,
25.6M edges and 124.2M synapses â€” the "125 million synaptic connections" headline is a neuron-level
claim, and it is exactly what this artifact holds.** The 311,833,243 figure is the segment-resolution
total across all 88,384,522 segments; the 124.2M is what survives restricting both ends to released
neurons.

Of the 124,177,617 synapses, **94,542,746 are excitatory, 26,402,637 inhibitory, and 3,232,234** are
on edges whose presynaptic neurotransmitter the release leaves `unclear` or assigns to a modulatory
monoamine â€” those get sign 0 rather than an invented polarity, and the manifest counts them.

At 25.6M edges the artifact is the demo's regime: 5 bytes per edge (a `u32` post index and an `i8`
sign) plus 2 for the weight, so `weights.bin` is 180,414,198 bytes and the whole artifact 190 MB.

## The artifact

Four files per artifact directory, little-endian, struct-of-arrays, every section digest-checked on
load.

`weights.bin` â€” magic `QLCN`:

```
magic "QLCN" (4 B) | version u16 | reserved u16 | neuron_count u32 | reserved u32 | edge_count u64
rowptr  u64[neuron_count + 1]
cols    u32[edge_count]          post-synaptic node index
sign    i8 [edge_count]          +1 excitatory, -1 inhibitory, 0 unknown
weight  u16[edge_count]          synapses on the edge, as the release counted them
```

The reserved `u32` after `neuron_count` is there so `edge_count` and every following section is
8-byte aligned with no padding between them.

`positions.bin` â€” magic `QLCB`:

```
magic "QLCB" (4 B) | version u16 | reserved u16 | neuron_count u32
id         u32[N]    released body id
x, y, z    f32[N]    somaLocation in the release's 8 nm voxel space; NaN when unplaced
cell_type  u32[N]    interned index into types.txt
```

One row per CSR node, in CSR node order, so a firing id is a row index with no lookup table. 139,662
of the 166,700 neurons carry a `somaLocation`; the rest are NaN and are counted as
`positioned_count` in the manifest.

`types.txt` â€” one interned cell-type label per line, index = `cell_type`.
`nodes.txt` â€” one line per CSR node, tab separated: body id, type, superclass, side, instance. Text,
because it is metadata; it is what the encoders select populations from.

`spikes.bin` â€” magic `QLSP`, the recorded run: header `magic "QLSP" (4 B) | version u16 |
reserved u16` = 8 bytes, then one frame per tick, `tick u64 | t_ns u64 | count u32 | ids
u32[count]`, ids ascending.

`manifest.json` â€” the schema literal, the counts, per-section `{offset, len, sha256}` for both binary
files, the digest of each text table, the source tables' own byte sizes, digests and row counts, and
the CC-BY attribution. This is the only JSON, and it is not on any hot path.

`Artifact::load` verifies every section digest and refuses a file that does not match. A single
mutated sign byte fails the load with `DigestMismatch { section: "sign", .. }`.

## The runner

One tick is one frame. Per neuron:

```
if refractory > 0        { refractory -= 1; v = reset; no spike }
else {
    current = Î£ over incoming edges (sign * weight * spike[pre])
    current += external
    v = v * decay + current
    if v >= threshold    { spike; v = reset; refractory = refractory_ticks }
}
```

`crates/connectome-cns/src/lif.rs` is the CPU reference. `kernels/cns_lif.cu` is the device path: one
thread per neuron over the **incoming**-edge CSR (the transpose of the artifact's preâ†’post CSR, built
once at load), so the kernel gathers instead of scattering, needs no atomics, and launches exactly
once per tick. `CUDAARCHS=87-real` asks `nvcc` for a real sm_87 cubin; without it the kernel source
is compiled by NVRTC at start-up.

Default parameters are in `LifParams`: `decay 0.9`, `threshold 1.0`, `reset 0.0`, `refractory 2`.
They are not fitted to anything and not claimed to be the fly's.

## The closed loop

`loop` reads raw 8-bit grayscale frames (one `width * height` block after another â€” what
`v4l2-ctl --stream-mmap --stream-to=...` writes from the camera), and per tick:

* **Encoder**: direct external current enters the 6,091 annotated R1-R6/R7/R8
  receptors. It no longer broadcasts camera brightness into 89,403 intrinsic
  optic neurons. The 28 positioned receptors use soma Y as an uncalibrated
  eight-column proxy. The remaining 6,063 receive the mean image-column drive;
  their retinal positions are unknown. Fixed gain is 0.5.

  The artifact assigns inhibitory signs to all 74,557 receptor outgoing edges.
  Its zero-resting LIF model has no justified resting/transduction dynamics
  for the recipient populations, so receptor-only stimulation does not establish
  downstream motor firing. Signs reflect the import convention, not measured
  per-edge electrophysiology. This limitation remains open; adding random or
  tonic activity solely to make the display move would not validate the model.
* **Decoder** â€” the read-out is the released `descending_neuron` and `vnc_motor` superclasses, split by
  the release's `somaSide` into L and R. `command` is the sign of (rate_R âˆ’ rate_L) against a 0.001
  dead band; `throttle` is the total output rate. `-1` steers left, `+1` right, `0` hold.

The run writes `spikes.bin` (every tick's firing set) and a CSV trace of tick, luminance, firing input
count, both output rates, command and throttle.

### Wheel frames for the leash transport

`loop --frames-out -` emits one newline-terminated JSON frame on stdout **during each tick**, before
fetching the next image. The CSV row is written and flushed before that frame. Loop status goes to
stderr. Without `--frames-out`, the loop only records the run. A path instead of `-` records the JSON
lines in a new file (an existing destination is refused); use stdout for the live pipe, since replaying
a completed file would deliver old decisions in a burst.

The CSV's `rate_l` and `rate_r` are neural firing fractions, not wheel speeds. The bridge uses the
same `command` and `throttle` values that write the trace, before decimal rounding:

| Decision | Wheel frame |
| --- | --- |
| `-1`, steer left | `L = -throttle * limit`, `R = throttle * limit` |
| `+1`, steer right | `L = throttle * limit`, `R = -throttle * limit` |
| `0`, hold | `L = 0`, `R = 0`, even with nonzero throttle |

`T` is the loop's tick, starting at zero. `--max-wheel-speed` sets `limit` (default `0.04`, finite,
between zero and one). This is a bounded pivot mapping in the transport's wheel-command units, not a
measurement of physical speed or a calibrated steering law. Leash separately applies the operator's
`speed_mode: low`. No transport lease, deadman, stop acknowledgement or session-label logic changes.

Build both programs **sequentially before** opening a motion window; do not pipe two `cargo run`
commands together. Once the operator confirms the acknowledgement fix, is present, and the zero-speed
drive probe succeeds, a bounded live command in a shell with native pipes is:

```sh
export QUALIA_LEASH_BASE_URL=http://192.168.55.1:8000
export QUALIA_LEASH_OPERATOR_TOKEN_FILE=/private/path/to/operator.token
export QUALIA_LEASH_TRANSPORT_ROUTE=http
export QUALIA_LEASH_TRANSPORT_DEADMAN_MS=500
export QUALIA_LEASH_TRANSPORT_LEASE_TTL_SECS=20
export QUALIA_LEASH_SPEED_MODE=low
export QUALIA_SHM_NAME=/qualia_body  # an existing init-owned arena

qualia-connectome-cns loop --artifact /path/to/artifact \
  --camera http://192.168.55.1:8000/camera/snapshot --ticks 20 --device gpu \
  --spikes run/spikes.bin --trace run/trace.csv --session fly-driving \
  --frames-out - --max-wheel-speed 0.04 2>run/loop.log |
  qualia-leash-transport 2>run/transport.log
```

Use a fresh run directory and exactly one transport. Set an operator-agreed wall-clock deadline as
well as the tick cap; camera fetches and acknowledgement latency affect duration. On end of input,
the transport requests a verified stop and continues publishing stop records. Observe that receipt
before ending it. A stalled camera produces no substitute commands, so the transport's existing
arrival-based deadman sees the gap. A frame-write failure exits the loop with an error; it never
silently skips a hold or substitutes a previous command. A deadman demonstration must hold the pipe
open without frames: closing it exercises the end-of-input stop instead.

The operator bearer must be supplied privately and removed after the window, as described in
[`handoff-driving.md`](../../docs/handoff-driving.md). This command is a recipe, not evidence of a live
run. #262 still needs leash's own applied-action evidence and telemetry for all three demonstrations;
the estop demonstration additionally waits for the operator's decision about clearing the latch.

## Commands

```console
$ cargo run -p qualia-connectome-cns -j 2 -- import \
      --weights       flat-connectome/connectome-weights-male-cns-v1.0-minconf-0.5.feather \
      --annotations   flat-connectome/body-annotations-male-cns-v1.0-minconf-0.5.feather \
      --neurotransmitters flat-connectome/body-neurotransmitters-male-cns-v1.0.feather \
      --out /path/to/artifact
$ cargo run -p qualia-connectome-cns -j 2 -- verify --artifact /path/to/artifact
$ CUDAARCHS=87-real cargo run -p qualia-connectome-cns --features cuda -j 2 --release -- \
      bench --artifact /path/to/artifact --ticks 600 --device gpu
$ cargo run -p qualia-connectome-cns --features cuda -j 2 --release -- loop \
      --artifact /path/to/artifact --frames frames.raw --width 640 --height 480 \
      --ticks 600 --spikes run/spikes.bin --trace run/trace.csv --device gpu --session <name>
```

Tests: `the_artifact_round_trips_the_released_synapse_counts` reads the real release from
`QUALIA_MALECNS_DIR` and asserts the numbers in the table above; without that directory it prints why
it skipped. `a_mutated_sign_fails_the_digest_check` builds a three-node artifact, flips one sign byte
and asserts the load refuses it.

## What this does not establish

No learning, no plasticity, no neuromodulation: the graph is fixed and the only state is membrane
potential and a refractory counter. The monoamines the release reports (dopamine, octopamine,
serotonin) get sign 0 â€” modulatory â€” rather than an invented polarity, and so does `unclear`; the
manifest reports how many synapses fall in each class. Nothing here shows the fly "is in there"; the
loop records measured visual input, simulated receptor activity and an uncalibrated decoder.
It does not establish downstream sensory-to-motor behavior or applied wheel motion.
A free-running network with no input is silent (every membrane sits at rest), which is why the loop's
drive is what makes it spike â€” there is no noise term and none is invented.

The released Feather tables are lz4-framed, so the reader needs arrow's `ipc_compression` feature;
that one line is the difference between "not Arrow IPC" and the release being readable at all.

One lesson the record should keep: the first exploration of these tables used a numpy bitmap built as
`bits[idx] |= value`, and numpy applies that once per unique byte index, so the mask silently
under-set bits and the neuron-level join came out 24Ã— too small. The Rust importer disagreed with it,
which is how the error surfaced. The numbers above are the ones the Rust importer and a corrected
independent pass agree on.

### Measured multisensor input (experimental)

`--camera <JPEG URL> --sensors <telemetry/compact URL> --fusion-evidence <new.jsonl>`
requires real, timestamped IMU, odometry pose and lidar. Inputs older than one
second, more than 500 ms apart, missing coordinate frames, nonfinite values or
insufficient valid ranges are refused. Failed acquisitions emit a zero wheel
frame if configured, record the failure, reset temporal history and skip the
neural step; the bounded loop retries. A run with no valid steps fails.

Gyroscope yaw and odometry yaw derivative are averaged. Speed and the measured
acceleration magnitude attenuate camera temporal contrast; lidar proximity
modulates global contrast attention. The eight conditioned columns are
`0.75 * brightness + 0.25 * temporal_difference * motion_attenuation *
(1 + proximity_attention)`, clamped to [0,1]. These are engineering features,
not measured fly currents, a calibrated Kalman estimator or registered optical
flow. Acceleration includes gravity. No camera pose or pixel-to-lidar alignment
is invented. The JPEG endpoint lacks a capture timestamp; evidence explicitly
records the request/retrieval interval instead.

A return inside 0.25 m, inconsistent yaw rates, excessive acceleration or
uninitialized odometry derivative holds the output. Evidence pairs measured
features with the decoded and gated proposal. It does not store raw JPEGs or
full scans and does not prove wheel application. Freshness is rechecked after
input evidence writes and before wheel serialization. If a nonzero candidate
expires while its trace is being written, the loop emits zero, annotates the
canceled trace candidate and fails. QLSP timestamps remain elapsed run time.

A local observation run uses `--max-wheel-speed 0` and omits `--frames-out`.
Physical driving remains subject to #262's acknowledgement and operator gates,
and to resolving the downstream model and sensor/controller calibration gaps.

For the console, `python scripts/live_console_feed.py --producer <built-executable>
--artifact <artifact-directory> --camera <snapshot-URL> --runtime <state-directory>`
starts one observation producer, bound to 127.0.0.1:18762, capped at 30 minutes
and 20,000 attempts. The camera base serves `/telemetry/compact`. It writes
`live-status.json` for `QUALIA_FLY_STATUS` and `live-run.txt` pointing to its
trace, spikes, fusion evidence and log. The helper always sets wheel speed zero
and never attaches the transport. New viewers receive only subsequent live
frames, not a cached last frame. Set `QUALIA_CONNECTOME_SPIKES` to
`tcp://127.0.0.1:18762`. A finished bounded run is explicitly stale, not silently
replaced with replayed or generated activity.

The bridge treats status-file I/O failures as optional diagnostics: a Windows
reader denying atomic replacement leaves the previous timestamped snapshot in
place, and the next polling cycle retries. This does not terminate the producer.
Warning-log failures are also nonfatal. `python scripts/test_live_console_feed.py
-v` covers a real Windows replacement-denying handle, recovery, persistent I/O
failure and a broken warning sink. A live lock experiment recorded forty frames
while the status destination stayed locked for eight seconds; see
`docs/evidence/live-stream-status-lock-20260912.json`.

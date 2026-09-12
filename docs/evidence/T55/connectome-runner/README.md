# T55 slice 1 — the Male CNS connectome as a spiking controller

Ticket: [#236](https://github.com/superposition/qualia/issues/236), step 55 items 1–3 (the importer,
the LIF runner on the Orin, and the closed loop as plumbing). Items 4–5 of the ticket — the game
bridge and the robot path — are later steps and are not attempted here.

Crate: `crates/connectome-cns` (`qualia-connectome-cns`), kernel `kernels/cns_lif.cu`.

## What was run

Artifact on the dev host's scratch, built from the released tables
(`gs://flyem-male-cns/v1.0/connectome-data/flat-connectome/`, CC-BY 4.0, downloaded 2026-09-12):

| File | Bytes | SHA-256 |
| --- | --- | --- |
| `connectome-weights-male-cns-v1.0-minconf-0.5.feather` | 1,051,241,946 | `e35da783d1c686b2…` |
| `body-annotations-male-cns-v1.0-minconf-0.5.feather` | 14,483,314 | `2177e246113e4cfb…` |
| `body-neurotransmitters-male-cns-v1.0.feather` | 43,282,834 | `95c9289220663abe…` |
| `body-stats-male-cns-v1.0-minconf-0.5.feather` | 778,062,826 | (cross-check only) |

Logs in this directory:

* `host-import.log` — `import` over the real tables (24.7 s).
* `host-verify.log` — `verify`, which re-loads the artifact and checks every section digest.
* `host-bench-cpu.log` — the CPU reference and the 4090, same artifact.
* `host-loop-gpu.log`, `host-loop-gpu-trace.csv` — the closed loop against the robot's own camera.

## The artifact

`import` reads all four tables' worth of facts and writes `weights.bin` (`QLCN`: `rowptr u64[N+1]`,
`cols u32[E]`, `sign i8[E]`, `weight u16[E]`), `positions.bin` (`QLCB`, struct-of-arrays
`id/x/y/z/cell_type`), `types.txt`, `nodes.txt` and a `manifest.json` carrying per-section byte ranges
and SHA-256 digests. `Artifact::load` refuses a file whose bytes do not match its digest.

```
cns-import: neurons 166700
cns-import: edges 25582938
cns-import: synapses 124177617
cns-import: sign +94542746 -26402637 ?3232234
cns-import: source rows 151856684 source synapses 311833243
cns-import: mapped rows 25582938 dropped pre 13640158 dropped post 121470994 folded 0
cns-import: positioned 139662 types 11752
cns-import: -> artifact in 26.0445714s
```

`weights.bin` is 180,414,198 B; the whole artifact 190 MB. The release is *segment*-level
(88,384,522 segments, 166,700 of them neurons), so "mapped rows" is the subset whose two ends are
both released neurons: **166,700 neurons, 25,582,938 edges, 124,177,617 synapses** — which is the
ticket's "125 million synaptic connections" at neuron level. Σ`weight` over the whole table,
311,833,243, equals Σ`post` in `body-stats` and is the segment-resolution total.

## Rates

| Device | ms/tick | ticks/s | M synapses/s | resident |
| --- | --- | --- | --- | --- |
| dev host CPU (one thread, i9-14900KF) | 20.615 | 48.5 | 1,241.0 | — |
| dev host RTX 4090 (NVRTC, sm_89) | 1.381 | 723.9 | 18,519.0 | 182,247,874 B |
| Jetson Orin NX, Pinkie (sm_87 cubin, 6 cores) | 11.840 | 84.5 | 2,160.6 | 182,247,874 B |
| Jetson Orin NX, Pinkie, CPU reference | 49.140 | 20.4 | 520.6 | — |

A free-running network is silent: with no input every membrane sits at rest and there is no noise
term, so the bench's last tick reports 0 spikes. That is the model, not a bug; the loop's drive is
what makes it spike.

## The closed loop, on the robot's own camera

The hardware is owned by the `leash` service, which serves the camera as JPEG
(`/camera/snapshot`). The loop fetches one snapshot per tick and drives the connectome's visual
stage; nothing opens `/dev/video0`.

```
cns-loop: encoder drives 95494 visual-stage cells (ol_intrinsic + R1-R6/R7/R8) over 8 columns, gain 0.5
cns-loop: decoder reads 2012 descending/motor neurons (1011 L, 1001 R)
cns-loop: device NVIDIA GeForce RTX 4090
cns-loop: 200 ticks in 1.775 s (112.7 ticks/s), 10048960 spikes, 196 non-hold commands
```

On the board the same loop ran 300 ticks at **31.3 ticks/s** (camera-bound: the leash serves ~33
snapshots/s, so the tick rate is the sensor rate, not the runner's) with 15,168,990 spikes and 296
non-hold commands, and every section digest re-verified on the board before the run. **60 Hz needs
16.7 ms/tick and the Orin does 11.840 ms/tick, so the imported artifact sustains 60 Hz with ~1.4x
headroom** at 2,160.6 M synapse-updates/s.

The trace (`host-loop-gpu-trace.csv`) is per tick: luminance, firing input count, both output rates,
`command` and `throttle`. Over those 200 ticks the command was `-1` (steer left) 130 times, `+1`
(right) 66 and `0` (hold) 4.

**What this is.** The released connectome, with the release's own wiring and the release's own
signs, turning a camera frame into a steering decision at frame rate, with nothing trained and no
reward anywhere in the path. **What it is not.** No behaviour claim: the encoder's column mapping
and the read-out rule are fixed constants I chose, the camera was pointed at a nearly uniform scene
(luminance 0.4613–0.4679 over the run, so the frame contributes little), and the network's own
recurrent dynamics dominate. It is plumbing, which is what this slice was for.

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
* `board-bench.log` — **historical**, kept for the record: an Orin build/verify/bench excerpt whose loop
  section is a *failed* read-back by the binary built before `71c9900` (the 12-byte-header reader). Its
  bench rows are the Orin numbers below; its loop section proves nothing and is superseded by
  `board-loop.log`.
* `board-loop.log` — the 300-tick board loop re-run with the fixed reader under a D-024 lease
  (`14dae29`, 2026-09-12 02:55 board-local): the extract/build, `verify`, a 200-fetch leash snapshot-rate
  probe, the loop and the `replay` read-back, at `set -x` fidelity.
* `board-loop-trace.csv` — the earlier board loop's per-tick trace (2026-09-12 02:0x).
* `board-loop-trace-rerun.csv` — the per-tick trace of the `board-loop.log` re-run.
* `cpu-aliasing-demo.log` — the before/after run for the CPU spike-buffer fix (review item N1).

The board run's 57.9 MB `spikes.bin` recording is deliberately **not** committed (it is a recording,
not source, and it is over GitHub's 50 MB recommendation). It is reproducible on the board with the
`loop` command above, and T56 consumed the byte-identical host-side stream
(`sha256 a0a27acbe61e0a9a5f013f77a9741ce466221b872bba711f7a07409f68593b28`) for its viewer.

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

### The board window's commands

The build and both benches behind the Orin rows and the loop were run by this script on Pinkie
(`--warmup` was 100 for the 600-tick GPU bench and 20 for the 200-tick CPU one):

```console
$ CUDAARCHS=87-real cargo build -p qualia-connectome-cns --features cuda -j 2 --release --offline
$ ./target/release/qualia-connectome-cns verify --artifact /home/jetson/t55-artifact
$ ./target/release/qualia-connectome-cns bench --artifact /home/jetson/t55-artifact --ticks 600 --warmup 100 --device gpu
$ ./target/release/qualia-connectome-cns bench --artifact /home/jetson/t55-artifact --ticks 200 --warmup 20 --device cpu
$ ./target/release/qualia-connectome-cns loop --artifact /home/jetson/t55-artifact \
      --camera http://127.0.0.1:8000/camera/snapshot --ticks 300 \
      --spikes /home/jetson/t55-run/spikes.bin --trace /home/jetson/t55-run/trace.csv \
      --device gpu --session leash-camera
$ ./target/release/qualia-connectome-cns replay --spikes /home/jetson/t55-run/spikes.bin
```

The 300-tick loop was re-run with the fixed reader under a D-024 lease in the review-fix commit; its
transcript is `board-loop.log`.

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

On the board the same loop ran 300 ticks at **32.1 ticks/s** with 15,085,686 spikes and 294 non-hold
commands — one camera JPEG per tick over USB, `--device gpu`, every section digest re-verified on the
board before the run, and the fixed QLSP reader (`board-loop.log` is the re-run's transcript). That is
**31.2 ms/tick end to end**, ~19 ms above the 11.840 ms bare step.

**The 60 Hz figure is a property of the bench, not of the loop.** 16.7 ms is a 60 Hz frame and the
free-running bench's 11.840 ms/tick sits 1.41x under it, so 60 Hz bounds the kernel's step over the
25,582,938 edges; the closed loop as run was **~32 Hz end to end**. It is not camera-bound: a 200-fetch
probe of the leash's own snapshot endpoint *on the board* measured **2.485 s = 80.5 snapshots/s**, well
above the 32.1 ticks/s the loop consumed, and the host loop fetching the same endpoint
(`http://192.168.55.1:8000/camera/snapshot`, Pinkie's camera) ran 200 ticks in 1.775 s
(**112.7 ticks/s**, 8.9 ms/tick) on the RTX 4090. What is claimed is the observed loop rate under those
conditions; the earlier "~33 snapshots/s, camera-bound" line was never measured and is withdrawn.

The trace (`host-loop-gpu-trace.csv`) is per tick: luminance, firing input count, both output rates,
`command` and `throttle`. Over those 200 ticks the command was `-1` (steer left) 130 times, `+1`
(right) 66 and `0` (hold) 4.

**What this is.** The released connectome, with the release's own wiring and the release's own
signs, turning a camera frame into a steering decision at frame rate, with nothing trained and no
reward anywhere in the path. **What it is not.** No behaviour claim: the encoder's column mapping
and the read-out rule are fixed constants I chose, the camera was pointed at a nearly uniform scene
(luminance 0.4613–0.4679 over the run, so the frame contributes little), and the network's own
recurrent dynamics dominate. It is plumbing, which is what this slice was for.

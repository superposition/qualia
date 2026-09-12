# T53 encoder-gradients — the encoder now receives the objective's gradient

Step 1 of ticket [#228](https://github.com/superposition/qualia/issues/228) (T53), the repair
`docs/decisions.md` D-021 named. T52 (#227) measured that no artifact this repository trains can
clear the promotion gates, and that the decisive fact is a defect rather than a data problem: across
a one-epoch and a ten-epoch run of the same fixture and seed, all 20 `encoder.*` and all 20
`target_encoder.*` tensors were byte-identical while the predictor moved, and `effective_rank` read
`1.11967` at both epoch counts against the gate's floor of `64`. This capture locates the line that
severed the objective's backward pass from the online encoder, fixes it, and re-runs the same ch4
fixture on the 4090.

**The answer, in one paragraph.** The encoder's last operation,
`self.fusion_norm.forward(&fused)` (`crates/jepa-model/src/lib.rs:143` before this change),
dispatched into `candle_nn::ops::layer_norm`, which is built with `apply_op3_no_bwd` and returns a
tensor with no gradient leaf — so the emitted latent changed when a parameter's storage changed but
carried no path back to any encoder parameter, and the whole encoder was detached from the
objective. The fix normalizes the fused representation with the same layer written in
differentiable ops (`candle_nn::ops::layer_norm_slow`, same weight/bias/epsilon) so the latent stays
in the graph. With that one change the encoder moves: `48 of 50` checkpoint tensors differ between
the before and after runs (all `20/20 encoder.` and `20/20 target_encoder.`; only the intentionally
fixed `adapter.` pair is unchanged), the online encoder's parameters move by `2.42e-3` and its
emitted latent by `1.50` over eight CPU steps, and `effective_rank` moves for the first time
(`1.11967` → `1.20479` at one epoch, `1.45009` at ten) — but it is still far below `64`, so the
promotion gates are **not** reachable and **T52's step 2 is still not producible**. That last number
is a data finding for T52 (#225); the threshold was not touched.

## The defect, named and quoted

`crates/jepa-model/src/lib.rs`, `TinyCnnEncoder::forward`, the pre-fix tree:

```rust
        let pose_features = self.pose_projection.forward(pose)?.relu()?;
        let fused = Tensor::cat(&[&camera_features, &lidar_features, &pose_features], 1)?;
        self.fusion_norm.forward(&fused)                                          // line 143
    }
```

`LayerNorm::forward` (candle-nn 0.9.1) takes its fused path for a contiguous, mean-removing,
biased input:

```rust
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        if x.is_contiguous() && self.remove_mean {
            if let Some(bias) = self.bias.as_ref() {
                return crate::ops::layer_norm(x, &self.weight, bias, self.eps as f32);
            }
        }
```

and `crate::ops::layer_norm` ends in a no-backward custom op:

```rust
    xs.apply_op3_no_bwd(alpha, beta, &LayerNorm { eps })
```

`apply_op3_no_bwd` returns `from_storage(storage, shape, BackpropOp::none(), false)`
(`candle-core-0.9.1/src/custom_op.rs:170`) — the returned tensor is a dead leaf. The normalizer is
the encoder's final operation, so every branch below it (camera, LiDAR, pose, and the three
convolutions and three projections) lost its gradient path at once. That is exactly the signature
T52's probe measured: `visible.sum_all().backward()` reached **zero** leaf variables while mutating
the map's `encoder.camera.conv1.weight` still changed `visible` (`docs/evidence/T52/checkpoint-experiments/gradient-probe.txt`).

## The fix

One paragraph, as a reviewer can check it: the encoder's fused representation is now normalized by
a crate-local `normalize_fused` helper (`crates/jepa-model/src/lib.rs`) that calls
`candle_nn::ops::layer_norm_slow` with the same `fusion_norm` weight, bias and epsilon
(`FUSION_NORM_EPSILON`, hoisted from the `1e-5` that was already passed to `layer_norm`), instead of
`LayerNorm::forward`'s fused no-backward path. `layer_norm_slow` is the same normalization written
with differentiable tensor ops, so the emitted values keep the layer's affine parameters and
epsilon while the gradient reaches the encoder. Nothing else changed: the objective (`jepa_loss`),
the VICReg terms, the optimizer group (`online_vars.all_vars()`, which always contained the encoder
parameters), the EMA update (`ema_update_prefix(&target_vars, &online_vars, decay, "encoder.")`,
whose target is deliberately detached), the target-network `detach` in `ema_target_latent`, and the
`EFFECTIVE_RANK_MINIMUM = 64.0` threshold are all untouched. The EMA target was never wrong — it
had nothing new to track while the online encoder it averages was frozen.

The ticket's test is `the_encoder_moves_when_the_objective_steps`
(`crates/jepa-model/src/train.rs`). It trains eight optimiser steps on a fixed four-transition batch
and asserts observable movement, not non-zero gradients: all 20 online `encoder.*` tensors change
(bit-exact digest), the online encoder's largest parameter change exceeds `1.0e-4`, the EMA target's
largest parameter change is positive and smaller than the online encoder's (the target trails it, as
the decay defines), the predictor's own update continues, and the emitted latent of a fixed probe
observation moves by more than `1.0e-3`.

Pre-fix (on `origin/main` + this test only), quoted from the run:

```text
thread 'train::tests::the_encoder_moves_when_the_objective_steps' panicked at crates\jepa-model\src\train.rs:877:9:
assertion `left != right` failed: the prediction objective must move the online encoder's parameters
  left: "27a27724372551ccb147619e1ac561bd8aa0c0617d608f43c36035899eba6c51"
 right: "27a27724372551ccb147619e1ac561bd8aa0c0617d608f43c36035899eba6c51"

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 24 filtered out
```

Post-fix, quoted from the run:

```text
test train::tests::the_encoder_moves_when_the_objective_steps ... ok

test result: ok. 25 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 18.31s
```

Measured movement over those eight CPU steps (temporary prints, removed from the committed test; the
assertions keep the floors `1.0e-4`, trailing-target and `1.0e-3`): online encoder `max |Δ| = 2.422154e-3`,
EMA target encoder `max |Δ| = 4.3094158e-5`, emitted latent `max |Δ| = 1.4990321`. The predictor's
own largest parameter change is `2.4259612e-3`, the same kind of update it always had.

## Host and tools

WSL2 `Ubuntu-22.04` on the development workstation, RTX 4090, `rustup` toolchain `1.98.1`, `nvcc`
12.8, `LD_LIBRARY_PATH=/usr/local/cuda/lib64:/usr/lib/wsl/lib`, `PATH` including `/usr/local/cuda/bin`.
The CUDA build ran `-j 2` (D-014) and the two GPU runs ran one at a time (D-011); no display adapter,
driver or `pnputil` call was made (D-012). Build rc 0, ~3m38s. Both runs used the ch4 manifest T52's
capture committed as its fixture, byte-for-byte and unmodified:
`jepa-dataset-26e2dfac396e343f09b27b350366b24ef06f33b65f0c90ca48919ac035245b8e.json`
(12 environments × 1 session × 5601 frames, chain 4, 50 400 transitions).

## The runs, against the committed before capture

Every number below is from a run's own training report. `slope` is `calibration_slope`, `msr`
`mean_standardized_squared_residual`, `clamp` `clamp_fraction`, `rank` `effective_rank.effective_rank`;
the validation split is shown, and the test split follows. "before" is T52's committed capture
(`docs/evidence/T52/checkpoint-experiments/`, PR #227); "after" is this capture.

| run | epochs | nll cnn / flat / const | rollout cnn / flat / const | slope | msr | cov50/90/95 | clamp | rank | trace |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| before `ch4-e1` (`8d0f4fc4…`) | 1 | −3.36228 / −1.49865 / −1.24642 | 0.128516 / 0.152176 / 1.48451 | 1.43005 | 1.15548 | .497952/.877320/.929558 | 7.8125e-05 | **1.11967** | 55.9204 |
| after `ch4-e1` (`9a19815a…`) | 1 | −3.24818 / −1.49865 / −0.898488 | 0.0150985 / 0.152176 / 0.747547 | 0.531798 | 1.19997 | .485757/.870773/.923415 | 1.69271e-03 | **1.20479** | 24.0721 |
| before `ch4-e10` (`29f68c12…`) | 10 | −4.39977 / −1.92172 / −1.24642 | 0.0229308 / 0.0984595 / 1.48451 | 0.426056 | 0.709466 | .582243/.948500/.978070 | 0.595309 | **1.11967** | 55.9204 |
| after `ch4-e10` (`b515fd79…`) | 10 | −4.25286 / −1.92172 / −0.676790 | 0.00176942 / 0.0984595 / 0.570653 | 0.271622 | 0.833118 | .608339/.924509/.954241 | 0.527731 | **1.45009** | 11.2427 |

Test split, same four runs:

| run | nll cnn / flat / const | rollout cnn / flat / const | slope | msr | cov50/90/95 | clamp |
| --- | --- | --- | --- | --- | --- | --- |
| before `ch4-e1` | −3.40221 / −1.34188 / −1.21891 | 0.146272 / 0.190448 / 1.44254 | 1.39492 | 1.11739 | .509733/.884253/.932046 | 6.77083e-04 |
| after `ch4-e1` | −3.31378 / −1.34188 / −0.892395 | 0.0410029 / 0.190448 / 0.734845 | 1.15682 | 1.15455 | .502229/.878654/.929876 | 6.04167e-03 |
| before `ch4-e10` | −4.38194 / −1.57749 / −1.21891 | 0.0758769 / 0.177568 / 1.44254 | 0.377954 | 0.730018 | .578295/.947595/.976918 | 0.655907 |
| after `ch4-e10` | −4.17354 / −1.57749 / −0.663880 | 0.0101836 / 0.177568 / 0.574728 | 0.236661 | 0.918882 | .584451/.913091/.947677 | 0.505681 |

Three things a reviewer can check in those tables:

- **The encoder moved, and only the encoder's consequences did.** `flat_mlp`'s validation and test
  subtree is *byte-identical* before and after (its numbers are identical to the last digit), and so
  is each split's `occupancy` subtree: the flat baseline has no fused LayerNorm, and the occupancy
  decoder is the same module. The candidate (`tiny_cnn`) moved, and the `constant` baseline moved
  because it is fitted from the target encoder's training residuals
  (`OfflineTrainer::calibrate_constant_baseline`), which now trains.
- **The prediction objective improves**: validation rollout error `0.128516` → `0.0150985` at one
  epoch and `0.0229308` → `0.00176942` at ten, still far below the constant baseline.
- **Calibration remains out of band, in the opposite direction.** Before, one epoch failed on slope
  `1.430` (too high); after, it fails on slope `0.532` (too low) with `msr 1.200`. Ten epochs fails
  on slope `0.272` and `clamp 0.528` on validation. The fixture's calibration is not fixed by making
  the encoder train; it is a separate, still-open finding.

## The encoder tensors actually moved between the checkpoints

`before-after-weights.txt` is `diff_weights.py` over T52's committed `ch4-e1` checkpoint
(`weights.safetensors` sha256 `9132e253fc81fabce572bc08b8d208a8316a2be4f74088530aefd4ad83825d67`,
the digest T52's record names) and each after-fix checkpoint. T52's comparison was one epoch against
ten on the same fixture and seed; both of these are the same fixture and seed with the fix applied.

| comparison | tensors | moved | encoder. | target_encoder. | adapter. |
| --- | --- | --- | --- | --- | --- |
| T52 before: `ch4-e1` → `ch4-e10` (its record) | 50 | 8 | **0/20** | **0/20** | 0/2 |
| T53 after: `ch4-e1` → after `ch4-e1` | 50 | 48 | **20/20** | **20/20** | 0/2 |
| T53 after: `ch4-e1` → after `ch4-e10` | 50 | 48 | **20/20** | **20/20** | 0/2 |

The largest encoder movements in the after-fix pair are
`encoder.pose.projection.weight` `2.904146e-01` (scale `8.656400e-01`),
`encoder.pose.projection.bias` `2.898350e-01`, `encoder.fusion.norm.weight` `2.289982e-01`, and
their EMA copies `2.575220e-01` / `2.570077e-01` / `1.984508e-01`; before, every one of those was
`0.000000e+00`. The two unchanged tensors are the fixed evidence adapter, which is not in the
objective. Weights digests: after `ch4-e1` `9acf6047b931698961459b1785c1bc0b168c9e84c93ad806cbc9b31c879b6012`,
after `ch4-e10` `3e0c77c5ec4ab796a7e9abdbc2ba9fe90fe23dd5ca14619e442b8d4f23a4d762`.

## Are the promotion gates reachable?

**No — measured, not assumed.** After the fix, on the ch4 fixture:

| gate | before | after `ch4-e1` | after `ch4-e10` |
| --- | --- | --- | --- |
| predictive (validation and test) | passes | passes | passes |
| occupancy (IoU/PR-AUC over trivial) | passes | passes | passes |
| calibration (`slope`, `msr` in [0.9, 1.1]; clamp ≤ 0.01) | fails (slope 1.430) | fails (slope 0.532) | fails (slope 0.272, clamp 0.528) |
| effective rank (≥ 64) | fails (`1.11967`) | fails (`1.20479`) | fails (`1.45009`) |
| `all_gates_passed` | false | false | false |

The trainer's own line is unchanged in kind: `gate=false` and
`qualia-jepa-train: candidate failed held-out predictive, grounding, or calibration gates`, and both
after-fix reports were published (every metric finite) yet `all_gates_passed=false`.

`effective_rank` now depends on training for the first time — it moves from `1.11967` to `1.20479`
at one epoch and `1.45009` at ten, where T52 measured it bit-identical (`1.119669434677054`) at both
epoch counts — but it is still `~44×` below the `64` floor at ten epochs on this fixture. The
representation the gate reads is the held-out `target_latent` of the **EMA target encoder**
(`qualia-jepa-train.rs`, `measure_holdout` → `candidate.predict_batch` → `target_latent`), so the
repair does what `D-021` asked: the gate's input is now a function of the training the pipeline
performs. Lowering `64` to fit `1.45` is still fitting a band to a number the pipeline cannot move
by training; per the ticket and D-021 the threshold is untouched.

**This is a data finding and it belongs to T52 (#225)**: on the chain-4 wave fixture, ten epochs of
the repaired objective take `effective_rank` from `1.11967` to `1.45009` (trace `55.9204` →
`11.2427`), which is far from `64`. Whether a different fixture geometry or a much longer schedule
reaches the band is T52's question, now answerable because the encoder trains; it is not a wiring
question any more. **T52's step 2 (`qualia-jepa-parity` and `qualia-jepa-plan-eval` against a
promotion-passing checkpoint) is therefore still not producible**: no artifact this repository
trains clears `all_gates_passed`, so there is still no promoted checkpoint to point those runs at.

## The board leg

**Measured on Pinkie** — the Jetson Orin NX at `jetson@192.168.55.1` (D-010): `aarch64`, Ubuntu
22.04.5 (GLIBC 2.35), kernel `5.15.148-tegra`, driver `NVRM 540.4.0`, CUDA 12.9, 6 cores. The tree
was head `18313c5`'s from a `git archive` (archive sha256
`5c5feaa145375ab30e6c9c7da9de847c696f5e5045790161b8c1a2e28e8a4a66`); `1015432` differs from it only
in this file, so the built source is the current head's source. Board3's posting on #228/#225 is the
authority for the transcript below.

```text
$ cargo fetch                                            FETCH_RC=0
   (nothing to download: every entry in this head's lock was already in the board's registry cache, 1.7 GB)
$ cargo search candle-core --limit 2                      # cargo on the board IS online
candle-core = "0.11.0"

$ RUSTFLAGS="-C target-feature=+fp16" cargo build --release -j 2 -p qualia-jepa-model --bins
   Compiling candle-core v0.9.1
   Compiling candle-nn v0.9.1
   Compiling qualia-jepa-model v0.1.0 (/home/jetson/qualia-t53/crates/jepa-model)
    Finished `release` profile [optimized] target(s) in 7m 19s     BUILD_RC=0
$ ls target/release/qualia-jepa-{train,parity,plan-eval,runtime-probe}
   2390656 parity · 2391088 plan-eval · 2018664 runtime-probe · 4501616 train   (all aarch64)

$ RUSTFLAGS="-C target-feature=+fp16" cargo build --release -j 2 -p qualia-jepa-model --bins --features cuda
    Finished `release` profile [optimized] target(s) in 5m 30s      BUILD_CUDA_RC=0
   (candle-kernels 0.9.2 built with nvcc for sm_87, then candle-core/candle-nn 0.9.1 with the cuda feature)
```

and the model package **runs** on both backends — T50's row-C fixture seed `12648430`, the whole
five-head step, native on the Orin:

```json
$ qualia-jepa-runtime-probe --backend cpu --iterations 30 --warmup 3
{
  "schema_version": "qualia.jepa-runtime-probe.v1",
  "runtime_id": "qualia.jepa.coherent-tiled-runtime.v1",
  "backend": "cpu",
  "iterations": 30,
  "warmup_iterations": 3,
  "fixture_seed": 12648430,
  "synchronized_latency_p50_us": 3168,
  "synchronized_latency_p95_us": 3406,
  "synchronized_latency_max_us": 3429,
  "outputs_finite": true,
  "output_dimensions": [256, 256, 256, 1024, 4096]
}
```

```json
$ qualia-jepa-runtime-probe --backend cuda --iterations 30 --warmup 3     # LD_LIBRARY_PATH=compat:cuda/lib64
{
  "schema_version": "qualia.jepa-runtime-probe.v1",
  "runtime_id": "qualia.jepa.coherent-tiled-runtime.v1",
  "backend": "cuda",
  "iterations": 30,
  "warmup_iterations": 3,
  "fixture_seed": 12648430,
  "synchronized_latency_p50_us": 2039,
  "synchronized_latency_p95_us": 2221,
  "synchronized_latency_max_us": 2235,
  "outputs_finite": true,
  "output_dimensions": [256, 256, 256, 1024, 4096]
}
```

The device was really used, not skipped: `backend: cuda` with `outputs_finite: true` and no
`skipping` line anywhere in the output, at p50 `2039 µs` against the same binary's CPU backend at
`3168 µs` (1.55× faster on this fixture). Board artefact sizes: `qualia-jepa-runtime-probe`
10 375 096 B with cuda (2 018 664 B without), `qualia-jepa-parity` 2 390 656 B,
`qualia-jepa-plan-eval` 2 391 088 B, `qualia-jepa-train` 4 501 616 B, all `ELF 64-bit LSB pie
executable, ARM aarch64`.

**The earlier "no `candle-core`, no DNS" sentence is refuted by this measurement.** The board's
registry cache was complete — `cargo fetch` had nothing to download, with 1.7 GB already cached —
and the board does fetch: its WiFi associates-rejected against the AP
(`wlP1p1s0: CTRL-EVENT-ASSOC-REJECT status_code=1`, both bands, BSSID-pinned and with the host's MAC
cloned, reading the AP at 17–29 % against the host's 62 %), so the working route is a userspace
CONNECT proxy on the host's USB-gadget link (`192.168.55.100:8085`), through which the sparse index
resolves and cargo's own `cargo search` runs. `--offline` was not needed. The limit was the board's
antenna, not the crate cache and not a missing network.

**The host half still stands, and it is the reason the board build is native.** On this dev host the
1.98.1 toolchain has only `x86_64-unknown-linux-gnu` installed, `which aarch64-linux-gnu-gcc cross`
finds neither, and the cross-build dies at **rc 101**, `error[E0463]: can't find crate for 'core'` —
the same refusal #224 recorded for this package and the fault D-018 names. That is D-018's lane (a)
working exactly as recorded: no cross-build here, a native aarch64 build on Pinkie from a `git
archive` of the head.

**The remaining limb, named.** `qualia-jepa-parity --checkpoint <candidate-dir> --target cuda` has not
run on the board: no candidate checkpoint exists in the tree (`assets/brain/prior` is a connectome
prior, and `qualia-jepa-train` needs a dataset manifest the tree does not carry), so the fixture
checkpoint the cross-lane job built is the missing piece and `ImplCrossAarch64` holds the board to run
parity against it there. The board leg above is the package's own smoke path — build and probe — and
the gradient measurement this ticket is about remains this capture's dev-host 4090 runs, because the
ch4 fixture is a 0.4 GB scratch input that the board tree does not carry. The host-only statement is
still useful for that half; the board half is now **measured**, not argued from an earlier note.

## Files

| file | bytes | sha256 |
| --- | --- | --- |
| `report_metrics.py` | 4413 | `9b259c37e143602b7b202b608214cd1270ef22239a46349afd006123cf026e49` |
| `diff_weights.py` | 2189 | `04bedbf56b81acd524ac5884025089b8a3ce7b9d37ebb1c131e55fd4b3a6e2df` |
| `before-after-weights.txt` | 8470 | `800b16f7c05beb14d4c19cabd3979cc83298b1da6a6218f52546c741571a8f02` |
| `runs/ch4-e1/report.json` | 3329 | `9a19815a0f435807a81ef889603d651e18701b4d87b051c101bc615ffab801bc` |
| `runs/ch4-e1/manifest.json` | 1750 | `9ddb0783e43fbdae2215dadf41c9b2c58f2b200f54fa9b9abb849858fe557fc4` |
| `runs/ch4-e1/run.log` | 2120 | `896a03dbf90dddca49b34338b1a3e4c46e33737feeb324fd599154cfab7f7f99` |
| `runs/ch4-e10/report.json` | 3332 | `b515fd790adb4dab8c7d5ecca60fddd3b91094d3dcf7bb9c9245d4ebdd32aa1b` |
| `runs/ch4-e10/manifest.json` | 1753 | `ae93f989a4e975ceee811a5dd17ccb809648f79514308185d66675f87b094d74` |
| `runs/ch4-e10/run.log` | 4252 | `251ad0736685e5f9ddb5ba42e304d64f8362d1ce588fc9c2c4532fdf02e50f6a` |

The report file names in the table are the reports' own SHA-256, and each `runs/<label>/report.json`
is byte-for-byte that published file (renamed): `9a19815a0f435807a81ef889603d651e18701b4d87b051c101bc615ffab801bc`
for `ch4-e1` and `b515fd790adb4dab8c7d5ecca60fddd3b91094d3dcf7bb9c9245d4ebdd32aa1b` for `ch4-e10`.

### Stays on the capturing machine

| artifact | bytes | why it is not committed |
| --- | --- | --- |
| the two after-fix `weights.safetensors` | 15 432 280 each | regenerable; their SHA-256 are above and `before-after-weights.txt` is the comparison they support |
| T52's before checkpoint (`9132e253…`) | 15 432 280 | T52's capture; cited by digest, lives in its own evidence and scratch |
| the ch4 fixture tree (sessions, catalog) | ~0.4 GB | T52's scratch input, unchanged and untouched by this capture |

## How it was taken

```sh
# one build at a time, -j 2 (D-014), in WSL2 Ubuntu-22.04
export LD_LIBRARY_PATH=/usr/local/cuda/lib64:/usr/lib/wsl/lib
export PATH=/usr/local/cuda/bin:$PATH
export CARGO_TARGET_DIR=/mnt/c/tmp/Impl53EncoderGrad/target
rustup run 1.98.1 cargo build --release -j 2 -p qualia-jepa-model --features cuda --bin qualia-jepa-train

# the fixture is T52's committed ch4 manifest, read in place and unmodified
MANIFEST=/mnt/c/tmp/Impl225PromotedE2E/fixture/wave-ch4/datasets/jepa-dataset-26e2dfac396e343f09b27b350366b24ef06f33b65f0c90ca48919ac035245b8e.json

# runs, one GPU job at a time (D-011)
qualia-jepa-train --manifest "$MANIFEST" --checkpoint-id t53-ch4-e1  --output-dir runs/ch4-e1  --backend cuda --epochs 1  --seed 42
qualia-jepa-train --manifest "$MANIFEST" --checkpoint-id t53-ch4-e10 --output-dir runs/ch4-e10 --backend cuda --epochs 10 --seed 42

# reporting
python3 report_metrics.py runs/ch4-e1/report.json runs/ch4-e10/report.json
python3 diff_weights.py <T52 ch4-e1 weights.safetensors> runs/ch4-e1/t53-ch4-e1/weights.safetensors
```

Wall clock: `98 223 ms` for `ch4-e1` (1313 steps), `392 810 ms` for `ch4-e10` (13 130 steps), both
rc 1 (the trainer exits 1 when `all_gates_passed` is false while still publishing a finite report).

One temporary instrument was added to the working tree for the runs and **reverted before this
commit**: five lines in `crates/jepa-model/src/bin/qualia-jepa-train.rs` writing the `TrainingReport`
to `$QUALIA_JEPA_REPORT_DUMP` before the finite-metric guard, so a refused run would still yield its
numbers. Both after-fix runs published, and the dumped report is byte-identical to the published one
(`cmp` clean for both), so the instrument changed no number in this capture; the committed tree does
not contain it.

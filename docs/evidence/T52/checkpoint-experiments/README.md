# T52 checkpoint experiments — can this repository publish a promotion-passing checkpoint?

Step 1 of ticket [#225](https://github.com/superposition/qualia/issues/225) (T52): the checkpoint
whose held-out gates pass, which parity and plan-eval need before either can run against a promoted
model. This capture answers with six bounded configurations — seven processes, since
`static-e1-instr` is an instrumented re-run of `static-e1` rather than a seventh configuration —
plus the gate metrics each produced and the one measurement that decides the ticket's three-way
question.

**The answer.** A promotion-passing checkpoint is **not producible today**, and the blocker is not
the amount of training data or the model's size: the effective-rank gate reads the held-out
representation of an encoder that **receives no gradient from the crate's own training objective**,
so its rank is a property of the deterministic initialization. Every other gate is reachable with a
suitable fixture — the chain-4 run passes the predictive gate and occupancy outright, its
calibration coverage and clamp fraction are inside the bands, and a smaller predictor brings the
test split's calibration fully inside — but `effective_rank` sits at 1.08–1.37 against a floor of
64 in **every** fixture and at **every** epoch count, and the encoder weights in the published
checkpoint are **byte-identical between a one-epoch and a ten-epoch run**. The *decision* half of
step 1 is therefore discharged — recorded as **D-021** (`docs/decisions.md`), which names the
measurement the repair belongs to and which **T53 ([#228](https://github.com/superposition/qualia/issues/228))** now
carries — while the *produce* half ("produce (or obtain) a checkpoint whose held-out gates pass")
and step 2 are exactly what D-021 unblocks: they stay blocked on that repair, not on data.

## Host and tools

WSL2 `Ubuntu-22.04` on the development workstation, RTX 4090 (`nvidia-smi -L` → `GPU 0: NVIDIA
GeForce RTX 4090`), `rustup` toolchain `1.98.1`, `nvcc` 12.8, `LD_LIBRARY_PATH`
`/usr/local/cuda/lib64:/usr/lib/wsl/lib`, `PATH` including `/usr/local/cuda/bin`. Every build ran
`-j 2` and one at a time (D-014); every GPU job ran one at a time (D-011); no display adapter,
driver or `pnputil` call was made (D-012). Builds: `qualia-jepa-dataset` and `qualia-jepa-model
--features cuda` (`--bin qualia-jepa-train`), both rc 0, into
`/mnt/c/tmp/Impl225PromotedE2E/target`.

## The gates a candidate must clear

From the crate itself, so the numbers below are the numbers the code asks for:

- **Predictive** (`crates/jepa-model/src/lib.rs:794-802`, `split_predictive_gate_passes`): for each
  held-out split, `tiny_cnn.transition_nll < constant.transition_nll` **and**
  `< flat_mlp.transition_nll`, and `tiny_cnn.rollout_error < constant.rollout_error` **and**
  `< flat_mlp.rollout_error`.
- **Calibration** (`crates/jepa-model/src/evaluation.rs:102-116`, `metrics_pass`):
  `mean_standardized_squared_residual ∈ [0.9, 1.1]`, `calibration_slope ∈ [0.9, 1.1]`,
  `coverage_50/90/95` each within **±0.05** of `0.5/0.9/0.95` (`centered_on`,
  `evaluation.rs:259-261`), `clamp_fraction ≤ 0.01` (`CALIBRATION_CLAMP_FRACTION_MAX`,
  `evaluation.rs:20`), `nonfinite_values == 0`.
- **Effective rank** (`crates/jepa-model/src/evaluation.rs:18,47-57`): ≥ 4096 samples, 256 dimensions,
  `effective_rank ≥ 64` (`EFFECTIVE_RANK_MINIMUM`), finite positive trace, converged.
- **Occupancy** (`evaluation.rs:217-222`): `IoU > trivial_iou` and `pr_auc > trivial_pr_auc`.
- **Publication** (`crates/jepa-model/src/lib.rs:1051-1100`): a training report or manifest carrying
  any non-finite metric is **refused by name** rather than written, because JSON would write `null`
  and the reader rejects it. A refused run leaves no report and no checkpoint; a run whose metrics
  are all finite publishes both and then exits 1 if `all_gates_passed` is false.

The gate-minimum dataset floor (`crates/jepa-dataset/src/lib.rs:285-350`) is 50 000 valid
transitions, 12 sessions, 3 conditions, 3 environments and ≥ 4096 transitions in each split. Every
fixture here clears it.

## The fixtures

The static profile is the T50/T31 recipe verbatim (a constant thumbnail, a fixed 4-ray LiDAR sweep,
a pose advancing 0.1 m per frame, one constant action) and was **not regenerated**: the four runs
use T50's own manifest, digest `e4e128e385344a9fc2da046277f6a3f24f6ab85b96343963f12f1432f76b3686`,
50 004 transitions, splits 41 670/4 167/4 167.

The wave profile (`wave-fixture.rs`, committed here) keeps the same evidence envelope but makes the
transition observable and action-conditioned: the camera carries a moving sinusoidal grating
(`0.5 + 0.30·sin(2π(x/8 + phase))·cos(2πy/16) + 0.10·cos(2π(y/12 − phase/2))`, phase stepping
0.07 · `speed_scale` per frame), LiDAR ranges and the pose oscillate in that phase, and each
session's `speed_scale` and `left`/`right` come from the session index. So the no-change baseline is
wrong (the fixture moves) and the action input carries the rate.

The wave profile also carries a **chain break**: every `C` frames the clock jumps 600 ms, past the
dataset's 500 ms `max_frame_gap_ns`, so the pair spanning the gap is rejected and the free-running
rollout restarts on the next transition. That decouples the *rollout chain length* (the number of
steps the predictor is fed its own mean) from the *session length*, and lets one session hold 4 000+
transitions with a chain of 4. Generation and manifest:

```sh
t52-fixture --root <root> --envs 12 --sessions S --frames F --chain C --phase-step 0.07 --profile wave
qualia-jepa-dataset --catalog <root>/catalog.json --output-dir <root>/datasets
```

| profile | shape | chain | transitions | manifest digest |
| --- | --- | --- | --- | --- |
| static (T50's) | 12 sessions × 4168 frames | 4167 | 50 004 | `e4e128e3…` |
| wave `A` | 12 envs × 66 sessions × 65 frames | 64 | 50 688 | `5aeeb4b47e44dd1b263c9b134b6008de76db7154a4d06165f4309963bb007dc5` |
| wave `ch16` | 12 envs × 1 session × 4448 frames | 16 | 50 040 | `02ef8261bf7bc74a1bfc3f442c8e5c144897685bf1081741e7f653c8c27191b2` |
| wave `ch4` | 12 envs × 1 session × 5601 frames | 4 | 50 400 | `26e2dfac396e343f09b27b350366b24ef06f33b65f0c90ca48919ac035245b8e` |

## The runs

Every run is the ticket's own binary on the CUDA lane, default batch size 32, seed 42:

```sh
qualia-jepa-train --manifest <manifest> --checkpoint-id t52-<label> \
  --output-dir <out> --backend cuda --epochs N --seed 42
```

`slope` is `calibration_slope`, `msr` `mean_standardized_squared_residual`, `clamp`
`clamp_fraction`; `rank` is `effective_rank.effective_rank`; all numbers are from the run's own
training report (`runs/<label>/report.json`, or `report-dump.json` where the writer refused).
"Published" means the report and the checkpoint both reached disk (the process still exits 1 when
`all_gates_passed` is false).

| run | fixture | epochs | nll cnn / flat / const | rollout cnn / flat / const | slope | msr | cov50/90/95 | clamp | rank | published |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `static-e1` | static | 1 | −4.364 / 0.355 / −4.437 | `NaN` / `NaN` / 1.720 | 3.254 | 1.116 | .481/.886/.939 | 0.710 | 1.372 | **refused** |
| `wave-e1` | wave A | 1 | −3.938 / −1.373 / −2.874 | 6.09e8 / 5.98e11 / 1.516 | 1.083 | 0.764 | .563/.942/.973 | 0.063 | 1.084 | yes, gate false |
| `ch16-e1` | wave ch16 | 1 | −3.246 / −1.457 / −2.033 | 3.743 / 20.69 / 1.492 | 0.898 | 1.694 | .448/.812/.877 | 0.006 | 1.120 | yes, gate false |
| `ch4-e1` | wave ch4 | 1 | −3.362 / −1.499 / −1.246 | **0.129** / 0.152 / 1.485 | 1.430 | 1.155 | .498/.877/.930 | 7.8e-5 | 1.120 | yes, **predictive gate passes** |
| `ch4-e10` | wave ch4 | 10 | −4.400 / −1.922 / −1.246 | **0.023** / 0.098 / 1.485 | 0.426 | 0.709 | .582/.949/.978 | 0.595 | 1.120 | yes, predictive gate passes |
| `small-e1` | wave ch4 | 1 | −3.108 / −0.329 / −1.246 | **0.223** / 0.267 / 1.485 | 1.009 | 1.146 | .503/.891/.932 | 1.0e-4 | 1.120 | yes, **test split passes all three** (temporary `PREDICTOR_HIDDEN_DIM` 512 → 64, reverted) |

Validation split shown for `static-e1`, `wave-e1`, `ch16-e1`, `ch4-e10` and `small-e1`; `ch4-e1`'s
table row is the validation split and its test split is numerically the same to three digits
(rollout 0.146 / 0.190 / 1.443, slope 1.395, clamp 6.8e-4). `small-e1`'s **test** split reads
nll −3.274 / −0.282 / −1.219, rollout 0.219 / 0.359 / 1.443, slope 0.973, msr 0.924, coverage
.519/.920/.957, clamp 3.1e-4 — predictive **and** calibration **and** occupancy all pass; its
validation split misses only `msr` (1.146 vs ≤ 1.1).

The table is the **six configurations**. The seventh process, `static-e1-instr`, is `static-e1` run
once more through the instrumented binary so that a refused run still yields its metrics; it has no
row of its own because it is not a different configuration (its epoch line and refusal are identical
to `static-e1`'s).

The refusal, verbatim (`runs/static-e1/run.log`, unmodified head binary):

```text
qualia-jepa-train: refusing to publish checkpoint t52-static-e1: non-finite training-report metric validation.flat_mlp.rollout_error = NaN; a non-finite metric means the evaluation diverged, and JSON writes it as null, which the reader rejects
```

The published-but-failed runs end with the crate's own line, e.g. `runs/ch4-e1/run.log`:

```text
candidate=t52-ch4-e1 weights=…/t52-ch4-e1/weights.safetensors manifest=…/t52-ch4-e1/manifest.json report=…/jepa-training-report-8d0f4fc4….json gate=false
qualia-jepa-train: candidate failed held-out predictive, grounding, or calibration gates
```

`ch4-e1`'s `manifest.json` carries the **test** split's three metric rows in `baseline_gate` — the
struct has no validation row (`crates/jepa-model/src/lib.rs:937-953`) — and `baseline_gate.passes()`
is true there, as is `split_predictive_gate_passes` for **both** splits. `all_gates_passed` is
nevertheless false for two further reasons (`crates/jepa-model/src/bin/qualia-jepa-train.rs:292-293`):
`grounding_calibration_gate_passed` is false (slope 1.43005 on validation and 1.39492 on test, and
`mean_standardized_squared_residual` 1.15548 and 1.11739, all outside `[0.9, 1.1]`), and
`effective_rank.passes()` is false (1.11967 against the floor of 64).

## What the runs establish

1. **The static fixture cannot publish at all** — on CUDA, `flat_mlp.rollout_error` is `NaN` at one
   epoch, exactly as T50's record has it. Its calibration is also structurally out of band: the
   residuals are so small that 71 % of dimensions sit on the log-variance clamp and the slope reads
   3.25.
2. **A non-degenerate fixture removes the divergence** — the wave profile publishes a finite
   report at one epoch and every chain length tried, so the free-running `NaN`/`inf` of the static
   fixture is a property of a *degenerate* fixture (constant thumbnail, unbounded linear pose), not
   of the trainer.
3. **Chain length is the lever on the rollout gate** — the same trainer on the same recipe reads
   rollout 6.1e8 at chain 64, 3.7 at chain 16, and **0.13 against the constant baseline's 1.48** at
   chain 4. Restarting the rollout every four steps is what makes the candidate beat the no-change
   baseline, because the baseline's own excursion shrinks with the chain while the one-step
   prediction error stays smaller.
4. **Everything except the rank gate is reachable.** `ch4-e1` passes the predictive gate and
   occupancy outright and its calibration coverage and clamp are inside the bands; `small-e1` (the
   predictor's hidden width cut 512 → 64) puts the whole test split's calibration inside
   (slope 0.973, msr 0.924, coverage .519/.920/.957, clamp 3.1e-4). The remaining miss is a single
   number — `effective_rank` — and it does not move.
5. **Training more makes the rollout better and the calibration worse, and never touches the
   rank**: 1 → 10 epochs takes `ch4-e1`'s rollout from 0.129 to 0.023 but drives `clamp_fraction`
   from 7.8e-5 to 0.595 and the slope from 1.430 to 0.426, while the rank stays 1.11967 both times.

## The measurement that decides it

`effective_rank_report` is computed over the held-out `target_latent` vectors
(`crates/jepa-model/src/bin/qualia-jepa-train.rs`, `measure_holdout`), which `predict_batch` fills
from **the EMA target encoder** (`crates/jepa-model/src/train.rs:242`, `ema_target_latent`). Two
measurements show what that representation is a function of:

- **The encoder weights do not move between 1 and 10 epochs.** `encoder-freeze-diff.txt` is
  `safetensors_diff.py` over `runs/ch4-e1/t52-ch4-e1/weights.safetensors` and
  `runs/ch4-e10/t52-ch4-e10/weights.safetensors` (same fixture, same seed): the predictor's and the
  occupancy decoder's tensors move a lot (`predictor.output.weight` max |Δ| 2.62 against a scale of
  0.51), while **all 20 `encoder.*` and all 20 `target_encoder.*` tensors differ by exactly
  `0.000000e+00`** (42 of the 50 tensors are byte-identical). The EMA target is frozen because the
  online encoder it averages is.
- **The encoder gets no gradient.** `gradient-probe.txt` is the training objective's own backward
  pass, dumped per parameter after the first step: `predictor.output.weight` 2.97e2,
  `predictor.hidden.weight` 1.57e2, `occupancy_decoder.output.weight` 1.55e2, … and **every
  `encoder.*` parameter and both fixed `adapter.*` parameters report no gradient at all** — 22 of
  the map's 30 variables, the 20 encoder parameters among them, are absent from the backward graph
  even though `is_variable()` is true for them in the map.
  The probe also shows `visible.sum_all().backward()` yields **zero** leaf variables, while mutating
  the map's `encoder.camera.conv1.weight` *does* change `visible` (4.196e-5 → 7.629e-6, storage
  shared) — so the forward reads the parameter map's storage but the graph it builds carries no
  gradient leaves for it.

Cross-check: the static fixture's rank in this capture, `1.3718248717017738`, matches T50's CPU
one-epoch report, `1.3718248721730777`, to nine significant figures while the two runs differ in
backend — the representation is the deterministic initialization, computed twice.

**Therefore**: `effective_rank` is not a measurement of anything training does. It reads 1.08–1.37
across four fixtures whose inputs differ by orders of magnitude in richness, and 1.11967 identically
at one and ten epochs; no fixture geometry and no amount of training can move it to 64 until the
encoder actually trains. This is the defect the ticket's step 1 was looking for, and it is a
measurement/wiring defect in the pipeline, not a data or capacity limit.

## The decision (three-way, on the measured basis)

Recorded as **D-021** in `docs/decisions.md`.

- **More/different training data — ruled out by measurement.** Four fixture geometries (the static
  recipe; 12 × 66 × 65 with chain 64; chain 16; chain 4) produced ranks 1.372 / 1.084 / 1.120 /
  1.120, and the 1 → 10 epoch sweep left the rank *bit-identical* (1.11967) while moving the
  encoder weights by exactly zero. Different data does fix the things that are data-shaped — the
  `NaN` rollout and the chain-length-sensitive rollout race — but not the gate that fails.
- **A smaller model — ruled out as the fix, though it is real for calibration.** Cutting the
  predictor's hidden width 512 → 64 (`small-e1`, temporary `PREDICTOR_HIDDEN_DIM`, reverted) moves
  the test split's calibration fully inside the bands (slope 1.430 → 0.973, msr 1.155 → 0.924) and
  leaves the rank at the same 1.11967. Capacity is not the blocker, and the one capacity-sensitive
  symptom — the recursive blow-up — is already removed by the chain-break fixture (rollout 0.129
  against 1.485 at chain 4).
- **A calibrated gate — this is the fix, and it is a calibration of the measurement, not of the
  threshold.** The rank gate is fed by a representation the pipeline never trains; lowering 64 to
  anything above 1.37 would be fitting a band to a broken number. The fix is to make the held-out
  representation a function of training — repair the encoder/EMA wiring measured above so the
  online encoder is in the objective's gradient — and only then re-derive the band and re-run this
  sweep. That repair is **T53 ([#228](https://github.com/superposition/qualia/issues/228))**, opened
  from this capture and carrying this measurement and the test that must fail today; the gate
  threshold itself is unchanged here, and no gate was edited by this PR. Until that repair lands,
  **no artifact this repository can publish clears the promotion gates**, and the ticket's step 2
  (parity and plan-eval against a promoted checkpoint) **remains blocked on it**; it is not blocked
  on a fixture or on training time.

**What the decision does not establish.** It does not establish that a repaired encoder clears the
rank floor, nor that it then clears calibration: `ch4-e10` shows calibration *growing worse* with
more training (`slope` 1.430 → 0.426, `clamp_fraction` 7.8e-5 → 0.595 over 1 → 10 epochs). The
sweep has to be re-run on the repaired pipeline rather than assumed green. Nor does it isolate the
mechanism: the probe measures that no gradient leaf exists for the encoder, not which detach site
produces that — #228's test is where that gap closes.

## The board leg (step 4)

Step 4's bar is recorded on its stated reason rather than attempted, because both limbs were already
measured by #224 and D-018 and neither changed here:

- **No aarch64 artifact of these binaries can be built on this host.** The dev-host WSL toolchain
  has no `aarch64-unknown-linux-gnu` target, no `aarch64-linux-gnu-gcc` and no `cross`, so the
  build dies at `rc 101` with `error[E0463]: can't find crate for 'core'` — #224's
  `docs/evidence/T50/model-eval/` records that build line, and D-018 records the same fault. No
  cross-build is claimed here; none was attempted.
- **None could run on Pinkie anyway.** The board's offline registry cache carries no `candle-core`
  and the board has no DNS to fetch one, so `qualia-jepa-train`, `qualia-jepa-parity` and
  `qualia-jepa-plan-eval` cannot be built or run there (D-016, D-018; T50's model-eval records the
  same prerequisite for its two binaries).

The ticket's DoD states the limb exactly this way — "record the aarch64 build as the board evidence
and state why execution on Pinkie is impossible" — and the whole capture is a dev-host 4090 capture
for that reason: every run above ran on this host's GPU, none on the board.

## Files

| file | bytes | sha256 |
| --- | --- | --- |
| `wave-fixture.rs` | 11191 | `13978e748fd1f69f963868a945378d0c331f2cbe8d31f8904432c6cc5eafe27c` |
| `report_metrics.py` | 4143 | `2334f9bcf7da3c7d4e53d1ec5a06e49324b01c88451b54a041643576f039dd0f` |
| `safetensors_diff.py` | 1127 | `f67d01998aefc7f2c2bc1731f235dc0453e7c694a86ab48deeb8cff88b7322c7` |
| `encoder-freeze-diff.txt` | 3025 | `693c9edf426e2cbf9b6ee9ea90e2fc75ba24c4f36ea39027f87f563308303fe4` |
| `gradient-probe.txt` | 1484 | `79d86103b262a5d77463b98d2f2ac640e3a2a7e311dd21e5375bb947f36fcb99` |
| `runs/static-e1/run.log` | 528 | `db51fb29115f0802d59772ba090dd9efa871acff5e558b1e18284036c295e13c` |
| `runs/static-e1-instr/run.log` | 534 | `e0daf667e119858ca39cdb41d43b9ea73fcb71f4f67c37a8fe66568c582fb8ab` |
| `runs/static-e1-instr/report-dump.json` | 3265 | `1fa992edcce137776e2b242fb0508dfc3bafa9957e408e017529ea45c9c05a94` |
| `runs/wave-e1/run.log` | 707 | `2bed7dce14de1f04f94e313c9668c9ba8852880e09bc0bf9d0d2258a6420bc98` |
| `runs/wave-e1/report.json` | 3308 | `aeb453c7fa8727a89258f26ff40b7a171a6d6cef872469e53893b8b818549ce1` |
| `runs/wave-e1/manifest.json` | 1751 | `62b89de6145c5df1b1893f6fe58c200e7f9265478f4f73d5910b14045ad33b08` |
| `runs/ch16-e1/run.log` | 706 | `2373386d237ab821fb3a52c0eb08a85f1aac939b3da201d42cc01f01a8637b66` |
| `runs/ch16-e1/report.json` | 3324 | `eeae3833399b1eeeeefd24b26c982f9d8d66966dd67f469a3e95f8ae383cc9d8` |
| `runs/ch16-e1/manifest.json` | 1751 | `27bb7cf849e2fcf51a9149fe5fbb6c2168bdc4a612e06532d20014301b3f3739` |
| `runs/ch4-e1/run.log` | 701 | `34165ff65f3c4c23a9c989f34a47fec5e78779259fcb267ab77e128850a83e76` |
| `runs/ch4-e1/report.json` | 3318 | `8d0f4fc4cdb4976e501837346d3f39765b662a57a7afb729c534f2dfdcbae021` |
| `runs/ch4-e1/manifest.json` | 1751 | `22da2c5aafa8103be4c72393a915c0f3bfa44ad0a15dd524f2559c00e9966ee5` |
| `runs/ch4-e10/run.log` | 2842 | `35271229c56044a64b2f6fee0459d6957945c3c94fb9e1612408e4d392fe29b9` |
| `runs/ch4-e10/report.json` | 3324 | `29f68c125634d1f2a7fe726ab5fe4583a0c9cda3c85757ec3ae1c30258258632` |
| `runs/ch4-e10/manifest.json` | 1752 | `95278f81c3a6c7b719a1de101aff346f81d5ab8e48bb27332c836fff96a34b71` |
| `runs/small-e1/run.log` | 712 | `9b8f0dd15ae5030d5e5dd896f0b6cbc61807a8e436242688b0870562eb852d8c` |
| `runs/small-e1/report.json` | 3309 | `c2a86e4a15ab4c36b6105ab7925111b5c106020cf1c19dad15933ba07f36ec97` |
| `runs/small-e1/manifest.json` | 1752 | `6500dc97511e61b579b19cbb37a39ae1a885220f4b28efd69f725c69c43ff26c` |

`report.json` for `wave-e1`, `ch16-e1`, `ch4-e1`, `ch4-e10` and `small-e1` is the candidate's own
report, byte-for-byte the file the trainer published (its name is its SHA-256), and `manifest.json`
is the candidate checkpoint manifest beside it. The refused runs published **nothing** — no report
and no checkpoint — so `runs/static-e1/` holds the log alone, and
`runs/static-e1-instr/report-dump.json` is the same `TrainingReport` captured just before the
finite-metric guard refused it.

### Stays on the capturing machine

| artifact | bytes | why it is not committed |
| --- | --- | --- |
| the five published `weights.safetensors` | 15 432 280 each | regenerable and 15 MB each; their SHA-256 are below, and `encoder-freeze-diff.txt` is the comparison they support |
| the four fixture trees (sessions, catalogs) | ~0.4 GB | scratch inputs; every shape and digest is in the table above |

Weights digests: `ch4-e1` `9132e253fc81fabce572bc08b8d208a8316a2be4f74088530aefd4ad83825d67`,
`ch4-e10` `3118e2648254b2e0970010eb5f668b512c42c9b3419c05a035cae766897f8a49`,
`small-e1` `58127205ad7bdf48d5766c10ac4b7d380e50ae35126ef793492df0834551f1c5`,
`wave-e1` `71ce248f9af213b9e7847aad0e110cf3a27c8d8dd648fc6b2e83a0c4992de95d`,
`ch16-e1` `cc8c02fb40d041629b608557b33c9a3c1c861cdaf14a24aafbcaedba523bfa9c`.

`run.log` is the process's stdout+stderr with the per-session `jepa materialize …` lines removed
(~1 500 lines in the 792-session `wave-e1` run); every `jepa train epoch=…`, `jepa calibrate …`,
`candidate=…` and refusal line is kept verbatim, and the files those lines name are here.

## How it was taken

```sh
# one build at a time, D-014
cargo +1.98.1 build --release -j 2 -p qualia-jepa-dataset --bin qualia-jepa-dataset
cargo +1.98.1 build --release -j 2 -p qualia-jepa-model --features cuda --bin qualia-jepa-train

# the fixture writer: a scratch crate (scratch/Cargo.toml below) with path deps on the
# worktree's crates/mcap-log and crates/jepa-dataset, built with the same rule
cd scratch && cargo +1.98.1 build --release -j 2

# fixtures (scratch): wave A, ch16 and ch4 as tabulated
t52-fixture --root <root> --envs 12 --sessions 66 --frames 65   --chain 0  --profile wave
t52-fixture --root <root> --envs 12 --sessions 1  --frames 4448 --chain 16 --profile wave
t52-fixture --root <root> --envs 12 --sessions 1  --frames 5601 --chain 4  --profile wave
qualia-jepa-dataset --catalog <root>/catalog.json --output-dir <root>/datasets

# runs, one GPU job at a time, D-011
qualia-jepa-train --manifest <manifest> --checkpoint-id t52-<label> --output-dir <out> \
  --backend cuda --epochs <N> --seed 42
```

The scratch crate is two files, both quoted here:

```toml
[package]
name = "t52-fixture"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "t52-fixture"
path = "src/main.rs"

[dependencies]
serde_json = "1"
qualia-mcap = { path = "<worktree>/crates/mcap-log" }
qualia-jepa-dataset = { path = "<worktree>/crates/jepa-dataset" }
```

`wave-fixture.rs` beside this README is that crate's `src/main.rs`. The copy that wrote the fixtures
hashes `73a39ed39b5b40151f2a0d8340b7cba6212ef4169609f91fe02fea3e2e75f6a3`; the committed copy
differs from it by exactly the header sentence that used to read "Lives in scratch, never in the
repository" (now "committed beside the capture as the recipe's evidence"), because a file that is in
the repository must not say it is not. No other byte changed, and the fixtures were generated before
that edit.


Three temporary instruments were added to the working tree for measurement and **reverted before
this commit**. The diff is documentation plus three scratch sources quoted here: the fixture writer
(`wave-fixture.rs`) and the two readers below.

- `crates/jepa-model/src/bin/qualia-jepa-train.rs` — five lines writing the `TrainingReport` to
  `$QUALIA_JEPA_REPORT_DUMP` before the finite-metric guard, so a refused run still yields its
  numbers. The epoch lines it prints are identical to the unmodified binary's (`static-e1` vs
  `static-e1-instr`: `cnn_total=21.618604 cnn_nll=-3.781032 … flat_total=478.954797`, and the same
  refusal line).
- `crates/jepa-model/src/lib.rs` — `PREDICTOR_HIDDEN_DIM` 512 → 64 for `small-e1` only.
- `crates/jepa-model/src/train.rs` — the gradient and storage probes that produced
  `gradient-probe.txt`. They print and then return an error, so no probe run published anything and
  the returned error is the only behaviour they add.

The `small-e1` run therefore carries a smaller predictor than `main`'s; its manifest still declares
`architecture_id: qualia.jepa.grounded-tiny-cnn.v1`, which is why it is recorded as an experiment
and not as a candidate.

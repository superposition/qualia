# T52 promoted-e2e

Ticket [#225](https://github.com/superposition/qualia/issues/225) (T52), the promoted-model end-to-end
run. Two things live here, and they are different kinds of result:

- **§1, the manifest-digest blocker** — the measured defect that stood between the real capture
  (`t65-real-01`, #250/#256) and *any* training run, and its fix. This is a defect the ticket did not
  expect and it is on the critical path of [#239](https://github.com/superposition/qualia/issues/239)
  too: the trainer refused the manifest the dataset binary had just written.
- **§2, step 1's decision** — the promotion floor stands: the real corpus is 1 499 valid transitions
  in one session / one environment / one condition against 50 000 / 12 / 3 / 3, so the fix is
  more/different data, recorded as D-027.
- **§3, step 2** — parity and plan-eval captures against the best available checkpoint, plainly
  labelled **not promotion-passing**; the reason no promoted one exists is §2's.
- **§4, step 3** — the F1 refactor, merged as PR #226, and this ticket's re-run of its test and path.
- **§5, step 4** — the board leg, with a live `--target cuda` run on Pinkie (rc 0) and the four
  DoD facts quoted.


## 1. The trainer refused the manifest the dataset had just written

`t65-dataset.sh` ran, on the board, `qualia-jepa-train --manifest …/jepa-dataset-6f98b125….json
--epochs 1 --batch-size 32 --seed 65` against the manifest `qualia-jepa-dataset` had written seconds
earlier from the real capture. It printed, before the promotion gate:

```text
qualia-jepa-train: dataset manifest is unsupported or has an invalid digest
TRAIN_RC=1
```

`preflight` (`crates/jepa-model/src/bin/qualia-jepa-train.rs:354-360`) then compares
`dataset.schema_version` to `"qualia.jepa-dataset.v3"` — which matches, measured — and `dataset.digest`
to `qualia_jepa_dataset::manifest_digest(dataset)`, which **re-serializes the parsed manifest**
(`crates/jepa-dataset/src/lib.rs:738-744`). The write path (`write_immutable_manifest`,
`lib.rs:710-716`) does not catch this: it re-checks the digest of the *in-memory* struct, where no
parse has happened yet.

### The measured cause: a one-ulp float parse in the audit

The manifest is faithful. Recomputing it in Python — SHA-256 over the compact JSON with `digest`
cleared, the same algorithm — reproduces the stored digest exactly:

```text
$ python digest_probe.py                     # the board's file, 3 401 416 B
schema      : qualia.jepa-dataset.v3
stored      : 6f98b125196ce48501410726309fa6c8b10b21ab4f1b67e0f61297851836b58d
samples     : 1499
python digest: 6f98b125196ce48501410726309fa6c8b10b21ab4f1b67e0f61297851836b58d
match        : True
```

So the file is self-consistent and the mismatch has to be in Rust's parse→serialize round trip. A
probe over the same file with the crate itself (`t52-digest-probe`, a scratch binary that parses with
`DatasetManifest`, prints both digests, and writes the re-serialized bytes) is where it comes apart:

```text
$ t52-digest-probe t65-manifest.json rust-compact.json      # before the fix
file_bytes=3401416
schema=qualia.jepa-dataset.v3
schema_expected_match=true
stored=6f98b125196ce48501410726309fa6c8b10b21ab4f1b67e0f61297851836b58d
recomputed=472b090bfd559bfbc53eb01014b52068cbe796f1529f96b5b8421115e86189c2
digest_match=false
compact_bytes=2426780
roundtrip_stable=true
recomputed2=472b090bfd559bfbc53eb01014b52068cbe796f1529f96b5b8421115e86189c2
```

Comparing the file with Rust's re-serialization leaves exactly two differences, and only one of them
is a value:

```text
('.digest', 'token', '6f98b125…', '')                       # cleared for hashing, as designed
('.audit.mean_sensor_skew_ns', 'token', '29192462.559039358', '29192462.55903936')
float tokens only in file:          [('29192462.559039358', 1)]
float tokens only in rust-compact:  [('29192462.55903936', 1)]
```

The two values are **one ulp apart** (IEEE-754 bit patterns `4718601502228337473` and
`4718601502228337474`). `audit.mean_sensor_skew_ns` is `f64`; the writer emitted its shortest form
(`29192462.559039358`, 17 significant digits) and `serde_json`'s default, best-effort float parser
read that literal back **one ulp low**, so the re-serialization is a different number and the digest
of the re-serialization is a different digest. `serde_json`'s `float_roundtrip` feature exists for
exactly this ("use sufficient precision when parsing fixed precision floats from JSON to ensure that
they maintain accuracy when round-tripped through JSON"); with it enabled the same probe reads:

```text
$ t52-digest-probe t65-manifest.json rust-compact2.json     # after the fix
recomputed=6f98b125196ce48501410726309fa6c8b10b21ab4f1b67e0f61297851836b58d
digest_match=true
compact_bytes=2426781
```

Why T58's manifest passed and this one did not: T58's audit carried an all-zero (`0.0`) mean skew and
no samples, so there was no "hard" 17-digit literal to mis-round. The measurement that separates them
is the value, not the schema and not the writer.

### The fix

`crates/jepa-dataset/Cargo.toml` and `crates/jepa-model/Cargo.toml` enable `serde_json`'s
`float_roundtrip` feature. No threshold moves, no wire field changes, and no manifest's own digest
changes: the writer's digest is computed from memory, and with the feature the reader now reproduces
it. The candidate manifest and the training report are read back by parity and the planner through the
same parser, on the same critical path, which is why the feature is set on both crates.

The regression test is the existing `manifest_round_trips_through_json_with_the_same_digest`
(`crates/jepa-dataset/src/tests.rs`), extended with the value the real capture produced. Falsified by
removing the feature again:

```text
---- tests::manifest_round_trips_through_json_with_the_same_digest stdout ----
thread '…' panicked at crates/jepa-dataset/src/tests.rs:206:5:
assertion `left == right` failed
  left: 29192462.55903936
 right: 29192462.559039358
test result: FAILED. 0 passed; 1 failed; 0 ignored; 8 filtered out; finished in 0.01s
```

and with the feature: `test result: ok. 1 passed; 0 failed; … 8 filtered out`.

### What the fix unblocks

The same trainer, rebuilt with the fix, on the board's own manifest:

```text
$ qualia-jepa-train --manifest /mnt/c/tmp/Impl225PromotedE2E/board/t65-manifest.json \
    --checkpoint-id t52-t65-real-e1 --output-dir train-out --backend cuda --epochs 1 --batch-size 32 --seed 65
qualia-jepa-train: dataset does not meet the 50k/12-session/3-condition/3-environment gate
TRAIN_RC=1
```

The digest step now passes and the refusal is the promotion gate the ticket named — the same message
`qualia-jepa-dataset` itself prints. §2 is the measurement that message carries.

### Files

| file | bytes | sha256 |
| --- | --- | --- |
| `digest-fix/probe-before.txt` | 1237 | `e91ff2813befbd5f45bbb3a53efabae8fc22422bf48e966f2ffba4039f57bad7` |
| `digest-fix/probe-after.txt` | 521 | `51de57e07b12096b98c6e726b094701a9e88cc8c51a86f3cd40c87638e13e465` |
| `digest-fix/trainer-before.txt` | 671 | `e26ca1dbbc62a6e18474765b50692923f61f596d24cd9667cff672b3cbb5d812` |
| `digest-fix/trainer-after.txt` | 939 | `1ec85a981a4dacfcd9d82808a5c27bd738fc58c7c1f4204442183bd3ca4dc6dd` |
| `digest-fix/test-falsification.txt` | 973 | `1e524abd599b9db06b6f501ee676c8a3d56e00672b37c6e77c5a1e577b4f85ca` |

The board's manifest is not committed (3.4 MB, and it is the board's own artifact):
`/home/jetson/t65-capture/t65-real-01/dataset/jepa-dataset-6f98b125….json`, sha256
`2fba5dc830354d6167e07a507b3b15001bb93aa144fbdafba4f9d19fa911cec0`.

### Reproduce

```sh
# the scratch probe (its two files are committed in digest-fix/)
cargo build --release -j 2
t52-digest-probe t65-manifest.json rust-compact.json
# the falsification, without the feature
cargo test --release -j 2 -p qualia-jepa-dataset manifest_round_trips
```

## 2. Step 1's decision: the promotion floor stands, and the fix is more data

With §1's defect fixed, the same trainer on the board's own manifest reaches the gate the ticket
named — and the gate refuses the corpus:

```text
$ qualia-jepa-train --manifest /mnt/c/tmp/Impl225PromotedE2E/board/t65-manifest.json \
    --checkpoint-id t52-t65-real-e1 --output-dir train-out --backend cuda --epochs 1 --batch-size 32 --seed 65
qualia-jepa-train: dataset does not meet the 50k/12-session/3-condition/3-environment gate
TRAIN_RC=1
```

The numbers on both sides, from the manifest's own audit:

| | `t65-real-01` | the floor (`validate_promotion_audit`, `crates/jepa-dataset/src/lib.rs:279-350`) |
| --- | --- | --- |
| valid transitions | **1 499** | ≥ 50 000 |
| sessions | **1** | ≥ 12 |
| environments | **1** | ≥ 3 |
| conditions | **1** (`bench-static`) | ≥ 3, each ≥ 5 % of valid transitions |
| splits | **train 1 499 only** | train, validation and test each non-empty, ≥ 4 096 each |

`assign_environment_splits` holds out nothing below three environments
(`crates/jepa-dataset/src/lib.rs:1464-1468`), so this capture has no held-out split at all and
neither the ≥ 4 096-per-split rule nor the effective-rank gate's own ≥ 4 096-sample requirement can
be met by 1 499 samples however they are split. The shortfall is **33.4× (50 000 / 1 499) plus the
session / environment / condition diversity**.

**The decision is more/different data**, recorded as **D-027** (`docs/decisions.md`) with both sides
of the numbers. The gate is **not** calibrated downward: the 3-environment / 3-condition floor is
the leakage boundary the promotion gate exists for, and there is no measurement here to
recalibrate (unlike D-021's rank case); a smaller model cannot touch a corpus floor and D-021
already ruled it out as the rank fix. What closes it is an operations programme — order 34× the
accepted transitions across ≥ 12 sessions in ≥ 3 environments under ≥ 3 conditions, recorded by the
producers #250 landed. On this robot that means the robot being driven in more places, and the drive
belongs to the leash (D-024/D-025): this session's transport ledger records a zero-speed stop, not
motion.

## 3. Step 2: parity and plan-eval, against the best available checkpoint

No promotion-passing checkpoint exists (§2), so the ticket's step 2 — parity and plan-eval against a
**promoted** model — cannot be run as written. What is here is the same two paths against the
repository's parity-fixture candidate, regenerated from #172's recipe by `t52-e2e-candidate`; its
weights sha256 is `a390d36db7284ca0249386f50aaa3c4da09e73b6e4dd0bae42363d43cfceff58`, the same
bytes T50's capture loaded. It is **not promotion-passing** and this is **not the ticket's step-2
result**: the gate block is synthetic, so the numbers measure the runtime paths, not a promoted
model. The decision entry carries the reason.

Unit: **milliseconds**. Lane: **CUDA** on the dev host (WSL2, RTX 4090, rustup 1.98.1), one GPU job
at a time (D-011); F2 is CPU-only, as its step does no device work.

| # | path | before (#172/#224, main) | now (plain, three runs) |
| --- | --- | --- | --- |
| E | `qualia-jepa-parity --target cuda` | 934 ms median process (879 / 934 / 1046) | 981 ms (1041 / 981 / 934) |
| F1 | `propose` — 64 candidates × 16 steps | 462 ms step (461.2 / 461.9 / 532.2) | **359.3 ms median step** (345.2 / 359.3 / 362.0); process 1 147 / 1 068 / 1 089 |
| F2 | `compare` — 120 decisions | 39 ms process (39 / 39 / 40); step 0.052 ms | 43 ms median process (45 / 42 / 43); step 0.0431–0.0529 ms |

F1 is the row step 3 moved: 1 024 `predict_step` calls became 16 batched calls, and the step fell
**462 → 359.3 ms (−102.7 ms, −22.2 %)**. F1's own capture for that refactor measured a 322.6 ms
median on the same recipe (`docs/evidence/T52/planner-batch/`); both numbers are the batched
planner, and the spread is host/clock state between capture sessions. E and F2 are within
run-to-run spread of #224's numbers, which is what their captures say: E's cost is process start-up
and CUDA module load — the step itself is milliseconds — and F2's is probe start-up around a
tens-of-microseconds comparison. The nsys kernel mix under F1 is
launch-for-launch the planner-batch capture's: 18 480 launches, 28 705.2 µs of kernels,
`copy2d_f32` 6 192, `badd_f32` 4 096, `gemv2T` 1 024 + 1 024, `gemvx` 2 048, `urelu_f32` 2 048,
`bmaximum_f32` / `bminimum_f32` 1 024 each.

The binaries' own refusal is unchanged and is quoted — `qualia-jepa-plan-eval` checks provenance
before its step, and the fixture has no training report:

```text
$ qualia-jepa-plan-eval propose --checkpoint …/t52-e2e-fixture --request …/propose-request.json \
    --output plan-eval-propose.json --enable-proposals --backend cuda
Error: Os { code: 2, kind: NotFound, message: "No such file or directory" }
PLAN_EVAL_PROPOSE_RC=1
$ qualia-jepa-plan-eval compare --input …/comparison-input.json --dataset …/t65-manifest.json …
Error: Os { code: 2, kind: NotFound, message: "No such file or directory" }
PLAN_EVAL_COMPARE_RC=1
```

So F1 and F2 are measured through `t52-e2e-probe`, T50's committed scratch source, which calls the
same public functions the binary calls (`evaluate_rollout_proposals`, `compare_offline_planners`)
on the same runtime; E is the ticket's own binary end to end.

## 4. Step 3: the F1 refactor — merged as PR #226, values bit-identical

Step 3 is discharged on `main` by PR [#226](https://github.com/superposition/qualia/pull/226)
(`c0a299e`, merged `b68b946`), and its own capture lives in
`docs/evidence/T52/planner-batch/`: `evaluate_rollout_proposals` now walks the proposal set one
*step index* at a time, so a 64 × 16 walk makes **16 calls instead of 1 024**, the D2H readbacks fall
**3 072 → 16**, and the step reads **453.6 → 322.6 ms (−28.9 %)** with zero change in compute kernel
launches (12 288 either way). The emitted proposal is **byte-identical** — before and after
`RolloutProposal` both sha256 `32c4eaef74418b3d71741b025de0ef9c4bfdf1ae67b6191fde5ea996c8df7d24` —
and the ticket's test `the same emitted values across the batched planner step`
(`crates/jepa-model/src/planner.rs`) covers it, falsified once by reversing the batch's
row-to-candidate mapping.

This ticket re-ran the F1 path on the current tree for §3 (359.3 ms median step; same kernel mix) and
re-ran that test:

```text
$ cargo test --release -j 2 -p qualia-jepa-model the_same_emitted_values_across_the_batched_planner_step
test planner::tests::the_same_emitted_values_across_the_batched_planner_step ... ok
test result: ok. 1 passed; 0 failed; …
```

## 5. Step 4: the board leg

Step 4's stated reason — "the model binaries need `candle-core`, which the board's offline registry
cache does not carry, so record the aarch64 build as the board evidence and state why execution on
Pinkie is impossible" — was **measured false** after provisioning (D-022), and the ticket's own
clause is "if that has changed, run it". It is run here, live, on a fixture checkpoint scp'd from the
dev host (weights sha256 `a390d36d…`, the same bytes as the dev-host capture):

```text
$ LD_LIBRARY_PATH=/usr/local/cuda-12.9/compat \
    /home/jetson/qualia-t53/target/release/qualia-jepa-parity \
    --checkpoint /home/jetson/t52-board-ck/t52-e2e-fixture --target cuda --output /home/jetson/t52-board-parity.json
BOARD_PARITY_RC=0 WALL_MS=900
checkpoint_id t52-e2e-fixture
weights_sha256 a390d36db7284ca0249386f50aaa3c4da09e73b6e4dd0bae42363d43cfceff58
reference_backend cpu target_backend cuda
single_step.evidence rmse=1.824e-07 max_abs=6.557e-07 cosine=0.9999999999999308 passes=True
eight_step.predicted_mean rmse=5.198e-06 max_abs=2.670e-05 cosine=0.9999999999998835 passes=True
reference_latency p50_us 2043
target_latency p50_us 865
outputs_finite True passes True
```

The four facts, in the DoD's order:

- **Build.** The aarch64 artifact is the board's own native build (`qualia-jepa-parity`
  10 762 424 B). D-022 measured `RUSTFLAGS="-C target-feature=+fp16" cargo build --release -j 2 -p
  qualia-jepa-model --bins` at 7m 19s feature-off and `--features cuda` at 5m 30s (candle-core /
  candle-nn 0.9.1, candle-kernels 0.9.2 with `nvcc` for sm_87); `docs/evidence/board/readiness/`
  records the same package's aarch64 binaries and a native `BUILD_FP16_RC=0` at 5m 26s.
- **Run.** The CUDA target passes on the device: RMSE 1.8e-7…8.0e-6 against the 2e-4 threshold,
  `outputs_finite: true`, P50 865 µs against the CPU reference's 2 043 µs. (The readiness capture's
  runtime probe reads 3 070 µs CPU / 3 376 µs CUDA; D-022's 30-iteration run reads 3 168 / 2 039 µs.)
- **Reason the old clause is refuted.** The board's crate cache is complete for the head and
  `cargo fetch` needs nothing; the board reaches crates.io through the host's CONNECT proxy
  (`HTTPS_PROXY=http://192.168.55.100:8085`). `candle-core` was never missing *after provisioning* —
  D-022 records that measurement, and this run is what it implies.
- **What does not work.** The feature-off binaries on the board refuse the accelerator target:
  `qualia-jepa-parity: backend cuda is not compiled into this binary` (rc 1, from
  `~/qualia-deploy/T65/target/release/`), which is why the run above uses the `--features cuda`
  build at `~/qualia-t53/target/release/`. No dev-host cross-build is claimed: the WSL toolchain has
  no `aarch64-unknown-linux-gnu` target and dies at rc 101 (`error[E0463]: can't find crate for
  'core'`), so the aarch64 record is the board's own.

## 6. What remains

- The corpus §2/D-027 names: ≥ 50 000 accepted transitions across ≥ 12 sessions in ≥ 3 environments
  under ≥ 3 conditions. Step 2 as the ticket writes it (parity and plan-eval against a promoted
  checkpoint) is blocked on exactly that, and on nothing else.
- A re-run of the T53 sweep on that corpus: with the encoder repaired (#228) the rank moved
  `1.11967 → 1.20479` at one epoch and `1.45009` at ten on the synthetic ch4 fixture, against the
  floor of 64 — the repaired encoder's rank on real imagery is unmeasured.
- Any other read path for these artifacts should get `serde_json/float_roundtrip` with them; #239's
  board training run is the next one that needs it.

### Files

| file | bytes | what it is |
| --- | --- | --- |
| `parity-plain.txt` | 2 672 | the E/F1/F2 timing table, unit and lane named |
| `parity-report.json` | 2 633 | E's own report (all heads `passes: true`) |
| `propose-report.json`, `propose-plain.jsonl` | 231, 691 | F1's probe report and the three plain runs |
| `compare-report.json`, `compare-plain.jsonl` | 1 030, 3 090 | F2's probe report and the three plain runs |
| `capture-E.json`, `capture-F1.json` | 1 083, 1 196 | the `mage profile-exec --backend nsys` capture manifests |
| `E-kernels.csv` | 33 429 | E's per-launch kernel rows (202 launches captured) |
| `F1-kernels-aggregate.csv` | 1 097 | F1's per-kernel projection (18 480 launches → 8 kernels) |
| `board-parity-run.txt`, `board-parity-report.json` | 4 752, 2 654 | the live Pinkie run and its report |

`E`'s launch-level kernel rows are committed because the file is 33 KB; F1's are 2.9 MB and stay on
the capturing machine (the aggregate answers the same question), exactly as T50's capture does.


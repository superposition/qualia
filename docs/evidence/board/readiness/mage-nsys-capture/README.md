# Board `mage profile-exec` capture — the JEPA runtime probe under `nsys` on Pinkie

This directory is the **readiness proof** of ticket
[#230](https://github.com/superposition/qualia/issues/230)'s `mage`+`nsys` capability, not a
`needs:profile` ticket's capture: it is the measured proof that the board's fresh `nsys` 2024.5.4 and
`mage` 0.1.0 installs produce a real board-side `mage profile-exec` capture with CUDA kernels in it —
the path the audit believed was unavailable ("mage is not installable there"). `docs/evidence/README.md`
§Layout defines this case (`board/readiness/<slug>/`, exempt from the `T<NN>/<slug>/` path because no
ticket's definition of done rests on it). The checklist and the installs are in
[`../README.md`](../README.md); the shapes below are the readiness check's, the fixture is the
probe's, and no ticket's numbers are claimed here.

## Host

- **Run on** Pinkie, the Waveshare-carried Jetson Orin NX (D-010): `aarch64`, kernel
  `5.15.148-tegra`, driver `NVRM 540.4.0`, toolkit CUDA `12.9.41`, compute capability `8.7`,
  board clock `2026-09-02T14:38Z` (this capture predates the clock correction of 2026-09-12 —
  `../README.md` §"Fixed on 2026-09-12").
- **Tooling installed on the board for this run**: `nsys`
  `2024.5.4.34-245434855735v0` at `/usr/local/bin/nsys`, `mage` 0.1.0 at
  `/home/jetson/.local/bin/mage` (`pip3 show mage` → `Version: 0.1.0`) with `torch 2.14.0+cpu`,
  `triton 3.8.0` and `numpy 2.2.6` from `aarch64` wheels.
- **GPU shared**: `llama-server` (`--n-gpu-layers 99`, pid 1304) has been resident since board clock
  `2026-09-01T16:56`, so this is a shared-GPU capability run, not a controlled measurement.

## Invocation

```console
$ export LD_LIBRARY_PATH=/usr/local/cuda-12.9/compat
$ mage profile-exec --backend nsys --capture-range all \
    --output-dir /home/jetson/readiness/mage-out-cuda3 \
    -- /home/jetson/qualia-t53/target/release/qualia-jepa-runtime-probe --backend cuda --iterations 5 --warmup 1
MAGE_EXEC_CUDA_RC=0 SECONDS=8
```

`LD_LIBRARY_PATH` is required for any CUDA target on this board (the toolkit is newer than the
driver; D-016). Under mage, `nsys` ran:

```text
/usr/local/bin/nsys profile --trace=cuda,nvtx --sample=none --cpuctxsw=none --stats=false \
  --export=sqlite --output=<output-dir>/mage-nsys-doiq0b7c/capture <binary> --backend cuda …
```

and the probe's own JSON in `process.log` reported `synchronized_latency_p50_us` 3376, p95 and max
3624, `outputs_finite: true` and `output_dimensions [256, 256, 256, 1024, 4096]`.

## Shape

**494 CUDA launches** across the probe's whole fixture build (its `curand` seeding and the
`candle` elementwise/GEMM work) plus the one warmup and five timed iterations. The largest groups:
`badd_f32` 90, `urelu_f32` 66, `copy2d_f32` 66, `ucopy_f32` 36, `affine_f32` 32,
`ampere_sgemm_32x128_tn` 24, `im2col_f32` and `im2col1d_f32` 18 each, the two
`void gen_sequenced<rng_config<curandStateXORWOW, …>>` seeding kernels 14 each, `gemvx` and
`fast_sum_f32` 12 each, and the remaining rows the probe's smaller `f32` elementwise kernels
(`usqr_f32`, `usqrt_f32`, `bdiv_f32`, `bmul_f32`, `bsub_f32` and siblings). `mage` parsed all 494 out
of the export and printed its table on the board.

## Files

| File | Contents |
| --- | --- |
| `capture.json` | mage's manifest — `backend`, `argv`, `profiler_argv`, `command`, `capture_range`, `status: "complete"`, `returncode: 0`, `kernel_count: 494` — with the absolute output path elided to `<output-dir>/mage-nsys-doiq0b7c` and the target to `<binary>`, as `docs/evidence/README.md` requires. |
| `capture.sqlite` | The `nsys` SQLite export, **trimmed** to the two tables mage's `nsys` backend reads (`StringIds`, `CUPTI_ACTIVITY_KIND_KERNEL`), with the 326 `StringIds` rows the kernel table does not reference dropped (32 of 358 kept — the raw export also interns the host's `PATH`, `HOME`, distro and session names) and `VACUUM`ed: 49 152 B from 884 736 B. It parses to the same 494 launches. |
| `kernels.json` | mage's own 494 parsed rows, unmodified — 459 131 B (larger than the tree's `Cargo.lock` at ~248 KB, and inside what the evidence tree already carries: `T50/model-step`'s `kernels.json` files are 740 800 and 782 910 B, `T52/planner-batch`'s exports 1.09 and 1.31 MB), committed whole because the convention commits `kernels.json` always and a hand-normalised projection would no longer be mage's reader output. |
| `kernels.csv` | The same rows as CSV (66 544 B). |
| `README.md` | This file. |

The untrimmed export, the `nsys` report (`capture.nsys-rep`, 228 004 B) and mage's `process.log`
stay on the capturing machine at `/home/jetson/readiness/mage-out-cuda3/mage-nsys-doiq0b7c/`, per the
convention's "keep `--output-dir` outside the tree" rule.

## What this capture does not establish

- **Nothing about a ticket's kernel.** The fixture and iteration counts are the probe's defaults for
  this readiness run; a `needs:profile` ticket takes its own capture, with its own `--output-dir`
  and slug.
- **No timeline analysis.** `--trace=cuda,nvtx` with `--sample=none` records launches, not a CPU
  timeline; T35/T31's CPU-timeline carve-out is a different capture.
- **No controlled numbers.** One run, ncu/nsys clock control left at defaults, on a GPU shared with
  `llama-server`; the durations in `kernels.json` are mage's read of that run.
- **Not the ncu path.** `--backend ncu` still hits the `--page raw`/details-page mismatch D-017
  records; this capture is the `nsys` backend, which works.

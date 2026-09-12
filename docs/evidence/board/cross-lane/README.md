# Cross lane: a host cross-build, run on Pinkie

This directory is board evidence, not a profiler capture, which is why it is not under `T<NN>/<slug>/`
and commits no `capture.json`, `kernels.json` or backend export: nothing in it launches a CUDA kernel.
It records the other half of what [`docs/agents.md`](../../../agents.md) §Definition of done calls
"build here, copy the artifact there, run it there" — the cross-build half that D-018 measured as
unavailable and whose amendment (in [`decisions.md`](../../../decisions.md)) restores.

Refs [#230](https://github.com/superposition/qualia/issues/230) (*Board readiness and retroactive DoD
remediation*): this directory backs that ticket's board-readiness claim, that an architecture-dependent
artifact can be built on the dev host and exercised on Pinkie without the board being the build farm.

## Scope

**Covered:** the board's own target, `aarch64-unknown-linux-gnu`, for `qualia-jepa-model`'s binaries —
so an artifact this recipe builds is the one DoD's "built for the robot's architecture" expects, and it
is exercised on Pinkie by the ticket's own smoke path.

**Not covered:** the host-only case DoD names. `--backend metal` (`crates/jepa-model`'s `metal`
feature, the macOS backend) is untouched by this recipe and still records its aarch64 build as its
board evidence, with execution on the board impossible. `--features cuda` is also out of scope for the
reasons in §"What still blocks the model steps on the board".

## Host

Two machines, because the point of the exercise is that they are different:

- **Built on** the dev host's WSL2 Ubuntu-22.04 guest — `x86_64`, kernel
  `5.15.167.4-microsoft-standard-WSL2`, `rustup 1.29.1`. It carries no `aarch64-linux-gnu-gcc`, no
  `zig` before this recipe ran, no `cross`, and no Docker engine; `sudo` needs a password. Nothing in
  the recipe asks for root.
- **Run on** Pinkie, the Jetson Orin NX at `jetson@192.168.55.1` (D-010): `aarch64`, Ubuntu 22.04.5
  LTS (jammy, JetPack 6), kernel `5.15.148-tegra`, `ldd (Ubuntu GLIBC 2.35-0ubuntu3.12) 2.35`. Its
  clock reads 2026-09-02 against the dev host's 2026-09-11 — D-010's skew, unchanged.

## Build

At `60beee4f30fb473c8d1222cd0b57b9d3be466581`, from a `git archive` of that head (not a working
copy — D-010's CRLF warning), on the WSL2 guest. Two prerequisites come from outside the tree:
`rustup`, and **`uv`**, which the third line needs and which rustup does not supply. The guest carries
`uv 0.6.11`; on a machine without it, upstream's rootless installer is
`curl -LsSf https://astral.sh/uv/install.sh | sh` — that is the project's documented route, not a step
measured here, because this guest already had `uv`.

```bash
rustup toolchain install 1.94.1 --profile minimal --target aarch64-unknown-linux-gnu
rustup toolchain install 1.98.1 --profile minimal          # cargo-zigbuild's rust-version is 1.88
uv tool install ziglang==0.16.0                            # the wheel ships the compiler; no root
ln -sf "$HOME/.local/share/uv/tools/ziglang/bin/python-zig" "$HOME/.local/bin/zig"
cargo +1.98.1 install cargo-zigbuild --version 0.23.4 --locked -j 2

export CARGO_TARGET_DIR="$HOME/scratch/target-cross"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C target-feature=+fp16"
cargo +1.94.1 zigbuild --release --target aarch64-unknown-linux-gnu.2.17 \
  -p qualia-jepa-model --bins -j 2
```

`Finished 'release' profile [optimized] target(s) in 2m 59s`, no errors, at `-j 2` (the host guard,
D-014). Versions: rustup 1.29.1, rustc 1.94.1 (`e408947bf 2026-03-25`) building the target, zig
0.16.0 (`ziglang` 0.16.0 wheel), cargo-zigbuild 0.23.4 built by rustc 1.98.1 (`48a229cea 2026-09-01`).

The same recipe replayed from a second fresh `git archive` into a second fresh `CARGO_TARGET_DIR`
produced **byte-identical** binaries: all four SHA-256s below are unchanged, in 2m 37s. That
byte-identity is **version-tied** — a property of the pins above, not of the recipe — so a reader who
moves a pin should re-measure it rather than assume it.

The first run needs the network: all three installs download. It also mutates shared per-user state on
the guest rather than the repository — `~/.rustup` (two toolchains and the aarch64 std),
`~/.cargo/bin/cargo-zigbuild`, and `~/.local/bin/zig`. Re-running it is a no-op, but it is not a
sandboxed or per-worktree build step.

Why the three unusual pieces are there:

- **`aarch64-unknown-linux-gnu.2.17`** — the `.2.17` is the glibc floor zig links against. aarch64
  Linux starts at 2.17 and Rust's prebuilt std for the triple wants 2.17, so it is the lowest floor
  available and the one that runs everywhere the board might be. Pinkie's glibc is 2.35.
- **`CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS`** — D-016's `+fp16` is required here
  (`gemm-f16` 0.18.2 is in `qualia-jepa-model`'s graph and compiles for the target), and the
  target-scoped variable states by construction that it belongs to the target crates only. It is a
  statement of intent, **not a workaround**: a two-crate probe (a proc-macro and a build script)
  cross-`check`ed for this triple shows plain `RUSTFLAGS` also reaching one unit only — the target
  crate, not the host build script or the proc macro — and exiting 0, with the target-scoped variable
  behaving identically.
- **The `zig` symlink** — `uv tool install ziglang` installs a `python-zig` entry point only, and
  `cargo-zigbuild` looks for a command named `zig`.

### The artifacts

```text
qualia-jepa-train:         ELF 64-bit LSB pie executable, ARM aarch64, version 1 (SYSV), dynamically linked, interpreter /lib/ld-linux-aarch64.so.1, for GNU/Linux 2.0.0, stripped
qualia-jepa-parity:        ELF 64-bit LSB pie executable, ARM aarch64, version 1 (SYSV), dynamically linked, interpreter /lib/ld-linux-aarch64.so.1, for GNU/Linux 2.0.0, stripped
qualia-jepa-plan-eval:     ELF 64-bit LSB pie executable, ARM aarch64, version 1 (SYSV), dynamically linked, interpreter /lib/ld-linux-aarch64.so.1, for GNU/Linux 2.0.0, stripped
qualia-jepa-runtime-probe: ELF 64-bit LSB pie executable, ARM aarch64, version 1 (SYSV), dynamically linked, interpreter /lib/ld-linux-aarch64.so.1, for GNU/Linux 2.0.0, stripped
```

`readelf -h` reports `Type: DYN (Position-Independent Executable file)` and `Machine: AArch64` for
each. Their only `NEEDED` entries are `libm.so.6`, `libc.so.6`, `libpthread.so.0` and `libdl.so.2`;
the highest glibc symbol version any of them imports is `GLIBC_2.17`.

The binaries are not committed. They are 10.4 MB together — nearly eight times the tree's largest
committed blob, `docs/evidence/T52/planner-batch/after-capture.sqlite` at 1 306 624 B — and the build
above reproduces them byte for byte, so the recipe is the durable artifact and these are the addresses:

| Artifact | Bytes | SHA-256 |
| --- | --- | --- |
| `qualia-jepa-train` | 3 691 848 | `1fe32005a55d2a436306d63cbda72cd9ef6ed5348ff84d5f895def4822dc62c2` |
| `qualia-jepa-parity` | 2 040 384 | `4bce9493f3defdb72c582666bb369f768c063310d2aee113bf7b51499ea6fb6b` |
| `qualia-jepa-plan-eval` | 1 955 112 | `e8b3fd880b2a22498258c67c336facadcef30af60d8e40c52eed605113c033cc` |
| `qualia-jepa-runtime-probe` | 1 769 552 | `278740dce06e13b693919d93c091aa66dc248e147bcd255f93d15f04ee8ed459` |

They were left on the dev host at `C:/tmp/ImplCrossAarch64/ship/`, and were copied from there to
Pinkie's `~/cross-lane/`, where `sha256sum` reported the same four digests — the transfer is the same
bytes the build produced.

### The fifth binary, and where its source lives

`qualia-jepa-parity` loads the CPU reference runtime out of a checkpoint *before* it consults the
requested backend, so reaching its `--target cuda` gate needs a checkpoint directory that
`CoherentJepaRuntime::from_checkpoint` accepts. No promotion-passing candidate exists in the tree or on
the board, so a helper writes the fixture the crate's own `parity.rs` test uses
(`initialize_deterministic(&weights, 0xc0ffee)` plus a passing
`BaselineGate`/`ActionSupport`/`GroundingGeometry`).

Its source is committed beside this README as [`zz-fixture-checkpoint.rs`](zz-fixture-checkpoint.rs),
following the `harness-*.rs` precedent of
[`T50/model-eval/harness-candidate.rs`](../../T50/model-eval/harness-candidate.rs) — which is the same
program, committed there for the same reason; the two were cross-checked on 2026-09-11 and produce the
same 15 432 280-byte `weights.safetensors` and 1 471-byte `manifest.json`. The committed source here is
3 930 bytes, SHA-256 `01e45e0f49b623ae3a5aa5fd531969d7afa802bea7f7d6e047dbf0da089ccd81`. Copy it into
the tree before the cross-build:

```bash
cp docs/evidence/board/cross-lane/zz-fixture-checkpoint.rs \
   <tree>/crates/jepa-model/src/bin/zz-fixture-checkpoint.rs
# the `cargo +1.94.1 zigbuild ... --bins` line above then builds it alongside the four
```

It is a binary of `qualia-jepa-model` rather than a crate of its own so that one `zigbuild` invocation
produces it with the four real binaries, on the same target and under the same `+fp16` flag. It is
**not** part of the package's committed targets: the recipe run against a clean `git archive` of the
head builds four binaries, and the four SHA-256s in the table above are unchanged by the fifth
target's presence.

## Board run

`scp` the four binaries plus the helper to `~/cross-lane/`, then, on Pinkie:

```bash
./zz-fixture-checkpoint "$HOME/cross-lane/ckpt"
./qualia-jepa-parity --checkpoint "$HOME/cross-lane/ckpt/cross-lane-fixture" \
  --target cuda --output "$HOME/cross-lane/parity-cuda.json"
./qualia-jepa-runtime-probe --backend cpu --iterations 20 --warmup 5
./qualia-jepa-train --help
./qualia-jepa-plan-eval --help
```

Observed (kept verbatim at `~/cross-lane/board-run.log` on the board):

```text
#### 1. file(1) on the shipped binaries
qualia-jepa-train:         ELF 64-bit LSB pie executable, ARM aarch64, version 1 (SYSV), dynamically linked, interpreter /lib/ld-linux-aarch64.so.1, for GNU/Linux 2.0.0, stripped
qualia-jepa-parity:        ELF 64-bit LSB pie executable, ARM aarch64, version 1 (SYSV), dynamically linked, interpreter /lib/ld-linux-aarch64.so.1, for GNU/Linux 2.0.0, stripped
qualia-jepa-plan-eval:     ELF 64-bit LSB pie executable, ARM aarch64, version 1 (SYSV), dynamically linked, interpreter /lib/ld-linux-aarch64.so.1, for GNU/Linux 2.0.0, stripped
qualia-jepa-runtime-probe: ELF 64-bit LSB pie executable, ARM aarch64, version 1 (SYSV), dynamically linked, interpreter /lib/ld-linux-aarch64.so.1, for GNU/Linux 2.0.0, stripped
zz-fixture-checkpoint:     ELF 64-bit LSB pie executable, ARM aarch64, version 1 (SYSV), dynamically linked, interpreter /lib/ld-linux-aarch64.so.1, for GNU/Linux 2.0.0, stripped

#### 2. zz-fixture-checkpoint: build a fixture candidate checkpoint
checkpoint_id=cross-lane-fixture
dir=/home/jetson/cross-lane/ckpt/cross-lane-fixture
weights=/home/jetson/cross-lane/ckpt/cross-lane-fixture/weights.safetensors
manifest=/home/jetson/cross-lane/ckpt/cross-lane-fixture/manifest.json
rc=0
-rw-rw-r-- 1 jetson jetson     1471 Sep  2 09:49 manifest.json
-rw-rw-r-- 1 jetson jetson 15432280 Sep  2 09:49 weights.safetensors

#### 3. qualia-jepa-parity --target cuda (requested smoke path)
qualia-jepa-parity: backend cuda is not compiled into this binary
rc=1

#### 4. qualia-jepa-runtime-probe --backend cpu (does the model actually compute?)
{
  "schema_version": "qualia.jepa-runtime-probe.v1",
  "runtime_id": "qualia.jepa.coherent-tiled-runtime.v1",
  "backend": "cpu",
  "iterations": 20,
  "warmup_iterations": 5,
  "fixture_seed": 12648430,
  "synchronized_latency_p50_us": 3022,
  "synchronized_latency_p95_us": 3176,
  "synchronized_latency_max_us": 3176,
  "outputs_finite": true,
  "output_dimensions": [
    256,
    256,
    256,
    1024,
    4096
  ]
}
rc=0

#### 5. qualia-jepa-train --help
usage: qualia-jepa-train --manifest <dataset.json> --checkpoint-id <id> [--output-dir <dir>] [--backend cpu|metal|cuda] [--epochs N] [--batch-size N] [--seed N]
rc=0

#### 6. qualia-jepa-plan-eval --help
usage:
  qualia-jepa-plan-eval propose --checkpoint <dir> --request <json> \
     --output <new-json> [--backend cpu|metal|cuda] --enable-proposals
  qualia-jepa-plan-eval compare --input <json> --dataset <manifest> \
     --checkpoint <dir> --existing-planner-config <file> --output <new-json>
rc=0
```

Read it as three separate claims:

- **The cross-built binaries run on the board.** `file` on Pinkie confirms the receiver's view of the
  ELF; the helper wrote a 15 432 280-byte `weights.safetensors` there, so candle's aarch64 code path
  executes; and `qualia-jepa-runtime-probe` completed 20 timed iterations of the tiled runtime with
  `outputs_finite: true` and p50 3022 µs.
- **The `--target cuda` refusal is the compile-time gate, not a missing device.** `backend cuda is not
  compiled into this binary` is `device_for_backend` in `crates/jepa-model/src/lib.rs` bailing before
  `Device::new_cuda(0)` is ever reached. The same message would appear on any host.
- **Nothing here is a CUDA result.** No kernel launched, which is why this directory has no
  `kernels.json`. **Do not compare the 3022 µs p50 against a GPU number** — it is a CPU figure on a
  6-core Orin NX and is not a throughput measurement of anything.

## What still blocks the model steps on the board

- **The CUDA feature is not cross-buildable from this host.** `--features cuda` for
  `aarch64-unknown-linux-gnu` needs the aarch64 CUDA toolkit (`nvcc`, `libcudart`), which the WSL2
  guest does not carry and which `apt` cannot supply without a password. The board's own native build
  with `--features cuda` (the D-016 lane) reaches the GPU; the cross lane does not. This is a toolkit
  gap, not a cross-lane gap: a rootless aarch64 CUDA toolkit install is the missing piece, and it is
  a separate ticket. The board agent's native CUDA build proves the GPU path natively: it ran a
  CPU-vs-CUDA parity report against the fixture checkpoint written above (their result, in comments on
  the T52/T53 issues #225 and #228; not reproduced here).
- **`qualia-jepa-parity` was exercised with a fixture, not a trained candidate.** Reaching the
  backend gate needs a checkpoint; proving *parity* needs a real gated one, and no `qualia-jepa-train`
  run has produced one on this board. Training needs a promoted dataset manifest and its sealed
  sessions, which are not on the board either.
- **The board still has no DNS** (D-010), so the tree and the binaries ship by `scp` and the board's
  crate cache is untouched by this lane — the cross build moved that work to the dev host.

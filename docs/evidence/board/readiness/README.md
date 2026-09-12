# Board readiness — what Pinkie can do, measured

This directory answers [#230](https://github.com/superposition/qualia/issues/230)'s readiness
workstream: one page listing every capability the definition of done
([`docs/agents.md`](../../../agents.md) §Definition of done) needs from **Pinkie**, the
Waveshare-carried Jetson Orin NX (D-010), with the command that shows it and the version it printed.
The two limits the issue asked to challenge — the board's lack of `mage` and of `nsys` — were
attempted rather than assumed, and **both are now installed**; §"The two attempts" quotes every
command and every failure on the way. Nothing in this file is inferred from a cache listing or from
a decision entry. Rows below were run on the board during this work — its clock read `2026-09-02`
before it was corrected and `2026-09-12`, matching the dev host, after — except two that cite a
measurement already in the record rather than repeating it: the candle build without `+fp16`
(D-016's compiler rejection) and the `--features cuda` build (D-022's `5m 30s`). Both say so.

This directory is the checklist, not a capture: the one capture-shaped thing in it —
[`mage-nsys-capture/`](mage-nsys-capture/README.md) — is the readiness proof of the `mage`/`nsys`
capability, and says so; the convention names that case in `docs/evidence/README.md` §Layout
(`board/readiness/<slug>/`). Its sibling readiness record is
[`../cross-lane/README.md`](../cross-lane/README.md) (D-018's amendment, PR
[#231](https://github.com/superposition/qualia/pull/231) merged as `56d8170`): the host-side
`aarch64` cross-build that replaced the unavailable `cross` image.

The board itself, measured: `aarch64`, kernel `5.15.148-tegra`, L4T **R36.4.7** (JetPack 6),
6 cores, 3 601 MiB RAM (`free -m` read 1 475 used / 479 free / 1 897 available during this work),
Ubuntu 22.04.5, glibc 2.35, `nvpmodel` **10W**, 233 GiB root filesystem with **76 GiB free** at the
end of this work (87 GiB at the start — the batch's builds are what moved it). Its clock read
`2026-09-02T14:50:48Z` when this work started, 9 d 12 h 42 m 40 s behind the dev host's
`2026-09-12T03:33:28Z` (D-010's skew) — that is what made every TLS fetch below fail with
"certificate … not yet valid" — and it was **set from the host's UTC and written to the RTC on
2026-09-12**; a bracketed measurement (board epoch against the host's before/after window, twice)
now puts it **1.43 s** behind the host, stable — trivially inside every certificate's validity window,
so the `-k` workarounds are gone (§"Fixed on 2026-09-12, and what is still missing").

## The checklist

| Capability | State | Command that shows it | Version / observed |
| --- | --- | --- | --- |
| `ssh` access | **present** | `ssh pinkie 'uname -a'` — alias in `~/.ssh/config` (`HostName 192.168.55.1`, `User jetson`, `IdentityFile C:/Users/ericm/.ssh/qualia_jetson_ed25519`) | `Linux ubuntu 5.15.148-tegra … aarch64` |
| Network route | **present (HTTPS and HTTP)** | `curl -x http://192.168.55.100:8085 https://index.crates.io/config.json`, then `curl -x http://192.168.55.100:8085 http://ports.ubuntu.com/ubuntu-ports/dists/jammy/Release` | `200` both; the host's CONNECT/forward proxy `C:/tmp/board3/gadget_proxy.py`, supervised as `gadget-proxy`, bound `192.168.55.100:8085` (its plain-HTTP path was `502` until 2026-09-12 — §"Fixed on 2026-09-12, and what is still missing") |
| DNS on the board | **absent by design** | `getent hosts crates.io` | no output, `GETENT_RC=2`; the proxy dials every host for it, so nothing on the board needs a resolver |
| WiFi association | **absent** | `nmcli device status` | `wlP1p1s0  wifi  disconnected`; the association is rejected (`CTRL-EVENT-ASSOC-REJECT status_code=1`, 17–29 % signal vs the host's 62 % — physical, D-022, not a provisioning gap) |
| Cargo registry cache | **present** | `cargo fetch --locked` (head `60beee4`) | `FETCH_RC=0` in 2 s, `1.7G /home/jetson/.cargo/registry`; the head's lock needed no download |
| crates.io through the proxy | **present** | `HTTPS_PROXY=http://192.168.55.100:8085 cargo search candle-core` | `candle-core = "0.11.0"` returned; D-022 records the sparse index at `https://index.crates.io/config.json` returning `200` |
| PyPI through the proxy | **present** | `pip3 download --dest wheels --only-binary=:all: triton textual rich numpy hatchling` | `Successfully downloaded …`; `pypi.org/simple/` returns `200`, and the aarch64 wheels land (§"`mage`") |
| PyTorch CPU index through the proxy | **present** | `pip3 download --index-url https://download.pytorch.org/whl/cpu torch` | `200` for `https://download.pytorch.org/whl/cpu/torch/` without `-k` now the board's clock is right; until 2026-09-12 the same fetch needed `--trusted-host` (§"`mage`") |
| Native build, plain | **present** | `cargo build --release -j 4 -p qualia-types` | `BUILD_PLAIN_RC=0`, `3m 43s`; `cargo 1.94.0`, `rustc 1.94.0` |
| Native build, candle-bearing, `+fp16` | **present** | `RUSTFLAGS="-C target-feature=+fp16" cargo build --release -j 4 -p qualia-jepa-model --bins` | `BUILD_FP16_RC=0`, `5m 26s` from a fresh target dir; `qualia-jepa-train` 4 592 760 B, `qualia-jepa-parity` 2 469 984 B, `qualia-jepa-plan-eval` 2 379 432 B, `qualia-jepa-runtime-probe` 2 106 864 B |
| Native build, candle-bearing, no `+fp16` | **rejected by the compiler** | same command without `RUSTFLAGS` | *cited, not re-run here*: D-016 records `gemm-f16`'s inline asm rejected by the default `neon`-only target (`instruction requires: fullfp16`); the Orin's A78AE has the feature |
| `nvcc` | **present** | `/usr/local/cuda/bin/nvcc --version` | `release 12.9, V12.9.41` (`cuda_12.9.r12.9/compiler.35813241_0`) |
| `sm_87` codegen | **present** | `/usr/local/cuda/bin/nvcc -fatbin -gencode arch=compute_87,code=sm_87 kernels/belief_update.cu` then `/usr/local/cuda/bin/cuobjdump --list-elf belief_update.fatbin` | `NVCC_SM87_RC=0`, `ELF file 1: belief_update.1.sm_87.cubin`; `cuobjdump` is not on `PATH` |
| CUDA-feature build | **present** | `RUSTFLAGS="-C target-feature=+fp16" cargo build --release -j 2 -p qualia-jepa-model --bins --features cuda` | *cited, not re-run here*: D-022 measured `5m 30s`, candle-kernels 0.9.2 built with `nvcc` for sm_87 |
| Running binaries on the device | **present** | `/home/jetson/readiness/head-target/release/qualia-jepa-runtime-probe --backend cpu --iterations 5 --warmup 1`, and `--backend cuda` under `LD_LIBRARY_PATH=/usr/local/cuda-12.9/compat` | `PROBE_CPU_RC=0`, `synchronized_latency_p50_us` **3070**, `outputs_finite: true`; the CUDA probe ran here too (`STANDALONE_RC=0`, p50 3376 µs under the `mage` capture), against D-022's 3168 µs CPU / 2039 µs CUDA at 30 iterations |
| `ncu` | **present (needs root)** | `/usr/local/cuda/bin/ncu --version` | `2025.2.0.0 (build 35613519) (public-release)`; `perf_event_paranoid=2` and no NOPASSWD, so captures run under `sudo -S` |
| `mage` | **present** | `pip3 show mage` for the version, `mage --help` for the CLI entry (the CLI has no `--version` flag) | `Version: 0.1.0`, `/home/jetson/.local/bin/mage`, installed from the board; deps `torch 2.14.0+cpu`, `triton 3.8.0`, `numpy 2.2.6`, `textual 8.2.8`, `rich 15.0.0` |
| `mage profile-exec` with `nsys` | **present** | `mage profile-exec --backend nsys --capture-range all --output-dir … -- <board binary>` | `MAGE_EXEC_CUDA_RC=0`, 494 kernel rows parsed out of the capture (§"The two attempts"; committed at `mage-nsys-capture/`) |
| `nsys` | **present** | `nsys --version` | `NVIDIA Nsight Systems version 2024.5.4.34-245434855735v0` at `/usr/local/bin/nsys` |
| `tegrastats` | **present** | `tegrastats --interval 1000` (bounded with `timeout 3`) | see §"Other board tools" |
| Disk headroom | **present** | `df -h /` | 76 GiB free of 233 GiB (66 % used) after this work; 87 GiB free before it |
| Plain-HTTP through the proxy | **present (fixed 2026-09-12)** | `apt-get -o Acquire::http::Proxy=http://192.168.55.100:8085 download libxcb-cursor0` | `Fetched 9 880 B`, deb staged; before the fix the same fetch was `502 Bad Gateway` (§"Fixed on 2026-09-12, and what is still missing") |

## The two attempts

### `nsys` — installed, no NVIDIA account

The audit comment on #230 treats "no aarch64 build of Nsight Systems" as a real standing limit. It is
false: NVIDIA's Jetson repository — **already in the board's apt sources**
(`/etc/apt/sources.list.d/nvidia-l4t-apt-source.list`: `deb https://repo.download.nvidia.com/jetson/common r36.4 main`)
— carries `nsight-systems-2024.5.4` for `arm64`, built for Tegra. The candidate was in the board's
own apt cache before this work started:

```console
jetson@ubuntu:~$ apt-cache policy nsight-systems-2024.5.4
nsight-systems-2024.5.4:
  Installed: (none)
  Candidate: 2024.5.4.34-245434855735v0
  Version table:
     2024.5.4.34-245434855735v0 600
        600 https://repo.download.nvidia.com/jetson/common r36.4/main arm64 Packages
```

The first route, `apt-get download`, failed on the board's clock when this attempt ran (the clock was
corrected later in the same session — §"Fixed on 2026-09-12, and what is still missing"): its certificates were "not yet
valid". Every other route below works around that, and none needs an account.

```console
jetson@ubuntu:~$ apt-get -o Acquire::https::Proxy=http://192.168.55.100:8085 download nsight-systems-2024.5.4
Err:1 https://repo.download.nvidia.com/jetson/common r36.4/main arm64 nsight-systems-2024.5.4 arm64 2024.5.4.34-245434855735v0
  Certificate verification failed: The certificate is NOT trusted. The certificate chain uses not yet valid certificate.
E: Failed to fetch https://repo.download.nvidia.com/.../nsight-systems-2024.5.4_2024.5.4.34-245434855735v0_arm64.deb
APT_NSYS_RC=100
```

After the clock fix, the same command — with no `-k` anywhere — works unchanged:

```console
jetson@ubuntu:~$ apt-get -o Acquire::https::Proxy=http://192.168.55.100:8085 download nsight-systems-2024.5.4
Get:1 https://repo.download.nvidia.com/jetson/common r36.4/main arm64 nsight-systems-2024.5.4 arm64 2024.5.4.34-245434855735v0 [313 MB]
Fetched 313 MB in 9s (33.0 MB/s)
APT_NSYS3_RC=0
404b1d921366d94f60a027298523e295c5484ee6f6e3b8a27da304ad4fd92bad  nsight-systems-2024.5.4_2024.5.4.34-245434855735v0_arm64.deb
```

`curl -k` gets the same file through the same proxy:

```console
jetson@ubuntu:~$ curl -sk -o nsight-systems-2024.5.4_2024.5.4.34-245434855735v0_arm64.deb \
  -w 'CURL_HTTP=%{http_code} CURL_BYTES=%{size_download} CURL_SPEED=%{speed_download}\n' \
  https://repo.download.nvidia.com/jetson/common/pool/main/n/nsight-systems-2024.5.4/nsight-systems-2024.5.4_2024.5.4.34-245434855735v0_arm64.deb
CURL_HTTP=200 CURL_BYTES=313390022 CURL_SPEED=23922902
CURL_RC=0
404b1d921366d94f60a027298523e295c5484ee6f6e3b8a27da304ad4fd92bad  nsight-systems-2024.5.4_2024.5.4.34-245434855735v0_arm64.deb
Package: nsight-systems-2024.5.4
Version: 2024.5.4.34-245434855735v0
Architecture: arm64
```

That SHA-256 is the one in the repository's own `Packages` index
(`https://repo.download.nvidia.com/jetson/common/dists/r36.4/main/binary-arm64/Packages`), so the
download is the published artifact, 23.9 MB/s through the USB gadget link. The `.deb` unpacks a
Tegra-native CLI — no `.run`, no account:

```console
jetson@ubuntu:~$ dpkg-deb -c nsight-systems-2024.5.4_*.deb | grep -E 'bin/nsys$|tegra-armv8/nsys$'
-rwxr-xr-x root/root 44682480 2024-09-17 16:16 ./opt/nvidia/nsight-systems/2024.5.4/target-linux-tegra-armv8/nsys
lrwxrwxrwx root/root        0 2024-09-17 17:32 ./opt/nvidia/nsight-systems/2024.5.4/bin/nsys -> ../target-linux-tegra-armv8/nsys
```

**The install.** The system route needs three X libraries the JetPack rootfs lacks
(`libxcb-xinerama0`, `libxcb-xinput0`, `libxcb-cursor0`, all GUI-only) and the proxy's plain-HTTP
path was broken at the time (§"Fixed on 2026-09-12, and what is still missing"), so those three were staged from the host and installed
locally. The package then configures cleanly:

```console
jetson@ubuntu:~$ echo jetson | sudo -S dpkg -i xlibs/libxcb-{xinerama0,xinput0,cursor0}_*.deb
DPKG_XLIBS_RC=0
jetson@ubuntu:~$ echo jetson | sudo -S dpkg --configure nsight-systems-2024.5.4
Setting up nsight-systems-2024.5.4 (2024.5.4.34-245434855735v0) ...
update-alternatives: using /opt/nvidia/nsight-systems/2024.5.4/target-linux-tegra-armv8/nsys to provide /usr/local/bin/nsys
DPKG_CONFIGURE_RC=0
jetson@ubuntu:~$ nsys --version
NVIDIA Nsight Systems version 2024.5.4.34-245434855735v0
jetson@ubuntu:~$ nsys profile --force-overwrite=true --stats=false --output=nsys-smoke-sys /bin/true
NSYS_SYS_SMOKE_RC=0   # nsys-smoke-sys.nsys-rep, 81 557 B
jetson@ubuntu:~$ dpkg -l nsight-systems-2024.5.4 | tail -1
ii  nsight-systems-2024.5.4  2024.5.4.34-245434855735v0  arm64  Nsight Systems is a statistical sampling profiler with tracing features.
```

A rootless route works too, for a board where the account has no `sudo`: `dpkg -x` the `.deb` and run
`nsightroot/opt/nvidia/nsight-systems/2024.5.4/bin/nsys`; it printed the same version and profiled
`/bin/true` (`NSYS_VERSION_RC=0`, `NSYS_SMOKE_RC=0`) before the system install existed. `nsys` is on
`PATH` now, so mage's `NativeBackend.find_executable` (`shutil.which("nsys")` first) finds it.

### `mage` — installed from PyPI, on the board

The audit comment says `triton>=3.0` "publishes no aarch64 wheel, so from the host's WSL it is
unsatisfiable on the board". Both halves are false as of 2026-09-12: PyPI carries
`triton-3.8.0-cp310-cp310-manylinux_2_27_aarch64.manylinux_2_28_aarch64.whl` (226.4 MB) and the
PyTorch CPU index carries `torch-2.14.0+cpu-cp310-cp310-manylinux_2_28_aarch64.whl` (159.2 MB), both
native `aarch64` wheels. The board's `python3` is 3.10.12 and it has `pip 22.0.2`, so no `uv` was
needed; the wheels were staged through the proxy (PyPI is reachable from the board — §"Network"), and
the install itself was offline:

```console
jetson@ubuntu:~$ pip3 download --dest wheels --only-binary=:all: triton textual rich numpy hatchling
Successfully downloaded triton textual rich numpy hatchling markdown-it-py packaging pathspec platformdirs pluggy pygments …
jetson@ubuntu:~$ pip3 download --dest wheels --only-binary=:all: --trusted-host download.pytorch.org \
  --trusted-host download-r2.pytorch.org --index-url https://download.pytorch.org/whl/cpu torch
Successfully downloaded torch fsspec networkx sympy typing-extensions filelock jinja2 MarkupSafe mpmath setuptools
jetson@ubuntu:~$ pip3 install --user --no-index --find-links=wheels torch triton textual rich numpy "numpy==2.2.6" hatchling
PIP_NUMPY_RC=0
jetson@ubuntu:~$ tar xzf mage-src.tar.gz -C mage-src   # `git archive HEAD` of github.com/superposition/mage
jetson@ubuntu:~$ pip3 install --user --no-index --find-links=wheels --no-build-isolation ./mage-src
Successfully built mage
Successfully installed mage-0.1.0
PIP_MAGE_RC=0 SECONDS=3
jetson@ubuntu:~$ python3 -c "import torch, triton, numpy; print(torch.__version__, triton.__version__, numpy.__version__)"
torch 2.14.0+cpu triton 3.8.0 2.2.6
jetson@ubuntu:~$ mage --help
usage: mage [-h] {demo,bench,profile,profile-exec,stress} ...
Mage - High-performance Triton CUDA kernels
positional arguments: {demo,bench,profile,profile-exec,stress}
MAGE_HELP_RC=0
```

Two board facts the install had to work around, worth knowing for the next one:
`pip install --user numpy` alone is a no-op on this image because the system's `numpy 1.21.5`
satisfies the unversioned requirement, and mage's `numpy>=2.2.6` then fails at import — the version
needs pinning so the user-site wheel shadows `/usr/lib/python3/dist-packages`. And
`download.pytorch.org`'s certificate was also "not yet valid" on the board's clock when these wheels
were staged, so the command above carries `--trusted-host` (or a `curl -k` fetch); after the clock fix
of 2026-09-12 the flag is no longer needed — `https://download.pytorch.org/whl/cpu/torch/` answers
`200` without it — and PyPI's own certificate validated throughout.

Then mage drove `nsys` itself, over a board binary, and read the export:

```console
jetson@ubuntu:~$ export LD_LIBRARY_PATH=/usr/local/cuda-12.9/compat
jetson@ubuntu:~$ mage profile-exec --backend nsys --capture-range all \
  --output-dir /home/jetson/readiness/mage-out-cuda3 \
  -- /home/jetson/qualia-t53/target/release/qualia-jepa-runtime-probe --backend cuda --iterations 5 --warmup 1
╭─ GPU Profiler - /home/jetson/qualia-t53/target/release/qualia-jepa-runtime-p─╮
│ ┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┳━━━━━━━━━━━━━┳━━━━━━━━━┳━━━━━━━━━━━┓ │
│ ┃ Kernel                        ┃ Duration (… ┃ Grid    ┃ Block     ┃ │
│ │ affine_f32                    │        7.52 │ 1       │ 1024      │ │
│ │ urelu_f32                     │        7.49 │ 1       │ 1024      │ │
│ │ copy2d_f32                    │        7.46 │ 1       │ 1024      │ │
…
MAGE_EXEC_CUDA_RC=0 SECONDS=8
jetson@ubuntu:~$ ls -l mage-out-cuda3/mage-nsys-doiq0b7c/
capture.json      825 B     kernels.csv   66 544 B
capture.nsys-rep  228 004 B kernels.json  459 131 B
capture.sqlite    884 736 B process.log   6 227 B
```

The export carries **494** `CUPTI_ACTIVITY_KIND_KERNEL` rows across the probe's whole fixture build
and its measured iterations — `badd_f32` 90, `urelu_f32` 66, `copy2d_f32` 66, `ucopy_f32` 36,
`affine_f32` 32, `ampere_sgemm_32x128_tn` 24, `im2col_f32` and `im2col1d_f32` 18 each, the two
`curand` seeding kernels 14 each, `gemvx` and `fast_sum_f32` 12 each, and the remaining rows the
probe's smaller `f32` elementwise kernels. `capture.json` is mage's manifest (`backend`, `argv`,
`profiler_argv`, `returncode: 0`, `status: complete`, `kernel_count: 494`).

The capture itself is committed, trimmed, in this directory at
[`mage-nsys-capture/`](mage-nsys-capture/README.md) — its own README carries the shape, the 494-row
kernel histogram and the file table. The untrimmed export, the `nsys` report
(`capture.nsys-rep`, 228 004 B) and mage's `process.log` stay on the capturing machine at
`/home/jetson/readiness/mage-out-cuda3/mage-nsys-doiq0b7c/`, per the convention's
"keep `--output-dir` outside the tree" rule.

The nsys-backed `profile-exec` needs the forward-compat `libcuda`, exactly as the ticket's own
captures do: without `LD_LIBRARY_PATH=/usr/local/cuda-12.9/compat` the probe dies with
`CUDA_ERROR_UNSUPPORTED_PTX_VERSION` before its first kernel, and mage then reports
`nsys exited with code 1` with an export that has no kernel table. With it, a kernel-less CPU target
is reported as the convention expects (`status: failed`, `captured no CUDA kernel launches …`) while
the CUDA target captures normally.

**What this unblocks.** `docs/evidence/README.md`'s `## Manual capture` clause existed because Pinkie
"carries no `nsys`, has no DNS, and has no mage installed today". The first two are fixed above and
the third is now false, so a `needs:profile` ticket can take its board capture with the convention's
own command — `mage profile-exec --backend nsys --capture-range all --output-dir ~/mage-capture-scratch/T<NN>/<slug> -- ./gpu-<hash> --test-threads=1`
— instead of a hand-driven `ncu` run. `ncu` remains available and unchanged for counter work, and
`--backend ncu` still hits the `--page raw`/details-page mismatch D-017 records (a mage defect
outside this tree).

## Native builds

Measured on the board at head `60beee4f30fb473c8d1222cd0b57b9d3be466581` from a `git archive`
(`Cargo.lock` sha256 `0c5eadb1f0b7f2ab14ff8f44dc8f8970b0bf44a1e56e52544d08025f5b2d0927`), in a fresh
`CARGO_TARGET_DIR`, after `cargo fetch --locked` returned `FETCH_RC=0` in 2 s with the head's closure
already in the 1.7 GiB registry cache:

```console
jetson@ubuntu:~/readiness/head$ cargo build --release -j 4 -p qualia-types
BUILD_PLAIN_RC=0 SECONDS=223        # Finished `release` profile [optimized] target(s) in 3m 43s
jetson@ubuntu:~/readiness/head$ RUSTFLAGS="-C target-feature=+fp16" cargo build --release -j 4 -p qualia-jepa-model --bins
BUILD_FP16_RC=0 SECONDS=327        # Finished `release` profile [optimized] target(s) in 5m 26s
-rwxrwxr-x  qualia-jepa-train      4 592 760 B
-rwxrwxr-x  qualia-jepa-parity     2 469 984 B
-rwxrwxr-x  qualia-jepa-plan-eval  2 379 432 B
-rwxrwxr-x  qualia-jepa-runtime-probe   2 106 864 B   # feature-off
```

D-022 records the same candle-bearing build at `-j 2` in `7m 19s` (and `5m 30s` with
`--features cuda`), with `qualia-jepa-train` 4 501 616 B, `qualia-jepa-parity` 2 390 656 B,
`qualia-jepa-plan-eval` 2 391 088 B and `qualia-jepa-runtime-probe` 2 018 664 B feature-off /
10 375 096 B with CUDA; this work's fresh-dir `-j 4` build is `5m 26s` with the same four binaries
within a few percent of those sizes (the head differs by the T53 fix, #229). Without `RUSTFLAGS`,
the candle closure does not compile at all: D-016 records `gemm-f16`'s `instruction requires:
fullfp16`. Non-candle packages need no flag.

## CUDA codegen

```console
jetson@ubuntu:~$ /usr/local/cuda/bin/nvcc --version | tail -2
Cuda compilation tools, release 12.9, V12.9.41
Build cuda_12.9.r12.9/compiler.35813241_0
jetson@ubuntu:~$ /usr/local/cuda/bin/nvcc -fatbin -gencode arch=compute_87,code=sm_87 \
  -o belief_update.fatbin kernels/belief_update.cu
NVCC_SM87_RC=0
jetson@ubuntu:~$ /usr/local/cuda/bin/cuobjdump --list-elf belief_update.fatbin
ELF file    1: belief_update.1.sm_87.cubin
CUOBJDUMP_RC=0
```

`cuobjdump` is in `/usr/local/cuda/bin` and **not on `PATH`** — call it by path. The same route at
suite scale is what `docs/evidence/T18/fatbin-sm-87/` records (`CUDAARCHS=87-real
NVCC=/usr/local/cuda/bin/nvcc cargo build -p qualia-cuda --features cuda`), and
`docs/evidence/T16/board-kernels/` is the `ncu` capture of the eight device tests it produced.

## Running on the device

```console
jetson@ubuntu:~$ /home/jetson/readiness/head-target/release/qualia-jepa-runtime-probe --backend cpu --iterations 5 --warmup 1
PROBE_CPU_RC=0
  "synchronized_latency_p50_us": 3070,
  "synchronized_latency_p95_us": 3084,
  "synchronized_latency_max_us": 3084,
  "outputs_finite": true,
  "output_dimensions": [256, 256, 256, 1024, 4096]
jetson@ubuntu:~$ LD_LIBRARY_PATH=/usr/local/cuda-12.9/compat \
  /home/jetson/qualia-t53/target/release/qualia-jepa-runtime-probe --backend cuda --iterations 5 --warmup 1
STANDALONE_RC=0
```

Both print the probe's JSON with `outputs_finite: true`; D-022 measured
`synchronized_latency_p50_us` **3168** on the CPU backend and **2039** on CUDA at 30 iterations, this
work's 5-iteration CPU run measured **3070**, and its CUDA run reported p50 3376 µs under `mage`'s
`nsys` capture — the same order, on a board whose GPU is shared with `llama-server`. The
`LD_LIBRARY_PATH` is required for the CUDA backend (the toolkit is newer than the driver; D-016).

## Other board tools

`tegrastats` is present at `/usr/bin/tegrastats` (79 936 B, from the L4T image) and prints on demand:

```console
jetson@ubuntu:~$ timeout 3 tegrastats --interval 1000
09-02-2026 10:18:54 RAM 3028/3602MB (lfb 5x512kB) SWAP 854/14089MB (cached 4MB) CPU [100%@1510,…]
```

`ncu` needs root (`perf_event_paranoid=2`, no NOPASSWD), so its captures run under `sudo -S` with the
board's password on stdin, as `docs/evidence/T16/board-kernels/` and the T50/T18 captures do.

## Fixed on 2026-09-12, and what is still missing

Two of the three gaps this work first found were provisioning bugs, not limits, and both are fixed
with the before/after measured. The third is physical.

**Fixed: the board's clock.** It read `2026-09-02T14:50:48Z` against the dev host's
`2026-09-12T03:33:28Z` — 9 d 12 h 42 m 40 s behind (D-010's skew), which is why `apt-get download`,
`pip` from `download.pytorch.org` and every freshly-issued certificate failed with "not yet valid".
It was set from the host's UTC and written to the RTC:

```console
$ echo jetson | sudo -S date -u -s "$(date -u +'%Y-%m-%d %H:%M:%S')"
Sat Sep 12 03:33:52 AM UTC 2026          # DATE_SET_RC=0
$ echo jetson | sudo -S hwclock -w       # HWCLOCK_W_RC=0; hwclock -r → 2026-09-11 23:34:02 -04:00
BOARD_NOW=2026-09-12 03:33:55 UTC   HOST_NOW=2026-09-12 03:33:57
# bracketed later, twice: round_trip=0.772s board_offset=-1.437s / round_trip=0.781s board_offset=-1.432s
```

After it, with no `-k` anywhere: `repo.download.nvidia.com` `200`, `download.pytorch.org` `200`,
`index.crates.io` `200`, `pypi.org` `200`, and `apt-get download nsight-systems-2024.5.4` fetched
313 MB at 33.0 MB/s with the published sha256 (§"The two attempts"). `systemd-timesyncd` is still
`active` and unsynchronised — it has no DNS to reach a pool — so the `hwclock -w` write is what makes
the fix survive a reboot.

**Fixed: the host proxy's plain-HTTP path.** `C:/tmp/board3/gadget_proxy.py` parsed the authority
with `rpartition(":")` and then `int(port)`; for an `http://` request (no port) the "port" was the
hostname, the parse raised, and the client got `502 Bad Gateway`. The parse now branches on
`":" in hostport`, the process was restarted (`hub restart gadget-proxy`, pid 21620, `ready`), and:

```console
$ curl -x http://192.168.55.100:8085 http://ports.ubuntu.com/ubuntu-ports/dists/jammy/Release
ports_release_rc=200 bytes=269219
$ apt-get -o Acquire::http::Proxy=http://192.168.55.100:8085 download libxcb-cursor0
Fetched 9,880 B in 0s (26.1 kB/s)
```

The `http://` archives are reachable through the proxy now, so staging Ubuntu `.deb`s from the host
is no longer necessary.

**Still missing: the board's WiFi does not associate.** `CTRL-EVENT-ASSOC-REJECT status_code=1` on
both bands, 17–29 % signal against the host's 62 %, `wlP1p1s0` disconnected. D-022 records that as a
physical/antenna finding, not a provisioning gap: the USB gadget link plus the proxy is the working
route, and the board resolves nothing itself — its DNS absence is by design, since the proxy dials
every host on its behalf.

## The standing rule

This directory exists to make one sentence checkable: **"the board cannot do X" is a claim that
requires a measurement after provisioning** — not an inference from a cache listing, a network state,
or an earlier decision entry. D-010, D-016 and D-018 recorded limits that were true of the board
before it was provisioned; D-022 amends them, and both limits this issue asked to challenge (`mage`,
`nsys`) turned out to be installs. When a future ticket wants to record that Pinkie cannot do
something, it names the command it ran on the board and quotes what came back, here or in that
ticket's own evidence directory. A refusal without a measurement is provisional.

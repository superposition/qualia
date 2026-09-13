# Real-camera flyvis probe

`scripts/flyvis_camera_probe.py` captures a bounded JPEG sequence and replays it through
one published, frozen flyvis visual network. It writes graded membrane voltage in
arbitrary model units. It has no wheel, head, light, transport, or QLSP interface.
It does not replace the running Qualia CNS.

The first host run used actual robot camera images and completed with 45,669 cells,
1,513,231 connections, 65 cell-type/compartment labels, and finite responses. Its
Windows peak working set was 847 MiB. That run establishes host CPU inference, not a
trained wheel controller. A subsequent [bounded live Jetson observer](LIVE.md)
uses the exported frozen computation and about 278 MiB RSS. See
[the evidence](../../docs/evidence/flyvis-camera-probe/README.md).

## Model and input contract

The model is ensemble member `flow/0000/000`, its supplied `chkpt_00000`, from
[TuragaLab flyvis 1.2.0](https://github.com/TuragaLab/flyvis/tree/92b3845cc426dd309a1a0e1b3890156c42e14021).
This is the authors' implementation of the
[2024 visual-system paper](https://www.nature.com/articles/s41586-024-07939-3).
The member is a reproducible initial choice, not a claim that it is the best
biological fit among the 50 pretrained models.

The probe checks the complete public archive SHA256 before reading its model config
or allowing its checkpoint through `torch.load`. It requires every checkpoint
parameter to load strictly and checks that inference has not changed any parameter.
It does not alter resting potentials, time constants, signs, gains, or model dynamics.
The published model's fitted resting potentials are part of the checkpoint.

Input preprocessing follows the public
[custom-input interface](https://turagalab.github.io/flyvis/examples/07_flyvision_providing_custom_stimuli/):
JPEG RGB channels are averaged and divided by 255, the whole image is resized to
the lattice's minimum support, and the official `BoxEye(extent=15, kernel_size=13)`
performs its mean box filtering and 721-position hexagonal sampling. The result
has shape `1 x frames x 1 x 721`. Whole-image resizing is an explicit geometric
choice; camera angular coverage, lens distortion, and physical luminance remain
uncalibrated. No color-specific photoreceptor sensitivity is claimed.

Integration uses the checkpoint's training interval, 20 ms. Initialization holds
the first measured image for 50 integration steps, starting from the model's
learned initial state. These repeated warmup inputs are labeled initialization,
not new camera acquisitions. Subsequent frames use zero-order hold on their host
receipt intervals, retaining neural state between steps. No interpolated movement,
synthetic stimulation, or training augmentation is added. A single JPEG is explicitly
a held-image response, so its dynamics do not establish detection of scene motion.

Camera records contain request-start and receipt times in Unix nanoseconds, and
receipt times from the host monotonic clock. Camera exposure times are unknown.
An HTTP poll may return the same JPEG as a previous poll; input hashes expose this.
Inference is offline replay after the bounded acquisition, not a claim that an old
response is current live control input.

## Isolated Windows CPU setup

Use Python 3.11 and keep the environment, models, caches, and camera images outside
the repository. The lock is specific to Windows x86-64 CPU wheels. It must not be
used as a Jetson dependency lock.

```powershell
$probeState = 'C:/Users/ericm/.local/state/qualia/flyvis-probe'
uv venv "$probeState/venv" --python 3.11
uv pip sync scripts/flyvis/requirements-windows-cpu.lock --require-hashes --no-build --python "$probeState/venv/Scripts/python.exe"
$env:FLYVIS_ROOT_DIR = "$probeState"
$env:MPLBACKEND = 'Agg'
& "$probeState/venv/Scripts/python.exe" -m flyvis_cli.download_pretrained_models
```

The official downloader also downloads a small clustering archive; the probe only
uses `results_pretrained_models.zip`. Neither archive belongs in git.

| Component | Exact download size | Provenance |
| --- | ---: | --- |
| flyvis 1.2.0 wheel | 385,602 bytes | PyPI; hash in the lock |
| PyTorch 2.7.1+cpu Windows CPython 3.11 wheel | 216,031,210 bytes | Official PyTorch CPU index; URL/hash in the lock |
| torchvision 0.22.1+cpu matching wheel | 1,708,132 bytes | Official PyTorch CPU index; URL/hash in the lock |
| All 84 pinned dependency wheels, including those above | 401,064,691 bytes | Resolved wheel sizes, approximately 382.5 MiB |
| Published pretrained archive, all 50 models | 3,417,042 bytes | Official downloader's public Drive file `13cJr2nMn89j-jBAd5RduYRJpBcXwoNrC` |
| Optional official clustering archive | 149,056 bytes | Downloaded by the upstream setup command; unused by this probe |

The pretrained archive expands to 5,255,400 bytes. The selected checkpoint is
49,396 bytes. Connectome expansion and inference require more memory and cache
space than the small checkpoint size suggests.

Pinned archive SHA256:

```text
71c78d4070556a536b13b23ee3139cd2788aa2a9d07d430a223b4edead281db1
```

Selected checkpoint SHA256:

```text
d0e42857e738d0315897c2d50fde9eb3fb1a3fb1f071d55dfc13d53d72bccc3f
```

Flyvis code is [MIT licensed](https://github.com/TuragaLab/flyvis/blob/v1.2.0/LICENSE).
PyTorch and torchvision use BSD-style licenses; their full notices remain in the
installed distributions. No separate checkpoint license document was present in
the published archive inspected here; the report does not assign it an invented
license. The repository stores neither upstream source nor pretrained weights.

The wheel-only lock uses `hydra-core==0.11.3` because the modern Hydra dependency
chain includes a source-only ANTLR package. The probe does not invoke Hydra's CLI
or training configuration machinery. Its imported flyvis inference path was
exercised with the exact lock. This is a tested narrow environment, not a promise
that every flyvis tutorial or training tool works with these pins.

Datamate 1.0.0 has an HDF5 writer path that unlinks a file while its own handle is
open, which fails on Windows. The probe installs a Windows-only adapter for array
writes: create the HDF5 `data` dataset, enable SWMR, and close the handle through a
context manager. It preserves array dtype and values and does not change neural
computation. The manifest records whether the adapter was active; a regression
checks numeric/string round trips and handle release.

## Run and inspect

Run from the repository root. The output directory must be new; an existing
directory is refused before touching its evidence. Raw camera images stay local.

```powershell
& "$probeState/venv/Scripts/python.exe" scripts/flyvis_camera_probe.py --camera-url http://10.0.0.180:8000/camera/snapshot --archive "$probeState/results_pretrained_models.zip" --cache-dir "$probeState/model-cache-v2" --out-dir "$probeState/camera-run-01" --frames 8 --interval-ms 100 --wall-seconds 180
```

For one already-recorded real image, replace `--camera-url ...` with
`--jpeg PATH_TO_REAL_JPEG` and optionally use `--hold-steps 20`. The input record
states that its acquisition time is unknown; file read time is not relabeled as
camera acquisition time.

Limits are 16 camera polls, 8 MiB per JPEG, 16,777,216 pixels per image, 500 total
integration steps including warmup, and at most 300 seconds of wall time. The
default deadline is 180 seconds. A supervisor kills an expired worker and marks
its manifest failed. Worker imports and execution force CPU, two numerical
threads, and a noninteractive plotting backend. Camera requests are GETs to the
exact `/camera/snapshot` path; redirects and credentials/query strings are refused.

Only `manifest.json` with `status: complete` describes a complete result. A timeout,
bad input, wrong checkpoint, nonfinite response, or modified parameter leaves a
failed result and diagnostic log. Partial arrays from a failed run are not valid
evidence of a completed probe.

| File | Meaning |
| --- | --- |
| `inputs.json`, `input-NNN.jpg` | Original bytes, SHA256 hashes, geometry, source, host timing, unknown exposure timestamp |
| `retina.npy` | Actual rendered input, shape `[1, camera polls, 1, 721]` |
| `cells.csv` | Model array index, real flyvis cell-type label, hexagonal `u,v` coordinates |
| `activity.npy` | Float32 graded voltage, shape `[replay steps, 45669]`; column index joins `cells.csv` |
| `warmup_activity.npy` | State after the first measured image was held for 1 second |
| `activity-by-type.csv` | Per-type voltage statistics and change from warmup at every replay step, with source input index and host receipt time |
| `manifest.json`, `worker.log` | Model identity, hashes, versions, preprocessing, memory, completion state, diagnostics |

Negative voltage is valid in this model. Nonzero voltage may reflect a fitted
baseline; it must not be displayed as a spike count or proof of evoked motion.
These indices are not MaleCNS body IDs. A console integration should present a
separate graded visual-network view and label the observation/replay time.

CPU boundary checks:

```powershell
& "$probeState/venv/Scripts/python.exe" scripts/test_flyvis_camera_probe.py
uv pip check --python "$probeState/venv/Scripts/python.exe"
```

## Remaining robot-control work

This feature demonstrates a real camera-to-pretrained-visual-network path. It does
not establish a biological visual-to-descending-neuron mapping, fused IMU/lidar
input to this model, optical-flow decoding accuracy on the robot, or a wheel
policy. Those require separately measured/calibrated interfaces and validation.
The subsequent [live observer](LIVE.md) uses the Jetson's installed CPU PyTorch
and records actual bounded latency and memory alongside the existing robot
services. This probe's original host evidence remains separate from that
deployment evidence.

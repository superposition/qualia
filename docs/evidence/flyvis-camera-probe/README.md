# Host CPU evidence: published visual model and real robot JPEGs

On 2026-09-12, the new bounded probe completed one eight-poll camera acquisition
and offline pretrained inference. The first host receipt was
`2026-09-12T23:36:41.498957+00:00`. Camera exposure time and the robot's lighting
state were not independently verified. The local directory is named
`light-off-02`, but that name is not proof of an illumination condition.

The full machine-readable summary is [host-cpu-result.json](host-cpu-result.json).
Raw JPEGs and full arrays remain outside git at:

```text
C:/Users/ericm/.local/state/qualia/flyvis-probe/light-off-02
```

The command was:

```powershell
C:/Users/ericm/.local/state/qualia/flyvis-probe/venv/Scripts/python.exe scripts/flyvis_camera_probe.py --camera-url http://10.0.0.180:8000/camera/snapshot --archive C:/Users/ericm/.local/state/qualia/flyvis-probe/results_pretrained_models.zip --cache-dir C:/Users/ericm/.local/state/qualia/flyvis-probe/model-cache-v2 --out-dir C:/Users/ericm/.local/state/qualia/flyvis-probe/light-off-02 --frames 8 --interval-ms 100 --wall-seconds 180
```

The process returned zero and wrote `status: complete`. It used the published
member `flow/0000/000`, strict checkpoint recovery, official BoxEye rendering,
two CPU threads, and no GPU. Loaded parameters were unchanged after inference.

| Observation | Result |
| --- | ---: |
| Real JPEG polls / distinct SHA256 hashes | 8 / 5 |
| Model cells / connections | 45,669 / 1,513,231 |
| Cell-type/compartment labels | 65 |
| Held first-image warmup steps | 50 at 20 ms |
| Replay steps after warmup | 48 at 20 ms |
| Graded activity array | `[48, 45669]`, float32 |
| All activity values finite | true |
| Full-array voltage range | -1.769664 to 1.759367 arbitrary model units |
| Mean absolute final change from warmup | 0.0012253 model units |
| Retinal input range | 0.0830294 to 0.1653163 |
| Worker wall time, including imports and capture | 26.25 seconds |
| Sampled peak working set | 752.65 MiB |
| Windows OS peak working set | 847.04 MiB |

Example final mean graded voltages were `T4a=0.0763592`, `T4b=0.0670273`,
`T5a=0.000400982`, and `T5b=-0.0514782`. These are not spike rates. Fitted
baseline activity and the held-image initialization contribute to absolute
voltage; these numbers alone do not establish motion-direction accuracy or a
wheel policy. Repeated identical JPEG hashes are retained as repeated observations,
not treated as independent exposure timestamps.

Eight CPU boundary checks passed in 0.698 seconds. They cover camera path and
credential refusal, measured-time replay bounds/order, exact JPEG provenance,
unknown file acquisition time, checkpoint size/hash refusal, existing evidence
preservation, nonfinite metadata refusal, supervisor timeout state, and Windows
HDF5 array round trips/handle release. The checks are grouped into eight tests.
`uv pip check` reported all 84 installed packages compatible, and synchronization
against the hash-pinned wheel lock in dry-run mode reported no changes needed.

The initial attempt, `light-off-01`, failed before camera acquisition because
datamate 1.0.0 tried to unlink its own open HDF5 handle on Windows. Its failed
manifest and traceback are preserved locally. The passing run used the explicit
Windows storage adapter described in the [run instructions](../../../scripts/flyvis/README.md).

Implementation SHA256 for the passing neural path:

```text
scripts/flyvis_camera_probe.py
605059c0b36578d1c966f023baaa95f3856dcd0e41ab9f90b8499244cbe96cb0
```

This is host evidence for a camera-to-pretrained-visual-model probe. It includes
no robot actuation, Jetson installation/execution, neural-to-wheel mapping,
whole-brain ID mapping, or validation against measured neural activity. A paired
lighting comparison needs separately identified conditions and has not yet been
claimed. The active Qualia CNS was not replaced.

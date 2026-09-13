# Bounded pretrained visual observer on Jetson

The frozen pretrained visual model is running on the robot with actual camera
JPEGs. Its read-only status endpoint is `http://10.0.0.180:8091/status`. It uses
the installed CPU PyTorch, two numerical threads, and no wheel, head, gimbal,
light, transport, or QLSP output. The old CNS is not part of this observer.

The deployed run started around `2026-09-13T00:10:01Z` and is bounded to 1,800
seconds, ending around **2026-09-12 20:40 EDT**. Its supervisor is PID **42093**;
its runtime child is PID **42094**. The run ID is
`326e2ca0-4b4f-4a53-80b1-3f4f503b5acb`. Deployment and logs are confined to
`/home/jetson/qualia-flyvis-20260912`. No service configuration, existing Python
package, Qwen process, or actuator was changed by this deployment.

At saved tick 1,557, the observer was `running`, had acquired 1,560 camera
responses, used 277.6 MiB RSS, and reported 1,696.1 MiB available system memory.
That update computed in 126.3 ms. The graded voltage range was
`-2.2615888..2.6927114`, with mean absolute change `0.000436244` from the prior
computed state. These are model voltage units, not spikes or Hz. Qwen PID 1299
remained present. See the [saved status](../../docs/evidence/flyvis-camera-probe/jetson-live-status.json).

## Exact model export

The host exporter loads the same hash-verified public checkpoint as the
[bounded probe](README.md). It expands the model's bias, time constant, signed
edge weight, source/target indices, photoreceptor input indices, type labels,
hex coordinates, and official BoxEye positions into a compressed NumPy artifact.
The artifact is **863,043 bytes**; it is stored outside git.

`flyvis_frozen.py` evaluates the published instantaneous graded-release network
using its frozen exported parameters and rectified source voltages. Each 20 ms
step sums the signed graded synaptic input into each target and advances the
passive membrane equation, with the same effective time-constant floor used by
the official implementation. It adds luminance only at the exported input
indices. There is no parameter fitting, sign repair, extra tonic current, noise,
or fabricated activity.

The one artifact parity check used an actual previously captured robot JPEG.
The official and exported retinal inputs had maximum absolute difference **0**;
32 recurrent steps also had maximum voltage difference **0** on host CPU.
Loaded pretrained parameters remained unchanged. This checks the exported
computation against flyvis 1.2.0, not its biological accuracy or cross-platform
bitwise equivalence. The reference and artifact hashes are recorded in
[export-provenance.json](../../docs/evidence/flyvis-camera-probe/export-provenance.json).

The Jetson uses its existing Python 3.10.12, CPU torch 2.14.0+cpu, NumPy 2.2.6,
Pillow 9.0.1, and psutil 7.1.3. No new wheels or native builds were needed there.
The complete flyvis analysis dependency tree stays on the host.

## Timing and publication

Warmup holds the first real image for 50 neural steps from the learned resting
state. It is explicitly initialization, not 50 new camera frames. After warmup,
the observer advances with the previous measured image held over the next host
receipt interval, then stores the new image for the following interval. It
retains neural state between updates and labels model time separately from wall
time. The `source` object is the exact held input responsible for the published
`output`, while `latest_camera` may describe a newer receipt.

The observer preserves the camera producer's sequence, original Unix acquisition
and dequeue milliseconds, raw driver timestamp and flags, and timestamp basis.
Exposure time remains unknown. Repeated producer sequences do not generate a
new computed response. Camera failure clears output and breaks continuity;
recovery initializes from a new real image. Gaps over 1,500 ms also restart
initialization. The actual producer acquisition timestamp, when present, must
be at most 1,500 ms old and no more than 100 ms ahead of the robot clock at
receipt.

`/status` calculates receipt/input/output ages from the robot monotonic clock on
every request. A nominally running response becomes `stale` if any relevant age
exceeds 1,500 ms. Unix timestamps retain their original meaning and support
cross-process comparison; source receipt time is not presented as exposure time.
`/input.jpg` provides the exact JPEG associated with the latest computed output.
No HTTP method or endpoint controls the robot.

The same status is published atomically to the run's `status.json`, and computed
updates are appended to `activity.jsonl`. These runtime files and camera data
remain outside git. A separate two-second read-only context lane polls
`/telemetry/compact` and `/camera/lights`. Their payloads are explicitly
`measured_context`; they do **not** enter the pretrained visual network.

CPU execution is fixed to two threads. Loading requires at least 700 MiB
available; observation pauses below 350 MiB available or above 1,100 MiB model
RSS. A supervisor bounds the entire process to at most 1,800 seconds. Failed or
expired runs do not silently restart.

## Status contract

The schema is `qualia.flyvis-live.v1`. The console and read-only guidance lane
consume these fields:

| Field | Meaning |
| --- | --- |
| `run_id`, `started_unix_ns`, `deadline_unix_ns` | Stable identity and bounded observation interval |
| `published_unix_ns` | Time of this status publication, not neural computation time |
| `state`, `error` | `starting`, `waiting_for_camera`, `warming_up`, `running`, `stale`, `memory_hold`, `stopped`, or `failed` |
| `tick`, `camera_frames` | Completed neural updates and successful camera HTTP responses |
| `model` | Model name/member/version, official source, checkpoint/export hashes, node/edge counts |
| `source` | Actual held JPEG hash, request and receipt Unix nanoseconds, monotonic receipt age, original producer metadata, unknown exposure time |
| `latest_camera` | Most recently received camera input, potentially newer than the computed source |
| `output` | Completion timestamp/age, source receipt/hash/input age, model time, graded voltage range/mean/change, `per_type` statistics; null when unavailable |
| `output.per_type[]` | `cell_type`, `count`, `mean_voltage`, `min_voltage`, `max_voltage`, `std_voltage`, `mean_abs_delta_from_previous` |
| `compute_ms`, `memory_rss_mib`, `memory_available_mib`, `threads` | Measured execution/resource state |
| `activity_kind`, `identity_space` | `graded_model_voltage`; flyvis indices and types, never MaleCNS IDs |
| `parameters_frozen`, `controls_locked` | True; checkpoint is immutable and this observer has no actuator interface |
| `model_inputs`, `measured_context_drives_model` | `["camera_luminance"]` and false |
| `measured_context` | Independently timestamped telemetry/lighting observations, errors, and age |

`source.received_unix_ns` and `source.jpeg_sha256` match
`output.input_received_unix_ns` and `output.input_sha256`. `source` is an object
with null observation fields before the first camera input, so startup/error
states remain inspectable. Graded values can be negative and nonzero resting
activity is expected; neither implies a spike or an established wheel command.

## Selected cell-type visualization data

`GET /cells?type=T4a` returns the actual per-cell response for one model cell
type, with at most 721 cells per response. The selector can use real names from
`/status`'s `output.per_type` or `GET /cell-types`; examples include `R1` through
`R8`, `T4a` through `T4d`, and `T5a` through `T5d`. `/cell-types` supplies the
model/run identity and an array of `cell_type`/`count` pairs supported by this
bounded endpoint.

The response schema is `qualia.flyvis-cells.v1`. It includes `cell_type`,
`run_id`, `tick`, `state`, `published_unix_ns`, `activity_kind`, `identity_space`,
`parameters_frozen`, `controls_locked`, `model`, `source`, `output`, `error`, and:

```text
cells: [{model_index, u, v, voltage, delta_voltage}, ...]
```

`model_index` is the real index in this exported flyvis network. `u,v` are its
actual hexagonal model coordinates, not invented anatomical positions.
`voltage` is the current float32 model value; `delta_voltage` is the signed
change from the immediately preceding computed neural update. Neither is a
spike event. A view can show graded intensity or signed change on a clearly
labeled scale without synthesizing pulses or random activity.

One lock covers the current JPEG identity, source/output metadata, voltage
array, signed-change array, and update tick. The handler copies one selected
layer and its matching metadata under that same lock before serialization.
It does not retrieve independent status and neural snapshots from different
updates. The full cell arrays remain internal; neither `/status` nor the
activity journal gains a 45,669-cell JSON payload.

A 200 response contains fresh cells only. Unavailable, warming, failed, or stale
states return HTTP 503 with `cells: []`; an unknown type returns 404, and a
malformed/multiple-type query returns 400. Clients still check the source/output
timestamps and monotonic ages on receipt, since network delay can age any
response after publication. Existing 1,500 ms freshness thresholds and frozen
model equations remain unchanged.

## Reproducing the deployment artifact

From the isolated host CPU environment described in `README.md`:

```powershell
& "$probeState/venv/Scripts/python.exe" scripts/export_flyvis_frozen.py --archive "$probeState/results_pretrained_models.zip" --cache-dir "$probeState/model-cache-v2" --jpeg "$probeState/light-off-02/input-000.jpg" --out-dir "$probeState/jetson-export-01"
```

The output directory must be new. Copy the resulting `model.npz` and `model.json`
to the robot's isolated `model/` directory, and copy `flyvis_frozen.py` and
`flyvis_live_observer.py` to its `scripts/` directory. The deployed invocation was:

```text
python3 /home/jetson/qualia-flyvis-20260912/scripts/flyvis_live_observer.py --model-dir /home/jetson/qualia-flyvis-20260912/model --run-dir /home/jetson/qualia-flyvis-20260912/run-20260913T001001Z --base-url http://127.0.0.1:8000 --seconds 1800 --period-ms 200 --port 8091
```

It was launched as a detached process with its output in the isolated directory's
`observer.log`; no system service was installed. A fresh invocation must use a
new run directory and must not overlap an existing observer on port 8091.

This deployment supplies a real, live pretrained visual response. Teaching a
wheel policy, validating direction-selective behavior on the robot, or integrating
measured IMU/lidar into a biologically justified model interface remains separate
work. No learned driving claim follows from this observation run.

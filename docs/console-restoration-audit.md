# Console restoration audit

Status: in progress, 2026-09-12. The operator requested all pre-rewrite
functionality, simultaneous separate dialogs, and measured multimodal input
to the fly rather than an apparently active display disconnected from wheels.

## Verified failures

- #111 closed the web-assets rewrite with a page that only polls `/braid`.
  The installed July 20 interface has camera/VSLAM views, spatial data and
  routes, belief activity/weight-delta matrices, arena events and controls.
- The rewritten agent registers numerous routes through `pending.rs`, which
  returns 503 for their missing subsystems. World, belief/sketch, arena,
  session-store, learning and compute-job surfaces are among them.
- Its MCP dispatcher also refuses the advertised tools. Route registration
  does not satisfy the previous behavioral contract.
- The installed host is still the older release. Its Leash URL points to an
  obsolete hostname, and its empty allowed-remote address disables the Leash
  reader. A prepared configuration backup/update/restart requires Windows
  administrator approval. The first UAC prompt was canceled; no host config
  mutation occurred.
- The old CNS camera encoder injects eight brightness values directly into
  95,494 photoreceptor and optic-intrinsic cells. It uses no IMU, range or
  odometry input. Its decoder provides pivot proposals, not demonstrated
  navigation. A camera timeout exits the producer, leaving a stale stream.

## Recovery and remaining acceptance

| Surface | Current recovery | Acceptance still required |
| --- | --- | --- |
| Camera | Direct robot JPEG retrieval, age and failure state | Producer capture timestamp; all configured entity cameras |
| Lidar and occupancy | Live timestamped returns and measured per-scan grid | Accumulated mapping and calibration; live reconnect verified |
| Brain | Published CNS spike IDs, stale highlighting removed, TCP reconnect | Calibrated optical mapping and downstream sensory-to-motor dynamics |
| Seven layers | Robot/host readings with aging; host sketch activity/delta views | Reconnect host, show advancing L3â€“L6 and persistence |
| World/route | Costmap points and paths, top/perspective | Nonempty real world producer, persistent map and applied route evidence |
| Perception | Source status, objects and landmark counts | Real entity camera/VSLAM and detection producers |
| Mission | Arena events, model influence, Observe | Restore and verify control dispatch after motion restriction clears |
| Safety/evidence | Recent applied-action ledger and reported safety state | Three #262 physical demonstrations, still blocked |
| Compute/session/learning | Existing readings and recording view | Restore missing backend workflows and verify their results |

The console recovery is in `fix/console-live-robot`. Sensor-loop work starts
from the reviewed driving shim in `fix/fly-live-sensor-fusion`; it must not
replace that loop with the console branch's older CNS implementation.
The user's modified root worktree remains untouched.

## Evidence limits

Before the network stopped responding, the native console's accessibility
tree reported live JPEG retrieval and lidar at 10 Hz with 257â€“262 returns.
The host reported `ready: false`, L3â€“L6 sequence zero and no world observations.
The robot had timestamped IMU, lidar and odometry, but localization remained
initializing. These are measured limitations, not successful mapping or driving.

The package build and focused tests cover the changed implementation only.
They do not close the parity audit. Do not mark this work complete because a
panel can render a fixture or an unavailable endpoint has a route name.

## Live recovery checkpoint

2026-09-12 17:50 America/New_York: native accessibility inspection and a full
console capture showed all sixteen separate dialogs coexisting. Camera JPEG
retrieval was live; lidar reported 259 returns at 10 Hz; the brain received
model frames at 5.2 frames/s. The Fly inputs dialog displays measured timestamps,
IMU, odometry derivative, proximity, conditioned camera features and hold reason.
The direct live mode does not display the internal braid fixture when the host
cannot supply a braid. Recorded-example tests are separate from live sources.

A four-minute observation excerpt contained 1,151 measured input bundles and
1,151 paired outputs, no acquisition failures, and zero nonzero decoded commands.
IMU timestamps advanced from 1789249575794 to 1789249815447; scan timestamps
advanced from 1789249575740 to 1789249815402. The latest measured nearest return
was 0.156 m, inside the explicit 0.25 m hold. Wheel transport was disconnected.

The model now stimulates only 6,091 annotated receptors. The 28 with positions
use a documented soma-column proxy; 6,063 without retinal mapping receive a
uniform measured visual feature. All receptor outgoing signs in the artifact
are inhibitory, and the model has zero resting activity downstream. Therefore
this run establishes measured input and receptor model spikes, not valid
sensory-to-motor behavior or wheel application. #262 remains open.

The host integration still reports ready=false and empty L3-L6/world data.
Its administrator repair prompt was canceled. That configuration repair,
backend parity, calibration/model dynamics and operator motion confirmation
remain outstanding. No host restart or wheel command was sent.

Source run and local capture hash: [live evidence summary](evidence/console-live-20260912.json).
The capture remains local; no camera image is committed.

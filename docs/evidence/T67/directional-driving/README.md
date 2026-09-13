# Live directional driving, 2026-09-12

The operator confirmed presence, clear space and motion readiness, then identified
that the centered camera's forward corner appeared in the scan's south quadrant.
Leash's scan yaw was changed from its default 180 degrees to 270 degrees. The body
mask moved from 45:136 to 135:226, preserving the same masked raw lidar rays. This
is an operator-observed forward reference, not metric camera extrinsics or a
verification of lateral handedness. The console now displays body +X upward and
+Y left for both lidar and occupancy. The operator confirmed the direction.

Leash already had directional collision checks. The redundant all-around hold
was in the Qualia readout. The new explicit directional mode checks its forward
120-degree sector instead: at least 75% measured coverage and 0.25 m clearance.
Actual forward coverage after alignment was 121/121, with nearest return 1.42 m.
The approximately 0.16 m rear return remains reported. The bridge forwards the
producer's original emitted frame; held zeros are never converted into proposals.

Two real low-mode transport runs accepted nonzero model-derived frames:

| Local run | Session label | Nonzero frames forwarded | First applied L/R (m/s) |
| --- | --- | ---: | --- |
| 22:27:44 | 82378755 | 6 | 0.01582 / 0.01491 |
| 22:31:06 | See JSON record | 4 | 0.04930 / 0.04866 |

The second run used explicit cruise gain 0.08 m/s and wheel cap 0.1 m/s, below
Leash's low-mode cap of 0.22. Leash's own applied records 897 through 912 show
nonzero wheels, armed true, safety_flags 0, collision_clamped false and
deadman_active false. Encoder readings changed from odl=-2/odr=0 to odl=-5/odr=-2
(firmware centimetres). Both runs ended with verified zero and inactive leases.
Firmware encoder signs are not normalized by Leash's outgoing drive inversion;
the negative pose is not proof of backwards physical travel.

The second run paused when the producer emitted zero for stale camera input.
The zero transition was not a Leash collision/deadman refusal. A separate bug
also erased valid odometry history on camera-only errors; this is corrected.
Neural computation latency is being reduced before a longer integration.

The JSON files quote original frame provenance, transport acknowledgement,
Leash applied evidence and before/after telemetry. The global ledger is not
claimed to carry the bridge session label. No bearer or pilot token is stored.
The model remains frozen, its wheel readout is engineered, and DeepSeek remains
advisory. This is not a learned navigation or metric VSLAM demonstration.

Ticket #262 remains open: sustained exploration and the requested explicit
deadman/estop demonstrations are not established by these short runs. The
500 ms arrival deadman, verified stop and lease termination remain implemented.

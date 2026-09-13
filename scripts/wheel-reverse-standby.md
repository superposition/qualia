# Directional reverse readout — standby

`wheel_shadow.py --reverse-escape` adds an optional, engineered reverse
proposal while continuing to write only the shadow's frame file. It has **not
been deployed**. The installed and checked-in service unit does not enable the
flag. Physical transport remains a separate operator action.

The mode assumes the published scan frame has been established as body-forward
at zero radians. That assumption is currently disputed by the operator's
physical observation and is not established by the recorded calibration.
Do not enable the mode until this body-to-scan relationship is resolved.
The camera's screen direction, the plot's axes, and wheel sign are separate
questions; a visual rotation alone does not correct the backend collision gate.

The actual console plots `(range*cos(angle), range*sin(angle))` with positive
x to the right and positive y upward. Thus scan zero points right on the
screen and minus 90 degrees points down. The current LD06 configuration uses
yaw offset 180 degrees, clockwise false, and an existing 45:136 degree body
mask. Those defaults are not a measured mounting transform. Operator-observed
drive mapping does establish logical positive as forward and negative as
reverse; Leash applies its configured inversion only at the final serial
encoding stage. Do not invert that logical drive sign a second time.

When explicitly enabled after alignment, the new body summary includes
`forward_sector` and `reverse_sector`, each with `center_deg`,
`half_width_deg:60`, `total_beams`, `valid_beams`, `unknown_beams`,
`coverage_fraction`, and `min_return_m`. These use the same forward-zero and
reverse-pi sectors as the existing Leash gate. Missing beams remain unknown.
All original sensor timestamps, skew checks, gravity/yaw envelopes, and neural
freshness requirements remain in force.

A reverse proposal is eligible only when a real forward-sector return is
inside the unchanged 0.25 m threshold, the reverse-sector minimum is at least
0.25 m, and reverse coverage is at least the operator-selected 0.75 fraction.
Current raw-frame data had 96/121 reverse bins valid, with 25 unknown and a
1.42 m minimum. It cannot yet be called physical rear clearance because the
scan-to-body alignment remains unresolved.

For an eligible reverse, both proposed wheels are negative and bounded to
0.02 m/s. The reverse magnitude retains measured T4a change strength and
odometry damping; the differential retains actual T4a horizontal-change
centroid and gyro damping but is limited to one quarter of reverse magnitude.
This prevents the proposal from becoming an in-place turn. It is engineered
avoidance, not a learned motor behavior.

`proposed.direction` is `reverse_escape` or `forward`,
`proposed.clearance_sector` is `reverse` or `all_around`, and
`proposed.selected_clearance_m` reports the actual selected minimum. The
original all-around `body.nearest_return_m` stays visible. All other hold
reasons still emit zero. A transport integration must explicitly validate the
directional eligibility and cannot reuse an unrelated clearance override.

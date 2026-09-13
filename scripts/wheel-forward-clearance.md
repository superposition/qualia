# Forward directional clearance

`wheel_shadow.py --directional-clearance --max-speed 0.03` enables a
file-only forward proposal using the published body-forward scan sector.
The existing service launcher accepts the same flags for `--role shadow`.
The checked-in unit leaves them disabled; root enables the installed unit
after its scan alignment deployment. Do not add `--reverse-escape` for this run.

The operator identified camera-forward as the old scan's minus-90-degree
direction. Root reported installing LD06 yaw 270 degrees and body mask
135:226, rotating the previous scan by plus 90 degrees while preserving the
same physically masked rays. This is operator alignment, not an independently
measured extrinsic calibration. Logical positive wheel commands remain forward.

An eligible proposal requires at least 75% valid measurements in the forward
sector, centered at zero with a 60-degree half width, and its minimum measured
range must be at least 0.25 m. Unknown beams remain counted as unknown. The
original all-around nearest return remains visible, including a close rear
return. All camera/body timestamp checks, initial odometry hold, sensor skew,
gravity and yaw disagreement gates remain in force. Every hold emits zero.

The engineered forward magnitude retains actual T4a change strength and
odometry damping. The differential retains the measured neural centroid and
gyro damping, limited to one quarter of forward magnitude so both wheel
proposals remain positive. Each wheel is capped at the explicit `--max-speed`
setting (0.03 m/s for the initial integration above). Frozen visual
weights and equations are unchanged; this readout is not learned control.

The status contract adds `directional_clearance_enabled` and
`body.forward_directional_eligible`. The existing `body.forward_sector`
contains `center_deg:0`, `half_width_deg:60`, total/valid/unknown beam counts,
`coverage_fraction`, and `min_return_m`. An eligible `proposed` reports
`direction:"forward"`, `clearance_sector:"forward"`, and
`selected_clearance_m` equal to that sector minimum. A separate transport must
validate these fields against the original emitted frame and all original
freshness/body gates. This service does not attach transport or acquire motor
credentials.

Source was statically reviewed; no additional test cycle or motion was run by
this change's author. Deployment and supervised physical execution are owned
by the operator's primary agent.

## Configurable cruise gain

`--cruise-speed` defaults to 0.025 m/s and controls the engineered forward gain:
`cruise * (0.5 + 0.5 * neural_strength) / (1 + measured_odom_speed / 0.05)`.
Here `neural_strength = mean_abs_delta / (mean_abs_delta + 0.002)` uses actual
T4a changes. Both cruise and wheel cap must be finite and within 0..0.1 m/s.
The status and service metadata expose `cruise_speed_mps` and `max_speed_mps`.
The reverse-escape ceiling remains 0.02 m/s and its flag remains disabled.

After the first supervised run applied roughly 0.015..0.017 m/s but showed
little measured travel, root requested the prepared next setting:
`--directional-clearance --cruise-speed 0.08 --max-speed 0.1`. This yields
0.04..0.08 m/s nominal forward magnitude before measured odometry damping,
plus the same bounded actual neural/gyro differential. The selected forward
sector still requires 0.25 m clearance and 75% valid measurements. No neural
weights, equations, or activity were fabricated to raise this engineered gain.
The source change does not deploy or start a physical run. Raw encoder signs
alone do not establish physical travel direction.

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
proposals remain positive. Each wheel is capped at 0.03 m/s. Frozen visual
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

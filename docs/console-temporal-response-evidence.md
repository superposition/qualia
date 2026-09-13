# Measured eye input and temporal response

The Brain dialog defaults to each selected cell's published signed voltage
change between consecutive producer outputs. Cyan means an increase and orange
means a decrease, with a fixed initial full scale of ±0.003 model units. The
absolute-voltage view remains selectable. Neither view synthesizes spikes,
firing rates, pulses, or intermediate samples.

The neighboring eye plot reads the compact `/retina` endpoint's actual R1
`input_drive` values, using black 0 / white 1. Both plots use the exported
BoxEye layout, `x = v`, `y = u + v/2`, with image y downward. The common scale
factor cancels during fitting. The model's coordinate layout does not establish
robot-to-camera calibration. Each independently polled plot shows its own tick;
the UI claims a shared image and tick only when both fields match.

The temporal traces show measured R1 mean absolute voltage change and the
cell-count-weighted mean for the eight T4/T5 types. Histories contain at most
180 distinct fresh outputs, reset with the producer run, and leave gaps across
missing intervals. The eight type bars use the actual published summaries.
Their names are not assigned unverified robot directions. Historical samples
turn gray, and the latest exact numeric values remain inspectable.

`qualia.flyvis-retina.v1` is polled at a 500 ms minimum interval through the
existing bounded HTTP client. The route shares run/model/source/output identity
and freshness fields with the cell endpoint. At most 721 unique, finite cells
are accepted. The full R1 matrix endpoint is not polled for this view.

## Body-frame display

Lidar points and current-scan occupancy now share a display-only projection:
`screen_x = -body_y`, `screen_y = -body_x`. Body +X points up and +Y points left.
Both overlays have the same upward arrow and explicit forward/back/left/right
labels. Parsing retains the measured body coordinates, and neither the raw rays
nor the robot's collision policy is changed by this UI patch. The display says
the reference is operator-aligned, not a metric calibration. The robot-side yaw
and occlusion-mask changes are separate operator work.

The wheel panel accepts the separately authorized bridge's reported speed cap
through 0.1 m/s and displays that cap; this does not grant the UI motor authority.
Forward-sector minimum, coverage, unknown beams, and the readout's selected
clearance are shown separately from the all-around nearest return. A usable
forward sector is not labeled as all-around clearance.

## Verification

The final cached build passed in 5.91 seconds:

```powershell
$env:CARGO_TARGET_DIR='C:/Users/ericm/qualia/target'
cargo build --offline --locked -p qualia-console --bin qualia-console -j 2
```

Console artifact SHA-256:
`37d56c5eb44c1034006d28e1a2825fc082f436223af1692a45494d32ce2f0220`.
No additional tests, GPU jobs, model updates, or actuator commands were run by
the UI agent. Existing occupancy assertions were updated to the new display
indices. `git diff --check` passed.

Read-only endpoint inspection observed run
`49458c95-e2fd-4d0a-839b-27d884316c21`, tick 773, and 721 cells on both routes.
The compact retinal payload was 71,713 bytes, with actual input drive from
0.112499 to 0.631194. T4a signed change ranged from -0.00467330 to 0.00511569;
mean absolute change was 0.00102312. Values beyond the displayed full scale
saturate in color while their numeric values remain available.

The previous temporal-only binary was inspected in a maximized console window:
eye input, signed changes, real histories, all eight bars, and the other six
dialogs fit together. The sampled camera/model source had become stale during
that capture and was correctly shown in gray. No control was invoked. The
private window captures are retained outside Git because they include camera
imagery. Final refreshed-window evidence is recorded separately below.

The refreshed process was PID 5736 with the exact artifact hash above. UIA
observed 721 current T4a cells and retinal tick 866 with matching image/tick,
mean absolute change 8.99e-4, and peak change 3.85e-3. A subsequent painted
window capture showed the live cyan/orange response at tick 1014, the grayscale
retinal image, the colored R1/T4+T5 histories, and all eight real type bars.
The existing freshness gates remained active; other captures truthfully show
gray historical samples when their age exceeded the limit.

The forward-up point scan was inspected alongside a temporarily opened
Occupancy dialog. Both placed the measured corner ahead at the top and used
the same reference arrows. The temporary Occupancy dialog was then closed.
Only display selection and window inspection were performed by this agent.

The wheel UI displayed a reported 0.100 m/s cap, all-around nearest 0.158 m,
and forward-sector nearest 1.413 m with 100% coverage and zero unknown beams.
These are dated readings, not a continuing clearance guarantee. The transport
was historical after the separately managed run; its consumed tick 149 does
not by itself establish physical movement. Model-guidance text was also
flagged to the broker owner because low visual change or a directionless
nearest return cannot justify calling a driving step safe.

Machine-readable readings and private-capture hashes are in
[console-temporal-response-20260912.json](evidence/console-temporal-response-20260912.json).

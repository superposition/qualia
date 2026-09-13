# Live pretrained vision on the Jetson

At 2026-09-12 20:14 America/New_York, the frozen flyvis visual network was running
on Pinkie at `http://10.0.0.180:8091/status`. It uses the real robot camera, 45,669
model cells and 1,513,231 edges. This is graded voltage activity, not MaleCNS
spikes or a trained wheel controller. The only neural input is camera luminance;
IMU, lidar, odometry and lighting are separately reported measured context.

The official pretrained checkpoint was expanded into fixed arrays for the
existing Jetson CPU PyTorch installation. The export's retinal transform and
32-step voltage result matched the upstream implementation with maximum absolute
error zero. Parameters remain frozen; the Qwen process and GPU were left alone.
The original source/checkpoint/export hashes are retained in the status evidence.

An independent root read observed tick 1350, 1353 camera acquisitions, compute
121.37 ms, memory 280.95 MiB, matching source/output image age 450.49 ms and output
age 20.64 ms. The run is `326e2ca0-4b4f-4a53-80b1-3f4f503b5acb`, bounded through
approximately 20:40 America/New_York. Its supervisor is PID 42093. The older
connectome CNS run ended and was not restarted concurrently.

Console PID 66384 exposes the live model as its first expanded dialog, with
source/output ages, graded voltage history and per-cell-type values. UI
automation read live tick 1323 and 58 actual received outputs from that dialog.
Camera and Lighting dialogs coexist. The old CNS stream is identified as ended
and unused by the pretrained model. No fixture spikes were substituted.

DeepSeek's read-only observer was restarted against this exact pretrained status
feed. Its local `/coach` status reported `deepseek-flash`, a successful actual
response, latency 2111 ms and no error. Advice is shown in the console; it does
not update model weights, beliefs, wheel control or navigation state.

Automatic camera lighting had already triggered a camera PWM 180 command after
sustained darkness; the operator confirmed seeing the light. The deployed shared
V4L2 owner permits simultaneous video and snapshots. The visual frontend then
reported 42 actual features, contrast 0.0579 and luma 0.2110. This is an improvement
in image usability, not evidence of metric SLAM, map coverage or successful driving.

No new test suite was run for this deployment; the user requested that completed
work be shipped directly. The combined console/coach build succeeded, and runtime
state was read from the real services. Physical controls remain locked because
the operator has not confirmed the acknowledgement repair.

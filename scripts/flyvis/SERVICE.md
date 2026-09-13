# Ongoing read-only observation

Two owned systemd **user** units renew the existing bounded workers:
`qualia-flyvis-vision.service` and `qualia-flyvis-shadow.service`. The visual
model and equations are unchanged. Each worker still runs for at most 1800
seconds; systemd starts the next interval with a new run directory and identity.
There is a brief warming/reconnection interval at renewal. The same HTTP
freshness rules and explicit unavailable states remain visible.

The shadow always writes to its own frame file. These services launch no
physical transport, acquire no lease, and inherit no motor credentials. The
launcher supplies only a small public environment: executable paths, home,
locale, unbuffered output, CPU thread limits, and disabled CUDA visibility.
`qualia-qwen.service`, Leash, and other services are untouched.

Read-only inspection on this robot found the user manager running and
`Linger=yes`, so ongoing user services already survive SSH disconnection.
No change to lingering or system services is required.

## Installation for root review

Stage the launcher and units into the existing owned directory:

```powershell
scp scripts/flyvis_service.py pinkie:/home/jetson/qualia-flyvis-20260912/scripts/
scp scripts/flyvis/systemd/qualia-flyvis-vision.service scripts/flyvis/systemd/qualia-flyvis-shadow.service pinkie:/home/jetson/qualia-flyvis-20260912/
```

Before starting, retire only the previously verified manual observer processes.
At preparation time they were vision supervisor/worker 73646/73647 and shadow
84004/84008; re-read each `/proc/PID/cmdline` and confirm the exact owned script
before terminating it. These numbers are historical, not unconditional kill
targets. The launcher refuses an existing same-role process or occupied
8091/8092 port, and holds a per-role `flock` for its entire lifetime.

After root authorization, install **only the two new user units**:

```sh
mkdir -p /home/jetson/.config/systemd/user
install -m 0644 /home/jetson/qualia-flyvis-20260912/qualia-flyvis-vision.service /home/jetson/.config/systemd/user/qualia-flyvis-vision.service
install -m 0644 /home/jetson/qualia-flyvis-20260912/qualia-flyvis-shadow.service /home/jetson/.config/systemd/user/qualia-flyvis-shadow.service
systemctl --user daemon-reload
systemctl --user enable --now qualia-flyvis-vision.service qualia-flyvis-shadow.service
```

One actual startup/status observation is sufficient for this deployment; no
model rebuild, training, synthetic input, or extra test cycle is needed.
`systemctl --user stop UNIT` stops its full owned process group and suppresses
automatic restart for that explicit stop. `disable --now` also removes its
autostart. `Restart=always` renews normal bounded exits and recovers unexpected
worker failures after two seconds.

## Ownership, status, and retention

Only `ROOT/service-runs/vision-*` and `ROOT/service-runs/shadow-*` are created.
The current role pointers are `ROOT/service-vision.json` and
`ROOT/service-shadow.json`; they report role, run directory, launcher/supervisor
PIDs, interval start/end, and actual exit code. Existing `/status`, `/cells`,
`/matrices`, and wheel-shadow `/status` remain the application interfaces.
Application response timestamps are never renewed by the service wrapper.

At the next launch, retention keeps the two most recent **completed** runs of
that role and deletes older completed directories only when they are direct,
nonsymlink children of `service-runs` and carry this launcher's matching
`service-owner.json` marker. Incomplete directories are preserved for diagnosis.
The active run is additional. All previous manual, experimental, image, model,
and evidence directories are outside this namespace and are never pruned.

The physical wheel bridge remains a separate bounded operator execution. These
services do not start it, change its zero-only default, renew an operator lease,
or authorize movement when the camera/shadow interval renews.

## Actual deployment

Root approved installation and the two units were enabled and started on
2026-09-13 at approximately 01:35:01 UTC. The four manual observer PIDs were
rechecked against their exact owned script paths before retirement. No Qwen,
Leash, physical transport, or experimental/evidence directory was changed.

One actual post-start observation found both units `active/running`, enabled,
and at zero restarts. Vision used launcher/supervisor/worker PIDs
110744/110747/110748 and run `18d7da14-b899-479c-bc9b-cbaf5426eb50`; its matched
camera/output snapshot at tick 58 had input age 933.5 ms, output age 44.0 ms,
RSS 280.7 MiB, and the unchanged checkpoint/export hashes. Shadow used PIDs
110745/110746/110749 and run `acd09e88-bd31-401e-a566-4be6df6c0ca9`; tick 139
emitted exact zero for a measured 0.160 m nearest lidar return. Qwen remained
active. These current 1800-second intervals end around 02:05:01 UTC, after
which the configured units renew them; no additional restart/test cycle was
performed. The sanitized status and process evidence is committed at
`docs/evidence/flyvis-camera-probe/service-deployment-evidence.json`.

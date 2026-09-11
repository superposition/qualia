# Leash deployment

Leash is the motion authority. Qualia never issues motor output; it reaches
Leash over HTTP through `QUALIA_LEASH_BASE_URL`.

`camera-stream.conf` is a systemd fragment for the camera stream that feeds
Leash. It reads its settings from `~/.config/leash/camera-stream.env`, which is
created on the target machine and is never committed.

`camera-stream.env.example` documents the keys that file must define.

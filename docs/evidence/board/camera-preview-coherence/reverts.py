#!/usr/bin/env python3
"""Regenerate the scratch reverts behind this directory's falsification runs.

Each mode rewrites one file of the tree in place, removing the ordering the fix
added (or materialising the reordering the fence prevents), so the same board
command can be run against it:

  camera-fence-only   drop the acquire fence from `CameraPreview::snapshot`
  camera-reordered    drop it and take the closing sequence load before the copy
  agent-pre-fix       put the hand-rolled, fence-less preview reader back in
                      `GET /perception/frame`
  agent-reordered     that reader with its closing load taken before the copy

The tree must have LF endings (a `git archive` extract); the anchors are LF.
Usage: python3 reverts.py <mode> <tree>
"""

import re
import sys

CAMERA = "crates/types/src/lib.rs"
AGENT = "runners/agent/src/perception.rs"

CAMERA_COPY = """            let mut bytes = Vec::with_capacity(len);
            unsafe {
                std::ptr::copy_nonoverlapping(self.bytes.as_ptr(), bytes.as_mut_ptr(), len);
                bytes.set_len(len);
            }
            fence(Ordering::Acquire);
            let after = self.seq.load(Ordering::Acquire);"""
CAMERA_COPY_REORDERED = """            // The closing load is taken before the copy completes: the
            // reordering aarch64 is permitted to make without the fence.
            let after = self.seq.load(Ordering::Acquire);
            let mut bytes = Vec::with_capacity(len);
            unsafe {
                std::ptr::copy_nonoverlapping(self.bytes.as_ptr(), bytes.as_mut_ptr(), len);
                bytes.set_len(len);
            }"""

AGENT_HANDLER = re.compile(
    r"pub async fn encoded_frame_get\(State\(state\): State<AppState>\) -> Response \{.*?\n\}\n",
    re.S,
)

AGENT_LOOP_HEAD = """    let preview = region.camera_preview();
    for _ in 0..3 {
        let before = preview.seq.load(std::sync::atomic::Ordering::Acquire);
        if before == 0 || before % 2 != 0 {
            continue;
        }
        let len = preview
            .len
            .load(std::sync::atomic::Ordering::Acquire)
            .min(qualia_types::CAMERA_PREVIEW_MAX_BYTES);
        if len == 0 {
            continue;
        }
        let format = preview.format;
"""
AGENT_LOOP_BODY = """        if before != after || after % 2 != 0 {
            continue;
        }
        let content_type = match format {
            1 => "image/jpeg",
            2 => "image/png",
            _ => return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response(),
        };
        return (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, "no-store"),
            ],
            Bytes::from(body),
        )
            .into_response();
    }
    StatusCode::SERVICE_UNAVAILABLE.into_response()
"""
CLOSING_IN_ORDER = """        let body = preview.bytes[..len].to_vec();
        let after = preview.seq.load(std::sync::atomic::Ordering::Acquire);
"""
CLOSING_REORDERED = """        // The closing load is taken before the copy completes: the
        // reordering aarch64 is permitted to make without the fence.
        let after = preview.seq.load(std::sync::atomic::Ordering::Acquire);
        let body = preview.bytes[..len].to_vec();
"""


def agent_handler(reordered: bool) -> str:
    closing = CLOSING_REORDERED if reordered else CLOSING_IN_ORDER
    return (
        "pub async fn encoded_frame_get(State(state): State<AppState>) -> Response {\n"
        "    let Some(region) = state.shm_opt() else {\n"
        "        return StatusCode::SERVICE_UNAVAILABLE.into_response();\n"
        "    };\n"
        + AGENT_LOOP_HEAD
        + closing
        + AGENT_LOOP_BODY
        + "}\n"
    )


def read(path: str) -> str:
    with open(path, "r", encoding="utf-8", newline="") as handle:
        return handle.read()


def write(path: str, text: str) -> None:
    with open(path, "w", encoding="utf-8", newline="") as handle:
        handle.write(text)


def main() -> None:
    mode, tree = sys.argv[1], sys.argv[2]
    camera, agent = f"{tree}/{CAMERA}", f"{tree}/{AGENT}"
    if mode in ("camera-fence-only", "camera-reordered"):
        text = read(camera)
        if text.count(CAMERA_COPY) != 1:
            raise SystemExit(f"{camera}: anchor appears {text.count(CAMERA_COPY)} times")
        if mode == "camera-fence-only":
            replacement = CAMERA_COPY.replace("            fence(Ordering::Acquire);\n", "")
        else:
            replacement = CAMERA_COPY_REORDERED
        write(camera, text.replace(CAMERA_COPY, replacement))
    elif mode in ("agent-pre-fix", "agent-reordered"):
        text = read(agent)
        if len(AGENT_HANDLER.findall(text)) != 1:
            raise SystemExit(f"{agent}: handler not found exactly once")
        write(agent, AGENT_HANDLER.sub(agent_handler(mode == "agent-reordered"), text, count=1))
    else:
        raise SystemExit(f"unknown mode {mode}")


if __name__ == "__main__":
    main()

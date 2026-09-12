"""Bounded, offline flyvis inference from recorded robot JPEGs. No actuator client."""

from __future__ import annotations

import argparse
import csv
import hashlib
import importlib.metadata
import io
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import time
import urllib.parse
import urllib.request
import zipfile


FLYVIS_VERSION = "1.2.0"
SOURCE_REVISION = "92b3845cc426dd309a1a0e1b3890156c42e14021"
ARCHIVE_SHA256 = "71c78d4070556a536b13b23ee3139cd2788aa2a9d07d430a223b4edead281db1"
ARCHIVE_BYTES = 3417042
MODEL_ID = "flow/0000/000"
MODEL_PREFIX = f"results/{MODEL_ID}/"
CHECKPOINT_MEMBER = MODEL_PREFIX + "chkpts/chkpt_00000"
CONFIG_MEMBER = MODEL_PREFIX + "_meta.yaml"
MAX_JPEG_BYTES = 8 * 1024 * 1024
MAX_PIXELS = 4096 * 4096
DT = 0.02
WARMUP_STEPS = 50
MAX_STEPS = 500


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, indent=2, allow_nan=False) + "\n", encoding="utf-8")


def snapshot_url(value: str) -> str:
    parsed = urllib.parse.urlsplit(value)
    if (parsed.scheme not in ("http", "https") or not parsed.hostname
            or parsed.username or parsed.password or parsed.query or parsed.fragment
            or parsed.path != "/camera/snapshot"):
        raise argparse.ArgumentTypeError("requires an HTTP(S) /camera/snapshot URL without credentials/query")
    return value


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError("camera redirects are refused")


def read_jpeg(data: bytes):
    from PIL import Image, ImageOps

    if not 0 < len(data) <= MAX_JPEG_BYTES:
        raise ValueError("JPEG size exceeds the 8 MiB limit or is empty")
    with Image.open(io.BytesIO(data)) as source:
        if source.format != "JPEG" or source.width * source.height > MAX_PIXELS:
            raise ValueError("input must be a JPEG of at most 4096 squared pixels")
        source.load()
        return ImageOps.exif_transpose(source).convert("RGB")


def capture(args, output: Path) -> tuple[list[dict], list]:
    """Record host receipt times; do not invent sensor exposure timestamps."""
    frames, images = [], []
    opener = urllib.request.build_opener(NoRedirect)
    next_request = time.monotonic()
    for index in range(args.frames if args.camera_url else 1):
        if args.camera_url:
            time.sleep(max(0.0, next_request - time.monotonic()))
        started_ns, start_mono_ns = time.time_ns(), time.monotonic_ns()
        if args.camera_url:
            request = urllib.request.Request(args.camera_url, headers={"Cache-Control": "no-cache"})
            with opener.open(request, timeout=5) as response:
                if response.status != 200:
                    raise ValueError(f"camera returned HTTP {response.status}")
                data = response.read(MAX_JPEG_BYTES + 1)
                content_type = response.headers.get_content_type()
            if content_type != "image/jpeg":
                raise ValueError(f"camera content type is {content_type!r}, expected image/jpeg")
        else:
            with args.jpeg.open("rb") as source:
                data = source.read(MAX_JPEG_BYTES + 1)
        received_ns, received_mono_ns = time.time_ns(), time.monotonic_ns()
        image = read_jpeg(data)
        filename = f"input-{index:03d}.jpg"
        (output / filename).write_bytes(data)
        record = {
            "index": index, "file": filename, "sha256": digest(data), "bytes": len(data),
            "width": image.width, "height": image.height,
            "source": args.camera_url if args.camera_url else str(args.jpeg.resolve()),
            "request_started_unix_ns": started_ns if args.camera_url else None,
            "received_unix_ns": received_ns,
            "received_monotonic_ns": received_mono_ns,
            "request_elapsed_ms": (received_mono_ns - start_mono_ns) / 1e6,
            "camera_exposure_unix_ns": None,
            "timestamp_basis": "host HTTP receipt" if args.camera_url else "host file read; acquisition time unknown",
        }
        frames.append(record)
        images.append(image)
        write_json(output / "inputs.json", frames)
        next_request = time.monotonic() + args.interval_ms / 1000
    return frames, images


def replay_schedule(frames: list[dict], hold_steps: int) -> list[tuple[int, float]]:
    """Zero-order hold on host receipt intervals; each tuple is one 20 ms step."""
    if len(frames) == 1:
        return [(0, step * DT) for step in range(hold_steps)]
    origin = frames[0]["received_monotonic_ns"]
    offsets = [(frame["received_monotonic_ns"] - origin) / 1e9 for frame in frames]
    if any(b <= a for a, b in zip(offsets, offsets[1:])):
        raise ValueError("camera receipt times must increase strictly")
    steps = math.ceil(offsets[-1] / DT) + 1
    if steps + WARMUP_STEPS > MAX_STEPS:
        raise ValueError("recorded sequence exceeds bounded integration budget")
    schedule, frame_index = [], 0
    for step in range(steps):
        at = step * DT
        while frame_index + 1 < len(offsets) and offsets[frame_index + 1] <= at:
            frame_index += 1
        schedule.append((frame_index, at))
    return schedule


def trusted_model(archive: Path) -> tuple[bytes, bytes, dict]:
    if archive.stat().st_size != ARCHIVE_BYTES:
        raise ValueError("pretrained archive size differs from the pinned release")
    raw = archive.read_bytes()
    if digest(raw) != ARCHIVE_SHA256:
        raise ValueError("pretrained archive SHA256 differs from the pinned release")
    with zipfile.ZipFile(io.BytesIO(raw)) as bundle:
        config = bundle.read(CONFIG_MEMBER)
        checkpoint = bundle.read(CHECKPOINT_MEMBER)
    return config, checkpoint, {
        "archive_sha256": ARCHIVE_SHA256, "archive_bytes": len(raw),
        "archive_public_file_id": "13cJr2nMn89j-jBAd5RduYRJpBcXwoNrC",
        "checkpoint_member": CHECKPOINT_MEMBER, "checkpoint_sha256": digest(checkpoint),
        "config_member": CONFIG_MEMBER, "config_sha256": digest(config),
        "selection": "published ensemble member 000; supplied checkpoint 00000; not claimed best biological fit",
    }


def install_windows_hdf5_writer() -> bool:
    """Work around datamate 1.0.0 unlinking an open HDF5 file on Windows."""
    if sys.platform != "win32":
        return False
    import datamate.directory
    import datamate.io
    import h5py
    import numpy as np

    def write_array(path, value):
        path = Path(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        with h5py.File(path, "w", libver="latest") as target:
            target.create_dataset("data", data=np.asarray(value))
            target.swmr_mode = True

    datamate.io._write_h5 = write_array
    datamate.directory._write_h5 = write_array
    return True


def worker(args) -> dict:
    started = time.monotonic()
    output = args.out_dir.resolve()
    config_bytes, checkpoint_bytes, model_provenance = trusted_model(args.archive)
    if importlib.metadata.version("flyvis") != FLYVIS_VERSION:
        raise ValueError(f"requires flyvis=={FLYVIS_VERSION}")

    # Set before importing torch/flyvis: imports cannot select the shared GPU.
    os.environ["CUDA_VISIBLE_DEVICES"] = ""
    os.environ["MPLBACKEND"] = "Agg"
    os.environ["NUMBA_NUM_THREADS"] = "2"
    os.environ["OMP_NUM_THREADS"] = "2"
    os.environ["MKL_NUM_THREADS"] = "2"
    os.environ["OPENBLAS_NUM_THREADS"] = "2"
    os.environ["FLYVIS_ROOT_DIR"] = str(args.cache_dir.resolve())
    import numpy as np
    import psutil
    import torch
    import torchvision.transforms.functional as tvf
    import yaml

    torch.set_num_threads(2)
    torch.set_num_interop_threads(2)
    import flyvis
    from flyvis.datasets.rendering import BoxEye

    hdf5_adapter = install_windows_hdf5_writer()
    if flyvis.device.type != "cpu":
        raise ValueError("probe requires CPU execution")
    config = yaml.safe_load(config_bytes)["config"]["network"]
    network = flyvis.Network(**config)
    # Only the exact public archive above is permitted to reach torch's loader.
    checkpoint = torch.load(io.BytesIO(checkpoint_bytes), map_location="cpu", weights_only=False)
    state_dict = checkpoint.get("network")
    if not isinstance(state_dict, dict):
        raise ValueError("published checkpoint has no network parameter dictionary")
    network.load_state_dict(state_dict, strict=True)
    network.eval()
    network.requires_grad_(False)
    for parameter in network.parameters():
        if not torch.isfinite(parameter).all():
            raise ValueError("checkpoint contains nonfinite parameters")
    loaded_parameters = {name: value.detach().clone() for name, value in network.named_parameters()}

    # Capture after cold model initialization. All processing below is explicitly offline replay.
    frames, images = capture(args, output)
    schedule = replay_schedule(frames, args.hold_steps)
    eye = BoxEye(extent=15, kernel_size=13)
    retina = []
    with torch.inference_mode():
        for image in images:
            rgb = torch.from_numpy(np.asarray(image, dtype=np.float32).copy()).permute(2, 0, 1) / 255
            grey = rgb.mean(dim=0)[None, None]
            # Map the entire image to the published lattice support. This is a stated
            # geometric resize, without a robot-to-fly angular calibration.
            grey = tvf.resize(grey, eye.min_frame_size.tolist(), antialias=True)
            sample = eye(grey, ftype="mean")
            if sample.shape != (1, 1, 1, 721) or not torch.isfinite(sample).all():
                raise ValueError("retinal input has unexpected dimensions or nonfinite values")
            retina.append(sample)
    np.save(output / "retina.npy", torch.cat(retina, dim=1).cpu().numpy(), allow_pickle=False)

    cells = network.connectome.nodes
    cell_types = cells.type[:].astype(str)
    groups = {name: np.flatnonzero(cell_types == name) for name in sorted(set(cell_types))}
    with (output / "cells.csv").open("w", newline="", encoding="utf-8") as stream:
        writer = csv.writer(stream)
        writer.writerow(["model_index", "cell_type", "hex_u", "hex_v"])
        writer.writerows(zip(cells.index[:], cell_types, cells.u[:], cells.v[:]))
    activity = np.lib.format.open_memmap(output / "activity.npy", mode="w+", dtype=np.float32,
                                         shape=(len(schedule), network.n_nodes))
    process = psutil.Process()
    peak_rss = process.memory_info().rss
    state = None
    with torch.inference_mode():
        # Measured first image held for 1 s initializes the state. These repeated
        # inputs are marked warmup and are never claimed to be new camera frames.
        for _ in range(WARMUP_STEPS):
            state = network.simulate(retina[0], DT, initial_state=state, as_states=True)[0]
            if not torch.isfinite(state.nodes.activity).all():
                raise ValueError("nonfinite activity during held-image warmup")
        initial = state.nodes.activity[0].cpu().numpy().copy()
        np.save(output / "warmup_activity.npy", initial, allow_pickle=False)
        with (output / "activity-by-type.csv").open("w", newline="", encoding="utf-8") as stream:
            writer = csv.writer(stream)
            writer.writerow(["step", "model_input_time_s", "model_output_time_s", "input_index",
                             "input_received_unix_ns", "cell_type", "count", "mean_voltage",
                             "min_voltage", "max_voltage", "std_voltage", "mean_abs_delta_from_warmup"])
            for step, (input_index, at) in enumerate(schedule):
                state = network.simulate(retina[input_index], DT, initial_state=state, as_states=True)[0]
                values = state.nodes.activity[0].cpu().numpy()
                if values.shape != (network.n_nodes,) or not np.isfinite(values).all():
                    raise ValueError("nonfinite or malformed neural response")
                activity[step] = values
                for name, indices in groups.items():
                    selected = values[indices]
                    writer.writerow([step, at, at + DT, input_index, frames[input_index]["received_unix_ns"],
                                     name, len(indices), float(selected.mean()), float(selected.min()),
                                     float(selected.max()), float(selected.std()),
                                     float(np.abs(selected - initial[indices]).mean())])
                peak_rss = max(peak_rss, process.memory_info().rss)
                if step % 25 == 0:
                    print(f"step {step + 1}/{len(schedule)}, rss_mib={peak_rss / 2**20:.1f}", flush=True)
    activity.flush()
    # Forward() clamps upstream parameters. Refuse to call a changed checkpoint frozen.
    if any(not torch.equal(value, loaded_parameters[name]) for name, value in network.named_parameters()):
        raise ValueError("upstream inference altered a loaded parameter")
    dependencies = {dist.metadata["Name"]: dist.version for dist in importlib.metadata.distributions()}
    return {
        "status": "complete", "schema": "qualia.flyvis-camera-probe.v1", "model_id": MODEL_ID,
        "flyvis_version": flyvis.__version__, "source_revision": SOURCE_REVISION,
        "source_url": f"https://github.com/TuragaLab/flyvis/tree/{SOURCE_REVISION}",
        "model_provenance": model_provenance,
        "windows_datamate_hdf5_writer_adapter": hdf5_adapter,
        "activity_kind": "graded model membrane voltage; arbitrary model units; not spikes or Hz",
        "identity_space": "flyvis average-filter visual network indices and hex coordinates; not MaleCNS body IDs",
        "device": "cpu", "threads": 2, "nodes": network.n_nodes, "edges": network.n_edges,
        "cell_types": len(groups), "input_frames": len(frames), "steps": len(schedule),
        "input_kind": "HTTP camera sequence, offline replay" if len(frames) > 1 else "single JPEG held-image response",
        "timestamp_basis": "host receipt intervals; camera exposure time unknown",
        "temporal_resampling": "zero-order hold, first received image at model time zero",
        "retina": {"implementation": "flyvis.datasets.rendering.BoxEye", "extent": 15, "kernel_size": 13,
                   "hexals": 721, "grayscale": "arithmetic mean RGB / 255", "filter": "mean",
                   "resize": "whole image to BoxEye.min_frame_size using torchvision bilinear antialias",
                   "angular_calibration": None, "luminance_calibration": None},
        "dt_s": DT, "warmup": {"steps": WARMUP_STEPS, "input": "first measured JPEG held; no recorded output rows"},
        "completed_unix_ns": time.time_ns(), "wall_seconds": time.monotonic() - started,
        "sampled_peak_rss_mib": peak_rss / 2**20,
        "os_peak_rss_mib": getattr(process.memory_info(), "peak_wset", peak_rss) / 2**20,
        "dependencies": dependencies,
        "actuator_output": None,
        "limitations": ["Pretrained visual network only; no whole-brain or wheel controller",
                        "No calibrated camera optics or sensor exposure timestamps",
                        "Warmup voltage and nonzero absolute activity are not evidence of camera-evoked motion",
                        "Host CPU memory/performance does not establish Jetson feasibility"],
    }


def arguments(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--camera-url", type=snapshot_url)
    source.add_argument("--jpeg", type=Path)
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--cache-dir", required=True, type=Path)
    parser.add_argument("--out-dir", required=True, type=Path)
    parser.add_argument("--frames", type=int, default=8)
    parser.add_argument("--interval-ms", type=int, default=100)
    parser.add_argument("--hold-steps", type=int, default=20)
    parser.add_argument("--wall-seconds", type=int, default=180)
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    if not 1 <= args.frames <= 16 or not 50 <= args.interval_ms <= 1000:
        parser.error("frames must be 1..16 and interval-ms 50..1000")
    if not 1 <= args.hold_steps <= 100 or not 10 <= args.wall_seconds <= 300:
        parser.error("hold-steps must be 1..100 and wall-seconds 10..300")
    return args


def main(argv=None) -> int:
    args = arguments(argv)
    if args.worker:
        try:
            result = worker(args)
            write_json(args.out_dir / "manifest.json", result)
            return 0
        except Exception as error:
            write_json(args.out_dir / "failure.json", {"status": "failed", "error": str(error)})
            raise
    args.out_dir.mkdir(parents=True, exist_ok=False)
    write_json(args.out_dir / "manifest.json", {"status": "running", "actuator_output": None})
    command = [sys.executable, str(Path(__file__).resolve()), *(argv if argv is not None else sys.argv[1:]), "--worker"]
    try:
        with (args.out_dir / "worker.log").open("w", encoding="utf-8") as log:
            result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT,
                                    timeout=args.wall_seconds,
                                    creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        if result.returncode:
            write_json(args.out_dir / "manifest.json", {"status": "failed", "exit_code": result.returncode,
                                                       "actuator_output": None})
            print(f"probe failed; see {args.out_dir / 'worker.log'}", file=sys.stderr)
            return 1
    except subprocess.TimeoutExpired:
        write_json(args.out_dir / "manifest.json", {"status": "failed", "error": "wall-time budget expired",
                                                   "actuator_output": None})
        print("probe exceeded its wall-time budget", file=sys.stderr)
        return 1
    except OSError as error:
        write_json(args.out_dir / "manifest.json", {"status": "failed", "error": str(error),
                                                   "actuator_output": None})
        return 1
    print(f"probe complete: {args.out_dir / 'manifest.json'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

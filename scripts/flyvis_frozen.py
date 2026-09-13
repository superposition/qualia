"""CPU execution of exported flyvis PPNeuronIGRSynapses parameters."""

from __future__ import annotations

import hashlib
import io
import json
from pathlib import Path

import numpy as np
from PIL import Image, ImageOps
import torch
import torch.nn.functional as functional


DT = 0.02


class FrozenVisualModel:
    def __init__(self, directory: Path):
        self.metadata = json.loads((directory / "model.json").read_text())
        path = directory / "model.npz"
        with path.open("rb") as stream:
            actual = hashlib.file_digest(stream, "sha256").hexdigest() if hasattr(hashlib, "file_digest") else hashlib.sha256(stream.read()).hexdigest()
        if actual != self.metadata["export_sha256"]:
            raise ValueError("model export SHA256 mismatch")
        if self.metadata["dynamics"] != "PPNeuronIGRSynapses/relu" or self.metadata["dt_s"] != DT:
            raise ValueError("unsupported exported dynamics")
        with np.load(path, allow_pickle=False) as data:
            self.bias = torch.from_numpy(data["bias"].copy())
            self.tau = torch.from_numpy(data["time_const"].copy())
            self.weight = torch.from_numpy(data["weight"].copy())
            self.source = torch.from_numpy(data["source"].astype(np.int64))
            self.target = torch.from_numpy(data["target"].astype(np.int64))
            self.input_index = torch.from_numpy(data["input_index"].astype(np.int64))
            self.centers = torch.from_numpy(data["retina_centers"].astype(np.int64))
            self.frame_size = data["retina_frame_size"].astype(int).tolist()
            self.cell_types = data["cell_types"].astype(str)
            self.u, self.v = data["u"].copy(), data["v"].copy()
        self.nodes = self.bias.numel()
        if self.nodes != 45669 or self.weight.numel() != 1513231:
            raise ValueError("unexpected exported network dimensions")
        if any(not torch.isfinite(value).all() for value in (self.bias, self.tau, self.weight)):
            raise ValueError("nonfinite exported parameters")
        self.groups = {name: np.flatnonzero(self.cell_types == name) for name in sorted(set(self.cell_types))}
        self.reciprocal_tau = 1 / torch.maximum(self.tau, torch.tensor(DT, dtype=torch.float32))
        self.kernel = torch.ones((1, 1, 13, 13), dtype=torch.float32)
        names, type_index = np.unique(self.cell_types, return_inverse=True)
        pairs = type_index[self.target.numpy()] * len(names) + type_index[self.source.numpy()]
        counts = np.bincount(pairs, minlength=len(names) ** 2).reshape(len(names), len(names))
        sums = np.bincount(pairs, weights=self.weight.numpy(), minlength=len(names) ** 2).reshape(counts.shape)
        self.weight_matrix = {
            "aggregation": "mean signed effective edge weight", "axis_order": "target_rows_source_columns",
            "cell_types": names.tolist(), "edge_counts": counts.tolist(),
            "mean_signed": [[float(sums[row, column] / counts[row, column]) if counts[row, column] else None
                             for column in range(len(names))] for row in range(len(names))],
        }
        self.reset()

    def reset(self):
        self.activity = self.bias.clone()
        self.last_recurrence = None

    def retina(self, jpeg: bytes):
        if not 0 < len(jpeg) <= 8 * 1024 * 1024:
            raise ValueError("JPEG must be between 1 byte and 8 MiB")
        with Image.open(io.BytesIO(jpeg)) as image:
            if image.format != "JPEG" or image.width * image.height > 2097152:
                raise ValueError("camera requires a JPEG of at most 2097152 pixels")
            rgb = np.asarray(ImageOps.exif_transpose(image).convert("RGB"), dtype=np.float32).copy()
        grey = torch.from_numpy(rgb).permute(2, 0, 1).div(255).mean(dim=0)[None, None]
        resized = functional.interpolate(grey, size=self.frame_size, mode="bilinear", align_corners=False, antialias=True)
        filtered = functional.conv2d(functional.pad(resized, (6, 6, 6, 6)), self.kernel) / 169
        positions = self.centers + torch.tensor([self.frame_size[0] // 2, self.frame_size[1] // 2])
        values = filtered[0, 0, positions[:, 0], positions[:, 1]]
        if values.numel() != 721 or not torch.isfinite(values).all():
            raise ValueError("invalid retinal response")
        return values

    def advance(self, retina, steps: int):
        if not 1 <= steps <= 100:
            raise ValueError("integration batch must have 1..100 steps")
        drive = torch.zeros(self.nodes, dtype=torch.float32)
        drive[self.input_index] = retina
        for step in range(steps):
            before = self.activity
            incoming = torch.zeros(self.nodes, dtype=torch.float32)
            incoming.scatter_add_(0, self.target, self.weight * torch.relu(self.activity[self.source]))
            derivative = self.reciprocal_tau * (-self.activity + self.bias + incoming + drive)
            self.activity = self.activity + derivative * DT
            if step == steps - 1:
                self.last_recurrence = {
                    "before_voltage": before, "input_drive": drive, "recurrent_drive": incoming,
                    "bias": self.bias, "tau_s": self.tau, "alpha": self.reciprocal_tau * DT,
                    "after_voltage": self.activity, "delta_voltage": self.activity - before,
                }
        if not torch.isfinite(self.activity).all():
            raise ValueError("nonfinite graded neural state")
        return self.activity

    def statistics(self, previous):
        values = self.activity.numpy()
        difference = np.abs(values - previous)
        return {
            "voltage_min": float(values.min()), "voltage_max": float(values.max()),
            "voltage_mean": float(values.mean()), "mean_abs_delta_from_previous": float(difference.mean()),
            "per_type": [{"cell_type": name, "count": len(indices),
                          "mean_voltage": float(values[indices].mean()),
                          "min_voltage": float(values[indices].min()),
                          "max_voltage": float(values[indices].max()),
                          "std_voltage": float(values[indices].std()),
                          "mean_abs_delta_from_previous": float(difference[indices].mean())}
                         for name, indices in self.groups.items()],
        }

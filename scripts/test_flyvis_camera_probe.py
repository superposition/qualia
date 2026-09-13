"""CPU checks for input provenance, bounded replay, and refusal boundaries."""

import argparse
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import subprocess
import sys

from PIL import Image

import flyvis_camera_probe as probe


class ProbeChecks(unittest.TestCase):
    def test_camera_url_cannot_address_actuators_or_carry_credentials(self):
        for url in ("http://robot/drive", "http://user:secret@robot/camera/snapshot",
                    "http://robot/camera/snapshot?token=secret", "file:///camera/snapshot"):
            with self.assertRaises(argparse.ArgumentTypeError):
                probe.snapshot_url(url)
        self.assertEqual(probe.snapshot_url("http://robot:8000/camera/snapshot"),
                         "http://robot:8000/camera/snapshot")

    def test_timestamp_resampling_preserves_order_and_labels_held_frames(self):
        frames = [{"received_monotonic_ns": n} for n in (1000000000, 1051000000, 1110000000)]
        schedule = probe.replay_schedule(frames, 20)
        self.assertEqual([index for index, _ in schedule], [0, 0, 0, 1, 1, 1, 2])
        self.assertEqual(probe.replay_schedule(frames[:1], 3), [(0, 0.0), (0, 0.02), (0, 0.04)])
        for times in ((10, 10), (20, 10), (0, 20000000000)):
            with self.assertRaises(ValueError):
                probe.replay_schedule([{"received_monotonic_ns": n} for n in times], 20)

    def test_actual_jpeg_bytes_and_unknown_acquisition_time_survive_file_input(self):
        # A synthetic test fixture checks byte handling; it is never model evidence.
        buffer = io.BytesIO()
        Image.new("RGB", (16, 16), (10, 20, 30)).save(buffer, format="JPEG")
        raw = buffer.getvalue()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            original = root / "original.jpg"
            original.write_bytes(raw)
            output = root / "out"
            output.mkdir()
            args = argparse.Namespace(camera_url=None, jpeg=original, interval_ms=100, frames=8)
            frames, images = probe.capture(args, output)
            self.assertEqual(len(images), 1)
            self.assertEqual((output / frames[0]["file"]).read_bytes(), raw)
            self.assertEqual(frames[0]["sha256"], probe.digest(raw))
            self.assertIsNone(frames[0]["camera_exposure_unix_ns"])
            self.assertIsNone(frames[0]["request_started_unix_ns"])
            self.assertIn("acquisition time unknown", frames[0]["timestamp_basis"])
            self.assertEqual(json.loads((output / "inputs.json").read_text()), frames)
        with self.assertRaises(Exception):
            probe.read_jpeg(b"not a JPEG")

    def test_untrusted_archive_fails_before_pickle_or_model_import(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "bad.zip"
            path.write_bytes(b"bad")
            with self.assertRaisesRegex(ValueError, "size differs"):
                probe.trusted_model(path)
            with patch.object(probe, "ARCHIVE_BYTES", 3):
                with self.assertRaisesRegex(ValueError, "SHA256 differs"):
                    probe.trusted_model(path)

    def test_existing_output_preserves_evidence(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            evidence = root / "manifest.json"
            evidence.write_text("existing evidence")
            with self.assertRaises(FileExistsError):
                probe.main(["--jpeg", "missing.jpg", "--archive", "missing.zip",
                            "--cache-dir", str(root / "cache"), "--out-dir", str(root)])
            self.assertEqual(evidence.read_text(), "existing evidence")

    def test_json_refuses_nonfinite_model_metadata(self):
        with tempfile.TemporaryDirectory() as temporary:
            with self.assertRaises(ValueError):
                probe.write_json(Path(temporary) / "out.json", {"voltage": float("nan")})

    def test_supervisor_marks_timeout_failed(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "run"
            with patch.object(probe.subprocess, "run", side_effect=subprocess.TimeoutExpired("worker", 10)):
                result = probe.main(["--jpeg", "missing.jpg", "--archive", "missing.zip",
                                     "--cache-dir", str(output / "cache"), "--out-dir", str(output)])
            self.assertEqual(result, 1)
            self.assertEqual(json.loads((output / "manifest.json").read_text())["status"], "failed")

    @unittest.skipUnless(sys.platform == "win32", "Windows HDF5 handle regression")
    def test_hdf5_adapter_round_trips_arrays_and_releases_handle(self):
        import datamate.directory
        import datamate.io
        import h5py
        import numpy as np

        prior_io, prior_directory = datamate.io._write_h5, datamate.directory._write_h5
        try:
            self.assertTrue(probe.install_windows_hdf5_writer())
            with tempfile.TemporaryDirectory() as temporary:
                path = Path(temporary) / "values.h5"
                for values in (np.array([b"R1", b"T4a"]), np.array([1.25, -2.0], dtype=np.float32)):
                    datamate.directory._write_h5(path, values)
                    with h5py.File(path, "r", swmr=True) as handle:
                        np.testing.assert_array_equal(handle["data"][:], values)
                        self.assertEqual(handle["data"].dtype, values.dtype)
                # Windows refuses this rename if the writer leaked its handle.
                path.rename(Path(temporary) / "closed.h5")
        finally:
            datamate.io._write_h5, datamate.directory._write_h5 = prior_io, prior_directory


if __name__ == "__main__":
    unittest.main()

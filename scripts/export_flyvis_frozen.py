"""Export one verified pretrained flyvis member for a small CPU observer."""

import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--cache-dir", type=Path, required=True)
    parser.add_argument("--jpeg", type=Path, required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    args = parser.parse_args()
    args.out_dir.mkdir(parents=True, exist_ok=False)
    os.environ.update(CUDA_VISIBLE_DEVICES="", MPLBACKEND="Agg", OMP_NUM_THREADS="2",
                      MKL_NUM_THREADS="2", OPENBLAS_NUM_THREADS="2", NUMBA_NUM_THREADS="2",
                      FLYVIS_ROOT_DIR=str(args.cache_dir.resolve()))
    import numpy as np
    import torch
    import torchvision.transforms.functional as tvf
    import yaml
    from flyvis_camera_probe import trusted_model, install_windows_hdf5_writer, read_jpeg, SOURCE_REVISION

    torch.set_num_threads(2)
    torch.set_num_interop_threads(2)
    import flyvis
    from flyvis.datasets.rendering import BoxEye
    from flyvis_frozen import FrozenVisualModel

    if flyvis.__version__ != "1.2.0" or flyvis.device.type != "cpu":
        raise ValueError("export requires pinned flyvis1.2.0 CPU environment")
    install_windows_hdf5_writer()
    config_raw, checkpoint_raw, provenance = trusted_model(args.archive)
    config = yaml.safe_load(config_raw)["config"]["network"]
    if config["dynamics"] != {"type": "PPNeuronIGRSynapses", "activation": {"type": "relu"}}:
        raise ValueError("unsupported checkpoint dynamics")
    model = flyvis.Network(**config)
    model.load_state_dict(torch.load(io.BytesIO(checkpoint_raw), map_location="cpu", weights_only=False)["network"], strict=True)
    model.eval().requires_grad_(False)
    eye = BoxEye(15, 13)
    with torch.inference_mode():
        params = model._param_api()
        cells = model.connectome.nodes
        np.savez_compressed(args.out_dir / "model.npz",
                            bias=params.nodes.bias.numpy(), time_const=params.nodes.time_const.numpy(),
                            weight=params.edges.weight.numpy(), source=model._source_indices.numpy().astype(np.int32),
                            target=model._target_indices.numpy().astype(np.int32),
                            input_index=model.stimulus.input_index.astype(np.int32),
                            retina_centers=eye.receptor_centers.numpy().astype(np.int32),
                            retina_frame_size=eye.min_frame_size.numpy().astype(np.int32),
                            cell_types=cells.type[:].astype(str), u=cells.u[:], v=cells.v[:])
        meta = {"name": "flyvis", "id": "flow/0000/000", "version": "1.2.0",
                "source_url": f"https://github.com/TuragaLab/flyvis/tree/{SOURCE_REVISION}",
                "source_revision": SOURCE_REVISION, "checkpoint_sha256": provenance["checkpoint_sha256"],
                "archive_sha256": provenance["archive_sha256"], "config_sha256": provenance["config_sha256"],
                "export_sha256": hashlib.sha256((args.out_dir / "model.npz").read_bytes()).hexdigest(),
                "nodes": model.n_nodes, "edges": model.n_edges,
                "dynamics": "PPNeuronIGRSynapses/relu", "dt_s": 0.02,
                "exported_unix_ns": time.time_ns(), "parameter_changes": None,
                "code_license": "MIT (flyvis); weights from official archive, no separate license file found",
                "exporter_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
        (args.out_dir / "model.json").write_text(json.dumps(meta, indent=2) + "\n")
        minimal = FrozenVisualModel(args.out_dir)
        jpeg = args.jpeg.read_bytes()
        image = read_jpeg(jpeg)
        rgb = torch.from_numpy(np.asarray(image, dtype=np.float32).copy()).permute(2, 0, 1) / 255
        grey = tvf.resize(rgb.mean(0)[None, None], eye.min_frame_size.tolist(), antialias=True)
        official_retina = eye(grey, ftype="mean")
        exported_retina = minimal.retina(jpeg)
        retinal_error = float((official_retina.flatten() - exported_retina).abs().max())
        state = None
        max_error = 0.0
        original = {name: value.clone() for name, value in model.named_parameters()}
        for _ in range(32):
            state = model.simulate(official_retina, 0.02, initial_state=state, as_states=True)[0]
            exported = minimal.advance(exported_retina, 1)
            max_error = max(max_error, float((exported - state.nodes.activity[0]).abs().max()))
        if retinal_error > 1e-7 or max_error > 1e-6:
            raise ValueError(f"export parity failed: retina={retinal_error}, recurrence={max_error}")
        if any(not torch.equal(value, original[name]) for name, value in model.named_parameters()):
            raise ValueError("reference inference changed pretrained parameters")
        meta["parity"] = {"steps": 32, "dt_s": 0.02, "retina_max_abs_error": retinal_error,
                          "voltage_max_abs_error": max_error, "input_jpeg_sha256": hashlib.sha256(jpeg).hexdigest(),
                          "parameters_unchanged": True, "reference": "flyvis1.2.0 Network.simulate CPU"}
        meta["status"] = "complete"
        (args.out_dir / "model.json").write_text(json.dumps(meta, indent=2) + "\n")
        print(json.dumps(meta, indent=2))


if __name__ == "__main__":
    main()

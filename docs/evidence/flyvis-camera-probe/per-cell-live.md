# Actual per-cell visual response endpoint

The selected-cell endpoint was activated with the authorized observer replacement
on 2026-09-13 at `00:49:32Z`. The prior verified observer PIDs 56413/56414 exited
before the replacement was launched. The new supervisor is PID **60786**, with
run ID **ca9d4589-fd21-4db8-b05f-62da9769e56a** and a bounded deadline of
`2026-09-13T01:19:32Z` (**2026-09-12 21:19:32 EDT**).

The runtime directory is:

```text
/home/jetson/qualia-flyvis-20260912/run-20260913T004932Z
```

An actual GET of `http://10.0.0.180:8091/cells?type=T4a` returned:

| Field | Observed value |
| --- | --- |
| Schema | `qualia.flyvis-cells.v1` |
| State / tick | `running` / 100 |
| Selected cell type / count | `T4a` / 721 |
| JSON response size | 100,468 bytes |
| Model indices | 24039 through 24759 |
| First hex coordinate / last coordinate | `u=-15,v=0` / `u=15,v=0` |
| Voltage range | -0.16999083757400513 through 0.8393464684486389 |
| Actual input age / output age | 505.008 ms / 80.542 ms |
| Source/output JPEG hashes match | true |
| Source/output receipt timestamps match | true |

The first cell, model index 24039 at `(-15,0)`, had graded voltage
`0.27769461274147034` and signed change `-0.00031131505966186523` from the
preceding computed update. The last cell, model index 24759 at `(15,0)`, had
graded voltage `0.5387623310089111` and change `-0.0009369254112243652`.
These are actual model coordinates and graded state values, not spike events or
MaleCNS anatomical body IDs.

The corresponding status read reported 265.906 MiB model RSS and 1,696.680 MiB
available system memory. Qwen PID 1299 remained present. The read-only
`measured_context.camera_aim` payload was present with no error. It remains
separate from the camera-luminance model input and does not modify neural
computation or issue actuator commands.

The activated observer source is commit
`371395a59ad94a85848ef6b3f18c7c05441c7e36`, including the per-cell feed from
`2c4fc6ced6a12de7f88f0cf080d03635d4a710c5`. Deployed script SHA256:

```text
358dce6bbc3ebceafe62dc420c5b1f3a843c1e9d31f4e5109c8a910b905e137c
```

The model export, parameters, recurrence, and camera preprocessing were unchanged.
This observation used the actual running camera/model endpoint after activation;
no additional test cycle, artificial activity, or physical movement was used.
The console agent received the live endpoint and source linkage for its selected
cell-type visualization.

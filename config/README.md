# `config`

Runtime and stack configuration artifacts. JSON only; secrets never live here.

| File | Purpose |
| --- | --- |
| `stack-manifest.default.json` | The full stack: L0-L6, health, vision, and the agent. |
| `stack-manifest.zero-motion.json` | The simulation stack used for end-to-end runs. It opens no motor path. |
| `windows-host.example.json` | Secret-free reference profile for a native Windows CUDA compute and `sync.v1` host replica. Copy to `windows-host.local.json` for machine-local overrides; the local file is ignored by Git. |

Every `*.env` file is ignored by Git except `*.env.example`. The stack supervisor
(`runners/init`) reads these manifests through `QUALIA_STACK_MANIFEST`.

# `qualia-fly-circuit`

A **rate model over the prior's type graph**: one scalar state per type, one `dt`-scaled integration
step, edge weights taken from the prior's CSR.

This is **not** a connectome simulation. The dataset carries no dynamics, so the dynamics here are
invented; the file is a model *over* a graph, not a model *of* the fly. Because the model is invented,
it is a separate crate behind the `sim` feature, observe-only, and is never reachable from the motor
path.

## The model

One `f32` rate per type, all zero at load. One `CircuitSim::step(input, dt)` is one explicit-Euler
step of

```text
ds_i / dt = -s_i + gain * sum over edges p -> i of w[p, i] * s_p + input_i
```

with the edges read from the prior's CSR (`rowptr`, `cols`, `weights`) and `input` one drive value per
type. The prior's weights are synapse counts, so they are scaled by
`gain = 1 / max(1, max_i sum over edges p -> i of w[p, i])`: the heaviest-driven type couples at unit
gain and every other type keeps its relative strength. Without that scale a single step would be
`dt` times a count in the hundreds of thousands and the state would leave the `f32` range at once.

**Weights are positive.** The builder aggregates `dimorphism` away, so the artifact carries no sign;
this model uses each weight exactly as written and therefore treats every edge as excitatory. Nothing
here should be read as inhibition.

Coupling reads the state as it was before the step, so the returned rates depend only on the pre-step
state, the input and `dt`. There is no randomness and no hash iteration: two models loaded from the
same directory and given the same drive return bitwise-identical rates.

## The artifact

`CircuitSim::load(dir)` reads a prior directory written by `qualia-connectome-prior`:
`manifest.json` for the schema, type count and edge count, then `graph.bin` as `rowptr` (`u64`,
little-endian), `cols` (`u32`) and `weights` (`u32`). A missing file, an unknown schema, a byte length
that disagrees with the manifest, or CSR offsets that do not hold is reported as `PriorError::Read`.

## The feature

`sim` is **off by default**. With it off the crate is empty and has no dependencies, so no part of the
simulator can be compiled into a build that did not ask for it. `CircuitSim` and `SIM_ID`
(`qualia.fly-circuit.rate.v1`) exist only with `--features sim`, as do the tests:

```bash
cargo build -p qualia-fly-circuit
cargo test -p qualia-fly-circuit --features sim
```

`cargo test -p qualia-fly-circuit` without the feature compiles the empty crate and runs no tests,
which is the intended state of an observe-only simulator that a build did not request.

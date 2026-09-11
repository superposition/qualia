# `qualia-fly-circuit`

A **rate model over the prior's type graph**: one scalar state per type, one `dt`-scaled integration
step, edge weights taken from the prior's CSR.

This is **not** a connectome simulation. The dataset carries no dynamics, so the dynamics here are
invented; the file is a model *over* a graph, not a model *of* the fly. Because the model is invented,
it is a separate crate behind the `sim` feature, observe-only, and is never reachable from the motor
path.

# `qualia-connectome-prior`

Builds a **type-level connectome prior** from the Male CNS dataset and writes it as a verified
artifact. Segment-level edges are aggregated to type level through the body-annotation table and
emitted as CSR with `u32` type indices, together with a manifest carrying SHA-256 digests and the
CC-BY attribution required by `NOTICE`.

This crate builds the artifact offline. It does not run in the motor path and it has no dynamics: it
is a graph, and only a graph.

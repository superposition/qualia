# qualia-connectome-prior

Builds a type-level connectome prior from the Male CNS dataset (Berg et al. 2026,
https://male-cns.janelia.org): it reads the dataset's segment-level weight and body-annotation
Feather files, aggregates every edge to the neuron-type level through the annotation table, and
emits the resulting graph as a CSR `graph.bin` plus a `manifest.json` and `attribution.json` whose
SHA-256 fields let a consumer reject a truncated or edited artifact. The dataset is licensed CC-BY
4.0; its attribution is carried in the repository `NOTICE` and in the emitted `attribution.json`.

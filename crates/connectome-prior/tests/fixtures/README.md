# Connectome prior fixtures

Hand-authored, synthetic fixtures for the `qualia-connectome-prior` tests. They exist so the builder
tests run offline and deterministically: no file here is downloaded from, or derived from, the Male
CNS dataset, and no private repository was consulted.

Both files are read by the crate's tests as plain CSV (a standard reader such as `csv`/`pandas`
parses them without options) and hold these invariants, which `tests/prior.rs` asserts:

- `mini-type-edges.csv` has header `pre_type,post_type,weight` and exactly 12 data rows.
- `mini-body-annotations.csv` has header `bodyId,type,superclass,side,dimorphism` and exactly 12 data rows.
- The set of `type` values in the annotations file equals the set of `pre_type` values in the edge file
  and equals the set of `post_type` values in the edge file: exactly five distinct types, and every
  type that appears on one side of an edge is annotated.
- Exactly one of those five types carries `dimorphism = male-specific`; the other four do not.
- Every `weight` is a positive integer.
- The 12 `(pre_type, post_type)` rows collapse to 9 distinct type pairs, i.e. fewer than 12, so
  segment-level-to-type-level aggregation is observable rather than a no-op.

The rows are deliberately redundant within a pair (`EPG,PEN_a` twice, `PEN_a,KCg-m` twice,
`KCg-m,DNp01` twice) to exercise that aggregation; `DNp01` is the male-specific type, matching the
Male CNS dataset's convention that `dimorphism` is a type-level property.

The real dataset (Berg et al. 2026, https://male-cns.janelia.org) is CC-BY 4.0 and its attribution
lives in the repository `NOTICE`.

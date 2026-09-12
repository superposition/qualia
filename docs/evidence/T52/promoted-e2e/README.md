# T52 promoted-e2e

Ticket [#225](https://github.com/superposition/qualia/issues/225) (T52), the promoted-model end-to-end
run. Two things live here, and they are different kinds of result:

- **§1, the manifest-digest blocker** — the measured defect that stood between the real capture
  (`t65-real-01`, #250/#256) and *any* training run, and its fix. This is a defect the ticket did not
  expect and it is on the critical path of [#239](https://github.com/superposition/qualia/issues/239)
  too: the trainer refused the manifest the dataset binary had just written.
- **§2 onward** — the end-to-end run itself: the checkpoint the loop needs, its decision, the parity
  and plan-eval captures, and the board leg.

## 1. The trainer refused the manifest the dataset had just written

`t65-dataset.sh` ran, on the board, `qualia-jepa-train --manifest …/jepa-dataset-6f98b125….json
--epochs 1 --batch-size 32 --seed 65` against the manifest `qualia-jepa-dataset` had written seconds
earlier from the real capture. It printed, before the promotion gate:

```text
qualia-jepa-train: dataset manifest is unsupported or has an invalid digest
TRAIN_RC=1
```

`preflight` (`crates/jepa-model/src/bin/qualia-jepa-train.rs:354-360`) then compares
`dataset.schema_version` to `"qualia.jepa-dataset.v3"` — which matches, measured — and `dataset.digest`
to `qualia_jepa_dataset::manifest_digest(dataset)`, which **re-serializes the parsed manifest**
(`crates/jepa-dataset/src/lib.rs:738-744`). The write path (`write_immutable_manifest`,
`lib.rs:710-716`) does not catch this: it re-checks the digest of the *in-memory* struct, where no
parse has happened yet.

### The measured cause: a one-ulp float parse in the audit

The manifest is faithful. Recomputing it in Python — SHA-256 over the compact JSON with `digest`
cleared, the same algorithm — reproduces the stored digest exactly:

```text
$ python digest_probe.py                     # the board's file, 3 401 416 B
schema      : qualia.jepa-dataset.v3
stored      : 6f98b125196ce48501410726309fa6c8b10b21ab4f1b67e0f61297851836b58d
samples     : 1499
python digest: 6f98b125196ce48501410726309fa6c8b10b21ab4f1b67e0f61297851836b58d
match        : True
```

So the file is self-consistent and the mismatch has to be in Rust's parse→serialize round trip. A
probe over the same file with the crate itself (`t52-digest-probe`, a scratch binary that parses with
`DatasetManifest`, prints both digests, and writes the re-serialized bytes) is where it comes apart:

```text
$ t52-digest-probe t65-manifest.json rust-compact.json      # before the fix
file_bytes=3401416
schema=qualia.jepa-dataset.v3
schema_expected_match=true
stored=6f98b125196ce48501410726309fa6c8b10b21ab4f1b67e0f61297851836b58d
recomputed=472b090bfd559bfbc53eb01014b52068cbe796f1529f96b5b8421115e86189c2
digest_match=false
compact_bytes=2426780
roundtrip_stable=true
recomputed2=472b090bfd559bfbc53eb01014b52068cbe796f1529f96b5b8421115e86189c2
```

Comparing the file with Rust's re-serialization leaves exactly two differences, and only one of them
is a value:

```text
('.digest', 'token', '6f98b125…', '')                       # cleared for hashing, as designed
('.audit.mean_sensor_skew_ns', 'token', '29192462.559039358', '29192462.55903936')
float tokens only in file:          [('29192462.559039358', 1)]
float tokens only in rust-compact:  [('29192462.55903936', 1)]
```

The two values are **one ulp apart** (IEEE-754 bit patterns `4718601502228337473` and
`4718601502228337474`). `audit.mean_sensor_skew_ns` is `f64`; the writer emitted its shortest form
(`29192462.559039358`, 17 significant digits) and `serde_json`'s default, best-effort float parser
read that literal back **one ulp low**, so the re-serialization is a different number and the digest
of the re-serialization is a different digest. `serde_json`'s `float_roundtrip` feature exists for
exactly this ("use sufficient precision when parsing fixed precision floats from JSON to ensure that
they maintain accuracy when round-tripped through JSON"); with it enabled the same probe reads:

```text
$ t52-digest-probe t65-manifest.json rust-compact2.json     # after the fix
recomputed=6f98b125196ce48501410726309fa6c8b10b21ab4f1b67e0f61297851836b58d
digest_match=true
compact_bytes=2426781
```

Why T58's manifest passed and this one did not: T58's audit carried an all-zero (`0.0`) mean skew and
no samples, so there was no "hard" 17-digit literal to mis-round. The measurement that separates them
is the value, not the schema and not the writer.

### The fix

`crates/jepa-dataset/Cargo.toml` and `crates/jepa-model/Cargo.toml` enable `serde_json`'s
`float_roundtrip` feature. No threshold moves, no wire field changes, and no manifest's own digest
changes: the writer's digest is computed from memory, and with the feature the reader now reproduces
it. The candidate manifest and the training report are read back by parity and the planner through the
same parser, on the same critical path, which is why the feature is set on both crates.

The regression test is the existing `manifest_round_trips_through_json_with_the_same_digest`
(`crates/jepa-dataset/src/tests.rs`), extended with the value the real capture produced. Falsified by
removing the feature again:

```text
---- tests::manifest_round_trips_through_json_with_the_same_digest stdout ----
thread '…' panicked at crates/jepa-dataset/src/tests.rs:206:5:
assertion `left == right` failed
  left: 29192462.55903936
 right: 29192462.559039358
test result: FAILED. 0 passed; 1 failed; 0 ignored; 8 filtered out; finished in 0.01s
```

and with the feature: `test result: ok. 1 passed; 0 failed; … 8 filtered out`.

### What the fix unblocks

The same trainer, rebuilt with the fix, on the board's own manifest:

```text
$ qualia-jepa-train --manifest /mnt/c/tmp/Impl225PromotedE2E/board/t65-manifest.json \
    --checkpoint-id t52-t65-real-e1 --output-dir train-out --backend cuda --epochs 1 --batch-size 32 --seed 65
qualia-jepa-train: dataset does not meet the 50k/12-session/3-condition/3-environment gate
TRAIN_RC=1
```

The digest step now passes and the refusal is the promotion gate the ticket named — the same message
`qualia-jepa-dataset` itself prints. §2 is the measurement that message carries.

### Files

| file | bytes | sha256 |
| --- | --- | --- |
| `digest-fix/probe-before.txt` | 1237 | `e91ff2813befbd5f45bbb3a53efabae8fc22422bf48e966f2ffba4039f57bad7` |
| `digest-fix/probe-after.txt` | 521 | `51de57e07b12096b98c6e726b094701a9e88cc8c51a86f3cd40c87638e13e465` |
| `digest-fix/trainer-before.txt` | 671 | `e26ca1dbbc62a6e18474765b50692923f61f596d24cd9667cff672b3cbb5d812` |
| `digest-fix/trainer-after.txt` | 939 | `1ec85a981a4dacfcd9d82808a5c27bd738fc58c7c1f4204442183bd3ca4dc6dd` |
| `digest-fix/test-falsification.txt` | 973 | `1e524abd599b9db06b6f501ee676c8a3d56e00672b37c6e77c5a1e577b4f85ca` |

The board's manifest is not committed (3.4 MB, and it is the board's own artifact):
`/home/jetson/t65-capture/t65-real-01/dataset/jepa-dataset-6f98b125….json`, sha256
`2fba5dc830354d6167e07a507b3b15001bb93aa144fbdafba4f9d19fa911cec0`.

### Reproduce

```sh
# the scratch probe (the scratch crate is quoted in the PR, not committed)
cargo build --release -j 2
t52-digest-probe t65-manifest.json rust-compact.json
# the falsification, without the feature
cargo test --release -j 2 -p qualia-jepa-dataset manifest_round_trips
```

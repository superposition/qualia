# `qualia-connectome-stream`

Binary framing for the Male CNS connectome artifacts: the **positions artifact** the importer
(`crates/connectome-cns`) writes, and the **per-tick spike stream** the connectome runner writes.
`crates/connectome-view` reads both; the importer and the runner write them. The layouts live here
so neither side re-derives them.

Everything is little-endian with a four-byte magic, a `u16` version, a `u16` reserved word and then
fixed-stride sections or frames. There is no JSON on the data path: the only text file is the
manifest beside a positions artifact, and the spike framing is identical for a file and a socket, so
a recorded run and a live run differ only in the reader the viewer is handed.

## Positions artifact

`positions.bin`, one row per CSR node, in CSR node order, no padding:

| offset | field | type |
|---|---|---|
| 0 | magic `QLCB` | 4 bytes |
| 4 | version (`1`) | `u16` |
| 6 | reserved (`0`) | `u16` |
| 8 | `neuron_count` = N, the CSR node count | `u32` |
| 12 | `id` — released body id per node | `u32[N]` |
| 12+4N | `x` | `f32[N]` |
| 12+8N | `y` | `f32[N]` |
| 12+12N | `z` | `f32[N]` |
| 12+16N | `cell_type` — index into `types.txt` | `u32[N]` |

`types.txt` holds the interned cell-type labels, one per line; the line number is the `cell_type`
value.

**Row k is CSR node k.** That is the contract that makes a spike id land on the right point: a spike
frame's id is a node index, and no lookup table sits between them. `id` is the released body id for
traceability, and is unique but not sorted — the row order carries the node index, so a reader must
not sort.

A row whose `x`/`y`/`z` are not finite is a node the released annotations place nowhere. It keeps its
id and cell type; the viewer leaves it out of the cloud instead of drawing it at a made-up
coordinate, and the manifest reports the placed count beside `neuron_count`.

`manifest.json` is small and carries: the schema, the node and placed counts, the label count, the
source table's name/size/sha256 and its own counts, one entry per section (byte range, sha256, value
range) and the digests of both files. `verify_positions` recomputes all of it and reports every
disagreement.

## Spike stream

One header, then frames to EOF:

| field | type |
|---|---|
| magic `QLSP` | 4 bytes |
| version (`1`) | `u16` |
| reserved (`0`) | `u16` |
| `tick` | `u64` |
| `t_ns` — wall clock at the tick, unix nanoseconds | `u64` |
| `count` | `u32` |
| `ids` — firing node ids, strictly ascending | `u32[count]` |

Frames are strictly ascending by tick, and a frame that stops halfway is an error rather than a
shorter run. The same bytes go out over `TcpStream` and into a file.

## Positions: what the artifact holds

The released tables are segment-level: 151,856,684 weight rows over 88,384,522 synapse-bearing
segments, Σweight = 311,833,243 — that larger figure is the segment-resolution total. Restricted to
released neurons on both ends, the graph the runner walks is **166,700 neurons, 25,582,938
neuron-level edges, 124,177,617 synapses** (Σ weight), split by inferred neurotransmitter into
+94,542,746 / −26,402,637 / 3,232,234 unknown, of which 164,506 of the 166,700 bodies carry a cell type.

Positions come from `somaLocation` in `body-annotations-male-cns-v1.0-minconf-0.5.feather`:
**139,662 of 211,577** bodies carry one, so the artifact has one row per CSR node and a placed point
for 139,662 of them. The cloud therefore shows the somata we have positions for, not every neuron.
Coordinates are the dataset's raw 8 nm EM voxel units, unrescaled and uncentred.

`crates/connectome-cns` owns the extraction; this crate owns the bytes.

## Usage

```rust
use qualia_connectome_stream::{read_positions, SpikeReader};

let positions = read_positions(std::path::Path::new("artifact/"))?;
println!("{} nodes, {} placed", positions.neuron_count(), positions.placed_count());

let file = std::fs::File::open("artifact/spikes.bin")?;
let mut stream = SpikeReader::new(std::io::BufReader::new(file))?;
while let Some(frame) = stream.next_frame()? {
    println!("tick {} fired {} nodes", frame.tick, frame.ids.len());
}
```

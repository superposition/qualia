//! `the_spike_stream_round_trips_a_recorded_run`: a recorded stream replayed
//! through the bridge yields the same per-tick firing sets and the same point
//! count as the artifact.
//!
//! The stream here is a test fixture, generated over the fixture artifact's own
//! node ids: it exists so the format, the node-index mapping and the bridge can
//! be checked without a runner attached. It is not a product path and nothing
//! in the viewer can produce it.

use std::collections::HashSet;
use std::path::Path;

use qualia_connectome_stream::{
    read_spike_run, write_positions, write_spike_run, Positions, SourceManifest, SpikeFrame,
};
use qualia_connectome_view::{load_cloud, run, ViewOptions, ViewerSink};
use qualia_rerun_bridge::{
    collect_connectome_cloud_records, collect_connectome_tick_records, ConnectomeProjection,
    ConnectomeProjectionContent,
};

const NODES: u32 = 24;

/// A fixture artifact: 24 nodes, three of them placed nowhere.
fn fixture_positions() -> Positions {
    let mut positions = Positions {
        ids: Vec::new(),
        x: Vec::new(),
        y: Vec::new(),
        z: Vec::new(),
        cell_type: Vec::new(),
        types: vec!["KCe".to_string(), "OA-VPM3".to_string(), "SLAV".to_string()],
    };
    for node in 0..NODES {
        positions.ids.push(1_000 + node);
        if node % 7 == 5 {
            positions.x.push(f32::NAN);
            positions.y.push(f32::NAN);
            positions.z.push(f32::NAN);
        } else {
            positions.x.push(100.0 + 8.0 * node as f32);
            positions.y.push(200.0 + 4.0 * node as f32);
            positions.z.push(300.0 + 2.0 * node as f32);
        }
        positions.cell_type.push(node % 3);
    }
    positions
}

/// SplitMix64, so the fixture run is the same bytes on every machine.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut mixed = self.0;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        mixed ^ (mixed >> 31)
    }

    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next_u64() % bound as u64) as usize
    }
}

/// Poisson-ish counts, then that many distinct node indices, sorted.
fn fixture_run(nodes: &[u32], ticks: u64, mean_fraction: f64, seed: u64) -> Vec<SpikeFrame> {
    let mut rng = Rng(seed);
    let cutoff = (-(mean_fraction * nodes.len() as f64)).exp();
    let mut pool: Vec<u32> = nodes.to_vec();
    let mut frames = Vec::with_capacity(ticks as usize);
    for tick in 0..ticks {
        // Knuth's Poisson: keep multiplying uniforms until the product crosses
        // e^-mean; the number of factors is the tick's firing count.
        let mut count = 0usize;
        let mut product = rng.next_f64();
        while product > cutoff {
            product *= rng.next_f64();
            count += 1;
        }
        let count = count.min(pool.len());
        let mut fired = Vec::with_capacity(count);
        for index in 0..count {
            let swap = index + rng.below(pool.len() - index);
            pool.swap(index, swap);
            fired.push(pool[index]);
        }
        fired.sort_unstable();
        frames.push(
            SpikeFrame::new(
                tick,
                1_700_000_000_000_000_000 + tick * 1_000_000,
                fired,
            )
            .expect("fixture frame"),
        );
    }
    frames
}

fn write_fixture(dir: &Path) -> (Vec<SpikeFrame>, Vec<u32>) {
    let positions = fixture_positions();
    let report = write_positions(
        &dir.join("positions.bin"),
        &dir.join("types.txt"),
        &positions,
    )
    .expect("write positions");
    let manifest = report.manifest(
        SourceManifest {
            file: "body-annotations-male-cns-v1.0-minconf-0.5.feather".to_string(),
            bytes: 14_483_314,
            sha256: "0".repeat(64),
            bodies: 211_577,
            positioned: report.placed_count as u64,
            type_labels: report.type_labels as u64,
        },
        "dataset EM voxel units (8 nm)",
    );
    qualia_connectome_stream::write_manifest(&dir.join("manifest.json"), &manifest)
        .expect("write manifest");

    let node_indices: Vec<u32> = (0..NODES)
        .filter(|row| positions.is_placed(*row as usize))
        .collect();
    let frames = fixture_run(&node_indices, 40, 0.4, 56);
    write_spike_run(&dir.join("spikes.bin"), &frames).expect("write run");
    (frames, node_indices)
}

#[test]
fn the_spike_stream_round_trips_a_recorded_run() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (written, nodes) = write_fixture(dir.path());

    // The artifact, as the viewer loads it.
    let cloud = load_cloud(dir.path()).expect("load cloud");
    assert_eq!(cloud.node_count, NODES);
    assert_eq!(cloud.neurons.len(), nodes.len());
    assert_eq!(cloud.unplaced_count(), (NODES as usize) - nodes.len());
    assert_eq!(cloud.cell_types.len(), 3);

    // The same point count the artifact holds, straight from the projection.
    let cloud_projection = cloud.cloud();
    let records = collect_connectome_cloud_records(&cloud_projection);
    let ConnectomeProjectionContent::Points3D {
        positions, colors, ..
    } = &records[0].content
    else {
        panic!("the cloud's first record is not points");
    };
    assert_eq!(positions.len(), nodes.len());
    assert_eq!(colors.len(), nodes.len());

    // The recorded run, read again through the same reader the viewer uses.
    let replayed = read_spike_run(&dir.path().join("spikes.bin")).expect("read run");
    assert_eq!(replayed, written);
    let expected: HashSet<(u64, u64, Vec<u32>)> = written
        .iter()
        .map(|frame| (frame.tick, frame.t_ns, frame.ids.clone()))
        .collect();

    for frame in &replayed {
        // The per-tick firing set is the recorded one, id for id.
        assert!(expected.contains(&(frame.tick, frame.t_ns, frame.ids.clone())));

        let projection = ConnectomeProjection {
            cloud: &cloud_projection,
            firing: &frame.ids,
            wall_clock_ns: frame.t_ns,
        };
        let tick_records = collect_connectome_tick_records(frame.tick, &projection);
        let ConnectomeProjectionContent::Points3D {
            positions,
            colors,
            radius,
        } = &tick_records[0].content
        else {
            panic!("a tick's first record is not points");
        };
        // Same set, drawn at each id's own row of the artifact.
        let drawn: Vec<[f32; 3]> = positions.clone();
        let expected_positions: Vec<[f32; 3]> = frame
            .ids
            .iter()
            .map(|node| {
                cloud
                    .neurons
                    .iter()
                    .find(|neuron| neuron.node == *node)
                    .expect("a firing node is placed")
                    .position
            })
            .collect();
        assert_eq!(drawn, expected_positions);
        assert_eq!(colors.len(), frame.ids.len());
        assert!(*radius < 0.0, "firing radius should be UI points");
        let ConnectomeProjectionContent::Scalars(count) = tick_records[1].content else {
            panic!("a tick's second record is not the count");
        };
        assert_eq!(count, frame.ids.len() as f64);
    }

    // And the whole thing replays through the bridge into a recording.
    let recording = dir.path().join("run.rrd");
    let options = ViewOptions {
        spikes: Some(dir.path().join("spikes.bin")),
        live: None,
        sink: ViewerSink::Save(recording.clone()),
        max_ticks: None,
        speed: 0.0,
        blueprint: true,
    };
    let file = std::fs::File::open(dir.path().join("spikes.bin")).expect("open run");
    let stats = run(&cloud, std::io::BufReader::new(file), &options).expect("replay");
    assert_eq!(stats.ticks, written.len() as u64);
    assert_eq!(stats.first_tick, 0);
    assert_eq!(stats.last_tick, written.len() as u64 - 1);
    assert_eq!(
        stats.firing_points,
        written.iter().map(|frame| frame.ids.len() as u64).sum::<u64>()
    );
    assert_eq!(stats.cloud_points, nodes.len() as u64);
    assert!(stats.points_logged() > stats.cloud_points);
    let bytes = std::fs::metadata(&recording).expect("stat recording").len();
    assert!(bytes > 0, "the recording holds no bytes");
}

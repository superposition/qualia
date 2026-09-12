use qualia_jepa_dataset::{manifest_digest, DatasetManifest};

fn main() {
    let path = std::env::args().nth(1).expect("manifest path");
    let out = std::env::args().nth(2).unwrap_or_else(|| "rust-compact.json".into());
    let bytes = std::fs::read(&path).expect("read manifest");
    println!("file_bytes={}", bytes.len());
    let m: DatasetManifest = serde_json::from_slice(&bytes).expect("parse manifest");
    println!("schema={}", m.schema_version);
    println!("schema_expected_match={}", m.schema_version == "qualia.jepa-dataset.v3");
    println!("stored={}", m.digest);
    println!("recomputed={}", manifest_digest(&m).unwrap());
    println!("digest_match={}", m.digest == manifest_digest(&m).unwrap());

    let mut u = m.clone();
    u.digest.clear();
    let compact = serde_json::to_vec(&u).unwrap();
    std::fs::write(&out, &compact).unwrap();
    println!("compact_bytes={}", compact.len());

    let m2: DatasetManifest = serde_json::from_slice(&compact).expect("reparse compact");
    let mut u2 = m2.clone();
    u2.digest.clear();
    let compact2 = serde_json::to_vec(&u2).unwrap();
    println!("roundtrip_stable={}", compact == compact2);
    println!("recomputed2={}", manifest_digest(&m2).unwrap());
}

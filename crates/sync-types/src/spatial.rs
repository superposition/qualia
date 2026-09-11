use serde::{Deserialize, Serialize};

pub const LEASH_SPATIAL_EVIDENCE_SCHEMA_VERSION: &str = "leash.spatial-evidence.v1";
pub const SPATIAL_ONTOLOGY_SCHEMA_VERSION: &str = "qualia.spatial-ontology.v1";
pub const MAX_SPATIAL_SCANS: usize = 32;
pub const MAX_SPATIAL_POINTS: usize = 20_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LeashSpatialEvidenceV1 {
    pub schema_version: String,
    pub frame_id: String,
    pub source_scans: Vec<LeashSpatialScanReferenceV1>,
    pub point_count: usize,
    pub points_xy_m: Vec<f32>,
    pub compute: LeashComputeReceiptV1,
}

impl LeashSpatialEvidenceV1 {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != LEASH_SPATIAL_EVIDENCE_SCHEMA_VERSION {
            return Err("worker evidence has an unsupported schema".to_string());
        }
        if self.frame_id != "odom" {
            return Err("worker evidence must use the odom frame".to_string());
        }
        if self.source_scans.is_empty() || self.source_scans.len() > MAX_SPATIAL_SCANS {
            return Err(format!(
                "worker evidence must reference 1..={MAX_SPATIAL_SCANS} scans"
            ));
        }
        if self.point_count > MAX_SPATIAL_POINTS || self.points_xy_m.len() != self.point_count * 2 {
            return Err("worker evidence point count is inconsistent".to_string());
        }
        if self.points_xy_m.iter().any(|value| !value.is_finite()) {
            return Err("worker evidence contains non-finite points".to_string());
        }
        if self.compute.input_point_count < self.point_count
            || self.compute.authoritative_backend.trim().is_empty()
            || self.compute.requested_backend.trim().is_empty()
        {
            return Err("worker compute receipt is inconsistent".to_string());
        }
        let epoch = self.source_scans[0].producer_epoch;
        let mut previous_sequence = 0;
        for scan in &self.source_scans {
            if scan.producer_epoch != epoch
                || scan.sequence <= previous_sequence
                || scan.scan_ts_ms == 0
                || scan.pose_ts_ms == 0
                || scan.scan_frame_id.trim().is_empty()
                || scan.pose_frame_id != "odom"
                || scan.sample_count == 0
            {
                return Err("worker scan provenance is inconsistent".to_string());
            }
            previous_sequence = scan.sequence;
        }
        Ok(())
    }

    pub fn captured_at_ms(&self) -> u128 {
        self.source_scans
            .iter()
            .map(|scan| scan.scan_ts_ms.max(scan.pose_ts_ms))
            .max()
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LeashSpatialScanReferenceV1 {
    pub producer_epoch: u64,
    pub sequence: u64,
    pub scan_ts_ms: u128,
    pub pose_ts_ms: u128,
    pub scan_frame_id: String,
    pub pose_frame_id: String,
    pub sample_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LeashComputeReceiptV1 {
    pub requested_backend: String,
    pub authoritative_backend: String,
    pub shadow_compared: bool,
    pub shadow_matches: u32,
    pub cuda_qualified: bool,
    pub fallback_reason: Option<String>,
    pub input_point_count: usize,
    pub elapsed_us: u128,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SpatialRegionKindV1 {
    Occupied,
    Obstacle,
    Traversable,
    Frontier,
    Corridor,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub struct SpatialGridCellV1 {
    pub x: u16,
    pub z: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpatialRegionV1 {
    pub region_id: String,
    pub region_kind: SpatialRegionKindV1,
    pub min_cell: SpatialGridCellV1,
    pub max_cell: SpatialGridCellV1,
    pub cell_count: u32,
    pub confidence: f32,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpatialOntologySnapshotV1 {
    pub schema_version: String,
    pub evidence_id: String,
    pub job_id: String,
    pub worker_id: String,
    pub frame_id: String,
    pub captured_at_ms: u128,
    pub materialized_at_ms: u128,
    pub expires_at_ms: u128,
    pub grid_resolution_m: f32,
    pub grid_width: u16,
    pub grid_depth: u16,
    pub occupied_cells: Vec<SpatialGridCellV1>,
    pub traversable_cells: Vec<SpatialGridCellV1>,
    pub frontier_cells: Vec<SpatialGridCellV1>,
    pub corridor_cells: Vec<SpatialGridCellV1>,
    pub regions: Vec<SpatialRegionV1>,
    pub authoritative_backend: String,
    pub cuda_qualified: bool,
    pub provenance_digest: String,
    pub safety_authority: String,
}

impl SpatialOntologySnapshotV1 {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SPATIAL_ONTOLOGY_SCHEMA_VERSION {
            return Err("spatial ontology has an unsupported schema".to_string());
        }
        if self.evidence_id.trim().is_empty()
            || self.job_id.trim().is_empty()
            || self.worker_id.trim().is_empty()
            || self.frame_id != "odom"
            || self.authoritative_backend.trim().is_empty()
            || self.provenance_digest.len() != 64
        {
            return Err("spatial ontology identity or provenance is invalid".to_string());
        }
        if self.captured_at_ms == 0
            || self.materialized_at_ms < self.captured_at_ms
            || self.expires_at_ms <= self.materialized_at_ms
        {
            return Err("spatial ontology timestamps are invalid".to_string());
        }
        if !self.grid_resolution_m.is_finite()
            || self.grid_resolution_m <= 0.0
            || self.grid_width == 0
            || self.grid_depth == 0
        {
            return Err("spatial ontology grid is invalid".to_string());
        }
        if !self.safety_authority.contains("advisory") {
            return Err("spatial ontology must declare advisory authority".to_string());
        }
        for cell in self
            .occupied_cells
            .iter()
            .chain(&self.traversable_cells)
            .chain(&self.frontier_cells)
            .chain(&self.corridor_cells)
        {
            if cell.x >= self.grid_width || cell.z >= self.grid_depth {
                return Err("spatial ontology contains an out-of-bounds cell".to_string());
            }
        }
        for region in &self.regions {
            if region.region_id.trim().is_empty()
                || region.cell_count == 0
                || !region.confidence.is_finite()
                || !(0.0..=1.0).contains(&region.confidence)
                || region.evidence_refs.is_empty()
                || region.min_cell.x > region.max_cell.x
                || region.min_cell.z > region.max_cell.z
                || region.max_cell.x >= self.grid_width
                || region.max_cell.z >= self.grid_depth
            {
                return Err("spatial ontology contains an invalid region".to_string());
            }
        }
        Ok(())
    }
}

//! The bridge handle: owns the optional Rerun recording and writes records
//! into it.

use rerun::blueprint::BlueprintActivation;
use rerun::{
    LineStrip2D, LineStrips2D, Points2D, Points3D, RecordingStream, RecordingStreamBuilder,
    Scalars, TextDocument, TextLog,
};

use crate::blueprint::{default_connectome_blueprint, default_thought_theater_blueprint};
use crate::config::{BridgeError, RerunBridgeConfig, RerunSinkConfig};
use crate::connectome::{
    collect_connectome_cloud_records, collect_connectome_tick_records, ConnectomeCloud,
    ConnectomeProjection, ConnectomeProjectionContent, ConnectomeProjectionRecord,
};
use crate::records::{
    collect_sync_projection_records, collect_world_model_projection_records, replay_frame_record,
    SessionReplayFrame, SyncProjection, WorldModelProjection, WorldModelProjectionContent,
    WorldModelProjectionRecord,
};
use crate::render::{world_model_line_style, world_model_point_style};

/// Timeline name every world-model and sync record is stamped with.
const TICK_TIMELINE: &str = "qualia_tick";

/// Projection-only handle over one Rerun recording. Constructing it never
/// mutates Qualia state.
pub struct QualiaRerunBridge {
    recording: Option<RecordingStream>,
}

impl QualiaRerunBridge {
    /// Builds the recording selected by `config.sink`. `Disabled` yields a
    /// handle whose methods all succeed without writing anything.
    pub fn new(config: &RerunBridgeConfig) -> Result<Self, BridgeError> {
        let recording = match &config.sink {
            RerunSinkConfig::Disabled => None,
            RerunSinkConfig::Buffered => Some(
                RecordingStreamBuilder::new(config.application_id.clone())
                    .buffered()
                    .map_err(|error| {
                        BridgeError::new("failed to create buffered Rerun stream", error)
                    })?,
            ),
            RerunSinkConfig::Save(path) => Some(
                RecordingStreamBuilder::new(config.application_id.clone())
                    .save(path)
                    .map_err(|error| {
                        BridgeError::new("failed to create saved Rerun stream", error)
                    })?,
            ),
            RerunSinkConfig::Connect(url) => Some(
                RecordingStreamBuilder::new(config.application_id.clone())
                    .connect_grpc_opts(url.clone())
                    .map_err(|error| {
                        BridgeError::new("failed to connect the Rerun stream", error)
                    })?,
            ),
        };
        Ok(Self { recording })
    }

    /// Whether this handle actually has a recording attached.
    pub fn is_enabled(&self) -> bool {
        self.recording.is_some()
    }

    /// Writes one world-model slice: summary, proposal and canonical views,
    /// the operational slice, graph/spatial layers, and coach decisions.
    pub fn log_world_model_projection(
        &self,
        tick: u64,
        projection: &WorldModelProjection<'_>,
    ) -> Result<(), BridgeError> {
        let Some(recording) = &self.recording else {
            return Ok(());
        };

        recording.set_time_sequence(TICK_TIMELINE, tick as i64);
        for record in collect_world_model_projection_records(tick, projection) {
            write_record(recording, record)?;
        }

        recording
            .flush_blocking()
            .map_err(|error| BridgeError::new("failed to flush Rerun recording", error))?;
        Ok(())
    }

    /// Writes one sync slice: replica registry, materialized state, append
    /// log, and op log.
    pub fn log_sync_projection(
        &self,
        tick: u64,
        projection: &SyncProjection<'_>,
    ) -> Result<(), BridgeError> {
        let Some(recording) = &self.recording else {
            return Ok(());
        };

        recording.set_time_sequence(TICK_TIMELINE, tick as i64);
        for record in collect_sync_projection_records(tick, projection) {
            write_record(recording, record)?;
        }

        recording
            .flush_blocking()
            .map_err(|error| BridgeError::new("failed to flush Rerun recording", error))?;
        Ok(())
    }

    /// Activates the default Thought Theater layout.
    pub fn send_default_thought_theater_blueprint(&self) -> Result<(), BridgeError> {
        let Some(recording) = &self.recording else {
            return Ok(());
        };
        default_thought_theater_blueprint()
            .send(recording, BlueprintActivation::default())
            .map_err(|error| BridgeError::new("failed to send Thought Theater blueprint", error))
    }

    /// Alias for [`Self::send_default_thought_theater_blueprint`].
    pub fn send_default_graph_blueprint(&self) -> Result<(), BridgeError> {
        self.send_default_thought_theater_blueprint()
    }

    /// Activates the default Brain layout: the 3D cloud, the firing set, the
    /// stream summary and the firing-count curve.
    pub fn send_default_connectome_blueprint(&self) -> Result<(), BridgeError> {
        let Some(recording) = &self.recording else {
            return Ok(());
        };
        default_connectome_blueprint()
            .send(recording, BlueprintActivation::default())
            .map_err(|error| BridgeError::new("failed to send the connectome blueprint", error))
    }

    /// Writes the connectome cloud once: one point per placed node, coloured by
    /// cell type, plus the cloud's summary. Static, because the cloud does not
    /// change from tick to tick.
    pub fn log_connectome_cloud(&self, cloud: &ConnectomeCloud<'_>) -> Result<(), BridgeError> {
        let Some(recording) = &self.recording else {
            return Ok(());
        };

        for record in collect_connectome_cloud_records(cloud) {
            write_connectome_record(recording, record, true)?;
        }
        recording
            .flush_blocking()
            .map_err(|error| BridgeError::new("failed to flush the connectome cloud", error))?;
        Ok(())
    }

    /// Writes one tick: the firing set highlighted, its count on the timeline,
    /// and the tick's summary, stamped with the tick index so scrubbing the
    /// timeline replays the activity.
    pub fn log_connectome_tick(
        &self,
        tick: u64,
        projection: &ConnectomeProjection<'_>,
    ) -> Result<(), BridgeError> {
        let Some(recording) = &self.recording else {
            return Ok(());
        };

        recording.set_time_sequence(TICK_TIMELINE, tick as i64);
        for record in collect_connectome_tick_records(tick, projection) {
            write_connectome_record(recording, record, false)?;
        }
        recording
            .flush_blocking()
            .map_err(|error| BridgeError::new("failed to flush a connectome tick", error))?;
        Ok(())
    }

    /// Writes a multi-frame replay timeline plus each frame's projections into
    /// the same recording.
    pub fn export_session_replay(
        &self,
        frames: &[SessionReplayFrame<'_>],
    ) -> Result<(), BridgeError> {
        let Some(recording) = &self.recording else {
            return Ok(());
        };

        for frame in frames {
            recording.set_time_sequence(TICK_TIMELINE, frame.tick as i64);
            write_record(recording, replay_frame_record(frame))?;
            if let Some(world_model) = &frame.world_model {
                for record in collect_world_model_projection_records(frame.tick, world_model) {
                    write_record(recording, record)?;
                }
            }
            if let Some(sync) = &frame.sync {
                for record in collect_sync_projection_records(frame.tick, sync) {
                    write_record(recording, record)?;
                }
            }
        }

        recording
            .flush_blocking()
            .map_err(|error| BridgeError::new("failed to flush replay export", error))?;
        Ok(())
    }
}

fn write_record(
    recording: &RecordingStream,
    record: WorldModelProjectionRecord,
) -> Result<(), BridgeError> {
    let entity_path = record.entity_path;
    match record.content {
        WorldModelProjectionContent::Text(text) => recording
            .log(entity_path, &TextLog::new(text))
            .map_err(|error| BridgeError::new("failed to log world-model text record", error))?,
        WorldModelProjectionContent::Document(markdown) => recording
            .log(entity_path, &TextDocument::from_markdown(markdown))
            .map_err(|error| {
                BridgeError::new("failed to log world-model document record", error)
            })?,
        WorldModelProjectionContent::LogEvent { text, level, color } => recording
            .log(
                entity_path,
                &TextLog::new(text).with_level(level).with_color(color),
            )
            .map_err(|error| BridgeError::new("failed to log world-model event record", error))?,
        WorldModelProjectionContent::Points2D(points) => {
            let (radius, color) = world_model_point_style(&entity_path);
            recording
                .log(
                    entity_path,
                    &Points2D::new(points.into_iter())
                        .with_radii([radius])
                        .with_colors([color]),
                )
                .map_err(|error| {
                    BridgeError::new("failed to log world-model point record", error)
                })?
        }
        WorldModelProjectionContent::LineStrip2D(points) => {
            let (radius, color) = world_model_line_style(&entity_path);
            recording
                .log(
                    entity_path,
                    &LineStrips2D::new([LineStrip2D::from_iter(points)])
                        .with_radii([radius])
                        .with_colors([color]),
                )
                .map_err(|error| BridgeError::new("failed to log world-model line record", error))?
        }
    }
    Ok(())
}

/// Writes one connectome record, either static (the cloud) or on the timeline
/// the caller already stamped (a tick).
fn write_connectome_record(
    recording: &RecordingStream,
    record: ConnectomeProjectionRecord,
    is_static: bool,
) -> Result<(), BridgeError> {
    let entity_path = record.entity_path;
    match record.content {
        ConnectomeProjectionContent::Document(markdown) => {
            let component = TextDocument::from_markdown(markdown);
            let result = if is_static {
                recording.log_static(entity_path, &component)
            } else {
                recording.log(entity_path, &component)
            };
            result.map_err(|error| {
                BridgeError::new("failed to log a connectome document record", error)
            })?;
        }
        ConnectomeProjectionContent::Points3D {
            positions,
            colors,
            radius,
        } => {
            let component = Points3D::new(
                positions
                    .iter()
                    .map(|position| rerun::Vec3D::from(position)),
            )
            .with_colors(colors)
            .with_radii([radius]);
            let result = if is_static {
                recording.log_static(entity_path, &component)
            } else {
                recording.log(entity_path, &component)
            };
            result.map_err(|error| {
                BridgeError::new("failed to log a connectome point record", error)
            })?;
        }
        ConnectomeProjectionContent::Scalars(value) => {
            let component = Scalars::single(value);
            let result = if is_static {
                recording.log_static(entity_path, &component)
            } else {
                recording.log(entity_path, &component)
            };
            result.map_err(|error| {
                BridgeError::new("failed to log a connectome scalar record", error)
            })?;
        }
    }
    Ok(())
}


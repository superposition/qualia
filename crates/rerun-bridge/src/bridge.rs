//! The bridge handle: owns the optional Rerun recording and writes records
//! into it.

use rerun::blueprint::BlueprintActivation;
use rerun::{
    LineStrip2D, LineStrips2D, Points2D, RecordingStream, RecordingStreamBuilder, TextDocument,
    TextLog,
};

use crate::blueprint::default_thought_theater_blueprint;
use crate::config::{BridgeError, RerunBridgeConfig, RerunSinkConfig};
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

//! Entity profiles: the static description of a body Qualia can inhabit.
//!
//! Embodiment and capability names are open strings rather than closed enums,
//! so a new robot form is a new profile document instead of a core change.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const ENTITY_PROFILE_SCHEMA_VERSION: &str = "qualia.entity.v1";

/// A body Qualia can observe and, where authority permits, command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityProfile {
    pub schema_version: String,
    pub entity: EntityDescriptor,
    #[serde(default)]
    pub runtime: Option<EntityRuntime>,
    #[serde(default)]
    pub frames: Vec<EntityFrame>,
    #[serde(default)]
    pub streams: Vec<EntityStream>,
    #[serde(default)]
    pub capabilities: Vec<EntityCapability>,
    #[serde(default)]
    pub authorities: Vec<EntityAuthority>,
}

/// Where the live state for an entity can be read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityRuntime {
    /// Qualia agent serving live state for this entity.
    pub agent_url: String,
}

/// Identity and form of an entity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityDescriptor {
    pub id: String,
    pub display_name: String,
    /// Open form label such as `ground`, `aerial`, or `humanoid`.
    pub embodiment: String,
    /// Concrete implementation name, when the profile names one.
    #[serde(default)]
    pub implementation: Option<String>,
}

/// A coordinate frame in the entity's kinematic tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityFrame {
    pub id: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    /// Open role label such as `world`, `base`, `camera`, or `end-effector`.
    pub role: String,
}

/// A named data flow the entity emits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityStream {
    pub id: String,
    /// Open kind label such as `pose`, `camera`, `lidar`, `joints`, or `map`.
    pub kind: String,
    #[serde(default)]
    pub frame_id: Option<String>,
    /// Adapter-owned source identifier or endpoint.
    pub source: String,
}

/// Something the entity can do, optionally bound to streams that feed it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityCapability {
    /// Namespaced identifier such as `spatial.map` or `planning.trajectory`.
    pub id: String,
    /// Coarse grouping for display; it does not constrain implementation.
    pub category: String,
    #[serde(default)]
    pub stream_ids: Vec<String>,
}

/// A command the entity accepts, and who remains answerable for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityAuthority {
    /// Namespaced command such as `motion.navigate`, `joints.goal`, or `safety.estop`.
    pub command: String,
    /// System that stays authoritative for accepting or refusing the command.
    pub owner: String,
    /// Concrete transport for the command; empty means no authority exists.
    pub transport: String,
    pub acknowledgement_required: bool,
}

impl EntityProfile {
    /// Whether the profile declares a capability by identifier.
    pub fn has_capability(&self, id: &str) -> bool {
        self.capabilities
            .iter()
            .any(|capability| capability.id == id)
    }

    /// The authority row for a command, if the profile declares one.
    pub fn authority(&self, command: &str) -> Option<&EntityAuthority> {
        self.authorities
            .iter()
            .find(|authority| authority.command == command)
    }

    /// Cross-checks the profile against itself.
    ///
    /// Identifiers must be non-empty and printable, references must resolve,
    /// ids must be unique within their collection, and safety commands must
    /// demand acknowledgement.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != ENTITY_PROFILE_SCHEMA_VERSION {
            return Err(format!(
                "unsupported entity profile schema_version '{}', expected '{}'",
                self.schema_version, ENTITY_PROFILE_SCHEMA_VERSION
            ));
        }
        validate_identifier("entity.id", &self.entity.id)?;
        validate_text("entity.display_name", &self.entity.display_name)?;
        validate_identifier("entity.embodiment", &self.entity.embodiment)?;
        if let Some(implementation) = self.entity.implementation.as_deref() {
            validate_identifier("entity.implementation", implementation)?;
        }
        if let Some(runtime) = &self.runtime {
            validate_text("runtime.agent_url", &runtime.agent_url)?;
            if !runtime.agent_url.starts_with("http://")
                && !runtime.agent_url.starts_with("https://")
            {
                return Err("runtime.agent_url must use http:// or https://".to_string());
            }
        }

        let frame_ids = unique_ids("frame", self.frames.iter().map(|frame| frame.id.as_str()))?;
        for frame in &self.frames {
            validate_identifier("frame.id", &frame.id)?;
            validate_identifier("frame.role", &frame.role)?;
            if let Some(parent_id) = frame.parent_id.as_deref() {
                validate_identifier("frame.parent_id", parent_id)?;
                if parent_id == frame.id {
                    return Err(format!("frame '{}' cannot parent itself", frame.id));
                }
                if !frame_ids.contains(parent_id) {
                    return Err(format!(
                        "frame '{}' references unknown parent '{}'",
                        frame.id, parent_id
                    ));
                }
            }
        }

        let stream_ids = unique_ids(
            "stream",
            self.streams.iter().map(|stream| stream.id.as_str()),
        )?;
        for stream in &self.streams {
            validate_identifier("stream.id", &stream.id)?;
            validate_identifier("stream.kind", &stream.kind)?;
            validate_text("stream.source", &stream.source)?;
            if let Some(frame_id) = stream.frame_id.as_deref() {
                if !frame_ids.contains(frame_id) {
                    return Err(format!(
                        "stream '{}' references unknown frame '{}'",
                        stream.id, frame_id
                    ));
                }
            }
        }

        unique_ids(
            "capability",
            self.capabilities
                .iter()
                .map(|capability| capability.id.as_str()),
        )?;
        for capability in &self.capabilities {
            validate_identifier("capability.id", &capability.id)?;
            validate_identifier("capability.category", &capability.category)?;
            for stream_id in &capability.stream_ids {
                if !stream_ids.contains(stream_id.as_str()) {
                    return Err(format!(
                        "capability '{}' references unknown stream '{}'",
                        capability.id, stream_id
                    ));
                }
            }
        }

        unique_ids(
            "authority",
            self.authorities
                .iter()
                .map(|authority| authority.command.as_str()),
        )?;
        for authority in &self.authorities {
            validate_identifier("authority.command", &authority.command)?;
            validate_identifier("authority.owner", &authority.owner)?;
            validate_text("authority.transport", &authority.transport)?;
            if matches!(authority.command.as_str(), "motion.stop" | "safety.estop")
                && !authority.acknowledgement_required
            {
                return Err(format!(
                    "safety command '{}' must require acknowledgement",
                    authority.command
                ));
            }
        }

        Ok(())
    }
}

/// Parses and validates an entity profile document.
pub fn parse_entity_profile(text: &str) -> Result<EntityProfile, String> {
    let profile: EntityProfile =
        serde_json::from_str(text).map_err(|error| format!("invalid entity profile: {error}"))?;
    profile.validate()?;
    Ok(profile)
}

fn unique_ids<'a>(
    label: &str,
    values: impl Iterator<Item = &'a str>,
) -> Result<HashSet<&'a str>, String> {
    let mut seen = HashSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(format!("duplicate {label} id '{value}'"));
        }
    }
    Ok(seen)
}

fn validate_identifier(label: &str, value: &str) -> Result<(), String> {
    validate_text(label, value)?;
    if !value.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | ':' | '/')
    }) {
        return Err(format!(
            "{label} contains unsupported characters: '{value}'"
        ));
    }
    Ok(())
}

fn validate_text(label: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{label} cannot be empty"));
    }
    Ok(())
}

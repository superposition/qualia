//! One module per view, one `View` value per module.
//!
//! `docs/frontend-lessons.md` records this shape twice, from two independent
//! front ends (sources 1 and 2): one small view enum, one label table, one
//! dispatch, one render function per panel. `crate::View` is that enum and
//! `crate::VIEWS` is that table; each module below owns exactly one arm.

pub mod belief;
pub mod brain;
/// The mission broker's decision stream (T63), the console's second source:
/// floating panel, read from `GET /coach` on the broker's loopback surface.
pub mod coach;
pub mod evidence;
pub mod mission;
/// The HUD's reading, not a seventh view: one row per runner telemetry frame,
/// drawn by [`crate::hud`]'s floating panels.
pub mod stats;
pub mod telemetry;
pub mod world;

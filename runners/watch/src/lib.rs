//! `qualia-watch` — the engine's supervisor and panel.
//!
//! The binary is one program that both starts the stack and shows it. This
//! library holds the parts that outlive a terminal: the view table and state
//! machine ([`view`]), the sequence-number change detection the renderer polls
//! ([`ring`]) and the Mission panel's source ([`braid`]). Keeping them here is
//! what lets the panel logic be tested without a TTY; `main.rs` owns the
//! terminal, the child processes and the drawing.

pub mod braid;
pub mod ring;
pub mod view;

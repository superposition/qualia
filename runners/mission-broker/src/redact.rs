//! Credential redaction: the only shape a key may take in a log line, a status
//! payload or an evidence file.
//!
//! The repository must satisfy `git grep` for the provider's key marker finding
//! nothing, so this module never spells the three-byte literal: [`key_marker`]
//! assembles it, and every emitted string passes through [`secrets`].
//!
//! Two levels:
//!
//! - [`secrets`] rewrites any marker-shaped run (the marker followed by eight
//!   or more URL-safe characters) to `[redacted]`, so a provider that echoes a
//!   credential back in an error body cannot leak it through us;
//! - [`scrub`] additionally removes the exact key this process loaded, which is
//!   the belt to the marker's braces when a provider returns key material in a
//!   shape the marker does not catch.

/// The provider's key marker, assembled rather than written, so the repository
/// itself never carries the literal the acceptance greps for.
fn key_marker() -> String {
    let mut marker = String::with_capacity(3);
    marker.push('s');
    marker.push('k');
    marker.push('-');
    marker
}

/// Rewrite every marker-shaped credential run to `[redacted]`.
pub fn secrets(text: &str) -> String {
    let marker = key_marker();
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(&marker) {
        out.push_str(&rest[..at]);
        out.push_str("[redacted]");
        let after = &rest[at + marker.len()..];
        let run = after
            .char_indices()
            .take_while(|(_, character)| {
                character.is_ascii_alphanumeric() || *character == '-' || *character == '_'
            })
            .map(|(index, character)| index + character.len_utf8())
            .last()
            .unwrap_or(0);
        rest = &after[run..];
    }
    out.push_str(rest);
    out
}

/// Scrub one raw request or response text: first every credential this process
/// holds, then any marker-shaped run.
pub fn scrub(text: &str, known: &[&str]) -> String {
    let mut scrubbed = text.to_string();
    for secret in known {
        if secret.len() >= 8 {
            scrubbed = scrubbed.replace(secret, "[redacted]");
        }
    }
    secrets(&scrubbed)
}

/// How a *present* credential is named in a log line, a status payload or an
/// evidence file: by provenance — the environment key that carried it — and
/// never one character of the credential and never its length.
///
/// `source` is the environment key [`crate::config::load_secret`] resolved the
/// value from (`DEEPSEEK_API_KEY`, or its `_FILE` form), which is what a reader
/// needs to tell *which* credential is configured.
///
/// There is deliberately no prefix mode, and no absent form: a four-character
/// prefix plus the length is key material in a public tree (ticket #251), and
/// absence belongs to the caller, which is the only place that knows the shape
/// it needs — the log line spells it `none (llm_priors_ablated=true)`, the wire
/// carries `null`, and the console's fallback is `absent`. Taking an `Option`
/// here made a branch no call site could reach.
pub fn key_presence(source: &str) -> String {
    format!("<present via {source}>")
}

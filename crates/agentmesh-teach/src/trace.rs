//! Semantic-trace schema: the recorder output that inference consumes.
//!
//! A trace is a JSONL document: one [`SemanticEvent`] per line, framed by
//! `session.start` / `session.stop`. Values are either harmless literals or
//! `secret://` references — raw passwords, tokens, and cookies never appear.

use serde::{Deserialize, Serialize, de::Error as _};

/// One recorded interaction, frugal by design: semantic descriptors and
/// metadata only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticEvent {
    /// Monotonic sequence number within the trace.
    #[serde(default)]
    pub seq: u64,
    /// Milliseconds since the Unix epoch.
    #[serde(default)]
    pub ts_ms: u64,
    /// Event family.
    pub kind: TraceKind,
    /// Page URL at capture time, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// True when the URL falls outside the recording scope.
    #[serde(default)]
    pub out_of_scope: bool,
    /// Element descriptor for UI events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<RecordedTarget>,
    /// Captured value for fills and selects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<TraceValue>,
    /// Selected option label for `ui.select`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<String>,
    /// HTTP method for sanitized network calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Host and optional port; never credentials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// URL path without query values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Query parameter names only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query_keys: Vec<String>,
    /// HTTP response status, when this is a response event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

/// Trace event families emitted by the recorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TraceKind {
    /// Recording started.
    #[serde(rename = "session.start")]
    SessionStart,
    /// Recording stopped.
    #[serde(rename = "session.stop")]
    SessionStop,
    /// Main-frame navigation.
    #[serde(rename = "browser.navigate")]
    Navigate,
    /// Pointer activation.
    #[serde(rename = "ui.click")]
    Click,
    /// Text entry.
    #[serde(rename = "ui.fill")]
    Fill,
    /// Dropdown selection.
    #[serde(rename = "ui.select")]
    Select,
    /// Form submission.
    #[serde(rename = "ui.submit")]
    Submit,
    /// Content extraction probe (smoke mode).
    #[serde(rename = "ui.extract")]
    Extract,
    /// Sanitized network metadata.
    #[serde(rename = "network.call")]
    NetworkCall,
    /// Sanitized network response metadata.
    #[serde(rename = "network.response")]
    NetworkResponse,
}

/// Element descriptor captured in-page.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecordedTarget {
    /// HTML tag.
    #[serde(default)]
    pub tag: String,
    /// ARIA role or equivalent.
    #[serde(default)]
    pub role: String,
    /// Accessible name.
    #[serde(default)]
    pub name: String,
    /// Placeholder text.
    #[serde(default)]
    pub placeholder: String,
    /// `data-testid` attribute.
    #[serde(default)]
    pub test_id: String,
    /// Element id.
    #[serde(default)]
    pub id: String,
    /// Visible text slice.
    #[serde(default)]
    pub text: String,
    /// Input type (`password` never carries a value).
    #[serde(rename = "inputType", default)]
    pub input_type: String,
    /// `autocomplete` attribute (`username`, `current-password`, ...):
    /// a stable semantic signal for login fields whose ids rotate.
    #[serde(default)]
    pub autocomplete: String,
    /// CSS selector fallbacks, strongest first.
    #[serde(default)]
    pub selectors: Vec<String>,
}

/// A captured value: displayable literal or opaque secret reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TraceValue {
    /// Ordinary displayable value.
    Literal {
        /// The captured text.
        literal: String,
    },
    /// Opaque reference resolved at runtime, never the secret itself.
    SecretRef {
        /// `secret://<host>/<field>`.
        secret_ref: String,
    },
}

/// Reads a JSONL trace, skipping blank lines.
///
/// # Errors
///
/// Returns [`crate::TeachError::Parse`] when a line is not a valid event.
pub fn read_trace(document: &str) -> Result<Vec<SemanticEvent>, crate::TeachError> {
    let mut events = Vec::new();
    for (index, line) in document.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let event: SemanticEvent = serde_json::from_str(line).map_err(|error| {
            crate::TeachError::Parse(serde_yaml::Error::custom(format!(
                "trace line {}: {error}",
                index + 1
            )))
        })?;
        events.push(event);
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_roundtrips_and_skips_blanks() {
        let document = "\n{\"kind\":\"session.start\"}\n{\"kind\":\"ui.fill\",\"value\":{\"literal\":\"hi\"}}\n";
        let events = read_trace(document).expect("trace parses");
        assert_eq!(events.len(), 2);
        assert!(matches!(events[1].value, Some(TraceValue::Literal { .. })));
    }

    #[test]
    fn secret_refs_parse() {
        let event: SemanticEvent = serde_json::from_str(
            "{\"kind\":\"ui.fill\",\"value\":{\"secret_ref\":\"secret://app/pw\"}}",
        )
        .expect("secret ref parses");
        assert!(matches!(event.value, Some(TraceValue::SecretRef { .. })));
    }

    #[test]
    fn corrupt_line_fails() {
        assert!(read_trace("{\"kind\":\n").is_err());
    }
}

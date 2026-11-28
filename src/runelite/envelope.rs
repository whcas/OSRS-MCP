//! The top-level structure that container, quest and diary exports share: a
//! size-bounded JSON object carrying `exported_at` and an optional
//! `plugin_version`.
//!
//! Each parser keeps its own public error type. [`EnvelopeError`] is the part
//! of those errors that does not depend on the dataset, and each parser
//! converts it with a `From` impl so the shared rules live in one place.
//! `character.json` is not read here: it accepts any `plugin_version` string
//! and has its own errors.

use serde_json::{Map, Value};

use super::ExportedAt;

pub(super) type Fields = Map<String, Value>;

/// Longest `exported_at` and `plugin_version` text accepted, in UTF-8 bytes.
pub(super) const MAX_ENVELOPE_TEXT_BYTES: usize = 128;

/// Why the shared structure of an export was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EnvelopeError {
    InputTooLarge,
    /// The input ended prematurely, which may mean it was read during a write.
    Truncated,
    InvalidJson,
    /// Valid JSON whose top level is not an object.
    UnexpectedShape,
    MissingField {
        field: &'static str,
    },
    InvalidField {
        field: &'static str,
    },
}

/// The fields every export file carries beside its payload.
pub(super) struct Envelope {
    pub(super) exported_at: ExportedAt,
    pub(super) plugin_version: Option<String>,
}

/// Bounds the input and decodes it to a JSON object, separating a premature
/// end from other syntax errors.
pub(super) fn decode_object(bytes: &[u8], max_bytes: usize) -> Result<Fields, EnvelopeError> {
    if bytes.len() > max_bytes {
        return Err(EnvelopeError::InputTooLarge);
    }
    let input: Value = serde_json::from_slice(bytes).map_err(|error| {
        if error.is_eof() {
            EnvelopeError::Truncated
        } else {
            EnvelopeError::InvalidJson
        }
    })?;
    match input {
        Value::Object(fields) => Ok(fields),
        _ => Err(EnvelopeError::UnexpectedShape),
    }
}

/// The value of a field the format requires.
pub(super) fn required<'a>(
    fields: &'a Fields,
    field: &'static str,
) -> Result<&'a Value, EnvelopeError> {
    fields
        .get(field)
        .ok_or(EnvelopeError::MissingField { field })
}

/// Reads `exported_at` (required) and `plugin_version` (optional; absent and
/// `null` are the same). `exported_at` is checked first.
pub(super) fn read_envelope(fields: &Fields) -> Result<Envelope, EnvelopeError> {
    let exported_at = required(fields, "exported_at")?
        .as_str()
        .filter(|text| text.len() <= MAX_ENVELOPE_TEXT_BYTES)
        .and_then(ExportedAt::parse)
        .ok_or(EnvelopeError::InvalidField {
            field: "exported_at",
        })?;
    let plugin_version = match fields.get("plugin_version") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) if text.len() <= MAX_ENVELOPE_TEXT_BYTES => Some(text.clone()),
        Some(_) => {
            return Err(EnvelopeError::InvalidField {
                field: "plugin_version",
            });
        }
    };
    Ok(Envelope {
        exported_at,
        plugin_version,
    })
}

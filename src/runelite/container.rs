//! Parsing of bank, inventory and equipment exports from Character State Exporter.
//!
//! Container files are independent observations. Item IDs are canonicalized by
//! the exporter, so different slots may have the same ID. A snapshot keeps each
//! row and its slot in file order.

use std::collections::HashSet;

use serde_json::Value;
use thiserror::Error;

use super::ExportedAt;
use super::envelope::{
    EnvelopeError, MAX_ENVELOPE_TEXT_BYTES, decode_object, read_envelope, required,
};
use crate::domain::{ItemId, Quantity};

/// Largest container export accepted, in bytes.
pub const MAX_CONTAINER_EXPORT_BYTES: usize = 4 * 1024 * 1024;
/// Largest number of rows accepted in a container export.
pub const MAX_CONTAINER_ROWS: usize = 4_096;

const MAX_ITEM_NAME_BYTES: usize = 256;
const LAST_KNOWN_EQUIPMENT_SLOT: u16 = 13;

/// Which inventory container a snapshot describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerKind {
    /// Bank holdings, as last observed by the exporter.
    Bank,
    /// The player's inventory.
    Inventory,
    /// The player's equipped items.
    Equipment,
}

impl ContainerKind {
    const fn key(self) -> &'static str {
        match self {
            Self::Bank => "bank",
            Self::Inventory => "inventory",
            Self::Equipment => "equipment",
        }
    }
}

/// One non-empty slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerRow {
    /// Zero-based index in the exported container.
    pub slot: u16,
    /// Canonical item identifier as exported.
    pub item_id: ItemId,
    /// Number of items in the slot.
    pub quantity: Quantity,
    /// Optional untrusted display text. Identify items by [`Self::item_id`].
    pub display_name: Option<String>,
}

/// A successfully parsed observation of one container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSnapshot {
    kind: ContainerKind,
    exported_at: ExportedAt,
    plugin_version: Option<String>,
    rows: Vec<ContainerRow>,
    warnings: Vec<ContainerWarning>,
}

impl ContainerSnapshot {
    /// The kind validated against the export.
    #[must_use]
    pub const fn kind(&self) -> ContainerKind {
        self.kind
    }

    /// The export's timestamp of last payload change.
    #[must_use]
    pub const fn exported_at(&self) -> &ExportedAt {
        &self.exported_at
    }

    /// Optional untrusted plugin version text.
    #[must_use]
    pub fn plugin_version(&self) -> Option<&str> {
        self.plugin_version.as_deref()
    }

    /// Observed rows in file order, including repeated item IDs.
    #[must_use]
    pub fn rows(&self) -> &[ContainerRow] {
        &self.rows
    }

    /// Non-fatal validation findings.
    #[must_use]
    pub fn warnings(&self) -> &[ContainerWarning] {
        &self.warnings
    }
}

/// A discrepancy that did not prevent parsing the export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerWarning {
    /// The recorded item count differs from the number of rows.
    ItemCountMismatch,
    /// A row uses an equipment slot outside the currently documented range.
    UnknownEquipmentSlot {
        /// One-based row position in the export.
        row: usize,
    },
}

/// Why a container export could not be parsed. Diagnostics contain structural
/// details only, such as a known field name or a row position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ContainerParseError {
    /// The input exceeds [`MAX_CONTAINER_EXPORT_BYTES`].
    #[error("container export exceeds the byte limit")]
    InputTooLarge,
    /// The input ended prematurely. It may have been read during a write.
    #[error("export ended unexpectedly (a write may be in progress)")]
    Truncated,
    /// The input is not valid JSON.
    #[error("export is not valid JSON")]
    InvalidJson,
    /// The input is valid JSON with a non-object top level.
    #[error("container export must be an object")]
    UnexpectedShape,
    /// A required field is absent.
    #[error("required field `{field}` is missing")]
    MissingField {
        /// The field name.
        field: &'static str,
    },
    /// A field has the wrong type or is outside its allowed bounds.
    #[error("field `{field}` has an invalid value")]
    InvalidField {
        /// The field name.
        field: &'static str,
    },
    /// The export's kind does not match the requested kind.
    #[error("export kind differs from the requested container kind")]
    WrongKind,
    /// There are more than [`MAX_CONTAINER_ROWS`] rows.
    #[error("container export exceeds the row limit")]
    TooManyRows,
    /// An item row is not an object.
    #[error("row {row} must be an object")]
    InvalidRow {
        /// One-based row position.
        row: usize,
    },
    /// A row field is missing, has the wrong type or is out of range.
    #[error("row {row} field `{field}` is missing or invalid")]
    InvalidRowField {
        /// One-based row position.
        row: usize,
        /// The field name.
        field: &'static str,
    },
    /// A slot already appeared in an earlier row.
    #[error("row {row} repeats an earlier slot")]
    DuplicateSlot {
        /// One-based row position of the repeat.
        row: usize,
    },
}

impl ContainerParseError {
    /// Whether retrying shortly may succeed if the export was being written.
    #[must_use]
    pub const fn is_transient(self) -> bool {
        matches!(self, Self::Truncated)
    }
}

/// Parses bytes from one bank, inventory or equipment export.
///
/// The parser treats file timestamps and item rows as observations and has no
/// file or network access. Optional display names may be at most 256 UTF-8
/// bytes. The timestamp and optional plugin version may each be at most 128
/// UTF-8 bytes.
///
/// # Errors
///
/// Returns [`ContainerParseError`] for malformed, missing, inconsistent or
/// oversized inputs. A differing item count and unrecognized equipment slots
/// produce warnings and preserve the observed rows.
pub fn parse_container_export(
    expected: ContainerKind,
    bytes: &[u8],
) -> Result<ContainerSnapshot, ContainerParseError> {
    let fields = decode_object(bytes, MAX_CONTAINER_EXPORT_BYTES)?;
    let kind = required(&fields, "kind")?
        .as_str()
        .filter(|text| text.len() <= MAX_ENVELOPE_TEXT_BYTES)
        .ok_or(ContainerParseError::InvalidField { field: "kind" })?;
    if kind != expected.key() {
        return Err(ContainerParseError::WrongKind);
    }

    let envelope = read_envelope(&fields)?;
    let items = required(&fields, "items")?
        .as_array()
        .ok_or(ContainerParseError::InvalidField { field: "items" })?;
    let item_count =
        required(&fields, "item_count")?
            .as_u64()
            .ok_or(ContainerParseError::InvalidField {
                field: "item_count",
            })?;
    let rows = read_rows(items)?;
    let warnings = warnings_for(expected, &rows, item_count);
    Ok(ContainerSnapshot {
        kind: expected,
        exported_at: envelope.exported_at,
        plugin_version: envelope.plugin_version,
        rows,
        warnings,
    })
}

impl From<EnvelopeError> for ContainerParseError {
    fn from(error: EnvelopeError) -> Self {
        match error {
            EnvelopeError::InputTooLarge => Self::InputTooLarge,
            EnvelopeError::Truncated => Self::Truncated,
            EnvelopeError::InvalidJson => Self::InvalidJson,
            EnvelopeError::UnexpectedShape => Self::UnexpectedShape,
            EnvelopeError::MissingField { field } => Self::MissingField { field },
            EnvelopeError::InvalidField { field } => Self::InvalidField { field },
        }
    }
}

fn read_rows(items: &[Value]) -> Result<Vec<ContainerRow>, ContainerParseError> {
    if items.len() > MAX_CONTAINER_ROWS {
        return Err(ContainerParseError::TooManyRows);
    }
    let mut seen = HashSet::with_capacity(items.len());
    let mut rows = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let row = index + 1;
        let record = read_row(item, row)?;
        if !seen.insert(record.slot) {
            return Err(ContainerParseError::DuplicateSlot { row });
        }
        rows.push(record);
    }
    Ok(rows)
}

fn read_row(item: &Value, row: usize) -> Result<ContainerRow, ContainerParseError> {
    let fields = item
        .as_object()
        .ok_or(ContainerParseError::InvalidRow { row })?;
    let invalid = |field| ContainerParseError::InvalidRowField { row, field };
    let number = |field| fields.get(field).and_then(Value::as_u64);
    let slot = number("slot")
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| invalid("slot"))?;
    let item_id = number("id")
        .and_then(positive_java_int)
        .ok_or_else(|| invalid("id"))?;
    let quantity = number("quantity")
        .and_then(positive_java_int)
        .and_then(|value| Quantity::new(u64::from(value)).ok())
        .ok_or_else(|| invalid("quantity"))?;
    let display_name = match fields.get("name") {
        None => None,
        Some(Value::String(name)) if name.is_empty() => None,
        Some(Value::String(name)) if name.len() <= MAX_ITEM_NAME_BYTES => Some(name.clone()),
        Some(_) => return Err(invalid("name")),
    };
    Ok(ContainerRow {
        slot,
        item_id: ItemId(item_id),
        quantity,
        display_name,
    })
}

fn positive_java_int(value: u64) -> Option<u32> {
    let value = i32::try_from(value).ok()?;
    (value > 0).then_some(value.unsigned_abs())
}

fn warnings_for(
    kind: ContainerKind,
    rows: &[ContainerRow],
    item_count: u64,
) -> Vec<ContainerWarning> {
    let mut warnings = Vec::new();
    if usize::try_from(item_count) != Ok(rows.len()) {
        warnings.push(ContainerWarning::ItemCountMismatch);
    }
    if kind == ContainerKind::Equipment {
        warnings.extend(
            rows.iter()
                .enumerate()
                .filter(|(_, row)| row.slot > LAST_KNOWN_EQUIPMENT_SLOT)
                .map(|(index, _)| ContainerWarning::UnknownEquipmentSlot { row: index + 1 }),
        );
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const BANK: &[u8] = include_bytes!("../../tests/fixtures/runelite_export/bank.synthetic.json");
    const INVENTORY: &[u8] =
        include_bytes!("../../tests/fixtures/runelite_export/inventory.synthetic.json");
    const EQUIPMENT: &[u8] =
        include_bytes!("../../tests/fixtures/runelite_export/equipment.synthetic.json");

    fn parse_value(
        kind: ContainerKind,
        input: &Value,
    ) -> Result<ContainerSnapshot, ContainerParseError> {
        parse_container_export(kind, input.to_string().as_bytes())
    }

    fn bank() -> Value {
        serde_json::from_slice(BANK).unwrap()
    }

    #[test]
    fn bank_preserves_order_repeated_ids_and_envelope() {
        let bank = parse_container_export(ContainerKind::Bank, BANK).unwrap();
        assert_eq!(bank.kind(), ContainerKind::Bank);
        assert_eq!(bank.exported_at().text(), "2026-01-01T13:00+01:00");
        assert_eq!(bank.exported_at().timestamp().as_second(), 1_767_268_800);
        assert_eq!(bank.plugin_version(), Some("0.6.0"));
        let rows = bank.rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].slot, 24);
        assert_eq!(rows[1].slot, 3);
        assert_eq!(rows[0].item_id, ItemId(5001));
        assert_eq!(rows[1].item_id, ItemId(5001));
        assert_eq!(rows[0].quantity.get(), 2_147_483_647);
        assert_eq!(rows[1].quantity.get(), 2);
        assert_eq!(rows[0].display_name.as_deref(), Some("Example hammer"));
        assert_eq!(rows[1].display_name, None);
        assert_eq!(bank.warnings(), []);
    }

    #[test]
    fn inventory_accepts_omitted_optional_fields() {
        let inventory = parse_container_export(ContainerKind::Inventory, INVENTORY).unwrap();
        assert_eq!(inventory.kind(), ContainerKind::Inventory);
        assert_eq!(inventory.plugin_version(), None);
        assert_eq!(inventory.rows()[0].display_name, None);
    }

    #[test]
    fn observed_empty_equipment_differs_from_missing_items() {
        let equipment = parse_container_export(ContainerKind::Equipment, EQUIPMENT).unwrap();
        assert_eq!(equipment.kind(), ContainerKind::Equipment);
        assert_eq!(equipment.rows(), []);

        let mut input = bank();
        input.as_object_mut().unwrap().remove("items");
        assert!(matches!(
            parse_value(ContainerKind::Bank, &input),
            Err(ContainerParseError::MissingField { field: "items" })
        ));
        input["items"] = Value::Null;
        assert!(matches!(
            parse_value(ContainerKind::Bank, &input),
            Err(ContainerParseError::InvalidField { field: "items" })
        ));
    }

    #[test]
    fn refuses_wrong_or_missing_kind() {
        assert!(matches!(
            parse_container_export(ContainerKind::Bank, INVENTORY),
            Err(ContainerParseError::WrongKind)
        ));
        let mut input = bank();
        input.as_object_mut().unwrap().remove("kind");
        assert!(matches!(
            parse_value(ContainerKind::Bank, &input),
            Err(ContainerParseError::MissingField { field: "kind" })
        ));
    }

    #[test]
    fn duplicate_slots_are_rejected() {
        let mut input = bank();
        input["items"][1]["slot"] = json!(24);
        assert!(matches!(
            parse_value(ContainerKind::Bank, &input),
            Err(ContainerParseError::DuplicateSlot { row: 2 })
        ));
    }

    #[test]
    fn rejects_invalid_quantities_and_identifiers() {
        let mut input = bank();
        for quantity in [
            json!(0),
            json!(-1),
            json!(2_147_483_648_u64),
            json!(1.5),
            json!("1"),
        ] {
            input["items"][0]["quantity"] = quantity;
            assert!(matches!(
                parse_value(ContainerKind::Bank, &input),
                Err(ContainerParseError::InvalidRowField {
                    row: 1,
                    field: "quantity"
                })
            ));
        }
        for id in [json!(0), json!(-1), json!(2_147_483_648_u64)] {
            input["items"][0]["id"] = id;
            assert!(matches!(
                parse_value(ContainerKind::Bank, &input),
                Err(ContainerParseError::InvalidRowField {
                    row: 1,
                    field: "id"
                })
            ));
        }
    }

    #[test]
    fn reports_disagreements_without_altering_rows() {
        let mut input = bank();
        input["kind"] = json!("equipment");
        input["item_count"] = json!(10);
        let snapshot = parse_value(ContainerKind::Equipment, &input).unwrap();
        assert_eq!(snapshot.rows().len(), 2);
        assert_eq!(snapshot.rows()[0].slot, 24);
        assert_eq!(
            snapshot.warnings(),
            [
                ContainerWarning::ItemCountMismatch,
                ContainerWarning::UnknownEquipmentSlot { row: 1 }
            ]
        );
    }

    #[test]
    fn truncation_is_transient_and_diagnostics_are_sanitized() {
        let error = parse_container_export(ContainerKind::Bank, b"{\"session_id\":\"private\"")
            .unwrap_err();
        assert_eq!(error, ContainerParseError::Truncated);
        assert!(error.is_transient());
        let mut input = bank();
        input["items"][0]["name"] = json!("secret".repeat(100));
        let error = parse_value(ContainerKind::Bank, &input).unwrap_err();
        assert!(matches!(
            error,
            ContainerParseError::InvalidRowField {
                row: 1,
                field: "name"
            }
        ));
        assert!(!error.is_transient());
        assert!(!format!("{error:?} {error}").contains("secret"));
    }

    #[test]
    fn enforces_input_row_and_utf8_text_limits() {
        let padded = [
            EQUIPMENT,
            &vec![b' '; MAX_CONTAINER_EXPORT_BYTES - EQUIPMENT.len()],
        ]
        .concat();
        assert!(parse_container_export(ContainerKind::Equipment, &padded).is_ok());
        let too_large = [padded.as_slice(), b"!"].concat();
        assert_eq!(
            parse_container_export(ContainerKind::Equipment, &too_large),
            Err(ContainerParseError::InputTooLarge)
        );

        let mut input = bank();
        input["items"][0]["name"] = json!("é".repeat(128));
        assert!(parse_value(ContainerKind::Bank, &input).is_ok());
        input["items"][0]["name"] = json!("é".repeat(129));
        assert!(parse_value(ContainerKind::Bank, &input).is_err());
        input["items"] = json!(vec![Value::Null; MAX_CONTAINER_ROWS + 1]);
        assert_eq!(
            parse_value(ContainerKind::Bank, &input),
            Err(ContainerParseError::TooManyRows)
        );
    }

    #[test]
    fn the_envelope_timestamp_and_plugin_version_are_validated() {
        let mut input = bank();
        let plugin_version = |input: &Value| {
            parse_value(ContainerKind::Bank, input)
                .map(|parsed| parsed.plugin_version().map(str::len))
        };
        let error = |input: &Value| parse_value(ContainerKind::Bank, input).err();

        input["plugin_version"] = json!("v".repeat(128));
        assert_eq!(plugin_version(&input), Ok(Some(128)));
        for invalid in [json!("v".repeat(129)), json!(7)] {
            input["plugin_version"] = invalid;
            assert_eq!(
                error(&input),
                Some(ContainerParseError::InvalidField {
                    field: "plugin_version"
                })
            );
        }
        input["plugin_version"] = Value::Null;
        assert_eq!(plugin_version(&input), Ok(None));
        input.as_object_mut().unwrap().remove("plugin_version");
        assert_eq!(plugin_version(&input), Ok(None));

        input["exported_at"] = json!("yesterday");
        assert_eq!(
            error(&input),
            Some(ContainerParseError::InvalidField {
                field: "exported_at"
            })
        );
        input.as_object_mut().unwrap().remove("exported_at");
        assert_eq!(
            error(&input),
            Some(ContainerParseError::MissingField {
                field: "exported_at"
            })
        );
    }
}

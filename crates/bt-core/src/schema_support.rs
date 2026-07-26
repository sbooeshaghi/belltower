//! Shared `schemars` helpers for foreign types that appear in the published
//! canonical schemas (`schema/belltower-*.schema.json`).
//!
//! These helpers describe the exact wire shape this workspace serializes —
//! verified empirically against live event JSON — and must only change when
//! the serde representation itself changes.

use schemars::{Schema, SchemaGenerator, json_schema};

/// Suppresses the embedded `default` for schema fields whose runtime serde
/// default is non-deterministic (for example a freshly generated random id).
/// Used as `#[schemars(skip_serializing_if = "...")]`, which schemars-derive
/// consults when deciding whether to embed a field default into the schema;
/// it never affects serde serialization.
pub fn omit_nondeterministic_default<T>(_value: &T) -> bool {
    true
}

/// Schema for `time::OffsetDateTime` as serialized by the `time` crate's
/// default serde representation (the representation used across this
/// workspace): a fixed 9-element JSON array of integers
/// `[year, day_of_year, hour, minute, second, nanosecond, offset_hour,
/// offset_minute, offset_second]`.
///
/// Example: `[2026, 101, 20, 14, 18, 505373000, 0, 0, 0]` is
/// 2026 day-of-year 101 at 20:14:18.505373 UTC.
pub fn offset_date_time_schema(_generator: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "array",
        "description": "Timestamp in the `time` crate's default serde representation: [year, day_of_year, hour, minute, second, nanosecond, offset_hour, offset_minute, offset_second].",
        "prefixItems": [
            {
                "type": "integer",
                "minimum": -9999,
                "maximum": 9999,
                "description": "calendar year"
            },
            {
                "type": "integer",
                "minimum": 1,
                "maximum": 366,
                "description": "ordinal day of the year"
            },
            {
                "type": "integer",
                "minimum": 0,
                "maximum": 23,
                "description": "hour"
            },
            {
                "type": "integer",
                "minimum": 0,
                "maximum": 59,
                "description": "minute"
            },
            {
                "type": "integer",
                "minimum": 0,
                "maximum": 59,
                "description": "second"
            },
            {
                "type": "integer",
                "minimum": 0,
                "maximum": 999999999,
                "description": "nanosecond"
            },
            {
                "type": "integer",
                "minimum": -25,
                "maximum": 25,
                "description": "UTC offset hours"
            },
            {
                "type": "integer",
                "minimum": -59,
                "maximum": 59,
                "description": "UTC offset minutes"
            },
            {
                "type": "integer",
                "minimum": -59,
                "maximum": 59,
                "description": "UTC offset seconds"
            }
        ],
        "items": false,
        "minItems": 9,
        "maxItems": 9
    })
}

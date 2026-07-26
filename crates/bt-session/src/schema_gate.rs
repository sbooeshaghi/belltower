//! Generation of the published hub JSON Schemas in `schema/` and the drift
//! gate that keeps them byte-identical to the Rust types.
//!
//! The Rust types (`bt_core::EventEnvelope`, `bt_core::SessionBundleManifest`)
//! are the source of truth; the committed `schema/*.schema.json` files are
//! generated artifacts. Regenerate them with:
//!
//! ```text
//! cargo run -p bt-session --example generate_schemas
//! ```

use bt_core::{EventEnvelope, SessionBundleManifest};
use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde_json::json;

/// File name of the published canonical event schema inside `schema/`.
pub const BELLTOWER_EVENT_SCHEMA_FILE: &str = "belltower-event-v1.schema.json";
/// File name of the published bundle manifest schema inside `schema/`.
pub const BELLTOWER_BUNDLE_MANIFEST_SCHEMA_FILE: &str = "belltower-bundle-manifest-v1.schema.json";

const EVENT_SCHEMA_ID: &str = "https://belltower.dev/schemas/belltower-event/v1.json";
const BUNDLE_MANIFEST_SCHEMA_ID: &str =
    "https://belltower.dev/schemas/belltower-bundle-manifest/v1.json";

const EVENT_SCHEMA_TITLE: &str = "Belltower canonical event envelope v1";
const BUNDLE_MANIFEST_SCHEMA_TITLE: &str = "Belltower session bundle manifest v1";

const EVENT_SCHEMA_DESCRIPTION: &str = "Canonical Belltower session event envelope, exactly as \
serialized from bt_core::EventEnvelope. This shape is served verbatim by the live event API \
(GET /sessions/{session_id}/events, with the store-assigned seq_id populated) and is the input \
to session.bt bundle canonicalization. Bundle event records (the `event` field of each \
events.jsonl line) hold the canonical form of this envelope: transport-only fields are removed \
(the envelope's `seq_id` and the payload fields `context_boundary_seq_id`, `source_seq_id`, and \
`first_kept_seq_id`), and store-local raw chunk row ids (`CompletionChunk.raw_chunk_index`, \
`RawChunkPersisted.chunk_index`) are removed and replaced by a portable \
`raw_chunk_content_ref` string (`sha256:<64 lowercase hex>`). `event_hash` covers exactly those \
canonical bytes (compact serde_json encoding of the canonical form, keys sorted). Every other \
field is semantic and identical on both surfaces.";

const BUNDLE_MANIFEST_SCHEMA_DESCRIPTION: &str = "Belltower session.bt bundle manifest \
(manifest.json at the bundle root), exactly as serialized from \
bt_core::SessionBundleManifest at schema_version 1. `bundle_hash` is the sha256 of the \
bundle's checksums.json bytes; branch heads and artifact source nodes are SessionNodeRef \
values that address canonical events by bundle ordinal and event_hash.";

/// Render the published canonical event envelope schema
/// (`schema/belltower-event-v1.schema.json`) as a deterministic pretty JSON
/// document.
#[must_use]
pub fn belltower_event_schema_json() -> String {
    render_schema::<EventEnvelope>(
        EVENT_SCHEMA_ID,
        EVENT_SCHEMA_TITLE,
        EVENT_SCHEMA_DESCRIPTION,
    )
}

/// Render the published bundle manifest schema
/// (`schema/belltower-bundle-manifest-v1.schema.json`) as a deterministic
/// pretty JSON document.
#[must_use]
pub fn belltower_bundle_manifest_schema_json() -> String {
    render_schema::<SessionBundleManifest>(
        BUNDLE_MANIFEST_SCHEMA_ID,
        BUNDLE_MANIFEST_SCHEMA_TITLE,
        BUNDLE_MANIFEST_SCHEMA_DESCRIPTION,
    )
}

fn render_schema<T: JsonSchema>(id: &str, title: &str, description: &str) -> String {
    let generator = SchemaSettings::draft2020_12().into_generator();
    let schema = generator.into_root_schema_for::<T>();
    let mut value = serde_json::to_value(&schema).expect("schema serializes to JSON");
    let object = value
        .as_object_mut()
        .expect("root schema serializes as a JSON object");
    object.insert(
        "$schema".to_owned(),
        json!("https://json-schema.org/draft/2020-12/schema"),
    );
    object.insert("$id".to_owned(), json!(id));
    object.insert("title".to_owned(), json!(title));
    object.insert("description".to_owned(), json!(description));
    let mut rendered =
        serde_json::to_string_pretty(&value).expect("schema value renders as JSON text");
    rendered.push('\n');
    rendered
}

#[cfg(test)]
mod tests {
    use super::{
        BELLTOWER_BUNDLE_MANIFEST_SCHEMA_FILE, BELLTOWER_EVENT_SCHEMA_FILE,
        belltower_bundle_manifest_schema_json, belltower_event_schema_json,
    };
    use std::path::Path;

    fn first_divergent_line(left: &str, right: &str) -> usize {
        left.lines()
            .zip(right.lines())
            .take_while(|(left, right)| left == right)
            .count()
            + 1
    }

    #[test]
    fn published_schemas_match_generated() {
        let schema_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema");
        let expected = [
            (BELLTOWER_EVENT_SCHEMA_FILE, belltower_event_schema_json()),
            (
                BELLTOWER_BUNDLE_MANIFEST_SCHEMA_FILE,
                belltower_bundle_manifest_schema_json(),
            ),
        ];
        for (file_name, generated) in expected {
            let path = schema_dir.join(file_name);
            let committed = std::fs::read_to_string(&path).unwrap_or_else(|error| {
                panic!(
                    "cannot read published schema {}: {error}; run \
                     `cargo run -p bt-session --example generate_schemas` and commit the result",
                    path.display()
                )
            });
            assert!(
                committed == generated,
                "published schema {} is stale relative to the Rust types (first divergent line \
                 {}); re-run `cargo run -p bt-session --example generate_schemas` and commit the \
                 result",
                path.display(),
                first_divergent_line(&committed, &generated),
            );
        }
    }
}

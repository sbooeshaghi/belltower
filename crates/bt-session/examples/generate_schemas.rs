//! Regenerates the published hub JSON Schemas in `schema/` from the Rust
//! types. Output is deterministic; the `published_schemas_match_generated`
//! drift gate in `bt_session::schema_gate` fails until the committed files
//! match this generator's output.
//!
//! Usage:
//!
//! ```text
//! cargo run -p bt-session --example generate_schemas
//! ```

use bt_session::schema_gate::{
    BELLTOWER_BUNDLE_MANIFEST_SCHEMA_FILE, BELLTOWER_EVENT_SCHEMA_FILE,
    belltower_bundle_manifest_schema_json, belltower_event_schema_json,
};
use std::path::PathBuf;

fn main() -> std::io::Result<()> {
    let schema_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schema");
    let outputs = [
        (BELLTOWER_EVENT_SCHEMA_FILE, belltower_event_schema_json()),
        (
            BELLTOWER_BUNDLE_MANIFEST_SCHEMA_FILE,
            belltower_bundle_manifest_schema_json(),
        ),
    ];
    for (file_name, rendered) in outputs {
        let path = schema_dir.join(file_name);
        std::fs::write(&path, rendered)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}

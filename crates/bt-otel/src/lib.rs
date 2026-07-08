#![forbid(unsafe_code)]

//! Crate entrypoint for Belltower export and telemetry helpers. Public API
//! lives here; OTLP shaping, mirrored-event projection, and message rendering
//! live in focused sibling modules so changes stay local to one concern.

mod exports;
mod mirror;
mod otlp;
mod render;

#[cfg(test)]
mod tests;

pub use exports::{SessionExport, export_session};
pub use mirror::mirror_event;
pub use otlp::export_otlp_protobuf;

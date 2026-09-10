// dashrs — data-plane exporter for the AIProf CollectionFramework.
//
// Ships the collector output directory as a single tar.gz to the
// dashboard server's HTTP endpoint (`POST /api/results/upload`, multipart).
// Ideas mirror OpenTelemetry's exporter → collector pattern: the client
// pushes on demand to a well-known endpoint; there is no daemon and no
// control-plane socket in this crate. See the crate README (and the
// approved plan file) for how CF wires this into `writer.rs`.

pub mod client;
pub mod error;
pub mod identity;
pub mod tar_gz;

pub use client::{DashClient, DashConfig, UploadResponse};
pub use error::{DashError, Result};
pub use identity::ClientIdentity;
pub use tar_gz::tar_gz_dir;

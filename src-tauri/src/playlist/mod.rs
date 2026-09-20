//! The playlist: what is queued, what is on air, and how the two advance.
//!
//! Split three ways:
//! - [`generate`] picks tracks out of the library with jingle/commercial
//!   interleaving (the auto-playlist's source of material).
//! - [`engine`] is the pure state machine — queueing, advancement, outage
//!   skip-to-cached, stop markers — expressed as transitions returning effects.
//! - [`service`] wires that machine to the database, the main deck, the
//!   prefetch cache and the renderer.
//!
//! The backend owns the playlist; the renderer mirrors the snapshots this
//! module emits. See `docs/playlist.md`.

pub mod engine;
pub mod generate;
pub mod model;
pub mod service;

pub use generate::ContentType;
pub use model::Snapshot;
pub use service::PlaylistService;

//! The hub: the Postgres database a shared library's machines connect out to.
//! Nothing the app plays, searches or edits reads it — one worker moves rows
//! through it. See `docs/shared-library.md`.

mod client;
mod schema;
mod worker;

pub use worker::{role_in_effect, Service, Status};

/// Commands a studio refuses: the scan, the analysis and what else changes the
/// library's shape are the owner's. The invoke handler checks every call
/// against this list, as it does `admin::ADMIN_COMMANDS`.
pub const OWNER_COMMANDS: &[&str] = &[
    "scan_libraries",
    "cancel_scan",
    "cancel_analysis",
    "recalculate_auto_cue",
    "purge_tracks",
    "add_path",
    "remove_path",
    "library_check_now",
    "retry_tag_write",
    "revert_track_tags",
];

/// What a refused owner command is answered with.
pub const STUDIO_ERROR: &str = "the library owner does this, not a studio";

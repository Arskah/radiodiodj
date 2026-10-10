//! The hub: the Postgres database a shared library's machines connect out to.
//! Nothing the app plays, searches or edits reads it — one worker moves rows
//! through it. See `docs/shared-library.md`.

mod client;
mod schema;
mod worker;

pub use worker::start;

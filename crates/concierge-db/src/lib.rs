//! Local `ClickHouse` metadata store for Concierge.
//!
//! Purpose: give the AI curation layer queryable context about each game's
//! mod catalog. Data comes from the PUBLIC Nexus GraphQL v2 endpoint (the
//! same one the website search uses — no API key), fetched politely
//! (throttled, incremental via `updatedAt` watermarks) and stored locally in
//! `ClickHouse` via `clickhouse local` (no server process; data persists under
//! the repo's `state/db/` directory). This is a private cache, never
//! re-served.

pub mod audit;
pub mod catalog;
/// Legacy ClickHouse store — only for `db migrate`. Behind the off-by-default
/// `clickhouse` feature so the default build neither compiles nor needs it.
#[cfg(feature = "clickhouse")]
pub mod ch;
pub mod error;
pub mod gql;
pub mod modrinth;
pub mod schema;
pub mod sync;

pub use error::{Error, Result};

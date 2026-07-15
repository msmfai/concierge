//! Concierge core: declarative Bethesda mod management (Fallout 4 prototype).
//!
//! The model is Nix's: `eval` (pure) turns the manifest into a Plan — the
//! fully-resolved desired state; `realize` (impure) fetches, builds, and syncs
//! the disposable game instance to match. The pristine Steam install is never
//! written to.

pub mod build;
pub mod check;
pub mod error;
pub mod game;
pub mod generations;
pub mod launch;
pub mod layout;
pub mod lint;
pub mod lockfile;
pub mod manifest;
pub mod manifest_edit;
pub mod nexus;
/// The `nix=` fixed-output-derivation mod source. Optional — behind the
/// `nix-source` feature so the default build never references the `nix` binary.
#[cfg(feature = "nix-source")]
pub mod nix;
pub mod pipeline;
pub mod plan;
pub mod profiles;
pub mod provision;
pub mod realize;
pub mod repo;
pub mod runtime;
pub mod sandbox;
pub mod saves;
pub mod shell;
pub mod state;
pub mod steam;
pub mod store;

pub use error::{Error, Result};

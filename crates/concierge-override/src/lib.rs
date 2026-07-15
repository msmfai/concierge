//! Concierge KOTOR accelerator — native 2DA format + `TSLPatcher` changes.ini
//! engine. Subsumes `HoloPatcher` for the common install-time-merge case so we
//! never redistribute a third-party patcher. Per-game crate: core services
//! KOTOR's file deploy; this crate adds the reconciliation.
pub mod adapter;
pub mod changes;
pub mod install;
pub mod tlk;
pub mod twoda;

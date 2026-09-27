//! Design B's consumers (docs/milestone-2-plan.md D6): each keeps one derived view of raw.db, with
//! its checkpoint in knowledge.db, driven by the worker in seq order.

pub mod claims;
pub mod compress;
pub mod digest;
pub mod fts;
pub mod gaps;
pub mod manifest;
pub mod rescan;

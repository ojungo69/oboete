//! Design B's consumers (docs/milestone-2-plan.md D6): each keeps one derived view of raw.db, with
//! its checkpoint in knowledge.db, driven by the worker in seq order.

pub mod compress;
pub mod fts;
pub mod manifest;
pub mod rescan;

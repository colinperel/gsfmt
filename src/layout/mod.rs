//! Laying a parsed formula out across lines.
//!
//! Four pieces: [`width`] measures, [`core`] lays out sequences and groups,
//! [`pairs`] handles the `LET`-style aligned forms, and [`bound`] is a second
//! model of what [`core`] and [`pairs`] will emit — see `LAYOUT-IR-PLAN.md`
//! for why that last one should not exist.

pub(crate) mod bound;
pub(crate) mod core;
pub(crate) mod pairs;
pub(crate) mod width;

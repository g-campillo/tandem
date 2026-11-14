//! Versioned local language tooling. Legacy Session remains for the M0 fixture.
mod legacy;
pub use legacy::{Session, utf16_position};
mod project;
pub use project::*;

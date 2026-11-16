//! Spawns and talks to Tandem's Node Claude helper over wire protocol v1.

pub mod client;
pub mod install;
pub mod protocol;

pub use client::*;
pub use install::*;
pub use protocol::*;

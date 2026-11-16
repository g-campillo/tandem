//! Spawns and talks to Tandem's Node Claude helper over wire protocol v1, and runs the
//! headless companion controller that the chat panel renders.

pub mod client;
pub mod companion;
mod guided;
pub mod install;
pub mod prompts;
pub mod protocol;
pub mod state;
mod takeover;
pub mod view;

pub use client::*;
pub use companion::Companion;
pub use install::*;
pub use protocol::*;
pub use state::*;

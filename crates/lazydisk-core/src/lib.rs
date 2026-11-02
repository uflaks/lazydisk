//! Shared core for the lazydisk frontends: filesystem scanning, the size tree,
//! sunburst geometry, keyboard-driven application state and file operations.
//!
//! Frontends (ratatui, gpui) translate input into [`state::Action`]s, apply them
//! to an [`state::AppState`], execute the returned [`state::Effect`]s and render
//! from the shared [`tree::Tree`] and [`layout::SunburstLayout`].

pub mod config;
pub mod format;
pub mod keymap;
pub mod layout;
pub mod ops;
pub mod palette;
pub mod scan;
pub mod state;
pub mod tree;
pub mod volumes;

pub use state::{Action, AppState, Effect};
pub use tree::{Node, NodeId, NodeKind, Tree};

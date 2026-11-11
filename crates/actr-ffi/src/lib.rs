//! libactr - UniFFI bindings for the Actor-RTC framework
//!
//! This crate provides FFI bindings for the actr framework using Mozilla UniFFI.
//!
//! ## Architecture
//!
//! The actr framework uses complex Rust features (generics, traits, async) that don't map
//! directly to UniFFI. This crate provides a "facade" layer that:
//!
//! 1. Exposes runtime wrappers and network lifecycle handles to UniFFI consumers
//! 2. Keeps callback interfaces for guest authoring APIs
//! 3. Exposes simplified APIs for creating and managing actors

mod context;
mod error;
mod log_callback;
mod logger;
mod opus;
mod realm_membership;
mod runtime;
mod types;
mod workload;

pub use error::*;
pub use log_callback::*;
pub use opus::*;
pub use realm_membership::*;
pub use runtime::*;
pub use types::*;
pub use workload::*;

// Generate UniFFI scaffolding
uniffi::setup_scaffolding!();

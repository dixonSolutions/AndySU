//! AndySU core.
//!
//! Sans-IO with respect to networking: the host fetches bytes and files, the core verifies them
//! and decides what to do. The core does touch the local filesystem, through narrow, testable
//! paths (atomic state writes, slot staging, blob ingestion), because those must be crash-safe
//! and identical on every platform.

pub mod error;
pub mod extract;
#[cfg(feature = "ffi")]
pub mod ffi;
pub mod fsx;
pub mod hash;
#[cfg(feature = "jni")]
mod jni_glue;
pub mod keys;
pub mod manifest;
pub mod patch;
pub mod payload;
pub mod select;
pub mod session;
pub mod sig;
pub mod state;
pub mod store;
pub mod time;
#[cfg(feature = "transport")]
pub mod transport;

pub use error::{Error, Result};

/// Version of this crate, reported through the C ABI.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

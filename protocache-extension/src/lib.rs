//! ProtoCache extension layer matching the extension header surface.
//!
//! For Rust-first integration, pair this crate with `protocache_core::mutable`.

mod proto;
pub mod reflection;
mod serialize;
pub mod utils;

pub use protocache_core::MutableError;

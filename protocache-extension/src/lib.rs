//! ProtoCache extension layer matching the extension header surface.
//!
//! For Rust-first integration, pair this crate with `protocache_core::mutable`.

#[cfg(all(feature = "native-proto", target_family = "unix"))]
mod proto;
pub mod reflection;
mod serialize;
pub mod utils;

pub use protocache_core::MutableError;

#[cfg(all(feature = "native-proto", not(target_family = "unix")))]
compile_error!("the `native-proto` feature is currently supported only on Unix targets");

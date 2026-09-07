//! Schema-aware conversion and reflection support for ProtoCache.
//!
//! The crate converts [`prost::Message`] and [`prost_reflect::DynamicMessage`]
//! values into ProtoCache words, loads and writes protobuf JSON, and exposes a
//! lightweight descriptor model used by the generator. Direct `.proto` source
//! parsing is available only with the `native-proto` feature on Unix; the
//! default build is pure Rust.
//!
//! Use [`utils`] for application-facing conversion helpers and [`reflection`]
//! when building schema-driven tooling.

#[cfg(all(feature = "native-proto", target_family = "unix"))]
mod proto;
/// ProtoCache's validated, lightweight schema descriptor model.
pub mod reflection;
mod serialize;
/// Protobuf, dynamic-message, JSON, and optional `.proto` conversion helpers.
pub mod utils;

pub use protocache_core::MutableError;

#[cfg(all(feature = "native-proto", not(target_family = "unix")))]
compile_error!("the `native-proto` feature is currently supported only on Unix targets");

#[cfg(all(test, feature = "native-proto", target_family = "unix"))]
mod tests;

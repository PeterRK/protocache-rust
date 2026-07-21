//! Core ProtoCache runtime primitives.
//!
//! The primary public surface keeps the original module layout:
//! - `access`
//! - `mutable`
//! - `serialize`
//! - `perfect_hash`
//! - `utils`
//!
//! For Rust-first integration, prefer the re-exported helper modules:
//! - `runtime`
//! - `mutable`
//! - `encoding`

pub mod access;
pub mod mutable;
pub mod perfect_hash;
pub mod serialize;
pub mod utils;

mod hash;

pub mod runtime {
    //! Rust-first aliases for the read-only runtime surface.
    pub use crate::access::*;
}

pub mod encoding {
    //! Rust-first aliases for encoding and buffer primitives.
    pub use crate::serialize::*;
    pub use crate::utils::{Buffer, Bytes, EnumValue, Scalar, Words};
}

pub use access::{
    ArrayIter, ArrayView, BoolArray, FieldDecode, FieldView, MapIter, MapKey, MapView, MessageView,
    PairView, ScalarArray, StringView, ViewArray, ViewMap, detect_array_with, detect_map_with,
    detect_slice_end,
};
pub use mutable::{
    MutableArray, MutableArrayElement, MutableError, MutableField, MutableMap, MutableMapKey,
    MutableMapKeyKind, MutableMessage, copy_words,
};
pub use perfect_hash::PerfectHashView;
pub use serialize::{
    Segment, Unit, build_perfect_hash_index, build_perfect_hash_index_with_positions, fold_field,
    serialize_array, serialize_array_at, serialize_array_at_mut, serialize_bool, serialize_bytes,
    serialize_map, serialize_map_at, serialize_map_at_mut, serialize_message, serialize_message_at,
    serialize_scalar, serialize_str,
};
pub use utils::{
    Buffer, Bytes, CorruptionKind, EnumValue, ReadError, Scalar, Words, compress, compress_into,
    decompress, decompress_into,
};

#[cfg(test)]
mod tests;

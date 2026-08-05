//! Extension utilities matching `extension/utils.h`.

use std::fs;
use std::path::Path;

use prost::Message;
use prost_reflect::{
    DeserializeOptions, DynamicMessage, MessageDescriptor, ReflectMessage, SerializeOptions,
};
#[cfg(all(feature = "native-proto", target_family = "unix"))]
use prost_types::FileDescriptorProto;
use protocache_core::{Buffer, MutableError};

#[cfg(all(feature = "native-proto", target_family = "unix"))]
pub use crate::proto::ProtoError;
#[cfg(all(feature = "native-proto", target_family = "unix"))]
pub use crate::proto::parse_proto;

#[derive(Debug)]
/// Error returned by protobuf JSON file helpers.
pub enum JsonError {
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl From<std::io::Error> for JsonError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for JsonError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::Json(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for JsonError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Json(err) => Some(err),
        }
    }
}

/// Transcodes a concrete prost message into owned ProtoCache words.
///
/// `descriptor` must describe `M`; a mismatch is reported as [`MutableError`].
pub fn serialize_prost<M: Message>(
    message: &M,
    descriptor: MessageDescriptor,
) -> Result<Vec<u32>, MutableError> {
    crate::serialize::serialize_prost_message(message, descriptor)
}

/// Transcodes a prost message into a reusable ProtoCache [`Buffer`].
///
/// The returned words borrow `buffer` until it is mutated again.
pub fn serialize_prost_into_buffer<'b, M: Message>(
    message: &M,
    descriptor: MessageDescriptor,
    buffer: &'b mut Buffer,
) -> Result<&'b [u32], MutableError> {
    crate::serialize::serialize_prost_message_into_buffer(message, descriptor, buffer)
}

/// Alias for [`serialize_prost`] retained for compatibility.
pub fn serialize<M: Message>(
    message: &M,
    descriptor: MessageDescriptor,
) -> Result<Vec<u32>, MutableError> {
    serialize_prost(message, descriptor)
}

/// Alias for [`serialize_prost_into_buffer`] retained for compatibility.
pub fn serialize_into_buffer<'b, M: Message>(
    message: &M,
    descriptor: MessageDescriptor,
    buffer: &'b mut Buffer,
) -> Result<&'b [u32], MutableError> {
    serialize_prost_into_buffer(message, descriptor, buffer)
}

/// Serializes a reflected protobuf message into owned ProtoCache words.
pub fn serialize_dynamic(message: &DynamicMessage) -> Result<Vec<u32>, MutableError> {
    crate::serialize::serialize_dynamic_message(message)
}

/// Serializes a reflected protobuf message into a reusable buffer.
pub fn serialize_dynamic_into_buffer<'b>(
    message: &DynamicMessage,
    buffer: &'b mut Buffer,
) -> Result<&'b [u32], MutableError> {
    crate::serialize::serialize_dynamic_message_into_buffer(message, buffer)
}

/// Loads protobuf JSON from a UTF-8 file using `descriptor` for field typing.
///
/// Unknown JSON fields are ignored to match the existing extension behavior.
pub fn load_json(
    path: impl AsRef<Path>,
    descriptor: MessageDescriptor,
) -> Result<DynamicMessage, JsonError> {
    let input = fs::read_to_string(path)?;
    let mut deserializer = serde_json::de::Deserializer::from_str(&input);
    let message = DynamicMessage::deserialize_with_options(
        descriptor,
        &mut deserializer,
        &DeserializeOptions::new().deny_unknown_fields(false),
    )?;
    deserializer.end()?;
    Ok(message)
}

/// Writes a reflected protobuf message as pretty JSON using protobuf field names.
pub fn dump_json(message: &impl ReflectMessage, path: impl AsRef<Path>) -> Result<(), JsonError> {
    let dynamic = message.transcode_to_dynamic();
    let file = fs::File::create(path)?;
    let writer = std::io::BufWriter::new(file);
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"  ");
    let mut serializer = serde_json::Serializer::with_formatter(writer, formatter);
    let options = SerializeOptions::new()
        .use_proto_field_name(true)
        .skip_default_fields(false);
    dynamic.serialize_with_options(&mut serializer, &options)?;
    Ok(())
}

#[cfg(all(feature = "native-proto", target_family = "unix"))]
/// Parses a `.proto` file through the optional Unix `libprotoc` bridge.
pub fn parse_proto_file(path: impl AsRef<Path>) -> Result<FileDescriptorProto, ProtoError> {
    crate::proto::parse_proto_file(path)
}

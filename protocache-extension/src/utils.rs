//! Extension utilities matching `extension/utils.h`.

use std::fs;
use std::path::Path;

use prost::Message;
use prost_reflect::{
    DeserializeOptions, DynamicMessage, MessageDescriptor, ReflectMessage, SerializeOptions,
};
use prost_types::FileDescriptorProto;
use protocache_core::{Buffer, MutableError};

pub use crate::proto::ProtoError;
pub use crate::proto::parse_proto;

#[derive(Debug)]
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

pub fn serialize_prost<M: Message>(
    message: &M,
    descriptor: MessageDescriptor,
) -> Result<Vec<u32>, MutableError> {
    crate::serialize::serialize_prost_message(message, descriptor)
}

pub fn serialize_prost_into_buffer<'b, M: Message>(
    message: &M,
    descriptor: MessageDescriptor,
    buffer: &'b mut Buffer,
) -> Result<&'b [u32], MutableError> {
    crate::serialize::serialize_prost_message_into_buffer(message, descriptor, buffer)
}

pub fn serialize<M: Message>(
    message: &M,
    descriptor: MessageDescriptor,
) -> Result<Vec<u32>, MutableError> {
    serialize_prost(message, descriptor)
}

pub fn serialize_into_buffer<'b, M: Message>(
    message: &M,
    descriptor: MessageDescriptor,
    buffer: &'b mut Buffer,
) -> Result<&'b [u32], MutableError> {
    serialize_prost_into_buffer(message, descriptor, buffer)
}

pub fn serialize_dynamic(message: &DynamicMessage) -> Result<Vec<u32>, MutableError> {
    crate::serialize::serialize_dynamic_message(message)
}

pub fn serialize_dynamic_into_buffer<'b>(
    message: &DynamicMessage,
    buffer: &'b mut Buffer,
) -> Result<&'b [u32], MutableError> {
    crate::serialize::serialize_dynamic_message_into_buffer(message, buffer)
}

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

pub fn dump_json(
    message: &impl ReflectMessage,
    path: impl AsRef<Path>,
) -> Result<(), JsonError> {
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

pub fn parse_proto_file(path: impl AsRef<Path>) -> Result<FileDescriptorProto, ProtoError> {
    crate::proto::parse_proto_file(path)
}

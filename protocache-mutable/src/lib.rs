use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use prost::Message;
use prost_reflect::{
    DynamicMessage, FieldDescriptor, Kind, MapKey as ReflectMapKey, MessageDescriptor,
    ReflectMessage, Value as ReflectValue,
};
use protocache_core::{
    ArrayView, Buffer, FieldView, GeneratedDescriptor, MapView, MessageView, StringView, Unit,
    build_perfect_hash_index_with_positions, serialize_array, serialize_bool, serialize_bytes,
    serialize_map, serialize_message, serialize_scalar, serialize_str,
};
use protocache_schema::load_reflect_descriptor_pool_from_proto_file;

#[derive(Debug)]
pub enum MutableError {
    InvalidRoot { descriptor: String },
    MissingField { descriptor: String, field: String },
    TypeMismatch { descriptor: String, field: String, expected: &'static str },
    InvalidMapKey { descriptor: String, field: String, key: String },
    SerializeFailed { descriptor: String, field: String },
    Schema(protocache_schema::ProtoError),
}

impl From<protocache_schema::ProtoError> for MutableError {
    fn from(value: protocache_schema::ProtoError) -> Self {
        Self::Schema(value)
    }
}

impl std::fmt::Display for MutableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRoot { descriptor } => write!(f, "invalid root for {descriptor}"),
            Self::MissingField { descriptor, field } => {
                write!(f, "missing field {field} in {descriptor}")
            }
            Self::TypeMismatch {
                descriptor,
                field,
                expected,
            } => write!(f, "type mismatch for {descriptor}.{field}, expected {expected}"),
            Self::InvalidMapKey {
                descriptor,
                field,
                key,
            } => write!(f, "invalid map key {key} for {descriptor}.{field}"),
            Self::SerializeFailed { descriptor, field } => {
                write!(f, "failed to serialize {descriptor}.{field}")
            }
            Self::Schema(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for MutableError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Schema(err) => Some(err),
            Self::InvalidRoot { .. }
            | Self::MissingField { .. }
            | Self::TypeMismatch { .. }
            | Self::InvalidMapKey { .. }
            | Self::SerializeFailed { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct DescriptorCacheKey {
    schema_path: PathBuf,
    root_message: String,
}

fn descriptor_cache() -> &'static Mutex<HashMap<DescriptorCacheKey, MessageDescriptor>> {
    static CACHE: OnceLock<Mutex<HashMap<DescriptorCacheKey, MessageDescriptor>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn normalize_schema_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub trait GeneratedMutableMessage: GeneratedDescriptor {
    fn open_mut<'a>(
        schema_path: impl AsRef<Path>,
        words: &'a [u32],
    ) -> Result<MessageMut<'a>, MutableError>
    where
        Self: Sized,
    {
        MessageMut::from_generated_type::<Self>(schema_path, words)
    }

    fn new_mut(schema_path: impl AsRef<Path>) -> Result<MessageMut<'static>, MutableError>
    where
        Self: Sized,
    {
        MessageMut::new_empty_from_generated_type::<Self>(schema_path)
    }
}

impl<T: GeneratedDescriptor> GeneratedMutableMessage for T {}

enum KeyBytes<'a> {
    Borrowed(&'a [u8]),
    Inline { len: usize, data: [u8; 8] },
}

impl AsRef<[u8]> for KeyBytes<'_> {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Borrowed(bytes) => bytes,
            Self::Inline { len, data } => &data[..*len],
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum MapKeyMut {
    String(String),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
}

impl MapKeyMut {
    fn as_bytes(&self) -> KeyBytes<'_> {
        match self {
            Self::String(value) => KeyBytes::Borrowed(value.as_bytes()),
            Self::I32(value) => {
                let mut data = [0u8; 8];
                data[..4].copy_from_slice(&value.to_le_bytes());
                KeyBytes::Inline { len: 4, data }
            }
            Self::U32(value) => {
                let mut data = [0u8; 8];
                data[..4].copy_from_slice(&value.to_le_bytes());
                KeyBytes::Inline { len: 4, data }
            }
            Self::I64(value) => KeyBytes::Inline {
                len: 8,
                data: value.to_le_bytes(),
            },
            Self::U64(value) => KeyBytes::Inline {
                len: 8,
                data: value.to_le_bytes(),
            },
        }
    }
}

#[derive(Clone)]
enum ValueKind {
    Bool,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
    String,
    Bytes,
    Enum,
    Message(MessageDescriptor),
}

#[derive(Clone)]
pub struct ListMut<'a> {
    item_kind: ValueKind,
    items: Vec<MutableValue<'a>>,
}

impl<'a> ListMut<'a> {
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn set_bool(&mut self, index: usize, value: bool) -> Result<(), MutableError> {
        self.set_value(index, MutableValue::Bool(value), "bool list element")
    }

    pub fn set_i32(&mut self, index: usize, value: i32) -> Result<(), MutableError> {
        self.set_value(index, MutableValue::I32(value), "int32 list element")
    }

    pub fn set_u32(&mut self, index: usize, value: u32) -> Result<(), MutableError> {
        self.set_value(index, MutableValue::U32(value), "uint32 list element")
    }

    pub fn set_i64(&mut self, index: usize, value: i64) -> Result<(), MutableError> {
        self.set_value(index, MutableValue::I64(value), "int64 list element")
    }

    pub fn set_u64(&mut self, index: usize, value: u64) -> Result<(), MutableError> {
        self.set_value(index, MutableValue::U64(value), "uint64 list element")
    }

    pub fn set_f32(&mut self, index: usize, value: f32) -> Result<(), MutableError> {
        self.set_value(index, MutableValue::F32(value), "float list element")
    }

    pub fn set_f64(&mut self, index: usize, value: f64) -> Result<(), MutableError> {
        self.set_value(index, MutableValue::F64(value), "double list element")
    }

    pub fn set_string(
        &mut self,
        index: usize,
        value: impl Into<String>,
    ) -> Result<(), MutableError> {
        self.set_value(index, MutableValue::String(value.into()), "string list element")
    }

    pub fn set_bytes(&mut self, index: usize, value: impl Into<Vec<u8>>) -> Result<(), MutableError> {
        self.set_value(index, MutableValue::Bytes(value.into()), "bytes list element")
    }

    pub fn set_enum_number(&mut self, index: usize, value: i32) -> Result<(), MutableError> {
        self.set_value(index, MutableValue::EnumNumber(value), "enum list element")
    }

    pub fn push_bool(&mut self, value: bool) -> Result<(), MutableError> {
        self.push_value(MutableValue::Bool(value), "bool")
    }

    pub fn push_i32(&mut self, value: i32) -> Result<(), MutableError> {
        self.push_value(MutableValue::I32(value), "int32")
    }

    pub fn push_u32(&mut self, value: u32) -> Result<(), MutableError> {
        self.push_value(MutableValue::U32(value), "uint32")
    }

    pub fn push_i64(&mut self, value: i64) -> Result<(), MutableError> {
        self.push_value(MutableValue::I64(value), "int64")
    }

    pub fn push_u64(&mut self, value: u64) -> Result<(), MutableError> {
        self.push_value(MutableValue::U64(value), "uint64")
    }

    pub fn push_f32(&mut self, value: f32) -> Result<(), MutableError> {
        self.push_value(MutableValue::F32(value), "float")
    }

    pub fn push_f64(&mut self, value: f64) -> Result<(), MutableError> {
        self.push_value(MutableValue::F64(value), "double")
    }

    pub fn push_string(&mut self, value: impl Into<String>) -> Result<(), MutableError> {
        self.push_value(MutableValue::String(value.into()), "string")
    }

    pub fn push_bytes(&mut self, value: impl Into<Vec<u8>>) -> Result<(), MutableError> {
        self.push_value(MutableValue::Bytes(value.into()), "bytes")
    }

    pub fn push_enum_number(&mut self, value: i32) -> Result<(), MutableError> {
        self.push_value(MutableValue::EnumNumber(value), "enum")
    }

    pub fn get_map_mut(&mut self, index: usize) -> Result<&mut MapMut<'a>, MutableError> {
        let item = self.items.get_mut(index).ok_or_else(|| MutableError::TypeMismatch {
            descriptor: "<list>".to_owned(),
            field: index.to_string(),
            expected: "map element",
        })?;
        item.as_map_mut("<list>", &index.to_string())
    }

    fn set_value(
        &mut self,
        index: usize,
        value: MutableValue<'a>,
        expected: &'static str,
    ) -> Result<(), MutableError> {
        let item = self.items.get_mut(index).ok_or_else(|| MutableError::TypeMismatch {
            descriptor: "<list>".to_owned(),
            field: index.to_string(),
            expected,
        })?;
        *item = value;
        Ok(())
    }

    fn push_value(
        &mut self,
        value: MutableValue<'a>,
        expected: &'static str,
    ) -> Result<(), MutableError> {
        ensure_value_matches_kind(&self.item_kind, &value, "<list>", expected)?;
        self.items.push(value);
        Ok(())
    }

    pub fn get_message_mut(&mut self, index: usize) -> Result<&mut MessageMut<'a>, MutableError> {
        let item = self.items.get_mut(index).ok_or_else(|| MutableError::TypeMismatch {
            descriptor: "<list>".to_owned(),
            field: index.to_string(),
            expected: "message list element",
        })?;
        item.as_message_mut("<list>", &index.to_string())
    }

    pub fn get_list_mut(&mut self, index: usize) -> Result<&mut ListMut<'a>, MutableError> {
        let item = self.items.get_mut(index).ok_or_else(|| MutableError::TypeMismatch {
            descriptor: "<list>".to_owned(),
            field: index.to_string(),
            expected: "list element",
        })?;
        item.as_list_mut("<list>", &index.to_string())
    }
}

#[derive(Clone)]
pub struct MapMut<'a> {
    descriptor_name: String,
    field_name: String,
    key_kind: ValueKind,
    value_kind: ValueKind,
    entries: BTreeMap<MapKeyMut, MutableValue<'a>>,
}

impl<'a> MapMut<'a> {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn remove_str(&mut self, key: &str) -> bool {
        self.entries.remove(&MapKeyMut::String(key.to_owned())).is_some()
    }

    pub fn remove_i32(&mut self, key: i32) -> bool {
        self.entries.remove(&MapKeyMut::I32(key)).is_some()
    }

    pub fn remove_u32(&mut self, key: u32) -> bool {
        self.entries.remove(&MapKeyMut::U32(key)).is_some()
    }

    pub fn remove_i64(&mut self, key: i64) -> bool {
        self.entries.remove(&MapKeyMut::I64(key)).is_some()
    }

    pub fn remove_u64(&mut self, key: u64) -> bool {
        self.entries.remove(&MapKeyMut::U64(key)).is_some()
    }

    pub fn set_bool_str(&mut self, key: &str, value: bool) -> Result<(), MutableError> {
        self.set_str_value(key, MutableValue::Bool(value), "bool")
    }

    pub fn set_i32_str(&mut self, key: &str, value: i32) -> Result<(), MutableError> {
        self.set_str_value(key, MutableValue::I32(value), "int32")
    }

    pub fn set_u32_str(&mut self, key: &str, value: u32) -> Result<(), MutableError> {
        self.set_str_value(key, MutableValue::U32(value), "uint32")
    }

    pub fn set_i64_str(&mut self, key: &str, value: i64) -> Result<(), MutableError> {
        self.set_str_value(key, MutableValue::I64(value), "int64")
    }

    pub fn set_u64_str(&mut self, key: &str, value: u64) -> Result<(), MutableError> {
        self.set_str_value(key, MutableValue::U64(value), "uint64")
    }

    pub fn set_f32_str(&mut self, key: &str, value: f32) -> Result<(), MutableError> {
        self.set_str_value(key, MutableValue::F32(value), "float")
    }

    pub fn set_f64_str(&mut self, key: &str, value: f64) -> Result<(), MutableError> {
        self.set_str_value(key, MutableValue::F64(value), "double")
    }

    pub fn set_string_str(&mut self, key: &str, value: impl Into<String>) -> Result<(), MutableError> {
        self.set_str_value(key, MutableValue::String(value.into()), "string")
    }

    pub fn set_bytes_str(&mut self, key: &str, value: impl Into<Vec<u8>>) -> Result<(), MutableError> {
        self.set_str_value(key, MutableValue::Bytes(value.into()), "bytes")
    }

    pub fn set_enum_number_str(&mut self, key: &str, value: i32) -> Result<(), MutableError> {
        self.set_str_value(key, MutableValue::EnumNumber(value), "enum")
    }

    pub fn set_bool_i32(&mut self, key: i32, value: bool) -> Result<(), MutableError> {
        self.set_i32_value(key, MutableValue::Bool(value), "bool")
    }

    pub fn set_i32_i32(&mut self, key: i32, value: i32) -> Result<(), MutableError> {
        self.set_i32_value(key, MutableValue::I32(value), "int32")
    }

    pub fn set_u32_i32(&mut self, key: i32, value: u32) -> Result<(), MutableError> {
        self.set_i32_value(key, MutableValue::U32(value), "uint32")
    }

    pub fn set_i64_i32(&mut self, key: i32, value: i64) -> Result<(), MutableError> {
        self.set_i32_value(key, MutableValue::I64(value), "int64")
    }

    pub fn set_u64_i32(&mut self, key: i32, value: u64) -> Result<(), MutableError> {
        self.set_i32_value(key, MutableValue::U64(value), "uint64")
    }

    pub fn set_f32_i32(&mut self, key: i32, value: f32) -> Result<(), MutableError> {
        self.set_i32_value(key, MutableValue::F32(value), "float")
    }

    pub fn set_f64_i32(&mut self, key: i32, value: f64) -> Result<(), MutableError> {
        self.set_i32_value(key, MutableValue::F64(value), "double")
    }

    pub fn set_string_i32(&mut self, key: i32, value: impl Into<String>) -> Result<(), MutableError> {
        self.set_i32_value(key, MutableValue::String(value.into()), "string")
    }

    pub fn set_bytes_i32(&mut self, key: i32, value: impl Into<Vec<u8>>) -> Result<(), MutableError> {
        self.set_i32_value(key, MutableValue::Bytes(value.into()), "bytes")
    }

    pub fn set_enum_number_i32(&mut self, key: i32, value: i32) -> Result<(), MutableError> {
        self.set_i32_value(key, MutableValue::EnumNumber(value), "enum")
    }

    pub fn set_bool_u32(&mut self, key: u32, value: bool) -> Result<(), MutableError> {
        self.set_u32_value(key, MutableValue::Bool(value), "bool")
    }

    pub fn set_i32_u32(&mut self, key: u32, value: i32) -> Result<(), MutableError> {
        self.set_u32_value(key, MutableValue::I32(value), "int32")
    }

    pub fn set_u32_u32(&mut self, key: u32, value: u32) -> Result<(), MutableError> {
        self.set_u32_value(key, MutableValue::U32(value), "uint32")
    }

    pub fn set_i64_u32(&mut self, key: u32, value: i64) -> Result<(), MutableError> {
        self.set_u32_value(key, MutableValue::I64(value), "int64")
    }

    pub fn set_u64_u32(&mut self, key: u32, value: u64) -> Result<(), MutableError> {
        self.set_u32_value(key, MutableValue::U64(value), "uint64")
    }

    pub fn set_f32_u32(&mut self, key: u32, value: f32) -> Result<(), MutableError> {
        self.set_u32_value(key, MutableValue::F32(value), "float")
    }

    pub fn set_f64_u32(&mut self, key: u32, value: f64) -> Result<(), MutableError> {
        self.set_u32_value(key, MutableValue::F64(value), "double")
    }

    pub fn set_string_u32(&mut self, key: u32, value: impl Into<String>) -> Result<(), MutableError> {
        self.set_u32_value(key, MutableValue::String(value.into()), "string")
    }

    pub fn set_bytes_u32(&mut self, key: u32, value: impl Into<Vec<u8>>) -> Result<(), MutableError> {
        self.set_u32_value(key, MutableValue::Bytes(value.into()), "bytes")
    }

    pub fn set_enum_number_u32(&mut self, key: u32, value: i32) -> Result<(), MutableError> {
        self.set_u32_value(key, MutableValue::EnumNumber(value), "enum")
    }

    pub fn set_bool_i64(&mut self, key: i64, value: bool) -> Result<(), MutableError> {
        self.set_i64_value(key, MutableValue::Bool(value), "bool")
    }

    pub fn set_i32_i64(&mut self, key: i64, value: i32) -> Result<(), MutableError> {
        self.set_i64_value(key, MutableValue::I32(value), "int32")
    }

    pub fn set_u32_i64(&mut self, key: i64, value: u32) -> Result<(), MutableError> {
        self.set_i64_value(key, MutableValue::U32(value), "uint32")
    }

    pub fn set_i64_i64(&mut self, key: i64, value: i64) -> Result<(), MutableError> {
        self.set_i64_value(key, MutableValue::I64(value), "int64")
    }

    pub fn set_u64_i64(&mut self, key: i64, value: u64) -> Result<(), MutableError> {
        self.set_i64_value(key, MutableValue::U64(value), "uint64")
    }

    pub fn set_f32_i64(&mut self, key: i64, value: f32) -> Result<(), MutableError> {
        self.set_i64_value(key, MutableValue::F32(value), "float")
    }

    pub fn set_f64_i64(&mut self, key: i64, value: f64) -> Result<(), MutableError> {
        self.set_i64_value(key, MutableValue::F64(value), "double")
    }

    pub fn set_string_i64(&mut self, key: i64, value: impl Into<String>) -> Result<(), MutableError> {
        self.set_i64_value(key, MutableValue::String(value.into()), "string")
    }

    pub fn set_bytes_i64(&mut self, key: i64, value: impl Into<Vec<u8>>) -> Result<(), MutableError> {
        self.set_i64_value(key, MutableValue::Bytes(value.into()), "bytes")
    }

    pub fn set_enum_number_i64(&mut self, key: i64, value: i32) -> Result<(), MutableError> {
        self.set_i64_value(key, MutableValue::EnumNumber(value), "enum")
    }

    pub fn set_bool_u64(&mut self, key: u64, value: bool) -> Result<(), MutableError> {
        self.set_u64_value(key, MutableValue::Bool(value), "bool")
    }

    pub fn set_i32_u64(&mut self, key: u64, value: i32) -> Result<(), MutableError> {
        self.set_u64_value(key, MutableValue::I32(value), "int32")
    }

    pub fn set_u32_u64(&mut self, key: u64, value: u32) -> Result<(), MutableError> {
        self.set_u64_value(key, MutableValue::U32(value), "uint32")
    }

    pub fn set_i64_u64(&mut self, key: u64, value: i64) -> Result<(), MutableError> {
        self.set_u64_value(key, MutableValue::I64(value), "int64")
    }

    pub fn set_u64_u64(&mut self, key: u64, value: u64) -> Result<(), MutableError> {
        self.set_u64_value(key, MutableValue::U64(value), "uint64")
    }

    pub fn set_f32_u64(&mut self, key: u64, value: f32) -> Result<(), MutableError> {
        self.set_u64_value(key, MutableValue::F32(value), "float")
    }

    pub fn set_f64_u64(&mut self, key: u64, value: f64) -> Result<(), MutableError> {
        self.set_u64_value(key, MutableValue::F64(value), "double")
    }

    pub fn set_string_u64(&mut self, key: u64, value: impl Into<String>) -> Result<(), MutableError> {
        self.set_u64_value(key, MutableValue::String(value.into()), "string")
    }

    pub fn set_bytes_u64(&mut self, key: u64, value: impl Into<Vec<u8>>) -> Result<(), MutableError> {
        self.set_u64_value(key, MutableValue::Bytes(value.into()), "bytes")
    }

    pub fn set_enum_number_u64(&mut self, key: u64, value: i32) -> Result<(), MutableError> {
        self.set_u64_value(key, MutableValue::EnumNumber(value), "enum")
    }

    pub fn get_message_mut_str(
        &mut self,
        key: &str,
    ) -> Result<&mut MessageMut<'a>, MutableError> {
        let entry = self
            .entries
            .get_mut(&MapKeyMut::String(key.to_owned()))
            .ok_or_else(|| MutableError::InvalidMapKey {
                descriptor: self.descriptor_name.clone(),
                field: self.field_name.clone(),
                key: key.to_owned(),
            })?;
        entry.as_message_mut(&self.descriptor_name, &self.field_name)
    }

    pub fn get_list_mut_str(&mut self, key: &str) -> Result<&mut ListMut<'a>, MutableError> {
        let entry = self
            .entries
            .get_mut(&MapKeyMut::String(key.to_owned()))
            .ok_or_else(|| MutableError::InvalidMapKey {
                descriptor: self.descriptor_name.clone(),
                field: self.field_name.clone(),
                key: key.to_owned(),
            })?;
        entry.as_list_mut(&self.descriptor_name, &self.field_name)
    }

    pub fn get_map_mut_str(&mut self, key: &str) -> Result<&mut MapMut<'a>, MutableError> {
        let entry = self
            .entries
            .get_mut(&MapKeyMut::String(key.to_owned()))
            .ok_or_else(|| MutableError::InvalidMapKey {
                descriptor: self.descriptor_name.clone(),
                field: self.field_name.clone(),
                key: key.to_owned(),
            })?;
        entry.as_map_mut(&self.descriptor_name, &self.field_name)
    }

    pub fn get_message_mut_i32(&mut self, key: i32) -> Result<&mut MessageMut<'a>, MutableError> {
        let entry = self
            .entries
            .get_mut(&MapKeyMut::I32(key))
            .ok_or_else(|| MutableError::InvalidMapKey {
                descriptor: self.descriptor_name.clone(),
                field: self.field_name.clone(),
                key: key.to_string(),
            })?;
        entry.as_message_mut(&self.descriptor_name, &self.field_name)
    }

    pub fn get_list_mut_i32(&mut self, key: i32) -> Result<&mut ListMut<'a>, MutableError> {
        let entry = self
            .entries
            .get_mut(&MapKeyMut::I32(key))
            .ok_or_else(|| MutableError::InvalidMapKey {
                descriptor: self.descriptor_name.clone(),
                field: self.field_name.clone(),
                key: key.to_string(),
            })?;
        entry.as_list_mut(&self.descriptor_name, &self.field_name)
    }

    pub fn get_map_mut_i32(&mut self, key: i32) -> Result<&mut MapMut<'a>, MutableError> {
        let entry = self
            .entries
            .get_mut(&MapKeyMut::I32(key))
            .ok_or_else(|| MutableError::InvalidMapKey {
                descriptor: self.descriptor_name.clone(),
                field: self.field_name.clone(),
                key: key.to_string(),
            })?;
        entry.as_map_mut(&self.descriptor_name, &self.field_name)
    }

    pub fn get_message_mut_u32(&mut self, key: u32) -> Result<&mut MessageMut<'a>, MutableError> {
        let descriptor_name = self.descriptor_name.clone();
        let field_name = self.field_name.clone();
        let entry = self.get_map_entry_mut(MapKeyMut::U32(key), key.to_string())?;
        entry.as_message_mut(&descriptor_name, &field_name)
    }

    pub fn get_list_mut_u32(&mut self, key: u32) -> Result<&mut ListMut<'a>, MutableError> {
        let descriptor_name = self.descriptor_name.clone();
        let field_name = self.field_name.clone();
        let entry = self.get_map_entry_mut(MapKeyMut::U32(key), key.to_string())?;
        entry.as_list_mut(&descriptor_name, &field_name)
    }

    pub fn get_map_mut_u32(&mut self, key: u32) -> Result<&mut MapMut<'a>, MutableError> {
        let descriptor_name = self.descriptor_name.clone();
        let field_name = self.field_name.clone();
        let entry = self.get_map_entry_mut(MapKeyMut::U32(key), key.to_string())?;
        entry.as_map_mut(&descriptor_name, &field_name)
    }

    pub fn get_message_mut_i64(&mut self, key: i64) -> Result<&mut MessageMut<'a>, MutableError> {
        let descriptor_name = self.descriptor_name.clone();
        let field_name = self.field_name.clone();
        let entry = self.get_map_entry_mut(MapKeyMut::I64(key), key.to_string())?;
        entry.as_message_mut(&descriptor_name, &field_name)
    }

    pub fn get_list_mut_i64(&mut self, key: i64) -> Result<&mut ListMut<'a>, MutableError> {
        let descriptor_name = self.descriptor_name.clone();
        let field_name = self.field_name.clone();
        let entry = self.get_map_entry_mut(MapKeyMut::I64(key), key.to_string())?;
        entry.as_list_mut(&descriptor_name, &field_name)
    }

    pub fn get_map_mut_i64(&mut self, key: i64) -> Result<&mut MapMut<'a>, MutableError> {
        let descriptor_name = self.descriptor_name.clone();
        let field_name = self.field_name.clone();
        let entry = self.get_map_entry_mut(MapKeyMut::I64(key), key.to_string())?;
        entry.as_map_mut(&descriptor_name, &field_name)
    }

    pub fn get_message_mut_u64(&mut self, key: u64) -> Result<&mut MessageMut<'a>, MutableError> {
        let descriptor_name = self.descriptor_name.clone();
        let field_name = self.field_name.clone();
        let entry = self.get_map_entry_mut(MapKeyMut::U64(key), key.to_string())?;
        entry.as_message_mut(&descriptor_name, &field_name)
    }

    pub fn get_list_mut_u64(&mut self, key: u64) -> Result<&mut ListMut<'a>, MutableError> {
        let descriptor_name = self.descriptor_name.clone();
        let field_name = self.field_name.clone();
        let entry = self.get_map_entry_mut(MapKeyMut::U64(key), key.to_string())?;
        entry.as_list_mut(&descriptor_name, &field_name)
    }

    pub fn get_map_mut_u64(&mut self, key: u64) -> Result<&mut MapMut<'a>, MutableError> {
        let descriptor_name = self.descriptor_name.clone();
        let field_name = self.field_name.clone();
        let entry = self.get_map_entry_mut(MapKeyMut::U64(key), key.to_string())?;
        entry.as_map_mut(&descriptor_name, &field_name)
    }

    pub fn get_or_insert_message_str(
        &mut self,
        key: &str,
    ) -> Result<&mut MessageMut<'a>, MutableError> {
        ensure_key_kind(&self.key_kind, &MapKeyMut::String(String::new()), &self.descriptor_name, &self.field_name)?;
        let value = self
            .entries
            .entry(MapKeyMut::String(key.to_owned()))
            .or_insert_with(|| MutableValue::default_for_kind(&self.value_kind));
        value.as_message_mut(&self.descriptor_name, &self.field_name)
    }

    pub fn get_or_insert_list_str(&mut self, key: &str) -> Result<&mut ListMut<'a>, MutableError> {
        ensure_key_kind(
            &self.key_kind,
            &MapKeyMut::String(String::new()),
            &self.descriptor_name,
            &self.field_name,
        )?;
        let value = self
            .entries
            .entry(MapKeyMut::String(key.to_owned()))
            .or_insert_with(|| MutableValue::default_for_kind(&self.value_kind));
        value.as_list_mut(&self.descriptor_name, &self.field_name)
    }

    pub fn get_or_insert_map_str(&mut self, key: &str) -> Result<&mut MapMut<'a>, MutableError> {
        ensure_key_kind(
            &self.key_kind,
            &MapKeyMut::String(String::new()),
            &self.descriptor_name,
            &self.field_name,
        )?;
        let value = self
            .entries
            .entry(MapKeyMut::String(key.to_owned()))
            .or_insert_with(|| MutableValue::default_for_kind(&self.value_kind));
        value.as_map_mut(&self.descriptor_name, &self.field_name)
    }

    pub fn get_or_insert_message_i32(&mut self, key: i32) -> Result<&mut MessageMut<'a>, MutableError> {
        ensure_key_kind(
            &self.key_kind,
            &MapKeyMut::I32(0),
            &self.descriptor_name,
            &self.field_name,
        )?;
        let value = self
            .entries
            .entry(MapKeyMut::I32(key))
            .or_insert_with(|| MutableValue::default_for_kind(&self.value_kind));
        value.as_message_mut(&self.descriptor_name, &self.field_name)
    }

    pub fn get_or_insert_list_i32(&mut self, key: i32) -> Result<&mut ListMut<'a>, MutableError> {
        ensure_key_kind(
            &self.key_kind,
            &MapKeyMut::I32(0),
            &self.descriptor_name,
            &self.field_name,
        )?;
        let value = self
            .entries
            .entry(MapKeyMut::I32(key))
            .or_insert_with(|| MutableValue::default_for_kind(&self.value_kind));
        value.as_list_mut(&self.descriptor_name, &self.field_name)
    }

    pub fn get_or_insert_map_i32(&mut self, key: i32) -> Result<&mut MapMut<'a>, MutableError> {
        ensure_key_kind(
            &self.key_kind,
            &MapKeyMut::I32(0),
            &self.descriptor_name,
            &self.field_name,
        )?;
        let value = self
            .entries
            .entry(MapKeyMut::I32(key))
            .or_insert_with(|| MutableValue::default_for_kind(&self.value_kind));
        value.as_map_mut(&self.descriptor_name, &self.field_name)
    }

    pub fn get_or_insert_message_u32(&mut self, key: u32) -> Result<&mut MessageMut<'a>, MutableError> {
        let descriptor_name = self.descriptor_name.clone();
        let field_name = self.field_name.clone();
        self.get_or_insert_value(MapKeyMut::U32(key))?
            .as_message_mut(&descriptor_name, &field_name)
    }

    pub fn get_or_insert_list_u32(&mut self, key: u32) -> Result<&mut ListMut<'a>, MutableError> {
        let descriptor_name = self.descriptor_name.clone();
        let field_name = self.field_name.clone();
        self.get_or_insert_value(MapKeyMut::U32(key))?
            .as_list_mut(&descriptor_name, &field_name)
    }

    pub fn get_or_insert_map_u32(&mut self, key: u32) -> Result<&mut MapMut<'a>, MutableError> {
        let descriptor_name = self.descriptor_name.clone();
        let field_name = self.field_name.clone();
        self.get_or_insert_value(MapKeyMut::U32(key))?
            .as_map_mut(&descriptor_name, &field_name)
    }

    pub fn get_or_insert_message_i64(&mut self, key: i64) -> Result<&mut MessageMut<'a>, MutableError> {
        let descriptor_name = self.descriptor_name.clone();
        let field_name = self.field_name.clone();
        self.get_or_insert_value(MapKeyMut::I64(key))?
            .as_message_mut(&descriptor_name, &field_name)
    }

    pub fn get_or_insert_list_i64(&mut self, key: i64) -> Result<&mut ListMut<'a>, MutableError> {
        let descriptor_name = self.descriptor_name.clone();
        let field_name = self.field_name.clone();
        self.get_or_insert_value(MapKeyMut::I64(key))?
            .as_list_mut(&descriptor_name, &field_name)
    }

    pub fn get_or_insert_map_i64(&mut self, key: i64) -> Result<&mut MapMut<'a>, MutableError> {
        let descriptor_name = self.descriptor_name.clone();
        let field_name = self.field_name.clone();
        self.get_or_insert_value(MapKeyMut::I64(key))?
            .as_map_mut(&descriptor_name, &field_name)
    }

    pub fn get_or_insert_message_u64(&mut self, key: u64) -> Result<&mut MessageMut<'a>, MutableError> {
        let descriptor_name = self.descriptor_name.clone();
        let field_name = self.field_name.clone();
        self.get_or_insert_value(MapKeyMut::U64(key))?
            .as_message_mut(&descriptor_name, &field_name)
    }

    pub fn get_or_insert_list_u64(&mut self, key: u64) -> Result<&mut ListMut<'a>, MutableError> {
        let descriptor_name = self.descriptor_name.clone();
        let field_name = self.field_name.clone();
        self.get_or_insert_value(MapKeyMut::U64(key))?
            .as_list_mut(&descriptor_name, &field_name)
    }

    pub fn get_or_insert_map_u64(&mut self, key: u64) -> Result<&mut MapMut<'a>, MutableError> {
        let descriptor_name = self.descriptor_name.clone();
        let field_name = self.field_name.clone();
        self.get_or_insert_value(MapKeyMut::U64(key))?
            .as_map_mut(&descriptor_name, &field_name)
    }

    fn set_str_value(
        &mut self,
        key: &str,
        value: MutableValue<'a>,
        expected: &'static str,
    ) -> Result<(), MutableError> {
        self.set_map_value(MapKeyMut::String(key.to_owned()), value, expected)
    }

    fn set_i32_value(
        &mut self,
        key: i32,
        value: MutableValue<'a>,
        expected: &'static str,
    ) -> Result<(), MutableError> {
        self.set_map_value(MapKeyMut::I32(key), value, expected)
    }

    fn set_u32_value(
        &mut self,
        key: u32,
        value: MutableValue<'a>,
        expected: &'static str,
    ) -> Result<(), MutableError> {
        self.set_map_value(MapKeyMut::U32(key), value, expected)
    }

    fn set_i64_value(
        &mut self,
        key: i64,
        value: MutableValue<'a>,
        expected: &'static str,
    ) -> Result<(), MutableError> {
        self.set_map_value(MapKeyMut::I64(key), value, expected)
    }

    fn set_u64_value(
        &mut self,
        key: u64,
        value: MutableValue<'a>,
        expected: &'static str,
    ) -> Result<(), MutableError> {
        self.set_map_value(MapKeyMut::U64(key), value, expected)
    }

    fn set_map_value(
        &mut self,
        key: MapKeyMut,
        value: MutableValue<'a>,
        expected: &'static str,
    ) -> Result<(), MutableError> {
        ensure_key_kind(&self.key_kind, &key, &self.descriptor_name, &self.field_name)?;
        ensure_value_matches_kind(&self.value_kind, &value, &self.descriptor_name, expected)?;
        self.entries.insert(key, value);
        Ok(())
    }

    fn get_map_entry_mut(
        &mut self,
        key: MapKeyMut,
        rendered_key: String,
    ) -> Result<&mut MutableValue<'a>, MutableError> {
        self.entries.get_mut(&key).ok_or_else(|| MutableError::InvalidMapKey {
            descriptor: self.descriptor_name.clone(),
            field: self.field_name.clone(),
            key: rendered_key,
        })
    }

    fn get_or_insert_value(
        &mut self,
        key: MapKeyMut,
    ) -> Result<&mut MutableValue<'a>, MutableError> {
        ensure_key_kind(&self.key_kind, &key, &self.descriptor_name, &self.field_name)?;
        Ok(self
            .entries
            .entry(key)
            .or_insert_with(|| MutableValue::default_for_kind(&self.value_kind)))
    }
}

#[derive(Clone)]
enum MutableValue<'a> {
    Bool(bool),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    F32(f32),
    F64(f64),
    String(String),
    Bytes(Vec<u8>),
    EnumNumber(i32),
    Message(Box<MessageMut<'a>>),
    List(ListMut<'a>),
    Map(MapMut<'a>),
}

impl<'a> MutableValue<'a> {
    fn default_for_kind(kind: &ValueKind) -> Self {
        match kind {
            ValueKind::Bool => Self::Bool(false),
            ValueKind::I32 => Self::I32(0),
            ValueKind::U32 => Self::U32(0),
            ValueKind::I64 => Self::I64(0),
            ValueKind::U64 => Self::U64(0),
            ValueKind::F32 => Self::F32(0.0),
            ValueKind::F64 => Self::F64(0.0),
            ValueKind::String => Self::String(String::new()),
            ValueKind::Bytes => Self::Bytes(Vec::new()),
            ValueKind::Enum => Self::EnumNumber(0),
            ValueKind::Message(desc) => {
                if let Some(alias) = alias_field(desc) {
                    default_alias_value(desc.full_name(), &alias)
                } else {
                    Self::Message(Box::new(MessageMut::new_empty(desc.clone())))
                }
            }
        }
    }

    fn as_message_mut(
        &mut self,
        descriptor: &str,
        field: &str,
    ) -> Result<&mut MessageMut<'a>, MutableError> {
        match self {
            Self::Message(value) => Ok(value),
            _ => Err(MutableError::TypeMismatch {
                descriptor: descriptor.to_owned(),
                field: field.to_owned(),
                expected: "message",
            }),
        }
    }

    fn as_list_mut(
        &mut self,
        descriptor: &str,
        field: &str,
    ) -> Result<&mut ListMut<'a>, MutableError> {
        match self {
            Self::List(value) => Ok(value),
            _ => Err(MutableError::TypeMismatch {
                descriptor: descriptor.to_owned(),
                field: field.to_owned(),
                expected: "array",
            }),
        }
    }

    fn as_map_mut(
        &mut self,
        descriptor: &str,
        field: &str,
    ) -> Result<&mut MapMut<'a>, MutableError> {
        match self {
            Self::Map(value) => Ok(value),
            _ => Err(MutableError::TypeMismatch {
                descriptor: descriptor.to_owned(),
                field: field.to_owned(),
                expected: "map",
            }),
        }
    }

    fn is_effectively_empty(&self) -> bool {
        match self {
            Self::Message(message) => message.is_effectively_empty(),
            Self::List(list) => list.is_empty(),
            Self::Map(map) => map.is_empty(),
            _ => false,
        }
    }

    #[cfg(test)]
    fn as_i32_for_test(&self) -> Option<i32> {
        match self {
            Self::I32(value) => Some(*value),
            _ => None,
        }
    }
}

#[derive(Clone)]
enum FieldState<'a> {
    Missing,
    Borrowed(FieldView<'a>),
    DecodedClean(MutableValue<'a>),
    DirtyOwned(MutableValue<'a>),
}

#[derive(Clone)]
struct FieldSlot<'a> {
    descriptor: FieldDescriptor,
    state: FieldState<'a>,
}

#[derive(Clone)]
pub struct MessageMut<'a> {
    descriptor: MessageDescriptor,
    fields: Vec<FieldSlot<'a>>,
}

impl<'a> MessageMut<'a> {
    fn from_dynamic_message(message: &DynamicMessage) -> Result<MessageMut<'static>, MutableError> {
        let descriptor = message.descriptor();
        let mut fields = Vec::with_capacity(descriptor.fields().len());
        for field in descriptor.fields() {
            let state = if message.has_field(&field) {
                FieldState::DecodedClean(reflect_field_to_mutable_value(
                    descriptor.full_name(),
                    &field,
                    message.get_field(&field).as_ref(),
                )?)
            } else {
                FieldState::Missing
            };
            fields.push(FieldSlot {
                descriptor: field,
                state,
            });
        }
        Ok(MessageMut { descriptor, fields })
    }

    pub fn from_generated_type<T: GeneratedDescriptor>(
        schema_path: impl AsRef<std::path::Path>,
        words: &'a [u32],
    ) -> Result<Self, MutableError> {
        let descriptor = load_message_descriptor(schema_path, T::FULL_NAME)?;
        Self::from_words(descriptor, words)
    }

    pub fn new_empty_from_generated_type<T: GeneratedDescriptor>(
        schema_path: impl AsRef<std::path::Path>,
    ) -> Result<Self, MutableError> {
        let descriptor = load_message_descriptor(schema_path, T::FULL_NAME)?;
        Ok(Self::new_empty(descriptor))
    }

    pub fn from_proto_file(
        schema_path: impl AsRef<std::path::Path>,
        root_message: &str,
        words: &'a [u32],
    ) -> Result<Self, MutableError> {
        let descriptor = load_message_descriptor(schema_path, root_message)?;
        Self::from_words(descriptor, words)
    }

    pub fn new_empty_from_proto_file(
        schema_path: impl AsRef<std::path::Path>,
        root_message: &str,
    ) -> Result<Self, MutableError> {
        let descriptor = load_message_descriptor(schema_path, root_message)?;
        Ok(Self::new_empty(descriptor))
    }

    pub fn new_empty(descriptor: MessageDescriptor) -> Self {
        let fields = descriptor
            .fields()
            .map(|field| FieldSlot {
                descriptor: field,
                state: FieldState::Missing,
            })
            .collect();
        Self { descriptor, fields }
    }

    pub fn from_words(
        descriptor: MessageDescriptor,
        words: &'a [u32],
    ) -> Result<Self, MutableError> {
        let view = MessageView::new(words).ok_or_else(|| MutableError::InvalidRoot {
            descriptor: descriptor.full_name().to_owned(),
        })?;
        let fields = descriptor
            .fields()
            .map(|field| FieldSlot {
                descriptor: field.clone(),
                state: view
                    .field(field.number() as usize - 1)
                    .map(FieldState::Borrowed)
                    .unwrap_or(FieldState::Missing),
            })
            .collect();
        Ok(Self { descriptor, fields })
    }

    pub fn field_message_mut(&mut self, name: &str) -> Result<&mut MessageMut<'a>, MutableError> {
        let idx = self.field_index(name)?;
        self.ensure_dirty(idx)?;
        let descriptor_name = self.descriptor.full_name().to_owned();
        self.fields[idx].state.as_value_mut().unwrap().as_message_mut(&descriptor_name, name)
    }

    pub fn field_alias_list_mut(&mut self, name: &str) -> Result<&mut ListMut<'a>, MutableError> {
        let idx = self.field_index(name)?;
        self.ensure_dirty(idx)?;
        let descriptor_name = self.descriptor.full_name().to_owned();
        self.fields[idx].state.as_value_mut().unwrap().as_list_mut(&descriptor_name, name)
    }

    pub fn field_alias_map_mut(&mut self, name: &str) -> Result<&mut MapMut<'a>, MutableError> {
        let idx = self.field_index(name)?;
        self.ensure_dirty(idx)?;
        let descriptor_name = self.descriptor.full_name().to_owned();
        self.fields[idx].state.as_value_mut().unwrap().as_map_mut(&descriptor_name, name)
    }

    pub fn field_list_mut(&mut self, name: &str) -> Result<&mut ListMut<'a>, MutableError> {
        let idx = self.field_index(name)?;
        self.ensure_dirty(idx)?;
        let descriptor_name = self.descriptor.full_name().to_owned();
        self.fields[idx].state.as_value_mut().unwrap().as_list_mut(&descriptor_name, name)
    }

    pub fn field_map_mut(&mut self, name: &str) -> Result<&mut MapMut<'a>, MutableError> {
        let idx = self.field_index(name)?;
        self.ensure_dirty(idx)?;
        let descriptor_name = self.descriptor.full_name().to_owned();
        self.fields[idx].state.as_value_mut().unwrap().as_map_mut(&descriptor_name, name)
    }

    pub fn decode_field(&mut self, name: &str) -> Result<(), MutableError> {
        let idx = self.field_index(name)?;
        self.ensure_decoded(idx)
    }

    pub fn set_i32(&mut self, name: &str, value: i32) -> Result<(), MutableError> {
        self.set_scalar_field(name, MutableValue::I32(value), "int32", |kind| {
            matches!(kind, Kind::Int32 | Kind::Sint32 | Kind::Sfixed32)
        })
    }

    pub fn set_bool(&mut self, name: &str, value: bool) -> Result<(), MutableError> {
        self.set_scalar_field(name, MutableValue::Bool(value), "bool", |kind| {
            matches!(kind, Kind::Bool)
        })
    }

    pub fn set_u32(&mut self, name: &str, value: u32) -> Result<(), MutableError> {
        self.set_scalar_field(name, MutableValue::U32(value), "uint32", |kind| {
            matches!(kind, Kind::Uint32 | Kind::Fixed32)
        })
    }

    pub fn set_i64(&mut self, name: &str, value: i64) -> Result<(), MutableError> {
        self.set_scalar_field(name, MutableValue::I64(value), "int64", |kind| {
            matches!(kind, Kind::Int64 | Kind::Sint64 | Kind::Sfixed64)
        })
    }

    pub fn set_u64(&mut self, name: &str, value: u64) -> Result<(), MutableError> {
        self.set_scalar_field(name, MutableValue::U64(value), "uint64", |kind| {
            matches!(kind, Kind::Uint64 | Kind::Fixed64)
        })
    }

    pub fn set_f32(&mut self, name: &str, value: f32) -> Result<(), MutableError> {
        self.set_scalar_field(name, MutableValue::F32(value), "float", |kind| {
            matches!(kind, Kind::Float)
        })
    }

    pub fn set_f64(&mut self, name: &str, value: f64) -> Result<(), MutableError> {
        self.set_scalar_field(name, MutableValue::F64(value), "double", |kind| {
            matches!(kind, Kind::Double)
        })
    }

    pub fn set_string(&mut self, name: &str, value: impl Into<String>) -> Result<(), MutableError> {
        self.set_scalar_field(name, MutableValue::String(value.into()), "string", |kind| {
            matches!(kind, Kind::String)
        })
    }

    pub fn set_bytes(&mut self, name: &str, value: impl Into<Vec<u8>>) -> Result<(), MutableError> {
        self.set_scalar_field(name, MutableValue::Bytes(value.into()), "bytes", |kind| {
            matches!(kind, Kind::Bytes)
        })
    }

    pub fn set_enum_number(&mut self, name: &str, value: i32) -> Result<(), MutableError> {
        self.set_scalar_field(name, MutableValue::EnumNumber(value), "enum", |kind| {
            matches!(kind, Kind::Enum(_))
        })
    }

    pub fn serialize_words(&self) -> Result<Vec<u32>, MutableError> {
        let mut buffer = Buffer::new();
        let _ = self.serialize_into_buffer(&mut buffer)?;
        Ok(buffer.view().to_vec())
    }

    pub fn serialize_into_buffer<'b>(
        &self,
        buffer: &'b mut Buffer,
    ) -> Result<&'b [u32], MutableError> {
        buffer.clear();
        let _ = self.encode(buffer)?;
        Ok(buffer.view())
    }

    fn encode(&self, buffer: &mut Buffer) -> Result<Unit, MutableError> {
        let max_number = self
            .fields
            .iter()
            .map(|field| field.descriptor.number() as usize)
            .max()
            .unwrap_or(0);
        let mut units = vec![Unit::empty(); max_number];

        for field in &self.fields {
            let id = field.descriptor.number() as usize - 1;
            units[id] = match &field.state {
                FieldState::DecodedClean(value) | FieldState::DirtyOwned(value) => encode_owned_field(
                    self.descriptor.full_name(),
                    field.descriptor.name(),
                    &field.descriptor,
                    value,
                    buffer,
                )?,
                FieldState::Borrowed(view) => borrow_field_unit(
                    self.descriptor.full_name(),
                    field.descriptor.name(),
                    &field.descriptor,
                    *view,
                    buffer,
                )?,
                FieldState::Missing => Unit::empty(),
            };
        }

        serialize_message(&mut units, buffer).ok_or_else(|| MutableError::SerializeFailed {
            descriptor: self.descriptor.full_name().to_owned(),
            field: "<message>".to_owned(),
        })
    }

    fn field_index(&self, name: &str) -> Result<usize, MutableError> {
        self.fields
            .iter()
            .position(|field| field.descriptor.name() == name)
            .ok_or_else(|| MutableError::MissingField {
                descriptor: self.descriptor.full_name().to_owned(),
                field: name.to_owned(),
            })
    }

    fn ensure_decoded(&mut self, idx: usize) -> Result<(), MutableError> {
        if matches!(
            self.fields[idx].state,
            FieldState::DecodedClean(_) | FieldState::DirtyOwned(_)
        ) {
            return Ok(());
        }
        let field = self.fields[idx].descriptor.clone();
        let owned = match self.fields[idx].state {
            FieldState::Borrowed(view) => decode_field_value(self.descriptor.full_name(), &field, view)?,
            FieldState::Missing => default_field_value(self.descriptor.full_name(), &field)?,
            FieldState::DecodedClean(_) | FieldState::DirtyOwned(_) => unreachable!(),
        };
        self.fields[idx].state = FieldState::DecodedClean(owned);
        Ok(())
    }

    fn ensure_dirty(&mut self, idx: usize) -> Result<(), MutableError> {
        self.ensure_decoded(idx)?;
        if matches!(self.fields[idx].state, FieldState::DirtyOwned(_)) {
            return Ok(());
        }
        if let FieldState::DecodedClean(value) =
            std::mem::replace(&mut self.fields[idx].state, FieldState::Missing)
        {
            self.fields[idx].state = FieldState::DirtyOwned(value);
        }
        Ok(())
    }

    fn is_effectively_empty(&self) -> bool {
        self.fields.iter().all(|field| match &field.state {
            FieldState::Missing => true,
            FieldState::Borrowed(_) => false,
            FieldState::DecodedClean(value) | FieldState::DirtyOwned(value) => {
                value.is_effectively_empty()
            }
        })
    }

    #[cfg(test)]
    fn field_state_name(&self, name: &str) -> Result<&'static str, MutableError> {
        let idx = self.field_index(name)?;
        Ok(match &self.fields[idx].state {
            FieldState::Missing => "missing",
            FieldState::Borrowed(_) => "borrowed untouched",
            FieldState::DecodedClean(_) => "decoded clean",
            FieldState::DirtyOwned(_) => "dirty owned",
        })
    }

    #[cfg(test)]
    fn decode_field_for_test(&mut self, name: &str) -> Result<(), MutableError> {
        let idx = self.field_index(name)?;
        self.ensure_decoded(idx)?;
        Ok(())
    }

    fn set_scalar_field(
        &mut self,
        name: &str,
        value: MutableValue<'a>,
        expected: &'static str,
        matches_kind: impl Fn(&Kind) -> bool,
    ) -> Result<(), MutableError> {
        let idx = self.field_index(name)?;
        let descriptor_name = self.descriptor.full_name().to_owned();
        let field_name = self.fields[idx].descriptor.name().to_owned();
        let field = self.fields[idx].descriptor.clone();
        if field.is_list() || field.is_map() || !matches_kind(&field.kind()) {
            return Err(MutableError::TypeMismatch {
                descriptor: descriptor_name,
                field: field_name,
                expected,
            });
        }
        self.fields[idx].state = FieldState::DirtyOwned(value);
        Ok(())
    }
}

pub fn serialize_protobuf_bytes(
    descriptor: MessageDescriptor,
    bytes: &[u8],
) -> Result<Vec<u32>, MutableError> {
    let message =
        DynamicMessage::decode(descriptor, bytes).map_err(|_| MutableError::InvalidRoot {
            descriptor: "<protobuf-bytes>".to_owned(),
        })?;
    MessageMut::from_dynamic_message(&message)?.serialize_words()
}

pub fn serialize_protobuf_bytes_into_buffer<'b>(
    descriptor: MessageDescriptor,
    bytes: &[u8],
    buffer: &'b mut Buffer,
) -> Result<&'b [u32], MutableError> {
    let message =
        DynamicMessage::decode(descriptor, bytes).map_err(|_| MutableError::InvalidRoot {
            descriptor: "<protobuf-bytes>".to_owned(),
        })?;
    MessageMut::from_dynamic_message(&message)?.serialize_into_buffer(buffer)
}

pub fn serialize_prost_message<M: Message>(
    message: &M,
    descriptor: MessageDescriptor,
) -> Result<Vec<u32>, MutableError> {
    let encoded = message.encode_to_vec();
    serialize_protobuf_bytes(descriptor, &encoded)
}

pub fn serialize_prost_message_into_buffer<'b, M: Message>(
    message: &M,
    descriptor: MessageDescriptor,
    buffer: &'b mut Buffer,
) -> Result<&'b [u32], MutableError> {
    let encoded = message.encode_to_vec();
    serialize_protobuf_bytes_into_buffer(descriptor, &encoded, buffer)
}

pub fn serialize_prost_message_from_proto_file<M: Message>(
    message: &M,
    schema_path: impl AsRef<Path>,
    root_message: &str,
) -> Result<Vec<u32>, MutableError> {
    let descriptor = load_message_descriptor(schema_path, root_message)?;
    serialize_prost_message(message, descriptor)
}

pub fn serialize_prost_message_from_proto_file_into_buffer<'b, M: Message>(
    message: &M,
    schema_path: impl AsRef<Path>,
    root_message: &str,
    buffer: &'b mut Buffer,
) -> Result<&'b [u32], MutableError> {
    let descriptor = load_message_descriptor(schema_path, root_message)?;
    serialize_prost_message_into_buffer(message, descriptor, buffer)
}

pub fn serialize_protobuf_bytes_from_proto_file(
    schema_path: impl AsRef<Path>,
    root_message: &str,
    bytes: &[u8],
) -> Result<Vec<u32>, MutableError> {
    let descriptor = load_message_descriptor(schema_path, root_message)?;
    serialize_protobuf_bytes(descriptor, bytes)
}

pub fn serialize_protobuf_bytes_from_proto_file_into_buffer<'b>(
    schema_path: impl AsRef<Path>,
    root_message: &str,
    bytes: &[u8],
    buffer: &'b mut Buffer,
) -> Result<&'b [u32], MutableError> {
    let descriptor = load_message_descriptor(schema_path, root_message)?;
    serialize_protobuf_bytes_into_buffer(descriptor, bytes, buffer)
}

fn load_message_descriptor(
    schema_path: impl AsRef<std::path::Path>,
    root_message: &str,
) -> Result<MessageDescriptor, MutableError> {
    let normalized = root_message.trim_start_matches('.');
    let cache_key = DescriptorCacheKey {
        schema_path: normalize_schema_path(schema_path.as_ref()),
        root_message: normalized.to_owned(),
    };

    if let Some(descriptor) = descriptor_cache().lock().unwrap().get(&cache_key).cloned() {
        return Ok(descriptor);
    }

    let pool = load_reflect_descriptor_pool_from_proto_file(&cache_key.schema_path)?;
    let descriptor = pool
        .get_message_by_name(normalized)
        .ok_or_else(|| MutableError::InvalidRoot {
            descriptor: normalized.to_owned(),
        })?;
    descriptor_cache()
        .lock()
        .unwrap()
        .insert(cache_key, descriptor.clone());
    Ok(descriptor)
}

impl<'a> FieldState<'a> {
    fn as_value_mut(&mut self) -> Option<&mut MutableValue<'a>> {
        match self {
            Self::DecodedClean(value) | Self::DirtyOwned(value) => Some(value),
            Self::Missing | Self::Borrowed(_) => None,
        }
    }
}

fn ensure_value_matches_kind(
    kind: &ValueKind,
    value: &MutableValue<'_>,
    descriptor: &str,
    expected: &'static str,
) -> Result<(), MutableError> {
    let ok = match (kind, value) {
        (ValueKind::Bool, MutableValue::Bool(_))
        | (ValueKind::I32, MutableValue::I32(_))
        | (ValueKind::U32, MutableValue::U32(_))
        | (ValueKind::I64, MutableValue::I64(_))
        | (ValueKind::U64, MutableValue::U64(_))
        | (ValueKind::F32, MutableValue::F32(_))
        | (ValueKind::F64, MutableValue::F64(_))
        | (ValueKind::String, MutableValue::String(_))
        | (ValueKind::Bytes, MutableValue::Bytes(_))
        | (ValueKind::Enum, MutableValue::EnumNumber(_))
        | (ValueKind::Message(_), MutableValue::Message(_)) => true,
        (ValueKind::Message(desc), MutableValue::List(_))
        | (ValueKind::Message(desc), MutableValue::Map(_)) => alias_field(desc).is_some(),
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(MutableError::TypeMismatch {
            descriptor: descriptor.to_owned(),
            field: expected.to_owned(),
            expected: "matching value",
        })
    }
}

fn ensure_key_kind(
    kind: &ValueKind,
    key: &MapKeyMut,
    descriptor: &str,
    field: &str,
) -> Result<(), MutableError> {
    let ok = matches!(
        (kind, key),
        (ValueKind::String, MapKeyMut::String(_))
            | (ValueKind::I32, MapKeyMut::I32(_))
            | (ValueKind::U32, MapKeyMut::U32(_))
            | (ValueKind::I64, MapKeyMut::I64(_))
            | (ValueKind::U64, MapKeyMut::U64(_))
    );
    if ok {
        Ok(())
    } else {
        Err(MutableError::InvalidMapKey {
            descriptor: descriptor.to_owned(),
            field: field.to_owned(),
            key: format!("{key:?}"),
        })
    }
}

fn default_field_value<'a>(
    descriptor_name: &str,
    field: &FieldDescriptor,
) -> Result<MutableValue<'a>, MutableError> {
    if let Kind::Message(desc) = field.kind() {
        if let Some(alias) = alias_field(&desc) {
            return default_field_value(descriptor_name, &alias);
        }
    }

    if field.is_map() {
        let entry = match field.kind() {
            Kind::Message(entry) => entry,
            _ => {
                return Err(MutableError::TypeMismatch {
                    descriptor: descriptor_name.to_owned(),
                    field: field.name().to_owned(),
                    expected: "map entry",
                });
            }
        };
        return Ok(MutableValue::Map(MapMut {
            descriptor_name: descriptor_name.to_owned(),
            field_name: field.name().to_owned(),
            key_kind: kind_to_value_kind(&entry.map_entry_key_field().kind()),
            value_kind: kind_to_value_kind(&entry.map_entry_value_field().kind()),
            entries: BTreeMap::new(),
        }));
    }

    if field.is_list() {
        return Ok(MutableValue::List(ListMut {
            item_kind: kind_to_value_kind(&field.kind()),
            items: Vec::new(),
        }));
    }

    Ok(MutableValue::default_for_kind(&kind_to_value_kind(&field.kind())))
}

fn default_alias_value<'a>(descriptor_name: &str, alias: &FieldDescriptor) -> MutableValue<'a> {
    if alias.is_map() {
        let entry = match alias.kind() {
            Kind::Message(entry) => entry,
            _ => unreachable!(),
        };
        MutableValue::Map(MapMut {
            descriptor_name: descriptor_name.to_owned(),
            field_name: alias.name().to_owned(),
            key_kind: kind_to_value_kind(&entry.map_entry_key_field().kind()),
            value_kind: kind_to_value_kind(&entry.map_entry_value_field().kind()),
            entries: BTreeMap::new(),
        })
    } else {
        MutableValue::List(ListMut {
            item_kind: kind_to_value_kind(&alias.kind()),
            items: Vec::new(),
        })
    }
}

fn decode_field_value<'a>(
    descriptor_name: &str,
    field: &FieldDescriptor,
    view: FieldView<'a>,
) -> Result<MutableValue<'a>, MutableError> {
    if field.is_map() {
        let map = view.map().ok_or_else(|| MutableError::TypeMismatch {
            descriptor: descriptor_name.to_owned(),
            field: field.name().to_owned(),
            expected: "map",
        })?;
        let entry = match field.kind() {
            Kind::Message(entry) => entry,
            _ => {
                return Err(MutableError::TypeMismatch {
                    descriptor: descriptor_name.to_owned(),
                    field: field.name().to_owned(),
                    expected: "map entry",
                });
            }
        };
        let key_field = entry.map_entry_key_field();
        let value_field = entry.map_entry_value_field();
        let mut entries = BTreeMap::new();
        for pair in map.iter() {
            let key = decode_map_key(descriptor_name, field.name(), &key_field.kind(), pair.key())?;
            let value =
                decode_kind_view(descriptor_name, field.name(), &value_field.kind(), pair.value())?;
            entries.insert(key, value);
        }
        return Ok(MutableValue::Map(MapMut {
            descriptor_name: descriptor_name.to_owned(),
            field_name: field.name().to_owned(),
            key_kind: kind_to_value_kind(&key_field.kind()),
            value_kind: kind_to_value_kind(&value_field.kind()),
            entries,
        }));
    }

    if field.is_list() {
        if matches!(field.kind(), Kind::Bool) {
            let bools = view
                .string()
                .ok_or_else(|| MutableError::TypeMismatch {
                    descriptor: descriptor_name.to_owned(),
                    field: field.name().to_owned(),
                    expected: "bool array",
                })?
                .as_bool_array();
            return Ok(MutableValue::List(ListMut {
                item_kind: ValueKind::Bool,
                items: bools.iter().map(MutableValue::Bool).collect(),
            }));
        }

        let array = view.array().ok_or_else(|| MutableError::TypeMismatch {
            descriptor: descriptor_name.to_owned(),
            field: field.name().to_owned(),
            expected: "array",
        })?;
        let mut items = Vec::with_capacity(array.len());
        for item in array.iter() {
            items.push(decode_kind_view(
                descriptor_name,
                field.name(),
                &field.kind(),
                item,
            )?);
        }
        return Ok(MutableValue::List(ListMut {
            item_kind: kind_to_value_kind(&field.kind()),
            items,
        }));
    }

    decode_kind_view(descriptor_name, field.name(), &field.kind(), view)
}

fn reflect_field_to_mutable_value(
    descriptor_name: &str,
    field: &FieldDescriptor,
    value: &ReflectValue,
) -> Result<MutableValue<'static>, MutableError> {
    if field.is_map() {
        let entry = match field.kind() {
            Kind::Message(entry) => entry,
            _ => return Err(type_mismatch(descriptor_name, field.name(), "map entry")),
        };
        let ReflectValue::Map(entries) = value else {
            return Err(type_mismatch(descriptor_name, field.name(), "map"));
        };
        let key_field = entry.map_entry_key_field();
        let value_field = entry.map_entry_value_field();
        let mut mapped = BTreeMap::new();
        for (key, value) in entries {
            mapped.insert(
                reflect_map_key_to_mutable(descriptor_name, field.name(), &key_field.kind(), key)?,
                reflect_kind_to_mutable_value(descriptor_name, field.name(), &value_field.kind(), value)?,
            );
        }
        return Ok(MutableValue::Map(MapMut {
            descriptor_name: descriptor_name.to_owned(),
            field_name: field.name().to_owned(),
            key_kind: kind_to_value_kind(&key_field.kind()),
            value_kind: kind_to_value_kind(&value_field.kind()),
            entries: mapped,
        }));
    }

    if field.is_list() {
        let ReflectValue::List(items) = value else {
            return Err(type_mismatch(descriptor_name, field.name(), "array"));
        };
        return Ok(MutableValue::List(ListMut {
            item_kind: kind_to_value_kind(&field.kind()),
            items: items
                .iter()
                .map(|item| reflect_kind_to_mutable_value(descriptor_name, field.name(), &field.kind(), item))
                .collect::<Result<Vec<_>, _>>()?,
        }));
    }

    reflect_kind_to_mutable_value(descriptor_name, field.name(), &field.kind(), value)
}

fn reflect_kind_to_mutable_value(
    descriptor_name: &str,
    field_name: &str,
    kind: &Kind,
    value: &ReflectValue,
) -> Result<MutableValue<'static>, MutableError> {
    match (kind, value) {
        (Kind::Bool, ReflectValue::Bool(value)) => Ok(MutableValue::Bool(*value)),
        (Kind::Int32 | Kind::Sint32 | Kind::Sfixed32, ReflectValue::I32(value)) => {
            Ok(MutableValue::I32(*value))
        }
        (Kind::Uint32 | Kind::Fixed32, ReflectValue::U32(value)) => Ok(MutableValue::U32(*value)),
        (Kind::Int64 | Kind::Sint64 | Kind::Sfixed64, ReflectValue::I64(value)) => {
            Ok(MutableValue::I64(*value))
        }
        (Kind::Uint64 | Kind::Fixed64, ReflectValue::U64(value)) => Ok(MutableValue::U64(*value)),
        (Kind::Float, ReflectValue::F32(value)) => Ok(MutableValue::F32(*value)),
        (Kind::Double, ReflectValue::F64(value)) => Ok(MutableValue::F64(*value)),
        (Kind::String, ReflectValue::String(value)) => Ok(MutableValue::String(value.clone())),
        (Kind::Bytes, ReflectValue::Bytes(value)) => Ok(MutableValue::Bytes(value.to_vec())),
        (Kind::Enum(_), ReflectValue::EnumNumber(value)) => Ok(MutableValue::EnumNumber(*value)),
        (Kind::Message(desc), ReflectValue::Message(message)) => {
            if let Some(alias) = alias_field(desc) {
                if message.has_field(&alias) {
                    return reflect_field_to_mutable_value(desc.full_name(), &alias, message.get_field(&alias).as_ref());
                }
                return Ok(default_alias_value(desc.full_name(), &alias));
            }
            MessageMut::from_dynamic_message(message)
                .map(|message| MutableValue::Message(Box::new(message)))
                .map_err(|_| type_mismatch(descriptor_name, field_name, "message"))
        }
        _ => Err(type_mismatch(descriptor_name, field_name, "matching value")),
    }
}

fn reflect_map_key_to_mutable(
    descriptor_name: &str,
    field_name: &str,
    kind: &Kind,
    key: &ReflectMapKey,
) -> Result<MapKeyMut, MutableError> {
    match (kind, key) {
        (Kind::String, ReflectMapKey::String(value)) => Ok(MapKeyMut::String(value.clone())),
        (Kind::Int32 | Kind::Sint32 | Kind::Sfixed32, ReflectMapKey::I32(value)) => Ok(MapKeyMut::I32(*value)),
        (Kind::Uint32 | Kind::Fixed32, ReflectMapKey::U32(value)) => Ok(MapKeyMut::U32(*value)),
        (Kind::Int64 | Kind::Sint64 | Kind::Sfixed64, ReflectMapKey::I64(value)) => Ok(MapKeyMut::I64(*value)),
        (Kind::Uint64 | Kind::Fixed64, ReflectMapKey::U64(value)) => Ok(MapKeyMut::U64(*value)),
        _ => Err(type_mismatch(descriptor_name, field_name, "matching map key")),
    }
}

fn decode_kind_view<'a>(
    descriptor_name: &str,
    field_name: &str,
    kind: &Kind,
    view: FieldView<'a>,
) -> Result<MutableValue<'a>, MutableError> {
    match kind {
        Kind::Bool => view
            .scalar::<bool>()
            .map(MutableValue::Bool)
            .ok_or_else(|| type_mismatch(descriptor_name, field_name, "bool")),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => view
            .scalar::<i32>()
            .map(MutableValue::I32)
            .ok_or_else(|| type_mismatch(descriptor_name, field_name, "int32")),
        Kind::Uint32 | Kind::Fixed32 => view
            .scalar::<u32>()
            .map(MutableValue::U32)
            .ok_or_else(|| type_mismatch(descriptor_name, field_name, "uint32")),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => view
            .scalar::<i64>()
            .map(MutableValue::I64)
            .ok_or_else(|| type_mismatch(descriptor_name, field_name, "int64")),
        Kind::Uint64 | Kind::Fixed64 => view
            .scalar::<u64>()
            .map(MutableValue::U64)
            .ok_or_else(|| type_mismatch(descriptor_name, field_name, "uint64")),
        Kind::Float => view
            .scalar::<f32>()
            .map(MutableValue::F32)
            .ok_or_else(|| type_mismatch(descriptor_name, field_name, "float")),
        Kind::Double => view
            .scalar::<f64>()
            .map(MutableValue::F64)
            .ok_or_else(|| type_mismatch(descriptor_name, field_name, "double")),
        Kind::String => view
            .string()
            .and_then(StringView::as_str)
            .map(|value| MutableValue::String(value.to_owned()))
            .ok_or_else(|| type_mismatch(descriptor_name, field_name, "string")),
        Kind::Bytes => view
            .string()
            .map(|value| MutableValue::Bytes(value.as_bytes().to_vec()))
            .ok_or_else(|| type_mismatch(descriptor_name, field_name, "bytes")),
        Kind::Enum(_) => view
            .scalar::<i32>()
            .map(MutableValue::EnumNumber)
            .ok_or_else(|| type_mismatch(descriptor_name, field_name, "enum")),
        Kind::Message(desc) => {
            if let Some(alias) = alias_field(desc) {
                let alias_words = view.object_words().ok_or_else(|| {
                    type_mismatch(descriptor_name, field_name, "alias message")
                })?;
                return decode_alias_words(desc.full_name(), &alias, alias_words);
            }
            let message = view.message().ok_or_else(|| type_mismatch(descriptor_name, field_name, "message"))?;
            Ok(MutableValue::Message(Box::new(
                MessageMut::from_words(desc.clone(), message.raw_words()).map_err(|_| {
                    MutableError::TypeMismatch {
                        descriptor: descriptor_name.to_owned(),
                        field: field_name.to_owned(),
                        expected: "message",
                    }
                })?,
            )))
        }
    }
}

fn decode_map_key(
    descriptor_name: &str,
    field_name: &str,
    kind: &Kind,
    view: FieldView<'_>,
) -> Result<MapKeyMut, MutableError> {
    match kind {
        Kind::String => view
            .string()
            .and_then(StringView::as_str)
            .map(|value| MapKeyMut::String(value.to_owned()))
            .ok_or_else(|| type_mismatch(descriptor_name, field_name, "string map key")),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => view
            .scalar::<i32>()
            .map(MapKeyMut::I32)
            .ok_or_else(|| type_mismatch(descriptor_name, field_name, "int32 map key")),
        Kind::Uint32 | Kind::Fixed32 => view
            .scalar::<u32>()
            .map(MapKeyMut::U32)
            .ok_or_else(|| type_mismatch(descriptor_name, field_name, "uint32 map key")),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => view
            .scalar::<i64>()
            .map(MapKeyMut::I64)
            .ok_or_else(|| type_mismatch(descriptor_name, field_name, "int64 map key")),
        Kind::Uint64 | Kind::Fixed64 => view
            .scalar::<u64>()
            .map(MapKeyMut::U64)
            .ok_or_else(|| type_mismatch(descriptor_name, field_name, "uint64 map key")),
        _ => Err(type_mismatch(descriptor_name, field_name, "supported map key")),
    }
}

fn kind_to_value_kind(kind: &Kind) -> ValueKind {
    match kind {
        Kind::Bool => ValueKind::Bool,
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => ValueKind::I32,
        Kind::Uint32 | Kind::Fixed32 => ValueKind::U32,
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => ValueKind::I64,
        Kind::Uint64 | Kind::Fixed64 => ValueKind::U64,
        Kind::Float => ValueKind::F32,
        Kind::Double => ValueKind::F64,
        Kind::String => ValueKind::String,
        Kind::Bytes => ValueKind::Bytes,
        Kind::Enum(_) => ValueKind::Enum,
        Kind::Message(desc) => ValueKind::Message(desc.clone()),
    }
}

fn borrow_field_unit(
    descriptor_name: &str,
    field_name: &str,
    field: &FieldDescriptor,
    view: FieldView<'_>,
    buffer: &mut Buffer,
) -> Result<Unit, MutableError> {
    let words = detect_borrowed_field_words(descriptor_name, field_name, field, view)?;
    if words.len() <= 3 {
        Ok(Unit::inline(words))
    } else {
        let last = buffer.len();
        buffer.expand(words.len()).copy_from_slice(words);
        Ok(Unit::segment(last, buffer.len()))
    }
}

fn detect_borrowed_field_words<'a>(
    descriptor_name: &str,
    field_name: &str,
    field: &FieldDescriptor,
    view: FieldView<'a>,
) -> Result<&'a [u32], MutableError> {
    if field.is_map() {
        return view
            .detect_map()
            .ok_or_else(|| type_mismatch(descriptor_name, field_name, "map"));
    }

    if field.is_list() {
        if matches!(field.kind(), Kind::Bool) {
            return view
                .detect_string()
                .ok_or_else(|| type_mismatch(descriptor_name, field_name, "bool array"));
        }
        return view
            .detect_array()
            .ok_or_else(|| type_mismatch(descriptor_name, field_name, "array"));
    }

    match field.kind() {
        Kind::String | Kind::Bytes => view
            .detect_string()
            .ok_or_else(|| type_mismatch(descriptor_name, field_name, "string")),
        Kind::Message(desc) => {
            if let Some(alias) = alias_field(&desc) {
                if alias.is_map() {
                    return view
                        .detect_map()
                        .ok_or_else(|| type_mismatch(descriptor_name, field_name, "alias map"));
                }
                if alias.is_list() && matches!(alias.kind(), Kind::Bool) {
                    return view
                        .detect_string()
                        .ok_or_else(|| type_mismatch(descriptor_name, field_name, "bool alias"));
                }
                return view
                    .detect_array()
                    .ok_or_else(|| type_mismatch(descriptor_name, field_name, "alias array"));
            }
            view.detect_message()
                .ok_or_else(|| type_mismatch(descriptor_name, field_name, "message"))
        }
        _ => view
            .detect_scalar()
            .ok_or_else(|| type_mismatch(descriptor_name, field_name, "scalar")),
    }
}

fn encode_owned_field(
    descriptor_name: &str,
    field_name: &str,
    field: &FieldDescriptor,
    value: &MutableValue<'_>,
    buffer: &mut Buffer,
) -> Result<Unit, MutableError> {
    if field.is_map() {
        return encode_map_contents(
            descriptor_name,
            field_name,
            expect_map_value(descriptor_name, field_name, value, "map")?,
            buffer,
        );
    }

    if field.is_list() {
        return encode_list_contents(
            descriptor_name,
            field_name,
            expect_list_value(descriptor_name, field_name, value, "array")?,
            "bool",
            buffer,
        );
    }

    if let Kind::Message(desc) = field.kind() {
        if alias_field(&desc).is_some() {
            return encode_alias_value(descriptor_name, field_name, value, buffer);
        }
    }

    encode_kind_value(
        descriptor_name,
        field_name,
        &kind_to_value_kind(&field.kind()),
        value,
        buffer,
    )
}

fn encode_alias_value(
    descriptor_name: &str,
    field_name: &str,
    value: &MutableValue<'_>,
    buffer: &mut Buffer,
) -> Result<Unit, MutableError> {
    match value {
        MutableValue::List(list) => encode_list_contents(
            descriptor_name,
            field_name,
            list,
            "bool alias element",
            buffer,
        ),
        MutableValue::Map(map) => encode_map_contents(descriptor_name, field_name, map, buffer),
        _ => Err(type_mismatch(descriptor_name, field_name, "alias value")),
    }
}

fn expect_list_value<'a, 'b>(
    descriptor_name: &str,
    field_name: &str,
    value: &'b MutableValue<'a>,
    expected: &'static str,
) -> Result<&'b ListMut<'a>, MutableError> {
    match value {
        MutableValue::List(list) => Ok(list),
        _ => Err(type_mismatch(descriptor_name, field_name, expected)),
    }
}

fn expect_map_value<'a, 'b>(
    descriptor_name: &str,
    field_name: &str,
    value: &'b MutableValue<'a>,
    expected: &'static str,
) -> Result<&'b MapMut<'a>, MutableError> {
    match value {
        MutableValue::Map(map) => Ok(map),
        _ => Err(type_mismatch(descriptor_name, field_name, expected)),
    }
}

fn encode_list_contents(
    descriptor_name: &str,
    field_name: &str,
    list: &ListMut<'_>,
    bool_expected: &'static str,
    buffer: &mut Buffer,
) -> Result<Unit, MutableError> {
    if list.is_empty() {
        return Ok(Unit::empty());
    }
    if matches!(list.item_kind, ValueKind::Bool) {
        let mut bytes = Vec::with_capacity(list.items.len());
        for item in &list.items {
            match item {
                MutableValue::Bool(value) => bytes.push(u8::from(*value)),
                _ => return Err(type_mismatch(descriptor_name, field_name, bool_expected)),
            }
        }
        return serialize_bytes(&bytes, buffer).ok_or_else(|| MutableError::SerializeFailed {
            descriptor: descriptor_name.to_owned(),
            field: field_name.to_owned(),
        });
    }

    let mut units = Vec::with_capacity(list.items.len());
    for item in &list.items {
        units.push(encode_kind_value(
            descriptor_name,
            field_name,
            &list.item_kind,
            item,
            buffer,
        )?);
    }
    serialize_array(&units, buffer).ok_or_else(|| MutableError::SerializeFailed {
        descriptor: descriptor_name.to_owned(),
        field: field_name.to_owned(),
    })
}

fn encode_map_contents(
    descriptor_name: &str,
    field_name: &str,
    map: &MapMut<'_>,
    buffer: &mut Buffer,
) -> Result<Unit, MutableError> {
    if map.is_empty() {
        return Ok(Unit::empty());
    }

    let ordered = map
        .entries
        .iter()
        .map(|(key, value)| {
            ensure_key_kind(&map.key_kind, key, descriptor_name, field_name)?;
            Ok((key, key.as_bytes(), value))
        })
        .collect::<Result<Vec<_>, MutableError>>()?;
    let keys_bytes = ordered
        .iter()
        .map(|(_, bytes, _)| bytes.as_ref())
        .collect::<Vec<_>>();
    let (index, positions) =
        build_perfect_hash_index_with_positions(&keys_bytes).ok_or_else(|| {
            MutableError::SerializeFailed {
                descriptor: descriptor_name.to_owned(),
                field: field_name.to_owned(),
            }
        })?;
    let mut keys = vec![Unit::empty(); ordered.len()];
    let mut values = vec![Unit::empty(); ordered.len()];
    for ((key, _, value), pos) in ordered.into_iter().zip(positions) {
        keys[pos] = encode_map_key(descriptor_name, field_name, &map.key_kind, key, buffer)?;
        values[pos] = encode_kind_value(
            descriptor_name,
            field_name,
            &map.value_kind,
            value,
            buffer,
        )?;
    }
    serialize_map(&index, &keys, &values, buffer).ok_or_else(|| MutableError::SerializeFailed {
        descriptor: descriptor_name.to_owned(),
        field: field_name.to_owned(),
    })
}

fn encode_kind_value(
    descriptor_name: &str,
    field_name: &str,
    kind: &ValueKind,
    value: &MutableValue<'_>,
    buffer: &mut Buffer,
) -> Result<Unit, MutableError> {
    if let ValueKind::Message(desc) = kind {
        if alias_field(desc).is_some() && matches!(value, MutableValue::List(_) | MutableValue::Map(_)) {
            return encode_alias_value(descriptor_name, field_name, value, buffer);
        }
    }
    match (kind, value) {
        (ValueKind::Bool, MutableValue::Bool(value)) => Ok(serialize_bool(*value)),
        (ValueKind::I32, MutableValue::I32(value)) => Ok(serialize_scalar::<i32>(*value)),
        (ValueKind::U32, MutableValue::U32(value)) => Ok(serialize_scalar::<u32>(*value)),
        (ValueKind::I64, MutableValue::I64(value)) => Ok(serialize_scalar::<i64>(*value)),
        (ValueKind::U64, MutableValue::U64(value)) => Ok(serialize_scalar::<u64>(*value)),
        (ValueKind::F32, MutableValue::F32(value)) => Ok(serialize_scalar::<f32>(*value)),
        (ValueKind::F64, MutableValue::F64(value)) => Ok(serialize_scalar::<f64>(*value)),
        (ValueKind::String, MutableValue::String(value)) => serialize_str(value, buffer).ok_or_else(|| {
            MutableError::SerializeFailed {
                descriptor: descriptor_name.to_owned(),
                field: field_name.to_owned(),
            }
        }),
        (ValueKind::Bytes, MutableValue::Bytes(value)) => serialize_bytes(value, buffer).ok_or_else(|| {
            MutableError::SerializeFailed {
                descriptor: descriptor_name.to_owned(),
                field: field_name.to_owned(),
            }
        }),
        (ValueKind::Enum, MutableValue::EnumNumber(value)) => Ok(serialize_scalar::<i32>(*value)),
        (ValueKind::Message(_), MutableValue::Message(message)) => {
            if message.is_effectively_empty() {
                Ok(Unit::empty())
            } else {
                message.encode(buffer)
            }
        }
        _ => Err(type_mismatch(descriptor_name, field_name, "matching value")),
    }
}

fn encode_map_key(
    descriptor_name: &str,
    field_name: &str,
    kind: &ValueKind,
    key: &MapKeyMut,
    buffer: &mut Buffer,
) -> Result<Unit, MutableError> {
    match (kind, key) {
        (ValueKind::String, MapKeyMut::String(value)) => serialize_str(value, buffer).ok_or_else(|| {
            MutableError::SerializeFailed {
                descriptor: descriptor_name.to_owned(),
                field: field_name.to_owned(),
            }
        }),
        (ValueKind::I32, MapKeyMut::I32(value)) => Ok(serialize_scalar::<i32>(*value)),
        (ValueKind::U32, MapKeyMut::U32(value)) => Ok(serialize_scalar::<u32>(*value)),
        (ValueKind::I64, MapKeyMut::I64(value)) => Ok(serialize_scalar::<i64>(*value)),
        (ValueKind::U64, MapKeyMut::U64(value)) => Ok(serialize_scalar::<u64>(*value)),
        _ => Err(type_mismatch(descriptor_name, field_name, "matching map key")),
    }
}

fn type_mismatch(descriptor: &str, field: &str, expected: &'static str) -> MutableError {
    MutableError::TypeMismatch {
        descriptor: descriptor.to_owned(),
        field: field.to_owned(),
        expected,
    }
}

fn alias_field(descriptor: &MessageDescriptor) -> Option<FieldDescriptor> {
    let mut fields = descriptor.fields();
    let field = fields.next()?;
    if fields.next().is_none() && field.name() == "_" && (field.is_list() || field.is_map()) {
        Some(field)
    } else {
        None
    }
}

fn decode_alias_words<'a>(
    descriptor_name: &str,
    alias: &FieldDescriptor,
    words: &'a [u32],
) -> Result<MutableValue<'a>, MutableError> {
    if alias.is_map() {
        let map = MapView::new(words).ok_or_else(|| MutableError::TypeMismatch {
            descriptor: descriptor_name.to_owned(),
            field: alias.name().to_owned(),
            expected: "alias map",
        })?;
        let entry = match alias.kind() {
            Kind::Message(entry) => entry,
            _ => {
                return Err(MutableError::TypeMismatch {
                    descriptor: descriptor_name.to_owned(),
                    field: alias.name().to_owned(),
                    expected: "map entry",
                });
            }
        };
        let key_field = entry.map_entry_key_field();
        let value_field = entry.map_entry_value_field();
        let mut entries = BTreeMap::new();
        for pair in map.iter() {
            let key = decode_map_key(descriptor_name, alias.name(), &key_field.kind(), pair.key())?;
            let value =
                decode_kind_view(descriptor_name, alias.name(), &value_field.kind(), pair.value())?;
            entries.insert(key, value);
        }
        Ok(MutableValue::Map(MapMut {
            descriptor_name: descriptor_name.to_owned(),
            field_name: alias.name().to_owned(),
            key_kind: kind_to_value_kind(&key_field.kind()),
            value_kind: kind_to_value_kind(&value_field.kind()),
            entries,
        }))
    } else if alias.is_list() {
        if matches!(alias.kind(), Kind::Bool) {
            let bools = StringView::new(words)
                .ok_or_else(|| MutableError::TypeMismatch {
                    descriptor: descriptor_name.to_owned(),
                    field: alias.name().to_owned(),
                    expected: "bool alias array",
                })?
                .as_bool_array();
            Ok(MutableValue::List(ListMut {
                item_kind: ValueKind::Bool,
                items: bools.iter().map(MutableValue::Bool).collect(),
            }))
        } else {
            let array = ArrayView::new(words).ok_or_else(|| MutableError::TypeMismatch {
                descriptor: descriptor_name.to_owned(),
                field: alias.name().to_owned(),
                expected: "alias array",
            })?;
            let mut items = Vec::with_capacity(array.len());
            for item in array.iter() {
                items.push(decode_kind_view(
                    descriptor_name,
                    alias.name(),
                    &alias.kind(),
                    item,
                )?);
            }
            Ok(MutableValue::List(ListMut {
                item_kind: kind_to_value_kind(&alias.kind()),
                items,
            }))
        }
    } else {
        Err(MutableError::TypeMismatch {
            descriptor: descriptor_name.to_owned(),
            field: alias.name().to_owned(),
            expected: "alias container",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost_reflect::{DynamicMessage, Value as ReflectValue};
    use protocache_core::{ArrayView, GeneratedDescriptor, MessageView, StringView, ViewArray};
    use std::collections::HashMap;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct GeneratedMain;

    impl GeneratedDescriptor for GeneratedMain {
        const FULL_NAME: &'static str = ".test.Main";
    }

    fn fixture_schema() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/proto/test.proto")
    }

    fn fixture_words() -> Vec<u32> {
        let root_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures");
        let bytes = std::fs::read(root_dir.join("benchmark/test.pc")).unwrap();
        bytes
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap()))
            .collect::<Vec<_>>()
    }

    fn unique_temp_schema_path(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("pcrs-mutable-{name}-{}-{nanos}.proto", std::process::id()))
    }

    #[test]
    fn empty_root_serializes_to_tiny_message() {
        let schema = fixture_schema();
        let pool = load_reflect_descriptor_pool_from_proto_file(&schema).unwrap();
        let descriptor = pool.get_message_by_name("test.Main").unwrap();

        let root = MessageMut::new_empty(descriptor);
        let words = root.serialize_words().unwrap();
        assert_eq!(words.len(), 1);
    }

    #[test]
    fn read_only_protobuf_message_entry_points_serialize_without_manual_reflection_step() {
        let schema = fixture_schema();
        let pool = load_reflect_descriptor_pool_from_proto_file(&schema).unwrap();
        let descriptor = pool.get_message_by_name("test.Main").unwrap();
        let small_descriptor = pool.get_message_by_name("test.Small").unwrap();

        let mut object = DynamicMessage::new(small_descriptor);
        object.set_field_by_name("i32", ReflectValue::I32(7));
        object.set_field_by_name("flag", ReflectValue::Bool(true));
        object.set_field_by_name("str", ReflectValue::String("nested".to_owned()));

        let mut root = DynamicMessage::new(descriptor.clone());
        root.set_field_by_name("i32", ReflectValue::I32(42));
        root.set_field_by_name("str", ReflectValue::String("hello".to_owned()));
        root.set_field_by_name("object", ReflectValue::Message(object));
        root.set_field_by_name(
            "i32v",
            ReflectValue::List(vec![ReflectValue::I32(1), ReflectValue::I32(2), ReflectValue::I32(3)]),
        );
        root.set_field_by_name(
            "index",
            ReflectValue::Map(HashMap::from([(ReflectMapKey::String("k".to_owned()), ReflectValue::I32(9))])),
        );

        let words = serialize_prost_message(&root, descriptor).unwrap();
        let view = MessageView::new(&words).unwrap();
        assert_eq!(view.scalar::<i32>(0), Some(42));
        assert_eq!(view.string(6).unwrap().as_str(), Some("hello"));
        assert_eq!(view.array(11).unwrap().len(), 3);
        assert_eq!(view.map(25).unwrap().len(), 1);
        let object = view.message(10).unwrap();
        assert_eq!(object.scalar::<i32>(0), Some(7));
        assert_eq!(object.scalar::<bool>(1), Some(true));
        assert_eq!(object.string(3).unwrap().as_str(), Some("nested"));
    }

    #[test]
    fn protobuf_bytes_and_prost_message_entry_points_serialize_without_manual_dynamic_step() {
        let schema = fixture_schema();
        let pool = load_reflect_descriptor_pool_from_proto_file(&schema).unwrap();
        let descriptor = pool.get_message_by_name("test.Main").unwrap();

        let mut root = DynamicMessage::new(descriptor.clone());
        root.set_field_by_name("i32", ReflectValue::I32(123));
        root.set_field_by_name("str", ReflectValue::String("hi".to_owned()));
        let bytes = root.encode_to_vec();

        let from_bytes = serialize_protobuf_bytes(descriptor.clone(), &bytes).unwrap();
        let from_message = serialize_prost_message(&root, descriptor).unwrap();

        let bytes_view = MessageView::new(&from_bytes).unwrap();
        let message_view = MessageView::new(&from_message).unwrap();
        assert_eq!(bytes_view.scalar::<i32>(0), Some(123));
        assert_eq!(message_view.scalar::<i32>(0), Some(123));
        assert_eq!(bytes_view.string(6).unwrap().as_str(), Some("hi"));
        assert_eq!(message_view.string(6).unwrap().as_str(), Some("hi"));
    }

    #[test]
    fn generated_type_entry_points_resolve_message_descriptor() {
        let schema = fixture_schema();
        let words = fixture_words();

        let root = MessageMut::from_generated_type::<GeneratedMain>(&schema, &words).unwrap();
        let roundtrip = root.serialize_words().unwrap();
        let view = MessageView::new(&roundtrip).unwrap();
        assert_eq!(view.scalar::<i32>(0), Some(-999));

        let empty = MessageMut::new_empty_from_generated_type::<GeneratedMain>(&schema).unwrap();
        assert_eq!(empty.serialize_words().unwrap().len(), 1);
    }

    #[test]
    fn generated_mutable_trait_provides_stable_type_entry_points() {
        let schema = fixture_schema();
        let words = fixture_words();

        let mut root = GeneratedMain::open_mut(&schema, &words).unwrap();
        root.set_i32("i32", 77).unwrap();
        let roundtrip = root.serialize_words().unwrap();
        let view = MessageView::new(&roundtrip).unwrap();
        assert_eq!(view.scalar::<i32>(0), Some(77));

        let empty = GeneratedMain::new_mut(&schema).unwrap();
        assert_eq!(empty.serialize_words().unwrap().len(), 1);
    }

    #[test]
    fn serialize_into_buffer_matches_owned_serialization() {
        let schema = fixture_schema();
        let words = fixture_words();
        let root = MessageMut::from_proto_file(&schema, "test.Main", &words).unwrap();

        let mut buffer = Buffer::new();
        let borrowed = root.serialize_into_buffer(&mut buffer).unwrap().to_vec();
        let owned = root.serialize_words().unwrap();

        assert_eq!(borrowed, owned);
    }

    #[test]
    fn descriptor_lookup_uses_cached_schema_after_first_load() {
        let schema = unique_temp_schema_path("cache");
        fs::write(
            &schema,
            r#"
            syntax = "proto3";
            package demo;
            message Root {
                int32 value = 1;
            }
            "#,
        )
        .unwrap();

        let words = vec![0u32];
        let first = MessageMut::from_proto_file(&schema, "demo.Root", &words).unwrap();
        assert_eq!(first.serialize_words().unwrap().len(), 1);

        fs::remove_file(&schema).unwrap();

        let second = MessageMut::from_proto_file(&schema, "demo.Root", &words).unwrap();
        assert_eq!(second.serialize_words().unwrap().len(), 1);
    }

    #[test]
    fn mutates_fixture_and_roundtrips() {
        let root_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures");
        let schema = root_dir.join("proto/test.proto");
        let words = fixture_words();

        let mut root = MessageMut::from_proto_file(&schema, "test.Main", &words).unwrap();
        root.field_list_mut("strv").unwrap().set_string(0, "xyz").unwrap();

        let row = root
            .field_alias_list_mut("matrix")
            .unwrap()
            .get_list_mut(1)
            .unwrap();
        row.items[1] = MutableValue::F32(999.0);

        root.field_alias_map_mut("arrays")
            .unwrap()
            .get_list_mut_str("lv5")
            .unwrap()
            .push_f32(53.0)
            .unwrap();

        let inserted = root
            .field_alias_map_mut("arrays")
            .unwrap()
            .get_or_insert_list_str("lv9")
            .unwrap();
        inserted.push_f32(91.0).unwrap();
        inserted.push_f32(92.0).unwrap();

        root.field_map_mut("objects")
            .unwrap()
            .get_message_mut_i32(1)
            .unwrap()
            .set_i32("i32", 2)
            .unwrap();
        root.field_map_mut("objects")
            .unwrap()
            .get_message_mut_i32(2)
            .unwrap()
            .set_i32("i32", 3)
            .unwrap();

        let modified = root.serialize_words().unwrap();
        let root_view = MessageView::new(&modified).unwrap();

        assert_eq!(root_view.string(6).unwrap().as_str(), Some("Hello World!"));

        let strv = ViewArray::<StringView<'_>>::new(root_view.array(13).unwrap());
        assert_eq!(strv.get(0).unwrap().as_str(), Some("xyz"));
        assert_eq!(strv.get(1).unwrap().as_str(), Some("apple"));

        let matrix = ArrayView::new(root_view.field(27).unwrap().object_words().unwrap()).unwrap();
        let row1 = ArrayView::new(matrix.field(1).unwrap().object_words().unwrap()).unwrap();
        assert_eq!(row1.scalars::<f32>().unwrap().get(1), Some(999.0));

        let arrays = protocache_core::MapView::new(root_view.field(29).unwrap().object_words().unwrap()).unwrap();
        let lv5 = ArrayView::new(arrays.find_str("lv5").unwrap().value().object_words().unwrap()).unwrap();
        assert_eq!(lv5.scalars::<f32>().unwrap().get(2), Some(53.0));
        let lv9 = ArrayView::new(arrays.find_str("lv9").unwrap().value().object_words().unwrap()).unwrap();
        assert_eq!(lv9.scalars::<f32>().unwrap().get(0), Some(91.0));
        assert_eq!(lv9.scalars::<f32>().unwrap().get(1), Some(92.0));

        let objects = root_view.map(26).unwrap();
        assert_eq!(
            objects.find_scalar(1i32).unwrap().value().message().unwrap().scalar::<i32>(0),
            Some(2)
        );
        assert_eq!(
            objects.find_scalar(2i32).unwrap().value().message().unwrap().scalar::<i32>(0),
            Some(3)
        );
    }

    #[test]
    fn untouched_fixture_roundtrips_semantically() {
        let schema = fixture_schema();
        let pool = load_reflect_descriptor_pool_from_proto_file(&schema).unwrap();
        let descriptor = pool.get_message_by_name("test.Main").unwrap();
        let words = fixture_words();

        let root = MessageMut::from_words(descriptor, &words).unwrap();
        let roundtrip = root.serialize_words().unwrap();
        let view = MessageView::new(&roundtrip).unwrap();

        assert_eq!(view.scalar::<i32>(0), Some(-999));
        assert_eq!(view.scalar::<u32>(1), Some(1234));
        assert_eq!(view.string(6).unwrap().as_str(), Some("Hello World!"));
        assert_eq!(view.array(13).unwrap().len(), 10);
        assert_eq!(view.map(25).unwrap().len(), 6);
        assert_eq!(view.map(26).unwrap().len(), 4);
    }

    #[test]
    fn borrowed_fields_reuse_original_field_words() {
        let schema = fixture_schema();
        let pool = load_reflect_descriptor_pool_from_proto_file(&schema).unwrap();
        let descriptor = pool.get_message_by_name("test.Main").unwrap();
        let words = fixture_words();
        let root_view = MessageView::new(&words).unwrap();

        for field in descriptor.fields() {
            let Some(field_view) = root_view.field(field.number() as usize - 1) else {
                continue;
            };
            let borrowed = detect_borrowed_field_words(
                descriptor.full_name(),
                field.name(),
                &field,
                field_view,
            )
            .unwrap();
            let expected = if field.is_map() {
                field_view.detect_map().unwrap()
            } else if field.is_list() {
                if matches!(field.kind(), Kind::Bool) {
                    field_view.detect_string().unwrap()
                } else {
                    field_view.detect_array().unwrap()
                }
            } else {
                match field.kind() {
                    Kind::String | Kind::Bytes => field_view.detect_string().unwrap(),
                    Kind::Message(desc) => {
                        if let Some(alias) = alias_field(&desc) {
                            if alias.is_map() {
                                field_view.detect_map().unwrap()
                            } else if alias.is_list() && matches!(alias.kind(), Kind::Bool) {
                                field_view.detect_string().unwrap()
                            } else {
                                field_view.detect_array().unwrap()
                            }
                        } else {
                            field_view.detect_message().unwrap()
                        }
                    }
                    _ => field_view.detect_scalar().unwrap(),
                }
            };
            assert_eq!(borrowed, expected, "field {}", field.name());
        }
    }

    #[test]
    fn decoded_clean_fields_roundtrip_stably() {
        let schema = fixture_schema();
        let pool = load_reflect_descriptor_pool_from_proto_file(&schema).unwrap();
        let descriptor = pool.get_message_by_name("test.Main").unwrap();
        let words = fixture_words();

        let mut root = MessageMut::from_words(descriptor, &words).unwrap();
        assert_eq!(root.field_state_name("strv").unwrap(), "borrowed untouched");
        assert_eq!(root.field_state_name("matrix").unwrap(), "borrowed untouched");
        assert_eq!(root.field_state_name("arrays").unwrap(), "borrowed untouched");
        assert_eq!(root.field_state_name("objects").unwrap(), "borrowed untouched");

        root.decode_field_for_test("strv").unwrap();
        root.decode_field_for_test("matrix").unwrap();
        root.decode_field_for_test("arrays").unwrap();
        root.decode_field_for_test("objects").unwrap();

        assert_eq!(root.field_state_name("strv").unwrap(), "decoded clean");
        assert_eq!(root.field_state_name("matrix").unwrap(), "decoded clean");
        assert_eq!(root.field_state_name("arrays").unwrap(), "decoded clean");
        assert_eq!(root.field_state_name("objects").unwrap(), "decoded clean");

        let roundtrip = root.serialize_words().unwrap();
        let roundtrip_view = MessageView::new(&roundtrip).unwrap();

        assert_eq!(roundtrip_view.scalar::<i32>(0), Some(-999));
        assert_eq!(roundtrip_view.scalar::<u32>(1), Some(1234));
        assert_eq!(roundtrip_view.string(6).unwrap().as_str(), Some("Hello World!"));
        assert_eq!(roundtrip_view.array(13).unwrap().len(), 10);
        assert_eq!(roundtrip_view.map(25).unwrap().len(), 6);
        assert!(roundtrip_view.field(27).is_some());
        assert!(roundtrip_view.field(29).is_some());
    }

    #[test]
    fn mutable_access_marks_fields_dirty_owned() {
        let schema = fixture_schema();
        let pool = load_reflect_descriptor_pool_from_proto_file(&schema).unwrap();
        let descriptor = pool.get_message_by_name("test.Main").unwrap();
        let words = fixture_words();

        let mut root = MessageMut::from_words(descriptor, &words).unwrap();
        root.field_list_mut("strv").unwrap();
        root.field_alias_list_mut("matrix").unwrap();
        root.field_alias_map_mut("arrays").unwrap();
        root.field_map_mut("objects").unwrap();

        assert_eq!(root.field_state_name("strv").unwrap(), "dirty owned");
        assert_eq!(root.field_state_name("matrix").unwrap(), "dirty owned");
        assert_eq!(root.field_state_name("arrays").unwrap(), "dirty owned");
        assert_eq!(root.field_state_name("objects").unwrap(), "dirty owned");
    }

    #[test]
    fn tiny_message_stays_tiny_after_scalar_mutation() {
        let schema = fixture_schema();
        let words = vec![0x0000_0100u32, 123u32];

        let pool = load_reflect_descriptor_pool_from_proto_file(&schema).unwrap();
        let descriptor = pool.get_message_by_name("test.Main").unwrap();
        let mut root = MessageMut::from_words(descriptor, &words).unwrap();
        root.set_i32("i32", 456).unwrap();

        let modified = root.serialize_words().unwrap();
        let root_view = MessageView::new(&modified).unwrap();

        assert_eq!(modified.len(), 2);
        assert_eq!(root_view.scalar::<i32>(0), Some(456));
        assert!(root_view.field(6).is_none());
        assert!(root_view.field(27).is_none());
    }

    #[test]
    fn empty_root_can_materialize_alias_map() {
        let schema = fixture_schema();
        let pool = load_reflect_descriptor_pool_from_proto_file(&schema).unwrap();
        let descriptor = pool.get_message_by_name("test.Main").unwrap();

        let mut root = MessageMut::new_empty(descriptor);
        let values = root
            .field_alias_map_mut("arrays")
            .unwrap()
            .get_or_insert_list_str("lv1")
            .unwrap();
        values.push_f32(11.0).unwrap();
        values.push_f32(12.0).unwrap();

        let modified = root.serialize_words().unwrap();
        let root_view = MessageView::new(&modified).unwrap();
        let arrays = protocache_core::MapView::new(root_view.field(29).unwrap().object_words().unwrap()).unwrap();
        let lv1 = ArrayView::new(arrays.find_str("lv1").unwrap().value().object_words().unwrap()).unwrap();

        assert_eq!(lv1.scalars::<f32>().unwrap().get(0), Some(11.0));
        assert_eq!(lv1.scalars::<f32>().unwrap().get(1), Some(12.0));
    }

    #[test]
    fn empty_root_can_materialize_message_field() {
        let schema = fixture_schema();
        let pool = load_reflect_descriptor_pool_from_proto_file(&schema).unwrap();
        let descriptor = pool.get_message_by_name("test.Main").unwrap();

        let mut root = MessageMut::new_empty(descriptor);
        let object = root.field_message_mut("object").unwrap();
        object.set_i32("i32", 88).unwrap();

        let modified = root.serialize_words().unwrap();
        let root_view = MessageView::new(&modified).unwrap();
        let object_view = root_view.message(10).unwrap();

        assert_eq!(object_view.scalar::<i32>(0), Some(88));
        assert!(root_view.field(6).is_none());
        assert!(root_view.field(29).is_none());
    }

    #[test]
    fn scalar_field_setters_cover_non_i32_types() {
        let schema = fixture_schema();
        let pool = load_reflect_descriptor_pool_from_proto_file(&schema).unwrap();
        let descriptor = pool.get_message_by_name("test.Main").unwrap();

        let mut root = MessageMut::new_empty(descriptor);
        root.set_u32("u32", 1234).unwrap();
        root.set_i64("i64", -9876543210).unwrap();
        root.set_u64("u64", 98765432123456789).unwrap();
        root.set_bool("flag", true).unwrap();
        root.set_enum_number("mode", 2).unwrap();
        root.set_string("str", "hello").unwrap();
        root.set_bytes("data", b"abc".to_vec()).unwrap();
        root.set_f32("f32", 1.25).unwrap();
        root.set_f64("f64", 9.5).unwrap();

        let words = root.serialize_words().unwrap();
        let view = MessageView::new(&words).unwrap();
        assert_eq!(view.scalar::<u32>(1), Some(1234));
        assert_eq!(view.scalar::<i64>(2), Some(-9876543210));
        assert_eq!(view.scalar::<u64>(3), Some(98765432123456789));
        assert_eq!(view.scalar::<bool>(4), Some(true));
        assert_eq!(view.scalar::<i32>(5), Some(2));
        assert_eq!(view.string(6).unwrap().as_str(), Some("hello"));
        assert_eq!(view.bytes(7), Some(&b"abc"[..]));
        assert_eq!(view.scalar::<f32>(8), Some(1.25));
        assert_eq!(view.scalar::<f64>(9), Some(9.5));
    }

    #[test]
    fn list_and_map_scalar_mutation_cover_more_types() {
        let schema = fixture_schema();
        let words = fixture_words();
        let mut root = MessageMut::from_proto_file(&schema, "test.Main", &words).unwrap();

        root.field_list_mut("u64v").unwrap().set_u64(0, 77).unwrap();
        root.field_list_mut("flags").unwrap().set_bool(0, false).unwrap();
        root.field_list_mut("flags").unwrap().push_bool(true).unwrap();
        root.field_list_mut("datav").unwrap().push_bytes(b"xyz".to_vec()).unwrap();
        root.field_list_mut("modev").unwrap().push_enum_number(1).unwrap();
        root.field_map_mut("index").unwrap().set_i32_str("abc-9", 9).unwrap();

        let words = root.serialize_words().unwrap();
        let view = MessageView::new(&words).unwrap();

        assert_eq!(view.array(12).unwrap().scalars::<u64>().unwrap().get(0), Some(77));
        let flags = view.string(17).unwrap().as_bool_array();
        assert_eq!(flags.get(0), Some(false));
        assert_eq!(flags.get(flags.len() - 1), Some(true));
        let datav = ViewArray::<StringView<'_>>::new(view.array(14).unwrap());
        assert_eq!(datav.get(0).unwrap().as_bytes(), b"xyz");
        let modev = view.array(31).unwrap().scalars::<i32>().unwrap();
        assert_eq!(modev.get(0), Some(1));
        let index = protocache_core::MapView::new(view.field(25).unwrap().object_words().unwrap()).unwrap();
        assert_eq!(index.find_str("abc-9").unwrap().value().scalar::<i32>(), Some(9));
    }

    #[test]
    fn map_mut_supports_all_supported_scalar_key_types() {
        let schema = fixture_schema();
        let pool = load_reflect_descriptor_pool_from_proto_file(&schema).unwrap();
        let descriptor = pool.get_message_by_name("test.Main").unwrap();

        let mut u32_map = MapMut {
            descriptor_name: "test.MapU32".to_owned(),
            field_name: "m".to_owned(),
            key_kind: ValueKind::U32,
            value_kind: ValueKind::I32,
            entries: BTreeMap::new(),
        };
        u32_map.set_i32_u32(7, 70).unwrap();
        assert_eq!(u32_map.get_map_entry_mut(MapKeyMut::U32(7), "7".to_owned()).unwrap().as_i32_for_test(), Some(70));
        assert!(u32_map.remove_u32(7));

        let mut i64_map = MapMut {
            descriptor_name: "test.MapI64".to_owned(),
            field_name: "m".to_owned(),
            key_kind: ValueKind::I64,
            value_kind: ValueKind::I32,
            entries: BTreeMap::new(),
        };
        i64_map.set_i32_i64(-9, 90).unwrap();
        assert_eq!(i64_map.get_map_entry_mut(MapKeyMut::I64(-9), "-9".to_owned()).unwrap().as_i32_for_test(), Some(90));
        assert!(i64_map.remove_i64(-9));

        let mut u64_map = MapMut {
            descriptor_name: "test.MapU64".to_owned(),
            field_name: "m".to_owned(),
            key_kind: ValueKind::U64,
            value_kind: ValueKind::Message(descriptor.clone()),
            entries: BTreeMap::new(),
        };
        u64_map.get_or_insert_message_u64(99).unwrap().set_i32("i32", 9).unwrap();
        assert_eq!(
            u64_map.get_message_mut_u64(99).unwrap().serialize_words().unwrap().len() > 1,
            true
        );
        assert!(u64_map.remove_u64(99));
    }

    #[test]
    fn touching_missing_fields_without_writes_keeps_root_tiny() {
        let schema = fixture_schema();
        let pool = load_reflect_descriptor_pool_from_proto_file(&schema).unwrap();
        let descriptor = pool.get_message_by_name("test.Main").unwrap();

        let mut root = MessageMut::new_empty(descriptor);
        assert!(root.field_message_mut("object").unwrap().is_effectively_empty());
        assert!(root.field_list_mut("strv").unwrap().is_empty());
        assert!(root.field_alias_list_mut("matrix").unwrap().is_empty());
        assert!(root.field_alias_map_mut("arrays").unwrap().is_empty());
        assert!(root.field_map_mut("objects").unwrap().is_empty());

        let modified = root.serialize_words().unwrap();
        let root_view = MessageView::new(&modified).unwrap();

        assert_eq!(modified.len(), 1);
        assert!(root_view.field(10).is_none());
        assert!(root_view.field(13).is_none());
        assert!(root_view.field(27).is_none());
        assert!(root_view.field(29).is_none());
        assert!(root_view.field(26).is_none());
    }

    #[test]
    fn alias_map_handles_many_long_string_keys() {
        let schema = fixture_schema();
        let pool = load_reflect_descriptor_pool_from_proto_file(&schema).unwrap();
        let descriptor = pool.get_message_by_name("test.Main").unwrap();

        let mut root = MessageMut::new_empty(descriptor);
        for i in 0..64 {
            let key = format!("very-long-key-{i:03}-abcdefghijklmnopqrstuvwxyz");
            let values = root
                .field_alias_map_mut("arrays")
                .unwrap()
                .get_or_insert_list_str(&key)
                .unwrap();
            values.push_f32(i as f32).unwrap();
            values.push_f32(i as f32 + 0.5).unwrap();
        }

        let modified = root.serialize_words().unwrap();
        let root_view = MessageView::new(&modified).unwrap();
        let arrays = protocache_core::MapView::new(root_view.field(29).unwrap().object_words().unwrap()).unwrap();

        for i in 0..64 {
            let key = format!("very-long-key-{i:03}-abcdefghijklmnopqrstuvwxyz");
            let entry = arrays.find_str(&key).unwrap();
            let values = ArrayView::new(entry.value().object_words().unwrap()).unwrap();
            assert_eq!(values.scalars::<f32>().unwrap().get(0), Some(i as f32));
            assert_eq!(values.scalars::<f32>().unwrap().get(1), Some(i as f32 + 0.5));
        }
    }
}

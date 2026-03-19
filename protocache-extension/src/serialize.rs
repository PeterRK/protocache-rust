use prost::Message;
use prost_reflect::{
    DynamicMessage, FieldDescriptor, Kind, MapKey as ReflectMapKey, MessageDescriptor,
    ReflectMessage, Value as ReflectValue,
};
use protocache_core::{
    Buffer, MutableError, Unit, build_perfect_hash_index_with_positions, fold_field,
    serialize_array_at, serialize_bool, serialize_bytes, serialize_map_at, serialize_message_at,
    serialize_scalar, serialize_str,
};

pub(crate) fn serialize_protobuf_bytes(
    descriptor: MessageDescriptor,
    bytes: &[u8],
) -> Result<Vec<u32>, MutableError> {
    let mut buffer = Buffer::new();
    let _ = serialize_protobuf_bytes_into_buffer(descriptor, bytes, &mut buffer)?;
    Ok(buffer.view().to_vec())
}

pub(crate) fn serialize_protobuf_bytes_into_buffer<'b>(
    descriptor: MessageDescriptor,
    bytes: &[u8],
    buffer: &'b mut Buffer,
) -> Result<&'b [u32], MutableError> {
    let message =
        DynamicMessage::decode(descriptor, bytes).map_err(|_| MutableError::InvalidRoot {
            descriptor: "<protobuf-bytes>".to_owned(),
        })?;
    encode_dynamic_message_into_buffer(&message, buffer)
}

pub(crate) fn serialize_dynamic_message(message: &DynamicMessage) -> Result<Vec<u32>, MutableError> {
    let mut buffer = Buffer::new();
    let _ = serialize_dynamic_message_into_buffer(message, &mut buffer)?;
    Ok(buffer.view().to_vec())
}

pub(crate) fn serialize_dynamic_message_into_buffer<'b>(
    message: &DynamicMessage,
    buffer: &'b mut Buffer,
) -> Result<&'b [u32], MutableError> {
    encode_dynamic_message_into_buffer(message, buffer)
}

fn encode_dynamic_message_into_buffer<'b>(
    message: &DynamicMessage,
    buffer: &'b mut Buffer,
) -> Result<&'b [u32], MutableError> {
    buffer.clear();
    let unit = encode_dynamic_message(message, buffer)?;
    if !unit.is_segment() {
        buffer.clear();
        buffer.put_words(unit.inline_words());
    }
    Ok(buffer.view())
}

pub(crate) fn serialize_prost_message<M: Message>(
    message: &M,
    descriptor: MessageDescriptor,
) -> Result<Vec<u32>, MutableError> {
    let encoded = message.encode_to_vec();
    serialize_protobuf_bytes(descriptor, &encoded)
}

pub(crate) fn serialize_prost_message_into_buffer<'b, M: Message>(
    message: &M,
    descriptor: MessageDescriptor,
    buffer: &'b mut Buffer,
) -> Result<&'b [u32], MutableError> {
    let encoded = message.encode_to_vec();
    serialize_protobuf_bytes_into_buffer(descriptor, &encoded, buffer)
}

fn encode_dynamic_message(
    message: &DynamicMessage,
    buffer: &mut Buffer,
) -> Result<Unit, MutableError> {
    let descriptor = message.descriptor();
    if let Some(alias) = alias_field(&descriptor) {
        return encode_field_value(
            descriptor.full_name(),
            alias.name(),
            &alias,
            message.get_field(&alias).as_ref(),
            buffer,
        );
    }

    let max_number = descriptor
        .fields()
        .map(|field| field.number() as usize)
        .max()
        .unwrap_or(0);
    let last = buffer.len();
    let mut fields = vec![Unit::empty(); max_number];
    let mut ordered_fields = descriptor.fields().collect::<Vec<_>>();
    ordered_fields.sort_by_key(|field| std::cmp::Reverse(field.number()));
    for field in ordered_fields {
        if !message.has_field(&field) {
            continue;
        }
        let mut unit = encode_field_value(
            descriptor.full_name(),
            field.name(),
            &field,
            message.get_field(&field).as_ref(),
            buffer,
        )?;
        if matches!(field.kind(), Kind::Message(_)) && !field.is_list() && !field.is_map() && unit.size() == 1
        {
            if unit.is_segment() {
                buffer.shrink(1);
            }
            fields[field.number() as usize - 1] = Unit::empty();
            continue;
        }
        fold_field(buffer, &mut unit);
        fields[field.number() as usize - 1] = unit;
    }

    serialize_message_at(&mut fields, buffer, last).ok_or_else(|| MutableError::SerializeFailed {
        descriptor: descriptor.full_name().to_owned(),
        field: "<message>".to_owned(),
    })
}

fn encode_field_value(
    descriptor_name: &str,
    field_name: &str,
    field: &FieldDescriptor,
    value: &ReflectValue,
    buffer: &mut Buffer,
) -> Result<Unit, MutableError> {
    if field.is_map() {
        let Kind::Message(entry) = field.kind() else {
            return Err(type_mismatch(descriptor_name, field_name, "map entry"));
        };
        let ReflectValue::Map(entries) = value else {
            return Err(type_mismatch(descriptor_name, field_name, "map"));
        };
        return encode_map_field(
            descriptor_name,
            field_name,
            &entry.map_entry_key_field().kind(),
            &entry.map_entry_value_field().kind(),
            entries,
            buffer,
        );
    }

    if field.is_list() {
        let ReflectValue::List(items) = value else {
            return Err(type_mismatch(descriptor_name, field_name, "array"));
        };
        return encode_list_field(descriptor_name, field_name, &field.kind(), items, buffer);
    }

    encode_kind_value(descriptor_name, field_name, &field.kind(), value, buffer)
}

fn encode_kind_value(
    descriptor_name: &str,
    field_name: &str,
    kind: &Kind,
    value: &ReflectValue,
    buffer: &mut Buffer,
) -> Result<Unit, MutableError> {
    match (kind, value) {
        (Kind::Bool, ReflectValue::Bool(value)) => Ok(serialize_bool(*value)),
        (Kind::Int32 | Kind::Sint32 | Kind::Sfixed32, ReflectValue::I32(value)) => {
            Ok(serialize_scalar::<i32>(*value))
        }
        (Kind::Uint32 | Kind::Fixed32, ReflectValue::U32(value)) => {
            Ok(serialize_scalar::<u32>(*value))
        }
        (Kind::Int64 | Kind::Sint64 | Kind::Sfixed64, ReflectValue::I64(value)) => {
            Ok(serialize_scalar::<i64>(*value))
        }
        (Kind::Uint64 | Kind::Fixed64, ReflectValue::U64(value)) => {
            Ok(serialize_scalar::<u64>(*value))
        }
        (Kind::Float, ReflectValue::F32(value)) => Ok(serialize_scalar::<f32>(*value)),
        (Kind::Double, ReflectValue::F64(value)) => Ok(serialize_scalar::<f64>(*value)),
        (Kind::String, ReflectValue::String(value)) => {
            serialize_str(value, buffer).ok_or_else(|| MutableError::SerializeFailed {
                descriptor: descriptor_name.to_owned(),
                field: field_name.to_owned(),
            })
        }
        (Kind::Bytes, ReflectValue::Bytes(value)) => {
            serialize_bytes(value, buffer).ok_or_else(|| MutableError::SerializeFailed {
                descriptor: descriptor_name.to_owned(),
                field: field_name.to_owned(),
            })
        }
        (Kind::Enum(_), ReflectValue::EnumNumber(value)) => Ok(serialize_scalar::<i32>(*value)),
        (Kind::Message(descriptor), ReflectValue::Message(message)) => {
            if let Some(alias) = alias_field(descriptor) {
                return encode_field_value(
                    descriptor.full_name(),
                    alias.name(),
                    &alias,
                    message.get_field(&alias).as_ref(),
                    buffer,
                );
            }
            encode_dynamic_message(message, buffer)
        }
        _ => Err(type_mismatch(descriptor_name, field_name, "matching value")),
    }
}

fn encode_list_field(
    descriptor_name: &str,
    field_name: &str,
    item_kind: &Kind,
    items: &[ReflectValue],
    buffer: &mut Buffer,
) -> Result<Unit, MutableError> {
    if matches!(item_kind, Kind::Bool) {
        let mut bytes = Vec::with_capacity(items.len());
        for item in items {
            let ReflectValue::Bool(value) = item else {
                return Err(type_mismatch(descriptor_name, field_name, "bool array"));
            };
            bytes.push(u8::from(*value));
        }
        return serialize_bytes(&bytes, buffer).ok_or_else(|| MutableError::SerializeFailed {
            descriptor: descriptor_name.to_owned(),
            field: field_name.to_owned(),
        });
    }

    let mut units = Vec::with_capacity(items.len());
    let last = buffer.len();
    for item in items {
        let mut unit = encode_kind_value(
            descriptor_name,
            field_name,
            item_kind,
            item,
            buffer,
        )?;
        if matches!(item_kind, Kind::Message(_)) && unit.size() > 1 {
            fold_field(buffer, &mut unit);
        }
        units.push(unit);
    }
    serialize_array_at(&units, buffer, last).ok_or_else(|| MutableError::SerializeFailed {
        descriptor: descriptor_name.to_owned(),
        field: field_name.to_owned(),
    })
}

fn encode_map_field(
    descriptor_name: &str,
    field_name: &str,
    key_kind: &Kind,
    value_kind: &Kind,
    entries: &std::collections::HashMap<ReflectMapKey, ReflectValue>,
    buffer: &mut Buffer,
) -> Result<Unit, MutableError> {
    let items = entries.iter().collect::<Vec<_>>();
    let key_bytes = items
        .iter()
        .map(|(key, _)| map_key_bytes(descriptor_name, field_name, key_kind, key))
        .collect::<Result<Vec<_>, _>>()?;
    let (index, positions) =
        build_perfect_hash_index_with_positions(&key_bytes).ok_or_else(|| {
            MutableError::SerializeFailed {
                descriptor: descriptor_name.to_owned(),
                field: field_name.to_owned(),
            }
        })?;

    let mut ordered = vec![None; items.len()];
    for (idx, position) in positions.into_iter().enumerate() {
        ordered[position] = Some(items[idx]);
    }

    let last = buffer.len();
    let mut keys = vec![Unit::empty(); ordered.len()];
    let mut values = vec![Unit::empty(); ordered.len()];
    for i in (0..ordered.len()).rev() {
        let (key, value) = ordered[i].expect("perfect-hash positions must cover all entries");
        keys[i] = encode_map_key(descriptor_name, field_name, key_kind, key, buffer)?;
        values[i] = encode_kind_value(descriptor_name, field_name, value_kind, value, buffer)?;
    }
    serialize_map_at(&index, &keys, &values, buffer, last).ok_or_else(|| MutableError::SerializeFailed {
        descriptor: descriptor_name.to_owned(),
        field: field_name.to_owned(),
    })
}

fn map_key_bytes(
    descriptor_name: &str,
    field_name: &str,
    kind: &Kind,
    key: &ReflectMapKey,
) -> Result<Vec<u8>, MutableError> {
    match (kind, key) {
        (Kind::String, ReflectMapKey::String(value)) => Ok(value.as_bytes().to_vec()),
        (Kind::Int32 | Kind::Sint32 | Kind::Sfixed32, ReflectMapKey::I32(value)) => {
            Ok(value.to_le_bytes().to_vec())
        }
        (Kind::Uint32 | Kind::Fixed32, ReflectMapKey::U32(value)) => {
            Ok(value.to_le_bytes().to_vec())
        }
        (Kind::Int64 | Kind::Sint64 | Kind::Sfixed64, ReflectMapKey::I64(value)) => {
            Ok(value.to_le_bytes().to_vec())
        }
        (Kind::Uint64 | Kind::Fixed64, ReflectMapKey::U64(value)) => {
            Ok(value.to_le_bytes().to_vec())
        }
        _ => Err(type_mismatch(descriptor_name, field_name, "supported map key")),
    }
}

fn encode_map_key(
    descriptor_name: &str,
    field_name: &str,
    kind: &Kind,
    key: &ReflectMapKey,
    buffer: &mut Buffer,
) -> Result<Unit, MutableError> {
    match (kind, key) {
        (Kind::String, ReflectMapKey::String(value)) => {
            serialize_str(value, buffer).ok_or_else(|| MutableError::SerializeFailed {
                descriptor: descriptor_name.to_owned(),
                field: field_name.to_owned(),
            })
        }
        (Kind::Int32 | Kind::Sint32 | Kind::Sfixed32, ReflectMapKey::I32(value)) => {
            Ok(serialize_scalar::<i32>(*value))
        }
        (Kind::Uint32 | Kind::Fixed32, ReflectMapKey::U32(value)) => {
            Ok(serialize_scalar::<u32>(*value))
        }
        (Kind::Int64 | Kind::Sint64 | Kind::Sfixed64, ReflectMapKey::I64(value)) => {
            Ok(serialize_scalar::<i64>(*value))
        }
        (Kind::Uint64 | Kind::Fixed64, ReflectMapKey::U64(value)) => {
            Ok(serialize_scalar::<u64>(*value))
        }
        _ => Err(type_mismatch(descriptor_name, field_name, "supported map key")),
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

fn type_mismatch(descriptor: &str, field: &str, expected: &'static str) -> MutableError {
    MutableError::TypeMismatch {
        descriptor: descriptor.to_owned(),
        field: field.to_owned(),
        expected,
    }
}

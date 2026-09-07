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
    serialize_dynamic_message_into_buffer(&message, buffer)
}

pub(crate) fn serialize_dynamic_message(
    message: &DynamicMessage,
) -> Result<Vec<u32>, MutableError> {
    let mut buffer = Buffer::new();
    let _ = serialize_dynamic_message_into_buffer(message, &mut buffer)?;
    Ok(buffer.view().to_vec())
}

pub(crate) fn serialize_dynamic_message_into_buffer<'b>(
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
    if let Some(alias) = alias_field(&descriptor)? {
        return encode_field_value(
            descriptor.full_name(),
            alias.name(),
            &alias,
            message.get_field(&alias).as_ref(),
            buffer,
        );
    }

    let mut ordered_fields = descriptor.fields().collect::<Vec<_>>();
    ordered_fields.sort_by_key(|field| std::cmp::Reverse(field.number()));
    let max_number = ordered_fields
        .first()
        .map_or(0, |field| field.number() as usize);
    if max_number == 0 || max_number > 12 + 25 * 255 {
        return Err(MutableError::SerializeFailed {
            descriptor: descriptor.full_name().to_owned(),
            field: "<field-numbers>".to_owned(),
        });
    }
    let last = buffer.len();
    let mut fields = vec![Unit::empty(); max_number];
    for field in ordered_fields {
        if crate::reflection::is_deprecated_field(field.field_descriptor_proto())
            || !message.has_field(&field)
        {
            continue;
        }
        let mut unit = encode_field_value(
            descriptor.full_name(),
            field.name(),
            &field,
            message.get_field(&field).as_ref(),
            buffer,
        )?;
        if matches!(field.kind(), Kind::Message(_))
            && !field.is_list()
            && !field.is_map()
            && unit.size() == 1
            // A one-word inline alias can contain values (e.g. 1–3 bools).
            // Only a zero inline word or an empty message segment is absent.
            && (unit.is_segment() || unit.inline_words()[0] == 0)
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
        (Kind::Message(_), ReflectValue::Message(message)) => {
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

    if items.is_empty() {
        // No element units remain from which the general encoder can infer width.
        let width = match item_kind {
            Kind::Int64
            | Kind::Sint64
            | Kind::Sfixed64
            | Kind::Uint64
            | Kind::Fixed64
            | Kind::Double => 2,
            _ => 1,
        };
        return Ok(Unit::inline(&[width]));
    }

    let mut units = Vec::with_capacity(items.len());
    let last = buffer.len();
    for item in items {
        let mut unit = encode_kind_value(descriptor_name, field_name, item_kind, item, buffer)?;
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
    serialize_map_at(&index, &keys, &values, buffer, last).ok_or_else(|| {
        MutableError::SerializeFailed {
            descriptor: descriptor_name.to_owned(),
            field: field_name.to_owned(),
        }
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
        _ => Err(type_mismatch(
            descriptor_name,
            field_name,
            "supported map key",
        )),
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
        _ => Err(type_mismatch(
            descriptor_name,
            field_name,
            "supported map key",
        )),
    }
}

fn alias_field(descriptor: &MessageDescriptor) -> Result<Option<FieldDescriptor>, MutableError> {
    crate::reflection::alias_field(descriptor.full_name(), descriptor.descriptor_proto())
        .map(|field| field.and_then(|field| descriptor.get_field(field.number() as u32)))
        .map_err(|_| MutableError::SerializeFailed {
            descriptor: descriptor.full_name().to_owned(),
            field: "_".to_owned(),
        })
}

fn type_mismatch(descriptor: &str, field: &str, expected: &'static str) -> MutableError {
    MutableError::TypeMismatch {
        descriptor: descriptor.to_owned(),
        field: field.to_owned(),
        expected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
        MessageOptions,
        field_descriptor_proto::{Label, Type},
    };
    use protocache_core::{ArrayView, MapView};

    fn field(name: &str, ty: Type, type_name: &str, repeated: bool) -> FieldDescriptorProto {
        FieldDescriptorProto {
            name: Some(name.to_owned()),
            number: Some(1),
            label: Some(if repeated {
                Label::Repeated
            } else {
                Label::Optional
            } as i32),
            r#type: Some(ty as i32),
            type_name: (!type_name.is_empty()).then(|| type_name.to_owned()),
            ..Default::default()
        }
    }

    #[test]
    fn reflection_and_encoding_reject_the_same_invalid_aliases() {
        for case in 0..3 {
            let mut alias = field("_", Type::Int32, "", true);
            if case == 0 {
                alias.number = Some(2);
            } else if case == 1 {
                alias.label = Some(Label::Optional as i32);
            }
            let mut fields = vec![alias];
            if case == 2 {
                let mut old = field("old", Type::Int32, "", false);
                old.number = Some(2);
                old.options = Some(prost_types::FieldOptions {
                    deprecated: Some(true),
                    ..Default::default()
                });
                fields.push(old);
            }
            let file = FileDescriptorProto {
                name: Some("invalid-alias.proto".to_owned()),
                syntax: Some("proto3".to_owned()),
                message_type: vec![DescriptorProto {
                    name: Some("Alias".to_owned()),
                    field: fields,
                    ..Default::default()
                }],
                ..Default::default()
            };
            let mut schema = crate::reflection::DescriptorPool::default();
            assert!(matches!(
                schema.register(&file),
                Err(crate::reflection::RegisterError::InvalidAlias { .. })
            ));
            let pool = prost_reflect::DescriptorPool::from_file_descriptor_set(FileDescriptorSet {
                file: vec![file],
            })
            .unwrap();
            let message = DynamicMessage::new(pool.get_message_by_name("Alias").unwrap());
            assert!(serialize_dynamic_message(&message).is_err(), "case {case}");
        }
    }

    #[test]
    fn dynamic_encoding_omits_deprecated_fields_without_reusing_their_slots() {
        let mut old = field("old", Type::String, "", false);
        old.options = Some(prost_types::FieldOptions {
            deprecated: Some(true),
            ..Default::default()
        });
        let mut value = field("value", Type::Int32, "", false);
        value.number = Some(2);
        let pool = prost_reflect::DescriptorPool::from_file_descriptor_set(FileDescriptorSet {
            file: vec![FileDescriptorProto {
                name: Some("deprecated.proto".to_owned()),
                syntax: Some("proto3".to_owned()),
                message_type: vec![DescriptorProto {
                    name: Some("Root".to_owned()),
                    field: vec![old, value],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        })
        .unwrap();
        let mut message = DynamicMessage::new(pool.get_message_by_name("Root").unwrap());
        message.set_field_by_name("old", ReflectValue::String("deprecated payload".to_owned()));
        message.set_field_by_name("value", ReflectValue::I32(7));
        let words = serialize_dynamic_message(&message).unwrap();
        let view = protocache_core::MessageView::new(&words).unwrap();
        assert!(!view.has_field(0));
        assert_eq!(view.scalar::<i32>(1), Some(7));
        assert_eq!(words.len(), 2);
    }

    #[test]
    fn short_aliases_preserve_root_and_nested_values_with_buffer_reuse() {
        for ty in [
            Type::Bool,
            Type::Int32,
            Type::Sint32,
            Type::Sfixed32,
            Type::Uint32,
            Type::Fixed32,
            Type::Float,
            Type::Int64,
            Type::Sint64,
            Type::Sfixed64,
            Type::Uint64,
            Type::Fixed64,
            Type::Double,
        ] {
            let mut plain = field("plain", Type::Message, ".Plain", false);
            plain.number = Some(2);
            let mut tail = field("tail", Type::String, "", false);
            tail.number = Some(3);
            let pool = prost_reflect::DescriptorPool::from_file_descriptor_set(FileDescriptorSet {
                file: vec![FileDescriptorProto {
                    name: Some("short-alias.proto".to_owned()),
                    syntax: Some("proto3".to_owned()),
                    message_type: vec![
                        DescriptorProto {
                            name: Some("Row".to_owned()),
                            field: vec![field("_", ty, "", true)],
                            ..Default::default()
                        },
                        DescriptorProto {
                            name: Some("Plain".to_owned()),
                            field: vec![field("value", Type::Int32, "", false)],
                            ..Default::default()
                        },
                        DescriptorProto {
                            name: Some("Holder".to_owned()),
                            field: vec![field("row", Type::Message, ".Row", false), plain, tail],
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }],
            })
            .unwrap();
            let is_bool = ty == Type::Bool;
            let width = match ty {
                Type::Bool => 0,
                Type::Int64
                | Type::Sint64
                | Type::Sfixed64
                | Type::Uint64
                | Type::Fixed64
                | Type::Double => 2,
                _ => 1,
            };
            let mut buffer = Buffer::new();
            for size in [0usize, 1, 2, 3, 4, 3, 2, 1, 0] {
                let expected: Vec<_> = (0..size)
                    .map(|i| match ty {
                        Type::Bool => ReflectValue::Bool(i % 2 != 0),
                        Type::Int64 | Type::Sint64 | Type::Sfixed64 => {
                            ReflectValue::I64(i as i64 - 1)
                        }
                        Type::Uint64 | Type::Fixed64 => ReflectValue::U64(i as u64),
                        Type::Double => ReflectValue::F64(i as f64 - 0.5),
                        Type::Uint32 | Type::Fixed32 => ReflectValue::U32(i as u32),
                        Type::Float => ReflectValue::F32(i as f32 - 0.5),
                        _ => ReflectValue::I32(i as i32 - 1),
                    })
                    .collect();
                let mut row = DynamicMessage::new(pool.get_message_by_name("Row").unwrap());
                row.set_field_by_name("_", ReflectValue::List(expected.clone()));
                let mut holder = DynamicMessage::new(pool.get_message_by_name("Holder").unwrap());
                holder.set_field_by_name("row", ReflectValue::Message(row.clone()));
                holder.set_field_by_name(
                    "plain",
                    ReflectValue::Message(DynamicMessage::new(
                        pool.get_message_by_name("Plain").unwrap(),
                    )),
                );
                let tail = "a sibling payload that must survive empty-message removal";
                holder.set_field_by_name("tail", ReflectValue::String(tail.to_owned()));

                for (input, nested) in [(&row, false), (&holder, true)] {
                    for protobuf_bytes in [false, true] {
                        let context = format!(
                            "{ty:?}, size={size}, nested={nested}, protobuf={protobuf_bytes}"
                        );
                        let words = if protobuf_bytes {
                            serialize_protobuf_bytes_into_buffer(
                                input.descriptor(),
                                &input.encode_to_vec(),
                                &mut buffer,
                            )
                        } else {
                            serialize_dynamic_message_into_buffer(input, &mut buffer)
                        }
                        .unwrap();
                        let row_words = if nested {
                            let view = protocache_core::MessageView::new(words).unwrap();
                            assert!(!view.has_field(1), "{context}");
                            assert_eq!(view.string(2).unwrap().as_str(), Some(tail), "{context}");
                            if is_bool && size == 0 {
                                assert!(!view.has_field(0), "{context}");
                                continue;
                            }
                            assert!(view.has_field(0), "{context}");
                            view.field(0).unwrap().object_words().unwrap()
                        } else {
                            assert_eq!(
                                words.len(),
                                if is_bool {
                                    (1 + size).div_ceil(4)
                                } else {
                                    1 + size * width
                                },
                                "{context}"
                            );
                            assert_eq!(
                                words[0] & 0xff,
                                ((size as u32) << 2) | width as u32,
                                "{context}"
                            );
                            words
                        };
                        if size == 0 {
                            assert_eq!(row_words[0], width as u32, "{context}");
                        }
                        let actual: Vec<_> = if is_bool {
                            protocache_core::StringView::new(row_words)
                                .unwrap()
                                .as_bool_array()
                                .iter()
                                .map(ReflectValue::Bool)
                                .collect()
                        } else {
                            let array = ArrayView::new(row_words).unwrap();
                            macro_rules! read_scalars {
                                ($scalar:ty, $value:ident) => {
                                    array
                                        .scalars::<$scalar>()
                                        .unwrap()
                                        .iter()
                                        .map(ReflectValue::$value)
                                        .collect()
                                };
                            }
                            match ty {
                                Type::Int64 | Type::Sint64 | Type::Sfixed64 => {
                                    read_scalars!(i64, I64)
                                }
                                Type::Uint64 | Type::Fixed64 => read_scalars!(u64, U64),
                                Type::Double => read_scalars!(f64, F64),
                                Type::Uint32 | Type::Fixed32 => read_scalars!(u32, U32),
                                Type::Float => read_scalars!(f32, F32),
                                _ => read_scalars!(i32, I32),
                            }
                        };
                        assert_eq!(actual, expected, "{context}");
                    }
                }
            }
        }
    }

    #[test]
    fn dynamic_encoding_rejects_unrepresentable_field_numbers_before_allocation() {
        for number in [12 + 25 * 255 + 1, 536_870_911] {
            let mut value = field("value", Type::Int32, "", false);
            value.number = Some(number);
            let pool = prost_reflect::DescriptorPool::from_file_descriptor_set(FileDescriptorSet {
                file: vec![FileDescriptorProto {
                    name: Some("sparse.proto".to_owned()),
                    syntax: Some("proto3".to_owned()),
                    message_type: vec![DescriptorProto {
                        name: Some("Sparse".to_owned()),
                        field: vec![value],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
            })
            .unwrap();
            let mut message = DynamicMessage::new(pool.get_message_by_name("Sparse").unwrap());
            message.set_field_by_name("value", ReflectValue::I32(7));
            assert!(serialize_dynamic_message(&message).is_err());
        }
    }

    #[test]
    fn nested_dynamic_containers_preserve_order_and_payloads_with_buffer_reuse() {
        // Array<map<string, array<string>>> exercises both compaction paths, including
        // empty aliases, mixed inline/segmented strings, and multi-entry maps.
        let mut value = field("value", Type::Message, ".Rows", false);
        value.number = Some(2);
        let pool = prost_reflect::DescriptorPool::from_file_descriptor_set(FileDescriptorSet {
            file: vec![FileDescriptorProto {
                name: Some("containers.proto".to_owned()),
                syntax: Some("proto3".to_owned()),
                message_type: vec![
                    DescriptorProto {
                        name: Some("Rows".to_owned()),
                        field: vec![field("_", Type::String, "", true)],
                        ..Default::default()
                    },
                    DescriptorProto {
                        name: Some("Book".to_owned()),
                        field: vec![field("_", Type::Message, ".Book.Entry", true)],
                        nested_type: vec![DescriptorProto {
                            name: Some("Entry".to_owned()),
                            field: vec![field("key", Type::String, "", false), value],
                            options: Some(MessageOptions {
                                map_entry: Some(true),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                    DescriptorProto {
                        name: Some("Root".to_owned()),
                        field: vec![field("_", Type::Message, ".Book", true)],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
        })
        .unwrap();
        for (name, header) in [("Rows", 1), ("Book", 5u32 << 28), ("Root", 1)] {
            let empty = DynamicMessage::new(pool.get_message_by_name(name).unwrap());
            assert_eq!(
                serialize_dynamic_message(&empty).unwrap(),
                [header],
                "{name}"
            );
        }
        let mut buffer = Buffer::new();
        for count in [0, 1, 2, 5, 33, 1, 0] {
            let expected: Vec<Vec<(String, Vec<String>)>> = (0..count)
                .map(|i| {
                    (0..i % 6)
                        .map(|j| {
                            let key = if j % 2 == 0 {
                                format!("{j}")
                            } else {
                                format!("key/{j}/{}", "k".repeat(i * 3))
                            };
                            let rows = (0..j).map(|k| "v".repeat((i * 17 + k * 31) % 97)).collect();
                            (key, rows)
                        })
                        .collect()
                })
                .collect();
            let books = expected
                .iter()
                .map(|entries| {
                    let entries = entries
                        .iter()
                        .map(|(key, rows)| {
                            let mut message =
                                DynamicMessage::new(pool.get_message_by_name("Rows").unwrap());
                            message.set_field_by_name(
                                "_",
                                ReflectValue::List(
                                    rows.iter().cloned().map(ReflectValue::String).collect(),
                                ),
                            );
                            (
                                ReflectMapKey::String(key.clone()),
                                ReflectValue::Message(message),
                            )
                        })
                        .collect();
                    let mut message =
                        DynamicMessage::new(pool.get_message_by_name("Book").unwrap());
                    message.set_field_by_name("_", ReflectValue::Map(entries));
                    ReflectValue::Message(message)
                })
                .collect();
            let mut root = DynamicMessage::new(pool.get_message_by_name("Root").unwrap());
            root.set_field_by_name("_", ReflectValue::List(books));
            let words = serialize_dynamic_message_into_buffer(&root, &mut buffer).unwrap();
            let array = ArrayView::new(words).unwrap();
            assert_eq!(array.len(), expected.len());
            for (i, entries) in expected.iter().enumerate() {
                let map = MapView::new(array.field(i).unwrap().object_words().unwrap()).unwrap();
                assert_eq!(map.len(), entries.len());
                for (key, rows) in entries {
                    let values = map.find_str(key).unwrap().value().array().unwrap();
                    let actual: Vec<_> = values
                        .iter()
                        .map(|item| item.string().unwrap().as_str().unwrap())
                        .collect();
                    assert_eq!(&actual, rows);
                }
            }
        }
    }
}

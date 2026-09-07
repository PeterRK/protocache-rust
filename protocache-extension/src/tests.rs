use prost::Message;
use prost_reflect::{
    DescriptorPool, DynamicMessage, MapKey as ReflectMapKey, Value as ReflectValue,
};
use protocache_core::{MapView, MessageView};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::serialize::serialize_protobuf_bytes;
use crate::utils::{
    dump_json, load_json, parse_proto, parse_proto_file, serialize_dynamic,
    serialize_dynamic_into_buffer, serialize_prost,
};

const BASIC_SCHEMA: &str = r#"
syntax = "proto3";

package test;

message Small {
    int32 i32 = 1;
    bool flag = 2;
    string str = 4;
}

message Main {
    int32 i32 = 1;
    string str = 7;
    Small object = 11;
    repeated int32 i32v = 12;
    map<string, int32> index = 26;
}
"#;

const ALIAS_SCHEMA: &str = r#"
syntax = "proto3";

package test;

message Vec2D {
    message Vec1D {
        repeated float _ = 1;
    }
    repeated Vec1D _ = 1;
}

message ArrMap {
    message Array {
        repeated float _ = 1;
    }
    map<string, Array> _ = 1;
}

message Main {
    Vec2D matrix = 28;
    ArrMap arrays = 30;
}
"#;

fn write_test_schema_file(contents: &str) -> (TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix("pcrs-ext-schema-")
        .tempdir()
        .unwrap();
    let path = dir.path().join("test.proto");
    fs::write(&path, contents).unwrap();
    (dir, path)
}

fn load_reflect_descriptor_pool_from_proto_file(
    path: impl AsRef<Path>,
) -> Result<DescriptorPool, crate::utils::ProtoError> {
    let file = parse_proto_file(path)?;
    DescriptorPool::from_file_descriptor_set(prost_types::FileDescriptorSet { file: vec![file] })
        .map_err(crate::utils::ProtoError::Reflect)
}

#[test]
fn serializes_dynamic_message_via_read_only_reflection() {
    let (_dir, schema) = write_test_schema_file(BASIC_SCHEMA);
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
        ReflectValue::List(vec![
            ReflectValue::I32(1),
            ReflectValue::I32(2),
            ReflectValue::I32(3),
        ]),
    );
    root.set_field_by_name(
        "index",
        ReflectValue::Map(HashMap::from([(
            ReflectMapKey::String("k".to_owned()),
            ReflectValue::I32(9),
        )])),
    );

    let words = serialize_dynamic(&root).unwrap();
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
fn protobuf_bytes_and_prost_entry_points_match() {
    let (_dir, schema) = write_test_schema_file(BASIC_SCHEMA);
    let pool = load_reflect_descriptor_pool_from_proto_file(&schema).unwrap();
    let descriptor = pool.get_message_by_name("test.Main").unwrap();

    let mut root = DynamicMessage::new(descriptor.clone());
    root.set_field_by_name("i32", ReflectValue::I32(123));
    root.set_field_by_name("str", ReflectValue::String("hi".to_owned()));
    let bytes = root.encode_to_vec();

    let from_bytes = serialize_protobuf_bytes(descriptor.clone(), &bytes).unwrap();
    let from_message = serialize_dynamic(&root).unwrap();
    let from_prost = serialize_prost(&root, descriptor).unwrap();
    assert_eq!(from_prost, from_message);
    assert_eq!(from_bytes, from_message);

    let bytes_view = MessageView::new(&from_bytes).unwrap();
    let message_view = MessageView::new(&from_message).unwrap();
    assert_eq!(bytes_view.scalar::<i32>(0), Some(123));
    assert_eq!(message_view.scalar::<i32>(0), Some(123));
    assert_eq!(bytes_view.string(6).unwrap().as_str(), Some("hi"));
    assert_eq!(message_view.string(6).unwrap().as_str(), Some("hi"));
}

#[test]
fn serializes_alias_fields_without_mutable_runtime() {
    let (_dir, schema) = write_test_schema_file(ALIAS_SCHEMA);
    let pool = load_reflect_descriptor_pool_from_proto_file(&schema).unwrap();
    let descriptor = pool.get_message_by_name("test.Main").unwrap();
    let vec2d = pool.get_message_by_name("test.Vec2D").unwrap();
    let vec1d = pool.get_message_by_name("test.Vec2D.Vec1D").unwrap();
    let arr_map = pool.get_message_by_name("test.ArrMap").unwrap();
    let array = pool.get_message_by_name("test.ArrMap.Array").unwrap();

    let mut row = DynamicMessage::new(vec1d.clone());
    row.set_field_by_name(
        "_",
        ReflectValue::List(vec![ReflectValue::F32(1.0), ReflectValue::F32(2.0)]),
    );
    let mut matrix = DynamicMessage::new(vec2d);
    matrix.set_field_by_name("_", ReflectValue::List(vec![ReflectValue::Message(row)]));

    let mut arr = DynamicMessage::new(array);
    arr.set_field_by_name("_", ReflectValue::List(vec![ReflectValue::F32(3.0)]));
    let mut arrays = DynamicMessage::new(arr_map.clone());
    arrays.set_field_by_name(
        "_",
        ReflectValue::Map(HashMap::from([(
            ReflectMapKey::String("lv".to_owned()),
            ReflectValue::Message(arr),
        )])),
    );

    let mut root = DynamicMessage::new(descriptor.clone());
    root.set_field_by_name("matrix", ReflectValue::Message(matrix));
    root.set_field_by_name("arrays", ReflectValue::Message(arrays));

    let words = serialize_dynamic(&root).unwrap();
    let view = MessageView::new(&words).unwrap();
    assert_eq!(view.field(27).unwrap().detect_array().unwrap().len(), 4);
    assert_eq!(
        MapView::new(view.field(29).unwrap().detect_map().unwrap())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn serialization_reuses_buffer_after_message_changes() {
    let file = parse_proto(
        r#"
            syntax = "proto3";
            package demo;
            message Root {
                int32 value = 1;
            }
            "#,
        "inline.proto",
    )
    .unwrap();
    let pool = DescriptorPool::from_file_descriptor_set(prost_types::FileDescriptorSet {
        file: vec![file],
    })
    .unwrap();
    let mut message = DynamicMessage::new(pool.get_message_by_name("demo.Root").unwrap());
    message.set_field_by_name("value", ReflectValue::I32(5));

    let mut buffer = protocache_core::Buffer::new();
    let first = serialize_dynamic_into_buffer(&message, &mut buffer).unwrap();
    assert_eq!(MessageView::new(first).unwrap().scalar::<i32>(0), Some(5));
    let allocated = buffer.allocated_words();
    message.set_field_by_name("value", ReflectValue::I32(8));
    let second = serialize_dynamic_into_buffer(&message, &mut buffer).unwrap();
    assert_eq!(MessageView::new(second).unwrap().scalar::<i32>(0), Some(8));
    assert_eq!(buffer.allocated_words(), allocated);
    message.clear_field_by_name("value");
    assert_eq!(
        serialize_dynamic_into_buffer(&message, &mut buffer).unwrap(),
        &[0]
    );
}

#[test]
fn loads_and_dumps_json_via_reflection() {
    let (dir, schema) = write_test_schema_file(BASIC_SCHEMA);
    let pool = load_reflect_descriptor_pool_from_proto_file(&schema).unwrap();
    let descriptor = pool.get_message_by_name("test.Main").unwrap();
    let json_path = dir.path().join("roundtrip.json");
    fs::write(&json_path, "{\n  \"i32\": 9,\n  \"str\": \"hello\"\n}\n").unwrap();

    let message = load_json(&json_path, descriptor.clone()).unwrap();
    assert_eq!(message.get_field_by_name("i32").unwrap().as_i32(), Some(9));
    assert_eq!(
        message.get_field_by_name("str").unwrap().as_str(),
        Some("hello")
    );

    dump_json(&message, &json_path).unwrap();
    let dumped = fs::read_to_string(&json_path).unwrap();
    let parsed = serde_json::from_str::<serde_json::Value>(&dumped).unwrap();
    assert!(dumped.contains("\"i32\": 9"));
    assert!(dumped.contains("\"str\": \"hello\""));
    assert!(parsed.get("i32").is_some());
    assert!(parsed.get("str").is_some());

    fs::remove_file(json_path).unwrap();
}

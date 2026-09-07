use super::*;
use std::collections::HashMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use protocache_core::ViewMap;
use prost_reflect::{DescriptorPool as ReflectDescriptorPool, DynamicMessage, MapKey as ReflectMapKey, Value as ReflectValue};
use protocache_extension::reflection::{DescriptorPool as PcDescriptorPool, FieldType, RegisterError};
use protocache_extension::utils::{
    JsonError, load_json, parse_proto, parse_proto_file, serialize_dynamic,
};

const BENCHMARK_REFLECT_FUSE: u64 = 0x41f3b5c6f40079b0;

fn fixture_root() -> PathBuf {
    repo_root().join("tests/fixtures")
}

fn load_fixture_words() -> Vec<u32> {
    bytes_to_words(protocache_fixture_bytes())
}

fn load_fixture_bytes(name: &str) -> Vec<u8> {
    match name {
        "test.pb" => protobuf_fixture_bytes().to_vec(),
        "test.pc" => protocache_fixture_bytes().to_vec(),
        _ => fs::read(fixture_root().join("benchmark").join(name)).unwrap(),
    }
}

fn unique_temp_json_path(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("pcrs-bench-{name}-{}-{nanos}.json", std::process::id()))
}

fn load_reflect_descriptor_pool_from_proto_file(
    path: &Path,
) -> Result<ReflectDescriptorPool, Box<dyn std::error::Error>> {
    let file = parse_proto_file(path)?;
    Ok(ReflectDescriptorPool::from_file_descriptor_set(FileDescriptorSet {
        file: vec![file],
    })?)
}

fn serialize_partly(words: &[u32]) -> Vec<u32> {
    let mut root = pcrs_generated::MainMutable::FromWords(words).unwrap();
    let _ = root.i32();
    let _ = root.u32();
    let _ = root.i64();
    let _ = root.u64();
    let _ = root.flag();
    let _ = root.mode();
    let _ = root.str();
    let _ = root.data();
    let _ = root.f32();
    let _ = root.f64();
    root.SerializeWords().unwrap()
}

fn serialize_fully_materialized(words: &[u32]) -> Vec<u32> {
    let mut root = pcrs_generated::MainMutable::FromWords(words).unwrap();
    let mut junk = Junk::default();
    traverse_pc_mutable_main(&mut root, &mut junk);
    assert_ne!(junk.fuse(), 0);
    root.SerializeWords().unwrap()
}

fn present_fields(words: &[u32]) -> Vec<usize> {
    let view = MessageView::new(words).unwrap();
    (0..30).filter(|&id| view.has_field(id)).collect()
}

fn field_detect_len(view: MessageView<'_>, id: usize) -> usize {
    let field = view.field(id).unwrap();
    match id {
        6 | 7 | 17 => field.detect_string().unwrap().len(),
        10 => field.detect_message().unwrap().len(),
        11 | 12 | 13 | 14 | 15 | 16 | 18 | 27 | 28 => field.detect_array().unwrap().len(),
        25 | 26 | 29 => field.detect_map().unwrap().len(),
        _ => field.detect_scalar().unwrap().len(),
    }
}

fn field_object_pos(words: &[u32], view: MessageView<'_>, id: usize) -> Option<usize> {
    let field = view.field(id).unwrap();
    let object = field.object_words()?;
    Some(unsafe { object.as_ptr().offset_from(words.as_ptr()) as usize })
}

fn field_width(view: MessageView<'_>, id: usize) -> usize {
    view.field(id).unwrap().raw_words().unwrap().len()
}

#[test]
fn generated_fully_serialization_matches_fixture() {
    let words = load_fixture_words();
    let root = pcrs_generated::MainMutable::FromWords(&words).unwrap();
    assert_eq!(root.SerializeWords().unwrap(), words);
}

#[test]
fn benchmark_raw_views_decode_fixture_maps_and_arrays() {
    let words = load_fixture_words();
    let root = MessageView::new(&words).unwrap();

    let strv = ViewArray::<StringView<'_>>::new(root.array(13).unwrap());
    assert_eq!(strv.len(), 10);
    assert_eq!(strv.get(0).unwrap().as_str(), Some("abc"));
    assert_eq!(strv.get(1).unwrap().as_str(), Some("apple"));

    let index = ViewMap::<StringView<'_>, i32>::new(root.map(25).unwrap());
    assert_eq!(index.len(), 6);
    let (key, value) = index.find_str("abc-1").unwrap();
    assert_eq!(key.as_str(), Some("abc-1"));
    assert_eq!(value, 1);

    let objects = ViewMap::<i32, MessageView<'_>>::new(root.map(26).unwrap());
    let (object_key, object_value) = objects.find_scalar(1).unwrap();
    assert_eq!(object_key, 1);
    assert_eq!(object_value.scalar::<i32>(0), Some(1));
    assert!(objects.find_scalar(5).is_none());
}

#[test]
fn benchmark_detect_and_corruption_guards_match_fixture_shape() {
    let mut words = load_fixture_words();
    let root = MessageView::new(&words).unwrap();

    let strv = root.field(13).unwrap().detect_array().unwrap();
    assert_eq!(ArrayView::detect(strv).unwrap(), strv);
    assert_eq!(ArrayView::detect_len(strv).unwrap(), strv.len());

    let index = root.field(25).unwrap().detect_map().unwrap();
    assert_eq!(MapView::detect(index).unwrap(), index);

    words[0] = u32::MAX;
    assert!(MessageView::new(&words).is_none());
}

#[test]
fn generated_fully_materialized_serialized_len_matches_fixture() {
    let words = load_fixture_words();
    let encoded = serialize_fully_materialized(&words);
    assert_eq!(present_fields(&encoded), present_fields(&words));
    let left = MessageView::new(&encoded).unwrap();
    let right = MessageView::new(&words).unwrap();
    let left_lens = present_fields(&encoded)
        .into_iter()
        .map(|id| (id, field_detect_len(left, id)))
        .collect::<Vec<_>>();
    let right_lens = present_fields(&words)
        .into_iter()
        .map(|id| (id, field_detect_len(right, id)))
        .collect::<Vec<_>>();
    assert_eq!(left_lens, right_lens);
    let left_widths = present_fields(&encoded)
        .into_iter()
        .map(|id| (id, field_width(left, id)))
        .collect::<Vec<_>>();
    let right_widths = present_fields(&words)
        .into_iter()
        .map(|id| (id, field_width(right, id)))
        .collect::<Vec<_>>();
    assert_eq!(left_widths, right_widths);
    assert_eq!(encoded.len(), words.len());
}

#[test]
fn generated_fully_materialized_serialization_matches_fixture() {
    let words = load_fixture_words();
    let encoded = serialize_fully_materialized(&words);
    let mut actual = Junk::default();
    traverse_pc_main(MessageView::new(&encoded).unwrap(), &mut actual).unwrap();

    let mut expected = Junk::default();
    traverse_pc_main(MessageView::new(&words).unwrap(), &mut expected).unwrap();

    assert_eq!(actual.fuse(), expected.fuse());
}

#[test]
fn benchmark_fixtures_match_expected_access_hashes() {
    let protobuf_raw = load_fixture_bytes("test.pb");
    let protocache_raw = load_fixture_bytes("test.pc");
    let protocache_words = bytes_to_words(&protocache_raw);

    let pb = pb::Main::decode(protobuf_raw.as_slice()).unwrap();
    let mut pb_junk = Junk::default();
    traverse_pb_main(&pb, &mut pb_junk);

    #[cfg(protocache_test_has_flatbuffers_generated)]
    {
        let raw = flatbuffers_fixture_bytes();
        let root = unsafe { fb_generated::test::root_as_main_unchecked(raw) };
        let mut junk = Junk::default();
        traverse_fb_main(root, &mut junk);
        assert_eq!(pb_junk.fuse(), junk.fuse());
    }

    #[cfg(protocache_test_has_fory_generated)]
    {
        let raw = load_fixture_bytes("test.fr");
        let fory = new_fory().unwrap();
        let root: fory_generated::Main = fory.deserialize(&raw).unwrap();
        let mut junk = Junk::default();
        traverse_fory_main(&root, &mut junk);
        assert_eq!(pb_junk.fuse(), junk.fuse());
    }

    let pc = MessageView::new(&protocache_words).unwrap();
    let mut pc_junk = Junk::default();
    traverse_pc_main(pc, &mut pc_junk).unwrap();
    assert_eq!(pb_junk.fuse(), pc_junk.fuse());

    let reflect_pool = load_reflect_descriptor_pool_from_proto_file(&fixture_root().join("proto/test.proto")).unwrap();
    let reflect_descriptor = reflect_pool.get_message_by_name("test.Main").unwrap();
    let reflect_plan = build_pb_reflect_descriptor_plan(&reflect_descriptor);
    let reflect_root = DynamicMessage::decode(reflect_descriptor, protobuf_raw.as_slice()).unwrap();
    let mut protobuf_reflect_junk = Junk::default();
    traverse_pb_reflect_descriptor(&reflect_plan, &reflect_root, &mut protobuf_reflect_junk).unwrap();
    assert_eq!(pb_junk.fuse(), protobuf_reflect_junk.fuse());

    let schema_path = fixture_root().join("proto/test.proto");
    let pool = load_descriptor_pool_from_proto_file(&schema_path).unwrap();
    let descriptor_name = "test.Main";
    let descriptor = pool.find(descriptor_name).unwrap();
    let plan = build_reflect_descriptor_plan(&pool, descriptor_name, descriptor).unwrap();
    let mut reflect_junk = Junk::default();
    traverse_pc_reflect_descriptor(&plan, pc, &mut reflect_junk);
    assert_ne!(reflect_junk.fuse(), 0);
}

#[test]
fn benchmark_reflect_hash_matches_cpp_baseline() {
    let protocache_raw = load_fixture_bytes("test.pc");
    let protocache_words = bytes_to_words(&protocache_raw);
    let pc = MessageView::new(&protocache_words).unwrap();
    let schema_path = fixture_root().join("proto/test.proto");
    let pool = load_descriptor_pool_from_proto_file(&schema_path).unwrap();
    let descriptor_name = "test.Main";
    let descriptor = pool.find(descriptor_name).unwrap();
    let plan = build_reflect_descriptor_plan(&pool, descriptor_name, descriptor).unwrap();
    let mut reflect_junk = Junk::default();
    traverse_pc_reflect_descriptor(&plan, pc, &mut reflect_junk);
    assert_eq!(reflect_junk.fuse(), BENCHMARK_REFLECT_FUSE);
}


#[test]
fn benchmark_generated_view_matches_cpp_basic_expectations() {
    let words = load_fixture_words();
    let root = pcrs_generated::test::Main::FromWords(&words).unwrap();

    assert_eq!(root.i32(), -999);
    assert_eq!(root.u32(), 1234);
    assert_eq!(root.i64(), -9_876_543_210);
    assert_eq!(root.u64(), 98_765_432_123_456_789);
    assert!(root.flag());
    assert_eq!(root.mode(), 2);
    assert_eq!(root.str(), Some("Hello World!"));
    assert_eq!(root.data(), Some(b"abc123!?$*&()'-=@~".as_slice()));
    assert_eq!(root.object().unwrap().i32(), 88);
    assert_eq!(root.objectv().unwrap().len(), 3);
    assert_eq!(root.matrix().unwrap().values().get(2).unwrap().values().get(2), Some(9.0));

    let arrays = root.arrays().unwrap().entries();
    let lv5 = arrays.find_str("lv5").unwrap().1.values();
    assert_eq!(lv5.get(0), Some(51.0));
    assert_eq!(lv5.get(1), Some(52.0));
}

#[test]
fn benchmark_generated_ex_matches_cpp_basic_expectations() {
    let words = load_fixture_words();
    let mut root = pcrs_generated::MainMutable::FromWords(&words).unwrap();

    assert_eq!(*root.i32(), -999);
    assert_eq!(*root.u32(), 1234);
    assert_eq!(*root.i64(), -9_876_543_210);
    assert_eq!(*root.u64(), 98_765_432_123_456_789);
    assert!(*root.flag());
    assert_eq!(*root.mode(), 2);
    assert_eq!(root.str().as_str(), "Hello World!");
    assert_eq!(root.data().as_slice(), b"abc123!?$*&()'-=@~");
    assert_eq!(root.object().i32(), &88);
    assert_eq!(root.objectv().len(), 3);
    assert_eq!(root.matrix()[2][2], 9.0);

    let lv5 = root.arrays().get("lv5").unwrap();
    assert_eq!(lv5[0], 51.0);
    assert_eq!(lv5[1], 52.0);
}

#[test]
#[cfg(protocache_test_has_fory_generated)]
fn benchmark_fory_generated_view_matches_cpp_basic_expectations() {
    let fory = new_fory().unwrap();
    let raw = load_fixture_bytes("test.fr");
    let root: fory_generated::Main = fory.deserialize(&raw).unwrap();

    assert_eq!(root.i32, -999);
    assert_eq!(root.u32, 1234);
    assert_eq!(root.i64, -9_876_543_210);
    assert_eq!(root.u64, 98_765_432_123_456_789);
    assert!(root.flag);
    assert_eq!(root.mode, fory_generated::Mode::C);
    assert_eq!(root.str, "Hello World!");
    assert_eq!(root.data.as_slice(), b"abc123!?$*&()'-=@~");
    assert_eq!(root.object.as_ref().unwrap().i32, 88);
    assert_eq!(root.objectv.len(), 3);
    assert_eq!(root.matrix.as_ref().unwrap().x[2].x[2], 9.0);
    assert_eq!(root.arrays.as_ref().unwrap().x["lv5"].x, [51.0, 52.0]);
}

#[test]
fn benchmark_generated_ex_serialize_mutation_regression() {
    let words = load_fixture_words();
    let mut root = pcrs_generated::MainMutable::FromWords(&words).unwrap();

    root.objectv();
    root.strv()[0] = "xyz".to_owned();
    root.matrix()[1][1] = 999.0;

    let arrays = root.arrays();
    arrays.get_mut("lv5").unwrap().push(53.0);
    let mut inserted = pcrs_generated::ArrayMutable::new();
    inserted.push(91.0);
    inserted.push(92.0);
    assert!(arrays.insert("lv9".to_owned(), inserted).is_none());

    let object_keys = root.objects().iter().map(|(key, _)| *key).collect::<Vec<_>>();
    for key in object_keys {
        root.objects().get_mut(&key).unwrap().i32().clone_from(&(key + 1));
    }

    let encoded = root.SerializeWords().unwrap();
    let view = pcrs_generated::test::Main::FromWords(&encoded).unwrap();

    assert_eq!(
        view.strv().unwrap().get(0).and_then(|value| value.as_str()),
        Some("xyz")
    );
    assert_eq!(
        view.matrix().unwrap().values().get(1).unwrap().values().get(1),
        Some(999.0)
    );

    let arrays = view.arrays().unwrap().entries();
    let available = arrays
        .iter()
        .filter_map(|(key, _)| key.as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    let lv5 = arrays
        .find_str("lv5")
        .unwrap_or_else(|| panic!("missing lv5, available keys: {available:?}"))
        .1
        .values();
    assert_eq!(lv5.len(), 3);
    assert_eq!(lv5.get(2), Some(53.0));
    let lv9 = arrays.find_str("lv9").unwrap().1.values();
    assert_eq!(lv9.get(0), Some(91.0));
    assert_eq!(lv9.get(1), Some(92.0));

    let objects = view.objects().unwrap();
    for (key, value) in objects.iter() {
        assert_eq!(key + 1, value.i32());
    }
}

#[test]
fn benchmark_compress_roundtrip_regression() {
    let check = |raw: &[u8]| {
        let mut compressed = Vec::new();
        compress_into(raw, &mut compressed);
        assert!(!compressed.is_empty());
        let mut restored = Vec::new();
        decompress_into(&compressed, &mut restored).unwrap();
        assert_eq!(restored, raw);
    };
    for name in ["test.pb", "test.pc"] {
        let raw = load_fixture_bytes(name);
        check(&raw);
    }
    #[cfg(protocache_test_has_flatbuffers_generated)]
    check(flatbuffers_fixture_bytes());
    #[cfg(protocache_test_has_fory_generated)]
    {
        let raw = load_fixture_bytes("test.fr");
        check(&raw);
    }
}

#[test]
fn benchmark_generated_alias_matches_cpp_shape() {
    let schema_path = fixture_root().join("proto/test.proto");
    let json_path = fixture_root().join("json/test-alias.json");
    let pool = load_reflect_descriptor_pool_from_proto_file(&schema_path).unwrap();
    let descriptor = pool.get_message_by_name("test.Main").unwrap();
    let root = load_json(&json_path, descriptor.clone()).unwrap();
    let words = serialize_dynamic(&root).unwrap();
    assert_eq!(words.len(), 17);
    assert_eq!(words[5], 0x0d);
    assert_eq!(words[6], 1, "alias words: {words:?}");
    assert_eq!(words[7], 1, "alias words: {words:?}");
}

#[test]
fn benchmark_generated_ex_alias_matches_cpp_shape() {
    let mut root = pcrs_generated::MainMutable::New();
    *root.object().i32() = 0;
    root.matrix().push(pcrs_generated::Vec1DMutable::new());
    root.matrix().push(pcrs_generated::Vec1DMutable::new());
    let mut row = pcrs_generated::Vec1DMutable::new();
    row.push(1.0);
    row.push(1.0);
    row.push(1.0);
    root.matrix().push(row);
    let words = root.SerializeWords().unwrap();
    assert_eq!(words.len(), 12);
    assert_eq!(words[4], 0x0d);
    assert_eq!(words[5], 1);
    assert_eq!(words[6], 1);
}

#[test]
fn benchmark_generated_ex_tiny_matches_cpp() {
    let root = pcrs_generated::MainMutable::New();
    assert_eq!(root.SerializeWords().unwrap().len(), 1);
}

#[test]
fn benchmark_generated_string_key_map_stress_matches_cpp() {
    let mut root = pcrs_generated::MainMutable::New();
    for i in 0..64 {
        let key = format!("very_long_key_prefix_to_disable_sso_{i:03}");
        let mut value = pcrs_generated::ArrayMutable::new();
        value.push(i as f32);
        value.push(i as f32 + 0.5);
        assert!(root.arrays().insert(key, value).is_none());
    }
    let words = root.SerializeWords().unwrap();
    let view = pcrs_generated::test::Main::FromWords(&words).unwrap();
    let arrays = view.arrays().unwrap().entries();
    for i in 0..64 {
        let key = format!("very_long_key_prefix_to_disable_sso_{i:03}");
        let values = arrays.find_str(&key).unwrap().1.values();
        assert_eq!(values.len(), 2);
        assert_eq!(values.get(0), Some(i as f32));
        assert_eq!(values.get(1), Some(i as f32 + 0.5));
    }
}

#[test]
fn benchmark_reflection_matches_cpp_contract() {
    let reflect_path = fixture_root().join("proto/reflect-test.proto");
    let test_path = fixture_root().join("proto/test.proto");
    let mut reflect_pool = PcDescriptorPool::default();
    reflect_pool.register(&parse_proto_file(&reflect_path).unwrap()).unwrap();
    let root = reflect_pool.find("test.Main").unwrap();
    assert_eq!(root.tags.len(), 1);
    assert!(root.tags.contains_key("test_b"));

    let mut pool = PcDescriptorPool::default();
    pool.register(&parse_proto_file(&test_path).unwrap()).unwrap();
    let root = pool.find("test.Main").unwrap();
    let f64 = root.fields.get("f64").unwrap();
    assert!(!f64.repeated);
    assert_eq!(f64.value, FieldType::Double);

    let mode = root.fields.get("mode").unwrap();
    assert_eq!(mode.value, FieldType::Enum);
    let object = root.fields.get("object").unwrap();
    let object_desc = pool.find(&object.value_type).unwrap();
    assert!(!object_desc.is_alias());
    assert_eq!(object_desc.fields.get("flag").unwrap().value, FieldType::Bool);

    let arrays = root.fields.get("arrays").unwrap();
    let arrays_desc = pool.find(&arrays.value_type).unwrap();
    assert!(arrays_desc.is_alias());
    assert!(arrays_desc.alias.is_map());
}

#[test]
fn benchmark_protobuf_to_dynamic_to_protocache_matches_fixture() {
    let protobuf_raw = load_fixture_bytes("test.pb");
    let words = load_fixture_words();
    let schema_path = fixture_root().join("proto/test.proto");
    let pool = load_reflect_descriptor_pool_from_proto_file(&schema_path).unwrap();
    let descriptor = pool.get_message_by_name("test.Main").unwrap();
    let prost_root = pb::Main::decode(protobuf_raw.as_slice()).unwrap();
    let mut dynamic = DynamicMessage::new(descriptor);
    dynamic.transcode_from(&prost_root).unwrap();

    let encoded = serialize_dynamic(&dynamic).unwrap();
    assert_eq!(encoded.len(), words.len());

    let mut actual = Junk::default();
    traverse_pc_main(MessageView::new(&encoded).unwrap(), &mut actual).unwrap();

    let mut expected = Junk::default();
    traverse_pc_main(MessageView::new(&words).unwrap(), &mut expected).unwrap();

    assert_eq!(actual.fuse(), expected.fuse());
}

#[test]
fn benchmark_big_object_matches_cpp() {
    let mut proto = String::from("syntax = \"proto3\";\nmessage Object {\n");
    for i in 1..=1000 {
        proto.push_str(&format!("  int32 i{i} = {i};\n"));
    }
    proto.push_str("}\n");

    let file = parse_proto(&proto, "big-object.proto").unwrap();
    let mut pc_pool = PcDescriptorPool::default();
    pc_pool.register(&file).unwrap();
    let object = pc_pool.find("Object").unwrap();
    for i in 1..=1000 {
        assert_eq!(object.fields.get(&format!("i{i}")).unwrap().id, i - 1);
    }

    let reflect_pool = ReflectDescriptorPool::from_file_descriptor_set(FileDescriptorSet { file: vec![file] }).unwrap();
    let descriptor = reflect_pool.get_message_by_name("Object").unwrap();
    let mut message = DynamicMessage::new(descriptor.clone());
    for i in 1..=1000 {
        message.set_field_by_name(&format!("i{i}"), ReflectValue::I32(i));
    }

    let words = serialize_dynamic(&message).unwrap();
    let view = MessageView::new(&words).unwrap();
    for i in 1..=1000 {
        assert_eq!(view.scalar::<i32>(i - 1), Some(i as i32));
    }
}

#[test]
fn benchmark_extension_utils_failure_paths_match_cpp() {
    assert!(parse_proto("syntax = \"proto3\"; message {", "broken.proto").is_err());
    assert!(parse_proto_file(fixture_root().join("proto/missing-file.proto")).is_err());

    let schema_path = fixture_root().join("proto/test.proto");
    let pool = load_reflect_descriptor_pool_from_proto_file(&schema_path).unwrap();
    let descriptor = pool.get_message_by_name("test.Main").unwrap();
    assert!(matches!(
        load_json(fixture_root().join("json/missing-file.json"), descriptor.clone()),
        Err(JsonError::Io(_))
    ));

    let bad_json = unique_temp_json_path("bad-json");
    fs::write(&bad_json, "{this is not valid json}").unwrap();
    assert!(matches!(load_json(&bad_json, descriptor), Err(JsonError::Json(_))));
    fs::remove_file(bad_json).unwrap();
}

#[test]
fn benchmark_serialize_rejects_unsupported_shapes_match_cpp() {
    let empty_file = parse_proto(
        "syntax = \"proto3\";\npackage cov;\nmessage Empty {}\n",
        "empty.proto",
    )
    .unwrap();
    let mut pc_pool = PcDescriptorPool::default();
    let err = pc_pool.register(&empty_file).unwrap_err();
    assert!(matches!(err, RegisterError::EmptyMessage { .. }));

    let bad_map_file = parse_proto(
        "syntax = \"proto3\";\npackage cov;\nmessage BadMap {\n  map<bool, int32> m = 1;\n}\n",
        "bad-map.proto",
    )
    .unwrap();
    let mut pc_pool = PcDescriptorPool::default();
    let err = pc_pool.register(&bad_map_file).unwrap_err();
    assert!(matches!(err, RegisterError::UnsupportedMapKeyType { .. }));

    let reflect_pool = ReflectDescriptorPool::from_file_descriptor_set(FileDescriptorSet { file: vec![bad_map_file] }).unwrap();
    let descriptor = reflect_pool.get_message_by_name("cov.BadMap").unwrap();
    let mut message = DynamicMessage::new(descriptor);
    message.set_field_by_name(
        "m",
        ReflectValue::Map(HashMap::from([(ReflectMapKey::Bool(true), ReflectValue::I32(7))])),
    );
    assert!(serialize_dynamic(&message).is_err());
}

#[test]
fn generated_partly_serialization_matches_fixture() {
    let words = load_fixture_words();
    let encoded = serialize_partly(&words);
    assert_eq!(present_fields(&encoded), present_fields(&words));
    let left = MessageView::new(&encoded).unwrap();
    let right = MessageView::new(&words).unwrap();
    let left_lens = present_fields(&encoded)
        .into_iter()
        .map(|id| (id, field_detect_len(left, id)))
        .collect::<Vec<_>>();
    let right_lens = present_fields(&words)
        .into_iter()
        .map(|id| (id, field_detect_len(right, id)))
        .collect::<Vec<_>>();
    assert_eq!(left_lens, right_lens);
    let object_fields = [6usize, 7, 10, 11, 12, 13, 15, 16, 17, 18, 25, 26, 27, 28, 29];
    let left_pos = object_fields
        .into_iter()
        .map(|id| (id, field_object_pos(&encoded, left, id)))
        .collect::<Vec<_>>();
    let right_pos = object_fields
        .into_iter()
        .map(|id| (id, field_object_pos(&words, right, id)))
        .collect::<Vec<_>>();
    let left_widths = present_fields(&encoded)
        .into_iter()
        .map(|id| (id, field_width(left, id)))
        .collect::<Vec<_>>();
    let right_widths = present_fields(&words)
        .into_iter()
        .map(|id| (id, field_width(right, id)))
        .collect::<Vec<_>>();
    assert_eq!(left_widths, right_widths);
    assert_eq!(left_pos, right_pos);
    assert_eq!(encoded.len(), words.len());
    assert_eq!(encoded, words);
}

#[test]
fn dynamic_container_workloads_match_protobuf_traversal() {
    let schema = fixture_root().join("proto/test.proto");
    let root = load_benchmark_dynamic_message(protobuf_fixture_bytes(), &schema).unwrap();
    for maps in [false, true] {
        let dynamic = container_workload(&root, maps);
        let protobuf = pb::Main::decode(dynamic.encode_to_vec().as_slice()).unwrap();
        let mut expected = Junk::default();
        traverse_pb_main(&protobuf, &mut expected);
        let words = serialize_dynamic(&dynamic).unwrap();
        let mut actual = Junk::default();
        traverse_pc_main(MessageView::new(&words).unwrap(), &mut actual).unwrap();
        assert_eq!(actual.fuse(), expected.fuse(), "maps={maps}");
    }
}

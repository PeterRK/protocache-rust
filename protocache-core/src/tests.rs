use crate::{
    ArrayView, Buffer, MapView, MessageView, StringView, Unit,
    build_perfect_hash_index_with_positions, detect_array_with, detect_map_with, detect_slice_end,
    serialize_map, serialize_scalar, serialize_str,
};

#[test]
fn reads_scalar_message() {
    let words = [0x0000_0100u32, 42u32];
    let view = MessageView::new(&words).unwrap();
    assert_eq!(view.scalar::<i32>(0), Some(42));
    assert!(view.scalar::<i32>(1).is_none());
}

#[test]
fn reads_inline_string() {
    let words = [0x0000_0100u32, 0x0069_6808u32];
    let view = MessageView::new(&words).unwrap();
    let string = view.string(0).unwrap();
    assert_eq!(string.as_str(), Some("hi"));
}

#[test]
fn empty_views_are_rejected() {
    assert!(MessageView::new(&[]).is_none());
    assert!(StringView::new(&[]).is_none());
    assert!(ArrayView::new(&[]).is_none());
    assert!(MapView::new(&[]).is_none());
}

#[test]
fn invalid_map_header_is_rejected() {
    let words = [0u32];
    assert!(MapView::new(&words).is_none());
}

#[test]
fn detect_returns_precise_object_ranges() {
    let words = [0x0000_0100u32, 0x0069_6808u32];
    let root = MessageView::new(&words).unwrap();

    assert_eq!(MessageView::detect(&words).unwrap(), &words[..2]);
    assert_eq!(
        root.field(0).unwrap().detect_scalar().unwrap(),
        &words[1..2]
    );

    let string_msg = [0x0000_0100u32, 0x0000_0007u32, 0x0069_6808u32];
    let root = MessageView::new(&string_msg).unwrap();
    assert_eq!(
        root.field(0).unwrap().detect_string().unwrap(),
        &string_msg[2..3]
    );
    assert_eq!(
        StringView::detect(&string_msg[2..]).unwrap(),
        &string_msg[2..3]
    );
}

#[test]
fn detect_array_stops_after_last_extending_item() {
    let mut buffer = Buffer::new();
    let short = serialize_str("abc", &mut buffer).unwrap();
    let long = serialize_str("this string spills past the array body", &mut buffer).unwrap();
    let _array = crate::serialize_array(&[short, long], &mut buffer).unwrap();
    let words = buffer.view();

    let mut calls = 0usize;
    let detected = detect_array_with(words, |field| {
        calls += 1;
        field.detect_string()
    })
    .unwrap();

    assert!(detected.len() > ArrayView::detect_len(words).unwrap());
    assert_eq!(calls, 1);
}

#[test]
fn detect_map_stops_after_last_extending_value() {
    let keys_src = vec![b"k0".to_vec(), b"k1".to_vec()];
    let (index, positions) = build_perfect_hash_index_with_positions(&keys_src).unwrap();

    let mut buffer = Buffer::new();
    let key0 = serialize_str("k0", &mut buffer).unwrap();
    let key1 = serialize_str("k1", &mut buffer).unwrap();
    let short = serialize_str("x", &mut buffer).unwrap();
    let long = serialize_str("this map value spills past the map body", &mut buffer).unwrap();

    let last = positions
        .iter()
        .enumerate()
        .max_by_key(|(_, pos)| *pos)
        .map(|(idx, _)| idx)
        .unwrap();

    let mut keys = vec![Unit::empty(); 2];
    let mut values = vec![Unit::empty(); 2];
    keys[positions[0]] = key0;
    keys[positions[1]] = key1;
    values[positions[last]] = long;
    values[positions[1 - last]] = short;

    let _map = serialize_map(&index, &keys, &values, &mut buffer).unwrap();
    let words = buffer.view();

    let mut key_calls = 0usize;
    let mut value_calls = 0usize;
    let detected = detect_map_with(
        words,
        |field| {
            key_calls += 1;
            field.detect_string()
        },
        |field| {
            value_calls += 1;
            field.detect_string()
        },
    )
    .unwrap();

    assert!(detected.len() > MapView::detect_len(words).unwrap());
    assert_eq!(value_calls, 1);
    assert_eq!(key_calls, 0);
}

#[test]
fn detect_helpers_reject_slices_from_another_allocation() {
    let foreign = [123u32];
    let words = [10u32, 20, 30];
    let mut end = 1usize;
    assert!(detect_slice_end(&words, &foreign, &mut end).is_none());
    assert_eq!(end, 1);

    let mut array_buffer = Buffer::new();
    let item = serialize_scalar::<u32>(7);
    let _array = crate::serialize_array(&[item], &mut array_buffer).unwrap();
    assert!(detect_array_with(array_buffer.view(), |_| Some(&foreign)).is_none());

    let keys_src = [b"key".as_slice()];
    let (index, _) = build_perfect_hash_index_with_positions(&keys_src).unwrap();
    let mut map_buffer = Buffer::new();
    let key = serialize_str("key", &mut map_buffer).unwrap();
    let value = serialize_scalar::<u32>(9);
    let _map = serialize_map(&index, &[key], &[value], &mut map_buffer).unwrap();
    assert!(detect_map_with(map_buffer.view(), |_| Some(&foreign), |_| Some(&foreign),).is_none());
}

#[test]
fn detect_slice_end_rejects_a_sibling_slice_outside_words() {
    let backing = [1u32, 2, 3, 4];
    let words = &backing[2..];
    let detected = &backing[..1];
    let mut end = 0usize;
    assert!(detect_slice_end(words, detected, &mut end).is_none());
    assert_eq!(end, 0);
}

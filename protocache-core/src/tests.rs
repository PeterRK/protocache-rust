use crate::{ArrayView, MapView, MessageView, StringView, ViewArray, ViewMap};

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
fn finds_map_entries_in_cpp_fixture() {
    let bytes = include_bytes!("../../tests/fixtures/benchmark/test.pc");
    let words = bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap()))
        .collect::<Vec<_>>();
    let root = MessageView::new(&words).unwrap();

    let index = root.map(25).unwrap();
    assert_eq!(
        index.find_str("abc-1").unwrap().value().scalar::<i32>(),
        Some(1)
    );
    assert_eq!(
        index.find_str("abc-2").unwrap().value().scalar::<i32>(),
        Some(2)
    );
    assert!(index.find_str("abc-3").is_none());

    let objects = root.map(26).unwrap();
    let one = objects.find_scalar(1i32).unwrap();
    assert_eq!(one.key().scalar::<i32>(), Some(1));
    let object = one.value().message().unwrap();
    assert_eq!(object.scalar::<i32>(0), Some(1));
    assert!(objects.find_scalar(5i32).is_none());
}

#[test]
fn view_array_decodes_strings() {
    let bytes = include_bytes!("../../tests/fixtures/benchmark/test.pc");
    let words = bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap()))
        .collect::<Vec<_>>();
    let root = MessageView::new(&words).unwrap();
    let array = ViewArray::<StringView<'_>>::new(root.array(13).unwrap());
    assert_eq!(array.len(), 10);
    assert_eq!(array.get(0).unwrap().as_str(), Some("abc"));
    assert_eq!(array.get(1).unwrap().as_str(), Some("apple"));
}

#[test]
fn view_map_decodes_entries() {
    let bytes = include_bytes!("../../tests/fixtures/benchmark/test.pc");
    let words = bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap()))
        .collect::<Vec<_>>();
    let root = MessageView::new(&words).unwrap();

    let index = ViewMap::<StringView<'_>, i32>::new(root.map(25).unwrap());
    assert_eq!(index.len(), 6);
    let (key, value) = index.find_str("abc-1").unwrap();
    assert_eq!(key.as_str(), Some("abc-1"));
    assert_eq!(value, 1);

    let objects = ViewMap::<i32, MessageView<'_>>::new(root.map(26).unwrap());
    let (key, value) = objects.find_scalar(1).unwrap();
    assert_eq!(key, 1);
    assert_eq!(value.scalar::<i32>(0), Some(1));
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
fn corrupt_message_header_is_rejected() {
    let bytes = include_bytes!("../../tests/fixtures/benchmark/test.pc");
    let mut words = bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap()))
        .collect::<Vec<_>>();
    words[0] = u32::MAX;
    assert!(MessageView::new(&words).is_none());
}

#[test]
fn detect_returns_precise_object_ranges() {
    let words = [0x0000_0100u32, 0x0069_6808u32];
    let root = MessageView::new(&words).unwrap();

    assert_eq!(MessageView::detect(&words).unwrap(), &words[..2]);
    assert_eq!(root.field(0).unwrap().detect_scalar().unwrap(), &words[1..2]);

    let string_msg = [0x0000_0100u32, 0x0000_0007u32, 0x0069_6808u32];
    let root = MessageView::new(&string_msg).unwrap();
    assert_eq!(root.field(0).unwrap().detect_string().unwrap(), &string_msg[2..3]);
    assert_eq!(StringView::detect(&string_msg[2..]).unwrap(), &string_msg[2..3]);
}

#[test]
fn detect_handles_fixture_array_and_map_payloads() {
    let bytes = include_bytes!("../../tests/fixtures/benchmark/test.pc");
    let words = bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap()))
        .collect::<Vec<_>>();
    let root = MessageView::new(&words).unwrap();

    let strv = root.field(13).unwrap().detect_array().unwrap();
    assert_eq!(ArrayView::detect(strv).unwrap(), strv);
    assert_eq!(ArrayView::detect_len(strv).unwrap(), strv.len());

    let index = root.field(25).unwrap().detect_map().unwrap();
    assert_eq!(MapView::detect(index).unwrap(), index);
}

use crate::{ArrayView, MapView, MessageView, StringView};

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
    assert_eq!(root.field(0).unwrap().detect_scalar().unwrap(), &words[1..2]);

    let string_msg = [0x0000_0100u32, 0x0000_0007u32, 0x0069_6808u32];
    let root = MessageView::new(&string_msg).unwrap();
    assert_eq!(root.field(0).unwrap().detect_string().unwrap(), &string_msg[2..3]);
    assert_eq!(StringView::detect(&string_msg[2..]).unwrap(), &string_msg[2..3]);
}

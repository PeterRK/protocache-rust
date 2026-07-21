use protocache_core::{Buffer, MessageView, serialize_message, serialize_scalar, serialize_str};

fn main() {
    let mut buffer = Buffer::new();
    let name = serialize_str("Ada", &mut buffer).expect("name is encodable");
    let mut fields = [serialize_scalar(42_i32), name];
    serialize_message(&mut fields, &mut buffer).expect("message is encodable");

    let view = MessageView::new(buffer.view()).expect("valid ProtoCache message");
    let id = view.scalar::<i32>(0).expect("id field");
    let name = view
        .string(1)
        .and_then(|value| value.as_str())
        .expect("UTF-8 name field");

    println!("id={id}, name={name}");
}

#![allow(
    non_snake_case,
    non_camel_case_types,
    dead_code,
    unused_imports,
    unused_variables
)]
// Inclusion under an arbitrary parent module also checks relative type paths.
mod bindings {
    include!("shared.pc.rs");
    include!("global.pc.rs");
    include!("names.pc.rs");
    include!("shared.pc-ex.rs");
    include!("global.pc-ex.rs");
    include!("names.pc-ex.rs");
}
use bindings::*;
use protocache_core::{Buffer, MutableField};

fn main() {
    let _ = PlainMutable_2::New();
    let _ = FieldView_2Mutable::New();
    let mut root = RootMutable::New();
    *root.child().name() = "external".into();
    let mut object = shared_types_CommonMutable::New();
    *object.name() = "array".into();
    root.children().push(object);
    let mut object = shared_types_CommonMutable::New();
    *object.name() = "map".into();
    root.objects().insert("key".into(), object);
    root.flags().push(true);
    root.flags().push(false);
    let mut object = shared_types_CommonMutable::New();
    *object.name() = "alias map".into();
    root.entries().insert("alias".into(), object);
    *root.plain().value() = 17;
    let mut local = app_inner_CommonMutable::New();
    *local.value() = 42;
    let mut buffer = Buffer::new();
    let words = root.SerializeIntoBuffer(&mut buffer).unwrap();
    let view = app::inner::Root::FromWords(words).unwrap();
    let child: shared::types::Common<'_> = view.child().unwrap();
    assert_eq!(child.name(), Some("external"));
    assert_eq!(
        view.children().unwrap().get(0).unwrap().name(),
        Some("array")
    );
    assert_eq!(
        view.objects().unwrap().find_str("key").unwrap().1.name(),
        Some("map")
    );
    assert!(view.flags().unwrap().values().get(0).unwrap());
    assert!(!view.flags().unwrap().values().get(1).unwrap());
    assert_eq!(
        view.entries()
            .unwrap()
            .entries()
            .find_str("alias")
            .unwrap()
            .1
            .name(),
        Some("alias map")
    );
    assert_eq!(view.plain().unwrap().value(), 17);
    assert_eq!(app::inner::Root::Detect(words).unwrap(), words);
    let mut borrowed = RootMutable::FromWords(words).unwrap();
    *borrowed.child().name() = "changed".into();
    assert!(borrowed.is_dirty());
    let changed = borrowed.SerializeWords().unwrap();
    let changed = app::inner::Root::FromWords(&changed).unwrap();
    assert_eq!(changed.child().unwrap().name(), Some("changed"));
    assert_eq!(
        changed.objects().unwrap().find_str("key").unwrap().1.name(),
        Some("map")
    );

    let mut a = AMutable::New();
    a.items().push(1);
    *a.child().value() = 7;
    let words = a.SerializeWords().unwrap();
    let view = app::inner::A::FromWords(&words).unwrap();
    let _: app::inner::A_Items<'_> = view.items().unwrap();
    let _: app::inner::A_B<'_> = view.child().unwrap();
    assert_eq!(view.items().unwrap().values().get(0), Some(1));
    let mut b = BMutable::New();
    b.items().push(2);
    let words = b.SerializeWords().unwrap();
    let _: app::inner::B_Items<'_> = app::inner::B::FromWords(&words).unwrap().items().unwrap();
    let _ = app::inner::A_Mode::ONE;
    let _ = app::inner::B_Mode::TWO;
    let _ = AB_2Mutable::New();

    let empty = NodeMutable::New();
    let words = empty.SerializeWords().unwrap();
    assert!(
        app::inner::Node::FromWords(&words)
            .unwrap()
            .child()
            .is_none()
    );
    let mut node = NodeMutable::New();
    *node.child().child().value() = 99;
    node.children().push(NodeMutable::New());
    node.objects().insert("empty".into(), NodeMutable::New());
    let words = node.SerializeWords().unwrap();
    let view = app::inner::Node::FromWords(&words).unwrap();
    assert_eq!(view.child().unwrap().child().unwrap().value(), 99);
    assert_eq!(view.children().unwrap().len(), 1);
    assert_eq!(
        view.objects().unwrap().find_str("empty").unwrap().1.value(),
        0
    );
    assert_eq!(app::inner::Node::Detect(&words).unwrap(), words.as_slice());
    let mut node = NodeMutable::FromWords(&words).unwrap();
    *node.child().child().value() = 100;
    let words = node.SerializeWords().unwrap();
    assert_eq!(
        app::inner::Node::FromWords(&words)
            .unwrap()
            .child()
            .unwrap()
            .child()
            .unwrap()
            .value(),
        100
    );

    let mut cycle = CyclicAMutable::New();
    *cycle.peer().peer().value() = 23;
    let clone = cycle.clone();
    *cycle.peer().peer().value() = 24;
    let words = clone.SerializeWords().unwrap();
    let view = app::inner::CyclicA::FromWords(&words).unwrap();
    assert_eq!(view.peer().unwrap().peer().unwrap().value(), 23);
    assert_eq!(
        app::inner::CyclicA::Detect(&words).unwrap(),
        words.as_slice()
    );
    let mut restored = CyclicAMutable::FromWords(&words).unwrap();
    *restored.peer().peer().value() = 25;
    let words = restored.SerializeWords().unwrap();
    assert_eq!(
        app::inner::CyclicA::FromWords(&words)
            .unwrap()
            .peer()
            .unwrap()
            .peer()
            .unwrap()
            .value(),
        25
    );
}

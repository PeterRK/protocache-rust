//! Schema reflection APIs matching `extension/reflection.h`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use prost_types::{
    DescriptorProto, EnumDescriptorProto, FieldDescriptorProto, FileDescriptorProto,
    MessageOptions, UninterpretedOption,
    field_descriptor_proto::{Label, Type},
};

#[cfg(test)]
pub(crate) fn build_descriptor_pool(
    files: &[FileDescriptorProto],
) -> Result<DescriptorPool, RegisterError> {
    let mut pool = DescriptorPool::default();
    for file in files {
        pool.register(file)?;
    }
    Ok(pool)
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldType {
    None = 0,
    Message,
    Bytes,
    String,
    Double,
    Float,
    Uint64,
    Uint32,
    Int64,
    Int32,
    Bool,
    Enum,
    Unknown = 255,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Field {
    pub id: usize,
    pub repeated: bool,
    pub key: FieldType,
    pub value: FieldType,
    pub value_type: String,
    pub tags: HashMap<String, String>,
}

impl Field {
    pub fn is_map(&self) -> bool {
        self.key != FieldType::None
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Descriptor {
    pub alias: Field,
    pub fields: HashMap<String, Field>,
    pub tags: HashMap<String, String>,
}

impl Default for Descriptor {
    fn default() -> Self {
        Self {
            alias: empty_field(),
            fields: HashMap::new(),
            tags: HashMap::new(),
        }
    }
}

impl Descriptor {
    pub fn is_alias(&self) -> bool {
        self.alias.value != FieldType::None
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegisterError {
    DuplicateDescriptor { name: String },
    EmptyMessage { name: String },
    InvalidAlias { name: String },
    InvalidFieldNumber { message: String, field: String, number: i32 },
    DuplicateFieldId { message: String, id: usize },
    OverlySparseFieldIds { name: String, max_field_number: i32, field_count: usize },
    UnsupportedFieldType { message: String, field: String },
    UnsupportedMapKeyType { message: String, field: String, key: FieldType },
    UnknownType { message: String, field: String, type_name: String },
}

#[derive(Default)]
pub struct DescriptorPool {
    enums: HashSet<String>,
    enum_values: HashMap<String, BTreeMap<String, i32>>,
    descriptors: HashMap<String, Descriptor>,
}

impl DescriptorPool {
    pub fn register(&mut self, file: &FileDescriptorProto) -> Result<(), RegisterError> {
        let package = file.package();
        for item in &file.enum_type {
            if !is_deprecated_enum(item) {
                let name = full_name(package, item.name());
                self.enums.insert(name.clone());
                self.enum_values.insert(name, collect_enum_values(item));
            }
        }
        for message in &file.message_type {
            if is_deprecated_message(message.options.as_ref()) {
                continue;
            }
            self.register_message(package, message)?;
        }

        let names = self.descriptors.keys().cloned().collect::<Vec<_>>();
        for name in names {
            let mut descriptor = self.descriptors.remove(&name).unwrap();
            self.resolve_descriptor_types(&name, &mut descriptor)?;
            self.descriptors.insert(name, descriptor);
        }
        Ok(())
    }

    pub fn find(&self, full_name: &str) -> Option<&Descriptor> {
        self.descriptors.get(full_name)
    }

    #[cfg(test)]
    pub fn find_enum_number(&self, enum_name: &str, variant: &str) -> Option<i32> {
        self.enum_values.get(enum_name)?.get(variant).copied()
    }

    fn register_message(
        &mut self,
        namespace: &str,
        message: &DescriptorProto,
    ) -> Result<(), RegisterError> {
        let message_name = full_name(namespace, message.name());

        for item in &message.enum_type {
            if !is_deprecated_enum(item) {
                let name = full_name(&message_name, item.name());
                self.enums.insert(name.clone());
                self.enum_values.insert(name, collect_enum_values(item));
            }
        }

        let mut map_entries = HashMap::new();
        for nested in &message.nested_type {
            if is_deprecated_message(nested.options.as_ref()) {
                continue;
            }
            if is_map_entry(nested.options.as_ref()) {
                register_map_entry_names(&mut map_entries, &message_name, nested);
                continue;
            }
            self.register_message(&message_name, nested)?;
        }

        let live_fields = message
            .field
            .iter()
            .filter(|field| !is_deprecated_field(field))
            .collect::<Vec<_>>();
        if message.field.is_empty() {
            return Err(RegisterError::EmptyMessage { name: message_name });
        }
        validate_field_shape(&message_name, &message.field)?;

        let mut descriptor = Descriptor {
            alias: empty_field(),
            fields: HashMap::new(),
            tags: collect_tags(
                message
                    .options
                    .as_ref()
                    .map(|opts| opts.uninterpreted_option.as_slice()),
            ),
        };

        if live_fields.len() == 1 && live_fields[0].name() == "_" {
            let field = live_fields[0];
            if field.label() != Label::Repeated {
                return Err(RegisterError::InvalidAlias { name: message_name });
            }
            descriptor.alias = self.convert_field(&message_name, field, &map_entries)?;
        } else {
            let mut used_ids = BTreeSet::new();
            for field in live_fields {
                let number = field.number();
                if number <= 0 {
                    return Err(RegisterError::InvalidFieldNumber {
                        message: message_name.clone(),
                        field: field.name().to_owned(),
                        number,
                    });
                }
                let id = (number - 1) as usize;
                if !used_ids.insert(id) {
                    return Err(RegisterError::DuplicateFieldId {
                        message: message_name.clone(),
                        id,
                    });
                }

                let mut converted = self.convert_field(&message_name, field, &map_entries)?;
                converted.id = id;
                descriptor.fields.insert(field.name().to_owned(), converted);
            }
        }

        if self
            .descriptors
            .insert(message_name.clone(), descriptor)
            .is_some()
        {
            return Err(RegisterError::DuplicateDescriptor { name: message_name });
        }
        Ok(())
    }

    fn convert_field(
        &self,
        message_name: &str,
        source: &FieldDescriptorProto,
        map_entries: &HashMap<String, DescriptorProto>,
    ) -> Result<Field, RegisterError> {
        let repeated = source.label() == Label::Repeated;
        let mut value = convert_type(source).ok_or_else(|| RegisterError::UnsupportedFieldType {
            message: message_name.to_owned(),
            field: source.name().to_owned(),
        })?;

        let mut key = FieldType::None;
        let mut value_type = String::new();
        if matches!(value, FieldType::Message | FieldType::Unknown) {
            let source_type = normalize_type_name(source.type_name());
            if let Some(entry) = map_entries.get(&source_type) {
                let map_key = convert_type(&entry.field[0]).ok_or_else(|| RegisterError::UnsupportedFieldType {
                    message: message_name.to_owned(),
                    field: source.name().to_owned(),
                })?;
                let Some(map_key) = as_key_type(map_key) else {
                    return Err(RegisterError::UnsupportedMapKeyType {
                        message: message_name.to_owned(),
                        field: source.name().to_owned(),
                        key: map_key,
                    });
                };
                let map_value = convert_type(&entry.field[1]).ok_or_else(|| RegisterError::UnsupportedFieldType {
                    message: message_name.to_owned(),
                    field: source.name().to_owned(),
                })?;
                key = map_key;
                value = map_value;
                let nested_type = normalize_type_name(entry.field[1].type_name());
                if !nested_type.is_empty() {
                    value_type = nested_type;
                }
            } else if !source_type.is_empty() {
                value_type = source_type;
            }
        }

        Ok(Field {
            id: 0,
            repeated,
            key,
            value,
            value_type,
            tags: collect_tags(
                source
                    .options
                    .as_ref()
                    .map(|opts| opts.uninterpreted_option.as_slice()),
            ),
        })
    }

    fn resolve_descriptor_types(
        &self,
        full_name: &str,
        descriptor: &mut Descriptor,
    ) -> Result<(), RegisterError> {
        if descriptor.is_alias() {
            self.resolve_field_type(full_name, "_", &mut descriptor.alias)?;
        } else {
            for (field_name, field) in &mut descriptor.fields {
                self.resolve_field_type(full_name, field_name, field)?;
            }
        }
        Ok(())
    }

    fn resolve_field_type(
        &self,
        message_name: &str,
        field_name: &str,
        field: &mut Field,
    ) -> Result<(), RegisterError> {
        if field.value != FieldType::Unknown {
            return Ok(());
        }
        let unresolved = field.value_type.clone();
        let resolved = self.resolve_type_name(message_name, &unresolved).ok_or_else(|| {
            RegisterError::UnknownType {
                message: message_name.to_owned(),
                field: field_name.to_owned(),
                type_name: unresolved.clone(),
            }
        })?;

        if self.enums.contains(&resolved) {
            field.value = FieldType::Enum;
            field.value_type.clear();
        } else {
            field.value = FieldType::Message;
            field.value_type = resolved;
        }
        Ok(())
    }

    fn resolve_type_name(&self, containing_type: &str, type_name: &str) -> Option<String> {
        let normalized = normalize_type_name(type_name);
        if normalized.is_empty() {
            return None;
        }
        if self.is_known_type(&normalized) {
            return Some(normalized);
        }

        let candidate = full_name(containing_type, &normalized);
        if self.is_known_type(&candidate) {
            return Some(candidate);
        }

        let mut scope = containing_type.to_owned();
        while let Some(pos) = scope.rfind('.') {
            scope.truncate(pos);
            let candidate = full_name(&scope, &normalized);
            if self.is_known_type(&candidate) {
                return Some(candidate);
            }
        }
        None
    }

    fn is_known_type(&self, name: &str) -> bool {
        self.enums.contains(name) || self.descriptors.contains_key(name)
    }
}

fn register_map_entry_names(
    map_entries: &mut HashMap<String, DescriptorProto>,
    parent_name: &str,
    nested: &DescriptorProto,
) {
    let full = full_name(parent_name, nested.name());
    let mut names = vec![nested.name().to_owned(), full.clone(), format!(".{full}")];
    if let Some(parent_short) = parent_name.rsplit('.').next() {
        names.push(format!("{parent_short}.{}", nested.name()));
    }
    for name in names {
        map_entries.insert(name, nested.clone());
    }
}

fn full_name(namespace: &str, name: &str) -> String {
    if namespace.is_empty() {
        name.to_owned()
    } else {
        format!("{namespace}.{name}")
    }
}

fn normalize_type_name(type_name: &str) -> String {
    type_name.trim_start_matches('.').to_owned()
}

fn convert_type(field: &FieldDescriptorProto) -> Option<FieldType> {
    if field.r#type.is_none() {
        return Some(FieldType::Unknown);
    }
    match Type::try_from(field.r#type.unwrap_or_default()) {
        Ok(Type::Message) => Some(FieldType::Unknown),
        Ok(Type::Bytes) => Some(FieldType::Bytes),
        Ok(Type::String) => Some(FieldType::String),
        Ok(Type::Double) => Some(FieldType::Double),
        Ok(Type::Float) => Some(FieldType::Float),
        Ok(Type::Fixed64) | Ok(Type::Uint64) => Some(FieldType::Uint64),
        Ok(Type::Fixed32) | Ok(Type::Uint32) => Some(FieldType::Uint32),
        Ok(Type::Sfixed64) | Ok(Type::Sint64) | Ok(Type::Int64) => Some(FieldType::Int64),
        Ok(Type::Sfixed32) | Ok(Type::Sint32) | Ok(Type::Int32) => Some(FieldType::Int32),
        Ok(Type::Bool) => Some(FieldType::Bool),
        Ok(Type::Enum) => Some(FieldType::Unknown),
        _ => None,
    }
}

fn as_key_type(value: FieldType) -> Option<FieldType> {
    match value {
        FieldType::String => Some(FieldType::String),
        FieldType::Uint64 => Some(FieldType::Uint64),
        FieldType::Uint32 => Some(FieldType::Uint32),
        FieldType::Int64 => Some(FieldType::Int64),
        FieldType::Int32 => Some(FieldType::Int32),
        FieldType::None
        | FieldType::Message
        | FieldType::Bytes
        | FieldType::Double
        | FieldType::Float
        | FieldType::Bool
        | FieldType::Enum
        | FieldType::Unknown => None,
    }
}

fn collect_tags(options: Option<&[UninterpretedOption]>) -> HashMap<String, String> {
    let mut tags = HashMap::new();
    for option in options.into_iter().flatten() {
        if option.name.len() == 1 {
            let name = &option.name[0];
            if !name.is_extension {
                if let Some(value) = &option.string_value {
                    tags.insert(
                        name.name_part.clone(),
                        String::from_utf8_lossy(value).into_owned(),
                    );
                }
            }
        }
    }
    tags
}

fn empty_field() -> Field {
    Field {
        id: usize::MAX,
        repeated: false,
        key: FieldType::None,
        value: FieldType::None,
        value_type: String::new(),
        tags: HashMap::new(),
    }
}

fn is_map_entry(options: Option<&MessageOptions>) -> bool {
    options.and_then(|opts| opts.map_entry).unwrap_or(false)
}

fn is_deprecated_message(options: Option<&MessageOptions>) -> bool {
    options.and_then(|opts| opts.deprecated).unwrap_or(false)
}

fn is_deprecated_enum(item: &EnumDescriptorProto) -> bool {
    item.options
        .as_ref()
        .and_then(|opts| opts.deprecated)
        .unwrap_or(false)
}

fn is_deprecated_field(field: &FieldDescriptorProto) -> bool {
    field.options
        .as_ref()
        .and_then(|opts| opts.deprecated)
        .unwrap_or(false)
}

fn collect_enum_values(item: &EnumDescriptorProto) -> BTreeMap<String, i32> {
    item.value
        .iter()
        .filter_map(|value| Some((value.name.clone()?, value.number?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use prost_types::{
        EnumValueDescriptorProto, FieldOptions, FileDescriptorSet, MessageOptions,
        field_descriptor_proto::Label,
    };
    use tempfile::TempDir;

    const TEST_SCHEMA: &str = r#"
        syntax = "proto3";
        package test;

        enum Mode {
            MODE_A = 0;
            MODE_B = 1;
            MODE_C = 2;
        }

        message Small {
            string str = 4;
            int32 i32 = 1;
            bool flag = 2;
        }

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
            int32 i32 = 1;
            string str = 2;
            Small object = 3;
            repeated int32 i32v = 4;
            map<string, int32> index = 5;
            Vec2D matrix = 6;
            ArrMap arrays = 7;
        }
    "#;

    #[test]
    fn registers_targeted_schema_and_resolves_aliases() {
        let mut pool = DescriptorPool::default();
        pool.register(&fixture_file()).unwrap();

        let root = pool.find("test.Main").unwrap();
        assert_eq!(root.tags.get("test_b").map(String::as_str), Some("123"));

        let score = root.fields.get("score").unwrap();
        assert_eq!(score.id, 1);
        assert!(!score.repeated);
        assert_eq!(score.value, FieldType::Double);
        assert_eq!(score.tags.get("mark").map(String::as_str), Some("xyz"));

        let mode = root.fields.get("mode").unwrap();
        assert_eq!(mode.value, FieldType::Enum);
        assert!(mode.value_type.is_empty());
        assert_eq!(pool.find_enum_number("test.Mode", "MODE_C"), Some(2));

        let object = root.fields.get("object").unwrap();
        assert_eq!(object.value, FieldType::Message);
        assert_eq!(object.value_type, "test.Small");
        let object_desc = pool.find(&object.value_type).unwrap();
        assert!(!object_desc.is_alias());
        assert_eq!(object_desc.fields.get("flag").unwrap().value, FieldType::Bool);

        let index = root.fields.get("index").unwrap();
        assert!(index.repeated);
        assert!(index.is_map());
        assert_eq!(index.key, FieldType::String);
        assert_eq!(index.value, FieldType::Int32);

        let matrix = root.fields.get("matrix").unwrap();
        let matrix_desc = pool.find(&matrix.value_type).unwrap();
        let outer_alias = &matrix_desc.alias;
        assert!(outer_alias.repeated);
        assert!(!outer_alias.is_map());
        assert_eq!(outer_alias.value, FieldType::Message);
        let inner_alias_desc = pool.find(&outer_alias.value_type).unwrap();
        let inner_alias = &inner_alias_desc.alias;
        assert!(inner_alias.repeated);
        assert_eq!(inner_alias.value, FieldType::Float);

        let arrays = root.fields.get("arrays").unwrap();
        let arrays_desc = pool.find(&arrays.value_type).unwrap();
        let alias = &arrays_desc.alias;
        assert!(alias.repeated);
        assert!(alias.is_map());
        assert_eq!(alias.key, FieldType::String);
        assert_eq!(alias.value, FieldType::Message);
        let array_desc = pool.find(&alias.value_type).unwrap();
        assert_eq!(array_desc.alias.value, FieldType::Float);
    }

    #[test]
    fn big_object_fields_keep_dense_zero_based_ids() {
        let mut pool = DescriptorPool::default();
        pool.register(&big_object_file(1000)).unwrap();

        let object = pool.find("Object").unwrap();
        for i in 1..=1000 {
            let name = format!("i{i}");
            assert_eq!(object.fields.get(&name).unwrap().id, i - 1);
        }
    }

    #[test]
    fn rejects_invalid_shapes() {
        let mut pool = DescriptorPool::default();
        let err = pool.register(&empty_message_file()).unwrap_err();
        assert!(matches!(err, RegisterError::EmptyMessage { .. }));

        let mut pool = DescriptorPool::default();
        let err = pool.register(&duplicate_id_file()).unwrap_err();
        assert!(matches!(err, RegisterError::DuplicateFieldId { .. }));

        let mut pool = DescriptorPool::default();
        let err = pool.register(&invalid_alias_file()).unwrap_err();
        assert!(matches!(err, RegisterError::InvalidAlias { .. }));

        let mut pool = DescriptorPool::default();
        let err = pool.register(&unknown_type_file()).unwrap_err();
        assert!(matches!(err, RegisterError::UnknownType { .. }));

        let mut pool = DescriptorPool::default();
        let err = pool.register(&unsupported_map_key_file()).unwrap_err();
        assert!(matches!(err, RegisterError::UnsupportedMapKeyType { .. }));

        let mut pool = DescriptorPool::default();
        let err = pool.register(&overly_sparse_file()).unwrap_err();
        assert!(matches!(err, RegisterError::OverlySparseFieldIds { .. }));
    }

    #[test]
    fn registers_descriptor_emitted_by_protoc_for_targeted_schema() {
        let (_dir, path) = write_test_proto();
        let descriptor_set = compile_descriptor_set(path.parent().unwrap().to_path_buf(), "test.proto");
        let file = descriptor_set
            .file
            .into_iter()
            .find(|file| file.name() == "test.proto")
            .unwrap();

        let mut pool = DescriptorPool::default();
        pool.register(&file).unwrap();

        let root = pool.find("test.Main").unwrap();
        assert_eq!(root.fields.get("matrix").unwrap().value, FieldType::Message);
        assert!(pool.find("test.Vec2D").unwrap().is_alias());
        assert!(pool.find("test.ArrMap").unwrap().alias.is_map());
    }

    fn fixture_file() -> FileDescriptorProto {
        FileDescriptorProto {
            package: Some("test".to_owned()),
            enum_type: vec![enum_descriptor("Mode")],
            message_type: vec![
                small_message(),
                vec2d_message(),
                arr_map_message(),
                main_message(),
            ],
            ..FileDescriptorProto::default()
        }
    }

    fn big_object_file(count: usize) -> FileDescriptorProto {
        FileDescriptorProto {
            message_type: vec![DescriptorProto {
                name: Some("Object".to_owned()),
                field: (1..=count)
                    .map(|i| scalar_field(&format!("i{i}"), i as i32, Type::Int32))
                    .collect(),
                ..DescriptorProto::default()
            }],
            ..FileDescriptorProto::default()
        }
    }

    fn empty_message_file() -> FileDescriptorProto {
        FileDescriptorProto {
            message_type: vec![DescriptorProto {
                name: Some("Empty".to_owned()),
                ..DescriptorProto::default()
            }],
            ..FileDescriptorProto::default()
        }
    }

    fn duplicate_id_file() -> FileDescriptorProto {
        FileDescriptorProto {
            message_type: vec![DescriptorProto {
                name: Some("Dup".to_owned()),
                field: vec![
                    scalar_field("a", 1, Type::Int32),
                    scalar_field("b", 1, Type::Int32),
                ],
                ..DescriptorProto::default()
            }],
            ..FileDescriptorProto::default()
        }
    }

    fn invalid_alias_file() -> FileDescriptorProto {
        FileDescriptorProto {
            message_type: vec![DescriptorProto {
                name: Some("Alias".to_owned()),
                field: vec![FieldDescriptorProto {
                    name: Some("_".to_owned()),
                    number: Some(1),
                    label: Some(Label::Optional as i32),
                    r#type: Some(Type::Int32 as i32),
                    ..FieldDescriptorProto::default()
                }],
                ..DescriptorProto::default()
            }],
            ..FileDescriptorProto::default()
        }
    }

    fn unknown_type_file() -> FileDescriptorProto {
        FileDescriptorProto {
            message_type: vec![DescriptorProto {
                name: Some("Root".to_owned()),
                field: vec![FieldDescriptorProto {
                    name: Some("child".to_owned()),
                    number: Some(1),
                    label: Some(Label::Optional as i32),
                    r#type: Some(Type::Message as i32),
                    type_name: Some("Missing".to_owned()),
                    ..FieldDescriptorProto::default()
                }],
                ..DescriptorProto::default()
            }],
            ..FileDescriptorProto::default()
        }
    }

    fn unsupported_map_key_file() -> FileDescriptorProto {
        let map_entry = DescriptorProto {
            name: Some("ByFlagEntry".to_owned()),
            field: vec![
                FieldDescriptorProto {
                    name: Some("key".to_owned()),
                    number: Some(1),
                    label: Some(Label::Optional as i32),
                    r#type: Some(Type::Bool as i32),
                    ..FieldDescriptorProto::default()
                },
                scalar_field("value", 2, Type::Int32),
            ],
            options: Some(MessageOptions {
                map_entry: Some(true),
                ..MessageOptions::default()
            }),
            ..DescriptorProto::default()
        };

        FileDescriptorProto {
            message_type: vec![DescriptorProto {
                name: Some("Root".to_owned()),
                nested_type: vec![map_entry],
                field: vec![FieldDescriptorProto {
                    name: Some("items".to_owned()),
                    number: Some(1),
                    label: Some(Label::Repeated as i32),
                    r#type: Some(Type::Message as i32),
                    type_name: Some("ByFlagEntry".to_owned()),
                    ..FieldDescriptorProto::default()
                }],
                ..DescriptorProto::default()
            }],
            ..FileDescriptorProto::default()
        }
    }

    fn overly_sparse_file() -> FileDescriptorProto {
        FileDescriptorProto {
            message_type: vec![DescriptorProto {
                name: Some("Sparse".to_owned()),
                field: vec![
                    scalar_field("a", 1, Type::Int32),
                    scalar_field("z", 20, Type::Int32),
                ],
                ..DescriptorProto::default()
            }],
            ..FileDescriptorProto::default()
        }
    }

    fn main_message() -> DescriptorProto {
        DescriptorProto {
            name: Some("Main".to_owned()),
            field: vec![
                scalar_field("id", 1, Type::Int32),
                named_field("score", 2, Label::Optional, Type::Double, None, false, &[tag("mark", "xyz")]),
                named_field("mode", 3, Label::Optional, Type::Enum, Some("Mode"), false, &[]),
                named_field("object", 4, Label::Optional, Type::Message, Some("Small"), false, &[]),
                named_field("index", 5, Label::Repeated, Type::Message, Some("Main.IndexEntry"), false, &[]),
                named_field("matrix", 6, Label::Optional, Type::Message, Some("Vec2D"), false, &[]),
                named_field("arrays", 7, Label::Optional, Type::Message, Some("ArrMap"), false, &[]),
            ],
            nested_type: vec![
                map_entry("IndexEntry", Type::String, None, Type::Int32, None),
            ],
            options: Some(MessageOptions {
                uninterpreted_option: vec![tag("test_a", "123"), tag("test_b", "123")],
                ..MessageOptions::default()
            }),
            ..DescriptorProto::default()
        }
    }

    fn small_message() -> DescriptorProto {
        DescriptorProto {
            name: Some("Small".to_owned()),
            field: vec![
                scalar_field("str", 4, Type::String),
                scalar_field("i32", 1, Type::Int32),
                scalar_field("flag", 2, Type::Bool),
                FieldDescriptorProto {
                    name: Some("junk".to_owned()),
                    number: Some(5),
                    label: Some(Label::Optional as i32),
                    r#type: Some(Type::Int64 as i32),
                    options: Some(FieldOptions {
                        deprecated: Some(true),
                        ..FieldOptions::default()
                    }),
                    ..FieldDescriptorProto::default()
                },
            ],
            ..DescriptorProto::default()
        }
    }

    fn vec2d_message() -> DescriptorProto {
        DescriptorProto {
            name: Some("Vec2D".to_owned()),
            field: vec![named_field(
                "_",
                1,
                Label::Repeated,
                Type::Message,
                Some("Vec2D.Vec1D"),
                false,
                &[],
            )],
            nested_type: vec![DescriptorProto {
                name: Some("Vec1D".to_owned()),
                field: vec![named_field("_", 1, Label::Repeated, Type::Float, None, false, &[])],
                ..DescriptorProto::default()
            }],
            ..DescriptorProto::default()
        }
    }

    fn arr_map_message() -> DescriptorProto {
        DescriptorProto {
            name: Some("ArrMap".to_owned()),
            field: vec![named_field(
                "_",
                1,
                Label::Repeated,
                Type::Message,
                Some("ArrMap.Entry"),
                false,
                &[],
            )],
            nested_type: vec![
                DescriptorProto {
                    name: Some("Array".to_owned()),
                    field: vec![named_field("_", 1, Label::Repeated, Type::Float, None, false, &[])],
                    ..DescriptorProto::default()
                },
                map_entry("Entry", Type::String, None, Type::Message, Some("ArrMap.Array")),
            ],
            ..DescriptorProto::default()
        }
    }

    fn enum_descriptor(name: &str) -> EnumDescriptorProto {
        EnumDescriptorProto {
            name: Some(name.to_owned()),
            value: vec![
                EnumValueDescriptorProto {
                    name: Some("MODE_A".to_owned()),
                    number: Some(0),
                    ..EnumValueDescriptorProto::default()
                },
                EnumValueDescriptorProto {
                    name: Some("MODE_B".to_owned()),
                    number: Some(1),
                    ..EnumValueDescriptorProto::default()
                },
                EnumValueDescriptorProto {
                    name: Some("MODE_C".to_owned()),
                    number: Some(2),
                    ..EnumValueDescriptorProto::default()
                },
            ],
            ..EnumDescriptorProto::default()
        }
    }

    fn map_entry(
        name: &str,
        key_type: Type,
        key_type_name: Option<&str>,
        value_type: Type,
        value_type_name: Option<&str>,
    ) -> DescriptorProto {
        DescriptorProto {
            name: Some(name.to_owned()),
            field: vec![
                named_field("key", 1, Label::Optional, key_type, key_type_name, false, &[]),
                named_field("value", 2, Label::Optional, value_type, value_type_name, false, &[]),
            ],
            options: Some(MessageOptions {
                map_entry: Some(true),
                ..MessageOptions::default()
            }),
            ..DescriptorProto::default()
        }
    }

    fn scalar_field(name: &str, number: i32, ty: Type) -> FieldDescriptorProto {
        named_field(name, number, Label::Optional, ty, None, false, &[])
    }

    fn named_field(
        name: &str,
        number: i32,
        label: Label,
        ty: Type,
        type_name: Option<&str>,
        deprecated: bool,
        tags: &[UninterpretedOption],
    ) -> FieldDescriptorProto {
        FieldDescriptorProto {
            name: Some(name.to_owned()),
            number: Some(number),
            label: Some(label as i32),
            r#type: Some(ty as i32),
            type_name: type_name.map(str::to_owned),
            options: if deprecated || !tags.is_empty() {
                Some(FieldOptions {
                    deprecated: deprecated.then_some(true),
                    uninterpreted_option: tags.to_vec(),
                    ..FieldOptions::default()
                })
            } else {
                None
            },
            ..FieldDescriptorProto::default()
        }
    }

    fn tag(name: &str, value: &str) -> UninterpretedOption {
        UninterpretedOption {
            name: vec![prost_types::uninterpreted_option::NamePart {
                name_part: name.to_owned(),
                is_extension: false,
            }],
            string_value: Some(value.as_bytes().to_vec()),
            ..UninterpretedOption::default()
        }
    }

    fn compile_descriptor_set(proto_dir: PathBuf, file_name: &str) -> FileDescriptorSet {
        crate::proto::parse_proto_file_set(proto_dir.join(file_name)).unwrap()
    }

    fn write_test_proto() -> (TempDir, PathBuf) {
        let dir = tempfile::Builder::new()
            .prefix("pcrs-reflect-schema-")
            .tempdir()
            .unwrap();
        let path = dir.path().join("test.proto");
        std::fs::write(&path, TEST_SCHEMA).unwrap();
        (dir, path)
    }
}

fn validate_field_shape(name: &str, fields: &[FieldDescriptorProto]) -> Result<(), RegisterError> {
    let field_count = fields.len();
    if field_count == 0 {
        return Err(RegisterError::EmptyMessage {
            name: name.to_owned(),
        });
    }

    let mut max_field_number = 1;
    for field in fields {
        let number = field.number();
        if number <= 0 {
            return Err(RegisterError::InvalidFieldNumber {
                message: name.to_owned(),
                field: field.name().to_owned(),
                number,
            });
        }
        max_field_number = max_field_number.max(number);
    }

    if max_field_number > (12 + 25 * 255)
        || ((max_field_number as usize).saturating_sub(field_count) > 6
            && (max_field_number as usize) > field_count * 2)
    {
        return Err(RegisterError::OverlySparseFieldIds {
            name: name.to_owned(),
            max_field_number,
            field_count,
        });
    }

    Ok(())
}

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::io::{Read, Write};

use prost::Message;
use protocache_schema::DescriptorPool;
use prost_types::compiler::{CodeGeneratorRequest, CodeGeneratorResponse, code_generator_response};
use prost_types::{
    DescriptorProto, EnumDescriptorProto, FieldDescriptorProto, FileDescriptorProto,
    field_descriptor_proto::{Label, Type},
};

#[derive(Clone)]
enum AliasTarget {
    BoolArray,
    Array,
    Map,
}

#[derive(Default)]
struct Registry {
    rust_types: HashMap<String, String>,
    map_entries: HashMap<String, DescriptorProto>,
    aliases: HashMap<String, AliasTarget>,
}

pub fn run() {
    let mut input = Vec::new();
    if std::io::stdin().read_to_end(&mut input).is_err() {
        emit_error("failed to read CodeGeneratorRequest");
        return;
    }

    let request = match CodeGeneratorRequest::decode(input.as_slice()) {
        Ok(request) => request,
        Err(err) => {
            emit_error(&format!("failed to decode CodeGeneratorRequest: {err}"));
            return;
        }
    };

    if let Err(err) = validate_request(&request) {
        emit_error(&err);
        return;
    }

    let registry = build_registry(&request.proto_file);
    let files: HashSet<_> = request.file_to_generate.iter().cloned().collect();
    let mut response = CodeGeneratorResponse::default();

    for proto in &request.proto_file {
        let proto_name = proto.name.as_deref().unwrap_or_default();
        if !files.contains(proto_name) {
            continue;
        }
        match generate_file(proto, &registry) {
            Ok(content) => response.file.push(code_generator_response::File {
                name: Some(convert_filename(proto_name)),
                insertion_point: None,
                content: Some(content),
                generated_code_info: None,
            }),
            Err(err) => {
                emit_error(&format!("failed to generate {proto_name}: {err}"));
                return;
            }
        }
    }

    let mut output = Vec::new();
    if response.encode(&mut output).is_err() || std::io::stdout().write_all(&output).is_err() {
        emit_error("failed to write CodeGeneratorResponse");
    }
}

fn validate_request(request: &CodeGeneratorRequest) -> Result<(), String> {
    let mut pool = DescriptorPool::default();
    for file in &request.proto_file {
        pool.register(file)
            .map_err(|err| format!("schema validation failed for {}: {err:?}", file.name()))?;
    }
    Ok(())
}

fn emit_error(message: &str) {
    let response = CodeGeneratorResponse {
        error: Some(message.to_owned()),
        supported_features: None,
        file: Vec::new(),
    };
    let mut output = Vec::new();
    let _ = response.encode(&mut output);
    let _ = std::io::stdout().write_all(&output);
}

fn convert_filename(name: &str) -> String {
    match name.rfind('.') {
        Some(pos) => format!("{}.pc.rs", &name[..pos]),
        None => format!("{name}.pc.rs"),
    }
}

fn build_registry(files: &[FileDescriptorProto]) -> Registry {
    let mut registry = Registry::default();
    for file in files {
        let package = file.package();
        for item in &file.enum_type {
            registry
                .rust_types
                .insert(format!("{}.{}", package_prefix(package), item.name()), item.name().to_owned());
        }
        for message in &file.message_type {
            collect_message_types(&mut registry, &package, None, message);
        }
    }
    registry
}

fn collect_message_types(
    registry: &mut Registry,
    package: &str,
    parent: Option<&str>,
    message: &DescriptorProto,
) {
    let full = if let Some(parent) = parent {
        format!("{parent}.{}", message.name())
    } else {
        format!("{}.{}", package_prefix(package), message.name())
    };
    registry
        .rust_types
        .insert(full.clone(), flatten_name(&full, package));

    for nested in &message.nested_type {
        let nested_full = format!("{full}.{}", nested.name());
        if nested.options.as_ref().and_then(|o| o.map_entry).unwrap_or(false) {
            registry.map_entries.insert(nested_full, nested.clone());
            continue;
        }
        collect_message_types(registry, package, Some(&full), nested);
    }

    for item in &message.enum_type {
        registry
            .rust_types
            .insert(format!("{full}.{}", item.name()), flatten_name(&format!("{full}.{}", item.name()), package));
    }

    if is_alias_message(message) {
        let field = &message.field[0];
        let target = if field.label() == Label::Repeated && field.r#type() == Type::Bool {
            AliasTarget::BoolArray
        } else if field.r#type() == Type::Message && registry.map_entries.contains_key(field.type_name()) {
            AliasTarget::Map
        } else {
            AliasTarget::Array
        };
        registry.aliases.insert(full, target);
    }
}

fn generate_file(file: &FileDescriptorProto, registry: &Registry) -> Result<String, String> {
    let mut out = String::new();
    out.push_str("// @generated by protoc-gen-pcrs.\n");

    if !file.package().is_empty() {
        for part in file.package().split('.') {
            writeln!(&mut out, "pub mod {part} {{").unwrap();
        }
        out.push('\n');
        let pad = "    ".repeat(package_depth(file.package()));
        writeln!(
            &mut out,
            "{pad}use protocache_core::{{ArrayView, BoolArray, FieldDecode, FieldView, GeneratedDescriptor, GeneratedMessage, MapView, MessageView, ScalarArray, StringView, ViewArray, ViewMap}};\n"
        )
        .unwrap();
        writeln!(
            &mut out,
            "{pad}use protocache_mutable::{{GeneratedMutableMessage, ListMut, MapMut, MessageMut, MutableError}};\n"
        )
        .unwrap();
    } else {
        out.push_str("use protocache_core::{ArrayView, BoolArray, FieldDecode, FieldView, GeneratedDescriptor, GeneratedMessage, MapView, MessageView, ScalarArray, StringView, ViewArray, ViewMap};\n");
        out.push_str("use protocache_mutable::{GeneratedMutableMessage, ListMut, MapMut, MessageMut, MutableError};\n\n");
    }

    let indent = package_depth(file.package());
    for item in &file.enum_type {
        generate_enum(&mut out, item, indent);
    }
    for message in &file.message_type {
        generate_message(
            &mut out,
            registry,
            message,
            file.package(),
            None,
            indent,
        )?;
    }

    if !file.package().is_empty() {
        for part in file.package().split('.').rev() {
            writeln!(&mut out, "}} // mod {part}").unwrap();
        }
    }
    Ok(out)
}

fn generate_enum(out: &mut String, item: &EnumDescriptorProto, indent: usize) {
    let pad = "    ".repeat(indent);
    writeln!(out, "{pad}#[derive(Clone, Copy, Debug, Eq, PartialEq)]").unwrap();
    writeln!(out, "{pad}#[allow(non_camel_case_types)]").unwrap();
    writeln!(out, "{pad}#[repr(i32)]").unwrap();
    writeln!(out, "{pad}pub enum {} {{", item.name()).unwrap();
    for value in &item.value {
        writeln!(out, "{pad}    {} = {},", value.name(), value.number()).unwrap();
    }
    writeln!(out, "{pad}}}\n").unwrap();
}

fn generate_message(
    out: &mut String,
    registry: &Registry,
    message: &DescriptorProto,
    package: &str,
    parent: Option<&str>,
    indent: usize,
) -> Result<(), String> {
    let full = if let Some(parent) = parent {
        format!("{parent}.{}", message.name())
    } else {
        format!("{}.{}", package_prefix(package), message.name())
    };

    for item in &message.enum_type {
        generate_enum(out, item, indent);
    }
    for nested in &message.nested_type {
        if nested.options.as_ref().and_then(|o| o.map_entry).unwrap_or(false) {
            continue;
        }
        generate_message(out, registry, nested, package, Some(&full), indent)?;
    }

    if is_alias_message(message) {
        generate_alias(out, registry, message, &full, indent)?;
    } else {
        generate_regular_message(out, registry, message, &full, indent)?;
        generate_mutable_message(out, registry, message, &full, indent)?;
    }
    Ok(())
}

fn generate_alias(
    out: &mut String,
    registry: &Registry,
    message: &DescriptorProto,
    full: &str,
    indent: usize,
) -> Result<(), String> {
    let pad = "    ".repeat(indent);
    let rust_name = registry
        .rust_types
        .get(full)
        .ok_or_else(|| format!("missing alias rust name for {full}"))?;
    let target = registry
        .aliases
        .get(full)
        .ok_or_else(|| format!("missing alias target for {full}"))?;

    writeln!(out, "{pad}#[derive(Clone, Copy, Debug)]").unwrap();
    writeln!(out, "{pad}pub struct {rust_name}<'a> {{").unwrap();
    writeln!(out, "{pad}    words: &'a [u32],").unwrap();
    writeln!(out, "{pad}}}\n").unwrap();

    writeln!(out, "{pad}impl<'a> {rust_name}<'a> {{").unwrap();
    writeln!(out, "{pad}    pub fn from_words(words: &'a [u32]) -> Option<Self> {{").unwrap();
    match target {
        AliasTarget::BoolArray => {
            writeln!(out, "{pad}        protocache_core::StringView::new(words)?;").unwrap();
        }
        AliasTarget::Array => {
            writeln!(out, "{pad}        ArrayView::new(words)?;").unwrap();
        }
        AliasTarget::Map => {
            writeln!(out, "{pad}        MapView::new(words)?;").unwrap();
        }
    }
    writeln!(out, "{pad}        Some(Self {{ words }})").unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    match target {
        AliasTarget::BoolArray => {
            writeln!(out, "{pad}    pub fn detect_len(words: &'a [u32]) -> Option<usize> {{").unwrap();
            writeln!(out, "{pad}        protocache_core::StringView::detect_len(words)").unwrap();
            writeln!(out, "{pad}    }}").unwrap();
            writeln!(out, "{pad}    pub fn detect(words: &'a [u32]) -> Option<&'a [u32]> {{").unwrap();
            writeln!(out, "{pad}        protocache_core::StringView::detect(words)").unwrap();
            writeln!(out, "{pad}    }}").unwrap();
        }
        AliasTarget::Array => {
            writeln!(out, "{pad}    pub fn detect_len(words: &'a [u32]) -> Option<usize> {{").unwrap();
            writeln!(out, "{pad}        ArrayView::detect_len(words)").unwrap();
            writeln!(out, "{pad}    }}").unwrap();
            writeln!(out, "{pad}    pub fn detect(words: &'a [u32]) -> Option<&'a [u32]> {{").unwrap();
            writeln!(out, "{pad}        ArrayView::detect(words)").unwrap();
            writeln!(out, "{pad}    }}").unwrap();
        }
        AliasTarget::Map => {
            writeln!(out, "{pad}    pub fn detect_len(words: &'a [u32]) -> Option<usize> {{").unwrap();
            writeln!(out, "{pad}        MapView::detect_len(words)").unwrap();
            writeln!(out, "{pad}    }}").unwrap();
            writeln!(out, "{pad}    pub fn detect(words: &'a [u32]) -> Option<&'a [u32]> {{").unwrap();
            writeln!(out, "{pad}        MapView::detect(words)").unwrap();
            writeln!(out, "{pad}    }}").unwrap();
        }
    }
    writeln!(out, "{pad}    pub fn raw_words(self) -> &'a [u32] {{ self.words }}").unwrap();
    match target {
        AliasTarget::BoolArray => {
            writeln!(out, "{pad}    pub fn raw(self) -> Option<BoolArray<'a>> {{").unwrap();
            writeln!(
                out,
                "{pad}        Some(protocache_core::StringView::new(self.words)?.as_bool_array())"
            )
            .unwrap();
            writeln!(out, "{pad}    }}").unwrap();
        }
        AliasTarget::Array => {
            writeln!(out, "{pad}    pub fn raw(self) -> Option<ArrayView<'a>> {{").unwrap();
            writeln!(out, "{pad}        ArrayView::new(self.words)").unwrap();
            writeln!(out, "{pad}    }}").unwrap();
        }
        AliasTarget::Map => {
            writeln!(out, "{pad}    pub fn raw(self) -> Option<MapView<'a>> {{").unwrap();
            writeln!(out, "{pad}        MapView::new(self.words)").unwrap();
            writeln!(out, "{pad}    }}").unwrap();
        }
    }

    let field = &message.field[0];
    match target {
        AliasTarget::Array => {
            if let Some(return_ty) = repeated_scalar_return(field) {
                writeln!(out, "{pad}    pub fn values(self) -> {return_ty} {{").unwrap();
                writeln!(out, "{pad}        {}", repeated_scalar_expr(field, "ArrayView::new(self.words)?")?).unwrap();
                writeln!(out, "{pad}    }}").unwrap();
            } else if let Some(return_ty) = repeated_view_return(field, registry)? {
                writeln!(out, "{pad}    pub fn values(self) -> Option<{return_ty}> {{").unwrap();
                writeln!(out, "{pad}        Some(ViewArray::new(ArrayView::new(self.words)?))").unwrap();
                writeln!(out, "{pad}    }}").unwrap();
            }
        }
        AliasTarget::Map => {
            let return_ty = alias_map_view_return(field, registry)?;
            writeln!(out, "{pad}    pub fn entries(self) -> Option<{return_ty}> {{").unwrap();
            writeln!(out, "{pad}        Some(ViewMap::new(MapView::new(self.words)?))").unwrap();
            writeln!(out, "{pad}    }}").unwrap();
        }
        AliasTarget::BoolArray => {}
    }
    writeln!(out, "{pad}}}\n").unwrap();
    writeln!(out, "{pad}impl<'a> FieldDecode<'a> for {rust_name}<'a> {{").unwrap();
    writeln!(out, "{pad}    fn decode(field: FieldView<'a>) -> Option<Self> {{").unwrap();
    writeln!(out, "{pad}        Self::from_words(field.object_words()?)").unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    writeln!(out, "{pad}}}\n").unwrap();
    writeln!(out, "{pad}impl GeneratedDescriptor for {rust_name}<'_> {{").unwrap();
    writeln!(out, "{pad}    const FULL_NAME: &'static str = \"{full}\";").unwrap();
    writeln!(out, "{pad}}}\n").unwrap();
    Ok(())
}

fn generate_regular_message(
    out: &mut String,
    registry: &Registry,
    message: &DescriptorProto,
    full: &str,
    indent: usize,
) -> Result<(), String> {
    let pad = "    ".repeat(indent);
    let rust_name = registry
        .rust_types
        .get(full)
        .ok_or_else(|| format!("missing rust name for {full}"))?;

    writeln!(out, "{pad}#[derive(Clone, Copy, Debug)]").unwrap();
    writeln!(out, "{pad}pub struct {rust_name}<'a> {{").unwrap();
    writeln!(out, "{pad}    view: MessageView<'a>,").unwrap();
    writeln!(out, "{pad}}}\n").unwrap();

    writeln!(out, "{pad}impl<'a> {rust_name}<'a> {{").unwrap();
    writeln!(out, "{pad}    pub fn from_words(words: &'a [u32]) -> Option<Self> {{").unwrap();
    writeln!(out, "{pad}        Some(Self {{ view: MessageView::new(words)? }})").unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    writeln!(out, "{pad}    pub fn detect_len(words: &'a [u32]) -> Option<usize> {{").unwrap();
    writeln!(out, "{pad}        MessageView::detect_len(words)").unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    writeln!(out, "{pad}    pub fn detect(words: &'a [u32]) -> Option<&'a [u32]> {{").unwrap();
    writeln!(out, "{pad}        MessageView::detect(words)").unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    writeln!(out, "{pad}    pub fn from_view(view: MessageView<'a>) -> Self {{ Self {{ view }} }}").unwrap();
    writeln!(out, "{pad}    pub fn raw(self) -> MessageView<'a> {{ self.view }}").unwrap();
    writeln!(out, "{pad}    pub fn raw_words(self) -> &'a [u32] {{ self.view.raw_words() }}").unwrap();
    writeln!(out, "{pad}    pub fn has_field(self, id: usize) -> bool {{ self.view.has_field(id) }}").unwrap();
    out.push('\n');

    let mut fields = BTreeMap::new();
    for field in &message.field {
        if field.options.as_ref().and_then(|o| o.deprecated).unwrap_or(false) {
            continue;
        }
        fields.insert(field.number(), field);
    }
    for field in fields.values() {
        writeln!(
            out,
            "{pad}    pub const {}_FIELD_ID: usize = {};",
            field.name().to_ascii_uppercase(),
            field.number() - 1
        )
        .unwrap();
    }
    out.push('\n');

    for field in fields.values() {
        generate_getter(out, registry, field, indent)?;
    }
    writeln!(out, "{pad}}}\n").unwrap();
    writeln!(out, "{pad}impl<'a> FieldDecode<'a> for {rust_name}<'a> {{").unwrap();
    writeln!(out, "{pad}    fn decode(field: FieldView<'a>) -> Option<Self> {{").unwrap();
    writeln!(out, "{pad}        Some(Self::from_view(field.message()?))").unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    writeln!(out, "{pad}}}\n").unwrap();
    writeln!(out, "{pad}impl<'a> GeneratedMessage<'a> for {rust_name}<'a> {{").unwrap();
    writeln!(out, "{pad}    fn from_message_view(view: MessageView<'a>) -> Self {{").unwrap();
    writeln!(out, "{pad}        Self::from_view(view)").unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    writeln!(out, "{pad}}}\n").unwrap();
    writeln!(out, "{pad}impl GeneratedDescriptor for {rust_name}<'_> {{").unwrap();
    writeln!(out, "{pad}    const FULL_NAME: &'static str = \"{full}\";").unwrap();
    writeln!(out, "{pad}}}\n").unwrap();
    Ok(())
}

fn generate_mutable_message(
    out: &mut String,
    registry: &Registry,
    message: &DescriptorProto,
    full: &str,
    indent: usize,
) -> Result<(), String> {
    let pad = "    ".repeat(indent);
    let rust_name = registry
        .rust_types
        .get(full)
        .ok_or_else(|| format!("missing rust name for {full}"))?;
    let mut_name = format!("{rust_name}Mut");
    let mut_ref_name = format!("{rust_name}MutRef");

    writeln!(out, "{pad}pub struct {mut_name}<'a> {{").unwrap();
    writeln!(out, "{pad}    inner: MessageMut<'a>,").unwrap();
    writeln!(out, "{pad}}}\n").unwrap();
    writeln!(out, "{pad}pub struct {mut_ref_name}<'b, 'a> {{").unwrap();
    writeln!(out, "{pad}    inner: &'b mut MessageMut<'a>,").unwrap();
    writeln!(out, "{pad}}}\n").unwrap();

    writeln!(out, "{pad}impl<'a> {rust_name}<'a> {{").unwrap();
    writeln!(
        out,
        "{pad}    pub fn open_mut(schema_path: impl AsRef<std::path::Path>, words: &'a [u32]) -> Result<{mut_name}<'a>, MutableError> {{"
    )
    .unwrap();
    writeln!(
        out,
        "{pad}        Ok({mut_name}::from_inner(<Self as GeneratedMutableMessage>::open_mut(schema_path, words)?))"
    )
    .unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    writeln!(
        out,
        "{pad}    pub fn new_mut(schema_path: impl AsRef<std::path::Path>) -> Result<{mut_name}<'static>, MutableError> {{"
    )
    .unwrap();
    writeln!(
        out,
        "{pad}        Ok({mut_name}::from_inner(<Self as GeneratedMutableMessage>::new_mut(schema_path)?))"
    )
    .unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    writeln!(out, "{pad}}}\n").unwrap();

    writeln!(out, "{pad}impl<'a> {mut_name}<'a> {{").unwrap();
    writeln!(out, "{pad}    pub fn from_inner(inner: MessageMut<'a>) -> Self {{").unwrap();
    writeln!(out, "{pad}        Self {{ inner }}").unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    writeln!(out, "{pad}    pub fn into_inner(self) -> MessageMut<'a> {{").unwrap();
    writeln!(out, "{pad}        self.inner").unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    writeln!(out, "{pad}    pub fn raw_mut(&mut self) -> &mut MessageMut<'a> {{").unwrap();
    writeln!(out, "{pad}        &mut self.inner").unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    writeln!(out, "{pad}    pub fn as_ref_mut(&mut self) -> {mut_ref_name}<'_, 'a> {{").unwrap();
    writeln!(out, "{pad}        {mut_ref_name}::from_inner(&mut self.inner)").unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    writeln!(out, "{pad}    pub fn serialize_words(&self) -> Result<Vec<u32>, MutableError> {{").unwrap();
    writeln!(out, "{pad}        self.inner.serialize_words()").unwrap();
    writeln!(out, "{pad}    }}").unwrap();

    let mut fields = BTreeMap::new();
    for field in &message.field {
        if field.options.as_ref().and_then(|o| o.deprecated).unwrap_or(false) {
            continue;
        }
        fields.insert(field.number(), field);
    }
    for field in fields.values() {
        generate_mutable_getter(out, registry, field, indent)?;
    }
    writeln!(out, "{pad}}}\n").unwrap();

    writeln!(out, "{pad}impl<'b, 'a> {mut_ref_name}<'b, 'a> {{").unwrap();
    writeln!(out, "{pad}    pub fn from_inner(inner: &'b mut MessageMut<'a>) -> Self {{").unwrap();
    writeln!(out, "{pad}        Self {{ inner }}").unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    writeln!(out, "{pad}    pub fn raw_mut(&mut self) -> &mut MessageMut<'a> {{").unwrap();
    writeln!(out, "{pad}        self.inner").unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    for field in fields.values() {
        generate_mutable_getter(out, registry, field, indent)?;
    }
    writeln!(out, "{pad}}}\n").unwrap();
    Ok(())
}

fn generate_mutable_getter(
    out: &mut String,
    registry: &Registry,
    field: &FieldDescriptorProto,
    indent: usize,
) -> Result<(), String> {
    let pad = "    ".repeat(indent);
    let name = field.name();

    if is_map_field(field, registry) {
        let method = if registry.aliases.contains_key(field.type_name()) {
            "field_alias_map_mut"
        } else {
            "field_map_mut"
        };
        writeln!(out, "{pad}    pub fn {name}_mut(&mut self) -> Result<&mut MapMut<'a>, MutableError> {{").unwrap();
        writeln!(out, "{pad}        self.inner.{method}(\"{name}\")").unwrap();
        writeln!(out, "{pad}    }}\n").unwrap();
        return Ok(());
    }

    if field.label() == Label::Repeated {
        let method = if registry.aliases.contains_key(field.type_name()) {
            "field_alias_list_mut"
        } else {
            "field_list_mut"
        };
        writeln!(out, "{pad}    pub fn {name}_mut(&mut self) -> Result<&mut ListMut<'a>, MutableError> {{").unwrap();
        writeln!(out, "{pad}        self.inner.{method}(\"{name}\")").unwrap();
        writeln!(out, "{pad}    }}\n").unwrap();
        return Ok(());
    }

    match field.r#type() {
        Type::Bool => {
            writeln!(out, "{pad}    pub fn set_{name}(&mut self, value: bool) -> Result<(), MutableError> {{").unwrap();
            writeln!(out, "{pad}        self.inner.set_bool(\"{name}\", value)").unwrap();
            writeln!(out, "{pad}    }}\n").unwrap();
        }
        Type::Int32 | Type::Sint32 | Type::Sfixed32 => {
            writeln!(out, "{pad}    pub fn set_{name}(&mut self, value: i32) -> Result<(), MutableError> {{").unwrap();
            writeln!(out, "{pad}        self.inner.set_i32(\"{name}\", value)").unwrap();
            writeln!(out, "{pad}    }}\n").unwrap();
        }
        Type::Uint32 | Type::Fixed32 => {
            writeln!(out, "{pad}    pub fn set_{name}(&mut self, value: u32) -> Result<(), MutableError> {{").unwrap();
            writeln!(out, "{pad}        self.inner.set_u32(\"{name}\", value)").unwrap();
            writeln!(out, "{pad}    }}\n").unwrap();
        }
        Type::Int64 | Type::Sint64 | Type::Sfixed64 => {
            writeln!(out, "{pad}    pub fn set_{name}(&mut self, value: i64) -> Result<(), MutableError> {{").unwrap();
            writeln!(out, "{pad}        self.inner.set_i64(\"{name}\", value)").unwrap();
            writeln!(out, "{pad}    }}\n").unwrap();
        }
        Type::Uint64 | Type::Fixed64 => {
            writeln!(out, "{pad}    pub fn set_{name}(&mut self, value: u64) -> Result<(), MutableError> {{").unwrap();
            writeln!(out, "{pad}        self.inner.set_u64(\"{name}\", value)").unwrap();
            writeln!(out, "{pad}    }}\n").unwrap();
        }
        Type::Float => {
            writeln!(out, "{pad}    pub fn set_{name}(&mut self, value: f32) -> Result<(), MutableError> {{").unwrap();
            writeln!(out, "{pad}        self.inner.set_f32(\"{name}\", value)").unwrap();
            writeln!(out, "{pad}    }}\n").unwrap();
        }
        Type::Double => {
            writeln!(out, "{pad}    pub fn set_{name}(&mut self, value: f64) -> Result<(), MutableError> {{").unwrap();
            writeln!(out, "{pad}        self.inner.set_f64(\"{name}\", value)").unwrap();
            writeln!(out, "{pad}    }}\n").unwrap();
        }
        Type::String => {
            writeln!(out, "{pad}    pub fn set_{name}(&mut self, value: impl Into<String>) -> Result<(), MutableError> {{").unwrap();
            writeln!(out, "{pad}        self.inner.set_string(\"{name}\", value)").unwrap();
            writeln!(out, "{pad}    }}\n").unwrap();
        }
        Type::Bytes => {
            writeln!(out, "{pad}    pub fn set_{name}(&mut self, value: impl Into<Vec<u8>>) -> Result<(), MutableError> {{").unwrap();
            writeln!(out, "{pad}        self.inner.set_bytes(\"{name}\", value)").unwrap();
            writeln!(out, "{pad}    }}\n").unwrap();
        }
        Type::Enum => {
            writeln!(out, "{pad}    pub fn set_{name}(&mut self, value: i32) -> Result<(), MutableError> {{").unwrap();
            writeln!(out, "{pad}        self.inner.set_enum_number(\"{name}\", value)").unwrap();
            writeln!(out, "{pad}    }}\n").unwrap();
        }
        Type::Message => {
            if registry.aliases.contains_key(field.type_name()) {
                match registry.aliases.get(field.type_name()).unwrap() {
                    AliasTarget::Map => {
                        writeln!(out, "{pad}    pub fn {name}_mut(&mut self) -> Result<&mut MapMut<'a>, MutableError> {{").unwrap();
                        writeln!(out, "{pad}        self.inner.field_alias_map_mut(\"{name}\")").unwrap();
                        writeln!(out, "{pad}    }}\n").unwrap();
                    }
                    AliasTarget::Array | AliasTarget::BoolArray => {
                        writeln!(out, "{pad}    pub fn {name}_mut(&mut self) -> Result<&mut ListMut<'a>, MutableError> {{").unwrap();
                        writeln!(out, "{pad}        self.inner.field_alias_list_mut(\"{name}\")").unwrap();
                        writeln!(out, "{pad}    }}\n").unwrap();
                    }
                }
            } else {
                let nested_rust_name = rust_type_name(registry, field.type_name())?;
                let nested_mut_ref_name = format!("{nested_rust_name}MutRef");
                writeln!(out, "{pad}    pub fn {name}_mut(&mut self) -> Result<{nested_mut_ref_name}<'_, 'a>, MutableError> {{").unwrap();
                writeln!(
                    out,
                    "{pad}        Ok({nested_mut_ref_name}::from_inner(self.inner.field_message_mut(\"{name}\")?))"
                )
                .unwrap();
                writeln!(out, "{pad}    }}\n").unwrap();
            }
        }
        _ => {}
    }
    Ok(())
}

fn generate_getter(
    out: &mut String,
    registry: &Registry,
    field: &FieldDescriptorProto,
    indent: usize,
) -> Result<(), String> {
    let pad = "    ".repeat(indent);
    let id = field.number() - 1;
    let name = field.name();

    if is_map_field(field, registry) {
        let return_ty = map_view_return(field, registry)?;
        writeln!(out, "{pad}    pub fn {name}(self) -> Option<{return_ty}> {{").unwrap();
        writeln!(out, "{pad}        Some(ViewMap::new(self.view.map({id}usize)?))").unwrap();
        writeln!(out, "{pad}    }}\n").unwrap();
        return Ok(());
    }

    if field.label() == Label::Repeated {
        if let Some(return_ty) = repeated_scalar_return(field) {
            writeln!(out, "{pad}    pub fn {name}(self) -> {return_ty} {{").unwrap();
            writeln!(out, "{pad}        {}", repeated_scalar_expr(field, &format!("self.view.array({id}usize)?"))?).unwrap();
            writeln!(out, "{pad}    }}\n").unwrap();
            return Ok(());
        }

        if let Some(return_ty) = repeated_view_return(field, registry)? {
            writeln!(out, "{pad}    pub fn {name}(self) -> Option<{return_ty}> {{").unwrap();
            writeln!(out, "{pad}        Some(ViewArray::new(self.view.array({id}usize)?))").unwrap();
            writeln!(out, "{pad}    }}\n").unwrap();
            return Ok(());
        }

        writeln!(out, "{pad}    pub fn {name}(self) -> Option<ArrayView<'a>> {{").unwrap();
        writeln!(out, "{pad}        self.view.array({id}usize)").unwrap();
        writeln!(out, "{pad}    }}\n").unwrap();
        return Ok(());
    }

    match field.r#type() {
        Type::String => {
            writeln!(out, "{pad}    pub fn {name}(self) -> Option<&'a str> {{").unwrap();
            writeln!(out, "{pad}        self.view.string({id}usize)?.as_str()").unwrap();
        }
        Type::Bytes => {
            writeln!(out, "{pad}    pub fn {name}(self) -> Option<&'a [u8]> {{").unwrap();
            writeln!(out, "{pad}        self.view.bytes({id}usize)").unwrap();
        }
        Type::Bool => {
            writeln!(out, "{pad}    pub fn {name}(self) -> bool {{").unwrap();
            writeln!(out, "{pad}        self.view.scalar::<bool>({id}usize).unwrap_or(false)").unwrap();
        }
        Type::Int32 | Type::Sint32 | Type::Sfixed32 => {
            writeln!(out, "{pad}    pub fn {name}(self) -> i32 {{").unwrap();
            writeln!(out, "{pad}        self.view.scalar::<i32>({id}usize).unwrap_or_default()").unwrap();
        }
        Type::Uint32 | Type::Fixed32 => {
            writeln!(out, "{pad}    pub fn {name}(self) -> u32 {{").unwrap();
            writeln!(out, "{pad}        self.view.scalar::<u32>({id}usize).unwrap_or_default()").unwrap();
        }
        Type::Int64 | Type::Sint64 | Type::Sfixed64 => {
            writeln!(out, "{pad}    pub fn {name}(self) -> i64 {{").unwrap();
            writeln!(out, "{pad}        self.view.scalar::<i64>({id}usize).unwrap_or_default()").unwrap();
        }
        Type::Uint64 | Type::Fixed64 => {
            writeln!(out, "{pad}    pub fn {name}(self) -> u64 {{").unwrap();
            writeln!(out, "{pad}        self.view.scalar::<u64>({id}usize).unwrap_or_default()").unwrap();
        }
        Type::Float => {
            writeln!(out, "{pad}    pub fn {name}(self) -> f32 {{").unwrap();
            writeln!(out, "{pad}        self.view.scalar::<f32>({id}usize).unwrap_or_default()").unwrap();
        }
        Type::Double => {
            writeln!(out, "{pad}    pub fn {name}(self) -> f64 {{").unwrap();
            writeln!(out, "{pad}        self.view.scalar::<f64>({id}usize).unwrap_or_default()").unwrap();
        }
        Type::Enum => {
            writeln!(out, "{pad}    pub fn {name}(self) -> i32 {{").unwrap();
            writeln!(out, "{pad}        self.view.scalar::<i32>({id}usize).unwrap_or_default()").unwrap();
        }
        Type::Message => {
            let rust_type = rust_type_name(registry, field.type_name())?;
            if registry.aliases.contains_key(field.type_name()) {
                writeln!(out, "{pad}    pub fn {name}(self) -> Option<{rust_type}<'a>> {{").unwrap();
                writeln!(
                    out,
                    "{pad}        {rust_type}::from_words(self.view.field({id}usize)?.object_words()?)"
                )
                .unwrap();
            } else {
                writeln!(out, "{pad}    pub fn {name}(self) -> Option<{rust_type}<'a>> {{").unwrap();
                writeln!(out, "{pad}        Some({rust_type}::from_view(self.view.message({id}usize)?))").unwrap();
            }
        }
        other => return Err(format!("unsupported field type {other:?} for {}", field.name())),
    }
    writeln!(out, "{pad}    }}\n").unwrap();
    Ok(())
}

fn repeated_scalar_return(field: &FieldDescriptorProto) -> Option<String> {
    if field.label() != Label::Repeated {
        return None;
    }
    match field.r#type() {
        Type::Bool => Some("Option<BoolArray<'a>>".to_owned()),
        Type::Int32 | Type::Sint32 | Type::Sfixed32 => Some("Option<ScalarArray<'a, i32>>".to_owned()),
        Type::Uint32 | Type::Fixed32 => Some("Option<ScalarArray<'a, u32>>".to_owned()),
        Type::Int64 | Type::Sint64 | Type::Sfixed64 => Some("Option<ScalarArray<'a, i64>>".to_owned()),
        Type::Uint64 | Type::Fixed64 => Some("Option<ScalarArray<'a, u64>>".to_owned()),
        Type::Float => Some("Option<ScalarArray<'a, f32>>".to_owned()),
        Type::Double => Some("Option<ScalarArray<'a, f64>>".to_owned()),
        Type::Enum => Some("Option<ScalarArray<'a, i32>>".to_owned()),
        _ => None,
    }
}

fn repeated_scalar_expr(field: &FieldDescriptorProto, array_expr: &str) -> Result<String, String> {
    Ok(match field.r#type() {
        Type::Bool => format!("self.view.bools({}usize)", field.number() - 1),
        Type::Int32 | Type::Sint32 | Type::Sfixed32 => format!("{array_expr}.scalars::<i32>()"),
        Type::Uint32 | Type::Fixed32 => format!("{array_expr}.scalars::<u32>()"),
        Type::Int64 | Type::Sint64 | Type::Sfixed64 => format!("{array_expr}.scalars::<i64>()"),
        Type::Uint64 | Type::Fixed64 => format!("{array_expr}.scalars::<u64>()"),
        Type::Float => format!("{array_expr}.scalars::<f32>()"),
        Type::Double => format!("{array_expr}.scalars::<f64>()"),
        Type::Enum => format!("{array_expr}.scalars::<i32>()"),
        other => return Err(format!("unsupported repeated scalar type: {other:?}")),
    })
}

fn repeated_view_return(
    field: &FieldDescriptorProto,
    registry: &Registry,
) -> Result<Option<String>, String> {
    Ok(match field.r#type() {
        Type::String | Type::Bytes => Some("ViewArray<'a, StringView<'a>>".to_owned()),
        Type::Message => {
            if is_map_field(field, registry) {
                None
            } else {
                let rust_type = rust_type_name(registry, field.type_name())?;
                Some(format!("ViewArray<'a, {rust_type}<'a>>"))
            }
        }
        _ => None,
    })
}

fn map_view_return(field: &FieldDescriptorProto, registry: &Registry) -> Result<String, String> {
    let entry = registry
        .map_entries
        .get(field.type_name())
        .ok_or_else(|| format!("missing map entry for {}", field.type_name()))?;
    let key = entry
        .field
        .first()
        .ok_or_else(|| format!("missing map key field for {}", field.type_name()))?;
    let value = entry
        .field
        .get(1)
        .ok_or_else(|| format!("missing map value field for {}", field.type_name()))?;
    Ok(format!(
        "ViewMap<'a, {}, {}>",
        decode_type_name(key, registry)?,
        decode_type_name(value, registry)?
    ))
}

fn alias_map_view_return(field: &FieldDescriptorProto, registry: &Registry) -> Result<String, String> {
    map_view_return(field, registry)
}

fn decode_type_name(field: &FieldDescriptorProto, registry: &Registry) -> Result<String, String> {
    Ok(match field.r#type() {
        Type::Bool => "bool".to_owned(),
        Type::Int32 | Type::Sint32 | Type::Sfixed32 => "i32".to_owned(),
        Type::Uint32 | Type::Fixed32 => "u32".to_owned(),
        Type::Int64 | Type::Sint64 | Type::Sfixed64 => "i64".to_owned(),
        Type::Uint64 | Type::Fixed64 => "u64".to_owned(),
        Type::Float => "f32".to_owned(),
        Type::Double => "f64".to_owned(),
        Type::Enum => "i32".to_owned(),
        Type::String | Type::Bytes => "StringView<'a>".to_owned(),
        Type::Message => {
            let rust_type = rust_type_name(registry, field.type_name())?;
            format!("{rust_type}<'a>")
        }
        other => return Err(format!("unsupported decode type: {other:?}")),
    })
}

fn rust_type_name(registry: &Registry, full_name: &str) -> Result<String, String> {
    registry
        .rust_types
        .get(full_name)
        .cloned()
        .ok_or_else(|| format!("unknown type {full_name}"))
}

fn flatten_name(full_name: &str, package: &str) -> String {
    let skip = if package.is_empty() {
        0
    } else {
        package.trim_start_matches('.').split('.').count()
    };
    full_name
        .trim_start_matches('.')
        .split('.')
        .skip(skip)
        .collect::<Vec<_>>()
        .join("")
}

fn package_prefix(package: &str) -> String {
    if package.is_empty() {
        String::new()
    } else {
        format!(".{package}")
    }
}

fn package_depth(package: &str) -> usize {
    if package.is_empty() {
        0
    } else {
        package.split('.').count()
    }
}

fn is_alias_message(message: &DescriptorProto) -> bool {
    message.field.len() == 1 && message.field[0].name() == "_"
}

fn is_map_field(field: &FieldDescriptorProto, registry: &Registry) -> bool {
    field.label() == Label::Repeated
        && field.r#type() == Type::Message
        && registry.map_entries.contains_key(field.type_name())
}

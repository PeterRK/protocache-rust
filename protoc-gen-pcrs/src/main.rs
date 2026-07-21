use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::io::{Read, Write};

use prost::Message;
use prost_types::compiler::{CodeGeneratorRequest, CodeGeneratorResponse, code_generator_response};
use prost_types::{
    DescriptorProto, EnumDescriptorProto, FieldDescriptorProto, FileDescriptorProto,
    field_descriptor_proto::{Label, Type},
};
use protocache_extension::reflection::DescriptorPool;

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

#[derive(Clone, Copy, Eq, PartialEq)]
enum OutputKind {
    Readonly,
    Extra,
}

fn main() {
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

    let response = match generate_response(&request) {
        Ok(response) => response,
        Err(err) => {
            emit_error(&err);
            return;
        }
    };

    let mut output = Vec::new();
    if response.encode(&mut output).is_err() || std::io::stdout().write_all(&output).is_err() {
        emit_error("failed to write CodeGeneratorResponse");
    }
}

fn generate_response(request: &CodeGeneratorRequest) -> Result<CodeGeneratorResponse, String> {
    validate_request(request)?;
    let extra = request.parameter.as_deref() == Some("extra");
    let registry = build_registry(&request.proto_file);
    let files: HashSet<_> = request.file_to_generate.iter().cloned().collect();
    let mut response = CodeGeneratorResponse::default();

    for proto in &request.proto_file {
        let proto_name = proto.name.as_deref().unwrap_or_default();
        if !files.contains(proto_name) {
            continue;
        }
        let content = generate_file(proto, &registry, OutputKind::Readonly)
            .map_err(|err| format!("failed to generate {proto_name}: {err}"))?;
        response.file.push(code_generator_response::File {
            name: Some(convert_filename(proto_name)),
            insertion_point: None,
            content: Some(content),
            generated_code_info: None,
        });
        if extra {
            let content = generate_file(proto, &registry, OutputKind::Extra)
                .map_err(|err| format!("failed to generate extra for {proto_name}: {err}"))?;
            response.file.push(code_generator_response::File {
                name: Some(convert_ex_filename(proto_name)),
                insertion_point: None,
                content: Some(content),
                generated_code_info: None,
            });
        }
    }
    Ok(response)
}

fn validate_request(request: &CodeGeneratorRequest) -> Result<(), String> {
    let mut pool = DescriptorPool::default();
    for file in &request.proto_file {
        pool.register(file)
            .map_err(|err| format!("schema validation failed: {err:?}"))?;
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

fn convert_ex_filename(name: &str) -> String {
    match name.rfind('.') {
        Some(pos) => format!("{}.pc-ex.rs", &name[..pos]),
        None => format!("{name}.pc-ex.rs"),
    }
}

fn build_registry(files: &[FileDescriptorProto]) -> Registry {
    let mut registry = Registry::default();
    for file in files {
        let package = file.package();
        for item in &file.enum_type {
            registry.rust_types.insert(
                format!("{}.{}", package_prefix(package), item.name()),
                item.name().to_owned(),
            );
        }
        for message in &file.message_type {
            collect_message_types(&mut registry, package, None, message);
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
    registry.rust_types.insert(
        full.clone(),
        flatten_name(&full, package, is_alias_message(message), parent.is_some()),
    );

    for nested in &message.nested_type {
        let nested_full = format!("{full}.{}", nested.name());
        if nested
            .options
            .as_ref()
            .and_then(|o| o.map_entry)
            .unwrap_or(false)
        {
            registry.map_entries.insert(nested_full, nested.clone());
            continue;
        }
        collect_message_types(registry, package, Some(&full), nested);
    }

    for item in &message.enum_type {
        registry.rust_types.insert(
            format!("{full}.{}", item.name()),
            flatten_name(&format!("{full}.{}", item.name()), package, false, true),
        );
    }

    if is_alias_message(message) {
        let field = &message.field[0];
        let target = if field.label() == Label::Repeated && field.r#type() == Type::Bool {
            AliasTarget::BoolArray
        } else if field.r#type() == Type::Message
            && registry.map_entries.contains_key(field.type_name())
        {
            AliasTarget::Map
        } else {
            AliasTarget::Array
        };
        registry.aliases.insert(full, target);
    }
}

fn generate_file(
    file: &FileDescriptorProto,
    registry: &Registry,
    kind: OutputKind,
) -> Result<String, String> {
    let mut out = String::new();
    out.push_str("// @generated by protoc-gen-pcrs.\n");
    if kind == OutputKind::Extra {
        out.push_str("// Requires the corresponding .pc.rs file to be included first.\n");
    }

    if kind == OutputKind::Readonly && !file.package().is_empty() {
        for part in file.package().split('.') {
            writeln!(&mut out, "pub mod {part} {{").unwrap();
        }
        out.push('\n');
        let pad = "    ".repeat(package_depth(file.package()));
        writeln!(
            &mut out,
            "{pad}use protocache_core::{{ArrayView, BoolArray, EnumValue, FieldDecode, FieldView, MapView, MessageView, ScalarArray, StringView, ViewArray, ViewMap}};\n"
        )
        .unwrap();
    } else {
        match kind {
            OutputKind::Readonly => {
                out.push_str("use protocache_core::{ArrayView, BoolArray, EnumValue, FieldDecode, FieldView, MapView, MessageView, ScalarArray, StringView, ViewArray, ViewMap};\n\n");
            }
            OutputKind::Extra => {
                if !file.package().is_empty() {
                    writeln!(&mut out, "use {};\n", base_types_import(file.package())).unwrap();
                }
                out.push_str("use protocache_core::{ArrayView, Buffer, EnumValue, FieldView, MutableError, Unit, serialize_message_at};\n");
                out.push_str("use protocache_core::mutable::{MutableArray, MutableArrayElement, MutableField, MutableMap, MutableMessage};\n\n");
            }
        }
    }

    let indent = if kind == OutputKind::Readonly {
        package_depth(file.package())
    } else {
        0
    };
    if kind == OutputKind::Readonly {
        for item in &file.enum_type {
            generate_enum(&mut out, item, indent);
        }
    }
    for message in &file.message_type {
        generate_message(
            &mut out,
            registry,
            message,
            file.package(),
            None,
            indent,
            kind,
        )?;
    }

    if kind == OutputKind::Readonly && !file.package().is_empty() {
        for part in file.package().split('.').rev() {
            writeln!(&mut out, "}} // mod {part}").unwrap();
        }
    }
    Ok(out)
}

fn base_types_import(package: &str) -> String {
    let mut path = String::from("self::");
    if package.is_empty() {
        path.push('*');
    } else {
        path.push_str(&package.replace('.', "::"));
        path.push_str("::*");
    }
    path
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
    kind: OutputKind,
) -> Result<(), String> {
    let full = if let Some(parent) = parent {
        format!("{parent}.{}", message.name())
    } else {
        format!("{}.{}", package_prefix(package), message.name())
    };

    if kind == OutputKind::Readonly {
        for item in &message.enum_type {
            generate_enum(out, item, indent);
        }
    }
    for nested in &message.nested_type {
        if nested
            .options
            .as_ref()
            .and_then(|o| o.map_entry)
            .unwrap_or(false)
        {
            continue;
        }
        generate_message(out, registry, nested, package, Some(&full), indent, kind)?;
    }

    match kind {
        OutputKind::Readonly => {
            if is_alias_message(message) {
                generate_alias_readonly(out, registry, message, &full, indent)?;
            } else {
                generate_regular_message(out, registry, message, &full, indent)?;
            }
        }
        OutputKind::Extra => {
            if is_alias_message(message) {
                generate_alias_extra(out, registry, message, &full, indent)?;
            } else {
                generate_mutable_message(out, registry, message, &full, indent)?;
            }
        }
    }
    Ok(())
}

fn generate_alias_readonly(
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

    let view_ty = match target {
        AliasTarget::BoolArray | AliasTarget::Array => "ArrayView<'a>",
        AliasTarget::Map => "MapView<'a>",
    };

    writeln!(out, "{pad}#[derive(Clone, Copy, Debug)]").unwrap();
    writeln!(out, "{pad}pub struct {rust_name}<'a> {{").unwrap();
    writeln!(out, "{pad}    view: {view_ty},").unwrap();
    writeln!(out, "{pad}}}\n").unwrap();

    writeln!(out, "{pad}impl<'a> {rust_name}<'a> {{").unwrap();
    writeln!(
        out,
        "{pad}    pub fn FromWords(words: &'a [u32]) -> Option<Self> {{"
    )
    .unwrap();
    match target {
        AliasTarget::BoolArray => {
            writeln!(
                out,
                "{pad}        Some(Self {{ view: ArrayView::new(words)? }})"
            )
            .unwrap();
        }
        AliasTarget::Array => {
            writeln!(
                out,
                "{pad}        Some(Self {{ view: ArrayView::new(words)? }})"
            )
            .unwrap();
        }
        AliasTarget::Map => {
            writeln!(
                out,
                "{pad}        Some(Self {{ view: MapView::new(words)? }})"
            )
            .unwrap();
        }
    }
    writeln!(out, "{pad}    }}").unwrap();
    match target {
        AliasTarget::BoolArray => {
            writeln!(
                out,
                "{pad}    fn DetectLen(words: &'a [u32]) -> Option<usize> {{"
            )
            .unwrap();
            writeln!(out, "{pad}        ArrayView::detect_len(words)").unwrap();
            writeln!(out, "{pad}    }}").unwrap();
            writeln!(
                out,
                "{pad}    pub fn Detect(words: &'a [u32]) -> Option<&'a [u32]> {{"
            )
            .unwrap();
            writeln!(
                out,
                "{pad}        Some(words.get(..Self::DetectLen(words)?)?)"
            )
            .unwrap();
            writeln!(out, "{pad}    }}").unwrap();
        }
        AliasTarget::Array => {
            write_alias_array_detect_len(out, registry, message, indent)?;
            writeln!(
                out,
                "{pad}    pub fn Detect(words: &'a [u32]) -> Option<&'a [u32]> {{"
            )
            .unwrap();
            writeln!(
                out,
                "{pad}        Some(words.get(..Self::DetectLen(words)?)?)"
            )
            .unwrap();
            writeln!(out, "{pad}    }}").unwrap();
        }
        AliasTarget::Map => {
            write_alias_map_detect_len(out, registry, message, indent)?;
            writeln!(
                out,
                "{pad}    pub fn Detect(words: &'a [u32]) -> Option<&'a [u32]> {{"
            )
            .unwrap();
            writeln!(
                out,
                "{pad}        Some(words.get(..Self::DetectLen(words)?)?)"
            )
            .unwrap();
            writeln!(out, "{pad}    }}").unwrap();
        }
    }
    let field = &message.field[0];
    match target {
        AliasTarget::Array => {
            if let Some(return_ty) = repeated_scalar_return(field) {
                let direct_return_ty = return_ty
                    .strip_prefix("Option<")
                    .and_then(|value| value.strip_suffix('>'))
                    .ok_or_else(|| {
                        format!("alias scalar return should be Option<T>: {return_ty}")
                    })?;
                writeln!(out, "{pad}    pub fn values(self) -> {direct_return_ty} {{").unwrap();
                writeln!(
                    out,
                    "{pad}        {}.expect(\"generated alias invariant violated: {rust_name}\")",
                    repeated_scalar_expr(field, "self.view")?
                )
                .unwrap();
                writeln!(out, "{pad}    }}").unwrap();
            } else if let Some(return_ty) = repeated_view_return(field, registry)? {
                writeln!(out, "{pad}    pub fn values(self) -> {return_ty} {{").unwrap();
                writeln!(out, "{pad}        ViewArray::new(self.view)").unwrap();
                writeln!(out, "{pad}    }}").unwrap();
            }
        }
        AliasTarget::Map => {
            let return_ty = alias_map_view_return(field, registry)?;
            writeln!(out, "{pad}    pub fn entries(self) -> {return_ty} {{").unwrap();
            writeln!(out, "{pad}        ViewMap::new(self.view)").unwrap();
            writeln!(out, "{pad}    }}").unwrap();
        }
        AliasTarget::BoolArray => {}
    }
    writeln!(out, "{pad}}}\n").unwrap();
    writeln!(out, "{pad}impl<'a> FieldDecode<'a> for {rust_name}<'a> {{").unwrap();
    writeln!(
        out,
        "{pad}    fn decode(field: FieldView<'a>) -> Option<Self> {{"
    )
    .unwrap();
    writeln!(out, "{pad}        Self::FromWords(field.object_words()?)").unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    writeln!(out, "{pad}}}\n").unwrap();
    Ok(())
}

fn generate_alias_extra(
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
    let field = &message.field[0];
    let mutable_name = format!("{rust_name}Mutable");
    let mutable_ty = alias_mutable_type(field, registry)?;
    writeln!(out, "{pad}pub type {mutable_name}<'a> = {mutable_ty};\n").unwrap();
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
    writeln!(
        out,
        "{pad}    pub fn FromWords(words: &'a [u32]) -> Option<Self> {{"
    )
    .unwrap();
    writeln!(
        out,
        "{pad}        Some(Self {{ view: MessageView::new(words)? }})"
    )
    .unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    write_message_detect_len(out, registry, message, indent)?;
    writeln!(
        out,
        "{pad}    pub fn Detect(words: &'a [u32]) -> Option<&'a [u32]> {{"
    )
    .unwrap();
    writeln!(
        out,
        "{pad}        Some(words.get(..Self::DetectLen(words)?)?)"
    )
    .unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    writeln!(
        out,
        "{pad}    pub fn HasField(self, id: usize) -> bool {{ self.view.has_field(id) }}"
    )
    .unwrap();
    out.push('\n');

    let mut fields = BTreeMap::new();
    for field in &message.field {
        if field
            .options
            .as_ref()
            .and_then(|o| o.deprecated)
            .unwrap_or(false)
        {
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
    writeln!(
        out,
        "{pad}    fn decode(field: FieldView<'a>) -> Option<Self> {{"
    )
    .unwrap();
    writeln!(out, "{pad}        Some(Self {{ view: field.message()? }})").unwrap();
    writeln!(out, "{pad}    }}").unwrap();
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
    let mutable_name = format!("{rust_name}Mutable");
    let readonly_name = rust_name.as_str();

    let mut fields = BTreeMap::new();
    for field in &message.field {
        if field
            .options
            .as_ref()
            .and_then(|o| o.deprecated)
            .unwrap_or(false)
        {
            continue;
        }
        fields.insert(field.number(), field);
    }
    let max_id = fields.keys().last().copied().unwrap_or(0);
    let accessed_words = (max_id + 63) / 64;

    writeln!(out, "{pad}#[derive(Clone, Debug, Default)]").unwrap();
    writeln!(out, "{pad}pub struct {mutable_name}<'a> {{").unwrap();
    writeln!(
        out,
        "{pad}    __view__: MutableMessage<'a, {max_id}, {accessed_words}>,"
    )
    .unwrap();
    for field in fields.values() {
        writeln!(
            out,
            "{pad}    _{}: {},",
            field.name(),
            mutable_field_type(field, registry)?
        )
        .unwrap();
    }
    writeln!(out, "{pad}}}\n").unwrap();

    writeln!(out, "{pad}impl<'a> {mutable_name}<'a> {{").unwrap();
    writeln!(out, "{pad}    pub fn New() -> Self {{ Self::default() }}").unwrap();
    writeln!(
        out,
        "{pad}    pub fn FromWords(words: &'a [u32]) -> Option<Self> {{"
    )
    .unwrap();
    writeln!(out, "{pad}        Some(Self {{ __view__: MutableMessage::from_words(words)?, ..Self::default() }})").unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    writeln!(
        out,
        "{pad}    pub fn HasField(&self, id: usize) -> bool {{ self.__view__.has_field(id) }}"
    )
    .unwrap();
    writeln!(
        out,
        "{pad}    fn EncodeToUnit(&self, buffer: &mut Buffer) -> Result<Unit, MutableError> {{"
    )
    .unwrap();
    writeln!(
        out,
        "{pad}        if let Some(words) = self.__view__.clean_words() {{"
    )
    .unwrap();
    writeln!(
        out,
        "{pad}            return Ok(protocache_core::mutable::copy_words(words, buffer, false));"
    )
    .unwrap();
    writeln!(out, "{pad}        }}").unwrap();
    writeln!(out, "{pad}        let last = buffer.len();").unwrap();
    writeln!(
        out,
        "{pad}        let mut parts = [Unit::empty(); {max_id}];"
    )
    .unwrap();
    for field in fields.values().rev() {
        let field_id = field_const_name(field);
        writeln!(
            out,
            "{pad}        self.__view__.serialize_field({readonly_name}::{field_id}, &self._{}, buffer, &mut parts[{readonly_name}::{field_id}])?;",
            field.name()
        )
        .unwrap();
    }
    writeln!(out, "{pad}        serialize_message_at(&mut parts, buffer, last).ok_or_else(|| MutableError::SerializeFailed {{").unwrap();
    writeln!(out, "{pad}            descriptor: \"{full}\".to_owned(),").unwrap();
    writeln!(out, "{pad}            field: \"<message>\".to_owned(),").unwrap();
    writeln!(out, "{pad}        }})").unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    writeln!(
        out,
        "{pad}    pub fn SerializeWords(&self) -> Result<Vec<u32>, MutableError> {{"
    )
    .unwrap();
    writeln!(out, "{pad}        let mut buffer = Buffer::new();").unwrap();
    writeln!(
        out,
        "{pad}        let _ = self.SerializeIntoBuffer(&mut buffer)?;"
    )
    .unwrap();
    writeln!(out, "{pad}        Ok(buffer.view().to_vec())").unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    writeln!(out, "{pad}    pub fn SerializeIntoBuffer<'b>(&self, buffer: &'b mut Buffer) -> Result<&'b [u32], MutableError> {{").unwrap();
    writeln!(out, "{pad}        buffer.clear();").unwrap();
    writeln!(out, "{pad}        let _ = self.EncodeToUnit(buffer)?;").unwrap();
    writeln!(out, "{pad}        Ok(buffer.view())").unwrap();
    writeln!(out, "{pad}    }}").unwrap();

    for field in fields.values() {
        generate_mutable_getter(out, registry, field, indent, readonly_name)?;
    }
    writeln!(out, "{pad}}}\n").unwrap();

    writeln!(
        out,
        "{pad}impl<'a> MutableField<'a> for {mutable_name}<'a> {{"
    )
    .unwrap();
    writeln!(
        out,
        "{pad}    fn decode(field: FieldView<'a>) -> Option<Self> {{"
    )
    .unwrap();
    writeln!(out, "{pad}        Self::FromWords(field.object_words()?)").unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    writeln!(
        out,
        "{pad}    fn detect<'b>(field: FieldView<'b>) -> Option<&'b [u32]> {{"
    )
    .unwrap();
    writeln!(
        out,
        "{pad}        {rust_name}::Detect(field.object_words()?)"
    )
    .unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    writeln!(
        out,
        "{pad}    fn encode(&self, buffer: &mut Buffer) -> Result<Unit, MutableError> {{"
    )
    .unwrap();
    writeln!(out, "{pad}        self.EncodeToUnit(buffer)").unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    if fields.is_empty() {
        writeln!(out, "{pad}    fn is_dirty(&self) -> bool {{ false }}").unwrap();
    } else {
        writeln!(
            out,
            "{pad}    fn is_dirty(&self) -> bool {{ self.__view__.has_any_accessed() }}"
        )
        .unwrap();
    }
    writeln!(out, "{pad}}}\n").unwrap();

    writeln!(
        out,
        "{pad}impl<'a> MutableArrayElement<'a> for {mutable_name}<'a> {{"
    )
    .unwrap();
    writeln!(
        out,
        "{pad}    fn decode_array(words: &'a [u32]) -> Option<Vec<Self>> {{"
    )
    .unwrap();
    writeln!(out, "{pad}        let array = ArrayView::new(words)?;").unwrap();
    writeln!(
        out,
        "{pad}        let mut values = Vec::with_capacity(array.len());"
    )
    .unwrap();
    writeln!(out, "{pad}        for item in array.iter() {{ values.push(Self::FromWords(item.object_words()?)?); }}").unwrap();
    writeln!(out, "{pad}        Some(values)").unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    writeln!(out, "{pad}    fn encode_array(values: &[Self], buffer: &mut Buffer) -> Result<Unit, MutableError> {{").unwrap();
    writeln!(
        out,
        "{pad}        if values.is_empty() {{ return Ok(Unit::inline(&[1])); }}"
    )
    .unwrap();
    writeln!(out, "{pad}        let last = buffer.len();").unwrap();
    writeln!(
        out,
        "{pad}        let mut units = vec![Unit::empty(); values.len()];"
    )
    .unwrap();
    writeln!(
        out,
        "{pad}        for i in (0..values.len()).rev() {{ units[i] = values[i].encode(buffer)?; }}"
    )
    .unwrap();
    writeln!(out, "{pad}        protocache_core::serialize_array_at_mut(&mut units, buffer, last).ok_or_else(|| MutableError::SerializeFailed {{").unwrap();
    writeln!(out, "{pad}            descriptor: \"{full}\".to_owned(),").unwrap();
    writeln!(out, "{pad}            field: \"<array>\".to_owned(),").unwrap();
    writeln!(out, "{pad}        }})").unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    writeln!(out, "{pad}}}\n").unwrap();
    Ok(())
}

fn generate_mutable_getter(
    out: &mut String,
    registry: &Registry,
    field: &FieldDescriptorProto,
    indent: usize,
    readonly_name: &str,
) -> Result<(), String> {
    let pad = "    ".repeat(indent);
    let name = field.name();
    let field_id = field_const_name(field);
    writeln!(
        out,
        "{pad}    pub fn {name}(&mut self) -> {} {{",
        mutable_getter_return_type(field, registry)?
    )
    .unwrap();
    if field.label() == Label::Optional
        && field.r#type() == Type::Message
        && !is_map_field(field, registry)
    {
        writeln!(
            out,
            "{pad}        self.__view__.get_field({readonly_name}::{field_id}, &mut self._{}).as_mut()",
            name
        )
        .unwrap();
    } else {
        writeln!(
            out,
            "{pad}        self.__view__.get_field({readonly_name}::{field_id}, &mut self._{})",
            name
        )
        .unwrap();
    }
    writeln!(out, "{pad}    }}\n").unwrap();
    Ok(())
}

fn write_alias_array_detect_len(
    out: &mut String,
    registry: &Registry,
    message: &DescriptorProto,
    indent: usize,
) -> Result<(), String> {
    let pad = "    ".repeat(indent);
    let field = &message.field[0];
    writeln!(
        out,
        "{pad}    fn DetectLen(words: &'a [u32]) -> Option<usize> {{"
    )
    .unwrap();
    match field.r#type() {
        Type::Int32
        | Type::Sint32
        | Type::Sfixed32
        | Type::Uint32
        | Type::Fixed32
        | Type::Int64
        | Type::Sint64
        | Type::Sfixed64
        | Type::Uint64
        | Type::Fixed64
        | Type::Float
        | Type::Double
        | Type::Enum => {
            writeln!(out, "{pad}        ArrayView::detect_len(words)").unwrap();
        }
        _ => {
            let detect_expr = singular_detect_expr(field, registry, "item")?;
            writeln!(
                out,
                "{pad}        Some(protocache_core::detect_array_with(words, |item| {detect_expr})?.len())"
            )
            .unwrap();
        }
    }
    writeln!(out, "{pad}    }}").unwrap();
    Ok(())
}

fn write_alias_map_detect_len(
    out: &mut String,
    registry: &Registry,
    message: &DescriptorProto,
    indent: usize,
) -> Result<(), String> {
    let pad = "    ".repeat(indent);
    let entry = registry
        .map_entries
        .get(message.field[0].type_name())
        .ok_or_else(|| {
            format!(
                "missing alias map entry for {}",
                message.field[0].type_name()
            )
        })?;
    let key = entry
        .field
        .first()
        .ok_or_else(|| format!("missing map key field for {}", message.field[0].type_name()))?;
    let value = entry.field.get(1).ok_or_else(|| {
        format!(
            "missing map value field for {}",
            message.field[0].type_name()
        )
    })?;
    let key_expr = singular_detect_expr(key, registry, "key")?;
    let value_expr = singular_detect_expr(value, registry, "value")?;
    writeln!(
        out,
        "{pad}    fn DetectLen(words: &'a [u32]) -> Option<usize> {{"
    )
    .unwrap();
    writeln!(
        out,
        "{pad}        Some(protocache_core::detect_map_with(words, |key| {key_expr}, |value| {value_expr})?.len())"
    )
    .unwrap();
    writeln!(out, "{pad}    }}").unwrap();
    Ok(())
}

fn write_message_detect_len(
    out: &mut String,
    registry: &Registry,
    message: &DescriptorProto,
    indent: usize,
) -> Result<(), String> {
    let pad = "    ".repeat(indent);
    writeln!(
        out,
        "{pad}    fn DetectLen(words: &'a [u32]) -> Option<usize> {{"
    )
    .unwrap();
    writeln!(out, "{pad}        let view = MessageView::detect(words)?;").unwrap();

    let mut fields = BTreeMap::new();
    for field in &message.field {
        if field
            .options
            .as_ref()
            .and_then(|o| o.deprecated)
            .unwrap_or(false)
        {
            continue;
        }
        fields.insert(field.number(), field);
    }

    if fields.is_empty() {
        writeln!(out, "{pad}        Some(view.len())").unwrap();
    } else {
        writeln!(out, "{pad}        let core = MessageView::new(words)?;").unwrap();

        for field in fields.values().rev() {
            if !field_needs_detect(field, registry) {
                continue;
            }
            let field_id = field_const_name(field);
            let detect_expr = field_detect_expr(field, registry, "field")?;
            writeln!(
                out,
                "{pad}        if let Some(field) = core.field(Self::{field_id}) {{"
            )
            .unwrap();
            writeln!(out, "{pad}            let mut end = view.len();").unwrap();
            writeln!(out, "{pad}            protocache_core::detect_slice_end(words, {detect_expr}?, &mut end)?;").unwrap();
            writeln!(out, "{pad}            if end > view.len() {{").unwrap();
            writeln!(out, "{pad}                return Some(end);").unwrap();
            writeln!(out, "{pad}            }}").unwrap();
            writeln!(out, "{pad}        }}").unwrap();
        }

        writeln!(out, "{pad}        Some(view.len())").unwrap();
    }
    writeln!(out, "{pad}    }}").unwrap();
    Ok(())
}

fn field_detect_expr(
    field: &FieldDescriptorProto,
    registry: &Registry,
    var: &str,
) -> Result<String, String> {
    if is_map_field(field, registry) {
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
        let key_expr = singular_detect_expr(key, registry, "key")?;
        let value_expr = singular_detect_expr(value, registry, "value")?;
        return Ok(format!(
            "protocache_core::detect_map_with({var}.object_words()?, |key| {key_expr}, |value| {value_expr})"
        ));
    }

    if field.label() == Label::Repeated {
        return Ok(match field.r#type() {
            Type::Bool => format!("{var}.detect_string()"),
            Type::Int32
            | Type::Sint32
            | Type::Sfixed32
            | Type::Uint32
            | Type::Fixed32
            | Type::Int64
            | Type::Sint64
            | Type::Sfixed64
            | Type::Uint64
            | Type::Fixed64
            | Type::Float
            | Type::Double
            | Type::Enum => format!("{var}.detect_array()"),
            _ => {
                let item_expr = singular_detect_expr(field, registry, "item")?;
                format!(
                    "protocache_core::detect_array_with({var}.object_words()?, |item| {item_expr})"
                )
            }
        });
    }

    singular_detect_expr(field, registry, var)
}

fn field_needs_detect(field: &FieldDescriptorProto, registry: &Registry) -> bool {
    if is_map_field(field, registry) || field.label() == Label::Repeated {
        return true;
    }

    matches!(field.r#type(), Type::Message | Type::String | Type::Bytes)
}

fn singular_detect_expr(
    field: &FieldDescriptorProto,
    registry: &Registry,
    var: &str,
) -> Result<String, String> {
    Ok(match field.r#type() {
        Type::Bool
        | Type::Int32
        | Type::Sint32
        | Type::Sfixed32
        | Type::Uint32
        | Type::Fixed32
        | Type::Int64
        | Type::Sint64
        | Type::Sfixed64
        | Type::Uint64
        | Type::Fixed64
        | Type::Float
        | Type::Double
        | Type::Enum => format!("{var}.detect_scalar()"),
        Type::String | Type::Bytes => format!("{var}.detect_string()"),
        Type::Message => {
            let rust_type = rust_type_name(registry, field.type_name())?;
            format!("{rust_type}::Detect({var}.object_words()?)")
        }
        other => return Err(format!("unsupported detect type: {other:?}")),
    })
}

fn mutable_field_type(field: &FieldDescriptorProto, registry: &Registry) -> Result<String, String> {
    if is_map_field(field, registry) {
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
        return Ok(format!(
            "MutableMap<'a, {}, {}>",
            mutable_key_type(key)?,
            mutable_value_type(value, registry)?,
        ));
    }

    if field.label() == Label::Repeated {
        return Ok(format!(
            "MutableArray<'a, {}>",
            mutable_value_type(field, registry)?
        ));
    }

    if field.r#type() == Type::Message {
        return Ok(format!("Box<{}>", mutable_value_type(field, registry)?));
    }

    mutable_value_type(field, registry)
}

fn mutable_getter_return_type(
    field: &FieldDescriptorProto,
    registry: &Registry,
) -> Result<String, String> {
    if field.label() == Label::Optional
        && field.r#type() == Type::Message
        && !is_map_field(field, registry)
    {
        return Ok(format!("&mut {}", mutable_value_type(field, registry)?));
    }
    Ok(format!("&mut {}", mutable_field_type(field, registry)?))
}

fn mutable_key_type(field: &FieldDescriptorProto) -> Result<String, String> {
    Ok(match field.r#type() {
        Type::String => "String".to_owned(),
        Type::Int32 | Type::Sint32 | Type::Sfixed32 => "i32".to_owned(),
        Type::Uint32 | Type::Fixed32 => "u32".to_owned(),
        Type::Int64 | Type::Sint64 | Type::Sfixed64 => "i64".to_owned(),
        Type::Uint64 | Type::Fixed64 => "u64".to_owned(),
        other => return Err(format!("unsupported mutable map key type: {other:?}")),
    })
}

fn mutable_value_type(field: &FieldDescriptorProto, registry: &Registry) -> Result<String, String> {
    Ok(match field.r#type() {
        Type::Bool => "bool".to_owned(),
        Type::Int32 | Type::Sint32 | Type::Sfixed32 => "i32".to_owned(),
        Type::Uint32 | Type::Fixed32 => "u32".to_owned(),
        Type::Int64 | Type::Sint64 | Type::Sfixed64 => "i64".to_owned(),
        Type::Uint64 | Type::Fixed64 => "u64".to_owned(),
        Type::Float => "f32".to_owned(),
        Type::Double => "f64".to_owned(),
        Type::String => "String".to_owned(),
        Type::Bytes => "Vec<u8>".to_owned(),
        Type::Enum => "EnumValue".to_owned(),
        Type::Message => format!(
            "{}<'a>",
            rust_mutable_base_name(registry, field.type_name())?
        ),
        other => return Err(format!("unsupported mutable value type: {other:?}")),
    })
}

fn alias_mutable_type(field: &FieldDescriptorProto, registry: &Registry) -> Result<String, String> {
    if is_map_field(field, registry) {
        let entry = registry
            .map_entries
            .get(field.type_name())
            .ok_or_else(|| format!("missing alias map entry for {}", field.type_name()))?;
        let key = entry
            .field
            .first()
            .ok_or_else(|| format!("missing map key field for {}", field.type_name()))?;
        let value = entry
            .field
            .get(1)
            .ok_or_else(|| format!("missing map value field for {}", field.type_name()))?;
        Ok(format!(
            "MutableMap<'a, {}, {}>",
            mutable_key_type(key)?,
            mutable_value_type(value, registry)?
        ))
    } else {
        Ok(format!(
            "MutableArray<'a, {}>",
            mutable_value_type(field, registry)?
        ))
    }
}

fn generate_getter(
    out: &mut String,
    registry: &Registry,
    field: &FieldDescriptorProto,
    indent: usize,
) -> Result<(), String> {
    let pad = "    ".repeat(indent);
    let name = field.name();
    let field_id = field_const_name(field);

    if is_map_field(field, registry) {
        let return_ty = map_view_return(field, registry)?;
        writeln!(
            out,
            "{pad}    pub fn {name}(self) -> Option<{return_ty}> {{"
        )
        .unwrap();
        writeln!(
            out,
            "{pad}        Some(ViewMap::new(self.view.map(Self::{field_id})?))"
        )
        .unwrap();
        writeln!(out, "{pad}    }}\n").unwrap();
        return Ok(());
    }

    if field.label() == Label::Repeated {
        if let Some(return_ty) = repeated_scalar_return(field) {
            writeln!(out, "{pad}    pub fn {name}(self) -> {return_ty} {{").unwrap();
            writeln!(
                out,
                "{pad}        {}",
                repeated_scalar_expr(field, &format!("self.view.array(Self::{field_id})?"))?
            )
            .unwrap();
            writeln!(out, "{pad}    }}\n").unwrap();
            return Ok(());
        }

        if let Some(return_ty) = repeated_view_return(field, registry)? {
            writeln!(
                out,
                "{pad}    pub fn {name}(self) -> Option<{return_ty}> {{"
            )
            .unwrap();
            writeln!(
                out,
                "{pad}        Some(ViewArray::new(self.view.array(Self::{field_id})?))"
            )
            .unwrap();
            writeln!(out, "{pad}    }}\n").unwrap();
            return Ok(());
        }

        writeln!(
            out,
            "{pad}    pub fn {name}(self) -> Option<ArrayView<'a>> {{"
        )
        .unwrap();
        writeln!(out, "{pad}        self.view.array(Self::{field_id})").unwrap();
        writeln!(out, "{pad}    }}\n").unwrap();
        return Ok(());
    }

    match field.r#type() {
        Type::String => {
            writeln!(out, "{pad}    pub fn {name}(self) -> Option<&'a str> {{").unwrap();
            writeln!(
                out,
                "{pad}        self.view.string(Self::{field_id})?.as_str()"
            )
            .unwrap();
        }
        Type::Bytes => {
            writeln!(out, "{pad}    pub fn {name}(self) -> Option<&'a [u8]> {{").unwrap();
            writeln!(out, "{pad}        self.view.bytes(Self::{field_id})").unwrap();
        }
        Type::Bool => {
            writeln!(out, "{pad}    pub fn {name}(self) -> bool {{").unwrap();
            writeln!(
                out,
                "{pad}        self.view.scalar::<bool>(Self::{field_id}).unwrap_or(false)"
            )
            .unwrap();
        }
        Type::Int32 | Type::Sint32 | Type::Sfixed32 => {
            writeln!(out, "{pad}    pub fn {name}(self) -> i32 {{").unwrap();
            writeln!(
                out,
                "{pad}        self.view.scalar::<i32>(Self::{field_id}).unwrap_or_default()"
            )
            .unwrap();
        }
        Type::Uint32 | Type::Fixed32 => {
            writeln!(out, "{pad}    pub fn {name}(self) -> u32 {{").unwrap();
            writeln!(
                out,
                "{pad}        self.view.scalar::<u32>(Self::{field_id}).unwrap_or_default()"
            )
            .unwrap();
        }
        Type::Int64 | Type::Sint64 | Type::Sfixed64 => {
            writeln!(out, "{pad}    pub fn {name}(self) -> i64 {{").unwrap();
            writeln!(
                out,
                "{pad}        self.view.scalar::<i64>(Self::{field_id}).unwrap_or_default()"
            )
            .unwrap();
        }
        Type::Uint64 | Type::Fixed64 => {
            writeln!(out, "{pad}    pub fn {name}(self) -> u64 {{").unwrap();
            writeln!(
                out,
                "{pad}        self.view.scalar::<u64>(Self::{field_id}).unwrap_or_default()"
            )
            .unwrap();
        }
        Type::Float => {
            writeln!(out, "{pad}    pub fn {name}(self) -> f32 {{").unwrap();
            writeln!(
                out,
                "{pad}        self.view.scalar::<f32>(Self::{field_id}).unwrap_or_default()"
            )
            .unwrap();
        }
        Type::Double => {
            writeln!(out, "{pad}    pub fn {name}(self) -> f64 {{").unwrap();
            writeln!(
                out,
                "{pad}        self.view.scalar::<f64>(Self::{field_id}).unwrap_or_default()"
            )
            .unwrap();
        }
        Type::Enum => {
            writeln!(out, "{pad}    pub fn {name}(self) -> EnumValue {{").unwrap();
            writeln!(
                out,
                "{pad}        self.view.scalar::<EnumValue>(Self::{field_id}).unwrap_or_default()"
            )
            .unwrap();
        }
        Type::Message => {
            let rust_type = rust_type_name(registry, field.type_name())?;
            if registry.aliases.contains_key(field.type_name()) {
                writeln!(
                    out,
                    "{pad}    pub fn {name}(self) -> Option<{rust_type}<'a>> {{"
                )
                .unwrap();
                writeln!(
                    out,
                    "{pad}        {rust_type}::FromWords(self.view.field(Self::{field_id})?.object_words()?)"
                )
                .unwrap();
            } else {
                writeln!(
                    out,
                    "{pad}    pub fn {name}(self) -> Option<{rust_type}<'a>> {{"
                )
                .unwrap();
                writeln!(out, "{pad}        Some({rust_type} {{ view: self.view.message(Self::{field_id})? }})").unwrap();
            }
        }
        other => {
            return Err(format!(
                "unsupported field type {other:?} for {}",
                field.name()
            ));
        }
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
        Type::Int32 | Type::Sint32 | Type::Sfixed32 => {
            Some("Option<ScalarArray<'a, i32>>".to_owned())
        }
        Type::Uint32 | Type::Fixed32 => Some("Option<ScalarArray<'a, u32>>".to_owned()),
        Type::Int64 | Type::Sint64 | Type::Sfixed64 => {
            Some("Option<ScalarArray<'a, i64>>".to_owned())
        }
        Type::Uint64 | Type::Fixed64 => Some("Option<ScalarArray<'a, u64>>".to_owned()),
        Type::Float => Some("Option<ScalarArray<'a, f32>>".to_owned()),
        Type::Double => Some("Option<ScalarArray<'a, f64>>".to_owned()),
        Type::Enum => Some("Option<ScalarArray<'a, EnumValue>>".to_owned()),
        _ => None,
    }
}

fn repeated_scalar_expr(field: &FieldDescriptorProto, array_expr: &str) -> Result<String, String> {
    Ok(match field.r#type() {
        Type::Bool => format!("self.view.bools(Self::{})", field_const_name(field)),
        Type::Int32 | Type::Sint32 | Type::Sfixed32 => format!("{array_expr}.scalars::<i32>()"),
        Type::Uint32 | Type::Fixed32 => format!("{array_expr}.scalars::<u32>()"),
        Type::Int64 | Type::Sint64 | Type::Sfixed64 => format!("{array_expr}.scalars::<i64>()"),
        Type::Uint64 | Type::Fixed64 => format!("{array_expr}.scalars::<u64>()"),
        Type::Float => format!("{array_expr}.scalars::<f32>()"),
        Type::Double => format!("{array_expr}.scalars::<f64>()"),
        Type::Enum => format!("{array_expr}.scalars::<EnumValue>()"),
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

fn alias_map_view_return(
    field: &FieldDescriptorProto,
    registry: &Registry,
) -> Result<String, String> {
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
        Type::Enum => "EnumValue".to_owned(),
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

fn rust_mutable_base_name(registry: &Registry, full_name: &str) -> Result<String, String> {
    Ok(format!("{}Mutable", rust_type_name(registry, full_name)?))
}

fn flatten_name(full_name: &str, package: &str, is_alias: bool, has_parent: bool) -> String {
    let skip = if package.is_empty() {
        0
    } else {
        package.trim_start_matches('.').split('.').count()
    };
    let parts = full_name
        .trim_start_matches('.')
        .split('.')
        .skip(skip)
        .collect::<Vec<_>>();
    if is_alias && has_parent {
        parts.last().copied().unwrap_or_default().to_owned()
    } else {
        parts.join("")
    }
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

fn field_const_name(field: &FieldDescriptorProto) -> String {
    format!("{}_FIELD_ID", field.name().to_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    use prost_types::{EnumValueDescriptorProto, FieldOptions, MessageOptions};

    #[test]
    fn generates_readonly_and_extra_for_representative_schema() {
        let request = CodeGeneratorRequest {
            file_to_generate: vec!["schemas/complex.proto".to_owned()],
            parameter: Some("extra".to_owned()),
            proto_file: vec![representative_file()],
            compiler_version: None,
        };

        let response = generate_response(&request).unwrap();
        assert_eq!(response.file.len(), 2);
        assert_eq!(response.file[0].name(), "schemas/complex.pc.rs");
        assert_eq!(response.file[1].name(), "schemas/complex.pc-ex.rs");

        let readonly = response.file[0].content();
        for expected in [
            "pub mod demo {",
            "pub mod nested {",
            "pub enum Mode",
            "pub struct Child<'a>",
            "pub struct Root<'a>",
            "pub struct RootInner<'a>",
            "pub struct Floats<'a>",
            "pub struct AliasMap<'a>",
            "pub fn parent(self) -> Option<Root<'a>>",
            "pub fn labels(self) -> Option<ViewMap<'a, StringView<'a>, Child<'a>>>",
        ] {
            assert!(
                readonly.contains(expected),
                "missing readonly fragment: {expected}"
            );
        }
        assert!(!readonly.contains("OBSOLETE_FIELD_ID"));
        assert!(!readonly.contains("fn obsolete("));

        let extra = response.file[1].content();
        for expected in [
            "pub struct ChildMutable<'a>",
            "pub struct RootMutable<'a>",
            "pub struct RootInnerMutable<'a>",
            "pub type FloatsMutable<'a> = MutableArray<'a, f32>;",
            "pub type AliasMapMutable<'a> = MutableMap<'a, String, ChildMutable<'a>>;",
            "_labels: MutableMap<'a, String, ChildMutable<'a>>",
        ] {
            assert!(
                extra.contains(expected),
                "missing extra fragment: {expected}"
            );
        }
        assert!(!extra.contains("_obsolete:"));
        assert!(!extra.contains("fn obsolete("));
    }

    #[test]
    fn emits_only_requested_files_and_readonly_without_extra_parameter() {
        let dependency = FileDescriptorProto {
            name: Some("dependency.proto".to_owned()),
            message_type: vec![DescriptorProto {
                name: Some("Dependency".to_owned()),
                field: vec![field("value", 1, Label::Optional, Type::Int32, None)],
                ..DescriptorProto::default()
            }],
            ..FileDescriptorProto::default()
        };
        let request = CodeGeneratorRequest {
            file_to_generate: vec!["dependency.proto".to_owned()],
            parameter: None,
            proto_file: vec![representative_file(), dependency],
            compiler_version: None,
        };

        let response = generate_response(&request).unwrap();
        assert_eq!(response.file.len(), 1);
        assert_eq!(response.file[0].name(), "dependency.pc.rs");
        assert!(
            response.file[0]
                .content()
                .contains("pub struct Dependency<'a>")
        );
    }

    #[test]
    fn rejects_invalid_schema_before_generation() {
        let request = CodeGeneratorRequest {
            file_to_generate: vec!["invalid.proto".to_owned()],
            parameter: Some("extra".to_owned()),
            proto_file: vec![FileDescriptorProto {
                name: Some("invalid.proto".to_owned()),
                message_type: vec![DescriptorProto {
                    name: Some("Empty".to_owned()),
                    ..DescriptorProto::default()
                }],
                ..FileDescriptorProto::default()
            }],
            compiler_version: None,
        };

        let error = generate_response(&request).unwrap_err();
        assert!(error.contains("schema validation failed"));
        assert!(error.contains("EmptyMessage"));
    }

    #[test]
    fn converts_proto_filenames() {
        assert_eq!(convert_filename("dir/item.proto"), "dir/item.pc.rs");
        assert_eq!(convert_ex_filename("dir/item.proto"), "dir/item.pc-ex.rs");
        assert_eq!(convert_filename("schema"), "schema.pc.rs");
    }

    fn representative_file() -> FileDescriptorProto {
        let child = DescriptorProto {
            name: Some("Child".to_owned()),
            field: vec![
                field("name", 1, Label::Optional, Type::String, None),
                field(
                    "parent",
                    2,
                    Label::Optional,
                    Type::Message,
                    Some(".demo.nested.Root"),
                ),
            ],
            ..DescriptorProto::default()
        };
        let floats = DescriptorProto {
            name: Some("Floats".to_owned()),
            field: vec![field("_", 1, Label::Repeated, Type::Float, None)],
            ..DescriptorProto::default()
        };
        let alias_map = DescriptorProto {
            name: Some("AliasMap".to_owned()),
            nested_type: vec![map_entry(
                "EntriesEntry",
                Type::String,
                None,
                Type::Message,
                Some(".demo.nested.Child"),
            )],
            field: vec![field(
                "_",
                1,
                Label::Repeated,
                Type::Message,
                Some(".demo.nested.AliasMap.EntriesEntry"),
            )],
            ..DescriptorProto::default()
        };
        let mut obsolete = field("obsolete", 9, Label::Optional, Type::Int32, None);
        obsolete.options = Some(FieldOptions {
            deprecated: Some(true),
            ..FieldOptions::default()
        });
        let root = DescriptorProto {
            name: Some("Root".to_owned()),
            nested_type: vec![
                map_entry(
                    "LabelsEntry",
                    Type::String,
                    None,
                    Type::Message,
                    Some(".demo.nested.Child"),
                ),
                DescriptorProto {
                    name: Some("Inner".to_owned()),
                    field: vec![field("enabled", 1, Label::Optional, Type::Bool, None)],
                    ..DescriptorProto::default()
                },
            ],
            field: vec![
                field("id", 1, Label::Optional, Type::Int32, None),
                field("title", 2, Label::Optional, Type::String, None),
                field("payload", 3, Label::Optional, Type::Bytes, None),
                field(
                    "mode",
                    4,
                    Label::Optional,
                    Type::Enum,
                    Some(".demo.nested.Mode"),
                ),
                field(
                    "child",
                    5,
                    Label::Optional,
                    Type::Message,
                    Some(".demo.nested.Child"),
                ),
                field("values", 6, Label::Repeated, Type::Uint64, None),
                field(
                    "labels",
                    7,
                    Label::Repeated,
                    Type::Message,
                    Some(".demo.nested.Root.LabelsEntry"),
                ),
                field(
                    "floats",
                    8,
                    Label::Optional,
                    Type::Message,
                    Some(".demo.nested.Floats"),
                ),
                obsolete,
                field(
                    "inner",
                    10,
                    Label::Optional,
                    Type::Message,
                    Some(".demo.nested.Root.Inner"),
                ),
                field(
                    "alias_map",
                    11,
                    Label::Optional,
                    Type::Message,
                    Some(".demo.nested.AliasMap"),
                ),
            ],
            ..DescriptorProto::default()
        };

        FileDescriptorProto {
            name: Some("schemas/complex.proto".to_owned()),
            package: Some("demo.nested".to_owned()),
            enum_type: vec![EnumDescriptorProto {
                name: Some("Mode".to_owned()),
                value: vec![
                    enum_value("MODE_UNSPECIFIED", 0),
                    enum_value("MODE_ACTIVE", 1),
                ],
                ..EnumDescriptorProto::default()
            }],
            message_type: vec![child, floats, alias_map, root],
            ..FileDescriptorProto::default()
        }
    }

    fn enum_value(name: &str, number: i32) -> EnumValueDescriptorProto {
        EnumValueDescriptorProto {
            name: Some(name.to_owned()),
            number: Some(number),
            ..EnumValueDescriptorProto::default()
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
                field("key", 1, Label::Optional, key_type, key_type_name),
                field("value", 2, Label::Optional, value_type, value_type_name),
            ],
            options: Some(MessageOptions {
                map_entry: Some(true),
                ..MessageOptions::default()
            }),
            ..DescriptorProto::default()
        }
    }

    fn field(
        name: &str,
        number: i32,
        label: Label,
        ty: Type,
        type_name: Option<&str>,
    ) -> FieldDescriptorProto {
        FieldDescriptorProto {
            name: Some(name.to_owned()),
            number: Some(number),
            label: Some(label as i32),
            r#type: Some(ty as i32),
            type_name: type_name.map(str::to_owned),
            ..FieldDescriptorProto::default()
        }
    }
}

use std::env;
use std::fs;
use std::io::ErrorKind;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use prost::Message;
use prost_types::FileDescriptorSet;
use prost_types::compiler::{CodeGeneratorRequest, CodeGeneratorResponse};

#[derive(Debug)]
struct GeneratedFile {
    name: String,
    content: String,
}

fn main() {
    for cfg in [
        "protocache_test_has_protobuf_generated",
        "protocache_test_has_flatbuffers_generated",
        "protocache_test_has_fory_generated",
    ] {
        println!("cargo:rustc-check-cfg=cfg({cfg})");
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo_root = manifest_dir.parent().unwrap().to_path_buf();
    let fixture_root = repo_root.join("tests/fixtures");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let checked_in_pcrs_generated_path = manifest_dir.join("src/test.pc.rs");
    let checked_in_pcrs_extra_generated_path = manifest_dir.join("src/test.pc-ex.rs");
    let test_proto = fixture_root.join("proto/test.proto");
    let test_json = fixture_root.join("json/test.json");
    let required = [
        test_proto.clone(),
        test_json.clone(),
        fixture_root.join("proto/reflect-test.proto"),
        fixture_root.join("json/test-alias.json"),
    ];

    for path in &required {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!(
        "cargo:rerun-if-changed={}",
        fixture_root.join("benchmark/test.fbs").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        fixture_root.join("benchmark/test-fb.json").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        fixture_root.join("benchmark/test.fdl").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        checked_in_pcrs_generated_path.display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        checked_in_pcrs_extra_generated_path.display()
    );
    println!("cargo:rerun-if-env-changed=PROTOC_GEN_PCRS");
    println!("cargo:rerun-if-env-changed=PROTOC_GEN_PCRS_PARAMETER");
    println!("cargo:rerun-if-env-changed=FLATC");
    println!("cargo:rerun-if-env-changed=FORYC");

    let missing = required
        .iter()
        .filter(|path| !path.is_file())
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        println!(
            "cargo:warning=skipping protocache-test resource-dependent tests; missing: {}",
            missing.join(", ")
        );
        return;
    }

    let prost_proto = out_dir.join("test-prost.proto");
    let pcrs_proto = out_dir.join("test-pcrs.proto");
    let mut prost_source = fs::read_to_string(&test_proto).unwrap();
    prost_source = prost_source.replace("repeated float _ = 1;", "repeated float values = 1;");
    prost_source = prost_source.replace("repeated Vec1D _ = 1;", "repeated Vec1D values = 1;");
    prost_source =
        prost_source.replace("map<string,Array> _ = 1;", "map<string,Array> entries = 1;");
    prost_source = prost_source.replace(
        "\tArrMap arrays = 30;\n}",
        "\tArrMap arrays = 30;\n\trepeated Mode modev = 32;\n}",
    );
    fs::write(&prost_proto, prost_source).unwrap();

    let mut pcrs_source = fs::read_to_string(&test_proto).unwrap();
    pcrs_source = pcrs_source.replace(
        "\tArrMap arrays = 30;\n}",
        "\tArrMap arrays = 30;\n\trepeated Mode modev = 32;\n}",
    );
    fs::write(&pcrs_proto, pcrs_source).unwrap();

    if let Err(err) = prost_build::Config::new()
        .out_dir(&out_dir)
        .compile_protos(&[prost_proto], std::slice::from_ref(&out_dir))
    {
        println!("cargo:warning=skipping protocache-test resource-dependent tests: {err}");
        return;
    }

    if let Err(err) = try_generate_binary_fixtures(&test_proto, &test_json, &out_dir) {
        println!(
            "cargo:warning=skipping protocache-test resource-dependent tests; failed to generate Protobuf/ProtoCache fixtures: {err}"
        );
        return;
    }
    println!("cargo:rustc-cfg=protocache_test_has_protobuf_generated");

    let protoc_gen_pcrs = env::var_os("PROTOC_GEN_PCRS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("protoc-gen-pcrs"));
    let parameter = env::var("PROTOC_GEN_PCRS_PARAMETER").unwrap_or_else(|_| "extra".to_owned());
    if let Err(err) = try_regenerate_pcrs(
        &out_dir,
        &protoc_gen_pcrs,
        normalize_parameter(&parameter),
        &checked_in_pcrs_generated_path,
        &checked_in_pcrs_extra_generated_path,
    ) {
        println!("cargo:warning=skipping pcrs regeneration; using checked-in bindings: {err}");
    }

    let flatc = env::var_os("FLATC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("flatc"));
    match try_generate_flatbuffers(&fixture_root, &out_dir, &flatc) {
        Ok(()) => println!("cargo:rustc-cfg=protocache_test_has_flatbuffers_generated"),
        Err(err) => println!("cargo:warning=skipping FlatBuffers benchmarks/tests: {err}"),
    }

    let foryc = env::var_os("FORYC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("foryc"));
    match try_generate_fory(&fixture_root, &out_dir, &foryc) {
        Ok(()) => println!("cargo:rustc-cfg=protocache_test_has_fory_generated"),
        Err(err) => println!("cargo:warning=skipping Fory benchmarks/tests: {err}"),
    }
}

fn try_generate_binary_fixtures(schema: &Path, json: &Path, out_dir: &Path) -> Result<(), String> {
    let descriptor_path = out_dir.join("benchmark-fixture.desc");
    let proto_dir = schema
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", schema.display()))?;
    let proto_name = schema
        .file_name()
        .ok_or_else(|| format!("{} has no file name", schema.display()))?;
    let status = Command::new("protoc")
        .arg(format!("--proto_path={}", proto_dir.display()))
        .arg(format!(
            "--descriptor_set_out={}",
            descriptor_path.display()
        ))
        .arg(proto_name)
        .status()
        .map_err(|err| format!("failed to launch protoc: {err}"))?;
    if !status.success() {
        return Err(format!("protoc exited with status {status}"));
    }

    let descriptor_bytes = fs::read(&descriptor_path).map_err(|err| err.to_string())?;
    let descriptor_set =
        FileDescriptorSet::decode(descriptor_bytes.as_slice()).map_err(|err| err.to_string())?;
    let pool = prost_reflect::DescriptorPool::from_file_descriptor_set(descriptor_set)
        .map_err(|err| err.to_string())?;
    let descriptor = pool
        .get_message_by_name("test.Main")
        .ok_or_else(|| "missing test.Main descriptor".to_owned())?;
    let message =
        protocache_extension::utils::load_json(json, descriptor).map_err(|err| err.to_string())?;

    fs::write(out_dir.join("test.pb"), message.encode_to_vec()).map_err(|err| err.to_string())?;
    let words = protocache_extension::utils::serialize_dynamic(&message)
        .map_err(|err| format!("failed to serialize ProtoCache fixture: {err:?}"))?;
    let bytes = words
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    fs::write(out_dir.join("test.pc"), bytes).map_err(|err| err.to_string())
}

fn try_regenerate_pcrs(
    out_dir: &Path,
    generator: &Path,
    parameter: Option<String>,
    readonly_path: &Path,
    extra_path: &Path,
) -> Result<(), String> {
    let descriptor_path = out_dir.join("benchmark-test.desc");
    let status = Command::new("protoc")
        .arg(format!("--proto_path={}", out_dir.display()))
        .arg(format!(
            "--descriptor_set_out={}",
            descriptor_path.display()
        ))
        .arg("test-pcrs.proto")
        .status()
        .map_err(|err| format!("failed to launch protoc: {err}"))?;
    if !status.success() {
        return Err(format!("protoc exited with status {status}"));
    }
    let descriptor_bytes = fs::read(&descriptor_path).map_err(|err| err.to_string())?;
    let descriptor_set =
        FileDescriptorSet::decode(descriptor_bytes.as_slice()).map_err(|err| err.to_string())?;
    let file = descriptor_set
        .file
        .into_iter()
        .find(|file| file.name() == "test-pcrs.proto")
        .ok_or_else(|| "missing test-pcrs.proto descriptor".to_owned())?;
    match generate_with_protoc_gen_pcrs(generator, file, parameter)? {
        Some(files) => write_generated_files(&files, readonly_path, extra_path),
        None => return Err(format!("{generator:?} was not found")),
    }
    Ok(())
}

fn try_generate_flatbuffers(
    fixture_root: &std::path::Path,
    out_dir: &std::path::Path,
    flatc: &std::path::Path,
) -> Result<(), String> {
    const REQUIRED_FLATC_VERSION: &str = "25.12.19";

    let output = Command::new(flatc)
        .arg("--version")
        .output()
        .map_err(|err| format!("failed to launch {flatc:?}: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "{flatc:?} --version exited with status {}",
            output.status
        ));
    }
    let version_output = String::from_utf8_lossy(&output.stdout);
    let actual_version = version_output
        .split_whitespace()
        .last()
        .ok_or_else(|| format!("{flatc:?} returned an empty version"))?;
    if actual_version != REQUIRED_FLATC_VERSION {
        return Err(format!(
            "{flatc:?} version {actual_version} is incompatible; expected {REQUIRED_FLATC_VERSION}"
        ));
    }

    let source_path = fixture_root.join("benchmark/test.fbs");
    let json_path = fixture_root.join("benchmark/test-fb.json");
    for path in [&source_path, &json_path] {
        if !path.is_file() {
            return Err(format!("missing {}", path.display()));
        }
    }

    let status = Command::new(flatc)
        .arg("--binary")
        .arg("-o")
        .arg(out_dir)
        .arg(&source_path)
        .arg(&json_path)
        .status()
        .map_err(|err| format!("failed to launch {flatc:?}: {err}"))?;
    if !status.success() {
        return Err(format!(
            "{flatc:?} binary generation exited with status {status}"
        ));
    }
    let binary = out_dir.join("test-fb.bin");
    if !binary.is_file() {
        return Err(format!("{flatc:?} did not generate {}", binary.display()));
    }

    let mut source = fs::read_to_string(&source_path)
        .map_err(|err| format!("{}: {err}", source_path.display()))?;
    source = source.replace("_:[float];", "values:[float];");
    source = source.replace("_:[Vec1D];", "values:[Vec1D];");
    source = source.replace("_:[ArrMapEntry];", "entries:[ArrMapEntry];");
    source = source.replace("_:[byte];", "bytes:[byte];");
    let rust_schema = out_dir.join("test.fbs");
    fs::write(&rust_schema, source).map_err(|err| err.to_string())?;

    let status = Command::new(flatc)
        .arg("--rust")
        .arg("-o")
        .arg(out_dir)
        .arg(&rust_schema)
        .status()
        .map_err(|err| format!("failed to launch {flatc:?}: {err}"))?;
    if !status.success() {
        return Err(format!(
            "{flatc:?} Rust generation exited with status {status}"
        ));
    }

    let generated_path = out_dir.join("test_generated.rs");
    let mut generated = fs::read_to_string(&generated_path).map_err(|err| err.to_string())?;
    rewrite_flatbuffers_generated_identifiers(&mut generated);
    fs::write(generated_path, generated).map_err(|err| err.to_string())
}

fn try_generate_fory(
    fixture_root: &std::path::Path,
    out_dir: &std::path::Path,
    foryc: &std::path::Path,
) -> Result<(), String> {
    let fixture = fixture_root.join("benchmark/test.fr");
    if !fixture.is_file() {
        return Err(format!("missing {}", fixture.display()));
    }
    let schema = fixture_root.join("benchmark/test.fdl");
    if !schema.is_file() {
        return Err(format!("missing {}", schema.display()));
    }
    let status = Command::new(foryc)
        .arg(&schema)
        .arg("--rust_out")
        .arg(out_dir)
        .status()
        .map_err(|err| format!("failed to launch {foryc:?}: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{foryc:?} exited with status {status}"))
    }
}

fn generate_with_protoc_gen_pcrs(
    generator: &Path,
    file: prost_types::FileDescriptorProto,
    parameter: Option<String>,
) -> Result<Option<Vec<GeneratedFile>>, String> {
    let request = CodeGeneratorRequest {
        file_to_generate: vec![file.name().to_owned()],
        parameter,
        proto_file: vec![file],
        compiler_version: None,
    };
    let mut input = Vec::new();
    request
        .encode(&mut input)
        .map_err(|err| format!("failed to encode CodeGeneratorRequest: {err}"))?;

    let child = Command::new(generator)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|err| {
            if err.kind() == ErrorKind::NotFound {
                return "not found".to_owned();
            }
            format!("failed to launch {:?}: {err}", generator)
        });
    let mut child = match child {
        Ok(child) => child,
        Err(err) if err == "not found" => return Ok(None),
        Err(err) => return Err(err),
    };

    child
        .stdin
        .take()
        .ok_or_else(|| format!("failed to open stdin for {:?}", generator))?
        .write_all(&input)
        .map_err(|err| format!("failed to write request to {:?}: {err}", generator))?;

    let mut output = Vec::new();
    child
        .stdout
        .take()
        .ok_or_else(|| format!("failed to open stdout for {:?}", generator))?
        .read_to_end(&mut output)
        .map_err(|err| format!("failed to read response from {:?}: {err}", generator))?;

    let status = child
        .wait()
        .map_err(|err| format!("failed to wait for {:?}: {err}", generator))?;
    if !status.success() {
        return Err(format!("{:?} exited with status {status}", generator));
    }

    let response = CodeGeneratorResponse::decode(output.as_slice())
        .map_err(|err| format!("failed to decode CodeGeneratorResponse: {err}"))?;
    if let Some(error) = response.error {
        return Err(format!("protoc-gen-pcrs returned error: {error}"));
    }

    let generated_files = response
        .file
        .into_iter()
        .map(|file| {
            let name = file
                .name
                .ok_or_else(|| "protoc-gen-pcrs returned a file without name".to_owned())?;
            let content = file
                .content
                .ok_or_else(|| format!("protoc-gen-pcrs returned {name} without content"))?;
            Ok(GeneratedFile { name, content })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if generated_files.is_empty() {
        return Err("protoc-gen-pcrs did not return generated content".to_owned());
    }
    Ok(Some(generated_files))
}

fn write_if_changed(path: &Path, contents: &str) {
    match fs::read_to_string(path) {
        Ok(existing) if existing == contents => {}
        _ => fs::write(path, contents).unwrap(),
    }
}

fn write_generated_files(
    generated_files: &[GeneratedFile],
    readonly_path: &Path,
    extra_path: &Path,
) {
    let mut readonly = None;
    let mut extra = None;
    for file in generated_files {
        if file.name.ends_with(".pc.rs") {
            readonly = Some(file.content.as_str());
        } else if file.name.ends_with(".pc-ex.rs") {
            extra = Some(file.content.as_str());
        }
    }

    write_if_changed(
        readonly_path,
        readonly.expect("protoc-gen-pcrs did not return a .pc.rs file"),
    );
    match extra {
        Some(extra) => write_if_changed(extra_path, extra),
        None => write_if_changed(extra_path, ""),
    }
}

fn normalize_parameter(parameter: &str) -> Option<String> {
    let trimmed = parameter.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn rewrite_flatbuffers_generated_identifiers(generated: &mut String) {
    *generated = generated.replacen(
        "if let Some(x) = args._ { builder.add__(x); }",
        "if let Some(x) = args.values { builder.add_values(x); }",
        3,
    );
    *generated = generated.replacen(
        "if let Some(x) = args._ { builder.add__(x); }",
        "if let Some(x) = args.entries { builder.add_entries(x); }",
        1,
    );
    *generated = generated.replacen(
        "if let Some(x) = args._ { builder.add__(x); }",
        "if let Some(x) = args.bytes { builder.add_bytes(x); }",
        1,
    );

    *generated = generated.replace(
        "pub fn _(&self) -> Option<flatbuffers::Vector<'a, f32>> {",
        "pub fn values(&self) -> Option<flatbuffers::Vector<'a, f32>> {",
    );
    *generated = generated.replace(
        "pub fn _(&self) -> Option<flatbuffers::Vector<'a, flatbuffers::ForwardsUOffset<Vec1D<'a>>>> {",
        "pub fn values(&self) -> Option<flatbuffers::Vector<'a, flatbuffers::ForwardsUOffset<Vec1D<'a>>>> {",
    );
    *generated = generated.replace(
        "pub fn _(&self) -> Option<flatbuffers::Vector<'a, flatbuffers::ForwardsUOffset<ArrMapEntry<'a>>>> {",
        "pub fn entries(&self) -> Option<flatbuffers::Vector<'a, flatbuffers::ForwardsUOffset<ArrMapEntry<'a>>>> {",
    );
    *generated = generated.replace(
        "pub fn _(&self) -> Option<&'a [i8]> {",
        "pub fn bytes(&self) -> Option<&'a [i8]> {",
    );

    *generated = generated.replacen(
        "pub _: Option<flatbuffers::WIPOffset<flatbuffers::Vector<'a, f32>>>,",
        "pub values: Option<flatbuffers::WIPOffset<flatbuffers::Vector<'a, f32>>>,",
        2,
    );
    *generated = generated.replace(
        "pub _: Option<flatbuffers::WIPOffset<flatbuffers::Vector<'a, flatbuffers::ForwardsUOffset<Vec1D<'a>>>>>,",
        "pub values: Option<flatbuffers::WIPOffset<flatbuffers::Vector<'a, flatbuffers::ForwardsUOffset<Vec1D<'a>>>>>,",
    );
    *generated = generated.replace(
        "pub _: Option<flatbuffers::WIPOffset<flatbuffers::Vector<'a, flatbuffers::ForwardsUOffset<ArrMapEntry<'a>>>>>,",
        "pub entries: Option<flatbuffers::WIPOffset<flatbuffers::Vector<'a, flatbuffers::ForwardsUOffset<ArrMapEntry<'a>>>>>,",
    );
    *generated = generated.replace(
        "pub _: Option<flatbuffers::WIPOffset<flatbuffers::Vector<'a, i8>>>,",
        "pub bytes: Option<flatbuffers::WIPOffset<flatbuffers::Vector<'a, i8>>>,",
    );

    *generated = generated.replacen("_: None,", "values: None,", 3);
    *generated = generated.replacen("_: None,", "entries: None,", 1);
    *generated = generated.replacen("_: None,", "bytes: None,", 1);

    *generated = generated.replacen(
        "pub fn add__(&mut self, _: flatbuffers::WIPOffset<flatbuffers::Vector<'b , f32>>) {",
        "pub fn add_values(&mut self, values: flatbuffers::WIPOffset<flatbuffers::Vector<'b , f32>>) {",
        2,
    );
    *generated = generated.replace(
        "pub fn add__(&mut self, _: flatbuffers::WIPOffset<flatbuffers::Vector<'b , flatbuffers::ForwardsUOffset<Vec1D<'b >>>>) {",
        "pub fn add_values(&mut self, values: flatbuffers::WIPOffset<flatbuffers::Vector<'b , flatbuffers::ForwardsUOffset<Vec1D<'b >>>>) {",
    );
    *generated = generated.replace(
        "pub fn add__(&mut self, _: flatbuffers::WIPOffset<flatbuffers::Vector<'b , flatbuffers::ForwardsUOffset<ArrMapEntry<'b >>>>) {",
        "pub fn add_entries(&mut self, entries: flatbuffers::WIPOffset<flatbuffers::Vector<'b , flatbuffers::ForwardsUOffset<ArrMapEntry<'b >>>>) {",
    );
    *generated = generated.replace(
        "pub fn add__(&mut self, _: flatbuffers::WIPOffset<flatbuffers::Vector<'b , i8>>) {",
        "pub fn add_bytes(&mut self, bytes: flatbuffers::WIPOffset<flatbuffers::Vector<'b , i8>>) {",
    );

    *generated = generated.replacen(
        "self.fbb_.push_slot_always::<flatbuffers::WIPOffset<_>>(Vec1D::VT__, _);",
        "self.fbb_.push_slot_always::<flatbuffers::WIPOffset<_>>(Vec1D::VT__, values);",
        1,
    );
    *generated = generated.replacen(
        "self.fbb_.push_slot_always::<flatbuffers::WIPOffset<_>>(Array::VT__, _);",
        "self.fbb_.push_slot_always::<flatbuffers::WIPOffset<_>>(Array::VT__, values);",
        1,
    );
    *generated = generated.replace(
        "self.fbb_.push_slot_always::<flatbuffers::WIPOffset<_>>(Vec2D::VT__, _);",
        "self.fbb_.push_slot_always::<flatbuffers::WIPOffset<_>>(Vec2D::VT__, values);",
    );
    *generated = generated.replace(
        "self.fbb_.push_slot_always::<flatbuffers::WIPOffset<_>>(ArrMap::VT__, _);",
        "self.fbb_.push_slot_always::<flatbuffers::WIPOffset<_>>(ArrMap::VT__, entries);",
    );
    *generated = generated.replace(
        "self.fbb_.push_slot_always::<flatbuffers::WIPOffset<_>>(Bytes::VT__, _);",
        "self.fbb_.push_slot_always::<flatbuffers::WIPOffset<_>>(Bytes::VT__, bytes);",
    );

    *generated = generated.replacen(
        r#"ds.field("_", &self._());"#,
        r#"ds.field("values", &self.values());"#,
        3,
    );
    *generated = generated.replacen(
        r#"ds.field("_", &self._());"#,
        r#"ds.field("entries", &self.entries());"#,
        1,
    );
    *generated = generated.replacen(
        r#"ds.field("_", &self._());"#,
        r#"ds.field("bytes", &self.bytes());"#,
        1,
    );

    *generated = generated.replacen(
        r#".visit_field::<flatbuffers::ForwardsUOffset<flatbuffers::Vector<'_, f32>>>("_", Self::VT__, false)?"#,
        r#".visit_field::<flatbuffers::ForwardsUOffset<flatbuffers::Vector<'_, f32>>>("values", Self::VT__, false)?"#,
        2,
    );
    *generated = generated.replace(
        r#".visit_field::<flatbuffers::ForwardsUOffset<flatbuffers::Vector<'_, flatbuffers::ForwardsUOffset<Vec1D>>>>("_", Self::VT__, false)?"#,
        r#".visit_field::<flatbuffers::ForwardsUOffset<flatbuffers::Vector<'_, flatbuffers::ForwardsUOffset<Vec1D>>>>("values", Self::VT__, false)?"#,
    );
    *generated = generated.replace(
        r#".visit_field::<flatbuffers::ForwardsUOffset<flatbuffers::Vector<'_, flatbuffers::ForwardsUOffset<ArrMapEntry>>>>("_", Self::VT__, false)?"#,
        r#".visit_field::<flatbuffers::ForwardsUOffset<flatbuffers::Vector<'_, flatbuffers::ForwardsUOffset<ArrMapEntry>>>>("entries", Self::VT__, false)?"#,
    );
    *generated = generated.replace(
        r#".visit_field::<flatbuffers::ForwardsUOffset<flatbuffers::Vector<'_, i8>>>("_", Self::VT__, false)?"#,
        r#".visit_field::<flatbuffers::ForwardsUOffset<flatbuffers::Vector<'_, i8>>>("bytes", Self::VT__, false)?"#,
    );
}

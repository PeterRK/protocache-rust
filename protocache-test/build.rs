use std::env;
use std::fs;
use std::io::ErrorKind;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use prost::Message;
use prost_types::compiler::{CodeGeneratorRequest, CodeGeneratorResponse};
use prost_types::FileDescriptorSet;

#[derive(Debug)]
struct GeneratedFile {
    name: String,
    content: String,
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo_root = manifest_dir.parent().unwrap().to_path_buf();
    let fixture_root = repo_root.join("tests/fixtures");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let checked_in_pcrs_generated_path = manifest_dir.join("src/test.pc.rs");
    let checked_in_pcrs_extra_generated_path = manifest_dir.join("src/test.pc-ex.rs");
    let flatc = env::var_os("FLATC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("flatc"));
    let protoc_gen_pcrs = env::var_os("PROTOC_GEN_PCRS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("protoc-gen-pcrs"));
    let protoc_gen_pcrs_parameter = env::var("PROTOC_GEN_PCRS_PARAMETER")
        .unwrap_or_else(|_| "extra".to_owned());

    println!(
        "cargo:rerun-if-changed={}",
        fixture_root.join("proto/test.proto").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        fixture_root.join("benchmark/test.fbs").display()
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

    let prost_proto = out_dir.join("test-prost.proto");
    let pcrs_proto = out_dir.join("test-pcrs.proto");
    let mut prost_source = fs::read_to_string(fixture_root.join("proto/test.proto")).unwrap();
    prost_source = prost_source.replace("repeated float _ = 1;", "repeated float values = 1;");
    prost_source = prost_source.replace("repeated Vec1D _ = 1;", "repeated Vec1D values = 1;");
    prost_source = prost_source.replace("map<string,Array> _ = 1;", "map<string,Array> entries = 1;");
    prost_source = prost_source.replace(
        "\tArrMap arrays = 30;\n}",
        "\tArrMap arrays = 30;\n\trepeated Mode modev = 32;\n}",
    );
    fs::write(&prost_proto, prost_source).unwrap();

    let mut pcrs_source = fs::read_to_string(fixture_root.join("proto/test.proto")).unwrap();
    pcrs_source = pcrs_source.replace(
        "\tArrMap arrays = 30;\n}",
        "\tArrMap arrays = 30;\n\trepeated Mode modev = 32;\n}",
    );
    fs::write(&pcrs_proto, pcrs_source).unwrap();

    prost_build::Config::new()
        .out_dir(&out_dir)
        .compile_protos(&[prost_proto], &[out_dir.clone()])
        .unwrap();

    let descriptor_path = out_dir.join("benchmark-test.desc");
    let status = Command::new("protoc")
        .arg(format!("--proto_path={}", out_dir.display()))
        .arg(format!("--descriptor_set_out={}", descriptor_path.display()))
        .arg("test-pcrs.proto")
        .status()
        .unwrap();
    if !status.success() {
        panic!("protoc failed to generate benchmark descriptor set");
    }
    let descriptor_bytes = fs::read(&descriptor_path).unwrap();
    let descriptor_set = FileDescriptorSet::decode(descriptor_bytes.as_slice()).unwrap();
    let pcrs_file = descriptor_set
        .file
        .into_iter()
        .find(|file| file.name() == "test-pcrs.proto")
        .unwrap();
    match generate_with_protoc_gen_pcrs(&protoc_gen_pcrs, pcrs_file, normalize_parameter(&protoc_gen_pcrs_parameter)).unwrap() {
        Some(generated_files) => write_generated_files(
            &generated_files,
            &checked_in_pcrs_generated_path,
            &checked_in_pcrs_extra_generated_path,
        ),
        None => {
            println!(
                "cargo:warning=skipping pcrs regeneration because {:?} was not found; using checked-in {} and {}",
                protoc_gen_pcrs,
                checked_in_pcrs_generated_path.display(),
                checked_in_pcrs_extra_generated_path.display()
            );
        }
    }

    let flatbuffers_schema = out_dir.join("test.fbs");
    let mut flatbuffers_source = fs::read_to_string(fixture_root.join("benchmark/test.fbs")).unwrap();
    flatbuffers_source = flatbuffers_source.replace("_:[float];", "values:[float];");
    flatbuffers_source = flatbuffers_source.replace("_:[Vec1D];", "values:[Vec1D];");
    flatbuffers_source = flatbuffers_source.replace("_:[ArrMapEntry];", "entries:[ArrMapEntry];");
    flatbuffers_source = flatbuffers_source.replace("_:[byte];", "bytes:[byte];");
    fs::write(&flatbuffers_schema, flatbuffers_source).unwrap();

    let status = Command::new(&flatc)
        .arg("--rust")
        .arg("-o")
        .arg(&out_dir)
        .arg(&flatbuffers_schema)
        .status()
        .unwrap();
    if !status.success() {
        panic!("flatc {:?} failed with status {status}", flatc);
    }

    let generated_path = out_dir.join("test_generated.rs");
    let mut generated = fs::read_to_string(&generated_path).unwrap();
    rewrite_flatbuffers_generated_identifiers(&mut generated);
    fs::write(generated_path, generated).unwrap();
}

fn generate_with_protoc_gen_pcrs(
    generator: &PathBuf,
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
                return format!("not found");
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

fn write_if_changed(path: &PathBuf, contents: &str) {
    match fs::read_to_string(path) {
        Ok(existing) if existing == contents => {}
        _ => fs::write(path, contents).unwrap(),
    }
}

fn write_generated_files(
    generated_files: &[GeneratedFile],
    readonly_path: &PathBuf,
    extra_path: &PathBuf,
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

    *generated = generated.replacen(r#"ds.field("_", &self._());"#, r#"ds.field("values", &self.values());"#, 3);
    *generated = generated.replacen(r#"ds.field("_", &self._());"#, r#"ds.field("entries", &self.entries());"#, 1);
    *generated = generated.replacen(r#"ds.field("_", &self._());"#, r#"ds.field("bytes", &self.bytes());"#, 1);

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

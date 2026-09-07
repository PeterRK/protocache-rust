use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use prost::Message;
use prost_types::FileDescriptorSet;

fn main() {
    for cfg in [
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
    println!("cargo:rerun-if-env-changed=FLATC");
    println!("cargo:rerun-if-env-changed=FORYC");

    for path in &required {
        assert!(
            path.is_file(),
            "required fixture missing: {}",
            path.display()
        );
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

    prost_build::Config::new()
        .out_dir(&out_dir)
        .compile_protos(&[prost_proto], std::slice::from_ref(&out_dir))
        .expect("failed to generate Protobuf test bindings; protoc is required");
    try_generate_binary_fixtures(&test_proto, &test_json, &out_dir)
        .expect("failed to generate required Protobuf/ProtoCache fixtures");

    // Builds may refresh generated bindings, but never overwrite source snapshots.
    let readonly_out = out_dir.join("test.pc.rs");
    let extra_out = out_dir.join("test.pc-ex.rs");
    fs::copy(&checked_in_pcrs_generated_path, &readonly_out).unwrap();
    fs::copy(&checked_in_pcrs_extra_generated_path, &extra_out).unwrap();
    if let Some(generator) = env::var_os("PROTOC_GEN_PCRS") {
        let generator = PathBuf::from(generator);
        println!("cargo:rerun-if-changed={}", generator.display());
        try_regenerate_pcrs(&out_dir, &generator, &readonly_out, &extra_out)
            .expect("failed to generate requested ProtoCache bindings");
    }

    if env::var_os("CARGO_FEATURE_FLATBUFFERS_BENCH").is_some() {
        let flatc = env::var_os("FLATC")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("flatc"));
        try_generate_flatbuffers(&fixture_root, &out_dir, &flatc)
            .expect("flatbuffers-bench requires the matching flatc compiler");
        println!("cargo:rustc-cfg=protocache_test_has_flatbuffers_generated");
    }
    if env::var_os("CARGO_FEATURE_FORY_BENCH").is_some() {
        let foryc = env::var_os("FORYC")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("foryc"));
        try_generate_fory(&fixture_root, &out_dir, &foryc)
            .expect("fory-bench requires foryc and its fixtures");
        println!("cargo:rustc-cfg=protocache_test_has_fory_generated");
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
    readonly_path: &Path,
    extra_path: &Path,
) -> Result<(), String> {
    let status = Command::new("protoc")
        .arg(format!("--proto_path={}", out_dir.display()))
        .arg(format!("--plugin=protoc-gen-pcrs={}", generator.display()))
        .arg(format!("--pcrs_out=extra:{}", out_dir.display()))
        .arg("test-pcrs.proto")
        .status()
        .map_err(|err| format!("failed to launch protoc: {err}"))?;
    if !status.success() {
        return Err(format!("protoc exited with status {status}"));
    }
    fs::rename(out_dir.join("test-pcrs.pc.rs"), readonly_path).map_err(|err| err.to_string())?;
    fs::rename(out_dir.join("test-pcrs.pc-ex.rs"), extra_path).map_err(|err| err.to_string())?;
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

    Ok(())
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

use std::fs;
use std::path::Path;
use std::process::Command;

fn run(command: &mut Command) {
    let output = command.output().expect("failed to start verification tool");
    assert!(
        output.status.success(),
        "{command:?}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn generated_names_and_recursive_messages_compile_and_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let src = dir.path().join("src");
    fs::create_dir(&src).unwrap();
    run(Command::new("protoc")
        .arg(format!("--proto_path={}", fixtures.display()))
        .arg(format!(
            "--plugin=protoc-gen-pcrs={}",
            env!("CARGO_BIN_EXE_protoc-gen-pcrs")
        ))
        .arg(format!("--pcrs_out=extra:{}", src.display()))
        .args(["shared.proto", "global.proto", "names.proto"]));
    fs::copy(fixtures.join("client.rs"), src.join("main.rs")).unwrap();
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("protocache-core");
    let dependency = if runtime.join("Cargo.toml").is_file() {
        format!("{{ path = {runtime:?} }}")
    } else {
        format!("\"={}\"", env!("CARGO_PKG_VERSION"))
    };
    fs::write(dir.path().join("Cargo.toml"), format!(
        "[package]\nname = \"pcrs-regression-client\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[dependencies]\nprotocache-core = {dependency}\n")).unwrap();
    run(Command::new(env!("CARGO"))
        .args(["run", "--quiet", "--offline", "--manifest-path"])
        .arg(dir.path().join("Cargo.toml"))
        // Use a separate target directory while the parent Cargo holds its lock.
        .env("CARGO_TARGET_DIR", dir.path().join("target")));
    // Merge identical packages and shared package prefixes in the combined entry.
    run(Command::new("protoc")
        .arg(format!("--proto_path={}", fixtures.display()))
        .arg(format!(
            "--plugin=protoc-gen-pcrs={}",
            env!("CARGO_BIN_EXE_protoc-gen-pcrs")
        ))
        .arg(format!("--pcrs_out=extra:{}", src.display()))
        .args([
            "shared.proto",
            "global.proto",
            "names.proto",
            "peer.proto",
            "sibling.proto",
        ]));
    let mut client = fs::read_to_string(fixtures.join("client.rs")).unwrap();
    let start = client.find("mod bindings {").unwrap();
    let end = client[start..].find("\n}").unwrap() + start + 2;
    client.replace_range(start..end, "mod bindings { include!(\"pcrs-mod.rs\"); }");
    client = client.replace("fn main() {", r#"fn main() {
        let mut peer = PeerMutable::New();
        *peer.root().child().name() = "same package".into();
        let words = peer.SerializeWords().unwrap();
        assert_eq!(app::inner::Peer::FromWords(&words).unwrap().root().unwrap().child().unwrap().name(), Some("same package"));
        let mut other = OtherMutable::New();
        *other.local().value() = 71;
        let words = other.SerializeWords().unwrap();
        let local: app::inner::Common<'_> = app::other::Other::FromWords(&words).unwrap().local().unwrap();
        assert_eq!(local.value(), 71);
    "#);
    fs::write(src.join("main.rs"), client).unwrap();
    run(Command::new(env!("CARGO"))
        .args(["run", "--quiet", "--offline", "--manifest-path"])
        .arg(dir.path().join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", dir.path().join("target")));
}

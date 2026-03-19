use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use prost::Message;
use prost_reflect::{DescriptorError as ReflectDescriptorError, DescriptorPool as ReflectDescriptorPool};
use prost_types::{FileDescriptorProto, FileDescriptorSet};
use crate::{DescriptorPool, RegisterError};
use tempfile::TempDir;

#[derive(Debug)]
pub enum ProtoError {
    Io(std::io::Error),
    ProtocFailed { stderr: String },
    Decode(prost::DecodeError),
    Reflect(ReflectDescriptorError),
    MissingFile { name: String },
    Register(RegisterError),
}

impl From<std::io::Error> for ProtoError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<prost::DecodeError> for ProtoError {
    fn from(value: prost::DecodeError) -> Self {
        Self::Decode(value)
    }
}

impl From<ReflectDescriptorError> for ProtoError {
    fn from(value: ReflectDescriptorError) -> Self {
        Self::Reflect(value)
    }
}

impl From<RegisterError> for ProtoError {
    fn from(value: RegisterError) -> Self {
        Self::Register(value)
    }
}

impl std::fmt::Display for ProtoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::ProtocFailed { stderr } => write!(f, "protoc failed: {stderr}"),
            Self::Decode(err) => write!(f, "{err}"),
            Self::Reflect(err) => write!(f, "{err}"),
            Self::MissingFile { name } => write!(f, "missing file in descriptor set: {name}"),
            Self::Register(err) => write!(f, "{err:?}"),
        }
    }
}

impl std::error::Error for ProtoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Decode(err) => Some(err),
            Self::Reflect(err) => Some(err),
            Self::ProtocFailed { .. } | Self::MissingFile { .. } | Self::Register(_) => None,
        }
    }
}

pub fn parse_proto(source: &str, file_name: &str) -> Result<FileDescriptorProto, ProtoError> {
    let temp_dir = tempfile::Builder::new()
        .prefix("parse-proto-")
        .tempdir()?;
    let proto_path = temp_dir.path().join(file_name);
    if let Some(parent) = proto_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&proto_path, source)?;

    parse_proto_file_with_imports(&proto_path, &[temp_dir.path().to_path_buf()])
}

pub fn parse_proto_file(path: impl AsRef<Path>) -> Result<FileDescriptorProto, ProtoError> {
    parse_proto_file_with_imports(path.as_ref(), &[])
}

pub fn load_descriptor_pool_from_proto(
    source: &str,
    file_name: &str,
) -> Result<DescriptorPool, ProtoError> {
    let temp_dir = tempfile::Builder::new()
        .prefix("parse-proto-")
        .tempdir()?;
    let proto_path = temp_dir.path().join(file_name);
    if let Some(parent) = proto_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&proto_path, source)?;

    let set = parse_proto_file_set_with_imports(&proto_path, &[temp_dir.path().to_path_buf()])?;
    let mut pool = DescriptorPool::default();
    for file in &set.file {
        pool.register(file)?;
    }
    Ok(pool)
}

pub fn load_descriptor_pool_from_proto_file(
    path: impl AsRef<Path>,
) -> Result<DescriptorPool, ProtoError> {
    let set = parse_proto_file_set_with_imports(path.as_ref(), &[])?;
    let mut pool = DescriptorPool::default();
    for file in &set.file {
        pool.register(file)?;
    }
    Ok(pool)
}

pub fn load_reflect_descriptor_pool_from_proto_file(
    path: impl AsRef<Path>,
) -> Result<ReflectDescriptorPool, ProtoError> {
    let set = parse_proto_file_set_with_imports(path.as_ref(), &[])?;
    ReflectDescriptorPool::from_file_descriptor_set(set)
        .map_err(ProtoError::Reflect)
}

fn parse_proto_file_with_imports(
    path: &Path,
    extra_imports: &[PathBuf],
) -> Result<FileDescriptorProto, ProtoError> {
    let set = parse_proto_file_set_with_imports(path, extra_imports)?;
    find_target_file(set, path)
}

fn parse_proto_file_set_with_imports(
    path: &Path,
    extra_imports: &[PathBuf],
) -> Result<FileDescriptorSet, ProtoError> {
    let temp_dir = tempdir_with_prefix("descriptor-out-")?;
    let descriptor_path = temp_dir.path().join("out.pb");

    let mut command = Command::new("protoc");
    if let Some(parent) = path.parent() {
        command.arg(format!("--proto_path={}", parent.display()));
    }
    for import in extra_imports {
        command.arg(format!("--proto_path={}", import.display()));
    }
    command.arg("--include_imports");
    command.arg(format!("--descriptor_set_out={}", descriptor_path.display()));
    command.arg(path.file_name().unwrap_or_else(|| OsStr::new("input.proto")));

    let output = command.current_dir(path.parent().unwrap_or_else(|| Path::new("."))).output()?;
    if !output.status.success() {
        return Err(ProtoError::ProtocFailed {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    let bytes = fs::read(&descriptor_path)?;
    FileDescriptorSet::decode(bytes.as_slice()).map_err(ProtoError::Decode)
}

fn find_target_file(set: FileDescriptorSet, path: &Path) -> Result<FileDescriptorProto, ProtoError> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned();
    set.file
        .into_iter()
        .find(|file| file.name() == file_name)
        .ok_or(ProtoError::MissingFile { name: file_name })
}

fn tempdir_with_prefix(prefix: &str) -> Result<TempDir, ProtoError> {
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir()
        .map_err(ProtoError::Io)
}

#[cfg(test)]
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write as _;

    fn fixture_schema() -> PathBuf {
        workspace_root().join("tests/fixtures/proto/test.proto")
    }

    fn write_temp_proto_files(files: &[(&str, &str)]) -> TempDir {
        let dir = tempfile::Builder::new()
            .prefix("schema-imports-")
            .tempdir()
            .unwrap();
        for (name, contents) in files {
            let path = dir.path().join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            let mut file = fs::File::create(path).unwrap();
            file.write_all(contents.as_bytes()).unwrap();
        }
        dir
    }

    #[test]
    fn parses_proto_source_to_descriptor() {
        let file = parse_proto(
            r#"
            syntax = "proto3";
            package demo;
            message Root {
                int32 value = 1;
            }
            "#,
            "inline.proto",
        )
        .unwrap();

        assert_eq!(file.package(), "demo");
        assert_eq!(file.message_type[0].name(), "Root");
        assert_eq!(file.message_type[0].field[0].name(), "value");
    }

    #[test]
    fn parses_real_fixture_proto_file() {
        let path = fixture_schema();
        let file = parse_proto_file(&path).unwrap();

        assert_eq!(file.package(), "test");
        assert!(file.message_type.iter().any(|message| message.name() == "Main"));
        assert!(file
            .message_type
            .iter()
            .find(|message| message.name() == "ArrMap")
            .unwrap()
            .nested_type
            .iter()
            .any(|nested| nested.options.as_ref().and_then(|opts| opts.map_entry).unwrap_or(false)));
    }

    #[test]
    fn reports_parse_failures() {
        let err = parse_proto("syntax = \"proto3\"; message {", "broken.proto").unwrap_err();
        assert!(matches!(err, ProtoError::ProtocFailed { .. }));

        let err = parse_proto_file(workspace_root().join("tests/fixtures/proto/missing.proto"))
            .unwrap_err();
        assert!(matches!(err, ProtoError::ProtocFailed { .. } | ProtoError::Io(_)));
    }

    #[test]
    fn builds_reflection_pool_from_real_fixture_proto() {
        let pool = load_descriptor_pool_from_proto_file(fixture_schema()).unwrap();

        let root = pool.find("test.Main").unwrap();
        assert_eq!(root.fields.get("matrix").unwrap().value_type.as_deref(), Some("test.Vec2D"));
        assert!(pool.find("test.Vec2D").unwrap().is_alias());
        assert!(pool.find("test.ArrMap").unwrap().alias.as_ref().unwrap().is_map());
    }

    #[test]
    fn builds_prost_reflect_pool_from_real_fixture_proto() {
        let pool = load_reflect_descriptor_pool_from_proto_file(fixture_schema()).unwrap();
        let root = pool.get_message_by_name("test.Main").unwrap();
        assert_eq!(root.full_name(), "test.Main");
    }

    #[test]
    fn loads_descriptor_pool_with_imported_proto_dependencies() {
        let dir = write_temp_proto_files(&[
            (
                "common.proto",
                r#"
                syntax = "proto3";
                package shared;
                message Common {
                    string name = 1;
                }
                "#,
            ),
            (
                "root.proto",
                r#"
                syntax = "proto3";
                package demo;
                import "common.proto";
                message Root {
                    shared.Common child = 1;
                }
                "#,
            ),
        ]);

        let pool = load_descriptor_pool_from_proto_file(dir.path().join("root.proto")).unwrap();
        let root = pool.find("demo.Root").unwrap();
        let child = root.fields.get("child").unwrap();
        assert_eq!(child.value_type.as_deref(), Some("shared.Common"));
        assert!(pool.find("shared.Common").is_some());
    }

    #[test]
    fn loads_reflect_pool_with_imported_proto_dependencies() {
        let dir = write_temp_proto_files(&[
            (
                "common.proto",
                r#"
                syntax = "proto3";
                package shared;
                message Common {
                    string name = 1;
                }
                "#,
            ),
            (
                "root.proto",
                r#"
                syntax = "proto3";
                package demo;
                import "common.proto";
                message Root {
                    shared.Common child = 1;
                }
                "#,
            ),
        ]);

        let pool = load_reflect_descriptor_pool_from_proto_file(dir.path().join("root.proto")).unwrap();
        let root = pool.get_message_by_name("demo.Root").unwrap();
        let child = root.get_field_by_name("child").unwrap();
        let child_kind = child.kind();
        let child_message = child_kind.as_message().unwrap();
        assert_eq!(child_message.full_name(), "shared.Common");
    }

    #[test]
    fn reports_schema_registration_failures_after_parse() {
        let result = load_descriptor_pool_from_proto(
            r#"
            syntax = "proto3";
            package demo;
            message Sparse {
                int32 a = 1;
                int32 z = 20;
            }
            "#,
            "sparse.proto",
        );

        assert!(matches!(
            result,
            Err(ProtoError::Register(RegisterError::OverlySparseFieldIds { .. }))
        ));
    }
}

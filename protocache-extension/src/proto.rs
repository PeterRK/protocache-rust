use std::ffi::CString;
#[cfg(test)]
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use crate::reflection::RegisterError;
#[cfg(test)]
use crate::reflection::{DescriptorPool, build_descriptor_pool};
use prost::Message;
use prost_reflect::DescriptorError as ReflectDescriptorError;
#[cfg(test)]
use prost_reflect::DescriptorPool as ReflectDescriptorPool;
use prost_types::FileDescriptorProto;
#[cfg(test)]
use prost_types::FileDescriptorSet;
#[derive(Debug)]
pub enum ProtoError {
    Io(std::io::Error),
    ParseFailed { message: String },
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
            Self::ParseFailed { message } => write!(f, "proto parse failed: {message}"),
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
            Self::ParseFailed { .. } | Self::MissingFile { .. } | Self::Register(_) => None,
        }
    }
}

pub fn parse_proto(source: &str, file_name: &str) -> Result<FileDescriptorProto, ProtoError> {
    let source_cstr = CString::new(source).map_err(|_| ProtoError::ParseFailed {
        message: "proto source contains interior NUL byte".to_owned(),
    })?;
    let file_name_cstr = CString::new(file_name).map_err(|_| ProtoError::ParseFailed {
        message: format!("file name contains interior NUL byte: {file_name}"),
    })?;
    let bytes = unsafe {
        parse_proto_via_ffi(
            |out_bytes, out_len, out_error| {
                protocache_parse_proto(
                    source_cstr.as_ptr(),
                    file_name_cstr.as_ptr(),
                    out_bytes,
                    out_len,
                    out_error,
                )
            },
            "libprotoc parser failed",
        )?
    };
    FileDescriptorProto::decode(bytes.as_slice()).map_err(ProtoError::Decode)
}

pub fn parse_proto_file(path: impl AsRef<Path>) -> Result<FileDescriptorProto, ProtoError> {
    let path_cstr = path_to_cstring(path.as_ref())?;
    let bytes = unsafe {
        parse_proto_via_ffi(
            |out_bytes, out_len, out_error| {
                protocache_parse_proto_file(path_cstr.as_ptr(), out_bytes, out_len, out_error)
            },
            "libprotoc file parser failed",
        )?
    };
    FileDescriptorProto::decode(bytes.as_slice()).map_err(ProtoError::Decode)
}

#[cfg(test)]
pub(crate) fn load_descriptor_pool_from_proto(
    source: &str,
    file_name: &str,
) -> Result<DescriptorPool, ProtoError> {
    let temp_dir = tempfile::Builder::new().prefix("parse-proto-").tempdir()?;
    let proto_path = temp_dir.path().join(file_name);
    if let Some(parent) = proto_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&proto_path, source)?;

    let set = parse_proto_file_set_with_imports(&proto_path, &[temp_dir.path().to_path_buf()])?;
    build_descriptor_pool(&set.file).map_err(ProtoError::Register)
}

#[cfg(test)]
pub(crate) fn load_descriptor_pool_from_proto_file(
    path: impl AsRef<Path>,
) -> Result<DescriptorPool, ProtoError> {
    let set = parse_proto_file_set_with_imports(path.as_ref(), &[])?;
    build_descriptor_pool(&set.file).map_err(ProtoError::Register)
}

#[cfg(test)]
pub(crate) fn load_reflect_descriptor_pool_from_proto_file(
    path: impl AsRef<Path>,
) -> Result<ReflectDescriptorPool, ProtoError> {
    let set = parse_proto_file_set_with_imports(path.as_ref(), &[])?;
    ReflectDescriptorPool::from_file_descriptor_set(set).map_err(ProtoError::Reflect)
}

#[cfg(test)]
pub(crate) fn parse_proto_file_set(
    path: impl AsRef<Path>,
) -> Result<FileDescriptorSet, ProtoError> {
    parse_proto_file_set_with_imports(path.as_ref(), &[])
}

#[cfg(test)]
fn parse_proto_file_set_with_imports(
    path: &Path,
    extra_imports: &[PathBuf],
) -> Result<FileDescriptorSet, ProtoError> {
    let bytes = parse_descriptor_set_bytes(path, extra_imports)?;
    FileDescriptorSet::decode(bytes.as_slice()).map_err(ProtoError::Decode)
}

unsafe fn parse_proto_via_ffi(
    invoke: impl FnOnce(*mut *mut u8, *mut usize, *mut *mut std::os::raw::c_char) -> i32,
    fallback_message: &str,
) -> Result<FfiBytes, ProtoError> {
    let mut out_bytes = std::ptr::null_mut();
    let mut out_len = 0usize;
    let mut out_error = std::ptr::null_mut();
    let status = invoke(&mut out_bytes, &mut out_len, &mut out_error);
    if status != 0 {
        return Err(ProtoError::ParseFailed {
            message: unsafe { ffi_error_message(out_error, fallback_message) },
        });
    }
    Ok(FfiBytes::new(out_bytes, out_len))
}

#[cfg(test)]
fn parse_descriptor_set_bytes(
    path: &Path,
    extra_imports: &[PathBuf],
) -> Result<FfiBytes, ProtoError> {
    let path_cstr = path_to_cstring(path)?;
    let import_cstrs = extra_imports
        .iter()
        .map(|import| path_to_cstring(import))
        .collect::<Result<Vec<_>, _>>()?;
    let import_ptrs = import_cstrs
        .iter()
        .map(|import| import.as_ptr())
        .collect::<Vec<_>>();

    let mut out_bytes = std::ptr::null_mut();
    let mut out_len = 0usize;
    let mut out_error = std::ptr::null_mut();

    let status = unsafe {
        protocache_parse_proto_file_set(
            path_cstr.as_ptr(),
            import_ptrs.as_ptr(),
            import_ptrs.len(),
            &mut out_bytes,
            &mut out_len,
            &mut out_error,
        )
    };

    if status != 0 {
        return Err(ProtoError::ParseFailed {
            message: unsafe { ffi_error_message(out_error, "libprotoc parsing failed") },
        });
    }

    Ok(FfiBytes::new(out_bytes, out_len))
}

fn path_to_cstring(path: &Path) -> Result<CString, ProtoError> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| ProtoError::ParseFailed {
        message: format!("path contains interior NUL byte: {}", path.display()),
    })
}

struct FfiBytes {
    ptr: *mut u8,
    len: usize,
}

impl FfiBytes {
    fn new(ptr: *mut u8, len: usize) -> Self {
        Self { ptr, len }
    }

    fn as_slice(&self) -> &[u8] {
        if self.ptr.is_null() {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
        }
    }
}

impl Drop for FfiBytes {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { protocache_free_proto_bytes(self.ptr) };
        }
    }
}

unsafe fn ffi_error_message(
    out_error: *mut std::os::raw::c_char,
    fallback_message: &str,
) -> String {
    if out_error.is_null() {
        return fallback_message.to_owned();
    }
    let message = unsafe { std::ffi::CStr::from_ptr(out_error) }
        .to_string_lossy()
        .into_owned();
    unsafe { protocache_free_proto_error(out_error) };
    message
}

unsafe extern "C" {
    fn protocache_parse_proto(
        source: *const std::os::raw::c_char,
        file_name: *const std::os::raw::c_char,
        out_bytes: *mut *mut u8,
        out_len: *mut usize,
        out_error: *mut *mut std::os::raw::c_char,
    ) -> i32;

    fn protocache_parse_proto_file(
        path: *const std::os::raw::c_char,
        out_bytes: *mut *mut u8,
        out_len: *mut usize,
        out_error: *mut *mut std::os::raw::c_char,
    ) -> i32;

    #[cfg(test)]
    fn protocache_parse_proto_file_set(
        proto_path: *const std::os::raw::c_char,
        extra_import_paths: *const *const std::os::raw::c_char,
        extra_import_count: usize,
        out_bytes: *mut *mut u8,
        out_len: *mut usize,
        out_error: *mut *mut std::os::raw::c_char,
    ) -> i32;

    fn protocache_free_proto_bytes(bytes: *mut u8);
    fn protocache_free_proto_error(error: *mut std::os::raw::c_char);
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write as _;
    use tempfile::TempDir;

    const TEST_SCHEMA: &str = r#"
        syntax = "proto3";
        package test;

        message Child {
            string name = 1;
        }

        message AliasVec {
            repeated Child _ = 1;
        }

        message AliasMap {
            map<string, Child> _ = 1;
        }

        message Main {
            Child child = 1;
            AliasVec items = 2;
            AliasMap lookup = 3;
        }
    "#;

    fn test_schema_file() -> (TempDir, PathBuf) {
        let dir = tempfile::Builder::new()
            .prefix("pcrs-proto-schema-")
            .tempdir()
            .unwrap();
        let path = dir.path().join("test.proto");
        fs::write(&path, TEST_SCHEMA).unwrap();
        (dir, path)
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
    fn parses_proto_file_from_targeted_schema() {
        let (_dir, path) = test_schema_file();
        let file = parse_proto_file(&path).unwrap();

        assert_eq!(file.package(), "test");
        assert!(
            file.message_type
                .iter()
                .any(|message| message.name() == "Main")
        );
        assert!(
            file.message_type
                .iter()
                .find(|message| message.name() == "AliasMap")
                .unwrap()
                .nested_type
                .iter()
                .any(|nested| nested
                    .options
                    .as_ref()
                    .and_then(|opts| opts.map_entry)
                    .unwrap_or(false))
        );
    }

    #[test]
    fn reports_parse_failures() {
        let err = parse_proto("syntax = \"proto3\"; message {", "broken.proto").unwrap_err();
        assert!(matches!(err, ProtoError::ParseFailed { .. }));

        let (_dir, path) = test_schema_file();
        let err = parse_proto_file(path.parent().unwrap().join("missing.proto")).unwrap_err();
        assert!(matches!(
            err,
            ProtoError::ParseFailed { .. } | ProtoError::Io(_)
        ));
    }

    #[test]
    fn builds_reflection_pool_from_targeted_schema_file() {
        let (_dir, path) = test_schema_file();
        let pool = load_descriptor_pool_from_proto_file(path).unwrap();

        let root = pool.find("test.Main").unwrap();
        assert_eq!(root.fields.get("child").unwrap().value_type, "test.Child");
        assert!(pool.find("test.AliasVec").unwrap().is_alias());
        assert!(pool.find("test.AliasMap").unwrap().alias.is_map());
    }

    #[test]
    fn builds_prost_reflect_pool_from_targeted_schema_file() {
        let (_dir, path) = test_schema_file();
        let pool = load_reflect_descriptor_pool_from_proto_file(path).unwrap();
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
        assert_eq!(child.value_type, "shared.Common");
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

        let pool =
            load_reflect_descriptor_pool_from_proto_file(dir.path().join("root.proto")).unwrap();
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
            Err(ProtoError::Register(
                RegisterError::OverlySparseFieldIds { .. }
            ))
        ));
    }
}

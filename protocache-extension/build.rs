use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/proto_bridge.cc");
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by cargo"));
    let object_path = out_dir.join("proto_bridge.o");
    let library_path = out_dir.join("libprotocache_extension_proto_bridge.a");

    let cxx = env::var("CXX").unwrap_or_else(|_| "c++".to_owned());
    let cxx_status = Command::new(&cxx)
        .args([
            "-std=c++17",
            "-fPIC",
            "-c",
            "src/proto_bridge.cc",
            "-o",
        ])
        .arg(&object_path)
        .status()
        .expect("failed to invoke C++ compiler");
    assert!(cxx_status.success(), "failed to compile src/proto_bridge.cc");

    let ar_status = Command::new("ar")
        .args(["crus"])
        .arg(&library_path)
        .arg(&object_path)
        .status()
        .expect("failed to invoke ar");
    assert!(ar_status.success(), "failed to archive proto bridge library");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=protocache_extension_proto_bridge");
    println!("cargo:rustc-link-lib=dylib=protoc");
    println!("cargo:rustc-link-lib=dylib=protobuf");
    println!("cargo:rustc-link-lib=dylib=stdc++");
}

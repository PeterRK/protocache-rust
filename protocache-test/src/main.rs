#[cfg(protocache_test_has_protobuf_generated)]
include!("benchmark.rs");

#[cfg(not(protocache_test_has_protobuf_generated))]
fn main() {
    println!("protocache-test: skipped because required benchmark resources are unavailable");
}

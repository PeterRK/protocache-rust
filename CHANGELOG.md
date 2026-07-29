# Changelog

All notable user-facing changes to this project will be documented in this
file. The project follows Semantic Versioning.

## [Unreleased]

## [0.1.0] - 2026-07-28

Initial beta release of the Rust ProtoCache implementation.

### Added

- Protobuf-free zero-copy runtime, encoding, mutable APIs, compression, and
  perfect-hash support in `protocache-core`.
- Schema reflection, JSON helpers, Protobuf/prost conversion, and optional
  Unix `libprotoc` parsing in `protocache-extension`.
- Typed Rust binding generation through the `protoc-gen-pcrs` protoc plugin.
- Compatibility tests, generated-code tests, benchmarks, CI checks, and a
  runnable core example.

### Known limitations

- The public Rust API may change during the `0.1.x` beta series.
- Official platform support is limited to 64-bit targets.
- The optional `native-proto` feature is Unix-only and requires a C++17
  compiler plus libprotoc/libprotobuf development libraries.
- Production-level fuzzing and Miri coverage are not yet complete.

[Unreleased]: https://github.com/PeterRK/protocache-rust/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/PeterRK/protocache-rust/releases/tag/v0.1.0

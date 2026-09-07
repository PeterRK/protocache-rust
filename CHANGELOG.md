# Changelog

All notable user-facing changes to this project will be documented in this
file. The project follows Semantic Versioning.

## [Unreleased]

## [1.0.0] - Unreleased

### Stability

- Establish the `1.x` public API compatibility contract for the 64-bit,
  trusted-input deployment model. Rust 1.89 remains the baseline MSRV;
  `native-proto` remains optional and Unix-only.
- Preserve the wire format and existing `0.1.1` generated bindings. No public
  function signatures or trait requirements were changed for this release.

### Fixed

- Share alias validation between reflection, dynamic encoding, and generation.
  Reject `_` fields with another declared field, a number other than 1, or a
  singular label; omit deprecated fields without reusing their field numbers.
- Preserve nonzero one-word nested aliases during dynamic/Protobuf serialization,
  including boolean arrays with one to three elements and empty array/map headers.
- Encode empty 64-bit scalar array aliases with the correct two-word element
  width. Generate boolean alias readers using the byte-array representation and
  expose their `values()` accessor; regenerate boolean alias bindings for this fix.
- Repack folded inline array/map units wider than the selected cell instead of
  panicking in mutable encoding, including the internal map-pair path.
- Clear reused decompression output on every reported decoding error.
- Reject out-of-format dynamic field numbers before allocating field tables.

### Maintenance

- Remove unused enum-value storage and generator dependencies; share container
  detection, perfect-hash lookup, map layout helpers, and generated array logic.
  Existing mutable trait implementations and generated bindings remain supported.
- Generate test bindings only in `OUT_DIR`. Test checked-in bindings by default;
  use `PROTOC_GEN_PCRS` for fresh bindings and the explicit regeneration script
  to update snapshots. An explicitly requested generator failure now fails the build.
- Remove obsolete FlatBuffers source rewriting. Make third-party benchmark
  dependencies opt-in through `flatbuffers-bench` and `fory-bench`; requested
  toolchains and mandatory test fixtures must be available.
- Clarify that dirty-state queries are observational; generated serialization
  decides whether to copy original fields from their access bitset.

### Validation and documentation

- Restore five previously disconnected extension tests and correct the Buffer
  reuse and prost entry-point checks. Add pure Rust nested-container and
  encoding/error-boundary regressions.
- Add larger synthetic array/map benchmark inputs and black-box input/output
  handling. The dynamic in-place optimization candidates were not retained;
  this release makes no claim of a serialization speedup.
- Correct raw `detect`, Buffer initialization, descriptor pairing, and failure
  state documentation. Document the `1.x` source compatibility policy.

### Migration from 0.1.1

- Update the three published package versions to `1.0.0`; existing generated
  source remains usable. Regeneration is optional when the schema is unchanged.
- Treat raw message/array/map detection as shallow, initialize all newly exposed
  Buffer words, and use matching prost descriptors. These clarify existing
  behavior rather than introducing new runtime requirements.
- Dynamic schemas with field numbers beyond 6387 now return an error before
  allocation; such field numbers cannot be represented in ProtoCache.


## [0.1.1] - 2026-08-09

### Fixed

- Bound perfect-hash construction retries so duplicate or otherwise
  unrepresentable key sets return `None` instead of retrying across the full
  seed space.
- Encode an empty `MutableMap` as the canonical inline empty map instead of
  panicking while initializing the entry-order table.

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

[Unreleased]: https://github.com/PeterRK/protocache-rust/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/PeterRK/protocache-rust/compare/v0.1.1...v1.0.0
[0.1.1]: https://github.com/PeterRK/protocache-rust/releases/tag/v0.1.1
[0.1.0]: https://github.com/PeterRK/protocache-rust/releases/tag/v0.1.0

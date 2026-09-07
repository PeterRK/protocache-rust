# ProtoCache Rust

Rust implementation of ProtoCache, including the core runtime, mutable APIs, schema/reflection extensions, and a `protoc` code generator for typed Rust bindings.

> [!WARNING]
>
> This Rust workspace was generated with AI assistance.
> Treat it as generated software and verify behavior with tests and benchmarks before relying on it in production.

## Release Status

This checkout prepares the `1.0.0` release. Publication is tracked in
[RELEASE.md](https://github.com/PeterRK/protocache-rust/blob/main/RELEASE.md).

The `1.x` series commits to source compatibility for the public Rust APIs on
supported targets. Breaking public API changes require a new major version.
The supported deployment model uses trusted ProtoCache data on 64-bit targets;
production-level hostile-input fuzzing and Miri coverage remain outside the
current validation scope. See the compatibility contract below.

## Overview

The workspace is organized into four crates:

| Crate | Purpose |
|:--|:--|
| `protocache-core` | Protobuf-free runtime for zero-copy reads, mutable access, encoding, hashing, compression, and perfect-hash utilities |
| `protocache-extension` | Descriptor handling, reflection, protobuf/prost bridging, schema-aware helpers, and optional native `.proto` parsing |
| `protoc-gen-pcrs` | `protoc` plugin that generates typed Rust APIs |
| `protocache-test` | Compatibility tests and the local benchmark harness |

The portable schema subset is documented in the upstream
[schema reference](https://github.com/PeterRK/ProtoCache/blob/main/schema.md),
and the binary layout is documented in the upstream
[data-format reference](https://github.com/PeterRK/ProtoCache/blob/main/data-format.md).

## Installation

Add the protobuf-free runtime:

```bash
cargo add protocache-core@1.0.0
```

Add schema, reflection, JSON, and Protobuf conversion support when needed:

```bash
cargo add protocache-extension@1.0.0
```

Install the `protoc` plugin:

```bash
cargo install protoc-gen-pcrs --version 1.0.0
```

Equivalent manifest dependencies are:

```toml
[dependencies]
protocache-core = "1.0.0"
# Optional, for reflection and Protobuf-facing workflows:
protocache-extension = "1.0.0"
```

The primary Rust-facing API layers are:

- `protocache_core::runtime` for zero-copy read access
- `protocache_core::mutable` for mutable message, map, and array APIs
- `protocache_core::encoding` for low-level buffer and encoding primitives
- `protocache_extension::{reflection, utils}` for schema-aware and protobuf-facing workflows

## Benchmark

Historical `0.1.1` samples using the local Rust harness in `protocache-test` on
the bundled fixture with `--loops 1000000`. These are not a `1.0.0` performance
claim. Timings are ns/op-equivalent local samples.

|  | Protobuf | ProtoCache | FlatBuffers | Fory |
|:-------|----:|----:|----:|----:|
| Data Size | **574B** | 780B | 1296B | 615B |
| Decode + Traverse + Dealloc | 1825ns | **219ns** | 265ns | 1347ns |
| Decode + Traverse(reflection) + Dealloc | 9628ns | **432ns** | - | - |
| Compressed Size | **566B** | 571B | 856B | 611B |
| Compress | 273ns | 436ns | 817ns | 315ns |
| Decompress | 117ns | 271ns | 600ns | 154ns |

Mutable/serialize paths from the same Rust benchmark:

| | Protobuf | ProtoCacheEX | ProtoCache |
|:-------|----:|----:|----:|
| Serialize | 800ns | 355ns / 2381ns | 8679ns |
| Decode + Traverse + Dealloc | 1825ns | 1615ns | 219ns |

## Build and Test

Run the full workspace test suite:

```bash
cargo test --workspace
```

Build the benchmark harness:

```bash
cargo build --release -p protocache-test
```

FlatBuffers and Fory comparisons are opt-in. Enable `flatbuffers-bench` or
`fory-bench` on `protocache-test` and provide the corresponding `flatc` 25.12.19
or `foryc` tool (`FLATC` / `FORYC` may override their paths). Requested generators
must succeed; missing required fixtures or Protobuf tools fail the build.

`protocache-extension` builds as a pure Rust crate by default. To enable direct
`.proto` source parsing through the native `libprotoc` bridge:

```bash
cargo build -p protocache-extension --features native-proto
```

The `native-proto` feature currently supports Unix targets and requires a C++17
compiler (`CXX` may override `c++`), `ar`, and development installations of
`libprotoc` and `libprotobuf` (on Ubuntu, install `libprotoc-dev` and
`libprotobuf-dev`). The `protocache-test` harness enables this feature because
its compatibility tests parse fixture schemas.

The Protobuf and ProtoCache benchmark binaries (`test.pb` and `test.pc`) are
derived at build time from `test.proto` and `test.json`. The FlatBuffers binary
is likewise derived from `test.fbs` and `test-fb.json`. These generated binaries
are written to Cargo's `OUT_DIR`; they are not source fixtures.

## Quick Start

Run the low-level encode and zero-copy read example:

```bash
cargo run -p protocache-core --example basic
```

## Code Generation

Build the `protoc` plugin and generate typed readonly Rust APIs from the existing test schema:

```bash
cargo build -p protoc-gen-pcrs
mkdir -p generated
protoc \
  --proto_path=tests/fixtures/proto \
  --plugin=protoc-gen-pcrs=target/debug/protoc-gen-pcrs \
  --pcrs_out=generated \
  tests/fixtures/proto/test.proto
```

This writes `generated/test.pc.rs`. To also generate mutable APIs in
`generated/test.pc-ex.rs`, pass the `extra` plugin parameter:

```bash
protoc \
  --proto_path=tests/fixtures/proto \
  --plugin=protoc-gen-pcrs=target/debug/protoc-gen-pcrs \
  --pcrs_out=extra:generated \
  tests/fixtures/proto/test.proto
```

Workspace tests use the checked-in bindings by default to cover compatibility.
To compile and test fresh bindings without modifying those snapshots:

```bash
cargo build -p protoc-gen-pcrs
PROTOC_GEN_PCRS="$PWD/target/debug/protoc-gen-pcrs" cargo test -p protocache-test
```

All generated build artifacts go to Cargo's `OUT_DIR`. To explicitly refresh
the checked-in test bindings, run `python3 scripts/regenerate-test-bindings.py`;
add `--check` to report differences without writing them. New object-array
bindings use the runtime's default trait implementations; older bindings remain
supported.

## Supported Platforms

The core runtime, extension, and code generator officially support 64-bit Rust
targets. 32-bit targets are not tested and are outside the compatibility
guarantee. The optional `native-proto` feature is additionally limited to Unix
targets.

## Dependency Boundary

The core runtime, code generator, and default `protocache-extension` build use
Rust workspace crates and published Rust dependencies. Fixture reuse in tests
and benchmarks is data reuse, not a runtime code dependency.

Native `.proto` parsing is an explicit exception: enabling
`protocache-extension/native-proto` compiles `src/proto_bridge.cc` and dynamically
links `libprotoc`, `libprotobuf`, and the platform C++ runtime. Additional
developer workflows use external tools:

- `protoc` for code generation and descriptor-set workflows
- `flatc` and `foryc` for their optional local benchmark cases

## Notes

For most integrations:

- start read-only access from `protocache_core::runtime::MessageView`
- use `protocache_core::mutable` for mutable document workflows
- add `protocache-extension` only when you need schema loading, reflection, or protobuf/prost conversion

Most users should not need to work with descriptor wiring or dynamic reflection types directly unless they are building schema-driven tooling.

## Security and Compatibility

- The supported MSRV is Rust `1.89`.
- Official support is limited to 64-bit targets.
- `protocache-extension` is pure Rust by default; `native-proto` adds a
  Unix-only C++/libprotoc FFI boundary.
- The repository test/benchmark crate is not published. Its pinned
  FlatBuffers and Fory dependencies are used only with checked-in,
  locally-generated benchmark fixtures and are not dependencies of the three
  public crates.
- Report suspected security issues privately to the maintainer using the
  contact address in the crate metadata.

## 1.x Compatibility Contract

- Public runtime, encoding, mutable, and extension APIs retain source
  compatibility within `1.x`, including traits used by generated bindings.
  Low-level callers must follow the documented Unit/Buffer and schema contracts.
- Existing supported ProtoCache data remains readable. Perfect-hash seeds and
  physical map ordering are not deterministic, so serialized byte identity is
  not promised. Compare decoded values rather than map byte sequences.
- Existing `0.1.1` generated bindings are supported by runtime `1.0.0`. Use the
  generator and runtime from the same release when regenerating bindings;
  generated source formatting and implementation details are not API contracts.
- Readers validate headers and bounds as data is accessed. A successful view
  constructor does not recursively validate the full graph or schema. `None`
  may mean missing or malformed data; `expect_*` accessors panic on failure.
  Raw message/array/map `detect` methods cover headers and cells; use generated
  typed detection for recursively referenced payloads.
- Mutable access conservatively marks containers dirty. Serialization does not
  reset that state. Dirty queries report container state; generated serialization
  reuses source words only for fields that have not been accessed through mutable
  accessors. Serialization failures are not transactional; discard intermediate
  units and clear or rebuild the buffer before retrying.
- `Buffer::expand` can return reused data and does not clear padding. A prost
  message must be paired with the correct descriptor by its caller; a
  wire-compatible mismatch cannot reliably be detected.
- Rust 1.89 is the baseline MSRV. An MSRV increase will be announced and made in
  a minor release, not a patch. Changes to publicly exposed dependency types
  that break source compatibility require a major release.

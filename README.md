# ProtoCache Rust

Rust implementation of ProtoCache, including the core runtime, mutable APIs, schema/reflection extensions, and a `protoc` code generator for typed Rust bindings.

> Warning
>
> This Rust workspace was generated with AI assistance.
> Treat it as generated software and verify behavior with tests and benchmarks before relying on it in production.

## Overview

The workspace is organized into four crates:

| Crate | Purpose |
|:--|:--|
| `protocache-core` | Protobuf-free runtime for zero-copy reads, mutable access, encoding, hashing, compression, and perfect-hash utilities |
| `protocache-extension` | Descriptor handling, reflection, protobuf/prost bridging, schema-aware helpers, and optional native `.proto` parsing |
| `protoc-gen-pcrs` | `protoc` plugin that generates typed Rust APIs |
| `protocache-test` | Compatibility tests and the local benchmark harness |

The primary Rust-facing API layers are:

- `protocache_core::runtime` for zero-copy read access
- `protocache_core::mutable` for mutable message, map, and array APIs
- `protocache_core::encoding` for low-level buffer and encoding primitives
- `protocache_extension::{reflection, utils}` for schema-aware and protobuf-facing workflows

## Benchmark

Using the local Rust harness in `protocache-test` on the bundled benchmark fixture
with `--loops 1000000`. Timings are ns/op-equivalent local samples.

|  | Protobuf | ProtoCache | FlatBuffers | Fory |
|:-------|----:|----:|----:|----:|
| Data Size | **574B** | 780B | 1296B | 615B |
| Decode + Traverse + Dealloc | 1825ns | **219ns** | 265ns | 1347ns |
| Decode + Traverse(reflection) + Dealloc | 9628ns | **432ns** | - | - |
| Compressed Size | **566B** | 571B | 856B | 611B |
| Compress | **273ns** | 436ns | 817ns | 315ns |
| Decompress | **117ns** | 271ns | 600ns | 154ns |

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

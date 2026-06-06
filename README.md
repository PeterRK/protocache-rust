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
| `protocache-extension` | Schema loading, descriptor handling, reflection, protobuf/prost bridging, and schema-aware helpers |
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

## Code Generation

Generate typed Rust APIs with the plugin:

```bash
cargo run -p protoc-gen-pcrs -- < input.bin > output.bin
```

## Dependency Boundary

The Rust implementation does not depend on any non-Rust source tree at build or runtime.

- Rust crates depend only on workspace crates and published Rust dependencies.
- Fixture reuse in tests and benchmarks is data reuse, not a runtime code dependency.

Some developer workflows still rely on external tools:

- `protoc` for `.proto` parsing and code generation flows
- `flatc` for the local benchmark harness

## Notes

For most integrations:

- start read-only access from `protocache_core::runtime::MessageView`
- use `protocache_core::mutable` for mutable document workflows
- add `protocache-extension` only when you need schema loading, reflection, or protobuf/prost conversion

Most users should not need to work with descriptor wiring or dynamic reflection types directly unless they are building schema-driven tooling.

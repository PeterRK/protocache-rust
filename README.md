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

Using the local Rust harness in `protocache-test` on the bundled benchmark fixture:

|  | Protobuf | ProtoCache | FlatBuffers |
|:-------|----:|----:|----:|
| Data Size | **574B** | 780B | 1296B |
| Decode + Traverse + Dealloc | 2269ns | **260ns** | 321ns |
| Decode + Traverse(reflection) + Dealloc | 12095ns | **550ns** | - |
| Compressed Size | 566B | 571B | 856B |
| Compress | 334ns | 548ns | 1000ns |
| Decompress | 143ns | 321ns | 714ns |

Mutable/serialize paths from the same Rust benchmark:

| | Protobuf | ProtoCacheEX | ProtoCache |
|:-------|----:|----:|----:|
| Serialize | **1003ns** | 418 ~ 2531ns | 10875ns |
| Decode + Traverse + Dealloc | 2269ns | 2021ns | **260ns** |

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

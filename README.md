# ProtoCache Rust

Rust implementation of ProtoCache.

> Warning
>
> This repository's Rust code is fully AI-generated.
> The implementation, tests, generated APIs, and supporting glue in this workspace were produced by AI.
> Verify behavior with tests before relying on it in production.

## Benchmark

Using the local Rust harness in `protocache-test` on the bundled benchmark fixture:

|  | Protobuf | ProtoCache | FlatBuffers |
|:-------|----:|----:|----:|
| Data Size | **574B** | 780B | 1296B |
| Decode + Traverse + Dealloc | 3071ns | **304ns** | 374ns |
| Decode + Traverse(reflection) + Dealloc | 15129ns | **610ns** | - |
| Compressed Size | 566B | 571B | 856B |
| Compress | 531ns | 1054ns | 2022ns |
| Decompress | 363ns | 967ns | 1898ns |

Mutable/serialize paths from the same Rust benchmark:

| | Protobuf | ProtoCacheEX | ProtoCache |
|:-------|----:|----:|----:|
| Serialize | **1134ns** | 859 ~ 6478ns | 16216ns |
| Decode + Traverse + Dealloc | 3071ns | 2680ns | **304ns** |

Run it with:

```bash
cargo run -p protocache-test --release
```

## Provenance

This repository's Rust code is fully AI-generated.

- The implementation, tests, and supporting glue in this workspace were produced by AI.
- Treat the codebase as generated software: verify behavior with tests before relying on it in production.

The Rust workspace uses the ProtoCache data format and keeps a clear API split between runtime, extension, and code generation layers:

- zero-copy read-only runtime
- extension APIs for schema reflection, `.proto` loading and protobuf/prost bridging
- `protoc` code generator

## Workspace

`Cargo.toml` contains these crates:

| Crate | Purpose |
|:--|:--|
| `protocache-core` | Protobuf-free core runtime for reading, writing, hashing, compression and mutable primitives |
| `protocache-extension` | Extension APIs for `.proto` parsing, descriptor loading, reflection, protobuf/prost bridging and schema-aware mutable APIs |
| `protoc-gen-pcrs` | Rust code generator for typed APIs |
| `protocache-test` | Local performance harness and test umbrella |

## Dependency Boundary

The Rust implementation does not depend on any non-Rust source tree at build or runtime.

- Rust crates only depend on other Rust crates in this workspace and published Rust dependencies.
- Tests and local performance tooling may reuse fixture data, but that is data reuse, not code dependency.

Some workflows still rely on external tools:

- `protoc` is used for `.proto` parsing and code generation related flows.
- `flatc` is used only by the local performance harness.

## Usage

Run workspace tests:

```bash
cargo test --workspace
```

Generate Rust typed APIs with the plugin:

```bash
cargo run -p protoc-gen-pcrs -- < input.bin > output.bin
```


### API Layers

The Rust API surface is grouped into a small set of layers.

Core runtime APIs:

- `protocache_core::runtime`: zero-copy read-only access to protocache data
- `protocache_core::mutable`: mutable APIs such as `MutableMessage`, `MutableMap`, and `MutableArray`
- `protocache_core::encoding`: low-level encoding primitives and `Buffer`

Support APIs:

- `protocache_extension::utils`: `.proto` loading, JSON helpers, and protobuf/prost -> protocache conversion
- `protocache_extension::reflection`: schema reflection support

Error model:

- protobuf/prost bridge operations use `protocache_core::MutableError`
- JSON helpers use `protocache_extension::utils::JsonError`

Primary entry points:

- `protocache_core::access`, `protocache_core::serialize`, `protocache_core::perfect_hash`
- `protocache_core::mutable`
- `protocache_extension::{reflection, utils}`

### Recommended Usage

Prefer these APIs for long-term integration:

- for zero-copy reads, start from `protocache_core::runtime::MessageView`
- for mutable writes, prefer `protocache_core::mutable::MutableMessage`
- for extension-side conversion flows, prefer `protocache_extension::utils::serialize`

## Notes

The Rust side follows the same two-layer split across this workspace:

- depend on `protocache-core` for the protobuf-free runtime and mutable primitives
- add `protocache-extension` when you need `extension/*` capabilities such as `.proto` parsing, descriptor handling, reflection, or protobuf/prost bridging

The main runtime surface should be read as:

- `protocache_core::{runtime, mutable, encoding}` are the primary Rust-first public APIs
- `protocache_core::{access, mutable, serialize, perfect_hash}` are the primary top-level modules
- `protocache_extension::{reflection, utils}` are the primary extension-side public APIs

Most users should not need to work with `DynamicMessage`, `ReflectMessage`, or descriptor wiring directly.

The current state is:

- functionality is covered by workspace tests and compatibility checks
- some serialization and reflection-heavy paths still need optimization work

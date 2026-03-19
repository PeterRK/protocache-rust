# ProtoCache Rust

Rust implementation of ProtoCache.

The Rust workspace keeps the same data format and overall capability split as the C++ version, while organizing the code as several crates. The main path is:

- zero-copy read-only runtime
- schema reflection and validation
- mutable runtime
- `protoc` code generator

## Workspace

`rust/Cargo.toml` contains these crates:

| Crate | Purpose |
|:--|:--|
| `protocache-core` | Core runtime for reading, writing, hashing and compression |
| `protocache-schema` | `.proto` parsing, descriptor loading, schema reflection and validation |
| `protocache-mutable` | Mutable / EX-style runtime |
| `protoc-gen-pcrs` | Rust code generator for typed APIs |
| `protocache-benchmark` | Rust benchmark entry for local comparison |

## Independence from C++

The Rust implementation does not depend on the C++ code at build or runtime.

- Rust crates only depend on other Rust crates in this workspace and published Rust dependencies.
- Benchmark and tests may reuse shared fixtures under `tests/fixtures`, but that is data reuse, not code dependency.

Some workflows still rely on external tools:

- `protoc` is used for `.proto` parsing and code generation related flows.
- `flatc` is used only by the benchmark crate.

## Usage

Run workspace tests:

```bash
cd rust
cargo test --workspace
```

Run the benchmark:

```bash
cd rust
cargo run -p protocache-benchmark --release -- --loops 1000
```

Generate Rust typed APIs with the plugin:

```bash
cd rust
cargo run -p protoc-gen-pcrs -- < input.bin > output.bin
```

### Recommended APIs

For most callers, prefer the lowest-requirement entry points from `protocache-mutable`:

- if you already have protobuf bytes, use `serialize_protobuf_bytes_from_proto_file`
- if you already have a `prost::Message`, use `serialize_prost_message_from_proto_file`
- if you already have protocache words and want mutable access, use `MessageMut::from_proto_file`

Convert protobuf bytes to protocache:

```rust
use protocache_mutable::serialize_protobuf_bytes_from_proto_file;

let words = serialize_protobuf_bytes_from_proto_file(
    "tests/fixtures/proto/benchmark-test.proto",
    "test.Main",
    &protobuf_bytes,
)?;
```

Convert a `prost` message to protocache:

```rust
use prost::Message;
use protocache_mutable::serialize_prost_message_from_proto_file;

let protobuf = my_pb::Main::decode(&*protobuf_bytes)?;
let words = serialize_prost_message_from_proto_file(
    &protobuf,
    "tests/fixtures/proto/benchmark-test.proto",
    "test.Main",
)?;
```

Open existing protocache data for mutable access and reserialize:

```rust
use protocache_core::Buffer;
use protocache_mutable::MessageMut;

let mut root = MessageMut::from_proto_file(
    "tests/fixtures/proto/benchmark-test.proto",
    "test.Main",
    &words,
)?;
root.set_i32("i32", 42)?;

let mut buffer = Buffer::new();
let encoded = root.serialize_into_buffer(&mut buffer)?;
```

## Notes

Compared with the C++ tree, the Rust side is split into smaller crates instead of a single library plus extension modules. This keeps dependency boundaries clearer:

- depend on `protocache-core` for read-only runtime
- add `protocache-schema` for `.proto` parsing and schema handling
- add `protocache-mutable` for mutable access

The mutable crate still contains internal reflection-based plumbing, but the recommended public surface is now schema-path based. Most users should not need to work with `DynamicMessage`, `ReflectMessage`, or descriptor wiring directly.

The remaining work is mainly around API polish, benchmark optimization and broader regression coverage, not a missing core implementation.

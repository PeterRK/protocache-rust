use std::env;
#[cfg(protocache_test_has_fory_generated)]
use std::fs;
#[cfg(protocache_test_has_flatbuffers_generated)]
use std::borrow::Borrow;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::Instant;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use prost::Message;
use prost_reflect::{
    DescriptorError as ReflectDescriptorError,
    DescriptorPool as ReflectDescriptorPool, DynamicMessage, FieldDescriptor as ReflectFieldDescriptor,
    Kind as ReflectKind, MapKey as ReflectMapKey, Value as ReflectValue, prost_types::FileDescriptorSet,
};
use protocache_core::{
    ArrayView, FieldView, MapView, MessageView, MutableError, ReadError,
    StringView, ViewArray, compress_into, decompress_into,
};
use protocache_extension::{
    reflection::{Descriptor as PcDescriptor, DescriptorPool as PcDescriptorPool, Field as PcField,
    FieldType as PcFieldType},
    utils::{ProtoError, parse_proto_file, serialize_dynamic_into_buffer},
};

#[allow(dead_code, unused_imports, warnings)]
mod pb {
    include!(concat!(env!("OUT_DIR"), "/test.rs"));
}

#[allow(
    dead_code,
    unused_imports,
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    warnings
)]
#[cfg(protocache_test_has_flatbuffers_generated)]
mod fb_generated {
    include!(concat!(env!("OUT_DIR"), "/test_generated.rs"));
}

#[allow(
    dead_code,
    unused_imports,
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    warnings
)]
#[cfg(protocache_test_has_fory_generated)]
mod fory_generated {
    include!(concat!(env!("OUT_DIR"), "/test_fory.rs"));
}

#[allow(
    dead_code,
    unused_imports,
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    warnings
)]
mod pcrs_generated {
    include!(concat!(env!("OUT_DIR"), "/test.pc.rs"));
    include!(concat!(env!("OUT_DIR"), "/test.pc-ex.rs"));
}

const DEFAULT_LOOPS: usize = 1_000_000;

type BenchResult<T> = Result<T, BenchError>;

#[derive(Debug)]
enum BenchError {
    Message(&'static str),
    Owned(String),
    Io(std::io::Error),
    Decode(prost::DecodeError),
    Proto(ProtoError),
    ReflectDescriptor(ReflectDescriptorError),
    #[cfg(protocache_test_has_fory_generated)]
    Fory(fory::Error),
    Mutable(MutableError),
    Read(ReadError),
}

impl From<&'static str> for BenchError {
    fn from(value: &'static str) -> Self {
        Self::Message(value)
    }
}

impl From<String> for BenchError {
    fn from(value: String) -> Self {
        Self::Owned(value)
    }
}

impl From<std::io::Error> for BenchError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<prost::DecodeError> for BenchError {
    fn from(value: prost::DecodeError) -> Self {
        Self::Decode(value)
    }
}

impl From<ProtoError> for BenchError {
    fn from(value: ProtoError) -> Self {
        Self::Proto(value)
    }
}

impl From<ReflectDescriptorError> for BenchError {
    fn from(value: ReflectDescriptorError) -> Self {
        Self::ReflectDescriptor(value)
    }
}

#[cfg(protocache_test_has_fory_generated)]
impl From<fory::Error> for BenchError {
    fn from(value: fory::Error) -> Self {
        Self::Fory(value)
    }
}

impl From<MutableError> for BenchError {
    fn from(value: MutableError) -> Self {
        Self::Mutable(value)
    }
}

impl From<ReadError> for BenchError {
    fn from(value: ReadError) -> Self {
        Self::Read(value)
    }
}

impl std::fmt::Display for BenchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Message(value) => f.write_str(value),
            Self::Owned(value) => f.write_str(value),
            Self::Io(err) => err.fmt(f),
            Self::Decode(err) => err.fmt(f),
            Self::Proto(err) => err.fmt(f),
            Self::ReflectDescriptor(err) => err.fmt(f),
            #[cfg(protocache_test_has_fory_generated)]
            Self::Fory(err) => err.fmt(f),
            Self::Mutable(err) => err.fmt(f),
            Self::Read(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for BenchError {}

#[derive(Default)]
struct Junk {
    u32_sum: u32,
    f32_sum: f32,
    u64_sum: u64,
    f64_sum: f64,
}

impl Junk {
    fn add_f32(&mut self, value: f32) {
        self.f32_sum += value;
    }

    fn add_f64(&mut self, value: f64) {
        self.f64_sum += value;
    }

    fn fuse(&self) -> u64 {
        (self.f64_sum + self.f32_sum as f64).to_bits() ^ self.u64_sum.wrapping_add(self.u32_sum as u64)
    }
}

struct BenchConfig {
    loops: usize,
    only: Option<String>,
}

#[derive(Clone)]
struct ReflectDescriptorPlan {
    fields: Vec<ReflectFieldPlan>,
}

#[derive(Clone)]
struct PbReflectDescriptorPlan {
    fields: Vec<PbReflectFieldPlan>,
}

#[derive(Clone)]
struct ReflectFieldPlan {
    id: usize,
    repeated: bool,
    key: Option<PcFieldType>,
    value: ReflectValuePlan,
}

#[derive(Clone)]
enum ReflectValuePlan {
    Message {
        alias: Option<Box<ReflectFieldPlan>>,
        descriptor: Option<Box<ReflectDescriptorPlan>>,
    },
    Bytes,
    String,
    Double,
    Float,
    Uint64,
    Uint32,
    Int64,
    Int32,
    Bool,
    Enum,
}

#[derive(Clone)]
struct PbReflectFieldPlan {
    descriptor: ReflectFieldDescriptor,
    value: PbReflectValuePlan,
}

#[derive(Clone)]
enum PbReflectValuePlan {
    Message(Box<PbReflectDescriptorPlan>),
    Bytes,
    String,
    Double,
    Float,
    Uint64,
    Uint32,
    Int64,
    Int32,
    Bool,
    Enum,
}

impl BenchConfig {
    fn from_args() -> Result<Self, String> {
        let mut loops = DEFAULT_LOOPS;
        let mut only = None;
        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--loops" => {
                    let value = args.next().ok_or("--loops requires a value")?;
                    loops = value
                        .parse()
                        .map_err(|_| format!("invalid --loops value: {value}"))?;
                }
                "--only" => {
                    let value = args.next().ok_or("--only requires a value")?;
                    only = Some(value);
                }
                "--help" | "-h" => {
                    println!("Usage: cargo run -p protocache-test --release -- [--loops N] [--only NAME]");
                    std::process::exit(0);
                }
                _ => return Err(format!("unknown argument: {arg}")),
            }
        }
        Ok(Self { loops, only })
    }
}

impl BenchConfig {
    fn should_run(&self, name: &str) -> bool {
        match &self.only {
            None => true,
            Some(only) => only == name,
        }
    }
}

fn main() -> BenchResult<()> {
    let config = BenchConfig::from_args().map_err(|err| format!("argument error: {err}"))?;
    let schema_path = repo_root().join("tests/fixtures/proto/test.proto");

    let protobuf_raw = protobuf_fixture_bytes();
    #[cfg(protocache_test_has_flatbuffers_generated)]
    let flatbuffers_raw = flatbuffers_fixture_bytes();
    #[cfg(protocache_test_has_fory_generated)]
    let fory_raw = fs::read(repo_root().join("tests/fixtures/benchmark/test.fr"))?;
    let protocache_raw = protocache_fixture_bytes();
    let protocache_words = bytes_to_words(protocache_raw);
    let protocache_dynamic = load_benchmark_dynamic_message(protobuf_raw, &schema_path)?;

    if config.should_run("protobuf") {
        benchmark_protobuf(protobuf_raw, config.loops)?;
    }
    #[cfg(protocache_test_has_flatbuffers_generated)]
    if config.should_run("flatbuffers") {
        benchmark_flatbuffers(flatbuffers_raw, config.loops)?;
    }
    #[cfg(protocache_test_has_fory_generated)]
    if config.should_run("fory") {
        benchmark_fory(&fory_raw, config.loops)?;
    }
    if config.should_run("protocache") {
        benchmark_protocache(&protocache_words, protocache_raw, config.loops)?;
    }
    if config.should_run("protobuf-reflect") {
        benchmark_protobuf_reflect(&schema_path, protobuf_raw, config.loops)?;
    }
    if config.should_run("protocache-reflect") {
        benchmark_protocache_reflect(&schema_path, &protocache_words, config.loops)?;
    }
    if config.should_run("protocache-ex") {
        benchmark_protocache_ex(&protocache_words, protocache_raw, config.loops)?;
    }

    if config.only.is_none()
        || config.should_run("protobuf-serialize")
        || config.should_run("protocache-serialize")
        || config.should_run("protocache-fully")
        || config.should_run("protocache-partly")
    {
        println!("========serialize========");
    }
    if config.should_run("protobuf-serialize") {
        benchmark_protobuf_serialize(protobuf_raw, config.loops)?;
    }
    if config.should_run("protocache-serialize") {
        benchmark_dynamic_to_protocache_serialize(
            "protocache-serialize",
            &protocache_dynamic,
            config.loops,
        )?;
    }
    for (name, maps) in [
        ("protocache-serialize-arrays", false),
        ("protocache-serialize-maps", true),
    ] {
        if config.should_run(name) {
            let root = container_workload(&protocache_dynamic, maps);
            benchmark_dynamic_to_protocache_serialize(name, &root, config.loops)?;
        }
    }
    if config.should_run("protocache-fully") {
        benchmark_protocache_generated_serialize(&protocache_words, false, config.loops)?;
    }
    if config.should_run("protocache-partly") {
        benchmark_protocache_generated_serialize(&protocache_words, true, config.loops)?;
    }

    if config.only.is_none()
        || config.should_run("pb-compress")
        || config.should_run("pc-compress")
        || config.should_run("fb-compress")
        || config.should_run("fr-compress")
    {
        println!("========compress========");
    }
    if config.should_run("pb-compress") {
        benchmark_compress("pb", protobuf_raw, config.loops)?;
    }
    if config.should_run("pc-compress") {
        benchmark_compress("pc", protocache_raw, config.loops)?;
    }
    #[cfg(protocache_test_has_flatbuffers_generated)]
    if config.should_run("fb-compress") {
        benchmark_compress("fb", flatbuffers_raw, config.loops)?;
    }
    #[cfg(protocache_test_has_fory_generated)]
    if config.should_run("fr-compress") {
        benchmark_compress("fr", &fory_raw, config.loops)?;
    }
    Ok(())
}

fn protobuf_fixture_bytes() -> &'static [u8] {
    include_bytes!(concat!(std::env!("OUT_DIR"), "/test.pb"))
}

fn protocache_fixture_bytes() -> &'static [u8] {
    include_bytes!(concat!(std::env!("OUT_DIR"), "/test.pc"))
}

#[cfg(protocache_test_has_flatbuffers_generated)]
fn flatbuffers_fixture_bytes() -> &'static [u8] {
    include_bytes!(concat!(std::env!("OUT_DIR"), "/test-fb.bin"))
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn bytes_to_words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap()))
        .collect()
}

fn load_descriptor_pool_from_proto_file(
    path: &Path,
) -> BenchResult<PcDescriptorPool> {
    let file = parse_proto_file(path)?;
    let mut pool = PcDescriptorPool::default();
    pool.register(&file)
        .map_err(|err| format!("{err:?}"))?;
    Ok(pool)
}

fn load_reflect_descriptor_pool_from_proto_file(
    path: &Path,
) -> BenchResult<ReflectDescriptorPool> {
    let file = parse_proto_file(path)?;
    Ok(ReflectDescriptorPool::from_file_descriptor_set(FileDescriptorSet {
        file: vec![file],
    })?)
}

fn load_benchmark_dynamic_message(
    raw: &[u8],
    schema_path: &Path,
) -> BenchResult<DynamicMessage> {
    let pool = load_reflect_descriptor_pool_from_proto_file(schema_path)?;
    let descriptor = pool
        .get_message_by_name("test.Main")
        .ok_or("missing test.Main descriptor")?;
    let prost_root = pb::Main::decode(raw)?;
    let mut root = DynamicMessage::new(descriptor);
    root.transcode_from(&prost_root)?;
    Ok(root)
}

fn junk_hash_bytes(data: &[u8]) -> u32 {
    let mut out = 0u32;
    let mut chunks = data.chunks_exact(4);
    for chunk in &mut chunks {
        out ^= u32::from_le_bytes(chunk.try_into().unwrap());
    }
    out
}

#[cfg(protocache_test_has_flatbuffers_generated)]
fn junk_hash_i8_items<I, T>(data: I) -> u32
where
    I: IntoIterator<Item = T>,
    T: Borrow<i8>,
{
    let mut out = 0u32;
    let mut word = 0u32;
    let mut shift = 0usize;

    for value in data {
        word |= u32::from(*value.borrow() as u8) << shift;
        shift += 8;
        if shift == 32 {
            out ^= word;
            word = 0;
            shift = 0;
        }
    }

    out
}

fn benchmark_protobuf(raw: &[u8], loops: usize) -> BenchResult<()> {
    let mut junk = Junk::default();
    let start = Instant::now();
    for _ in 0..loops {
        let root = pb::Main::decode(raw)?;
        traverse_pb_main(&root, &mut junk);
    }
    print_access_result("protobuf", raw.len(), start.elapsed(), loops, junk.fuse());
    Ok(())
}

#[cfg(protocache_test_has_flatbuffers_generated)]
fn benchmark_flatbuffers(raw: &[u8], loops: usize) -> BenchResult<()> {
    let mut junk = Junk::default();
    let start = Instant::now();
    for _ in 0..loops {
        let root = unsafe { fb_generated::test::root_as_main_unchecked(raw) };
        traverse_fb_main(root, &mut junk);
    }
    print_access_result("flatbuffers", raw.len(), start.elapsed(), loops, junk.fuse());
    Ok(())
}

#[cfg(protocache_test_has_fory_generated)]
fn new_fory() -> BenchResult<fory::Fory> {
    let mut fory = fory::Fory::builder()
        .xlang(true)
        .track_ref(true)
        .compatible(true)
        .build();
    fory_generated::register_types(&mut fory)?;
    Ok(fory)
}

#[cfg(protocache_test_has_fory_generated)]
fn benchmark_fory(raw: &[u8], loops: usize) -> BenchResult<()> {
    let fory = new_fory()?;
    let mut junk = Junk::default();
    let start = Instant::now();
    for _ in 0..loops {
        let root: fory_generated::Main = fory.deserialize(raw)?;
        traverse_fory_main(&root, &mut junk);
    }
    print_access_result("fory", raw.len(), start.elapsed(), loops, junk.fuse());
    Ok(())
}

fn benchmark_protocache(
    words: &[u32],
    raw: &[u8],
    loops: usize,
) -> BenchResult<()> {
    let mut junk = Junk::default();
    let start = Instant::now();
    for _ in 0..loops {
        let root = MessageView::new(words).ok_or("invalid protocache fixture")?;
        traverse_pc_main(root, &mut junk)?;
    }
    print_access_result("protocache", raw.len(), start.elapsed(), loops, junk.fuse());
    Ok(())
}

fn benchmark_protocache_reflect(
    schema_path: &Path,
    words: &[u32],
    loops: usize,
) -> BenchResult<()> {
    let pool = load_descriptor_pool_from_proto_file(schema_path)?;
    let descriptor_name = "test.Main";
    let descriptor = pool.find(descriptor_name).ok_or("missing test.Main descriptor")?;
    let plan = build_reflect_descriptor_plan(&pool, descriptor_name, descriptor)?;

    let mut junk = Junk::default();
    let start = Instant::now();
    for _ in 0..loops {
        let root = MessageView::new(words).ok_or("invalid protocache fixture")?;
        traverse_pc_reflect_descriptor(&plan, root, &mut junk);
    }
    print_reflect_result("protocache-reflect", start.elapsed(), junk.fuse());
    Ok(())
}

fn benchmark_protocache_ex(
    words: &[u32],
    raw: &[u8],
    loops: usize,
) -> BenchResult<()> {
    let mut junk = Junk::default();
    let start = Instant::now();
    for _ in 0..loops {
        let mut root =
            pcrs_generated::MainMutable::FromWords(words).ok_or("invalid protocache fixture")?;
        traverse_pc_mutable_main(&mut root, &mut junk);
    }
    print_access_result("protocache-ex", raw.len(), start.elapsed(), loops, junk.fuse());
    Ok(())
}

fn benchmark_protobuf_reflect(
    schema_path: &Path,
    raw: &[u8],
    loops: usize,
) -> BenchResult<()> {
    let pool = load_reflect_descriptor_pool_from_proto_file(schema_path)?;
    let descriptor = pool
        .get_message_by_name("test.Main")
        .ok_or("missing test.Main reflect descriptor")?;
    let plan = build_pb_reflect_descriptor_plan(&descriptor);

    let mut junk = Junk::default();
    let start = Instant::now();
    for _ in 0..loops {
        let root = DynamicMessage::decode(descriptor.clone(), raw)?;
        traverse_pb_reflect_descriptor(&plan, &root, &mut junk)?;
    }
    print_reflect_result("protobuf-reflect", start.elapsed(), junk.fuse());
    Ok(())
}

fn benchmark_protobuf_serialize(raw: &[u8], loops: usize) -> BenchResult<()> {
    let root = pb::Main::decode(raw)?;
    let mut total_size = 0usize;
    let start = Instant::now();
    for _ in 0..loops {
        let encoded = black_box(&root).encode_to_vec();
        total_size += black_box(encoded.as_slice()).len();
    }
    print_throughput_result("protobuf-serialize", start.elapsed(), loops, total_size);
    Ok(())
}

// Synthetic batch-shaped inputs reuse the fixture schema. Construction is outside timing.
fn container_workload(root: &DynamicMessage, maps: bool) -> DynamicMessage {
    let mut root = root.clone();
    if maps {
        for name in ["index", "objects"] {
            let ReflectValue::Map(entries) = root.get_field_by_name(name).unwrap().into_owned()
            else {
                unreachable!("fixture field must be a map");
            };
            let mut samples = entries.into_iter().collect::<Vec<_>>();
            samples.sort_by(|(left, _), (right, _)| left.cmp(right));
            let entries = (0..32)
                .map(|i| {
                    let key = if name == "index" {
                        ReflectMapKey::String(format!(
                            "config/service/{i:04}/{}",
                            "x".repeat(i as usize % 32)
                        ))
                    } else {
                        ReflectMapKey::I32(i)
                    };
                    (key, samples[i as usize % samples.len()].1.clone())
                })
                .collect();
            root.set_field_by_name(name, ReflectValue::Map(entries));
        }
    } else {
        for name in ["i32v", "u64v", "strv", "datav", "objectv"] {
            let ReflectValue::List(items) = root.get_field_by_name(name).unwrap().into_owned()
            else {
                unreachable!("fixture field must be a list");
            };
            let items = (0..32)
                .map(|i| {
                    if name == "strv" {
                        ReflectValue::String("x".repeat((i * 17) % 129))
                    } else if name == "datav" {
                        ReflectValue::Bytes(vec![i as u8; (i * 31) % 257].into())
                    } else {
                        items[i % items.len()].clone()
                    }
                })
                .collect();
            root.set_field_by_name(name, ReflectValue::List(items));
        }
    }
    root
}

fn benchmark_dynamic_to_protocache_serialize(
    name: &str,
    root: &DynamicMessage,
    loops: usize,
) -> BenchResult<()> {
    let mut total_size = 0usize;
    let mut buffer = protocache_core::Buffer::new();
    let start = Instant::now();
    for _ in 0..loops {
        let encoded = serialize_dynamic_into_buffer(black_box(root), &mut buffer)?;
        total_size += black_box(encoded).len();
    }
    print_throughput_result(name, start.elapsed(), loops, total_size);
    Ok(())
}

fn benchmark_protocache_generated_serialize(
    words: &[u32],
    partly: bool,
    loops: usize,
) -> BenchResult<()> {
    let mut root =
        pcrs_generated::MainMutable::FromWords(words).ok_or("invalid protocache fixture")?;

    if partly {
        let _ = root.i32();
        let _ = root.u32();
        let _ = root.i64();
        let _ = root.u64();
        let _ = root.flag();
        let _ = root.mode();
        let _ = root.str();
        let _ = root.data();
        let _ = root.f32();
        let _ = root.f64();
    } else {
        let mut junk = Junk::default();
        traverse_pc_mutable_main(&mut root, &mut junk);
    }

    let mut total_size = 0usize;
    let mut buffer = protocache_core::Buffer::new();
    let start = Instant::now();
    for _ in 0..loops {
        let encoded = black_box(&root).SerializeIntoBuffer(&mut buffer)?;
        total_size += black_box(encoded).len();
    }
    print_throughput_result(
        if partly {
            "protocache-partly"
        } else {
            "protocache-fully"
        },
        start.elapsed(),
        loops,
        total_size,
    );
    Ok(())
}

fn traverse_pc_mutable_small(root: &mut pcrs_generated::SmallMutable, junk: &mut Junk) {
    junk.u32_sum = junk
        .u32_sum
        .wrapping_add(*root.i32() as u32)
        .wrapping_add(u32::from(*root.flag()));
    junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(root.str().as_bytes()));
}

fn traverse_pc_mutable_main(root: &mut pcrs_generated::MainMutable, junk: &mut Junk) {
    junk.u32_sum = junk
        .u32_sum
        .wrapping_add(*root.i32() as u32)
        .wrapping_add(*root.u32())
        .wrapping_add(u32::from(*root.flag()))
        .wrapping_add(*root.mode() as u32);
    junk.u32_sum = junk
        .u32_sum
        .wrapping_add(*root.t_i32() as u32)
        .wrapping_add(*root.t_s32() as u32)
        .wrapping_add(*root.t_u32());
    for value in root.i32v().iter() {
        junk.u32_sum = junk.u32_sum.wrapping_add(*value as u32);
    }
    for value in root.flags().iter() {
        junk.u32_sum = junk.u32_sum.wrapping_add(u32::from(*value));
    }
    junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(root.str().as_bytes()));
    junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(root.data()));
    for value in root.strv().iter() {
        junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(value.as_bytes()));
    }
    for value in root.datav().iter() {
        junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(value));
    }

    junk.u64_sum = junk
        .u64_sum
        .wrapping_add(*root.i64() as u64)
        .wrapping_add(*root.u64())
        .wrapping_add(*root.t_i64() as u64)
        .wrapping_add(*root.t_s64() as u64)
        .wrapping_add(*root.t_u64());
    for value in root.u64v().iter() {
        junk.u64_sum = junk.u64_sum.wrapping_add(*value);
    }

    junk.add_f32(*root.f32());
    for value in root.f32v().iter() {
        junk.add_f32(*value);
    }

    junk.add_f64(*root.f64());
    for value in root.f64v().iter() {
        junk.add_f64(*value);
    }

    traverse_pc_mutable_small(root.object(), junk);
    for value in root.objectv().iter_mut() {
        traverse_pc_mutable_small(value, junk);
    }

    for (key, value) in root.index().iter() {
        junk.u32_sum = junk
            .u32_sum
            .wrapping_add(junk_hash_bytes(key.as_bytes()))
            .wrapping_add(*value as u32);
    }

    let object_keys = root.objects().iter().map(|(key, _)| *key).collect::<Vec<_>>();
    for key in object_keys {
        junk.u32_sum = junk.u32_sum.wrapping_add(key as u32);
        traverse_pc_mutable_small(root.objects().get_mut(&key).unwrap(), junk);
    }

    for row in root.matrix().iter() {
        for value in row.iter() {
            junk.add_f32(*value);
        }
    }
    for map in root.vector().iter() {
        for (key, value) in map.iter() {
            junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(key.as_bytes()));
            for item in value.iter() {
                junk.add_f32(*item);
            }
        }
    }
    for (key, value) in root.arrays().iter() {
        junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(key.as_bytes()));
        for item in value.iter() {
            junk.add_f32(*item);
        }
    }
}

fn benchmark_compress(
    name: &str,
    raw: &[u8],
    loops: usize,
) -> BenchResult<()> {
    let mut compressed = Vec::new();
    let start = Instant::now();
    for _ in 0..loops {
        compress_into(raw, &mut compressed);
    }
    print_size_result(
        &format!("{name}-compress"),
        compressed.len(),
        start.elapsed(),
        loops,
    );

    let mut restored = Vec::new();
    let start = Instant::now();
    for _ in 0..loops {
        decompress_into(&compressed, &mut restored)?;
    }
    if restored != raw {
        return Err(format!("{name} decompression roundtrip mismatch").into());
    }
    print_duration_result(&format!("{name}-decompress"), start.elapsed(), loops);
    Ok(())
}

fn print_access_result(name: &str, bytes: usize, elapsed: Duration, loops: usize, fuse: u64) {
    println!(
        "{name}: {bytes}B {} {fuse:016x}",
        format_millis(elapsed, loops),
    );
}

fn print_throughput_result(name: &str, elapsed: Duration, loops: usize, total_size: usize) {
    println!(
        "{name}: {} {:x}",
        format_millis(elapsed, loops),
        total_size
    );
}

fn print_size_result(name: &str, size: usize, elapsed: Duration, loops: usize) {
    println!(
        "{name}: {size}B {}",
        format_millis(elapsed, loops),
    );
}

fn print_duration_result(name: &str, elapsed: Duration, loops: usize) {
    println!("{name}: {}", format_millis(elapsed, loops));
}

fn print_reflect_result(name: &str, elapsed: Duration, fuse: u64) {
    println!("{name}: {}ms {fuse:016x}", elapsed.as_millis());
}

fn format_millis(elapsed: Duration, _loops: usize) -> String {
    let ms = elapsed.as_secs_f64() * 1_000.0;
    let total = ms.round() as u128;
    format!("{total}ms")
}

fn traverse_pb_small(root: &pb::Small, junk: &mut Junk) {
    junk.u32_sum = junk
        .u32_sum
        .wrapping_add(root.i32 as u32)
        .wrapping_add(u32::from(root.flag));
    junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(root.str.as_bytes()));
}

fn traverse_pb_vec2d(root: &pb::Vec2D, junk: &mut Junk) {
    for row in &root.values {
        for value in &row.values {
            junk.add_f32(*value);
        }
    }
}

fn traverse_pb_arr_map(root: &pb::ArrMap, junk: &mut Junk) {
    for (key, value) in &root.entries {
        junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(key.as_bytes()));
        for item in &value.values {
            junk.add_f32(*item);
        }
    }
}

fn traverse_pb_main(root: &pb::Main, junk: &mut Junk) {
    junk.u32_sum = junk
        .u32_sum
        .wrapping_add(root.i32 as u32)
        .wrapping_add(root.u32)
        .wrapping_add(u32::from(root.flag))
        .wrapping_add(root.mode as u32);
    junk.u32_sum = junk
        .u32_sum
        .wrapping_add(root.t_i32 as u32)
        .wrapping_add(root.t_s32 as u32)
        .wrapping_add(root.t_u32);
    for value in &root.i32v {
        junk.u32_sum = junk.u32_sum.wrapping_add(*value as u32);
    }
    for value in &root.flags {
        junk.u32_sum = junk.u32_sum.wrapping_add(u32::from(*value));
    }
    junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(root.str.as_bytes()));
    junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(&root.data));
    for value in &root.strv {
        junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(value.as_bytes()));
    }
    for value in &root.datav {
        junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(value));
    }

    junk.u64_sum = junk
        .u64_sum
        .wrapping_add(root.i64 as u64)
        .wrapping_add(root.u64)
        .wrapping_add(root.t_i64 as u64)
        .wrapping_add(root.t_s64 as u64)
        .wrapping_add(root.t_u64);
    for value in &root.u64v {
        junk.u64_sum = junk.u64_sum.wrapping_add(*value);
    }

    junk.add_f32(root.f32);
    for value in &root.f32v {
        junk.add_f32(*value);
    }

    junk.add_f64(root.f64);
    for value in &root.f64v {
        junk.add_f64(*value);
    }

    if let Some(object) = root.object.as_ref() {
        traverse_pb_small(object, junk);
    }
    for object in &root.objectv {
        traverse_pb_small(object, junk);
    }

    for (key, value) in &root.index {
        junk.u32_sum = junk
            .u32_sum
            .wrapping_add(junk_hash_bytes(key.as_bytes()))
            .wrapping_add(*value as u32);
    }
    for (key, value) in &root.objects {
        junk.u32_sum = junk.u32_sum.wrapping_add(*key as u32);
        traverse_pb_small(value, junk);
    }

    if let Some(matrix) = root.matrix.as_ref() {
        traverse_pb_vec2d(matrix, junk);
    }
    for value in &root.vector {
        traverse_pb_arr_map(value, junk);
    }
    if let Some(arrays) = root.arrays.as_ref() {
        traverse_pb_arr_map(arrays, junk);
    }
}

#[cfg(protocache_test_has_fory_generated)]
fn traverse_fory_small(root: &fory_generated::Small, junk: &mut Junk) {
    junk.u32_sum = junk
        .u32_sum
        .wrapping_add(root.i32 as u32)
        .wrapping_add(u32::from(root.flag));
    junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(root.str.as_bytes()));
}

#[cfg(protocache_test_has_fory_generated)]
fn traverse_fory_vec2d(root: &fory_generated::Vec2D, junk: &mut Junk) {
    for row in &root.x {
        for value in &row.x {
            junk.add_f32(*value);
        }
    }
}

#[cfg(protocache_test_has_fory_generated)]
fn traverse_fory_arr_map(root: &fory_generated::ArrMap, junk: &mut Junk) {
    for (key, value) in &root.x {
        junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(key.as_bytes()));
        for item in &value.x {
            junk.add_f32(*item);
        }
    }
}

#[cfg(protocache_test_has_fory_generated)]
fn traverse_fory_main(root: &fory_generated::Main, junk: &mut Junk) {
    junk.u32_sum = junk
        .u32_sum
        .wrapping_add(root.i32 as u32)
        .wrapping_add(root.u32)
        .wrapping_add(u32::from(root.flag))
        .wrapping_add(root.mode.clone() as u32);
    junk.u32_sum = junk
        .u32_sum
        .wrapping_add(root.t_i32 as u32)
        .wrapping_add(root.t_s32 as u32)
        .wrapping_add(root.t_u32);
    for value in &root.i32v {
        junk.u32_sum = junk.u32_sum.wrapping_add(*value as u32);
    }
    for value in &root.flags {
        junk.u32_sum = junk.u32_sum.wrapping_add(u32::from(*value));
    }
    junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(root.str.as_bytes()));
    junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(&root.data));
    for value in &root.strv {
        junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(value.as_bytes()));
    }
    for value in &root.datav {
        junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(value));
    }

    junk.u64_sum = junk
        .u64_sum
        .wrapping_add(root.i64 as u64)
        .wrapping_add(root.u64)
        .wrapping_add(root.t_i64 as u64)
        .wrapping_add(root.t_s64 as u64)
        .wrapping_add(root.t_u64);
    for value in &root.u64v {
        junk.u64_sum = junk.u64_sum.wrapping_add(*value);
    }

    junk.add_f32(root.f32);
    for value in &root.f32v {
        junk.add_f32(*value);
    }

    junk.add_f64(root.f64);
    for value in &root.f64v {
        junk.add_f64(*value);
    }

    if let Some(object) = root.object.as_ref() {
        traverse_fory_small(object, junk);
    }
    for value in &root.objectv {
        traverse_fory_small(value, junk);
    }

    for (key, value) in &root.index {
        junk.u32_sum = junk
            .u32_sum
            .wrapping_add(junk_hash_bytes(key.as_bytes()))
            .wrapping_add(*value as u32);
    }
    for (key, value) in &root.objects {
        junk.u32_sum = junk.u32_sum.wrapping_add(*key as u32);
        traverse_fory_small(value, junk);
    }

    if let Some(matrix) = root.matrix.as_ref() {
        traverse_fory_vec2d(matrix, junk);
    }
    for value in &root.vector {
        traverse_fory_arr_map(value, junk);
    }
    if let Some(arrays) = root.arrays.as_ref() {
        traverse_fory_arr_map(arrays, junk);
    }
}

#[cfg(protocache_test_has_flatbuffers_generated)]
fn traverse_fb_small(root: fb_generated::test::Small<'_>, junk: &mut Junk) {
    junk.u32_sum = junk
        .u32_sum
        .wrapping_add(root.i32_() as u32)
        .wrapping_add(u32::from(root.flag()));
    if let Some(value) = root.str() {
        junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(value.as_bytes()));
    }
}

#[cfg(protocache_test_has_flatbuffers_generated)]
fn traverse_fb_vec2d(root: fb_generated::test::Vec2D<'_>, junk: &mut Junk) {
    if let Some(rows) = root.values() {
        for row in rows {
            if let Some(values) = row.values() {
                for value in values {
                    junk.add_f32(value);
                }
            }
        }
    }
}

#[cfg(protocache_test_has_flatbuffers_generated)]
fn traverse_fb_arr_map(root: fb_generated::test::ArrMap<'_>, junk: &mut Junk) {
    if let Some(entries) = root.entries() {
        for entry in entries {
            junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(entry.key().as_bytes()));
            if let Some(value) = entry.value().and_then(|item| item.values()) {
                for item in value {
                    junk.add_f32(item);
                }
            }
        }
    }
}

#[cfg(protocache_test_has_flatbuffers_generated)]
fn traverse_fb_main(root: fb_generated::test::Main<'_>, junk: &mut Junk) {
    junk.u32_sum = junk
        .u32_sum
        .wrapping_add(root.i32_() as u32)
        .wrapping_add(root.u32_())
        .wrapping_add(u32::from(root.flag()))
        .wrapping_add(root.mode().0 as u32);
    junk.u32_sum = junk
        .u32_sum
        .wrapping_add(root.t_i32() as u32 + root.t_s32() as u32 + root.t_u32());
    if let Some(values) = root.i32v() {
        for value in values {
            junk.u32_sum = junk.u32_sum.wrapping_add(value as u32);
        }
    }
    if let Some(values) = root.flags() {
        for value in values.iter() {
            junk.u32_sum = junk.u32_sum.wrapping_add(u32::from(value));
        }
    }
    if let Some(value) = root.str() {
        junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(value.as_bytes()));
    }
    if let Some(value) = root.data() {
        junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_i8_items(value.iter()));
    }
    if let Some(values) = root.strv() {
        for value in values {
            junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(value.as_bytes()));
        }
    }
    if let Some(values) = root.datav() {
        for value in values {
            if let Some(bytes) = value.bytes() {
                junk.u32_sum = junk
                    .u32_sum
                    .wrapping_add(junk_hash_i8_items(bytes.iter()));
            }
        }
    }

    junk.u64_sum = junk.u64_sum.wrapping_add(
        (root.i64_() as u64)
            .wrapping_add(root.u64_())
            .wrapping_add(root.t_i64() as u64)
            .wrapping_add(root.t_s64() as u64)
            .wrapping_add(root.t_u64()),
    );
    if let Some(values) = root.u64v() {
        for value in values {
            junk.u64_sum = junk.u64_sum.wrapping_add(value);
        }
    }

    junk.add_f32(root.f32_());
    if let Some(values) = root.f32v() {
        for value in values {
            junk.add_f32(value);
        }
    }

    junk.add_f64(root.f64_());
    if let Some(values) = root.f64v() {
        for value in values {
            junk.add_f64(value);
        }
    }

    if let Some(object) = root.object() {
        traverse_fb_small(object, junk);
    }
    if let Some(values) = root.objectv() {
        for value in values {
            traverse_fb_small(value, junk);
        }
    }

    if let Some(entries) = root.index() {
        for entry in entries {
            junk.u32_sum = junk
                .u32_sum
                .wrapping_add(junk_hash_bytes(entry.key().as_bytes()))
                .wrapping_add(entry.value() as u32);
        }
    }
    if let Some(entries) = root.objects() {
        for entry in entries {
            junk.u32_sum = junk.u32_sum.wrapping_add(entry.key() as u32);
            if let Some(value) = entry.value() {
                traverse_fb_small(value, junk);
            }
        }
    }

    if let Some(matrix) = root.matrix() {
        traverse_fb_vec2d(matrix, junk);
    }
    if let Some(values) = root.vector() {
        for value in values {
            traverse_fb_arr_map(value, junk);
        }
    }
    if let Some(arrays) = root.arrays() {
        traverse_fb_arr_map(arrays, junk);
    }
}

fn traverse_pc_small(root: MessageView<'_>, junk: &mut Junk) -> BenchResult<()> {
    junk.u32_sum = junk
        .u32_sum
        .wrapping_add(root.scalar::<i32>(0).unwrap_or_default() as u32);
    junk.u32_sum = junk
        .u32_sum
        .wrapping_add(u32::from(root.scalar::<bool>(1).unwrap_or_default()));
    if let Some(value) = root.string(3) {
        junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(value.as_bytes()));
    }
    Ok(())
}

fn traverse_pc_alias_array(field: FieldView<'_>, junk: &mut Junk) -> BenchResult<()> {
    let array = ArrayView::new(field.object_words().ok_or("alias array missing words")?)
        .ok_or("invalid alias array")?;
    let values = array.scalars::<f32>().ok_or("alias array is not f32")?;
    for value in values.iter() {
        junk.add_f32(value);
    }
    Ok(())
}

fn traverse_pc_vec2d(field: FieldView<'_>, junk: &mut Junk) -> BenchResult<()> {
    let rows = ArrayView::new(field.object_words().ok_or("matrix missing words")?).ok_or("invalid matrix")?;
    for row in rows.iter() {
        traverse_pc_alias_array(row, junk)?;
    }
    Ok(())
}

fn traverse_pc_arr_map(field: FieldView<'_>, junk: &mut Junk) -> BenchResult<()> {
    let map = MapView::new(field.object_words().ok_or("arr map missing words")?)
        .ok_or("invalid arr map")?;
    for pair in map.iter() {
        if let Some(key) = pair.key().string() {
            junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(key.as_bytes()));
        }
        traverse_pc_alias_array(pair.value(), junk)?;
    }
    Ok(())
}

fn traverse_pc_main(root: MessageView<'_>, junk: &mut Junk) -> BenchResult<()> {
    junk.u32_sum = junk.u32_sum.wrapping_add(
        (root.scalar::<i32>(0).unwrap_or_default() as u32)
            .wrapping_add(root.scalar::<u32>(1).unwrap_or_default())
            .wrapping_add(u32::from(root.scalar::<bool>(4).unwrap_or_default()))
            .wrapping_add(root.scalar::<i32>(5).unwrap_or_default() as u32),
    );
    junk.u32_sum = junk.u32_sum.wrapping_add(
        (root.scalar::<i32>(20).unwrap_or_default() as u32)
            .wrapping_add(root.scalar::<i32>(21).unwrap_or_default() as u32)
            .wrapping_add(root.scalar::<u32>(19).unwrap_or_default()),
    );

    if let Some(values) = root.array(11).and_then(|array| array.scalars::<i32>()) {
        for value in values.iter() {
            junk.u32_sum = junk.u32_sum.wrapping_add(value as u32);
        }
    }
    if let Some(values) = root.bools(17) {
        for value in values.iter() {
            junk.u32_sum = junk.u32_sum.wrapping_add(u32::from(value));
        }
    }
    if let Some(value) = root.string(6) {
        junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(value.as_bytes()));
    }
    if let Some(value) = root.bytes(7) {
        junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(value));
    }
    if let Some(values) = root.array(13) {
        for value in ViewArray::<StringView<'_>>::new(values).iter() {
            junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(value.as_bytes()));
        }
    }
    if let Some(values) = root.array(14) {
        for value in values.iter() {
            if let Some(bytes) = value.string() {
                junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(bytes.as_bytes()));
            }
        }
    }

    junk.u64_sum = junk.u64_sum.wrapping_add(
        (root.scalar::<i64>(2).unwrap_or_default() as u64)
            .wrapping_add(root.scalar::<u64>(3).unwrap_or_default())
            .wrapping_add(root.scalar::<i64>(23).unwrap_or_default() as u64)
            .wrapping_add(root.scalar::<i64>(24).unwrap_or_default() as u64)
            .wrapping_add(root.scalar::<u64>(22).unwrap_or_default()),
    );
    if let Some(values) = root.array(12).and_then(|array| array.scalars::<u64>()) {
        for value in values.iter() {
            junk.u64_sum = junk.u64_sum.wrapping_add(value);
        }
    }

    junk.add_f32(root.scalar::<f32>(8).unwrap_or_default());
    if let Some(values) = root.array(15).and_then(|array| array.scalars::<f32>()) {
        for value in values.iter() {
            junk.add_f32(value);
        }
    }

    junk.add_f64(root.scalar::<f64>(9).unwrap_or_default());
    if let Some(values) = root.array(16).and_then(|array| array.scalars::<f64>()) {
        for value in values.iter() {
            junk.add_f64(value);
        }
    }

    if let Some(value) = root.message(10) {
        traverse_pc_small(value, junk)?;
    }
    if let Some(values) = root.array(18) {
        for value in ViewArray::<MessageView<'_>>::new(values).iter() {
            traverse_pc_small(value, junk)?;
        }
    }

    if let Some(index) = root.map(25) {
        for pair in index.iter() {
            if let Some(key) = pair.key().string() {
                junk.u32_sum = junk.u32_sum.wrapping_add(
                    junk_hash_bytes(key.as_bytes())
                        .wrapping_add(pair.value().scalar::<i32>().unwrap_or_default() as u32),
                );
            }
        }
    }
    if let Some(objects) = root.map(26) {
        for pair in objects.iter() {
            junk.u32_sum = junk.u32_sum.wrapping_add(pair.key().scalar::<i32>().unwrap_or_default() as u32);
            if let Some(value) = pair.value().message() {
                traverse_pc_small(value, junk)?;
            }
        }
    }

    if let Some(value) = root.field(27) {
        traverse_pc_vec2d(value, junk)?;
    }
    if let Some(values) = root.array(28) {
        for value in values.iter() {
            traverse_pc_arr_map(value, junk)?;
        }
    }
    if let Some(value) = root.field(29) {
        traverse_pc_arr_map(value, junk)?;
    }
    Ok(())
}

fn build_reflect_descriptor_plan(
    pool: &PcDescriptorPool,
    descriptor_name: &str,
    descriptor: &PcDescriptor,
) -> BenchResult<ReflectDescriptorPlan> {
    let mut fields = descriptor.fields.iter().collect::<Vec<_>>();
    fields.sort_by_key(|(name, field)| reflect_field_order(descriptor_name, name, field.id));
    let mut planned = Vec::with_capacity(fields.len());
    for (_, field) in fields {
        planned.push(build_reflect_field_plan(pool, field)?);
    }
    Ok(ReflectDescriptorPlan { fields: planned })
}

fn build_pb_reflect_descriptor_plan(descriptor: &prost_reflect::MessageDescriptor) -> PbReflectDescriptorPlan {
    let mut fields = descriptor.fields().collect::<Vec<_>>();
    fields.sort_by_key(|field| {
        reflect_field_order(
            descriptor.full_name(),
            field.name(),
            field.number() as usize,
        )
    });

    let fields = fields
        .into_iter()
        .map(|descriptor| {
            let value = build_pb_reflect_value_plan(&descriptor);
            PbReflectFieldPlan { descriptor, value }
        })
        .collect();
    PbReflectDescriptorPlan { fields }
}

fn build_pb_reflect_value_plan(field: &ReflectFieldDescriptor) -> PbReflectValuePlan {
    let kind = if field.is_map() {
        match field.kind() {
            ReflectKind::Message(entry) => entry
                .get_field(2)
                .expect("protobuf reflect map entry must contain value field")
                .kind(),
            _ => unreachable!("protobuf reflect map field must use a map-entry message"),
        }
    } else {
        field.kind()
    };

    match kind {
        ReflectKind::Message(descriptor) => {
            PbReflectValuePlan::Message(Box::new(build_pb_reflect_descriptor_plan(&descriptor)))
        }
        ReflectKind::Bytes => PbReflectValuePlan::Bytes,
        ReflectKind::String => PbReflectValuePlan::String,
        ReflectKind::Double => PbReflectValuePlan::Double,
        ReflectKind::Float => PbReflectValuePlan::Float,
        ReflectKind::Uint64
        | ReflectKind::Fixed64 => PbReflectValuePlan::Uint64,
        ReflectKind::Uint32
        | ReflectKind::Fixed32 => PbReflectValuePlan::Uint32,
        ReflectKind::Int64
        | ReflectKind::Sint64
        | ReflectKind::Sfixed64 => PbReflectValuePlan::Int64,
        ReflectKind::Int32
        | ReflectKind::Sint32
        | ReflectKind::Sfixed32 => PbReflectValuePlan::Int32,
        ReflectKind::Bool => PbReflectValuePlan::Bool,
        ReflectKind::Enum(_) => PbReflectValuePlan::Enum,
    }
}

fn build_reflect_field_plan(
    pool: &PcDescriptorPool,
    field: &PcField,
) -> BenchResult<ReflectFieldPlan> {
    Ok(ReflectFieldPlan {
        id: field.id,
        repeated: field.repeated,
        key: (field.key != PcFieldType::None).then_some(field.key),
        value: build_reflect_value_plan(pool, field)?,
    })
}

fn build_reflect_value_plan(
    pool: &PcDescriptorPool,
    field: &PcField,
) -> BenchResult<ReflectValuePlan> {
    Ok(match field.value {
        PcFieldType::Message => {
            let descriptor = pool
                .find(if field.value_type.is_empty() {
                    return Err("missing reflected message type".into());
                } else {
                    &field.value_type
                })
                .ok_or("missing reflected message descriptor")?;
            ReflectValuePlan::Message {
                alias: if descriptor.is_alias() {
                    Some(Box::new(build_reflect_field_plan(pool, &descriptor.alias)?))
                } else {
                    None
                },
                descriptor: if !descriptor.is_alias() {
                    Some(Box::new(build_reflect_descriptor_plan(pool, &field.value_type, descriptor)?))
                } else {
                    None
                },
            }
        }
        PcFieldType::Bytes => ReflectValuePlan::Bytes,
        PcFieldType::String => ReflectValuePlan::String,
        PcFieldType::Double => ReflectValuePlan::Double,
        PcFieldType::Float => ReflectValuePlan::Float,
        PcFieldType::Uint64 => ReflectValuePlan::Uint64,
        PcFieldType::Uint32 => ReflectValuePlan::Uint32,
        PcFieldType::Int64 => ReflectValuePlan::Int64,
        PcFieldType::Int32 => ReflectValuePlan::Int32,
        PcFieldType::Bool => ReflectValuePlan::Bool,
        PcFieldType::Enum => ReflectValuePlan::Enum,
        PcFieldType::None | PcFieldType::Unknown => {
            return Err("invalid reflected field type".into());
        }
    })
}

fn reflect_field_order(descriptor_name: &str, field_name: &str, field_id: usize) -> usize {
    const SMALL_ORDER: &[&str] = &["str", "flag", "i32"];
    const MAIN_ORDER: &[&str] = &[
        "arrays",
        "t_i64",
        "vector",
        "t_i32",
        "objects",
        "flags",
        "f32v",
        "datav",
        "strv",
        "t_u64",
        "u64v",
        "matrix",
        "t_s64",
        "object",
        "f32",
        "objectv",
        "data",
        "str",
        "mode",
        "f64v",
        "flag",
        "f64",
        "u64",
        "index",
        "t_s32",
        "t_u32",
        "i32v",
        "i64",
        "u32",
        "i32",
    ];

    let known = match descriptor_name {
        "test.Small" => SMALL_ORDER,
        "test.Main" => MAIN_ORDER,
        _ => &[],
    };
    known
        .iter()
        .position(|name| *name == field_name)
        .unwrap_or(known.len() + field_id)
}

fn traverse_pc_reflect_descriptor(
    descriptor: &ReflectDescriptorPlan,
    root: MessageView<'_>,
    junk: &mut Junk,
) {
    for field in &descriptor.fields {
        let Some(value) = root.field(field.id) else {
            continue;
        };
        traverse_pc_reflect_field(field, value, junk);
    }
}

fn traverse_pc_reflect_field(
    field: &ReflectFieldPlan,
    value: FieldView<'_>,
    junk: &mut Junk,
) {
    if let Some(key_type) = field.key {
        let map = value.expect_map();
        for pair in map.iter() {
            traverse_pc_reflect_map_key(key_type, pair.key(), junk);
            traverse_pc_reflect_value(&field.value, pair.value(), junk);
        }
        return;
    }

    if field.repeated {
        match &field.value {
            ReflectValuePlan::Message { alias, descriptor } => {
                if let Some(alias) = alias {
                    let array = value.expect_array();
                    for item in array.iter() {
                        traverse_pc_reflect_field(alias, item, junk);
                    }
                } else {
                    let array = value.expect_array();
                    for item in array.iter() {
                        traverse_pc_reflect_descriptor(
                            descriptor.as_deref().expect("missing reflected descriptor plan"),
                            item.expect_message(),
                            junk,
                        );
                    }
                }
            }
            ReflectValuePlan::Bytes | ReflectValuePlan::String => {
                let array = value.expect_array();
                for item in array.iter() {
                    traverse_pc_reflect_value(&field.value, item, junk);
                }
            }
            ReflectValuePlan::Double => {
                let array = value.expect_array();
                for item in array.expect_scalars::<f64>().iter() {
                    junk.add_f64(item);
                }
            }
            ReflectValuePlan::Float => {
                let array = value.expect_array();
                for item in array.expect_scalars::<f32>().iter() {
                    junk.add_f32(item);
                }
            }
            ReflectValuePlan::Uint64 => {
                let array = value.expect_array();
                for item in array.expect_scalars::<u64>().iter() {
                    junk.u64_sum = junk.u64_sum.wrapping_add(item);
                }
            }
            ReflectValuePlan::Uint32 => {
                let array = value.expect_array();
                for item in array.expect_scalars::<u32>().iter() {
                    junk.u32_sum = junk.u32_sum.wrapping_add(item);
                }
            }
            ReflectValuePlan::Int64 => {
                let array = value.expect_array();
                for item in array.expect_scalars::<i64>().iter() {
                    junk.u64_sum = junk.u64_sum.wrapping_add(item as u64);
                }
            }
            ReflectValuePlan::Int32 | ReflectValuePlan::Enum => {
                let array = value.expect_array();
                for item in array.expect_scalars::<i32>().iter() {
                    junk.u32_sum = junk.u32_sum.wrapping_add(item as u32);
                }
            }
            ReflectValuePlan::Bool => {
                let bools = value.expect_string().as_bool_array();
                for item in bools.iter() {
                    junk.u32_sum = junk.u32_sum.wrapping_add(u32::from(item));
                }
            }
        }
        return;
    }

    traverse_pc_reflect_value(&field.value, value, junk)
}

fn traverse_pc_reflect_map_key(key_type: PcFieldType, key: FieldView<'_>, junk: &mut Junk) {
    match key_type {
        PcFieldType::String => {
            let value = key.expect_string();
            junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(value.as_bytes()));
        }
        PcFieldType::Uint64 => {
            junk.u32_sum = junk
                .u32_sum
                .wrapping_add(key.expect_scalar::<u64>() as u32);
        }
        PcFieldType::Uint32 => {
            junk.u32_sum = junk.u32_sum.wrapping_add(key.expect_scalar::<u32>());
        }
        PcFieldType::Int64 => {
            junk.u32_sum = junk
                .u32_sum
                .wrapping_add(key.expect_scalar::<i64>() as u32);
        }
        PcFieldType::Int32 | PcFieldType::Enum => {
            junk.u32_sum = junk
                .u32_sum
                .wrapping_add(key.expect_scalar::<i32>() as u32);
        }
        PcFieldType::Bool => {
            junk.u32_sum = junk
                .u32_sum
                .wrapping_add(u32::from(key.expect_scalar::<bool>()));
        }
        PcFieldType::Message
        | PcFieldType::Bytes
        | PcFieldType::Double
        | PcFieldType::Float
        | PcFieldType::None
        | PcFieldType::Unknown => {}
    }
}

fn traverse_pc_reflect_value(
    field: &ReflectValuePlan,
    value: FieldView<'_>,
    junk: &mut Junk,
) {
    match field {
        ReflectValuePlan::Message { alias, descriptor } => {
            if let Some(alias) = alias {
                traverse_pc_reflect_field(alias, value, junk);
            } else {
                let message = value.expect_message();
                traverse_pc_reflect_descriptor(
                    descriptor.as_deref().expect("missing reflected descriptor plan"),
                    message,
                    junk,
                );
            }
        }
        ReflectValuePlan::Bytes => {
            let bytes = value.expect_string();
            junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(bytes.as_bytes()));
        }
        ReflectValuePlan::String => {
            let string = value.expect_string();
            junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(string.as_bytes()));
        }
        ReflectValuePlan::Double => junk.add_f64(value.expect_scalar::<f64>()),
        ReflectValuePlan::Float => junk.add_f32(value.expect_scalar::<f32>()),
        ReflectValuePlan::Uint64 => {
            junk.u64_sum = junk.u64_sum.wrapping_add(value.expect_scalar::<u64>());
        }
        ReflectValuePlan::Uint32 => {
            junk.u32_sum = junk.u32_sum.wrapping_add(value.expect_scalar::<u32>());
        }
        ReflectValuePlan::Int64 => {
            junk.u64_sum = junk
                .u64_sum
                .wrapping_add(value.expect_scalar::<i64>() as u64);
        }
        ReflectValuePlan::Int32 | ReflectValuePlan::Enum => {
            junk.u32_sum = junk
                .u32_sum
                .wrapping_add(value.expect_scalar::<i32>() as u32);
        }
        ReflectValuePlan::Bool => {
            junk.u32_sum = junk
                .u32_sum
                .wrapping_add(u32::from(value.expect_scalar::<bool>()));
        }
    }
}

fn traverse_pb_reflect_descriptor(
    descriptor: &PbReflectDescriptorPlan,
    root: &DynamicMessage,
    junk: &mut Junk,
) -> BenchResult<()> {
    for field in &descriptor.fields {
        if !root.has_field(&field.descriptor)
            && (field.descriptor.supports_presence()
                || field.descriptor.is_list()
                || field.descriptor.is_map())
        {
            continue;
        }
        let value = root.get_field(&field.descriptor);
        traverse_pb_reflect_field(field, value.as_ref(), junk)?;
    }
    Ok(())
}

fn traverse_pb_reflect_field(
    field: &PbReflectFieldPlan,
    value: &ReflectValue,
    junk: &mut Junk,
) -> BenchResult<()> {
    if field.descriptor.is_map() {
        let ReflectValue::Map(entries) = value else {
            return Err("invalid protobuf reflected map".into());
        };
        for (key, value) in entries {
            traverse_pb_reflect_map_key(key, junk);
            traverse_pb_reflect_value(&field.value, value, junk)?;
        }
        return Ok(());
    }

    if field.descriptor.is_list() {
        let ReflectValue::List(items) = value else {
            return Err("invalid protobuf reflected list".into());
        };
        for item in items {
            traverse_pb_reflect_value(&field.value, item, junk)?;
        }
        return Ok(());
    }

    traverse_pb_reflect_value(&field.value, value, junk)
}

fn traverse_pb_reflect_map_key(key: &ReflectMapKey, junk: &mut Junk) {
    match key {
        ReflectMapKey::Bool(value) => {
            junk.u32_sum = junk.u32_sum.wrapping_add(u32::from(*value));
        }
        ReflectMapKey::I32(value) => {
            junk.u32_sum = junk.u32_sum.wrapping_add(*value as u32);
        }
        ReflectMapKey::I64(value) => {
            junk.u32_sum = junk.u32_sum.wrapping_add(*value as u32);
        }
        ReflectMapKey::U32(value) => {
            junk.u32_sum = junk.u32_sum.wrapping_add(*value);
        }
        ReflectMapKey::U64(value) => {
            junk.u32_sum = junk.u32_sum.wrapping_add(*value as u32);
        }
        ReflectMapKey::String(value) => {
            junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(value.as_bytes()));
        }
    }
}

fn traverse_pb_reflect_value(
    field: &PbReflectValuePlan,
    value: &ReflectValue,
    junk: &mut Junk,
) -> BenchResult<()> {
    match (field, value) {
        (PbReflectValuePlan::Message(descriptor), ReflectValue::Message(message)) => {
            traverse_pb_reflect_descriptor(descriptor, message, junk)?;
        }
        (PbReflectValuePlan::Bytes, ReflectValue::Bytes(bytes)) => {
            junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(bytes));
        }
        (PbReflectValuePlan::String, ReflectValue::String(string)) => {
            junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(string.as_bytes()));
        }
        (PbReflectValuePlan::Double, ReflectValue::F64(value)) => junk.add_f64(*value),
        (PbReflectValuePlan::Float, ReflectValue::F32(value)) => junk.add_f32(*value),
        (PbReflectValuePlan::Uint64, ReflectValue::U64(value)) => {
            junk.u64_sum = junk.u64_sum.wrapping_add(*value);
        }
        (PbReflectValuePlan::Uint32, ReflectValue::U32(value)) => {
            junk.u32_sum = junk.u32_sum.wrapping_add(*value);
        }
        (PbReflectValuePlan::Int64, ReflectValue::I64(value)) => {
            junk.u64_sum = junk.u64_sum.wrapping_add(*value as u64);
        }
        (PbReflectValuePlan::Int32, ReflectValue::I32(value))
        | (PbReflectValuePlan::Enum, ReflectValue::EnumNumber(value)) => {
            junk.u32_sum = junk.u32_sum.wrapping_add(*value as u32);
        }
        (PbReflectValuePlan::Bool, ReflectValue::Bool(value)) => {
            junk.u32_sum = junk.u32_sum.wrapping_add(u32::from(*value));
        }
        _ => return Err("invalid protobuf reflected value".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::borrow::Borrow;
use std::time::Duration;
use std::time::Instant;

use prost::Message;
use protocache_core::{
    ArrayView, FieldView, MapView, MessageView, StringView, ViewArray, compress_into,
    decompress_into,
};
use protocache_mutable::{MessageMut, serialize_protobuf_bytes_into_buffer};
use protocache_schema::load_reflect_descriptor_pool_from_proto_file;

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
mod fb_generated {
    include!(concat!(env!("OUT_DIR"), "/test_generated.rs"));
}

const DEFAULT_LOOPS: usize = 1_000_000;

#[derive(Default)]
struct Junk {
    u32_sum: u32,
    u64_sum: u64,
    f32_bits_sum: u32,
    f64_bits_sum: u64,
}

impl Junk {
    fn add_f32(&mut self, value: f32) {
        self.f32_bits_sum = self.f32_bits_sum.wrapping_add(value.to_bits());
    }

    fn add_f64(&mut self, value: f64) {
        self.f64_bits_sum = self.f64_bits_sum.wrapping_add(value.to_bits());
    }

    fn fuse(&self) -> u64 {
        self.f64_bits_sum ^ ((self.f32_bits_sum as u64) << 32 | self.u32_sum as u64) ^ self.u64_sum
    }
}

struct BenchConfig {
    loops: usize,
}

impl BenchConfig {
    fn from_args() -> Result<Self, String> {
        let mut loops = DEFAULT_LOOPS;
        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--loops" => {
                    let value = args.next().ok_or("--loops requires a value")?;
                    loops = value
                        .parse()
                        .map_err(|_| format!("invalid --loops value: {value}"))?;
                }
                "--help" | "-h" => {
                    println!("Usage: cargo run -p protocache-benchmark --release -- [--loops N]");
                    std::process::exit(0);
                }
                _ => return Err(format!("unknown argument: {arg}")),
            }
        }
        Ok(Self { loops })
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = BenchConfig::from_args().map_err(|err| format!("argument error: {err}"))?;
    let fixture_root = repo_root().join("tests/fixtures");
    let fixture_dir = fixture_root.join("benchmark");
    let schema_path = fixture_root.join("proto/benchmark-test.proto");

    let protobuf_raw = fs::read(fixture_dir.join("test.pb"))?;
    let flatbuffers_raw = fs::read(fixture_dir.join("test.fb"))?;
    let protocache_raw = fs::read(fixture_dir.join("test.pc"))?;
    let protocache_words = bytes_to_words(&protocache_raw);

    benchmark_protobuf(&protobuf_raw, config.loops)?;
    benchmark_flatbuffers(&flatbuffers_raw, config.loops)?;
    benchmark_flatbuffers_unchecked(&flatbuffers_raw, config.loops)?;
    benchmark_protocache(&protocache_words, &protocache_raw, config.loops)?;

    println!("========serialize========");
    benchmark_protobuf_serialize(&protobuf_raw, config.loops)?;
    benchmark_protobuf_to_protocache_serialize(&protobuf_raw, &schema_path, config.loops)?;
    benchmark_protocache_serialize_aligned(&schema_path, &protocache_words, config.loops, false)?;
    benchmark_protocache_serialize_aligned(&schema_path, &protocache_words, config.loops, true)?;
    benchmark_serialize_micro(&protobuf_raw, &schema_path, &protocache_words, config.loops)?;

    println!("========compress========");
    benchmark_compress("pb", &protobuf_raw, config.loops)?;
    benchmark_compress("pc", &protocache_raw, config.loops)?;
    benchmark_compress("fb", &flatbuffers_raw, config.loops)?;
    Ok(())
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

fn junk_hash_bytes(data: &[u8]) -> u32 {
    let mut out = 0u32;
    let mut chunks = data.chunks_exact(4);
    for chunk in &mut chunks {
        out ^= u32::from_le_bytes(chunk.try_into().unwrap());
    }
    out
}

fn junk_hash_i8_items<I, T>(data: I) -> u32
where
    I: IntoIterator<Item = T>,
    T: Borrow<i8>,
{
    let bytes = data.into_iter().map(|v| *v.borrow() as u8).collect::<Vec<_>>();
    junk_hash_bytes(&bytes)
}

fn benchmark_protobuf(raw: &[u8], loops: usize) -> Result<(), Box<dyn std::error::Error>> {
    let mut junk = Junk::default();
    let start = Instant::now();
    for _ in 0..loops {
        let root = pb::Main::decode(raw)?;
        traverse_pb_main(&root, &mut junk);
    }
    print_access_result("protobuf", raw.len(), start.elapsed(), loops, junk.fuse());
    Ok(())
}

fn benchmark_flatbuffers(raw: &[u8], loops: usize) -> Result<(), Box<dyn std::error::Error>> {
    let mut junk = Junk::default();
    let start = Instant::now();
    for _ in 0..loops {
        let root = fb_generated::test::root_as_main(raw)?;
        traverse_fb_main(root, &mut junk);
    }
    print_access_result("flatbuffers", raw.len(), start.elapsed(), loops, junk.fuse());
    Ok(())
}

fn benchmark_flatbuffers_unchecked(raw: &[u8], loops: usize) -> Result<(), Box<dyn std::error::Error>> {
    let mut junk = Junk::default();
    let start = Instant::now();
    for _ in 0..loops {
        let root = unsafe { fb_generated::test::root_as_main_unchecked(raw) };
        traverse_fb_main(root, &mut junk);
    }
    print_access_result(
        "flatbuffers-unchecked",
        raw.len(),
        start.elapsed(),
        loops,
        junk.fuse(),
    );
    Ok(())
}

fn benchmark_protocache(
    words: &[u32],
    raw: &[u8],
    loops: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut junk = Junk::default();
    let start = Instant::now();
    for _ in 0..loops {
        let root = MessageView::new(words).ok_or("invalid protocache fixture")?;
        traverse_pc_main(root, &mut junk)?;
    }
    print_access_result("protocache", raw.len(), start.elapsed(), loops, junk.fuse());
    Ok(())
}

fn benchmark_protobuf_serialize(raw: &[u8], loops: usize) -> Result<(), Box<dyn std::error::Error>> {
    let root = pb::Main::decode(raw)?;
    let mut total_size = 0usize;
    let start = Instant::now();
    for _ in 0..loops {
        let encoded = root.encode_to_vec();
        total_size += encoded.len();
    }
    print_throughput_result("protobuf-serialize", start.elapsed(), loops, total_size);
    Ok(())
}

fn benchmark_protocache_serialize_aligned(
    schema_path: &Path,
    words: &[u32],
    loops: usize,
    partly: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut root = MessageMut::from_proto_file(schema_path, "test.Main", words)
        .map_err(|err| format!("failed to open mutable protocache root: {err:?}"))?;

    if partly {
        touch_protocache_partly(&mut root)?;
    } else {
        touch_protocache_fully(&mut root)?;
    }

    let mut total_size = 0usize;
    let mut buffer = protocache_core::Buffer::new();
    let start = Instant::now();
    for _ in 0..loops {
        let encoded = root
            .serialize_into_buffer(&mut buffer)
            .map_err(|err| format!("failed to serialize protocache words into reusable buffer: {err:?}"))?;
        total_size += encoded.len() * std::mem::size_of::<u32>();
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

fn benchmark_protobuf_to_protocache_serialize(
    raw: &[u8],
    schema_path: &Path,
    loops: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let pool = load_reflect_descriptor_pool_from_proto_file(schema_path)?;
    let descriptor = pool
        .get_message_by_name("test.Main")
        .ok_or("missing test.Main descriptor")?;

    let mut total_size = 0usize;
    let mut buffer = protocache_core::Buffer::new();
    let start = Instant::now();
    for _ in 0..loops {
        let encoded = serialize_protobuf_bytes_into_buffer(descriptor.clone(), raw, &mut buffer)?;
        total_size += encoded.len() * std::mem::size_of::<u32>();
    }
    print_throughput_result(
        "protobuf-to-protocache",
        start.elapsed(),
        loops,
        total_size,
    );
    Ok(())
}

fn benchmark_serialize_micro(
    protobuf_raw: &[u8],
    schema_path: &Path,
    protocache_words: &[u32],
    loops: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let pb_root = pb::Main::decode(protobuf_raw)?;
    let reflect_pool = load_reflect_descriptor_pool_from_proto_file(schema_path)?;
    let reflect_descriptor = reflect_pool
        .get_message_by_name("test.Main")
        .ok_or("missing test.Main descriptor")?;
    let pc_root = MessageMut::from_proto_file(schema_path, "test.Main", protocache_words)
        .map_err(|err| format!("failed to open mutable protocache root: {err:?}"))?;

    let sample_loops = loops.clamp(1_000, 50_000);
    let rounds = 7usize;

    let (pb_times, pb_total) = collect_samples(rounds, || {
        let mut total_size = 0usize;
        let start = Instant::now();
        for _ in 0..sample_loops {
            total_size += pb_root.encode_to_vec().len();
        }
        (start.elapsed().as_micros(), total_size)
    });

    let (pc_times, pc_total) = collect_samples_result(rounds, || {
        let mut total_size = 0usize;
        let start = Instant::now();
        for _ in 0..sample_loops {
            total_size += pc_root
                .serialize_words()
                .map(|words| words.len() * std::mem::size_of::<u32>())
                .map_err(|err| format!("failed to serialize protocache words: {err:?}"))?;
        }
        Ok::<_, String>((start.elapsed().as_micros(), total_size))
    })?;

    let (pc_reuse_times, pc_reuse_total) = collect_samples_result(rounds, || {
        let mut total_size = 0usize;
        let mut buffer = protocache_core::Buffer::new();
        let start = Instant::now();
        for _ in 0..sample_loops {
            total_size += pc_root
                .serialize_into_buffer(&mut buffer)
                .map(|words| words.len() * std::mem::size_of::<u32>())
                .map_err(|err| format!("failed to serialize protocache words into reusable buffer: {err:?}"))?;
        }
        Ok::<_, String>((start.elapsed().as_micros(), total_size))
    })?;

    let (pb_to_pc_times, pb_to_pc_total) = collect_samples_result(rounds, || {
        let mut total_size = 0usize;
        let mut buffer = protocache_core::Buffer::new();
        let start = Instant::now();
        for _ in 0..sample_loops {
            total_size += serialize_protobuf_bytes_into_buffer(
                reflect_descriptor.clone(),
                protobuf_raw,
                &mut buffer,
            )
                .map(|words| words.len() * std::mem::size_of::<u32>())
                .map_err(|err| err.to_string())?;
        }
        Ok::<_, String>((start.elapsed().as_micros(), total_size))
    })?;

    println!("========serialize-micro========");
    print_sample_stats("protobuf-serialize-micro", sample_loops, &pb_times, pb_total);
    print_sample_stats(
        "protobuf-to-protocache-micro",
        sample_loops,
        &pb_to_pc_times,
        pb_to_pc_total,
    );
    print_sample_stats("protocache-serialize-micro", sample_loops, &pc_times, pc_total);
    print_sample_stats(
        "protocache-serialize-reuse-micro",
        sample_loops,
        &pc_reuse_times,
        pc_reuse_total,
    );
    Ok(())
}

fn touch_protocache_partly(root: &mut MessageMut<'_>) -> Result<(), Box<dyn std::error::Error>> {
    for field in [
        "i32", "u32", "i64", "u64", "flag", "mode", "str", "data", "f32", "f64",
    ] {
        root.decode_field(field)
            .map_err(|err| format!("failed to decode field {field}: {err:?}"))?;
    }
    Ok(())
}

fn touch_protocache_fully(root: &mut MessageMut<'_>) -> Result<(), Box<dyn std::error::Error>> {
    for field in [
        "i32", "u32", "i64", "u64", "flag", "mode", "str", "data", "f32", "f64", "object", "i32v",
        "u64v", "strv", "datav", "f32v", "f64v", "flags", "objectv", "t_u32", "t_i32", "t_s32",
        "t_u64", "t_i64", "t_s64", "index", "objects", "matrix", "vector", "arrays",
    ] {
        root.decode_field(field)
            .map_err(|err| format!("failed to decode field {field}: {err:?}"))?;
    }
    Ok(())
}

fn collect_samples<F>(rounds: usize, mut run: F) -> (Vec<u128>, usize)
where
    F: FnMut() -> (u128, usize),
{
    let mut samples = Vec::with_capacity(rounds);
    let mut total_size = 0usize;
    for _ in 0..rounds {
        let (elapsed_us, size) = run();
        total_size = size;
        samples.push(elapsed_us);
    }
    (samples, total_size)
}

fn collect_samples_result<F, E>(rounds: usize, mut run: F) -> Result<(Vec<u128>, usize), E>
where
    F: FnMut() -> Result<(u128, usize), E>,
{
    let mut samples = Vec::with_capacity(rounds);
    let mut total_size = 0usize;
    for _ in 0..rounds {
        let (elapsed_us, size) = run()?;
        total_size = size;
        samples.push(elapsed_us);
    }
    Ok((samples, total_size))
}

fn print_sample_stats(name: &str, loops: usize, samples: &[u128], total_size: usize) {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let best = sorted[0];
    let median = sorted[sorted.len() / 2];
    let best_ns = nanos_per_op(best * 1_000, loops);
    let median_ns = nanos_per_op(median * 1_000, loops);
    println!(
        "{name}: loops={loops} best={best}us ({best_ns:.1}ns/op) median={median}us ({median_ns:.1}ns/op) {:x}",
        total_size
    );
}

fn benchmark_compress(
    name: &str,
    raw: &[u8],
    loops: usize,
) -> Result<(), Box<dyn std::error::Error>> {
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
        "{name}: {bytes}B {} {:.1}ns/op {fuse:016x}",
        format_duration(elapsed),
        nanos_per_op(elapsed.as_nanos(), loops),
    );
}

fn print_throughput_result(name: &str, elapsed: Duration, loops: usize, total_size: usize) {
    println!(
        "{name}: {} {:.1}ns/op {:x}",
        format_duration(elapsed),
        nanos_per_op(elapsed.as_nanos(), loops),
        total_size
    );
}

fn print_size_result(name: &str, size: usize, elapsed: Duration, loops: usize) {
    println!(
        "{name}: {size}B {} {:.1}ns/op",
        format_duration(elapsed),
        nanos_per_op(elapsed.as_nanos(), loops),
    );
}

fn print_duration_result(name: &str, elapsed: Duration, loops: usize) {
    println!(
        "{name}: {} {:.1}ns/op",
        format_duration(elapsed),
        nanos_per_op(elapsed.as_nanos(), loops),
    );
}

fn nanos_per_op(total_ns: u128, loops: usize) -> f64 {
    total_ns as f64 / loops.max(1) as f64
}

fn format_duration(elapsed: Duration) -> String {
    let total_ns = elapsed.as_nanos();
    if total_ns < 1_000 {
        format!("{total_ns}ns")
    } else if total_ns < 1_000_000 {
        format!("{:.1}us", total_ns as f64 / 1_000.0)
    } else {
        format!("{:.3}ms", total_ns as f64 / 1_000_000.0)
    }
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

fn traverse_fb_small(root: fb_generated::test::Small<'_>, junk: &mut Junk) {
    junk.u32_sum = junk
        .u32_sum
        .wrapping_add(root.i32_() as u32)
        .wrapping_add(u32::from(root.flag()));
    if let Some(value) = root.str() {
        junk.u32_sum = junk.u32_sum.wrapping_add(junk_hash_bytes(value.as_bytes()));
    }
}

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
            junk.u32_sum = junk
                .u32_sum
                .wrapping_add(u32::from(*value));
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

fn traverse_pc_small(root: MessageView<'_>, junk: &mut Junk) -> Result<(), Box<dyn std::error::Error>> {
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

fn traverse_pc_alias_array(field: FieldView<'_>, junk: &mut Junk) -> Result<(), Box<dyn std::error::Error>> {
    let array = ArrayView::new(field.object_words().ok_or("alias array missing words")?)
        .ok_or("invalid alias array")?;
    let values = array.scalars::<f32>().ok_or("alias array is not f32")?;
    for value in values.iter() {
        junk.add_f32(value);
    }
    Ok(())
}

fn traverse_pc_vec2d(field: FieldView<'_>, junk: &mut Junk) -> Result<(), Box<dyn std::error::Error>> {
    let rows = ArrayView::new(field.object_words().ok_or("matrix missing words")?).ok_or("invalid matrix")?;
    for row in rows.iter() {
        traverse_pc_alias_array(row, junk)?;
    }
    Ok(())
}

fn traverse_pc_arr_map(field: FieldView<'_>, junk: &mut Junk) -> Result<(), Box<dyn std::error::Error>> {
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

fn traverse_pc_main(root: MessageView<'_>, junk: &mut Junk) -> Result<(), Box<dyn std::error::Error>> {
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

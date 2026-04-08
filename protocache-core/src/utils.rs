//! Utility surface matching `utils.h`.

use core::mem;

pub type Words<'a> = &'a [u32];
pub type Bytes<'a> = &'a [u8];
pub type EnumValue = i32;

#[inline(always)]
pub const fn word_size(size: usize) -> usize {
    size.div_ceil(4)
}

pub trait Scalar: Copy + Sized {
    const WIDTH: usize;
    fn from_words(words: &[u32]) -> Option<Self>;
    fn write_words(self, words: &mut [u32]) -> Option<()>;
}

macro_rules! impl_scalar {
    ($($ty:ty),* $(,)?) => {
        $(
            impl Scalar for $ty {
                const WIDTH: usize = word_size(mem::size_of::<$ty>());

                #[inline(always)]
                fn from_words(words: &[u32]) -> Option<Self> {
                    if words.len() != Self::WIDTH {
                        return None;
                    }
                    let mut bytes = [0u8; mem::size_of::<$ty>()];
                    let raw = unsafe {
                        core::slice::from_raw_parts(
                            words.as_ptr().cast::<u8>(),
                            words.len() * mem::size_of::<u32>(),
                        )
                    };
                    bytes.copy_from_slice(raw.get(..mem::size_of::<$ty>())?);
                    Some(<$ty>::from_le_bytes(bytes))
                }

                #[inline(always)]
                fn write_words(self, words: &mut [u32]) -> Option<()> {
                    if words.len() != Self::WIDTH {
                        return None;
                    }
                    words.fill(0);
                    let raw = unsafe {
                        core::slice::from_raw_parts_mut(
                            words.as_mut_ptr().cast::<u8>(),
                            words.len() * mem::size_of::<u32>(),
                        )
                    };
                    raw.get_mut(..mem::size_of::<$ty>())?
                        .copy_from_slice(&self.to_le_bytes());
                    Some(())
                }
            }
        )*
    };
}

impl_scalar!(u32, i32, u64, i64, f32, f64);

impl Scalar for bool {
    const WIDTH: usize = 1;

    #[inline(always)]
    fn from_words(words: &[u32]) -> Option<Self> {
        Some(u32::from_words(words)? != 0)
    }

    #[inline(always)]
    fn write_words(self, words: &mut [u32]) -> Option<()> {
        if words.len() != Self::WIDTH {
            return None;
        }
        words[0] = u32::from(self);
        Some(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CorruptionKind {
    Truncated,
    InvalidHeader,
    InvalidUtf8,
    IntegerOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadError {
    pub kind: CorruptionKind,
}

impl ReadError {
    pub const fn new(kind: CorruptionKind) -> Self {
        Self { kind }
    }
}

impl core::fmt::Display for ReadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}", self.kind)
    }
}

impl std::error::Error for ReadError {}

#[derive(Clone, Debug, Default)]
pub struct Buffer {
    data: Vec<u32>,
    off: usize,
}

impl Buffer {
    #[inline(always)]
    pub fn new() -> Self {
        Self::default()
    }

    #[inline(always)]
    pub fn with_capacity_words(words: usize) -> Self {
        Self {
            data: vec![0; words],
            off: words,
        }
    }

    #[inline(always)]
    pub fn clear(&mut self) {
        self.off = self.data.len();
    }

    #[inline(always)]
    pub fn allocated_words(&self) -> usize {
        self.data.len()
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.data.len().saturating_sub(self.off)
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline(always)]
    pub fn view(&self) -> &[u32] {
        &self.data[self.off..]
    }

    #[inline(always)]
    pub fn head(&self) -> &[u32] {
        self.view()
    }

    #[inline(always)]
    pub fn head_mut(&mut self) -> &mut [u32] {
        let off = self.off;
        &mut self.data[off..]
    }

    #[inline(always)]
    pub fn at_from_end(&self, words: usize) -> Option<&[u32]> {
        if words > self.data.len() {
            return None;
        }
        let start = self.data.len() - words;
        Some(&self.data[start..])
    }

    #[inline(always)]
    pub fn at_from_end_mut(&mut self, words: usize) -> Option<&mut [u32]> {
        if words > self.data.len() {
            return None;
        }
        let start = self.data.len() - words;
        Some(&mut self.data[start..])
    }

    #[inline(always)]
    pub fn reserve_words(&mut self, words: usize) {
        if words > self.data.len() {
            self.grow(words);
        }
    }

    #[inline(always)]
    pub fn expand(&mut self, delta: usize) -> &mut [u32] {
        if self.off < delta {
            let needed = self.len().saturating_add(delta);
            self.grow(needed);
        }
        self.off -= delta;
        let ptr = unsafe { self.data.as_mut_ptr().add(self.off) };
        // `off + delta` is within `data` because we either had capacity or just grew.
        unsafe { core::slice::from_raw_parts_mut(ptr, delta) }
    }

    #[inline(always)]
    pub fn shrink(&mut self, delta: usize) {
        assert!(delta <= self.len());
        self.off += delta;
    }

    #[inline(always)]
    pub fn put(&mut self, value: u32) {
        self.expand(1)[0] = value;
    }

    #[inline(always)]
    pub fn put_words(&mut self, words: &[u32]) {
        let dst = self.expand(words.len());
        // `expand` returned a non-overlapping destination slice with the exact length.
        unsafe { core::ptr::copy_nonoverlapping(words.as_ptr(), dst.as_mut_ptr(), words.len()) };
    }

    #[inline(always)]
    fn grow(&mut self, min_words: usize) {
        let active_len = self.len();
        let mut new_words = self.data.len().max(8);
        while new_words < min_words {
            new_words = new_words.saturating_mul(2);
        }
        if new_words < min_words {
            new_words = min_words;
        }
        let mut new_data = vec![0; new_words];
        let new_off = new_words - active_len;
        new_data[new_off..].copy_from_slice(self.view());
        self.data = new_data;
        self.off = new_off;
    }
}

pub fn load_file(path: impl AsRef<std::path::Path>) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

pub fn compress(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    compress_into(src, &mut out);
    out
}

pub fn compress_into(src: &[u8], out: &mut Vec<u8>) {
    if src.is_empty() {
        out.clear();
        return;
    }

    let header_len = varint_len(src.len());
    let capacity = header_len + src.len() + src.len().div_ceil(14);
    out.clear();
    if out.capacity() < capacity {
        out.reserve(capacity - out.capacity());
    }

    let mut size = src.len();
    while (size & !0x7f) != 0 {
        out.push(0x80 | (size as u8 & 0x7f));
        size >>= 7;
    }
    out.push(size as u8);

    // After the varint header, write the compressed body via a raw pointer so
    // that the hot loop does not repeatedly reload Vec metadata (data ptr +
    // length) from memory.  We call set_len once at the end.
    // SAFETY: we reserved `capacity` bytes above; all writes stay within that.
    let base = out.len();
    let mut off = 0usize;
    let dst = unsafe { out.as_mut_ptr().add(base) };

    let mut pos = 0usize;
    while pos < src.len() {
        let a_start = pos;
        let a = pick_run(src, &mut pos);
        if pos == src.len() {
            unsafe { dst.add(off).write(a); }
            off += 1;
            if (a & 0x8) == 0 {
                off = emit_run_raw(src, a_start, pos, dst, off);
            }
            break;
        }
        let b_start = pos;
        let b = pick_run(src, &mut pos);
        unsafe { dst.add(off).write(a | (b << 4)); }
        off += 1;
        if (a & 0x8) == 0 {
            off = emit_run_raw(src, a_start, b_start, dst, off);
        }
        if (b & 0x8) == 0 {
            off = emit_run_raw(src, b_start, pos, dst, off);
        }
    }

    unsafe { out.set_len(base + off); }
}

pub fn decompress(src: &[u8]) -> Result<Vec<u8>, ReadError> {
    let mut out = Vec::new();
    decompress_into(src, &mut out)?;
    Ok(out)
}

pub fn decompress_into(src: &[u8], out: &mut Vec<u8>) -> Result<(), ReadError> {
    if src.is_empty() {
        out.clear();
        return Ok(());
    }
    let (target, mut pos) = parse_varint(src)?;
    // Overallocate by 7 bytes so unpack can safely write 8 bytes at a time
    // without a per-write bounds check on the destination.  Mirrors the C++
    // technique of resize(size+7) followed by a final trim.
    out.resize(target + 7, 0u8);
    let mut out_pos = 0usize;
    let buf = out.as_mut_slice();
    while pos < src.len() {
        let mark = src[pos];
        pos += 1;
        unpack(mark & 0x0f, src, &mut pos, buf, &mut out_pos, target)?;
        unpack(mark >> 4, src, &mut pos, buf, &mut out_pos, target)?;
    }
    if out_pos != target {
        out.clear();
        return Err(ReadError::new(CorruptionKind::Truncated));
    }
    out.truncate(target);
    Ok(())
}

#[inline]
fn varint_len(mut value: usize) -> usize {
    let mut len = 1usize;
    while (value & !0x7f) != 0 {
        value >>= 7;
        len += 1;
    }
    len
}

#[inline(always)]
fn pick_run(src: &[u8], pos: &mut usize) -> u8 {
    let start = *pos;
    let first = src[start];
    *pos += 1;
    // Same trick as C++: arithmetic right-shift of i8 is true only for 0x00 and 0xff,
    // letting us handle both special bytes in a single branch instead of two.
    let fi = first as i8;
    if fi == fi >> 1 {
        while *pos < src.len() && *pos - start < 4 && src[*pos] == first {
            *pos += 1;
        }
        // first & 0x4 == 0 for 0x00, 0x4 for 0xff — encodes the fill value
        0x8 | (first & 0x4) | ((*pos - start - 1) as u8)
    } else {
        while *pos < src.len() && *pos - start < 7 && src[*pos] != 0 && src[*pos] != 0xff {
            *pos += 1;
        }
        (*pos - start) as u8
    }
}

// Raw-pointer variant used by compress_into to avoid Vec metadata reloads.
// SAFETY: caller must ensure dst has room for `end - start` bytes at offset `off`.
#[inline(always)]
fn emit_run_raw(src: &[u8], start: usize, end: usize, dst: *mut u8, off: usize) -> usize {
    let len = end - start;
    if len == 0 {
        return off;
    }
    unsafe {
        // When the source window has >= 8 bytes left, a single 8-byte word
        // copy avoids a memcpy function call (mirrors the C++ optimisation).
        if start + 8 <= src.len() {
            let src_ptr = src.as_ptr().add(start) as *const u64;
            (dst.add(off) as *mut u64).write_unaligned(src_ptr.read_unaligned());
        } else {
            std::ptr::copy_nonoverlapping(src.as_ptr().add(start), dst.add(off), len);
        }
    }
    off + len
}

fn parse_varint(src: &[u8]) -> Result<(usize, usize), ReadError> {
    let mut size = 0usize;
    let mut pos = 0usize;
    for shift in (0..32).step_by(7) {
        let byte = *src
            .get(pos)
            .ok_or(ReadError::new(CorruptionKind::Truncated))?;
        pos += 1;
        if (byte & 0x80) != 0 {
            size |= ((byte & 0x7f) as usize) << shift;
        } else {
            size |= (byte as usize) << shift;
            return Ok((size, pos));
        }
    }
    Err(ReadError::new(CorruptionKind::InvalidHeader))
}

#[inline(always)]
fn unpack(
    mark: u8,
    src: &[u8],
    pos: &mut usize,
    out: &mut [u8],
    out_pos: &mut usize,
    target: usize,
) -> Result<(), ReadError> {
    // out.len() == target + 7; use `target` for logical end.
    if *out_pos >= target {
        return Ok(());
    }
    if (mark & 0x8) != 0 {
        let count = ((mark & 0x3) + 1) as usize;
        if count > target - *out_pos {
            return Err(ReadError::new(CorruptionKind::Truncated));
        }
        // Write 4 bytes at once: out_pos < target, so out_pos + 4 <= target + 3 < target + 7.
        // SAFETY: out is target+7 bytes, so this is always in bounds.
        let fill_val: u32 = if (mark & 0x4) != 0 { u32::MAX } else { 0 };
        unsafe {
            (out.as_mut_ptr().add(*out_pos) as *mut u32).write_unaligned(fill_val);
        }
        *out_pos += count;
        return Ok(());
    }
    let len = (mark & 0x7) as usize;
    if len == 0 {
        return Ok(());
    }
    if len > src.len() - *pos || len > target - *out_pos {
        return Err(ReadError::new(CorruptionKind::Truncated));
    }
    // Fast path: when the source has >= 8 bytes left, copy 8 bytes at once.
    // Writing 8 bytes is safe: out_pos <= target - len <= target - 1,
    // so out_pos + 8 <= target + 7 == out.len().
    if src.len() - *pos >= 8 {
        unsafe {
            let src_ptr = src.as_ptr().add(*pos) as *const u64;
            let dst_ptr = out.as_mut_ptr().add(*out_pos) as *mut u64;
            dst_ptr.write_unaligned(src_ptr.read_unaligned());
        }
        *pos += len;
        *out_pos += len;
    } else {
        let end = *pos + len;
        let out_end = *out_pos + len;
        out[*out_pos..out_end].copy_from_slice(&src[*pos..end]);
        *pos = end;
        *out_pos = out_end;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Buffer, compress, compress_into, decompress, decompress_into};

    #[test]
    fn put_prepends_words() {
        let mut buffer = Buffer::new();
        buffer.put(3);
        buffer.put(2);
        buffer.put(1);
        assert_eq!(buffer.view(), &[1, 2, 3]);
    }

    #[test]
    fn expand_preserves_existing_payload() {
        let mut buffer = Buffer::with_capacity_words(4);
        buffer.put_words(&[3, 4]);
        buffer.put_words(&[1, 2]);
        assert_eq!(buffer.view(), &[1, 2, 3, 4]);

        buffer.put_words(&[0]);
        assert_eq!(buffer.view(), &[0, 1, 2, 3, 4]);
        assert!(buffer.allocated_words() >= 5);
    }

    #[test]
    fn shrink_discards_from_front() {
        let mut buffer = Buffer::new();
        buffer.put_words(&[1, 2, 3, 4]);
        buffer.shrink(2);
        assert_eq!(buffer.view(), &[3, 4]);
        buffer.clear();
        assert!(buffer.is_empty());
    }

    #[test]
    fn compression_roundtrip_preserves_payload() {
        let src = b"\0\0\0\0abcd\xff\xff\xffefgh\0";
        let compressed = compress(src);
        let restored = decompress(&compressed).unwrap();
        assert_eq!(restored, src);
    }

    #[test]
    fn compression_into_reuses_buffers() {
        let src = b"\0\0\0\0abcd\xff\xff\xffefgh\0";
        let mut compressed = Vec::with_capacity(128);
        let compressed_capacity = compressed.capacity();
        compress_into(src, &mut compressed);
        assert_eq!(compressed.capacity(), compressed_capacity);

        let mut restored = Vec::with_capacity(128);
        let restored_capacity = restored.capacity();
        decompress_into(&compressed, &mut restored).unwrap();
        assert_eq!(restored, src);
        assert_eq!(restored.capacity(), restored_capacity);
    }
}

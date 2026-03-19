//! Perfect-hash surface matching `perfect_hash.h`.

pub use crate::serialize::{
    build_perfect_hash_index,
    build_perfect_hash_index_with_positions,
};

use crate::utils::{CorruptionKind, ReadError};
use crate::hash::hash128;

#[derive(Clone, Copy, Debug)]
pub struct PerfectHashView<'a> {
    data: &'a [u8],
    bitmap: &'a [u8],
    table: &'a [u8],
    section: usize,
    len: usize,
    table_width: usize,
}

impl<'a> PerfectHashView<'a> {
    #[inline(always)]
    pub fn new(data: &'a [u8]) -> Result<Self, ReadError> {
        let header = read_u32_le(data).ok_or(ReadError::new(CorruptionKind::Truncated))?;
        let len = (header & 0x0fff_ffff) as usize;
        if len <= 1 {
            let data = data.get(..4).ok_or(ReadError::new(CorruptionKind::Truncated))?;
            return Ok(Self {
                data,
                bitmap: &data[..0],
                table: &data[..0],
                section: 0,
                len,
                table_width: 0,
            });
        }

        let (section, bitmap_size, table_width, bytes) = perfect_hash_layout(len);
        let data = data
            .get(..bytes)
            .ok_or(ReadError::new(CorruptionKind::Truncated))?;
        let bitmap = data
            .get(8..8 + bitmap_size)
            .ok_or(ReadError::new(CorruptionKind::Truncated))?;
        let table = data
            .get(8 + bitmap_size..)
            .ok_or(ReadError::new(CorruptionKind::Truncated))?;

        Ok(Self {
            data,
            bitmap,
            table,
            section,
            len,
            table_width,
        })
    }

    #[inline(always)]
    pub fn data_size(&self) -> usize {
        self.data.len()
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline(always)]
    pub fn locate(&self, key: &[u8]) -> Option<usize> {
        if self.len == 0 {
            return None;
        }
        if self.len == 1 {
            return Some(0);
        }

        let seed = read_u32_le(self.data.get(4..8)?)? as u64;
        let code = hash128(key, seed);
        let slots = [
            code[0] as usize % self.section,
            code[1] as usize % self.section + self.section,
            code[2] as usize % self.section + self.section * 2,
        ];
        self.locate_slots(slots)
    }

    #[inline(always)]
    fn locate_slots(&self, slots: [usize; 3]) -> Option<usize> {
        let m = bit2(self.bitmap, slots[0])? + bit2(self.bitmap, slots[1])? + bit2(self.bitmap, slots[2])?;
        let slot = slots[(m % 3) as usize];
        let a = slot >> 5;
        let b = slot & 31;

        let off = read_offset(self.table, self.table_width, a)?;

        let block = read_u64_le(self.bitmap.get(a * 8..a * 8 + 8)?)? | (u64::MAX << (b << 1));
        Some(off + count_valid_slot(block))
    }
}

#[inline(always)]
fn perfect_hash_layout(len: usize) -> (usize, usize, usize, usize) {
    let section = ((len * 105).saturating_add(255) / 256).max(10);
    let bitmap_size = ((section * 3 + 31) & !31) / 4;
    let table_width = if len > u16::MAX as usize {
        4
    } else if len > u8::MAX as usize {
        2
    } else if len > 24 {
        1
    } else {
        0
    };
    let bytes = 8 + bitmap_size + bitmap_size / 8 * table_width;
    (section, bitmap_size, table_width, bytes)
}

#[inline(always)]
fn read_offset(table: &[u8], table_width: usize, index: usize) -> Option<usize> {
    match table_width {
        4 => {
            let start = index * 4;
            Some(read_u32_le(table.get(start..start + 4)?)? as usize)
        }
        2 => {
            let start = index * 2;
            Some(u16::from_le_bytes(table.get(start..start + 2)?.try_into().ok()?) as usize)
        }
        1 => Some(*table.get(index)? as usize),
        0 => Some(0),
        _ => None,
    }
}

#[inline(always)]
fn read_u32_le(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?))
}

#[inline(always)]
fn read_u64_le(bytes: &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(..8)?.try_into().ok()?))
}

#[inline(always)]
fn bit2(vec: &[u8], pos: usize) -> Option<u32> {
    Some(((vec.get(pos >> 2)? >> ((pos & 3) << 1)) & 3) as u32)
}

#[inline(always)]
fn count_valid_slot(mut v: u64) -> usize {
    v &= v >> 1;
    v = (v & 0x1111_1111_1111_1111) + ((v >> 2) & 0x1111_1111_1111_1111);
    v += v >> 4;
    v += v >> 8;
    v = (v & 0x0f0f_0f0f_0f0f_0f0f) + ((v >> 16) & 0x0f0f_0f0f_0f0f_0f0f);
    v += v >> 32;
    32 - ((v & 0xff) as usize)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{PerfectHashView, build_perfect_hash_index};

    fn run_case(size: usize) {
        let keys = (0..size).map(|i| i.to_string()).collect::<Vec<_>>();
        let index = build_perfect_hash_index(&keys).expect("perfect hash index should build");
        let view = PerfectHashView::new(&index).expect("perfect hash view should decode");

        assert_eq!(view.len(), keys.len());
        assert_eq!(view.data_size(), index.len());

        let mut seen = HashSet::new();
        for key in &keys {
            let pos = view
                .locate(key.as_bytes())
                .expect("every inserted key should resolve");
            assert!(pos < keys.len());
            assert!(seen.insert(pos), "duplicate slot {pos} for key {key}");
        }
    }

    #[test]
    fn tiny_cases_match_cpp_coverage() {
        for size in [0usize, 1, 2, 24] {
            run_case(size);
        }
    }

    #[test]
    fn small_cases_match_cpp_coverage() {
        for size in [200usize, 255, 1000] {
            run_case(size);
        }
    }

    #[test]
    fn big_cases_match_cpp_coverage() {
        for size in [65_535usize, 100_000] {
            run_case(size);
        }
    }

    #[test]
    fn rejects_truncated_indices() {
        assert!(PerfectHashView::new(&[]).is_err());
        assert!(PerfectHashView::new(&[0, 0, 0]).is_err());

        let index = build_perfect_hash_index(&["a", "b", "c"]).unwrap();
        for len in 0..index.len() {
            assert!(PerfectHashView::new(&index[..len]).is_err());
        }
    }
}

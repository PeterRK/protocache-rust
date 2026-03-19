use crate::error::{CorruptionKind, ReadError};
use crate::hash::hash128;

#[derive(Clone, Copy, Debug)]
pub struct PerfectHashView<'a> {
    data: &'a [u8],
    section: usize,
    len: usize,
}

impl<'a> PerfectHashView<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, ReadError> {
        let header = read_u32_le(data).ok_or(ReadError::new(CorruptionKind::Truncated))?;
        let len = (header & 0x0fff_ffff) as usize;
        if len <= 1 {
            return Ok(Self {
                data: data.get(..4).ok_or(ReadError::new(CorruptionKind::Truncated))?,
                section: 0,
                len,
            });
        }

        let section = ((len * 105).saturating_add(255) / 256).max(10);
        let mut bytes = (((section * 3 + 31) & !31) / 4) + 8;
        if len > u16::MAX as usize {
            bytes += bytes / 2;
        } else if len > u8::MAX as usize {
            bytes += bytes / 4;
        } else if len > 24 {
            bytes += bytes / 8;
        }

        Ok(Self {
            data: data.get(..bytes).ok_or(ReadError::new(CorruptionKind::Truncated))?,
            section,
            len,
        })
    }

    pub fn data_size(&self) -> usize {
        self.data.len()
    }

    pub fn len(&self) -> usize {
        self.len
    }

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

    fn locate_slots(&self, slots: [usize; 3]) -> Option<usize> {
        let bitmap_size = ((self.section * 3 + 31) & !31) / 4;
        let bitmap = self.data.get(8..8 + bitmap_size)?;
        let table = self.data.get(8 + bitmap_size..)?;

        let m = bit2(bitmap, slots[0])? + bit2(bitmap, slots[1])? + bit2(bitmap, slots[2])?;
        let slot = slots[(m % 3) as usize];
        let a = slot >> 5;
        let b = slot & 31;

        let off = if self.len > u16::MAX as usize {
            let start = a * 4;
            read_u32_le(table.get(start..start + 4)?)? as usize
        } else if self.len > u8::MAX as usize {
            let start = a * 2;
            u16::from_le_bytes(table.get(start..start + 2)?.try_into().ok()?) as usize
        } else if self.len > 24 {
            *table.get(a)? as usize
        } else {
            0
        };

        let block = read_u64_le(bitmap.get(a * 8..a * 8 + 8)?)? | (u64::MAX << (b << 1));
        Some(off + count_valid_slot(block))
    }
}

fn read_u32_le(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?))
}

fn read_u64_le(bytes: &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(..8)?.try_into().ok()?))
}

fn bit2(vec: &[u8], pos: usize) -> Option<u32> {
    Some(((vec.get(pos >> 2)? >> ((pos & 3) << 1)) & 3) as u32)
}

fn count_valid_slot(mut v: u64) -> usize {
    v &= v >> 1;
    v = (v & 0x1111_1111_1111_1111) + ((v >> 2) & 0x1111_1111_1111_1111);
    v += v >> 4;
    v += v >> 8;
    v = (v & 0x0f0f_0f0f_0f0f_0f0f) + ((v >> 16) & 0x0f0f_0f0f_0f0f_0f0f);
    v += v >> 32;
    32 - ((v & 0xff) as usize)
}

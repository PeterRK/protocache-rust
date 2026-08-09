//! Serialization surface matching `serialize.h`.

use std::time::{SystemTime, UNIX_EPOCH};

pub use crate::Buffer;

use crate::Scalar;
use crate::hash::hash128;

#[derive(Clone, Copy, Debug, Default)]
/// A contiguous range stored in a reverse-growing [`Buffer`].
pub struct Segment {
    /// Position measured from the end of the buffer.
    pub pos: usize,
    /// Segment length in `u32` words.
    pub len: usize,
}

impl Segment {
    #[inline(always)]
    pub fn end(self) -> usize {
        self.pos - self.len
    }
}

#[derive(Clone, Copy, Debug, Default)]
/// Intermediate representation of an encoded field.
///
/// Values up to three words can remain inline; larger values refer to a
/// [`Segment`] already written into a [`Buffer`].
pub struct Unit {
    inline_len: usize,
    inline_data: [u32; 3],
    segment: Segment,
}

impl Unit {
    #[inline(always)]
    pub fn empty() -> Self {
        Self::default()
    }

    #[inline(always)]
    pub fn inline(words: &[u32]) -> Self {
        assert!(words.len() <= 3);
        let mut data = [0u32; 3];
        data[..words.len()].copy_from_slice(words);
        Self {
            inline_len: words.len(),
            inline_data: data,
            segment: Segment::default(),
        }
    }

    #[inline(always)]
    pub fn segment(last: usize, now: usize) -> Self {
        Self {
            inline_len: 0,
            inline_data: [0; 3],
            segment: Segment {
                pos: now,
                len: now - last,
            },
        }
    }

    #[inline(always)]
    pub fn size(&self) -> usize {
        if self.inline_len != 0 {
            self.inline_len
        } else {
            self.segment.len
        }
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.size() == 0
    }

    #[inline(always)]
    pub fn is_segment(&self) -> bool {
        self.inline_len == 0 && self.segment.len != 0
    }

    #[inline(always)]
    pub fn inline_words(&self) -> &[u32] {
        &self.inline_data[..self.inline_len]
    }

    #[inline(always)]
    pub fn segment_info(&self) -> Segment {
        self.segment
    }
}

#[inline(always)]
fn write_varint(buf: &mut [u8; 5], mut n: u32) -> usize {
    let mut written = 0usize;
    while (n & !0x7f) != 0 {
        buf[written] = 0x80 | (n as u8 & 0x7f);
        written += 1;
        n >>= 7;
    }
    buf[written] = n as u8;
    written + 1
}

#[inline(always)]
fn offset(off: usize) -> u32 {
    ((off as u32) << 2) | 3
}

#[inline(always)]
fn copy_inline(body: &mut [u32], body_index: &mut usize, pos: &mut usize, field: &Unit) {
    let len = field.inline_len;
    body[*body_index..*body_index + len].copy_from_slice(field.inline_words());
    *body_index += len;
    *pos -= len;
}

#[inline(always)]
fn unit_words<'a>(unit: &'a Unit, buffer: &'a Buffer) -> Option<&'a [u32]> {
    if unit.inline_len != 0 {
        return Some(unit.inline_words());
    }
    if unit.segment.len == 0 {
        return Some(&[]);
    }
    let start = buffer.len().checked_sub(unit.segment.pos)?;
    buffer.view().get(start..start + unit.segment.len)
}

#[inline(always)]
fn pick_unit(unit: &mut Unit, buffer: &mut Buffer, tail: &mut usize, width: usize) {
    if unit.inline_len != 0 {
        return;
    }

    let seg = unit.segment;
    if seg.len <= width {
        let start = buffer.len() - seg.pos;
        let words = &buffer.head()[start..start + seg.len];
        unit.inline_len = seg.len;
        unit.inline_data[..seg.len].copy_from_slice(words);
        return;
    }

    let start = buffer.len() - seg.pos;
    let end = start + seg.len;
    if *tail > end {
        let new_start = *tail - seg.len;
        buffer.head_mut().copy_within(start..end, new_start);
        unit.segment.pos -= new_start - start;
        *tail = new_start;
    } else {
        debug_assert!(*tail >= seg.len);
        *tail -= seg.len;
    }
}

#[inline(always)]
fn mark_unit(unit: &Unit, buffer: &mut Buffer, width: usize) {
    let expanded_len = buffer.len() + width;
    let cell = buffer.expand(width);
    if unit.inline_len != 0 {
        cell[..unit.inline_len].copy_from_slice(unit.inline_words());
        for word in &mut cell[unit.inline_len..] {
            *word = 0;
        }
    } else {
        cell[0] = offset(expanded_len - unit.segment.pos);
        for word in &mut cell[1..] {
            *word = 0;
        }
    }
}

#[inline(always)]
fn best_array_size(elements: &[Unit]) -> (usize, usize) {
    let mut sizes = [0usize; 3];
    for element in elements {
        sizes[0] += 1;
        sizes[1] += 2;
        sizes[2] += 3;
        let len = element.size();
        if len <= 1 {
            continue;
        }
        sizes[0] += len;
        if len <= 2 {
            continue;
        }
        sizes[1] += len;
        if len <= 3 {
            continue;
        }
        sizes[2] += len;
    }

    let mut mode = 0usize;
    for idx in 1..3 {
        if sizes[idx] < sizes[mode] {
            mode = idx;
        }
    }
    (sizes[mode], mode + 1)
}

#[inline(always)]
fn best_array_size_pairs(elements: &[(Unit, Unit)]) -> ((usize, usize), (usize, usize)) {
    let mut key_sizes = [0usize; 3];
    let mut value_sizes = [0usize; 3];
    for (key, value) in elements {
        for sizes in [&mut key_sizes, &mut value_sizes] {
            sizes[0] += 1;
            sizes[1] += 2;
            sizes[2] += 3;
        }

        let key_len = key.size();
        if key_len > 1 {
            key_sizes[0] += key_len;
            if key_len > 2 {
                key_sizes[1] += key_len;
                if key_len > 3 {
                    key_sizes[2] += key_len;
                }
            }
        }

        let value_len = value.size();
        if value_len > 1 {
            value_sizes[0] += value_len;
            if value_len > 2 {
                value_sizes[1] += value_len;
                if value_len > 3 {
                    value_sizes[2] += value_len;
                }
            }
        }
    }

    let mut key_mode = 0usize;
    let mut value_mode = 0usize;
    for idx in 1..3 {
        if key_sizes[idx] < key_sizes[key_mode] {
            key_mode = idx;
        }
        if value_sizes[idx] < value_sizes[value_mode] {
            value_mode = idx;
        }
    }

    (
        (key_sizes[key_mode], key_mode + 1),
        (value_sizes[value_mode], value_mode + 1),
    )
}

#[inline(always)]
fn perfect_hash_section(size: usize) -> usize {
    ((size * 105).saturating_add(255) / 256).max(10)
}

#[inline(always)]
fn perfect_hash_bitmap_size(section: usize) -> usize {
    ((section * 3 + 31) & !31) / 4
}

#[inline(always)]
fn set_bit2(vec: &mut [u8], pos: usize, val: u8) {
    let shift = ((pos & 3) << 1) as u8;
    let idx = pos >> 2;
    vec[idx] &= !(3u8 << shift);
    vec[idx] |= (val & 3) << shift;
}

#[inline(always)]
fn get_bit2(vec: &[u8], pos: usize) -> u8 {
    (vec[pos >> 2] >> ((pos & 3) << 1)) & 3
}

#[derive(Clone, Copy)]
struct Edge {
    slots: [usize; 3],
}

#[inline(always)]
fn next_seed(state: &mut [u32; 4]) -> u32 {
    let t = state[0] ^ (state[0] << 11);
    state[0] = state[1];
    state[1] = state[2];
    state[2] = state[3];
    state[3] ^= (state[3] >> 19) ^ t ^ (t >> 8);
    state[3]
}

#[inline(always)]
fn peel_graph(edges: &[Edge], slot_cnt: usize) -> Option<Vec<usize>> {
    const NONE: usize = usize::MAX;

    #[derive(Clone, Copy)]
    struct Vertex {
        slot: usize,
        prev: usize,
        next: usize,
    }

    let mut heads = vec![NONE; slot_cnt];
    let mut vertices = vec![
        Vertex {
            slot: NONE,
            prev: NONE,
            next: NONE,
        };
        edges.len() * 3
    ];

    for (edge_idx, edge) in edges.iter().enumerate() {
        for (slot_index, &slot) in edge.slots.iter().enumerate() {
            let node = edge_idx * 3 + slot_index;
            let head = heads[slot];
            vertices[node] = Vertex {
                slot,
                prev: NONE,
                next: head,
            };
            if head != NONE {
                vertices[head].prev = node;
            }
            heads[slot] = node;
        }
    }

    let mut queue = Vec::with_capacity(slot_cnt);
    for (slot, &head) in heads.iter().enumerate() {
        if head != NONE && vertices[head].next == NONE {
            queue.push(slot);
        }
    }
    let mut queue_head = 0usize;

    let mut order = Vec::with_capacity(edges.len());

    while let Some(&slot) = queue.get(queue_head) {
        queue_head += 1;
        let head = heads[slot];
        if head == NONE || vertices[head].next != NONE {
            continue;
        }
        let edge_idx = head / 3;
        order.push(edge_idx);

        for offset in 0..3 {
            let node = edge_idx * 3 + offset;
            let vertex = vertices[node];
            if vertex.slot == NONE {
                continue;
            }

            if vertex.prev != NONE {
                vertices[vertex.prev].next = vertex.next;
            } else {
                heads[vertex.slot] = vertex.next;
            }
            if vertex.next != NONE {
                vertices[vertex.next].prev = vertex.prev;
            }

            let head = heads[vertex.slot];
            if head != NONE && vertices[head].next == NONE {
                queue.push(vertex.slot);
            }
            vertices[node].slot = NONE;
        }
    }

    if order.len() == edges.len() {
        Some(order)
    } else {
        None
    }
}

#[inline(always)]
fn count_valid_slots(bitmap: &[u8], block: usize) -> usize {
    let start = block * 8;
    let bits = u64::from_le_bytes(
        bitmap[start..start + 8]
            .try_into()
            .expect("bitmap block must fit"),
    );
    count_valid_slots_in_word(bits)
}

#[inline(always)]
fn locate_in_perfect_hash(index: &[u8], key: &[u8]) -> Option<usize> {
    let size = u32::from_le_bytes(index.get(..4)?.try_into().ok()?) as usize & 0x0fff_ffff;
    if size < 2 {
        return Some(0);
    }
    let section = perfect_hash_section(size);
    let section_u32 = section as u32;
    let section_magic = fast_mod_magic(section_u32);
    let bitmap_size = perfect_hash_bitmap_size(section);
    let bitmap = index.get(8..8 + bitmap_size)?;
    let table = index.get(8 + bitmap_size..)?;
    let seed = u32::from_le_bytes(index.get(4..8)?.try_into().ok()?) as u64;
    let code = hash128(key, seed);
    let slots = [
        fast_mod_u32(code[0], section_u32, section_magic) as usize,
        fast_mod_u32(code[1], section_u32, section_magic) as usize + section,
        fast_mod_u32(code[2], section_u32, section_magic) as usize + section * 2,
    ];
    let m = get_bit2(bitmap, slots[0]) as usize
        + get_bit2(bitmap, slots[1]) as usize
        + get_bit2(bitmap, slots[2]) as usize;
    let slot = slots[m % 3];
    let block = slot >> 5;
    let bit = slot & 31;

    let off = if size > u16::MAX as usize {
        u32::from_le_bytes(table.get(block * 4..block * 4 + 4)?.try_into().ok()?) as usize
    } else if size > u8::MAX as usize {
        u16::from_le_bytes(table.get(block * 2..block * 2 + 2)?.try_into().ok()?) as usize
    } else if size > 24 {
        *table.get(block)? as usize
    } else {
        0
    };

    let word_start = block * 8;
    let bits = u64::from_le_bytes(bitmap.get(word_start..word_start + 8)?.try_into().ok()?);
    let masked = bits | (u64::MAX << (bit << 1));
    Some(off + count_valid_slots_in_word(masked))
}

#[inline(always)]
fn count_valid_slots_in_word(bits: u64) -> usize {
    let invalid = ((bits & 0x5555_5555_5555_5555) & (bits >> 1)).count_ones() as usize;
    32 - invalid
}

#[inline(always)]
fn fast_mod_magic(divisor: u32) -> u64 {
    u64::MAX / divisor as u64 + 1
}

#[inline(always)]
fn fast_mod_u32(value: u32, divisor: u32, magic: u64) -> u32 {
    let low = magic.wrapping_mul(value as u64);
    (((low as u128) * divisor as u128) >> 64) as u32
}

#[inline(always)]
/// Builds a ProtoCache perfect-hash index for unique encoded keys.
///
/// Returns `None` if no valid index can be constructed, including duplicate
/// key layouts that cannot be represented.
pub fn build_perfect_hash_index<K: AsRef<[u8]>>(keys: &[K]) -> Option<Vec<u8>> {
    Some(build_perfect_hash_index_with_positions(keys)?.0)
}

#[inline(always)]
/// Builds a perfect-hash index and reports each input key's storage position.
pub fn build_perfect_hash_index_with_positions<K: AsRef<[u8]>>(
    keys: &[K],
) -> Option<(Vec<u8>, Vec<usize>)> {
    let total = keys.len();
    if total >= (1usize << 28) {
        return None;
    }
    if total <= 1 {
        return Some(((total as u32).to_le_bytes().to_vec(), vec![0; total]));
    }

    let section = perfect_hash_section(total);
    let section_u32 = section as u32;
    let section_magic = fast_mod_magic(section_u32);
    let slot_cnt = section * 3;
    let bitmap_size = perfect_hash_bitmap_size(section);

    let clock_seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|error| error.duration())
        .as_nanos() as u32;
    let mut rand32 = [0x6c07_8965, 0x9908_b0df, 0x9d2c_5680, clock_seed];
    let tries = if total <= u8::MAX as usize { 40 } else { 16 };
    let mut result = None;
    for _ in 0..tries {
        let seed = next_seed(&mut rand32);
        let edges: Vec<_> = keys
            .iter()
            .map(|key| {
                let code = hash128(key.as_ref(), seed as u64);
                Edge {
                    slots: [
                        fast_mod_u32(code[0], section_u32, section_magic) as usize,
                        fast_mod_u32(code[1], section_u32, section_magic) as usize + section,
                        fast_mod_u32(code[2], section_u32, section_magic) as usize + section * 2,
                    ],
                }
            })
            .collect();
        if let Some(order) = peel_graph(&edges, slot_cnt) {
            result = Some((seed, edges, order));
            break;
        }
    }
    let (seed, edges, order) = result?;

    let mut bitmap = vec![0xffu8; bitmap_size];
    let mut taken = vec![false; slot_cnt];
    for &edge_idx in order.iter().rev() {
        let [a, b, c] = edges[edge_idx].slots;
        let (chosen, target) = if !taken[a] {
            taken[a] = true;
            taken[b] = true;
            taken[c] = true;
            (a, 0u8)
        } else if !taken[b] {
            taken[b] = true;
            taken[c] = true;
            (b, 1u8)
        } else {
            taken[c] = true;
            (c, 2u8)
        };

        let sum = match chosen {
            x if x == a => get_bit2(&bitmap, b) as u16 + get_bit2(&bitmap, c) as u16,
            x if x == b => get_bit2(&bitmap, a) as u16 + get_bit2(&bitmap, c) as u16,
            _ => get_bit2(&bitmap, a) as u16 + get_bit2(&bitmap, b) as u16,
        };
        let chosen_val = ((target as i16 - (sum % 3) as i16 + 3) % 3) as u8;
        set_bit2(&mut bitmap, chosen, chosen_val);
    }

    let mut out = Vec::with_capacity(8 + bitmap_size + bitmap_size / 2);
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&seed.to_le_bytes());
    out.extend_from_slice(&bitmap);

    if total > u16::MAX as usize {
        let mut cnt = 0u32;
        for block in 0..(bitmap_size / 8) {
            out.extend_from_slice(&cnt.to_le_bytes());
            cnt += count_valid_slots(&bitmap, block) as u32;
        }
    } else if total > u8::MAX as usize {
        let mut cnt = 0u16;
        for block in 0..(bitmap_size / 8) {
            out.extend_from_slice(&cnt.to_le_bytes());
            cnt += count_valid_slots(&bitmap, block) as u16;
        }
    } else if total > 24 {
        let mut cnt = 0u8;
        for block in 0..(bitmap_size / 8) {
            out.push(cnt);
            cnt += count_valid_slots(&bitmap, block) as u8;
        }
    }

    let mut positions = Vec::with_capacity(keys.len());
    for key in keys.iter() {
        positions.push(locate_in_perfect_hash(&out, key.as_ref())?);
    }

    Some((out, positions))
}

#[inline(always)]
/// Moves a small buffered segment into the inline storage of `unit` when possible.
pub fn fold_field(buffer: &mut Buffer, unit: &mut Unit) {
    if !unit.is_segment() || unit.segment.len >= 4 || unit.segment.pos != buffer.len() {
        return;
    }
    let seg = unit.segment;
    let head = buffer.view();
    unit.inline_len = seg.len;
    unit.inline_data[..seg.len].copy_from_slice(&head[..seg.len]);
    unit.segment = Segment::default();
    buffer.shrink(seg.len);
}

#[inline(always)]
/// Encodes a scalar as an inline field unit.
pub fn serialize_scalar<T: Scalar>(value: T) -> Unit {
    let mut words = [0u32; 3];
    let word_len = T::WIDTH;
    value
        .write_words(&mut words[..word_len])
        .expect("scalar width must match");
    Unit {
        inline_len: word_len,
        inline_data: words,
        segment: Segment::default(),
    }
}

#[inline(always)]
/// Encodes a boolean as an inline field unit.
pub fn serialize_bool(value: bool) -> Unit {
    Unit::inline(&[u32::from(value)])
}

#[inline(always)]
/// Encodes a byte string into `buffer`.
///
/// Short payloads may be returned inline. `None` indicates an encoded-size or
/// offset overflow.
pub fn serialize_bytes(bytes: &[u8], buffer: &mut Buffer) -> Option<Unit> {
    if bytes.len() >= (1usize << 30) {
        return None;
    }
    let mark = (bytes.len() as u32) << 2;
    let mut header = [0u8; 5];
    let header_size = write_varint(&mut header, mark);
    let total_words = (header_size + bytes.len()).div_ceil(4);

    let last = buffer.len();
    let mut inline = [0u32; 3];
    if total_words == 1 {
        let raw = unsafe { core::slice::from_raw_parts_mut(inline.as_mut_ptr().cast::<u8>(), 4) };
        raw.fill(0);
        raw[..header_size].copy_from_slice(&header[..header_size]);
        raw[header_size..header_size + bytes.len()].copy_from_slice(bytes);
        Some(Unit {
            inline_len: 1,
            inline_data: inline,
            segment: Segment::default(),
        })
    } else {
        let words = buffer.expand(total_words);
        words[total_words - 1] = 0;
        let raw = unsafe {
            core::slice::from_raw_parts_mut(words.as_mut_ptr().cast::<u8>(), total_words * 4)
        };
        raw[..header_size].copy_from_slice(&header[..header_size]);
        raw[header_size..header_size + bytes.len()].copy_from_slice(bytes);
        Some(Unit::segment(last, buffer.len()))
    }
}

#[inline(always)]
/// Encodes a UTF-8 string using the ProtoCache byte-string representation.
pub fn serialize_str(value: &str, buffer: &mut Buffer) -> Option<Unit> {
    serialize_bytes(value.as_bytes(), buffer)
}

#[inline(always)]
/// Encodes message fields, restricting referenced segments to data written
/// since the caller-provided `last` buffer length.
pub fn serialize_message_at(fields: &mut [Unit], buffer: &mut Buffer, last: usize) -> Option<Unit> {
    if fields.is_empty() {
        return None;
    }
    let used_len = fields
        .iter()
        .rposition(|unit| !unit.is_empty())
        .map(|index| index + 1)
        .unwrap_or(0);
    if used_len == 0 {
        let last = buffer.len();
        buffer.put(0);
        return Some(Unit::segment(last, buffer.len()));
    }
    let fields = &mut fields[..used_len];

    let mut body_size = 0usize;
    let mut size = 0usize;
    for field in fields.iter().rev() {
        if field.inline_len != 0 {
            body_size += field.inline_len;
            size += field.inline_len;
        } else if field.segment.len != 0 {
            body_size += 1;
            size += field.segment.len;
        }
    }
    if size >= (1usize << 30) {
        return None;
    }

    let section = (fields.len() + 12) / 25;
    if section > 0xff {
        return None;
    }

    let head_size = 1 + section * 2;
    let current_size = buffer.len();
    let total_size = current_size + head_size + body_size;
    let block = buffer.expand(head_size + body_size);
    let (head, body) = block.split_at_mut(head_size);
    let mut body_index = 0usize;
    let mut pos = total_size - head_size;
    head.fill(0);
    head[0] = section as u32;

    let mut cnt = 0u32;
    for (i, field) in fields.iter().take(12).enumerate() {
        if field.inline_len != 0 {
            head[0] |= (field.inline_len as u32) << (8 + i * 2);
            cnt += field.inline_len as u32;
            copy_inline(body, &mut body_index, &mut pos, field);
        } else if field.segment.len != 0 {
            head[0] |= 1u32 << (8 + i * 2);
            cnt += 1;
            body[body_index] = offset(pos - field.segment.pos);
            body_index += 1;
            pos -= 1;
        }
    }

    for sec in 0..section {
        let start = 12 + sec * 25;
        let end = fields.len().min(start + 25);
        let mut mark = (cnt as u64) << 50;
        let mut shift = 0usize;
        for field in &fields[start..end] {
            if field.inline_len != 0 {
                mark |= (field.inline_len as u64) << shift;
                cnt += field.inline_len as u32;
                copy_inline(body, &mut body_index, &mut pos, field);
            } else if field.segment.len != 0 {
                mark |= 1u64 << shift;
                cnt += 1;
                body[body_index] = offset(pos - field.segment.pos);
                body_index += 1;
                pos -= 1;
            }
            shift += 2;
        }
        head[1 + sec * 2] = mark as u32;
        head[1 + sec * 2 + 1] = (mark >> 32) as u32;
    }

    Some(Unit::segment(last, buffer.len()))
}

#[inline(always)]
/// Encodes a message from zero-based field units.
pub fn serialize_message(fields: &mut [Unit], buffer: &mut Buffer) -> Option<Unit> {
    serialize_message_at(fields, buffer, buffer.len())
}

#[inline(always)]
/// Encodes array elements written since `last`, updating their segment metadata.
pub fn serialize_array_at_mut(
    elements: &mut [Unit],
    buffer: &mut Buffer,
    last: usize,
) -> Option<Unit> {
    if elements.is_empty() {
        return Some(Unit::inline(&[1]));
    }

    let (size, width) = best_array_size(elements);
    if size >= (1usize << 30) {
        return None;
    }

    let mut tail = buffer.len().checked_sub(last)?;
    for unit in elements.iter_mut().rev() {
        pick_unit(unit, buffer, &mut tail, width);
    }
    buffer.shrink(tail);
    for unit in elements.iter().rev() {
        mark_unit(unit, buffer, width);
    }
    buffer.put(((elements.len() as u32) << 2) | width as u32);
    Some(Unit::segment(last, buffer.len()))
}

#[inline(always)]
/// Encodes array elements written since the caller-provided `last` length.
pub fn serialize_array_at(elements: &[Unit], buffer: &mut Buffer, last: usize) -> Option<Unit> {
    if elements.is_empty() {
        return Some(Unit::inline(&[1]));
    }

    let (size, width) = best_array_size(elements);
    if size >= (1usize << 30) {
        return None;
    }

    let mut payloads = Vec::with_capacity(size.saturating_sub(elements.len() * width));
    let mut cells = vec![0u32; elements.len() * width];
    let cells_len = cells.len();
    for (index, unit) in elements.iter().enumerate() {
        let words = unit_words(unit, buffer)?;
        let cell = &mut cells[index * width..(index + 1) * width];
        if words.len() <= width {
            cell[..words.len()].copy_from_slice(words);
        } else {
            let payload_start = cells_len.checked_add(payloads.len())?;
            let cell_start = index.checked_mul(width)?;
            cell[0] = offset(payload_start.checked_sub(cell_start)?);
            payloads.extend_from_slice(words);
        }
    }
    buffer.shrink(buffer.len().checked_sub(last)?);
    buffer.put_words(&payloads);
    buffer.put_words(&cells);
    buffer.put(((elements.len() as u32) << 2) | width as u32);
    Some(Unit::segment(last, buffer.len()))
}

#[inline(always)]
/// Encodes an array from element units.
pub fn serialize_array(elements: &[Unit], buffer: &mut Buffer) -> Option<Unit> {
    serialize_array_at(elements, buffer, buffer.len())
}

#[inline(always)]
/// Encodes map key/value units and their perfect-hash index from mutable inputs.
pub fn serialize_map_at_mut(
    index: &[u8],
    keys: &mut [Unit],
    values: &mut [Unit],
    buffer: &mut Buffer,
    last: usize,
) -> Option<Unit> {
    if keys.len() != values.len() {
        return None;
    }
    if keys.is_empty() {
        return Some(Unit::inline(&[5u32 << 28]));
    }

    let (key_size, key_width) = best_array_size(keys);
    let (value_size, value_width) = best_array_size(values);
    let index_words = index.len().div_ceil(4);
    let size = index_words + key_size + value_size;
    if size >= (1usize << 30) {
        return None;
    }

    let mut tail = buffer.len().checked_sub(last)?;
    for index in (0..keys.len()).rev() {
        pick_unit(&mut values[index], buffer, &mut tail, value_width);
        pick_unit(&mut keys[index], buffer, &mut tail, key_width);
    }
    buffer.shrink(tail);
    for index in (0..keys.len()).rev() {
        mark_unit(&values[index], buffer, value_width);
        mark_unit(&keys[index], buffer, key_width);
    }

    let head = buffer.expand(index_words);
    head.fill(0);
    let raw =
        unsafe { core::slice::from_raw_parts_mut(head.as_mut_ptr().cast::<u8>(), index_words * 4) };
    raw[..index.len()].copy_from_slice(index);
    head[0] |= (key_width as u32) << 30 | (value_width as u32) << 28;
    Some(Unit::segment(last, buffer.len()))
}

#[inline(always)]
pub(crate) fn serialize_map_pairs_at_mut(
    index: &[u8],
    pairs: &mut [(Unit, Unit)],
    buffer: &mut Buffer,
    last: usize,
) -> Option<Unit> {
    if pairs.is_empty() {
        return Some(Unit::inline(&[5u32 << 28]));
    }

    let ((key_size, key_width), (value_size, value_width)) = best_array_size_pairs(pairs);
    let index_words = index.len().div_ceil(4);
    let size = index_words + key_size + value_size;
    if size >= (1usize << 30) {
        return None;
    }

    let mut tail = buffer.len().checked_sub(last)?;
    for (key, value) in pairs.iter_mut().rev() {
        pick_unit(value, buffer, &mut tail, value_width);
        pick_unit(key, buffer, &mut tail, key_width);
    }
    buffer.shrink(tail);
    for (key, value) in pairs.iter().rev() {
        mark_unit(value, buffer, value_width);
        mark_unit(key, buffer, key_width);
    }

    let head = buffer.expand(index_words);
    head.fill(0);
    let raw =
        unsafe { core::slice::from_raw_parts_mut(head.as_mut_ptr().cast::<u8>(), index_words * 4) };
    raw[..index.len()].copy_from_slice(index);
    head[0] |= (key_width as u32) << 30 | (value_width as u32) << 28;
    Some(Unit::segment(last, buffer.len()))
}

#[inline(always)]
/// Encodes map key/value units and their perfect-hash index using `last` as the
/// boundary for referenced buffer segments.
pub fn serialize_map_at(
    index: &[u8],
    keys: &[Unit],
    values: &[Unit],
    buffer: &mut Buffer,
    last: usize,
) -> Option<Unit> {
    if keys.len() != values.len() {
        return None;
    }
    if keys.is_empty() {
        return Some(Unit::inline(&[5u32 << 28]));
    }

    let (key_size, key_width) = best_array_size(keys);
    let (value_size, value_width) = best_array_size(values);
    let index_words = index.len().div_ceil(4);
    let size = index_words + key_size + value_size;
    if size >= (1usize << 30) {
        return None;
    }

    let pair_width = key_width + value_width;
    let mut payloads = Vec::with_capacity(size.saturating_sub(keys.len() * pair_width));
    let mut cells = vec![0u32; keys.len() * pair_width];
    let cells_len = cells.len();
    for index in 0..keys.len() {
        let key_words = unit_words(&keys[index], buffer)?;
        let value_words = unit_words(&values[index], buffer)?;
        let cell_start = index.checked_mul(pair_width)?;
        let (key_cell, value_cell) =
            cells[cell_start..cell_start + pair_width].split_at_mut(key_width);

        if key_words.len() <= key_width {
            key_cell[..key_words.len()].copy_from_slice(key_words);
        } else {
            let payload_start = cells_len.checked_add(payloads.len())?;
            key_cell[0] = offset(payload_start.checked_sub(cell_start)?);
            payloads.extend_from_slice(key_words);
        }

        let value_cell_start = cell_start + key_width;
        if value_words.len() <= value_width {
            value_cell[..value_words.len()].copy_from_slice(value_words);
        } else {
            let payload_start = cells_len.checked_add(payloads.len())?;
            value_cell[0] = offset(payload_start.checked_sub(value_cell_start)?);
            payloads.extend_from_slice(value_words);
        }
    }
    buffer.shrink(buffer.len().checked_sub(last)?);
    buffer.put_words(&payloads);
    buffer.put_words(&cells);

    let head = buffer.expand(index_words);
    head.fill(0);
    let raw =
        unsafe { core::slice::from_raw_parts_mut(head.as_mut_ptr().cast::<u8>(), index_words * 4) };
    raw[..index.len()].copy_from_slice(index);
    head[0] |= (key_width as u32) << 30 | (value_width as u32) << 28;
    Some(Unit::segment(last, buffer.len()))
}

#[inline(always)]
/// Encodes a map from canonical key bytes and key/value field units.
pub fn serialize_map(
    index: &[u8],
    keys: &[Unit],
    values: &[Unit],
    buffer: &mut Buffer,
) -> Option<Unit> {
    serialize_map_at(index, keys, values, buffer, buffer.len())
}

#[cfg(test)]
mod tests {
    use super::{
        Unit, build_perfect_hash_index, build_perfect_hash_index_with_positions, fold_field,
        serialize_array, serialize_array_at, serialize_bool, serialize_map, serialize_message,
        serialize_scalar, serialize_str,
    };
    use crate::{ArrayView, Buffer, MapView, MessageView, StringView, ViewArray};

    #[test]
    fn serializes_inline_string() {
        let mut buffer = Buffer::new();
        let unit = serialize_str("hi", &mut buffer).unwrap();
        assert_eq!(unit.inline_words().len(), 1);
        assert!(buffer.is_empty());
    }

    #[test]
    fn folds_segmented_field_when_small() {
        let mut buffer = Buffer::new();
        let mut unit = serialize_str("hello", &mut buffer).unwrap();
        assert!(unit.is_segment());
        fold_field(&mut buffer, &mut unit);
        assert_eq!(unit.inline_words().len(), 2);
        assert!(buffer.is_empty());

        let mut small = serialize_str("abc", &mut buffer).unwrap();
        assert_eq!(small.inline_words().len(), 1);
        fold_field(&mut buffer, &mut small);
        assert_eq!(small.inline_words().len(), 1);
    }

    #[test]
    fn serializes_simple_message_roundtrip() {
        let mut buffer = Buffer::new();
        let field0 = serialize_scalar::<i32>(42);
        let field1 = serialize_bool(true);
        let field3 = serialize_str("hi", &mut buffer).unwrap();
        let mut fields = vec![field0, field1, Unit::empty(), field3];
        let _message = serialize_message(&mut fields, &mut buffer).unwrap();

        let view = MessageView::new(buffer.view()).unwrap();
        assert_eq!(view.scalar::<i32>(0), Some(42));
        assert_eq!(view.scalar::<bool>(1), Some(true));
        assert_eq!(view.string(3).unwrap().as_str(), Some("hi"));
        assert!(view.scalar::<i32>(2).is_none());
    }

    #[test]
    fn serializes_all_empty_fields_as_empty_message() {
        let mut buffer = Buffer::new();
        let mut fields = vec![Unit::empty(), Unit::empty()];
        let _message = serialize_message(&mut fields, &mut buffer).unwrap();
        let view = MessageView::new(buffer.view()).unwrap();
        assert!(!view.has_field(0));
        assert!(!view.has_field(1));
    }

    #[test]
    fn serializes_scalar_array_roundtrip() {
        let mut buffer = Buffer::new();
        let elements = vec![serialize_scalar::<i32>(1), serialize_scalar::<i32>(2)];
        let _array = serialize_array(&elements, &mut buffer).unwrap();
        let view = ArrayView::new(buffer.view()).unwrap();
        let scalars = view.scalars::<i32>().unwrap();
        assert_eq!(scalars.get(0), Some(1));
        assert_eq!(scalars.get(1), Some(2));
    }

    #[test]
    fn serializes_string_array_roundtrip() {
        let mut buffer = Buffer::new();
        let a = serialize_str("abc", &mut buffer).unwrap();
        let b = serialize_str("apple", &mut buffer).unwrap();
        let elements = vec![a, b];
        let _array = serialize_array(&elements, &mut buffer).unwrap();
        let view = ArrayView::new(buffer.view()).unwrap();
        let strings = ViewArray::<StringView<'_>>::new(view);
        assert_eq!(strings.get(0).unwrap().as_str(), Some("abc"));
        assert_eq!(strings.get(1).unwrap().as_str(), Some("apple"));
    }

    #[test]
    fn builds_index_and_serializes_string_int_map_roundtrip() {
        let keys_bytes = vec![b"abc-1".to_vec(), b"abc-2".to_vec()];
        let (index, positions) = build_perfect_hash_index_with_positions(&keys_bytes).unwrap();

        let mut buffer = Buffer::new();
        let key0 = serialize_str("abc-1", &mut buffer).unwrap();
        let key1 = serialize_str("abc-2", &mut buffer).unwrap();
        let mut keys = vec![Unit::empty(); 2];
        let mut values = vec![Unit::empty(); 2];
        keys[positions[0]] = key0;
        keys[positions[1]] = key1;
        values[positions[0]] = serialize_scalar::<i32>(1);
        values[positions[1]] = serialize_scalar::<i32>(2);
        let _map = serialize_map(&index, &keys, &values, &mut buffer).unwrap();

        let view = MapView::new(buffer.view()).unwrap();
        assert_eq!(
            view.find_str("abc-1").unwrap().value().scalar::<i32>(),
            Some(1)
        );
        assert_eq!(
            view.find_str("abc-2").unwrap().value().scalar::<i32>(),
            Some(2)
        );
        assert!(view.find_str("abc-3").is_none());
    }

    #[test]
    fn index_builder_matches_reader_expectations() {
        let keys = vec![b"abc-1".to_vec(), b"abc-2".to_vec(), b"abc-4".to_vec()];
        let index = build_perfect_hash_index(&keys).unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for key in &keys {
            seen.insert(super::locate_in_perfect_hash(&index, key).unwrap());
        }
        assert_eq!(seen.len(), keys.len());
    }

    #[test]
    fn index_builder_rejects_duplicate_keys() {
        let keys = [b"duplicate".as_slice(), b"duplicate".as_slice()];
        assert!(build_perfect_hash_index(&keys).is_none());
        assert!(build_perfect_hash_index_with_positions(&keys).is_none());
    }

    #[test]
    fn serializes_short_string_key_float_array_map_roundtrip() {
        let keys_bytes = vec![b"lv5".to_vec(), b"lv9".to_vec()];
        let (index, positions) = build_perfect_hash_index_with_positions(&keys_bytes).unwrap();

        let mut buffer = Buffer::new();
        let key0 = serialize_str("lv5", &mut buffer).unwrap();
        let key1 = serialize_str("lv9", &mut buffer).unwrap();

        let last = buffer.len();
        let value0 = serialize_array_at(
            &[
                serialize_scalar::<f32>(51.0),
                serialize_scalar::<f32>(52.0),
                serialize_scalar::<f32>(53.0),
            ],
            &mut buffer,
            last,
        )
        .unwrap();
        let last = buffer.len();
        let value1 = serialize_array_at(
            &[serialize_scalar::<f32>(91.0), serialize_scalar::<f32>(92.0)],
            &mut buffer,
            last,
        )
        .unwrap();

        let mut keys = vec![Unit::empty(); 2];
        let mut values = vec![Unit::empty(); 2];
        keys[positions[0]] = key0;
        keys[positions[1]] = key1;
        values[positions[0]] = value0;
        values[positions[1]] = value1;
        let _map = serialize_map(&index, &keys, &values, &mut buffer).unwrap();

        let view = MapView::new(buffer.view()).unwrap();
        let lv5 = view.find_str("lv5").unwrap().value().array().unwrap();
        assert_eq!(
            lv5.scalars::<f32>().unwrap().iter().collect::<Vec<_>>(),
            vec![51.0, 52.0, 53.0]
        );
        let lv9 = view.find_str("lv9").unwrap().value().array().unwrap();
        assert_eq!(
            lv9.scalars::<f32>().unwrap().iter().collect::<Vec<_>>(),
            vec![91.0, 92.0]
        );
    }

    #[test]
    fn serializes_multi_entry_short_string_key_float_array_map_roundtrip() {
        let keys = ["lv1", "lv2", "lv3", "lv4", "lv5", "lv9"];
        let key_bytes = keys
            .iter()
            .map(|key| key.as_bytes().to_vec())
            .collect::<Vec<_>>();
        let (index, positions) = build_perfect_hash_index_with_positions(&key_bytes).unwrap();

        let values_src = [
            vec![11.0f32, 12.0],
            vec![21.0, 22.0],
            vec![31.0, 32.0],
            vec![41.0, 42.0],
            vec![51.0, 52.0, 53.0],
            vec![91.0, 92.0],
        ];

        let mut buffer = Buffer::new();
        let encoded_keys = keys
            .iter()
            .map(|key| serialize_str(key, &mut buffer).unwrap())
            .collect::<Vec<_>>();
        let encoded_values = values_src
            .iter()
            .map(|values| {
                let units = values
                    .iter()
                    .map(|value| serialize_scalar::<f32>(*value))
                    .collect::<Vec<_>>();
                serialize_array(&units, &mut buffer).unwrap()
            })
            .collect::<Vec<_>>();

        let mut keys = vec![Unit::empty(); positions.len()];
        let mut values = vec![Unit::empty(); positions.len()];
        for (idx, pos) in positions.iter().copied().enumerate() {
            keys[pos] = encoded_keys[idx];
            values[pos] = encoded_values[idx];
        }

        let _map = serialize_map(&index, &keys, &values, &mut buffer).unwrap();
        let view = MapView::new(buffer.view()).unwrap();
        for (key, expected) in ["lv1", "lv2", "lv3", "lv4", "lv5", "lv9"]
            .into_iter()
            .zip(values_src.iter())
        {
            let array = view.find_str(key).unwrap().value().array().unwrap();
            assert_eq!(
                array.scalars::<f32>().unwrap().iter().collect::<Vec<_>>(),
                expected.clone()
            );
        }
    }
}

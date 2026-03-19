use crate::{Buffer, Scalar, hash128};

#[derive(Clone, Copy, Debug, Default)]
pub struct Segment {
    pub pos: usize,
    pub len: usize,
}

impl Segment {
    pub fn end(self) -> usize {
        self.pos - self.len
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Unit {
    inline_len: usize,
    inline_data: [u32; 3],
    segment: Segment,
}

impl Unit {
    pub fn empty() -> Self {
        Self::default()
    }

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

    pub fn size(&self) -> usize {
        if self.inline_len != 0 {
            self.inline_len
        } else {
            self.segment.len
        }
    }

    pub fn is_empty(&self) -> bool {
        self.size() == 0
    }

    pub fn is_segment(&self) -> bool {
        self.inline_len == 0 && self.segment.len != 0
    }

    pub fn inline_words(&self) -> &[u32] {
        &self.inline_data[..self.inline_len]
    }

    pub fn segment_info(&self) -> Segment {
        self.segment
    }
}

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

fn offset(off: usize) -> u32 {
    ((off as u32) << 2) | 3
}

fn copy_inline(body: &mut [u32], body_index: &mut usize, pos: &mut usize, field: &Unit) {
    let len = field.inline_len;
    body[*body_index..*body_index + len].copy_from_slice(field.inline_words());
    *body_index += len;
    *pos -= len;
}

fn write_padded_unit_cell(buffer: &mut Buffer, unit: &Unit, width: usize) -> Option<()> {
    let old_len = buffer.len();
    let mut copied_segment = [0u32; 3];
    let copied_len =
        if unit.inline_len == 0 && unit.segment.len <= width && unit.segment.len != 0 {
            let start = old_len.checked_sub(unit.segment.pos)?;
            let words = buffer.view().get(start..start + unit.segment.len)?;
            copied_segment[..words.len()].copy_from_slice(words);
            words.len()
        } else {
            0
        };

    let new_len = old_len + width;
    let cell = buffer.expand(width);
    if unit.inline_len != 0 {
        cell[..unit.inline_len].copy_from_slice(unit.inline_words());
        for slot in &mut cell[unit.inline_len..] {
            *slot = 0;
        }
    } else if copied_len != 0 {
        cell[..copied_len].copy_from_slice(&copied_segment[..copied_len]);
        for slot in &mut cell[copied_len..] {
            *slot = 0;
        }
    } else {
        cell[0] = offset(new_len - unit.segment.pos);
        for slot in &mut cell[1..] {
            *slot = 0;
        }
    }
    Some(())
}

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

fn perfect_hash_section(size: usize) -> usize {
    ((size * 105).saturating_add(255) / 256).max(10)
}

fn perfect_hash_bitmap_size(section: usize) -> usize {
    ((section * 3 + 31) & !31) / 4
}

fn set_bit2(vec: &mut [u8], pos: usize, val: u8) {
    let shift = ((pos & 3) << 1) as u8;
    let idx = pos >> 2;
    vec[idx] &= !(3u8 << shift);
    vec[idx] |= (val & 3) << shift;
}

fn get_bit2(vec: &[u8], pos: usize) -> u8 {
    (vec[pos >> 2] >> ((pos & 3) << 1)) & 3
}

#[derive(Clone, Copy)]
struct Edge {
    slots: [usize; 3],
}

fn peel_graph(edges: &[Edge], slot_cnt: usize) -> Option<Vec<usize>> {
    let mut adjacency = vec![Vec::<usize>::new(); slot_cnt];
    let mut degree = vec![0usize; slot_cnt];
    for (idx, edge) in edges.iter().enumerate() {
        for &slot in &edge.slots {
            adjacency[slot].push(idx);
            degree[slot] += 1;
        }
    }

    let mut queue = std::collections::VecDeque::new();
    for (slot, &deg) in degree.iter().enumerate() {
        if deg == 1 {
            queue.push_back(slot);
        }
    }

    let mut removed = vec![false; edges.len()];
    let mut order = Vec::with_capacity(edges.len());

    while let Some(slot) = queue.pop_front() {
        if degree[slot] != 1 {
            continue;
        }
        let edge_idx = adjacency[slot].iter().copied().find(|&idx| !removed[idx])?;
        removed[edge_idx] = true;
        order.push(edge_idx);
        for &other in &edges[edge_idx].slots {
            if degree[other] == 0 {
                continue;
            }
            degree[other] -= 1;
            if degree[other] == 1 {
                queue.push_back(other);
            }
        }
    }

    if order.len() == edges.len() {
        Some(order)
    } else {
        None
    }
}

fn count_valid_slots(bitmap: &[u8], block: usize) -> usize {
    let start = block * 32;
    let mut count = 0usize;
    for pos in start..start + 32 {
        if get_bit2(bitmap, pos) != 3 {
            count += 1;
        }
    }
    count
}

fn locate_in_perfect_hash(index: &[u8], key: &[u8]) -> Option<usize> {
    let size = u32::from_le_bytes(index.get(..4)?.try_into().ok()?) as usize & 0x0fff_ffff;
    if size < 2 {
        return Some(0);
    }
    let section = perfect_hash_section(size);
    let bitmap_size = perfect_hash_bitmap_size(section);
    let bitmap = index.get(8..8 + bitmap_size)?;
    let table = index.get(8 + bitmap_size..)?;
    let seed = u32::from_le_bytes(index.get(4..8)?.try_into().ok()?) as u64;
    let code = hash128(key, seed);
    let slots = [
        code[0] as usize % section,
        code[1] as usize % section + section,
        code[2] as usize % section + section * 2,
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

    let start = block * 32;
    let rank = (start..start + bit + 1)
        .filter(|&pos| get_bit2(bitmap, pos) != 3)
        .count();
    Some(off + rank - 1)
}

pub fn build_perfect_hash_index<K: AsRef<[u8]>>(keys: &[K]) -> Option<Vec<u8>> {
    Some(build_perfect_hash_index_with_positions(keys)?.0)
}

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
    let slot_cnt = section * 3;
    let bitmap_size = perfect_hash_bitmap_size(section);

    let mut seed = 0u32;
    let (edges, order) = loop {
        let edges: Vec<_> = keys
            .iter()
            .map(|key| {
                let code = hash128(key.as_ref(), seed as u64);
                Edge {
                    slots: [
                        code[0] as usize % section,
                        code[1] as usize % section + section,
                        code[2] as usize % section + section * 2,
                    ],
                }
            })
            .collect();
        if let Some(order) = peel_graph(&edges, slot_cnt) {
            break (edges, order);
        }
        seed = seed.wrapping_add(1);
        if seed == 0 {
            return None;
        }
    };

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

    let positions = keys
        .iter()
        .map(|key| locate_in_perfect_hash(&out, key.as_ref()))
        .collect::<Option<Vec<_>>>()?;

    Some((out, positions))
}

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

pub fn serialize_scalar<T: Scalar>(value: T) -> Unit {
    let mut words = [0u32; 3];
    let word_len = T::WIDTH;
    value.write_words(&mut words[..word_len]).expect("scalar width must match");
    Unit {
        inline_len: word_len,
        inline_data: words,
        segment: Segment::default(),
    }
}

pub fn serialize_bool(value: bool) -> Unit {
    Unit::inline(&[u32::from(value)])
}

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
        words.fill(0);
        let raw = unsafe {
            core::slice::from_raw_parts_mut(words.as_mut_ptr().cast::<u8>(), total_words * 4)
        };
        raw[..header_size].copy_from_slice(&header[..header_size]);
        raw[header_size..header_size + bytes.len()].copy_from_slice(bytes);
        Some(Unit::segment(last, buffer.len()))
    }
}

pub fn serialize_str(value: &str, buffer: &mut Buffer) -> Option<Unit> {
    serialize_bytes(value.as_bytes(), buffer)
}

pub fn serialize_message(fields: &mut Vec<Unit>, buffer: &mut Buffer) -> Option<Unit> {
    if fields.is_empty() {
        return None;
    }
    while matches!(fields.last(), Some(unit) if unit.is_empty()) {
        fields.pop();
    }
    if fields.is_empty() {
        let last = buffer.len();
        buffer.put(0);
        return Some(Unit::segment(last, buffer.len()));
    }

    let last = buffer.len();
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
    let total_size = last + head_size + body_size;
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

pub fn serialize_array(elements: &[Unit], buffer: &mut Buffer) -> Option<Unit> {
    if elements.is_empty() {
        return Some(Unit::inline(&[1]));
    }

    let (size, width) = best_array_size(elements);
    if size >= (1usize << 30) {
        return None;
    }

    let last = buffer.len();
    for unit in elements.iter().rev() {
        write_padded_unit_cell(buffer, unit, width)?;
    }
    buffer.put(((elements.len() as u32) << 2) | width as u32);
    Some(Unit::segment(last, buffer.len()))
}

pub fn serialize_map(index: &[u8], keys: &[Unit], values: &[Unit], buffer: &mut Buffer) -> Option<Unit> {
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

    let last = buffer.len();
    for (key, value) in keys.iter().zip(values).rev() {
        for (unit, width) in [(value, value_width), (key, key_width)] {
            write_padded_unit_cell(buffer, unit, width)?;
        }
    }

    let head = buffer.expand(index_words);
    head.fill(0);
    let raw = unsafe { core::slice::from_raw_parts_mut(head.as_mut_ptr().cast::<u8>(), index_words * 4) };
    raw[..index.len()].copy_from_slice(index);
    head[0] |= (key_width as u32) << 30 | (value_width as u32) << 28;
    Some(Unit::segment(last, buffer.len()))
}

#[cfg(test)]
mod tests {
    use super::{
        Unit, build_perfect_hash_index, build_perfect_hash_index_with_positions, fold_field,
        serialize_array, serialize_bool, serialize_map, serialize_message, serialize_scalar,
        serialize_str,
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
        assert_eq!(view.find_str("abc-1").unwrap().value().scalar::<i32>(), Some(1));
        assert_eq!(view.find_str("abc-2").unwrap().value().scalar::<i32>(), Some(2));
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
}

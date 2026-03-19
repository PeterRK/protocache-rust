use crate::error::{CorruptionKind, ReadError};

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

    let mut pos = 0usize;
    while pos < src.len() {
        let a_start = pos;
        let a = pick(src, &mut pos);
        if pos == src.len() {
            out.push(a);
            emit(src, a_start, pos, a, out);
            break;
        }
        let b_start = pos;
        let b = pick(src, &mut pos);
        out.push(a | (b << 4));
        emit(src, a_start, b_start, a, out);
        emit(src, b_start, pos, b, out);
    }
}

fn pick(src: &[u8], pos: &mut usize) -> u8 {
    let start = *pos;
    let first = src[start];
    *pos += 1;
    if first == 0 {
        while *pos < src.len() && *pos - start < 4 && src[*pos] == 0 {
            *pos += 1;
        }
        0x8 | ((*pos - start - 1) as u8)
    } else if first == 0xff {
        while *pos < src.len() && *pos - start < 4 && src[*pos] == 0xff {
            *pos += 1;
        }
        0xC | ((*pos - start - 1) as u8)
    } else {
        while *pos < src.len() && *pos - start < 7 && src[*pos] != 0 && src[*pos] != 0xff {
            *pos += 1;
        }
        (*pos - start) as u8
    }
}

fn emit(src: &[u8], start: usize, end: usize, mark: u8, out: &mut Vec<u8>) {
    if (mark & 0x8) == 0 {
        let len = end - start;
        if len == 0 {
            return;
        }

        out.extend_from_slice(&src[start..end]);
    }
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
    out.clear();
    out.resize(target, 0u8);
    let mut out_pos = 0usize;
    while pos < src.len() {
        let mark = src[pos];
        pos += 1;
        unpack(mark & 0x0f, src, &mut pos, out, &mut out_pos)?;
        unpack(mark >> 4, src, &mut pos, out, &mut out_pos)?;
    }
    if out_pos != target {
        return Err(ReadError::new(CorruptionKind::Truncated));
    }
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

fn parse_varint(src: &[u8]) -> Result<(usize, usize), ReadError> {
    let mut size = 0usize;
    let mut pos = 0usize;
    for shift in (0..32).step_by(7) {
        let byte = *src.get(pos).ok_or(ReadError::new(CorruptionKind::Truncated))?;
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

fn unpack(
    mark: u8,
    src: &[u8],
    pos: &mut usize,
    out: &mut [u8],
    out_pos: &mut usize,
) -> Result<(), ReadError> {
    if *out_pos == out.len() {
        return Ok(());
    }
    if (mark & 0x8) != 0 {
        let count = ((mark & 0x3) + 1) as usize;
        let end = out_pos
            .checked_add(count)
            .ok_or(ReadError::new(CorruptionKind::IntegerOverflow))?;
        if end > out.len() {
            return Err(ReadError::new(CorruptionKind::Truncated));
        }
        out[*out_pos..end].fill(if (mark & 0x4) != 0 { 0xff } else { 0x00 });
        *out_pos = end;
        return Ok(());
    }
    let len = (mark & 0x7) as usize;
    let end = pos.checked_add(len).ok_or(ReadError::new(CorruptionKind::IntegerOverflow))?;
    let out_end = out_pos
        .checked_add(len)
        .ok_or(ReadError::new(CorruptionKind::IntegerOverflow))?;
    if end > src.len() || out_end > out.len() {
        return Err(ReadError::new(CorruptionKind::Truncated));
    }
    out[*out_pos..out_end].copy_from_slice(&src[*pos..end]);
    *pos = end;
    *out_pos = out_end;
    Ok(())
}

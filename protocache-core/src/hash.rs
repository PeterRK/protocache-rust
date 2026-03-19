fn read_u64_le(bytes: &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(..8)?.try_into().ok()?))
}

#[inline]
fn rot64(x: u64, k: u32) -> u64 {
    x.rotate_left(k)
}

#[inline]
fn mix(a: &mut u64, b: &mut u64, c: &mut u64, d: &mut u64) {
    *c = rot64(*c, 50);
    *c = (*c).wrapping_add(*d);
    *a ^= *c;
    *d = rot64(*d, 52);
    *d = (*d).wrapping_add(*a);
    *b ^= *d;
    *a = rot64(*a, 30);
    *a = (*a).wrapping_add(*b);
    *c ^= *a;
    *b = rot64(*b, 41);
    *b = (*b).wrapping_add(*c);
    *d ^= *b;
    *c = rot64(*c, 54);
    *c = (*c).wrapping_add(*d);
    *a ^= *c;
    *d = rot64(*d, 48);
    *d = (*d).wrapping_add(*a);
    *b ^= *d;
    *a = rot64(*a, 38);
    *a = (*a).wrapping_add(*b);
    *c ^= *a;
    *b = rot64(*b, 37);
    *b = (*b).wrapping_add(*c);
    *d ^= *b;
    *c = rot64(*c, 62);
    *c = (*c).wrapping_add(*d);
    *a ^= *c;
    *d = rot64(*d, 34);
    *d = (*d).wrapping_add(*a);
    *b ^= *d;
    *a = rot64(*a, 5);
    *a = (*a).wrapping_add(*b);
    *c ^= *a;
    *b = rot64(*b, 36);
    *b = (*b).wrapping_add(*c);
    *d ^= *b;
}

#[inline]
fn end(a: &mut u64, b: &mut u64, c: &mut u64, d: &mut u64) {
    *d ^= *c;
    *c = rot64(*c, 15);
    *d = (*d).wrapping_add(*c);
    *a ^= *d;
    *d = rot64(*d, 52);
    *a = (*a).wrapping_add(*d);
    *b ^= *a;
    *a = rot64(*a, 26);
    *b = (*b).wrapping_add(*a);
    *c ^= *b;
    *b = rot64(*b, 51);
    *c = (*c).wrapping_add(*b);
    *d ^= *c;
    *c = rot64(*c, 28);
    *d = (*d).wrapping_add(*c);
    *a ^= *d;
    *d = rot64(*d, 9);
    *a = (*a).wrapping_add(*d);
    *b ^= *a;
    *a = rot64(*a, 47);
    *b = (*b).wrapping_add(*a);
    *c ^= *b;
    *b = rot64(*b, 54);
    *c = (*c).wrapping_add(*b);
    *d ^= *c;
    *c = rot64(*c, 32);
    *d = (*d).wrapping_add(*c);
    *a ^= *d;
    *d = rot64(*d, 25);
    *a = (*a).wrapping_add(*d);
    *b ^= *a;
    *a = rot64(*a, 63);
    *b = (*b).wrapping_add(*a);
}

pub fn hash128(msg: &[u8], seed: u64) -> [u32; 4] {
    const MAGIC: u64 = 0xdead_beef_dead_beef;

    let mut a = seed;
    let mut b = seed;
    let mut c = MAGIC;
    let mut d = MAGIC;

    let mut offset = 0usize;
    let long_end = msg.len() & !0x1f;
    while offset < long_end {
        c = c.wrapping_add(read_u64_le(&msg[offset..offset + 8]).unwrap());
        d = d.wrapping_add(read_u64_le(&msg[offset + 8..offset + 16]).unwrap());
        mix(&mut a, &mut b, &mut c, &mut d);
        a = a.wrapping_add(read_u64_le(&msg[offset + 16..offset + 24]).unwrap());
        b = b.wrapping_add(read_u64_le(&msg[offset + 24..offset + 32]).unwrap());
        offset += 32;
    }

    if (msg.len() & 0x10) != 0 {
        c = c.wrapping_add(read_u64_le(&msg[offset..offset + 8]).unwrap());
        d = d.wrapping_add(read_u64_le(&msg[offset + 8..offset + 16]).unwrap());
        mix(&mut a, &mut b, &mut c, &mut d);
        offset += 16;
    }

    let tail = &msg[offset..];
    d = d.wrapping_add((msg.len() as u64) << 56);
    match tail.len() & 0xf {
        15 => d = d.wrapping_add((tail[14] as u64) << 48),
        _ => {}
    }
    match tail.len() & 0xf {
        15 | 14 => d = d.wrapping_add((tail[13] as u64) << 40),
        _ => {}
    }
    match tail.len() & 0xf {
        15 | 14 | 13 => d = d.wrapping_add((tail[12] as u64) << 32),
        _ => {}
    }
    match tail.len() & 0xf {
        15 | 14 | 13 | 12 => {
            d = d.wrapping_add(u32::from_le_bytes(tail[8..12].try_into().unwrap()) as u64);
            c = c.wrapping_add(read_u64_le(tail).unwrap());
        }
        11 => d = d.wrapping_add((tail[10] as u64) << 16),
        _ => {}
    }
    match tail.len() & 0xf {
        11 | 10 => d = d.wrapping_add((tail[9] as u64) << 8),
        _ => {}
    }
    match tail.len() & 0xf {
        11 | 10 | 9 => d = d.wrapping_add(tail[8] as u64),
        _ => {}
    }
    match tail.len() & 0xf {
        11 | 10 | 9 | 8 => c = c.wrapping_add(read_u64_le(tail).unwrap()),
        7 => c = c.wrapping_add((tail[6] as u64) << 48),
        _ => {}
    }
    match tail.len() & 0xf {
        7 | 6 => c = c.wrapping_add((tail[5] as u64) << 40),
        _ => {}
    }
    match tail.len() & 0xf {
        7 | 6 | 5 => c = c.wrapping_add((tail[4] as u64) << 32),
        _ => {}
    }
    match tail.len() & 0xf {
        7 | 6 | 5 | 4 => {
            c = c.wrapping_add(u32::from_le_bytes(tail[0..4].try_into().unwrap()) as u64)
        }
        3 => c = c.wrapping_add((tail[2] as u64) << 16),
        _ => {}
    }
    match tail.len() & 0xf {
        3 | 2 => c = c.wrapping_add((tail[1] as u64) << 8),
        _ => {}
    }
    match tail.len() & 0xf {
        3 | 2 | 1 => c = c.wrapping_add(tail[0] as u64),
        0 => {
            c = c.wrapping_add(MAGIC);
            d = d.wrapping_add(MAGIC);
        }
        _ => {}
    }

    end(&mut a, &mut b, &mut c, &mut d);

    [
        (a & 0xffff_ffff) as u32,
        (a >> 32) as u32,
        (b & 0xffff_ffff) as u32,
        (b >> 32) as u32,
    ]
}

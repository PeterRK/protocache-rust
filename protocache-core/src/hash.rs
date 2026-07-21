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
    if tail.len() & 0xf == 15 {
        d = d.wrapping_add((tail[14] as u64) << 48);
    }
    match tail.len() & 0xf {
        15 | 14 => d = d.wrapping_add((tail[13] as u64) << 40),
        _ => {}
    }
    if let 13..=15 = tail.len() & 0xf {
        d = d.wrapping_add((tail[12] as u64) << 32);
    }
    match tail.len() & 0xf {
        12..=15 => {
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
    if let 9..=11 = tail.len() & 0xf {
        d = d.wrapping_add(tail[8] as u64);
    }
    match tail.len() & 0xf {
        8..=11 => c = c.wrapping_add(read_u64_le(tail).unwrap()),
        7 => c = c.wrapping_add((tail[6] as u64) << 48),
        _ => {}
    }
    match tail.len() & 0xf {
        7 | 6 => c = c.wrapping_add((tail[5] as u64) << 40),
        _ => {}
    }
    if let 5..=7 = tail.len() & 0xf {
        c = c.wrapping_add((tail[4] as u64) << 32);
    }
    match tail.len() & 0xf {
        4..=7 => c = c.wrapping_add(u32::from_le_bytes(tail[0..4].try_into().unwrap()) as u64),
        3 => c = c.wrapping_add((tail[2] as u64) << 16),
        _ => {}
    }
    match tail.len() & 0xf {
        3 | 2 => c = c.wrapping_add((tail[1] as u64) << 8),
        _ => {}
    }
    match tail.len() & 0xf {
        1..=3 => c = c.wrapping_add(tail[0] as u64),
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

#[cfg(test)]
mod tests {
    use super::hash128;

    #[test]
    fn matches_hash_test_go_prefix_vectors() {
        let expected = [
            [0x6bf50919, 0x232706fc, 0xb4e851c7, 0x8b72ee65],
            [0xd54ec67e, 0x50209687, 0x8df1cf6d, 0x62fe8510],
            [0x68f3fb4f, 0xfbe67d83, 0x706d5a5a, 0xb54a5a89],
            [0x5846ccfa, 0x2882d11a, 0x70109222, 0x6b21b0e8],
            [0x25d6d000, 0xf5e0d563, 0xf9ac75e5, 0xaf8703c9],
            [0x7ae7a5ad, 0x59a0f67b, 0xc053b848, 0x84d7aeab],
            [0x68e42c21, 0xf01562a2, 0x22873e7e, 0xdfe994ab],
            [0x620725dd, 0x16133104, 0xa7182e6a, 0xa5ca36af],
            [0xdf599479, 0x7a9378dc, 0xa74ecdd7, 0x30f5a569],
            [0x76c20a78, 0xd9f07bdc, 0x47f7888a, 0x34f06218],
            [0x07df83da, 0x332a4fff, 0xc0ea6b72, 0xfa40557c],
            [0xd11659dc, 0x976beeef, 0xa72d0039, 0x8a3187b6],
            [0xe4c6832a, 0xc3fcc139, 0xe01e2f2e, 0xdadfeff6],
            [0xc7746a6f, 0x86130593, 0x904fe39d, 0x8ac9fb14],
            [0x5cdde280, 0x70550dbe, 0x282706c0, 0xddb95757],
            [0xf6b9122d, 0x67211fba, 0xbbc700db, 0x68f4e8f3],
            [0x964b80ad, 0xe2d06846, 0xc75c4c20, 0x6005068a],
            [0x0258ce93, 0xd55b3c01, 0x659d9950, 0x981c8b03],
            [0xa032fa13, 0x5a2507da, 0xfc0c6cf7, 0x0d1c989b],
            [0x8ae5cd55, 0xaf861867, 0xd427eefc, 0xe0b75cfa],
            [0xe8a139d8, 0xad5a7047, 0x988a753e, 0x183621cf],
            [0x2723cd5e, 0x8fc11019, 0x0764b844, 0x203129f8],
            [0x85d7af19, 0x50170b44, 0x45db7d35, 0x7f2c79d1],
            [0x52212bf3, 0x7c324446, 0x156e2ad2, 0x27fd51b9],
            [0x5cce7360, 0x90e57122, 0xf7433428, 0xf743b8f6],
            [0x1add41e1, 0x9919537c, 0x05b261f2, 0x7ff0158f],
            [0x0883029f, 0x3a70a807, 0x1815d20a, 0xc5dcba91],
            [0x290e2879, 0xcc32b418, 0xd79b5dfb, 0xbb7945d6],
            [0x46077aeb, 0xde493e46, 0x2660973a, 0x465c2ea5],
            [0x5316f970, 0x4d3ad9b5, 0x0a7d87bb, 0x9137e304],
            [0xefe848f4, 0x1547de75, 0xb5330aac, 0x21ae3f08],
            [0x6aab6aff, 0xe2ead0cc, 0xf77e70a7, 0x29a20bcc],
            [0xe9b451b4, 0x3dc2f4a9, 0xde7b60d2, 0x27de306d],
            [0xa4de9f51, 0xce247654, 0x5e948d66, 0x040097e4],
            [0xa2305503, 0xbc118f2b, 0xea32853f, 0x810f05d0],
            [0xcac2a118, 0xb55cd8bd, 0x64705d2a, 0x4e93b651],
            [0x07c32f38, 0xb7c97db8, 0x0adef63d, 0x51072323],
        ];
        let data = b"0123456789abcdefghijklmnopqrstuvwxyz";
        assert_eq!(expected.len(), data.len() + 1);
        for (len, want) in expected.iter().enumerate() {
            assert_eq!(hash128(&data[..len], 0), *want, "prefix length {len}");
        }
    }
}

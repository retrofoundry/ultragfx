// F3DEX2/F3DZEX's exceptional fog lane: f3dex2.s:1356–1481 at bd31393.
// Ordinary positive-W fog keeps the renderer's float precision.
pub(super) fn alpha(z: i32, w: i32, norm: u16, factors: [i16; 2]) -> u8 {
    let normalized = ((i64::from(w) * i64::from(norm)) >> 16) as i32;
    let seed = reciprocal(normalized);
    let (acc, frac) = multiply(normalized, seed);
    let error = (4 << 16) - i64::from(pack(acc, frac)) * 4;
    let correction = pack(error, error as u16);
    let (mut acc, frac) = multiply(correction, seed);
    if w < 0 {
        acc += (0x7fff_i64 * 0x7fff) << 16;
    }
    let inverse = pack(acc, frac);
    let (acc, frac) = multiply(z, inverse);
    let projected = (i64::from(pack(acc, frac)) * i64::from(norm)) >> 16;
    (((projected * i64::from(factors[0])) >> 16) + i64::from(factors[1])).clamp(0, 255) as u8
}

fn pack(acc: i64, frac: u16) -> i32 {
    (((acc >> 16).clamp(-0x8000, 0x7fff) << 16) | i64::from(frac)) as i32
}

fn multiply(a: i32, b: i32) -> (i64, u16) {
    let acc = (i64::from(a) * i64::from(b)) >> 16;
    // VMADN reads the fraction before VMADH adds the integer × integer term.
    let partial = acc - ((i64::from(a >> 16) * i64::from(b >> 16)) << 16);
    let frac = match partial >> 16 {
        h if h < -0x8000 => 0,
        h if h > 0x7fff => 0xffff,
        _ => partial as u16,
    };
    (acc, frac)
}

fn reciprocal(input: i32) -> i32 {
    let mask = input >> 31;
    let mut data = input ^ mask;
    if input > -32768 {
        data = data.wrapping_sub(mask);
    }
    if data == 0 {
        return 0x7fff_ffff;
    }
    if input == -32768 {
        return -65536;
    }
    let shift = (data as u32).leading_zeros();
    let index = ((data as u32) << shift & 0x7fc0_0000) >> 22;
    let entry = if index == 0 {
        0xffff
    } else {
        ((((1u64 << 34) / (u64::from(index) + 512) + 1) >> 8) & 0xffff) as u32
    };
    ((((0x10000 | entry) << 14) >> (31 - shift)) as i32) ^ mask
}

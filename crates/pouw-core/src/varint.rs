use alloc::vec::Vec;

use crate::error::{Error, ErrorKind, Result};

pub fn write_u32(output: &mut Vec<u8>, mut value: u32) {
    while value >= 0x80 {
        output.push(((value as u8) & 0x7f) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

pub fn write_u64(output: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        output.push(((value as u8) & 0x7f) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

pub fn write_i32(output: &mut Vec<u8>, value: i32) {
    let zigzag = ((value as u32) << 1) ^ ((value >> 31) as u32);
    write_u32(output, zigzag);
}

pub fn write_i16(output: &mut Vec<u8>, value: i16) {
    write_i32(output, i32::from(value));
}

pub fn read_u32(input: &[u8], offset: &mut usize) -> Result<u32> {
    let mut value = 0_u32;
    for index in 0..5 {
        let byte = *input.get(*offset).ok_or_else(|| {
            Error::new(
                ErrorKind::Truncated,
                "truncated-varint",
                "Unexpected end of canonical u32 varint.",
            )
        })?;
        *offset += 1;
        let payload = u32::from(byte & 0x7f);
        if index == 4 && payload > 0x0f {
            return Err(Error::new(
                ErrorKind::ArithmeticOverflow,
                "varint-overflow",
                "Canonical u32 varint exceeds 32 bits.",
            ));
        }
        value |= payload << (index * 7);
        if byte & 0x80 == 0 {
            if index > 0 && payload == 0 {
                return Err(Error::new(
                    ErrorKind::NonCanonical,
                    "non-canonical-varint",
                    "Canonical varints must use the shortest representation.",
                ));
            }
            return Ok(value);
        }
    }
    Err(Error::new(
        ErrorKind::ArithmeticOverflow,
        "varint-overflow",
        "Canonical u32 varint is too long.",
    ))
}

pub fn read_u64(input: &[u8], offset: &mut usize) -> Result<u64> {
    let mut value = 0_u64;
    for index in 0..10 {
        let byte = *input.get(*offset).ok_or_else(|| {
            Error::new(
                ErrorKind::Truncated,
                "truncated-varint",
                "Unexpected end of canonical u64 varint.",
            )
        })?;
        *offset += 1;
        let payload = u64::from(byte & 0x7f);
        if index == 9 && payload > 1 {
            return Err(Error::new(
                ErrorKind::ArithmeticOverflow,
                "varint-overflow",
                "Canonical u64 varint exceeds 64 bits.",
            ));
        }
        value |= payload << (index * 7);
        if byte & 0x80 == 0 {
            if index > 0 && payload == 0 {
                return Err(Error::new(
                    ErrorKind::NonCanonical,
                    "non-canonical-varint",
                    "Canonical varints must use the shortest representation.",
                ));
            }
            return Ok(value);
        }
    }
    Err(Error::new(
        ErrorKind::ArithmeticOverflow,
        "varint-overflow",
        "Canonical u64 varint is too long.",
    ))
}

pub fn read_i32(input: &[u8], offset: &mut usize) -> Result<i32> {
    let value = read_u32(input, offset)?;
    Ok(((value >> 1) as i32) ^ -((value & 1) as i32))
}

pub fn read_i16(input: &[u8], offset: &mut usize) -> Result<i16> {
    let value = read_i32(input, offset)?;
    i16::try_from(value).map_err(|_| {
        Error::new(
            ErrorKind::OutOfBounds,
            "i16-out-of-range",
            "Signed value does not fit i16.",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u32_round_trip_and_redundant_zero_rejection() {
        for value in [0, 1, 127, 128, 16_384, u32::MAX] {
            let mut bytes = Vec::new();
            write_u32(&mut bytes, value);
            let mut offset = 0;
            assert_eq!(read_u32(&bytes, &mut offset).unwrap(), value);
            assert_eq!(offset, bytes.len());
        }
        let error = read_u32(&[0x80, 0x00], &mut 0).unwrap_err();
        assert_eq!(error.kind, ErrorKind::NonCanonical);
    }

    #[test]
    fn signed_round_trip() {
        for value in [i32::MIN, -1000, -1, 0, 1, 1000, i32::MAX] {
            let mut bytes = Vec::new();
            write_i32(&mut bytes, value);
            assert_eq!(read_i32(&bytes, &mut 0).unwrap(), value);
        }
    }
}

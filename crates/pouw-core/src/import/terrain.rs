use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::error::{Error, ErrorKind, Result};
use crate::model::{Coord, IncumbentFormat, Profile, Semantics, TerrainSemantics};

use super::ImportedAsset;

const HEADER_BYTES: usize = 16;
const RECORD_BYTES: usize = 3;
const MAX_CAPACITY: usize = 2_048;

pub(super) fn import(input: &[u8]) -> Result<ImportedAsset> {
    if input.len() < HEADER_BYTES {
        return Err(Error::new(
            ErrorKind::Truncated,
            "chunkbroken-header",
            "ChunkBroken v1 account is shorter than its 16-byte header.",
        ));
    }
    if &input[0..4] != b"NCBK" {
        return Err(Error::invalid(
            "chunkbroken-magic",
            "ChunkBroken v1 account magic must be NCBK.",
        ));
    }
    if input[4] != 1 {
        return Err(Error::new(
            ErrorKind::UnsupportedVersion,
            "chunkbroken-version",
            "Only ChunkBroken version 1 is supported.",
        ));
    }
    let count = usize::from(u16::from_le_bytes([input[6], input[7]]));
    let capacity = usize::from(u16::from_le_bytes([input[8], input[9]]));
    let min_y = i16::from_le_bytes([input[10], input[11]]);
    if count > capacity || capacity > MAX_CAPACITY {
        return Err(Error::limit(
            "chunkbroken-capacity",
            "ChunkBroken count or capacity exceeds version 1 limits.",
        ));
    }
    let expected = HEADER_BYTES
        .checked_add(
            capacity
                .checked_mul(RECORD_BYTES)
                .ok_or_else(|| Error::overflow("ChunkBroken account length overflow."))?,
        )
        .ok_or_else(|| Error::overflow("ChunkBroken account length overflow."))?;
    if input.len() != expected {
        return Err(Error::new(
            ErrorKind::TrailingData,
            "chunkbroken-size",
            "ChunkBroken account length does not match its declared capacity.",
        ));
    }

    let mut identities = BTreeSet::new();
    for index in 0..count {
        let offset = HEADER_BYTES + index * RECORD_BYTES;
        let packed = u32::from(input[offset])
            | (u32::from(input[offset + 1]) << 8)
            | (u32::from(input[offset + 2]) << 16);
        if packed & !0x1ffff != 0 {
            return Err(Error::new(
                ErrorKind::NonCanonical,
                "chunkbroken-reserved-bits",
                "ChunkBroken coordinate uses reserved high bits.",
            ));
        }
        identities.insert(packed);
    }
    let deleted = identities
        .into_iter()
        .map(|packed| Coord {
            x: (packed & 0x0f) as u16,
            z: ((packed >> 4) & 0x0f) as u16,
            y: ((packed >> 8) & 0x01ff) as u16,
        })
        .collect::<Vec<_>>();

    Ok(ImportedAsset {
        profile: Profile::TerrainDelta,
        format: IncumbentFormat::ChunkBrokenV1,
        incumbent_encoding: input.to_vec(),
        semantics: Semantics::TerrainDelta(TerrainSemantics { min_y, deleted }),
    })
}

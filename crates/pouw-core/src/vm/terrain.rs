use alloc::collections::BTreeSet;
use alloc::vec;
use alloc::vec::Vec;

use crate::error::{Error, ErrorKind, Result};
use crate::model::{
    terrain_coord_from_id, terrain_coord_id, LimitsV1, Semantics, TerrainSemantics, TERRAIN_SIZE_X,
    TERRAIN_SIZE_Y, TERRAIN_SIZE_Z, TERRAIN_UNIVERSE,
};
use crate::varint::{write_i16, write_u32};

use super::{
    checked_u16, ensure_count, read_bool_bits, write_bool_bits, Cursor, StatsBuilder, TerrainOp,
    TerrainPatchKind, TerrainProgram, VmStats,
};

const DELETE_RUN: u8 = 1;
const DELETE_BOX: u8 = 2;
const LAYER_BITMAP: u8 = 3;
const ELIAS_FANO: u8 = 4;
const PATCH_ADD: u8 = 0x80;
const PATCH_RESTORE: u8 = 0x81;

pub(super) fn encode(
    output: &mut Vec<u8>,
    program: &TerrainProgram,
    limits: &LimitsV1,
) -> Result<()> {
    ensure_count(
        program.ops.len() as u32,
        limits.max_commands,
        "terrain-command-limit",
    )?;
    ensure_count(
        program.patches.len() as u32,
        limits.max_patches,
        "terrain-patch-limit",
    )?;
    write_i16(output, program.min_y);
    write_u32(output, program.ops.len() as u32);
    for op in &program.ops {
        match op {
            TerrainOp::DeleteRun { start, length } => {
                output.push(DELETE_RUN);
                write_u32(output, *start);
                write_u32(output, *length);
            }
            TerrainOp::DeleteBox {
                x,
                y,
                z,
                width,
                height,
                depth,
            } => {
                output.push(DELETE_BOX);
                for value in [*x, *y, *z, *width, *height, *depth] {
                    write_u32(output, u32::from(value));
                }
            }
            TerrainOp::LayerBitmap { y, bitmap } => {
                output.push(LAYER_BITMAP);
                write_u32(output, u32::from(*y));
                output.extend_from_slice(bitmap);
            }
            TerrainOp::EliasFano { values } => encode_elias_fano(output, values)?,
        }
    }
    write_u32(output, program.patches.len() as u32);
    let mut previous = 0_u32;
    for (index, patch) in program.patches.iter().enumerate() {
        output.push(match patch.kind {
            TerrainPatchKind::Add => PATCH_ADD,
            TerrainPatchKind::Restore => PATCH_RESTORE,
        });
        let delta = if index == 0 {
            patch.id
        } else {
            patch
                .id
                .checked_sub(previous)
                .and_then(|value| value.checked_sub(1))
                .ok_or_else(|| {
                    Error::new(
                        ErrorKind::NonCanonical,
                        "terrain-patch-order",
                        "Terrain patches must be strictly coordinate sorted.",
                    )
                })?
        };
        write_u32(output, delta);
        previous = patch.id;
    }
    Ok(())
}

fn encode_elias_fano(output: &mut Vec<u8>, values: &[u32]) -> Result<()> {
    if values.is_empty() {
        return Err(Error::new(
            ErrorKind::NonCanonical,
            "terrain-elias-fano-empty",
            "Elias-Fano operations cannot encode an empty set.",
        ));
    }
    let mut previous = None;
    for value in values {
        if *value >= TERRAIN_UNIVERSE || previous.is_some_and(|item| item >= *value) {
            return Err(Error::new(
                ErrorKind::NonCanonical,
                "terrain-elias-fano-order",
                "Elias-Fano values must be strictly sorted inside the terrain universe.",
            ));
        }
        previous = Some(*value);
    }
    output.push(ELIAS_FANO);
    write_u32(output, values.len() as u32);
    let low_bits = elias_low_bits(values.len() as u32);
    output.push(low_bits);
    let low_mask = if low_bits == 0 {
        0
    } else {
        (1_u32 << low_bits) - 1
    };
    let mut lows = Vec::with_capacity(values.len() * usize::from(low_bits));
    for value in values {
        for bit in (0..low_bits).rev() {
            lows.push((value & low_mask) & (1 << bit) != 0);
        }
    }
    write_bool_bits(output, &lows);
    let high_length = elias_high_length(low_bits, values.len() as u32)?;
    let mut highs = vec![false; high_length];
    for (index, value) in values.iter().enumerate() {
        let position = (*value >> low_bits) as usize + index;
        highs[position] = true;
    }
    write_bool_bits(output, &highs);
    Ok(())
}

pub(super) fn decode(cursor: &mut Cursor<'_>, limits: &LimitsV1) -> Result<(Semantics, VmStats)> {
    let min_y = cursor.i16()?;
    let mut deleted = BTreeSet::new();
    let mut stats = StatsBuilder::default();
    let program_start = cursor.offset;
    let command_count = cursor.u32()?;
    ensure_count(command_count, limits.max_commands, "terrain-command-limit")?;
    for _ in 0..command_count {
        match cursor.byte("terrain opcode")? {
            DELETE_RUN => decode_run(cursor, &mut deleted, &mut stats, limits)?,
            DELETE_BOX => decode_box(cursor, &mut deleted, &mut stats, limits)?,
            LAYER_BITMAP => decode_layer(cursor, &mut deleted, &mut stats, limits)?,
            ELIAS_FANO => decode_elias_fano(cursor, &mut deleted, &mut stats, limits)?,
            _ => {
                return Err(Error::new(
                    ErrorKind::UnknownOpcode,
                    "terrain-opcode",
                    "Terrain VM contains an unknown opcode.",
                ))
            }
        }
        check_resource_stats(&stats, limits)?;
    }
    stats.program_bytes = byte_delta(program_start, cursor.offset)?;

    let residual_start = cursor.offset;
    let patch_count = cursor.u32()?;
    ensure_count(patch_count, limits.max_patches, "terrain-patch-limit")?;
    let mut previous = 0_u32;
    for index in 0..patch_count {
        let opcode = cursor.byte("terrain patch opcode")?;
        let delta = cursor.u32()?;
        let id = if index == 0 {
            delta
        } else {
            previous
                .checked_add(delta)
                .and_then(|value| value.checked_add(1))
                .ok_or_else(|| Error::overflow("Terrain patch coordinate overflow."))?
        };
        if id >= TERRAIN_UNIVERSE || (index > 0 && id <= previous) {
            return Err(Error::new(
                ErrorKind::NonCanonical,
                "terrain-patch-order",
                "Terrain patches must be strictly sorted within the terrain universe.",
            ));
        }
        match opcode {
            PATCH_ADD => {
                if !deleted.insert(id) {
                    return Err(noop_patch());
                }
            }
            PATCH_RESTORE => {
                if !deleted.remove(&id) {
                    return Err(noop_patch());
                }
            }
            _ => {
                return Err(Error::new(
                    ErrorKind::UnknownOpcode,
                    "terrain-patch-opcode",
                    "Terrain residual contains an unknown patch opcode.",
                ))
            }
        }
        stats.add_patch(1, 4)?;
        check_resource_stats(&stats, limits)?;
        previous = id;
    }
    stats.residual_bytes = byte_delta(residual_start, cursor.offset)?;
    let deleted = deleted
        .into_iter()
        .map(terrain_coord_from_id)
        .collect::<Result<Vec<_>>>()?;
    Ok((
        Semantics::TerrainDelta(TerrainSemantics { min_y, deleted }),
        stats.finish(),
    ))
}

fn decode_run(
    cursor: &mut Cursor<'_>,
    deleted: &mut BTreeSet<u32>,
    stats: &mut StatsBuilder,
    limits: &LimitsV1,
) -> Result<()> {
    let start = cursor.u32()?;
    let length = cursor.u32()?;
    let end = start
        .checked_add(length)
        .ok_or_else(|| Error::overflow("Terrain delete run overflow."))?;
    if length == 0 || end > TERRAIN_UNIVERSE {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "terrain-delete-run",
            "Terrain delete run is empty or outside the terrain universe.",
        ));
    }
    ensure_expansion(length, limits)?;
    deleted.extend(start..end);
    stats.add_command(length, 4)
}

fn decode_box(
    cursor: &mut Cursor<'_>,
    deleted: &mut BTreeSet<u32>,
    stats: &mut StatsBuilder,
    limits: &LimitsV1,
) -> Result<()> {
    let x = checked_u16(cursor.u32()?, "Terrain box x exceeds u16.")?;
    let y = checked_u16(cursor.u32()?, "Terrain box y exceeds u16.")?;
    let z = checked_u16(cursor.u32()?, "Terrain box z exceeds u16.")?;
    let width = checked_u16(cursor.u32()?, "Terrain box width exceeds u16.")?;
    let height = checked_u16(cursor.u32()?, "Terrain box height exceeds u16.")?;
    let depth = checked_u16(cursor.u32()?, "Terrain box depth exceeds u16.")?;
    if width == 0
        || height == 0
        || depth == 0
        || x.checked_add(width).is_none_or(|end| end > TERRAIN_SIZE_X)
        || y.checked_add(height).is_none_or(|end| end > TERRAIN_SIZE_Y)
        || z.checked_add(depth).is_none_or(|end| end > TERRAIN_SIZE_Z)
    {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "terrain-delete-box",
            "Terrain delete box is empty or outside the terrain universe.",
        ));
    }
    let writes = u32::from(width)
        .checked_mul(u32::from(height))
        .and_then(|value| value.checked_mul(u32::from(depth)))
        .ok_or_else(|| Error::overflow("Terrain delete box volume overflow."))?;
    ensure_expansion(writes, limits)?;
    for cy in y..y + height {
        for cz in z..z + depth {
            for cx in x..x + width {
                deleted.insert(terrain_coord_id(crate::model::Coord {
                    x: cx,
                    y: cy,
                    z: cz,
                }));
            }
        }
    }
    stats.add_command(writes, 8)
}

fn decode_layer(
    cursor: &mut Cursor<'_>,
    deleted: &mut BTreeSet<u32>,
    stats: &mut StatsBuilder,
    limits: &LimitsV1,
) -> Result<()> {
    let y = checked_u16(cursor.u32()?, "Terrain layer y exceeds u16.")?;
    if y >= TERRAIN_SIZE_Y {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "terrain-layer",
            "Terrain layer bitmap is outside the terrain universe.",
        ));
    }
    let bitmap = cursor.bytes(32, "terrain layer bitmap")?;
    let writes = bitmap.iter().map(|byte| byte.count_ones()).sum::<u32>();
    ensure_expansion(writes, limits)?;
    for index in 0..256_usize {
        if bitmap[index / 8] & (1 << (7 - index % 8)) != 0 {
            let x = (index % 16) as u16;
            let z = (index / 16) as u16;
            deleted.insert(terrain_coord_id(crate::model::Coord { x, y, z }));
        }
    }
    stats.add_command(writes, 12 + 32)
}

fn decode_elias_fano(
    cursor: &mut Cursor<'_>,
    deleted: &mut BTreeSet<u32>,
    stats: &mut StatsBuilder,
    limits: &LimitsV1,
) -> Result<()> {
    let count = cursor.u32()?;
    if count == 0 || count > limits.max_expanded_per_op || count > limits.max_voxels {
        return Err(Error::limit(
            "terrain-elias-fano-count",
            "Elias-Fano count is empty or exceeds task limits.",
        ));
    }
    let low_bits = cursor.byte("terrain Elias-Fano low bits")?;
    if low_bits != elias_low_bits(count) {
        return Err(Error::new(
            ErrorKind::NonCanonical,
            "terrain-elias-fano-low-bits",
            "Elias-Fano low-bit width is not canonical for this universe and count.",
        ));
    }
    let low_length = usize::try_from(count)
        .ok()
        .and_then(|value| value.checked_mul(usize::from(low_bits)))
        .ok_or_else(|| Error::overflow("Elias-Fano low-bit length overflow."))?;
    let lows = read_bool_bits(cursor, low_length, "terrain Elias-Fano low bits")?;
    let high_length = elias_high_length(low_bits, count)?;
    let highs = read_bool_bits(cursor, high_length, "terrain Elias-Fano high bits")?;
    let mut values = Vec::with_capacity(count as usize);
    for (position, set) in highs.iter().enumerate() {
        if !set {
            continue;
        }
        if values.len() >= count as usize {
            return Err(Error::new(
                ErrorKind::NonCanonical,
                "terrain-elias-fano-high-bits",
                "Elias-Fano high bitmap contains too many set bits.",
            ));
        }
        let index = values.len();
        let high = position.checked_sub(index).ok_or_else(|| {
            Error::new(
                ErrorKind::NonCanonical,
                "terrain-elias-fano-high-bits",
                "Invalid Elias-Fano high bitmap.",
            )
        })?;
        let mut low = 0_u32;
        for bit in 0..usize::from(low_bits) {
            low = (low << 1) | u32::from(lows[index * usize::from(low_bits) + bit]);
        }
        let value = (u32::try_from(high)
            .map_err(|_| Error::overflow("Elias-Fano high value overflow."))?
            << low_bits)
            | low;
        if value >= TERRAIN_UNIVERSE || values.last().is_some_and(|previous| *previous >= value) {
            return Err(Error::new(
                ErrorKind::NonCanonical,
                "terrain-elias-fano-order",
                "Decoded Elias-Fano values are not strictly sorted in range.",
            ));
        }
        values.push(value);
    }
    if values.len() != count as usize {
        return Err(Error::new(
            ErrorKind::Truncated,
            "terrain-elias-fano-high-bits",
            "Elias-Fano high bitmap contains too few set bits.",
        ));
    }
    deleted.extend(values);
    stats.add_command(count, 20 + high_length as u64)
}

fn elias_low_bits(count: u32) -> u8 {
    let quotient = TERRAIN_UNIVERSE / count.max(1);
    if quotient <= 1 {
        return 0;
    }
    (31 - quotient.leading_zeros()) as u8
}

fn elias_high_length(low_bits: u8, count: u32) -> Result<usize> {
    usize::try_from(((TERRAIN_UNIVERSE - 1) >> low_bits) + count + 1)
        .map_err(|_| Error::overflow("Elias-Fano high-bit length overflow."))
}

fn ensure_expansion(writes: u32, limits: &LimitsV1) -> Result<()> {
    if writes > limits.max_expanded_per_op {
        return Err(Error::limit(
            "terrain-expansion-limit",
            "Terrain opcode exceeds the per-operation expansion limit.",
        ));
    }
    Ok(())
}

fn check_resource_stats(stats: &StatsBuilder, limits: &LimitsV1) -> Result<()> {
    if stats.writes > limits.max_writes || stats.decode_units > limits.max_decode_units {
        return Err(Error::limit(
            "terrain-resource-limit",
            "Terrain VM expansion exceeds task limits.",
        ));
    }
    Ok(())
}

fn byte_delta(start: usize, end: usize) -> Result<u32> {
    u32::try_from(end.saturating_sub(start))
        .map_err(|_| Error::overflow("Terrain VM byte accounting overflow."))
}

fn noop_patch() -> Error {
    Error::new(
        ErrorKind::NonCanonical,
        "terrain-noop-patch",
        "Terrain residual patches must change the program output.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::TerrainPatch;
    use crate::vm::{decode_candidate, encode_candidate, CandidateProgram};
    use crate::Profile;

    #[test]
    fn elias_fano_round_trip_is_exact() {
        let limits = LimitsV1::default();
        let values = vec![0, 1, 255, 256, 40_000, TERRAIN_UNIVERSE - 1];
        let program = CandidateProgram::TerrainDelta(TerrainProgram {
            min_y: -64,
            ops: vec![TerrainOp::EliasFano {
                values: values.clone(),
            }],
            patches: vec![],
        });
        let bytes = encode_candidate(&program, &limits).unwrap();
        let decoded = decode_candidate(&bytes, Profile::TerrainDelta, &limits).unwrap();
        let Semantics::TerrainDelta(terrain) = decoded.semantics else {
            panic!("wrong profile")
        };
        assert_eq!(
            terrain
                .deleted
                .iter()
                .copied()
                .map(terrain_coord_id)
                .collect::<Vec<_>>(),
            values
        );
    }

    #[test]
    fn residual_rejects_noops() {
        let limits = LimitsV1::default();
        let program = CandidateProgram::TerrainDelta(TerrainProgram {
            min_y: 0,
            ops: vec![TerrainOp::DeleteRun {
                start: 0,
                length: 2,
            }],
            patches: vec![TerrainPatch {
                id: 1,
                kind: TerrainPatchKind::Add,
            }],
        });
        assert!(encode_candidate(&program, &limits).is_err());
    }
}

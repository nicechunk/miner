use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::error::{Error, ErrorKind, Result};
use crate::model::{
    building_coord_from_id, building_coord_id, BuildingSemantics, LimitsV1, Semantics, Voxel,
};
use crate::varint::{write_i32, write_u32};

use super::{
    checked_u16, checked_volume, ensure_count, read_bool_bits, write_bool_bits, BuildingOp,
    BuildingPatchKind, BuildingProgram, Cursor, StatsBuilder, VmStats,
};

const BOX: u8 = 1;
const RUN: u8 = 2;
const WALL: u8 = 3;
const EXTRUDE: u8 = 4;
const REPEAT: u8 = 5;
const MIRROR: u8 = 6;
const CUT: u8 = 7;
const LITERAL: u8 = 8;
const PATCH_SET: u8 = 0x80;
const PATCH_CLEAR: u8 = 0x81;
const PATCH_PAINT: u8 = 0x82;

pub(super) fn encode(
    output: &mut Vec<u8>,
    program: &BuildingProgram,
    limits: &LimitsV1,
) -> Result<()> {
    ensure_count(
        program.ops.len() as u32,
        limits.max_commands,
        "building-command-limit",
    )?;
    ensure_count(
        program.patches.len() as u32,
        limits.max_patches,
        "building-patch-limit",
    )?;
    for size in program.size {
        write_u32(output, u32::from(size));
    }
    write_u32(output, program.ops.len() as u32);
    for op in &program.ops {
        match op {
            BuildingOp::Box {
                material,
                origin,
                size,
            } => {
                output.push(BOX);
                write_material(output, *material);
                write_vec3_u16(output, *origin);
                write_vec3_u16(output, *size);
            }
            BuildingOp::Run {
                material,
                origin,
                axis,
                length,
            } => {
                output.push(RUN);
                write_material(output, *material);
                write_vec3_u16(output, *origin);
                output.push(*axis);
                write_u32(output, u32::from(*length));
            }
            BuildingOp::Wall {
                material,
                origin,
                normal_axis,
                u_length,
                v_length,
                thickness,
            } => {
                output.push(WALL);
                write_material(output, *material);
                write_vec3_u16(output, *origin);
                output.push(*normal_axis);
                for value in [*u_length, *v_length, *thickness] {
                    write_u32(output, u32::from(value));
                }
            }
            BuildingOp::Extrude {
                material,
                origin,
                axis,
                u_length,
                v_length,
                depth,
                mask,
            } => {
                output.push(EXTRUDE);
                write_material(output, *material);
                write_vec3_u16(output, *origin);
                output.push(*axis);
                for value in [*u_length, *v_length, *depth] {
                    write_u32(output, u32::from(value));
                }
                let expected = usize::from(*u_length)
                    .checked_mul(usize::from(*v_length))
                    .ok_or_else(|| Error::overflow("Building extrude mask length overflow."))?;
                if mask.len() != expected {
                    return Err(Error::invalid(
                        "building-extrude-mask",
                        "Building extrude mask length does not match its dimensions.",
                    ));
                }
                write_bool_bits(output, mask);
            }
            BuildingOp::Repeat {
                material,
                origin,
                size,
                count,
                delta,
            } => {
                output.push(REPEAT);
                write_material(output, *material);
                write_vec3_u16(output, *origin);
                write_vec3_u16(output, *size);
                write_u32(output, u32::from(*count));
                for value in delta {
                    write_i32(output, *value);
                }
            }
            BuildingOp::Mirror {
                source_origin,
                source_size,
                axis,
                pivot_twice,
            } => {
                output.push(MIRROR);
                write_vec3_u16(output, *source_origin);
                write_vec3_u16(output, *source_size);
                output.push(*axis);
                write_i32(output, *pivot_twice);
            }
            BuildingOp::Cut { origin, size } => {
                output.push(CUT);
                write_vec3_u16(output, *origin);
                write_vec3_u16(output, *size);
            }
            BuildingOp::Literal { voxels } => {
                output.push(LITERAL);
                write_u32(output, voxels.len() as u32);
                let mut previous = 0_u32;
                for (index, voxel) in voxels.iter().enumerate() {
                    let id = building_coord_id(program.size, *voxel);
                    let delta = if index == 0 {
                        id
                    } else {
                        id.checked_sub(previous)
                            .and_then(|value| value.checked_sub(1))
                            .ok_or_else(|| {
                                Error::new(
                                    ErrorKind::NonCanonical,
                                    "building-literal-order",
                                    "Literal voxels must be strictly coordinate sorted.",
                                )
                            })?
                    };
                    write_u32(output, delta);
                    write_material(output, voxel.material);
                    previous = id;
                }
            }
        }
    }
    write_u32(output, program.patches.len() as u32);
    let mut previous = 0_u32;
    for (index, patch) in program.patches.iter().enumerate() {
        output.push(match patch.kind {
            BuildingPatchKind::Set => PATCH_SET,
            BuildingPatchKind::Clear => PATCH_CLEAR,
            BuildingPatchKind::Paint => PATCH_PAINT,
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
                        "building-patch-order",
                        "Building patches must be strictly coordinate sorted.",
                    )
                })?
        };
        write_u32(output, delta);
        write_u32(output, u32::from(patch.material));
        previous = patch.id;
    }
    Ok(())
}

pub(super) fn decode(cursor: &mut Cursor<'_>, limits: &LimitsV1) -> Result<(Semantics, VmStats)> {
    let size = [
        checked_u16(cursor.u32()?, "Building x dimension exceeds u16.")?,
        checked_u16(cursor.u32()?, "Building y dimension exceeds u16.")?,
        checked_u16(cursor.u32()?, "Building z dimension exceeds u16.")?,
    ];
    if size.iter().any(|value| *value == 0 || *value > 512) {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "building-dimensions",
            "Building dimensions must be in 1..=512.",
        ));
    }
    let volume = checked_volume(size)?;
    let mut voxels = BTreeMap::<u32, u16>::new();
    let mut stats = StatsBuilder::default();
    let program_start = cursor.offset;
    let command_count = cursor.u32()?;
    ensure_count(command_count, limits.max_commands, "building-command-limit")?;
    for _ in 0..command_count {
        match cursor.byte("building opcode")? {
            BOX => decode_box(cursor, size, &mut voxels, &mut stats, limits)?,
            RUN => decode_run(cursor, size, &mut voxels, &mut stats, limits)?,
            WALL => decode_wall(cursor, size, &mut voxels, &mut stats, limits)?,
            EXTRUDE => decode_extrude(cursor, size, &mut voxels, &mut stats, limits)?,
            REPEAT => decode_repeat(cursor, size, &mut voxels, &mut stats, limits)?,
            MIRROR => decode_mirror(cursor, size, &mut voxels, &mut stats, limits)?,
            CUT => decode_cut(cursor, size, &mut voxels, &mut stats, limits)?,
            LITERAL => decode_literal(cursor, size, &mut voxels, &mut stats, limits)?,
            _ => {
                return Err(Error::new(
                    ErrorKind::UnknownOpcode,
                    "building-opcode",
                    "Building VM contains an unknown opcode.",
                ))
            }
        }
        check_resources(&stats, &voxels, limits)?;
    }
    stats.program_bytes = byte_delta(program_start, cursor.offset)?;

    let residual_start = cursor.offset;
    let patch_count = cursor.u32()?;
    ensure_count(patch_count, limits.max_patches, "building-patch-limit")?;
    let mut previous = 0_u32;
    for index in 0..patch_count {
        let opcode = cursor.byte("building patch opcode")?;
        let delta = cursor.u32()?;
        let id = if index == 0 {
            delta
        } else {
            previous
                .checked_add(delta)
                .and_then(|value| value.checked_add(1))
                .ok_or_else(|| Error::overflow("Building patch coordinate overflow."))?
        };
        if id >= volume || (index > 0 && id <= previous) {
            return Err(Error::new(
                ErrorKind::NonCanonical,
                "building-patch-order",
                "Building patches must be strictly sorted inside declared dimensions.",
            ));
        }
        let material = checked_u16(cursor.u32()?, "Building patch material exceeds u16.")?;
        match opcode {
            PATCH_SET => {
                if material == 0 || voxels.insert(id, material).is_some() {
                    return Err(noop_patch(
                        "SET requires an empty voxel and non-zero material.",
                    ));
                }
            }
            PATCH_CLEAR => {
                if material != 0 || voxels.remove(&id).is_none() {
                    return Err(noop_patch(
                        "CLEAR requires an occupied voxel and zero material.",
                    ));
                }
            }
            PATCH_PAINT => {
                if material == 0 {
                    return Err(noop_patch("PAINT requires a non-zero material."));
                }
                let current = voxels
                    .get_mut(&id)
                    .ok_or_else(|| noop_patch("PAINT requires an occupied voxel."))?;
                if *current == material {
                    return Err(noop_patch("PAINT must change the material."));
                }
                *current = material;
            }
            _ => {
                return Err(Error::new(
                    ErrorKind::UnknownOpcode,
                    "building-patch-opcode",
                    "Building residual contains an unknown patch opcode.",
                ))
            }
        }
        stats.add_patch(1, 5)?;
        check_resources(&stats, &voxels, limits)?;
        previous = id;
    }
    stats.residual_bytes = byte_delta(residual_start, cursor.offset)?;
    let output = voxels
        .into_iter()
        .map(|(id, material)| building_coord_from_id(size, id, material))
        .collect::<Result<Vec<_>>>()?;
    Ok((
        Semantics::Building(BuildingSemantics {
            size,
            voxels: output,
        }),
        stats.finish(),
    ))
}

fn decode_box(
    cursor: &mut Cursor<'_>,
    bounds: [u16; 3],
    voxels: &mut BTreeMap<u32, u16>,
    stats: &mut StatsBuilder,
    limits: &LimitsV1,
) -> Result<()> {
    let material = read_material(cursor)?;
    let origin = read_vec3(cursor)?;
    let size = read_vec3(cursor)?;
    let writes = write_box(voxels, bounds, material, origin, size, false)?;
    ensure_expansion(writes, limits)?;
    stats.add_command(writes, 8)
}

fn decode_run(
    cursor: &mut Cursor<'_>,
    bounds: [u16; 3],
    voxels: &mut BTreeMap<u32, u16>,
    stats: &mut StatsBuilder,
    limits: &LimitsV1,
) -> Result<()> {
    let material = read_material(cursor)?;
    let origin = read_vec3(cursor)?;
    let axis = read_axis(cursor)?;
    let length = checked_u16(cursor.u32()?, "Building run length exceeds u16.")?;
    let mut size = [1_u16; 3];
    size[axis] = length;
    let writes = write_box(voxels, bounds, material, origin, size, false)?;
    ensure_expansion(writes, limits)?;
    stats.add_command(writes, 5)
}

fn decode_wall(
    cursor: &mut Cursor<'_>,
    bounds: [u16; 3],
    voxels: &mut BTreeMap<u32, u16>,
    stats: &mut StatsBuilder,
    limits: &LimitsV1,
) -> Result<()> {
    let material = read_material(cursor)?;
    let origin = read_vec3(cursor)?;
    let axis = read_axis(cursor)?;
    let u_length = checked_u16(cursor.u32()?, "Building wall length exceeds u16.")?;
    let v_length = checked_u16(cursor.u32()?, "Building wall height exceeds u16.")?;
    let thickness = checked_u16(cursor.u32()?, "Building wall thickness exceeds u16.")?;
    let tangent = tangent_axes(axis);
    let mut size = [1_u16; 3];
    size[axis] = thickness;
    size[tangent[0]] = u_length;
    size[tangent[1]] = v_length;
    let writes = write_box(voxels, bounds, material, origin, size, false)?;
    ensure_expansion(writes, limits)?;
    stats.add_command(writes, 7)
}

fn decode_extrude(
    cursor: &mut Cursor<'_>,
    bounds: [u16; 3],
    voxels: &mut BTreeMap<u32, u16>,
    stats: &mut StatsBuilder,
    limits: &LimitsV1,
) -> Result<()> {
    let material = read_material(cursor)?;
    let origin = read_vec3(cursor)?;
    let axis = read_axis(cursor)?;
    let u_length = checked_u16(cursor.u32()?, "Building extrude u length exceeds u16.")?;
    let v_length = checked_u16(cursor.u32()?, "Building extrude v length exceeds u16.")?;
    let depth = checked_u16(cursor.u32()?, "Building extrude depth exceeds u16.")?;
    if u_length == 0 || v_length == 0 || depth == 0 {
        return Err(empty_geometry());
    }
    let mask_length = usize::from(u_length)
        .checked_mul(usize::from(v_length))
        .ok_or_else(|| Error::overflow("Building extrude mask length overflow."))?;
    let mask = read_bool_bits(cursor, mask_length, "building extrude mask")?;
    let selected = mask.iter().filter(|value| **value).count() as u32;
    if selected == 0 {
        return Err(empty_geometry());
    }
    let writes = selected
        .checked_mul(u32::from(depth))
        .ok_or_else(|| Error::overflow("Building extrude expansion overflow."))?;
    ensure_expansion(writes, limits)?;
    let tangent = tangent_axes(axis);
    for v in 0..v_length {
        for u in 0..u_length {
            if !mask[usize::from(u) + usize::from(u_length) * usize::from(v)] {
                continue;
            }
            for layer in 0..depth {
                let mut coordinate = origin;
                coordinate[axis] = coordinate[axis]
                    .checked_add(layer)
                    .ok_or_else(|| Error::overflow("Building extrude coordinate overflow."))?;
                coordinate[tangent[0]] = coordinate[tangent[0]]
                    .checked_add(u)
                    .ok_or_else(|| Error::overflow("Building extrude coordinate overflow."))?;
                coordinate[tangent[1]] = coordinate[tangent[1]]
                    .checked_add(v)
                    .ok_or_else(|| Error::overflow("Building extrude coordinate overflow."))?;
                write_voxel(voxels, bounds, coordinate, material)?;
            }
        }
    }
    stats.add_command(writes, 12 + mask_length as u64)
}

fn decode_repeat(
    cursor: &mut Cursor<'_>,
    bounds: [u16; 3],
    voxels: &mut BTreeMap<u32, u16>,
    stats: &mut StatsBuilder,
    limits: &LimitsV1,
) -> Result<()> {
    let material = read_material(cursor)?;
    let origin = read_vec3(cursor)?;
    let size = read_vec3(cursor)?;
    let count = checked_u16(cursor.u32()?, "Building repeat count exceeds u16.")?;
    let delta = [cursor.i32()?, cursor.i32()?, cursor.i32()?];
    if count == 0 || count > 512 {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "building-repeat-count",
            "Building repeat count must be in 1..=512.",
        ));
    }
    let per_box = checked_volume(size)?;
    let writes = per_box
        .checked_mul(u32::from(count))
        .ok_or_else(|| Error::overflow("Building repeat expansion overflow."))?;
    ensure_expansion(writes, limits)?;
    for index in 0..count {
        let mut repeated = [0_u16; 3];
        for axis in 0..3 {
            let value = i64::from(origin[axis])
                .checked_add(
                    i64::from(delta[axis])
                        .checked_mul(i64::from(index))
                        .ok_or_else(|| Error::overflow("Building repeat coordinate overflow."))?,
                )
                .ok_or_else(|| Error::overflow("Building repeat coordinate overflow."))?;
            repeated[axis] = u16::try_from(value).map_err(|_| {
                Error::new(
                    ErrorKind::OutOfBounds,
                    "building-repeat-coordinate",
                    "Building repeat produces a negative or oversized coordinate.",
                )
            })?;
        }
        write_box(voxels, bounds, material, repeated, size, false)?;
    }
    stats.add_command(writes, 12 + u64::from(count))
}

fn decode_mirror(
    cursor: &mut Cursor<'_>,
    bounds: [u16; 3],
    voxels: &mut BTreeMap<u32, u16>,
    stats: &mut StatsBuilder,
    limits: &LimitsV1,
) -> Result<()> {
    let source_origin = read_vec3(cursor)?;
    let source_size = read_vec3(cursor)?;
    ensure_box(bounds, source_origin, source_size)?;
    let axis = read_axis(cursor)?;
    let pivot_twice = cursor.i32()?;
    let mut copied = Vec::new();
    for (id, material) in voxels.iter() {
        let voxel = building_coord_from_id(bounds, *id, *material)?;
        let coordinate = [voxel.x, voxel.y, voxel.z];
        if (0..3).all(|item| {
            coordinate[item] >= source_origin[item]
                && coordinate[item] < source_origin[item] + source_size[item]
        }) {
            let reflected = i64::from(pivot_twice) - i64::from(coordinate[axis]);
            let mut target = coordinate;
            target[axis] = u16::try_from(reflected).map_err(|_| {
                Error::new(
                    ErrorKind::OutOfBounds,
                    "building-mirror-coordinate",
                    "Building mirror produces a negative or oversized coordinate.",
                )
            })?;
            if target[axis] >= bounds[axis] {
                return Err(Error::new(
                    ErrorKind::OutOfBounds,
                    "building-mirror-coordinate",
                    "Building mirror produces a coordinate outside declared dimensions.",
                ));
            }
            copied.push((target, *material));
        }
    }
    let writes = copied.len() as u32;
    if writes == 0 {
        return Err(empty_geometry());
    }
    ensure_expansion(writes, limits)?;
    for (coordinate, material) in copied {
        write_voxel(voxels, bounds, coordinate, material)?;
    }
    stats.add_command(writes, 16 + u64::from(writes))
}

fn decode_cut(
    cursor: &mut Cursor<'_>,
    bounds: [u16; 3],
    voxels: &mut BTreeMap<u32, u16>,
    stats: &mut StatsBuilder,
    limits: &LimitsV1,
) -> Result<()> {
    let origin = read_vec3(cursor)?;
    let size = read_vec3(cursor)?;
    let writes = write_box(voxels, bounds, 0, origin, size, true)?;
    ensure_expansion(writes, limits)?;
    stats.add_command(writes, 7)
}

fn decode_literal(
    cursor: &mut Cursor<'_>,
    bounds: [u16; 3],
    voxels: &mut BTreeMap<u32, u16>,
    stats: &mut StatsBuilder,
    limits: &LimitsV1,
) -> Result<()> {
    let count = cursor.u32()?;
    if count == 0 || count > limits.max_expanded_per_op || count > limits.max_voxels {
        return Err(Error::limit(
            "building-literal-count",
            "Building literal count is empty or exceeds task limits.",
        ));
    }
    let volume = checked_volume(bounds)?;
    let mut previous = 0_u32;
    for index in 0..count {
        let delta = cursor.u32()?;
        let id = if index == 0 {
            delta
        } else {
            previous
                .checked_add(delta)
                .and_then(|value| value.checked_add(1))
                .ok_or_else(|| Error::overflow("Building literal coordinate overflow."))?
        };
        if id >= volume || (index > 0 && id <= previous) {
            return Err(Error::new(
                ErrorKind::NonCanonical,
                "building-literal-order",
                "Building literal coordinates must be strictly sorted in range.",
            ));
        }
        let material = read_material(cursor)?;
        voxels.insert(id, material);
        previous = id;
    }
    stats.add_command(count, 8 + u64::from(count))
}

fn write_box(
    voxels: &mut BTreeMap<u32, u16>,
    bounds: [u16; 3],
    material: u16,
    origin: [u16; 3],
    size: [u16; 3],
    clear: bool,
) -> Result<u32> {
    ensure_box(bounds, origin, size)?;
    let writes = checked_volume(size)?;
    for y in origin[1]..origin[1] + size[1] {
        for z in origin[2]..origin[2] + size[2] {
            for x in origin[0]..origin[0] + size[0] {
                let voxel = Voxel { x, y, z, material };
                let id = building_coord_id(bounds, voxel);
                if clear {
                    voxels.remove(&id);
                } else {
                    voxels.insert(id, material);
                }
            }
        }
    }
    Ok(writes)
}

fn write_voxel(
    voxels: &mut BTreeMap<u32, u16>,
    bounds: [u16; 3],
    coordinate: [u16; 3],
    material: u16,
) -> Result<()> {
    if coordinate
        .iter()
        .zip(bounds)
        .any(|(value, bound)| *value >= bound)
    {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "building-coordinate",
            "Building VM writes outside declared dimensions.",
        ));
    }
    let voxel = Voxel {
        x: coordinate[0],
        y: coordinate[1],
        z: coordinate[2],
        material,
    };
    voxels.insert(building_coord_id(bounds, voxel), material);
    Ok(())
}

fn ensure_box(bounds: [u16; 3], origin: [u16; 3], size: [u16; 3]) -> Result<()> {
    if size.contains(&0)
        || (0..3).any(|axis| {
            origin[axis]
                .checked_add(size[axis])
                .is_none_or(|end| end > bounds[axis])
        })
    {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "building-box",
            "Building cuboid is empty or outside declared dimensions.",
        ));
    }
    Ok(())
}

fn read_material(cursor: &mut Cursor<'_>) -> Result<u16> {
    let material = checked_u16(cursor.u32()?, "Building material exceeds u16.")?;
    if material == 0 {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "building-material",
            "Building program material zero is reserved for air.",
        ));
    }
    Ok(material)
}

fn read_vec3(cursor: &mut Cursor<'_>) -> Result<[u16; 3]> {
    Ok([
        checked_u16(cursor.u32()?, "Building vector x exceeds u16.")?,
        checked_u16(cursor.u32()?, "Building vector y exceeds u16.")?,
        checked_u16(cursor.u32()?, "Building vector z exceeds u16.")?,
    ])
}

fn read_axis(cursor: &mut Cursor<'_>) -> Result<usize> {
    let axis = cursor.byte("building axis")? as usize;
    if axis > 2 {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "building-axis",
            "Building VM axis must be 0..=2.",
        ));
    }
    Ok(axis)
}

fn tangent_axes(axis: usize) -> [usize; 2] {
    match axis {
        0 => [1, 2],
        1 => [0, 2],
        2 => [0, 1],
        _ => unreachable!(),
    }
}

fn write_material(output: &mut Vec<u8>, value: u16) {
    write_u32(output, u32::from(value));
}

fn write_vec3_u16(output: &mut Vec<u8>, values: [u16; 3]) {
    for value in values {
        write_u32(output, u32::from(value));
    }
}

fn ensure_expansion(writes: u32, limits: &LimitsV1) -> Result<()> {
    if writes == 0 || writes > limits.max_expanded_per_op {
        return Err(Error::limit(
            "building-expansion-limit",
            "Building opcode is empty or exceeds the per-operation expansion limit.",
        ));
    }
    Ok(())
}

fn check_resources(
    stats: &StatsBuilder,
    voxels: &BTreeMap<u32, u16>,
    limits: &LimitsV1,
) -> Result<()> {
    if stats.writes > limits.max_writes
        || stats.decode_units > limits.max_decode_units
        || voxels.len() > limits.max_voxels as usize
        || (voxels.len() as u64).saturating_mul(16) > limits.max_memory_bytes
    {
        return Err(Error::limit(
            "building-resource-limit",
            "Building VM expansion exceeds task limits.",
        ));
    }
    Ok(())
}

fn byte_delta(start: usize, end: usize) -> Result<u32> {
    u32::try_from(end.saturating_sub(start))
        .map_err(|_| Error::overflow("Building VM byte accounting overflow."))
}

fn empty_geometry() -> Error {
    Error::new(
        ErrorKind::NonCanonical,
        "building-empty-op",
        "Building program operations must expand at least one voxel.",
    )
}

fn noop_patch(message: &'static str) -> Error {
    Error::new(ErrorKind::NonCanonical, "building-noop-patch", message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::BuildingPatch;
    use crate::vm::{decode_candidate, encode_candidate, CandidateProgram};
    use crate::Profile;

    #[test]
    fn box_cut_and_exact_patch_round_trip() {
        let limits = LimitsV1::default();
        let program = CandidateProgram::Building(BuildingProgram {
            size: [4, 4, 4],
            ops: vec![
                BuildingOp::Box {
                    material: 7,
                    origin: [0, 0, 0],
                    size: [4, 2, 4],
                },
                BuildingOp::Cut {
                    origin: [1, 0, 1],
                    size: [2, 1, 2],
                },
            ],
            patches: vec![BuildingPatch {
                id: 0,
                kind: BuildingPatchKind::Paint,
                material: 8,
            }],
        });
        let bytes = encode_candidate(&program, &limits).unwrap();
        let decoded = decode_candidate(&bytes, Profile::Building, &limits).unwrap();
        assert_eq!(decoded.stats.commands, 2);
        assert_eq!(decoded.stats.patches, 1);
        assert_eq!(decoded.semantics.voxel_count(), 28);
    }

    #[test]
    fn noncanonical_patch_is_rejected() {
        let limits = LimitsV1::default();
        let program = CandidateProgram::Building(BuildingProgram {
            size: [1, 1, 1],
            ops: vec![],
            patches: vec![BuildingPatch {
                id: 0,
                kind: BuildingPatchKind::Clear,
                material: 0,
            }],
        });
        assert!(encode_candidate(&program, &limits).is_err());
    }
}

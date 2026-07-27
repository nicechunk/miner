use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

use crate::error::{Error, ErrorKind, Result};
use crate::model::{
    building_coord_from_id, BuildingSemantics, IncumbentFormat, LimitsV1, Profile, Semantics,
};
use crate::varint::read_u32;

use super::{trim_ascii, ImportedAsset};

const PREFIX: &[u8] = b"NCM3:";
const MAX_PAYLOAD_BYTES: usize = 65_535;
const MAX_COMMANDS: u32 = 4_096;
const MAX_OPERATIONS: u64 = 262_144;
const MAX_VOXELS: usize = 131_072;

const BOX: u8 = 1;
const REPEAT_BOX: u8 = 2;
const GABLE: u8 = 3;
const TREE: u8 = 4;
const FENCE: u8 = 5;
const GABLE_TRIM: u8 = 6;
const GABLE_FILL: u8 = 7;
const GABLE_Z: u8 = 8;
const GABLE_TRIM_Z: u8 = 9;
const GABLE_FILL_Z: u8 = 10;

pub(super) fn import(input: &[u8], limits: &LimitsV1) -> Result<ImportedAsset> {
    let value = trim_ascii(input);
    let raw = if value.starts_with(PREFIX) {
        let encoded = core::str::from_utf8(&value[PREFIX.len()..]).map_err(|_| {
            Error::invalid("ncm3-base64", "NCM3 text must contain ASCII Base64URL.")
        })?;
        decode_canonical_base64(encoded)?
    } else {
        value.to_vec()
    };
    import_raw(&raw, limits)
}

pub(super) fn import_raw(input: &[u8], limits: &LimitsV1) -> Result<ImportedAsset> {
    if input.is_empty()
        || input.len() > MAX_PAYLOAD_BYTES
        || input.len() > limits.max_input_bytes as usize
    {
        return Err(Error::limit(
            "ncm3-payload-limit",
            "NCM3 payload is empty or exceeds the supported byte limit.",
        ));
    }
    let semantics = decode(input, limits)?;
    Ok(ImportedAsset {
        profile: Profile::Building,
        format: IncumbentFormat::Ncm3V1,
        incumbent_encoding: input.to_vec(),
        semantics: Semantics::Building(semantics),
    })
}

fn decode_canonical_base64(encoded: &str) -> Result<Vec<u8>> {
    if encoded.is_empty()
        || encoded.len() % 4 == 1
        || encoded
            .bytes()
            .any(|byte| !byte.is_ascii_alphanumeric() && byte != b'-' && byte != b'_')
    {
        return Err(Error::new(
            ErrorKind::NonCanonical,
            "ncm3-base64",
            "NCM3 requires canonical unpadded Base64URL.",
        ));
    }
    let raw = URL_SAFE_NO_PAD.decode(encoded.as_bytes()).map_err(|_| {
        Error::new(
            ErrorKind::NonCanonical,
            "ncm3-base64",
            "NCM3 requires canonical unpadded Base64URL.",
        )
    })?;
    if raw.len() > MAX_PAYLOAD_BYTES || URL_SAFE_NO_PAD.encode(&raw) != encoded {
        return Err(Error::new(
            ErrorKind::NonCanonical,
            "ncm3-base64",
            "NCM3 Base64URL is non-canonical or oversized.",
        ));
    }
    Ok(raw)
}

fn decode(input: &[u8], limits: &LimitsV1) -> Result<BuildingSemantics> {
    let mut offset = 0_usize;
    let version = read_byte(input, &mut offset, "NCM3 version")?;
    if version != 1 {
        return Err(Error::new(
            ErrorKind::UnsupportedVersion,
            "ncm3-version",
            "Only NCM3 version 1 is supported.",
        ));
    }
    let size = [
        read_dimension(input, &mut offset)?,
        read_dimension(input, &mut offset)?,
        read_dimension(input, &mut offset)?,
    ];
    let command_count = read_u32(input, &mut offset)?;
    if command_count > MAX_COMMANDS || command_count > limits.max_commands {
        return Err(Error::limit(
            "ncm3-command-limit",
            "NCM3 command count exceeds the configured limit.",
        ));
    }

    let mut voxels = BTreeMap::<u32, u16>::new();
    let mut materials = BTreeSet::<u16>::new();
    let mut operation_budget = 0_u64;
    for _ in 0..command_count {
        let opcode = read_byte(input, &mut offset, "NCM3 opcode")?;
        if opcode == TREE {
            let trunk = read_material(input, &mut offset, &mut materials)?;
            let leaves = read_material(input, &mut offset, &mut materials)?;
            let x = read_u32(input, &mut offset)?;
            let y = read_u32(input, &mut offset)?;
            let z = read_u32(input, &mut offset)?;
            let height = read_u32(input, &mut offset)?;
            let crown = read_u32(input, &mut offset)?;
            if !(2..=64).contains(&height) || !(1..=16).contains(&crown) {
                return Err(Error::new(
                    ErrorKind::OutOfBounds,
                    "ncm3-tree",
                    "NCM3 tree parameters exceed safety bounds.",
                ));
            }
            let crown_diameter = crown
                .checked_mul(2)
                .and_then(|value| value.checked_add(2))
                .ok_or_else(|| Error::overflow("NCM3 tree crown overflow."))?;
            if x < crown || z < crown {
                return Err(out_of_bounds(
                    "NCM3 tree crown extends outside the blueprint.",
                ));
            }
            check_box(
                size,
                x - crown,
                y,
                z - crown,
                crown_diameter,
                height.max(crown + 1),
                crown_diameter,
            )?;
            let trunk_height = height.saturating_sub(crown).max(2);
            add_budget(
                &mut operation_budget,
                u64::from(trunk_height) * 4 + u64::from(crown) * 8 * u64::from(crown_diameter) + 4,
                limits,
            )?;
            write_box(&mut voxels, size, trunk, x, y, z, 2, trunk_height, 2)?;
            for layer in 0..crown {
                let radius = (crown - layer / 2).max(1);
                write_box(
                    &mut voxels,
                    size,
                    leaves,
                    x - radius,
                    y + trunk_height - 1 + layer,
                    z - 1,
                    radius * 2 + 2,
                    1,
                    4,
                )?;
                write_box(
                    &mut voxels,
                    size,
                    leaves,
                    x - 1,
                    y + trunk_height - 1 + layer,
                    z - radius,
                    4,
                    1,
                    radius * 2 + 2,
                )?;
            }
            write_box(&mut voxels, size, leaves, x, y + height - 1, z, 2, 1, 2)?;
            continue;
        }

        let material = read_material(input, &mut offset, &mut materials)?;
        match opcode {
            BOX => {
                let (x, y, z, w, h, d) = read_box(input, &mut offset)?;
                add_budget(&mut operation_budget, box_volume(w, h, d)?, limits)?;
                write_box(&mut voxels, size, material, x, y, z, w, h, d)?;
            }
            REPEAT_BOX => {
                let (x, y, z, w, h, d) = read_box(input, &mut offset)?;
                let count = read_u32(input, &mut offset)?
                    .checked_add(1)
                    .ok_or_else(|| Error::overflow("NCM3 repeat count overflow."))?;
                let dx = read_signed_var(input, &mut offset)?;
                let dy = read_signed_var(input, &mut offset)?;
                let dz = read_signed_var(input, &mut offset)?;
                if count > 512
                    || !(-256..=256).contains(&dx)
                    || !(-256..=256).contains(&dy)
                    || !(-256..=256).contains(&dz)
                {
                    return Err(out_of_bounds(
                        "NCM3 repeat parameters exceed safety bounds.",
                    ));
                }
                add_budget(
                    &mut operation_budget,
                    box_volume(w, h, d)?
                        .checked_mul(u64::from(count))
                        .ok_or_else(|| Error::overflow("NCM3 repeat operation overflow."))?,
                    limits,
                )?;
                for index in 0..count {
                    let rx = repeated_axis(x, dx, index)?;
                    let ry = repeated_axis(y, dy, index)?;
                    let rz = repeated_axis(z, dz, index)?;
                    write_box(&mut voxels, size, material, rx, ry, rz, w, h, d)?;
                }
            }
            GABLE | GABLE_TRIM | GABLE_FILL | GABLE_Z | GABLE_TRIM_Z | GABLE_FILL_Z => {
                let x = read_u32(input, &mut offset)?;
                let y = read_u32(input, &mut offset)?;
                let z = read_u32(input, &mut offset)?;
                let width = read_u32(input, &mut offset)?
                    .checked_add(1)
                    .ok_or_else(|| Error::overflow("NCM3 gable width overflow."))?;
                let depth = read_u32(input, &mut offset)?
                    .checked_add(1)
                    .ok_or_else(|| Error::overflow("NCM3 gable depth overflow."))?;
                let z_oriented = matches!(opcode, GABLE_Z | GABLE_TRIM_Z | GABLE_FILL_Z);
                let layers = if z_oriented {
                    depth.div_ceil(2)
                } else {
                    width.div_ceil(2)
                };
                check_box(size, x, y, z, width, layers, depth)?;
                let upper = match opcode {
                    GABLE => u64::from(layers) * 2 * u64::from(depth),
                    GABLE_TRIM => u64::from(layers) * 4,
                    GABLE_FILL => u64::from(layers) * u64::from(width) * u64::from(depth),
                    GABLE_Z => u64::from(layers) * 2 * u64::from(width),
                    GABLE_TRIM_Z => u64::from(layers) * 4,
                    _ => u64::from(layers) * u64::from(width) * u64::from(depth),
                };
                add_budget(&mut operation_budget, upper, limits)?;
                write_gable(
                    &mut voxels,
                    size,
                    material,
                    opcode,
                    x,
                    y,
                    z,
                    width,
                    depth,
                    layers,
                )?;
            }
            FENCE => {
                let x = read_u32(input, &mut offset)?;
                let y = read_u32(input, &mut offset)?;
                let z = read_u32(input, &mut offset)?;
                let length = read_u32(input, &mut offset)?
                    .checked_add(1)
                    .ok_or_else(|| Error::overflow("NCM3 fence length overflow."))?;
                let axis = read_u32(input, &mut offset)?;
                let spacing = read_u32(input, &mut offset)?;
                if length > 256 || axis > 1 || !(1..=64).contains(&spacing) {
                    return Err(out_of_bounds("NCM3 fence parameters exceed safety bounds."));
                }
                let (width, depth) = if axis == 0 { (length, 1) } else { (1, length) };
                check_box(size, x, y, z, width, 5, depth)?;
                add_budget(
                    &mut operation_budget,
                    u64::from(length) * 2 + (u64::from(length.div_ceil(spacing)) + 1) * 5,
                    limits,
                )?;
                write_box(&mut voxels, size, material, x, y + 1, z, width, 1, depth)?;
                write_box(&mut voxels, size, material, x, y + 3, z, width, 1, depth)?;
                let mut position = 0;
                while position < length {
                    write_box(
                        &mut voxels,
                        size,
                        material,
                        x + if axis == 0 { position } else { 0 },
                        y,
                        z + if axis == 1 { position } else { 0 },
                        1,
                        5,
                        1,
                    )?;
                    position = position
                        .checked_add(spacing)
                        .ok_or_else(|| Error::overflow("NCM3 fence spacing overflow."))?;
                }
                let end = length - 1;
                write_box(
                    &mut voxels,
                    size,
                    material,
                    x + if axis == 0 { end } else { 0 },
                    y,
                    z + if axis == 1 { end } else { 0 },
                    1,
                    5,
                    1,
                )?;
            }
            _ => {
                return Err(Error::new(
                    ErrorKind::UnknownOpcode,
                    "ncm3-opcode",
                    "NCM3 payload contains an unknown opcode.",
                ))
            }
        }
        if voxels.len() > MAX_VOXELS || voxels.len() > limits.max_voxels as usize {
            return Err(Error::limit(
                "ncm3-voxel-limit",
                "NCM3 expansion exceeds the voxel limit.",
            ));
        }
        if materials.len() > limits.max_materials as usize {
            return Err(Error::limit(
                "material-limit",
                "NCM3 references too many materials.",
            ));
        }
    }
    if offset != input.len() {
        return Err(Error::new(
            ErrorKind::TrailingData,
            "ncm3-trailing-data",
            "NCM3 payload contains trailing bytes.",
        ));
    }
    let dimensions = [size[0] as u16, size[1] as u16, size[2] as u16];
    let mut output = Vec::with_capacity(voxels.len());
    for (id, material) in voxels {
        output.push(building_coord_from_id(dimensions, id, material)?);
    }
    Ok(BuildingSemantics {
        size: dimensions,
        voxels: output,
    })
}

fn read_byte(input: &[u8], offset: &mut usize, label: &'static str) -> Result<u8> {
    let value = *input
        .get(*offset)
        .ok_or_else(|| Error::new(ErrorKind::Truncated, "ncm3-truncated", label))?;
    *offset += 1;
    Ok(value)
}

fn read_dimension(input: &[u8], offset: &mut usize) -> Result<u32> {
    let value = read_u32(input, offset)?;
    if !(1..=256).contains(&value) {
        return Err(out_of_bounds("NCM3 dimensions must be in 1..=256."));
    }
    Ok(value)
}

fn read_material(input: &[u8], offset: &mut usize, materials: &mut BTreeSet<u16>) -> Result<u16> {
    let value = read_u32(input, offset)?;
    let material =
        u16::try_from(value).map_err(|_| out_of_bounds("NCM3 material ID exceeds u16."))?;
    if material == 0 {
        return Err(out_of_bounds("NCM3 material ID zero is reserved for air."));
    }
    materials.insert(material);
    Ok(material)
}

fn read_box(input: &[u8], offset: &mut usize) -> Result<(u32, u32, u32, u32, u32, u32)> {
    let x = read_u32(input, offset)?;
    let y = read_u32(input, offset)?;
    let z = read_u32(input, offset)?;
    let w = read_u32(input, offset)?
        .checked_add(1)
        .ok_or_else(|| Error::overflow("NCM3 width overflow."))?;
    let h = read_u32(input, offset)?
        .checked_add(1)
        .ok_or_else(|| Error::overflow("NCM3 height overflow."))?;
    let d = read_u32(input, offset)?
        .checked_add(1)
        .ok_or_else(|| Error::overflow("NCM3 depth overflow."))?;
    Ok((x, y, z, w, h, d))
}

fn read_signed_var(input: &[u8], offset: &mut usize) -> Result<i32> {
    let value = read_u32(input, offset)?;
    if value & 1 == 1 {
        Ok(-(((value as i64 + 1) / 2) as i32))
    } else {
        Ok((value / 2) as i32)
    }
}

fn repeated_axis(start: u32, delta: i32, index: u32) -> Result<u32> {
    let value = i64::from(start)
        .checked_add(
            i64::from(delta)
                .checked_mul(i64::from(index))
                .ok_or_else(|| Error::overflow("NCM3 repeated axis overflow."))?,
        )
        .ok_or_else(|| Error::overflow("NCM3 repeated axis overflow."))?;
    u32::try_from(value)
        .map_err(|_| out_of_bounds("NCM3 repeated geometry uses a negative coordinate."))
}

fn add_budget(current: &mut u64, value: u64, limits: &LimitsV1) -> Result<()> {
    *current = current
        .checked_add(value)
        .ok_or_else(|| Error::overflow("NCM3 operation budget overflow."))?;
    if *current > MAX_OPERATIONS || *current > u64::from(limits.max_writes) {
        return Err(Error::limit(
            "ncm3-operation-limit",
            "NCM3 expansion exceeds the operation budget.",
        ));
    }
    Ok(())
}

fn box_volume(w: u32, h: u32, d: u32) -> Result<u64> {
    u64::from(w)
        .checked_mul(u64::from(h))
        .and_then(|value| value.checked_mul(u64::from(d)))
        .ok_or_else(|| Error::overflow("NCM3 box volume overflow."))
}

fn check_box(size: [u32; 3], x: u32, y: u32, z: u32, w: u32, h: u32, d: u32) -> Result<()> {
    if w == 0
        || h == 0
        || d == 0
        || x.checked_add(w).is_none_or(|end| end > size[0])
        || y.checked_add(h).is_none_or(|end| end > size[1])
        || z.checked_add(d).is_none_or(|end| end > size[2])
    {
        return Err(out_of_bounds(
            "NCM3 command extends outside the declared dimensions.",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn write_box(
    voxels: &mut BTreeMap<u32, u16>,
    size: [u32; 3],
    material: u16,
    x: u32,
    y: u32,
    z: u32,
    w: u32,
    h: u32,
    d: u32,
) -> Result<()> {
    check_box(size, x, y, z, w, h, d)?;
    for yy in y..y + h {
        for zz in z..z + d {
            for xx in x..x + w {
                let id = xx + size[0] * (zz + size[2] * yy);
                voxels.insert(id, material);
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn write_gable(
    voxels: &mut BTreeMap<u32, u16>,
    size: [u32; 3],
    material: u16,
    opcode: u8,
    x: u32,
    y: u32,
    z: u32,
    width: u32,
    depth: u32,
    layers: u32,
) -> Result<()> {
    for layer in 0..layers {
        match opcode {
            GABLE => {
                let left = x + layer;
                let right = x + width - 1 - layer;
                write_box(voxels, size, material, left, y + layer, z, 1, 1, depth)?;
                if right != left {
                    write_box(voxels, size, material, right, y + layer, z, 1, 1, depth)?;
                }
            }
            GABLE_TRIM => {
                let left = x + layer;
                let right = x + width - 1 - layer;
                for edge_z in [z, z + depth - 1] {
                    write_box(voxels, size, material, left, y + layer, edge_z, 1, 1, 1)?;
                }
                if right != left {
                    for edge_z in [z, z + depth - 1] {
                        write_box(voxels, size, material, right, y + layer, edge_z, 1, 1, 1)?;
                    }
                }
            }
            GABLE_FILL => write_box(
                voxels,
                size,
                material,
                x + layer,
                y + layer,
                z,
                width - layer * 2,
                1,
                depth,
            )?,
            GABLE_Z => {
                let front = z + layer;
                let back = z + depth - 1 - layer;
                write_box(voxels, size, material, x, y + layer, front, width, 1, 1)?;
                if back != front {
                    write_box(voxels, size, material, x, y + layer, back, width, 1, 1)?;
                }
            }
            GABLE_TRIM_Z => {
                let front = z + layer;
                let back = z + depth - 1 - layer;
                for edge_x in [x, x + width - 1] {
                    write_box(voxels, size, material, edge_x, y + layer, front, 1, 1, 1)?;
                }
                if back != front {
                    for edge_x in [x, x + width - 1] {
                        write_box(voxels, size, material, edge_x, y + layer, back, 1, 1, 1)?;
                    }
                }
            }
            GABLE_FILL_Z => write_box(
                voxels,
                size,
                material,
                x,
                y + layer,
                z + layer,
                width,
                1,
                depth - layer * 2,
            )?,
            _ => {
                return Err(Error::new(
                    ErrorKind::UnknownOpcode,
                    "ncm3-opcode",
                    "Unknown gable opcode.",
                ))
            }
        }
    }
    Ok(())
}

fn out_of_bounds(message: &'static str) -> Error {
    Error::new(ErrorKind::OutOfBounds, "ncm3-out-of-bounds", message)
}

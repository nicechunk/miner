use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::error::{Error, ErrorKind, Result};
use crate::model::{
    forge_cell_from_id, forge_cell_id, AppearanceQuad, ForgeAppearance, ForgeComponent,
    ForgeGeometry, ForgedSemantics, Grip, LimitsV1, PaintQuad, Semantics, FORGE_CELL_COUNT,
    FORGE_GRID_X, FORGE_GRID_Y, FORGE_GRID_Z,
};
use crate::varint::{write_i16, write_u32};

use super::{
    checked_u16, ensure_count, read_bool_bits, write_bool_bits, Cursor, ForgeComponentProgram,
    ForgePatchKind, ForgeProgram, ForgeProgramGeometry, ForgeSolidOp, StatsBuilder, VmStats,
};

const SOLID: u8 = 1;
const CUT_BOX: u8 = 2;
const EXTRUDE: u8 = 3;
const RLE: u8 = 4;
const SYMMETRY: u8 = 5;
const SPARSE: u8 = 6;
const PATCH_ADD: u8 = 0x80;
const PATCH_CLEAR: u8 = 0x81;

pub(super) fn encode(
    output: &mut Vec<u8>,
    program: &ForgeProgram,
    limits: &LimitsV1,
) -> Result<()> {
    output.extend_from_slice(&program.equipment.mass_5g.to_le_bytes());
    output.extend_from_slice(&program.equipment.encoded_volume.to_le_bytes());
    output.extend_from_slice(&program.equipment.attributes_6);
    match &program.geometry {
        ForgeProgramGeometry::Components { components } => {
            output.push(0);
            write_u32(output, components.len() as u32);
            let total_commands = components.iter().try_fold(0_u32, |total, item| {
                total
                    .checked_add(item.ops.len() as u32)
                    .ok_or_else(|| Error::overflow("Forge command count overflow."))
            })?;
            let total_patches = components.iter().try_fold(0_u32, |total, item| {
                total
                    .checked_add(item.patches.len() as u32)
                    .and_then(|value| value.checked_add(item.paint.len() as u32))
                    .ok_or_else(|| Error::overflow("Forge patch count overflow."))
            })?;
            ensure_count(total_commands, limits.max_commands, "forge-command-limit")?;
            ensure_count(total_patches, limits.max_patches, "forge-patch-limit")?;
            for component in components {
                encode_component(output, component)?;
            }
        }
        ForgeProgramGeometry::Appearance {
            dimensions_q,
            grip,
            quads,
        } => {
            output.push(1);
            for dimension in dimensions_q {
                write_u32(output, u32::from(*dimension));
            }
            encode_grip(output, *grip);
            ensure_count(quads.len() as u32, limits.max_patches, "forge-quad-limit")?;
            write_u32(output, quads.len() as u32);
            for quad in quads {
                encode_appearance_quad(output, *quad);
            }
        }
    }
    Ok(())
}

fn encode_component(output: &mut Vec<u8>, component: &ForgeComponentProgram) -> Result<()> {
    output.push(component.resource);
    output.extend_from_slice(&component.color_444.to_le_bytes());
    output.extend_from_slice(&component.dimensions_q);
    for offset in component.offset_q {
        write_i16(output, offset);
    }
    encode_grip(output, component.grip);
    write_u32(output, component.ops.len() as u32);
    for op in &component.ops {
        match op {
            ForgeSolidOp::Solid => output.push(SOLID),
            ForgeSolidOp::CutBox { origin, size } => {
                output.push(CUT_BOX);
                output.extend_from_slice(origin);
                output.extend_from_slice(size);
            }
            ForgeSolidOp::Extrude { axis, mask } => {
                output.push(EXTRUDE);
                output.push(*axis);
                let expected = forge_tangent_area(usize::from(*axis))?;
                if mask.len() != expected {
                    return Err(Error::invalid(
                        "forge-extrude-mask",
                        "Forge extrusion mask length does not match its axis.",
                    ));
                }
                write_bool_bits(output, mask);
            }
            ForgeSolidOp::Rle { occupancy } => {
                output.push(RLE);
                if occupancy.len() != usize::from(FORGE_CELL_COUNT) {
                    return Err(Error::invalid(
                        "forge-rle-length",
                        "Forge RLE occupancy must cover the complete component grid.",
                    ));
                }
                encode_runs(output, occupancy);
            }
            ForgeSolidOp::Symmetry { axis } => {
                output.push(SYMMETRY);
                output.push(*axis);
            }
            ForgeSolidOp::Sparse { cells } => {
                output.push(SPARSE);
                write_u32(output, cells.len() as u32);
                let mut previous = 0_u32;
                for (index, cell) in cells.iter().enumerate() {
                    let cell = u32::from(*cell);
                    let delta = if index == 0 {
                        cell
                    } else {
                        cell.checked_sub(previous)
                            .and_then(|value| value.checked_sub(1))
                            .ok_or_else(|| {
                                Error::new(
                                    ErrorKind::NonCanonical,
                                    "forge-sparse-order",
                                    "Forge sparse cells must be strictly sorted.",
                                )
                            })?
                    };
                    write_u32(output, delta);
                    previous = cell;
                }
            }
        }
    }
    write_u32(output, component.patches.len() as u32);
    let mut previous = 0_u32;
    for (index, patch) in component.patches.iter().enumerate() {
        output.push(match patch.kind {
            ForgePatchKind::Add => PATCH_ADD,
            ForgePatchKind::Clear => PATCH_CLEAR,
        });
        let cell = u32::from(patch.cell);
        let delta = if index == 0 {
            cell
        } else {
            cell.checked_sub(previous)
                .and_then(|value| value.checked_sub(1))
                .ok_or_else(|| {
                    Error::new(
                        ErrorKind::NonCanonical,
                        "forge-patch-order",
                        "Forge residual cells must be strictly sorted.",
                    )
                })?
        };
        write_u32(output, delta);
        previous = cell;
    }
    write_u32(output, component.paint.len() as u32);
    for quad in &component.paint {
        encode_paint_quad(output, *quad);
    }
    Ok(())
}

fn encode_runs(output: &mut Vec<u8>, occupancy: &[bool]) {
    output.push(u8::from(occupancy.first().copied().unwrap_or(false)));
    let mut lengths = Vec::new();
    let mut value = occupancy.first().copied().unwrap_or(false);
    let mut length = 0_u32;
    for occupied in occupancy {
        if *occupied == value {
            length += 1;
        } else {
            lengths.push(length);
            value = *occupied;
            length = 1;
        }
    }
    lengths.push(length);
    write_u32(output, lengths.len() as u32);
    for length in lengths {
        write_u32(output, length);
    }
}

pub(super) fn decode(cursor: &mut Cursor<'_>, limits: &LimitsV1) -> Result<(Semantics, VmStats)> {
    let mass_bytes = cursor.bytes(2, "forge equipment mass")?;
    let volume_bytes = cursor.bytes(2, "forge equipment volume")?;
    let mass_5g = u16::from_le_bytes([mass_bytes[0], mass_bytes[1]]);
    let encoded_volume = u16::from_le_bytes([volume_bytes[0], volume_bytes[1]]);
    let mut attributes_6 = [0_u8; 12];
    attributes_6.copy_from_slice(cursor.bytes(12, "forge equipment attributes")?);
    if attributes_6.iter().any(|value| *value > 63) {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "forge-attribute",
            "Forge attributes must fit six bits.",
        ));
    }
    let equipment = crate::model::ForgeEquipment {
        mass_5g,
        encoded_volume,
        attributes_6,
    };
    let mode = cursor.byte("forge geometry mode")?;
    let mut stats = StatsBuilder::default();
    let geometry = match mode {
        0 => {
            let count = cursor.u32()?;
            if !(1..=31).contains(&count) {
                return Err(Error::new(
                    ErrorKind::OutOfBounds,
                    "forge-component-count",
                    "Forge candidate component count must be in 1..=31.",
                ));
            }
            let mut components = Vec::with_capacity(count as usize);
            for _ in 0..count {
                components.push(decode_component(cursor, limits, &mut stats)?);
            }
            ForgeGeometry::Components { components }
        }
        1 => {
            let dimensions_q = [
                checked_u16(cursor.u32()?, "Forge appearance x dimension exceeds u16.")?,
                checked_u16(cursor.u32()?, "Forge appearance y dimension exceeds u16.")?,
                checked_u16(cursor.u32()?, "Forge appearance z dimension exceeds u16.")?,
            ];
            let grip = decode_grip(cursor)?;
            let residual_start = cursor.offset;
            let count = cursor.u32()?;
            if count == 0 || count > limits.max_patches || count > limits.max_voxels {
                return Err(Error::limit(
                    "forge-appearance-count",
                    "Forge appearance quad count is empty or exceeds task limits.",
                ));
            }
            let mut quads = Vec::with_capacity(count as usize);
            let mut previous = None;
            for _ in 0..count {
                let quad = decode_appearance_quad(cursor)?;
                if previous.is_some_and(|value| value >= quad) {
                    return Err(Error::new(
                        ErrorKind::NonCanonical,
                        "forge-appearance-order",
                        "Forge appearance quads must be strictly sorted.",
                    ));
                }
                reject_appearance_overlap(&quads, quad)?;
                previous = Some(quad);
                quads.push(quad);
                stats.add_patch(1, 12)?;
            }
            stats.residual_bytes = stats
                .residual_bytes
                .checked_add(byte_delta(residual_start, cursor.offset)?)
                .ok_or_else(|| Error::overflow("Forge residual byte count overflow."))?;
            ForgeGeometry::Appearance {
                appearance: ForgeAppearance {
                    dimensions_q,
                    grip,
                    quads,
                },
            }
        }
        _ => {
            return Err(Error::new(
                ErrorKind::UnknownOpcode,
                "forge-mode",
                "Forge candidate contains an unknown geometry mode.",
            ))
        }
    };
    check_resources(&stats, limits)?;
    Ok((
        Semantics::ForgedItem(ForgedSemantics {
            equipment,
            geometry,
        }),
        stats.finish(),
    ))
}

fn decode_component(
    cursor: &mut Cursor<'_>,
    limits: &LimitsV1,
    stats: &mut StatsBuilder,
) -> Result<ForgeComponent> {
    let resource = cursor.byte("forge component resource")?;
    if resource >= 6 {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "forge-resource",
            "Forge component references an unknown resource.",
        ));
    }
    let color = cursor.bytes(2, "forge component color")?;
    let color_444 = u16::from_le_bytes([color[0], color[1]]);
    if color_444 > 0x0fff {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "forge-color",
            "Forge component color must fit RGB444.",
        ));
    }
    let dimensions = cursor.bytes(3, "forge component dimensions")?;
    let dimensions_q = [dimensions[0], dimensions[1], dimensions[2]];
    if dimensions_q.contains(&0) {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "forge-dimensions",
            "Forge component dimensions must be non-zero.",
        ));
    }
    let offset_q = [cursor.i16()?, cursor.i16()?, cursor.i16()?];
    let grip = decode_grip(cursor)?;
    let mut solid = BTreeSet::new();
    let program_start = cursor.offset;
    let command_count = cursor.u32()?;
    ensure_count(command_count, limits.max_commands, "forge-command-limit")?;
    for _ in 0..command_count {
        match cursor.byte("forge solid opcode")? {
            SOLID => {
                solid.extend(0..FORGE_CELL_COUNT);
                stats.add_command(u32::from(FORGE_CELL_COUNT), 4)?;
            }
            CUT_BOX => decode_cut_box(cursor, &mut solid, stats, limits)?,
            EXTRUDE => decode_extrude(cursor, &mut solid, stats, limits)?,
            RLE => decode_rle(cursor, &mut solid, stats, limits)?,
            SYMMETRY => decode_symmetry(cursor, &mut solid, stats, limits)?,
            SPARSE => decode_sparse(cursor, &mut solid, stats, limits)?,
            _ => {
                return Err(Error::new(
                    ErrorKind::UnknownOpcode,
                    "forge-solid-opcode",
                    "Forge solid program contains an unknown opcode.",
                ))
            }
        }
        check_resources(stats, limits)?;
    }
    stats.program_bytes = stats
        .program_bytes
        .checked_add(byte_delta(program_start, cursor.offset)?)
        .ok_or_else(|| Error::overflow("Forge program byte count overflow."))?;

    let residual_start = cursor.offset;
    let patch_count = cursor.u32()?;
    ensure_count(patch_count, limits.max_patches, "forge-patch-limit")?;
    let mut previous = 0_u32;
    for index in 0..patch_count {
        let opcode = cursor.byte("forge patch opcode")?;
        let delta = cursor.u32()?;
        let cell = if index == 0 {
            delta
        } else {
            previous
                .checked_add(delta)
                .and_then(|value| value.checked_add(1))
                .ok_or_else(|| Error::overflow("Forge residual coordinate overflow."))?
        };
        if cell >= u32::from(FORGE_CELL_COUNT) || (index > 0 && cell <= previous) {
            return Err(Error::new(
                ErrorKind::NonCanonical,
                "forge-patch-order",
                "Forge residual cells must be strictly sorted in range.",
            ));
        }
        match opcode {
            PATCH_ADD => {
                if !solid.insert(cell as u16) {
                    return Err(noop_patch());
                }
            }
            PATCH_CLEAR => {
                if !solid.remove(&(cell as u16)) {
                    return Err(noop_patch());
                }
            }
            _ => {
                return Err(Error::new(
                    ErrorKind::UnknownOpcode,
                    "forge-patch-opcode",
                    "Forge residual contains an unknown patch opcode.",
                ))
            }
        }
        stats.add_patch(1, 4)?;
        previous = cell;
    }
    if solid.is_empty() {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "forge-empty-component",
            "Forge residual leaves an empty component.",
        ));
    }
    let paint_count = cursor.u32()?;
    ensure_count(paint_count, limits.max_patches, "forge-paint-limit")?;
    let mut paint = Vec::with_capacity(paint_count as usize);
    let mut previous_quad = None;
    for _ in 0..paint_count {
        let quad = decode_paint_quad(cursor)?;
        if previous_quad.is_some_and(|value| value >= quad) {
            return Err(Error::new(
                ErrorKind::NonCanonical,
                "forge-paint-order",
                "Forge paint quads must be strictly sorted.",
            ));
        }
        reject_paint_overlap(&paint, quad)?;
        previous_quad = Some(quad);
        paint.push(quad);
        stats.add_patch(1, 10)?;
    }
    stats.residual_bytes = stats
        .residual_bytes
        .checked_add(byte_delta(residual_start, cursor.offset)?)
        .ok_or_else(|| Error::overflow("Forge residual byte count overflow."))?;
    check_resources(stats, limits)?;
    Ok(ForgeComponent {
        resource,
        color_444,
        dimensions_q,
        offset_q,
        grip,
        solid: solid.into_iter().collect(),
        paint,
    })
}

fn decode_cut_box(
    cursor: &mut Cursor<'_>,
    solid: &mut BTreeSet<u16>,
    stats: &mut StatsBuilder,
    limits: &LimitsV1,
) -> Result<()> {
    let origin_bytes = cursor.bytes(3, "forge cut-box origin")?;
    let size_bytes = cursor.bytes(3, "forge cut-box size")?;
    let origin = [origin_bytes[0], origin_bytes[1], origin_bytes[2]];
    let size = [size_bytes[0], size_bytes[1], size_bytes[2]];
    ensure_forge_box(origin, size)?;
    let writes = u32::from(size[0]) * u32::from(size[1]) * u32::from(size[2]);
    ensure_expansion(writes, limits)?;
    for z in origin[2]..origin[2] + size[2] {
        for y in origin[1]..origin[1] + size[1] {
            for x in origin[0]..origin[0] + size[0] {
                solid.remove(&forge_cell_id(x, y, z));
            }
        }
    }
    stats.add_command(writes, 7)
}

fn decode_extrude(
    cursor: &mut Cursor<'_>,
    solid: &mut BTreeSet<u16>,
    stats: &mut StatsBuilder,
    limits: &LimitsV1,
) -> Result<()> {
    let axis = read_axis(cursor)?;
    let sizes = forge_sizes();
    let tangent = tangent_axes(axis);
    let area = sizes[tangent[0]] * sizes[tangent[1]];
    let mask = read_bool_bits(cursor, area, "forge extrude mask")?;
    let selected = mask.iter().filter(|value| **value).count() as u32;
    if selected == 0 {
        return Err(empty_op());
    }
    let writes = selected
        .checked_mul(sizes[axis] as u32)
        .ok_or_else(|| Error::overflow("Forge extrusion expansion overflow."))?;
    ensure_expansion(writes, limits)?;
    for layer in 0..sizes[axis] {
        for v in 0..sizes[tangent[1]] {
            for u in 0..sizes[tangent[0]] {
                if !mask[u + sizes[tangent[0]] * v] {
                    continue;
                }
                let mut cell = [0_u8; 3];
                cell[axis] = layer as u8;
                cell[tangent[0]] = u as u8;
                cell[tangent[1]] = v as u8;
                solid.insert(forge_cell_id(cell[0], cell[1], cell[2]));
            }
        }
    }
    stats.add_command(writes, 10 + area as u64)
}

fn decode_rle(
    cursor: &mut Cursor<'_>,
    solid: &mut BTreeSet<u16>,
    stats: &mut StatsBuilder,
    limits: &LimitsV1,
) -> Result<()> {
    let mut value = match cursor.byte("forge RLE initial value")? {
        0 => false,
        1 => true,
        _ => {
            return Err(Error::new(
                ErrorKind::NonCanonical,
                "forge-rle-initial",
                "Forge RLE initial value must be zero or one.",
            ))
        }
    };
    let count = cursor.u32()?;
    if count == 0 || count > u32::from(FORGE_CELL_COUNT) {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "forge-rle-count",
            "Forge RLE run count is empty or oversized.",
        ));
    }
    let mut cursor_cell = 0_u32;
    let mut set_count = 0_u32;
    let mut decoded = BTreeSet::new();
    for _ in 0..count {
        let length = cursor.u32()?;
        let end = cursor_cell
            .checked_add(length)
            .ok_or_else(|| Error::overflow("Forge RLE length overflow."))?;
        if length == 0 || end > u32::from(FORGE_CELL_COUNT) {
            return Err(Error::new(
                ErrorKind::OutOfBounds,
                "forge-rle-length",
                "Forge RLE runs do not match the component grid.",
            ));
        }
        if value {
            decoded.extend((cursor_cell as u16)..(end as u16));
            set_count += length;
        }
        cursor_cell = end;
        value = !value;
    }
    if cursor_cell != u32::from(FORGE_CELL_COUNT) || set_count == 0 {
        return Err(Error::new(
            ErrorKind::Truncated,
            "forge-rle-length",
            "Forge RLE does not cover a non-empty component grid.",
        ));
    }
    ensure_expansion(set_count, limits)?;
    solid.extend(decoded);
    stats.add_command(set_count, 8 + u64::from(count))
}

fn decode_symmetry(
    cursor: &mut Cursor<'_>,
    solid: &mut BTreeSet<u16>,
    stats: &mut StatsBuilder,
    limits: &LimitsV1,
) -> Result<()> {
    let axis = read_axis(cursor)?;
    if solid.is_empty() {
        return Err(empty_op());
    }
    let sizes = forge_sizes();
    let source: Vec<u16> = solid.iter().copied().collect();
    ensure_expansion(source.len() as u32, limits)?;
    for cell in &source {
        let mut coordinate = forge_cell_from_id(*cell)?;
        coordinate[axis] = (sizes[axis] - 1) as u8 - coordinate[axis];
        solid.insert(forge_cell_id(coordinate[0], coordinate[1], coordinate[2]));
    }
    stats.add_command(source.len() as u32, 8 + source.len() as u64)
}

fn decode_sparse(
    cursor: &mut Cursor<'_>,
    solid: &mut BTreeSet<u16>,
    stats: &mut StatsBuilder,
    limits: &LimitsV1,
) -> Result<()> {
    let count = cursor.u32()?;
    if count == 0 || count > u32::from(FORGE_CELL_COUNT) {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "forge-sparse-count",
            "Forge sparse operation is empty or oversized.",
        ));
    }
    ensure_expansion(count, limits)?;
    let mut previous = 0_u32;
    for index in 0..count {
        let delta = cursor.u32()?;
        let cell = if index == 0 {
            delta
        } else {
            previous
                .checked_add(delta)
                .and_then(|value| value.checked_add(1))
                .ok_or_else(|| Error::overflow("Forge sparse coordinate overflow."))?
        };
        if cell >= u32::from(FORGE_CELL_COUNT) || (index > 0 && cell <= previous) {
            return Err(Error::new(
                ErrorKind::NonCanonical,
                "forge-sparse-order",
                "Forge sparse cells must be strictly sorted in range.",
            ));
        }
        solid.insert(cell as u16);
        previous = cell;
    }
    stats.add_command(count, 6 + u64::from(count))
}

fn encode_grip(output: &mut Vec<u8>, grip: Option<Grip>) {
    match grip {
        None => output.push(0),
        Some(grip) => {
            output.push(1);
            for offset in grip.offset_q {
                write_i16(output, offset);
            }
            output.extend_from_slice(&[grip.axis, u8::from(grip.sign > 0), grip.rotation]);
        }
    }
}

fn decode_grip(cursor: &mut Cursor<'_>) -> Result<Option<Grip>> {
    match cursor.byte("forge grip flag")? {
        0 => Ok(None),
        1 => {
            let grip = Grip {
                offset_q: [cursor.i16()?, cursor.i16()?, cursor.i16()?],
                axis: cursor.byte("forge grip axis")?,
                sign: match cursor.byte("forge grip sign")? {
                    0 => -1,
                    1 => 1,
                    _ => {
                        return Err(Error::new(
                            ErrorKind::NonCanonical,
                            "forge-grip-sign",
                            "Forge grip sign must be encoded as zero or one.",
                        ))
                    }
                },
                rotation: cursor.byte("forge grip rotation")?,
            };
            if grip.axis > 2 || grip.rotation > 3 {
                return Err(Error::new(
                    ErrorKind::OutOfBounds,
                    "forge-grip",
                    "Forge grip axis or rotation is outside its v1 range.",
                ));
            }
            Ok(Some(grip))
        }
        _ => Err(Error::new(
            ErrorKind::NonCanonical,
            "forge-grip-flag",
            "Forge grip flag must be zero or one.",
        )),
    }
}

fn encode_paint_quad(output: &mut Vec<u8>, quad: PaintQuad) {
    output.extend_from_slice(&[
        quad.axis, quad.side, quad.plane, quad.u0, quad.u1, quad.v0, quad.v1,
    ]);
    output.extend_from_slice(&quad.color_444.to_le_bytes());
}

fn decode_paint_quad(cursor: &mut Cursor<'_>) -> Result<PaintQuad> {
    let fields = cursor.bytes(7, "forge paint quad")?;
    let color = cursor.bytes(2, "forge paint color")?;
    let quad = PaintQuad {
        axis: fields[0],
        side: fields[1],
        plane: fields[2],
        u0: fields[3],
        u1: fields[4],
        v0: fields[5],
        v1: fields[6],
        color_444: u16::from_le_bytes([color[0], color[1]]),
    };
    validate_paint_quad(quad)?;
    Ok(quad)
}

fn encode_appearance_quad(output: &mut Vec<u8>, quad: AppearanceQuad) {
    output.extend_from_slice(&[
        quad.axis,
        quad.side,
        quad.resource,
        quad.plane,
        quad.u0,
        quad.u1,
        quad.v0,
        quad.v1,
    ]);
    output.extend_from_slice(&quad.color_444.to_le_bytes());
}

fn decode_appearance_quad(cursor: &mut Cursor<'_>) -> Result<AppearanceQuad> {
    let fields = cursor.bytes(8, "forge appearance quad")?;
    let color = cursor.bytes(2, "forge appearance color")?;
    let quad = AppearanceQuad {
        axis: fields[0],
        side: fields[1],
        resource: fields[2],
        plane: fields[3],
        u0: fields[4],
        u1: fields[5],
        v0: fields[6],
        v1: fields[7],
        color_444: u16::from_le_bytes([color[0], color[1]]),
    };
    if quad.axis > 2
        || quad.side > 1
        || quad.resource >= 6
        || quad.plane > 24
        || quad.u0 >= quad.u1
        || quad.v0 >= quad.v1
        || quad.u1 > 24
        || quad.v1 > 24
        || quad.color_444 > 0x0fff
    {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "forge-appearance-quad",
            "Forge appearance quad is outside the v1 geometry envelope.",
        ));
    }
    Ok(quad)
}

fn validate_paint_quad(quad: PaintQuad) -> Result<()> {
    let sizes = forge_sizes();
    if quad.axis > 2 || quad.side > 1 || quad.color_444 > 0x0fff {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "forge-paint-quad",
            "Forge paint quad header is invalid.",
        ));
    }
    let axis = usize::from(quad.axis);
    let tangent = tangent_axes(axis);
    if usize::from(quad.plane) > sizes[axis]
        || quad.u0 >= quad.u1
        || quad.v0 >= quad.v1
        || usize::from(quad.u1) > sizes[tangent[0]]
        || usize::from(quad.v1) > sizes[tangent[1]]
    {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "forge-paint-quad",
            "Forge paint quad is outside the component grid.",
        ));
    }
    Ok(())
}

fn reject_paint_overlap(existing: &[PaintQuad], quad: PaintQuad) -> Result<()> {
    if existing.iter().any(|other| {
        quad.axis == other.axis
            && quad.side == other.side
            && quad.plane == other.plane
            && quad.u0 < other.u1
            && quad.u1 > other.u0
            && quad.v0 < other.v1
            && quad.v1 > other.v0
    }) {
        return Err(Error::new(
            ErrorKind::NonCanonical,
            "forge-paint-overlap",
            "Forge paint quads cannot overlap.",
        ));
    }
    Ok(())
}

fn reject_appearance_overlap(existing: &[AppearanceQuad], quad: AppearanceQuad) -> Result<()> {
    if existing.iter().any(|other| {
        quad.axis == other.axis
            && quad.side == other.side
            && quad.plane == other.plane
            && quad.u0 < other.u1
            && quad.u1 > other.u0
            && quad.v0 < other.v1
            && quad.v1 > other.v0
    }) {
        return Err(Error::new(
            ErrorKind::NonCanonical,
            "forge-appearance-overlap",
            "Forge appearance quads cannot overlap.",
        ));
    }
    Ok(())
}

fn ensure_forge_box(origin: [u8; 3], size: [u8; 3]) -> Result<()> {
    let bounds = [FORGE_GRID_X, FORGE_GRID_Y, FORGE_GRID_Z];
    if size.contains(&0)
        || (0..3).any(|axis| {
            origin[axis]
                .checked_add(size[axis])
                .is_none_or(|end| end > bounds[axis])
        })
    {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "forge-cut-box",
            "Forge cut box is empty or outside the component grid.",
        ));
    }
    Ok(())
}

fn forge_sizes() -> [usize; 3] {
    [
        usize::from(FORGE_GRID_X),
        usize::from(FORGE_GRID_Y),
        usize::from(FORGE_GRID_Z),
    ]
}

fn forge_tangent_area(axis: usize) -> Result<usize> {
    if axis > 2 {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "forge-axis",
            "Forge VM axis must be 0..=2.",
        ));
    }
    let sizes = forge_sizes();
    let tangent = tangent_axes(axis);
    Ok(sizes[tangent[0]] * sizes[tangent[1]])
}

fn read_axis(cursor: &mut Cursor<'_>) -> Result<usize> {
    let axis = usize::from(cursor.byte("forge axis")?);
    if axis > 2 {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "forge-axis",
            "Forge VM axis must be 0..=2.",
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

fn ensure_expansion(writes: u32, limits: &LimitsV1) -> Result<()> {
    if writes == 0 || writes > limits.max_expanded_per_op {
        return Err(Error::limit(
            "forge-expansion-limit",
            "Forge opcode is empty or exceeds the per-operation expansion limit.",
        ));
    }
    Ok(())
}

fn check_resources(stats: &StatsBuilder, limits: &LimitsV1) -> Result<()> {
    if stats.commands > limits.max_commands
        || stats.patches > limits.max_patches
        || stats.writes > limits.max_writes
        || stats.decode_units > limits.max_decode_units
    {
        return Err(Error::limit(
            "forge-resource-limit",
            "Forge VM expansion exceeds task limits.",
        ));
    }
    Ok(())
}

fn byte_delta(start: usize, end: usize) -> Result<u32> {
    u32::try_from(end.saturating_sub(start))
        .map_err(|_| Error::overflow("Forge VM byte accounting overflow."))
}

fn empty_op() -> Error {
    Error::new(
        ErrorKind::NonCanonical,
        "forge-empty-op",
        "Forge solid operations must expand at least one cell.",
    )
}

fn noop_patch() -> Error {
    Error::new(
        ErrorKind::NonCanonical,
        "forge-noop-patch",
        "Forge residual patches must change the program output.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ForgeEquipment;
    use crate::vm::ForgeSolidPatch;
    use crate::vm::{decode_candidate, encode_candidate, CandidateProgram};
    use crate::Profile;
    use alloc::vec;

    fn equipment() -> ForgeEquipment {
        ForgeEquipment {
            mass_5g: 10,
            encoded_volume: 20,
            attributes_6: [1; 12],
        }
    }

    #[test]
    fn solid_cut_and_residual_round_trip() {
        let limits = LimitsV1::default();
        let program = CandidateProgram::ForgedItem(ForgeProgram {
            equipment: equipment(),
            geometry: ForgeProgramGeometry::Components {
                components: vec![ForgeComponentProgram {
                    resource: 0,
                    color_444: 0x9aa,
                    dimensions_q: [64, 64, 64],
                    offset_q: [0, 0, 0],
                    grip: None,
                    ops: vec![
                        ForgeSolidOp::Solid,
                        ForgeSolidOp::CutBox {
                            origin: [0, 0, 0],
                            size: [1, 1, 1],
                        },
                    ],
                    patches: vec![ForgeSolidPatch {
                        cell: 0,
                        kind: ForgePatchKind::Add,
                    }],
                    paint: vec![],
                }],
            },
        });
        let bytes = encode_candidate(&program, &limits).unwrap();
        let decoded = decode_candidate(&bytes, Profile::ForgedItem, &limits).unwrap();
        assert_eq!(
            decoded.semantics.voxel_count(),
            usize::from(FORGE_CELL_COUNT)
        );
        assert_eq!(decoded.stats.commands, 2);
        assert_eq!(decoded.stats.patches, 1);
    }
}

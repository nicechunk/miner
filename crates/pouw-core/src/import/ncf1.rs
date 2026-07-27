use alloc::collections::BTreeSet;
use alloc::vec;
use alloc::vec::Vec;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

use crate::error::{Error, ErrorKind, Result};
use crate::model::{
    forge_cell_id, AppearanceQuad, ForgeAppearance, ForgeComponent, ForgeEquipment, ForgeGeometry,
    ForgedSemantics, Grip, IncumbentFormat, LimitsV1, PaintQuad, Profile, Semantics,
    FORGE_CELL_COUNT, FORGE_GRID_X, FORGE_GRID_Y, FORGE_GRID_Z,
};

use super::{trim_ascii, ImportedAsset};

const PREFIX: &[u8] = b"NCF1.";
const VERSION: u32 = 15;
const MAX_RAW_BYTES: usize = 640;
const RESOURCE_COUNT: u32 = 6;
const DEFAULT_COLORS: [u16; 6] = [0x09aa, 0x0b64, 0x0ccb, 0x0332, 0x0753, 0x0edc];

pub(super) fn import(input: &[u8], limits: &LimitsV1) -> Result<ImportedAsset> {
    let value = trim_ascii(input);
    let raw = if value.starts_with(PREFIX) {
        decode_canonical_base64(&value[PREFIX.len()..])?
    } else {
        value.to_vec()
    };
    import_raw(&raw, limits)
}

pub(super) fn import_raw(input: &[u8], limits: &LimitsV1) -> Result<ImportedAsset> {
    if input.is_empty() {
        return Err(Error::invalid("ncf1-empty", "NCF1 input cannot be empty."));
    }
    if input.len() > MAX_RAW_BYTES || input.len() > limits.max_input_bytes as usize {
        return Err(Error::limit(
            "ncf1-byte-limit",
            "NCF1 input exceeds the 640-byte v15 limit.",
        ));
    }
    let semantics = decode(input, limits)?;
    Ok(ImportedAsset {
        profile: Profile::ForgedItem,
        format: IncumbentFormat::Ncf1V15,
        incumbent_encoding: input.to_vec(),
        semantics: Semantics::ForgedItem(semantics),
    })
}

fn decode_canonical_base64(encoded: &[u8]) -> Result<Vec<u8>> {
    let encoded = core::str::from_utf8(encoded).map_err(|_| {
        Error::new(
            ErrorKind::NonCanonical,
            "ncf1-base64",
            "NCF1 text must contain canonical ASCII Base64URL.",
        )
    })?;
    if encoded.is_empty()
        || encoded.len() % 4 == 1
        || encoded
            .bytes()
            .any(|byte| !byte.is_ascii_alphanumeric() && byte != b'-' && byte != b'_')
    {
        return Err(Error::new(
            ErrorKind::NonCanonical,
            "ncf1-base64",
            "NCF1 requires canonical unpadded Base64URL.",
        ));
    }
    let raw = URL_SAFE_NO_PAD.decode(encoded.as_bytes()).map_err(|_| {
        Error::new(
            ErrorKind::NonCanonical,
            "ncf1-base64",
            "NCF1 requires canonical unpadded Base64URL.",
        )
    })?;
    if raw.len() > MAX_RAW_BYTES || URL_SAFE_NO_PAD.encode(&raw) != encoded {
        return Err(Error::new(
            ErrorKind::NonCanonical,
            "ncf1-base64",
            "NCF1 Base64URL is non-canonical or oversized.",
        ));
    }
    Ok(raw)
}

fn decode(input: &[u8], limits: &LimitsV1) -> Result<ForgedSemantics> {
    let mut reader = BitReader::new(input);
    let version = reader.read(4, "version")?;
    if version != VERSION {
        return Err(Error::new(
            ErrorKind::UnsupportedVersion,
            "ncf1-version",
            "Only NCF1 version 15 is supported.",
        ));
    }
    let equipment = read_equipment(&mut reader)?;
    let geometry = if reader.read(1, "design mode")? == 1 {
        ForgeGeometry::Appearance {
            appearance: read_appearance(&mut reader, limits)?,
        }
    } else {
        ForgeGeometry::Components {
            components: read_components(&mut reader, limits)?,
        }
    };
    reader.finish()?;
    Ok(ForgedSemantics {
        equipment,
        geometry,
    })
}

fn read_equipment(reader: &mut BitReader<'_>) -> Result<ForgeEquipment> {
    let mass_5g = reader.read(16, "equipment mass")? as u16;
    let encoded_volume = reader.read(16, "equipment volume")? as u16;
    let mut attributes_6 = [0_u8; 12];
    for (index, attribute) in attributes_6.iter_mut().enumerate() {
        *attribute = reader.read(6, equipment_attribute_label(index))? as u8;
    }
    Ok(ForgeEquipment {
        mass_5g,
        encoded_volume,
        attributes_6,
    })
}

fn equipment_attribute_label(index: usize) -> &'static str {
    const LABELS: [&str; 12] = [
        "equipment attribute 0",
        "equipment attribute 1",
        "equipment attribute 2",
        "equipment attribute 3",
        "equipment attribute 4",
        "equipment attribute 5",
        "equipment attribute 6",
        "equipment attribute 7",
        "equipment attribute 8",
        "equipment attribute 9",
        "equipment attribute 10",
        "equipment attribute 11",
    ];
    LABELS[index]
}

fn read_components(reader: &mut BitReader<'_>, limits: &LimitsV1) -> Result<Vec<ForgeComponent>> {
    let count = reader.read(5, "component count")?;
    if !(1..=31).contains(&count) || count > limits.max_commands {
        return Err(Error::limit(
            "ncf1-component-count",
            "NCF1 component count must be in 1..=31 and within task limits.",
        ));
    }
    let mut components = Vec::with_capacity(count as usize);
    let mut total_solid = 0_u32;
    let mut total_paint = 0_u32;
    for index in 0..count {
        let resource = read_resource(reader, "component resource")?;
        let color_444 = if reader.read(1, "component default color")? == 1 {
            DEFAULT_COLORS[resource as usize]
        } else {
            reader.read(12, "component color")? as u16
        };
        let dimensions_q = [
            reader.read(8, "component dimension x")? as u8,
            reader.read(8, "component dimension y")? as u8,
            reader.read(8, "component dimension z")? as u8,
        ];
        if dimensions_q.contains(&0) {
            return Err(Error::new(
                ErrorKind::OutOfBounds,
                "ncf1-component-dimensions",
                "NCF1 component dimensions must be non-zero.",
            ));
        }
        let offset_q = if reader.read(1, "component zero offset")? == 1 {
            [0, 0, 0]
        } else {
            [
                reader.read_signed(10, "component offset x")? as i16,
                reader.read_signed(10, "component offset y")? as i16,
                reader.read_signed(10, "component offset z")? as i16,
            ]
        };
        let grip = if reader.read(1, "component grip flag")? == 1 {
            Some(read_grip(reader, 10)?)
        } else {
            None
        };
        let solid = read_solid(reader)?;
        if solid.is_empty() {
            return Err(Error::new(
                ErrorKind::OutOfBounds,
                "ncf1-empty-component",
                "NCF1 components cannot have empty geometry.",
            ));
        }
        total_solid = total_solid
            .checked_add(solid.len() as u32)
            .ok_or_else(|| Error::overflow("NCF1 solid count overflow."))?;
        if total_solid > limits.max_voxels {
            return Err(Error::limit(
                "ncf1-voxel-limit",
                "NCF1 component geometry exceeds the task voxel limit.",
            ));
        }
        let paint_count = reader.read(11, "component paint count")?;
        total_paint = total_paint
            .checked_add(paint_count)
            .ok_or_else(|| Error::overflow("NCF1 paint count overflow."))?;
        if total_paint > limits.max_patches {
            return Err(Error::limit(
                "ncf1-paint-limit",
                "NCF1 paint geometry exceeds the task patch limit.",
            ));
        }
        let mut paint = Vec::with_capacity(paint_count as usize);
        for _ in 0..paint_count {
            paint.push(read_paint_quad(reader)?);
        }
        paint.sort_unstable();
        reject_duplicate_or_overlapping_paint(&paint, &solid, index)?;
        components.push(ForgeComponent {
            resource,
            color_444,
            dimensions_q,
            offset_q,
            grip,
            solid,
            paint,
        });
    }
    Ok(components)
}

fn read_resource(reader: &mut BitReader<'_>, label: &'static str) -> Result<u8> {
    let value = reader.read(3, label)?;
    if value >= RESOURCE_COUNT {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "ncf1-resource",
            "NCF1 references an unknown forge resource.",
        ));
    }
    Ok(value as u8)
}

fn read_grip(reader: &mut BitReader<'_>, bits: u8) -> Result<Grip> {
    let offset_q = [
        reader.read_signed(bits, "grip offset x")? as i16,
        reader.read_signed(bits, "grip offset y")? as i16,
        reader.read_signed(bits, "grip offset z")? as i16,
    ];
    let packed = reader.read(3, "grip normal")? as u8;
    let axis = packed >> 1;
    if axis > 2 {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "ncf1-grip-axis",
            "NCF1 grip normal axis must be 0..=2.",
        ));
    }
    Ok(Grip {
        offset_q,
        axis,
        sign: if packed & 1 == 1 { 1 } else { -1 },
        rotation: reader.read(2, "grip rotation")? as u8,
    })
}

fn read_solid(reader: &mut BitReader<'_>) -> Result<Vec<u16>> {
    let mode = reader.read(2, "solid encoding mode")?;
    let occupancy = match mode {
        0 => read_runs(
            reader,
            usize::from(FORGE_CELL_COUNT),
            11,
            "solid voxel runs",
        )?,
        1 => vec![true; usize::from(FORGE_CELL_COUNT)],
        2 => read_cut_boxes(reader)?,
        3 => read_extruded_mask(reader)?,
        _ => unreachable!(),
    };
    Ok(occupancy
        .iter()
        .enumerate()
        .filter_map(|(index, value)| value.then_some(index as u16))
        .collect())
}

fn read_runs(
    reader: &mut BitReader<'_>,
    total: usize,
    length_bits: u8,
    label: &'static str,
) -> Result<Vec<bool>> {
    let mut output = vec![false; total];
    let mut value = reader.read(1, label)? == 1;
    let count = reader.read(length_bits, label)? as usize;
    if count == 0 {
        return Err(Error::new(
            ErrorKind::NonCanonical,
            "ncf1-empty-runs",
            "NCF1 run encoding must contain at least one run.",
        ));
    }
    let mut cursor = 0_usize;
    for _ in 0..count {
        let length = reader.read(length_bits, label)? as usize;
        let end = cursor
            .checked_add(length)
            .ok_or_else(|| Error::overflow("NCF1 run length overflow."))?;
        if length == 0 || end > total {
            return Err(Error::new(
                ErrorKind::OutOfBounds,
                "ncf1-invalid-runs",
                "NCF1 runs do not match their target size.",
            ));
        }
        if value {
            output[cursor..end].fill(true);
        }
        cursor = end;
        value = !value;
    }
    if cursor != total {
        return Err(Error::new(
            ErrorKind::Truncated,
            "ncf1-invalid-runs",
            "NCF1 runs are truncated before their target size.",
        ));
    }
    Ok(output)
}

fn read_cut_boxes(reader: &mut BitReader<'_>) -> Result<Vec<bool>> {
    let count = reader.read(5, "cut box count")?;
    if count == 0 {
        return Err(Error::new(
            ErrorKind::NonCanonical,
            "ncf1-cut-box-count",
            "NCF1 cut-box encoding requires at least one box.",
        ));
    }
    let mut solid = vec![true; usize::from(FORGE_CELL_COUNT)];
    let mut removed = vec![false; usize::from(FORGE_CELL_COUNT)];
    for _ in 0..count {
        let x = reader.read(4, "cut box x")? as u8;
        let y = reader.read(4, "cut box y")? as u8;
        let z = reader.read(4, "cut box z")? as u8;
        let sx = reader.read(4, "cut box width")? as u8;
        let sy = reader.read(4, "cut box height")? as u8;
        let sz = reader.read(4, "cut box depth")? as u8;
        if sx == 0
            || sy == 0
            || sz == 0
            || x.checked_add(sx).is_none_or(|end| end > FORGE_GRID_X)
            || y.checked_add(sy).is_none_or(|end| end > FORGE_GRID_Y)
            || z.checked_add(sz).is_none_or(|end| end > FORGE_GRID_Z)
        {
            return Err(Error::new(
                ErrorKind::OutOfBounds,
                "ncf1-cut-box",
                "NCF1 cut box is outside the component grid.",
            ));
        }
        for cz in z..z + sz {
            for cy in y..y + sy {
                for cx in x..x + sx {
                    let cell = usize::from(forge_cell_id(cx, cy, cz));
                    if removed[cell] {
                        return Err(Error::new(
                            ErrorKind::NonCanonical,
                            "ncf1-overlapping-cut-boxes",
                            "NCF1 cut boxes cannot overlap.",
                        ));
                    }
                    removed[cell] = true;
                    solid[cell] = false;
                }
            }
        }
    }
    Ok(solid)
}

fn read_extruded_mask(reader: &mut BitReader<'_>) -> Result<Vec<bool>> {
    let axis = reader.read(2, "extruded solid axis")? as usize;
    if axis > 2 {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "ncf1-extrude-axis",
            "NCF1 extruded solid axis must be 0..=2.",
        ));
    }
    let sizes = [
        usize::from(FORGE_GRID_X),
        usize::from(FORGE_GRID_Y),
        usize::from(FORGE_GRID_Z),
    ];
    let tangent = tangent_axes(axis);
    let mask = read_runs(
        reader,
        sizes[tangent[0]] * sizes[tangent[1]],
        8,
        "extruded solid mask runs",
    )?;
    let mut solid = vec![false; usize::from(FORGE_CELL_COUNT)];
    for layer in 0..sizes[axis] {
        for v in 0..sizes[tangent[1]] {
            for u in 0..sizes[tangent[0]] {
                let mut cell = [0_usize; 3];
                cell[axis] = layer;
                cell[tangent[0]] = u;
                cell[tangent[1]] = v;
                let id = usize::from(forge_cell_id(cell[0] as u8, cell[1] as u8, cell[2] as u8));
                solid[id] = mask[u + sizes[tangent[0]] * v];
            }
        }
    }
    Ok(solid)
}

fn read_paint_quad(reader: &mut BitReader<'_>) -> Result<PaintQuad> {
    let axis = reader.read(2, "paint axis")? as u8;
    if axis > 2 {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "ncf1-paint-axis",
            "NCF1 paint axis must be 0..=2.",
        ));
    }
    Ok(PaintQuad {
        axis,
        side: reader.read(1, "paint side")? as u8,
        plane: reader.read(4, "paint plane")? as u8,
        u0: reader.read(4, "paint u0")? as u8,
        u1: reader.read(4, "paint u1")? as u8,
        v0: reader.read(4, "paint v0")? as u8,
        v1: reader.read(4, "paint v1")? as u8,
        color_444: reader.read(12, "paint color")? as u16,
    })
}

fn read_appearance(reader: &mut BitReader<'_>, limits: &LimitsV1) -> Result<ForgeAppearance> {
    let dimensions_q = [
        (reader.read(9, "appearance dimension x")? as u16) * 2,
        (reader.read(9, "appearance dimension y")? as u16) * 2,
        (reader.read(9, "appearance dimension z")? as u16) * 2,
    ];
    if dimensions_q.iter().any(|value| *value < 2) {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "ncf1-appearance-dimensions",
            "NCF1 appearance dimensions must be non-zero.",
        ));
    }
    let grip = if reader.read(1, "appearance grip flag")? == 1 {
        Some(read_grip(reader, 11)?)
    } else {
        None
    };
    let count = reader.read(12, "appearance quad count")?;
    if count == 0 || count > limits.max_patches || count > limits.max_voxels {
        return Err(Error::limit(
            "ncf1-appearance-count",
            "NCF1 appearance quad count is empty or exceeds task limits.",
        ));
    }
    let palette = if reader.read(1, "appearance coordinate palette flag")? == 1 {
        let palette_count = reader.read(5, "appearance coordinate palette count")?;
        if palette_count == 0 {
            return Err(Error::new(
                ErrorKind::NonCanonical,
                "ncf1-coordinate-palette",
                "NCF1 coordinate palettes cannot be empty.",
            ));
        }
        let mut values = Vec::with_capacity(palette_count as usize);
        for _ in 0..palette_count {
            let value = reader.read(5, "appearance coordinate palette value")? as u8;
            if value > 24 || values.last().is_some_and(|previous| *previous >= value) {
                return Err(Error::new(
                    ErrorKind::NonCanonical,
                    "ncf1-coordinate-palette",
                    "NCF1 coordinate palette must be strictly sorted within 0..=24.",
                ));
            }
            values.push(value);
        }
        Some(values)
    } else {
        None
    };
    let mut quads = Vec::with_capacity(count as usize);
    for _ in 0..count {
        quads.push(read_appearance_quad(reader, palette.as_deref())?);
    }
    quads.sort_unstable();
    reject_overlapping_appearance(&quads)?;
    Ok(ForgeAppearance {
        dimensions_q,
        grip,
        quads,
    })
}

fn read_appearance_quad(
    reader: &mut BitReader<'_>,
    palette: Option<&[u8]>,
) -> Result<AppearanceQuad> {
    let first = reader.read(1, "appearance quad compression")?;
    if first == 0 {
        let header = read_appearance_header(reader, palette)?;
        return Ok(AppearanceQuad {
            u0: 0,
            u1: 24,
            v0: 0,
            v1: 24,
            ..header
        });
    }
    let general = reader.read(1, "appearance quad compression mode")?;
    let mut quad = read_appearance_header(reader, palette)?;
    if general == 1 {
        quad.u0 = read_appearance_coord(reader, palette)?;
        quad.u1 = read_appearance_coord(reader, palette)?;
        quad.v0 = read_appearance_coord(reader, palette)?;
        quad.v1 = read_appearance_coord(reader, palette)?;
    } else {
        let range_is_v = reader.read(1, "appearance quad range axis")? == 1;
        let start = read_appearance_coord(reader, palette)?;
        let end = read_appearance_coord(reader, palette)?;
        if range_is_v {
            quad.u0 = 0;
            quad.u1 = 24;
            quad.v0 = start;
            quad.v1 = end;
        } else {
            quad.u0 = start;
            quad.u1 = end;
            quad.v0 = 0;
            quad.v1 = 24;
        }
    }
    Ok(quad)
}

fn read_appearance_header(
    reader: &mut BitReader<'_>,
    palette: Option<&[u8]>,
) -> Result<AppearanceQuad> {
    let axis = reader.read(2, "appearance quad axis")? as u8;
    if axis > 2 {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "ncf1-appearance-axis",
            "NCF1 appearance quad axis must be 0..=2.",
        ));
    }
    Ok(AppearanceQuad {
        axis,
        side: reader.read(1, "appearance quad side")? as u8,
        resource: read_resource(reader, "appearance quad resource")?,
        plane: read_appearance_coord(reader, palette)?,
        u0: 0,
        u1: 0,
        v0: 0,
        v1: 0,
        color_444: reader.read(12, "appearance quad color")? as u16,
    })
}

fn read_appearance_coord(reader: &mut BitReader<'_>, palette: Option<&[u8]>) -> Result<u8> {
    match palette {
        None => {
            let value = reader.read(5, "appearance coordinate")? as u8;
            if value > 24 {
                return Err(Error::new(
                    ErrorKind::OutOfBounds,
                    "ncf1-appearance-coordinate",
                    "NCF1 appearance coordinates must be in 0..=24.",
                ));
            }
            Ok(value)
        }
        Some(values) => {
            let width = palette_bit_width(values.len());
            let index = reader.read(width, "appearance palette index")? as usize;
            values.get(index).copied().ok_or_else(|| {
                Error::new(
                    ErrorKind::OutOfBounds,
                    "ncf1-appearance-palette-index",
                    "NCF1 appearance quad references a missing palette coordinate.",
                )
            })
        }
    }
}

fn palette_bit_width(length: usize) -> u8 {
    let mut width = 0_u8;
    let mut value = length.saturating_sub(1);
    while value > 0 {
        width += 1;
        value >>= 1;
    }
    width.max(1)
}

fn reject_duplicate_or_overlapping_paint(
    quads: &[PaintQuad],
    solid: &[u16],
    _component_index: u32,
) -> Result<()> {
    let occupied: BTreeSet<u16> = solid.iter().copied().collect();
    for (index, quad) in quads.iter().enumerate() {
        validate_paint_surface(*quad, &occupied)?;
        for other in quads.iter().take(index) {
            if rectangles_overlap_paint(*quad, *other) {
                return Err(Error::new(
                    ErrorKind::NonCanonical,
                    "ncf1-overlapping-paint",
                    "NCF1 paint quads cannot overlap.",
                ));
            }
        }
    }
    Ok(())
}

fn validate_paint_surface(quad: PaintQuad, occupied: &BTreeSet<u16>) -> Result<()> {
    let sizes = [FORGE_GRID_X, FORGE_GRID_Y, FORGE_GRID_Z];
    let axis = usize::from(quad.axis);
    let tangent = tangent_axes(axis);
    if quad.axis > 2
        || quad.side > 1
        || quad.plane > sizes[axis]
        || quad.u0 >= quad.u1
        || quad.v0 >= quad.v1
        || quad.u1 > sizes[tangent[0]]
        || quad.v1 > sizes[tangent[1]]
    {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "ncf1-paint-range",
            "NCF1 paint quad is outside the component grid.",
        ));
    }
    let axis_cell = if quad.side == 1 {
        i16::from(quad.plane) - 1
    } else {
        i16::from(quad.plane)
    };
    if axis_cell < 0 || axis_cell >= i16::from(sizes[axis]) {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "ncf1-paint-surface",
            "NCF1 paint quad does not point at a component surface.",
        ));
    }
    let neighbor = axis_cell + if quad.side == 1 { 1 } else { -1 };
    for v in quad.v0..quad.v1 {
        for u in quad.u0..quad.u1 {
            let mut cell = [0_u8; 3];
            cell[axis] = axis_cell as u8;
            cell[tangent[0]] = u;
            cell[tangent[1]] = v;
            if !occupied.contains(&forge_cell_id(cell[0], cell[1], cell[2])) {
                return Err(Error::new(
                    ErrorKind::OutOfBounds,
                    "ncf1-paint-surface",
                    "NCF1 paint quad covers an empty cell.",
                ));
            }
            if neighbor >= 0 && neighbor < i16::from(sizes[axis]) {
                cell[axis] = neighbor as u8;
                if occupied.contains(&forge_cell_id(cell[0], cell[1], cell[2])) {
                    return Err(Error::new(
                        ErrorKind::OutOfBounds,
                        "ncf1-paint-surface",
                        "NCF1 paint quad covers an internal face.",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn rectangles_overlap_paint(left: PaintQuad, right: PaintQuad) -> bool {
    left.axis == right.axis
        && left.side == right.side
        && left.plane == right.plane
        && left.u0 < right.u1
        && left.u1 > right.u0
        && left.v0 < right.v1
        && left.v1 > right.v0
}

fn reject_overlapping_appearance(quads: &[AppearanceQuad]) -> Result<()> {
    for (index, quad) in quads.iter().enumerate() {
        if quad.plane > 24
            || quad.u0 >= quad.u1
            || quad.v0 >= quad.v1
            || quad.u1 > 24
            || quad.v1 > 24
        {
            return Err(Error::new(
                ErrorKind::OutOfBounds,
                "ncf1-appearance-range",
                "NCF1 appearance quad is outside the 24-cube grid.",
            ));
        }
        for other in quads.iter().take(index) {
            if quad.axis == other.axis
                && quad.side == other.side
                && quad.plane == other.plane
                && quad.u0 < other.u1
                && quad.u1 > other.u0
                && quad.v0 < other.v1
                && quad.v1 > other.v0
            {
                return Err(Error::new(
                    ErrorKind::NonCanonical,
                    "ncf1-overlapping-appearance",
                    "NCF1 appearance quads cannot overlap.",
                ));
            }
        }
    }
    Ok(())
}

fn tangent_axes(axis: usize) -> [usize; 2] {
    match axis {
        0 => [1, 2],
        1 => [0, 2],
        2 => [0, 1],
        _ => unreachable!(),
    }
}

struct BitReader<'a> {
    bytes: &'a [u8],
    bit_offset: usize,
}

impl<'a> BitReader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            bit_offset: 0,
        }
    }

    fn read(&mut self, bits: u8, label: &'static str) -> Result<u32> {
        let end = self
            .bit_offset
            .checked_add(usize::from(bits))
            .ok_or_else(|| Error::overflow("NCF1 bit offset overflow."))?;
        if end > self.bytes.len() * 8 {
            return Err(Error::new(ErrorKind::Truncated, "ncf1-truncated", label));
        }
        let mut value = 0_u32;
        for _ in 0..bits {
            let byte = self.bytes[self.bit_offset / 8];
            let bit = (byte >> (7 - self.bit_offset % 8)) & 1;
            value = (value << 1) | u32::from(bit);
            self.bit_offset += 1;
        }
        Ok(value)
    }

    fn read_signed(&mut self, bits: u8, label: &'static str) -> Result<i32> {
        let value = self.read(bits, label)?;
        let sign = 1_u32 << (bits - 1);
        if value >= sign {
            Ok((i64::from(value) - (1_i64 << bits)) as i32)
        } else {
            Ok(value as i32)
        }
    }

    fn finish(&mut self) -> Result<()> {
        let remaining = self.bytes.len() * 8 - self.bit_offset;
        if remaining > 7 {
            return Err(Error::new(
                ErrorKind::TrailingData,
                "ncf1-trailing-data",
                "NCF1 contains trailing bytes.",
            ));
        }
        if remaining > 0 && self.read(remaining as u8, "padding")? != 0 {
            return Err(Error::new(
                ErrorKind::NonCanonical,
                "ncf1-nonzero-padding",
                "NCF1 final-byte padding must be zero.",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_truncated_header_and_noncanonical_base64() {
        let limits = LimitsV1::default();
        assert_eq!(
            import_raw(&[0xf0], &limits).unwrap_err().kind,
            ErrorKind::Truncated
        );
        assert!(decode_canonical_base64(b"AA==").is_err());
    }

    #[test]
    fn bit_reader_is_msb_first_and_checks_padding() {
        let mut reader = BitReader::new(&[0b1011_0000]);
        assert_eq!(reader.read(4, "test").unwrap(), 0b1011);
        assert!(reader.finish().is_ok());
        let mut reader = BitReader::new(&[0b1011_0001]);
        reader.read(4, "test").unwrap();
        assert_eq!(reader.finish().unwrap_err().kind, ErrorKind::NonCanonical);
    }
}

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;
use core::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{Error, ErrorKind, Result};
use crate::varint::{write_i16, write_i32, write_u32};

pub const ABSOLUTE_MAX_INPUT_BYTES: u32 = 4 * 1024 * 1024;
pub const ABSOLUTE_MAX_COMMANDS: u32 = 16_384;
pub const ABSOLUTE_MAX_MATERIALS: u32 = 8_192;
pub const ABSOLUTE_MAX_PATCHES: u32 = 262_144;
pub const ABSOLUTE_MAX_VOXELS: u32 = 262_144;
pub const ABSOLUTE_MAX_WRITES: u32 = 1_048_576;
pub const ABSOLUTE_MAX_DECODE_UNITS: u64 = 8_000_000;
pub const ABSOLUTE_MAX_MEMORY_BYTES: u64 = 256 * 1024 * 1024;
pub const ABSOLUTE_MAX_EXPANDED_PER_OP: u32 = 262_144;

pub const TERRAIN_SIZE_X: u16 = 16;
pub const TERRAIN_SIZE_Y: u16 = 512;
pub const TERRAIN_SIZE_Z: u16 = 16;
pub const TERRAIN_UNIVERSE: u32 = 16 * 512 * 16;
pub const FORGE_GRID_X: u8 = 14;
pub const FORGE_GRID_Y: u8 = 10;
pub const FORGE_GRID_Z: u8 = 14;
pub const FORGE_CELL_COUNT: u16 = 14 * 10 * 14;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    TerrainDelta,
    Building,
    ForgedItem,
}

impl Profile {
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::TerrainDelta => 1,
            Self::Building => 2,
            Self::ForgedItem => 3,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TerrainDelta => "terrain_delta",
            Self::Building => "building",
            Self::ForgedItem => "forged_item",
        }
    }

    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Self::TerrainDelta),
            2 => Ok(Self::Building),
            3 => Ok(Self::ForgedItem),
            _ => Err(Error::invalid("unknown-profile", "Unknown PoUW profile.")),
        }
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl core::str::FromStr for Profile {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "terrain_delta" | "terrain" => Ok(Self::TerrainDelta),
            "building" => Ok(Self::Building),
            "forged_item" | "forged-item" | "forge" => Ok(Self::ForgedItem),
            _ => Err(Error::invalid(
                "unknown-profile",
                "Profile must be terrain_delta, building, or forged_item.",
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IncumbentFormat {
    ChunkBrokenV1,
    Ncm3V1,
    Ncf1V15,
    PouwVmV1,
    Ncm4PouwV1,
}

impl IncumbentFormat {
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::ChunkBrokenV1 => 1,
            Self::Ncm3V1 => 2,
            Self::Ncf1V15 => 3,
            Self::PouwVmV1 => 4,
            Self::Ncm4PouwV1 => 5,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ChunkBrokenV1 => "chunkbroken-v1",
            Self::Ncm3V1 => "ncm3-v1",
            Self::Ncf1V15 => "ncf1-v15",
            Self::PouwVmV1 => "pouw-vm-v1",
            Self::Ncm4PouwV1 => "ncm4-pouw-v1",
        }
    }

    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Self::ChunkBrokenV1),
            2 => Ok(Self::Ncm3V1),
            3 => Ok(Self::Ncf1V15),
            4 => Ok(Self::PouwVmV1),
            5 => Ok(Self::Ncm4PouwV1),
            _ => Err(Error::invalid(
                "unknown-incumbent-format",
                "Unknown incumbent encoding format.",
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitsV1 {
    pub max_input_bytes: u32,
    pub max_commands: u32,
    pub max_materials: u32,
    pub max_patches: u32,
    pub max_voxels: u32,
    pub max_writes: u32,
    pub max_decode_units: u64,
    pub max_memory_bytes: u64,
    pub max_expanded_per_op: u32,
}

impl Default for LimitsV1 {
    fn default() -> Self {
        Self {
            max_input_bytes: 1024 * 1024,
            max_commands: 4_096,
            max_materials: 4_096,
            max_patches: 131_072,
            max_voxels: 131_072,
            max_writes: 262_144,
            max_decode_units: 2_000_000,
            max_memory_bytes: 64 * 1024 * 1024,
            max_expanded_per_op: 131_072,
        }
    }
}

impl LimitsV1 {
    pub fn validate(&self) -> Result<()> {
        let bounded = self.max_input_bytes > 0
            && self.max_input_bytes <= ABSOLUTE_MAX_INPUT_BYTES
            && self.max_commands > 0
            && self.max_commands <= ABSOLUTE_MAX_COMMANDS
            && self.max_materials > 0
            && self.max_materials <= ABSOLUTE_MAX_MATERIALS
            && self.max_patches > 0
            && self.max_patches <= ABSOLUTE_MAX_PATCHES
            && self.max_voxels > 0
            && self.max_voxels <= ABSOLUTE_MAX_VOXELS
            && self.max_writes > 0
            && self.max_writes <= ABSOLUTE_MAX_WRITES
            && self.max_decode_units > 0
            && self.max_decode_units <= ABSOLUTE_MAX_DECODE_UNITS
            && self.max_memory_bytes >= 1024 * 1024
            && self.max_memory_bytes <= ABSOLUTE_MAX_MEMORY_BYTES
            && self.max_expanded_per_op > 0
            && self.max_expanded_per_op <= ABSOLUTE_MAX_EXPANDED_PER_OP;
        if !bounded {
            return Err(Error::limit(
                "invalid-limits",
                "Task limits exceed the protocol's absolute resource envelope.",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Coord {
    pub x: u16,
    pub y: u16,
    pub z: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Voxel {
    pub x: u16,
    pub y: u16,
    pub z: u16,
    pub material: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerrainSemantics {
    pub min_y: i16,
    pub deleted: Vec<Coord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildingSemantics {
    pub size: [u16; 3],
    pub voxels: Vec<Voxel>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grip {
    pub offset_q: [i16; 3],
    pub axis: u8,
    pub sign: i8,
    pub rotation: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaintQuad {
    pub axis: u8,
    pub side: u8,
    pub plane: u8,
    pub u0: u8,
    pub u1: u8,
    pub v0: u8,
    pub v1: u8,
    pub color_444: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceQuad {
    pub axis: u8,
    pub side: u8,
    pub resource: u8,
    pub plane: u8,
    pub u0: u8,
    pub u1: u8,
    pub v0: u8,
    pub v1: u8,
    pub color_444: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeEquipment {
    pub mass_5g: u16,
    pub encoded_volume: u16,
    pub attributes_6: [u8; 12],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeComponent {
    pub resource: u8,
    pub color_444: u16,
    pub dimensions_q: [u8; 3],
    pub offset_q: [i16; 3],
    pub grip: Option<Grip>,
    pub solid: Vec<u16>,
    pub paint: Vec<PaintQuad>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeAppearance {
    pub dimensions_q: [u16; 3],
    pub grip: Option<Grip>,
    pub quads: Vec<AppearanceQuad>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ForgeGeometry {
    Components { components: Vec<ForgeComponent> },
    Appearance { appearance: ForgeAppearance },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgedSemantics {
    pub equipment: ForgeEquipment,
    pub geometry: ForgeGeometry,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "profile", content = "semantics", rename_all = "snake_case")]
pub enum Semantics {
    TerrainDelta(TerrainSemantics),
    Building(BuildingSemantics),
    ForgedItem(ForgedSemantics),
}

impl Semantics {
    pub const fn profile(&self) -> Profile {
        match self {
            Self::TerrainDelta(_) => Profile::TerrainDelta,
            Self::Building(_) => Profile::Building,
            Self::ForgedItem(_) => Profile::ForgedItem,
        }
    }

    pub fn voxel_count(&self) -> usize {
        match self {
            Self::TerrainDelta(value) => value.deleted.len(),
            Self::Building(value) => value.voxels.len(),
            Self::ForgedItem(value) => match &value.geometry {
                ForgeGeometry::Components { components } => components
                    .iter()
                    .map(|component| component.solid.len())
                    .sum(),
                ForgeGeometry::Appearance { appearance } => appearance
                    .quads
                    .iter()
                    .map(|quad| usize::from(quad.u1 - quad.u0) * usize::from(quad.v1 - quad.v0))
                    .sum(),
            },
        }
    }

    pub fn validate(&self, limits: &LimitsV1) -> Result<()> {
        limits.validate()?;
        if self.voxel_count() > limits.max_voxels as usize {
            return Err(Error::limit(
                "voxel-limit",
                "Canonical semantics exceed the task voxel limit.",
            ));
        }
        match self {
            Self::TerrainDelta(value) => validate_terrain(value),
            Self::Building(value) => validate_building(value, limits),
            Self::ForgedItem(value) => validate_forged(value, limits),
        }
    }

    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut output = Vec::new();
        match self {
            Self::TerrainDelta(value) => {
                write_i16(&mut output, value.min_y);
                write_u32(&mut output, value.deleted.len() as u32);
                let mut previous = 0_u32;
                for (index, coord) in value.deleted.iter().enumerate() {
                    let id = terrain_coord_id(*coord);
                    write_u32(&mut output, if index == 0 { id } else { id - previous - 1 });
                    previous = id;
                }
            }
            Self::Building(value) => {
                for size in value.size {
                    write_u32(&mut output, u32::from(size));
                }
                write_u32(&mut output, value.voxels.len() as u32);
                let mut previous = 0_u32;
                for (index, voxel) in value.voxels.iter().enumerate() {
                    let id = building_coord_id(value.size, *voxel);
                    write_u32(&mut output, if index == 0 { id } else { id - previous - 1 });
                    write_u32(&mut output, u32::from(voxel.material));
                    previous = id;
                }
            }
            Self::ForgedItem(value) => encode_forged_semantics(&mut output, value),
        }
        output
    }

    pub fn mismatch_count(&self, other: &Self) -> u64 {
        match (self, other) {
            (Self::TerrainDelta(left), Self::TerrainDelta(right)) => {
                let left: BTreeSet<_> = left.deleted.iter().collect();
                let right: BTreeSet<_> = right.deleted.iter().collect();
                left.symmetric_difference(&right).count() as u64
                    + u64::from(left != right && left.is_empty() && right.is_empty())
                    + u64::from(self_min_y(self) != self_min_y(other))
            }
            (Self::Building(left), Self::Building(right)) => {
                let left_map: BTreeMap<_, _> = left
                    .voxels
                    .iter()
                    .map(|voxel| ((voxel.x, voxel.y, voxel.z), voxel.material))
                    .collect();
                let right_map: BTreeMap<_, _> = right
                    .voxels
                    .iter()
                    .map(|voxel| ((voxel.x, voxel.y, voxel.z), voxel.material))
                    .collect();
                let keys: BTreeSet<_> = left_map.keys().chain(right_map.keys()).collect();
                keys.iter()
                    .filter(|key| left_map.get(**key) != right_map.get(**key))
                    .count() as u64
                    + u64::from(left.size != right.size)
            }
            (Self::ForgedItem(left), Self::ForgedItem(right)) => forged_mismatch(left, right),
            _ => u64::MAX,
        }
    }
}

fn self_min_y(value: &Semantics) -> Option<i16> {
    match value {
        Semantics::TerrainDelta(terrain) => Some(terrain.min_y),
        _ => None,
    }
}

pub fn terrain_coord_id(coord: Coord) -> u32 {
    u32::from(coord.x) + 16 * (u32::from(coord.z) + 16 * u32::from(coord.y))
}

pub fn terrain_coord_from_id(id: u32) -> Result<Coord> {
    if id >= TERRAIN_UNIVERSE {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "terrain-coordinate-out-of-range",
            "Terrain coordinate identity exceeds the ChunkBroken envelope.",
        ));
    }
    Ok(Coord {
        x: (id & 15) as u16,
        z: ((id >> 4) & 15) as u16,
        y: ((id >> 8) & 511) as u16,
    })
}

pub fn building_coord_id(size: [u16; 3], voxel: Voxel) -> u32 {
    u32::from(voxel.x)
        + u32::from(size[0]) * (u32::from(voxel.z) + u32::from(size[2]) * u32::from(voxel.y))
}

pub fn building_coord_from_id(size: [u16; 3], id: u32, material: u16) -> Result<Voxel> {
    let volume = u32::from(size[0])
        .checked_mul(u32::from(size[1]))
        .and_then(|value| value.checked_mul(u32::from(size[2])))
        .ok_or_else(|| Error::overflow("Building volume overflow."))?;
    if id >= volume {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "building-coordinate-out-of-range",
            "Building coordinate identity exceeds declared dimensions.",
        ));
    }
    let x = id % u32::from(size[0]);
    let rest = id / u32::from(size[0]);
    let z = rest % u32::from(size[2]);
    let y = rest / u32::from(size[2]);
    Ok(Voxel {
        x: x as u16,
        y: y as u16,
        z: z as u16,
        material,
    })
}

pub fn forge_cell_id(x: u8, y: u8, z: u8) -> u16 {
    u16::from(x) + 14 * (u16::from(y) + 10 * u16::from(z))
}

pub fn forge_cell_from_id(id: u16) -> Result<[u8; 3]> {
    if id >= FORGE_CELL_COUNT {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "forge-cell-out-of-range",
            "Forge component cell exceeds the 14 x 10 x 14 grid.",
        ));
    }
    let x = id % 14;
    let rest = id / 14;
    let y = rest % 10;
    let z = rest / 10;
    Ok([x as u8, y as u8, z as u8])
}

fn validate_terrain(value: &TerrainSemantics) -> Result<()> {
    let mut previous = None;
    for coord in &value.deleted {
        if coord.x >= TERRAIN_SIZE_X || coord.y >= TERRAIN_SIZE_Y || coord.z >= TERRAIN_SIZE_Z {
            return Err(Error::new(
                ErrorKind::OutOfBounds,
                "terrain-coordinate-out-of-range",
                "Terrain coordinate exceeds the ChunkBroken v1 envelope.",
            ));
        }
        let id = terrain_coord_id(*coord);
        if previous.is_some_and(|item| item >= id) {
            return Err(Error::new(
                ErrorKind::NonCanonical,
                "terrain-order",
                "Terrain coordinates must be strictly sorted and unique.",
            ));
        }
        previous = Some(id);
    }
    Ok(())
}

fn validate_building(value: &BuildingSemantics, limits: &LimitsV1) -> Result<()> {
    if value.size.iter().any(|size| *size == 0 || *size > 512) {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "building-dimensions",
            "Building dimensions must be in 1..=512 for PoUW v1.",
        ));
    }
    let mut previous = None;
    let mut materials = BTreeSet::new();
    for voxel in &value.voxels {
        if voxel.x >= value.size[0]
            || voxel.y >= value.size[1]
            || voxel.z >= value.size[2]
            || voxel.material == 0
        {
            return Err(Error::new(
                ErrorKind::OutOfBounds,
                "building-voxel",
                "Building voxel is outside the declared dimensions or uses air as material.",
            ));
        }
        let id = building_coord_id(value.size, *voxel);
        if previous.is_some_and(|item| item >= id) {
            return Err(Error::new(
                ErrorKind::NonCanonical,
                "building-order",
                "Building voxels must be strictly coordinate sorted and unique.",
            ));
        }
        previous = Some(id);
        materials.insert(voxel.material);
    }
    if materials.len() > limits.max_materials as usize {
        return Err(Error::limit(
            "material-limit",
            "Building references too many materials.",
        ));
    }
    Ok(())
}

fn validate_forged(value: &ForgedSemantics, limits: &LimitsV1) -> Result<()> {
    if value.equipment.attributes_6.iter().any(|value| *value > 63) {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "forge-attribute",
            "Forge attributes must fit six bits.",
        ));
    }
    match &value.geometry {
        ForgeGeometry::Components { components } => {
            if components.is_empty() || components.len() > 31 {
                return Err(Error::new(
                    ErrorKind::OutOfBounds,
                    "forge-component-count",
                    "Forged component count must be in 1..=31.",
                ));
            }
            for component in components {
                validate_component(component)?;
            }
        }
        ForgeGeometry::Appearance { appearance } => {
            if appearance
                .dimensions_q
                .iter()
                .any(|value| *value < 2 || *value > 1022 || value % 2 != 0)
                || appearance.quads.is_empty()
                || appearance.quads.len() > 4095
            {
                return Err(Error::new(
                    ErrorKind::OutOfBounds,
                    "forge-appearance",
                    "Forged appearance metadata exceeds NCF1 v15 bounds.",
                ));
            }
            let mut previous = None;
            for quad in &appearance.quads {
                validate_appearance_quad(*quad)?;
                if previous.is_some_and(|item| item >= *quad) {
                    return Err(Error::new(
                        ErrorKind::NonCanonical,
                        "forge-quad-order",
                        "Appearance quads must be strictly sorted.",
                    ));
                }
                previous = Some(*quad);
            }
        }
    }
    if value.voxel_like_count() > limits.max_voxels as usize {
        return Err(Error::limit(
            "forge-geometry-limit",
            "Forged geometry exceeds the task limit.",
        ));
    }
    Ok(())
}

impl ForgedSemantics {
    fn voxel_like_count(&self) -> usize {
        match &self.geometry {
            ForgeGeometry::Components { components } => {
                components.iter().map(|item| item.solid.len()).sum()
            }
            ForgeGeometry::Appearance { appearance } => appearance.quads.len(),
        }
    }
}

fn validate_component(component: &ForgeComponent) -> Result<()> {
    if component.resource >= 6
        || component.color_444 > 0x0fff
        || component.dimensions_q.contains(&0)
        || component.solid.is_empty()
    {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "forge-component",
            "Forged component metadata or geometry is invalid.",
        ));
    }
    let mut previous = None;
    for cell in &component.solid {
        if *cell >= FORGE_CELL_COUNT || previous.is_some_and(|item| item >= *cell) {
            return Err(Error::new(
                ErrorKind::NonCanonical,
                "forge-solid-order",
                "Forged component cells must be strictly sorted and in range.",
            ));
        }
        previous = Some(*cell);
    }
    let mut previous_quad = None;
    for quad in &component.paint {
        validate_paint_quad(*quad)?;
        if previous_quad.is_some_and(|item| item >= *quad) {
            return Err(Error::new(
                ErrorKind::NonCanonical,
                "forge-paint-order",
                "Forged paint quads must be strictly sorted.",
            ));
        }
        previous_quad = Some(*quad);
    }
    validate_grip(component.grip)
}

fn validate_grip(grip: Option<Grip>) -> Result<()> {
    if let Some(grip) = grip {
        if grip.axis > 2 || !matches!(grip.sign, -1 | 1) || grip.rotation > 3 {
            return Err(Error::new(
                ErrorKind::OutOfBounds,
                "forge-grip",
                "Forged grip normal or rotation is invalid.",
            ));
        }
    }
    Ok(())
}

fn validate_paint_quad(quad: PaintQuad) -> Result<()> {
    let sizes = [FORGE_GRID_X, FORGE_GRID_Y, FORGE_GRID_Z];
    if quad.axis > 2 || quad.side > 1 || quad.color_444 > 0x0fff {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "forge-paint",
            "Invalid paint quad.",
        ));
    }
    let tangent: Vec<usize> = (0..3)
        .filter(|axis| *axis != usize::from(quad.axis))
        .collect();
    if quad.plane > sizes[usize::from(quad.axis)]
        || quad.u0 >= quad.u1
        || quad.v0 >= quad.v1
        || quad.u1 > sizes[tangent[0]]
        || quad.v1 > sizes[tangent[1]]
    {
        return Err(Error::new(
            ErrorKind::OutOfBounds,
            "forge-paint",
            "Paint quad is outside the component.",
        ));
    }
    Ok(())
}

fn validate_appearance_quad(quad: AppearanceQuad) -> Result<()> {
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
            "Appearance quad is outside the NCF1 v15 grid.",
        ));
    }
    Ok(())
}

fn encode_forged_semantics(output: &mut Vec<u8>, value: &ForgedSemantics) {
    output.extend_from_slice(&value.equipment.mass_5g.to_le_bytes());
    output.extend_from_slice(&value.equipment.encoded_volume.to_le_bytes());
    output.extend_from_slice(&value.equipment.attributes_6);
    match &value.geometry {
        ForgeGeometry::Components { components } => {
            output.push(0);
            write_u32(output, components.len() as u32);
            for component in components {
                encode_component(output, component);
            }
        }
        ForgeGeometry::Appearance { appearance } => {
            output.push(1);
            for dimension in appearance.dimensions_q {
                write_u32(output, u32::from(dimension));
            }
            encode_grip(output, appearance.grip);
            write_u32(output, appearance.quads.len() as u32);
            for quad in &appearance.quads {
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
        }
    }
}

fn encode_component(output: &mut Vec<u8>, component: &ForgeComponent) {
    output.push(component.resource);
    output.extend_from_slice(&component.color_444.to_le_bytes());
    output.extend_from_slice(&component.dimensions_q);
    for offset in component.offset_q {
        write_i16(output, offset);
    }
    encode_grip(output, component.grip);
    write_u32(output, component.solid.len() as u32);
    let mut previous = 0_u32;
    for (index, cell) in component.solid.iter().enumerate() {
        let cell = u32::from(*cell);
        write_u32(
            output,
            if index == 0 {
                cell
            } else {
                cell - previous - 1
            },
        );
        previous = cell;
    }
    write_u32(output, component.paint.len() as u32);
    for quad in &component.paint {
        output.extend_from_slice(&[
            quad.axis, quad.side, quad.plane, quad.u0, quad.u1, quad.v0, quad.v1,
        ]);
        output.extend_from_slice(&quad.color_444.to_le_bytes());
    }
}

fn encode_grip(output: &mut Vec<u8>, grip: Option<Grip>) {
    match grip {
        None => output.push(0),
        Some(grip) => {
            output.push(1);
            for offset in grip.offset_q {
                write_i32(output, i32::from(offset));
            }
            output.extend_from_slice(&[grip.axis, (grip.sign > 0) as u8, grip.rotation]);
        }
    }
}

fn forged_mismatch(left: &ForgedSemantics, right: &ForgedSemantics) -> u64 {
    let mut mismatch = u64::from(left.equipment != right.equipment);
    match (&left.geometry, &right.geometry) {
        (
            ForgeGeometry::Components { components: left },
            ForgeGeometry::Components { components: right },
        ) => {
            mismatch += left.len().abs_diff(right.len()) as u64;
            for (left, right) in left.iter().zip(right) {
                mismatch += u64::from(
                    left.resource != right.resource
                        || left.color_444 != right.color_444
                        || left.dimensions_q != right.dimensions_q
                        || left.offset_q != right.offset_q
                        || left.grip != right.grip
                        || left.paint != right.paint,
                );
                let left_cells: BTreeSet<_> = left.solid.iter().collect();
                let right_cells: BTreeSet<_> = right.solid.iter().collect();
                mismatch += left_cells.symmetric_difference(&right_cells).count() as u64;
            }
        }
        (
            ForgeGeometry::Appearance { appearance: left },
            ForgeGeometry::Appearance { appearance: right },
        ) => {
            mismatch +=
                u64::from(left.dimensions_q != right.dimensions_q || left.grip != right.grip);
            let left_quads: BTreeSet<_> = left.quads.iter().collect();
            let right_quads: BTreeSet<_> = right.quads.iter().collect();
            mismatch += left_quads.symmetric_difference(&right_quads).count() as u64;
        }
        _ => mismatch += 1,
    }
    mismatch
}

pub fn profile_from_semantics_name(value: &str) -> Result<Profile> {
    value
        .parse()
        .map_err(|_| Error::invalid("unknown-profile", "Unknown semantic profile name."))
}

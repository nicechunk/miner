mod building;
mod forged;
mod terrain;

use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::error::{Error, ErrorKind, Result};
use crate::model::{
    AppearanceQuad, ForgeEquipment, Grip, LimitsV1, PaintQuad, Profile, Semantics, Voxel,
};
use crate::{COST_MODEL_VERSION, VM_VERSION};

pub const VM_MAGIC: &[u8; 4] = b"NCPV";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VmStats {
    pub program_bytes: u32,
    pub residual_bytes: u32,
    pub overhead_bytes: u32,
    pub total_bytes: u32,
    pub commands: u32,
    pub patches: u32,
    pub writes: u32,
    pub decode_units: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodedCandidate {
    pub semantics: Semantics,
    pub stats: VmStats,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "profile", content = "program", rename_all = "snake_case")]
pub enum CandidateProgram {
    TerrainDelta(TerrainProgram),
    Building(BuildingProgram),
    ForgedItem(ForgeProgram),
}

impl CandidateProgram {
    pub const fn profile(&self) -> Profile {
        match self {
            Self::TerrainDelta(_) => Profile::TerrainDelta,
            Self::Building(_) => Profile::Building,
            Self::ForgedItem(_) => Profile::ForgedItem,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerrainProgram {
    pub min_y: i16,
    pub ops: Vec<TerrainOp>,
    pub patches: Vec<TerrainPatch>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum TerrainOp {
    DeleteRun {
        start: u32,
        length: u32,
    },
    DeleteBox {
        x: u16,
        y: u16,
        z: u16,
        width: u16,
        height: u16,
        depth: u16,
    },
    LayerBitmap {
        y: u16,
        bitmap: [u8; 32],
    },
    EliasFano {
        values: Vec<u32>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerrainPatchKind {
    Add,
    Restore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerrainPatch {
    pub id: u32,
    pub kind: TerrainPatchKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildingProgram {
    pub size: [u16; 3],
    pub ops: Vec<BuildingOp>,
    pub patches: Vec<BuildingPatch>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum BuildingOp {
    Box {
        material: u16,
        origin: [u16; 3],
        size: [u16; 3],
    },
    Run {
        material: u16,
        origin: [u16; 3],
        axis: u8,
        length: u16,
    },
    Wall {
        material: u16,
        origin: [u16; 3],
        normal_axis: u8,
        u_length: u16,
        v_length: u16,
        thickness: u16,
    },
    Extrude {
        material: u16,
        origin: [u16; 3],
        axis: u8,
        u_length: u16,
        v_length: u16,
        depth: u16,
        mask: Vec<bool>,
    },
    Repeat {
        material: u16,
        origin: [u16; 3],
        size: [u16; 3],
        count: u16,
        delta: [i32; 3],
    },
    Mirror {
        source_origin: [u16; 3],
        source_size: [u16; 3],
        axis: u8,
        pivot_twice: i32,
    },
    Cut {
        origin: [u16; 3],
        size: [u16; 3],
    },
    Literal {
        voxels: Vec<Voxel>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildingPatchKind {
    Set,
    Clear,
    Paint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildingPatch {
    pub id: u32,
    pub kind: BuildingPatchKind,
    pub material: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeProgram {
    pub equipment: ForgeEquipment,
    pub geometry: ForgeProgramGeometry,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ForgeProgramGeometry {
    Components {
        components: Vec<ForgeComponentProgram>,
    },
    Appearance {
        dimensions_q: [u16; 3],
        grip: Option<Grip>,
        quads: Vec<AppearanceQuad>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeComponentProgram {
    pub resource: u8,
    pub color_444: u16,
    pub dimensions_q: [u8; 3],
    pub offset_q: [i16; 3],
    pub grip: Option<Grip>,
    pub ops: Vec<ForgeSolidOp>,
    pub patches: Vec<ForgeSolidPatch>,
    pub paint: Vec<PaintQuad>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ForgeSolidOp {
    Solid,
    CutBox { origin: [u8; 3], size: [u8; 3] },
    Extrude { axis: u8, mask: Vec<bool> },
    Rle { occupancy: Vec<bool> },
    Symmetry { axis: u8 },
    Sparse { cells: Vec<u16> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForgePatchKind {
    Add,
    Clear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeSolidPatch {
    pub cell: u16,
    pub kind: ForgePatchKind,
}

pub fn encode_candidate(program: &CandidateProgram, limits: &LimitsV1) -> Result<Vec<u8>> {
    limits.validate()?;
    let mut output = Vec::new();
    output.extend_from_slice(VM_MAGIC);
    output.extend_from_slice(&[VM_VERSION, program.profile().as_u8(), COST_MODEL_VERSION, 0]);
    match program {
        CandidateProgram::TerrainDelta(value) => terrain::encode(&mut output, value, limits)?,
        CandidateProgram::Building(value) => building::encode(&mut output, value, limits)?,
        CandidateProgram::ForgedItem(value) => forged::encode(&mut output, value, limits)?,
    }
    if output.len() > limits.max_input_bytes as usize {
        return Err(Error::limit(
            "candidate-byte-limit",
            "Candidate encoding exceeds the task input-byte limit.",
        ));
    }
    let decoded = decode_candidate(&output, program.profile(), limits)?;
    if decoded.semantics.profile() != program.profile() {
        return Err(Error::new(
            ErrorKind::Internal,
            "candidate-profile",
            "Candidate encoder produced the wrong semantic profile.",
        ));
    }
    Ok(output)
}

pub fn decode_candidate(
    input: &[u8],
    expected: Profile,
    limits: &LimitsV1,
) -> Result<DecodedCandidate> {
    limits.validate()?;
    if input.len() > limits.max_input_bytes as usize {
        return Err(Error::limit(
            "candidate-byte-limit",
            "Candidate encoding exceeds the task input-byte limit.",
        ));
    }
    if input.len() < 8 {
        return Err(Error::new(
            ErrorKind::Truncated,
            "candidate-header",
            "Candidate encoding is shorter than its fixed header.",
        ));
    }
    if &input[0..4] != VM_MAGIC {
        return Err(Error::invalid(
            "candidate-magic",
            "Candidate encoding magic must be NCPV.",
        ));
    }
    if input[4] != VM_VERSION {
        return Err(Error::new(
            ErrorKind::UnsupportedVersion,
            "candidate-vm-version",
            "Candidate VM version is unsupported.",
        ));
    }
    let profile = Profile::from_u8(input[5])?;
    if profile != expected {
        return Err(Error::invalid(
            "candidate-profile",
            "Candidate profile does not match the task profile.",
        ));
    }
    if input[6] != COST_MODEL_VERSION {
        return Err(Error::new(
            ErrorKind::UnsupportedVersion,
            "candidate-cost-version",
            "Candidate cost-model version is unsupported.",
        ));
    }
    if input[7] != 0 {
        return Err(Error::new(
            ErrorKind::NonCanonical,
            "candidate-reserved",
            "Candidate reserved header byte must be zero.",
        ));
    }
    let mut cursor = Cursor::new(input, 8);
    let (semantics, mut stats) = match profile {
        Profile::TerrainDelta => terrain::decode(&mut cursor, limits)?,
        Profile::Building => building::decode(&mut cursor, limits)?,
        Profile::ForgedItem => forged::decode(&mut cursor, limits)?,
    };
    if cursor.offset != input.len() {
        return Err(Error::new(
            ErrorKind::TrailingData,
            "candidate-trailing-data",
            "Candidate encoding contains trailing bytes.",
        ));
    }
    semantics.validate(limits)?;
    stats.total_bytes = u32::try_from(input.len())
        .map_err(|_| Error::limit("candidate-byte-limit", "Candidate byte length exceeds u32."))?;
    stats.overhead_bytes = stats
        .total_bytes
        .checked_sub(stats.program_bytes)
        .and_then(|value| value.checked_sub(stats.residual_bytes))
        .ok_or_else(|| Error::overflow("Candidate byte accounting underflow."))?;
    if stats.commands > limits.max_commands
        || stats.patches > limits.max_patches
        || stats.writes > limits.max_writes
        || stats.decode_units > limits.max_decode_units
    {
        return Err(Error::limit(
            "candidate-resource-limit",
            "Candidate expansion exceeds task limits.",
        ));
    }
    Ok(DecodedCandidate { semantics, stats })
}

#[derive(Default)]
pub(super) struct StatsBuilder {
    pub program_bytes: u32,
    pub residual_bytes: u32,
    pub commands: u32,
    pub patches: u32,
    pub writes: u32,
    pub decode_units: u64,
}

impl StatsBuilder {
    pub fn add_command(&mut self, writes: u32, base_units: u64) -> Result<()> {
        self.commands = self
            .commands
            .checked_add(1)
            .ok_or_else(|| Error::overflow("VM command count overflow."))?;
        self.add_work(writes, base_units)
    }

    pub fn add_patch(&mut self, writes: u32, base_units: u64) -> Result<()> {
        self.patches = self
            .patches
            .checked_add(1)
            .ok_or_else(|| Error::overflow("VM patch count overflow."))?;
        self.add_work(writes, base_units)
    }

    fn add_work(&mut self, writes: u32, base_units: u64) -> Result<()> {
        self.writes = self
            .writes
            .checked_add(writes)
            .ok_or_else(|| Error::overflow("VM write count overflow."))?;
        self.decode_units = self
            .decode_units
            .checked_add(base_units)
            .and_then(|value| value.checked_add(u64::from(writes)))
            .ok_or_else(|| Error::overflow("VM decode-unit count overflow."))?;
        Ok(())
    }

    pub fn finish(self) -> VmStats {
        VmStats {
            program_bytes: self.program_bytes,
            residual_bytes: self.residual_bytes,
            overhead_bytes: 0,
            total_bytes: 0,
            commands: self.commands,
            patches: self.patches,
            writes: self.writes,
            decode_units: self.decode_units,
        }
    }
}

pub(super) struct Cursor<'a> {
    input: &'a [u8],
    pub offset: usize,
}

impl<'a> Cursor<'a> {
    pub const fn new(input: &'a [u8], offset: usize) -> Self {
        Self { input, offset }
    }

    pub fn byte(&mut self, label: &'static str) -> Result<u8> {
        let byte = *self
            .input
            .get(self.offset)
            .ok_or_else(|| Error::new(ErrorKind::Truncated, "candidate-truncated", label))?;
        self.offset += 1;
        Ok(byte)
    }

    pub fn bytes(&mut self, length: usize, label: &'static str) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| Error::overflow("Candidate byte offset overflow."))?;
        let bytes = self
            .input
            .get(self.offset..end)
            .ok_or_else(|| Error::new(ErrorKind::Truncated, "candidate-truncated", label))?;
        self.offset = end;
        Ok(bytes)
    }

    pub fn u32(&mut self) -> Result<u32> {
        crate::varint::read_u32(self.input, &mut self.offset)
    }

    pub fn i32(&mut self) -> Result<i32> {
        crate::varint::read_i32(self.input, &mut self.offset)
    }

    pub fn i16(&mut self) -> Result<i16> {
        crate::varint::read_i16(self.input, &mut self.offset)
    }
}

pub(super) fn checked_u16(value: u32, label: &'static str) -> Result<u16> {
    u16::try_from(value).map_err(|_| Error::new(ErrorKind::OutOfBounds, "vm-u16", label))
}

pub(super) fn write_bool_bits(output: &mut Vec<u8>, values: &[bool]) {
    let mut byte = 0_u8;
    for (index, value) in values.iter().enumerate() {
        if *value {
            byte |= 1 << (7 - (index % 8));
        }
        if index % 8 == 7 {
            output.push(byte);
            byte = 0;
        }
    }
    if values.len() % 8 != 0 {
        output.push(byte);
    }
}

pub(super) fn read_bool_bits(
    cursor: &mut Cursor<'_>,
    length: usize,
    label: &'static str,
) -> Result<Vec<bool>> {
    let byte_length = length
        .checked_add(7)
        .ok_or_else(|| Error::overflow("VM bitset length overflow."))?
        / 8;
    let bytes = cursor.bytes(byte_length, label)?;
    if length % 8 != 0 {
        let unused = 8 - length % 8;
        if bytes
            .last()
            .is_some_and(|byte| byte & ((1 << unused) - 1) != 0)
        {
            return Err(Error::new(
                ErrorKind::NonCanonical,
                "vm-bitset-padding",
                "VM bitset padding must be zero.",
            ));
        }
    }
    Ok((0..length)
        .map(|index| bytes[index / 8] & (1 << (7 - index % 8)) != 0)
        .collect())
}

pub(super) fn checked_volume(size: [u16; 3]) -> Result<u32> {
    u32::from(size[0])
        .checked_mul(u32::from(size[1]))
        .and_then(|value| value.checked_mul(u32::from(size[2])))
        .ok_or_else(|| Error::overflow("VM cuboid volume overflow."))
}

pub(super) fn ensure_count(count: u32, limit: u32, code: &'static str) -> Result<()> {
    if count > limit {
        return Err(Error::limit(
            code,
            "VM section count exceeds the task limit.",
        ));
    }
    Ok(())
}

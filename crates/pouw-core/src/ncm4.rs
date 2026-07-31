//! NCM4 PoUW profile (`NC4P`).
//!
//! Chunk.js already uses the public `NCM4:` prefix for an incompatible
//! character-animation record. This codec therefore has a distinct binary
//! magic and text prefix. NCM3 is imported as immutable source data and is
//! never reinterpreted as this format.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::error::{Error, ErrorKind, Result};
use crate::hash::{encoding_hash, semantic_root, Hash32};
use crate::import::{import_incumbent, ImportedAsset};
use crate::model::{
    building_coord_from_id, building_coord_id, BuildingSemantics, IncumbentFormat, LimitsV1,
    Profile, Semantics, Voxel,
};
use crate::varint::{read_u32, write_u32};

pub const NCM4_MAGIC: &[u8; 4] = b"NC4P";
pub const NCM4_TEXT_PREFIX: &str = "NCM4P:";
pub const NCM4_VERSION: u8 = 1;
pub const NCM4_FIXED_HEADER_BYTES: u32 = 8;

const CODEC_WRAPPED_SOURCE: u8 = 0;
const CODEC_COMPACT_BUILDING: u8 = 1;
const MAX_NCM3_BYTES: usize = 65_535;
const MAX_NCM3_COMMANDS: u32 = 4_096;
const MAX_REPEAT: u32 = 512;

const OP_BOX: u8 = 0;
const OP_REPEAT_BOX: u8 = 1;
const OP_GABLE: u8 = 2;
const OP_TREE: u8 = 3;
const OP_FENCE: u8 = 4;
const OP_RUN: u8 = 5;
const OP_WALL: u8 = 6;
const OP_EXTRUDE: u8 = 7;
const OP_TRANSLATE: u8 = 8;
const OP_ROTATE_Y: u8 = 9;
const OP_MIRROR: u8 = 10;
const OP_REPEAT_REGION: u8 = 11;
const OP_CLEAR_BOX: u8 = 12;

const RESIDUAL_NONE: u8 = 0;
const RESIDUAL_SPARSE: u8 = 1;
const RESIDUAL_RUNS: u8 = 2;
const RESIDUAL_BOXES: u8 = 3;
const RESIDUAL_LAYERS: u8 = 4;
const RESIDUAL_XOR: u8 = 5;
const RESIDUAL_MATERIAL_GROUPS: u8 = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectedFormat {
    ChunkBrokenV1,
    Ncm3V1,
    Ncf1V15,
    Ncm4PouwV1,
    PouwVmV1,
    Unknown,
}

impl DetectedFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ChunkBrokenV1 => "chunkbroken-v1",
            Self::Ncm3V1 => "ncm3-v1",
            Self::Ncf1V15 => "ncf1-v15",
            Self::Ncm4PouwV1 => "ncm4-pouw-v1",
            Self::PouwVmV1 => "pouw-vm-v1",
            Self::Unknown => "unknown",
        }
    }

    pub const fn profile(self) -> Option<Profile> {
        match self {
            Self::ChunkBrokenV1 => Some(Profile::TerrainDelta),
            Self::Ncm3V1 => Some(Profile::Building),
            Self::Ncf1V15 => Some(Profile::ForgedItem),
            Self::Ncm4PouwV1 | Self::PouwVmV1 | Self::Unknown => None,
        }
    }
}

pub fn detect_format(input: &[u8]) -> DetectedFormat {
    let input = trim_ascii(input);
    if input.starts_with(NCM4_MAGIC) || input.starts_with(NCM4_TEXT_PREFIX.as_bytes()) {
        DetectedFormat::Ncm4PouwV1
    } else if input.starts_with(b"NCM3:") {
        DetectedFormat::Ncm3V1
    } else if input.starts_with(b"NCBK") {
        DetectedFormat::ChunkBrokenV1
    } else if input.starts_with(b"NCF1.") {
        DetectedFormat::Ncf1V15
    } else if input.starts_with(crate::vm::VM_MAGIC) {
        DetectedFormat::PouwVmV1
    } else {
        DetectedFormat::Unknown
    }
}

pub fn looks_like_ncm4(input: &[u8]) -> bool {
    matches!(detect_format(input), DetectedFormat::Ncm4PouwV1)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ncm4Stats {
    pub fixed_header_bytes: u32,
    pub profile_header_bytes: u32,
    pub body_bytes: u32,
    pub residual_bytes: u32,
    pub total_bytes: u32,
    pub commands: u32,
    pub patches: u32,
    pub writes: u32,
    pub decode_units: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodedNcm4 {
    pub profile: Profile,
    pub semantics: Semantics,
    pub semantic_root: Hash32,
    pub encoding_hash: Hash32,
    pub stats: Ncm4Stats,
    #[serde(skip)]
    pub raw_encoding: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageAudit {
    pub source_format: String,
    pub profile: Profile,
    pub source_bytes: u32,
    pub fixed_header_bytes: u32,
    pub profile_header_bytes: u32,
    pub body_bytes: u32,
    pub residual_bytes: u32,
    pub ncm4_total_bytes: u32,
    pub theoretical_fixed_lower_bound: u32,
    pub deterministic_seed_bytes: u32,
    pub saved_bytes: i64,
    pub saved_basis_points: i64,
    pub semantic_root: Hash32,
    pub candidate_semantic_root: Hash32,
    pub exact: bool,
    pub witness_exists: bool,
    pub recommend_deep_search: bool,
    pub selected_format: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ncm4Seed {
    pub encoding: Vec<u8>,
    pub decoded: DecodedNcm4,
    pub audit: LanguageAudit,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Ncm4BuildingOp {
    Box {
        material: u16,
        origin: [u16; 3],
        size: [u16; 3],
    },
    RepeatBox {
        material: u16,
        origin: [u16; 3],
        size: [u16; 3],
        count: u16,
        delta: [i16; 3],
    },
    Gable {
        material: u16,
        origin: [u16; 3],
        width: u16,
        depth: u16,
        style: GableStyle,
        z_oriented: bool,
    },
    Tree {
        trunk_material: u16,
        leaf_material: u16,
        origin: [u16; 3],
        height: u16,
        crown: u16,
    },
    Fence {
        material: u16,
        origin: [u16; 3],
        length: u16,
        axis: u8,
        spacing: u16,
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
    Translate {
        source_origin: [u16; 3],
        source_size: [u16; 3],
        delta: [i16; 3],
    },
    RotateY {
        source_origin: [u16; 3],
        source_size: [u16; 3],
        destination_origin: [u16; 3],
        quarter_turns: u8,
    },
    Mirror {
        source_origin: [u16; 3],
        source_size: [u16; 3],
        destination_origin: [u16; 3],
        axis: u8,
    },
    RepeatRegion {
        source_origin: [u16; 3],
        source_size: [u16; 3],
        count: u16,
        delta: [i16; 3],
    },
    ClearBox {
        origin: [u16; 3],
        size: [u16; 3],
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GableStyle {
    Outline,
    Trim,
    Fill,
}

impl GableStyle {
    fn bits(self) -> u8 {
        match self {
            Self::Outline => 0,
            Self::Trim => 1,
            Self::Fill => 2,
        }
    }

    fn from_bits(value: u64) -> Result<Self> {
        match value {
            0 => Ok(Self::Outline),
            1 => Ok(Self::Trim),
            2 => Ok(Self::Fill),
            _ => Err(noncanonical(
                "ncm4-gable-style",
                "Reserved NCM4 gable style.",
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResidualKind {
    Set,
    Clear,
    Paint,
}

impl ResidualKind {
    fn byte(self) -> u8 {
        match self {
            Self::Set => 0,
            Self::Clear => 1,
            Self::Paint => 2,
        }
    }

    fn from_byte(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::Set),
            1 => Ok(Self::Clear),
            2 => Ok(Self::Paint),
            _ => Err(noncanonical(
                "ncm4-residual-kind",
                "Reserved NCM4 residual action.",
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ncm4Patch {
    pub id: u32,
    pub kind: ResidualKind,
    pub material: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "codec", content = "records", rename_all = "snake_case")]
pub enum Ncm4Residual {
    None,
    Sparse(Vec<Ncm4Patch>),
    Runs(Vec<Ncm4PatchRun>),
    Boxes(Vec<Ncm4PatchBox>),
    Layers(Vec<Ncm4PatchLayer>),
    Xor(Vec<Ncm4Patch>),
    MaterialGroups(Vec<Ncm4PatchGroup>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ncm4PatchRun {
    pub start: u32,
    pub length: u32,
    pub kind: ResidualKind,
    pub material: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ncm4PatchBox {
    pub origin: [u16; 3],
    pub size: [u16; 3],
    pub kind: ResidualKind,
    pub material: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ncm4PatchLayer {
    pub y: u16,
    pub kind: ResidualKind,
    pub material: u16,
    pub bitmap: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ncm4PatchGroup {
    pub kind: ResidualKind,
    pub material: u16,
    pub ids: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ncm4BuildingProgram {
    pub size: [u16; 3],
    pub palette: Vec<u16>,
    pub ops: Vec<Ncm4BuildingOp>,
    pub residual: Ncm4Residual,
}

pub fn decode_ncm4(input: &[u8], limits: &LimitsV1) -> Result<DecodedNcm4> {
    limits.validate()?;
    let raw = normalize_ncm4(input, limits)?;
    if raw.len() < NCM4_FIXED_HEADER_BYTES as usize {
        return Err(Error::new(
            ErrorKind::Truncated,
            "ncm4-header",
            "NCM4 encoding is shorter than its fixed header.",
        ));
    }
    if &raw[..4] != NCM4_MAGIC {
        return Err(Error::invalid("ncm4-magic", "NCM4 magic must be NC4P."));
    }
    if raw[4] != NCM4_VERSION {
        return Err(Error::new(
            ErrorKind::UnsupportedVersion,
            "ncm4-version",
            "Unsupported NCM4 PoUW version.",
        ));
    }
    let profile = Profile::from_u8(raw[5])?;
    if raw[6] != 0 {
        return Err(noncanonical(
            "ncm4-flags",
            "NCM4 reserved flags must be zero.",
        ));
    }
    let codec = raw[7];
    let (semantics, stats) = match codec {
        CODEC_WRAPPED_SOURCE => decode_wrapped(profile, &raw, limits)?,
        CODEC_COMPACT_BUILDING if profile == Profile::Building => {
            decode_compact_building(&raw, limits)?
        }
        CODEC_COMPACT_BUILDING => {
            return Err(Error::invalid(
                "ncm4-codec-profile",
                "Compact building codec requires the building profile.",
            ))
        }
        _ => {
            return Err(Error::new(
                ErrorKind::UnknownOpcode,
                "ncm4-codec",
                "Unknown NCM4 profile codec.",
            ))
        }
    };
    semantics.validate(limits)?;
    let root = semantic_root(&semantics);
    Ok(DecodedNcm4 {
        profile,
        encoding_hash: encoding_hash(profile, IncumbentFormat::Ncm4PouwV1, &raw),
        semantic_root: root,
        semantics,
        stats,
        raw_encoding: raw,
    })
}

pub fn deterministic_ncm4_seed(imported: &ImportedAsset, limits: &LimitsV1) -> Result<Ncm4Seed> {
    imported.semantics.validate(limits)?;
    let wrapped = encode_wrapped(imported, limits)?;
    let mut choices = vec![wrapped];
    if imported.profile == Profile::Building && imported.format == IncumbentFormat::Ncm3V1 {
        let program = transcode_ncm3(&imported.incumbent_encoding, limits)?;
        choices.push(encode_ncm4_building(&program, limits)?);
    }
    choices.sort_by(|left, right| left.len().cmp(&right.len()).then_with(|| left.cmp(right)));
    let encoding = choices
        .into_iter()
        .next()
        .ok_or_else(|| Error::new(ErrorKind::Internal, "ncm4-seed", "No NCM4 seed."))?;
    let decoded = decode_ncm4(&encoding, limits)?;
    let target_root = semantic_root(&imported.semantics);
    let exact = decoded.semantic_root == target_root
        && decoded.semantics.mismatch_count(&imported.semantics) == 0;
    if !exact {
        return Err(Error::new(
            ErrorKind::SemanticMismatch,
            "ncm4-seed-mismatch",
            "Deterministic NCM4 seed failed independent semantic verification.",
        ));
    }
    let source_bytes = imported.incumbent_encoding.len() as u32;
    let total = decoded.stats.total_bytes;
    let saved = i64::from(source_bytes) - i64::from(total);
    let saved_basis_points = if source_bytes == 0 {
        0
    } else {
        saved.saturating_mul(10_000) / i64::from(source_bytes)
    };
    let witness_exists = total < source_bytes;
    let audit = LanguageAudit {
        source_format: imported.format.as_str().to_string(),
        profile: imported.profile,
        source_bytes,
        fixed_header_bytes: decoded.stats.fixed_header_bytes,
        profile_header_bytes: decoded.stats.profile_header_bytes,
        body_bytes: decoded.stats.body_bytes,
        residual_bytes: decoded.stats.residual_bytes,
        ncm4_total_bytes: total,
        theoretical_fixed_lower_bound: NCM4_FIXED_HEADER_BYTES + 2,
        deterministic_seed_bytes: total,
        saved_bytes: saved,
        saved_basis_points,
        semantic_root: target_root,
        candidate_semantic_root: decoded.semantic_root,
        exact,
        witness_exists,
        recommend_deep_search: imported.profile == Profile::Building
            && (witness_exists || total <= source_bytes.saturating_add(16)),
        selected_format: if witness_exists {
            "ncm4-pouw-v1".into()
        } else {
            imported.format.as_str().into()
        },
    };
    Ok(Ncm4Seed {
        encoding,
        decoded,
        audit,
    })
}

pub fn deterministic_ncm4_building_program(
    imported: &ImportedAsset,
    limits: &LimitsV1,
) -> Result<Ncm4BuildingProgram> {
    if imported.profile != Profile::Building {
        return Err(Error::invalid(
            "ncm4-building-seed-source",
            "NCM4 building search requires a building source asset.",
        ));
    }
    let target = match &imported.semantics {
        Semantics::Building(target) => target,
        _ => {
            return Err(Error::invalid(
                "ncm4-building-seed-semantics",
                "NCM4 building search requires building semantics.",
            ))
        }
    };
    let program = match imported.format {
        IncumbentFormat::Ncm3V1 => transcode_ncm3(&imported.incumbent_encoding, limits)?,
        IncumbentFormat::Ncm4PouwV1 => {
            ncm4_building_seed_program(&imported.incumbent_encoding, target, limits)?
        }
        _ => {
            return Err(Error::invalid(
                "ncm4-building-seed-source",
                "NCM4 building search supports NCM3 and NCM4 PoUW source assets.",
            ))
        }
    };
    let program = exactify_ncm4_building(program, target, limits)?;
    let encoding = encode_ncm4_building(&program, limits)?;
    let decoded = decode_ncm4(&encoding, limits)?;
    if decoded.semantics != imported.semantics {
        return Err(Error::new(
            ErrorKind::SemanticMismatch,
            "ncm4-building-seed-mismatch",
            "NCM4 building seed is not exactly equivalent to its NCM3 source.",
        ));
    }
    Ok(program)
}

fn ncm4_building_seed_program(
    raw: &[u8],
    target: &BuildingSemantics,
    limits: &LimitsV1,
) -> Result<Ncm4BuildingProgram> {
    let decoded = decode_ncm4(raw, limits)?;
    if decoded.profile != Profile::Building {
        return Err(Error::invalid(
            "ncm4-building-seed-profile",
            "NCM4 source does not contain building semantics.",
        ));
    }
    if decoded.raw_encoding.get(7).copied() == Some(CODEC_COMPACT_BUILDING) {
        return decode_compact_building_program(&decoded.raw_encoding, limits);
    }
    let mut palette = target
        .voxels
        .iter()
        .map(|voxel| voxel.material)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if palette.is_empty() {
        // The compact grammar keeps one palette entry even when a residual
        // represents an empty building scene.
        palette.push(1);
    }
    Ok(Ncm4BuildingProgram {
        size: target.size,
        palette,
        ops: Vec::new(),
        residual: Ncm4Residual::None,
    })
}

fn decode_compact_building_program(raw: &[u8], limits: &LimitsV1) -> Result<Ncm4BuildingProgram> {
    let mut offset = NCM4_FIXED_HEADER_BYTES as usize;
    let size = [
        checked_dimension(read_u32(raw, &mut offset)?)?,
        checked_dimension(read_u32(raw, &mut offset)?)?,
        checked_dimension(read_u32(raw, &mut offset)?)?,
    ];
    let palette_len = read_u32(raw, &mut offset)?;
    if palette_len == 0 || palette_len > limits.max_materials {
        return Err(Error::limit(
            "ncm4-palette-limit",
            "NCM4 palette is empty or exceeds the material limit.",
        ));
    }
    let mut palette = Vec::with_capacity(palette_len as usize);
    for _ in 0..palette_len {
        palette.push(
            u16::try_from(read_u32(raw, &mut offset)?)
                .map_err(|_| out_of_bounds("ncm4-material", "NCM4 material exceeds u16."))?,
        );
    }
    let command_count = read_u32(raw, &mut offset)?;
    if command_count > limits.max_commands {
        return Err(Error::limit(
            "ncm4-command-limit",
            "NCM4 command count exceeds the configured limit.",
        ));
    }
    let widths = FieldWidths::new(size, palette.len())?;
    let mut bits = BitReader::new(raw, offset);
    let mut ops = Vec::with_capacity(command_count as usize);
    for _ in 0..command_count {
        let opcode = bits.read(4, "NCM4 building opcode is truncated.")? as u8;
        ops.push(decode_op(&mut bits, opcode, &palette, widths)?);
    }
    let _ = bits.align_zero()?;
    Ok(Ncm4BuildingProgram {
        size,
        palette,
        ops,
        residual: Ncm4Residual::None,
    })
}

pub fn encode_ncm4_building(program: &Ncm4BuildingProgram, limits: &LimitsV1) -> Result<Vec<u8>> {
    validate_program_header(program, limits)?;
    let mut output = Vec::new();
    output.extend_from_slice(NCM4_MAGIC);
    output.extend_from_slice(&[
        NCM4_VERSION,
        Profile::Building.as_u8(),
        0,
        CODEC_COMPACT_BUILDING,
    ]);
    for dimension in program.size {
        write_u32(&mut output, u32::from(dimension));
    }
    write_u32(&mut output, program.palette.len() as u32);
    for material in &program.palette {
        write_u32(&mut output, u32::from(*material));
    }
    write_u32(&mut output, program.ops.len() as u32);
    let widths = FieldWidths::new(program.size, program.palette.len())?;
    let mut bits = BitWriter::default();
    for op in &program.ops {
        encode_op(&mut bits, op, &program.palette, widths)?;
    }
    bits.finish(&mut output);
    encode_residual(&mut output, program, limits)?;
    if output.len() > limits.max_input_bytes as usize {
        return Err(Error::limit(
            "ncm4-input-limit",
            "NCM4 encoding exceeds the configured input limit.",
        ));
    }
    let decoded = decode_ncm4(&output, limits)?;
    if decoded.profile != Profile::Building {
        return Err(Error::new(
            ErrorKind::Internal,
            "ncm4-encoder-profile",
            "NCM4 encoder produced the wrong profile.",
        ));
    }
    Ok(output)
}

pub fn exactify_ncm4_building(
    mut program: Ncm4BuildingProgram,
    target: &BuildingSemantics,
    limits: &LimitsV1,
) -> Result<Ncm4BuildingProgram> {
    program.residual = Ncm4Residual::None;
    let structural = encode_ncm4_building(&program, limits)?;
    let decoded = decode_ncm4(&structural, limits)?;
    let Semantics::Building(base) = decoded.semantics else {
        return Err(Error::new(
            ErrorKind::Internal,
            "ncm4-exactify-profile",
            "NCM4 structural decoder returned the wrong profile.",
        ));
    };
    if base.size != target.size {
        return Err(Error::invalid(
            "ncm4-exactify-size",
            "NCM4 program and target dimensions differ.",
        ));
    }
    let base_map = base
        .voxels
        .iter()
        .map(|voxel| (building_coord_id(base.size, *voxel), voxel.material))
        .collect::<BTreeMap<_, _>>();
    let target_map = target
        .voxels
        .iter()
        .map(|voxel| (building_coord_id(target.size, *voxel), voxel.material))
        .collect::<BTreeMap<_, _>>();
    let ids = base_map
        .keys()
        .chain(target_map.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    let mut patches = Vec::new();
    for id in ids {
        let before = base_map.get(&id).copied();
        let after = target_map.get(&id).copied();
        if before == after {
            continue;
        }
        let (kind, material) = match (before, after) {
            (None, Some(material)) => (ResidualKind::Set, material),
            (Some(_), None) => (ResidualKind::Clear, 0),
            (Some(_), Some(material)) => (ResidualKind::Paint, material),
            (None, None) => continue,
        };
        if material != 0 && program.palette.binary_search(&material).is_err() {
            program.palette.push(material);
        }
        patches.push(Ncm4Patch { id, kind, material });
    }
    program.palette.sort_unstable();
    program.palette.dedup();
    if patches.is_empty() {
        program.residual = Ncm4Residual::None;
        return Ok(program);
    }
    let candidates = residual_candidates(program.size, &patches);
    let mut best = None::<(usize, u8, Ncm4Residual)>;
    for residual in candidates {
        program.residual = residual.clone();
        let bytes = encode_ncm4_building(&program, limits)?;
        let tag = residual_tag(&residual);
        let key = (bytes.len(), tag);
        if best
            .as_ref()
            .is_none_or(|current| key < (current.0, current.1))
        {
            best = Some((bytes.len(), tag, residual));
        }
    }
    program.residual = best
        .map(|value| value.2)
        .ok_or_else(|| Error::new(ErrorKind::Internal, "ncm4-residual", "No residual codec."))?;
    let encoding = encode_ncm4_building(&program, limits)?;
    let decoded = decode_ncm4(&encoding, limits)?;
    if decoded.semantics != Semantics::Building(target.clone()) {
        return Err(Error::new(
            ErrorKind::SemanticMismatch,
            "ncm4-exactify-mismatch",
            "NCM4 exact residual did not reproduce the target.",
        ));
    }
    Ok(program)
}

fn encode_wrapped(imported: &ImportedAsset, limits: &LimitsV1) -> Result<Vec<u8>> {
    let wrapped_format = match (imported.profile, imported.format) {
        (Profile::TerrainDelta, IncumbentFormat::ChunkBrokenV1) => 1,
        (Profile::Building, IncumbentFormat::Ncm3V1) => 2,
        (Profile::ForgedItem, IncumbentFormat::Ncf1V15) => 3,
        _ => {
            return Err(Error::invalid(
                "ncm4-wrapped-format",
                "NCM4 wrapper supports only the three audited source formats.",
            ))
        }
    };
    let mut output = Vec::new();
    output.extend_from_slice(NCM4_MAGIC);
    output.extend_from_slice(&[
        NCM4_VERSION,
        imported.profile.as_u8(),
        0,
        CODEC_WRAPPED_SOURCE,
    ]);
    output.push(wrapped_format);
    write_u32(&mut output, imported.incumbent_encoding.len() as u32);
    output.extend_from_slice(&imported.incumbent_encoding);
    if output.len() > limits.max_input_bytes as usize {
        return Err(Error::limit(
            "ncm4-input-limit",
            "Wrapped NCM4 encoding exceeds the configured input limit.",
        ));
    }
    Ok(output)
}

fn decode_wrapped(
    profile: Profile,
    raw: &[u8],
    limits: &LimitsV1,
) -> Result<(Semantics, Ncm4Stats)> {
    let mut offset = NCM4_FIXED_HEADER_BYTES as usize;
    let code = read_byte(raw, &mut offset, "NCM4 wrapped format")?;
    let format = match code {
        1 => IncumbentFormat::ChunkBrokenV1,
        2 => IncumbentFormat::Ncm3V1,
        3 => IncumbentFormat::Ncf1V15,
        _ => {
            return Err(Error::new(
                ErrorKind::UnknownOpcode,
                "ncm4-wrapped-format",
                "Unknown wrapped source format.",
            ))
        }
    };
    let length = read_u32(raw, &mut offset)? as usize;
    let end = offset
        .checked_add(length)
        .ok_or_else(|| Error::overflow("NCM4 wrapped length overflow."))?;
    let payload = raw.get(offset..end).ok_or_else(|| {
        Error::new(
            ErrorKind::Truncated,
            "ncm4-wrapped-truncated",
            "NCM4 wrapped source is truncated.",
        )
    })?;
    if end != raw.len() {
        return Err(Error::new(
            ErrorKind::TrailingData,
            "ncm4-trailing-data",
            "NCM4 wrapped source has trailing bytes.",
        ));
    }
    let semantics = import_incumbent(profile, format, payload, limits)?;
    Ok((
        semantics,
        Ncm4Stats {
            fixed_header_bytes: NCM4_FIXED_HEADER_BYTES,
            profile_header_bytes: (offset - NCM4_FIXED_HEADER_BYTES as usize) as u32,
            body_bytes: length as u32,
            residual_bytes: 0,
            total_bytes: raw.len() as u32,
            commands: 1,
            patches: 0,
            writes: 0,
            decode_units: length as u64,
        },
    ))
}

fn normalize_ncm4(input: &[u8], limits: &LimitsV1) -> Result<Vec<u8>> {
    let text_input = trim_ascii(input);
    let raw = if input.starts_with(NCM4_MAGIC) {
        input.to_vec()
    } else if text_input.starts_with(NCM4_TEXT_PREFIX.as_bytes()) {
        let encoded =
            core::str::from_utf8(&text_input[NCM4_TEXT_PREFIX.len()..]).map_err(|_| {
                Error::new(
                    ErrorKind::NonCanonical,
                    "ncm4-base64",
                    "NCM4 text must use ASCII Base64URL.",
                )
            })?;
        if encoded.is_empty()
            || encoded.len() % 4 == 1
            || encoded
                .bytes()
                .any(|byte| !byte.is_ascii_alphanumeric() && byte != b'-' && byte != b'_')
        {
            return Err(noncanonical(
                "ncm4-base64",
                "NCM4 requires canonical unpadded Base64URL.",
            ));
        }
        let decoded = URL_SAFE_NO_PAD.decode(encoded.as_bytes()).map_err(|_| {
            noncanonical("ncm4-base64", "NCM4 requires canonical unpadded Base64URL.")
        })?;
        if URL_SAFE_NO_PAD.encode(&decoded) != encoded {
            return Err(noncanonical(
                "ncm4-base64",
                "NCM4 Base64URL text is not canonical.",
            ));
        }
        decoded
    } else if text_input.starts_with(NCM4_MAGIC) {
        return Err(Error::new(
            ErrorKind::TrailingData,
            "ncm4-binary-whitespace",
            "Binary NCM4 cannot contain leading or trailing whitespace.",
        ));
    } else {
        text_input.to_vec()
    };
    if raw.len() > limits.max_input_bytes as usize {
        return Err(Error::limit(
            "ncm4-input-limit",
            "NCM4 encoding exceeds the configured input limit.",
        ));
    }
    Ok(raw)
}

pub fn ncm4_to_text(raw: &[u8], limits: &LimitsV1) -> Result<String> {
    let decoded = decode_ncm4(raw, limits)?;
    Ok(format_ncm4_text(&decoded.raw_encoding))
}

fn format_ncm4_text(raw: &[u8]) -> String {
    let mut value = NCM4_TEXT_PREFIX.to_string();
    value.push_str(&URL_SAFE_NO_PAD.encode(raw));
    value
}

#[derive(Clone, Copy)]
struct FieldWidths {
    coordinate: [u8; 3],
    material: u8,
}

impl FieldWidths {
    fn new(size: [u16; 3], palette_len: usize) -> Result<Self> {
        if palette_len == 0 {
            return Err(Error::invalid(
                "ncm4-palette",
                "NCM4 building palette cannot be empty.",
            ));
        }
        Ok(Self {
            coordinate: [
                bits_needed(u64::from(size[0] - 1)),
                bits_needed(u64::from(size[1] - 1)),
                bits_needed(u64::from(size[2] - 1)),
            ],
            material: bits_needed((palette_len - 1) as u64),
        })
    }
}

#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    bit_len: usize,
}

impl BitWriter {
    fn write(&mut self, value: u64, width: u8) -> Result<()> {
        if width == 0 || width > 63 || value >= (1_u64 << width) {
            return Err(Error::new(
                ErrorKind::OutOfBounds,
                "ncm4-bit-value",
                "NCM4 fixed-width field does not fit its canonical width.",
            ));
        }
        for shift in (0..width).rev() {
            if self.bit_len % 8 == 0 {
                self.bytes.push(0);
            }
            if (value >> shift) & 1 != 0 {
                let index = self.bytes.len() - 1;
                self.bytes[index] |= 1 << (7 - self.bit_len % 8);
            }
            self.bit_len += 1;
        }
        Ok(())
    }

    fn finish(self, output: &mut Vec<u8>) {
        output.extend_from_slice(&self.bytes);
    }
}

struct BitReader<'a> {
    input: &'a [u8],
    bit_offset: usize,
}

impl<'a> BitReader<'a> {
    fn new(input: &'a [u8], byte_offset: usize) -> Self {
        Self {
            input,
            bit_offset: byte_offset.saturating_mul(8),
        }
    }

    fn read(&mut self, width: u8, label: &'static str) -> Result<u64> {
        if width == 0 || width > 63 {
            return Err(Error::new(
                ErrorKind::Internal,
                "ncm4-bit-width",
                "Invalid NCM4 bit width.",
            ));
        }
        let end = self
            .bit_offset
            .checked_add(usize::from(width))
            .ok_or_else(|| Error::overflow("NCM4 bit offset overflow."))?;
        if end > self.input.len().saturating_mul(8) {
            return Err(Error::new(ErrorKind::Truncated, "ncm4-truncated", label));
        }
        let mut value = 0_u64;
        for _ in 0..width {
            let byte = self.input[self.bit_offset / 8];
            let bit = (byte >> (7 - self.bit_offset % 8)) & 1;
            value = (value << 1) | u64::from(bit);
            self.bit_offset += 1;
        }
        Ok(value)
    }

    fn align_zero(&mut self) -> Result<usize> {
        while self.bit_offset % 8 != 0 {
            if self.read(1, "NCM4 command padding is truncated.")? != 0 {
                return Err(noncanonical(
                    "ncm4-command-padding",
                    "NCM4 command padding bits must be zero.",
                ));
            }
        }
        Ok(self.bit_offset / 8)
    }
}

fn encode_op(
    bits: &mut BitWriter,
    op: &Ncm4BuildingOp,
    palette: &[u16],
    widths: FieldWidths,
) -> Result<()> {
    match op {
        Ncm4BuildingOp::Box {
            material,
            origin,
            size,
        } => {
            bits.write(u64::from(OP_BOX), 4)?;
            write_material_bits(bits, palette, *material, widths)?;
            write_origin_bits(bits, *origin, widths)?;
            write_size_bits(bits, *size, widths)?;
        }
        Ncm4BuildingOp::RepeatBox {
            material,
            origin,
            size,
            count,
            delta,
        } => {
            if !(2..=MAX_REPEAT as u16).contains(count) {
                return Err(out_of_bounds(
                    "ncm4-repeat-count",
                    "NCM4 repeat count must be in 2..=512.",
                ));
            }
            bits.write(u64::from(OP_REPEAT_BOX), 4)?;
            write_material_bits(bits, palette, *material, widths)?;
            write_origin_bits(bits, *origin, widths)?;
            write_size_bits(bits, *size, widths)?;
            bits.write(u64::from(*count - 1), 9)?;
            write_delta_bits(bits, *delta)?;
        }
        Ncm4BuildingOp::Gable {
            material,
            origin,
            width,
            depth,
            style,
            z_oriented,
        } => {
            bits.write(u64::from(OP_GABLE), 4)?;
            write_material_bits(bits, palette, *material, widths)?;
            bits.write(u64::from(style.bits()), 2)?;
            bits.write(u64::from(*z_oriented), 1)?;
            write_origin_bits(bits, *origin, widths)?;
            write_length_bits(bits, *width, widths.coordinate[0])?;
            write_length_bits(bits, *depth, widths.coordinate[2])?;
        }
        Ncm4BuildingOp::Tree {
            trunk_material,
            leaf_material,
            origin,
            height,
            crown,
        } => {
            if !(2..=64).contains(height) || !(1..=16).contains(crown) {
                return Err(out_of_bounds(
                    "ncm4-tree",
                    "NCM4 tree height or crown exceeds its bounded envelope.",
                ));
            }
            bits.write(u64::from(OP_TREE), 4)?;
            write_material_bits(bits, palette, *trunk_material, widths)?;
            write_material_bits(bits, palette, *leaf_material, widths)?;
            write_origin_bits(bits, *origin, widths)?;
            bits.write(u64::from(*height - 2), 6)?;
            bits.write(u64::from(*crown - 1), 4)?;
        }
        Ncm4BuildingOp::Fence {
            material,
            origin,
            length,
            axis,
            spacing,
        } => {
            if *axis > 1 || !(1..=64).contains(spacing) {
                return Err(out_of_bounds(
                    "ncm4-fence",
                    "NCM4 fence axis or spacing is invalid.",
                ));
            }
            bits.write(u64::from(OP_FENCE), 4)?;
            write_material_bits(bits, palette, *material, widths)?;
            write_origin_bits(bits, *origin, widths)?;
            bits.write(u64::from(*axis), 1)?;
            let dimension_axis = if *axis == 0 { 0 } else { 2 };
            write_length_bits(bits, *length, widths.coordinate[dimension_axis])?;
            bits.write(u64::from(*spacing - 1), 6)?;
        }
        Ncm4BuildingOp::Run {
            material,
            origin,
            axis,
            length,
        } => {
            let axis = checked_axis(*axis)?;
            bits.write(u64::from(OP_RUN), 4)?;
            write_material_bits(bits, palette, *material, widths)?;
            write_origin_bits(bits, *origin, widths)?;
            bits.write(axis as u64, 2)?;
            write_length_bits(bits, *length, widths.coordinate[axis])?;
        }
        Ncm4BuildingOp::Wall {
            material,
            origin,
            normal_axis,
            u_length,
            v_length,
            thickness,
        } => {
            let normal = checked_axis(*normal_axis)?;
            let tangent = tangent_axes(normal);
            bits.write(u64::from(OP_WALL), 4)?;
            write_material_bits(bits, palette, *material, widths)?;
            write_origin_bits(bits, *origin, widths)?;
            bits.write(normal as u64, 2)?;
            write_length_bits(bits, *u_length, widths.coordinate[tangent[0]])?;
            write_length_bits(bits, *v_length, widths.coordinate[tangent[1]])?;
            write_length_bits(bits, *thickness, widths.coordinate[normal])?;
        }
        Ncm4BuildingOp::Extrude {
            material,
            origin,
            axis,
            u_length,
            v_length,
            depth,
            mask,
        } => {
            let axis = checked_axis(*axis)?;
            let tangent = tangent_axes(axis);
            let mask_len = usize::from(*u_length)
                .checked_mul(usize::from(*v_length))
                .ok_or_else(|| Error::overflow("NCM4 extrude mask overflow."))?;
            if mask_len == 0 || mask.len() != mask_len || !mask.iter().any(|value| *value) {
                return Err(noncanonical(
                    "ncm4-extrude-mask",
                    "NCM4 extrude mask must be non-empty and match its dimensions.",
                ));
            }
            bits.write(u64::from(OP_EXTRUDE), 4)?;
            write_material_bits(bits, palette, *material, widths)?;
            write_origin_bits(bits, *origin, widths)?;
            bits.write(axis as u64, 2)?;
            write_length_bits(bits, *u_length, widths.coordinate[tangent[0]])?;
            write_length_bits(bits, *v_length, widths.coordinate[tangent[1]])?;
            write_length_bits(bits, *depth, widths.coordinate[axis])?;
            for occupied in mask {
                bits.write(u64::from(*occupied), 1)?;
            }
        }
        Ncm4BuildingOp::Translate {
            source_origin,
            source_size,
            delta,
        } => {
            if *delta == [0, 0, 0] {
                return Err(noncanonical(
                    "ncm4-translate-noop",
                    "NCM4 translation cannot use a zero delta.",
                ));
            }
            bits.write(u64::from(OP_TRANSLATE), 4)?;
            write_origin_bits(bits, *source_origin, widths)?;
            write_size_bits(bits, *source_size, widths)?;
            write_delta_bits(bits, *delta)?;
        }
        Ncm4BuildingOp::RotateY {
            source_origin,
            source_size,
            destination_origin,
            quarter_turns,
        } => {
            if !(1..=3).contains(quarter_turns) {
                return Err(noncanonical(
                    "ncm4-rotation-noop",
                    "NCM4 rotation must use one to three quarter turns.",
                ));
            }
            bits.write(u64::from(OP_ROTATE_Y), 4)?;
            write_origin_bits(bits, *source_origin, widths)?;
            write_size_bits(bits, *source_size, widths)?;
            write_origin_bits(bits, *destination_origin, widths)?;
            bits.write(u64::from(*quarter_turns), 2)?;
        }
        Ncm4BuildingOp::Mirror {
            source_origin,
            source_size,
            destination_origin,
            axis,
        } => {
            let axis = checked_axis(*axis)?;
            bits.write(u64::from(OP_MIRROR), 4)?;
            write_origin_bits(bits, *source_origin, widths)?;
            write_size_bits(bits, *source_size, widths)?;
            write_origin_bits(bits, *destination_origin, widths)?;
            bits.write(axis as u64, 2)?;
        }
        Ncm4BuildingOp::RepeatRegion {
            source_origin,
            source_size,
            count,
            delta,
        } => {
            if !(2..=MAX_REPEAT as u16).contains(count) || *delta == [0, 0, 0] {
                return Err(out_of_bounds(
                    "ncm4-repeat-region",
                    "NCM4 region repeat requires count 2..=512 and non-zero delta.",
                ));
            }
            bits.write(u64::from(OP_REPEAT_REGION), 4)?;
            write_origin_bits(bits, *source_origin, widths)?;
            write_size_bits(bits, *source_size, widths)?;
            bits.write(u64::from(*count - 1), 9)?;
            write_delta_bits(bits, *delta)?;
        }
        Ncm4BuildingOp::ClearBox { origin, size } => {
            bits.write(u64::from(OP_CLEAR_BOX), 4)?;
            write_origin_bits(bits, *origin, widths)?;
            write_size_bits(bits, *size, widths)?;
        }
    }
    Ok(())
}

fn decode_compact_building(raw: &[u8], limits: &LimitsV1) -> Result<(Semantics, Ncm4Stats)> {
    let mut offset = NCM4_FIXED_HEADER_BYTES as usize;
    let size = [
        checked_dimension(read_u32(raw, &mut offset)?)?,
        checked_dimension(read_u32(raw, &mut offset)?)?,
        checked_dimension(read_u32(raw, &mut offset)?)?,
    ];
    let volume = building_volume(size)?;
    let palette_len = read_u32(raw, &mut offset)?;
    if palette_len == 0 || palette_len > limits.max_materials {
        return Err(Error::limit(
            "ncm4-palette-limit",
            "NCM4 palette is empty or exceeds the material limit.",
        ));
    }
    let mut palette = Vec::with_capacity(palette_len as usize);
    for _ in 0..palette_len {
        let material = u16::try_from(read_u32(raw, &mut offset)?)
            .map_err(|_| out_of_bounds("ncm4-material", "NCM4 material exceeds u16."))?;
        if material == 0 || palette.last().is_some_and(|previous| *previous >= material) {
            return Err(noncanonical(
                "ncm4-palette-order",
                "NCM4 palette must be strictly sorted and exclude air.",
            ));
        }
        palette.push(material);
    }
    let command_count = read_u32(raw, &mut offset)?;
    if command_count > limits.max_commands {
        return Err(Error::limit(
            "ncm4-command-limit",
            "NCM4 command count exceeds the configured limit.",
        ));
    }
    let header_end = offset;
    let widths = FieldWidths::new(size, palette.len())?;
    let mut bits = BitReader::new(raw, offset);
    let mut voxels = BTreeMap::<u32, u16>::new();
    let mut writes = 0_u64;
    let mut decode_units = 0_u64;
    for _ in 0..command_count {
        let opcode = bits.read(4, "NCM4 building opcode is truncated.")? as u8;
        let op = decode_op(&mut bits, opcode, &palette, widths)?;
        execute_op(
            &op,
            size,
            &mut voxels,
            &mut writes,
            &mut decode_units,
            limits,
        )?;
        if voxels.len() > limits.max_voxels as usize {
            return Err(Error::limit(
                "ncm4-voxel-limit",
                "NCM4 command expansion exceeds the voxel limit.",
            ));
        }
    }
    offset = bits.align_zero()?;
    let body_end = offset;
    let patch_count = decode_residual(
        raw,
        &mut offset,
        size,
        volume,
        &palette,
        &mut voxels,
        &mut writes,
        &mut decode_units,
        limits,
    )?;
    if offset != raw.len() {
        return Err(Error::new(
            ErrorKind::TrailingData,
            "ncm4-trailing-data",
            "NCM4 compact building contains trailing bytes.",
        ));
    }
    let output = voxels
        .into_iter()
        .map(|(id, material)| building_coord_from_id(size, id, material))
        .collect::<Result<Vec<_>>>()?;
    let stats = Ncm4Stats {
        fixed_header_bytes: NCM4_FIXED_HEADER_BYTES,
        profile_header_bytes: (header_end - NCM4_FIXED_HEADER_BYTES as usize) as u32,
        body_bytes: (body_end - header_end) as u32,
        residual_bytes: (offset - body_end) as u32,
        total_bytes: raw.len() as u32,
        commands: command_count,
        patches: patch_count,
        writes: u32::try_from(writes)
            .map_err(|_| Error::overflow("NCM4 write count exceeds u32."))?,
        decode_units: decode_units
            .checked_add(u64::from(command_count))
            .and_then(|value| value.checked_add(u64::from(patch_count)))
            .ok_or_else(|| Error::overflow("NCM4 decode unit overflow."))?,
    };
    if stats.decode_units > limits.max_decode_units {
        return Err(Error::limit(
            "ncm4-decode-unit-limit",
            "NCM4 decode units exceed the configured limit.",
        ));
    }
    Ok((
        Semantics::Building(BuildingSemantics {
            size,
            voxels: output,
        }),
        stats,
    ))
}

fn decode_op(
    bits: &mut BitReader<'_>,
    opcode: u8,
    palette: &[u16],
    widths: FieldWidths,
) -> Result<Ncm4BuildingOp> {
    Ok(match opcode {
        OP_BOX => Ncm4BuildingOp::Box {
            material: read_material_bits(bits, palette, widths)?,
            origin: read_origin_bits(bits, widths)?,
            size: read_size_bits(bits, widths)?,
        },
        OP_REPEAT_BOX => Ncm4BuildingOp::RepeatBox {
            material: read_material_bits(bits, palette, widths)?,
            origin: read_origin_bits(bits, widths)?,
            size: read_size_bits(bits, widths)?,
            count: bits.read(9, "NCM4 repeat count is truncated.")? as u16 + 1,
            delta: read_delta_bits(bits)?,
        },
        OP_GABLE => Ncm4BuildingOp::Gable {
            material: read_material_bits(bits, palette, widths)?,
            style: GableStyle::from_bits(bits.read(2, "NCM4 gable style is truncated.")?)?,
            z_oriented: bits.read(1, "NCM4 gable orientation is truncated.")? != 0,
            origin: read_origin_bits(bits, widths)?,
            width: read_length_bits(bits, widths.coordinate[0])?,
            depth: read_length_bits(bits, widths.coordinate[2])?,
        },
        OP_TREE => Ncm4BuildingOp::Tree {
            trunk_material: read_material_bits(bits, palette, widths)?,
            leaf_material: read_material_bits(bits, palette, widths)?,
            origin: read_origin_bits(bits, widths)?,
            height: bits.read(6, "NCM4 tree height is truncated.")? as u16 + 2,
            crown: bits.read(4, "NCM4 tree crown is truncated.")? as u16 + 1,
        },
        OP_FENCE => {
            let material = read_material_bits(bits, palette, widths)?;
            let origin = read_origin_bits(bits, widths)?;
            let axis = bits.read(1, "NCM4 fence axis is truncated.")? as u8;
            let dimension_axis = if axis == 0 { 0 } else { 2 };
            Ncm4BuildingOp::Fence {
                material,
                origin,
                axis,
                length: read_length_bits(bits, widths.coordinate[dimension_axis])?,
                spacing: bits.read(6, "NCM4 fence spacing is truncated.")? as u16 + 1,
            }
        }
        OP_RUN => {
            let material = read_material_bits(bits, palette, widths)?;
            let origin = read_origin_bits(bits, widths)?;
            let axis = bits.read(2, "NCM4 run axis is truncated.")? as u8;
            let axis_index = checked_axis(axis)?;
            Ncm4BuildingOp::Run {
                material,
                origin,
                axis,
                length: read_length_bits(bits, widths.coordinate[axis_index])?,
            }
        }
        OP_WALL => {
            let material = read_material_bits(bits, palette, widths)?;
            let origin = read_origin_bits(bits, widths)?;
            let normal_axis = bits.read(2, "NCM4 wall axis is truncated.")? as u8;
            let normal = checked_axis(normal_axis)?;
            let tangent = tangent_axes(normal);
            Ncm4BuildingOp::Wall {
                material,
                origin,
                normal_axis,
                u_length: read_length_bits(bits, widths.coordinate[tangent[0]])?,
                v_length: read_length_bits(bits, widths.coordinate[tangent[1]])?,
                thickness: read_length_bits(bits, widths.coordinate[normal])?,
            }
        }
        OP_EXTRUDE => {
            let material = read_material_bits(bits, palette, widths)?;
            let origin = read_origin_bits(bits, widths)?;
            let axis = bits.read(2, "NCM4 extrude axis is truncated.")? as u8;
            let axis_index = checked_axis(axis)?;
            let tangent = tangent_axes(axis_index);
            let u_length = read_length_bits(bits, widths.coordinate[tangent[0]])?;
            let v_length = read_length_bits(bits, widths.coordinate[tangent[1]])?;
            let depth = read_length_bits(bits, widths.coordinate[axis_index])?;
            let count = usize::from(u_length)
                .checked_mul(usize::from(v_length))
                .ok_or_else(|| Error::overflow("NCM4 extrude mask overflow."))?;
            let mut mask = Vec::with_capacity(count);
            for _ in 0..count {
                mask.push(bits.read(1, "NCM4 extrude mask is truncated.")? != 0);
            }
            if !mask.iter().any(|value| *value) {
                return Err(noncanonical(
                    "ncm4-extrude-mask",
                    "NCM4 extrude mask cannot be empty.",
                ));
            }
            Ncm4BuildingOp::Extrude {
                material,
                origin,
                axis,
                u_length,
                v_length,
                depth,
                mask,
            }
        }
        OP_TRANSLATE => Ncm4BuildingOp::Translate {
            source_origin: read_origin_bits(bits, widths)?,
            source_size: read_size_bits(bits, widths)?,
            delta: read_delta_bits(bits)?,
        },
        OP_ROTATE_Y => Ncm4BuildingOp::RotateY {
            source_origin: read_origin_bits(bits, widths)?,
            source_size: read_size_bits(bits, widths)?,
            destination_origin: read_origin_bits(bits, widths)?,
            quarter_turns: bits.read(2, "NCM4 rotation is truncated.")? as u8,
        },
        OP_MIRROR => Ncm4BuildingOp::Mirror {
            source_origin: read_origin_bits(bits, widths)?,
            source_size: read_size_bits(bits, widths)?,
            destination_origin: read_origin_bits(bits, widths)?,
            axis: bits.read(2, "NCM4 mirror axis is truncated.")? as u8,
        },
        OP_REPEAT_REGION => Ncm4BuildingOp::RepeatRegion {
            source_origin: read_origin_bits(bits, widths)?,
            source_size: read_size_bits(bits, widths)?,
            count: bits.read(9, "NCM4 region repeat count is truncated.")? as u16 + 1,
            delta: read_delta_bits(bits)?,
        },
        OP_CLEAR_BOX => Ncm4BuildingOp::ClearBox {
            origin: read_origin_bits(bits, widths)?,
            size: read_size_bits(bits, widths)?,
        },
        _ => {
            return Err(Error::new(
                ErrorKind::UnknownOpcode,
                "ncm4-building-opcode",
                "Unknown NCM4 building opcode.",
            ))
        }
    })
}

fn execute_op(
    op: &Ncm4BuildingOp,
    dimensions: [u16; 3],
    voxels: &mut BTreeMap<u32, u16>,
    writes: &mut u64,
    decode_units: &mut u64,
    limits: &LimitsV1,
) -> Result<()> {
    match op {
        Ncm4BuildingOp::Box {
            material,
            origin,
            size,
        } => write_box(
            dimensions,
            voxels,
            *material,
            *origin,
            *size,
            writes,
            decode_units,
            limits,
        ),
        Ncm4BuildingOp::RepeatBox {
            material,
            origin,
            size,
            count,
            delta,
        } => {
            if !(2..=MAX_REPEAT as u16).contains(count) {
                return Err(out_of_bounds(
                    "ncm4-repeat-count",
                    "NCM4 repeat count must be in 2..=512.",
                ));
            }
            for index in 0..*count {
                let repeated = translated(*origin, *delta, i32::from(index))?;
                write_box(
                    dimensions,
                    voxels,
                    *material,
                    repeated,
                    *size,
                    writes,
                    decode_units,
                    limits,
                )?;
            }
            Ok(())
        }
        Ncm4BuildingOp::Gable {
            material,
            origin,
            width,
            depth,
            style,
            z_oriented,
        } => write_gable(
            dimensions,
            voxels,
            *material,
            *origin,
            *width,
            *depth,
            *style,
            *z_oriented,
            writes,
            decode_units,
            limits,
        ),
        Ncm4BuildingOp::Tree {
            trunk_material,
            leaf_material,
            origin,
            height,
            crown,
        } => write_tree(
            dimensions,
            voxels,
            *trunk_material,
            *leaf_material,
            *origin,
            *height,
            *crown,
            writes,
            decode_units,
            limits,
        ),
        Ncm4BuildingOp::Fence {
            material,
            origin,
            length,
            axis,
            spacing,
        } => write_fence(
            dimensions,
            voxels,
            *material,
            *origin,
            *length,
            *axis,
            *spacing,
            writes,
            decode_units,
            limits,
        ),
        Ncm4BuildingOp::Run {
            material,
            origin,
            axis,
            length,
        } => {
            let axis = checked_axis(*axis)?;
            let mut size = [1_u16; 3];
            size[axis] = *length;
            write_box(
                dimensions,
                voxels,
                *material,
                *origin,
                size,
                writes,
                decode_units,
                limits,
            )
        }
        Ncm4BuildingOp::Wall {
            material,
            origin,
            normal_axis,
            u_length,
            v_length,
            thickness,
        } => {
            let normal = checked_axis(*normal_axis)?;
            let tangent = tangent_axes(normal);
            let mut size = [1_u16; 3];
            size[normal] = *thickness;
            size[tangent[0]] = *u_length;
            size[tangent[1]] = *v_length;
            write_box(
                dimensions,
                voxels,
                *material,
                *origin,
                size,
                writes,
                decode_units,
                limits,
            )
        }
        Ncm4BuildingOp::Extrude {
            material,
            origin,
            axis,
            u_length,
            v_length,
            depth,
            mask,
        } => {
            let axis = checked_axis(*axis)?;
            let tangent = tangent_axes(axis);
            let expected = usize::from(*u_length)
                .checked_mul(usize::from(*v_length))
                .ok_or_else(|| Error::overflow("NCM4 extrude mask overflow."))?;
            if expected == 0 || mask.len() != expected || !mask.iter().any(|value| *value) {
                return Err(noncanonical(
                    "ncm4-extrude-mask",
                    "NCM4 extrude mask is empty or has the wrong length.",
                ));
            }
            let mut maximum = *origin;
            maximum[axis] = maximum[axis]
                .checked_add(*depth)
                .ok_or_else(|| Error::overflow("NCM4 extrude bound overflow."))?;
            maximum[tangent[0]] = maximum[tangent[0]]
                .checked_add(*u_length)
                .ok_or_else(|| Error::overflow("NCM4 extrude bound overflow."))?;
            maximum[tangent[1]] = maximum[tangent[1]]
                .checked_add(*v_length)
                .ok_or_else(|| Error::overflow("NCM4 extrude bound overflow."))?;
            if (0..3).any(|index| maximum[index] > dimensions[index]) {
                return Err(out_of_bounds(
                    "ncm4-extrude-bounds",
                    "NCM4 extrude extends outside the building.",
                ));
            }
            let occupied = mask.iter().filter(|value| **value).count() as u64;
            charge(
                writes,
                decode_units,
                occupied
                    .checked_mul(u64::from(*depth))
                    .ok_or_else(|| Error::overflow("NCM4 extrude write overflow."))?,
                limits,
            )?;
            for v in 0..*v_length {
                for u in 0..*u_length {
                    if !mask[usize::from(u) + usize::from(*u_length) * usize::from(v)] {
                        continue;
                    }
                    for d in 0..*depth {
                        let mut coordinate = *origin;
                        coordinate[axis] += d;
                        coordinate[tangent[0]] += u;
                        coordinate[tangent[1]] += v;
                        voxels.insert(coord_id(dimensions, coordinate)?, *material);
                    }
                }
            }
            Ok(())
        }
        Ncm4BuildingOp::Translate {
            source_origin,
            source_size,
            delta,
        } => {
            if *delta == [0, 0, 0] {
                return Err(noncanonical(
                    "ncm4-translate-noop",
                    "NCM4 translation cannot use a zero delta.",
                ));
            }
            let snapshot = snapshot_region(dimensions, voxels, *source_origin, *source_size)?;
            charge(writes, decode_units, snapshot.len() as u64, limits)?;
            for (coordinate, material) in snapshot {
                let destination = translated(coordinate, *delta, 1)?;
                voxels.insert(coord_id(dimensions, destination)?, material);
            }
            Ok(())
        }
        Ncm4BuildingOp::RotateY {
            source_origin,
            source_size,
            destination_origin,
            quarter_turns,
        } => {
            if !(1..=3).contains(quarter_turns) {
                return Err(noncanonical(
                    "ncm4-rotation-noop",
                    "NCM4 rotation must use one to three quarter turns.",
                ));
            }
            let snapshot = snapshot_region(dimensions, voxels, *source_origin, *source_size)?;
            charge(writes, decode_units, snapshot.len() as u64, limits)?;
            for (coordinate, material) in snapshot {
                let relative = [
                    coordinate[0] - source_origin[0],
                    coordinate[1] - source_origin[1],
                    coordinate[2] - source_origin[2],
                ];
                let (x, z) = match quarter_turns {
                    1 => (source_size[2] - 1 - relative[2], relative[0]),
                    2 => (
                        source_size[0] - 1 - relative[0],
                        source_size[2] - 1 - relative[2],
                    ),
                    3 => (relative[2], source_size[0] - 1 - relative[0]),
                    _ => unreachable!(),
                };
                let destination = [
                    destination_origin[0]
                        .checked_add(x)
                        .ok_or_else(|| Error::overflow("NCM4 rotation overflow."))?,
                    destination_origin[1]
                        .checked_add(relative[1])
                        .ok_or_else(|| Error::overflow("NCM4 rotation overflow."))?,
                    destination_origin[2]
                        .checked_add(z)
                        .ok_or_else(|| Error::overflow("NCM4 rotation overflow."))?,
                ];
                voxels.insert(coord_id(dimensions, destination)?, material);
            }
            Ok(())
        }
        Ncm4BuildingOp::Mirror {
            source_origin,
            source_size,
            destination_origin,
            axis,
        } => {
            let axis = checked_axis(*axis)?;
            let snapshot = snapshot_region(dimensions, voxels, *source_origin, *source_size)?;
            charge(writes, decode_units, snapshot.len() as u64, limits)?;
            for (coordinate, material) in snapshot {
                let mut relative = [
                    coordinate[0] - source_origin[0],
                    coordinate[1] - source_origin[1],
                    coordinate[2] - source_origin[2],
                ];
                relative[axis] = source_size[axis] - 1 - relative[axis];
                let destination = [
                    destination_origin[0]
                        .checked_add(relative[0])
                        .ok_or_else(|| Error::overflow("NCM4 mirror overflow."))?,
                    destination_origin[1]
                        .checked_add(relative[1])
                        .ok_or_else(|| Error::overflow("NCM4 mirror overflow."))?,
                    destination_origin[2]
                        .checked_add(relative[2])
                        .ok_or_else(|| Error::overflow("NCM4 mirror overflow."))?,
                ];
                voxels.insert(coord_id(dimensions, destination)?, material);
            }
            Ok(())
        }
        Ncm4BuildingOp::RepeatRegion {
            source_origin,
            source_size,
            count,
            delta,
        } => {
            if !(2..=MAX_REPEAT as u16).contains(count) || *delta == [0, 0, 0] {
                return Err(out_of_bounds(
                    "ncm4-repeat-region",
                    "NCM4 region repeat requires count 2..=512 and non-zero delta.",
                ));
            }
            let snapshot = snapshot_region(dimensions, voxels, *source_origin, *source_size)?;
            let copy_count = u64::from(*count - 1);
            charge(
                writes,
                decode_units,
                (snapshot.len() as u64)
                    .checked_mul(copy_count)
                    .ok_or_else(|| Error::overflow("NCM4 region repeat overflow."))?,
                limits,
            )?;
            for index in 1..*count {
                for (coordinate, material) in &snapshot {
                    let destination = translated(*coordinate, *delta, i32::from(index))?;
                    voxels.insert(coord_id(dimensions, destination)?, *material);
                }
            }
            Ok(())
        }
        Ncm4BuildingOp::ClearBox { origin, size } => {
            check_box(dimensions, *origin, *size)?;
            let mut removed = 0_u64;
            for y in origin[1]..origin[1] + size[1] {
                for z in origin[2]..origin[2] + size[2] {
                    for x in origin[0]..origin[0] + size[0] {
                        if voxels.remove(&coord_id(dimensions, [x, y, z])?).is_some() {
                            removed += 1;
                        }
                    }
                }
            }
            if removed == 0 {
                return Err(noncanonical(
                    "ncm4-clear-noop",
                    "NCM4 CLEAR_BOX must remove at least one voxel.",
                ));
            }
            charge(writes, decode_units, removed, limits)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn write_box(
    dimensions: [u16; 3],
    voxels: &mut BTreeMap<u32, u16>,
    material: u16,
    origin: [u16; 3],
    size: [u16; 3],
    writes: &mut u64,
    decode_units: &mut u64,
    limits: &LimitsV1,
) -> Result<()> {
    if material == 0 {
        return Err(out_of_bounds(
            "ncm4-material",
            "NCM4 program material zero is reserved for air.",
        ));
    }
    check_box(dimensions, origin, size)?;
    let volume = u64::from(size[0])
        .checked_mul(u64::from(size[1]))
        .and_then(|value| value.checked_mul(u64::from(size[2])))
        .ok_or_else(|| Error::overflow("NCM4 box volume overflow."))?;
    charge(writes, decode_units, volume, limits)?;
    for y in origin[1]..origin[1] + size[1] {
        for z in origin[2]..origin[2] + size[2] {
            for x in origin[0]..origin[0] + size[0] {
                voxels.insert(coord_id(dimensions, [x, y, z])?, material);
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn write_gable(
    dimensions: [u16; 3],
    voxels: &mut BTreeMap<u32, u16>,
    material: u16,
    origin: [u16; 3],
    width: u16,
    depth: u16,
    style: GableStyle,
    z_oriented: bool,
    writes: &mut u64,
    decode_units: &mut u64,
    limits: &LimitsV1,
) -> Result<()> {
    let layers = if z_oriented {
        depth.div_ceil(2)
    } else {
        width.div_ceil(2)
    };
    check_box(dimensions, origin, [width, layers, depth])?;
    for layer in 0..layers {
        if z_oriented {
            let front = origin[2] + layer;
            let back = origin[2] + depth - 1 - layer;
            match style {
                GableStyle::Outline => {
                    write_box(
                        dimensions,
                        voxels,
                        material,
                        [origin[0], origin[1] + layer, front],
                        [width, 1, 1],
                        writes,
                        decode_units,
                        limits,
                    )?;
                    if back != front {
                        write_box(
                            dimensions,
                            voxels,
                            material,
                            [origin[0], origin[1] + layer, back],
                            [width, 1, 1],
                            writes,
                            decode_units,
                            limits,
                        )?;
                    }
                }
                GableStyle::Trim => {
                    for x in [origin[0], origin[0] + width - 1] {
                        write_box(
                            dimensions,
                            voxels,
                            material,
                            [x, origin[1] + layer, front],
                            [1, 1, 1],
                            writes,
                            decode_units,
                            limits,
                        )?;
                        if back != front {
                            write_box(
                                dimensions,
                                voxels,
                                material,
                                [x, origin[1] + layer, back],
                                [1, 1, 1],
                                writes,
                                decode_units,
                                limits,
                            )?;
                        }
                    }
                }
                GableStyle::Fill => write_box(
                    dimensions,
                    voxels,
                    material,
                    [origin[0], origin[1] + layer, origin[2] + layer],
                    [width, 1, depth - layer * 2],
                    writes,
                    decode_units,
                    limits,
                )?,
            }
        } else {
            let left = origin[0] + layer;
            let right = origin[0] + width - 1 - layer;
            match style {
                GableStyle::Outline => {
                    write_box(
                        dimensions,
                        voxels,
                        material,
                        [left, origin[1] + layer, origin[2]],
                        [1, 1, depth],
                        writes,
                        decode_units,
                        limits,
                    )?;
                    if right != left {
                        write_box(
                            dimensions,
                            voxels,
                            material,
                            [right, origin[1] + layer, origin[2]],
                            [1, 1, depth],
                            writes,
                            decode_units,
                            limits,
                        )?;
                    }
                }
                GableStyle::Trim => {
                    for z in [origin[2], origin[2] + depth - 1] {
                        write_box(
                            dimensions,
                            voxels,
                            material,
                            [left, origin[1] + layer, z],
                            [1, 1, 1],
                            writes,
                            decode_units,
                            limits,
                        )?;
                        if right != left {
                            write_box(
                                dimensions,
                                voxels,
                                material,
                                [right, origin[1] + layer, z],
                                [1, 1, 1],
                                writes,
                                decode_units,
                                limits,
                            )?;
                        }
                    }
                }
                GableStyle::Fill => write_box(
                    dimensions,
                    voxels,
                    material,
                    [left, origin[1] + layer, origin[2]],
                    [width - layer * 2, 1, depth],
                    writes,
                    decode_units,
                    limits,
                )?,
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn write_tree(
    dimensions: [u16; 3],
    voxels: &mut BTreeMap<u32, u16>,
    trunk_material: u16,
    leaf_material: u16,
    origin: [u16; 3],
    height: u16,
    crown: u16,
    writes: &mut u64,
    decode_units: &mut u64,
    limits: &LimitsV1,
) -> Result<()> {
    if !(2..=64).contains(&height) || !(1..=16).contains(&crown) {
        return Err(out_of_bounds(
            "ncm4-tree",
            "NCM4 tree parameters exceed their bounded envelope.",
        ));
    }
    let [x, y, z] = origin;
    if x < crown || z < crown {
        return Err(out_of_bounds(
            "ncm4-tree-bounds",
            "NCM4 tree crown extends outside the building.",
        ));
    }
    let crown_diameter = crown
        .checked_mul(2)
        .and_then(|value| value.checked_add(2))
        .ok_or_else(|| Error::overflow("NCM4 tree crown overflow."))?;
    check_box(
        dimensions,
        [x - crown, y, z - crown],
        [crown_diameter, height.max(crown + 1), crown_diameter],
    )?;
    let trunk_height = height.saturating_sub(crown).max(2);
    write_box(
        dimensions,
        voxels,
        trunk_material,
        [x, y, z],
        [2, trunk_height, 2],
        writes,
        decode_units,
        limits,
    )?;
    for layer in 0..crown {
        let radius = (crown - layer / 2).max(1);
        write_box(
            dimensions,
            voxels,
            leaf_material,
            [x - radius, y + trunk_height - 1 + layer, z - 1],
            [radius * 2 + 2, 1, 4],
            writes,
            decode_units,
            limits,
        )?;
        write_box(
            dimensions,
            voxels,
            leaf_material,
            [x - 1, y + trunk_height - 1 + layer, z - radius],
            [4, 1, radius * 2 + 2],
            writes,
            decode_units,
            limits,
        )?;
    }
    write_box(
        dimensions,
        voxels,
        leaf_material,
        [x, y + height - 1, z],
        [2, 1, 2],
        writes,
        decode_units,
        limits,
    )
}

#[allow(clippy::too_many_arguments)]
fn write_fence(
    dimensions: [u16; 3],
    voxels: &mut BTreeMap<u32, u16>,
    material: u16,
    origin: [u16; 3],
    length: u16,
    axis: u8,
    spacing: u16,
    writes: &mut u64,
    decode_units: &mut u64,
    limits: &LimitsV1,
) -> Result<()> {
    if axis > 1 || length == 0 || !(1..=64).contains(&spacing) {
        return Err(out_of_bounds(
            "ncm4-fence",
            "NCM4 fence parameters exceed their bounded envelope.",
        ));
    }
    let rail_size = if axis == 0 {
        [length, 1, 1]
    } else {
        [1, 1, length]
    };
    let bounds = if axis == 0 {
        [length, 5, 1]
    } else {
        [1, 5, length]
    };
    check_box(dimensions, origin, bounds)?;
    for rail_y in [origin[1] + 1, origin[1] + 3] {
        write_box(
            dimensions,
            voxels,
            material,
            [origin[0], rail_y, origin[2]],
            rail_size,
            writes,
            decode_units,
            limits,
        )?;
    }
    let mut position = 0_u16;
    while position < length {
        let post = [
            origin[0] + if axis == 0 { position } else { 0 },
            origin[1],
            origin[2] + if axis == 1 { position } else { 0 },
        ];
        write_box(
            dimensions,
            voxels,
            material,
            post,
            [1, 5, 1],
            writes,
            decode_units,
            limits,
        )?;
        position = position
            .checked_add(spacing)
            .ok_or_else(|| Error::overflow("NCM4 fence spacing overflow."))?;
    }
    let end = length - 1;
    let end_post = [
        origin[0] + if axis == 0 { end } else { 0 },
        origin[1],
        origin[2] + if axis == 1 { end } else { 0 },
    ];
    write_box(
        dimensions,
        voxels,
        material,
        end_post,
        [1, 5, 1],
        writes,
        decode_units,
        limits,
    )
}

fn snapshot_region(
    dimensions: [u16; 3],
    voxels: &BTreeMap<u32, u16>,
    origin: [u16; 3],
    size: [u16; 3],
) -> Result<Vec<([u16; 3], u16)>> {
    check_box(dimensions, origin, size)?;
    let mut snapshot = Vec::new();
    for (id, material) in voxels {
        let voxel = building_coord_from_id(dimensions, *id, *material)?;
        let coordinate = [voxel.x, voxel.y, voxel.z];
        if (0..3).all(|axis| {
            coordinate[axis] >= origin[axis]
                && coordinate[axis] < origin[axis].saturating_add(size[axis])
        }) {
            snapshot.push((coordinate, *material));
        }
    }
    if snapshot.is_empty() {
        return Err(noncanonical(
            "ncm4-empty-source",
            "NCM4 transform source region cannot be empty.",
        ));
    }
    Ok(snapshot)
}

fn charge(writes: &mut u64, decode_units: &mut u64, amount: u64, limits: &LimitsV1) -> Result<()> {
    if amount == 0 || amount > u64::from(limits.max_expanded_per_op) {
        return Err(Error::limit(
            "ncm4-op-expansion-limit",
            "NCM4 opcode expansion is empty or exceeds the per-op limit.",
        ));
    }
    *writes = writes
        .checked_add(amount)
        .ok_or_else(|| Error::overflow("NCM4 write count overflow."))?;
    *decode_units = decode_units
        .checked_add(amount)
        .ok_or_else(|| Error::overflow("NCM4 decode unit overflow."))?;
    if *writes > u64::from(limits.max_writes) || *decode_units > limits.max_decode_units {
        return Err(Error::limit(
            "ncm4-resource-limit",
            "NCM4 expansion exceeds the configured write or decode-unit limit.",
        ));
    }
    Ok(())
}

fn translated(origin: [u16; 3], delta: [i16; 3], multiplier: i32) -> Result<[u16; 3]> {
    let mut output = [0_u16; 3];
    for axis in 0..3 {
        let value = i64::from(origin[axis])
            .checked_add(
                i64::from(delta[axis])
                    .checked_mul(i64::from(multiplier))
                    .ok_or_else(|| Error::overflow("NCM4 translation overflow."))?,
            )
            .ok_or_else(|| Error::overflow("NCM4 translation overflow."))?;
        output[axis] = u16::try_from(value).map_err(|_| {
            out_of_bounds(
                "ncm4-translation-bounds",
                "NCM4 translation produces an invalid coordinate.",
            )
        })?;
    }
    Ok(output)
}

fn check_box(dimensions: [u16; 3], origin: [u16; 3], size: [u16; 3]) -> Result<()> {
    if size.contains(&0)
        || (0..3).any(|axis| {
            origin[axis]
                .checked_add(size[axis])
                .is_none_or(|end| end > dimensions[axis])
        })
    {
        return Err(out_of_bounds(
            "ncm4-box-bounds",
            "NCM4 box extends outside the declared dimensions.",
        ));
    }
    Ok(())
}

fn coord_id(dimensions: [u16; 3], coordinate: [u16; 3]) -> Result<u32> {
    if (0..3).any(|axis| coordinate[axis] >= dimensions[axis]) {
        return Err(out_of_bounds(
            "ncm4-coordinate-bounds",
            "NCM4 coordinate is outside the declared dimensions.",
        ));
    }
    Ok(u32::from(coordinate[0])
        + u32::from(dimensions[0])
            * (u32::from(coordinate[2]) + u32::from(dimensions[2]) * u32::from(coordinate[1])))
}

fn residual_candidates(size: [u16; 3], patches: &[Ncm4Patch]) -> Vec<Ncm4Residual> {
    vec![
        Ncm4Residual::Sparse(patches.to_vec()),
        Ncm4Residual::Runs(patches_to_runs(patches)),
        Ncm4Residual::Boxes(patches_to_boxes(size, patches)),
        Ncm4Residual::Layers(patches_to_layers(size, patches)),
        Ncm4Residual::Xor(patches.to_vec()),
        Ncm4Residual::MaterialGroups(patches_to_groups(patches)),
    ]
}

fn residual_tag(residual: &Ncm4Residual) -> u8 {
    match residual {
        Ncm4Residual::None => RESIDUAL_NONE,
        Ncm4Residual::Sparse(_) => RESIDUAL_SPARSE,
        Ncm4Residual::Runs(_) => RESIDUAL_RUNS,
        Ncm4Residual::Boxes(_) => RESIDUAL_BOXES,
        Ncm4Residual::Layers(_) => RESIDUAL_LAYERS,
        Ncm4Residual::Xor(_) => RESIDUAL_XOR,
        Ncm4Residual::MaterialGroups(_) => RESIDUAL_MATERIAL_GROUPS,
    }
}

fn encode_residual(
    output: &mut Vec<u8>,
    program: &Ncm4BuildingProgram,
    limits: &LimitsV1,
) -> Result<()> {
    output.push(residual_tag(&program.residual));
    match &program.residual {
        Ncm4Residual::None => {}
        Ncm4Residual::Sparse(patches) => {
            ensure_patch_count(patches.len(), limits)?;
            write_u32(output, patches.len() as u32);
            encode_sorted_patches(output, patches, &program.palette)?;
        }
        Ncm4Residual::Runs(runs) => {
            ensure_patch_count(runs.len(), limits)?;
            write_u32(output, runs.len() as u32);
            let mut previous_end = 0_u32;
            for (index, run) in runs.iter().enumerate() {
                if run.length == 0 || (index > 0 && run.start < previous_end) {
                    return Err(noncanonical(
                        "ncm4-residual-run-order",
                        "NCM4 residual runs must be non-empty, sorted, and disjoint.",
                    ));
                }
                let delta = if index == 0 {
                    run.start
                } else {
                    run.start - previous_end
                };
                write_u32(output, delta);
                write_u32(output, run.length - 1);
                encode_patch_action(output, run.kind, run.material, &program.palette)?;
                previous_end = run
                    .start
                    .checked_add(run.length)
                    .ok_or_else(|| Error::overflow("NCM4 residual run overflow."))?;
            }
        }
        Ncm4Residual::Boxes(boxes) => {
            ensure_patch_count(boxes.len(), limits)?;
            write_u32(output, boxes.len() as u32);
            let mut previous = None;
            for item in boxes {
                check_box(program.size, item.origin, item.size)?;
                let id = coord_id(program.size, item.origin)?;
                if previous.is_some_and(|value| value >= id) {
                    return Err(noncanonical(
                        "ncm4-residual-box-order",
                        "NCM4 residual boxes must be strictly coordinate sorted.",
                    ));
                }
                for value in item.origin {
                    write_u32(output, u32::from(value));
                }
                for value in item.size {
                    write_u32(output, u32::from(value - 1));
                }
                encode_patch_action(output, item.kind, item.material, &program.palette)?;
                previous = Some(id);
            }
        }
        Ncm4Residual::Layers(layers) => {
            ensure_patch_count(layers.len(), limits)?;
            write_u32(output, layers.len() as u32);
            let bytes = layer_bitmap_bytes(program.size)?;
            let mut previous = None;
            for layer in layers {
                let key = (layer.y, layer.kind.byte(), layer.material);
                if layer.y >= program.size[1]
                    || layer.bitmap.len() != bytes
                    || layer.bitmap.iter().all(|value| *value == 0)
                    || previous.is_some_and(|value| value >= key)
                {
                    return Err(noncanonical(
                        "ncm4-residual-layer-order",
                        "NCM4 residual layers must be non-empty and canonically sorted.",
                    ));
                }
                check_bitmap_padding(
                    &layer.bitmap,
                    usize::from(program.size[0]) * usize::from(program.size[2]),
                )?;
                write_u32(output, u32::from(layer.y));
                encode_patch_action(output, layer.kind, layer.material, &program.palette)?;
                output.extend_from_slice(&layer.bitmap);
                previous = Some(key);
            }
        }
        Ncm4Residual::Xor(patches) => {
            ensure_patch_count(patches.len(), limits)?;
            validate_sorted_patches(patches)?;
            let volume = building_volume(program.size)? as usize;
            let mut bitmap = vec![0_u8; volume.div_ceil(8)];
            for patch in patches {
                if patch.id as usize >= volume {
                    return Err(out_of_bounds(
                        "ncm4-residual-coordinate",
                        "NCM4 XOR residual coordinate exceeds the building volume.",
                    ));
                }
                bitmap[patch.id as usize / 8] |= 1 << (7 - patch.id as usize % 8);
            }
            output.extend_from_slice(&bitmap);
            for patch in patches {
                encode_patch_action(output, patch.kind, patch.material, &program.palette)?;
            }
        }
        Ncm4Residual::MaterialGroups(groups) => {
            ensure_patch_count(groups.len(), limits)?;
            write_u32(output, groups.len() as u32);
            let mut previous_key = None;
            for group in groups {
                let key = (group.kind.byte(), group.material);
                if group.ids.is_empty() || previous_key.is_some_and(|value| value >= key) {
                    return Err(noncanonical(
                        "ncm4-residual-group-order",
                        "NCM4 material groups must be non-empty and canonically sorted.",
                    ));
                }
                encode_patch_action(output, group.kind, group.material, &program.palette)?;
                write_u32(output, group.ids.len() as u32);
                let mut previous = 0_u32;
                for (index, id) in group.ids.iter().enumerate() {
                    let delta = if index == 0 {
                        *id
                    } else {
                        id.checked_sub(previous)
                            .and_then(|value| value.checked_sub(1))
                            .ok_or_else(|| {
                                noncanonical(
                                    "ncm4-residual-group-coordinate",
                                    "NCM4 group coordinates must be strictly sorted.",
                                )
                            })?
                    };
                    write_u32(output, delta);
                    previous = *id;
                }
                previous_key = Some(key);
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn decode_residual(
    input: &[u8],
    offset: &mut usize,
    size: [u16; 3],
    volume: u32,
    palette: &[u16],
    voxels: &mut BTreeMap<u32, u16>,
    writes: &mut u64,
    decode_units: &mut u64,
    limits: &LimitsV1,
) -> Result<u32> {
    let tag = read_byte(input, offset, "NCM4 residual codec")?;
    let mut patches = Vec::<Ncm4Patch>::new();
    match tag {
        RESIDUAL_NONE => {}
        RESIDUAL_SPARSE => {
            let count = read_patch_count(input, offset, limits)?;
            let mut previous = 0_u32;
            for index in 0..count {
                let delta = read_u32(input, offset)?;
                let id = delta_id(index, previous, delta)?;
                let (kind, material) = decode_patch_action(input, offset, palette)?;
                push_bounded_patch(&mut patches, Ncm4Patch { id, kind, material }, limits)?;
                previous = id;
            }
        }
        RESIDUAL_RUNS => {
            let count = read_patch_count(input, offset, limits)?;
            let mut previous_end = 0_u32;
            for index in 0..count {
                let delta = read_u32(input, offset)?;
                let start = if index == 0 {
                    delta
                } else {
                    previous_end
                        .checked_add(delta)
                        .ok_or_else(|| Error::overflow("NCM4 residual run overflow."))?
                };
                let length = read_u32(input, offset)?
                    .checked_add(1)
                    .ok_or_else(|| Error::overflow("NCM4 residual run overflow."))?;
                let end = start
                    .checked_add(length)
                    .ok_or_else(|| Error::overflow("NCM4 residual run overflow."))?;
                if end > volume {
                    return Err(out_of_bounds(
                        "ncm4-residual-run-bounds",
                        "NCM4 residual run exceeds the building volume.",
                    ));
                }
                let (kind, material) = decode_patch_action(input, offset, palette)?;
                ensure_expanded_patch_capacity(&patches, u64::from(length), limits)?;
                for id in start..end {
                    patches.push(Ncm4Patch { id, kind, material });
                }
                previous_end = end;
            }
        }
        RESIDUAL_BOXES => {
            let count = read_patch_count(input, offset, limits)?;
            let mut previous = None;
            for _ in 0..count {
                let origin = [
                    checked_u16(read_u32(input, offset)?, "NCM4 residual box x exceeds u16.")?,
                    checked_u16(read_u32(input, offset)?, "NCM4 residual box y exceeds u16.")?,
                    checked_u16(read_u32(input, offset)?, "NCM4 residual box z exceeds u16.")?,
                ];
                let box_size = [
                    plus_one_u16(
                        read_u32(input, offset)?,
                        "NCM4 residual box width overflow.",
                    )?,
                    plus_one_u16(
                        read_u32(input, offset)?,
                        "NCM4 residual box height overflow.",
                    )?,
                    plus_one_u16(
                        read_u32(input, offset)?,
                        "NCM4 residual box depth overflow.",
                    )?,
                ];
                check_box(size, origin, box_size)?;
                let start = coord_id(size, origin)?;
                if previous.is_some_and(|value| value >= start) {
                    return Err(noncanonical(
                        "ncm4-residual-box-order",
                        "NCM4 residual boxes must be strictly coordinate sorted.",
                    ));
                }
                let (kind, material) = decode_patch_action(input, offset, palette)?;
                let expanded = u64::from(box_size[0])
                    .checked_mul(u64::from(box_size[1]))
                    .and_then(|value| value.checked_mul(u64::from(box_size[2])))
                    .ok_or_else(|| Error::overflow("NCM4 residual box expansion overflow."))?;
                ensure_expanded_patch_capacity(&patches, expanded, limits)?;
                for y in origin[1]..origin[1] + box_size[1] {
                    for z in origin[2]..origin[2] + box_size[2] {
                        for x in origin[0]..origin[0] + box_size[0] {
                            patches.push(Ncm4Patch {
                                id: coord_id(size, [x, y, z])?,
                                kind,
                                material,
                            });
                        }
                    }
                }
                previous = Some(start);
            }
        }
        RESIDUAL_LAYERS => {
            let count = read_patch_count(input, offset, limits)?;
            let bytes = layer_bitmap_bytes(size)?;
            let bit_count = usize::from(size[0]) * usize::from(size[2]);
            let mut previous = None;
            for _ in 0..count {
                let y = checked_u16(read_u32(input, offset)?, "NCM4 residual layer exceeds u16.")?;
                let (kind, material) = decode_patch_action(input, offset, palette)?;
                let key = (y, kind.byte(), material);
                if y >= size[1] || previous.is_some_and(|value| value >= key) {
                    return Err(noncanonical(
                        "ncm4-residual-layer-order",
                        "NCM4 residual layers must be canonically sorted.",
                    ));
                }
                let end = offset
                    .checked_add(bytes)
                    .ok_or_else(|| Error::overflow("NCM4 layer bitmap overflow."))?;
                let bitmap = input.get(*offset..end).ok_or_else(|| {
                    Error::new(
                        ErrorKind::Truncated,
                        "ncm4-residual-layer-truncated",
                        "NCM4 residual layer bitmap is truncated.",
                    )
                })?;
                *offset = end;
                if bitmap.iter().all(|value| *value == 0) {
                    return Err(noncanonical(
                        "ncm4-residual-layer-empty",
                        "NCM4 residual layer bitmap cannot be empty.",
                    ));
                }
                check_bitmap_padding(bitmap, bit_count)?;
                for bit in 0..bit_count {
                    if bitmap[bit / 8] & (1 << (7 - bit % 8)) != 0 {
                        let x = bit % usize::from(size[0]);
                        let z = bit / usize::from(size[0]);
                        push_bounded_patch(
                            &mut patches,
                            Ncm4Patch {
                                id: coord_id(size, [x as u16, y, z as u16])?,
                                kind,
                                material,
                            },
                            limits,
                        )?;
                    }
                }
                previous = Some(key);
            }
        }
        RESIDUAL_XOR => {
            let bytes = (volume as usize).div_ceil(8);
            let end = offset
                .checked_add(bytes)
                .ok_or_else(|| Error::overflow("NCM4 XOR bitmap overflow."))?;
            let bitmap = input.get(*offset..end).ok_or_else(|| {
                Error::new(
                    ErrorKind::Truncated,
                    "ncm4-residual-xor-truncated",
                    "NCM4 XOR residual bitmap is truncated.",
                )
            })?;
            *offset = end;
            check_bitmap_padding(bitmap, volume as usize)?;
            for id in 0..volume {
                if bitmap[id as usize / 8] & (1 << (7 - id as usize % 8)) != 0 {
                    let (kind, material) = decode_patch_action(input, offset, palette)?;
                    push_bounded_patch(&mut patches, Ncm4Patch { id, kind, material }, limits)?;
                }
            }
            if patches.is_empty() {
                return Err(noncanonical(
                    "ncm4-residual-xor-empty",
                    "NCM4 XOR residual bitmap cannot be empty.",
                ));
            }
        }
        RESIDUAL_MATERIAL_GROUPS => {
            let groups = read_patch_count(input, offset, limits)?;
            let mut previous_key = None;
            for _ in 0..groups {
                let (kind, material) = decode_patch_action(input, offset, palette)?;
                let key = (kind.byte(), material);
                if previous_key.is_some_and(|value| value >= key) {
                    return Err(noncanonical(
                        "ncm4-residual-group-order",
                        "NCM4 material groups must be canonically sorted.",
                    ));
                }
                let count = read_patch_count(input, offset, limits)?;
                if count == 0 {
                    return Err(noncanonical(
                        "ncm4-residual-group-empty",
                        "NCM4 material group cannot be empty.",
                    ));
                }
                let mut previous = 0_u32;
                for index in 0..count {
                    let id = delta_id(index, previous, read_u32(input, offset)?)?;
                    push_bounded_patch(&mut patches, Ncm4Patch { id, kind, material }, limits)?;
                    previous = id;
                }
                previous_key = Some(key);
            }
        }
        _ => {
            return Err(Error::new(
                ErrorKind::UnknownOpcode,
                "ncm4-residual-codec",
                "Unknown NCM4 residual codec.",
            ))
        }
    }
    if tag != RESIDUAL_NONE && patches.is_empty() {
        return Err(noncanonical(
            "ncm4-residual-empty",
            "A non-empty NCM4 residual codec must contain at least one patch.",
        ));
    }
    if patches.len() > limits.max_patches as usize {
        return Err(Error::limit(
            "ncm4-patch-limit",
            "NCM4 expanded residual exceeds the patch limit.",
        ));
    }
    patches.sort_unstable();
    for pair in patches.windows(2) {
        if pair[0].id == pair[1].id {
            return Err(noncanonical(
                "ncm4-residual-overlap",
                "NCM4 residual records cannot overlap.",
            ));
        }
    }
    if !patches.is_empty() {
        charge(writes, decode_units, patches.len() as u64, limits)?;
    }
    for patch in &patches {
        if patch.id >= volume {
            return Err(out_of_bounds(
                "ncm4-residual-coordinate",
                "NCM4 residual coordinate exceeds the building volume.",
            ));
        }
        match patch.kind {
            ResidualKind::Set => {
                if patch.material == 0 || voxels.contains_key(&patch.id) {
                    return Err(noncanonical(
                        "ncm4-residual-noop",
                        "NCM4 SET requires an empty voxel and non-zero material.",
                    ));
                }
                if voxels.len() >= limits.max_voxels as usize {
                    return Err(Error::limit(
                        "ncm4-voxel-limit",
                        "NCM4 residual expansion exceeds the voxel limit.",
                    ));
                }
                voxels.insert(patch.id, patch.material);
            }
            ResidualKind::Clear => {
                if patch.material != 0 || voxels.remove(&patch.id).is_none() {
                    return Err(noncanonical(
                        "ncm4-residual-noop",
                        "NCM4 CLEAR requires an occupied voxel and no material.",
                    ));
                }
            }
            ResidualKind::Paint => {
                if patch.material == 0 {
                    return Err(noncanonical(
                        "ncm4-residual-noop",
                        "NCM4 PAINT requires a non-zero material.",
                    ));
                }
                let current = voxels.get_mut(&patch.id).ok_or_else(|| {
                    noncanonical(
                        "ncm4-residual-noop",
                        "NCM4 PAINT requires an occupied voxel.",
                    )
                })?;
                if *current == patch.material {
                    return Err(noncanonical(
                        "ncm4-residual-noop",
                        "NCM4 PAINT must change the voxel material.",
                    ));
                }
                *current = patch.material;
            }
        }
    }
    Ok(patches.len() as u32)
}

fn ensure_expanded_patch_capacity(
    patches: &[Ncm4Patch],
    additional: u64,
    limits: &LimitsV1,
) -> Result<()> {
    let total = (patches.len() as u64)
        .checked_add(additional)
        .ok_or_else(|| Error::overflow("NCM4 expanded residual count overflow."))?;
    let modeled_bytes = total
        .checked_mul(core::mem::size_of::<Ncm4Patch>() as u64)
        .ok_or_else(|| Error::overflow("NCM4 residual memory estimate overflow."))?;
    if additional == 0
        || total > u64::from(limits.max_patches)
        || modeled_bytes > limits.max_memory_bytes
    {
        return Err(Error::limit(
            "ncm4-patch-limit",
            "NCM4 expanded residual exceeds its patch or memory limit.",
        ));
    }
    Ok(())
}

fn push_bounded_patch(
    patches: &mut Vec<Ncm4Patch>,
    patch: Ncm4Patch,
    limits: &LimitsV1,
) -> Result<()> {
    ensure_expanded_patch_capacity(patches, 1, limits)?;
    patches.push(patch);
    Ok(())
}

fn patches_to_runs(patches: &[Ncm4Patch]) -> Vec<Ncm4PatchRun> {
    let mut output = Vec::<Ncm4PatchRun>::new();
    for patch in patches {
        if let Some(last) = output.last_mut() {
            if last.start.saturating_add(last.length) == patch.id
                && last.kind == patch.kind
                && last.material == patch.material
            {
                last.length = last.length.saturating_add(1);
                continue;
            }
        }
        output.push(Ncm4PatchRun {
            start: patch.id,
            length: 1,
            kind: patch.kind,
            material: patch.material,
        });
    }
    output
}

fn patches_to_boxes(size: [u16; 3], patches: &[Ncm4Patch]) -> Vec<Ncm4PatchBox> {
    let mut output = Vec::<Ncm4PatchBox>::new();
    for patch in patches {
        let voxel = building_coord_from_id(size, patch.id, patch.material).unwrap_or(Voxel {
            x: 0,
            y: 0,
            z: 0,
            material: patch.material,
        });
        if let Some(last) = output.last_mut() {
            if last.kind == patch.kind
                && last.material == patch.material
                && last.origin[1] == voxel.y
                && last.origin[2] == voxel.z
                && last.origin[0].saturating_add(last.size[0]) == voxel.x
            {
                last.size[0] = last.size[0].saturating_add(1);
                continue;
            }
        }
        output.push(Ncm4PatchBox {
            origin: [voxel.x, voxel.y, voxel.z],
            size: [1, 1, 1],
            kind: patch.kind,
            material: patch.material,
        });
    }
    output
}

fn patches_to_layers(size: [u16; 3], patches: &[Ncm4Patch]) -> Vec<Ncm4PatchLayer> {
    let bytes = layer_bitmap_bytes(size).unwrap_or(0);
    let mut groups = BTreeMap::<(u16, ResidualKind, u16), Vec<u8>>::new();
    for patch in patches {
        if let Ok(voxel) = building_coord_from_id(size, patch.id, patch.material) {
            let bitmap = groups
                .entry((voxel.y, patch.kind, patch.material))
                .or_insert_with(|| vec![0_u8; bytes]);
            let bit = usize::from(voxel.x) + usize::from(size[0]) * usize::from(voxel.z);
            bitmap[bit / 8] |= 1 << (7 - bit % 8);
        }
    }
    groups
        .into_iter()
        .map(|((y, kind, material), bitmap)| Ncm4PatchLayer {
            y,
            kind,
            material,
            bitmap,
        })
        .collect()
}

fn patches_to_groups(patches: &[Ncm4Patch]) -> Vec<Ncm4PatchGroup> {
    let mut groups = BTreeMap::<(ResidualKind, u16), Vec<u32>>::new();
    for patch in patches {
        groups
            .entry((patch.kind, patch.material))
            .or_default()
            .push(patch.id);
    }
    groups
        .into_iter()
        .map(|((kind, material), ids)| Ncm4PatchGroup {
            kind,
            material,
            ids,
        })
        .collect()
}

fn encode_sorted_patches(
    output: &mut Vec<u8>,
    patches: &[Ncm4Patch],
    palette: &[u16],
) -> Result<()> {
    validate_sorted_patches(patches)?;
    let mut previous = 0_u32;
    for (index, patch) in patches.iter().enumerate() {
        let delta = if index == 0 {
            patch.id
        } else {
            patch
                .id
                .checked_sub(previous)
                .and_then(|value| value.checked_sub(1))
                .ok_or_else(|| {
                    noncanonical(
                        "ncm4-residual-order",
                        "NCM4 sparse residual must be strictly coordinate sorted.",
                    )
                })?
        };
        write_u32(output, delta);
        encode_patch_action(output, patch.kind, patch.material, palette)?;
        previous = patch.id;
    }
    Ok(())
}

fn validate_sorted_patches(patches: &[Ncm4Patch]) -> Result<()> {
    for pair in patches.windows(2) {
        if pair[0].id >= pair[1].id {
            return Err(noncanonical(
                "ncm4-residual-order",
                "NCM4 sparse residual must be strictly coordinate sorted.",
            ));
        }
    }
    Ok(())
}

fn encode_patch_action(
    output: &mut Vec<u8>,
    kind: ResidualKind,
    material: u16,
    palette: &[u16],
) -> Result<()> {
    output.push(kind.byte());
    match kind {
        ResidualKind::Clear if material == 0 => Ok(()),
        ResidualKind::Clear => Err(noncanonical(
            "ncm4-residual-material",
            "NCM4 CLEAR residual cannot carry a material.",
        )),
        ResidualKind::Set | ResidualKind::Paint => {
            let index = palette.binary_search(&material).map_err(|_| {
                noncanonical(
                    "ncm4-residual-material",
                    "NCM4 residual material is absent from the palette.",
                )
            })?;
            write_u32(output, index as u32);
            Ok(())
        }
    }
}

fn decode_patch_action(
    input: &[u8],
    offset: &mut usize,
    palette: &[u16],
) -> Result<(ResidualKind, u16)> {
    let kind = ResidualKind::from_byte(read_byte(input, offset, "NCM4 residual action")?)?;
    let material = match kind {
        ResidualKind::Clear => 0,
        ResidualKind::Set | ResidualKind::Paint => {
            let index = read_u32(input, offset)? as usize;
            *palette.get(index).ok_or_else(|| {
                out_of_bounds(
                    "ncm4-residual-material",
                    "NCM4 residual palette index is out of range.",
                )
            })?
        }
    };
    Ok((kind, material))
}

fn read_patch_count(input: &[u8], offset: &mut usize, limits: &LimitsV1) -> Result<u32> {
    let count = read_u32(input, offset)?;
    if count > limits.max_patches {
        return Err(Error::limit(
            "ncm4-patch-limit",
            "NCM4 residual record count exceeds the patch limit.",
        ));
    }
    Ok(count)
}

fn ensure_patch_count(count: usize, limits: &LimitsV1) -> Result<()> {
    if count > limits.max_patches as usize {
        return Err(Error::limit(
            "ncm4-patch-limit",
            "NCM4 residual record count exceeds the patch limit.",
        ));
    }
    Ok(())
}

fn delta_id(index: u32, previous: u32, delta: u32) -> Result<u32> {
    if index == 0 {
        Ok(delta)
    } else {
        previous
            .checked_add(delta)
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| Error::overflow("NCM4 residual coordinate overflow."))
    }
}

fn layer_bitmap_bytes(size: [u16; 3]) -> Result<usize> {
    usize::from(size[0])
        .checked_mul(usize::from(size[2]))
        .map(|bits| bits.div_ceil(8))
        .ok_or_else(|| Error::overflow("NCM4 layer bitmap size overflow."))
}

fn check_bitmap_padding(bitmap: &[u8], bit_count: usize) -> Result<()> {
    let unused = bitmap.len().saturating_mul(8).saturating_sub(bit_count);
    if unused > 0
        && bitmap
            .last()
            .is_some_and(|last| last & ((1_u8 << unused) - 1) != 0)
    {
        return Err(noncanonical(
            "ncm4-bitmap-padding",
            "NCM4 bitmap padding bits must be zero.",
        ));
    }
    Ok(())
}

fn validate_program_header(program: &Ncm4BuildingProgram, limits: &LimitsV1) -> Result<()> {
    if program.size.iter().any(|value| *value == 0 || *value > 512) {
        return Err(out_of_bounds(
            "ncm4-dimensions",
            "NCM4 dimensions must be in 1..=512.",
        ));
    }
    if program.palette.is_empty() || program.palette.len() > limits.max_materials as usize {
        return Err(Error::limit(
            "ncm4-palette-limit",
            "NCM4 palette is empty or exceeds the material limit.",
        ));
    }
    for pair in program.palette.windows(2) {
        if pair[0] >= pair[1] {
            return Err(noncanonical(
                "ncm4-palette-order",
                "NCM4 palette must be strictly sorted.",
            ));
        }
    }
    if program.palette[0] == 0 {
        return Err(noncanonical(
            "ncm4-palette-air",
            "NCM4 palette cannot contain air.",
        ));
    }
    if program.ops.len() > limits.max_commands as usize {
        return Err(Error::limit(
            "ncm4-command-limit",
            "NCM4 command count exceeds the configured limit.",
        ));
    }
    let _ = building_volume(program.size)?;
    Ok(())
}

fn transcode_ncm3(input: &[u8], limits: &LimitsV1) -> Result<Ncm4BuildingProgram> {
    if input.is_empty()
        || input.len() > MAX_NCM3_BYTES
        || input.len() > limits.max_input_bytes as usize
    {
        return Err(Error::limit(
            "ncm3-payload-limit",
            "NCM3 payload is empty or exceeds the supported byte limit.",
        ));
    }
    let mut offset = 0_usize;
    if read_byte(input, &mut offset, "NCM3 version")? != 1 {
        return Err(Error::new(
            ErrorKind::UnsupportedVersion,
            "ncm3-version",
            "Only NCM3 version 1 can be transcoded.",
        ));
    }
    let size = [
        read_ncm3_dimension(input, &mut offset)?,
        read_ncm3_dimension(input, &mut offset)?,
        read_ncm3_dimension(input, &mut offset)?,
    ];
    let command_count = read_u32(input, &mut offset)?;
    if command_count > MAX_NCM3_COMMANDS || command_count > limits.max_commands {
        return Err(Error::limit(
            "ncm3-command-limit",
            "NCM3 command count exceeds the configured limit.",
        ));
    }
    let mut palette = BTreeSet::new();
    let mut ops = Vec::with_capacity(command_count as usize);
    for _ in 0..command_count {
        let opcode = read_byte(input, &mut offset, "NCM3 opcode")?;
        if opcode == 4 {
            let trunk_material = read_ncm3_material(input, &mut offset, &mut palette)?;
            let leaf_material = read_ncm3_material(input, &mut offset, &mut palette)?;
            let origin = read_ncm3_origin(input, &mut offset)?;
            let height = checked_u16(
                read_u32(input, &mut offset)?,
                "NCM3 tree height exceeds u16.",
            )?;
            let crown = checked_u16(
                read_u32(input, &mut offset)?,
                "NCM3 tree crown exceeds u16.",
            )?;
            ops.push(Ncm4BuildingOp::Tree {
                trunk_material,
                leaf_material,
                origin,
                height,
                crown,
            });
            continue;
        }
        let material = read_ncm3_material(input, &mut offset, &mut palette)?;
        match opcode {
            1 => {
                let (origin, box_size) = read_ncm3_box(input, &mut offset)?;
                ops.push(Ncm4BuildingOp::Box {
                    material,
                    origin,
                    size: box_size,
                });
            }
            2 => {
                let (origin, box_size) = read_ncm3_box(input, &mut offset)?;
                let count = plus_one_u16(
                    read_u32(input, &mut offset)?,
                    "NCM3 repeat count exceeds u16.",
                )?;
                let delta = [
                    read_ncm3_signed(input, &mut offset)?,
                    read_ncm3_signed(input, &mut offset)?,
                    read_ncm3_signed(input, &mut offset)?,
                ];
                ops.push(Ncm4BuildingOp::RepeatBox {
                    material,
                    origin,
                    size: box_size,
                    count,
                    delta,
                });
            }
            3 | 6 | 7 | 8 | 9 | 10 => {
                let origin = read_ncm3_origin(input, &mut offset)?;
                let width = plus_one_u16(
                    read_u32(input, &mut offset)?,
                    "NCM3 gable width exceeds u16.",
                )?;
                let depth = plus_one_u16(
                    read_u32(input, &mut offset)?,
                    "NCM3 gable depth exceeds u16.",
                )?;
                let style = match opcode {
                    3 | 8 => GableStyle::Outline,
                    6 | 9 => GableStyle::Trim,
                    7 | 10 => GableStyle::Fill,
                    _ => unreachable!(),
                };
                ops.push(Ncm4BuildingOp::Gable {
                    material,
                    origin,
                    width,
                    depth,
                    style,
                    z_oriented: opcode >= 8,
                });
            }
            5 => {
                let origin = read_ncm3_origin(input, &mut offset)?;
                let length = plus_one_u16(
                    read_u32(input, &mut offset)?,
                    "NCM3 fence length exceeds u16.",
                )?;
                let axis = u8::try_from(read_u32(input, &mut offset)?)
                    .map_err(|_| out_of_bounds("ncm3-fence-axis", "NCM3 fence axis exceeds u8."))?;
                let spacing = checked_u16(
                    read_u32(input, &mut offset)?,
                    "NCM3 fence spacing exceeds u16.",
                )?;
                ops.push(Ncm4BuildingOp::Fence {
                    material,
                    origin,
                    length,
                    axis,
                    spacing,
                });
            }
            _ => {
                return Err(Error::new(
                    ErrorKind::UnknownOpcode,
                    "ncm3-opcode",
                    "NCM3 payload contains an unknown opcode.",
                ))
            }
        }
    }
    if offset != input.len() {
        return Err(Error::new(
            ErrorKind::TrailingData,
            "ncm3-trailing-data",
            "NCM3 payload contains trailing bytes.",
        ));
    }
    Ok(Ncm4BuildingProgram {
        size,
        palette: palette.into_iter().collect(),
        ops,
        residual: Ncm4Residual::None,
    })
}

fn read_ncm3_dimension(input: &[u8], offset: &mut usize) -> Result<u16> {
    let value = read_u32(input, offset)?;
    if !(1..=256).contains(&value) {
        return Err(out_of_bounds(
            "ncm3-dimensions",
            "NCM3 dimensions must be in 1..=256.",
        ));
    }
    Ok(value as u16)
}

fn read_ncm3_material(
    input: &[u8],
    offset: &mut usize,
    palette: &mut BTreeSet<u16>,
) -> Result<u16> {
    let material = checked_u16(read_u32(input, offset)?, "NCM3 material exceeds u16.")?;
    if material == 0 {
        return Err(out_of_bounds(
            "ncm3-material",
            "NCM3 material zero is reserved for air.",
        ));
    }
    palette.insert(material);
    Ok(material)
}

fn read_ncm3_origin(input: &[u8], offset: &mut usize) -> Result<[u16; 3]> {
    Ok([
        checked_u16(read_u32(input, offset)?, "NCM3 x coordinate exceeds u16.")?,
        checked_u16(read_u32(input, offset)?, "NCM3 y coordinate exceeds u16.")?,
        checked_u16(read_u32(input, offset)?, "NCM3 z coordinate exceeds u16.")?,
    ])
}

fn read_ncm3_box(input: &[u8], offset: &mut usize) -> Result<([u16; 3], [u16; 3])> {
    let origin = read_ncm3_origin(input, offset)?;
    let size = [
        plus_one_u16(read_u32(input, offset)?, "NCM3 box width exceeds u16.")?,
        plus_one_u16(read_u32(input, offset)?, "NCM3 box height exceeds u16.")?,
        plus_one_u16(read_u32(input, offset)?, "NCM3 box depth exceeds u16.")?,
    ];
    Ok((origin, size))
}

fn read_ncm3_signed(input: &[u8], offset: &mut usize) -> Result<i16> {
    let value = read_u32(input, offset)?;
    let decoded = if value & 1 == 1 {
        -i64::from(value.div_ceil(2))
    } else {
        i64::from(value / 2)
    };
    let decoded = i16::try_from(decoded).map_err(|_| {
        out_of_bounds(
            "ncm3-repeat-delta",
            "NCM3 repeat delta exceeds the NCM4 bounded range.",
        )
    })?;
    if !(-256..=256).contains(&decoded) {
        return Err(out_of_bounds(
            "ncm3-repeat-delta",
            "NCM3 repeat delta exceeds the NCM4 bounded range.",
        ));
    }
    Ok(decoded)
}

fn write_material_bits(
    bits: &mut BitWriter,
    palette: &[u16],
    material: u16,
    widths: FieldWidths,
) -> Result<()> {
    let index = palette.binary_search(&material).map_err(|_| {
        noncanonical(
            "ncm4-material-palette",
            "NCM4 command material is absent from the palette.",
        )
    })?;
    bits.write(index as u64, widths.material)
}

fn read_material_bits(
    bits: &mut BitReader<'_>,
    palette: &[u16],
    widths: FieldWidths,
) -> Result<u16> {
    let index = bits.read(widths.material, "NCM4 material index is truncated.")? as usize;
    palette.get(index).copied().ok_or_else(|| {
        out_of_bounds(
            "ncm4-material-palette",
            "NCM4 command material index is out of range.",
        )
    })
}

fn write_origin_bits(bits: &mut BitWriter, origin: [u16; 3], widths: FieldWidths) -> Result<()> {
    for (axis, value) in origin.iter().enumerate() {
        bits.write(u64::from(*value), widths.coordinate[axis])?;
    }
    Ok(())
}

fn read_origin_bits(bits: &mut BitReader<'_>, widths: FieldWidths) -> Result<[u16; 3]> {
    Ok([
        bits.read(widths.coordinate[0], "NCM4 x coordinate is truncated.")? as u16,
        bits.read(widths.coordinate[1], "NCM4 y coordinate is truncated.")? as u16,
        bits.read(widths.coordinate[2], "NCM4 z coordinate is truncated.")? as u16,
    ])
}

fn write_size_bits(bits: &mut BitWriter, size: [u16; 3], widths: FieldWidths) -> Result<()> {
    for (axis, value) in size.iter().enumerate() {
        write_length_bits(bits, *value, widths.coordinate[axis])?;
    }
    Ok(())
}

fn read_size_bits(bits: &mut BitReader<'_>, widths: FieldWidths) -> Result<[u16; 3]> {
    Ok([
        read_length_bits(bits, widths.coordinate[0])?,
        read_length_bits(bits, widths.coordinate[1])?,
        read_length_bits(bits, widths.coordinate[2])?,
    ])
}

fn write_length_bits(bits: &mut BitWriter, value: u16, width: u8) -> Result<()> {
    if value == 0 {
        return Err(out_of_bounds(
            "ncm4-zero-length",
            "NCM4 geometry length cannot be zero.",
        ));
    }
    bits.write(u64::from(value - 1), width)
}

fn read_length_bits(bits: &mut BitReader<'_>, width: u8) -> Result<u16> {
    let value = bits.read(width, "NCM4 geometry length is truncated.")?;
    u16::try_from(value + 1).map_err(|_| Error::overflow("NCM4 geometry length exceeds u16."))
}

fn write_delta_bits(bits: &mut BitWriter, delta: [i16; 3]) -> Result<()> {
    for value in delta {
        if !(-256..=256).contains(&value) {
            return Err(out_of_bounds(
                "ncm4-delta",
                "NCM4 translation delta must be in -256..=256.",
            ));
        }
        let zigzag = if value < 0 {
            u64::from(value.unsigned_abs()) * 2 - 1
        } else {
            value as u64 * 2
        };
        bits.write(zigzag, 10)?;
    }
    Ok(())
}

fn read_delta_bits(bits: &mut BitReader<'_>) -> Result<[i16; 3]> {
    let mut output = [0_i16; 3];
    for value in &mut output {
        let encoded = bits.read(10, "NCM4 translation delta is truncated.")?;
        let decoded = if encoded & 1 == 1 {
            -((encoded + 1) as i64 / 2)
        } else {
            (encoded / 2) as i64
        };
        *value = i16::try_from(decoded)
            .map_err(|_| out_of_bounds("ncm4-delta", "NCM4 translation delta exceeds i16."))?;
        if !(-256..=256).contains(value) {
            return Err(out_of_bounds(
                "ncm4-delta",
                "NCM4 translation delta must be in -256..=256.",
            ));
        }
    }
    Ok(output)
}

fn bits_needed(max_value: u64) -> u8 {
    (64 - max_value.leading_zeros()).max(1) as u8
}

fn checked_axis(axis: u8) -> Result<usize> {
    if axis > 2 {
        Err(out_of_bounds("ncm4-axis", "NCM4 axis must be 0, 1, or 2."))
    } else {
        Ok(usize::from(axis))
    }
}

fn tangent_axes(axis: usize) -> [usize; 2] {
    match axis {
        0 => [1, 2],
        1 => [0, 2],
        2 => [0, 1],
        _ => unreachable!(),
    }
}

fn checked_dimension(value: u32) -> Result<u16> {
    if !(1..=512).contains(&value) {
        Err(out_of_bounds(
            "ncm4-dimensions",
            "NCM4 dimensions must be in 1..=512.",
        ))
    } else {
        Ok(value as u16)
    }
}

fn building_volume(size: [u16; 3]) -> Result<u32> {
    u32::from(size[0])
        .checked_mul(u32::from(size[1]))
        .and_then(|value| value.checked_mul(u32::from(size[2])))
        .ok_or_else(|| Error::overflow("NCM4 building volume overflow."))
}

fn checked_u16(value: u32, message: &'static str) -> Result<u16> {
    u16::try_from(value).map_err(|_| Error::overflow(message))
}

fn plus_one_u16(value: u32, message: &'static str) -> Result<u16> {
    value
        .checked_add(1)
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| Error::overflow(message))
}

fn read_byte(input: &[u8], offset: &mut usize, label: &'static str) -> Result<u8> {
    let value = input
        .get(*offset)
        .copied()
        .ok_or_else(|| Error::new(ErrorKind::Truncated, "ncm4-truncated", label))?;
    *offset += 1;
    Ok(value)
}

fn trim_ascii(input: &[u8]) -> &[u8] {
    let start = input
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(input.len());
    let end = input
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map(|index| index + 1)
        .unwrap_or(start);
    &input[start..end]
}

fn noncanonical(code: &'static str, message: &'static str) -> Error {
    Error::new(ErrorKind::NonCanonical, code, message)
}

fn out_of_bounds(code: &'static str, message: &'static str) -> Error {
    Error::new(ErrorKind::OutOfBounds, code, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::hash_hex;
    use crate::import_asset;
    use proptest::prelude::*;

    #[test]
    fn real_ncm3_cottage_has_a_shorter_exact_ncm4_witness() {
        let limits = LimitsV1::default();
        let imported = import_asset(
            Profile::Building,
            include_bytes!("../../../test-vectors/building/complex-cottage.ncm3"),
            &limits,
        )
        .unwrap();
        let seed = deterministic_ncm4_seed(&imported, &limits).unwrap();
        assert!(seed.audit.exact);
        assert!(seed.audit.witness_exists, "{:#?}", seed.audit);
        assert!(seed.encoding.len() < imported.incumbent_encoding.len());
        assert_eq!(seed.decoded.semantics, imported.semantics);
    }

    #[test]
    fn ncm3_golden_roots_remain_unchanged() {
        let limits = LimitsV1::default();
        for (input, expected) in [
            (
                include_bytes!("../../../test-vectors/building/boundary.ncm3").as_slice(),
                "43afefa8ff118f3a7c53fd4ff6e949cb18b52bda13b1bcda8478265ba0c1e451",
            ),
            (
                include_bytes!("../../../test-vectors/building/complex-cottage.ncm3").as_slice(),
                "5d08001e2f3d0d2fdd560774858c52ed2cf52fbcfd36c2111c07eafa5130e21b",
            ),
            (
                include_bytes!("../../../test-vectors/building/normal-box.ncm3").as_slice(),
                "37c31cab8ce82ad1fdd42c4d63819eb95af309431bcba56b0d2565c6f4584c8c",
            ),
        ] {
            let imported = import_asset(Profile::Building, input, &limits).unwrap();
            assert_eq!(hash_hex(&semantic_root(&imported.semantics)), expected);
        }
    }

    #[test]
    fn ncm4_witness_generalizes_to_variant_and_held_out_assets() {
        let limits = LimitsV1::default();
        for (input, expected_bytes, expected_root) in [
            (
                include_bytes!("../../../test-vectors/building/cottage-variant.ncm3").as_slice(),
                60_usize,
                "bc5039d7702f720c02d6e5cd821a4cad541ab50cbe2496c6b62fbe2f8eeb1290",
            ),
            (
                include_bytes!("../../../test-vectors/building/workshop-heldout.ncm3").as_slice(),
                79_usize,
                "460acc2c6fe10be340d24d5b7592586cee8f252437daf5354f7b3528df8c9d7d",
            ),
        ] {
            let imported = import_asset(Profile::Building, input, &limits).unwrap();
            let seed = deterministic_ncm4_seed(&imported, &limits).unwrap();
            assert_eq!(seed.encoding.len(), expected_bytes);
            assert_eq!(hash_hex(&seed.audit.semantic_root), expected_root);
            assert!(seed.audit.witness_exists);
            assert!(seed.audit.exact);
            assert_eq!(seed.decoded.semantics, imported.semantics);
        }
    }

    #[test]
    fn every_residual_codec_round_trips_the_same_exact_scene() {
        let limits = LimitsV1::default();
        let patches = vec![
            Ncm4Patch {
                id: 0,
                kind: ResidualKind::Set,
                material: 1,
            },
            Ncm4Patch {
                id: 1,
                kind: ResidualKind::Set,
                material: 1,
            },
            Ncm4Patch {
                id: 6,
                kind: ResidualKind::Set,
                material: 2,
            },
        ];
        let mut expected = None;
        let mut codecs = BTreeSet::new();
        for residual in residual_candidates([4, 2, 2], &patches) {
            codecs.insert(residual_tag(&residual));
            let program = Ncm4BuildingProgram {
                size: [4, 2, 2],
                palette: vec![1, 2],
                ops: Vec::new(),
                residual,
            };
            let decoded =
                decode_ncm4(&encode_ncm4_building(&program, &limits).unwrap(), &limits).unwrap();
            if let Some(expected) = &expected {
                assert_eq!(&decoded.semantics, expected);
            } else {
                expected = Some(decoded.semantics);
            }
        }
        assert_eq!(codecs.len(), 6);
    }

    #[test]
    fn malformed_compact_ncm4_is_rejected_before_expansion() {
        let limits = LimitsV1::default();
        let program = Ncm4BuildingProgram {
            size: [2, 2, 2],
            palette: vec![1],
            ops: vec![Ncm4BuildingOp::Box {
                material: 1,
                origin: [0, 0, 0],
                size: [2, 2, 2],
            }],
            residual: Ncm4Residual::None,
        };
        let encoded = encode_ncm4_building(&program, &limits).unwrap();

        let mut unknown_opcode = encoded.clone();
        unknown_opcode[14] = 0xf0;
        assert_eq!(
            decode_ncm4(&unknown_opcode, &limits).unwrap_err().kind,
            ErrorKind::UnknownOpcode
        );

        let mut nonzero_padding = encoded.clone();
        nonzero_padding[15] |= 1;
        assert_eq!(
            decode_ncm4(&nonzero_padding, &limits).unwrap_err().kind,
            ErrorKind::NonCanonical
        );

        let mut invalid_dimension = encoded.clone();
        invalid_dimension[8] = 0;
        assert_eq!(
            decode_ncm4(&invalid_dimension, &limits).unwrap_err().kind,
            ErrorKind::OutOfBounds
        );

        let mut trailing = encoded.clone();
        trailing.push(0);
        assert_eq!(
            decode_ncm4(&trailing, &limits).unwrap_err().kind,
            ErrorKind::TrailingData
        );
        let mut whitespace_trailing = encoded.clone();
        whitespace_trailing.push(b'\n');
        assert_eq!(
            decode_ncm4(&whitespace_trailing, &limits).unwrap_err().kind,
            ErrorKind::TrailingData
        );
        let mut whitespace_leading = vec![b'\n'];
        whitespace_leading.extend_from_slice(&encoded);
        assert_eq!(
            decode_ncm4(&whitespace_leading, &limits).unwrap_err().kind,
            ErrorKind::TrailingData
        );
        assert_eq!(
            decode_ncm4(&encoded[..encoded.len() - 1], &limits)
                .unwrap_err()
                .kind,
            ErrorKind::Truncated
        );

        let mut tight = limits.clone();
        tight.max_writes = 1;
        assert_eq!(
            decode_ncm4(&encoded, &tight).unwrap_err().kind,
            ErrorKind::ResourceLimit
        );

        let mut oversized_patch = Vec::new();
        oversized_patch.extend_from_slice(NCM4_MAGIC);
        oversized_patch.extend_from_slice(&[
            NCM4_VERSION,
            Profile::Building.as_u8(),
            0,
            CODEC_COMPACT_BUILDING,
        ]);
        for _ in 0..3 {
            write_u32(&mut oversized_patch, 512);
        }
        write_u32(&mut oversized_patch, 1);
        write_u32(&mut oversized_patch, 1);
        write_u32(&mut oversized_patch, 0);
        oversized_patch.push(RESIDUAL_BOXES);
        write_u32(&mut oversized_patch, 1);
        for _ in 0..3 {
            write_u32(&mut oversized_patch, 0);
        }
        for _ in 0..3 {
            write_u32(&mut oversized_patch, 511);
        }
        oversized_patch.push(ResidualKind::Set.byte());
        write_u32(&mut oversized_patch, 0);
        assert!(oversized_patch.len() < 32);
        assert_eq!(
            decode_ncm4(&oversized_patch, &limits).unwrap_err().kind,
            ErrorKind::ResourceLimit
        );

        let mut empty_sparse = encoded.clone();
        *empty_sparse.last_mut().unwrap() = RESIDUAL_SPARSE;
        empty_sparse.push(0);
        assert_eq!(
            decode_ncm4(&empty_sparse, &limits).unwrap_err().kind,
            ErrorKind::NonCanonical
        );
    }

    #[test]
    fn ncm4_text_is_canonical_and_does_not_collide_with_character_ncm4() {
        let limits = LimitsV1::default();
        let imported = import_asset(
            Profile::Building,
            include_bytes!("../../../test-vectors/building/complex-cottage.ncm3"),
            &limits,
        )
        .unwrap();
        let seed = deterministic_ncm4_seed(&imported, &limits).unwrap();
        let text = ncm4_to_text(&seed.encoding, &limits).unwrap();
        assert!(text.starts_with("NCM4P:"));
        assert!(!text.starts_with("NCM4:"));
        assert_eq!(
            decode_ncm4(text.as_bytes(), &limits).unwrap().semantics,
            imported.semantics
        );
    }

    #[test]
    fn dispatcher_recognizes_current_ncf1_text_prefix() {
        assert_eq!(detect_format(b"NCF1.AAAA"), DetectedFormat::Ncf1V15);
        assert_eq!(detect_format(b"NCF1:AAAA"), DetectedFormat::Unknown);
    }

    #[test]
    fn generic_transform_ops_round_trip() {
        let limits = LimitsV1::default();
        let program = Ncm4BuildingProgram {
            size: [12, 12, 12],
            palette: vec![1, 2],
            ops: vec![
                Ncm4BuildingOp::Box {
                    material: 1,
                    origin: [0, 0, 0],
                    size: [2, 2, 2],
                },
                Ncm4BuildingOp::Translate {
                    source_origin: [0, 0, 0],
                    source_size: [2, 2, 2],
                    delta: [3, 0, 0],
                },
                Ncm4BuildingOp::Mirror {
                    source_origin: [0, 0, 0],
                    source_size: [2, 2, 2],
                    destination_origin: [0, 3, 0],
                    axis: 0,
                },
                Ncm4BuildingOp::RotateY {
                    source_origin: [0, 0, 0],
                    source_size: [2, 2, 2],
                    destination_origin: [3, 3, 0],
                    quarter_turns: 1,
                },
                Ncm4BuildingOp::RepeatRegion {
                    source_origin: [0, 0, 0],
                    source_size: [2, 2, 2],
                    count: 2,
                    delta: [0, 0, 3],
                },
                Ncm4BuildingOp::Run {
                    material: 2,
                    origin: [0, 8, 0],
                    axis: 0,
                    length: 4,
                },
                Ncm4BuildingOp::Extrude {
                    material: 2,
                    origin: [6, 0, 0],
                    axis: 1,
                    u_length: 2,
                    v_length: 2,
                    depth: 3,
                    mask: vec![true, false, true, true],
                },
            ],
            residual: Ncm4Residual::None,
        };
        let bytes = encode_ncm4_building(&program, &limits).unwrap();
        let decoded = decode_ncm4(&bytes, &limits).unwrap();
        assert_eq!(decoded.profile, Profile::Building);
        assert!(decoded.semantics.voxel_count() > 30);
        assert_eq!(decoded.stats.commands, program.ops.len() as u32);
    }

    #[test]
    fn exact_residual_competition_is_independently_verified() {
        let limits = LimitsV1::default();
        let imported = import_asset(
            Profile::Building,
            include_bytes!("../../../test-vectors/building/normal-box.ncm3"),
            &limits,
        )
        .unwrap();
        let Semantics::Building(target) = imported.semantics else {
            unreachable!()
        };
        let structural = Ncm4BuildingProgram {
            size: target.size,
            palette: vec![1, 2],
            ops: vec![Ncm4BuildingOp::Box {
                material: 2,
                origin: [0, 0, 0],
                size: [8, 4, 8],
            }],
            residual: Ncm4Residual::None,
        };
        let exact = exactify_ncm4_building(structural, &target, &limits).unwrap();
        let bytes = encode_ncm4_building(&exact, &limits).unwrap();
        let decoded = decode_ncm4(&bytes, &limits).unwrap();
        assert_eq!(decoded.semantics, Semantics::Building(target));
        assert!(decoded.stats.residual_bytes > 1);
    }

    proptest! {
        #[test]
        fn arbitrary_ncm4_inputs_never_panic(input in proptest::collection::vec(any::<u8>(), 0..512)) {
            let _ = decode_ncm4(&input, &LimitsV1::default());
        }

        #[test]
        fn bounded_box_programs_round_trip(
            x in 0_u16..8,
            y in 0_u16..8,
            z in 0_u16..8,
            sx in 1_u16..=8,
            sy in 1_u16..=8,
            sz in 1_u16..=8,
            material in 1_u16..=4095,
        ) {
            let program = Ncm4BuildingProgram {
                size: [16, 16, 16],
                palette: vec![material],
                ops: vec![Ncm4BuildingOp::Box {
                    material,
                    origin: [x, y, z],
                    size: [sx, sy, sz],
                }],
                residual: Ncm4Residual::None,
            };
            let encoded = encode_ncm4_building(&program, &LimitsV1::default()).unwrap();
            let decoded = decode_ncm4(&encoded, &LimitsV1::default()).unwrap();
            prop_assert_eq!(
                decoded.semantics.voxel_count(),
                usize::from(sx) * usize::from(sy) * usize::from(sz)
            );
        }
    }
}

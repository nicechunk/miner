use alloc::string::String;
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::error::{Error, ErrorKind, Result};
use crate::hash::{
    candidate_encoding_hash, encoding_hash, result_id, semantic_root, task_id, Hash32,
};
use crate::import::{import_incumbent, ImportedAsset};
use crate::model::{IncumbentFormat, LimitsV1, Profile, ABSOLUTE_MAX_INPUT_BYTES};
use crate::varint::{read_u32, read_u64, write_u32, write_u64};
use crate::vm::{decode_candidate, VmStats};
use crate::{COST_MODEL_VERSION, PROTOCOL_VERSION, VM_VERSION};

pub const TASK_MAGIC: &[u8; 4] = b"NCPT";
pub const RESULT_MAGIC: &[u8; 4] = b"NCPR";
const MAX_ASSET_ID_BYTES: usize = 256;
const MAX_NETWORK_BYTES: usize = 64;
const MAX_IDENTITY_BYTES: usize = 512;
const MAX_SIGNATURE_BYTES: usize = 1024;
const MAX_ALGORITHM_BYTES: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkReferenceV1 {
    pub network: String,
    pub slot: u64,
    pub expires_at_slot: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskV1 {
    pub protocol_version: u8,
    pub vm_version: u8,
    pub cost_model_version: u8,
    pub profile: Profile,
    pub asset_id: String,
    pub semantic_root: Hash32,
    pub incumbent_format: IncumbentFormat,
    pub incumbent_encoding: Vec<u8>,
    pub incumbent_encoding_hash: Hash32,
    pub limits: LimitsV1,
    pub network_reference: Option<NetworkReferenceV1>,
}

impl TaskV1 {
    pub fn create(
        imported: ImportedAsset,
        asset_id: impl Into<String>,
        limits: LimitsV1,
        network_reference: Option<NetworkReferenceV1>,
    ) -> Result<Self> {
        let asset_id = asset_id.into();
        let task = Self {
            protocol_version: PROTOCOL_VERSION,
            vm_version: VM_VERSION,
            cost_model_version: COST_MODEL_VERSION,
            profile: imported.profile,
            asset_id,
            semantic_root: semantic_root(&imported.semantics),
            incumbent_format: imported.format,
            incumbent_encoding_hash: encoding_hash(
                imported.profile,
                imported.format,
                &imported.incumbent_encoding,
            ),
            incumbent_encoding: imported.incumbent_encoding,
            limits,
            network_reference,
        };
        task.validate()?;
        Ok(task)
    }

    pub fn validate(&self) -> Result<()> {
        validate_versions(
            self.protocol_version,
            self.vm_version,
            self.cost_model_version,
        )?;
        validate_text(&self.asset_id, MAX_ASSET_ID_BYTES, "task-asset-id")?;
        self.limits.validate()?;
        if self.incumbent_encoding.len() > self.limits.max_input_bytes as usize {
            return Err(Error::limit(
                "task-incumbent-limit",
                "Task incumbent encoding exceeds its input-byte limit.",
            ));
        }
        if let Some(reference) = &self.network_reference {
            validate_text(&reference.network, MAX_NETWORK_BYTES, "task-network")?;
            if reference
                .expires_at_slot
                .is_some_and(|expiry| expiry < reference.slot)
            {
                return Err(Error::invalid(
                    "task-expiry",
                    "Task expiry slot cannot precede its reference slot.",
                ));
            }
        }
        let expected_encoding_hash = encoding_hash(
            self.profile,
            self.incumbent_format,
            &self.incumbent_encoding,
        );
        if expected_encoding_hash != self.incumbent_encoding_hash {
            return Err(Error::new(
                ErrorKind::HashMismatch,
                "task-incumbent-hash",
                "Task incumbent encoding hash does not match its bytes.",
            ));
        }
        let semantics = import_incumbent(
            self.profile,
            self.incumbent_format,
            &self.incumbent_encoding,
            &self.limits,
        )?;
        if semantic_root(&semantics) != self.semantic_root {
            return Err(Error::new(
                ErrorKind::HashMismatch,
                "task-semantic-root",
                "Task semantic root does not match the independently decoded incumbent.",
            ));
        }
        Ok(())
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut output = Vec::new();
        output.extend_from_slice(TASK_MAGIC);
        output.extend_from_slice(&[
            self.protocol_version,
            self.vm_version,
            self.cost_model_version,
            self.profile.as_u8(),
            self.incumbent_format.as_u8(),
            0,
        ]);
        write_text(&mut output, &self.asset_id);
        output.extend_from_slice(&self.semantic_root);
        write_bytes(&mut output, &self.incumbent_encoding);
        output.extend_from_slice(&self.incumbent_encoding_hash);
        encode_limits(&mut output, &self.limits);
        match &self.network_reference {
            None => output.push(0),
            Some(reference) => {
                output.push(1);
                write_text(&mut output, &reference.network);
                write_u64(&mut output, reference.slot);
                match reference.expires_at_slot {
                    None => output.push(0),
                    Some(expiry) => {
                        output.push(1);
                        write_u64(&mut output, expiry);
                    }
                }
            }
        }
        Ok(output)
    }

    pub fn from_bytes(input: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(input, TASK_MAGIC, "task")?;
        let protocol_version = reader.byte("task protocol version")?;
        let vm_version = reader.byte("task VM version")?;
        let cost_model_version = reader.byte("task cost-model version")?;
        validate_versions(protocol_version, vm_version, cost_model_version)?;
        let profile = Profile::from_u8(reader.byte("task profile")?)?;
        let incumbent_format = IncumbentFormat::from_u8(reader.byte("task incumbent format")?)?;
        if reader.byte("task reserved byte")? != 0 {
            return Err(Error::new(
                ErrorKind::NonCanonical,
                "task-reserved",
                "Task reserved byte must be zero.",
            ));
        }
        let asset_id = reader.text(MAX_ASSET_ID_BYTES, "task asset ID")?;
        let semantic_root = reader.hash("task semantic root")?;
        let incumbent_encoding =
            reader.byte_vec(ABSOLUTE_MAX_INPUT_BYTES as usize, "task incumbent encoding")?;
        let incumbent_encoding_hash = reader.hash("task incumbent encoding hash")?;
        let limits = decode_limits(&mut reader)?;
        let network_reference = match reader.byte("task network flag")? {
            0 => None,
            1 => {
                let network = reader.text(MAX_NETWORK_BYTES, "task network")?;
                let slot = reader.u64()?;
                let expires_at_slot = match reader.byte("task expiry flag")? {
                    0 => None,
                    1 => Some(reader.u64()?),
                    _ => return Err(noncanonical_flag("task expiry")),
                };
                Some(NetworkReferenceV1 {
                    network,
                    slot,
                    expires_at_slot,
                })
            }
            _ => return Err(noncanonical_flag("task network")),
        };
        reader.finish("task")?;
        let task = Self {
            protocol_version,
            vm_version,
            cost_model_version,
            profile,
            asset_id,
            semantic_root,
            incumbent_format,
            incumbent_encoding,
            incumbent_encoding_hash,
            limits,
            network_reference,
        };
        task.validate()?;
        Ok(task)
    }

    pub fn id(&self) -> Result<Hash32> {
        Ok(task_id(&self.to_bytes()?))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MinerProofV1 {
    pub identity: Vec<u8>,
    pub signature: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchMetadataV1 {
    pub algorithm: String,
    pub attempts: u64,
    pub elapsed_ms: u64,
    pub seed: u64,
    pub threads: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultV1 {
    pub protocol_version: u8,
    pub task_id: Hash32,
    pub candidate_encoding: Vec<u8>,
    pub encoding_hash: Hash32,
    pub miner_proof: Option<MinerProofV1>,
    pub non_consensus_search_metadata: Option<SearchMetadataV1>,
}

impl ResultV1 {
    pub fn create(
        task: &TaskV1,
        candidate_encoding: Vec<u8>,
        miner_proof: Option<MinerProofV1>,
        metadata: Option<SearchMetadataV1>,
    ) -> Result<Self> {
        task.validate()?;
        let result = Self {
            protocol_version: PROTOCOL_VERSION,
            task_id: task.id()?,
            encoding_hash: candidate_encoding_hash(task.profile, &candidate_encoding),
            candidate_encoding,
            miner_proof,
            non_consensus_search_metadata: metadata,
        };
        result.validate_envelope()?;
        Ok(result)
    }

    pub fn validate_envelope(&self) -> Result<()> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(Error::new(
                ErrorKind::UnsupportedVersion,
                "result-version",
                "Result protocol version is unsupported.",
            ));
        }
        if self.candidate_encoding.is_empty()
            || self.candidate_encoding.len() > ABSOLUTE_MAX_INPUT_BYTES as usize
        {
            return Err(Error::limit(
                "result-candidate-limit",
                "Result candidate is empty or exceeds the absolute byte limit.",
            ));
        }
        if let Some(proof) = &self.miner_proof {
            if proof.identity.is_empty()
                || proof.identity.len() > MAX_IDENTITY_BYTES
                || proof.signature.is_empty()
                || proof.signature.len() > MAX_SIGNATURE_BYTES
            {
                return Err(Error::limit(
                    "result-miner-proof",
                    "Result miner identity or signature is empty or oversized.",
                ));
            }
        }
        if let Some(metadata) = &self.non_consensus_search_metadata {
            validate_text(&metadata.algorithm, MAX_ALGORITHM_BYTES, "result-algorithm")?;
            if metadata.threads == 0 {
                return Err(Error::invalid(
                    "result-threads",
                    "Search metadata thread count must be non-zero.",
                ));
            }
        }
        Ok(())
    }

    pub fn consensus_bytes(&self) -> Result<Vec<u8>> {
        self.validate_envelope()?;
        let mut output = Vec::new();
        output.extend_from_slice(RESULT_MAGIC);
        output.push(self.protocol_version);
        output.extend_from_slice(&self.task_id);
        write_bytes(&mut output, &self.candidate_encoding);
        output.extend_from_slice(&self.encoding_hash);
        match &self.miner_proof {
            None => output.push(0),
            Some(proof) => {
                output.push(1);
                write_bytes(&mut output, &proof.identity);
                write_bytes(&mut output, &proof.signature);
            }
        }
        Ok(output)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut output = self.consensus_bytes()?;
        match &self.non_consensus_search_metadata {
            None => output.push(0),
            Some(metadata) => {
                output.push(1);
                write_text(&mut output, &metadata.algorithm);
                write_u64(&mut output, metadata.attempts);
                write_u64(&mut output, metadata.elapsed_ms);
                write_u64(&mut output, metadata.seed);
                write_u32(&mut output, u32::from(metadata.threads));
            }
        }
        Ok(output)
    }

    pub fn from_bytes(input: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(input, RESULT_MAGIC, "result")?;
        let protocol_version = reader.byte("result protocol version")?;
        let task_id = reader.hash("result task ID")?;
        let candidate_encoding = reader.byte_vec(
            ABSOLUTE_MAX_INPUT_BYTES as usize,
            "result candidate encoding",
        )?;
        let encoding_hash = reader.hash("result encoding hash")?;
        let miner_proof = match reader.byte("result miner-proof flag")? {
            0 => None,
            1 => Some(MinerProofV1 {
                identity: reader.byte_vec(MAX_IDENTITY_BYTES, "result miner identity")?,
                signature: reader.byte_vec(MAX_SIGNATURE_BYTES, "result miner signature")?,
            }),
            _ => return Err(noncanonical_flag("result miner proof")),
        };
        let non_consensus_search_metadata = match reader.byte("result metadata flag")? {
            0 => None,
            1 => Some(SearchMetadataV1 {
                algorithm: reader.text(MAX_ALGORITHM_BYTES, "result algorithm")?,
                attempts: reader.u64()?,
                elapsed_ms: reader.u64()?,
                seed: reader.u64()?,
                threads: u16::try_from(reader.u32()?).map_err(|_| {
                    Error::new(
                        ErrorKind::OutOfBounds,
                        "result-threads",
                        "Result thread count exceeds u16.",
                    )
                })?,
            }),
            _ => return Err(noncanonical_flag("result metadata")),
        };
        reader.finish("result")?;
        let result = Self {
            protocol_version,
            task_id,
            candidate_encoding,
            encoding_hash,
            miner_proof,
            non_consensus_search_metadata,
        };
        result.validate_envelope()?;
        Ok(result)
    }

    pub fn id(&self) -> Result<Hash32> {
        Ok(result_id(&self.consensus_bytes()?))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationReportV1 {
    pub accepted: bool,
    pub exact: bool,
    pub improved: bool,
    pub mismatch_count: u64,
    pub task_id: Hash32,
    pub target_semantic_root: Hash32,
    pub candidate_semantic_root: Hash32,
    pub incumbent_encoding_hash: Hash32,
    pub candidate_encoding_hash: Hash32,
    pub incumbent_bytes: u32,
    pub candidate_bytes: u32,
    pub saved_bytes: i64,
    pub saved_bps: i64,
    pub vm_stats: VmStats,
}

pub fn verify_result(task: &TaskV1, result: &ResultV1) -> Result<VerificationReportV1> {
    task.validate()?;
    result.validate_envelope()?;
    let expected_task_id = task.id()?;
    if result.task_id != expected_task_id {
        return Err(Error::new(
            ErrorKind::HashMismatch,
            "result-task-id",
            "Result task ID does not match the supplied task.",
        ));
    }
    let expected_encoding_hash = candidate_encoding_hash(task.profile, &result.candidate_encoding);
    if result.encoding_hash != expected_encoding_hash {
        return Err(Error::new(
            ErrorKind::HashMismatch,
            "result-encoding-hash",
            "Result encoding hash does not match its candidate bytes.",
        ));
    }
    let target = import_incumbent(
        task.profile,
        task.incumbent_format,
        &task.incumbent_encoding,
        &task.limits,
    )?;
    let decoded = decode_candidate(&result.candidate_encoding, task.profile, &task.limits)?;
    let candidate_semantic_root = semantic_root(&decoded.semantics);
    let mismatch_count = target.mismatch_count(&decoded.semantics);
    let exact = mismatch_count == 0 && candidate_semantic_root == task.semantic_root;
    if mismatch_count == 0 && candidate_semantic_root != task.semantic_root {
        return Err(Error::new(
            ErrorKind::HashMismatch,
            "result-semantic-root",
            "Candidate equality and semantic-root calculation disagree.",
        ));
    }
    let incumbent_bytes = u32::try_from(task.incumbent_encoding.len())
        .map_err(|_| Error::overflow("Incumbent byte count exceeds u32."))?;
    let candidate_bytes = u32::try_from(result.candidate_encoding.len())
        .map_err(|_| Error::overflow("Candidate byte count exceeds u32."))?;
    let saved_bytes = i64::from(incumbent_bytes) - i64::from(candidate_bytes);
    let saved_bps = if incumbent_bytes == 0 {
        0
    } else {
        saved_bytes * 10_000 / i64::from(incumbent_bytes)
    };
    let improved = candidate_bytes < incumbent_bytes;
    Ok(VerificationReportV1 {
        accepted: exact && improved,
        exact,
        improved,
        mismatch_count,
        task_id: expected_task_id,
        target_semantic_root: task.semantic_root,
        candidate_semantic_root,
        incumbent_encoding_hash: task.incumbent_encoding_hash,
        candidate_encoding_hash: expected_encoding_hash,
        incumbent_bytes,
        candidate_bytes,
        saved_bytes,
        saved_bps,
        vm_stats: decoded.stats,
    })
}

pub fn verify_improvement(task: &TaskV1, result: &ResultV1) -> Result<VerificationReportV1> {
    let report = verify_result(task, result)?;
    if !report.exact {
        return Err(Error::new(
            ErrorKind::SemanticMismatch,
            "result-semantic-mismatch",
            "Candidate does not exactly reproduce the task semantics.",
        ));
    }
    if !report.improved {
        return Err(Error::new(
            ErrorKind::NotSmaller,
            "result-not-smaller",
            "Exact candidate is not strictly smaller than the incumbent encoding.",
        ));
    }
    Ok(report)
}

fn validate_versions(protocol: u8, vm: u8, cost: u8) -> Result<()> {
    if protocol != PROTOCOL_VERSION || vm != VM_VERSION || cost != COST_MODEL_VERSION {
        return Err(Error::new(
            ErrorKind::UnsupportedVersion,
            "protocol-version",
            "Task protocol, VM, or cost-model version is unsupported.",
        ));
    }
    Ok(())
}

fn validate_text(value: &str, maximum: usize, code: &'static str) -> Result<()> {
    if value.is_empty() || value.len() > maximum || value.as_bytes().contains(&0) {
        return Err(Error::invalid(
            code,
            "Text field is empty, oversized, or contains a NUL byte.",
        ));
    }
    Ok(())
}

fn encode_limits(output: &mut Vec<u8>, limits: &LimitsV1) {
    for value in [
        limits.max_input_bytes,
        limits.max_commands,
        limits.max_materials,
        limits.max_patches,
        limits.max_voxels,
        limits.max_writes,
    ] {
        write_u32(output, value);
    }
    write_u64(output, limits.max_decode_units);
    write_u64(output, limits.max_memory_bytes);
    write_u32(output, limits.max_expanded_per_op);
}

fn decode_limits(reader: &mut Reader<'_>) -> Result<LimitsV1> {
    let limits = LimitsV1 {
        max_input_bytes: reader.u32()?,
        max_commands: reader.u32()?,
        max_materials: reader.u32()?,
        max_patches: reader.u32()?,
        max_voxels: reader.u32()?,
        max_writes: reader.u32()?,
        max_decode_units: reader.u64()?,
        max_memory_bytes: reader.u64()?,
        max_expanded_per_op: reader.u32()?,
    };
    limits.validate()?;
    Ok(limits)
}

fn write_text(output: &mut Vec<u8>, value: &str) {
    write_bytes(output, value.as_bytes());
}

fn write_bytes(output: &mut Vec<u8>, value: &[u8]) {
    write_u32(output, value.len() as u32);
    output.extend_from_slice(value);
}

fn noncanonical_flag(label: &'static str) -> Error {
    Error::new(
        ErrorKind::NonCanonical,
        "noncanonical-flag",
        [label, " flag must be zero or one."].concat(),
    )
}

struct Reader<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(input: &'a [u8], magic: &[u8; 4], label: &'static str) -> Result<Self> {
        if input.len() < 4 {
            return Err(Error::new(
                ErrorKind::Truncated,
                "protocol-header",
                [label, " file is shorter than its magic."].concat(),
            ));
        }
        if &input[..4] != magic {
            return Err(Error::invalid(
                "protocol-magic",
                [label, " file has the wrong magic."].concat(),
            ));
        }
        Ok(Self { input, offset: 4 })
    }

    fn byte(&mut self, label: &'static str) -> Result<u8> {
        let value = *self
            .input
            .get(self.offset)
            .ok_or_else(|| Error::new(ErrorKind::Truncated, "protocol-truncated", label))?;
        self.offset += 1;
        Ok(value)
    }

    fn bytes(&mut self, length: usize, label: &'static str) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| Error::overflow("Protocol byte offset overflow."))?;
        let value = self
            .input
            .get(self.offset..end)
            .ok_or_else(|| Error::new(ErrorKind::Truncated, "protocol-truncated", label))?;
        self.offset = end;
        Ok(value)
    }

    fn u32(&mut self) -> Result<u32> {
        read_u32(self.input, &mut self.offset)
    }

    fn u64(&mut self) -> Result<u64> {
        read_u64(self.input, &mut self.offset)
    }

    fn byte_vec(&mut self, maximum: usize, label: &'static str) -> Result<Vec<u8>> {
        let length = self.u32()? as usize;
        if length > maximum {
            return Err(Error::limit(
                "protocol-byte-limit",
                [label, " exceeds its byte limit."].concat(),
            ));
        }
        Ok(self.bytes(length, label)?.to_vec())
    }

    fn text(&mut self, maximum: usize, label: &'static str) -> Result<String> {
        let bytes = self.byte_vec(maximum, label)?;
        let value = String::from_utf8(bytes).map_err(|_| {
            Error::new(
                ErrorKind::NonCanonical,
                "protocol-utf8",
                [label, " must be valid UTF-8."].concat(),
            )
        })?;
        validate_text(&value, maximum, "protocol-text")?;
        Ok(value)
    }

    fn hash(&mut self, label: &'static str) -> Result<Hash32> {
        let mut hash = [0_u8; 32];
        hash.copy_from_slice(self.bytes(32, label)?);
        Ok(hash)
    }

    fn finish(self, label: &'static str) -> Result<()> {
        if self.offset != self.input.len() {
            return Err(Error::new(
                ErrorKind::TrailingData,
                "protocol-trailing-data",
                [label, " file contains trailing bytes."].concat(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Coord, TerrainSemantics};
    use crate::vm::{encode_candidate, CandidateProgram, TerrainOp, TerrainProgram};

    fn task() -> TaskV1 {
        let mut account = vec![0_u8; 16 + 3];
        account[0..4].copy_from_slice(b"NCBK");
        account[4] = 1;
        account[6..8].copy_from_slice(&1_u16.to_le_bytes());
        account[8..10].copy_from_slice(&1_u16.to_le_bytes());
        account[16] = 1;
        let imported =
            crate::import::import_asset(Profile::TerrainDelta, &account, &LimitsV1::default())
                .unwrap();
        TaskV1::create(imported, "terrain:test", LimitsV1::default(), None).unwrap()
    }

    #[test]
    fn task_and_result_binary_round_trip() {
        let task = task();
        let bytes = task.to_bytes().unwrap();
        assert_eq!(TaskV1::from_bytes(&bytes).unwrap(), task);
        let candidate = encode_candidate(
            &CandidateProgram::TerrainDelta(TerrainProgram {
                min_y: 0,
                ops: vec![TerrainOp::DeleteRun {
                    start: 1,
                    length: 1,
                }],
                patches: vec![],
            }),
            &task.limits,
        )
        .unwrap();
        let result = ResultV1::create(&task, candidate, None, None).unwrap();
        let bytes = result.to_bytes().unwrap();
        assert_eq!(ResultV1::from_bytes(&bytes).unwrap(), result);
        assert!(verify_result(&task, &result).unwrap().exact);
    }

    #[test]
    fn semantic_root_changes_with_final_voxel() {
        let left = crate::model::Semantics::TerrainDelta(TerrainSemantics {
            min_y: 0,
            deleted: vec![Coord { x: 0, y: 0, z: 0 }],
        });
        let right = crate::model::Semantics::TerrainDelta(TerrainSemantics {
            min_y: 0,
            deleted: vec![Coord { x: 1, y: 0, z: 0 }],
        });
        assert_ne!(semantic_root(&left), semantic_root(&right));
    }
}

use alloc::vec::Vec;

use pouw_core::{
    candidate_encoding_hash, semantic_root, CandidateProgram, Error, ErrorKind, Hash32, Result,
    Semantics, TaskV1, COST_MODEL_VERSION, PROTOCOL_VERSION, VM_VERSION,
};
use serde::{Deserialize, Serialize};

use crate::{evaluate_program, SearchCandidate, SearchConfig, SEARCH_ENGINE_VERSION};

const MAGIC: &[u8] = b"NCPC1\n";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointV1 {
    pub checkpoint_version: u8,
    pub search_engine_version: u8,
    pub protocol_version: u8,
    pub vm_version: u8,
    pub cost_model_version: u8,
    pub task_id: Hash32,
    pub semantic_root: Hash32,
    pub task_bytes: Vec<u8>,
    pub config: SearchConfig,
    pub generation: u32,
    pub attempts: u64,
    pub best_program: CandidateProgram,
    pub best_encoding: Vec<u8>,
    pub best_encoding_hash: Hash32,
}

impl CheckpointV1 {
    pub fn new(
        task: &TaskV1,
        config: SearchConfig,
        generation: u32,
        attempts: u64,
        best: &SearchCandidate,
    ) -> Result<Self> {
        let checkpoint = Self {
            checkpoint_version: 1,
            search_engine_version: SEARCH_ENGINE_VERSION,
            protocol_version: PROTOCOL_VERSION,
            vm_version: VM_VERSION,
            cost_model_version: COST_MODEL_VERSION,
            task_id: task.id()?,
            semantic_root: task.semantic_root,
            task_bytes: task.to_bytes()?,
            config,
            generation,
            attempts,
            best_program: best.program.clone(),
            best_encoding: best.encoding.clone(),
            best_encoding_hash: best.encoding_hash,
        };
        checkpoint.validate_for_task(task)?;
        Ok(checkpoint)
    }

    pub fn validate_for_task(&self, task: &TaskV1) -> Result<()> {
        if self.checkpoint_version != 1
            || self.search_engine_version != SEARCH_ENGINE_VERSION
            || self.protocol_version != PROTOCOL_VERSION
            || self.vm_version != VM_VERSION
            || self.cost_model_version != COST_MODEL_VERSION
        {
            return Err(Error::new(
                ErrorKind::UnsupportedVersion,
                "checkpoint-version",
                "Checkpoint protocol, VM, cost model, or search-engine version is unsupported.",
            ));
        }
        task.validate()?;
        if self.task_id != task.id()?
            || self.semantic_root != task.semantic_root
            || TaskV1::from_bytes(&self.task_bytes)? != *task
        {
            return Err(Error::new(
                ErrorKind::HashMismatch,
                "checkpoint-task",
                "Checkpoint belongs to a different task or semantic root.",
            ));
        }
        self.config.validate(&task.limits)?;
        if self.best_encoding.is_empty()
            || self.best_encoding.len() > task.limits.max_input_bytes as usize
            || candidate_encoding_hash(task.profile, &self.best_encoding) != self.best_encoding_hash
        {
            return Err(Error::new(
                ErrorKind::HashMismatch,
                "checkpoint-candidate",
                "Checkpoint best candidate is empty, oversized, or has the wrong encoding hash.",
            ));
        }
        Ok(())
    }

    pub(crate) fn best_candidate(
        &self,
        task: &TaskV1,
        target: &Semantics,
    ) -> Result<SearchCandidate> {
        self.validate_for_task(task)?;
        if semantic_root(target) != self.semantic_root {
            return Err(Error::new(
                ErrorKind::HashMismatch,
                "checkpoint-target",
                "Checkpoint target semantics do not match its recorded root.",
            ));
        }
        let candidate = evaluate_program(self.best_program.clone(), target, &task.limits)?;
        if candidate.encoding != self.best_encoding
            || candidate.encoding_hash != self.best_encoding_hash
        {
            return Err(Error::new(
                ErrorKind::HashMismatch,
                "checkpoint-replay",
                "Checkpoint program does not reproduce its recorded candidate bytes.",
            ));
        }
        Ok(candidate)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let json = serde_json::to_vec(self).map_err(|error| {
            Error::new(ErrorKind::Internal, "checkpoint-json", error.to_string())
        })?;
        let mut output = Vec::with_capacity(MAGIC.len() + json.len());
        output.extend_from_slice(MAGIC);
        output.extend_from_slice(&json);
        Ok(output)
    }

    pub fn from_bytes(input: &[u8]) -> Result<Self> {
        if !input.starts_with(MAGIC) {
            return Err(Error::invalid(
                "checkpoint-magic",
                "Checkpoint magic must be NCPC1.",
            ));
        }
        serde_json::from_slice(&input[MAGIC.len()..]).map_err(|error| {
            Error::new(
                ErrorKind::InvalidInput,
                "checkpoint-json",
                error.to_string(),
            )
        })
    }
}

extern crate alloc;

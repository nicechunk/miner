mod ncf1;
mod ncm3;
mod terrain;

use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::{IncumbentFormat, LimitsV1, Profile, Semantics};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedAsset {
    pub profile: Profile,
    pub format: IncumbentFormat,
    pub incumbent_encoding: Vec<u8>,
    pub semantics: Semantics,
}

pub fn import_asset(profile: Profile, input: &[u8], limits: &LimitsV1) -> Result<ImportedAsset> {
    limits.validate()?;
    if input.len() > limits.max_input_bytes as usize {
        return Err(Error::limit(
            "input-byte-limit",
            "Input exceeds the task's byte limit.",
        ));
    }
    if crate::ncm4::looks_like_ncm4(input) {
        let decoded = crate::ncm4::decode_ncm4(input, limits)?;
        if decoded.semantics.profile() != profile {
            return Err(Error::invalid(
                "profile-format-mismatch",
                "NCM4 profile does not match the selected profile.",
            ));
        }
        decoded.semantics.validate(limits)?;
        return Ok(ImportedAsset {
            profile,
            format: IncumbentFormat::Ncm4PouwV1,
            incumbent_encoding: decoded.raw_encoding,
            semantics: decoded.semantics,
        });
    }
    let imported = match profile {
        Profile::TerrainDelta => terrain::import(input)?,
        Profile::Building => ncm3::import(input, limits)?,
        Profile::ForgedItem => ncf1::import(input, limits)?,
    };
    imported.semantics.validate(limits)?;
    Ok(imported)
}

pub fn import_incumbent(
    profile: Profile,
    format: IncumbentFormat,
    input: &[u8],
    limits: &LimitsV1,
) -> Result<Semantics> {
    let imported = match format {
        IncumbentFormat::ChunkBrokenV1 if profile == Profile::TerrainDelta => {
            terrain::import(input)?
        }
        IncumbentFormat::Ncm3V1 if profile == Profile::Building => ncm3::import_raw(input, limits)?,
        IncumbentFormat::Ncf1V15 if profile == Profile::ForgedItem => {
            ncf1::import_raw(input, limits)?
        }
        IncumbentFormat::PouwVmV1 => {
            return crate::vm::decode_candidate(input, profile, limits)
                .map(|decoded| decoded.semantics)
        }
        IncumbentFormat::Ncm4PouwV1 => {
            let decoded = crate::ncm4::decode_ncm4(input, limits)?;
            if decoded.semantics.profile() != profile {
                return Err(Error::invalid(
                    "profile-format-mismatch",
                    "NCM4 profile does not match the selected profile.",
                ));
            }
            return Ok(decoded.semantics);
        }
        _ => {
            return Err(Error::invalid(
                "profile-format-mismatch",
                "Incumbent format does not belong to the selected profile.",
            ))
        }
    };
    imported.semantics.validate(limits)?;
    Ok(imported.semantics)
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

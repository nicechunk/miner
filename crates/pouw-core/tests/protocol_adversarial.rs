use pouw_core::varint::{read_u32, read_u64, write_i16, write_u32, write_u64};
use pouw_core::{
    candidate_encoding_hash, decode_candidate, encode_candidate, import_asset, semantic_root,
    verify_improvement, verify_result, CandidateProgram, ErrorKind, ForgeComponent, ForgeEquipment,
    ForgeGeometry, ForgedSemantics, LimitsV1, Profile, ResultV1, Semantics, TaskV1, TerrainOp,
    TerrainProgram, COST_MODEL_VERSION, TERRAIN_UNIVERSE, VM_MAGIC, VM_VERSION,
};
use proptest::collection::{btree_set, vec};
use proptest::prelude::*;

fn terrain_account(ids: &[u32], min_y: i16) -> Vec<u8> {
    let capacity = u16::try_from(ids.len()).expect("test account capacity");
    let mut account = vec![0_u8; 16 + ids.len() * 3];
    account[0..4].copy_from_slice(b"NCBK");
    account[4] = 1;
    account[6..8].copy_from_slice(&capacity.to_le_bytes());
    account[8..10].copy_from_slice(&capacity.to_le_bytes());
    account[10..12].copy_from_slice(&min_y.to_le_bytes());
    for (index, id) in ids.iter().copied().enumerate() {
        let offset = 16 + index * 3;
        account[offset] = id as u8;
        account[offset + 1] = (id >> 8) as u8;
        account[offset + 2] = (id >> 16) as u8;
    }
    account
}

fn terrain_task(ids: &[u32]) -> TaskV1 {
    let limits = LimitsV1::default();
    let imported = import_asset(Profile::TerrainDelta, &terrain_account(ids, 0), &limits).unwrap();
    TaskV1::create(imported, "adversarial:terrain", limits, None).unwrap()
}

fn terrain_candidate(min_y: i16, ops: Vec<TerrainOp>) -> Vec<u8> {
    encode_candidate(
        &CandidateProgram::TerrainDelta(TerrainProgram {
            min_y,
            ops,
            patches: vec![],
        }),
        &LimitsV1::default(),
    )
    .unwrap()
}

fn terrain_wire_prefix() -> Vec<u8> {
    let mut bytes = VM_MAGIC.to_vec();
    bytes.extend_from_slice(&[
        VM_VERSION,
        Profile::TerrainDelta.as_u8(),
        COST_MODEL_VERSION,
        0,
    ]);
    bytes
}

#[test]
fn equivalent_programs_share_semantic_root_but_not_encoding_hash() {
    let limits = LimitsV1::default();
    let run = terrain_candidate(
        0,
        vec![TerrainOp::DeleteRun {
            start: 0,
            length: 4,
        }],
    );
    let boxed = terrain_candidate(
        0,
        vec![TerrainOp::DeleteBox {
            x: 0,
            y: 0,
            z: 0,
            width: 4,
            height: 1,
            depth: 1,
        }],
    );
    let run_semantics = decode_candidate(&run, Profile::TerrainDelta, &limits)
        .unwrap()
        .semantics;
    let box_semantics = decode_candidate(&boxed, Profile::TerrainDelta, &limits)
        .unwrap()
        .semantics;
    assert_eq!(run_semantics, box_semantics);
    assert_eq!(semantic_root(&run_semantics), semantic_root(&box_semantics));
    assert_ne!(
        candidate_encoding_hash(Profile::TerrainDelta, &run),
        candidate_encoding_hash(Profile::TerrainDelta, &boxed)
    );
}

#[test]
fn forged_geometry_and_identity_properties_change_the_semantic_root() {
    let base = ForgedSemantics {
        equipment: ForgeEquipment {
            mass_5g: 10,
            encoded_volume: 20,
            attributes_6: [3; 12],
        },
        geometry: ForgeGeometry::Components {
            components: vec![ForgeComponent {
                resource: 1,
                color_444: 0xabc,
                dimensions_q: [20, 30, 40],
                offset_q: [0, 1, -1],
                grip: None,
                solid: vec![0, 1, 2],
                paint: vec![],
            }],
        },
    };
    let mut changed_property = base.clone();
    changed_property.equipment.mass_5g += 1;
    let mut changed_geometry = base.clone();
    let ForgeGeometry::Components { components } = &mut changed_geometry.geometry else {
        unreachable!()
    };
    components[0].solid.push(3);
    assert_ne!(
        semantic_root(&Semantics::ForgedItem(base.clone())),
        semantic_root(&Semantics::ForgedItem(changed_property))
    );
    assert_ne!(
        semantic_root(&Semantics::ForgedItem(base)),
        semantic_root(&Semantics::ForgedItem(changed_geometry))
    );
}

#[test]
fn verifier_recomputes_task_candidate_hashes_cost_and_semantics() {
    let task = terrain_task(&[0]);
    let candidate = terrain_candidate(
        0,
        vec![TerrainOp::DeleteRun {
            start: 0,
            length: 1,
        }],
    );
    let result = ResultV1::create(&task, candidate.clone(), None, None).unwrap();
    assert!(verify_result(&task, &result).unwrap().exact);

    let mut bad_hash = result.clone();
    bad_hash.encoding_hash[0] ^= 1;
    assert_eq!(
        verify_result(&task, &bad_hash).unwrap_err().kind,
        ErrorKind::HashMismatch
    );

    let mut bad_task_id = result.clone();
    bad_task_id.task_id[0] ^= 1;
    assert_eq!(
        verify_result(&task, &bad_task_id).unwrap_err().kind,
        ErrorKind::HashMismatch
    );

    let mut bad_root = task.clone();
    bad_root.semantic_root[0] ^= 1;
    assert_eq!(
        bad_root.validate().unwrap_err().kind,
        ErrorKind::HashMismatch
    );

    let mut bad_cost = candidate;
    bad_cost[6] ^= 1;
    let bad_cost_result = ResultV1::create(&task, bad_cost, None, None).unwrap();
    assert_eq!(
        verify_result(&task, &bad_cost_result).unwrap_err().kind,
        ErrorKind::UnsupportedVersion
    );

    let different = terrain_candidate(
        0,
        vec![TerrainOp::DeleteRun {
            start: 1,
            length: 1,
        }],
    );
    let different_result = ResultV1::create(&task, different, None, None).unwrap();
    let report = verify_result(&task, &different_result).unwrap();
    assert!(!report.exact);
    assert_eq!(report.mismatch_count, 2);
    assert_eq!(
        verify_improvement(&task, &different_result)
            .unwrap_err()
            .kind,
        ErrorKind::SemanticMismatch
    );
}

#[test]
fn malformed_vm_and_protocol_envelopes_are_rejected() {
    let limits = LimitsV1::default();
    let valid = terrain_candidate(
        0,
        vec![TerrainOp::DeleteRun {
            start: 0,
            length: 1,
        }],
    );

    for end in 0..valid.len() {
        assert!(decode_candidate(&valid[..end], Profile::TerrainDelta, &limits).is_err());
    }

    let mut trailing = valid.clone();
    trailing.push(0);
    assert_eq!(
        decode_candidate(&trailing, Profile::TerrainDelta, &limits)
            .unwrap_err()
            .kind,
        ErrorKind::TrailingData
    );

    let mut noncanonical_varint = terrain_wire_prefix();
    noncanonical_varint.extend_from_slice(&[0x80, 0x00]);
    assert_eq!(
        decode_candidate(&noncanonical_varint, Profile::TerrainDelta, &limits)
            .unwrap_err()
            .kind,
        ErrorKind::NonCanonical
    );

    let mut unknown_opcode = terrain_wire_prefix();
    write_i16(&mut unknown_opcode, 0);
    write_u32(&mut unknown_opcode, 1);
    unknown_opcode.push(0xff);
    assert_eq!(
        decode_candidate(&unknown_opcode, Profile::TerrainDelta, &limits)
            .unwrap_err()
            .kind,
        ErrorKind::UnknownOpcode
    );

    let mut expansion_bomb = terrain_wire_prefix();
    write_i16(&mut expansion_bomb, 0);
    write_u32(&mut expansion_bomb, 1);
    expansion_bomb.push(1);
    write_u32(&mut expansion_bomb, 0);
    write_u32(&mut expansion_bomb, 9);
    write_u32(&mut expansion_bomb, 0);
    let mut tight_limits = limits.clone();
    tight_limits.max_expanded_per_op = 8;
    assert_eq!(
        decode_candidate(&expansion_bomb, Profile::TerrainDelta, &tight_limits)
            .unwrap_err()
            .kind,
        ErrorKind::ResourceLimit
    );

    let task = terrain_task(&[0]);
    let task_bytes = task.to_bytes().unwrap();
    assert!(TaskV1::from_bytes(&task_bytes[..task_bytes.len() - 1]).is_err());
    let mut task_trailing = task_bytes;
    task_trailing.push(0);
    assert_eq!(
        TaskV1::from_bytes(&task_trailing).unwrap_err().kind,
        ErrorKind::TrailingData
    );

    let result = ResultV1::create(&task, valid, None, None).unwrap();
    let result_bytes = result.to_bytes().unwrap();
    assert!(ResultV1::from_bytes(&result_bytes[..result_bytes.len() - 1]).is_err());
    let mut result_trailing = result_bytes;
    result_trailing.push(0);
    assert_eq!(
        ResultV1::from_bytes(&result_trailing).unwrap_err().kind,
        ErrorKind::TrailingData
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn arbitrary_short_inputs_never_panic_or_allocate_unboundedly(input in vec(any::<u8>(), 0..4096)) {
        let limits = LimitsV1::default();
        for profile in [Profile::TerrainDelta, Profile::Building, Profile::ForgedItem] {
            let _ = decode_candidate(&input, profile, &limits);
            let _ = import_asset(profile, &input, &limits);
        }
        let _ = TaskV1::from_bytes(&input);
        let _ = ResultV1::from_bytes(&input);
    }

    #[test]
    fn canonical_varints_round_trip(value32 in any::<u32>(), value64 in any::<u64>()) {
        let mut bytes32 = Vec::new();
        write_u32(&mut bytes32, value32);
        let mut offset32 = 0;
        prop_assert_eq!(read_u32(&bytes32, &mut offset32).unwrap(), value32);
        prop_assert_eq!(offset32, bytes32.len());

        let mut bytes64 = Vec::new();
        write_u64(&mut bytes64, value64);
        let mut offset64 = 0;
        prop_assert_eq!(read_u64(&bytes64, &mut offset64).unwrap(), value64);
        prop_assert_eq!(offset64, bytes64.len());
    }

    #[test]
    fn random_terrain_task_result_round_trips(ids in btree_set(0_u32..TERRAIN_UNIVERSE, 1..64)) {
        let ids = ids.into_iter().collect::<Vec<_>>();
        let task = terrain_task(&ids);
        let task_bytes = task.to_bytes().unwrap();
        prop_assert_eq!(TaskV1::from_bytes(&task_bytes).unwrap(), task.clone());

        let candidate = terrain_candidate(
            0,
            vec![TerrainOp::EliasFano { values: ids }],
        );
        let result = ResultV1::create(&task, candidate, None, None).unwrap();
        let result_bytes = result.to_bytes().unwrap();
        prop_assert_eq!(ResultV1::from_bytes(&result_bytes).unwrap(), result.clone());
        prop_assert!(verify_result(&task, &result).unwrap().exact);
    }
}

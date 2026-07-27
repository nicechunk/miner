use std::collections::{BTreeMap, BTreeSet};

use pouw_core::{
    building_coord_id, decode_candidate, encode_candidate, forge_cell_from_id, forge_cell_id,
    terrain_coord_id, BuildingOp, BuildingPatch, BuildingPatchKind, BuildingProgram,
    CandidateProgram, Error, ErrorKind, ForgeComponentProgram, ForgeGeometry, ForgePatchKind,
    ForgeProgram, ForgeProgramGeometry, ForgeSolidOp, ForgeSolidPatch, ForgedSemantics, LimitsV1,
    Result, Semantics, TerrainOp, TerrainPatch, TerrainPatchKind, TerrainProgram, Voxel,
    FORGE_CELL_COUNT, FORGE_GRID_X, FORGE_GRID_Y, FORGE_GRID_Z,
};

use crate::{evaluate_program, SearchCandidate};

pub fn baseline_candidates(target: &Semantics, limits: &LimitsV1) -> Result<Vec<SearchCandidate>> {
    target.validate(limits)?;
    let programs = match target {
        Semantics::TerrainDelta(value) => terrain_programs(value),
        Semantics::Building(value) => building_programs(value),
        Semantics::ForgedItem(value) => forged_programs(value),
    };
    let mut candidates = Vec::new();
    for program in programs {
        if let Ok(candidate) = evaluate_program(program, target, limits) {
            if !candidates
                .iter()
                .any(|value: &SearchCandidate| value.encoding == candidate.encoding)
            {
                candidates.push(candidate);
            }
        }
    }
    candidates.sort_by(SearchCandidate::fitness_cmp);
    if candidates.is_empty() {
        return Err(Error::new(
            ErrorKind::Internal,
            "baseline-generation",
            "No exact deterministic baseline could be encoded.",
        ));
    }
    Ok(candidates)
}

pub fn best_baseline(target: &Semantics, limits: &LimitsV1) -> Result<SearchCandidate> {
    baseline_candidates(target, limits)?
        .into_iter()
        .next()
        .ok_or_else(|| {
            Error::new(
                ErrorKind::Internal,
                "baseline-generation",
                "No baseline candidate.",
            )
        })
}

pub(crate) fn exactify(
    mut program: CandidateProgram,
    target: &Semantics,
    limits: &LimitsV1,
) -> Result<CandidateProgram> {
    match &mut program {
        CandidateProgram::TerrainDelta(candidate) => candidate.patches.clear(),
        CandidateProgram::Building(candidate) => candidate.patches.clear(),
        CandidateProgram::ForgedItem(candidate) => {
            if let ForgeProgramGeometry::Components { components } = &mut candidate.geometry {
                for component in components {
                    component.patches.clear();
                }
            }
        }
    }
    let base = decode_program_semantics(&program, program.profile(), limits)?;
    match (&mut program, target, base) {
        (
            CandidateProgram::TerrainDelta(candidate),
            Semantics::TerrainDelta(target),
            Semantics::TerrainDelta(base),
        ) => {
            let base: BTreeSet<u32> = base.deleted.iter().copied().map(terrain_coord_id).collect();
            let target: BTreeSet<u32> = target
                .deleted
                .iter()
                .copied()
                .map(terrain_coord_id)
                .collect();
            let ids = base.union(&target).copied().collect::<BTreeSet<_>>();
            candidate.patches = ids
                .into_iter()
                .filter_map(|id| match (base.contains(&id), target.contains(&id)) {
                    (false, true) => Some(TerrainPatch {
                        id,
                        kind: TerrainPatchKind::Add,
                    }),
                    (true, false) => Some(TerrainPatch {
                        id,
                        kind: TerrainPatchKind::Restore,
                    }),
                    _ => None,
                })
                .collect();
        }
        (
            CandidateProgram::Building(candidate),
            Semantics::Building(target),
            Semantics::Building(base),
        ) => {
            let base = base
                .voxels
                .iter()
                .map(|voxel| (building_coord_id(base.size, *voxel), voxel.material))
                .collect::<BTreeMap<_, _>>();
            let target_map = target
                .voxels
                .iter()
                .map(|voxel| (building_coord_id(target.size, *voxel), voxel.material))
                .collect::<BTreeMap<_, _>>();
            let ids = base
                .keys()
                .chain(target_map.keys())
                .copied()
                .collect::<BTreeSet<_>>();
            candidate.patches = ids
                .into_iter()
                .filter_map(|id| match (base.get(&id), target_map.get(&id)) {
                    (None, Some(material)) => Some(BuildingPatch {
                        id,
                        kind: BuildingPatchKind::Set,
                        material: *material,
                    }),
                    (Some(_), None) => Some(BuildingPatch {
                        id,
                        kind: BuildingPatchKind::Clear,
                        material: 0,
                    }),
                    (Some(left), Some(right)) if left != right => Some(BuildingPatch {
                        id,
                        kind: BuildingPatchKind::Paint,
                        material: *right,
                    }),
                    _ => None,
                })
                .collect();
        }
        (
            CandidateProgram::ForgedItem(candidate),
            Semantics::ForgedItem(target),
            Semantics::ForgedItem(base),
        ) => {
            let ForgeProgramGeometry::Components { components } = &mut candidate.geometry else {
                return Ok(program);
            };
            let (
                ForgeGeometry::Components {
                    components: base_components,
                },
                ForgeGeometry::Components {
                    components: target_components,
                },
            ) = (&base.geometry, &target.geometry)
            else {
                return Err(Error::invalid(
                    "forge-mode-mismatch",
                    "Forge exact residual cannot change geometry modes.",
                ));
            };
            if components.len() != target_components.len()
                || base_components.len() != target_components.len()
            {
                return Err(Error::invalid(
                    "forge-component-mismatch",
                    "Forge exact residual cannot change component count.",
                ));
            }
            for index in 0..components.len() {
                let base: BTreeSet<u16> = base_components[index].solid.iter().copied().collect();
                let target: BTreeSet<u16> =
                    target_components[index].solid.iter().copied().collect();
                let cells = base.union(&target).copied().collect::<BTreeSet<_>>();
                components[index].patches = cells
                    .into_iter()
                    .filter_map(
                        |cell| match (base.contains(&cell), target.contains(&cell)) {
                            (false, true) => Some(ForgeSolidPatch {
                                cell,
                                kind: ForgePatchKind::Add,
                            }),
                            (true, false) => Some(ForgeSolidPatch {
                                cell,
                                kind: ForgePatchKind::Clear,
                            }),
                            _ => None,
                        },
                    )
                    .collect();
            }
        }
        _ => {
            return Err(Error::invalid(
                "search-profile-mismatch",
                "Candidate program profile does not match target semantics.",
            ))
        }
    }
    Ok(program)
}

fn decode_program_semantics(
    program: &CandidateProgram,
    profile: pouw_core::Profile,
    limits: &LimitsV1,
) -> Result<Semantics> {
    let bytes = encode_candidate(program, limits)?;
    Ok(decode_candidate(&bytes, profile, limits)?.semantics)
}

fn terrain_programs(target: &pouw_core::TerrainSemantics) -> Vec<CandidateProgram> {
    let values = target
        .deleted
        .iter()
        .copied()
        .map(terrain_coord_id)
        .collect::<Vec<_>>();
    let mut output = vec![CandidateProgram::TerrainDelta(TerrainProgram {
        min_y: target.min_y,
        ops: vec![],
        patches: vec![],
    })];
    if !values.is_empty() {
        output.push(CandidateProgram::TerrainDelta(TerrainProgram {
            min_y: target.min_y,
            ops: vec![TerrainOp::EliasFano {
                values: values.clone(),
            }],
            patches: vec![],
        }));
        for minimum_run in [1_u32, 2, 3, 4] {
            let mut ops = Vec::new();
            let mut index = 0;
            while index < values.len() {
                let start = values[index];
                let mut end = index + 1;
                while end < values.len() && values[end] == values[end - 1] + 1 {
                    end += 1;
                }
                let length = (end - index) as u32;
                if length >= minimum_run {
                    ops.push(TerrainOp::DeleteRun { start, length });
                }
                index = end;
            }
            output.push(CandidateProgram::TerrainDelta(TerrainProgram {
                min_y: target.min_y,
                ops,
                patches: vec![],
            }));
        }
        let by_layer =
            target
                .deleted
                .iter()
                .fold(BTreeMap::<u16, [u8; 32]>::new(), |mut map, coord| {
                    let bitmap = map.entry(coord.y).or_insert([0; 32]);
                    let index = usize::from(coord.x) + 16 * usize::from(coord.z);
                    bitmap[index / 8] |= 1 << (7 - index % 8);
                    map
                });
        for threshold in [1_u32, 32, 64, 128] {
            let ops = by_layer
                .iter()
                .filter(|(_, bitmap)| {
                    bitmap.iter().map(|byte| byte.count_ones()).sum::<u32>() >= threshold
                })
                .map(|(y, bitmap)| TerrainOp::LayerBitmap {
                    y: *y,
                    bitmap: *bitmap,
                })
                .collect();
            output.push(CandidateProgram::TerrainDelta(TerrainProgram {
                min_y: target.min_y,
                ops,
                patches: vec![],
            }));
        }
        let min = target
            .deleted
            .iter()
            .fold([u16::MAX; 3], |mut result, coord| {
                result[0] = result[0].min(coord.x);
                result[1] = result[1].min(coord.y);
                result[2] = result[2].min(coord.z);
                result
            });
        let max = target.deleted.iter().fold([0_u16; 3], |mut result, coord| {
            result[0] = result[0].max(coord.x);
            result[1] = result[1].max(coord.y);
            result[2] = result[2].max(coord.z);
            result
        });
        output.push(CandidateProgram::TerrainDelta(TerrainProgram {
            min_y: target.min_y,
            ops: vec![TerrainOp::DeleteBox {
                x: min[0],
                y: min[1],
                z: min[2],
                width: max[0] - min[0] + 1,
                height: max[1] - min[1] + 1,
                depth: max[2] - min[2] + 1,
            }],
            patches: vec![],
        }));
        output.push(CandidateProgram::TerrainDelta(TerrainProgram {
            min_y: target.min_y,
            ops: greedy_terrain_boxes(target),
            patches: vec![],
        }));
    }
    output
}

fn greedy_terrain_boxes(target: &pouw_core::TerrainSemantics) -> Vec<TerrainOp> {
    let target_set: BTreeSet<(u16, u16, u16)> = target
        .deleted
        .iter()
        .map(|coord| (coord.x, coord.y, coord.z))
        .collect();
    let mut covered = BTreeSet::new();
    let mut output = Vec::new();
    for coord in &target.deleted {
        let start = (coord.x, coord.y, coord.z);
        if covered.contains(&start) {
            continue;
        }
        let mut width = 1_u16;
        while coord.x + width < 16
            && target_set.contains(&(coord.x + width, coord.y, coord.z))
            && !covered.contains(&(coord.x + width, coord.y, coord.z))
        {
            width += 1;
        }
        let mut depth = 1_u16;
        'depth: while coord.z + depth < 16 {
            for x in coord.x..coord.x + width {
                let value = (x, coord.y, coord.z + depth);
                if !target_set.contains(&value) || covered.contains(&value) {
                    break 'depth;
                }
            }
            depth += 1;
        }
        let mut height = 1_u16;
        'height: while coord.y + height < 512 {
            for z in coord.z..coord.z + depth {
                for x in coord.x..coord.x + width {
                    let value = (x, coord.y + height, z);
                    if !target_set.contains(&value) || covered.contains(&value) {
                        break 'height;
                    }
                }
            }
            height += 1;
        }
        for y in coord.y..coord.y + height {
            for z in coord.z..coord.z + depth {
                for x in coord.x..coord.x + width {
                    covered.insert((x, y, z));
                }
            }
        }
        output.push(TerrainOp::DeleteBox {
            x: coord.x,
            y: coord.y,
            z: coord.z,
            width,
            height,
            depth,
        });
    }
    output
}

fn building_programs(target: &pouw_core::BuildingSemantics) -> Vec<CandidateProgram> {
    let mut output = vec![CandidateProgram::Building(BuildingProgram {
        size: target.size,
        ops: vec![],
        patches: vec![],
    })];
    if target.voxels.is_empty() {
        return output;
    }
    output.push(CandidateProgram::Building(BuildingProgram {
        size: target.size,
        ops: vec![BuildingOp::Literal {
            voxels: target.voxels.clone(),
        }],
        patches: vec![],
    }));
    output.push(CandidateProgram::Building(BuildingProgram {
        size: target.size,
        ops: greedy_building_boxes(target),
        patches: vec![],
    }));
    for axis in 0..3_u8 {
        output.push(CandidateProgram::Building(BuildingProgram {
            size: target.size,
            ops: greedy_building_runs(target, axis),
            patches: vec![],
        }));
    }
    let mut by_material = BTreeMap::<u16, Vec<&Voxel>>::new();
    for voxel in &target.voxels {
        by_material.entry(voxel.material).or_default().push(voxel);
    }
    let mut bounding = Vec::new();
    for (material, voxels) in &by_material {
        let min = voxels.iter().fold([u16::MAX; 3], |mut value, voxel| {
            value[0] = value[0].min(voxel.x);
            value[1] = value[1].min(voxel.y);
            value[2] = value[2].min(voxel.z);
            value
        });
        let max = voxels.iter().fold([0_u16; 3], |mut value, voxel| {
            value[0] = value[0].max(voxel.x);
            value[1] = value[1].max(voxel.y);
            value[2] = value[2].max(voxel.z);
            value
        });
        bounding.push(BuildingOp::Box {
            material: *material,
            origin: min,
            size: [
                max[0] - min[0] + 1,
                max[1] - min[1] + 1,
                max[2] - min[2] + 1,
            ],
        });
    }
    output.push(CandidateProgram::Building(BuildingProgram {
        size: target.size,
        ops: bounding,
        patches: vec![],
    }));
    for axis in 0..3_u8 {
        let tangent = tangent_axes(usize::from(axis));
        let mut ops = Vec::new();
        for material in by_material.keys() {
            let mut mask = vec![
                false;
                usize::from(target.size[tangent[0]])
                    * usize::from(target.size[tangent[1]])
            ];
            for voxel in &target.voxels {
                if voxel.material != *material
                    || [voxel.x, voxel.y, voxel.z][usize::from(axis)] != 0
                {
                    continue;
                }
                let coordinate = [voxel.x, voxel.y, voxel.z];
                mask[usize::from(coordinate[tangent[0]])
                    + usize::from(target.size[tangent[0]]) * usize::from(coordinate[tangent[1]])] =
                    true;
            }
            if mask.iter().any(|value| *value) {
                ops.push(BuildingOp::Extrude {
                    material: *material,
                    origin: [0, 0, 0],
                    axis,
                    u_length: target.size[tangent[0]],
                    v_length: target.size[tangent[1]],
                    depth: target.size[usize::from(axis)],
                    mask,
                });
            }
        }
        if !ops.is_empty() {
            output.push(CandidateProgram::Building(BuildingProgram {
                size: target.size,
                ops,
                patches: vec![],
            }));
        }
    }
    output
}

fn greedy_building_boxes(target: &pouw_core::BuildingSemantics) -> Vec<BuildingOp> {
    let map = target
        .voxels
        .iter()
        .map(|voxel| ((voxel.x, voxel.y, voxel.z), voxel.material))
        .collect::<BTreeMap<_, _>>();
    let mut covered = BTreeSet::new();
    let mut output = Vec::new();
    for voxel in &target.voxels {
        let start = (voxel.x, voxel.y, voxel.z);
        if covered.contains(&start) {
            continue;
        }
        let material = voxel.material;
        let mut width = 1_u16;
        while voxel.x + width < target.size[0]
            && map.get(&(voxel.x + width, voxel.y, voxel.z)) == Some(&material)
            && !covered.contains(&(voxel.x + width, voxel.y, voxel.z))
        {
            width += 1;
        }
        let mut depth = 1_u16;
        'depth: while voxel.z + depth < target.size[2] {
            for x in voxel.x..voxel.x + width {
                let key = (x, voxel.y, voxel.z + depth);
                if map.get(&key) != Some(&material) || covered.contains(&key) {
                    break 'depth;
                }
            }
            depth += 1;
        }
        let mut height = 1_u16;
        'height: while voxel.y + height < target.size[1] {
            for z in voxel.z..voxel.z + depth {
                for x in voxel.x..voxel.x + width {
                    let key = (x, voxel.y + height, z);
                    if map.get(&key) != Some(&material) || covered.contains(&key) {
                        break 'height;
                    }
                }
            }
            height += 1;
        }
        for y in voxel.y..voxel.y + height {
            for z in voxel.z..voxel.z + depth {
                for x in voxel.x..voxel.x + width {
                    covered.insert((x, y, z));
                }
            }
        }
        output.push(BuildingOp::Box {
            material,
            origin: [voxel.x, voxel.y, voxel.z],
            size: [width, height, depth],
        });
    }
    output
}

fn greedy_building_runs(target: &pouw_core::BuildingSemantics, axis: u8) -> Vec<BuildingOp> {
    let map = target
        .voxels
        .iter()
        .map(|voxel| ((voxel.x, voxel.y, voxel.z), voxel.material))
        .collect::<BTreeMap<_, _>>();
    let mut covered = BTreeSet::new();
    let mut output = Vec::new();
    for voxel in &target.voxels {
        let origin = [voxel.x, voxel.y, voxel.z];
        let key = (origin[0], origin[1], origin[2]);
        if covered.contains(&key) {
            continue;
        }
        let mut length = 1_u16;
        loop {
            let mut next = origin;
            let Some(value) = next[usize::from(axis)].checked_add(length) else {
                break;
            };
            if value >= target.size[usize::from(axis)] {
                break;
            }
            next[usize::from(axis)] = value;
            let key = (next[0], next[1], next[2]);
            if map.get(&key) != Some(&voxel.material) || covered.contains(&key) {
                break;
            }
            length += 1;
        }
        if length >= 2 {
            for index in 0..length {
                let mut coordinate = origin;
                coordinate[usize::from(axis)] += index;
                covered.insert((coordinate[0], coordinate[1], coordinate[2]));
            }
            output.push(BuildingOp::Run {
                material: voxel.material,
                origin,
                axis,
                length,
            });
        }
    }
    output
}

fn forged_programs(target: &ForgedSemantics) -> Vec<CandidateProgram> {
    let ForgeGeometry::Components { components } = &target.geometry else {
        let ForgeGeometry::Appearance { appearance } = &target.geometry else {
            unreachable!()
        };
        return vec![CandidateProgram::ForgedItem(ForgeProgram {
            equipment: target.equipment.clone(),
            geometry: ForgeProgramGeometry::Appearance {
                dimensions_q: appearance.dimensions_q,
                grip: appearance.grip,
                quads: appearance.quads.clone(),
            },
        })];
    };
    let strategies: [fn(&pouw_core::ForgeComponent) -> Vec<ForgeSolidOp>; 6] = [
        forge_sparse,
        forge_rle,
        forge_solid,
        forge_extrude,
        forge_symmetry,
        forge_cut_boxes,
    ];
    strategies
        .into_iter()
        .map(|strategy| {
            CandidateProgram::ForgedItem(ForgeProgram {
                equipment: target.equipment.clone(),
                geometry: ForgeProgramGeometry::Components {
                    components: components
                        .iter()
                        .map(|component| ForgeComponentProgram {
                            resource: component.resource,
                            color_444: component.color_444,
                            dimensions_q: component.dimensions_q,
                            offset_q: component.offset_q,
                            grip: component.grip,
                            ops: strategy(component),
                            patches: vec![],
                            paint: component.paint.clone(),
                        })
                        .collect(),
                },
            })
        })
        .collect()
}

fn forge_sparse(component: &pouw_core::ForgeComponent) -> Vec<ForgeSolidOp> {
    vec![ForgeSolidOp::Sparse {
        cells: component.solid.clone(),
    }]
}

fn forge_rle(component: &pouw_core::ForgeComponent) -> Vec<ForgeSolidOp> {
    let solid: BTreeSet<u16> = component.solid.iter().copied().collect();
    vec![ForgeSolidOp::Rle {
        occupancy: (0..FORGE_CELL_COUNT)
            .map(|cell| solid.contains(&cell))
            .collect(),
    }]
}

fn forge_solid(_component: &pouw_core::ForgeComponent) -> Vec<ForgeSolidOp> {
    vec![ForgeSolidOp::Solid]
}

fn forge_extrude(component: &pouw_core::ForgeComponent) -> Vec<ForgeSolidOp> {
    let solid: BTreeSet<u16> = component.solid.iter().copied().collect();
    let axis = (0..3_usize)
        .min_by_key(|axis| {
            let tangent = tangent_axes(*axis);
            forge_sizes()[tangent[0]] * forge_sizes()[tangent[1]]
        })
        .unwrap_or(1);
    let sizes = forge_sizes();
    let tangent = tangent_axes(axis);
    let mut mask = vec![false; sizes[tangent[0]] * sizes[tangent[1]]];
    for v in 0..sizes[tangent[1]] {
        for u in 0..sizes[tangent[0]] {
            let mut cell = [0_u8; 3];
            cell[tangent[0]] = u as u8;
            cell[tangent[1]] = v as u8;
            mask[u + sizes[tangent[0]] * v] =
                solid.contains(&forge_cell_id(cell[0], cell[1], cell[2]));
        }
    }
    if !mask.iter().any(|value| *value) {
        return forge_sparse(component);
    }
    vec![ForgeSolidOp::Extrude {
        axis: axis as u8,
        mask,
    }]
}

fn forge_symmetry(component: &pouw_core::ForgeComponent) -> Vec<ForgeSolidOp> {
    let sizes = forge_sizes();
    let axis = 0_usize;
    let cells = component
        .solid
        .iter()
        .copied()
        .filter(|cell| {
            forge_cell_from_id(*cell)
                .map(|coordinate| usize::from(coordinate[axis]) * 2 < sizes[axis])
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    if cells.is_empty() {
        return forge_sparse(component);
    }
    vec![
        ForgeSolidOp::Sparse { cells },
        ForgeSolidOp::Symmetry { axis: axis as u8 },
    ]
}

fn forge_cut_boxes(component: &pouw_core::ForgeComponent) -> Vec<ForgeSolidOp> {
    let occupied: BTreeSet<u16> = component.solid.iter().copied().collect();
    let mut covered = BTreeSet::new();
    let mut ops = vec![ForgeSolidOp::Solid];
    for z in 0..FORGE_GRID_Z {
        for y in 0..FORGE_GRID_Y {
            for x in 0..FORGE_GRID_X {
                let cell = forge_cell_id(x, y, z);
                if occupied.contains(&cell) || covered.contains(&cell) || ops.len() >= 32 {
                    continue;
                }
                let mut sx = 1_u8;
                while x + sx < FORGE_GRID_X {
                    let next = forge_cell_id(x + sx, y, z);
                    if occupied.contains(&next) || covered.contains(&next) {
                        break;
                    }
                    sx += 1;
                }
                let mut sy = 1_u8;
                'grow_y: while y + sy < FORGE_GRID_Y {
                    for cx in x..x + sx {
                        let next = forge_cell_id(cx, y + sy, z);
                        if occupied.contains(&next) || covered.contains(&next) {
                            break 'grow_y;
                        }
                    }
                    sy += 1;
                }
                let mut sz = 1_u8;
                'grow_z: while z + sz < FORGE_GRID_Z {
                    for cy in y..y + sy {
                        for cx in x..x + sx {
                            let next = forge_cell_id(cx, cy, z + sz);
                            if occupied.contains(&next) || covered.contains(&next) {
                                break 'grow_z;
                            }
                        }
                    }
                    sz += 1;
                }
                for cz in z..z + sz {
                    for cy in y..y + sy {
                        for cx in x..x + sx {
                            covered.insert(forge_cell_id(cx, cy, cz));
                        }
                    }
                }
                ops.push(ForgeSolidOp::CutBox {
                    origin: [x, y, z],
                    size: [sx, sy, sz],
                });
            }
        }
    }
    ops
}

fn tangent_axes(axis: usize) -> [usize; 2] {
    match axis {
        0 => [1, 2],
        1 => [0, 2],
        2 => [0, 1],
        _ => unreachable!(),
    }
}

fn forge_sizes() -> [usize; 3] {
    [
        usize::from(FORGE_GRID_X),
        usize::from(FORGE_GRID_Y),
        usize::from(FORGE_GRID_Z),
    ]
}

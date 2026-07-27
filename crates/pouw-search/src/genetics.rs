use std::collections::BTreeSet;

use pouw_core::{
    forge_cell_from_id, forge_cell_id, terrain_coord_id, BuildingOp, CandidateProgram,
    ForgeGeometry, ForgeProgramGeometry, ForgeSolidOp, LimitsV1, Result, Semantics, TerrainOp,
    FORGE_CELL_COUNT, FORGE_GRID_X, FORGE_GRID_Y, FORGE_GRID_Z,
};
use rand_chacha::ChaCha8Rng;
use rand_core::{RngCore, SeedableRng};

use crate::{evaluate_program, IslandState, SearchCandidate, SearchConfig};

pub(crate) fn evolve_epoch(
    island: &mut IslandState,
    target: &Semantics,
    limits: &LimitsV1,
    config: &SearchConfig,
    generations: u32,
) -> Result<()> {
    for _ in 0..generations {
        island.population.sort_by(SearchCandidate::fitness_cmp);
        let mut next = island
            .population
            .iter()
            .take(usize::from(config.elite_count))
            .cloned()
            .collect::<Vec<_>>();
        let mut rng = generation_rng(config.seed, island.index, island.generation);
        while next.len() < config.population as usize {
            let first = tournament(&island.population, &mut rng, config.tournament_size).clone();
            let mut program = if rng.next_u32() % 100 < 45 {
                let second = tournament(&island.population, &mut rng, config.tournament_size);
                crossover(&first.program, &second.program, &mut rng)
            } else {
                first.program.clone()
            };
            mutate(&mut program, target, &mut rng);
            local_optimize(&mut program);
            island.attempts = island.attempts.saturating_add(1);
            match evaluate_program(program, target, limits) {
                Ok(candidate) => next.push(candidate),
                Err(_) => next.push(first),
            }
        }
        next.sort_by(SearchCandidate::fitness_cmp);
        island.population = next;
        island.generation = island.generation.saturating_add(1);
    }
    Ok(())
}

fn tournament<'a>(
    population: &'a [SearchCandidate],
    rng: &mut ChaCha8Rng,
    size: u8,
) -> &'a SearchCandidate {
    let mut best = random_index(rng, population.len());
    for _ in 1..size {
        let candidate = random_index(rng, population.len());
        if population[candidate].fitness_cmp(&population[best]).is_lt() {
            best = candidate;
        }
    }
    &population[best]
}

fn crossover(
    left: &CandidateProgram,
    right: &CandidateProgram,
    rng: &mut ChaCha8Rng,
) -> CandidateProgram {
    match (left, right) {
        (CandidateProgram::TerrainDelta(left), CandidateProgram::TerrainDelta(right)) => {
            let left_cut = random_cut(rng, left.ops.len());
            let right_cut = random_cut(rng, right.ops.len());
            let mut child = left.clone();
            child.ops = left.ops[..left_cut]
                .iter()
                .cloned()
                .chain(right.ops[right_cut..].iter().cloned())
                .collect();
            child.patches.clear();
            CandidateProgram::TerrainDelta(child)
        }
        (CandidateProgram::Building(left), CandidateProgram::Building(right)) => {
            let threshold = if left.size[0] == 0 {
                0
            } else {
                (rng.next_u32() % u32::from(left.size[0])) as u16
            };
            let mut child = left.clone();
            child.ops = left
                .ops
                .iter()
                .filter(|op| op_origin_x(op).is_none_or(|x| x <= threshold))
                .cloned()
                .chain(
                    right
                        .ops
                        .iter()
                        .filter(|op| op_origin_x(op).is_none_or(|x| x > threshold))
                        .cloned(),
                )
                .collect();
            child.patches.clear();
            CandidateProgram::Building(child)
        }
        (CandidateProgram::ForgedItem(left), CandidateProgram::ForgedItem(right)) => {
            let mut child = left.clone();
            if let (
                ForgeProgramGeometry::Components {
                    components: child_components,
                },
                ForgeProgramGeometry::Components {
                    components: right_components,
                },
            ) = (&mut child.geometry, &right.geometry)
            {
                for (index, component) in child_components.iter_mut().enumerate() {
                    if let Some(right) = right_components.get(index) {
                        if rng.next_u32() & 1 == 1 {
                            component.ops = right.ops.clone();
                        } else {
                            let left_cut = random_cut(rng, component.ops.len());
                            let right_cut = random_cut(rng, right.ops.len());
                            component.ops = component.ops[..left_cut]
                                .iter()
                                .cloned()
                                .chain(right.ops[right_cut..].iter().cloned())
                                .collect();
                        }
                        component.patches.clear();
                    }
                }
            }
            CandidateProgram::ForgedItem(child)
        }
        _ => left.clone(),
    }
}

fn mutate(program: &mut CandidateProgram, target: &Semantics, rng: &mut ChaCha8Rng) {
    match (program, target) {
        (CandidateProgram::TerrainDelta(program), Semantics::TerrainDelta(target)) => {
            mutate_terrain(program, target, rng)
        }
        (CandidateProgram::Building(program), Semantics::Building(target)) => {
            mutate_building(program, target, rng)
        }
        (CandidateProgram::ForgedItem(program), Semantics::ForgedItem(target)) => {
            mutate_forge(program, target, rng)
        }
        _ => {}
    }
}

fn mutate_terrain(
    program: &mut pouw_core::TerrainProgram,
    target: &pouw_core::TerrainSemantics,
    rng: &mut ChaCha8Rng,
) {
    if target.deleted.is_empty() {
        program.ops.clear();
        return;
    }
    match rng.next_u32() % 6 {
        0 if !program.ops.is_empty() => {
            let index = random_index(rng, program.ops.len());
            program.ops.remove(index);
        }
        1 => {
            let coord = target.deleted[random_index(rng, target.deleted.len())];
            let start = terrain_coord_id(coord);
            let maximum = (1 + rng.next_u32() % 32).min(pouw_core::TERRAIN_UNIVERSE - start);
            program.ops.push(TerrainOp::DeleteRun {
                start,
                length: maximum.max(1),
            });
        }
        2 => {
            let coord = target.deleted[random_index(rng, target.deleted.len())];
            let width = (1 + (rng.next_u32() % 4) as u16).min(16 - coord.x);
            let height = (1 + (rng.next_u32() % 8) as u16).min(512 - coord.y);
            let depth = (1 + (rng.next_u32() % 4) as u16).min(16 - coord.z);
            program.ops.push(TerrainOp::DeleteBox {
                x: coord.x,
                y: coord.y,
                z: coord.z,
                width,
                height,
                depth,
            });
        }
        3 => {
            let coord = target.deleted[random_index(rng, target.deleted.len())];
            let mut bitmap = [0_u8; 32];
            for item in target.deleted.iter().filter(|value| value.y == coord.y) {
                let index = usize::from(item.x) + 16 * usize::from(item.z);
                bitmap[index / 8] |= 1 << (7 - index % 8);
            }
            program
                .ops
                .push(TerrainOp::LayerBitmap { y: coord.y, bitmap });
        }
        4 => {
            program
                .ops
                .retain(|op| !matches!(op, TerrainOp::EliasFano { .. }));
            program.ops.push(TerrainOp::EliasFano {
                values: target
                    .deleted
                    .iter()
                    .copied()
                    .map(terrain_coord_id)
                    .collect(),
            });
        }
        _ => {
            if program.ops.len() >= 2 {
                let left = random_index(rng, program.ops.len());
                let right = random_index(rng, program.ops.len());
                program.ops.swap(left, right);
            }
        }
    }
    program.patches.clear();
}

fn mutate_building(
    program: &mut pouw_core::BuildingProgram,
    target: &pouw_core::BuildingSemantics,
    rng: &mut ChaCha8Rng,
) {
    if target.voxels.is_empty() {
        program.ops.clear();
        return;
    }
    let voxel = target.voxels[random_index(rng, target.voxels.len())];
    let origin = [voxel.x, voxel.y, voxel.z];
    match rng.next_u32() % 8 {
        0 if !program.ops.is_empty() => {
            let index = random_index(rng, program.ops.len());
            program.ops.remove(index);
        }
        1 => {
            let size = [
                (1 + rng.next_u32() as u16 % 8).min(target.size[0] - voxel.x),
                (1 + rng.next_u32() as u16 % 8).min(target.size[1] - voxel.y),
                (1 + rng.next_u32() as u16 % 8).min(target.size[2] - voxel.z),
            ];
            program.ops.push(BuildingOp::Box {
                material: voxel.material,
                origin,
                size,
            });
        }
        2 => {
            let axis = (rng.next_u32() % 3) as u8;
            let length = (1 + rng.next_u32() as u16 % 16)
                .min(target.size[usize::from(axis)] - origin[usize::from(axis)]);
            program.ops.push(BuildingOp::Run {
                material: voxel.material,
                origin,
                axis,
                length,
            });
        }
        3 => {
            let size = [
                (1 + rng.next_u32() as u16 % 4).min(target.size[0] - voxel.x),
                (1 + rng.next_u32() as u16 % 4).min(target.size[1] - voxel.y),
                (1 + rng.next_u32() as u16 % 4).min(target.size[2] - voxel.z),
            ];
            program.ops.push(BuildingOp::Cut { origin, size });
        }
        4 => {
            program.ops.push(BuildingOp::Literal {
                voxels: vec![voxel],
            });
        }
        5 => {
            if let Some(BuildingOp::Box { origin, size, .. }) = program
                .ops
                .iter_mut()
                .find(|op| matches!(op, BuildingOp::Box { .. }))
            {
                let axis = random_index(rng, 3);
                if size[axis] > 1 && rng.next_u32() & 1 == 0 {
                    size[axis] -= 1;
                } else if origin[axis] + size[axis] < target.size[axis] {
                    size[axis] += 1;
                }
            }
        }
        6 => {
            let axis = (rng.next_u32() % 3) as u8;
            let tangent = tangent_axes(usize::from(axis));
            program.ops.push(BuildingOp::Wall {
                material: voxel.material,
                origin,
                normal_axis: axis,
                u_length: 1.min(target.size[tangent[0]] - origin[tangent[0]]),
                v_length: 1.min(target.size[tangent[1]] - origin[tangent[1]]),
                thickness: 1,
            });
        }
        _ if program.ops.len() >= 2 => {
            let left = random_index(rng, program.ops.len());
            let right = random_index(rng, program.ops.len());
            program.ops.swap(left, right);
        }
        _ => {}
    }
    program.patches.clear();
}

fn mutate_forge(
    program: &mut pouw_core::ForgeProgram,
    target: &pouw_core::ForgedSemantics,
    rng: &mut ChaCha8Rng,
) {
    let (
        ForgeProgramGeometry::Components { components },
        ForgeGeometry::Components {
            components: target_components,
        },
    ) = (&mut program.geometry, &target.geometry)
    else {
        return;
    };
    if components.is_empty() {
        return;
    }
    let index = random_index(rng, components.len());
    let component = &mut components[index];
    let target = &target_components[index];
    match rng.next_u32() % 7 {
        0 => component.ops = vec![ForgeSolidOp::Solid],
        1 => {
            let occupied: BTreeSet<u16> = target.solid.iter().copied().collect();
            component.ops = vec![ForgeSolidOp::Rle {
                occupancy: (0..FORGE_CELL_COUNT)
                    .map(|cell| occupied.contains(&cell))
                    .collect(),
            }];
        }
        2 => {
            component.ops = vec![ForgeSolidOp::Sparse {
                cells: target.solid.clone(),
            }]
        }
        3 => {
            let axis = (rng.next_u32() % 3) as usize;
            let sizes = forge_sizes();
            let tangent = tangent_axes(axis);
            let occupied: BTreeSet<u16> = target.solid.iter().copied().collect();
            let mut mask = vec![false; sizes[tangent[0]] * sizes[tangent[1]]];
            for v in 0..sizes[tangent[1]] {
                for u in 0..sizes[tangent[0]] {
                    let mut cell = [0_u8; 3];
                    cell[tangent[0]] = u as u8;
                    cell[tangent[1]] = v as u8;
                    mask[u + sizes[tangent[0]] * v] =
                        occupied.contains(&forge_cell_id(cell[0], cell[1], cell[2]));
                }
            }
            if mask.iter().any(|value| *value) {
                component.ops = vec![ForgeSolidOp::Extrude {
                    axis: axis as u8,
                    mask,
                }];
            }
        }
        4 => {
            let axis = (rng.next_u32() % 3) as usize;
            let sizes = forge_sizes();
            let cells = target
                .solid
                .iter()
                .copied()
                .filter(|cell| {
                    forge_cell_from_id(*cell)
                        .map(|coordinate| usize::from(coordinate[axis]) * 2 < sizes[axis])
                        .unwrap_or(false)
                })
                .collect::<Vec<_>>();
            if !cells.is_empty() {
                component.ops = vec![
                    ForgeSolidOp::Sparse { cells },
                    ForgeSolidOp::Symmetry { axis: axis as u8 },
                ];
            }
        }
        5 if !component.ops.is_empty() => {
            let op = random_index(rng, component.ops.len());
            component.ops.remove(op);
            if component.ops.is_empty() {
                component.ops.push(ForgeSolidOp::Sparse {
                    cells: target.solid.clone(),
                });
            }
        }
        _ => {
            let cell = target.solid[random_index(rng, target.solid.len())];
            let coordinate = forge_cell_from_id(cell).unwrap_or([0, 0, 0]);
            component.ops.push(ForgeSolidOp::CutBox {
                origin: coordinate,
                size: [1, 1, 1],
            });
        }
    }
    component.patches.clear();
}

fn local_optimize(program: &mut CandidateProgram) {
    match program {
        CandidateProgram::TerrainDelta(program) => {
            let mut output = Vec::new();
            for op in program.ops.drain(..) {
                if let (
                    Some(TerrainOp::DeleteRun {
                        start: previous_start,
                        length: previous_length,
                    }),
                    TerrainOp::DeleteRun { start, length },
                ) = (output.last_mut(), &op)
                {
                    if previous_start.saturating_add(*previous_length) == *start {
                        *previous_length = previous_length.saturating_add(*length);
                        continue;
                    }
                }
                if !output.contains(&op) {
                    output.push(op);
                }
            }
            program.ops = output;
        }
        CandidateProgram::Building(program) => {
            let mut changed = true;
            while changed {
                changed = false;
                'outer: for left in 0..program.ops.len() {
                    for right in left + 1..program.ops.len() {
                        if let Some(merged) = merge_boxes(&program.ops[left], &program.ops[right]) {
                            program.ops[left] = merged;
                            program.ops.remove(right);
                            changed = true;
                            break 'outer;
                        }
                    }
                }
            }
            let mut unique = Vec::new();
            for op in program.ops.drain(..) {
                if !unique.contains(&op) {
                    unique.push(op);
                }
            }
            program.ops = unique;
        }
        CandidateProgram::ForgedItem(program) => {
            if let ForgeProgramGeometry::Components { components } = &mut program.geometry {
                for component in components {
                    let mut unique = Vec::new();
                    for op in component.ops.drain(..) {
                        if !unique.contains(&op) {
                            unique.push(op);
                        }
                    }
                    component.ops = unique;
                }
            }
        }
    }
}

fn merge_boxes(left: &BuildingOp, right: &BuildingOp) -> Option<BuildingOp> {
    let (
        BuildingOp::Box {
            material: left_material,
            origin: left_origin,
            size: left_size,
        },
        BuildingOp::Box {
            material: right_material,
            origin: right_origin,
            size: right_size,
        },
    ) = (left, right)
    else {
        return None;
    };
    if left_material != right_material {
        return None;
    }
    for axis in 0..3 {
        let tangent = tangent_axes(axis);
        if left_origin[tangent[0]] == right_origin[tangent[0]]
            && left_origin[tangent[1]] == right_origin[tangent[1]]
            && left_size[tangent[0]] == right_size[tangent[0]]
            && left_size[tangent[1]] == right_size[tangent[1]]
        {
            if left_origin[axis] + left_size[axis] == right_origin[axis] {
                let mut size = *left_size;
                size[axis] = size[axis].checked_add(right_size[axis])?;
                return Some(BuildingOp::Box {
                    material: *left_material,
                    origin: *left_origin,
                    size,
                });
            }
            if right_origin[axis] + right_size[axis] == left_origin[axis] {
                let mut size = *right_size;
                size[axis] = size[axis].checked_add(left_size[axis])?;
                return Some(BuildingOp::Box {
                    material: *left_material,
                    origin: *right_origin,
                    size,
                });
            }
        }
    }
    None
}

fn op_origin_x(op: &BuildingOp) -> Option<u16> {
    match op {
        BuildingOp::Box { origin, .. }
        | BuildingOp::Run { origin, .. }
        | BuildingOp::Wall { origin, .. }
        | BuildingOp::Extrude { origin, .. }
        | BuildingOp::Repeat { origin, .. }
        | BuildingOp::Cut { origin, .. } => Some(origin[0]),
        BuildingOp::Mirror { source_origin, .. } => Some(source_origin[0]),
        BuildingOp::Literal { voxels } => voxels.first().map(|voxel| voxel.x),
    }
}

fn generation_rng(seed: u64, island: u16, generation: u32) -> ChaCha8Rng {
    let mut state = seed
        ^ u64::from(island).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ u64::from(generation).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    let mut bytes = [0_u8; 32];
    for chunk in bytes.chunks_exact_mut(8) {
        state = splitmix64(state);
        chunk.copy_from_slice(&state.to_le_bytes());
    }
    ChaCha8Rng::from_seed(bytes)
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn random_index(rng: &mut ChaCha8Rng, length: usize) -> usize {
    if length <= 1 {
        0
    } else {
        (rng.next_u64() % length as u64) as usize
    }
}

fn random_cut(rng: &mut ChaCha8Rng, length: usize) -> usize {
    if length == 0 {
        0
    } else {
        random_index(rng, length + 1)
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

fn forge_sizes() -> [usize; 3] {
    [
        usize::from(FORGE_GRID_X),
        usize::from(FORGE_GRID_Y),
        usize::from(FORGE_GRID_Z),
    ]
}

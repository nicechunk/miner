#include <stdint.h>

struct PackedOp {
  uint32_t kind;
  int32_t parameters[20];
};

struct PackedScore {
  uint32_t valid;
  uint32_t mismatches;
  uint32_t set_patches;
  uint32_t clear_patches;
  uint32_t paint_patches;
  uint32_t patch_runs;
  uint32_t writes;
  uint32_t reserved;
};

static __device__ __forceinline__ uint32_t coordinate_id(
    int32_t x, int32_t y, int32_t z, uint32_t size_x, uint32_t size_z) {
  return static_cast<uint32_t>(x) + size_x *
      (static_cast<uint32_t>(z) + size_z * static_cast<uint32_t>(y));
}

static __device__ __forceinline__ void coordinate_from_id(
    uint32_t id, uint32_t size_x, uint32_t size_z,
    int32_t *x, int32_t *y, int32_t *z) {
  *x = static_cast<int32_t>(id % size_x);
  const uint32_t rest = id / size_x;
  *z = static_cast<int32_t>(rest % size_z);
  *y = static_cast<int32_t>(rest / size_z);
}

static __device__ __forceinline__ bool valid_box(
    int32_t ox, int32_t oy, int32_t oz,
    int32_t sx, int32_t sy, int32_t sz,
    uint32_t size_x, uint32_t size_y, uint32_t size_z) {
  return ox >= 0 && oy >= 0 && oz >= 0 && sx > 0 && sy > 0 && sz > 0 &&
      static_cast<uint64_t>(ox) + static_cast<uint64_t>(sx) <= size_x &&
      static_cast<uint64_t>(oy) + static_cast<uint64_t>(sy) <= size_y &&
      static_cast<uint64_t>(oz) + static_cast<uint64_t>(sz) <= size_z;
}

static __device__ __forceinline__ bool in_box(
    int32_t x, int32_t y, int32_t z,
    int32_t ox, int32_t oy, int32_t oz,
    int32_t sx, int32_t sy, int32_t sz) {
  return x >= ox && x < ox + sx && y >= oy && y < oy + sy &&
      z >= oz && z < oz + sz;
}

static __device__ __forceinline__ bool nonzero_delta(const int32_t *p) {
  return p[9] != 0 || p[10] != 0 || p[11] != 0;
}

static __device__ __forceinline__ void charge(
    uint32_t amount, uint32_t maximum_expansion, uint32_t maximum_writes,
    uint32_t *writes, uint32_t *invalid) {
  if (amount == 0 || amount > maximum_expansion || amount > maximum_writes ||
      *writes > maximum_writes - amount) {
    *invalid = 1;
  } else {
    *writes += amount;
  }
}

static __device__ __forceinline__ bool gable_contains(
    const int32_t *p, int32_t x, int32_t y, int32_t z) {
  const int32_t ox = p[2];
  const int32_t oy = p[3];
  const int32_t oz = p[4];
  const int32_t width = p[5];
  const int32_t depth = p[6];
  const int32_t style = p[8];
  const bool z_oriented = p[9] != 0;
  const int32_t layers = z_oriented ? (depth + 1) / 2 : (width + 1) / 2;
  const int32_t layer = y - oy;
  if (layer < 0 || layer >= layers) return false;
  if (z_oriented) {
    if (x < ox || x >= ox + width) return false;
    const int32_t front = oz + layer;
    const int32_t back = oz + depth - 1 - layer;
    if (style == 2) return z >= front && z <= back;
    if (style == 0) return z == front || z == back;
    return (z == front || z == back) && (x == ox || x == ox + width - 1);
  }
  if (z < oz || z >= oz + depth) return false;
  const int32_t left = ox + layer;
  const int32_t right = ox + width - 1 - layer;
  if (style == 2) return x >= left && x <= right;
  if (style == 0) return x == left || x == right;
  return (x == left || x == right) && (z == oz || z == oz + depth - 1);
}

static __device__ __forceinline__ uint16_t tree_material(
    const int32_t *p, int32_t x, int32_t y, int32_t z) {
  const int32_t ox = p[2];
  const int32_t oy = p[3];
  const int32_t oz = p[4];
  const int32_t height = p[5];
  const int32_t crown = p[6];
  const int32_t trunk_height = (height - crown) > 2 ? height - crown : 2;
  bool leaf = in_box(x, y, z, ox, oy + height - 1, oz, 2, 1, 2);
  for (int32_t layer = 0; layer < crown; ++layer) {
    int32_t radius = crown - layer / 2;
    if (radius < 1) radius = 1;
    const int32_t layer_y = oy + trunk_height - 1 + layer;
    leaf = leaf || in_box(x, y, z, ox - radius, layer_y, oz - 1,
                          radius * 2 + 2, 1, 4);
    leaf = leaf || in_box(x, y, z, ox - 1, layer_y, oz - radius,
                          4, 1, radius * 2 + 2);
  }
  if (leaf) return static_cast<uint16_t>(p[1]);
  if (in_box(x, y, z, ox, oy, oz, 2, trunk_height, 2))
    return static_cast<uint16_t>(p[0]);
  return 0;
}

static __device__ __forceinline__ bool fence_contains(
    const int32_t *p, int32_t x, int32_t y, int32_t z) {
  const int32_t ox = p[2];
  const int32_t oy = p[3];
  const int32_t oz = p[4];
  const int32_t length = p[5];
  const int32_t axis = p[8];
  const int32_t spacing = p[9];
  const int32_t along = axis == 0 ? x - ox : z - oz;
  const bool cross = axis == 0 ? z == oz : x == ox;
  if (!cross || along < 0 || along >= length || y < oy || y >= oy + 5)
    return false;
  const bool rail = y == oy + 1 || y == oy + 3;
  const bool post = along % spacing == 0 || along == length - 1;
  return rail || post;
}

static __device__ __forceinline__ uint32_t mismatch_signature(
    uint16_t before, uint16_t after) {
  if (before == after) return 0;
  if (before == 0) return 0x10000u | static_cast<uint32_t>(after);
  if (after == 0) return 0x20000u;
  return 0x30000u | static_cast<uint32_t>(after);
}

extern "C" __global__ void nicechunk_score_ncm4(
    const PackedOp *operations,
    const uint32_t *operation_offsets,
    const uint8_t *masks,
    uint32_t mask_count,
    const uint16_t *target,
    uint16_t *scenes,
    uint16_t *snapshots,
    uint32_t volume,
    uint32_t size_x,
    uint32_t size_y,
    uint32_t size_z,
    uint32_t candidate_count,
    uint32_t maximum_expansion,
    uint32_t maximum_writes,
    PackedScore *scores) {
  const uint32_t candidate = blockIdx.x;
  if (candidate >= candidate_count) return;
  uint16_t *scene = scenes + static_cast<uint64_t>(candidate) * volume;
  uint16_t *snapshot = snapshots + static_cast<uint64_t>(candidate) * volume;
  PackedScore *score = scores + candidate;
  __shared__ uint32_t invalid;
  __shared__ uint32_t writes;
  __shared__ uint32_t auxiliary;

  if (threadIdx.x == 0) {
    invalid = 0;
    writes = 0;
    auxiliary = 0;
    score->valid = 0;
    score->mismatches = 0;
    score->set_patches = 0;
    score->clear_patches = 0;
    score->paint_patches = 0;
    score->patch_runs = 0;
    score->writes = 0;
    score->reserved = 0;
  }
  for (uint32_t id = threadIdx.x; id < volume; id += blockDim.x) scene[id] = 0;
  __syncthreads();

  const uint32_t first = operation_offsets[candidate];
  const uint32_t last = operation_offsets[candidate + 1];
  for (uint32_t operation_index = first; operation_index < last; ++operation_index) {
    const PackedOp operation = operations[operation_index];
    const int32_t *p = operation.parameters;
    if (threadIdx.x == 0) auxiliary = 0;
    __syncthreads();

    if (operation.kind == 0 || operation.kind == 5 || operation.kind == 6) {
      int32_t sx = p[5], sy = p[6], sz = p[7];
      if (operation.kind == 5) {
        sx = sy = sz = 1;
        if (p[8] == 0) sx = p[5];
        else if (p[8] == 1) sy = p[5];
        else if (p[8] == 2) sz = p[5];
      } else if (operation.kind == 6) {
        sx = sy = sz = 1;
        const int32_t normal = p[8];
        if (normal == 0) { sx = p[7]; sy = p[5]; sz = p[6]; }
        else if (normal == 1) { sy = p[7]; sx = p[5]; sz = p[6]; }
        else if (normal == 2) { sz = p[7]; sx = p[5]; sy = p[6]; }
      }
      if (threadIdx.x == 0) {
        if (p[0] <= 0 || (operation.kind == 5 && (p[8] < 0 || p[8] > 2)) ||
            (operation.kind == 6 && (p[8] < 0 || p[8] > 2)) ||
            !valid_box(p[2], p[3], p[4], sx, sy, sz, size_x, size_y, size_z)) {
          invalid = 1;
        } else {
          charge(static_cast<uint32_t>(sx * sy * sz), maximum_expansion,
                 maximum_writes, &writes, &invalid);
        }
      }
      __syncthreads();
      if (!invalid) {
        for (uint32_t id = threadIdx.x; id < volume; id += blockDim.x) {
          int32_t x, y, z;
          coordinate_from_id(id, size_x, size_z, &x, &y, &z);
          if (in_box(x, y, z, p[2], p[3], p[4], sx, sy, sz))
            scene[id] = static_cast<uint16_t>(p[0]);
        }
      }
    } else if (operation.kind == 1) {
      if (threadIdx.x == 0) {
        if (p[0] <= 0 || p[8] < 2 || p[8] > 512 ||
            p[5] <= 0 || p[6] <= 0 || p[7] <= 0) invalid = 1;
        for (int32_t index = 0; !invalid && index < p[8]; ++index) {
          const int32_t ox = p[2] + p[9] * index;
          const int32_t oy = p[3] + p[10] * index;
          const int32_t oz = p[4] + p[11] * index;
          if (!valid_box(ox, oy, oz, p[5], p[6], p[7], size_x, size_y, size_z))
            invalid = 1;
        }
        const uint64_t amount = static_cast<uint64_t>(p[5]) * p[6] * p[7] * p[8];
        if (!invalid && amount <= UINT32_MAX)
          charge(static_cast<uint32_t>(amount), maximum_expansion,
                 maximum_writes, &writes, &invalid);
        else if (amount > UINT32_MAX) invalid = 1;
      }
      __syncthreads();
      if (!invalid) {
        for (uint32_t id = threadIdx.x; id < volume; id += blockDim.x) {
          int32_t x, y, z;
          coordinate_from_id(id, size_x, size_z, &x, &y, &z);
          bool match = false;
          for (int32_t index = 0; index < p[8] && !match; ++index) {
            match = in_box(x, y, z, p[2] + p[9] * index,
                           p[3] + p[10] * index, p[4] + p[11] * index,
                           p[5], p[6], p[7]);
          }
          if (match) scene[id] = static_cast<uint16_t>(p[0]);
        }
      }
    } else if (operation.kind == 2) {
      if (threadIdx.x == 0) {
        const int32_t layers = p[9] ? (p[6] + 1) / 2 : (p[5] + 1) / 2;
        if (p[0] <= 0 || p[5] <= 0 || p[6] <= 0 || p[8] < 0 || p[8] > 2 ||
            (p[9] != 0 && p[9] != 1) ||
            !valid_box(p[2], p[3], p[4], p[5], layers, p[6],
                       size_x, size_y, size_z)) invalid = 1;
      }
      __syncthreads();
      if (!invalid) {
        for (uint32_t id = threadIdx.x; id < volume; id += blockDim.x) {
          int32_t x, y, z;
          coordinate_from_id(id, size_x, size_z, &x, &y, &z);
          if (gable_contains(p, x, y, z)) scene[id] = static_cast<uint16_t>(p[0]);
        }
      }
    } else if (operation.kind == 3) {
      if (threadIdx.x == 0) {
        const int32_t diameter = p[6] * 2 + 2;
        if (p[0] <= 0 || p[1] <= 0 || p[5] < 2 || p[5] > 64 ||
            p[6] < 1 || p[6] > 16 || p[2] < p[6] || p[4] < p[6] ||
            !valid_box(p[2] - p[6], p[3], p[4] - p[6], diameter,
                       p[5] > p[6] + 1 ? p[5] : p[6] + 1, diameter,
                       size_x, size_y, size_z)) invalid = 1;
      }
      __syncthreads();
      if (!invalid) {
        for (uint32_t id = threadIdx.x; id < volume; id += blockDim.x) {
          int32_t x, y, z;
          coordinate_from_id(id, size_x, size_z, &x, &y, &z);
          const uint16_t material = tree_material(p, x, y, z);
          if (material != 0) scene[id] = material;
        }
      }
    } else if (operation.kind == 4) {
      if (threadIdx.x == 0) {
        const int32_t bx = p[8] == 0 ? p[5] : 1;
        const int32_t bz = p[8] == 1 ? p[5] : 1;
        if (p[0] <= 0 || p[5] <= 0 || p[8] < 0 || p[8] > 1 ||
            p[9] < 1 || p[9] > 64 ||
            !valid_box(p[2], p[3], p[4], bx, 5, bz,
                       size_x, size_y, size_z)) invalid = 1;
      }
      __syncthreads();
      if (!invalid) {
        for (uint32_t id = threadIdx.x; id < volume; id += blockDim.x) {
          int32_t x, y, z;
          coordinate_from_id(id, size_x, size_z, &x, &y, &z);
          if (fence_contains(p, x, y, z)) scene[id] = static_cast<uint16_t>(p[0]);
        }
      }
    } else if (operation.kind == 7) {
      int32_t tangent0 = 0, tangent1 = 0;
      if (p[8] == 0) { tangent0 = 1; tangent1 = 2; }
      else if (p[8] == 1) { tangent0 = 0; tangent1 = 2; }
      else { tangent0 = 0; tangent1 = 1; }
      int32_t bounds[3] = {1, 1, 1};
      if (p[8] >= 0 && p[8] <= 2) {
        bounds[p[8]] = p[7]; bounds[tangent0] = p[5]; bounds[tangent1] = p[6];
      }
      if (threadIdx.x == 0) {
        const uint64_t mask_end = static_cast<uint64_t>(p[16]) +
                                  static_cast<uint64_t>(p[5]) * p[6];
        if (p[0] <= 0 || p[8] < 0 || p[8] > 2 || p[5] <= 0 || p[6] <= 0 ||
            p[7] <= 0 || p[16] < 0 || mask_end > mask_count ||
            !valid_box(p[2], p[3], p[4], bounds[0], bounds[1], bounds[2],
                       size_x, size_y, size_z)) invalid = 1;
      }
      __syncthreads();
      if (!invalid) {
        for (uint32_t id = threadIdx.x; id < volume; id += blockDim.x) {
          int32_t coordinate[3];
          coordinate_from_id(id, size_x, size_z,
                             &coordinate[0], &coordinate[1], &coordinate[2]);
          const int32_t d = coordinate[p[8]] - p[2 + p[8]];
          const int32_t u = coordinate[tangent0] - p[2 + tangent0];
          const int32_t v = coordinate[tangent1] - p[2 + tangent1];
          if (d >= 0 && d < p[7] && u >= 0 && u < p[5] && v >= 0 && v < p[6] &&
              masks[p[16] + u + p[5] * v] != 0)
            scene[id] = static_cast<uint16_t>(p[0]);
        }
      }
    } else if (operation.kind == 12) {
      if (threadIdx.x == 0 &&
          !valid_box(p[2], p[3], p[4], p[5], p[6], p[7],
                     size_x, size_y, size_z)) invalid = 1;
      __syncthreads();
      if (!invalid) {
        for (uint32_t id = threadIdx.x; id < volume; id += blockDim.x) {
          int32_t x, y, z;
          coordinate_from_id(id, size_x, size_z, &x, &y, &z);
          if (in_box(x, y, z, p[2], p[3], p[4], p[5], p[6], p[7])) {
            if (scene[id] != 0) atomicAdd(&auxiliary, 1u);
            scene[id] = 0;
          }
        }
      }
      __syncthreads();
      if (threadIdx.x == 0 && !invalid) {
        if (auxiliary == 0) invalid = 1;
        else charge(auxiliary, maximum_expansion, maximum_writes, &writes, &invalid);
      }
    } else if (operation.kind >= 8 && operation.kind <= 11) {
      if (threadIdx.x == 0) {
        if (!valid_box(p[2], p[3], p[4], p[5], p[6], p[7],
                       size_x, size_y, size_z)) invalid = 1;
        if (operation.kind == 8 && !nonzero_delta(p)) invalid = 1;
        if (operation.kind == 9 && (p[15] < 1 || p[15] > 3)) invalid = 1;
        if (operation.kind == 10 && (p[15] < 0 || p[15] > 2)) invalid = 1;
        if (operation.kind == 11 &&
            (p[8] < 2 || p[8] > 512 || !nonzero_delta(p))) invalid = 1;
      }
      for (uint32_t id = threadIdx.x; id < volume; id += blockDim.x) snapshot[id] = scene[id];
      __syncthreads();
      if (!invalid) {
        for (uint32_t id = threadIdx.x; id < volume; id += blockDim.x) {
          int32_t x, y, z;
          coordinate_from_id(id, size_x, size_z, &x, &y, &z);
          if (!in_box(x, y, z, p[2], p[3], p[4], p[5], p[6], p[7]) ||
              snapshot[id] == 0) continue;
          atomicAdd(&auxiliary, 1u);
          const int32_t rx = x - p[2], ry = y - p[3], rz = z - p[4];
          if (operation.kind == 8) {
            const int32_t dx = x + p[9], dy = y + p[10], dz = z + p[11];
            if (dx < 0 || dy < 0 || dz < 0 || dx >= static_cast<int32_t>(size_x) ||
                dy >= static_cast<int32_t>(size_y) || dz >= static_cast<int32_t>(size_z))
              atomicExch(&invalid, 1u);
          } else if (operation.kind == 9) {
            int32_t tx = 0, tz = 0;
            if (p[15] == 1) { tx = p[7] - 1 - rz; tz = rx; }
            else if (p[15] == 2) { tx = p[5] - 1 - rx; tz = p[7] - 1 - rz; }
            else { tx = rz; tz = p[5] - 1 - rx; }
            const int32_t dx = p[12] + tx, dy = p[13] + ry, dz = p[14] + tz;
            if (dx < 0 || dy < 0 || dz < 0 || dx >= static_cast<int32_t>(size_x) ||
                dy >= static_cast<int32_t>(size_y) || dz >= static_cast<int32_t>(size_z))
              atomicExch(&invalid, 1u);
          } else if (operation.kind == 10) {
            int32_t relative[3] = {rx, ry, rz};
            relative[p[15]] = p[5 + p[15]] - 1 - relative[p[15]];
            const int32_t dx = p[12] + relative[0];
            const int32_t dy = p[13] + relative[1];
            const int32_t dz = p[14] + relative[2];
            if (dx < 0 || dy < 0 || dz < 0 || dx >= static_cast<int32_t>(size_x) ||
                dy >= static_cast<int32_t>(size_y) || dz >= static_cast<int32_t>(size_z))
              atomicExch(&invalid, 1u);
          } else {
            for (int32_t index = 1; index < p[8]; ++index) {
              const int32_t dx = x + p[9] * index;
              const int32_t dy = y + p[10] * index;
              const int32_t dz = z + p[11] * index;
              if (dx < 0 || dy < 0 || dz < 0 || dx >= static_cast<int32_t>(size_x) ||
                  dy >= static_cast<int32_t>(size_y) || dz >= static_cast<int32_t>(size_z))
                atomicExch(&invalid, 1u);
            }
          }
        }
      }
      __syncthreads();
      if (threadIdx.x == 0 && !invalid) {
        const uint64_t amount = static_cast<uint64_t>(auxiliary) *
            (operation.kind == 11 ? static_cast<uint32_t>(p[8] - 1) : 1u);
        if (amount > UINT32_MAX) invalid = 1;
        else charge(static_cast<uint32_t>(amount), maximum_expansion,
                    maximum_writes, &writes, &invalid);
      }
      __syncthreads();
      if (!invalid) {
        for (uint32_t id = threadIdx.x; id < volume; id += blockDim.x) {
          int32_t x, y, z;
          coordinate_from_id(id, size_x, size_z, &x, &y, &z);
          int32_t sx = -1, sy = -1, sz = -1;
          if (operation.kind == 8) {
            sx = x - p[9]; sy = y - p[10]; sz = z - p[11];
          } else if (operation.kind == 9) {
            const int32_t dx = x - p[12], dy = y - p[13], dz = z - p[14];
            sy = p[3] + dy;
            if (p[15] == 1) { sx = p[2] + dz; sz = p[4] + p[7] - 1 - dx; }
            else if (p[15] == 2) { sx = p[2] + p[5] - 1 - dx; sz = p[4] + p[7] - 1 - dz; }
            else { sx = p[2] + p[5] - 1 - dz; sz = p[4] + dx; }
          } else if (operation.kind == 10) {
            int32_t relative[3] = {x - p[12], y - p[13], z - p[14]};
            relative[p[15]] = p[5 + p[15]] - 1 - relative[p[15]];
            sx = p[2] + relative[0]; sy = p[3] + relative[1]; sz = p[4] + relative[2];
          } else {
            for (int32_t index = p[8] - 1; index >= 1; --index) {
              const int32_t candidate_x = x - p[9] * index;
              const int32_t candidate_y = y - p[10] * index;
              const int32_t candidate_z = z - p[11] * index;
              if (in_box(candidate_x, candidate_y, candidate_z,
                         p[2], p[3], p[4], p[5], p[6], p[7])) {
                const uint32_t source_id = coordinate_id(candidate_x, candidate_y,
                                                         candidate_z, size_x, size_z);
                if (snapshot[source_id] != 0) {
                  sx = candidate_x; sy = candidate_y; sz = candidate_z;
                  break;
                }
              }
            }
          }
          if (sx >= 0 && sy >= 0 && sz >= 0 &&
              in_box(sx, sy, sz, p[2], p[3], p[4], p[5], p[6], p[7])) {
            const uint16_t material = snapshot[coordinate_id(sx, sy, sz, size_x, size_z)];
            if (material != 0) scene[id] = material;
          }
        }
      }
    } else {
      if (threadIdx.x == 0) invalid = 1;
    }
    __syncthreads();
  }

  for (uint32_t id = threadIdx.x; id < volume; id += blockDim.x) {
    const uint16_t before = scene[id];
    const uint16_t after = target[id];
    const uint32_t signature = mismatch_signature(before, after);
    if (signature == 0) continue;
    atomicAdd(&score->mismatches, 1u);
    if ((signature >> 16) == 1) atomicAdd(&score->set_patches, 1u);
    else if ((signature >> 16) == 2) atomicAdd(&score->clear_patches, 1u);
    else atomicAdd(&score->paint_patches, 1u);
    const uint32_t previous = id == 0 ? 0 : mismatch_signature(scene[id - 1], target[id - 1]);
    if (id == 0 || previous != signature) atomicAdd(&score->patch_runs, 1u);
  }
  __syncthreads();
  if (threadIdx.x == 0) {
    score->valid = invalid == 0 ? 1u : 0u;
    score->writes = writes;
  }
}

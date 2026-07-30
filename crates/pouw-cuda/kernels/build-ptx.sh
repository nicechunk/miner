#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"
nvcc \
  --ptx \
  --std=c++14 \
  --gpu-architecture=compute_70 \
  --output-file ncm4_score.ptx \
  ncm4_score.cu
perl -0pi -e 's/\n+\z/\n/' ncm4_score.ptx

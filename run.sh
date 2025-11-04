#!/bin/bash

USE_CUDA=false
if [ "$1" == "cuda" ]; then
  USE_CUDA=true
fi

set -e

mkdir -p rpc-cache
source .env
# MODE=execute # can be execute-host, execute, execute-metered, prove-app, prove-stark, or prove-evm (needs "evm-verify" feature)

cd bin/client-eth
RUSTFLAGS="-Clink-arg=--emit-relocs" cargo openvm build --no-transpile
mkdir -p ../host/elf
SRC="target/riscv32im-risc0-zkvm-elf/release/openvm-client-eth"
DEST="../host/elf/openvm-client-eth"

if [ ! -f "$DEST" ] || ! cmp -s "$SRC" "$DEST"; then
  cp "$SRC" "$DEST"
fi
cd ../..

mkdir -p rpc-cache
source .env
# MODE=execute # can be compile, execute, execute-metered, prove-mock, prove-app, prove-stark, or prove-evm (needs "evm-verify" feature)
PROFILE="release"
FEATURES="metrics,jemalloc,unprotected" # removed tco here till we have that fixed
BLOCK_NUMBER=21882667
# switch to +nightly-2025-08-19 if using tco
TOOLCHAIN="+nightly-2025-08-19" # "+stable"
BIN_NAME="openvm-reth-benchmark-bin"
MAX_SEGMENT_LENGTH=$((1 << 22))
SEGMENT_MAX_CELLS=1200000000
VPMM_PAGE_SIZE=$((4 << 20))
VPMM_PAGES=$((12 * $MAX_SEGMENT_LENGTH/ $VPMM_PAGE_SIZE))

if [ "$USE_CUDA" = "true" ]; then
  FEATURES="$FEATURES,cuda"
else
  FEATURES="$FEATURES,nightly-features"
fi
if [ "$MODE" = "prove-evm" ]; then
  FEATURES="$FEATURES,evm-verify"
fi

if grep -m1 -q 'avx512f' /proc/cpuinfo; then
  RUSTFLAGS="-Ctarget-cpu=native -C target-feature=+avx512f"
else
  RUSTFLAGS="-Ctarget-cpu=native"
fi

export JEMALLOC_SYS_WITH_MALLOC_CONF="retain:true,background_thread:true,metadata_thp:always,dirty_decay_ms:10000,muzzy_decay_ms:10000,abort_conf:true"
RUSTFLAGS=$RUSTFLAGS cargo $TOOLCHAIN build --bin $BIN_NAME --profile=$PROFILE --no-default-features --features=$FEATURES
PARAMS_DIR="$HOME/.openvm/params/"

# Use target/debug if profile is dev
if [ "$PROFILE" = "dev" ]; then
  TARGET_DIR="debug"
else
  TARGET_DIR="$PROFILE"
fi

# Default options if not set
: "${APC_SETUP_NAME:=my-setup}"
: "${MODE:=execute}"
: "${APC:=0}"
: "${APC_SKIP:=0}"
: "${PGO_TYPE:=cell}"

POWDR_APC_CANDIDATES_DIR=apcs RUST_LOG="debug" OUTPUT_PATH="metrics.json" VPMM_PAGES=$VPMM_PAGES VPMM_PAGE_SIZE=$VPMM_PAGE_SIZE ./target/$TARGET_DIR/$BIN_NAME \
  --kzg-params-dir $PARAMS_DIR \
  --mode $MODE \
  --block-number $BLOCK_NUMBER \
  --rpc-url $RPC_1 \
  --cache-dir rpc-cache \
  --app-log-blowup 1 \
  --leaf-log-blowup 1 \
  --internal-log-blowup 2 \
  --root-log-blowup 3 \
  --max-segment-length $MAX_SEGMENT_LENGTH \
  --segment-max-cells $SEGMENT_MAX_CELLS \
  --num-children-leaf 1 \
  --num-children-internal 3 \
  --apc-cache-dir apc-cache \
  --apc-setup-name ${APC_SETUP_NAME}_${APC}_${APC_SKIP}_${PGO_TYPE}_${BLOCK_NUMBER} \
  --apc "$APC" \
  --apc-skip "$APC_SKIP" \
  --pgo-type "$PGO_TYPE"

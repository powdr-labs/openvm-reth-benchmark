#!/usr/bin/env bash
set -euo pipefail

BIN_PATH="${OVM_BIN}"

# if no args, print usage
if [[ $# -lt 1 ]]; then
  echo "[prove_block.sh] Usage: $0 <BLOCK_NUMBER>" >&2
  exit 2
fi

source .env

export JEMALLOC_SYS_WITH_MALLOC_CONF="retain:true,background_thread:true,metadata_thp:always,dirty_decay_ms:10000,muzzy_decay_ms:10000,abort_conf:true"

PARAMS_DIR="$HOME/.openvm/params/"

BLOCK_NUMBER="$1"
PGO_BLOCK_NUMBERS="${PGO_BLOCK_NUMBERS:-24171377}"
PGO_BLOCK_NUMBERS_ESCAPED="${PGO_BLOCK_NUMBERS//,/_}"
# bench params
APP_LOG_BLOWUP="${APP_LOG_BLOWUP:-1}"
LEAF_LOG_BLOWUP="${LEAF_LOG_BLOWUP:-1}"
INTERNAL_LOG_BLOWUP="${INTERNAL_LOG_BLOWUP:-2}"
ROOT_LOG_BLOWUP="${ROOT_LOG_BLOWUP:-3}"
MAX_SEGMENT_LENGTH="${MAX_SEGMENT_LENGTH:-4194304}"
SEGMENT_MAX_CELLS="${SEGMENT_MAX_CELLS:-1200000000}"
VPMM_PAGE_SIZE=$((4 << 20))
VPMM_PAGES=$((12 * $MAX_SEGMENT_LENGTH/ $VPMM_PAGE_SIZE))
# apc params
APC="${APC:-0}"
APC_SKIP="${APC_SKIP:-0}"
PGO_TYPE="${PGO_TYPE:-cell}"
APC_SETUP_NAME="${APC_SETUP_NAME:-reth-setup}"

# output params
OUTPUT_DIR="output-${BLOCK_NUMBER}-apc-${APC}"
OUTPUT_PATH="${OUTPUT_PATH:-metrics.json}"

echo "[prove_block.sh] Downloading block ${BLOCK_NUMBER}" >&2

mkdir -p "$OUTPUT_DIR"

echo "[prove_block.sh] Starting proof at $(date -Is) with BIN=$BIN_PATH" >&2
start_ts_ms=$(date +%s%3N)

set -x

if [[ "${2:-}" == "make-input" ]]; then
  # make input ################################
  echo "[prove_block.sh] Downloading block and preparing input" >&2
  "$BIN_PATH" \
      --kzg-params-dir $PARAMS_DIR \
      --mode make-input \
      --generated-input-path input.json \
      --block-number $BLOCK_NUMBER \
      --pgo-block-numbers $PGO_BLOCK_NUMBERS \
      --rpc-url $RPC_1 \
      --cache-dir rpc-cache \
      --app-log-blowup "$APP_LOG_BLOWUP" \
      --leaf-log-blowup "$LEAF_LOG_BLOWUP" \
      --internal-log-blowup "$INTERNAL_LOG_BLOWUP" \
      --root-log-blowup "$ROOT_LOG_BLOWUP" \
      --max-segment-length "$MAX_SEGMENT_LENGTH" \
      --segment-max-cells "$SEGMENT_MAX_CELLS" \
      --num-children-leaf 1 \
      --num-children-internal 3 \
      --output-dir "$OUTPUT_DIR" \
      --apc-cache-dir apc-cache \
      --apc-setup-name ${APC_SETUP_NAME}_${APC}_${APC_SKIP}_${PGO_TYPE}_${PGO_BLOCK_NUMBERS_ESCAPED} \
      --apc "$APC" \
      --apc-skip "$APC_SKIP" \
      --pgo-type "$PGO_TYPE" \
      --skip-comparison

  # exit if command failed
  status=$?
  if [ $status -ne 0 ]; then
      echo "[prove_block.sh] Failed to make input for block ${BLOCK_NUMBER} with status=$status" >&2
  else
      echo "[prove_block.sh] Successfully made input for block ${BLOCK_NUMBER}" >&2
  fi
else
  # prove stark ################################

  "$BIN_PATH" \
      --kzg-params-dir $PARAMS_DIR \
      --mode prove-stark \
      --input-path input.json \
      --block-number $BLOCK_NUMBER \
      --pgo-block-numbers $PGO_BLOCK_NUMBERS \
      --rpc-url $RPC_1 \
      --cache-dir rpc-cache \
      --app-log-blowup "$APP_LOG_BLOWUP" \
      --leaf-log-blowup "$LEAF_LOG_BLOWUP" \
      --internal-log-blowup "$INTERNAL_LOG_BLOWUP" \
      --root-log-blowup "$ROOT_LOG_BLOWUP" \
      --max-segment-length "$MAX_SEGMENT_LENGTH" \
      --segment-max-cells "$SEGMENT_MAX_CELLS" \
      --num-children-leaf 1 \
      --num-children-internal 3 \
      --output-dir "$OUTPUT_DIR" \
      --apc-cache-dir apc-cache \
      --apc-setup-name ${APC_SETUP_NAME}_${APC}_${APC_SKIP}_${PGO_TYPE}_${PGO_BLOCK_NUMBERS_ESCAPED} \
      --apc "$APC" \
      --apc-skip "$APC_SKIP" \
      --pgo-type "$PGO_TYPE" \
      --skip-comparison 2>&1 > "${OUTPUT_DIR}/out.txt"
  status=$?

  end_ts_ms=$(date +%s%3N)
  duration_ms=$(( end_ts_ms - start_ts_ms ))
  echo "$duration_ms" > "${OUTPUT_DIR}/latency_ms.txt"

  mv "${OUTPUT_PATH}" "${OUTPUT_DIR}/"

  echo "[prove_block.sh] Proof finished with status=$status in ${duration_ms}ms at $(date -Is)" >&2
fi

set +x

exit $status

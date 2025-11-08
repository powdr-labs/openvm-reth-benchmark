import os
import subprocess
import sys
from pathlib import Path

import time
import requests

import ethproofs_api as api

MACHINE_ID=1
VERIFIER_ID="powdr_verifier"

def read_env_var_or_error(v):
    ev = os.getenv(v)
    if not ev :
        raise RuntimeError(f"Environment variable {v} must be set")
    return ev

RPC_URL = read_env_var_or_error("RPC_1")

def get_latest_block():
    """Fetch the latest Ethereum block number from the RPC."""
    payload = {
        "jsonrpc": "2.0",
        "method": "eth_blockNumber",
        "params": [],
        "id": 1
    }
    response = requests.post(RPC_URL, json=payload)
    response.raise_for_status()
    return int(response.json()["result"], 16)

def prove(block):
    """The function to run every 100 blocks."""
    print(f"Proving block {block}")
    script_path = Path(__file__).parent / "prove_block.sh"

    # api.submit_queued(block, MACHINE_ID)

    # download block and prepare input
    status = subprocess.Popen([script_path, str(block), "make-input"]).wait()
    if status != 0:
        RuntimeError(f"make-input failed for block {block}")

    # api.submit_proving(block, MACHINE_ID)

    # do the proof
    status = subprocess.Popen([script_path, str(block)]).wait()
    if status != 0:
        RuntimeError(f"proving failed for block {block}")

    # TODO: load proving_cycles from ...

    # TODO: load proving_time from 'latency_ms' file

    # TODO: load proof from 'proof.json' file

    # api.submit_proof(block, MACHINE_ID, proving_time, proving_cycles, proof, VERIFIER_ID)
    print(f"Done proving block {block}")


def main():
    # ensure the machine id exists in ethproofs
    data = api.get_clusters()
    machine_ids = [c["id"] for c in data]
    if MACHINE_ID not in machine_ids:
        raise RuntimeError(f'Machine ID {MACHINE_ID} not found in ethproofs clusters. Available IDs: {machine_ids}')

    last_checked = 0
    while True:
        try:
            latest_block = get_latest_block()
            print(f"Latest Ethereum block is {latest_block}")

            if last_checked >= latest_block:
                raise RuntimeError(f"Last checked block {last_checked} >= latest Ethereum block {latest_block}")

            next_target = latest_block // 100 * 100
            if next_target != last_checked:
                last_checked = next_target
                prove(next_target)
            else:
                # compute how many blocks until next milestone
                blocks_until_next = 100 - (latest_block % 100)
                # assume average 12s per block, estimate wait time
                est_wait = blocks_until_next * 12
                print(f"Waiting ~{est_wait:.1f}s until next check...")
                time.sleep(est_wait)
        except Exception as e:
            print(f"Error: {e}")
            time.sleep(10)

if __name__ == "__main__":
    main()

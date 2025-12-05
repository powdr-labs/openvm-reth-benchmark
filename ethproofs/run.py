import os
import subprocess
import sys
from pathlib import Path
import json
import base64

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
APC = read_env_var_or_error("APC")

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

def json_to_base64(path):
    # Read JSON file
    with open(path, "r", encoding="utf-8") as f:
        data = json.load(f)

    # Serialize to canonical JSON string (no whitespace)
    json_bytes = json.dumps(data, separators=(",", ":"), ensure_ascii=False).encode("utf-8")

    # Base64 encode
    encoded = base64.b64encode(json_bytes).decode("utf-8")

    return encoded

def read_number(path):
    with open(path, "r") as f:
        return int(f.read().strip())

def extract_total_number(path):
    with open(path, "r") as f:
        lines = f.readlines()

    line = lines[2].strip()

    # Example line:
    # | Total |  121.09 |  7.69 |
    parts = [p.strip() for p in line.split("|") if p.strip()]

    total_number = float(parts[1])

    return total_number

def submit_proved(block, output_dir):
    cycles_file = f"{output_dir}/num_instret"
    cycles = read_number(cycles_file)

    latency_file = f"{output_dir}/latency_ms.txt"
    latency_ms = read_number(latency_file)

    # now we only read time from latency file
    #proof_time_file = f"{output_dir}/metrics.md"
    #proof_time = int(extract_total_number(proof_time_file) * 1000)

    proof_json = f"{output_dir}/proof.json"
    proof = json_to_base64(proof_json)

    api.submit_proof(block, MACHINE_ID, latency_ms, cycles, proof, "powdr")
    print(f"[info] Submitted submit_proof block {block}, proof_time {latency_ms}, cycles {cycles}, proof file {proof_json}")

def prove(block):
    print(f"[info] Proving block {block}")
    script_path = Path(__file__).parent / "prove_block.sh"

    api.submit_queued(block, MACHINE_ID)
    print(f"[info] Submitted submit_queued block {block}")

    # download block and prepare input
    while True:
        status = subprocess.Popen([script_path, str(block), "make-input"]).wait()
        if status != 0:
            print(f"[error] make-input failed for block {block}, trying again in 5s...")
            time.sleep(5)
        else:
            break

    api.submit_proving(block, MACHINE_ID)
    print(f"[info] Submitted submit_proving block {block}")

    # do the proof
    status = subprocess.Popen([script_path, str(block)]).wait()
    if status != 0:
        RuntimeError(f"[error] proving failed for block {block}")

    output_dir = f"output-{block}-apc-{APC}"

    # no need to call openvm prof anymore
    # call openvm prof
    #prof_bin_path = '/workspace/openvm/target/debug/openvm-prof'
    #metrics_file = f"{output_dir}/metrics.json"
    #status = subprocess.Popen([prof_bin_path, '--json-paths', metrics_file]).wait()

    submit_proved(block, output_dir)

    print(f"[info] Done proving block {block}")

def main():
    # ensure the machine id exists in ethproofs
    data = api.get_clusters()
    print(data)
    machine_ids = [c["id"] for c in data]
    print(machine_ids)
    if MACHINE_ID not in machine_ids:
        raise RuntimeError(f'[error] Machine ID {MACHINE_ID} not found in ethproofs clusters. Available IDs: {machine_ids}')

    last_checked = 23946500
    while True:
        try:
            latest_block = get_latest_block()
            print(f"[info] Latest Ethereum block is {latest_block}")

            if last_checked >= latest_block:
                raise RuntimeError(f"[error] Last checked block {last_checked} >= latest Ethereum block {latest_block}")

            next_target = latest_block // 100 * 100
            if next_target != last_checked:
                last_checked = next_target
                prove(next_target)
            else:
                # compute how many blocks until next milestone
                blocks_until_next = 100 - (latest_block % 100)
                # assume average 12s per block, estimate wait time
                est_wait = blocks_until_next * 12
                print(f"[info] Waiting ~{est_wait:.1f}s until next check...")
                time.sleep(est_wait)
        except Exception as e:
            print(f"[error] Error: {e}")
            time.sleep(10)

if __name__ == "__main__":
    main()

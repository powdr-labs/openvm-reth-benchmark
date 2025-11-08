import os
import requests

# API: https://staging--ethproofs.netlify.app/api.html

POWDR_OPENVM_SINGLE_MACHINE_ID = 1

# Configuration
API_BASE = "https://staging--ethproofs.netlify.app/api/v0"

CREATE_SINGLE_MACHINE_ENDPOINT = f"{API_BASE}/single-machine"
GET_CLUSTERS_ENDPOINT = f"{API_BASE}/clusters"
PROOFS_BASE = f"{API_BASE}/proofs"
PROOFS_QUEUED = f"{PROOFS_BASE}/queued"
PROOFS_PROVING = f"{PROOFS_BASE}/proving"
PROOFS_PROVED = f"{PROOFS_BASE}/proved"

API_KEY = os.getenv("ETHPROOFS_API_KEY_STAGING")
if not API_KEY:
    raise RuntimeError("Environment variable ETHPROOFS_API_KEY_STAGING must be set")

SUCCESS_STATUS_CODE = 200

ZKVM_ID_OPENVM_1_4_0 = 14

# Headers including API key
headers = {
    "Authorization": f"Bearer {API_KEY}",
    "Content-Type": "application/json"
}

def submit_proving(block_number, cluster_id):
    payload = {
        "block_number": block_number,
        "cluster_id": cluster_id,
    }

    response = requests.post(PROOFS_PROVING, json=payload, headers=headers)

    if response.status_code == SUCCESS_STATUS_CODE:
        data = response.json()
        print("Proving proof submited with proof_id:", data.get("proof_id"))
    else:
        print("Failed to submit proving proof. Status:", response.status_code)

#submit_proving(23668650, POWDR_OPENVM_SINGLE_MACHINE_ID)

def submit_queued(block_number, cluster_id):
    payload = {
        "block_number": block_number,
        "cluster_id": cluster_id,
    }

    response = requests.post(PROOFS_QUEUED, json=payload, headers=headers)

    if response.status_code == SUCCESS_STATUS_CODE:
        data = response.json()
        print("Queued proof submited with proof_id:", data.get("proof_id"))
    else:
        print("Failed to submit queued proof. Status:", response.status_code)


def submit_proof(block_number, cluster_id, proving_time, proving_cycles, proof, verifier_id):
    payload = {
        "block_number": block_number,
        "cluster_id": cluster_id,
        "proving_time": proving_time,
        "proving_cycles": proving_cycles,
        "proof": proof,
        "verifier_id": verifier_id,
    }

    response = requests.post(PROOFS_PROVED, json=payload, headers=headers)

    if response.status_code == SUCCESS_STATUS_CODE:
        data = response.json()
        print("Proof submited with proof_id:", data.get("proof_id"))
    else:
        print("Failed to submit proof. Status:", response.status_code)

#submit_proof(23668650, POWDR_OPENVM_SINGLE_MACHINE_ID, 60_000, 200_000_000, "proof", "powdr_verifier")

# Already ran this once, returned id=1
def create_single_machine():
    payload = {
        "nickname": "powdr-OpenVM",
        "description": "OpenVM + powdr autoprecompiles",
        "zkvm_version_id": ZKVM_ID_OPENVM_1_4_0,
        "hardware": "Hetzner EX-130R",
        "cycle_type": "OpenVM-RISCV-powdr-autoprecompiles",
        "proof_type": "FRI-based STARKs",
        "machine": {
            "cpu_model": "Intel(R) Xeon(R) Gold 5412U @ 0.8GHz",
            "cpu_cores": 24,
            "gpu_models": ["none"],
            "gpu_count": [1],
            "gpu_memory_gb": [1],
            "memory_size_gb": [256],
            "memory_count": [8],
            "memory_type": ["DDR5-5600 ECC"],
            "storage_size_gb": 3084,
            "total_tera_flops": 450,
            "network_between_machines": "Dual-port 100GbE NIC"
        },
        "cloud_instance_name": "m6i.2xlarge"
    }

    response = requests.post(CREATE_SINGLE_MACHINE_ENDPOINT, json=payload, headers=headers)

    if response.status_code == SUCCESS_STATUS_CODE:
        data = response.json()
        print("Single machine created with ID:", data.get("id"))
    else:
        print("Failed to create single machine. Status:", response.status_code)
        print("Response:", response.text)

def get_clusters():
    resp = requests.get(GET_CLUSTERS_ENDPOINT, headers=headers)
    resp.raise_for_status()

    data = resp.json()
    if not isinstance(data, list):
        raise RuntimeError(f"Unexpected response format: {data!r}")

    return data

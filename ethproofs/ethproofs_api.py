import os
import requests

# STATING API: https://staging--ethproofs.netlify.app/api.html
# PROD API: https://ethproofs.org/api.html

# Configuration
API_BASE_STAGING = "https://staging--ethproofs.netlify.app/api/v0"
API_BASE = "https://ethproofs.org/api/v0"

CREATE_SINGLE_MACHINE_ENDPOINT = f"{API_BASE}/single-machine"
GET_CLUSTERS_ENDPOINT = f"{API_BASE}/clusters"
PROOFS_BASE = f"{API_BASE}/proofs"
PROOFS_QUEUED = f"{PROOFS_BASE}/queued"
PROOFS_PROVING = f"{PROOFS_BASE}/proving"
PROOFS_PROVED = f"{PROOFS_BASE}/proved"

#API_KEY = os.getenv("ETHPROOFS_API_KEY_STAGING")
API_KEY = os.getenv("ETHPROOFS_API_KEY_PROD")
if not API_KEY:
    raise RuntimeError("Environment variable ETHPROOFS_API_KEY_PROD must be set")

SUCCESS_STATUS_CODE = 200

ZKVM_ID_OPENVM_1_4_0 = 17

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
        print("[info] Proving proof submitted with proof_id:", data.get("proof_id"))
    else:
        print("[error] Failed to submit proving proof. Status:", response.status_code)

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

def create_single_machine():
    payload = {
        "nickname": "powdr-OpenVM",
        "description": "OpenVM + powdr autoprecompiles",
        "zkvm_version_id": ZKVM_ID_OPENVM_1_4_0,
        "hardware": "Hetzner EX-130R",
        "cycle_type": "OpenVM-RISCV-powdr-autoprecompiles",
        "proof_type": "FRI-based STARKs",
        "machine": {
            "cpu_model": "AMD Ryzen 9 7950X",
            "cpu_cores": 16,
            "gpu_models": ["NVIDIA RTX 4090"],
            "gpu_count": [1],
            "gpu_memory_gb": [24],
            "memory_size_gb": [41],
            "memory_count": [1],
            "memory_type": ["DDR5-5600 ECC"],
            "storage_size_gb": 100,
            "total_tera_flops": 450,
            "network_between_machines": "Dual-port 100GbE NIC"
        },
        "cloud_instance_name": "NVIDIA RTX 4090-1"
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

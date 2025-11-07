# powdr integration with EthProofs

[EthProofs](https://ethproofs.org/) is an effort by the Ethereum Foundation to compare zkVMs while making proofs for Ethereum block verification.
Their goal is reaching Real Time Proving (RTP), that is, spending less than 12s per block proof.

Currently each zkVM produces one proof every 100 blocks.

Our goal is to do the same by using [openvm-reth-benchmark](https://github.com/powdr-labs/openvm-reth-benchmark) with autoprecompiles,
showing that a zkVM can be accelerated by plugging in APCs.

# Relevant Links

- [EthProofs](https://ethproofs.org/): EthProofs production page
- [EthProofs Staging](https://staging--ethproofs.netlify.app/): EthProofs testing page
- [EthProofs API](https://staging--ethproofs.netlify.app/api.html)
- [openvm-reth-benchmark](https://github.com/powdr-labs/openvm-reth-benchmark): the host and guest we run to generate proofs.

# Workflow

1. Wait until a new Ethereum block is produced, such that `block_number % 100 == 0`.
2. Send a `proof-queued` request to EthProofs.
3. Use `openvm-reth-benchmark` to download and cache that block.
4. Send a `proof-proving` request to EthProofs.
5. Use `openvm-reth-benchmark` to make a `prove-stark` proof for that block.
    - When using CPU for testing, the same server can be used to run the bot and proofs.
    - When using GPU for production, we need to delegate the proofs to some cloud provider (AWS, Vast.ai, RunPod, ...).
6. Send a `proof-proved` request to EthProofs, uploading the generated proof.
7. Loop.

# TODO

- [X] Requests to EthProofs (2, 4, 6).
- [ ] Wait for next target block (1).
- [ ] Download and cache target block (3).
- [ ] Prove target block on CPU (5.1).
- [ ] Prove target block on GPU (5.2).

# Python Setup

```bash
python3 -m venv .venv
source .venv/bin/activate
pip install -r openvm/scripts/requirements.txt
```

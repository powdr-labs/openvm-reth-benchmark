#![cfg_attr(feature = "tco", allow(incomplete_features))]
#![cfg_attr(feature = "tco", feature(explicit_tail_calls))]
use clap_builder::Parser;
use openvm_reth_benchmark::{complete_args, precompute_prover_data, run_reth_benchmark, HostArgs};

const OPENVM_CLIENT_ETH_ELF: &[u8] = include_bytes!("../elf/openvm-client-eth");

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let args = HostArgs::parse();
    let args = complete_args(args);
    let start = std::time::Instant::now();
    let setup = precompute_prover_data(&args, OPENVM_CLIENT_ETH_ELF).await?;
    let elapsed = start.elapsed();
    println!(">>>>>>> PRECOMPUTE_PROVER_DATA: {:?}", elapsed);

    let start = std::time::Instant::now();
    let res = run_reth_benchmark(args, setup, OPENVM_CLIENT_ETH_ELF).await;
    let elapsed = start.elapsed();
    println!(">>>>>>> RUN_RETH_BENCHMARK: {:?}", elapsed);
    res
}

#![cfg_attr(feature = "tco", allow(incomplete_features))]
#![cfg_attr(feature = "tco", feature(explicit_tail_calls))]
use clap_builder::Parser;
use openvm_reth_benchmark::{compile, complete_args, run_reth_benchmark, HostArgs};

const OPENVM_CLIENT_ETH_ELF: &[u8] = include_bytes!("../elf/openvm-client-eth");

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let args = HostArgs::parse();
    let args = complete_args(args);
    let setup = compile(&args, OPENVM_CLIENT_ETH_ELF).await?;
    run_reth_benchmark(args, setup, OPENVM_CLIENT_ETH_ELF).await
}

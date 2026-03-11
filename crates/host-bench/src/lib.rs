#![cfg_attr(feature = "tco", allow(incomplete_features))]
#![cfg_attr(feature = "tco", feature(explicit_tail_calls))]
use alloy_primitives::hex::ToHexExt;
use alloy_provider::RootProvider;
use alloy_rpc_client::RpcClient;
use alloy_transport::layers::RetryBackoffLayer;
use clap::Parser;
use openvm_client_executor::{
    io::ClientExecutorInput, ChainVariant, ClientExecutor, CHAIN_ID_ETH_MAINNET,
};
use openvm_host_executor::HostExecutor;
use openvm_sdk_config::SdkVmConfig;
use openvm_stark_sdk::{
    bench::run_with_metric_collection,
    config::baby_bear_poseidon2::BabyBearPoseidon2CpuEngine,
};
use openvm_transpiler::{elf::Elf, openvm_platform::memory::MEM_SIZE};
use powdr_autoprecompiles::PgoType;
#[cfg(not(feature = "cuda"))]
use powdr_openvm::PowdrSdkCpu;
#[cfg(feature = "cuda")]
use powdr_openvm::PowdrSdkGpu;
use powdr_openvm::{
    extraction_utils::OriginalVmConfig, CompiledProgram, OriginalCompiledProgram,
};
#[cfg(feature = "cuda")]
use powdr_openvm_riscv::ExtendedVmConfigGpuBuilder;
use powdr_openvm_riscv::{ExtendedVmConfig, ExtendedVmConfigCpuBuilder, RiscvISA};

use powdr_openvm_riscv_hints_circuit::HintsExtension;
pub use reth_primitives;
use sdk_v2::{
    config::{
        default_app_params, default_internal_params, default_leaf_params,
        AggregationSystemParams, AppConfig, DEFAULT_APP_LOG_BLOWUP, DEFAULT_APP_L_SKIP,
        DEFAULT_INTERNAL_LOG_BLOWUP, DEFAULT_LEAF_LOG_BLOWUP,
    },
    GenericSdk, StdIn,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    fs::{self, File},
    io::{BufReader, BufWriter},
    path::PathBuf,
};
use tracing::{info, info_span};

mod cli;
use cli::ProviderArgs;

use crate::cli::ProviderConfig;

pub const DEFAULT_LOG_STACKED_HEIGHT: usize = 24;

/// Enum representing the execution mode of the host executable.
#[derive(Debug, Clone, clap::ValueEnum)]
pub enum BenchMode {
    /// Execute natively on host.
    ExecuteHost,
    /// Execute the VM without generating a proof.
    Execute,
    /// Generate sequence of app proofs for continuation segments.
    ProveApp,
    /// Generate a full end-to-end STARK proof with aggregation.
    ProveStark,
    /// Generate input file only.
    MakeInput,
    /// Compile with apcs, no execution.
    Compile,
}

impl std::fmt::Display for BenchMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExecuteHost => write!(f, "execute_host"),
            Self::Execute => write!(f, "execute"),
            Self::ProveApp => write!(f, "prove_app"),
            Self::ProveStark => write!(f, "prove_stark"),
            Self::MakeInput => write!(f, "make_input"),
            Self::Compile => write!(f, "compile"),
        }
    }
}

/// CLI for benchmark configuration.
#[derive(Parser, Debug, Clone)]
#[command(allow_external_subcommands = true)]
pub struct BenchmarkCli {
    /// Application level log blowup
    #[arg(long, default_value_t = DEFAULT_APP_LOG_BLOWUP)]
    pub app_log_blowup: usize,

    /// Log of univariate skip domain size
    #[arg(long, default_value_t = DEFAULT_APP_L_SKIP)]
    pub app_l_skip: usize,

    /// Aggregation (leaf) level log blowup
    #[arg(long, default_value_t = DEFAULT_LEAF_LOG_BLOWUP)]
    pub leaf_log_blowup: usize,

    /// Internal level log blowup
    #[arg(long, default_value_t = DEFAULT_INTERNAL_LOG_BLOWUP)]
    pub internal_log_blowup: usize,

    /// Max trace height per chip in segment for continuations
    #[arg(long, alias = "max_segment_length")]
    pub max_segment_length: Option<u32>,

    /// GPU memory soft limit for segmentation
    #[arg(long)]
    pub segment_max_memory: Option<usize>,
}

/// The arguments for the host executable.
#[derive(Debug, Parser)]
pub struct HostArgs {
    /// The block number of the block to execute.
    #[clap(long)]
    block_number: u64,

    /// The block numbers to do PGO on (comma-separated).
    #[clap(long, value_delimiter = ',')]
    pgo_block_numbers: Vec<u64>,

    #[clap(flatten)]
    provider: ProviderArgs,

    /// The execution mode.
    #[clap(long, value_enum)]
    mode: BenchMode,

    /// Optional path to the directory containing cached client input. A new cache file will be
    /// created from RPC data if it doesn't already exist.
    #[clap(long)]
    cache_dir: Option<PathBuf>,

    /// Path to the directory containing cached apc compilation output.
    #[clap(long)]
    apc_cache_dir: PathBuf,

    #[clap(long)]
    apc_setup_name: String,

    /// The path to the CSV file containing the execution data.
    #[clap(long, default_value = "report.csv")]
    report_path: PathBuf,

    #[clap(flatten)]
    benchmark: BenchmarkCli,

    /// Optional path to the input file.
    #[arg(long)]
    pub input_path: Option<PathBuf>,

    #[arg(long)]
    apc: usize,

    #[arg(long)]
    apc_skip: usize,

    #[arg(long)]
    pgo_type: PgoType,
    /// Path to write the fixtures to. Only needed for mode=make_input
    #[arg(long)]
    pub fixtures_path: Option<PathBuf>,

    /// In make_input mode, this path is where the input JSON is written.
    #[arg(long)]
    pub generated_input_path: Option<PathBuf>,

    /// If specificed, the proof and other output is written to this dir.
    #[arg(long)]
    pub output_dir: Option<PathBuf>,

    #[arg(long, default_value_t = false)]
    pub skip_comparison: bool,
}

pub fn reth_vm_config(app_log_blowup: usize) -> ExtendedVmConfig {
    let mut config = SdkVmConfig::standard();
    config.system.config = config
        .system
        .config
        .with_max_constraint_degree((1 << app_log_blowup) + 1)
        .with_public_values(32);
    ExtendedVmConfig { sdk: config, hints: HintsExtension }
}

pub const RETH_DEFAULT_APP_LOG_BLOWUP: usize = 1;
pub const RETH_DEFAULT_LEAF_LOG_BLOWUP: usize = 1;

const PGO_CHAIN_ID: u64 = CHAIN_ID_ETH_MAINNET;
const APP_LOG_BLOWUP: usize = 1;

/// Cached APC compilation output.
#[derive(Serialize, Deserialize)]
pub struct PrecomputedProverData {
    program: CompiledProgram<RiscvISA>,
}

async fn get_client_input(
    provider_config: &ProviderConfig,
    cache_dir: &Option<PathBuf>,
    chain_id: u64,
    block_number: u64,
) -> eyre::Result<ClientExecutorInput> {
    let client_input_from_cache =
        try_load_input_from_cache(cache_dir.as_ref(), chain_id, block_number)?;

    match (client_input_from_cache, &provider_config.rpc_url) {
        (Some(client_input_from_cache), _) => Ok(client_input_from_cache),
        (None, Some(rpc_url)) => {
            // Cache not found but we have RPC
            // Setup the provider.
            let client = RpcClient::builder()
                .layer(RetryBackoffLayer::new(5, 1000, 100))
                .http(rpc_url.clone());
            let provider = RootProvider::new(client);

            // Setup the host executor.
            let host_executor = HostExecutor::new(provider);

            // Execute the host.
            let client_input =
                host_executor.execute(block_number).await.expect("failed to execute host");

            if let Some(cache_dir) = cache_dir {
                let input_folder = cache_dir.join(format!("input/{}", chain_id));
                if !input_folder.exists() {
                    std::fs::create_dir_all(&input_folder)?;
                }

                let input_path = input_folder.join(format!("{}.bin", block_number));
                let mut cache_file = std::fs::File::create(input_path)?;

                bincode::serde::encode_into_std_write(
                    &client_input,
                    &mut cache_file,
                    bincode::config::standard(),
                )?;
            }

            Ok(client_input)
        }
        (None, None) => {
            eyre::bail!("cache not found and RPC URL not provided")
        }
    }
}

/// Complete the host arguments with defaults
pub fn complete_args(args: HostArgs) -> HostArgs {
    let app_log_blowup = args.benchmark.app_log_blowup;
    assert_eq!(app_log_blowup, APP_LOG_BLOWUP, "App log blowup must be {RETH_DEFAULT_APP_LOG_BLOWUP} because it must match the one used when compiling this benchmark");
    args
}

/// Precompute the APC-specialized program. If the data is already present in the cache, deserialize
/// it and return it.
pub async fn precompute_prover_data(
    args: &HostArgs,
    openvm_client_eth_elf: &[u8],
) -> eyre::Result<PrecomputedProverData> {
    // We do this in a separate scope so the log initialization does not conflict with OpenVM's.
    // The powdr log is enabled during the scope of `_guard`.
    let subscriber =
        tracing_subscriber::FmtSubscriber::builder().with_max_level(tracing::Level::DEBUG).finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let cache_file_path = args.apc_cache_dir.join(&args.apc_setup_name).with_extension("bin");

    if let Some(compiled_program) =
        File::open(&cache_file_path).ok().map(BufReader::new).map(|mut file| {
            bincode::serde::decode_from_std_read(&mut file, bincode::config::standard())
                .expect("Found cached precomputed prover data, but deserialization failed")
        })
    {
        tracing::info!("Precomputed prover data for key {} found in cache", args.apc_setup_name);
        return Ok(compiled_program);
    }

    tracing::info!(
        "Precomputed prover data for key {} not found in cache. Precomputing prover data.",
        args.apc_setup_name
    );

    let provider_config = args.provider.clone().into_provider().await?;

    let mut pgo_stdins = Vec::new();

    for block_id in args.pgo_block_numbers.iter() {
        let pgo_client_input =
            get_client_input(&provider_config, &args.cache_dir, PGO_CHAIN_ID, *block_id)
                .await
                .unwrap();

        let mut pgo_stdin = StdIn::default();
        pgo_stdin.write(&pgo_client_input);
        pgo_stdins.push(pgo_stdin);
    }

    let app_log_blowup = args.benchmark.app_log_blowup;
    let app_l_skip = args.benchmark.app_l_skip;

    let vm_config = reth_vm_config(app_log_blowup);

    let app_n_stack = DEFAULT_LOG_STACKED_HEIGHT - app_l_skip;
    let system_params = default_app_params(app_log_blowup, app_l_skip, app_n_stack);
    let app_config = AppConfig::new(vm_config.clone(), system_params);

    let sdk: GenericSdk<BabyBearPoseidon2CpuEngine, ExtendedVmConfigCpuBuilder> =
        GenericSdk::new(app_config, AggregationSystemParams::default())?;
    let elf = Elf::decode(openvm_client_eth_elf, MEM_SIZE as u32)?;
    let exe = sdk.convert_to_exe(elf.clone())?;
    let elf = powdr_riscv_elf::load_elf_from_buffer(openvm_client_eth_elf);

    let program = powdr::apc(
        OriginalCompiledProgram::new(exe, OriginalVmConfig::new(vm_config), elf),
        args.apc,
        args.apc_skip,
        args.pgo_type,
        pgo_stdins,
    );

    let setup = PrecomputedProverData { program };

    tracing::info!("Saving prover data to cache at {}", cache_file_path.display());
    std::fs::create_dir_all(&args.apc_cache_dir).unwrap();
    bincode::serde::encode_into_std_write(
        &setup,
        &mut BufWriter::new(File::create(cache_file_path).unwrap()),
        bincode::config::standard(),
    )
    .unwrap();

    Ok(setup)
}

pub async fn run_reth_benchmark(
    args: HostArgs,
    setup: PrecomputedProverData,
    openvm_client_eth_elf: &[u8],
) -> eyre::Result<()> {
    // Initialize the environment variables.
    dotenv::dotenv().ok();

    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }

    // Parse the command line arguments.
    let args = args;
    let provider_config = args.provider.into_provider().await?;

    match provider_config.chain_id {
        #[allow(non_snake_case)]
        CHAIN_ID_ETH_MAINNET => (),
        _ => {
            eyre::bail!("unknown chain ID: {}", provider_config.chain_id);
        }
    };

    let chain_id = provider_config.chain_id;

    let client_input =
        get_client_input(&provider_config, &args.cache_dir, chain_id, args.block_number).await?;

    let mut stdin = StdIn::default();
    stdin.write(&client_input);
    info!("input loaded");

    if matches!(args.mode, BenchMode::MakeInput) {
        let words: Vec<u32> = openvm::serde::to_vec(&client_input).unwrap();
        let bytes: Vec<u8> = words.into_iter().flat_map(|w| w.to_le_bytes()).collect();
        let hex_bytes = String::from("0x01") + &hex::encode(&bytes);
        let input = json!({
            "input": [hex_bytes]
        });
        let input = serde_json::to_string(&input).unwrap();
        fs::write(args.generated_input_path.unwrap(), input)?;
        return Ok(());
    }

    let app_log_blowup = args.benchmark.app_log_blowup;
    let app_l_skip = args.benchmark.app_l_skip;
    let leaf_log_blowup = args.benchmark.leaf_log_blowup;
    let internal_log_blowup = args.benchmark.internal_log_blowup;

    let elf = Elf::decode(openvm_client_eth_elf, MEM_SIZE as u32)?;

    let PrecomputedProverData { program: CompiledProgram { exe, vm_config } } = setup;

    let app_n_stack = DEFAULT_LOG_STACKED_HEIGHT - app_l_skip;
    let system_params = default_app_params(app_log_blowup, app_l_skip, app_n_stack);
    let app_config = AppConfig::new(vm_config.clone(), system_params);
    let agg_params = AggregationSystemParams {
        leaf: default_leaf_params(leaf_log_blowup),
        internal: default_internal_params(internal_log_blowup),
        compression: None,
    };

    // Create an SDK based on the `SpecializedConfig` we generated
    #[cfg(feature = "cuda")]
    let specialized_sdk = PowdrSdkGpu::new(app_config, agg_params)?;
    #[cfg(not(feature = "cuda"))]
    let specialized_sdk = PowdrSdkCpu::new(app_config, agg_params)?;

    let program_name = format!("reth.{}.block_{}", args.mode, args.block_number);

    // `prover` can be called over both `elf` and `exe`.
    // We had a bug before where `prover(elf)` was called and silently didn't use any apcs.
    // So we drop `elf` here to make sure it's never used later.
    drop(elf);

    run_with_metric_collection("OUTPUT_PATH", || {
        info_span!("reth-block", block_number = args.block_number).in_scope(
            || -> eyre::Result<()> {
                // Run host execution for comparison
                if !args.skip_comparison {
                    let block_hash = info_span!("host.execute", group = program_name).in_scope(
                        || -> eyre::Result<_> {
                            let executor = ClientExecutor;
                            // Create a child span to get the group label propagated
                            let header = info_span!("client.execute").in_scope(|| {
                                executor.execute(ChainVariant::Mainnet, client_input.clone())
                            })?;
                            let block_hash =
                                info_span!("header.hash_slow").in_scope(|| header.hash_slow());
                            Ok(block_hash)
                        },
                    )?;
                    println!("block_hash (execute-host): {}", ToHexExt::encode_hex(&block_hash));
                }

                // For ExecuteHost mode, only do host execution
                if matches!(args.mode, BenchMode::ExecuteHost) {
                    return Ok(());
                }

                // Execute for benchmarking:
                if !args.skip_comparison {
                    let pvs = info_span!("sdk.execute", group = program_name)
                        .in_scope(|| specialized_sdk.execute(exe.clone(), stdin.clone()))?;
                    let block_hash = pvs;
                    println!("block_hash (execute): {}", ToHexExt::encode_hex(&block_hash));
                }

                match args.mode {
                    BenchMode::Compile => {
                        // This mode is used to compile the program with APCs, no execution.
                        println!("Compiled program with APCs");
                    }
                    BenchMode::Execute => {}
                    BenchMode::ProveApp => {
                        let mut prover = specialized_sdk
                            .app_prover(exe)?
                            .with_program_name(program_name);
                        let proof = prover.prove(stdin)?;
                        tracing::info!(
                            "App proof generated with {} segments",
                            proof.per_segment.len()
                        );
                    }
                    BenchMode::ProveStark => {
                        let mut prover = specialized_sdk.prover(exe)?;
                        prover.app_prover.set_program_name(program_name);
                        let _proof = prover.prove(stdin)?;
                        tracing::info!("STARK proof generated");
                    }
                    _ => {
                        // This case is handled earlier and should not reach here
                        unreachable!();
                    }
                }

                Ok(())
            },
        )
    })?;
    Ok(())
}

fn try_load_input_from_cache(
    cache_dir: Option<&PathBuf>,
    chain_id: u64,
    block_number: u64,
) -> eyre::Result<Option<ClientExecutorInput>> {
    Ok(if let Some(cache_dir) = cache_dir {
        let cache_path = cache_dir.join(format!("input/{chain_id}/{block_number}.bin"));

        if cache_path.exists() {
            // TODO: prune the cache if invalid instead
            let mut cache_file = std::fs::File::open(cache_path)?;
            let client_input: ClientExecutorInput =
                bincode::serde::decode_from_std_read(&mut cache_file, bincode::config::standard())?;

            Some(client_input)
        } else {
            None
        }
    } else {
        None
    })
}

mod powdr {

    use sdk_v2::{
        config::{
            default_app_params, AggregationSystemParams, AppConfig, DEFAULT_APP_LOG_BLOWUP,
            DEFAULT_APP_L_SKIP,
        },
        StdIn,
    };
    use powdr_autoprecompiles::{
        empirical_constraints::EmpiricalConstraints, execution_profile::execution_profile, PgoType,
        PowdrConfig,
    };
    use powdr_openvm::{
        default_powdr_openvm_config, detect_empirical_constraints, BabyBearOpenVmApcAdapter,
        CompiledProgram, OriginalCompiledProgram, PowdrExecutionProfileSdkCpu, Prog,
    };
    use powdr_openvm_riscv::{compile_exe, DegreeBound, PgoConfig, RiscvISA};
    use std::fs;

    use super::DEFAULT_LOG_STACKED_HEIGHT;

    /// This function is used to generate the specialized program for the Powdr APC.
    pub fn apc(
        original_program: OriginalCompiledProgram<RiscvISA>,
        apc: usize,
        apc_skip: usize,
        pgo_type: PgoType,
        pgo_stdin: Vec<StdIn>,
    ) -> CompiledProgram<RiscvISA> {
        // Set app configuration
        let app_n_stack = DEFAULT_LOG_STACKED_HEIGHT - DEFAULT_APP_L_SKIP;
        let system_params =
            default_app_params(DEFAULT_APP_LOG_BLOWUP, DEFAULT_APP_L_SKIP, app_n_stack);
        let app_config = AppConfig::new(original_program.vm_config.config.clone(), system_params);

        // prepare for execute
        let sdk = PowdrExecutionProfileSdkCpu::<RiscvISA>::new(
            app_config,
            AggregationSystemParams::default(),
        )
        .unwrap();

        let execute = || {
            for stdin in &pgo_stdin {
                sdk.execute(original_program.exe.clone(), stdin.clone()).unwrap();
            }
        };

        let program = Prog::from(&original_program.exe.program);

        let pgo_config = match pgo_type {
            PgoType::None => PgoConfig::None,
            PgoType::Instruction => PgoConfig::Instruction(execution_profile::<
                BabyBearOpenVmApcAdapter<RiscvISA>,
            >(&program, execute)),
            PgoType::Cell => PgoConfig::Cell(
                execution_profile::<BabyBearOpenVmApcAdapter<RiscvISA>>(&program, execute),
                None, // max total columns
            ),
        };

        let mut config = default_powdr_openvm_config(apc as u64, apc_skip as u64);

        config.degree_bound = DegreeBound { identities: 3, bus_interactions: 2 };

        if let Ok(path) = std::env::var("POWDR_APC_CANDIDATES_DIR") {
            fs::create_dir_all(&path).unwrap();
            config = config.with_apc_candidates_dir(path);
        }

        let empirical_constraints = match std::env::var("POWDR_OPTIMISTIC_PRECOMPILES") {
            Ok(use_op) if use_op == "1" => {
                match std::env::var("POWDR_EMPIRICAL_CONSTRAINTS_PATH") {
                    Ok(path) => {
                        tracing::info!("Loading empirical constraints from file: {path}");
                        let file = fs::File::open(path).unwrap();
                        let reader = std::io::BufReader::new(file);
                        let empirical_constraints: EmpiricalConstraints =
                            serde_json::from_reader(reader).unwrap();
                        empirical_constraints
                    }
                    Err(_) => {
                        tracing::info!(
                            "Computing empirical constraints using PGO stdins ({} inputs)...",
                            pgo_stdin.len()
                        );
                        tracing::info!("This can take a while. If you have precomputed constraints, you can set the POWDR_EMPIRICAL_CONSTRAINTS_PATH environment variable to load them from a file.");
                        compute_empirical_constraints(&original_program, &config, pgo_stdin)
                    }
                }
            }
            _ => EmpiricalConstraints::default(),
        };

        compile_exe(original_program, config, pgo_config, empirical_constraints).unwrap()
    }

    fn compute_empirical_constraints(
        guest_program: &OriginalCompiledProgram<RiscvISA>,
        powdr_config: &PowdrConfig,
        stdins: Vec<StdIn>,
    ) -> EmpiricalConstraints {
        tracing::info!("Computing empirical constraints...");
        tracing::warn!("Optimistic precompiles currently lead to invalid proofs!");
        let empirical_constraints =
            detect_empirical_constraints(guest_program, powdr_config.degree_bound, stdins);
        if let Some(path) = &powdr_config.apc_candidates_dir_path {
            let path = path.join("empirical_constraints.json");
            tracing::info!("Saving empirical constraints debug info to {}", path.display());
            let json = serde_json::to_string_pretty(&empirical_constraints).unwrap();
            std::fs::write(path, json).unwrap();
        }
        empirical_constraints
    }
}

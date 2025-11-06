#![cfg_attr(feature = "tco", allow(incomplete_features))]
#![cfg_attr(feature = "tco", feature(explicit_tail_calls))]
use alloy_primitives::hex::ToHexExt;
use alloy_provider::RootProvider;
use alloy_rpc_client::RpcClient;
use alloy_transport::layers::RetryBackoffLayer;
use clap::Parser;
use openvm_benchmarks_prove::util::BenchmarkCli;
use openvm_circuit::{
    arch::*,
    arch::execution_mode::Segment,
    openvm_stark_sdk::{
        bench::run_with_metric_collection, openvm_stark_backend::p3_field::PrimeField32,
    },
};
use openvm_client_executor::{io::ClientExecutorInput, ClientExecutor, CHAIN_ID_ETH_MAINNET};
use openvm_host_executor::HostExecutor;
pub use openvm_native_circuit::NativeConfig;
use openvm_native_circuit::NativeCpuBuilder;

use openvm_sdk::{
    config::{AppConfig, SdkVmConfig},
    keygen::{AggProvingKey, AppProvingKey},
    prover::{verify_app_proof, vm::new_local_prover},
    types::VersionedVmStarkProof,
    DefaultStarkEngine, GenericSdk, StdIn,
};
use openvm_stark_sdk::{
    config::baby_bear_poseidon2::BabyBearPoseidon2Engine, engine::StarkFriEngine,
};
use openvm_transpiler::{elf::Elf, openvm_platform::memory::MEM_SIZE};
use powdr_autoprecompiles::PgoType;
#[cfg(feature = "cuda")]
use powdr_openvm::ExtendedVmConfigGpuBuilder;
#[cfg(not(feature = "cuda"))]
use powdr_openvm::PowdrSdkCpu;
#[cfg(feature = "cuda")]
use powdr_openvm::PowdrSdkGpu;
#[cfg(feature = "cuda")]
pub use openvm_cuda_backend::engine::GpuBabyBearPoseidon2Engine;
use powdr_openvm::{
    CompiledProgram, ExtendedVmConfig, ExtendedVmConfigCpuBuilder, OriginalCompiledProgram,
    SpecializedConfig, SpecializedConfigCpuBuilder,
};

use powdr_openvm_hints_circuit::HintsExtension;
pub use reth_primitives;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    fs::{self, File},
    io::{BufReader, BufWriter},
    path::PathBuf,
};
use tracing::{info, info_span};

mod execute;

mod cli;
use cli::ProviderArgs;

use crate::cli::ProviderConfig;

/// Enum representing the execution mode of the host executable.
#[derive(Debug, Clone, clap::ValueEnum)]
pub enum BenchMode {
    /// Execute natively on host.
    ExecuteHost,
    /// Execute the VM without generating a proof.
    Execute,
    /// Execute the VM with metering to get segments information.
    ExecuteMetered,
    /// Execute, generate trace, and check constraints and bus interactions without proving.
    ProveMock,
    /// Generate sequence of app proofs for continuation segments.
    ProveApp,
    /// Generate a full end-to-end STARK proof with aggregation.
    ProveStark,
    /// Generate a full end-to-end halo2 proof for EVM verifier.
    #[cfg(feature = "evm-verify")]
    ProveEvm,
    /// Generate input file only.
    MakeInput,
    /// Compile with apcs, no execution.
    Compile,
    /// Generate fixtures file for futher benchmarking.
    GenerateFixtures,
}

impl std::fmt::Display for BenchMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExecuteHost => write!(f, "execute_host"),
            Self::Execute => write!(f, "execute"),
            Self::ExecuteMetered => write!(f, "execute_metered"),
            Self::ProveMock => write!(f, "prove_mock"),
            Self::ProveApp => write!(f, "prove_app"),
            Self::ProveStark => write!(f, "prove_stark"),
            #[cfg(feature = "evm-verify")]
            Self::ProveEvm => write!(f, "prove_evm"),
            Self::MakeInput => write!(f, "make_input"),
            Self::Compile => write!(f, "compile"),
            Self::GenerateFixtures => write!(f, "generate_fixtures"),
        }
    }
}
/// The arguments for the host executable.
#[derive(Debug, Parser)]
pub struct HostArgs {
    /// The block number of the block to execute.
    #[clap(long)]
    block_number: u64,
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

    /// If specified, loads the app proving key from this path.
    #[arg(long)]
    pub app_pk_path: Option<PathBuf>,

    /// If specified, loads the agg proving key from this path.
    #[arg(long)]
    pub agg_pk_path: Option<PathBuf>,

    #[arg(long, default_value_t = false)]
    pub skip_comparison: bool,
}

pub fn reth_vm_config(app_log_blowup: usize) -> ExtendedVmConfig {
    let mut config = toml::from_str::<AppConfig<SdkVmConfig>>(include_str!(
        "../../../bin/client-eth/openvm.toml"
    ))
    .unwrap()
    .app_vm_config;
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
const PGO_BLOCK_NUMBERS: [u64; 1] = [23100006];
const APP_LOG_BLOWUP: usize = 1;

#[derive(Serialize, Deserialize)]
pub struct PrecomputedProverData {
    program: CompiledProgram,
    app_pk: AppProvingKey<SpecializedConfig>,
    agg_pk: AggProvingKey,
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
pub fn complete_args(mut args: HostArgs) -> HostArgs {
    let app_log_blowup = args.benchmark.app_log_blowup.unwrap_or(RETH_DEFAULT_APP_LOG_BLOWUP);
    assert_eq!(app_log_blowup, APP_LOG_BLOWUP, "App log blowup must be {RETH_DEFAULT_APP_LOG_BLOWUP} because it must match the one used when compiling this benchmark");
    args.benchmark.app_log_blowup = Some(app_log_blowup);
    let leaf_log_blowup = args.benchmark.leaf_log_blowup.unwrap_or(RETH_DEFAULT_LEAF_LOG_BLOWUP);
    args.benchmark.leaf_log_blowup = Some(leaf_log_blowup);

    args
}

/// Precompute the prover data, in particular the specialized config taking into account APCs, as
/// well as associated proving keys. If the data is already present in the cache, deserialize it and
/// return it.
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

    let start = std::time::Instant::now();
    if let Some(compiled_program) =
        File::open(&cache_file_path).ok().map(BufReader::new).map(|mut file| {
            bincode::serde::decode_from_std_read(&mut file, bincode::config::standard())
                .expect("Found cached precomputed prover data, but deserialization failed")
        })
    {
        tracing::info!("Precomputed prover data for key {} found in cache", args.apc_setup_name);
        println!(">>> Time to load precomputed prover data: {elapsed:?}");
        return Ok(compiled_program);
    }
    let elapsed = start.elapsed();


    tracing::info!(
        "Precomputed prover data for key {} not found in cache. Precomputing prover data.",
        args.apc_setup_name
    );

    let provider_config = args.provider.clone().into_provider().await?;

    let mut pgo_stdins = Vec::new();

    let start = std::time::Instant::now();
    for block_id in PGO_BLOCK_NUMBERS {
        let pgo_client_input =
            get_client_input(&provider_config, &args.cache_dir, PGO_CHAIN_ID, block_id)
                .await
                .unwrap();

        let mut pgo_stdin = StdIn::default();
        pgo_stdin.write(&pgo_client_input);
        pgo_stdins.push(pgo_stdin);
    }
    let elapsed = start.elapsed();
    println!(">>> Time to get_client_input for PGO blocks: {elapsed:?}");

    let app_log_blowup = args.benchmark.app_log_blowup.unwrap();

    let vm_config = reth_vm_config(app_log_blowup);
    let app_config = args.benchmark.app_config(vm_config.clone());

    let sdk: GenericSdk<BabyBearPoseidon2Engine, ExtendedVmConfigCpuBuilder, NativeCpuBuilder> =
        GenericSdk::new(app_config.clone())?
            .with_agg_config(args.benchmark.agg_config())
        .with_agg_tree_config(args.benchmark.agg_tree_config);
    let start = std::time::Instant::now();
    let elf = Elf::decode(openvm_client_eth_elf, MEM_SIZE as u32)?;
    let elapsed = start.elapsed();
    println!(">>> Time to decode elf: {elapsed:?}");
    let start = std::time::Instant::now();
    let exe = sdk.convert_to_exe(elf.clone())?;
    let elapsed = start.elapsed();
    println!(">>> Time to convert to exe: {elapsed:?}");

    let start = std::time::Instant::now();
    let program = powdr::apc(
        OriginalCompiledProgram { exe, vm_config },
        openvm_client_eth_elf,
        args.apc,
        args.apc_skip,
        args.pgo_type,
        pgo_stdins,
    );
    let elapsed = start.elapsed();
    println!(">>> Time to powdr::apc: {elapsed:?}");

    // Precompute proving keys
    let specialized_sdk: GenericSdk<
        BabyBearPoseidon2Engine,
        SpecializedConfigCpuBuilder,
        NativeCpuBuilder,
    > = GenericSdk::new(args.benchmark.app_config(program.vm_config.clone()))?
        .with_agg_config(args.benchmark.agg_config())
        .with_agg_tree_config(args.benchmark.agg_tree_config);

    let start = std::time::Instant::now();
    tracing::info!("Run app keygen");
    let (app_pk, _) = specialized_sdk.app_keygen();
    tracing::info!("Run agg keygen");
    let (agg_pk, _) = specialized_sdk.agg_keygen().unwrap();
    let elapsed = start.elapsed();
    println!(">>> Time to generate proving keys: {elapsed:?}");

    let setup = PrecomputedProverData { program, app_pk, agg_pk };

    tracing::info!("Saving prover data to cache at {}", cache_file_path.display());
    std::fs::create_dir_all(&args.apc_cache_dir).unwrap();
    let start = std::time::Instant::now();
    bincode::serde::encode_into_std_write(
        &setup,
        &mut BufWriter::new(File::create(cache_file_path).unwrap()),
        bincode::config::standard(),
    )
    .unwrap();
    let elapsed = start.elapsed();
    println!(">>> Time to save prover data: {elapsed:?}");

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
    let mut args = args;
    let provider_config = args.provider.into_provider().await?;

    match provider_config.chain_id {
        #[allow(non_snake_case)]
        CHAIN_ID_ETH_MAINNET => (),
        _ => {
            eyre::bail!("unknown chain ID: {}", provider_config.chain_id);
        }
    };

    let chain_id = provider_config.chain_id;

    let start = std::time::Instant::now();

    let client_input =
        get_client_input(&provider_config, &args.cache_dir, chain_id, args.block_number).await?;

    let elapsed = start.elapsed();
    println!(">>> Time to get client input: {elapsed:?}");

    let start = std::time::Instant::now();
    let mut stdin = StdIn::default();
    stdin.write(&client_input);
    info!("input loaded");
    let elapsed = start.elapsed();
    println!(">>> Time to load input: {elapsed:?}");

    if matches!(args.mode, BenchMode::MakeInput) {
        let start = std::time::Instant::now();
        let words: Vec<u32> = openvm::serde::to_vec(&client_input).unwrap();
        let bytes: Vec<u8> = words.into_iter().flat_map(|w| w.to_le_bytes()).collect();
        let hex_bytes = String::from("0x01") + &hex::encode(&bytes);
        let input = json!({
            "input": [hex_bytes]
        });
        let input = serde_json::to_string(&input).unwrap();
        fs::write(args.generated_input_path.unwrap(), input)?;
        let elapsed = start.elapsed();
        println!(">>> Time to make input: {elapsed:?}");
        return Ok(());
    }

    let app_log_blowup = args.benchmark.app_log_blowup.unwrap();

    let vm_config = reth_vm_config(app_log_blowup);
    let app_config = args.benchmark.app_config(vm_config.clone());

    let start = std::time::Instant::now();
    let elf = Elf::decode(openvm_client_eth_elf, MEM_SIZE as u32)?;
    let elapsed = start.elapsed();
    println!(">>> Time to elf decode: {elapsed:?}");


    let PrecomputedProverData { program: CompiledProgram { exe, vm_config }, app_pk, agg_pk } =
        setup;

    let start = std::time::Instant::now();
    // Create an SDK based on the `SpecializedConfig` we generated
    #[cfg(feature = "cuda")]
    let generic_sdk = PowdrSdkGpu::new(args.benchmark.app_config(vm_config.clone()))?;
    #[cfg(not(feature = "cuda"))]
    let generic_sdk = PowdrSdkCpu::new(args.benchmark.app_config(vm_config.clone()))?;
    let specialized_sdk = generic_sdk
        .with_agg_config(args.benchmark.agg_config())
        .with_agg_tree_config(args.benchmark.agg_tree_config);

    // Load the precomputed proving keys
    tracing::info!("Load app pk");
    specialized_sdk.set_app_pk(app_pk).map_err(|_| ()).unwrap();
    tracing::info!("Load agg pk");
    specialized_sdk.set_agg_pk(agg_pk).map_err(|_| ()).unwrap();

    let program_name = format!("reth.{}.block_{}", args.mode, args.block_number);
    // NOTE: args.benchmark.app_config resets SegmentationLimits if max_segment_length is set
    args.benchmark.max_segment_length = None;

    // `prover` can be called over both `elf` and `exe`.
    // We had a bug before where `prover(elf)` was called and silently didn't use any apcs.
    // So we drop `elf` here to make sure it's never used later.
    drop(elf);
    let elapsed = start.elapsed();
    println!(">>> Time to create SDK: {elapsed:?}");

    run_with_metric_collection("OUTPUT_PATH", || {
        info_span!("reth-block", block_number = args.block_number).in_scope(
            || -> eyre::Result<()> {
                // Run host execution for comparison
                if !args.skip_comparison {
                    let start = std::time::Instant::now();
                    let block_hash = info_span!("host.execute", group = program_name).in_scope(
                        || -> eyre::Result<_> {
                            let executor = ClientExecutor;
                            // Create a child span to get the group label propagated
                            let header = info_span!("client.execute")
                                .in_scope(|| executor.execute(client_input.clone()))?;
                            let block_hash =
                                info_span!("header.hash_slow").in_scope(|| header.hash_slow());
                            Ok(block_hash)
                        },
                    )?;
                    let elapsed = start.elapsed();
                    println!(">>> Time to host execute for comparison: {elapsed:?}");
                    println!("block_hash (execute-host): {}", ToHexExt::encode_hex(&block_hash));
                }

                // For ExecuteHost mode, only do host execution
                if matches!(args.mode, BenchMode::ExecuteHost) {
                    return Ok(());
                }

                // Execute for benchmarking:
                if !args.skip_comparison {
                    let start = std::time::Instant::now();
                    let pvs = info_span!("sdk.execute", group = program_name)
                        .in_scope(|| specialized_sdk.execute(exe.clone(), stdin.clone()))?;
                    let block_hash = pvs;
                    let elapsed = start.elapsed();
                    println!(">>> Time to sdk execute for comparison: {elapsed:?}");
                    println!("block_hash (execute): {}", ToHexExt::encode_hex(&block_hash));
                }

                match args.mode {
                    BenchMode::Compile => {
                        // This mode is used to compile the program with APCs, no execution.
                        println!("Compiled program with APCs");
                    }
                    BenchMode::Execute => {}
                    BenchMode::ExecuteMetered => {
                        let engine = DefaultStarkEngine::new(app_config.app_fri_params.fri_params);
                        let (vm, _) = VirtualMachine::new_with_keygen(
                            engine,
                            #[cfg(feature = "cuda")]
                            ExtendedVmConfigGpuBuilder,
                            #[cfg(not(feature = "cuda"))]
                            ExtendedVmConfigCpuBuilder,
                            app_config.app_vm_config,
                        )?;
                        let executor_idx_to_air_idx = vm.executor_idx_to_air_idx();
                        let interpreter =
                            vm.executor().metered_instance(&exe, &executor_idx_to_air_idx)?;
                        let metered_ctx = vm.build_metered_ctx(&exe);
                        let (segments, _) =
                            info_span!("interpreter.execute_metered", group = program_name)
                                .in_scope(|| interpreter.execute_metered(stdin, metered_ctx))?;
                        println!("Number of segments: {}", segments.len());
                    }
                    BenchMode::ProveMock => {
                        // Build owned vm instance, so we can mutate it later
                        let vm_builder = specialized_sdk.app_vm_builder().clone();
                        let vm_pk = specialized_sdk.app_pk().app_vm_pk.clone();
                        let exe = specialized_sdk.convert_to_exe(exe.clone())?;
                        let mut vm_instance: VmInstance<_, _> = new_local_prover(vm_builder, &vm_pk, exe.clone())?;
                
                        vm_instance.reset_state(stdin.clone());
                        let metered_ctx = vm_instance.vm.build_metered_ctx(&exe);
                        let metered_interpreter = vm_instance.vm.metered_interpreter(vm_instance.exe())?;
                        let (segments, _) = metered_interpreter.execute_metered(stdin.clone(), metered_ctx)?;
                        let mut state = vm_instance.state_mut().take();
                
                        // Get reusable inputs for `debug_proving_ctx`, the mock prover API from OVM.
                        let vm = &mut vm_instance.vm;
                        let air_inv = vm.config().create_airs().unwrap();
                        #[cfg(feature = "cuda")]
                        let pk = air_inv.keygen::<GpuBabyBearPoseidon2Engine>(&vm.engine);
                        #[cfg(not(feature = "cuda"))]
                        let pk = air_inv.keygen::<BabyBearPoseidon2Engine>(&vm.engine);
                
                        for (seg_idx, segment) in segments.into_iter().enumerate() {
                            let _segment_span = info_span!("prove_segment", segment = seg_idx).entered();
                            // We need a separate span so the metric label includes "segment" from _segment_span
                            let _prove_span = info_span!("total_proof").entered();
                            let Segment {
                                instret_start,
                                num_insns,
                                trace_heights,
                            } = segment;
                            assert_eq!(state.as_ref().unwrap().instret(), instret_start);
                            let from_state = Option::take(&mut state).unwrap();
                            vm.transport_init_memory_to_device(&from_state.memory);
                            let PreflightExecutionOutput {
                                system_records,
                                record_arenas,
                                to_state,
                            } = vm.execute_preflight(
                                &mut vm_instance.interpreter,
                                from_state,
                                Some(num_insns),
                                &trace_heights,
                            )?;
                            state = Some(to_state);
                
                            // Generate proving context for each segment
                            let ctx = vm.generate_proving_ctx(system_records, record_arenas)?;
                
                            // Run the mock prover for each segment
                            debug_proving_ctx(vm, &pk, &ctx);
                        }
                    }
                    BenchMode::ProveApp => {
                        let mut prover =
                            specialized_sdk.app_prover(exe)?.with_program_name(program_name);
                        let (_, app_vk) = specialized_sdk.app_keygen();
                        let proof = prover.prove(stdin)?;
                        verify_app_proof(&app_vk, &proof)?;
                    }
                    BenchMode::ProveStark => {
                        let start = std::time::Instant::now();
                        let mut prover =
                            specialized_sdk.prover(exe)?.with_program_name(program_name);
                        let proof = prover.prove(stdin)?;
                        let elapsed = start.elapsed();
                        println!(">>> Time to sdk prove: {elapsed:?}");
                        let block_hash = proof
                            .user_public_values
                            .iter()
                            .map(|pv| pv.as_canonical_u32() as u8)
                            .collect::<Vec<u8>>();
                        println!("block_hash (prove_stark): {}", ToHexExt::encode_hex(&block_hash));

                        if let Some(state) = prover.app_prover.instance().state() {
                            info!("state instret: {}", state.instret());
                            if let Some(output_dir) = args.output_dir.as_ref() {
                                fs::write(
                                    output_dir.join("num_instret"),
                                    state.instret().to_string(),
                                )?;
                                info!("wrote state instret to {}", output_dir.display());
                            }
                        }

                        if let Some(output_dir) = args.output_dir.as_ref() {
                            let start = std::time::Instant::now();
                            let versioned_proof = VersionedVmStarkProof::new(proof)?;
                            let json = serde_json::to_vec_pretty(&versioned_proof)?;
                            fs::write(output_dir.join("proof.json"), json)?;
                            let elapsed = start.elapsed();
                            println!(">>> Time to write proof: {elapsed:?}");
                            println!("wrote proof json to {}", output_dir.display());
                        }
                    }
                    #[cfg(feature = "evm-verify")]
                    BenchMode::ProveEvm => {
                        let mut prover =
                            specialized_sdk.evm_prover(exe)?.with_program_name(program_name);
                        let halo2_pk = specialized_sdk.halo2_pk();
                        tracing::info!(
                            "halo2_outer_k: {}",
                            halo2_pk.verifier.pinning.metadata.config_params.k
                        );
                        tracing::info!(
                            "halo2_wrapper_k: {}",
                            halo2_pk.wrapper.pinning.metadata.config_params.k
                        );
                        let proof = prover.prove_evm(stdin)?;
                        let block_hash = &proof.user_public_values;
                        println!("block_hash (prove_evm): {}", ToHexExt::encode_hex(block_hash));
                    }
                    BenchMode::GenerateFixtures => {
                        let mut prover =
                            specialized_sdk.prover(exe)?.with_program_name(program_name);
                        let app_proof = prover.app_prover.prove(stdin)?;
                        let leaf_proofs = prover.agg_prover.generate_leaf_proofs(&app_proof)?;
                        let fixture_path = args.fixtures_path.unwrap();

                        let mut app_proof_path = fixture_path.clone();
                        app_proof_path.push("app_proof.bitcode");
                        fs::write(app_proof_path, bitcode::serialize(&app_proof)?)?;

                        let mut leaf_proofs_path = fixture_path.clone();
                        leaf_proofs_path.push("leaf_proofs.bitcode");
                        fs::write(leaf_proofs_path, bitcode::serialize(&leaf_proofs)?)?;

                        let mut app_pk_path = fixture_path.clone();
                        app_pk_path.push("app_pk.bitcode");
                        fs::write(app_pk_path, bitcode::serialize(specialized_sdk.app_pk())?)?;

                        let mut agg_pk_path = fixture_path.clone();
                        agg_pk_path.push("agg_pk.bitcode");
                        fs::write(agg_pk_path, bitcode::serialize(specialized_sdk.agg_pk())?)?;
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
        let cache_path = cache_dir.join(format!("input/{}/{}.bin", chain_id, block_number));

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
    use openvm_native_circuit::NativeCpuBuilder;
    use openvm_sdk::{
        config::{AppConfig, DEFAULT_APP_LOG_BLOWUP},
        GenericSdk, StdIn,
    };
    use openvm_stark_sdk::config::FriParameters;
    use powdr_autoprecompiles::{execution_profile::execution_profile, PgoType};
    use powdr_openvm::{
        compile_exe_with_elf, default_powdr_openvm_config, BabyBearOpenVmApcAdapter,
        CompiledProgram, DegreeBound, ExtendedVmConfigCpuBuilder, OriginalCompiledProgram,
        PgoConfig, Prog,
    };

    /// This function is used to generate the specialized program for the Powdr APC.
    /// It takes:
    /// - `original_program`: The original program, including the original vm config.
    /// - `elf`: The original ELF file, used to detect the basic blocks.
    /// - `apc`: The number of apcs to generate
    /// - `apc_skip`: The number of apcs to skip when selecting. Used for debugging.
    /// - `pgo_type`: The PGO strategy to use when choosing the blocks to accelerate.
    /// - `pgo_stdin`: The standard inputs to the program used for PGO data generation to choose
    ///   which basic blocks to accelerate.
    pub fn apc(
        original_program: OriginalCompiledProgram,
        elf: &[u8],
        apc: usize,
        apc_skip: usize,
        pgo_type: PgoType,
        pgo_stdin: Vec<StdIn>,
    ) -> CompiledProgram {
        // Set app configuration
        let app_fri_params =
            FriParameters::standard_with_100_bits_conjectured_security(DEFAULT_APP_LOG_BLOWUP);
        let app_config = AppConfig::new(app_fri_params, original_program.vm_config.clone());

        use openvm_stark_sdk::config::baby_bear_poseidon2::BabyBearPoseidon2Engine;

        // prepare for execute
        let sdk: GenericSdk<BabyBearPoseidon2Engine, ExtendedVmConfigCpuBuilder, NativeCpuBuilder> =
            GenericSdk::new(app_config).unwrap();

        let execute = || {
            for stdin in pgo_stdin {
                sdk.execute(original_program.exe.clone(), stdin).unwrap();
            }
        };

        let program = Prog::from(&original_program.exe.program);

        let pgo_config = match pgo_type {
            PgoType::None => PgoConfig::None,
            PgoType::Instruction => PgoConfig::Instruction(execution_profile::<
                BabyBearOpenVmApcAdapter,
            >(&program, execute)),
            PgoType::Cell => PgoConfig::Cell(
                execution_profile::<BabyBearOpenVmApcAdapter>(&program, execute),
                None, // max total columns
            ),
        };

        let mut config = default_powdr_openvm_config(apc as u64, apc_skip as u64);

        config.degree_bound = DegreeBound { identities: 3, bus_interactions: 2 };

        if let Ok(path) = std::env::var("POWDR_APC_CANDIDATES_DIR") {
            config = config.with_apc_candidates_dir(path);
        }

        compile_exe_with_elf(
            original_program,
            elf,
            config,
            pgo_config,
        )
        .unwrap()
    }
}

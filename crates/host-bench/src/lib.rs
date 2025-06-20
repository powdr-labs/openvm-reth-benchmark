use alloy_provider::RootProvider;
use alloy_rpc_client::RpcClient;
use alloy_transport::layers::RetryBackoffLayer;
use clap::Parser;
use openvm_algebra_circuit::{Fp2Extension, ModularExtension};
use openvm_benchmarks_prove::util::BenchmarkCli;
use openvm_bigint_circuit::Int256;
use openvm_circuit::{
    arch::{instructions::exe::VmExe, SegmentationStrategy, SystemConfig, VmConfig, VmExecutor},
    openvm_stark_sdk::{
        bench::run_with_metric_collection, config::baby_bear_poseidon2::BabyBearPoseidon2Config,
        openvm_stark_backend::p3_field::PrimeField32, p3_baby_bear::BabyBear,
    },
};
use openvm_client_executor::{
    io::ClientExecutorInput, ChainVariant, CHAIN_ID_ETH_MAINNET, CHAIN_ID_LINEA_MAINNET,
    CHAIN_ID_OP_MAINNET,
};
use openvm_ecc_circuit::{WeierstrassExtension, SECP256K1_CONFIG};
use openvm_host_executor::HostExecutor;
use openvm_native_recursion::halo2::utils::CacheHalo2ParamsReader;
use openvm_pairing_circuit::{PairingCurve, PairingExtension};
use openvm_rv32im_circuit::Rv32M;
use openvm_sdk::{
    config::SdkVmConfig,
    keygen::AggStarkProvingKey,
    prover::{AppProver, EvmHalo2Prover, StarkProver},
    DefaultStaticVerifierPvHandler, GenericSdk, StdIn, SC,
};
use openvm_stark_sdk::engine::StarkFriEngine;
use openvm_transpiler::{elf::Elf, openvm_platform::memory::MEM_SIZE, FromElf};
pub use reth_primitives;
use reth_primitives::hex::ToHexExt;
use serde_json::json;
use std::{fs, path::PathBuf, sync::Arc};
use tracing::info_span;

mod execute;

mod cli;
use cli::ProviderArgs;

/// Enum representing the execution mode of the host executable.
#[derive(Debug, Clone, clap::ValueEnum)]
pub enum BenchMode {
    /// Execute the VM without generating a proof.
    Execute,
    /// Generate trace data without proving.
    Tracegen,
    /// Generate sequence of app proofs for continuation segments.
    ProveApp,
    /// Generate a full end-to-end STARK proof with aggregation.
    ProveStark,
    /// Generate a full end-to-end halo2 proof for EVM verifier.
    ProveEvm,
    /// Generate input file only.
    MakeInput,
}

impl std::fmt::Display for BenchMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Execute => write!(f, "execute"),
            Self::Tracegen => write!(f, "tracegen"),
            Self::ProveApp => write!(f, "prove_app"),
            Self::ProveStark => write!(f, "prove_stark"),
            Self::ProveEvm => write!(f, "prove_evm"),
            Self::MakeInput => write!(f, "make_input"),
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
    /// The path to the CSV file containing the execution data.
    #[clap(long, default_value = "report.csv")]
    report_path: PathBuf,

    #[clap(flatten)]
    benchmark: BenchmarkCli,

    /// Max cells per chip in segment for continuations
    #[arg(long)]
    pub segment_max_cells: Option<usize>,

    #[arg(long)]
    pub no_kzg_intrinsics: bool,

    /// Optional path to write the input to. Only needed for mode=make_input
    #[arg(long)]
    pub input_path: Option<PathBuf>,
}

/// Segments based on total trace cells across all chips
#[derive(Debug, Clone, Copy)]
pub struct TraceSizeSegmentationStrategy {
    max_height: usize,
    max_cells: usize,
}

impl TraceSizeSegmentationStrategy {
    pub fn new(max_height: usize, max_cells: usize) -> Self {
        assert!(max_height < (1 << 27));
        Self { max_height, max_cells }
    }
}

impl SegmentationStrategy for TraceSizeSegmentationStrategy {
    fn should_segment(
        &self,
        air_names: &[String],
        trace_heights: &[usize],
        trace_cells: &[usize],
    ) -> bool {
        for (i, &height) in trace_heights.iter().enumerate() {
            if height > self.max_height {
                tracing::info!(
                    "Should segment because chip {i} (name: {}) has height {height} > {}",
                    air_names[i],
                    self.max_height
                );
                return true;
            }
        }
        let total_cells: usize = trace_cells.iter().sum();
        if total_cells > self.max_cells {
            tracing::info!(
                "Should segment because total trace cells = {total_cells} > {}",
                self.max_cells
            );
            return true;
        }
        false
    }

    fn stricter_strategy(&self) -> Arc<dyn SegmentationStrategy> {
        Arc::new(Self { max_height: self.max_height / 2, max_cells: self.max_cells / 2 })
    }
}

pub fn reth_vm_config(
    app_log_blowup: usize,
    segment_max_height: usize,
    segment_max_cells: usize,
    use_kzg_intrinsics: bool,
) -> SdkVmConfig {
    let mut system_config = SystemConfig::default()
        .with_continuations()
        .with_max_constraint_degree((1 << app_log_blowup) + 1)
        .with_public_values(32);
    system_config.set_segmentation_strategy(Arc::new(TraceSizeSegmentationStrategy::new(
        segment_max_height,
        segment_max_cells,
    )));
    let int256 = Int256::default();
    let bn_config = PairingCurve::Bn254.curve_config();
    let bls_config = PairingCurve::Bls12_381.curve_config();
    // The builder will do this automatically, but we set it just in case.
    let rv32m = Rv32M { range_tuple_checker_sizes: int256.range_tuple_checker_sizes };
    let mut supported_moduli = vec![
        bn_config.modulus.clone(),
        bn_config.scalar.clone(),
        SECP256K1_CONFIG.modulus.clone(),
        SECP256K1_CONFIG.scalar.clone(),
    ];
    let mut supported_complex_moduli = vec![bn_config.modulus.clone()];
    let mut supported_curves = vec![bn_config.clone(), SECP256K1_CONFIG.clone()];
    let mut supported_pairing_curves = vec![PairingCurve::Bn254];
    if use_kzg_intrinsics {
        supported_moduli.push(bls_config.modulus.clone());
        supported_moduli.push(bls_config.scalar.clone());
        supported_complex_moduli.push(bls_config.modulus.clone());
        supported_curves.push(bls_config.clone());
        supported_pairing_curves.push(PairingCurve::Bls12_381);
    }
    SdkVmConfig::builder()
        .system(system_config.into())
        .rv32i(Default::default())
        .rv32m(rv32m)
        .io(Default::default())
        .keccak(Default::default())
        .sha256(Default::default())
        .bigint(int256)
        .modular(ModularExtension::new(supported_moduli))
        .fp2(Fp2Extension::new(supported_complex_moduli))
        .ecc(WeierstrassExtension::new(supported_curves))
        .pairing(PairingExtension::new(supported_pairing_curves))
        .build()
}

pub const RETH_DEFAULT_APP_LOG_BLOWUP: usize = 1;
pub const RETH_DEFAULT_LEAF_LOG_BLOWUP: usize = 1;

#[tokio::main]
pub async fn run_reth_benchmark<E: StarkFriEngine<SC>>(
    args: HostArgs,
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

    let variant = match provider_config.chain_id {
        CHAIN_ID_ETH_MAINNET => ChainVariant::Ethereum,
        CHAIN_ID_OP_MAINNET => ChainVariant::Optimism,
        CHAIN_ID_LINEA_MAINNET => ChainVariant::Linea,
        _ => {
            eyre::bail!("unknown chain ID: {}", provider_config.chain_id);
        }
    };

    let client_input_from_cache = try_load_input_from_cache(
        args.cache_dir.as_ref(),
        provider_config.chain_id,
        args.block_number,
    )?;

    let client_input = match (client_input_from_cache, provider_config.rpc_url) {
        (Some(client_input_from_cache), _) => client_input_from_cache,
        (None, Some(rpc_url)) => {
            // Cache not found but we have RPC
            // Setup the provider.
            let client =
                RpcClient::builder().layer(RetryBackoffLayer::new(5, 1000, 100)).http(rpc_url);
            let provider = RootProvider::new(client);

            // Setup the host executor.
            let host_executor = HostExecutor::new(provider);

            // Execute the host.
            let client_input = host_executor
                .execute(args.block_number, variant)
                .await
                .expect("failed to execute host");

            if let Some(cache_dir) = args.cache_dir {
                let input_folder = cache_dir.join(format!("input/{}", provider_config.chain_id));
                if !input_folder.exists() {
                    std::fs::create_dir_all(&input_folder)?;
                }

                let input_path = input_folder.join(format!("{}.bin", args.block_number));
                let mut cache_file = std::fs::File::create(input_path)?;

                bincode::serde::encode_into_std_write(
                    &client_input,
                    &mut cache_file,
                    bincode::config::standard(),
                )?;
            }

            client_input
        }
        (None, None) => {
            eyre::bail!("cache not found and RPC URL not provided")
        }
    };

    let mut stdin = StdIn::default();
    stdin.write(&client_input);

    if matches!(args.mode, BenchMode::MakeInput) {
        let words: Vec<u32> = openvm::serde::to_vec(&client_input).unwrap();
        let bytes: Vec<u8> = words.into_iter().flat_map(|w| w.to_le_bytes()).collect();
        let hex_bytes = String::from("0x01") + &hex::encode(&bytes);
        let input = json!({
            "input": [hex_bytes]
        });
        let input = serde_json::to_string(&input).unwrap();
        fs::write(args.input_path.unwrap(), input)?;
        return Ok(());
    }

    let app_log_blowup = args.benchmark.app_log_blowup.unwrap_or(RETH_DEFAULT_APP_LOG_BLOWUP);
    args.benchmark.app_log_blowup = Some(app_log_blowup);
    let segment_max_height = args.benchmark.max_segment_length.unwrap_or((1 << 23) - 100);
    let segment_max_cells = args.segment_max_cells.unwrap_or(u32::MAX as usize); // 2^32 u32's = 16gb
    let leaf_log_blowup = args.benchmark.leaf_log_blowup.unwrap_or(RETH_DEFAULT_LEAF_LOG_BLOWUP);
    args.benchmark.leaf_log_blowup = Some(leaf_log_blowup);

    let vm_config = reth_vm_config(
        app_log_blowup,
        segment_max_height,
        segment_max_cells,
        !args.no_kzg_intrinsics,
    );
    let mut sdk = GenericSdk::<E>::new();
    sdk.set_agg_tree_config(args.benchmark.agg_tree_config);
    let elf = Elf::decode(openvm_client_eth_elf, MEM_SIZE as u32)?;
    let exe = VmExe::from_elf(elf, vm_config.transpiler()).unwrap();

    let powdr_openvm::CompiledProgram { exe, vm_config } =
        powdr::apc(exe, vm_config, openvm_client_eth_elf, stdin.clone());

    let program_name = format!("reth.{}.block_{}", args.mode, args.block_number);
    // NOTE: args.benchmark.app_config resets SegmentationStrategy if max_segment_length is set
    args.benchmark.max_segment_length = None;
    let app_config = args.benchmark.app_config(vm_config.clone());

    run_with_metric_collection("OUTPUT_PATH", || {
        info_span!("reth-block", block_number = args.block_number).in_scope(
            || -> eyre::Result<()> {
                match args.mode {
                    BenchMode::Execute => {
                        let pvs = info_span!("execute", group = program_name)
                            .in_scope(|| sdk.execute(exe, app_config.app_vm_config, stdin))?;
                        let block_hash: Vec<u8> = pvs
                            .iter()
                            .map(|x| x.as_canonical_u32().try_into().unwrap())
                            .collect::<Vec<_>>();
                        println!("block_hash: {}", ToHexExt::encode_hex(&block_hash));
                    }
                    BenchMode::Tracegen => {
                        let executor = VmExecutor::<_, _>::new(app_config.app_vm_config);
                        info_span!("tracegen", group = program_name).in_scope(|| {
                            executor.execute_and_generate::<BabyBearPoseidon2Config>(exe, stdin)
                        })?;
                    }
                    BenchMode::ProveApp => {
                        let app_pk = sdk.app_keygen(app_config)?;
                        let app_committed_exe = sdk.commit_app_exe(app_pk.app_fri_params(), exe)?;

                        let app_prover =
                            AppProver::<_, E>::new(app_pk.app_vm_pk.clone(), app_committed_exe)
                                .with_program_name(program_name);
                        let proof = app_prover.generate_app_proof(stdin);
                        let app_vk = app_pk.get_app_vk();
                        sdk.verify_app_proof(&app_vk, &proof)?;
                    }
                    BenchMode::ProveStark => {
                        // TODO: update this to use the new generate_stark_proof API once v1.2.1 is
                        // released
                        let app_pk = sdk.app_keygen(app_config)?;
                        let app_committed_exe = sdk.commit_app_exe(app_pk.app_fri_params(), exe)?;
                        let agg_stark_config = args.benchmark.agg_config().agg_stark_config;
                        let agg_stark_pk = AggStarkProvingKey::keygen(agg_stark_config);
                        let mut prover = StarkProver::<_, E>::new(
                            Arc::new(app_pk),
                            app_committed_exe,
                            agg_stark_pk,
                            args.benchmark.agg_tree_config,
                        );
                        prover.set_program_name(program_name);
                        let proof = prover.generate_root_verifier_input(stdin);
                        let block_hash = proof
                            .public_values
                            .iter()
                            .map(|pv| pv.as_canonical_u32() as u8)
                            .collect::<Vec<u8>>();
                        println!("block_hash: {}", ToHexExt::encode_hex(&block_hash));
                    }
                    BenchMode::ProveEvm => {
                        let halo2_params_reader = CacheHalo2ParamsReader::new(
                            args.benchmark
                                .kzg_params_dir
                                .as_ref()
                                .expect("must set --kzg-params-dir"),
                        );
                        let mut agg_config = args.benchmark.agg_config();
                        agg_config.agg_stark_config.max_num_user_public_values =
                            VmConfig::<BabyBear>::system(&vm_config).num_public_values;

                        let app_pk = sdk.app_keygen(app_config)?;
                        let full_agg_pk = sdk.agg_keygen(
                            agg_config,
                            &halo2_params_reader,
                            &DefaultStaticVerifierPvHandler,
                        )?;
                        tracing::info!(
                            "halo2_outer_k: {}",
                            full_agg_pk.halo2_pk.verifier.pinning.metadata.config_params.k
                        );
                        tracing::info!(
                            "halo2_wrapper_k: {}",
                            full_agg_pk.halo2_pk.wrapper.pinning.metadata.config_params.k
                        );
                        let app_committed_exe = sdk.commit_app_exe(app_pk.app_fri_params(), exe)?;

                        let mut prover = EvmHalo2Prover::<_, E>::new(
                            &halo2_params_reader,
                            Arc::new(app_pk),
                            app_committed_exe,
                            full_agg_pk,
                            args.benchmark.agg_tree_config,
                        );
                        prover.set_program_name(program_name);
                        let evm_proof = prover.generate_proof_for_evm(stdin);
                        let block_hash = &evm_proof.user_public_values;
                        println!("block_hash: {}", ToHexExt::encode_hex(block_hash));
                    }
                    BenchMode::MakeInput => {
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
    type F = crate::BabyBear;
    use openvm_circuit::arch::instructions::exe::VmExe;
    use openvm_sdk::{config::SdkVmConfig, StdIn};
    use powdr_openvm::{
        customize, export_pil, instructions_to_airs, pgo, BusMap, BusType, CompiledProgram,
        OriginalCompiledProgram, PowdrConfig, SpecializedConfig,
    };
    use powdr_riscv_elf::load_elf_from_buffer;

    /// This function is used to generate the specialized program for the Powdr APC.
    /// It takes:
    /// - `exe`: The original transpiled OpenVM executable.
    /// - `vm_config`: The base VM configuration the executable relates to.
    /// - `elf`: The original ELF file, used to detect the basic blocks.
    /// - `stdin`: The standard input to the program, used for PGO data generation to choose which basic blocks to accelerate.
    pub fn apc(
        exe: VmExe<F>,
        vm_config: SdkVmConfig,
        elf: &[u8],
        stdin: StdIn,
    ) -> CompiledProgram<F> {
        let og = OriginalCompiledProgram { exe: exe.clone(), sdk_vm_config: vm_config.clone() };

        let pgo_data = pgo(og, stdin.clone()).unwrap();

        let bus_map =
            BusMap::openvm_base().with_sha(7).with_bus_type(8, BusType::TupleRangeChecker);

        let powdr_config = PowdrConfig::new(1, 0).with_bus_map(bus_map);

        let elf_powdr = load_elf_from_buffer(elf);

        let airs =
            instructions_to_airs::<_, powdr_number::BabyBearField>(exe.clone(), vm_config.clone());

        let (exe, extension) = customize(
            exe,
            vm_config.clone(),
            &elf_powdr.text_labels,
            &airs,
            powdr_config.clone(),
            Some(pgo_data),
        );
        // Generate the custom config based on the generated instructions
        let vm_config = SpecializedConfig::from_base_and_extension(vm_config, extension);
        export_pil(vm_config.clone(), "debug.pil", 1000, &powdr_config.bus_map);
        CompiledProgram { exe, vm_config }
    }
}

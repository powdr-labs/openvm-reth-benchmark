#[cfg(target_os = "zkvm")]
use openvm::io::{println, read, reveal_bytes32};

use openvm_client_executor::{io::ClientExecutorInput, ChainVariant, ClientExecutor};

#[cfg(all(target_os = "zkvm", feature = "precompiles"))]
openvm::init!();

#[cfg(all(target_arch = "wasm32", feature = "no-precompiles"))]
fn custom_getrandom(buf: &mut [u8]) -> Result<(), getrandom::Error> {
    for byte in buf.iter_mut() {
        *byte = 4;
    }
    Ok(())
}

#[cfg(all(target_arch = "wasm32", feature = "no-precompiles"))]
getrandom::register_custom_getrandom!(custom_getrandom);

pub fn main() {
    #[cfg(target_os = "zkvm")]
    {
        println("client-eth starting");
        // Read the input.
        let input: ClientExecutorInput = read();
        println("finished reading input");

        // Execute the block (crypto is installed inside executor).
        let executor = ClientExecutor;
        let header =
            executor.execute(ChainVariant::Mainnet, input).expect("failed to execute client");
        let block_hash = header.hash_slow();

        // Reveal the block hash.
        reveal_bytes32(*block_hash);
    }

    #[cfg(target_arch = "wasm32")]
    {
        womir_guest_io::debug_print("1: reading bytes");
        let bytes = womir_guest_io::read_bytes();
        womir_guest_io::debug_print("2: deserializing");
        let (input, _): (ClientExecutorInput, usize) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
        drop(bytes);
        womir_guest_io::debug_print("3: executing");
        let executor = ClientExecutor;
        let header =
            executor.execute(ChainVariant::Mainnet, input).expect("failed to execute client");
        womir_guest_io::debug_print("4: hashing");
        let block_hash = header.hash_slow();
        womir_guest_io::debug_print(&format!("{block_hash:#x}"));
    }
}

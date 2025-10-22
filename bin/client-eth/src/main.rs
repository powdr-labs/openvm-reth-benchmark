// use openvm::io::{println, read, reveal_bytes32};
use getrandom::{register_custom_getrandom, Error};
use openvm_client_executor::{io::ClientExecutorInput, ClientExecutor};

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    pub safe fn __hint_input();
    pub unsafe fn __hint_buffer(ptr: *mut u8, num_words: u32);
    pub unsafe fn __debug_print(ptr: *const u8, num_bytes: u32);
}

pub fn read_vec() -> Vec<u8> {
    println("inside read_vec\n");
    __hint_input();
    println("after hint input\n");
    let len = read_word();
    println(&format!("read len {len}\n"));
    read_vec_by_len(len as usize)
}

// len in bytes
pub fn read_vec_by_len(len: usize) -> Vec<u8> {
    println("inside read_vec_by_len\n");
    let num_words = len.div_ceil(4);
    let capacity = num_words * 4;

    let mut bytes: Vec<u8> = Vec::with_capacity(capacity);
    println("after allocating bytes\n");
    unsafe { __hint_buffer(bytes.as_mut_ptr(), num_words as u32) }
    println("after hint_buffer\n");
    // SAFETY: We populate a `Vec<u8>` by hintstore-ing `num_words` 4 byte words. We set the
    // length to `len` and don't care about the extra `capacity - len` bytes stored.
    unsafe {
        bytes.set_len(len);
    }
    println("after set_len\n");
    bytes
}

pub fn read_word() -> u32 {
    let mut bytes = [0u8; 4];
    unsafe { __hint_buffer(bytes.as_mut_ptr(), 1) }
    u32::from_le_bytes(bytes)
}

pub fn println(s: &str) {
    unsafe {
        __debug_print(s.as_ptr(), s.len() as u32);
    }
}

use bincode::{config, serde::decode_from_slice};
use serde::de::DeserializeOwned;

fn read_t<T: DeserializeOwned>() -> T {
    // let len = read_data_len();
    // println(&format!("syscall len: {len}"));
    // let mut bytes: Vec<u8> = Vec::with_capacity(len as usize);
    // unsafe {
    //     bytes.set_len(len as usize);
    //     read_data(bytes.as_mut_ptr());
    // }
    let bytes = read_vec();
    println(&format!("size: {}\n", bytes.len()));
    let cfg = config::standard();
    let (value, _len): (T, usize) = decode_from_slice(&bytes, cfg).unwrap();
    value
}

// fn read_bytes() -> Vec<u8> {
//     let len = read_data_len();
//     println(&format!("syscall len: {len}"));
//     let mut bytes: Vec<u8> = vec![0; len as usize];
//     println(&format!("bytes len: {}", bytes.len()));
//     unsafe {
//         read_data(bytes.as_mut_ptr());
//     }
//     bytes
// }

// Imports needed by the linker, but clippy can't tell:
#[allow(unused_imports, clippy::single_component_path_imports)]
// use {
//     k256::Secp256k1Point,
//     openvm_algebra_guest::IntMod,
//     openvm_pairing::{bls12_381::Bls12_381G1Affine, bn254::Bn254G1Affine},
// };

// openvm::init!();

pub fn main() {
    println("client-eth starting\n");
    // Read the input.
    // TODO check how tihs was serialized.
    let input: ClientExecutorInput = read_t();
    // let input: Vec<u8> = read_bytes();
    println("finished reading input\n");

    // println(&format!("len: {}", input.len()));
    // assert_eq!(input.len(), 8);
    // println("assert 1");
    //
    // assert_eq!("abcdefg\n", std::str::from_utf8(&input).unwrap());
    // println("assert 2");
    // Execute the block.
    // let executor = ClientExecutor;
    // println("start executing");
    // let header = executor.execute(input).expect("failed to execute client");
    // println("finished executing");
    // let block_hash = header.hash_slow();

    // Reveal the block hash.
    // reveal_bytes32(*block_hash);
}

// Implementation taken from here: https://xkcd.com/221/
fn random_source(buf: &mut [u8]) -> Result<(), Error> {
    for byte in buf.iter_mut() {
        *byte = 4; // Chosen by fair dice roll. Guaranteed to be random.
    }
    Ok(())
}
register_custom_getrandom!(random_source);

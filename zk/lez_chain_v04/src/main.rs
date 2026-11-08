// The v0.4 LEZ chain guest. Input (stdin, lez_chain_v04::encode_stream): a
// borsh `lez_chain_v04::ChainHead`, then the blocks one by one, each read
// into the same buffer. Journal: lez_chain_v04::journal after the blocks. A
// block that does not decode, hash or link panics, and a panicking guest has
// no receipt.
#![no_main]

use std::io::Read;

use lez_chain_v04 as v04;
use risc0_zkvm::guest::env;

risc0_zkvm::guest::entry!(main);

fn read_u32(r: &mut impl Read) -> usize {
    let mut n = [0u8; 4];
    r.read_exact(&mut n).expect("a length");
    u32::from_le_bytes(n) as usize
}

fn main() {
    let mut stdin = env::stdin();
    let mut buf = vec![0u8; read_u32(&mut stdin)];
    stdin.read_exact(&mut buf).expect("the head");
    let head: v04::ChainHead = borsh::from_slice(&buf).expect("a borsh ChainHead");
    let mut state = v04::ChainState::at_anchor(head.anchor_id, head.anchor_hash);
    for _ in 0..head.blocks {
        let n = read_u32(&mut stdin);
        buf.resize(n, 0);
        stdin.read_exact(&mut buf).expect("a block");
        v04::apply_block(&mut state, &head.params, &buf).expect("a block that extends the tip");
    }
    env::commit_slice(&v04::journal(&head.self_image_id, &head.params, &state));
}

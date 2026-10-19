// The LEZ chain guest. Input (stdin, lez_chain::encode_stream): a borsh
// `lez_chain::ChainHead`, then the blocks one by one, each read into the same
// buffer. If the head names a prior journal, that journal must be a receipt
// of the image id the head names as this guest's own (`env::verify`, resolved
// when the receipt is compressed), and of the same deployment and anchor.
// Journal: the 1,120-byte `lez_chain::journal` after the blocks. A block that
// does not decode, hash or link panics, and a panicking guest has no receipt.
#![no_main]

use std::io::Read;

use risc0_zkvm::{guest::env, sha::Digest};

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
    let head: lez_chain::ChainHead = borsh::from_slice(&buf).expect("a borsh ChainHead");
    if let Some(prior) = &head.prior {
        env::verify(Digest::from(head.self_image_id), prior).expect("the prior journal's receipt");
    }
    let mut state = lez_chain::start(&head).expect("a prior of this chain");
    for _ in 0..head.blocks {
        let n = read_u32(&mut stdin);
        buf.resize(n, 0);
        stdin.read_exact(&mut buf).expect("a block");
        lez_chain::apply_block(&mut state, &head.params, &buf).expect("a block that extends the tip");
    }
    env::commit_slice(&lez_chain::journal(&head.self_image_id, &head.params, &state));
}

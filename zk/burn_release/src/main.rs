// The release guest over a chain-derived burn tree. Input: a borsh
// `(wrap_core::zk::ZkReleaseInput, Vec<u8>)`: the release's context and the
// burn's opening, and a journal of the LEZ chain guest. That journal's
// receipt is verified under the image id it names for itself (word 1), which
// the journal here repeats and Ethereum pins. Journal: the release preimage
// (whose sha256 is WrapVault.releaseDigest), then lez_chain::release_tail.
#![no_main]

use risc0_zkvm::{guest::env, sha::Digest};

risc0_zkvm::guest::entry!(main);

fn main() {
    let bytes: Vec<u8> = env::read();
    let (input, chain_journal): (wrap_core::zk::ZkReleaseInput, Vec<u8>) = borsh::from_slice(&bytes).expect("a borsh (ZkReleaseInput, chain journal)");
    let (chain_image_id, _, _) = lez_chain::parse_journal(&chain_journal).expect("a chain journal");
    env::verify(Digest::from(chain_image_id), &chain_journal).expect("the chain journal's receipt");
    let journal = lez_chain::release_journal(&input, &chain_journal).expect("a burn of the chain's tree");
    env::commit_slice(&journal);
}

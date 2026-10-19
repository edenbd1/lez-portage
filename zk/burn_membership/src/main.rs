// The zero-knowledge release guest. Input: a borsh `wrap_core::zk::ZkReleaseInput`
// (the release's context and the burn's opening). Journal: the 352-byte
// release preimage of the root the opening leads to and of its nullifier,
// whose sha256 is `WrapVault.releaseDigest(...)`. A malformed opening
// panics, and a panicking guest has no receipt.
#![no_main]

use risc0_zkvm::guest::env;

risc0_zkvm::guest::entry!(main);

fn main() {
    let bytes: Vec<u8> = env::read();
    let input: wrap_core::zk::ZkReleaseInput = borsh::from_slice(&bytes).expect("a borsh ZkReleaseInput");
    let journal = wrap_core::zk::zk_release_journal(&input).expect("a path of DEPTH nodes and an index in range");
    env::commit_slice(&journal);
}

// The Bedrock anchor guest. Input (stdin): u32 LE length, then a borsh
// `lez_bedrock::compose::AnchorInput`: its own image id (which the release
// guest verifies this receipt under and Ethereum pins), the pinned channel and
// sequencer key, a journal of the chain guest, and the Bedrock block that
// inscribes that journal's tip with the headers above it. The chain journal's
// receipt is verified under the image id it names for itself (`env::verify`,
// resolved when this receipt is compressed). Journal:
// lez_bedrock::compose::anchor_journal. Anything that does not check panics,
// and a panicking guest has no receipt.
#![no_main]

use std::io::Read;

use risc0_zkvm::{guest::env, sha::Digest};

risc0_zkvm::guest::entry!(main);

fn main() {
    let mut stdin = env::stdin();
    let mut n = [0u8; 4];
    stdin.read_exact(&mut n).expect("a length");
    let mut buf = vec![0u8; u32::from_le_bytes(n) as usize];
    stdin.read_exact(&mut buf).expect("the input");
    let input: lez_bedrock::compose::AnchorInput = borsh::from_slice(&buf).expect("a borsh AnchorInput");
    let (chain_image_id, _, _) = lez_chain::parse_journal(&input.chain_journal).expect("a chain journal");
    env::verify(Digest::from(chain_image_id), &input.chain_journal).expect("the chain journal's receipt");
    let journal = lez_bedrock::compose::anchor_journal(&input).expect("the chain's tip, inscribed on Bedrock");
    env::commit_slice(&journal);
}

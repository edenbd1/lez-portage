// The release guest over a Bedrock-anchored chain. Input: a borsh
// `(wrap_core::zk::ZkReleaseInput, Vec<u8>)`: the release's context and the
// burn's opening, and a journal of the anchor guest zk/bedrock_anchor. That
// journal's receipt is verified under the image id it names for itself
// (word 1), which the journal here repeats and Ethereum pins. Journal:
// lez_bedrock::compose::release_journal.
#![no_main]

use risc0_zkvm::{guest::env, sha::Digest};

risc0_zkvm::guest::entry!(main);

fn main() {
    let bytes: Vec<u8> = env::read();
    let (input, anchor_journal): (wrap_core::zk::ZkReleaseInput, Vec<u8>) = borsh::from_slice(&bytes).expect("a borsh (ZkReleaseInput, anchor journal)");
    let (head, _) = lez_bedrock::compose::parse_anchor_journal(&anchor_journal).expect("an anchor journal");
    env::verify(Digest::from(head.self_image_id), &anchor_journal).expect("the anchor journal's receipt");
    let journal = lez_bedrock::compose::release_journal(&input, &anchor_journal).expect("a burn of the anchored chain's tree");
    env::commit_slice(&journal);
}

//! The claim as a privacy-preserving transaction to a private account, with
//! real proving: carol's 1 ETH deposit (vectors/anvil, index 4), claimed
//! against the vault root after five deposits, minted to the private account
//! her deposit committed to. Prints the proving time, the transaction's size
//! and what it publishes, then settles it as the sequencer would and reads
//! the note back with carol's keys.
//!
//! `scripts/measure-private-claim.sh` runs it under `/usr/bin/time -l` for the
//! peak memory. Refuses to run in RISC Zero dev mode.

use std::time::Instant;
use wrap_bridge::*;
use wrap_executor_tests::*;

fn main() {
    if std::env::var("RISC0_DEV_MODE").is_ok_and(|v| !v.is_empty() && v != "0" && v != "false") {
        eprintln!("RISC0_DEV_MODE is set: this measures real proving; unset it");
        std::process::exit(2);
    }
    let mut w = world();
    let carol = carol_private();
    let d = w.a.deposits[4].clone();
    let s = wrap_core::derive_secret(&CAROL_SEED, b'd', 0);
    let c = claim_ix(&w.a, &w.cfg, &d, 5, carol.id(), s, w.slot);
    let seed = wrap_core::sha256(&[b"note randomness", &CAROL_SEED]);

    println!("private claim of deposit {} ({} wei), root after 5 deposits, header slot {}", d.index, d.amount, w.slot);
    println!("wrap_bridge ImageID {}", hex::encode(image_id("wrap_bridge.bin")));
    let t = Instant::now();
    let tx = prove_private_claim(&w.cfg, &c, &carol, seed).expect("the private claim proves");
    let prove_ms = t.elapsed().as_millis();
    println!("prove_ms {prove_ms}");

    let bytes = borsh::to_vec(&tx).unwrap();
    let m = tx.message();
    let proof_len = borsh::to_vec(&tx.witness_set).unwrap().len();
    println!("tx_bytes {} (borsh PrivacyPreservingTransaction; the wire LeeTransaction adds a 1-byte tag)", bytes.len());
    println!("  witness set (the succinct receipt, no signatures) {proof_len}");
    println!("  message {}", borsh::to_vec(m).unwrap().len());
    println!("  signatures {} nonces {}", tx.witness_set.signatures_and_public_keys().len(), m.nonces.len());
    println!("public actions (deferred effects the sequencer applies):");
    let ca = config_account(&BR, &w.cfg);
    let names = [
        (sextant_program::header_account(&LC, &INSTANCE, w.slot), "Sextant header record (slot)"),
        (ca, "bridge config"),
        (entry_account(&BR, &ca, &ETH), "token entry (ETH)"),
        (nullifier_account(&BR, &ca, &wrap_core::nullifier(&w.a.pairing, &s, d.index)), "deposit nullifier PDA"),
        (definition_account(&BR, &ca, &ETH, &SALT).0, "wrapped token definition"),
    ];
    for a in &m.public_actions {
        let name = names.iter().find(|(id, _)| *id == a.account_id).map_or("UNEXPECTED", |(_, n)| n);
        for e in &a.effects {
            println!("  {} [{}] program {} shard {} effect {} bytes", a.account_id, name, e.program_account_id, e.shard_program_account_id, borsh::to_vec(&e.data).unwrap().len());
        }
    }
    println!("private actions {} (commitment, nullifier, encrypted note {} bytes)", m.private_actions.len(), m.private_actions.iter().map(|a| borsh::to_vec(&a.encrypted_post_state).unwrap().len()).sum::<usize>());
    println!("program image claims {}", m.program_image_claims.len());

    // Nothing that opens the deposit is in the transaction.
    let has = |x: &[u8]| bytes.windows(x.len()).any(|y| y == x);
    let zeros = wrap_core::zeros();
    let siblings: Vec<_> = c.path.iter().filter(|p| !zeros.contains(p)).collect();
    println!("opening absent from the transaction bytes: destination {}, secret {}, commitment {}, leaf {}, {} non-empty path nodes {}",
        !has(carol.id().value()), !has(&s), !has(&d.commitment), !has(&d.leaf), siblings.len(), siblings.iter().all(|p| !has(*p)));

    let t = Instant::now();
    settle_private(&mut w.ch, &tx).expect("the sequencer's checks accept it");
    println!("settle_ms {} (proof verification + deferred public effects)", t.elapsed().as_millis());
    let note = read_note(&w.ch, &tx, &carol).expect("carol's keys open the note");
    println!("carol reads her private account: {} wei; supply {}", private_balance(&note), supply(&w.ch, &w.cfg, &ETH));
}

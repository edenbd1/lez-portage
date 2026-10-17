//! The claim as a privacy-preserving transaction, to the private account
//! carol's deposit committed to (vectors/anvil, deposit 4).
//!
//! Each test proves at least one privacy-preserving transaction, so they are
//! `#[ignore]`d in the default run. CI runs them with `RISC0_DEV_MODE=1`: the
//! programs and the privacy circuit execute for real, every settlement check
//! but the receipt's seal runs, and every privacy assertion below is the same;
//! the receipt is a stand-in. The same transaction with real proving, measured:
//! `bin measure_private`, docs/PRIVATE-CLAIM-MEASURED.md.
//!
//!     RISC0_DEV_MODE=1 cargo test --release --test private -- --ignored

use wrap_bridge::*;
use wrap_executor_tests::*;

fn carol_claim(w: &World) -> (Deposit, H256, Claim) {
    let d = w.a.deposits[4].clone();
    let s = wrap_core::derive_secret(&CAROL_SEED, b'd', 0);
    let c = claim_ix(&w.a, &w.cfg, &d, 5, carol_private().id(), s, w.slot);
    (d, s, c)
}

type H256 = [u8; 32];

/// F.4 (private destination), PP.1: the deposit is minted into a fresh
/// private account; the transaction publishes no part of the opening (the
/// destination, the secret, the commitment, the leaf, the path), touches only
/// the bridge's own public accounts and the definition, and needs no signer.
#[test]
#[ignore = "proves a privacy-preserving transaction: RISC0_DEV_MODE=1 cargo test --release --test private -- --ignored"]
fn a_private_claim_mints_to_a_private_account_and_publishes_no_opening() {
    let mut w = world();
    let carol = carol_private();
    let (d, s, c) = carol_claim(&w);
    let tx = prove_private_claim(&w.cfg, &c, &carol, [7; 32]).expect("the private claim proves");

    let bytes = borsh::to_vec(&tx).unwrap();
    let has = |x: &[u8]| bytes.windows(x.len()).any(|y| y == x);
    assert!(!has(carol.id().value()), "the destination is not published");
    assert!(!has(&s), "the secret is not published");
    assert!(!has(&d.commitment), "the commitment is not published");
    assert!(!has(&d.leaf), "the leaf is not published");
    let zeros = wrap_core::zeros();
    for p in c.path.iter().filter(|p| !zeros.contains(p)) {
        assert!(!has(p), "no non-empty node of the path is published");
    }

    let ca = config_account(&BR, &w.cfg);
    let nf = wrap_core::nullifier(&w.a.pairing, &s, d.index);
    let mut public: Vec<_> = tx.message().public_actions.iter().map(|a| a.account_id).collect();
    public.sort();
    let mut expected = vec![
        sextant_program::header_account(&LC, &INSTANCE, w.slot),
        ca,
        entry_account(&BR, &ca, &ETH),
        nullifier_account(&BR, &ca, &nf),
        definition_account(&BR, &ca, &ETH, &SALT).0,
    ];
    expected.sort();
    assert_eq!(public, expected, "the public accounts are the header, the bridge's, the nullifier and the definition");
    assert_eq!(tx.message().private_actions.len(), 1);
    assert!(tx.witness_set.signatures_and_public_keys().is_empty() && tx.message().nonces.is_empty(), "nobody signs");

    settle_private(&mut w.ch, &tx).expect("the sequencer's checks accept it");
    let note = read_note(&w.ch, &tx, &carol).expect("carol's keys open the note");
    assert_eq!(private_balance(&note), d.amount);
    assert_eq!(supply(&w.ch, &w.cfg, &ETH), d.amount);
    assert_eq!(w.ch.shard_of(nullifier_account(&BR, &ca, &nf), BR), vec![1]);
    assert!(read_note(&w.ch, &tx, &Shielded::from_seed(&CAROL_SEED, 1)).is_none(), "other keys read nothing");

    // R.3 across both paths: the same transaction again, or the same deposit
    // claimed publicly, is refused by the one nullifier.
    assert!(settle_private(&mut w.ch, &tx).is_err());
    let rows = claim_rows(&w.cfg, &c);
    let e = w.ch.send(BR, &Instruction::Claim(Box::new(c.clone())), rows).expect_err("the public replay is refused");
    assert_eq!(refusal(&e), Some(E_CLAIMED));
    assert_eq!(supply(&w.ch, &w.cfg, &ETH), d.amount);
}

/// PP.5 (fact 8): a private claim that the bridge refuses (here, the wrong
/// secret) fails while the claimant proves it, on the claimant's machine, so
/// it never reaches the chain; nothing in the state moves.
#[test]
#[ignore = "proves a privacy-preserving transaction: RISC0_DEV_MODE=1 cargo test --release --test private -- --ignored"]
fn a_refused_private_claim_fails_before_it_reaches_the_chain() {
    let w = world();
    let carol = carol_private();
    let (_, _, mut c) = carol_claim(&w);
    c.secret = wrap_core::derive_secret(&CAROL_SEED, b'd', 1);
    let e = prove_private_claim(&w.cfg, &c, &carol, [7; 32]).expect_err("the bridge refuses the opening while it is proven");
    assert_eq!(refusal(&e), Some(E_PATH), "{e}");
    assert_eq!(supply(&w.ch, &w.cfg, &ETH), 0);
}

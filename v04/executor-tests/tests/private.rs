//! The private claim on LEZ v0.4.0-rc3 (src/private.rs), through rc3's own
//! prover and state machine: `ClaimPrivate` to carol's own wrap_bridge actor,
//! run privately inside a privacy-preserving transaction, on the anvil
//! deposits of vectors/anvil-v04 (deposit 4: 1 ETH committed to carol's v0.4
//! private account; the vault deployed with this deployment's pairing).
//!
//! The receipts are fake (`RISC0_DEV_MODE=1`: the programs and the privacy
//! circuit execute in full, the receipt is not a STARK); the same
//! transaction with real proving and its fee leg is `wrap claim 4 --private`
//! on a local rc3 sequencer (v04/scripts/e2e-seq.sh, private-claim.json).
//! Settlement here is `V03State::transition_from_privacy_preserving_transaction`,
//! the check a sequencer runs; it does not classify fees, so no fee leg here.

use lee::{Actor, Simulation};
use lee_core::execution_state::BoundaryStep;
use wrap_bridge_v04::*;
use wrap_executor_tests_v04::private::{self, carol, Keys};
use wrap_executor_tests_v04::*;

const CAROL_SEED: [u8; 32] = *b"carol's wallet seed, test only!!";

fn v04_world() -> World {
    world_on(anvil_at(&root_dir().join("vectors/anvil-v04")))
}

fn programs() -> private::Programs {
    private::Programs { token: TOKEN, ata: ATA, sextant_lc: LC, wrap_bridge: BR }
}

fn carol_claim(w: &World) -> Claim {
    let d = w.a.deposits[4].clone();
    assert_eq!((d.token, d.amount), (ETH, 1_000_000_000_000_000_000), "vectors/anvil-v04 deposit 4: carol's 1 ETH");
    claim_message(&w.a, &w.cfg, &d, 5, carol().account_id(), secret(&CAROL_SEED, 0), SLOT)
}

fn prove_claim(w: &World, c: &Claim, keys: &Keys) -> Result<lee::PrivacyPreservingTransaction, String> {
    let p = programs();
    let (root, msg, public) = private::claim_call(&p, &w.cfg, c);
    let alias = private::claim_alias(&c.secret);
    let input = private::claim_input(root, msg, &public, keys, alias);
    let states = public.iter().map(|a| (*a, w.ch.state.get_account_by_id(a.account_id).data.actor_state(a.program_account_id).clone())).collect();
    let sim = Simulation { public_actor_states: states, admitted_accounts: None };
    let present = private::presenter(keys.account_id(), alias, None);
    private::prove(input, &sim, &p.catalog(), present, private::promote_alias(keys.account_id().blinded(&alias))).map_err(|e| e.to_string())
}

fn settle(w: &mut World, tx: &lee::PrivacyPreservingTransaction) -> Result<(), String> {
    let r = w.ch.state.transition_from_privacy_preserving_transaction(tx, w.ch.block, w.ch.now).map(|_| ()).map_err(|e| e.to_string());
    w.ch.block += 1;
    w.ch.now += 12_000;
    r
}

/// F.4 (private destination), PP.1: carol's deposit is minted into her own
/// private token actor; the transaction publishes neither her account nor the
/// secret, the commitment, the leaf or a non-empty node of the path; its
/// public messages name only a blinded alias of her; the nullifier is spent
/// and the supply raised; her wallet decrypts 1 wrapped ETH.
#[test]
fn a_private_claim_mints_to_a_private_account_and_publishes_no_opening() {
    private::dev_mode();
    let mut w = v04_world();
    let cfg = w.cfg.clone();
    let c = carol_claim(&w);
    let d = w.a.deposits[4].clone();
    let tx = prove_claim(&w, &c, &carol()).expect("the private claim proves");

    let bytes = borsh::to_vec(&tx).unwrap();
    let has = |x: &[u8]| bytes.windows(x.len()).any(|y| y == x);
    assert!(!has(carol().account_id().value()), "the destination is not published");
    assert!(!has(&c.secret), "the secret is not published");
    assert!(!has(&d.commitment), "the commitment is not published");
    assert!(!has(&d.leaf), "the leaf is not published");
    let zeros = wrap_core::zeros();
    for p in c.path.iter().filter(|p| !zeros.contains(p)) {
        assert!(!has(p), "no non-empty node of the path is published");
    }
    // The public part: Attest, CheckLive, Spend, Mint from the alias, nothing to carol's account.
    let alias = carol().account_id().blinded(&private::claim_alias(&c.secret));
    let crossings: Vec<_> = tx.message.execution.boundary.iter().filter_map(|s| if let BoundaryStep::PrivateToPublic(d) = s { Some(d) } else { None }).collect();
    assert!(!crossings.is_empty());
    for x in &crossings {
        assert_ne!(x.envelope.to.account_id, carol().account_id());
    }
    let ca = ca(&cfg);
    let nf = wrap_core::nullifier(&w.a.pairing, &c.secret, c.index);
    let mut declared: Vec<_> = tx.message.public_context.actors.iter().map(|a| a.account_id).collect();
    declared.sort();
    declared.dedup();
    let mut expected = vec![
        wrap_bridge_v04::header_account(&LC, &INSTANCE, SLOT),
        ca,
        entry(&cfg, &ETH),
        nullifier_account(&BR, &ca, &nf),
        definition(&cfg, &ETH),
    ];
    expected.sort();
    assert_eq!(declared, expected, "the public actors: the header, the bridge's own, the nullifier, the definition");

    settle(&mut w, &tx).expect("settles");
    assert_eq!(w.ch.actor_state(nullifier_account(&BR, &ca, &nf), BR), vec![1], "the nullifier is spent");
    assert_eq!(supply(&w.ch, &cfg, &ETH), d.amount);
    assert_eq!(entry_state_minted(&w), d.amount);
    assert!(w.ch.token_state(alias).is_none() && w.ch.token_state(carol().account_id()).is_none(), "no public holding at carol or her alias");
    let (def, bal) = private::private_balance(&carol(), &tx, TOKEN).expect("carol decrypts her holding");
    assert_eq!((def, bal), (definition(&cfg, &ETH), d.amount));
}

fn entry_state_minted(w: &World) -> u128 {
    borsh::from_slice::<TokenEntry>(&w.ch.actor_state(entry(&w.cfg, &ETH), BR)).unwrap().minted
}

/// The same deposit cannot be claimed twice, privately then privately again
/// (another wallet's proof of the same opening), nor publicly after: the
/// nullifier PDA is shared by both paths.
#[test]
fn a_private_claim_lands_once_and_blocks_the_public_claim() {
    private::dev_mode();
    let mut w = v04_world();
    let cfg = w.cfg.clone();
    let c = carol_claim(&w);
    let first = prove_claim(&w, &c, &carol()).unwrap();
    let again = prove_claim(&w, &c, &carol()).unwrap();
    settle(&mut w, &first).expect("the first lands");
    let e = settle(&mut w, &again).expect_err("the second must not settle");
    println!("second: {e}");
    let e = claim(&mut w.ch, &cfg, &c).expect_err("the public claim of the same deposit");
    assert!(refusal(&e).is_some_and(|r| r == E_CLAIMED || r == E_ACCOUNTS) || e.contains("violated"), "{e}");
    assert_eq!(supply(&w.ch, &cfg, &ETH), c.amount, "minted once");
}

/// Refused inside the proof, so nothing reaches the chain: a claim sent to
/// another private account's actor than the committed destination, a forged
/// amount, and a root the vault never had (Sextant's attestation, simulated
/// against the header record before any proof).
#[test]
fn a_refused_private_claim_never_proves() {
    private::dev_mode();
    let w = v04_world();
    let c = carol_claim(&w);
    // Mallory proves carol's opening at her own account: the receiver is not the destination.
    let mallory = Keys::new(0x71);
    let mut m = c.clone();
    m.destination = mallory.account_id();
    let e = prove_claim(&w, &m, &mallory).unwrap_err();
    assert!(refusal(&e) == Some(E_PATH), "{e}");
    let e = {
        let p = programs();
        let (_, msg, public) = private::claim_call(&p, &w.cfg, &c);
        let alias = private::claim_alias(&c.secret);
        let input = private::claim_input(Actor::new(mallory.account_id(), BR), msg, &public, &mallory, alias);
        let states = public.iter().map(|a| (*a, w.ch.state.get_account_by_id(a.account_id).data.actor_state(a.program_account_id).clone())).collect();
        let sim = Simulation { public_actor_states: states, admitted_accounts: None };
        private::prove(input, &sim, &p.catalog(), private::presenter(mallory.account_id(), alias, None), private::promote_alias(mallory.account_id().blinded(&alias))).map(|_| ()).unwrap_err().to_string()
    };
    assert_eq!(refusal(&e), Some(E_ACCOUNTS), "carol's claim at mallory's actor: {e}");
    let mut f = c.clone();
    f.amount += 1;
    assert_eq!(refusal(&prove_claim(&w, &f, &carol()).unwrap_err()), Some(E_PATH));
    let mut u = c.clone();
    let fake = vec![w.a.deposits[4].leaf, [0xab; 32]];
    u.index = 0;
    u.path = wrap_core::path(&fake, 0);
    u.root = wrap_core::root(&fake);
    let mut p = w.a.unknown.1.clone();
    p.slot = wrap_core::known_root_slot(&u.root);
    u.proof = p;
    let e = prove_claim(&w, &u, &carol()).unwrap_err();
    assert!(refusal(&e).is_some_and(|r| (7200..7300).contains(&r)) || refusal(&e) == Some(E_PATH), "a root the vault never had: {e}");
}

/// F.6 (burn from a private account): carol, holding 1 wrapped ETH privately,
/// burns 0.1 ETH with a public root call (the leaf is in the block); the
/// holder is a fresh blinded alias of her, her token actor runs privately and
/// moves the tokens into the entry's escrow, which burns them. Her account is
/// not in the transaction; the supply falls; she decrypts 0.9.
#[test]
fn a_private_holder_burns_with_a_public_root_and_stays_hidden() {
    private::dev_mode();
    let mut w = v04_world();
    let cfg = w.cfg.clone();
    let c = carol_claim(&w);
    let claim_tx = prove_claim(&w, &c, &carol()).unwrap();
    settle(&mut w, &claim_tx).unwrap();
    let state = carol().decrypt(&claim_tx).expect("carol's note");
    let proof = w.ch.state.get_proof_for_commitment(&lee_core::Commitment::new(&carol().account_id(), &state)).expect("committed");

    let amount = eth_policy().denominations[0];
    let blinding = [0x5b; 32];
    let ec = wrap_core::eth_commitment(&[0x42; 20], 0, &blinding);
    let opening = private::burn_alias(&blinding);
    let alias = carol().account_id().blinded(&opening);
    let p = programs();
    let (root, msg, public) = private::burn_call(&p, &cfg, ETH, amount, ec, alias);
    let input = private::burn_input(root, msg, &public, carol().update_witness(state, proof, &[opening]));
    let states = public.iter().map(|a| (*a, w.ch.state.get_account_by_id(a.account_id).data.actor_state(a.program_account_id).clone())).collect();
    let sim = Simulation { public_actor_states: states, admitted_accounts: None };
    let tx = private::prove(input, &sim, &p.catalog(), private::presenter(carol().account_id(), opening, None), |_, _| false).expect("the private burn proves");
    let bytes = borsh::to_vec(&tx).unwrap();
    assert!(!bytes.windows(32).any(|x| x == carol().account_id().value()), "carol's account is not in the transaction");
    settle(&mut w, &tx).expect("settles");
    assert_eq!(supply(&w.ch, &cfg, &ETH), c.amount - amount);
    let leaf = wrap_core::burn_leaf(&w.a.pairing, &ETH, amount, &ec);
    let f: wrap_core::Frontier = borsh::from_slice(&w.ch.actor_state(burns(&cfg), BR)).unwrap();
    assert_eq!((f.next_index, f.root), (1, wrap_core::root(&[leaf])));
    assert_eq!(raw_balance(&w.ch, entry(&cfg, &ETH)), 0, "the escrow burned what it received");
    let (_, bal) = private::private_balance(&carol(), &tx, TOKEN).expect("carol decrypts her holding");
    assert_eq!(bal, c.amount - amount);
}

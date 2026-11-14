//! A real Sextant attestation for the real Sepolia deposits, on v0.4.0-rc3.
//!
//! Sextant's light client (sextant_lc on v0.4) is initialised from the real
//! Sepolia checkpoint finalized after the two deposits into the Sepolia
//! WrapVault (slot 11,536,448, execution block 12,092,950: lez-sextant
//! vectors/sepolia-portage, the same checkpoint the v0.3 testnet run used),
//! verifying the bootstrap's committee branch and execution header. Then the
//! exact statement wrap_bridge's Claim sends it (`wrap_bridge_v04::query`:
//! `knownRoot[root] == 1` at the vault, over the vault's real account and
//! storage proofs at that block, evidence/testnet/sepolia/proofs.json) is
//! attested for both deposits' roots, and a root the vault never had is
//! refused with E7208, as on the v0.3 testnet.
//!
//! The full Claim (the mint) is not run here: it mints to the deposit's
//! committed destination, whose holding only its own key can open.

use lee::Actor;
use sextant_core::lc;
use sextant_program_v04::{header_account, instance_account, instance_seed, Message as Sx, E_BASE};
use wrap_bridge_v04::{query, AttestMessage};
use wrap_executor_tests_v04::*;

const ID: [u8; 32] = *b"sepolia/portage/v04/instance/001";

#[test]
fn sextant_on_v04_attests_the_sepolia_vault_roots_a_claim_asks_for() {
    let m = sextant_host::sepolia_portage();
    let mut ch = Chain::new();
    ch.now = lc::slot_time(&m.cfg, m.boot_header.beacon.slot) * 1000 + 600_000;
    let slot = m.boot_header.beacon.slot;
    let hdr = Actor::new(header_account(&LC, &ID, slot), LC);
    let init = Sx::Init { instance_id: ID, checkpoint: m.checkpoint, cfg: m.cfg.clone(), header: m.boot_header.clone(), committee: m.boot_committee.clone(), branch: m.boot_branch.clone() };
    let k = ch.fresh();
    ch.send("Init (Sepolia checkpoint)", Actor::new(instance_account(&LC, &ID), LC), &[hdr], &[&k], &[(LC, instance_seed(&ID))], &init).unwrap();

    let a = anvil_at(&root_dir().join("evidence/testnet/sepolia"));
    let mut cfg = config(a.vault);
    cfg.chain_id = 11_155_111;
    cfg.sextant = LC;
    cfg.instance_id = ID;
    assert_eq!(a.proofs.len(), 3, "the empty tree's root and both deposits' roots");
    for (n, (root, proof)) in a.proofs.iter().enumerate() {
        assert_eq!(proof.slot, wrap_core::known_root_slot(root));
        let k = ch.fresh();
        let o = ch.send("Attest", hdr, &[], &[&k], &[], &AttestMessage::Attest { instance_id: ID, slot, query: query(&cfg, proof.clone()) });
        println!("vault root {n} attested: {:?} cycles", o.as_ref().map(|o| o.cycles));
        o.unwrap();
    }
    let (_, unknown) = &a.unknown;
    let k = ch.fresh();
    let e = ch.send("Attest", hdr, &[], &[&k], &[], &AttestMessage::Attest { instance_id: ID, slot, query: query(&cfg, unknown.clone()) }).unwrap_err();
    assert_eq!(refusal(&e), Some(E_BASE + sextant_core::Error::PredicateFalse as u32), "{e}");
    // The same statement as a mainnet one: refused, another chain.
    let mut main = cfg.clone();
    main.chain_id = 1;
    let k = ch.fresh();
    let e = ch.send("Attest", hdr, &[], &[&k], &[], &AttestMessage::Attest { instance_id: ID, slot, query: query(&main, a.proofs[0].1.clone()) }).unwrap_err();
    assert_eq!(refusal(&e), Some(E_BASE + sextant_core::Error::ChainMismatch as u32), "{e}");
}

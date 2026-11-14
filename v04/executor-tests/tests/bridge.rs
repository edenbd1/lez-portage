//! wrap_bridge on LEZ v0.4.0-rc3, through `lee::V03State`, next to the
//! canonical token program's actor-model port and Sextant's light client
//! (sextant_lc on v0.4, initialised on the devnet header of the anvil block),
//! on the anvil deposits of vectors/anvil. Every claim is sent by a brand-new
//! relayer account.

use wrap_bridge_v04::*;
use wrap_executor_tests_v04::*;

fn refused(r: Result<Outcome, String>) -> u32 {
    let e = r.expect_err("must be refused");
    refusal(&e).unwrap_or_else(|| panic!("no typed refusal in: {e}"))
}

fn reverted_with(r: Result<Outcome, String>, needle: &str) {
    let e = r.expect_err("must be refused");
    assert!(e.contains(needle), "expected {needle:?} in: {e}");
}

fn entry_state(w: &World, t: &[u8; 20]) -> TokenEntry {
    borsh::from_slice(&w.ch.actor_state(entry(&w.cfg, t), BR)).unwrap()
}

#[test]
fn the_token_mirror_and_burn_layout_are_pinned() {
    use lee::AccountId;
    // The block scan reads Burn's leaf fields from the root message's first bytes.
    let cfg = config([9; 20]);
    let m = burn_message(&cfg, [1; 20], 7, [2; 32], AccountId::new([3; 32]));
    let b = borsh::to_vec(&m).unwrap();
    assert_eq!(b[0], BURN_VARIANT);
    assert_eq!(b[0], lez_chain_v04::BURN_VARIANT);
    assert_eq!(&b[1..21], &[1; 20]);
    assert_eq!(u128::from_le_bytes(b[21..37].try_into().unwrap()), 7);
    assert_eq!(&b[37..69], &[2; 32]);
}

#[test]
fn register_creates_a_zero_supply_definition_minted_only_by_the_entry() {
    let w = world();
    let e = entry_state(&w, &ETH);
    assert_eq!((e.minted, e.burned, e.definition), (0, 0, definition(&w.cfg, &ETH)));
    match w.ch.token_state(definition(&w.cfg, &ETH)) {
        Some(token::TokenState::Definition(d)) => {
            assert_eq!(d.total_supply, 0);
            assert_eq!(d.authority, Some(entry(&w.cfg, &ETH)), "the entry PDA is the mint authority");
        }
        other => panic!("{other:?}"),
    }
    // The entry's token actor is the (empty) burn escrow.
    assert!(matches!(w.ch.token_state(entry(&w.cfg, &ETH)), Some(token::TokenState::Holding(h)) if h.balance == 0 && h.definition_id == definition(&w.cfg, &ETH)));
    // The burn tree exists from Init.
    let f: wrap_core::Frontier = borsh::from_slice(&w.ch.actor_state(burns(&w.cfg), BR)).unwrap();
    assert_eq!(f.next_index, 0);
}

#[test]
fn only_the_admin_registers_and_a_token_registers_once() {
    let mut w = world();
    let (cfg, t) = (w.cfg.clone(), [0x77; 20]);
    let stranger = w.ch.fresh();
    assert_eq!(refused(register(&mut w.ch, &stranger, &cfg, t, "x", eth_policy())), E_NOT_ADMIN);
    assert_eq!(refused(register(&mut w.ch, &admin(), &cfg, ETH, "again", eth_policy())), E_EXISTS);
}

/// F.3, F.4, R.1: a deposit is minted once, to the destination it committed to.
#[test]
fn a_deposit_is_minted_once_to_its_committed_destination() {
    let mut w = world();
    let cfg = w.cfg.clone();
    open_holding(&mut w.ch, &alice(), &cfg, &ETH).expect("alice opens her holding");
    let d = w.a.deposits[0].clone(); // alice, 1 ETH
    let c = claim_message(&w.a, &cfg, &d, 4, alice().id, secret(&ALICE_SEED, 0), SLOT);
    let out = claim(&mut w.ch, &cfg, &c).expect("the claim lands");
    assert!(out.cycles < GAS_CAP, "within one block's execution gas: {}", out.cycles);
    assert_eq!(balance(&w.ch, alice().id), d.amount);
    assert_eq!(supply(&w.ch, &cfg, &ETH), d.amount);
    assert_eq!(entry_state(&w, &ETH).minted, d.amount);
    let nf = wrap_core::nullifier(&w.a.pairing, &c.secret, c.index);
    assert_eq!(w.ch.actor_state(nullifier_account(&BR, &ca(&cfg), &nf), BR), vec![1]);
    // The Claimed event names the nullifier.
    let sel = event("Claimed", &Claimed { nullifier: nf });
    assert!(out.events.iter().any(|e| e.event.selector == sel.selector && e.event.data == sel.data));

    assert_eq!(refused(claim(&mut w.ch, &cfg, &c)), E_CLAIMED);
    let early = claim_message(&w.a, &cfg, &d, 1, alice().id, secret(&ALICE_SEED, 0), SLOT);
    assert_ne!(early.root, c.root);
    assert_eq!(refused(claim(&mut w.ch, &cfg, &early)), E_CLAIMED);
    assert_eq!(balance(&w.ch, alice().id), d.amount, "nothing more was minted");
}

/// Security #3: a relayer cannot redirect a claim; a wrong opening has no leaf.
#[test]
fn a_claim_cannot_be_redirected_or_forged() {
    let mut w = world();
    let cfg = w.cfg.clone();
    open_holding(&mut w.ch, &bob(), &cfg, &ETH).unwrap();
    let d = w.a.deposits[0].clone();
    let mut c = claim_message(&w.a, &cfg, &d, 4, alice().id, secret(&ALICE_SEED, 0), SLOT);
    c.destination = bob().id;
    assert_eq!(refused(claim(&mut w.ch, &cfg, &c)), E_PATH);
    let mut c = claim_message(&w.a, &cfg, &d, 4, alice().id, secret(&ALICE_SEED, 0), SLOT);
    c.amount += 1;
    assert_eq!(refused(claim(&mut w.ch, &cfg, &c)), E_PATH);
    assert_eq!(balance(&w.ch, bob().id), 0);
}

/// The root must be the vault's: a proof of an unknown root fails Attest, a
/// proof of another slot or address fails before.
#[test]
fn the_root_is_attested_by_sextant() {
    let mut w = world();
    let cfg = w.cfg.clone();
    open_holding(&mut w.ch, &alice(), &cfg, &ETH).unwrap();
    let d = w.a.deposits[0].clone();
    // A tree the vault never had, with its (zero) knownRoot proof: the path is
    // consistent, the attestation is not.
    let mut c = claim_message(&w.a, &cfg, &d, 4, alice().id, secret(&ALICE_SEED, 0), SLOT);
    let fake_leaves = vec![d.leaf, [0xab; 32]];
    c.path = wrap_core::path(&fake_leaves, 0);
    c.root = wrap_core::root(&fake_leaves);
    let mut p = w.a.unknown.1.clone();
    p.slot = wrap_core::known_root_slot(&c.root);
    c.proof = p;
    assert_eq!(refused(claim(&mut w.ch, &cfg, &c)), sextant_program_v04::E_BASE + sextant_core::Error::BadInclusion as u32);
    // The real unknown-root proof against its own root: the slot holds 0.
    let mut c = claim_message(&w.a, &cfg, &d, 4, alice().id, secret(&ALICE_SEED, 0), SLOT);
    c.proof = w.a.unknown.1.clone();
    assert_eq!(refused(claim(&mut w.ch, &cfg, &c)), E_PROOF);
    // A header slot nobody recorded: its record account does not exist, so
    // the engine refuses to deliver Attest to it (an unadmitted public actor)
    // before the program runs; the claim reverts all the same.
    let c = claim_message(&w.a, &cfg, &d, 4, alice().id, secret(&ALICE_SEED, 0), SLOT + 1);
    reverted_with(claim(&mut w.ch, &cfg, &c), "violated execution rules");
    assert_eq!(balance(&w.ch, alice().id), 0);
}

/// The destination must have opened its holding (no ATA program on v0.4
/// yet): a destination account that does not exist is not admitted, and one
/// that exists without a holding of this token is refused by the token
/// program's credit.
#[test]
fn a_claim_needs_the_destinations_holding() {
    let mut w = world();
    let cfg = w.cfg.clone();
    let d = w.a.deposits[0].clone();
    let c = claim_message(&w.a, &cfg, &d, 4, alice().id, secret(&ALICE_SEED, 0), SLOT);
    reverted_with(claim(&mut w.ch, &cfg, &c), "violated execution rules");
    // Alice exists now and holds another token: the token program's credit refuses.
    let junk = w.ch.fresh();
    let nd = token::Message::NewDefinition { name: "junk".into(), total_supply: 1, mint_authority: None, holding: alice().id };
    w.ch.send("NewDefinition", tk(junk.id), &[tk(alice().id)], &[&junk, &alice()], &[], &nd).unwrap();
    reverted_with(claim(&mut w.ch, &cfg, &c), "recipient holds another token");
    assert_eq!(supply(&w.ch, &cfg, &ETH), 0, "both reverted whole: no supply was raised");
    // The reverts dropped everything: the nullifier is not spent, so the
    // claim lands later, to an account that holds the wrapped token.
    let nul = claim_actors(&cfg, &c).0.account_id;
    assert!(w.ch.actor_state(nul, BR).is_empty());
}

#[test]
fn freezes_and_the_cap_hold() {
    let mut w = world();
    let cfg = w.cfg.clone();
    open_holding(&mut w.ch, &alice(), &cfg, &ETH).unwrap();
    let d = w.a.deposits[0].clone();
    let c = claim_message(&w.a, &cfg, &d, 4, alice().id, secret(&ALICE_SEED, 0), SLOT);
    let freeze = |ch: &mut Chain, who: &Key, t: Option<[u8; 20]>, mint: bool| {
        ch.send("Freeze", br(who.id), &[br(ca(&cfg)), br(entry(&cfg, &ETH))], &[who], &[], &Message::Freeze { config: cfg.clone(), eth_token: t, mint, burn: false })
    };
    let stranger = w.ch.fresh();
    assert_eq!(refused(freeze(&mut w.ch, &stranger, None, true)), E_NOT_FREEZER);
    freeze(&mut w.ch, &freezer(), None, true).unwrap();
    assert_eq!(refused(claim(&mut w.ch, &cfg, &c)), E_FROZEN);
    freeze(&mut w.ch, &freezer(), None, false).unwrap();
    freeze(&mut w.ch, &freezer(), Some(ETH), true).unwrap();
    assert_eq!(refused(claim(&mut w.ch, &cfg, &c)), E_FROZEN);
    freeze(&mut w.ch, &freezer(), Some(ETH), false).unwrap();
    // A cap below the deposit.
    let set = SetPolicyMsg::new(&cfg, Policy { denominations: eth_policy().denominations, mint_cap: d.amount - 1 });
    w.ch.send("SetPolicy", br(admin().id), &[br(entry(&cfg, &ETH))], &[&admin()], &[], &set.0).unwrap();
    assert_eq!(refused(claim(&mut w.ch, &cfg, &c)), E_CAP);
    let set = SetPolicyMsg::new(&cfg, eth_policy());
    w.ch.send("SetPolicy", br(admin().id), &[br(entry(&cfg, &ETH))], &[&admin()], &[], &set.0).unwrap();
    claim(&mut w.ch, &cfg, &c).expect("unfrozen, under the cap");
}

struct SetPolicyMsg(Message);
impl SetPolicyMsg {
    fn new(cfg: &Config, policy: Policy) -> Self {
        Self(Message::SetPolicy { config: cfg.clone(), eth_token: ETH, policy })
    }
}

fn claimed_world() -> (World, u128) {
    let mut w = world();
    let cfg = w.cfg.clone();
    open_holding(&mut w.ch, &alice(), &cfg, &ETH).unwrap();
    let d = w.a.deposits[0].clone();
    let c = claim_message(&w.a, &cfg, &d, 4, alice().id, secret(&ALICE_SEED, 0), SLOT);
    claim(&mut w.ch, &cfg, &c).unwrap();
    (w, d.amount)
}

/// F.5 outbound: the holder burns a permitted amount; the supply falls, the
/// leaf is appended, the escrow is empty again.
#[test]
fn a_burn_takes_the_wrapped_token_out_and_appends_its_leaf() {
    let (mut w, minted) = claimed_world();
    let cfg = w.cfg.clone();
    let amount = eth_policy().denominations[0];
    let ec = wrap_core::eth_commitment(&[0x42; 20], 0, &[0x43; 32]);
    let out = burn(&mut w.ch, &cfg, &alice(), ETH, amount, ec).expect("the burn lands");
    assert!(out.cycles < GAS_CAP);
    assert_eq!(balance(&w.ch, alice().id), minted - amount);
    assert_eq!(supply(&w.ch, &cfg, &ETH), minted - amount);
    assert_eq!(balance(&w.ch, entry(&cfg, &ETH)), 0, "the escrow burned what it received");
    let e = entry_state(&w, &ETH);
    assert_eq!((e.minted, e.burned), (minted, amount));
    let leaf = wrap_core::burn_leaf(&w.a.pairing, &ETH, amount, &ec);
    let f: wrap_core::Frontier = borsh::from_slice(&w.ch.actor_state(burns(&cfg), BR)).unwrap();
    assert_eq!((f.next_index, f.root), (1, wrap_core::root(&[leaf])));
    let sel = event("Burned", &Burned { leaf });
    assert!(out.events.iter().any(|e| e.event.selector == sel.selector && e.event.data == sel.data));
}

#[test]
fn a_burn_is_refused_unless_it_is_the_holders_wrapped_token_in_a_permitted_amount() {
    let (mut w, minted) = claimed_world();
    let cfg = w.cfg.clone();
    let ec = [0x44; 32];
    // Not a denomination.
    assert_eq!(refused(burn(&mut w.ch, &cfg, &alice(), ETH, 12345, ec)), E_AMOUNT);
    // Unsigned: bob sends alice's burn; the token program refuses the debit.
    let m = burn_message(&cfg, ETH, eth_policy().denominations[0], ec, alice().id);
    reverted_with(w.ch.send("Burn", br(burns(&cfg)), &burn_actors(&cfg, &ETH, alice().id), &[&bob()], &[], &m), "sender holding must authorize");
    // Another token's holding: a worthless token bob mints himself, burned
    // as if it were wrapped ETH. The escrow refuses another definition.
    let junk = w.ch.fresh();
    let nd = token::Message::NewDefinition { name: "junk".into(), total_supply: 10u128.pow(19), mint_authority: None, holding: bob().id };
    w.ch.send("NewDefinition", tk(junk.id), &[tk(bob().id)], &[&junk, &bob()], &[], &nd).expect("bob's own token");
    reverted_with(burn(&mut w.ch, &cfg, &bob(), ETH, eth_policy().denominations[0], ec), "recipient holds another token");
    // A wrong salt is not this token.
    let mut m = burn_message(&cfg, ETH, eth_policy().denominations[0], ec, alice().id);
    if let Message::Burn { salt, .. } = &mut m {
        *salt = [0; 32];
    }
    assert_eq!(refused(w.ch.send("Burn", br(burns(&cfg)), &burn_actors(&cfg, &ETH, alice().id), &[&alice()], &[], &m)), E_UNSUPPORTED);
    // Nothing moved; the tree is empty.
    assert_eq!(balance(&w.ch, alice().id), minted);
    assert_eq!(supply(&w.ch, &cfg, &ETH), minted);
    let f: wrap_core::Frontier = borsh::from_slice(&w.ch.actor_state(burns(&cfg), BR)).unwrap();
    assert_eq!(f.next_index, 0);
}

/// The cycles each transaction was charged (printed with --nocapture).
#[test]
fn cycles_per_transaction() {
    let (mut w, _) = claimed_world();
    let cfg = w.cfg.clone();
    burn(&mut w.ch, &cfg, &alice(), ETH, eth_policy().denominations[0], [0x45; 32]).unwrap();
    println!("| transaction | cycles | applied |");
    println!("|---|---:|---|");
    for (what, cycles, ok) in &w.ch.log {
        println!("| {what} | {cycles} | {ok} |");
        assert!(*cycles < GAS_CAP);
    }
}

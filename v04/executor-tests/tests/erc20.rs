//! The ERC-20 path of wrap_bridge on LEZ v0.4.0-rc3: the USDT-shaped token of
//! vectors/anvil (6 decimals, transfers that return nothing), deposited into
//! the anvil WrapVault (deposit 1: 100 USDT, committed to alice), claimed and
//! burned through the same three programs as ETH (the canonical token
//! program's actor-model port, Sextant's light client, wrap_bridge), with the
//! refusals that matter for a second denomination: a double claim, a root the
//! vault never had, and the wrong token, at the claim and at the burn.
//!
//! Holdings are ATA holdings (lez-programs' actor-model ATA program), one per
//! owner and wrapped token, so one account holds wrapped ETH and wrapped USDT
//! side by side (`one_account_holds_several_wrapped_tokens`; before the ATA,
//! an account's single token actor held one token: docs/V04-PORT.md).

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

fn tree(w: &World) -> wrap_core::Frontier {
    borsh::from_slice(&w.ch.actor_state(burns(&w.cfg), BR)).unwrap()
}

/// Deposit 1 of vectors/anvil: alice's 100 USDT, under the root after four deposits.
fn usdt_claim(w: &World) -> (Deposit, Claim) {
    let d = w.a.deposits[1].clone();
    assert_eq!((d.token, d.amount), (w.a.usdt, 100_000_000), "vectors/anvil deposit 1 is 100 USDT");
    let c = claim_message(&w.a, &w.cfg, &d, 4, alice().id, secret(&ALICE_SEED, 1), SLOT);
    (d, c)
}

/// Alice holds 100 wrapped USDT, claimed from deposit 1.
fn usdt_world() -> (World, u128) {
    let mut w = world();
    let cfg = w.cfg.clone();
    let usdt = w.a.usdt;
    open_holding(&mut w.ch, &alice(), &cfg, &usdt).expect("alice opens her wrapped-USDT holding");
    let (d, c) = usdt_claim(&w);
    claim(&mut w.ch, &cfg, &c).expect("the USDT claim lands");
    (w, d.amount)
}

#[test]
fn the_erc20_entry_is_its_own_wrapped_token() {
    let w = world();
    let usdt = w.a.usdt;
    let e = entry_state(&w, &usdt);
    assert_eq!((e.minted, e.burned, e.definition), (0, 0, definition(&w.cfg, &usdt)));
    assert_eq!(e.policy, usdt_policy());
    assert_ne!(definition(&w.cfg, &usdt), definition(&w.cfg, &ETH), "another definition than wrapped ETH");
    match w.ch.token_state(definition(&w.cfg, &usdt)) {
        Some(token::TokenState::Definition(d)) => {
            assert_eq!((d.total_supply, d.name.as_str()), (0, "Wrapped USDT (anvil)"));
            assert_eq!(d.authority, Some(entry(&w.cfg, &usdt)), "the USDT entry PDA is the mint authority");
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(w.ch.token_state(entry(&w.cfg, &usdt)), Some(token::TokenState::Holding(h)) if h.balance == 0 && h.definition_id == definition(&w.cfg, &usdt)));
}

/// F.1/F.3 for an ERC-20: the USDT deposit is minted once, to its committed
/// destination, in the token's own base units; replays under the same or an
/// earlier root are refused.
#[test]
fn an_erc20_deposit_is_minted_once() {
    let mut w = world();
    let cfg = w.cfg.clone();
    let usdt = w.a.usdt;
    open_holding(&mut w.ch, &alice(), &cfg, &usdt).unwrap();
    let (d, c) = usdt_claim(&w);
    let out = claim(&mut w.ch, &cfg, &c).expect("the claim lands");
    assert!(out.cycles < GAS_CAP, "{}", out.cycles);
    assert_eq!(balance(&w.ch, &cfg, alice().id, &usdt), d.amount);
    assert_eq!(supply(&w.ch, &cfg, &usdt), d.amount);
    assert_eq!(entry_state(&w, &usdt).minted, d.amount);
    assert_eq!(entry_state(&w, &ETH).minted, 0, "the ETH entry is untouched");
    let nf = wrap_core::nullifier(&w.a.pairing, &c.secret, c.index);
    assert_eq!(w.ch.actor_state(nullifier_account(&BR, &ca(&cfg), &nf), BR), vec![1]);
    let sel = event("Claimed", &Claimed { nullifier: nf });
    assert!(out.events.iter().any(|e| e.event.selector == sel.selector && e.event.data == sel.data));

    // Double claim: the same opening again, and under an earlier root.
    assert_eq!(refused(claim(&mut w.ch, &cfg, &c)), E_CLAIMED);
    let early = claim_message(&w.a, &cfg, &d, 2, alice().id, secret(&ALICE_SEED, 1), SLOT);
    assert_ne!(early.root, c.root);
    assert_eq!(refused(claim(&mut w.ch, &cfg, &early)), E_CLAIMED);
    assert_eq!(balance(&w.ch, &cfg, alice().id, &usdt), d.amount, "nothing more was minted");
    assert_eq!(supply(&w.ch, &cfg, &usdt), d.amount);
}

/// An ERC-20 claim against a root the vault never had: refused by Sextant
/// for a fabricated tree (the vault's storage proof shows its `knownRoot`
/// slot empty, so the statement `knownRoot[root] == 1` is false), and by the
/// bridge for the real proof of a root it never had presented for this one.
#[test]
fn an_erc20_claim_needs_a_root_the_vault_had() {
    let mut w = world();
    let cfg = w.cfg.clone();
    let usdt = w.a.usdt;
    open_holding(&mut w.ch, &alice(), &cfg, &usdt).unwrap();
    let (d, mut c) = usdt_claim(&w);
    let fake = vec![[0xab; 32], d.leaf];
    c.index = 1;
    c.path = wrap_core::path(&fake, 1);
    c.root = wrap_core::root(&fake);
    let mut p = w.a.unknown.1.clone();
    p.slot = wrap_core::known_root_slot(&c.root);
    c.proof = p;
    assert_eq!(refused(claim(&mut w.ch, &cfg, &c)), sextant_program_v04::E_BASE + sextant_core::Error::PredicateFalse as u32);
    let (_, mut c) = usdt_claim(&w);
    c.proof = w.a.unknown.1.clone();
    assert_eq!(refused(claim(&mut w.ch, &cfg, &c)), E_PROOF);
    assert_eq!(balance(&w.ch, &cfg, alice().id, &usdt), 0);
    assert_eq!(supply(&w.ch, &cfg, &usdt), 0);
}

/// The wrong token at the claim: the leaf commits to the ERC-20's address,
/// so the USDT deposit claimed as ETH or as another ERC-20, and an ETH
/// deposit claimed as USDT, have no leaf in the tree; the right token with
/// another definition salt is not the registered wrapped token.
#[test]
fn an_erc20_claim_is_refused_for_the_wrong_token() {
    let mut w = world();
    let cfg = w.cfg.clone();
    let usdt = w.a.usdt;
    open_holding(&mut w.ch, &alice(), &cfg, &usdt).unwrap();
    let (_, mut c) = usdt_claim(&w);
    c.eth_token = ETH;
    assert_eq!(refused(claim(&mut w.ch, &cfg, &c)), E_PATH, "the USDT leaf claimed as ETH");
    let (_, mut c) = usdt_claim(&w);
    c.eth_token = [0x77; 20];
    assert_eq!(refused(claim(&mut w.ch, &cfg, &c)), E_PATH, "the USDT leaf claimed as an unregistered ERC-20");
    let eth = w.a.deposits[0].clone();
    let mut c = claim_message(&w.a, &cfg, &eth, 4, alice().id, secret(&ALICE_SEED, 0), SLOT);
    c.eth_token = usdt;
    assert_eq!(refused(claim(&mut w.ch, &cfg, &c)), E_PATH, "an ETH leaf claimed as USDT");
    // The right leaf, another salt: the entry refuses before any mint. The
    // transaction declares the real definition, so only the salt differs.
    let (_, good) = usdt_claim(&w);
    let (to, actors, pda) = claim_actors(&cfg, &good);
    let mut bad = good.clone();
    bad.salt = [0x11; 32];
    let relayer = w.ch.fresh();
    assert_eq!(refused(w.ch.send("Claim", to, &actors, &[&relayer], &[pda], &Message::Claim(Box::new(bad)))), E_UNSUPPORTED);
    assert_eq!(balance(&w.ch, &cfg, alice().id, &usdt), 0);
    // None of these spent the nullifier: the honest claim still lands.
    claim(&mut w.ch, &cfg, &good).expect("the honest claim");
    assert_eq!(balance(&w.ch, &cfg, alice().id, &usdt), 100_000_000);
}

/// Freezes are per token: USDT minting frozen does not stop ETH, and the
/// cap is in the ERC-20's base units.
#[test]
fn an_erc20_freeze_and_cap_are_its_own() {
    let mut w = world();
    let cfg = w.cfg.clone();
    let usdt = w.a.usdt;
    open_holding(&mut w.ch, &alice(), &cfg, &usdt).unwrap();
    open_holding(&mut w.ch, &bob(), &cfg, &ETH).unwrap();
    let freeze = |ch: &mut Chain, mint: bool| {
        ch.send("Freeze", br(freezer().id), &[br(entry(&cfg, &usdt))], &[&freezer()], &[], &Message::Freeze { config: cfg.clone(), eth_token: Some(usdt), mint, burn: false })
    };
    freeze(&mut w.ch, true).unwrap();
    let (_, c) = usdt_claim(&w);
    assert_eq!(refused(claim(&mut w.ch, &cfg, &c)), E_FROZEN);
    // Bob's ETH deposit (vectors/anvil deposit 2) still lands.
    let b = w.a.deposits[2].clone();
    claim(&mut w.ch, &cfg, &claim_message(&w.a, &cfg, &b, 4, bob().id, secret(&BOB_SEED, 0), SLOT)).expect("ETH is not frozen");
    freeze(&mut w.ch, false).unwrap();
    let set = |ch: &mut Chain, p: Policy| ch.send("SetPolicy", br(admin().id), &[br(entry(&cfg, &usdt))], &[&admin()], &[], &Message::SetPolicy { config: cfg.clone(), eth_token: usdt, policy: p });
    set(&mut w.ch, Policy { mint_cap: 99_999_999, ..usdt_policy() }).unwrap();
    assert_eq!(refused(claim(&mut w.ch, &cfg, &c)), E_CAP);
    set(&mut w.ch, usdt_policy()).unwrap();
    claim(&mut w.ch, &cfg, &c).expect("unfrozen, under the cap");
    assert_eq!((balance(&w.ch, &cfg, alice().id, &usdt), balance(&w.ch, &cfg, bob().id, &ETH)), (100_000_000, b.amount));
}

/// F.5 outbound for an ERC-20: the holder burns a permitted USDT amount; the
/// leaf names the ERC-20, the supply falls, the escrow is empty again.
#[test]
fn an_erc20_burn_takes_the_wrapped_token_out_and_appends_its_leaf() {
    let (mut w, minted) = usdt_world();
    let cfg = w.cfg.clone();
    let usdt = w.a.usdt;
    let amount = usdt_policy().denominations[0];
    let ec = wrap_core::eth_commitment(&[0x42; 20], 0, &[0x46; 32]);
    let out = burn(&mut w.ch, &cfg, &alice(), usdt, amount, ec).expect("the burn lands");
    assert!(out.cycles < GAS_CAP);
    assert_eq!(balance(&w.ch, &cfg, alice().id, &usdt), minted - amount);
    assert_eq!(supply(&w.ch, &cfg, &usdt), minted - amount);
    assert_eq!(raw_balance(&w.ch, entry(&cfg, &usdt)), 0, "the entry's own holding is never used");
    let e = entry_state(&w, &usdt);
    assert_eq!((e.minted, e.burned), (minted, amount));
    let leaf = wrap_core::burn_leaf(&w.a.pairing, &usdt, amount, &ec);
    assert_ne!(leaf, wrap_core::burn_leaf(&w.a.pairing, &ETH, amount, &ec), "the leaf names the ERC-20");
    let f = tree(&w);
    assert_eq!((f.next_index, f.root), (1, wrap_core::root(&[leaf])));
    let sel = event("Burned", &Burned { leaf });
    assert!(out.events.iter().any(|e| e.event.selector == sel.selector && e.event.data == sel.data));
}

/// Burns of wrapped USDT refused: not a USDT denomination (an ETH one), more
/// than held, unsigned, and the wrong token both ways (wrapped USDT burned as
/// ETH, wrapped ETH burned as USDT): each entry's escrow takes only its own
/// definition.
#[test]
fn an_erc20_burn_is_refused_for_the_wrong_amount_or_token() {
    let (mut w, minted) = usdt_world();
    let cfg = w.cfg.clone();
    let usdt = w.a.usdt;
    let ec = [0x47; 32];
    assert_eq!(refused(burn(&mut w.ch, &cfg, &alice(), usdt, 12_345, ec)), E_AMOUNT);
    assert_eq!(refused(burn(&mut w.ch, &cfg, &alice(), usdt, eth_policy().denominations[0], ec)), E_AMOUNT, "an ETH denomination is not a USDT one");
    // 1,000 USDT is a denomination; alice holds 100.
    let e = burn(&mut w.ch, &cfg, &alice(), usdt, usdt_policy().denominations[1], ec).expect_err("more than held");
    assert!(refusal(&e).is_none(), "the token program refuses the debit, not the bridge: {e}");
    let m = burn_message(&cfg, usdt, usdt_policy().denominations[0], ec, alice().id);
    reverted_with(w.ch.send("Burn", br(burns(&cfg)), &burn_actors(&cfg, &usdt, alice().id), &[&bob()], &[], &m), "ATA owner must authorize spending");
    // Wrapped USDT burned as ETH: every ETH denomination is more than alice's
    // 100 USDT units, so the admin adds a 100-wei one for the test; the ETH
    // entry accepts the amount and burns from alice's wrapped-ETH holding,
    // which does not exist: her USDT is out of its reach.
    let mut p = eth_policy();
    p.denominations.push(100);
    w.ch.send("SetPolicy", br(admin().id), &[br(entry(&cfg, &ETH))], &[&admin()], &[], &Message::SetPolicy { config: cfg.clone(), eth_token: ETH, policy: p }).unwrap();
    let m = burn_message(&cfg, ETH, 100, ec, alice().id);
    let e = w.ch.send("Burn", br(burns(&cfg)), &burn_actors(&cfg, &ETH, alice().id), &[&alice()], &[], &m).expect_err("no wrapped ETH holding");
    assert!(refusal(&e).is_none(), "{e}");
    // Wrapped ETH burned as USDT: bob claims his ETH deposit, then burns 100
    // USDT: the USDT entry burns from his wrapped-USDT holding, which he does not have.
    let b = w.a.deposits[2].clone();
    claim(&mut w.ch, &cfg, &claim_message(&w.a, &cfg, &b, 4, bob().id, secret(&BOB_SEED, 0), SLOT)).unwrap();
    let e = burn(&mut w.ch, &cfg, &bob(), usdt, usdt_policy().denominations[0], ec).expect_err("no wrapped USDT holding");
    assert!(refusal(&e).is_none(), "{e}");
    // Nothing moved; the tree is empty.
    assert_eq!(balance(&w.ch, &cfg, alice().id, &usdt), minted);
    assert_eq!(balance(&w.ch, &cfg, bob().id, &ETH), b.amount);
    assert_eq!(supply(&w.ch, &cfg, &usdt), minted);
    assert_eq!(supply(&w.ch, &cfg, &ETH), b.amount);
    assert_eq!(tree(&w).next_index, 0);
}

/// With the ATA program, one account holds several wrapped tokens: alice
/// claims her ETH deposit and her USDT deposit into the same account (two ATA
/// holdings, each created by its claim), and burns each. Before the ATA, her
/// single token actor held wrapped ETH and the USDT claim reverted ("recipient
/// holds another token"); this test pinned that and is now its reverse.
#[test]
fn one_account_holds_several_wrapped_tokens() {
    let mut w = world();
    let cfg = w.cfg.clone();
    let usdt = w.a.usdt;
    let eth = w.a.deposits[0].clone();
    claim(&mut w.ch, &cfg, &claim_message(&w.a, &cfg, &eth, 4, alice().id, secret(&ALICE_SEED, 0), SLOT)).expect("wrapped ETH");
    let (d, c) = usdt_claim(&w);
    claim(&mut w.ch, &cfg, &c).expect("wrapped USDT, into the same account");
    let (he, hu) = (holding(&cfg, alice().id, &ETH), holding(&cfg, alice().id, &usdt));
    assert_ne!(he, hu, "two holdings");
    assert!(matches!(w.ch.token_state(he), Some(token::TokenState::Holding(h)) if h.definition_id == definition(&cfg, &ETH) && h.balance == eth.amount));
    assert!(matches!(w.ch.token_state(hu), Some(token::TokenState::Holding(h)) if h.definition_id == definition(&cfg, &usdt) && h.balance == d.amount));
    assert!(w.ch.token_state(alice().id).is_none(), "her own token actor holds nothing");
    burn(&mut w.ch, &cfg, &alice(), usdt, usdt_policy().denominations[0], [0x49; 32]).expect("burn the USDT");
    burn(&mut w.ch, &cfg, &alice(), ETH, eth_policy().denominations[0], [0x4a; 32]).expect("burn 0.1 ETH");
    assert_eq!((balance(&w.ch, &cfg, alice().id, &usdt), balance(&w.ch, &cfg, alice().id, &ETH)), (0, eth.amount - eth_policy().denominations[0]));
    assert_eq!((supply(&w.ch, &cfg, &usdt), supply(&w.ch, &cfg, &ETH)), (0, eth.amount - eth_policy().denominations[0]));
    assert_eq!(tree(&w).next_index, 2);
}

/// Cycles of the ERC-20 claim and burn (printed with --nocapture).
#[test]
fn erc20_cycles() {
    let (mut w, _) = usdt_world();
    let cfg = w.cfg.clone();
    let usdt = w.a.usdt;
    burn(&mut w.ch, &cfg, &alice(), usdt, usdt_policy().denominations[0], [0x48; 32]).unwrap();
    println!("| transaction (ERC-20: USDT, 6 decimals) | cycles | applied |");
    println!("|---|---:|---|");
    for (what, cycles, ok) in &w.ch.log {
        println!("| {what} | {cycles} | {ok} |");
        assert!(*cycles < GAS_CAP);
    }
}

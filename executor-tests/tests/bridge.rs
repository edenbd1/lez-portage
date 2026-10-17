//! wrap_bridge through `lee::V03State`, next to the committed sextant_lc and
//! token binaries, on deposits made into WrapVault on anvil (vectors/anvil).
//!
//! Every claim is sent by a brand-new account: the claimant needs no role and
//! no relationship to the depositor, which is what lets a relayer submit it.

use lee::AccountId;
use wrap_bridge::*;
use wrap_executor_tests::*;

fn refused(r: Result<u64, String>) -> u32 {
    let e = r.expect_err("must be refused");
    refusal(&e).unwrap_or_else(|| panic!("no typed refusal in: {e}"))
}

fn secret(seed: &[u8; 32], n: u64) -> [u8; 32] {
    wrap_core::derive_secret(seed, b'd', n)
}

/// sextant_lc's refusal code for a false predicate (`Error::PredicateFalse`).
const E_ATTEST_FALSE: u32 = sextant_program::E_BASE + sextant_core::Error::PredicateFalse as u32;

#[test]
fn the_token_instruction_mirror_is_token_cores_wire_format() {
    let def = AccountId::new([7; 32]);
    let pairs: Vec<(Vec<u8>, Vec<u8>)> = vec![
        (
            borsh::to_vec(&token::Instruction::Mint { amount_to_mint: 5 }).unwrap(),
            borsh::to_vec(&token_core::Instruction::Mint { amount_to_mint: 5 }).unwrap(),
        ),
        (
            borsh::to_vec(&token::Instruction::Burn { amount_to_burn: 9, kind: token::TokenKind::Fungible }).unwrap(),
            borsh::to_vec(&token_core::Instruction::Burn { amount_to_burn: 9, kind: token_core::TokenKind::Fungible }).unwrap(),
        ),
        (
            borsh::to_vec(&token::Instruction::NewFungibleDefinition { name: "w".into(), total_supply: 0 }).unwrap(),
            borsh::to_vec(&token_core::Instruction::NewFungibleDefinition { name: "w".into(), total_supply: 0 }).unwrap(),
        ),
        (
            borsh::to_vec(&token::TokenHolding::Fungible { definition_id: def, balance: 3 }).unwrap(),
            borsh::to_vec(&token_core::TokenHolding::Fungible { definition_id: def, balance: 3 }).unwrap(),
        ),
    ];
    for (mine, theirs) in pairs {
        assert_eq!(mine, theirs);
    }
}

#[test]
fn the_vectors_are_the_vaults_tree() {
    let a = anvil();
    // Every event's leaf is wrap_core::leaf of its commitment, and the roots
    // the vault reported are the roots of the prefixes.
    for (n, d) in a.deposits.iter().enumerate() {
        assert_eq!(d.index, n as u64);
        assert_eq!(d.leaf, wrap_core::leaf(ANVIL_CHAIN_ID, &a.vault, &a.pairing, &d.token, d.amount, &d.commitment));
        assert_eq!(a.proofs[n + 1].0, wrap_core::root(&a.leaves(n + 1)));
    }
    assert_eq!(a.proofs[0].0, wrap_core::zeros()[wrap_core::DEPTH]);
    // Each storage proof is the vault's knownRoot slot and holds 1, under the anvil block's state root.
    let state_root: [u8; 32] = h(&a.block["stateRoot"]);
    for (r, p) in &a.proofs {
        assert_eq!(p.address, a.vault);
        assert_eq!(p.slot, wrap_core::known_root_slot(r));
        let v: sextant_core::mpt::AccountSlot = borsh::from_slice(&sextant_core::mpt::verify_account_storage(&state_root, p).unwrap()).unwrap();
        assert_eq!(v.slot_value, wrap_core::word_u64(1));
    }
    let v: sextant_core::mpt::AccountSlot = borsh::from_slice(&sextant_core::mpt::verify_account_storage(&state_root, &a.unknown.1).unwrap()).unwrap();
    assert_eq!(v.slot_value, [0; 32]);
}

/// F.3, F.4 (public destination), R.1, R.3, Security #6: a deposit is minted
/// once, to the destination it committed to; the replay is refused, under the
/// same root or any other root that contains it.
#[test]
fn a_deposit_is_minted_once_to_its_committed_destination() {
    let mut w = world();
    let d = w.a.deposits[0].clone(); // alice, 1 ETH
    let c = claim_ix(&w.a, &w.cfg, &d, 4, alice().id, secret(&ALICE_SEED, 0), w.slot);
    let gas = claim(&mut w.ch, &w.cfg, &c).expect("the claim lands");
    assert!(gas < GAS_CAP, "one public transaction within the cap: {gas}");
    assert_eq!(balance(&w.ch, &w.cfg, &ETH, alice().id), d.amount);
    assert_eq!(supply(&w.ch, &w.cfg, &ETH), d.amount);

    let nf = wrap_core::nullifier(&w.a.pairing, &c.secret, c.index);
    let ca = config_account(&BR, &w.cfg);
    assert_eq!(w.ch.shard_of(nullifier_account(&BR, &ca, &nf), BR), vec![1]);
    let e: TokenEntry = borsh::from_slice(&w.ch.shard_of(entry_account(&BR, &ca, &ETH), BR)).unwrap();
    assert_eq!((e.minted, e.burned), (d.amount, 0));

    assert_eq!(refused(claim(&mut w.ch, &w.cfg, &c)), E_CLAIMED);
    // The same deposit under the root right after it (a one-leaf tree): the same nullifier.
    let early = claim_ix(&w.a, &w.cfg, &d, 1, alice().id, secret(&ALICE_SEED, 0), w.slot);
    assert_ne!(early.root, c.root);
    assert_eq!(refused(claim(&mut w.ch, &w.cfg, &early)), E_CLAIMED);
    assert_eq!(balance(&w.ch, &w.cfg, &ETH, alice().id), d.amount, "nothing more was minted");
}

/// The mint row may also be the committed account itself (the private path
/// mints to a fresh private account that way); a public claim can do it too.
#[test]
fn a_claim_may_mint_to_the_committed_account_itself() {
    let mut w = world();
    let d = w.a.deposits[0].clone();
    let c = claim_ix(&w.a, &w.cfg, &d, 4, alice().id, secret(&ALICE_SEED, 0), w.slot);
    let mut rows = claim_rows(&w.cfg, &c);
    rows[5] = sel(alice().id, TOKEN);
    w.ch.send(BR, &Instruction::Claim(Box::new(c)), rows).expect("the claim lands in the destination's own token shard");
    let held: token::TokenHolding = borsh::from_slice(&w.ch.shard_of(alice().id, TOKEN)).unwrap();
    assert!(matches!(held, token::TokenHolding::Fungible { balance, .. } if balance == d.amount));
    assert_eq!(balance(&w.ch, &w.cfg, &ETH, alice().id), 0, "nothing went to the ATA");
}

/// R.4: an old root stays a valid anchor; a claim built when the tree had one
/// leaf lands after three more deposits.
#[test]
fn a_claim_against_an_old_root_still_lands() {
    let mut w = world();
    let d = w.a.deposits[0].clone();
    let c = claim_ix(&w.a, &w.cfg, &d, 1, alice().id, secret(&ALICE_SEED, 0), w.slot);
    claim(&mut w.ch, &w.cfg, &c).expect("the claim lands against the first root");
    assert_eq!(balance(&w.ch, &w.cfg, &ETH, alice().id), d.amount);
}

/// Security #3 and #2: whoever submits cannot redirect the mint, change the
/// amount or token, or claim without the secret; an unknown root, another
/// contract's storage, or another slot is refused.
#[test]
fn a_submitter_cannot_redirect_inflate_or_forge() {
    let mut w = world();
    let d = w.a.deposits[0].clone();
    let good = claim_ix(&w.a, &w.cfg, &d, 4, alice().id, secret(&ALICE_SEED, 0), w.slot);

    let mut x = good.clone();
    x.destination = bob().id; // a relayer redirecting to itself
    assert_eq!(refused(claim(&mut w.ch, &w.cfg, &x)), E_PATH);
    let mut x = good.clone();
    x.amount *= 10;
    assert_eq!(refused(claim(&mut w.ch, &w.cfg, &x)), E_PATH);
    let mut x = good.clone();
    x.secret = secret(&BOB_SEED, 0);
    assert_eq!(refused(claim(&mut w.ch, &w.cfg, &x)), E_PATH);
    let mut x = good.clone();
    x.eth_token = w.a.usdt; // the ETH leaf claimed as USDT
    assert_eq!(refused(claim(&mut w.ch, &w.cfg, &x)), E_PATH);
    let mut x = good.clone();
    x.index = 1;
    assert_eq!(refused(claim(&mut w.ch, &w.cfg, &x)), E_PATH);

    // A tree the vault never had, with the proof of another slot.
    let mut x = good.clone();
    let fake = vec![d.leaf, [9; 32]];
    x.root = wrap_core::root(&fake);
    x.path = wrap_core::path(&fake, 0);
    assert_eq!(refused(claim(&mut w.ch, &w.cfg, &x)), E_PROOF, "the proof is of knownRoot[the real root], not of this one");

    // The right opening, but the mint row pointed at another account's holding,
    // or at another account itself.
    let mut rows = claim_rows(&w.cfg, &good);
    rows[5] = sel(ata(&w.cfg, &ETH, bob().id), TOKEN);
    assert_eq!(refused(w.ch.send(BR, &Instruction::Claim(Box::new(good.clone())), rows)), E_ACCOUNTS);
    let mut rows = claim_rows(&w.cfg, &good);
    rows[5] = sel(bob().id, TOKEN);
    assert_eq!(refused(w.ch.send(BR, &Instruction::Claim(Box::new(good.clone())), rows)), E_ACCOUNTS);

    let mut x = good.clone();
    x.proof.address = w.a.usdt;
    assert_eq!(refused(claim(&mut w.ch, &w.cfg, &x)), E_PROOF);
    let mut x = good.clone();
    x.proof.storage_proof.pop();
    assert_ne!(refused(claim(&mut w.ch, &w.cfg, &x)), 0, "a truncated storage proof is refused by Attest");

    assert_eq!(balance(&w.ch, &w.cfg, &ETH, alice().id), 0);
    assert_eq!(balance(&w.ch, &w.cfg, &ETH, bob().id), 0);
    claim(&mut w.ch, &w.cfg, &good).expect("the genuine claim still lands after every refusal (R.1)");
    assert_eq!(balance(&w.ch, &w.cfg, &ETH, alice().id), d.amount);
}

/// A root the vault never had, with its genuine storage proof (the slot holds
/// 0): the path and the slot check pass, and Sextant's Attest refuses the
/// predicate, so nothing mints.
#[test]
fn a_root_the_vault_never_had_is_refused_by_attest() {
    let mut w = world();
    let s = secret(&ALICE_SEED, 9); // vectors::PHANTOM_N: a deposit alice never made
    let cm = wrap_core::commitment(&w.a.pairing, alice().id.value(), &s);
    let leaf = wrap_core::leaf(ANVIL_CHAIN_ID, &w.a.vault, &w.a.pairing, &ETH, 1_000_000_000_000_000_000, &cm);
    assert_eq!(w.a.unknown.0, wrap_core::root(&[leaf]));
    let mut c = claim_ix(&w.a, &w.cfg, &w.a.deposits[0], 4, alice().id, s, w.slot);
    c.root = w.a.unknown.0;
    c.path = wrap_core::path(&[leaf], 0);
    c.index = 0;
    c.proof = w.a.unknown.1.clone();
    assert_eq!(refused(claim(&mut w.ch, &w.cfg, &c)), E_ATTEST_FALSE);
    assert_eq!(balance(&w.ch, &w.cfg, &ETH, alice().id), 0);
}

/// F.15, F.16: a second configuration (another deployment id, so another
/// pairing) cannot claim a deposit made for the first, and has its own accounts.
#[test]
fn a_deposit_for_one_pairing_is_not_claimable_in_another() {
    let mut w = world();
    let other = config(w.a.vault, *b"rfp022/test/deployment/000000002");
    assert_ne!(config_account(&BR, &other), config_account(&BR, &w.cfg));
    init_bridge(&mut w.ch, &other).unwrap();
    register(&mut w.ch, &admin(), &other, ETH, "Wrapped Ether (other)", SALT, eth_policy()).unwrap();
    let d = w.a.deposits[0].clone();
    let c = claim_ix(&w.a, &other, &d, 4, alice().id, secret(&ALICE_SEED, 0), w.slot);
    assert_eq!(refused(claim(&mut w.ch, &other, &c)), E_PATH);
    // A configuration bound to another vault cannot use this vault's proof either.
    let mut v2 = w.a.vault;
    v2[0] ^= 1;
    let third = config(v2, DEPLOYMENT_ID);
    init_bridge(&mut w.ch, &third).unwrap();
    register(&mut w.ch, &admin(), &third, ETH, "Wrapped Ether (third)", SALT, eth_policy()).unwrap();
    let c = claim_ix(&w.a, &third, &d, 4, alice().id, secret(&ALICE_SEED, 0), w.slot);
    assert_eq!(refused(claim(&mut w.ch, &third, &c)), E_PATH);
}

/// F.10, F.14, Security #4: only the admin registers and sets policy; only the
/// freezer freezes; a global or per-token mint freeze, and the supply cap,
/// refuse a claim and leave it claimable.
#[test]
fn admin_freeze_and_cap() {
    let mut w = world();
    let ca = config_account(&BR, &w.cfg);
    let usdt = w.a.usdt;
    assert_eq!(refused(register(&mut w.ch, &bob(), &w.cfg, [3; 20], "x", SALT, eth_policy())), E_NOT_ADMIN);

    let freeze = |ch: &mut Chain, k: &Key, cfg: &Config, tok: Option<[u8; 20]>, mint: bool| {
        let target = match tok {
            None => ca,
            Some(t) => entry_account(&BR, &ca, &t),
        };
        ch.send_as(k, BR, &Instruction::Freeze { config: cfg.clone(), eth_token: tok, mint, burn: false }, vec![sel(target, BR), sel(k.id, BR)])
    };
    assert_eq!(refused(freeze(&mut w.ch, &bob(), &w.cfg, None, true)), E_NOT_FREEZER);

    let d = w.a.deposits[1].clone(); // alice, 100 USDT
    let c = claim_ix(&w.a, &w.cfg, &d, 4, alice().id, secret(&ALICE_SEED, 1), w.slot);
    freeze(&mut w.ch, &freezer(), &w.cfg, None, true).unwrap();
    assert_eq!(refused(claim(&mut w.ch, &w.cfg, &c)), E_FROZEN);
    freeze(&mut w.ch, &freezer(), &w.cfg, None, false).unwrap();
    freeze(&mut w.ch, &freezer(), &w.cfg, Some(usdt), true).unwrap();
    assert_eq!(refused(claim(&mut w.ch, &w.cfg, &c)), E_FROZEN);
    freeze(&mut w.ch, &freezer(), &w.cfg, Some(usdt), false).unwrap();

    let set = |ch: &mut Chain, k: &Key, p: Policy| {
        ch.send_as(k, BR, &Instruction::SetPolicy { config: w.cfg.clone(), eth_token: usdt, policy: p }, vec![sel(ca, BR), sel(entry_account(&BR, &ca, &usdt), BR), sel(k.id, BR)])
    };
    assert_eq!(refused(set(&mut w.ch, &bob(), usdt_policy())), E_NOT_ADMIN);
    set(&mut w.ch, &admin(), Policy { mint_cap: 50_000_000, ..usdt_policy() }).unwrap();
    assert_eq!(refused(claim(&mut w.ch, &w.cfg, &c)), E_CAP);
    set(&mut w.ch, &admin(), usdt_policy()).unwrap();

    claim(&mut w.ch, &w.cfg, &c).expect("frozen and capped refusals consumed nothing (R.1)");
    assert_eq!(balance(&w.ch, &w.cfg, &usdt, alice().id), 100_000_000);
}

/// A definition squatted before registration (anyone may create a token
/// definition at an empty account) makes Register fail rather than adopt it,
/// and another salt registers cleanly.
#[test]
fn a_squatted_definition_is_refused_not_adopted() {
    let a = anvil();
    let mut ch = Chain::new();
    init_light_client(&mut ch, &devnet(&a.block, SLOT)).unwrap();
    let cfg = config(a.vault, DEPLOYMENT_ID);
    init_bridge(&mut ch, &cfg).unwrap();
    let ca = config_account(&BR, &cfg);
    let (def, _) = definition_account(&BR, &ca, &ETH, &SALT);
    let mallory = Key::new(666);
    ch.send_as(
        &mallory,
        TOKEN,
        &token::Instruction::NewFungibleDefinition { name: "fake".into(), total_supply: 1_000_000 },
        vec![sel(def, TOKEN), sel(mallory.id, TOKEN)],
    )
    .expect("the squat itself lands: the token program does not gate creation");
    assert!(register(&mut ch, &admin(), &cfg, ETH, "Wrapped Ether", SALT, eth_policy()).is_err());
    assert!(ch.shard_of(entry_account(&BR, &ca, &ETH), BR).is_empty(), "nothing was registered");
    register(&mut ch, &admin(), &cfg, ETH, "Wrapped Ether", [0x77; 32], eth_policy()).expect("another salt");
}

/// F.5, F.6 (public holder), R.2: a burn destroys the owner's tokens (from its
/// associated token account, through the ATA program) and
/// supply, records `burn_leaf` with the Ethereum recipient hidden in a
/// commitment, and only a permitted amount, by the holder, when not frozen.
#[test]
fn a_burn_destroys_supply_and_records_a_hidden_recipient() {
    let mut w = world();
    let d = w.a.deposits[0].clone();
    claim(&mut w.ch, &w.cfg, &claim_ix(&w.a, &w.cfg, &d, 4, alice().id, secret(&ALICE_SEED, 0), w.slot)).unwrap();
    let ca = config_account(&BR, &w.cfg);
    let recipient = [0xee; 20];
    let bsecret = wrap_core::derive_secret(&ALICE_SEED, b'b', 0);
    let ec = wrap_core::eth_commitment(&recipient, 0, &bsecret);
    let burn = |ch: &mut Chain, k: &Key, amount: u128| {
        ch.send_as(
            k,
            BR,
            &Instruction::Burn { config: w.cfg.clone(), eth_token: ETH, salt: SALT, amount, eth_commitment: ec },
            vec![
                sel(ca, BR),
                sel(entry_account(&BR, &ca, &ETH), BR),
                sel(burns_account(&BR, &ca), BR),
                sel(definition_account(&BR, &ca, &ETH, &SALT).0, TOKEN),
                sel(k.id, BR),
                sel(ata(&w.cfg, &ETH, k.id), TOKEN),
            ],
        )
    };
    assert_eq!(refused(burn(&mut w.ch, &alice(), 300_000_000_000_000_000)), E_AMOUNT);
    // Bob holds nothing: the token program refuses his burn.
    assert!(burn(&mut w.ch, &bob(), 1_000_000_000_000_000_000).is_err());
    w.ch.send_as(&freezer(), BR, &Instruction::Freeze { config: w.cfg.clone(), eth_token: None, mint: false, burn: true }, vec![sel(ca, BR), sel(freezer().id, BR)]).unwrap();
    assert_eq!(refused(burn(&mut w.ch, &alice(), 1_000_000_000_000_000_000)), E_FROZEN);
    w.ch.send_as(&freezer(), BR, &Instruction::Freeze { config: w.cfg.clone(), eth_token: None, mint: false, burn: false }, vec![sel(ca, BR), sel(freezer().id, BR)]).unwrap();
    let entry = entry_account(&BR, &ca, &ETH);
    w.ch.send_as(&freezer(), BR, &Instruction::Freeze { config: w.cfg.clone(), eth_token: Some(ETH), mint: false, burn: true }, vec![sel(entry, BR), sel(freezer().id, BR)]).unwrap();
    assert_eq!(refused(burn(&mut w.ch, &alice(), 1_000_000_000_000_000_000)), E_FROZEN);
    w.ch.send_as(&freezer(), BR, &Instruction::Freeze { config: w.cfg.clone(), eth_token: Some(ETH), mint: false, burn: false }, vec![sel(entry, BR), sel(freezer().id, BR)]).unwrap();

    let gas = burn(&mut w.ch, &alice(), 1_000_000_000_000_000_000).expect("the burn lands");
    assert!(gas < GAS_CAP);
    assert_eq!(balance(&w.ch, &w.cfg, &ETH, alice().id), 0);
    assert_eq!(supply(&w.ch, &w.cfg, &ETH), 0);

    // Burning from someone else's ATA is refused: the ATA must be the signing owner's.
    claim(&mut w.ch, &w.cfg, &claim_ix(&w.a, &w.cfg, &w.a.deposits[3].clone(), 4, alice().id, secret(&ALICE_SEED, 2), w.slot)).unwrap();
    let theft = w.ch.send_as(
        &bob(),
        BR,
        &Instruction::Burn { config: w.cfg.clone(), eth_token: ETH, salt: SALT, amount: 100_000_000_000_000_000, eth_commitment: ec },
        vec![
            sel(ca, BR),
            sel(entry_account(&BR, &ca, &ETH), BR),
            sel(burns_account(&BR, &ca), BR),
            sel(definition_account(&BR, &ca, &ETH, &SALT).0, TOKEN),
            sel(bob().id, BR),
            sel(ata(&w.cfg, &ETH, alice().id), TOKEN),
        ],
    );
    assert_eq!(refused(theft), E_ACCOUNTS);
    assert_eq!(balance(&w.ch, &w.cfg, &ETH, alice().id), 100_000_000_000_000_000);
    let f: wrap_core::Frontier = borsh::from_slice(&w.ch.shard_of(burns_account(&BR, &ca), BR)).unwrap();
    let leaf = wrap_core::burn_leaf(&w.a.pairing, &ETH, 1_000_000_000_000_000_000, &ec);
    assert_eq!((f.next_index, f.root), (1, wrap_core::root(&[leaf])));
    let e: TokenEntry = borsh::from_slice(&w.ch.shard_of(entry_account(&BR, &ca, &ETH), BR)).unwrap();
    assert_eq!((e.minted, e.burned), (d.amount + 100_000_000_000_000_000, d.amount));
    // Nothing the burn wrote names the recipient: the tree holds a hash of it.
    for id in [burns_account(&BR, &ca), entry_account(&BR, &ca, &ETH), ca] {
        assert!(!w.ch.shard_of(id, BR).windows(20).any(|x| x == recipient));
    }
}

/// F.9, R.5: from the seed alone and the vault's public events, a client finds
/// every one of its deposits, tells claimed from unclaimed by the nullifier
/// accounts, and claims the rest. No other state is kept between the steps.
#[test]
fn positions_are_recovered_from_the_seed_alone() {
    let mut w = world();
    // Before losing everything, alice claimed her first deposit.
    let first = w.a.deposits[0].clone();
    claim(&mut w.ch, &w.cfg, &claim_ix(&w.a, &w.cfg, &first, 4, alice().id, secret(&ALICE_SEED, 0), w.slot)).unwrap();

    // Recovery: scan secrets 0.. until a gap of 5 unmatched counters.
    let pair = wrap_bridge::pairing(&BR, &w.cfg);
    let ca = config_account(&BR, &w.cfg);
    let mut found = vec![];
    let (mut n, mut misses) = (0u64, 0);
    while misses < 5 {
        let s = secret(&ALICE_SEED, n);
        let cm = wrap_core::commitment(&pair, alice().id.value(), &s);
        match w.a.deposits.iter().find(|d| d.commitment == cm) {
            Some(d) => {
                let spent = !w.ch.shard_of(nullifier_account(&BR, &ca, &wrap_core::nullifier(&pair, &s, d.index)), BR).is_empty();
                found.push((d.index, s, spent));
                misses = 0;
            }
            None => misses += 1,
        }
        n += 1;
    }
    let summary: Vec<(u64, bool)> = found.iter().map(|(i, _, sp)| (*i, *sp)).collect();
    assert_eq!(summary, vec![(0, true), (1, false), (3, false)], "alice's three deposits, not bob's");

    for (i, s, spent) in found {
        if !spent {
            let d = w.a.deposits[i as usize].clone();
            claim(&mut w.ch, &w.cfg, &claim_ix(&w.a, &w.cfg, &d, 4, alice().id, s, w.slot)).expect("a recovered claim lands");
        }
    }
    assert_eq!(supply(&w.ch, &w.cfg, &ETH), 1_100_000_000_000_000_000);
    assert_eq!(supply(&w.ch, &w.cfg, &w.a.usdt), 100_000_000);
    // One owner, two wrapped tokens: one associated token account each.
    assert_eq!(balance(&w.ch, &w.cfg, &ETH, alice().id), 1_100_000_000_000_000_000);
    assert_eq!(balance(&w.ch, &w.cfg, &w.a.usdt, alice().id), 100_000_000);
}

/// F.6: `BurnFrom` burns from the account that holds the wrapped token itself
/// (on the private path, a private account; here alice's own public account,
/// which a claim minted into directly), only when that holding is authorized,
/// and records the same burn leaf as `Burn`.
#[test]
fn burn_from_takes_the_tokens_from_the_authorized_holding_only() {
    let mut w = world();
    let d = w.a.deposits[0].clone();
    let c = claim_ix(&w.a, &w.cfg, &d, 4, alice().id, secret(&ALICE_SEED, 0), w.slot);
    let mut rows = claim_rows(&w.cfg, &c);
    rows[5] = sel(alice().id, TOKEN);
    w.ch.send(BR, &Instruction::Claim(Box::new(c)), rows).expect("minted into alice's own account");
    let ca = config_account(&BR, &w.cfg);
    let ec = wrap_core::eth_commitment(&[0xee; 20], 0, &wrap_core::derive_secret(&ALICE_SEED, b'b', 0));
    let ix = |amount: u128| Instruction::BurnFrom { config: w.cfg.clone(), eth_token: ETH, salt: SALT, amount, eth_commitment: ec };
    let one = 1_000_000_000_000_000_000u128;

    // Bob signs, naming alice's holding: not authorized.
    assert_eq!(refused(w.ch.send_as(&bob(), BR, &ix(one), burn_from_rows(&w.cfg, &ETH, alice().id))), E_NOT_HOLDER);
    // The holding row must be the token program's shard of the account.
    let mut wrong_shard = burn_from_rows(&w.cfg, &ETH, alice().id);
    wrong_shard[4] = sel(alice().id, BR);
    assert_eq!(refused(w.ch.send_as(&alice(), BR, &ix(one), wrong_shard)), E_ACCOUNTS);
    // The burn tree row must be this deployment's.
    let mut wrong_tree = burn_from_rows(&w.cfg, &ETH, alice().id);
    wrong_tree[2] = sel(nullifier_account(&BR, &ca, &[9; 32]), BR);
    assert_eq!(refused(w.ch.send_as(&alice(), BR, &ix(one), wrong_tree)), E_ACCOUNTS);
    let mut wrong_cfg = burn_from_rows(&w.cfg, &ETH, alice().id);
    wrong_cfg[0] = sel(entry_account(&BR, &ca, &w.a.usdt), BR);
    assert_eq!(refused(w.ch.send_as(&alice(), BR, &ix(one), wrong_cfg)), E_ACCOUNTS);
    // Only a permitted amount.
    assert_eq!(refused(w.ch.send_as(&alice(), BR, &ix(300_000_000_000_000_000), burn_from_rows(&w.cfg, &ETH, alice().id))), E_AMOUNT);
    assert_eq!(supply(&w.ch, &w.cfg, &ETH), one, "nothing burned yet");

    w.ch.send_as(&alice(), BR, &ix(one), burn_from_rows(&w.cfg, &ETH, alice().id)).expect("alice burns her own holding");
    let held: token::TokenHolding = borsh::from_slice(&w.ch.shard_of(alice().id, TOKEN)).unwrap();
    assert!(matches!(held, token::TokenHolding::Fungible { balance: 0, .. }));
    assert_eq!(supply(&w.ch, &w.cfg, &ETH), 0);
    let f: wrap_core::Frontier = borsh::from_slice(&w.ch.shard_of(burns_account(&BR, &ca), BR)).unwrap();
    assert_eq!((f.next_index, f.root), (1, wrap_core::root(&[wrap_core::burn_leaf(&w.a.pairing, &ETH, one, &ec)])));
    let e: TokenEntry = borsh::from_slice(&w.ch.shard_of(entry_account(&BR, &ca, &ETH), BR)).unwrap();
    assert_eq!((e.minted, e.burned), (one, one));
}

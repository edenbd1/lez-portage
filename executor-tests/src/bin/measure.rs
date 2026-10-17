//! The gas of every wrap_bridge instruction, as `lee::V03State` charges it
//! (cycles, one for one, against the 10,000,000 per-transaction cap), and the
//! share of a claim that is Sextant's chained `Attest` (P.1).
//!
//! `cargo run --release --bin measure`

use wrap_bridge::*;
use wrap_executor_tests::*;

fn row(what: &str, gas: u64) {
    println!("| {what} | {gas} | {:.2}% |", gas as f64 * 100.0 / GAS_CAP as f64);
}

fn main() {
    let a = anvil();
    let mut ch = Chain::new();
    let d = devnet(&a.block, SLOT);
    let g_lc = init_light_client(&mut ch, &d).unwrap();
    let cfg = config(a.vault, DEPLOYMENT_ID);
    let g_init = init_bridge(&mut ch, &cfg).unwrap();
    let g_reg = register(&mut ch, &admin(), &cfg, ETH, "Wrapped Ether (anvil)", SALT, eth_policy()).unwrap();
    register(&mut ch, &admin(), &cfg, a.usdt, "Wrapped USDT (anvil)", SALT, usdt_policy()).unwrap();

    let s0 = wrap_core::derive_secret(&ALICE_SEED, b'd', 0);
    let c = claim_ix(&a, &cfg, &a.deposits[0], 4, alice().id, s0, SLOT);
    let g_claim = claim(&mut ch, &cfg, &c).unwrap();
    let bytes = borsh::to_vec(&Instruction::Claim(Box::new(c.clone()))).unwrap().len();

    // The same Attest on its own, sent to sextant_lc directly.
    let attest = sextant_program::Instruction::Attest { instance_id: INSTANCE, slot: SLOT, query: query(&cfg, c.proof.clone()) };
    let g_attest = ch.send(LC, &attest, vec![sel(sextant_program::header_account(&LC, &INSTANCE, SLOT), LC)]).unwrap();

    let s1 = wrap_core::derive_secret(&ALICE_SEED, b'd', 1);
    let g_claim_usdt = claim(&mut ch, &cfg, &claim_ix(&a, &cfg, &a.deposits[1], 4, alice().id, s1, SLOT)).unwrap();

    let ca = config_account(&BR, &cfg);
    let ec = wrap_core::eth_commitment(&[0xee; 20], 0, &wrap_core::derive_secret(&ALICE_SEED, b'b', 0));
    let g_burn = ch
        .send_as(
            &alice(),
            BR,
            &Instruction::Burn { config: cfg.clone(), eth_token: ETH, salt: SALT, amount: 1_000_000_000_000_000_000, eth_commitment: ec },
            vec![
                sel(ca, BR),
                sel(entry_account(&BR, &ca, &ETH), BR),
                sel(burns_account(&BR, &ca), BR),
                sel(definition_account(&BR, &ca, &ETH, &SALT).0, TOKEN),
                sel(alice().id, BR),
                sel(ata(&cfg, &ETH, alice().id), TOKEN),
            ],
        )
        .unwrap();
    let g_freeze = ch
        .send_as(&freezer(), BR, &Instruction::Freeze { config: cfg.clone(), eth_token: Some(ETH), mint: true, burn: false }, vec![sel(entry_account(&BR, &ca, &ETH), BR), sel(freezer().id, BR)])
        .unwrap();

    println!("wrap_bridge gas, LEZ v0.3 (lee db66590a), anvil vectors at block {}", a.block["number"].as_str().unwrap());
    println!();
    println!("| transaction | gas | share of the 10,000,000 cap |");
    println!("|---|---:|---:|");
    row("sextant_lc Init (devnet checkpoint)", g_lc);
    row("Init (bridge configuration)", g_init);
    row("Register (entry + chained token NewFungibleDefinition)", g_reg);
    row("Claim, 1 ETH (path + chained Attest + chained token Mint)", g_claim);
    row("Claim, 100 USDT", g_claim_usdt);
    row("  of which: the same Attest sent alone", g_attest);
    row("Burn, 1 ETH (burn-tree append + chained ATA Burn -> token Burn)", g_burn);
    row("Freeze (one token)", g_freeze);
    println!();
    println!("Claim instruction: {bytes} bytes (the storage proof is {} account nodes, {} storage nodes; the path is 24 x 32 bytes)", c.proof.account_proof.len(), c.proof.storage_proof.len());
    println!("Nullifier account: 1 byte; token entry: {} bytes; burn tree frontier: {} bytes", ch.shard_of(entry_account(&BR, &ca, &ETH), BR).len(), ch.shard_of(burns_account(&BR, &ca), BR).len());
}

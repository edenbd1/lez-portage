//! What scripts/anvil-vectors.sh needs from the LEZ side before it deposits:
//! the pairing to deploy the vault with, and each test deposit's commitment,
//! derived from the depositor's seed and LEZ account exactly as a wallet would.
//!
//! `cargo run --release --bin vectors` prints one JSON object.

use wrap_executor_tests::*;

/// `(depositor, seed counter, token, amount)`, in deposit order. Carol's
/// deposit commits to a private LEZ account (`carol_private()`), the one the
/// privacy-preserving claim mints to (tests/private.rs, bin measure_private).
pub const PLAN: [(&str, u64, &str, u128); 5] = [
    ("alice", 0, "eth", 1_000_000_000_000_000_000),
    ("alice", 1, "usdt", 100_000_000),
    ("bob", 0, "eth", 1_000_000_000_000_000_000),
    ("alice", 2, "eth", 100_000_000_000_000_000),
    ("carol", 0, "eth", 1_000_000_000_000_000_000),
];

/// The seed counter of the deposit alice never made.
pub const PHANTOM_N: u64 = 9;

fn hx(b: &[u8]) -> String {
    format!("0x{}", hex::encode(b))
}

fn main() {
    // The vault address is not part of the pairing (the LEZ config pins the
    // vault; the vault pins the LEZ deployment), so it can be computed first.
    let pairing = wrap_core::pairing(&LEZ_CHAIN_ID, &ZONE_ID, BR.value(), &DEPLOYMENT_ID);
    let deposits: Vec<String> = PLAN
        .iter()
        .map(|(who, n, token, amount)| {
            let (seed, dest) = match *who {
                "alice" => (ALICE_SEED, alice().id),
                "bob" => (BOB_SEED, bob().id),
                _ => (CAROL_SEED, carol_private().id()),
            };
            let secret = wrap_core::derive_secret(&seed, b'd', *n);
            let c = wrap_core::commitment(&pairing, dest.value(), &secret);
            format!(r#"{{"from":"{who}","token":"{token}","amount":"{amount}","commitment":"{}"}}"#, hx(&c))
        })
        .collect();
    // A deposit alice could have made and did not: its one-leaf tree's root is
    // never a vault root, so a claim of it carries a genuine proof of value 0.
    let phantom = wrap_core::commitment(&pairing, alice().id.value(), &wrap_core::derive_secret(&ALICE_SEED, b'd', PHANTOM_N));
    println!(r#"{{"pairing":"{}","phantom":"{}","deposits":[{}]}}"#, hx(&pairing), hx(&phantom), deposits.join(","));
}

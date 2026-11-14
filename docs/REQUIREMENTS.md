# RFP-022 requirements, one by one

Status of this repository against the hard requirements of RFP-022
(Privacy-Preserving Wrapped ERC-20 and Ether Bridge for LEZ), on 2026-11-14,
branch `main`. IDs are the RFP's: F = Functionality, U = Usability,
R = Reliability, P = Performance, S = Supportability, Sec = Security,
PP = Privacy Preservation.

**Status**, exactly one per row:

- **Met**: built, and backed by a named test or a recorded run in this
  repository. The evidence column says where it ran: *executor* (the
  committed binaries through `lee::V03State`), *Foundry*, *local* (a local
  LEZ v0.3.1 sequencer, anvil, a local Bedrock devnet) or *public* (Sepolia
  and the public LEZ testnet, [TESTNET.md](TESTNET.md)). Local evidence is
  not public-testnet evidence; S.1 tracks the difference.
- **Partial**: some of it is built and evidenced; the rest, and the
  milestone it falls in, is under "remaining".
- **Planned in Mx**: nothing in this repository meets it yet.

**Milestones:** M0 design freeze, threat model, privacy model, F.7
measurements, decision log; M1 inbound on public testnets; M2 outbound on
public testnets; M3 Logos core module, mini-app and admin UI, headless
daemon, catalog, Linux and macOS; M4 privacy suite, performance on LEZ 0.4,
end-to-end CI, IDL, documentation, audit freeze; M5 audit remediation and
re-verification.

Test names are function names: Foundry tests in `vault/test/`, executor
tests in `executor-tests/tests/`. Everything here runs on LEZ v0.3 (the
public testnet is v0.3.1); a port to LEZ v0.4.0-rc1 is on the `v0.4-spike`
branch and is not counted.

## Functionality

| ID | requirement | status | evidence | remaining |
|---|---|---|---|---|
| F.1 | Vault escrows registered ERC-20s and native ETH, no wrapping by the user | Met | [`WrapVault.sol`](../vault/src/WrapVault.sol) (`ETH = address(0)`); `test_deposit_eth_directly_without_wrapping`, `test_deposit_6_and_18_decimal_tokens` (Foundry); public: two 0.001 ETH deposits on Sepolia, [`0x8385eedf…`](https://sepolia.etherscan.io/tx/0x8385eedf92b93fba39939376e17717da29a0ca77c329cba65e7b9f3e5ac8dbfb), [`0x2508a663…`](https://sepolia.etherscan.io/tx/0x2508a66321461e382fe7c00fe5cff9ca9232ef089ecde500bebcb92934207fdd) | USDC, USDT, DAI, WETH registered on Sepolia (M1) |
| F.2 | A deposit reveals no LEZ destination | Met | `deposit(token, amount, commitment)`, the `Deposit` event carries index, leaf, commitment, token, amount, root; `test_leaf_binds_chain_vault_pairing_token_amount_commitment` (Foundry); public: the two Sepolia deposits, re-checked by `scripts/verify-sepolia.py` | |
| F.3 | LEZ program mints on verification by RFP-021's attestation; uniqueness keyed on the attestation's statement id | Partial | `Claim` chains Sextant's `Attest` of `knownRoot[root] == 1`, then the token `Mint` ([`lib.rs`](../programs/wrap_bridge/src/lib.rs)); `a_deposit_is_minted_once_to_its_committed_destination`, `a_root_the_vault_never_had_is_refused_by_attest` (executor); public: [claim `834a3460…`](https://explorer.testnet.lez.logos.co/transaction/834a346002e0410e5248a45e4ac79ce92bda88e327c3d8c7f1611b6ee5881752), replay refused `E7402` [`b0c840a2…`](https://explorer.testnet.lez.logos.co/transaction/b0c840a246afbc3b58a5bf093ac499bc4f43910950a09b88b8e4deee37370579) | Uniqueness is keyed on a hidden per-deposit nullifier, not on the statement id: the attested statement ("`knownRoot[R]` is 1") is shared by every deposit under `R`, and a per-log id would link the mint to its deposit (PP.1). The deviation is to be recorded in the decision log (M0) |
| F.4 | Mint to a private or a public LEZ account | Met | `a_deposit_is_minted_once_to_its_committed_destination` (public), `a_private_claim_mints_to_a_private_account_and_publishes_no_opening` (executor, dev mode); public: [public claim `834a3460…`](https://explorer.testnet.lez.logos.co/transaction/834a346002e0410e5248a45e4ac79ce92bda88e327c3d8c7f1611b6ee5881752), [private claim `4f6dfe0a…`](https://explorer.testnet.lez.logos.co/transaction/4f6dfe0ab587d1c15f6c4292c8be9183d8f1ef036b250f67fc70bdebfa837ca3), decrypted by the wallet | |
| F.5 | Burn path that hides the Ethereum destination | Met | `Burn` / `BurnFrom` append `burn_leaf(token, amount, eth_commitment)`; `a_burn_destroys_supply_and_records_a_hidden_recipient` (executor); local: `burn-alice-eth`, `burn-carol-private` ([LOCAL-E2E.md](LOCAL-E2E.md)) | Not yet on a public network (S.1, M2) |
| F.6 | Burn from a private or a public account | Partial | `a_burn_destroys_supply_and_records_a_hidden_recipient`, `burn_from_takes_the_tokens_from_the_authorized_holding_only`, `a_private_burn_spends_a_private_holding_and_publishes_no_holder` (executor); local: the private burn, 202.9 s, 248,358 B ([OUTBOUND.md](OUTBOUND.md) §1) | Both burns work, but only private burns can be released through `ZkChainBurnVerifier` and `ZkBedrockBurnVerifier`: a reverted public transaction stays in its v0.3.1 block unmarked, so public burns are left out of the tree rebuilt from blocks. Include them (M2) |
| F.7 | Vault releases on cryptographic verification of a LEZ burn, natively on Ethereum | Partial | `ZkBedrockBurnVerifier`: one Groth16 seal proves the burn is in a tree rebuilt from LEZ blocks and the last block is inscribed on Bedrock by the sequencer key, k headers deep; 19 Foundry tests on a real seal; local: 15 releases, 1 applied, 14 refused ([`evidence/local-bedrock/`](../evidence/local-bedrock/), [BEDROCK-FINALITY.md](BEDROCK-FINALITY.md) §8) | **Not met as worded.** A 2-of-3 `BedrockHeaderCommittee` attests the top Bedrock header canonical; Bedrock's leader signature and the channel key's accreditation, LEZ block validity and one block per height are not proven (README, "What is still trusted"). A light client costs about 240 to 330 billion cycles per day ([POL-MEASURED.md](POL-MEASURED.md)). Bonded, slashable attestation, freshness check, lineage, public burns, on Sepolia (M2) |
| F.8 | Amounts on Ethereum do not identify the mint or burn; mechanism and anonymity-set effect stated | Partial | Fixed denominations on deposit, burn and release: `test_only_permitted_amounts_and_listed_tokens` (Foundry), `burn-alice-eth-not-a-denomination` refused `E7408` (local); anonymity set stated in [PRIVATE-CLAIM-MEASURED.md](PRIVATE-CLAIM-MEASURED.md) and [OUTBOUND.md](OUTBOUND.md) §3 | The relayer fee is any value up to the amount, chosen per burn, so it can fingerprint a release (the local run used 0.01 ETH); fixed fee tiers per denomination (M1, M2). Anonymity-set sizing (M4) |
| F.9 | Recover unclaimed deposits and unreleased burns from existing credentials only | Partial | Secrets derived from the seed (`wrap_core::derive_secret`); `positions_are_recovered_from_the_seed_alone` (executor): scans deposits, tells claimed from unclaimed by the nullifier accounts, claims the rest | Inbound only, at the executor level, for a public destination known to the test; private destinations, outbound recovery (M2); a recovery command in the client and a wipe-and-recover test (M3) |
| F.10 | Admin registers and deregisters tokens; registration-change documentation | Partial | Vault: `registerToken`, `delistToken`; `test_only_admin_registers_and_delisting_keeps_releases_open`, `test_registration_rejects_amounts_lez_cannot_represent` (Foundry). LEZ: `Register`, `SetPolicy`; `admin_freeze_and_cap`, `a_squatted_definition_is_refused_not_adopted` (executor); public: [`registerToken` `0x1c8f7105…`](https://sepolia.etherscan.io/tx/0x1c8f710574e9a050497cb0b80af03c2d26b0565749b7d2b04cca1a4ff497df12), [`Register` `3ad79714…`](https://explorer.testnet.lez.logos.co/transaction/3ad7971422cec2a73fa1dcf901cbaf14ea6966e199e7000900a25a00a2d121c6) | No deregistration on LEZ (`SetPolicy` and freezes only). The admin is a single-account stand-in for RFP-001: fixed in the LEZ configuration, no transfer on the vault. Vault and LEZ denominations are set separately and not cross-checked. Registration-change and delisting runbook (M1, M4) |
| F.11 | Global and per-token deposit and redemption caps over a rolling window | Partial | Vault: per token, per direction, over a fixed window set at deployment (1 day on Sepolia, cap 10 denominations); `test_deposit_cap_per_window`, `test_release_cap_per_window` (Foundry). LEZ: an outstanding-supply cap per token (`mint_cap`), `admin_freeze_and_cap` (executor) | Rolling windows on both halves (on LEZ, the 50-block clock), a global cap (M1) |
| F.12 | Finality depth configurable per deployment; a change never invalidates a valid claim | Partial | Inbound: claims read only headers Sextant finalized (Casper finality). Outbound: `MIN_DEPTH` of `ZkBedrockBurnVerifier` and `FINALITY_DEPTH` of `CommitteeBurnVerifier` are constructor arguments; `test_an_anchor_shallower_than_min_depth_is_refused`, `test_a_burn_not_yet_final_by_the_configured_depth_is_refused` (Foundry); k = 30 (about 600 s) in [BEDROCK-FINALITY.md](BEDROCK-FINALITY.md) §2 | No extra inbound depth setting; the outbound depth is immutable per verifier. Admin-set depth schedule and recommended defaults per direction with residual risk (M1, M2) |
| F.13 | Protocol fee on mint and burn, admin-set denomination and rate, can be zero | Planned in M1 | | No protocol fee on either half. Mint side in M1, burn side in M2 |
| F.14 | Freeze minting and/or redemption, globally or per token, on each half independently | Met | Vault `freeze`, `freezeAll`: `test_freeze_per_token_and_global_each_direction_independently` (Foundry); LEZ `Freeze`: `admin_freeze_and_cap` (executor) | The freezer is a stand-in for RFP-002 (see below) |
| F.15 | Each half pins the other (chain id and vault; LEZ chain, zone, program); any EVM chain; pairing checked in verification | Met | `pairing = sha256(LEZ chain ‖ zone ‖ program ‖ deployment)` in every leaf and release digest; the LEZ configuration pins chain id and vault; `a_deposit_for_one_pairing_is_not_claimable_in_another` (executor), `test_the_digest_binds_this_vault_and_chain`, `test_another_pairing_is_refused` (Foundry); the same contract on anvil and on Sepolia (chain 11155111) | |
| F.16 | Independent configurations with strict separation; how a client identifies and switches | Partial | The configuration account is a hash of the whole `Config`; every account the program writes derives from it; `a_deposit_for_one_pairing_is_not_claimable_in_another` checks separate accounts | No test that one configuration's admin or freezer cannot act on another (M1). The pairing commits to the deployment id, not to the whole `Config`: two configurations with the same deployment id share a pairing, so a depositor could claim one deposit in each; the second configuration's burns are not in the burn tree the chain-derived verifiers pin (`BURNS_ACCOUNT`). Bind the pairing to the configuration (M1). Client-side identification and switching documented (M4) |
| F.17 | Deposit and release ERC-20s whose transfers return no boolean (USDT) | Met | `test_deposit_and_release_usdt_which_returns_nothing`, `test_a_token_returning_false_is_refused`, `test_a_fee_on_transfer_token_is_refused` (Foundry); local: a USDT-shaped token deposited and claimed (`claim-alice-usdt`) | USDT on Sepolia (M1) |

## Usability

| ID | requirement | status | evidence | remaining |
|---|---|---|---|---|
| U.1 | Logos core module for every user and admin function | Planned in M3 | | The flows exist only in `wrap` ([`cli/`](../cli/)), a run harness with fixed actors, not a module |
| U.2 | Logos ui module (mini-app): deposit, redemption, recovery, registry view; admin UI | Planned in M3 | | |
| U.3 | Long-running components as a Logos module with a headless CLI/daemon; operator journey | Planned in M3 | | No daemon: the chain and anchor prover, the committee members and the release submitter are scripts in this repo |
| U.4 | IDL for the LEZ program in the SPEL format | Planned in M4 | | SPEL builds LEZ 0.2.x programs only |
| U.5 | Amount, timing and fee-payer mitigations on by default; leak indicator | Planned in M3 | | Fixed denominations are enforced by the contracts (F.8); no delay default, no relayer, no indicator |
| U.6 | Private account pre-selected for mint and burn; public needs an explicit action | Planned in M3 | | `wrap claim` is public unless `--private` is given |
| U.7 | Documentation and UI explain what is public and private at each step | Partial | [PRIVATE-CLAIM-MEASURED.md](PRIVATE-CLAIM-MEASURED.md) "What is public and what is hidden"; [OUTBOUND.md](OUTBOUND.md) §3 "What the zero-knowledge release reveals"; [TESTNET.md](TESTNET.md) "What this shows" | Developer-level only; user documentation per step, in the UI (M3) and the docs (M4) |
| U.8 | Actionable errors for every failure mode, naming no deposit or burn | Partial | LEZ: typed `E74xx` codes with text (`E7401` frozen, `E7402` already claimed, `E7403` path, `E7404` proof, `E7407` unsupported, `E7408` amount, `E7409` cap) in [`lib.rs`](../programs/wrap_bridge/src/lib.rs), Sextant's `E7208` for an unknown root; vault custom errors (`UnsupportedToken`, `InvalidAmount`, `CapExceeded`, `IsFrozen`, `AlreadyReleased`, `VerificationFailed`) and `NotFinal` in the verifiers | No typed code for insufficient finality on the inbound side; user-facing messages in the module and mini-app (M3) |
| U.9 | UI lets the user change the Ethereum RPC and the LEZ sequencer or zone | Planned in M3 | | |
| U.10 | Modules published in the team's own catalog from the Logos template | Planned in M3 | | |

## Reliability

| ID | requirement | status | evidence | remaining |
|---|---|---|---|---|
| R.1 | A failed claim leaves the deposit claimable and consumes nothing | Met | `admin_freeze_and_cap` (a frozen or capped claim is refused, then lands), `a_refused_private_claim_fails_before_it_reaches_the_chain` (executor); local: refusals charged their limit and change nothing ([LOCAL-E2E.md](LOCAL-E2E.md), "How a verdict is read") | |
| R.2 | Redemption atomic at each stage: a failed burn or release keeps the entitlement | Partial | A burn is one transaction (tree append and token burn); `a_burn_destroys_supply_and_records_a_hidden_recipient` (executor). A release spends its nullifier only on success: local, carol's release is refused six times, then applied ([`evidence/local/`](../evidence/local/)) | Release on local anvil only. A burn of an amount the LEZ policy permits but the vault does not cannot be released (denominations not cross-checked, F.10). Outbound on Sepolia (M2) |
| R.3 | No deposit claimed twice, no burn released twice, under adversarial retry | Met | `a_deposit_is_minted_once_to_its_committed_destination` (executor, under the same and an older root); `test_replay_is_refused` in each verifier's suite (Foundry); public: replay refused `E7402` [`b0c840a2…`](https://explorer.testnet.lez.logos.co/transaction/b0c840a246afbc3b58a5bf093ac499bc4f43910950a09b88b8e4deee37370579); local: release replays refused `AlreadyReleased` | |
| R.4 | A valid claim stays valid indefinitely | Partial | `knownRoot` is never cleared; `a_claim_against_an_old_root_still_lands` (executor); `Mint` does not re-check denominations, so a policy change does not strand a deposit | Inbound only. A release needs a header attested after the burn, so it stays valid only while the committee is live (F.7, M2). A `mint_cap` lowered below outstanding supply refuses valid claims until supply drops; the recommended handling goes in the deployer docs (M4) |
| R.5 | Recovery from credentials alone rediscovers every position | Partial | `positions_are_recovered_from_the_seed_alone` (executor) | As F.9: outbound (M2), private destinations and the client (M3) |
| R.6 | Off-chain components survive RPC failures and resume without duplicating work | Planned in M3 | | No daemon yet. `scripts/testnet-inbound.sh` resumes by stage and a deployment by `RESUME_<program>` ([TESTNET.md](TESTNET.md)), which is a script, not a component |
| R.7 | An interrupted user operation does not consume, corrupt or expose the entitlement | Partial | Every secret is derived from the seed and a counter (`wrap_core::derive_secret`); a private claim refused or abandoned before submission publishes nothing (`a_refused_private_claim_fails_before_it_reaches_the_chain`) | The release proof reads the client's burn record (`run/burns.json`); rebuilding it from the seed and the `Burned` events, and an interruption test (M2, M3) |
| R.8 | Mature, audited proof systems, no home-made ZK primitives | Met | RISC Zero r0vm 3.0.5 and its Groth16 wrap; `RiscZeroGroth16Verifier` from risc0-ethereum v3.0.1, vendored unchanged (README, "Layout"); `ed25519-dalek` (strict) for Bedrock signatures | |
| R.9 | Effect of an outage or malfunction of any third-party off-chain component documented and mitigated | Partial | The committee's role and what it can forge: README "What is still trusted", [OUTBOUND.md](OUTBOUND.md) "The trust assumption, stated plainly" | Outage effects of the committee, provers, relayers and the Sextant update submitter, and their mitigations (M2, M4) |
| R.10 | End-to-end round trip against a standalone LEZ sequencer and an Ethereum test chain, in CI | Partial | [`scripts/e2e-local.sh`](../scripts/e2e-local.sh), `e2e-chain.sh`, `e2e-bedrock.sh`: deposit, claims, burns, releases on a standalone v0.3.1 sequencer and anvil, PASS, verifier 81 checks, its mutation test 32 of 32 ([`evidence/local/`](../evidence/local/)) | Run locally, not in CI: [CI](../.github/workflows/ci.yml) runs Foundry, the crates, the executor tests (the private claim in dev mode), the reproducible rebuilds and the program's mutation test (M4) |
| R.11 | Every hard requirement has a test | Partial | This table; 105 Foundry tests, 15 executor tests plus 3 private, `wrap-core` 9, `lez-chain` 10, `lez-bedrock` 14; mutation tests: program 21 of 21, vault 66 of 66, `lez-bedrock` 30 of 30 ([`docs/raw/`](raw/)) | No test yet for U.1 to U.10, P.1 to P.7 as requirements, R.6, Sec.9, PP.3 over a full round trip (M4) |
| R.12 | Core-module documentation: developer journey for both flows and recovery | Planned in M4 | | The module is M3 |
| R.13 | CLI and operator documentation | Planned in M4 | | [LOCAL-E2E.md](LOCAL-E2E.md) and [TESTNET.md](TESTNET.md) "Reproduce" document the run harness only |
| R.14 | Deployer journey: tokens, fees, admin and freeze authorities, finality depth | Planned in M4 | | [TESTNET.md](TESTNET.md) records one deployment; no deployer guide |

## Performance

| ID | requirement | status | evidence | remaining |
|---|---|---|---|---|
| P.1 | Inbound claim and mint in one LEZ public transaction, compute cost broken down by component | Partial | Public: one transaction, 451,107 gas (4.5% of the 10M cap), Sepolia proof of 9 account and 4 storage nodes ([TESTNET.md](TESTNET.md)). Local: 297,457 gas, of which Sextant's `Attest` 111,122 (README, "Gas"); reproduced in CI by `executor-tests` `measure` | Full breakdown by component, in the RFP-020 methodology, at the budget in force on LEZ 0.4 (M4) |
| P.2 | Cost of additional LEZ-side verification, amortised per deposit | Partial | A Sextant instance `Init` 1,834,893 gas; an update is three transactions, about 13M gas ([TESTNET.md](TESTNET.md)) | Recommended update cadence and the amortised cost per deposit (M4) |
| P.3 | Ethereum gas of a deposit, a release and any extra verification | Partial | Deposit: median 257,854, max 336,694 (Foundry, [`docs/raw/vault-gas-2026-10-17.txt`](raw/vault-gas-2026-10-17.txt)); public: 847,409 and 531,046 on Sepolia ([TESTNET.md](TESTNET.md)). Release, anvil only: 143,096 to 349,997 by verifier, of which Groth16 `verify` 253,318 (README) | Release on Sepolia, with the bonded attestation, caps and fees (M2) |
| P.4 | User-side proving practical on Basecamp hardware: a mid-range laptop and the declared minimum | Partial | Apple M4 Pro, 24 GiB only: private claim 247.6 to 285.0 s, 6.17 to 7.85 GB (local), 264.6 s, 8.72 GB (public); private burn 201.8 to 202.9 s, 7.22 to 7.82 GB; release 52 s STARK + 62 s Groth16 emulated ([LOCAL-E2E.md](LOCAL-E2E.md), [PRIVATE-CLAIM-MEASURED.md](PRIVATE-CLAIM-MEASURED.md), [BEDROCK-FINALITY.md](BEDROCK-FINALITY.md) §8) | A mid-range laptop and a declared minimum, Groth16 native on x86_64, on LEZ 0.4 (M4) |
| P.5 | End-to-end latency per direction, by finality wait, proving, privacy delay, verification | Planned in M4 | | Components are measured (proofs above; Bedrock k = 30, about 600 s, [BEDROCK-FINALITY.md](BEDROCK-FINALITY.md) §2); no breakdown per direction |
| P.6 | CPU, RAM, time of any off-chain component | Partial | Chain proof: 87 s and 141 s for blocks 28-120 and 121-317, 7.5 to 8.7 GB; Bedrock anchor 5 deep: 111 s, 5.9 GB; 30 deep: 1.80M cycles ([BEDROCK-FINALITY.md](BEDROCK-FINALITY.md) §7, §8) | Per daemon role (prover, relayer, committee member), on one declared machine (M3, M4) |
| P.7 | Growth and storage cost of accumulated state at 1M and 10M operations | Planned in M4 | | The state is known (one nullifier account per claim on LEZ; one `knownRoot` slot per deposit and one nullifier slot per release on Ethereum), not projected |

## Supportability

| ID | requirement | status | evidence | remaining |
|---|---|---|---|---|
| S.1 | Vault on a public Ethereum testnet, program on LEZ testnet 0.4 | Partial | Public, inbound only: `WrapVault` on Sepolia [`0x86521e22…`](https://sepolia.etherscan.io/tx/0x86521e22b33f49aa3a533d7e6a01251cd4c6d8265c77fe0b02b37ad42f84d288), `wrap_bridge` on the public LEZ testnet v0.3.1 [`59ef746a…`](https://explorer.testnet.lez.logos.co/transaction/59ef746a13bcb30d913ea3639b706cf366a20fb161b7b32c2b1d1c535d48bf62); `scripts/verify-testnet.sh`: 29 LEZ and 14 Sepolia checks, 0 fail | LEZ testnet 0.4 (M1); the Sepolia vault's `CommitteeBurnVerifier` was never exercised: outbound on public networks (M2) |
| S.2 | Modules work on Linux and macOS | Planned in M3 | | |
| S.3 | Modules built and released with `logos-modules-release-action` | Planned in M3 | | |
| S.4 | Design decision log, current at each milestone | Planned in M0 | | Options and rankings exist for the release path only ([BEDROCK-FINALITY.md](BEDROCK-FINALITY.md) §5) |

## Security

| ID | requirement | status | evidence | remaining |
|---|---|---|---|---|
| Sec.1 | Independent audit of both contracts, report published | Planned in M5 | | Audit on the code frozen at M4; report and remediation in M5 |
| Sec.2 | Both halves reject invalid proofs: wrong public inputs, wrong chain state, tampered headers, replays; independently verifiable | Partial | Inbound: `a_submitter_cannot_redirect_inflate_or_forge`, `a_root_the_vault_never_had_is_refused_by_attest` (executor); public: `E7208` [`974f5e7d…`](https://explorer.testnet.lez.logos.co/transaction/974f5e7de5c1914bb28098e949fbff5655d7037f185930a7d7f1898e39a901b3), `E7402`. Outbound: tampered journal, seal, tip, Bedrock header, anchor, channel, key, depth refused on real seals (Foundry); local: 14 refusals ([`evidence/local-bedrock/`](../evidence/local-bedrock/)). Re-checkable: `scripts/verify-testnet.sh`, `scripts/verify-onchain.sh` | Inbound header tampering is tested in Sextant's repository, not here. A real proof over a made-up Bedrock block signed by the real sequencer key is refused only by the committee (F.7). Outbound refusals on Sepolia (M2) |
| Sec.3 | A submitter cannot redirect, inflate a fee or replay to another destination | Met | Destination inside the deposit commitment: `a_submitter_cannot_redirect_inflate_or_forge` (executor), local `claim-bob-redirected-to-alice` refused `E7403`. Recipient, relayer, fee inside the burn commitment and release digest: `test_a_higher_fee_is_refused_and_a_front_runner_earns_only_the_committed_fee`, `test_another_recipient_amount_fee_relayer_root_or_nullifier_is_refused` (Foundry); local: redirect and front-run refused `NotThisRelease` | |
| Sec.4 | Caps bound value at risk per window; recommended defaults documented | Partial | Caps as F.11 | Rolling windows (M1); recommended defaults and their reasoning (M4) |
| Sec.5 | Freeze exercisable on each half without the other | Met | Each half's freeze touches only its own state: `test_freeze_per_token_and_global_each_direction_independently` (Foundry), `admin_freeze_and_cap` (executor) | |
| Sec.6 | Wrapped supply never exceeds vault holdings; mint without deposit, double mint, release without burn all fail | Partial | Inbound: `a_deposit_is_minted_once_to_its_committed_destination`, `a_root_the_vault_never_had_is_refused_by_attest` (executor), public `E7402`, `E7208`. Local: supply checked after each claim and burn, vault payout at the end ([LOCAL-E2E.md](LOCAL-E2E.md), "Results") | A release without a burn fails only while the committee threshold is honest (F.7, M2). A supply-versus-escrow invariant test (M4) |
| Sec.7 | User documentation states the trustless model and the liveness-only role of off-chain participants | Partial | README "What is still trusted"; [OUTBOUND.md](OUTBOUND.md) "The trust assumption, stated plainly" | The release committee is a safety role, not liveness-only (F.7, M2). User-facing documentation (M4) |
| Sec.8 | Verification logic cannot be swapped; migration documented | Partial | `WrapVault` is not upgradeable, `VERIFIER` is immutable, `test_constructor_refuses_a_missing_verifier` (Foundry); the verifiers pin guests and committees as immutables; `wrap_bridge` loaded with an immutable header and its configuration fixed by its address (public, [TESTNET.md](TESTNET.md)) | Migration mechanism and how in-flight deposits and burns are honoured across it (M4) |
| Sec.9 | Failsafe for a verification bug without a backdoor, or why none | Planned in M4 | | Position in the decision log (M0), document (M4) |

## Privacy Preservation

| ID | requirement | status | evidence | remaining |
|---|---|---|---|---|
| PP.1 | Inbound unlinkability beyond amount, token, timing | Partial | Private claim: destination, secret, commitment, leaf, index and path absent from the transaction; `a_private_claim_mints_to_a_private_account_and_publishes_no_opening` (executor); local and public byte checks by `scripts/verify-onchain.sh`; anonymity set stated ([PRIVATE-CLAIM-MEASURED.md](PRIVATE-CLAIM-MEASURED.md)) | A public claim carries the deposit's opening and links to it. Anonymity-set test and timing defaults (M3, M4) |
| PP.2 | Outbound unlinkability beyond amount, token, timing | Partial | Burn leaf hides the recipient; the zero-knowledge release carries no leaf, index, path, secret or commitment: `test_the_release_calldata_omits_the_opening` (Foundry); local: none of the releases sent them ([LOCAL-E2E.md](LOCAL-E2E.md)) | Local only; the free-form relayer fee (F.8); outbound on Sepolia (M2) |
| PP.3 | No argument, event, log or state change reveals either destination, over full diffs of a round trip | Partial | Byte checks on each private claim, private burn and release (local); private account id absent from every chain copy (`verify-onchain.sh`, public and local) | Full event and state diffs over a complete round trip (M4) |
| PP.4 | Linking information never leaves the user; every component handling user data documented | Partial | The private claim needs no signer and no fee payer; a relayer submits a finished release proof ([PRIVATE-CLAIM-MEASURED.md](PRIVATE-CLAIM-MEASURED.md), [OUTBOUND.md](OUTBOUND.md) §3) | Inventory of every component handling user data (M4) |
| PP.5 | Failures indistinguishable as to which deposit or burn | Partial | A refused private claim fails on the claimant's machine and publishes nothing: `a_refused_private_claim_fails_before_it_reaches_the_chain` (executor) | Public refusals are included and charged; a failed public claim publishes its opening. Error-path tests (M4) |
| PP.6 | What a third-party submitter learns; switchable per operation | Partial | The relayer is bound per release in the journal (`--relayer`), and learns only the journal ([OUTBOUND.md](OUTBOUND.md) §3); a private claim can be submitted by anyone without a signature | No relayer software; per-operation choice in the client (M2, M3); network-level leakage stated (M4) |
| PP.7 | Private by default; weakening overrides need explicit confirmation | Planned in M3 | | The run harness defaults to the public claim (U.6) |

## Design rationale and platform dependencies

Not numbered in the RFP; counted separately.

| item | status | evidence | remaining |
|---|---|---|---|
| Trust model: no trusted signer, federation, relayer or prover | Partial | Inbound: a light client's record of a finalized Sepolia header and a storage proof, nothing signed (public, [TESTNET.md](TESTNET.md)) | Outbound rests on the header committee (F.7, M2) |
| Target tokens: USDC, USDT, DAI, WETH | Partial | 6- and 18-decimal tokens and a USDT-shaped token in Foundry and the local run | Registered on Sepolia (M1) |
| Admin authority (RFP-001) | Partial | Single-account stand-ins on both halves | The RFP-001 account model, with transfer and renounce (M1) |
| Freeze authority (RFP-002) | Partial | Single-account stand-ins on both halves (F.14) | The RFP-002 account model (M1) |
| Token program and ATA compatibility (LP-0013) | Partial | Mint authority by PDA seed, no stored authority; mints to the owner's ATA; public: LEZ v0.3.0's token and ATA programs loaded on the testnet, which has no builtin token program | The canonical token and ATA programs of LEZ 0.4 (M1) |
| Logos Ethereum core module | Planned in M3 | | |
| Wallet SDK | Planned in M3 | | |
| RISC Zero zkVM | Met | Every program and guest built in RISC Zero's pinned builder, rebuilt and `cmp`'d in CI (README, "Reproducible binaries") | |
| Events (LP-0012) | Met | `Claimed { nullifier }` and `Burned { leaf }` on the public path ([`lib.rs`](../programs/wrap_bridge/src/lib.rs)); the private path emits none, recovery reads accounts ([PRIVATE-CLAIM-MEASURED.md](PRIVATE-CLAIM-MEASURED.md)) | |
| MIT and Apache-2.0 dual licence | Met | [LICENSE-MIT](../LICENSE-MIT), [LICENSE-APACHE](../LICENSE-APACHE) | |

## Soft requirements

| # | requirement | status | evidence | remaining |
|---|---|---|---|---|
| 1 | Committed amounts | Planned in M4 | | Migration path documented: a new leaf version in the same tree |
| 2 | Voluntary viewing keys | Planned in M3 | | |
| 3 | Per-token release delay | Planned in M2 | | |
| 4 | Pluggable proof system | Met | `IBurnVerifier`: four verifiers behind one vault interface ([OUTBOUND.md](OUTBOUND.md)); the Sextant program is a configuration field | |
| 5 | Common Ethereum wallet compatibility | Planned in M4 | | Security trade-offs documented only; no implementation |

## Count

Hard requirements (68):

| status | count | IDs |
|---|---:|---|
| Met | 12 | F.1, F.2, F.4, F.5, F.14, F.15, F.17, R.1, R.3, R.8, Sec.3, Sec.5 |
| Partial | 35 | F.3, F.6 to F.12, F.16, U.7, U.8, R.2, R.4, R.5, R.7, R.9 to R.11, P.1 to P.4, P.6, S.1, Sec.2, Sec.4, Sec.6 to Sec.8, PP.1 to PP.6 |
| Planned in M0 | 1 | S.4 |
| Planned in M1 | 1 | F.13 |
| Planned in M3 | 11 | U.1 to U.3, U.5, U.6, U.9, U.10, R.6, S.2, S.3, PP.7 |
| Planned in M4 | 7 | U.4, R.12 to R.14, P.5, P.7, Sec.9 |
| Planned in M5 | 1 | Sec.1 |

Design rationale and platform dependencies (10): 3 Met, 5 Partial, 2
Planned in M3. Soft requirements (5): 1 Met, 4 Planned.

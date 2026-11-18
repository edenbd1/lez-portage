# Failsafe for a flaw in the verification logic (Security #6, #9)

RFP-022 Security #9 asks what happens once a vulnerability in the verification
logic is found. The freeze stops new activity, but funds already at risk and
deposits or burns still in flight have to be dealt with too, and that has to
happen without a backdoor. Recovery must still be claimed by whoever proves
their own entitlement. Nothing may mint, redirect or release funds to a
destination the proof does not name, or act beyond what the flaw put at risk.

This document says what the vault does about such a flaw, what it deliberately
does not do, and why. The mechanism is in `vault/src/WrapVault.sol`. The tests
are in `vault/test/WrapVaultFailsafe.t.sol`. `scripts/mutation-test-vault.sh`
shows that every guard fires.

## 1. What a flaw can take is bounded, even with a verifier that accepts everything

- **Escrow per token (Security #6, on the Ethereum side).**
  - `escrowed[token]` counts what deposits of `token` brought in, minus what
    releases took out.
  - A release larger than its token's escrow is refused (`Insolvent`).
  - So a forged release can never take another token's escrow, or funds that
    reached the vault outside `deposit` (a direct transfer, a forced
    `selfdestruct`).
  - It can also never take more, in total, than was ever deposited.
  - The invariant test drives random deposits, releases, bookings, payouts,
    stray transfers and time through a verifier that accepts every proof
    (`FlawedVerifier`). Over 256 runs × 500 calls, nothing beyond the deposits
    is ever released, and the vault always holds the escrow plus every booked
    payout.
- **Rolling caps (Security #4, as before).**
  - Over a detection time T, at most `cap × (1 + T / window)` leaves the vault
    per token.
  - At most the global cap's equivalent leaves across tokens.

## 2. The release delay: the window in which a pause stops a payout

- **How it works.** With `releaseDelay` > 0, `release` still checks
  everything: the proof, the nullifier, the caps and the escrow. But it only
  *books* the payout, with the recipient, relayer, amount and fee that the
  proof fixed.
- **Payout.** `finalize(nullifier)` pays exactly that, once the delay has
  passed. Anyone can call it, so a relayer, the user or a keeper can.
- **The guardian's role.** The freezer pauses releases at once, per token or
  globally, independently of the admin and of the LEZ half (Security #5). A
  pause blocks `finalize`. So a flaw noticed within the delay stops forged
  releases **before they pay**.
- **No redirect.** A booked payout cannot be redirected, reassigned or
  cancelled by anyone:
  - the freezer can only pause;
  - the admin cannot touch `pending`;
  - the nullifier is spent at booking, so the same burn cannot be booked
    twice.
- **Changing the delay.** Lengthening it is immediate. Shortening it is a
  loosening, so it runs only as a timelocked operation (2 days by default).
  The delay is bounded by `MAX_RELEASE_DELAY`, 7 days. A booked release keeps
  the time it was booked with.
- **Default.** The delay is 0 by default: a release then pays at once, as
  before. The deployed testnet vaults use 0.
- **Recommendation for a deployment that holds real value.** Set the delay to a
  few hours, longer than the time its monitors need to notice an anomalous
  release, for example 6 hours. The cost is latency on the way out only.
  Deposits and claims are unaffected.

## 3. What is deliberately not implemented, and why

**No authority sorts booked releases into honest and forged.**
- Once releases are paused, an honest burner's booked payout waits along with
  any forged one.
- Picking which to pay would be an admin "identifying who owns what", which
  Security #9 forbids.
- The only way out of a pause is the admin's `unfreeze`, which is:
  - **public** and **timelocked**;
  - unable to lift a pause newer than the one it was scheduled against;
  - **all-or-nothing** for a token, so it pays every booked release of that
    token. It should run only once the flaw is known not to affect them. A
    compromised committee whose attestations anyone can re-check against the
    canonical chain is an example of such a case.

**No migration of escrow to a successor vault.**
- Security #8 makes the verification logic immutable: a fix is a new
  deployment.
- Moving the old vault's escrow into it would need someone to designate the
  successor, and that is a redirect of every depositor's funds by an
  authority.
- So burners re-prove their burns under the new version. If the old vault's
  pause cannot safely be lifted, its escrow stays where it is: no one can take
  it, the attacker included.
- What is lost to a flaw is bounded by §1: the caps over the detection time,
  and never more than the token's escrow.

**No refund path for deposits.**
- A deposit can never be both refunded and minted, because there is no
  refund.
- A deposit's root is a vault root forever (`knownRoot` is never cleared), so
  the deposit stays claimable on LEZ forever by whoever holds the seed that
  produced its commitment. That claim is its exit.
- A refund would need the vault to verify that a deposit was *not* claimed on
  LEZ. That is a proof about LEZ state in the other direction, made by
  verification logic of the same kind the failsafe is meant to survive.

## 4. A flaw on the LEZ side

- **Example.** A flaw in the claim path, Sextant's attestation or the bridge
  program lets wrapped tokens be minted without a deposit.
- **The LEZ half pauses on its own:** the program's freeze and its mint cap
  (`admin_freeze_and_cap`).
- **What reaches the vault.** Over-minted tokens that are burned reach the
  vault as burns with valid proofs, so the vault cannot tell them apart from
  honest ones. They are bounded, as above:
  - by the release caps;
  - by the token's escrow;
  - and, with a delay, by the window in which the vault's own freezer can
    pause before they pay.

## 5. Gas

Measured with the slots warm, in one test transaction, against `origin/main`:
`deposit` +370 gas, `release` (no delay) +2,761 gas. In a real transaction the
first touch of a slot is cold:
- **`deposit`:** about 5k more (the `escrowed` slot), and about 22k more on a
  token's very first deposit.
- **`release` with no delay:** about 7k more (the escrow slot and the delay
  slot).
- **`release` with a delay:** books the payout in place of paying it. Writing
  about four storage slots adds about 90k; the payout itself moves to
  `finalize` (37k to 45k in the Foundry gas report).

## 6. Tested

- `vault/test/WrapVaultFailsafe.t.sol`:
  - escrow follows deposits and releases;
  - a flawed verifier cannot release more than the token's deposits, even with
    stray funds in the vault;
  - one token's escrow never pays another token;
  - with a delay, a release is booked, cannot be paid early, is paid after the
    delay to the proof's recipient whoever calls `finalize`, and only once;
  - a per-token or global pause inside the delay stops the payout;
  - a booked release cannot be replayed;
  - shortening the delay is timelocked and bounded, and the freezer cannot set
    it;
  - a booked release keeps its time when the delay changes;
  - two invariants under a verifier that accepts everything.
- `scripts/mutation-test-vault.sh`: 11 new mutations, each caught:
  - the escrow check removed;
  - deposits not counted;
  - a release not charged;
  - escrow pooled across tokens;
  - the delay ignored;
  - a payout before the delay;
  - a pause not stopping a payout;
  - only a global pause stopping it;
  - a payout made twice;
  - a shorter delay not timelocked;
  - the delay unbounded.

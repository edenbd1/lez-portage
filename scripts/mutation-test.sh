#!/usr/bin/env bash
# A check nobody has seen fail is not a check. Remove each of wrap_bridge's
# guards in turn, rebuild the guest, and require the executor tests to fail.
# The source and the committed binary are restored at the end.
#   scripts/mutation-test.sh        (each mutation is one local guest build, ~20 s)
set -uo pipefail
cd "$(dirname "$0")/.."
SRC=programs/wrap_bridge/src/lib.rs
BIN=artifacts/programs/v0.3/wrap_bridge.bin
cp "$SRC" /tmp/wrap_bridge.lib.rs.orig
cp "$BIN" /tmp/wrap_bridge.bin.orig
restore() { cp /tmp/wrap_bridge.lib.rs.orig "$SRC"; cp /tmp/wrap_bridge.bin.orig "$BIN"; }
trap restore EXIT

# name @@ literal text @@ replacement   (\n is a newline)
MUTATIONS=(
  "merkle path not checked@@wrap_core::root_from_path(&leaf, index, &path) == Some(root)@@true"
  "vault address not checked@@proof.address == config.vault@@true"
  "knownRoot slot not checked@@proof.slot == wrap_core::known_root_slot(&root)@@true"
  "nullifier not spent once@@refuse_unless(pre.is_empty(), E_CLAIMED@@refuse_unless(true, E_CLAIMED"
  "no chained Attest@@plan.call(ChainedCall::new(\n                config.sextant,@@let _ = (ChainedCall::new(\n                config.sextant,"
  "no PDA seed on the mint@@.with_pda_seeds(vec![dseed])@@.with_pda_seeds(vec![])"
  "global mint freeze ignored@@refuse_unless(!(if mint { l.frozen_mint } else { l.frozen_burn })@@refuse_unless(true"
  "token mint freeze ignored@@refuse_unless(!e.frozen_mint@@refuse_unless(true"
  "token burn freeze ignored@@refuse_unless(!e.frozen_burn@@refuse_unless(true"
  "supply cap ignored@@is_some_and(|o| o <= e.policy.mint_cap)@@is_some_and(|_o| true)"
  "admin not checked on Register@@admin.account_id == config.admin && admin.is_authorized, E_NOT_ADMIN, \"only the admin authority registers@@true, E_NOT_ADMIN, \"only the admin authority registers"
  "admin not checked on SetPolicy@@admin.account_id == config.admin && admin.is_authorized, E_NOT_ADMIN, \"only the admin authority sets@@true, E_NOT_ADMIN, \"only the admin authority sets"
  "freezer not checked@@freezer.account_id == config.freezer && freezer.is_authorized@@true"
  "burn amount not a denomination@@refuse_unless(e.policy.denominations.contains(&amount)@@refuse_unless(true"
  "burn ATA not the signer's@@expect_row(&ata, token::ata_account(&config.ata_program, &owner.account_id, &did, &config.token_program), config.token_program@@expect_row(&ata, ata.account_id, config.token_program"
  "mint row may be any account@@|| dest.account_id == destination)@@|| true)"
  "destination row not checked@@(dest.account_id == ata || dest.account_id == destination) && dest.program_account_id@@dest.program_account_id"
  "BurnFrom holding not authorized@@refuse_unless(holding.is_authorized, E_NOT_HOLDER@@refuse_unless(true, E_NOT_HOLDER"
  "BurnFrom holding shard not checked@@refuse_unless(holding.program_account_id == config.token_program, E_ACCOUNTS@@refuse_unless(true, E_ACCOUNTS"
  "BurnFrom burn tree row not checked@@expect_row(&burns, burns_account(&me, &ca), me, \"BurnFrom: the third@@expect_row(&burns, burns.account_id, me, \"BurnFrom: the third"
  "BurnFrom config row not checked@@expect_row(&cfg, ca, me, \"BurnFrom: the first@@expect_row(&cfg, cfg.account_id, me, \"BurnFrom: the first"
)

# The unmutated source, built the same way, must pass: otherwise every
# mutation below would count as caught.
restore
scripts/build-guest.sh >/dev/null 2>&1 || { echo "BASELINE BUILD FAIL"; exit 1; }
cargo test -q --release --manifest-path executor-tests/Cargo.toml --test bridge >/dev/null 2>&1 \
  || { echo "BASELINE FAIL: the unmutated build does not pass the tests"; exit 1; }
echo "baseline    the unmutated build passes"

caught=0; missed=0
for m in "${MUTATIONS[@]}"; do
  name=${m%%@@*}; rest=${m#*@@}; from=${rest%%@@*}; to=${rest#*@@}
  restore
  python3 - "$SRC" "$from" "$to" <<'EOF' || { echo "SETUP FAIL  $name (text not found)"; missed=$((missed+1)); continue; }
import sys
p, a, b = sys.argv[1], sys.argv[2].encode().decode('unicode_escape'), sys.argv[3].encode().decode('unicode_escape')
s = open(p).read()
if a not in s:
    sys.exit(1)
open(p, 'w').write(s.replace(a, b, 1))
EOF
  if ! scripts/build-guest.sh >/dev/null 2>&1; then echo "BUILD FAIL  $name"; missed=$((missed+1)); continue; fi
  if cargo test -q --release --manifest-path executor-tests/Cargo.toml --test bridge >/dev/null 2>&1; then
    echo "MISSED      $name"; missed=$((missed+1))
  else
    echo "caught      $name"; caught=$((caught+1))
  fi
done
restore
echo "$caught caught, $missed missed"
[ "$missed" = 0 ]

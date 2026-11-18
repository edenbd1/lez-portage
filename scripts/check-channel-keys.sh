#!/usr/bin/env bash
# The public LEZ testnet's Bedrock channel is posted to by several keys in turn,
# and ZkBedrockBurnVerifier pins the channel's key set, not one key. This
# re-checks it on the Bedrock blocks the public outbound run fetched read-only
# (evidence/testnet-outbound/bedrock/, from a node following the public
# testnet): every inscription on channel 0101…01 is signed by one of the four
# pinned keys; lez_bedrock::anchor accepts each with the key that signed it and
# refuses it with any other key of the set named (a journal names the key that
# signed, and the seal binds it); an anchor record is built for one LEZ block
# per signer with --keys; and a one-key set refuses the other signers'
# inscriptions. Needs bedrock-tools (cargo build --release --manifest-path
# bedrock/tools/Cargo.toml; it builds against the Bedrock node's own types).
#   scripts/check-channel-keys.sh
set -uo pipefail
cd "$(dirname "$0")/.."
BT="${BEDROCK_TOOLS:-bedrock/tools/target/release/bedrock-tools}"
CH=0101010101010101010101010101010101010101010101010101010101010101
KEYS=06abcb81d02a679c9756236e1c18d908c5d2e8df318ae71c3c56ad038c938e39,0d27b7090beb0713bffbfa6961dcc17e071a410942e9d600a96c52e74aa592a8,30cfd05a0fd5a056166f8d337b5e4018e40a6cf517140a8056db21f128959522,a12da31c0df1e8cbd2416822b80864411d102355e049edc56609b076312259aa
HASH=ee0c1980ecb6e98f1a2f9f69093d8633250a42e37b94140c18379d945f22a982   # = ZkBedrockBurnVerifier.t.sol's PUBLIC_KEYS_HASH
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
fail=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fail=1; }
for D in evidence/testnet-outbound/bedrock/*/; do
  echo "== $D"
  if ! R="$("$BT" channel-keys "$D" --channel $CH --keys $KEYS)"; then bad "an inscription outside the set, or one anchor refused"; continue; fi
  python3 - "$R" "$HASH" <<'PY' || fail=1
import json, sys
r = json.loads(sys.argv[1])
print("  ok   %d inscriptions in %d Bedrock blocks, signed by %d keys: %s" % (r["inscriptions"], r["blocks"], len(r["inscriptions_by_signer"]),
      ", ".join("%s… %d (LEZ %d-%d)" % (k[:8], n, *r["lez_block_ids_by_signer"][k]) for k, n in sorted(r["inscriptions_by_signer"].items()))))
print("  ok   each accepted with its own signer (%d), refused with another key of the set named (%d)" % (r["accepted_with_their_signer"], r["refused_with_another_key_named"]))
assert r["channel_keys_hash"] == sys.argv[2], "CHANNEL_KEYS_HASH differs from the Solidity vector"
print("  ok   CHANNEL_KEYS_HASH %s… = the verifier's" % r["channel_keys_hash"][:16])
PY
  echo "$R" > "$T/r.json"
  for k in $(python3 -c 'import json,sys;print(" ".join(json.load(open(sys.argv[1]))["a_lez_hash_by_signer"]))' "$T/r.json"); do
    h="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["a_lez_hash_by_signer"][sys.argv[2]])' "$T/r.json" "$k")"
    if A="$("$BT" anchor "$D" --channel $CH --keys $KEYS --lez-hash "$h" --depth 3 --out "$T/a.json")" \
       && [ "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["signer"])' "$T/a.json")" = "$k" ]; then
      ok "anchor --keys: LEZ block $(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["lez_block_id"])' "$T/a.json"), signed by ${k:0:8}…, 3 deep"
    else bad "anchor --keys for the inscription by ${k:0:8}…"; fi
    other=0d27b7090beb0713bffbfa6961dcc17e071a410942e9d600a96c52e74aa592a8
    if [ "$k" != "$other" ]; then
      if "$BT" anchor "$D" --channel $CH --signer $other --lez-hash "$h" --depth 3 --out "$T/b.json" >/dev/null 2>&1; then
        bad "a one-key pin (${other:0:8}…) accepted the inscription by ${k:0:8}…"
      else ok "a one-key pin (${other:0:8}…) refuses the inscription by ${k:0:8}…"; fi
    fi
  done
done
echo "== $( [ $fail = 0 ] && echo PASS || echo FAIL )"
exit $fail

# The privacy check, and the controls that make it able to fail

A privacy-preserving claim or burn must not carry what it hides. On the public
runs that means: the user accounts (alice, bob, carol), the two deposit leaves
on Sepolia, and the two Ethereum recipients. `scripts/privacy-check.py` reads
each private transaction exactly as the LEZ testnet served it (from
`evidence/archive/`, offline, each one checked against its own hash; `--rpc URL`
reads a live node instead) and searches its bytes for every one of those values.

A search that has never found anything proves nothing. So the same detector is
run, in the same command, on transactions that do carry those values, and must
find them:

- **The public path of the same program.** alice's public claim carries her
  account and the leaf of the deposit it claims; her public burn carries her
  account. The detector must find exactly those.
- **A leaky build.** Each private transaction's real bytes with one hidden value
  written into them, at the start, the middle and the end: every one of the
  planted leaks must be found.

`scripts/privacy-check.py --build leaky` takes the leaky build as the build
under test, and must fail. `scripts/mutation-test-privacy.sh` weakens the
detector in five ways a real bug could (finds nothing; searches hex text
instead of bytes; reads only the first 64 KiB; checks one value; skips the
transaction's own account), and the controls must catch every one. CI runs all
three (job `archive`).

What the check does not show: that a transaction is unlinkable by timing or by
elimination. With fixed deposit amounts, a private claim reveals neither its
deposit nor its holder, but in a run with two deposits, one of them claimed in
public, the other is linked by elimination: the anonymity set is the set of
unclaimed deposits, and it grows with use, not with cryptography.

## Real build

```
$ scripts/privacy-check.py
privacy check, build under test: real, transactions from evidence/archive
  ok   burn-carol-private: privacy-preserving, 248,358 bytes; none of the 7 hidden values occurs in it (its own: carol's account F8WThjdH…)
  ok   claim-carol-private: privacy-preserving, 257,739 bytes; none of the 7 hidden values occurs in it (its own: carol's account F8WThjdH…)
controls: the same detector on transactions that do carry these values
  ok   claim-alice (public, same program): the detector finds alice's account J2rKAcRT…, deposit 1 leaf
  ok   burn-alice (public, same program): the detector finds alice's account J2rKAcRT…
  ok   burn-carol-private, leaky build: 21 of 21 planted leaks found (7 values x start, middle, end)
  ok   claim-carol-private, leaky build: 21 of 21 planted leaks found (7 values x start, middle, end)
6 checks pass, 0 fail
== PASS
```

## The leaky build, as the build under test

```
$ scripts/privacy-check.py --build leaky
privacy check, build under test: leaky, transactions from evidence/archive
  FAIL burn-carol-private: carries alice's Ethereum recipient, alice's account J2rKAcRT…, bob's account 6GvE3DdU…, carol's Ethereum recipient, carol's account F8WThjdH…, deposit 0 leaf, deposit 1 leaf
  FAIL claim-carol-private: carries alice's Ethereum recipient, alice's account J2rKAcRT…, bob's account 6GvE3DdU…, carol's Ethereum recipient, carol's account F8WThjdH…, deposit 0 leaf, deposit 1 leaf
controls: the same detector on transactions that do carry these values
  ok   claim-alice (public, same program): the detector finds alice's account J2rKAcRT…, deposit 1 leaf
  ok   burn-alice (public, same program): the detector finds alice's account J2rKAcRT…
  ok   burn-carol-private, leaky build: 21 of 21 planted leaks found (7 values x start, middle, end)
  ok   claim-carol-private, leaky build: 21 of 21 planted leaks found (7 values x start, middle, end)
4 checks pass, 2 fail
== FAIL
```

## The detector, mutated

```
$ scripts/mutation-test-privacy.sh
  caught  blind: 5 failing line(s)
  caught  hex: 5 failing line(s)
  caught  prefix64k: 3 failing line(s)
  caught  first-only: 4 failing line(s)
  caught  skip-own: 5 failing line(s)
5 of 5 mutants caught
```

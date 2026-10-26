# witness-evm

A [witness](https://github.com/ascrivener/witness) check for **settled bug bounties**: a protocol
escrows a reward against a precise condition, "a transaction sequence that breaks invariant I of
our contracts, on block N's state", and a hunter's report is paid automatically if it does. No
human judge: the check replays the report's transactions in [revm](https://github.com/bluealloy/revm)
against Merkle-proven Ethereum state and runs the invariant before and after.

It runs a bounty end to end, in witness-sim or on a local PolkaJam testnet: posted with a
disclosure window, the exploit disclosed privately and verified by the protocol off chain, its
early reveal refused, then a fair swap and a forged report refused and the real exploit (a 29 KB
report, 9.4×10⁶ gas to check) accepted and paid. See
[docs/design.md](docs/design.md) for the measurements, the go/no-go, and what's next.

## How it works

- **Params** (the bounty's terms): block N's state root, the hard fork and block environment the
  transactions run in, the hunters' starting ETH, and the invariant: EVM runtime code returning
  nonzero while it holds.
- **Witness** (the report): every account and storage slot the replay reads, each with its
  Merkle-Patricia proof against the state root, the accounts' code, and the transactions.
- **The check** verifies every proof, runs the invariant (it must hold), replays the
  transactions in revm on the proven state only (reading anything unproven fails), and runs the
  invariant again. The report wins iff it now fails.
- **Who sends:** four hunter accounts derived from block N's state root, made up by the check
  with the bounty's ETH capital. No contract can know or compute them, so a report can't act as a
  protocol's owner, and a protocol can't refuse hunters: a win shows that *anyone* could break
  the invariant.
- **As mainnet would run it:** hunters pay gas at the block's base fee, and their transactions
  together fit one block's gas limit, so a win is one a real attacker could have.
- **Disclosure:** the hunter commits (witness's commit-reveal fixes priority), discloses
  privately, and reveals after the patch: the bounty's `disclosure_window` refuses any reveal
  until that many slots after its commitment. The check replays block N's state, so the report
  still wins against the patched protocol, and the protocol can't refuse to pay.

## Numbers

Measured in witness-sim (PolkaVM's recompiler, PolkaJam v0.1.29's gas), against a refine's
5×10⁹:

| Report | Gas | Of 5×10⁹ |
|---|---:|---:|
| The toy exploit: 4 accounts, 9 slots | 9.4×10⁶ | 0.2% |
| A 64-hop swap route: 131 accounts, 449 slots, 3.1M EVM gas | 3.6×10⁸ | 7% |
| 400 accounts × 10 slots, proofs only | 3.1×10⁹ | 61% |
| 30M EVM gas of pure compute | 5.0×10⁹ | 100% |

Proofs cost about 90,000 gas a node (an account is 9, a big token's slot 7); EVM compute about 167
gas per EVM gas, DeFi-style transactions about 20. A report's size, not its gas, is usually the
limit.

## Layout

```
check/                     the check (crate evm-bounty-check): revm 43, no_std, over alloy-trie
tools/evm-report/          builds test cases: toy contracts (contracts/Toys.sol) in synthetic state
                           with mainnet-deep proofs; the reports a hunter's tooling would write;
                           `evm-report lint`, the invariant lint
scripts/gas.sh             measures the check's gas, phase by phase, on evm-report's cases
scripts/demo.sh            a settled bug bounty, in witness-sim or on a testnet: posted, disclosed,
                           three reports revealed after the window, paid
docs/design.md             the spike: design, measurements, go/no-go, scope
docs/review.md             adversarial review of the check, first pass: what's fixed, what's open
docs/invariants.md         how to write a bounty's invariant, and the lint that checks one
```

## Build and test

Needs what witness needs: stable Rust, `rust-src`, and `cargo install cargo-jam-build --version
0.1.29 --locked`. witness-check and witness-types come from witness, pinned in `Cargo.toml`; the
scripts also need a witness checkout at that commit, for witness-sim, its CLI and the service.

```bash
cargo test                                  # the cases natively: the exploit wins, forgeries fail
cargo jam-build --out-dir out               # the check blob, out/evm-bounty-check.jam (1.1 MB)
WITNESS_DIR=../witness scripts/demo.sh      # a bounty, posted to paid, in witness-sim (~50 s)
WITNESS_DIR=../witness POLKAJAM_DIR=../polkajam-v0.1.29 scripts/demo.sh --testnet  # on a testnet
WITNESS_DIR=../witness scripts/gas.sh       # the gas table in docs/design.md
```

PolkaVM's recompiler needs `userfaultfd`. In a container that forbids it unprivileged, run
`sysctl -w vm.unprivileged_userfaultfd=1` first.

CI reads witness, a private repository, with a token: a fine-grained personal access token with
read access to its contents, saved as the `WITNESS_READ_TOKEN` secret.

## Limitations

- All test cases are toy contracts in synthetic state; nothing yet runs on real mainnet state or
  real-size contract code.
- `BLOCKHASH` fails the check.
- The protocol commits its own state root; a light client would remove that trust.
- The check has had one adversarial review, by its author's tooling
  ([docs/review.md](docs/review.md)), with open items. Don't put real money behind it before an
  outside review.

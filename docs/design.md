# Design: settled bug bounties

Status: feasibility spike, done: a go. In this repo: `check/` (the check), `tools/evm-report`
(test cases), `scripts/gas.sh` (the measurement) and `scripts/demo.sh` (a bounty in witness-sim,
start to finish).

## The question

A protocol funds a precise condition: *a transaction sequence that breaks invariant I of our
contracts, run on block N's state, earns R.* A hunter's report is the transactions plus
Merkle-Patricia proofs of every account and slot they touch. A witness check verifies the
proofs against block N's state root, replays the transactions in revm, and runs the invariant
before and after. If the invariant held and now fails, the bounty pays out, with no human judge.

Does verifying a report fit in one JAM refine, 5×10⁹ gas?

**Answer: yes, for the reports that matter, with room to spare.** The toy exploit costs
9.4×10⁶ gas, 0.2% of the budget. A large DeFi-shaped report (131 accounts, 449 slots, 3.1M EVM
gas) costs 3.6×10⁸, 7%. Execution is rarely the limit: about 30M EVM gas of pure compute fits,
more than any single transaction may use since Osaka (2²⁴, about 16.8M), and DeFi-style work,
mostly storage reads, is about 8× cheaper to replay than that. What grows with a big exploit is
its footprint, the accounts and slots it touches: proofs are about 3.5 KB an account, so report
size binds before gas does.
**Go**, with the open items in [What has to change](#what-has-to-change).

## How the check works

`check/src/lib.rs`, about 400 lines over revm 43 (no_std, unchanged) and alloy-trie.

- **Params** (the bounty's terms, RLP, under `MAX_PARAMS_LEN` = 4 KB): block N's state root, the
  hard fork, the environment of the block the transactions run in (number, timestamp, gas limit,
  base fee, coinbase, `prevrandao`), the hunters' starting `capital` in wei, and the invariant:
  EVM runtime code returning one word, nonzero while the invariant holds.
- **Witness** (the report, RLP): every account the replay reads, with its proof, its code and
  the storage slots read, each with its proof (absences proven too), and the transactions.
- **Flow:** decode; verify every proof and code hash, building the only state the replay can
  read (anything else, and `BLOCKHASH`, fails the check); run the invariant, which must hold;
  run the transactions in order on one journal (reverts are fine, invalid transactions fail);
  run the invariant again. The report wins iff it now returns zero.

### Who sends the transactions

Transactions are unsigned, so who they come from is the check's choice. The first version let a
report send from *any* proven account. That is unsound: a report could send as a protocol's
owner or admin key and "break" the invariant through a privileged function.

Now transactions come only from four **hunter accounts**, addresses derived from block N's state
root (`hunter(state_root, i)`), made up by the check, each holding the bounty's `capital` in ETH
and nothing else. No contract in N's state can single them out: it can't hold a hash of the
state that holds it, and no opcode reads a state root. (The first version derived them from
`prevrandao`, which a contract reads with `PREVRANDAO`; see the [review](review.md).) A winning
report shows that *anyone* could break the invariant. Hunters get tokens as an attacker would, by trading or flash-borrowing them on chain.
The toy exploit wraps 1 wei of ETH, pays it into the pair and takes 99.8% of the other reserve.

Hunters pay for gas at the block's base fee, with no tip, and their transactions together must
fit the block's gas limit, so the replay is one mainnet would run. Nonces aren't checked, since
nothing is signed. The invariant is called from a separate empty account (EIP-3607 forbids
sending from one with code, which made the first version's invariant call always fail) with
2²⁴ gas, Osaka's per-transaction cap.

## Test cases

All synthetic, from toy contracts written for this (`tools/evm-report/contracts/Toys.sol`):
a WETH-style token, a constant-product pair with a planted bug (its `swap` checks the
fee-adjusted product at the wrong scale), a router, a gas burner, and the pair's invariant
(pair 0's reserve product never falls below its value at block N). No real incidents are
replayed. Public Ethereum RPCs aren't reachable from the environment the spike ran in, so
nothing here uses mainnet state; that is a [next step](#milestones).

The state is synthetic but the proofs are mainnet-shaped. `evm-report` pads every trie so each
proven key has filler siblings at every level of its path: account proofs are 9 nodes (6 full
16-way branches), about 3.5 KB, as `eth_getProof` returns on mainnet; a big token's storage
proofs are 7 nodes, like WETH's. To build a case it replays the transactions with the check's
own `replay`, over a database that records every account and slot read, then proves exactly
those, as a hunter's tooling would with `eth_getProof`.

| Case | What | Outcome |
|---|---|---|
| `burn-1m`, `-10m`, `-30m` | keccak, arithmetic and memory in a loop, in transactions of ≤10M gas | holds |
| `proofs-AxS` | A big-token accounts with S slots each proven, no transactions | holds |
| `pair-swap` | wrap ETH, pay it in, take the fair amount out | holds |
| `pair-exploit` | the planted bug: 1 wei in, 99.8% of a reserve out | **violated: the check accepts** |
| `route-H` | the router swaps through H pairs: a pair and a token more each hop | holds |

The tool's tests also check that a report missing a proof, or with a wrong storage value, fails.

## Measurements

`scripts/gas.sh` runs each case through `witness-sim check`: PolkaVM's recompiler
with PolkaJam v0.1.29's gas, the inner VM the service runs checks in. A phase's gas is the
difference between stopping after it and stopping before it (`Report::stop_after`).

| Case | EVM gas | Accounts | Slots | Report | Decode | Proofs | Invariant (before) | Transactions | Invariant (after) | **Total** | % of 5×10⁹ |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| burn-1m | 1.0M | 2 | 0 | 7.7 KB | 0.18M | 1.98M | 0.50M | 164M | 0.06M | **167M** | 3.3% |
| burn-10m | 10.0M | 2 | 0 | 7.7 KB | 0.18M | 1.98M | 0.50M | 1,671M | 0.06M | **1,674M** | 33% |
| burn-30m | 30.0M | 2 | 0 | 7.9 KB | 0.19M | 1.98M | 0.50M | 5,014M | 0.05M | **5,016M** | 100.3% |
| proofs-10x10 | 0 | 11 | 100 | 295 KB | 5.9M | 71M | 0.50M | 0 | 0.05M | **77M** | 1.5% |
| proofs-100x10 | 0 | 101 | 1,000 | 2.9 MB | 59M | 705M | 0.50M | 0 | 0.06M | **764M** | 15% |
| proofs-400x10 | 0 | 401 | 4,000 | 11.7 MB | 236M | 2,824M | 0.50M | 0 | 0.06M | **3,061M** | 61% |
| pair-swap | 0.15M | 4 | 9 | 29 KB | 0.64M | 7.1M | 0.61M | 0.94M | 0.14M | **9.4M** | 0.2% |
| **pair-exploit** | 0.15M | 4 | 9 | 29 KB | 0.63M | 7.1M | 0.61M | 0.94M | 0.14M | **9.4M** | 0.2% |
| route-4 | 0.22M | 11 | 29 | 86 KB | 1.8M | 21M | 0.61M | 3.9M | 0.14M | **27M** | 0.5% |
| route-16 | 0.80M | 35 | 113 | 296 KB | 6.3M | 72M | 0.62M | 15.5M | 0.14M | **95M** | 1.9% |
| route-64 | 3.1M | 131 | 449 | 1.1 MB | 24M | 276M | 0.61M | 62M | 0.14M | **363M** | 7.3% |

(M = 10⁶ gas. "Invariant (before)" includes building revm's context. Measured after the
[adversarial review](review.md)'s fixes: hunters pay gas, and absent keys' proofs are shallower
than the first measurements', since the synthetic tries now pad only keys that exist.)

### What dominates

- **Proof verification**, for anything that touches real state: **about 90,000 gas per proof
  node**, about 256 gas a byte (89k/node across `proofs-400x10`, 96k/node in `route-64`, where
  storage proofs are shallower). It is keccak over each node plus RLP decoding. Decoding the
  report adds about 20 gas a byte. A proven account with 10 big-token slots is 79 nodes, about
  7M gas.
- **EVM execution** costs **about 167 PVM gas per EVM gas on pure compute** (all three burns).
  On DeFi-shaped transactions it is about **20 PVM gas per EVM gas** (`route-*`), because their
  EVM gas is mostly cold `SLOAD`s and calls, priced for disk reads that cost the replay nothing.
  So compute-bound reports are bounded by EVM gas, state-bound ones by proofs.
- **Fixed costs** are small: the invariant's two runs and building revm's context take about
  0.6–0.75M gas.

At about 0.9×10⁹ gas a second, the worst case that fits (5×10⁹) is about 5.5 s of refine. The
same check natively takes 75 ms for `burn-30m` and 7 ms for `route-64`, so in PolkaVM the check
runs about 60–75× slower than natively.

### What fits

As a rough model, gas ≈ 1M + 90k × proof nodes + 20 × report bytes + k × EVM gas, with k
between 20 (state-bound) and 167 (compute-bound).

- **EVM compute:** about **30M EVM gas** of pure compute, nearly two transactions at Osaka's
  per-transaction cap. Storage-bound transactions replay about 8× cheaper per EVM gas, so far
  more of them fit, until their proofs hit the input limit below.
- **State:** about **50,000 proof nodes** fit the gas, about 1,700 accounts with 3 big-token
  slots each (30 nodes an account). But at about 350 bytes a node that is 17 MB of proofs, more than a work package's input (about 13.8 MB), so the
  input limit binds first: about **40,000 nodes** in a package.
- **A large real exploit**, say 100 accounts, 1,000 slots and 10M EVM gas: about 7,900 nodes
  (0.7×10⁹), a few MB of report and code (under 0.1×10⁹ to decode, plus hashing the code), and
  0.2–1.7×10⁹ to execute. **About 1–2.5×10⁹ in all**, half the budget or less.

Not measured here, and worth measuring next: real contract code. Mainnet contracts run up to
24 KB each, and the check hashes and analyzes every proven account's code; `route-64` proves 156 KB of
toy code. Hashing code should cost about what proof bytes do (keccak), so 1 MB of code is about
0.25×10⁹.

## What has to change

Blocking, found by the spike, now fixed in witness:

1. **The check blob is 1,156,339 bytes, over the old `MAX_CHECK_LEN` (1 MiB).** It is almost all
   revm and its precompiles. Optimizing the check crate for size saves only 4% and costs 25% more
   gas. Dropping the BLS12-381, KZG and P-256 precompiles takes it to 715 KB, but revm reaches all
   precompiles through one `Crypto` trait object, so that means patching revm-precompile (or
   rewriting the precompiles), and any slip makes the replay disagree with Ethereum. It would
   also lose P-256, which passkey wallets use. **Done instead:** `MAX_CHECK_LEN` is now 2 MiB.
   A reveal carries the blob as an extrinsic, 1.1 MB of a package's 13.8 MB input.
2. **Reports must travel as extrinsics.** A reveal's witness was inline only, at most 64 KiB
   (`MAX_WITNESS_LEN`); even the toy exploit's report is 29 KB, and realistic ones are hundreds of
   KB to a few MB. **Done:** `Reveal`'s witness is now a `Witness`, inline or an extrinsic, as
   `Attest`'s is; the commitment is to the witness's bytes either way.

Soundness, before money depends on it:

3. **Adversarial review of the check.** First pass done: [review.md](review.md). It fixed one
   flaw a malicious protocol could use (hunters a contract could recognise) and three ways the
   replay differed from mainnet, and tested the rest. Before real money: a second, outside
   review, and the open items it lists.
4. **Invariants that revert.** If the invariant reverts or runs out of gas after the
   transactions, the report loses, rather than letting a hunter "break" an invariant by making
   it too expensive to evaluate. So invariants must be cheap and total: rule 5 of
   [invariants.md](invariants.md), and the lint warns past a quarter of the gas.
5. **`BLOCKHASH`** fails the check. Supporting it means proving the last 256 headers (or the
   EIP-2935 history contract's storage), which a light client would provide.

Cheaper, not required:

6. **Deduplicate proof nodes.** Every account proof repeats the root and the top levels; a
   report could send each node once and refer to it by hash. An estimated 10% on `route-64`, more
   on wide reports.
7. **Faster keccak** in PolkaVM, since it is most of the proof cost.

## Scope beyond the spike

### Responsible disclosure

A reveal publishes the exploit. A bounty that has to be revealed against a live, unpatched
protocol would be a public 0-day. The check's design avoids that: it runs against **block N's
state, a snapshot**, so the exploit still verifies after the protocol has patched.

1. The hunter commits (witness's commit-reveal already does this: a hash, timestamped, that
   fixes priority).
2. The hunter discloses privately to the protocol, which patches.
3. After an agreed window, the hunter reveals against block N and is paid automatically. The
   protocol can't refuse to pay or quibble over severity: the escrow is locked and the check is
   the judge.

witness enforces the window: a bounty's `disclosure_window` (up to about 90 days) refuses any
reveal until that many slots after its commitment, and keeps the reward escrowed until every
commitment made before expiry has had its window. Every commitment waits the same, so an
earlier one is always revealable first: if the disclosure leaks, or the protocol tries to claim
the report itself, its later commitment loses. The protocol checks a disclosed report with
`witness commitment` (the hash it should find on chain) and `evm-report args` plus
`witness-sim check` (the verdict the chain will reach). `scripts/demo.sh` runs this flow.
Protocols must still re-post with a fresh N after upgrades, since a bounty on old state pays for
bugs that state had. JAM's data availability keeps reports
fetchable for weeks for anyone auditing a payout. Stronger privacy (revealing only that *some*
report wins) would take ZK proofs of the same replay or a TEE; both are much heavier than this,
and not needed for v1.

### Trusting the state root

v1: the protocol commits to block N's state root in its own bounty. It is the one paying, so it
has no reason to lie, and a wrong root only makes its own bounty unwinnable, which hunters can
check before working on it. Later: take roots from an Ethereum light client on JAM (Snowbridge's
beacon client on Polkadot is the obvious source), so a bounty can say "any recent block" and
reports can't be dodged by a stale root.

### Invariants

The invariant is ordinary EVM code, so makers write it in Solidity. Good ones are about value:
"the vault holds at least its total deposits", "no pair's k decreases", "the oracle's price stays
within X of the TWAP". Bounty terms have to fix the constants (here, k at block N). Oracle- and
token-dependent invariants need care, since hunters can move prices within the replay; that is
the point, but makers must decide which moves count as a bug. Writing and auditing invariants is
a service in itself: probably the business's main product, more than the check.
[invariants.md](invariants.md) is the guide so far, with `evm-report lint` to check one.

### Milestones

1. ~~Make reports extrinsics; get the blob under the limit.~~ Done.
2. Adversarial hardening of the check (items 3–5 above), with a review by someone outside.
3. ~~Bounty terms for disclosure: patch window, minimum commit-to-reveal delay.~~ Done:
   witness's `disclosure_window`.
4. A demo, **started**: `scripts/demo.sh` runs a bounty from posting to payout, in witness-sim
   or (`--testnet`) on a local PolkaJam testnet. A protocol escrows 5000 on its pair, with a
   disclosure window; the hunter with the exploit discloses it privately, and the protocol
   verifies the report off chain; his reveal is refused until the window has passed. Then a
   fair swap and an exploit on forged state are refused, and the real exploit (a 29 KB report,
   carried as an extrinsic, 9.4×10⁶ gas to check) opens the claim window and is paid at
   finalize. Still to do: about a dozen cases, including
   measurements on real mainnet state (needs an RPC, and an archive node for older blocks), and
   real-size contract code.
5. State roots from a light client.
6. Invariant tooling: templates for common DeFi invariants, and a local harness (Foundry) to test
   them against the same replay.
7. Splitting reports too big for one refine (attest proofs in batches, as witness's
   `sorted-words` check attests word volumes, then reveal against attested state).

### Business

Immunefi, Sherlock, Cantina and Hats all have humans judge reports and protocols decide what to
pay. The edge here is **automatic settlement**: funds escrowed against a precise condition,
paid by a check, with nothing to negotiate. Risks:

- **Protocols may prefer discretion.** A pre-committed, automatically paid bounty removes
  their room to argue severity or scope; some will see that as the cost, not the feature.
- **Disclosure.** The flow above depends on hunters waiting out the patch window, and on
  protocols re-posting after upgrades.
- **State trust** until a light client exists.
- **JAM's timeline.** This needs JAM in production with real value on it.
- **Adam's employment agreement**, which should be checked before this becomes a business.

## Reproducing

```bash
cargo test                                  # the cases natively, including the exploit winning
WITNESS_DIR=../witness scripts/gas.sh       # build everything and print the table above
WITNESS_DIR=../witness scripts/demo.sh      # a bounty, posted to paid, in witness-sim
WITNESS_DIR=../witness POLKAJAM_DIR=... scripts/demo.sh --testnet   # the same on a PolkaJam testnet
```

PolkaVM's recompiler needs `userfaultfd`. In a container that forbids it unprivileged, run
`sysctl -w vm.unprivileged_userfaultfd=1` first. `tools/evm-report/contracts/compile.js`
recompiles `Toys.sol` with solcjs (npm `solc@0.8.28`) if the contracts change.

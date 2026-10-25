# Adversarial review of the check, first pass

Status: first pass, by the author's tooling, not independent. Its fixes and tests are in
`check/src/lib.rs` and `tools/evm-report/src/lib.rs`. Real money needs a second, outside review.

## What was reviewed, against whom

`check/src/lib.rs`: decoding, proof verification, the proven-state database, the made-up
accounts, the revm configuration, and the replay. Three adversaries:

- **A hunter** wants a win the bounty shouldn't pay: a report that "breaks" the invariant on
  state that isn't block N's, or in a way no real attacker could.
- **A protocol** (the bounty's maker) wants a bounty that looks winnable and isn't, so it gets
  the credibility of a public bounty without paying one.
- **Anyone** wants to make the check misbehave: panic, loop, or accept malformed input.

Each claim below is a test that fails on the code it guards against. Tests that pass when nothing
happens have a control that shows the harness sees the effect; one control caught a broken test
(hand-assembled bytecode with a stray `POP`) that had been passing for the wrong reason.

## Fixed

| # | Who | Finding | Fix | Test |
|---|---|---|---|---|
| R1 | protocol | Hunter addresses derived from `prevrandao`, which contracts read at run time (`PREVRANDAO`). A protocol's contracts could recompute them and refuse hunters, making its bounty unwinnable while looking normal. | Derive hunters from block N's state root. A contract in N's state can't hold a hash of that state, and no opcode reads a state root. | `a_protocol_cannot_single_out_hunters`: `RiggedToken` refuses hunters it computes from `prevrandao`; the exploit failed, now wins |
| R2 | hunter | Gas was free (gas price zero, base fee off). A win could rest on what mainnet never runs: contracts that read `GASPRICE`, or balances that never pay fees. | Hunters pay at the block's base fee, no tip; the invariant's caller is funded for its two runs. | `hunters_pay_for_gas`: 1 wei covers the exploit's deposit but not its gas, and now fails |
| R3 | hunter | No limit on the transactions' total gas: a report could do in one "block" what needs several, with the block's environment fixed. | The transactions' gas, together, must fit `Params::gas_limit`. | `the_transactions_fit_one_block` |
| R4 | anyone | Params and report decoded one RLP item and ignored what followed, so many byte strings meant one report. | Trailing bytes are malformed. | `trailing_bytes_are_malformed` |

Found while building the spike, fixed before this pass:

- **Any sender** (critical, hunter): the first draft let transactions come from any proven
  account, so a report could act as a protocol's owner. Now only the hunter accounts send.
  Tests: `only_hunters_send`, `a_report_cannot_prove_a_hunter`.
- **The invariant call always failed**: it was sent from the invariant's own address, which has
  code, and EIP-3607 forbids that. Now an empty account calls it.

## Checked, holds

- **Unproven state can't be read, however deep.** Any account, code or slot the replay touches
  must be proven; a missing one aborts the whole check, including three calls down, never
  turning into a revert that makes a contract look empty. Tests: `a_missing_proof_fails`,
  `a_missing_slot_fails_even_deep_in_a_call`.
- **Proven means proven.** A wrong value, or forged reserves, fails its proof. Tests:
  `a_wrong_value_fails`; the demo's forged report.
- **Transient storage doesn't leak** between a report's transactions (EIP-1153). Test:
  `transient_storage_does_not_leak_between_transactions`, with
  `the_transient_harness_sees_a_real_write` as its control.
- **Made-up accounts can't be proven** (the invariant's, its caller's, the hunters'), and no
  transaction sends from anyone else.
- **Nothing that goes wrong wins.** A panic, running out of gas, an invalid transaction, the
  invariant reverting or not returning one word, and `stop_after` all end in false.
  `Bytecode::new_raw` panics on malformed `0xEF01…` code; proven code is mainnet's, so that can
  only make a report lose.

## Open

| # | Who | Issue | What to do |
|---|---|---|---|
| O1 | hunter | **Invariants that ask hunter code.** A hunter's contract can tell when the invariant is calling (`tx.origin` is the invariant's caller, the only sender that isn't a hunter) and answer differently. If an invariant reads values that pass through hunter-controlled code (a permissionless strategy's `totalAssets`, a hunter's token), the hunter can "break" it with no real damage. | Invariant guidance: read balances and storage of trusted contracts only. Invariant tooling should flag calls that can reach untrusted code. |
| O2 | protocol | **The protocol sets the terms.** A wrong state root, fork, or block environment, or a protocol that special-cases the invariant's fixed address, makes a bounty unwinnable. All of it is public, in the params and the protocol's code. | Hunters check the params against the chain before working; a light client (state roots and headers) removes most of this. |
| O3 | — | **Hunters are fresh accounts.** A protocol that only serves accounts it already knows (an allowlist) can't be exploited in the replay, though an insider could exploit it on mainnet. A miss, never a false win. | Scope note for bounty terms. Later: let bounties name accounts hunters may act as. |
| O4 | — | **Block fields the params don't set** take revm's defaults: the blob base fee is 1 wei, whatever mainnet's was. | Add the blob fields to `Params`. |
| O5 | — | **One block.** The replay is one block's worth of transactions with one environment, so attacks over several blocks (moving a TWAP, waiting out a delay) can't be expressed. A miss, never a false win. | Scope note; multi-block replay is a design change. |
| O6 | — | **`BLOCKHASH`** fails the check. | Prove recent headers or the EIP-2935 history contract's storage; a light client would provide them. |
| O7 | — | **Trusted dependencies:** revm's EVM and precompiles, alloy-trie's `verify_proof`. A bug in either is a bug in the check. | Pin exact versions (done, by `Cargo.lock`); follow their advisories; the outside review should cover how the check uses them. |

## Not covered by this pass

Gas exhaustion attacks against refine itself (a report built to cost exactly 5×10⁹ and fail
late) only cost the hunter their commit deposit, so they weren't pursued. Nor were the witness
service's own rules, reviewed separately in witness.

# Writing a bounty's invariant

A bounty pays whoever breaks its invariant, and nobody judges the report but the check. So the
invariant *is* the bounty's terms: what it says is what gets paid for, exactly. This guide is how
to write one that pays for real bugs and only for them. `evm-report lint` checks the mechanical
parts (see [The lint](#the-lint)).

## What an invariant is

EVM runtime code. The check deploys it at a fixed address and calls it twice with empty calldata,
2²⁴ gas each time: on block N's state, where it must hold, and after the report's
transactions. It returns one 32-byte word: nonzero while the invariant holds. In Solidity:

```solidity
contract MyInvariant {
	Vault constant VAULT = Vault(0x...);
	Token constant TOKEN = Token(0x...);

	fallback(bytes calldata) external returns (bytes memory) {
		return abi.encode(TOKEN.balanceOf(address(VAULT)) >= VAULT.deposits());
	}
}
```

The bounty's params carry its runtime code, under 4 KB with everything else in the params.

## The rules

### 1. Read what's held, not what you're told

Read state the protocol can't misreport: token balances in the token's own books, storage of
your own contracts. Don't ask code that someone else controls.

The trap: anything the invariant calls that a hunter can choose. A vault that sums what its
strategies report, where anyone can add a strategy, lets a hunter add one that answers the
invariant differently from everyone else. Its code can tell when the invariant is asking (the
invariant's caller is the only sender that isn't a hunter). Then the invariant "breaks" with
nothing actually lost, and the bounty pays for nothing.

```solidity
// Asks the vault, which asks every strategy anyone added: the lint flags this.
return abi.encode(VAULT.totalAssets() >= VAULT.deposits());

// Reads what the vault holds from the token: a strategy's word never enters it.
return abi.encode(TOKEN.balanceOf(address(VAULT)) >= VAULT.deposits());
```

The same goes for any address the protocol reads from storage that anyone can write: a
registry, a hook, a callback, a list of markets. If the protocol's real accounting does depend
on such code, that dependency is the bug to put a bounty on, and the invariant should still read
the funds it protects, not the accounting.

### 2. Depend on state, never on the block

Don't read `block.timestamp`, `block.number`, `tx.origin`, `msg.sender`, `tx.gasprice`,
`block.prevrandao`, `block.coinbase` or `block.basefee`. The bounty's params set them, the same
before and after the report, so an invariant that reads them tests the params, not the protocol.
(Protocol code may read them; that's its business. The invariant's own code mustn't.)

### 3. Hold at block N, with room

The check refuses every report if the invariant doesn't hold on block N's state, so a bounty
whose invariant is already broken can never pay. Run the lint on N's state before posting.

Fix constants from block N where the invariant needs a baseline (a pair's reserve product, a
vault's share price): read them at N and write them into the code.

### 4. Leave a margin for rounding

Integer arithmetic rounds. If rounding can move a value by a few wei in a fair transaction, an
invariant with no margin pays out for dust. State what loss is a bug: "the vault holds at least
its deposits, less 1 wei per depositor", "k never falls by more than 1 part in 10¹²". The margin
is part of the terms.

### 5. Be cheap and total

The invariant gets 2²⁴ gas. If it reverts or runs out of gas after the report's transactions, the
report loses: the check can't tell a broken protocol from a broken invariant. So:

- Keep loops bounded, by a constant in the invariant rather than by a list a hunter can grow.
- Decide what a failing call means. If a hunter can make a view revert (bricking a market, say)
  and that's a bug worth paying for, catch it and return zero; if not, let it revert.
- The lint warns past a quarter of the budget.

### 6. Say what's in scope

A report runs one block's worth of transactions, from up to four fresh accounts holding the
bounty's ETH capital, so that's the attacker a bounty prices:

- Anyone, with no special role: hunters can't act as owners, admins, or accounts the protocol
  already knows. A bug only an insider can reach needs a different bounty.
- One block: attacks that need time to pass (a TWAP to move, a delay to elapse) are out of scope.
- Capital: the ETH you give hunters, plus anything they can borrow or trade for on chain within
  the block. Flash loans are in scope if the lenders are in the state the report proves.

## The lint

`evm-report lint` runs an invariant once on block N's state, as the check does, and watches.

| Severity | Finding |
|---|---|
| Error | The invariant doesn't hold at N, reverts, or doesn't return one word |
| Error | Its own code reads the block or transaction (`TIMESTAMP`, `ORIGIN`, ...) |
| Error | It deploys a contract |
| Warning | It uses more than a quarter of its gas |
| Warning | Code it calls reads `ORIGIN`: that code can answer the invariant differently |
| Review | Code it runs reached by an address read from storage. An array or mapping entry (a list of strategies, a registry) is the pattern in rule 1; a fixed slot (a token set at deployment) is usually fine. Say which, and why. |

It sees one run, on the state before any report: a path the invariant only takes after some
transactions (a new market, a list grown longer) isn't in it. So a clean lint means none of these
were on the path at block N, not that there are none. Read every Review item against the
protocol's code: who can write that slot?

For the toy invariants (`tools/evm-report/contracts/Toys.sol`):

- `PairInvariant` reads a pair's reserves: clean.
- `VaultAssetsInvariant` asks the vault for its total assets: two Review items, the strategy (an
  entry in the vault's list, which anyone can add to) and the token (slot 0, set once). The
  strategy is rule 1's trap.
- `VaultBalanceInvariant` reads the token's balance of the vault: clean, and the strategy's code
  never runs.

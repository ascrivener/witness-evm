#!/usr/bin/env bash
# A settled bug bounty, start to finish:
#
#   WITNESS_DIR=path/to/witness scripts/demo.sh            # in witness-sim, about a minute
#   WITNESS_DIR=path/to/witness POLKAJAM_DIR=path/to/polkajam-v0.1.29 scripts/demo.sh --testnet
#
# WITNESS_DIR is a checkout of witness (at the commit Cargo.toml pins), for the service, its CLI,
# witness-sim and its submit scripts. --testnet starts a local PolkaJam testnet (v0.1.29), unless
# JAM_RPC names a running one, and runs every op on it: about 10 minutes, mostly waiting out
# the disclosure and claim windows at 6 seconds a slot.
#
# A protocol posts a bounty on its toy trading pair (tools/evm-report/contracts/Toys.sol): a
# report that breaks the pair's invariant (its reserves' product never falls) on block N's state
# earns 5000. The bounty's check is check/ (evm-bounty-check); its params commit to block N's
# state root and the invariant. Its terms give the protocol a disclosure window: no report goes
# on chain until DISCLOSE slots after its commitment. Three hunters' reports, written by
# `evm-report demo`, come in:
#
#   mallory  a fair swap: the invariant still holds, so the check says no
#   carol    the exploit on forged state (the pair's reserves claimed tiny): a proof fails
#   bob      the exploit: 1 wei in, 99.8% of a reserve out
#
# Each hunter commits. Bob then shows the protocol his report privately: it recomputes his
# commitment and finds it on chain, and runs the check on the report itself, off chain. Bob's
# reveal is refused while the window is open, which is the protocol's time to fix the pair. Once
# it has passed, all three reveal: the check refuses mallory's and carol's and accepts bob's,
# since it replays block N's state, from before the fix. After the claim window, bob is paid.
#
# Reports are tens of KB of Merkle proofs, so they travel as extrinsics. witness-sim runs the
# check in PolkaVM (whose recompiler needs userfaultfd: in a container, run
# `sysctl -w vm.unprivileged_userfaultfd=1` first).
set -euo pipefail
: "${WITNESS_DIR:?set WITNESS_DIR to a witness checkout}"
WITNESS_DIR=$(cd "$WITNESS_DIR" && pwd)
TESTNET=
case "${1:-}" in
	--testnet) TESTNET=1 ;;
	"") ;;
	*) echo "usage: $0 [--testnet]" >&2; exit 2 ;;
esac
cd "$(dirname "$0")/.."
# PolkaVM's recompiler makes its sandbox with userfaultfd: without it, every check is BadCheckBlob.
if [ "$(cat /proc/sys/vm/unprivileged_userfaultfd 2>/dev/null || echo 1)" = 0 ]; then
	echo 'vm.unprivileged_userfaultfd is 0: run `sysctl -w vm.unprivileged_userfaultfd=1` first' >&2
	exit 1
fi

W=$WITNESS_DIR/target/debug/witness
SIM=$WITNESS_DIR/target/debug/witness-sim
SERVICE=$WITNESS_DIR/out/witness-service.jam
TMP=$(mktemp -d)
X=$TMP/extrinsics
DEMO=$TMP/demo
CHECK=out/evm-bounty-check.jam
# The check takes about 10⁷; 10⁸ leaves room, and fits the testnet's refine limit of 10⁹.
GAS=100000000
REWARD=5000
# The protocol's time to fix the bug, in slots: a demo's worth, where a real bounty gives weeks.
# On a testnet, long enough that bob's early reveal surely lands inside it.
DISCLOSE=${DISCLOSE:-$([ -n "$TESTNET" ] && echo 20 || echo 10)}

step() { printf '\n== %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

# The chain, witness-sim or a testnet, gives `slot`, `submit SERVICE PAYLOAD [EXTRINSIC...]`,
# `raw_storage SERVICE KEY`, and POLL, the seconds between looks at it.
if [ -n "$TESTNET" ]; then
	JAMT=${POLKAJAM_DIR:+$POLKAJAM_DIR/}jamt
	POLL=2
	slot() { "$JAMT" inspect slot; }
	submit() { python3 "$WITNESS_DIR/scripts/jam-submit.py" "$@"; }
	# jamt cuts values over 1 KiB short.
	raw_storage() { python3 "$WITNESS_DIR/scripts/jam-storage.py" "$1" "$2" 2>/dev/null || true; }
else
	export WITNESS_SIM_DIR=$TMP/sim
	POLL=0
	# witness-sim passes a slot with every item and every `inspect slot`.
	slot() { "$SIM" inspect slot; }
	submit() { "$SIM" item "$@"; }
	raw_storage() { "$SIM" inspect storage --raw "$1" "$2" 2>/dev/null || true; }
fi
wait_slot() { while [ "$(slot)" -lt "$1" ]; do sleep $POLL; done; }
# One item per slot: a testnet silently drops some packages submitted together in one slot.
item() { submit "$SVC" "$@" >/dev/null; wait_slot $(( $(slot) + 1 )); }
itemx() { item "$1" "$X"/*; }

storage() {
	local v; v=$(raw_storage "$SVC" "$($W key "$@")")
	case $v in 0x*) echo "$v" ;; *) echo "" ;; esac
}
# `witness decode` of bounty 0, WHO's test-asset balance, or bounty 0's commitment HASH.
bounty() { local v; v=$(storage --bounty 0); [ -n "$v" ] && $W decode --bounty "$v" || echo none; }
balance() { local v; v=$(storage --balance "$1" --asset test); [ -n "$v" ] && $W decode --balance "$v" || echo 0; }
commitment() { local v; v=$(storage --bounty 0 --commitment "$1"); [ -n "$v" ] && $W decode --commitment "$v" || echo none; }
# Wait until COMMAND prints something matching PATTERN.
expect() { # DESCRIPTION PATTERN COMMAND...
	local what=$1 pattern=$2; shift 2
	local out=
	for _ in $(seq 1 60); do
		out=$("$@")
		if grep -qE "$pattern" <<<"$out"; then echo "ok: $what"; return; fi
		sleep $POLL
	done
	fail "$what: wanted /$pattern/, got: $out"
}
# Something that must not happen: give it time to (wrongly) happen, then check.
expect_still() { sleep $((POLL * 9)); expect "$@"; }
# Why witness-sim refused the last op (a testnet doesn't say).
why() { if [ -z "$TESTNET" ]; then grep -E 'Rejected|not applied' "$WITNESS_SIM_DIR/log" | tail -1 | sed -E 's/.*INFO witness +/   witness-sim: /' || true; fi; }

step "building"
(cd "$WITNESS_DIR" && cargo build -q -p witness-cli -p witness-sim &&
	cargo jam-build --out-dir out -p witness-service >/dev/null)
cargo build -q -p evm-report
cargo jam-build --out-dir out -p evm-bounty-check >/dev/null
./target/debug/evm-report demo "$DEMO"
PARAMS=$(cat "$DEMO/bounty.params")
echo "check blob: $(wc -c <"$CHECK") bytes; params: $(( (${#PARAMS} - 2) / 2 )) bytes"
for r in fair forged exploit; do echo "$r report: $(wc -c <"$DEMO/$r.report") bytes"; done

if [ -n "$TESTNET" ] && [ -z "${JAM_RPC:-}" ]; then
	step "starting a testnet (log: $TMP/testnet.log)"
	"${POLKAJAM_DIR:+$POLKAJAM_DIR/}polkajam-testnet" >"$TMP/testnet.log" 2>&1 &
	TESTNET_PID=$!
	trap 'pkill -P $TESTNET_PID 2>/dev/null; kill $TESTNET_PID 2>/dev/null || true' EXIT
	for _ in $(seq 1 60); do "$JAMT" list >/dev/null 2>&1 && break; sleep 2; done
fi

step "deploying witness"
if [ -n "$TESTNET" ]; then
	before=$("$JAMT" list | wc -l)
	# witness_types::DEPOSIT_GAS: what witness needs to credit a deposit.
	"$JAMT" create-service "$SERVICE" 10000000000 --min-memo-gas 300000 >/dev/null
	for _ in $(seq 1 40); do [ "$("$JAMT" list | wc -l)" -gt "$before" ] && break; sleep $POLL; done
	SVC=$((16#$("$JAMT" list | tail -1 | cut -f1)))
	# Packages anchored before the service existed can't see its code.
	for _ in $(seq 1 40); do "$JAMT" inspect final service "$SVC" >/dev/null 2>&1 && break; sleep $POLL; done
else
	"$SIM" create-service "$SERVICE" 10000000000 >/dev/null
	SVC=$((16#$("$SIM" list | tail -1 | cut -f1)))
fi
echo "service $SVC"
# On a testnet the first package can be anchored before guarantors see the new code (refine:
# BadCode), so send the configuration until it's there.
config() { local v; v=$(storage --config); [ -n "$v" ] && $W decode --config "$v" || echo none; }
for _ in 1 2 3 4 5; do
	item "$($W propose --service "$SVC" --nonce 0 --fee-asset test --commit-deposit 10 --attest-deposit 10)"
	for _ in 1 2 3 4 5 6; do [ "$(config)" != none ] && break 2; sleep $POLL; done
	echo "   the configuration didn't land; sending it again"
done
expect "witness is configured" 'commit_deposit: 10' config
item "$($W mint --to protocol --amount 10000)"
for who in bob carol mallory; do item "$($W mint --to $who --amount 100)"; done
expect "the protocol has 10000" '^10000$' balance protocol
for who in bob carol mallory; do expect "$who has 100" '^100$' balance $who; done

step "the protocol posts the bounty, with a disclosure window of $DISCLOSE slots"
item "$($W create --service "$SVC" --maker protocol --nonce 0 --check $CHECK --params "$PARAMS" \
	--reward $REWARD --check-gas $GAS --expiry $(( $(slot) + 200 )) --disclosure-window "$DISCLOSE")"
expect "bounty 0 open" 'status: Open' bounty
expect "the reward is escrowed" "^$((10000 - REWARD))\$" balance protocol

step "hunters commit"
commit() { # HUNTER SALT REPORT
	item "$($W commit --service "$SVC" --bounty 0 --hunter "$1" --nonce 0 --salt "$2" \
		--witness-file "$DEMO/$3.report" 2>/dev/null)"
}
commit mallory 0x01 fair
commit carol 0x02 forged
commit bob 0x03 exploit
for who in bob carol mallory; do expect "$who committed, deposit taken" '^90$' balance $who; done

step "bob shows the protocol his report, privately"
# Off chain: bob sends the protocol the report and his salt. It recomputes his commitment from
# them, finds it on chain, and runs the bounty's check on the report itself.
BOB_C=$($W commitment --bounty 0 --hunter bob --salt 0x03 --witness-file "$DEMO/exploit.report")
expect "the protocol finds bob's commitment on chain" 'slot: [0-9]+' commitment "$BOB_C"
COMMITTED=$(commitment "$BOB_C" | sed -nE 's/.*slot: ([0-9]+).*/\1/p')
./target/debug/evm-report args "$DEMO/bounty.params" "$DEMO/exploit.report" "$TMP/exploit.args"
OFF_CHAIN=$("$SIM" check "$CHECK" "$TMP/exploit.args" $GAS)
expect "the check, run by the protocol, accepts bob's report ($(sed -E 's/.*used ([0-9]+) gas.*/\1/' <<<"$OFF_CHAIN") gas)" \
	'^Ok\(\(\)\)' echo "$OFF_CHAIN"
echo "   the protocol has until slot $((COMMITTED + DISCLOSE)) to fix the pair before the report goes on chain"

reveal() { # HUNTER SALT REPORT
	itemx "$($W reveal --bounty 0 --check $CHECK --params "$PARAMS" --check-gas $GAS --hunter "$1" \
		--salt "$2" --witness-file "$DEMO/$3.report" --extrinsics "$X")"
}

step "bob tries to reveal early"
reveal bob 0x03 exploit
expect_still "his reveal is refused while the window is open" 'status: Open' bounty
why
expect "and his commitment stands" 'slot: [0-9]+' commitment "$BOB_C"

step "the protocol fixes its pair; the window passes"
# The fix is the protocol's own business, off this chain. The bounty doesn't change: its state
# root is still block N's, from before the fix.
wait_slot $((COMMITTED + DISCLOSE))
echo "   slot $(slot): the window is over"

step "hunters reveal"
reveal mallory 0x01 fair
expect_still "the check says no to a fair swap" 'status: Open' bounty
why
expect "mallory's deposit stays taken" '^90$' balance mallory
reveal carol 0x02 forged
expect_still "forged state fails its proof" 'status: Open' bounty
why
expect "carol's deposit stays taken" '^90$' balance carol
reveal bob 0x03 exploit
expect "the exploit breaks the invariant and opens the claim window" 'Revealing' bounty
expect "bob's deposit refunded" '^100$' balance bob

step "waiting out the claim window, then finalizing"
wait_slot "$(bounty | sed -nE 's/.*window_end: ([0-9]+).*/\1/p')"
item "$($W finalize --bounty 0)"
expect "bounty 0 claimed" 'Claimed' bounty
expect "bob is paid the reward" "^$((100 + REWARD))\$" balance bob
for who in carol mallory; do expect "$who lost the commit deposit" '^90$' balance $who; done

printf '\nthe settled bug bounty demo passed\n'

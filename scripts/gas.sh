#!/usr/bin/env bash
# Measures the evm-bounty check's gas, phase by phase, on the spike's test cases.
#
#   WITNESS_DIR=path/to/witness scripts/gas.sh [OUT_DIR]
#
# WITNESS_DIR is a checkout of witness (at the commit Cargo.toml pins), for witness-sim. Builds
# the check blob (cargo jam-build), evm-report and witness-sim, writes the cases with
# evm-report into OUT_DIR (default out/evm-cases), then runs `witness-sim check` on each case's
# arguments at every `stop_after`. Prints a table: the gas each phase took (by difference), the
# total, and whether the whole check accepted. Checks get far more gas than a refine has
# (GAS, default 10¹²), so a case past 5×10⁹ still shows what it would cost.
#
# PolkaVM's recompiler needs userfaultfd: in a container, run
# `sysctl -w vm.unprivileged_userfaultfd=1` first.
set -euo pipefail
: "${WITNESS_DIR:?set WITNESS_DIR to a witness checkout}"
WITNESS_DIR=$(cd "$WITNESS_DIR" && pwd)
cd "$(dirname "$0")/.."
# PolkaVM's recompiler makes its sandbox with userfaultfd: without it, every check is BadCheckBlob.
if [ "$(cat /proc/sys/vm/unprivileged_userfaultfd 2>/dev/null || echo 1)" = 0 ]; then
	echo 'vm.unprivileged_userfaultfd is 0: run `sysctl -w vm.unprivileged_userfaultfd=1` first' >&2
	exit 1
fi
SIM=$WITNESS_DIR/target/release/witness-sim
OUT=${1:-out/evm-cases}
GAS=${GAS:-1000000000000}

cargo jam-build --out-dir out -p evm-bounty-check >/dev/null
cargo build --release -q -p evm-report
(cd "$WITNESS_DIR" && cargo build --release -q -p witness-sim)
target/release/evm-report "$OUT" >/dev/null

gas() { # args file -> gas used
	"$SIM" check out/evm-bounty-check.jam "$1" "$GAS" | sed -E 's/.*used ([0-9]+) gas.*/\1/'
}

printf '%-14s %14s %14s %14s %14s %14s %16s  %s\n' case decode proofs inv_before txs inv_after total result
tail -n +2 "$OUT/cases.tsv" | cut -f1 | while read -r name; do
	prev=0
	row=()
	for p in 1 2 3 4 0; do
		used=$(gas "$OUT/$name.$p.args")
		row+=($((used - prev)))
		prev=$used
	done
	result=$("$SIM" check out/evm-bounty-check.jam "$OUT/$name.0.args" "$GAS" | cut -d: -f1)
	printf '%-14s %14s %14s %14s %14s %14s %16s  %s\n' "$name" "${row[@]}" "$prev" "$result"
done

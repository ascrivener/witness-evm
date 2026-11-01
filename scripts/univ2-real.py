#!/usr/bin/env python3
"""A bounty and a report on real mainnet state: a fair swap on Uniswap V2's USDC/WETH pair, under
"the pair's reserve product never falls", at a recent block. Checks `evm-report rpc` end to end
against a node, without an archive: public endpoints prove recent blocks.

    scripts/univ2-real.py RPC_URL OUT_DIR [EVM_REPORT]

Writes OUT_DIR/invariant.hex and OUT_DIR/txs.json, then runs `evm-report rpc` on them."""
import json, os, subprocess, sys, urllib.request

PAIR = "b4e16d0168e52d35cacd2c6185b44281ec28c9dc"  # Uniswap V2 USDC/WETH: token0 USDC, token1 WETH
WETH = "c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
AMOUNT_IN = 10**18

url, out = sys.argv[1], sys.argv[2]
evm_report = sys.argv[3] if len(sys.argv) > 3 else "target/release/evm-report"

def rpc(method, params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(url, body, {"content-type": "application/json", "user-agent": "evm-report"})
    reply = json.load(urllib.request.urlopen(req, timeout=30))
    if "error" in reply:
        sys.exit(f"{method}: {reply['error']}")
    return reply["result"]

n = int(rpc("eth_blockNumber", []), 16) - 3
reserves = rpc("eth_call", [{"to": "0x" + PAIR, "data": "0x0902f1ac"}, hex(n)])
r_usdc, r_weth = int(reserves[2:66], 16), int(reserves[66:130], 16)
usdc_out = AMOUNT_IN * 997 * r_usdc // (r_weth * 1000 + AMOUNT_IN * 997)
k = r_usdc * r_weth

# The invariant, by hand: getReserves() on the pair, then return r0 * r1 >= k as one word.
code = (
    "630902f1ac60e01b5f52"        # mstore(0, getReserves.selector << 224)
    "60405f60045f73" + PAIR +     # staticcall(gas, pair, 0, 4, 0, 0x40)
    "5afa"
    "1560XX57"                    # if it failed, jump to revert
    "5f5160205102"                # mload(0) * mload(0x20)
    "7f" + f"{k:064x}" +          # k
    "11155f52"                    # mstore(0, !(k > product))
    "60205ff3"                    # return(0, 32)
    "5b5f5ffd"                    # revert(0, 0)
)
dest = len(bytes.fromhex(code.replace("XX", "00"))) - 4  # the JUMPDEST
code = code.replace("XX", f"{dest:02x}")

word = lambda x: f"{x:064x}"
txs = [
    {"hunter": 0, "to": "0x" + WETH, "value": hex(AMOUNT_IN), "data": "0xd0e30db0", "gas": 100000},
    {"hunter": 0, "to": "0x" + WETH, "data": "0xa9059cbb" + word(int(PAIR, 16)) + word(AMOUNT_IN), "gas": 100000},
    {"hunter": 0, "to": "0x" + PAIR, "gas": 200000,
     "data": "0x022c0d9f" + word(usdc_out) + word(0) + "000000000000000000000000HUNTER0" + word(0x80) + word(0)},
]
os.makedirs(out, exist_ok=True)
open(os.path.join(out, "invariant.hex"), "w").write(code + "\n")
json.dump(txs, open(os.path.join(out, "txs.json"), "w"), indent=1)
print(f"block {n}: reserves {r_usdc / 1e6:.0f} USDC, {r_weth / 1e18:.1f} WETH; 1 WETH in for {usdc_out / 1e6:.2f} USDC out")
sys.exit(subprocess.call([evm_report, "rpc", url, str(n), os.path.join(out, "invariant.hex"), os.path.join(out, "txs.json"), out]))

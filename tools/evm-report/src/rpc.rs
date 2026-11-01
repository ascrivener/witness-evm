//! Reports on real chain state, from a JSON-RPC node.
//!
//! [`build`] replays a report's transactions on block N's state as the node serves it (each
//! account and slot fetched the first time revm reads it), records what the replay read, and
//! proves exactly that with `eth_getProof` at block N, as a hunter's tooling would. The bounty's
//! params take N's state root, and the environment of block N + 1, the block the transactions
//! would have run in.
//!
//! `eth_getProof` at an old block needs an archive node. Public endpoints serve recent blocks
//! only (publicnode: about the last 128).

use alloy_trie::{Nibbles, proof::verify_proof};
use evm_bounty_check::{AccountWitness, Outcome, Params, Report, Slot, TrieAccount, Tx, Unproven, made_up, replay};
use revm::{
	Database,
	bytecode::Bytecode,
	primitives::{Address, B256, Bytes, KECCAK_EMPTY, U256, keccak256},
	state::AccountInfo,
};
use serde_json::{Value, json};
use std::{
	cell::Cell,
	collections::{BTreeMap, BTreeSet},
	str::FromStr,
};

/// A JSON-RPC endpoint.
pub struct Rpc {
	agent: ureq::Agent,
	url: String,
	id: Cell<u64>,
	/// Requests made, for the summary.
	pub calls: Cell<u64>,
}

impl Rpc {
	pub fn new(url: &str) -> Self {
		// `HTTPS_PROXY` and the system's CA certificates are honored where set.
		let agent = ureq::AgentBuilder::new().try_proxy_from_env(true).build();
		Self { agent, url: url.into(), id: Cell::new(0), calls: Cell::new(0) }
	}

	pub fn call(&self, method: &str, params: Value) -> Value {
		self.id.set(self.id.get() + 1);
		self.calls.set(self.calls.get() + 1);
		let body = json!({ "jsonrpc": "2.0", "id": self.id.get(), "method": method, "params": params });
		let mut last = String::new();
		for attempt in 0..4 {
			if attempt > 0 {
				std::thread::sleep(std::time::Duration::from_secs(1 << attempt));
			}
			match self.agent.post(&self.url).send_json(&body) {
				Ok(response) => {
					let reply: Value = response.into_json().expect("JSON-RPC reply");
					if let Some(error) = reply.get("error") {
						panic!("{method}: {error}");
					}
					return reply["result"].clone();
				},
				Err(e) => last = e.to_string(),
			}
		}
		panic!("{method}: {last}");
	}
}

pub fn tag(block: u64) -> String {
	format!("{block:#x}")
}

fn quantity(value: &Value) -> U256 {
	let s = value.as_str().expect("a hex quantity");
	U256::from_str_radix(s.trim_start_matches("0x"), 16).expect("a hex quantity")
}

fn u64_of(value: &Value) -> u64 {
	quantity(value).to()
}

fn data(value: &Value) -> Bytes {
	Bytes::from_str(value.as_str().expect("hex data")).expect("hex data")
}

fn b256(value: &Value) -> B256 {
	B256::from_str(value.as_str().expect("a hex word")).expect("a hex word")
}

fn address(value: &Value) -> Address {
	Address::from_str(value.as_str().expect("an address")).expect("an address")
}

/// Block N's state, as the node serves it, recording what's read.
struct RpcDb<'a> {
	rpc: &'a Rpc,
	block: String,
	made_up: BTreeMap<Address, AccountInfo>,
	accounts: BTreeMap<Address, Option<AccountInfo>>,
	storage: BTreeMap<(Address, U256), U256>,
	reads: BTreeMap<Address, BTreeSet<U256>>,
}

impl Database for RpcDb<'_> {
	type Error = Unproven;

	fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Unproven> {
		if let Some(info) = self.made_up.get(&address) {
			return Ok(Some(info.clone()));
		}
		self.reads.entry(address).or_default();
		if let Some(info) = self.accounts.get(&address) {
			return Ok(info.clone());
		}
		let a = format!("{address:#x}");
		let proof = self.rpc.call("eth_getProof", json!([a, [], self.block]));
		let nonce = u64_of(&proof["nonce"]);
		let balance = quantity(&proof["balance"]);
		let code = data(&self.rpc.call("eth_getCode", json!([a, self.block])));
		// An account with no nonce, balance or code is one revm treats as absent (EIP-161).
		let info = (nonce != 0 || !balance.is_zero() || !code.is_empty()).then(|| AccountInfo {
			balance,
			nonce,
			code_hash: keccak256(&code),
			account_id: None,
			code: Some(Bytecode::new_raw(code)),
		});
		self.accounts.insert(address, info.clone());
		Ok(info)
	}

	fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Unproven> {
		if code_hash == KECCAK_EMPTY { Ok(Bytecode::default()) } else { Err(Unproven) }
	}

	fn storage(&mut self, address: Address, index: U256) -> Result<U256, Unproven> {
		if self.made_up.contains_key(&address) {
			return Ok(U256::ZERO);
		}
		self.reads.entry(address).or_default().insert(index);
		if let Some(value) = self.storage.get(&(address, index)) {
			return Ok(*value);
		}
		let slot = format!("{:#x}", B256::from(index));
		let value = quantity(&self.rpc.call("eth_getStorageAt", json!([format!("{address:#x}"), slot, self.block])));
		self.storage.insert((address, index), value);
		Ok(value)
	}

	fn block_hash(&mut self, _number: u64) -> Result<B256, Unproven> {
		Err(Unproven)
	}
}

/// The witness for `address` and its `slots`, from `eth_getProof` at `block`, checked against
/// `state_root` here so a node's mistake shows up now rather than as a losing report.
fn prove(rpc: &Rpc, block: &str, state_root: B256, address: Address, slots: &BTreeSet<U256>) -> AccountWitness {
	let keys: Vec<String> = slots.iter().map(|s| format!("{:#x}", B256::from(*s))).collect();
	let p = rpc.call("eth_getProof", json!([format!("{address:#x}"), keys, block]));
	let proof: Vec<Bytes> = p["accountProof"].as_array().expect("accountProof").iter().map(data).collect();
	let key = Nibbles::unpack(keccak256(address));
	let account = TrieAccount {
		nonce: u64_of(&p["nonce"]),
		balance: quantity(&p["balance"]),
		storage_root: b256(&p["storageHash"]),
		code_hash: b256(&p["codeHash"]),
	};
	let exists = verify_proof(state_root, key, Some(alloy_rlp::encode(&account)), &proof).is_ok();
	let account = if exists {
		account
	} else {
		verify_proof(state_root, key, None, &proof).unwrap_or_else(|e| panic!("{address}: the node's proof fails: {e}"));
		TrieAccount::empty()
	};
	let code = if exists && account.code_hash != KECCAK_EMPTY {
		data(&rpc.call("eth_getCode", json!([format!("{address:#x}"), block])))
	} else {
		Bytes::new()
	};
	assert_eq!(keccak256(&code), account.code_hash, "{address}: code doesn't match its hash");
	let slots = p["storageProof"]
		.as_array()
		.expect("storageProof")
		.iter()
		.map(|s| Slot {
			key: B256::from(quantity(&s["key"])),
			value: quantity(&s["value"]),
			proof: s["proof"].as_array().expect("proof").iter().map(data).collect(),
		})
		.collect();
	AccountWitness { address, exists, account, proof, code, slots }
}

/// Bounty terms on block `n`: its state root, and block N + 1's environment.
pub fn params(rpc: &Rpc, n: u64, spec: u8, capital: U256, invariant: Bytes) -> Params {
	let header = rpc.call("eth_getBlockByNumber", json!([tag(n), false]));
	let next = rpc.call("eth_getBlockByNumber", json!([tag(n + 1), false]));
	assert!(!next.is_null(), "block N + 1 isn't out yet: pick an older N");
	Params {
		state_root: b256(&header["stateRoot"]),
		spec,
		chain_id: u64_of(&rpc.call("eth_chainId", json!([]))),
		number: n + 1,
		timestamp: u64_of(&next["timestamp"]),
		gas_limit: u64_of(&next["gasLimit"]),
		basefee: u64_of(&next["baseFeePerGas"]),
		coinbase: address(&next["miner"]),
		prevrandao: b256(&next["mixHash"]),
		capital,
		invariant,
	}
}

/// What [`build`] found.
pub struct Built {
	pub params: Params,
	pub report: Report,
	/// What the replay on the node's state found, before anything was proven.
	pub expected: Outcome,
}

/// Replay `txs` on block `n`'s state from `rpc`, then prove everything the replay read (and the
/// invariant's runs read), as a report. `extra` adds accounts and slots to prove regardless.
pub fn build(rpc: &Rpc, params: Params, txs: Vec<Tx>, extra: BTreeMap<Address, BTreeSet<U256>>) -> Built {
	let block = tag(params.number - 1);
	let mut db = RpcDb {
		rpc,
		block: block.clone(),
		made_up: made_up(&params).into_iter().collect(),
		accounts: BTreeMap::new(),
		storage: BTreeMap::new(),
		reads: extra,
	};
	let expected = match replay(&params, &mut db, txs.clone(), 0, &mut 0) {
		Ok(outcome) | Err(outcome) => outcome,
	};
	let accounts =
		db.reads.iter().map(|(address, slots)| prove(rpc, &block, params.state_root, *address, slots)).collect();
	Built { report: Report { accounts, txs, stop_after: 0 }, params, expected }
}

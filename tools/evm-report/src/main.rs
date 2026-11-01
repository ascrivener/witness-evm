//! Writes the spike's test cases for the evm-bounty check:
//!
//!     evm-report OUT_DIR
//!
//! For each case, `OUT_DIR/<case>.<p>.args` holds the check's arguments (reveal mode, the report
//! as the witness) with `stop_after` = p, for p in 1..=4, and `<case>.0.args` the whole check, so
//! `witness-sim check` measures each phase by difference. `OUT_DIR/cases.tsv` says what each
//! report holds and how the check went natively.
//!
//!     evm-report demo OUT_DIR
//!
//! writes the demo bounty's params, as hex, to `OUT_DIR/bounty.params`, and its three reports
//! (see `evm_report::pair_bounty`) to `fair.report`, `exploit.report` and `forged.report`.
//!
//!     evm-report args PARAMS REPORT OUT
//!
//! writes the check's arguments for a report to OUT (reveal mode, the report as the witness), for
//! `witness-sim check`: how a protocol given a report privately checks it before it goes on chain.
//! PARAMS is a file holding the bounty's params as hex (as `demo` writes them), REPORT the report.
//!
//!     evm-report lint
//!
//! lints the toy invariants (see `evm_report::lint` and `docs/invariants.md`).
//!
//!     evm-report rpc URL BLOCK INVARIANT TXS OUT_DIR [SPEC]
//!
//! builds a bounty and a report on real chain state (see `evm_report::rpc`): block BLOCK's state,
//! from the JSON-RPC node at URL, and block BLOCK + 1's environment. INVARIANT is a file holding
//! the invariant's runtime code as hex. TXS is a JSON list of transactions, each
//! `{"hunter": 0, "to": "0x…", "data": "0x…", "value": "0x…", "gas": 300000, "create": false}`
//! (`value` and `create` optional); `HUNTER0` to `HUNTER3` in `data` or `to` stand for the
//! hunters' addresses, which derive from the state root. SPEC is the hard fork (`prague` or
//! `osaka`, the default). Writes `bounty.params` (hex), `report` and `summary.tsv` to OUT_DIR.

use evm_bounty_check::{Report, run_measured};
use std::{io::Write, path::PathBuf, process::ExitCode, time::Instant};
use witness_types::{CheckMode, check_args};

fn main() -> ExitCode {
	let args: Vec<String> = std::env::args().skip(1).collect();
	let out = match args.as_slice() {
		[lint] if lint == "lint" => return lint_toys(),
		[out] => out,
		[demo, out] if demo == "demo" => return write_demo(&PathBuf::from(out)),
		[cmd, params, report, out] if cmd == "args" => return write_args(params, report, out),
		[cmd, url, block, invariant, txs, out] if cmd == "rpc" => return write_rpc(url, block, invariant, txs, out, "osaka"),
		[cmd, url, block, invariant, txs, out, spec] if cmd == "rpc" => return write_rpc(url, block, invariant, txs, out, spec),
		_ => {
			eprintln!(
				"usage: evm-report [demo] OUT_DIR | evm-report args PARAMS REPORT OUT | evm-report lint\n       \
				 evm-report rpc URL BLOCK INVARIANT TXS OUT_DIR [SPEC]"
			);
			return ExitCode::FAILURE;
		},
	};
	let out = PathBuf::from(out);
	std::fs::create_dir_all(&out).expect("create OUT_DIR");
	let mut tsv = std::fs::File::create(out.join("cases.tsv")).expect("create cases.tsv");
	let header = "case\toutcome\tevm_gas\ttxs\taccounts\tslots\tproof_nodes\tproof_bytes\tcode_bytes\treport_bytes\tnative_us";
	writeln!(tsv, "{header}").unwrap();
	println!("{header}");
	for case in evm_report::all() {
		let params = alloy_rlp::encode(&case.params);
		let report = alloy_rlp::encode(&case.report);
		// Natively, best of a few runs.
		let mut best = u128::MAX;
		let mut result = None;
		for _ in 0..5 {
			let start = Instant::now();
			result = Some(run_measured(&params, &report));
			best = best.min(start.elapsed().as_micros());
		}
		let (outcome, evm_gas) = result.unwrap();
		if outcome != case.expected {
			eprintln!("{}: the replay found {:?}, the check {:?}", case.name, case.expected, outcome);
			return ExitCode::FAILURE;
		}
		for stop_after in 0..=4 {
			let report = alloy_rlp::encode(&Report { stop_after, ..case.report.clone() });
			let args = check_args(CheckMode::Reveal, &[], &params, &report);
			std::fs::write(out.join(format!("{}.{stop_after}.args", case.name)), args).expect("write args");
		}
		let accounts = &case.report.accounts;
		let slots: usize = accounts.iter().map(|a| a.slots.len()).sum();
		let proofs = accounts.iter().flat_map(|a| std::iter::once(&a.proof).chain(a.slots.iter().map(|s| &s.proof)));
		let (nodes, proof_bytes) = proofs.fold((0, 0), |(n, b), p| (n + p.len(), b + p.iter().map(|x| x.len()).sum::<usize>()));
		let code: usize = accounts.iter().map(|a| a.code.len()).sum();
		let line = format!(
			"{}\t{outcome:?}\t{evm_gas}\t{}\t{}\t{slots}\t{nodes}\t{proof_bytes}\t{code}\t{}\t{best}",
			case.name,
			case.report.txs.len(),
			accounts.len(),
			report.len(),
		);
		writeln!(tsv, "{line}").unwrap();
		println!("{line}");
	}
	ExitCode::SUCCESS
}

fn write_demo(out: &PathBuf) -> ExitCode {
	std::fs::create_dir_all(out).expect("create OUT_DIR");
	let (params, reports) = evm_report::pair_bounty();
	let hex: String = alloy_rlp::encode(&params).iter().map(|b| format!("{b:02x}")).collect();
	std::fs::write(out.join("bounty.params"), format!("0x{hex}\n")).expect("write params");
	for (name, report) in ["fair", "exploit", "forged"].iter().zip(reports) {
		std::fs::write(out.join(format!("{name}.report")), alloy_rlp::encode(&report)).expect("write report");
	}
	ExitCode::SUCCESS
}

fn write_args(params: &str, report: &str, out: &str) -> ExitCode {
	let hex = std::fs::read_to_string(params).expect("read PARAMS");
	let hex = hex.trim().trim_start_matches("0x");
	let params = evm_report::hex_bytes(hex);
	let report = std::fs::read(report).expect("read REPORT");
	std::fs::write(out, check_args(CheckMode::Reveal, &[], &params, &report)).expect("write OUT");
	ExitCode::SUCCESS
}

fn lint_toys() -> ExitCode {
	use evm_report::*;
	let pairs = pairs_world(1, revm::primitives::U256::ZERO);
	let vault = vault_world();
	let invariants = [
		("PairInvariant", &pairs, pair_invariant()),
		("VaultAssetsInvariant", &vault, vault_assets_invariant()),
		("VaultBalanceInvariant", &vault, vault_balance_invariant()),
	];
	for (name, world, code) in invariants {
		let lint = lint::lint(&params_for(world, code), world);
		println!("{name}: holds {:?}, {} gas, ran code at {} accounts", lint.holds, lint.gas_used, lint.code_run.len());
		for finding in &lint.findings {
			println!("  {:?}: {}", finding.severity, finding.message);
		}
	}
	ExitCode::SUCCESS
}

fn write_rpc(url: &str, block: &str, invariant: &str, txs: &str, out: &str, spec: &str) -> ExitCode {
	use evm_bounty_check::{Tx, hunter};
	use evm_report::rpc::{Rpc, build, params};
	use revm::primitives::{Address, Bytes, U256, hardfork::SpecId};
	use std::str::FromStr;

	let spec = match spec {
		"prague" => SpecId::PRAGUE,
		"osaka" => SpecId::OSAKA,
		_ => {
			eprintln!("SPEC: prague or osaka");
			return ExitCode::FAILURE;
		},
	};
	let block: u64 = match block.strip_prefix("0x") {
		Some(hex) => u64::from_str_radix(hex, 16).expect("BLOCK"),
		None => block.parse().expect("BLOCK"),
	};
	let invariant = std::fs::read_to_string(invariant).expect("read INVARIANT");
	let invariant = evm_report::hex_bytes(invariant.trim().trim_start_matches("0x"));
	let rpc = Rpc::new(url);
	let start = Instant::now();
	let params = params(&rpc, block, spec as u8, U256::from(evm_report::CAPITAL), invariant);

	// Hunters' addresses derive from the state root, so substitute them now.
	let txs: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(txs).expect("read TXS")).expect("TXS: JSON");
	let substitute = |s: &str| {
		(0..evm_bounty_check::HUNTERS).fold(s.to_string(), |s, i| {
			s.replace(&format!("HUNTER{i}"), &format!("{:x}", hunter(params.state_root, i)))
		})
	};
	let txs: Vec<Tx> = txs
		.as_array()
		.expect("TXS: a list")
		.iter()
		.map(|t| {
			let field = |k: &str| t.get(k).and_then(|v| v.as_str()).map(&substitute);
			let hex = |s: String| if s.starts_with("0x") { s } else { format!("0x{s}") };
			Tx {
				hunter: t["hunter"].as_u64().expect("hunter") as u8,
				create: t.get("create").and_then(|c| c.as_bool()).unwrap_or(false),
				to: field("to").filter(|a| !a.is_empty()).map(|a| Address::from_str(&hex(a)).expect("to")).unwrap_or_default(),
				value: field("value").map(|v| U256::from_str(&v).expect("value")).unwrap_or_default(),
				data: field("data").map(|d| Bytes::from_str(&hex(d)).expect("data")).unwrap_or_default(),
				gas_limit: t["gas"].as_u64().expect("gas"),
			}
		})
		.collect();

	let built = build(&rpc, params, txs, Default::default());
	let fetched = start.elapsed();
	let params = alloy_rlp::encode(&built.params);
	let report = alloy_rlp::encode(&built.report);
	let start = Instant::now();
	let (outcome, evm_gas) = run_measured(&params, &report);
	let native_us = start.elapsed().as_micros();

	let out = PathBuf::from(out);
	std::fs::create_dir_all(&out).expect("create OUT_DIR");
	let hex: String = params.iter().map(|b| format!("{b:02x}")).collect();
	std::fs::write(out.join("bounty.params"), format!("0x{hex}\n")).expect("write params");
	std::fs::write(out.join("report"), &report).expect("write report");

	let accounts = &built.report.accounts;
	let slots: usize = accounts.iter().map(|a| a.slots.len()).sum();
	let proofs = accounts.iter().flat_map(|a| std::iter::once(&a.proof).chain(a.slots.iter().map(|s| &s.proof)));
	let (nodes, proof_bytes) = proofs.fold((0, 0), |(n, b), p| (n + p.len(), b + p.iter().map(|x| x.len()).sum::<usize>()));
	let code: usize = accounts.iter().map(|a| a.code.len()).sum();
	let header = "block\tstate_root\treplay\tcheck\tevm_gas\ttxs\taccounts\tslots\tproof_nodes\tproof_bytes\tcode_bytes\treport_bytes\tnative_us\trpc_calls\tfetch_s";
	let line = format!(
		"{block}\t{}\t{:?}\t{outcome:?}\t{evm_gas}\t{}\t{}\t{slots}\t{nodes}\t{proof_bytes}\t{code}\t{}\t{native_us}\t{}\t{:.1}",
		built.params.state_root,
		built.expected,
		built.report.txs.len(),
		accounts.len(),
		report.len(),
		rpc.calls.get(),
		fetched.as_secs_f64(),
	);
	std::fs::write(out.join("summary.tsv"), format!("{header}\n{line}\n")).expect("write summary");
	println!("{header}\n{line}");
	if outcome != built.expected {
		eprintln!("the replay on the node's state found {:?}, the check on the proven state {outcome:?}", built.expected);
		return ExitCode::FAILURE;
	}
	ExitCode::SUCCESS
}

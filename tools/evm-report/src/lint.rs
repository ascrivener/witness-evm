//! A lint for bounty invariants: run the invariant once on block N's state, as the check does,
//! and report what a maker should fix or look at before posting. See `docs/invariants.md`.
//!
//! It sees one run, on the state before any report, so it finds patterns, not proofs: a clean
//! lint means none of these were on the path the invariant took at block N.

use crate::{Reads, Recorder, World};
use evm_bounty_check::{INVARIANT_GAS, Params, context, invariant_call, made_up};
use revm::{
	Database, InspectEvm, Inspector, MainBuilder,
	context::{
		ContextTr,
		result::{ExecutionResult, Output},
	},
	interpreter::{
		CallInputs, CallOutcome, CreateInputs, CreateOutcome, Interpreter, interpreter_types::Jumps,
	},
	primitives::{Address, U256},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
	/// The invariant is wrong as it stands: don't post it.
	Error,
	/// It works, but something about it may make a bounty miss or misfire.
	Warning,
	/// Something only the maker can judge: look at it and say why it's fine.
	Review,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
	pub severity: Severity,
	pub message: String,
}

/// What the lint found.
#[derive(Debug)]
pub struct Lint {
	/// Whether the invariant held at block N; `None` if it didn't return one word.
	pub holds: Option<bool>,
	pub gas_used: u64,
	/// Every account whose code the invariant ran, in order of first call.
	pub code_run: Vec<Address>,
	pub findings: Vec<Finding>,
}

impl Lint {
	/// No errors or warnings: only, perhaps, things to review.
	pub fn passes(&self) -> bool {
		self.findings.iter().all(|f| f.severity == Severity::Review)
	}
}

/// Opcodes that read the block or transaction a report runs in. In an invariant they make it
/// depend on what the bounty's params say about the block, not on the protocol's state.
const ENVIRONMENT: [(u8, &str); 11] = [
	(0x32, "ORIGIN"),
	(0x33, "CALLER"),
	(0x3a, "GASPRICE"),
	(0x40, "BLOCKHASH"),
	(0x41, "COINBASE"),
	(0x42, "TIMESTAMP"),
	(0x43, "NUMBER"),
	(0x44, "PREVRANDAO"),
	(0x45, "GASLIMIT"),
	(0x48, "BASEFEE"),
	(0x4a, "BLOBBASEFEE"),
];

const SLOAD: u8 = 0x54;

/// A frame of the invariant's run: whose code, and the slots it read, with their values.
struct Frame {
	address: Address,
	sloads: Vec<(U256, U256)>,
}

#[derive(Default)]
struct Watcher {
	frames: Vec<Frame>,
	pending_sload: Option<U256>,
	code_run: Vec<Address>,
	findings: Vec<Finding>,
}

impl Watcher {
	fn find(&mut self, severity: Severity, message: String) {
		let finding = Finding { severity, message };
		if !self.findings.contains(&finding) {
			self.findings.push(finding);
		}
	}
}

/// Where a slot is: a small number is a fixed variable; a hash is an entry in an array or mapping.
fn describe(slot: U256) -> String {
	if slot < U256::from(1u64 << 32) { format!("its slot {slot}") } else { format!("an array or mapping entry (slot {slot:#x})") }
}

impl<CTX: ContextTr> Inspector<CTX> for Watcher {
	fn step(&mut self, interp: &mut Interpreter, _: &mut CTX) {
		let opcode = interp.bytecode.opcode();
		let depth = self.frames.len();
		if opcode == SLOAD {
			self.pending_sload = interp.stack.peek(0).ok();
		}
		if let Some((_, name)) = ENVIRONMENT.iter().find(|(op, _)| *op == opcode) {
			// The invariant's own code mustn't read the environment at all; anyone's may read
			// `ORIGIN`, which marks code that answers differently depending on who sent the
			// transaction, and so can tell when the invariant is asking.
			let address = self.frames.last().map(|f| f.address).unwrap_or_default();
			if depth == 1 {
				self.find(Severity::Error, format!("the invariant reads {name}: it should depend only on state"));
			} else if opcode == 0x32 {
				self.find(
					Severity::Warning,
					format!("{address} reads ORIGIN while the invariant runs: it can answer the invariant differently"),
				);
			}
		}
	}

	fn step_end(&mut self, interp: &mut Interpreter, _: &mut CTX) {
		if let (Some(slot), Ok(value)) = (self.pending_sload.take(), interp.stack.peek(0)) {
			if let Some(frame) = self.frames.last_mut() {
				frame.sloads.push((slot, value));
			}
		}
	}

	fn call(&mut self, _: &mut CTX, inputs: &mut CallInputs) -> Option<CallOutcome> {
		let code = inputs.bytecode_address;
		if let Some(caller) = self.frames.last() {
			let chosen = caller.sloads.iter().find(|(_, value)| *value == U256::from_be_slice(code.as_slice()));
			if let Some(&(slot, _)) = chosen {
				let caller = caller.address;
				self.find(
					Severity::Review,
					format!(
						"{caller} runs code at {code}, an address it read from {}: if anyone can change that, anyone can put their own code in the invariant's path",
						describe(slot)
					),
				);
			}
		}
		if !self.code_run.contains(&code) {
			self.code_run.push(code);
		}
		self.frames.push(Frame { address: inputs.target_address, sloads: Vec::new() });
		None
	}

	fn call_end(&mut self, _: &mut CTX, _: &CallInputs, _: &mut CallOutcome) {
		self.frames.pop();
	}

	fn create(&mut self, _: &mut CTX, _: &mut CreateInputs) -> Option<CreateOutcome> {
		self.find(Severity::Error, "the invariant deploys a contract: it should only read".into());
		None
	}
}

/// Lint `params.invariant` on `db`, which must answer for the accounts [`made_up`] makes up.
pub fn lint_on<DB: Database>(params: &Params, db: DB) -> Lint {
	let Ok(ctx) = context(params, db) else {
		let findings = vec![Finding { severity: Severity::Error, message: "the params name no known fork".into() }];
		return Lint { holds: None, gas_used: 0, code_run: Vec::new(), findings };
	};
	let mut evm = ctx.build_mainnet_with_inspector(Watcher::default());
	let result = evm.inspect_one_tx(invariant_call(params));
	let mut watcher = std::mem::take(&mut evm.inspector);
	let (holds, gas_used) = match &result {
		Ok(ExecutionResult::Success { output: Output::Call(word), .. }) if word.len() == 32 =>
			(Some(word.iter().any(|&b| b != 0)), result.as_ref().unwrap().tx_gas_used()),
		Ok(other) => (None, other.tx_gas_used()),
		Err(_) => (None, 0),
	};
	match holds {
		Some(true) => {},
		Some(false) => watcher.find(Severity::Error, "the invariant doesn't hold at block N: no report can win".into()),
		None => watcher.find(
			Severity::Error,
			"the invariant reverted, ran out of gas or didn't return one word at block N".into(),
		),
	}
	if gas_used > INVARIANT_GAS / 4 {
		watcher.find(
			Severity::Warning,
			format!("the invariant used {gas_used} of its {INVARIANT_GAS} gas: a report that grows what it reads can make it run out, and lose"),
		);
	}
	watcher.findings.sort_by_key(|f| f.severity);
	Lint { holds, gas_used, code_run: watcher.code_run, findings: watcher.findings }
}

/// Lint `params.invariant` on a synthetic world.
pub fn lint(params: &Params, world: &World) -> Lint {
	let recorder = Recorder { world, made_up: made_up(params).into_iter().collect(), reads: Reads::default() };
	lint_on(params, recorder)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{STRATEGY, hex_bytes, pair_invariant, pairs_world, params_for, token, vault_assets_invariant, vault_balance_invariant, vault_world};

	fn messages(lint: &Lint, severity: Severity) -> Vec<&str> {
		lint.findings.iter().filter(|f| f.severity == severity).map(|f| f.message.as_str()).collect()
	}

	#[test]
	fn the_pair_invariant_is_clean() {
		let world = pairs_world(1, U256::ZERO);
		let lint = lint(&params_for(&world, pair_invariant()), &world);
		assert_eq!(lint.holds, Some(true));
		assert!(lint.findings.is_empty(), "{:?}", lint.findings);
		assert_eq!(lint.code_run.len(), 2, "the invariant and the pair");
	}

	#[test]
	fn asking_the_vault_means_trusting_its_strategies() {
		let world = vault_world();
		let lint = lint(&params_for(&world, vault_assets_invariant()), &world);
		assert!(lint.passes(), "{:?}", lint.findings);
		let review = messages(&lint, Severity::Review);
		// The vault calls the strategy it read from its list (anyone can add to it), and the
		// token it read from slot 0 (set once): both flagged, the list entry as such.
		assert!(review.iter().any(|m| m.contains(&STRATEGY.to_string()) && m.contains("array or mapping entry")), "{review:?}");
		assert!(review.iter().any(|m| m.contains(&token(0).to_string()) && m.contains("its slot 0")), "{review:?}");
	}

	#[test]
	fn reading_the_token_trusts_no_one_else() {
		let world = vault_world();
		let lint = lint(&params_for(&world, vault_balance_invariant()), &world);
		assert_eq!(lint.holds, Some(true));
		assert!(lint.findings.is_empty(), "{:?}", lint.findings);
		assert!(!lint.code_run.contains(&STRATEGY));
	}

	#[test]
	fn an_invariant_that_reads_the_environment_is_an_error() {
		// Return TIMESTAMP: nonzero, so it holds, but on the block's word, not the state's.
		let world = vault_world();
		let lint = lint(&params_for(&world, hex_bytes("4260005260206000f3")), &world);
		assert_eq!(lint.holds, Some(true));
		assert!(messages(&lint, Severity::Error).iter().any(|m| m.contains("TIMESTAMP")), "{:?}", lint.findings);
	}

	#[test]
	fn an_invariant_that_does_not_hold_is_an_error() {
		let world = vault_world();
		let lint = lint(&params_for(&world, hex_bytes("60205ff3")), &world);
		assert_eq!(lint.holds, Some(false));
		assert!(!lint.passes());
	}
}

//! Genesis presets: `development` and `local_testnet` for running locally, and `wika_testnet` /
//! `wika_mainnet` with the authority and sudo keys of the original 2021 networks.

use crate::{AccountId, BalancesConfig, RuntimeGenesisConfig, SudoConfig, UNIT};
use alloc::{vec, vec::Vec};
use frame_support::build_struct_json_patch;
use serde_json::Value;
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
use sp_consensus_grandpa::AuthorityId as GrandpaId;
use sp_core::{ed25519, sr25519};
use sp_genesis_builder::{self, PresetId};
use sp_keyring::{Ed25519Keyring, Sr25519Keyring};

pub const WIKA_TESTNET_PRESET: &str = "wika_testnet";
pub const WIKA_MAINNET_PRESET: &str = "wika_mainnet";

/// (sr25519, ed25519) public keys of a node.
type NodeKeys = ([u8; 32], [u8; 32]);

fn wika_genesis(
	nodes: Vec<NodeKeys>,
	mut endowed: Vec<(AccountId, u128)>,
	root: AccountId,
	verifiers: Vec<AccountId>,
) -> Value {
	// Operating reserve is separate from author liabilities on a fresh chain.
	endowed.push((pallet_owners::Pallet::<crate::Runtime>::escrow_id(), UNIT));
	build_struct_json_patch!(RuntimeGenesisConfig {
		balances: BalancesConfig { balances: endowed },
		aura: pallet_aura::GenesisConfig {
			authorities: nodes
				.iter()
				.map(|(sr, _)| AuraId::from(sr25519::Public::from_raw(*sr)))
				.collect::<Vec<_>>(),
		},
		grandpa: pallet_grandpa::GenesisConfig {
			authorities: nodes
				.iter()
				.map(|(_, ed)| (GrandpaId::from(ed25519::Public::from_raw(*ed)), 1))
				.collect::<Vec<_>>(),
		},
		sudo: SudoConfig { key: Some(root) },
		authorities: pallet_authorities::GenesisConfig { keys: nodes },
		owners: pallet_owners::GenesisConfig { verifiers },
	})
}

fn dev_node(seed: Sr25519Keyring, ed: Ed25519Keyring) -> NodeKeys {
	(seed.public().0, ed.public().0)
}

/// `//Alice//ownr`: the verifier key a `--dev` or `--alice` node puts in its keystore. It is a
/// separate account from Alice so the offchain worker's commits and reveals never compete with
/// Alice's own transactions for nonces.
pub fn dev_verifier() -> AccountId {
	AccountId::from(hex(
		"ea6f32f09e566804971c3ec4b05f404be58f1f6a1ad8f8dcd30381ad5312c20e",
	))
}

fn dev_endowed() -> Vec<(AccountId, u128)> {
	Sr25519Keyring::iter()
		.map(|k| k.to_account_id())
		.chain([dev_verifier()])
		.map(|a| (a, 1u128 << 60))
		.collect()
}

/// Alice authors blocks and `//Alice//ownr` verifies URL ownership; the dev accounts are funded.
pub fn development_config_genesis() -> Value {
	wika_genesis(
		vec![dev_node(Sr25519Keyring::Alice, Ed25519Keyring::Alice)],
		dev_endowed(),
		Sr25519Keyring::Alice.to_account_id(),
		vec![dev_verifier()],
	)
}

/// Alice and Bob author blocks; `//Alice//ownr` verifies URL ownership.
pub fn local_config_genesis() -> Value {
	wika_genesis(
		vec![
			dev_node(Sr25519Keyring::Alice, Ed25519Keyring::Alice),
			dev_node(Sr25519Keyring::Bob, Ed25519Keyring::Bob),
		],
		dev_endowed(),
		Sr25519Keyring::Alice.to_account_id(),
		vec![dev_verifier()],
	)
}

fn original_nodes() -> Vec<NodeKeys> {
	vec![
		(
			hex("3277d22306435a4800ebc611216d301cb79d2166b7519a6bdf58b0b6fb523267"),
			hex("2bdb6dab53650339faa80a06fde1b27ea46cee14ee0523bfc43d6c37d64f933d"),
		),
		(
			hex("d021327f24e3ee188449b9264cb401f94caea467656a505fae1fdb0f6eda523b"),
			hex("ced4ef1df5f05aabc0ae1f7d5ee499edf765291aa4f5efc0f0b074e059e0ef52"),
		),
	]
}

fn original_network(root: &str) -> Value {
	let nodes = original_nodes();
	let endowed = nodes
		.iter()
		.map(|(sr, _)| (AccountId::from(*sr), 1_000 * UNIT))
		.collect();
	wika_genesis(nodes, endowed, AccountId::from(hex(root)), vec![])
}

fn hex(s: &str) -> [u8; 32] {
	let mut out = [0u8; 32];
	for (i, byte) in out.iter_mut().enumerate() {
		*byte = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).expect("valid hex constant");
	}
	out
}

/// Provides the JSON representation of predefined genesis config for given `id`.
pub fn get_preset(id: &PresetId) -> Option<Vec<u8>> {
	let patch = match id.as_ref() {
		sp_genesis_builder::DEV_RUNTIME_PRESET => development_config_genesis(),
		sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET => local_config_genesis(),
		WIKA_TESTNET_PRESET => {
			original_network("56442d2ddbace4927f284c799c0e706ce7e0df06f395e2c1bbfb967ece5cf053")
		}
		WIKA_MAINNET_PRESET => {
			original_network("a47f12db28111cfc0052494f434f4bf2cd316dcd13852d6f64e538d861c23534")
		}
		_ => return None,
	};
	Some(
		serde_json::to_string(&patch)
			.expect("serialization to json is expected to work. qed.")
			.into_bytes(),
	)
}

/// List of supported presets.
pub fn preset_names() -> Vec<PresetId> {
	vec![
		PresetId::from(sp_genesis_builder::DEV_RUNTIME_PRESET),
		PresetId::from(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET),
		PresetId::from(WIKA_TESTNET_PRESET),
		PresetId::from(WIKA_MAINNET_PRESET),
	]
}

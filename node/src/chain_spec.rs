use sc_service::{ChainType, Properties};
use wika_runtime::{
	genesis_config_presets::{WIKA_MAINNET_PRESET, WIKA_TESTNET_PRESET},
	WASM_BINARY,
};

/// Specialized `ChainSpec`. This is a specialization of the general Substrate ChainSpec type.
pub type ChainSpec = sc_service::GenericChainSpec;

/// Token metadata wallets read from `system_properties`.
fn properties() -> Properties {
	let mut p = Properties::new();
	p.insert("tokenSymbol".into(), "WIKA".into());
	p.insert("tokenDecimals".into(), 12.into());
	p.insert("ss58Format".into(), 42.into());
	p
}

fn builder(name: &str, id: &str, chain_type: ChainType, preset: &str) -> Result<ChainSpec, String> {
	Ok(ChainSpec::builder(WASM_BINARY.ok_or_else(|| "Wasm runtime not available".to_string())?, None)
		.with_name(name)
		.with_id(id)
		.with_chain_type(chain_type)
		.with_protocol_id("wika")
		.with_properties(properties())
		.with_genesis_config_preset_name(preset)
		.build())
}

pub fn development_chain_spec() -> Result<ChainSpec, String> {
	builder(
		"Wika Development",
		"dev",
		ChainType::Development,
		sp_genesis_builder::DEV_RUNTIME_PRESET,
	)
}

pub fn local_chain_spec() -> Result<ChainSpec, String> {
	builder(
		"Wika Local Testnet",
		"local_testnet",
		ChainType::Local,
		sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET,
	)
}

/// A fresh testnet with the original testnet authorities and sudo key.
pub fn testnet_chain_spec() -> Result<ChainSpec, String> {
	builder("Wika TestNet", "test", ChainType::Live, WIKA_TESTNET_PRESET)
}

/// A fresh mainnet with the original mainnet authorities and sudo key.
pub fn mainnet_chain_spec() -> Result<ChainSpec, String> {
	builder("Wika MainNet", "main", ChainType::Live, WIKA_MAINNET_PRESET)
}

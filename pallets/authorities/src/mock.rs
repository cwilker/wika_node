use crate as pallet_authorities;
use frame_support::{
	derive_impl,
	traits::{ConstBool, ConstU32, ConstU64},
};
use sp_runtime::BuildStorage;

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
	pub enum Test {
		System: frame_system,
		Timestamp: pallet_timestamp,
		Aura: pallet_aura,
		Grandpa: pallet_grandpa,
		Authorities: pallet_authorities,
	}
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
	type Block = Block;
}

impl pallet_timestamp::Config for Test {
	type Moment = u64;
	type OnTimestampSet = Aura;
	type MinimumPeriod = ConstU64<3000>;
	type WeightInfo = ();
}

impl pallet_aura::Config for Test {
	type AuthorityId = sp_consensus_aura::sr25519::AuthorityId;
	type DisabledValidators = ();
	type MaxAuthorities = ConstU32<4>;
	type AllowMultipleBlocksPerSlot = ConstBool<false>;
	type SlotDuration = pallet_aura::MinimumPeriodTimesTwo<Test>;
}

impl pallet_grandpa::Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = ();
	type MaxAuthorities = ConstU32<4>;
	type MaxNominators = ConstU32<0>;
	type MaxSetIdSessionEntries = ConstU64<0>;
	type KeyOwnerProof = sp_core::Void;
	type EquivocationReportSystem = ();
}

impl pallet_authorities::Config for Test {}

pub fn keys(n: u8) -> ([u8; 32], [u8; 32]) {
	([n; 32], [n + 100; 32])
}

pub fn account(n: u8) -> u64 {
	u64::from_le_bytes([n; 8])
}

pub fn new_test_ext() -> sp_io::TestExternalities {
	let mut t = frame_system::GenesisConfig::<Test>::default().build_storage().unwrap();
	pallet_aura::GenesisConfig::<Test> {
		authorities: vec![sp_core::sr25519::Public::from_raw(keys(1).0).into()],
	}
	.assimilate_storage(&mut t)
	.unwrap();
	pallet_grandpa::GenesisConfig::<Test> {
		authorities: vec![(sp_core::ed25519::Public::from_raw(keys(1).1).into(), 1)],
		..Default::default()
	}
	.assimilate_storage(&mut t)
	.unwrap();
	pallet_authorities::GenesisConfig::<Test> { keys: vec![keys(1)], ..Default::default() }
		.assimilate_storage(&mut t)
		.unwrap();
	let mut ext = sp_io::TestExternalities::new(t);
	ext.execute_with(|| System::set_block_number(1));
	ext
}

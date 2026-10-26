use crate as pallet_likes;
use core::cell::RefCell;
use frame_support::{derive_impl, parameter_types};
use sp_runtime::BuildStorage;
use std::collections::BTreeMap;

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
	pub enum Test {
		System: frame_system,
		Balances: pallet_balances,
		Likes: pallet_likes,
	}
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
	type Block = Block;
	type AccountData = pallet_balances::AccountData<u64>;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
	type AccountStore = System;
}

parameter_types! {
	pub const MaxLengthURL: u32 = 255;
}

pub const OWNERS_POT: u64 = 999;

thread_local! {
	static OWNERS: RefCell<BTreeMap<Vec<u8>, u64>> = RefCell::new(BTreeMap::new());
}

pub struct MockOwners;
impl MockOwners {
	pub fn set(url: &[u8], owner: u64) {
		OWNERS.with(|o| o.borrow_mut().insert(url.to_vec(), owner));
	}
}
impl wika_traits::OwnershipRegistry<u64> for MockOwners {
	fn pot_id() -> u64 {
		OWNERS_POT
	}
	fn owner_of(url: &[u8]) -> u64 {
		OWNERS.with(|o| o.borrow().get(url).copied().unwrap_or(OWNERS_POT))
	}
}

impl pallet_likes::Config for Test {
	type Currency = Balances;
	type MaxLengthURL = MaxLengthURL;
	type OwnershipRegistry = MockOwners;
}

pub const UNIT: u64 = 1_000_000_000_000;
pub const ALICE: u64 = 1;
pub const BOB: u64 = 2;
pub const CHARLIE: u64 = 3;
pub const DAVE: u64 = 4;
pub const AUTHOR: u64 = 10;
pub const POOR: u64 = 20;

pub fn new_test_ext() -> sp_io::TestExternalities {
	OWNERS.with(|o| o.borrow_mut().clear());
	let mut t = frame_system::GenesisConfig::<Test>::default().build_storage().unwrap();
	pallet_balances::GenesisConfig::<Test> {
		balances: vec![
			(ALICE, 1_000 * UNIT),
			(BOB, 1_000 * UNIT),
			(CHARLIE, 1_000 * UNIT),
			(DAVE, 1_000 * UNIT),
			(AUTHOR, UNIT),
			(OWNERS_POT, UNIT),
			(POOR, UNIT / 2),
		],
		..Default::default()
	}
	.assimilate_storage(&mut t)
	.unwrap();
	let mut ext = sp_io::TestExternalities::new(t);
	ext.execute_with(|| System::set_block_number(1));
	ext
}

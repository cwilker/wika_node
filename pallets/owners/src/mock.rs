use crate as pallet_owners;
use frame_support::{derive_impl, parameter_types, traits::ConstU32};
use sp_core::{
	offchain::{testing, OffchainDbExt, OffchainWorkerExt, TransactionPoolExt},
	sr25519, Pair,
};
use sp_keystore::{testing::MemoryKeystore, Keystore, KeystoreExt};
use sp_runtime::{
	testing::TestXt,
	traits::{IdentityLookup, Verify},
	BuildStorage,
};
use std::sync::Arc;

pub type Extrinsic = TestXt<RuntimeCall, ()>;
type Block = sp_runtime::generic::Block<
	sp_runtime::generic::Header<u64, sp_runtime::traits::BlakeTwo256>,
	Extrinsic,
>;
pub type AccountId = sr25519::Public;

frame_support::construct_runtime!(
	pub enum Test {
		System: frame_system,
		Balances: pallet_balances,
		Owners: pallet_owners,
		Likes: pallet_likes,
	}
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
	type Block = Block;
	type AccountId = AccountId;
	type Lookup = IdentityLookup<AccountId>;
	type AccountData = pallet_balances::AccountData<u64>;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
	type AccountStore = System;
	type ExistentialDeposit = ExistentialDeposit;
}

impl frame_system::offchain::SigningTypes for Test {
	type Public = <sr25519::Signature as Verify>::Signer;
	type Signature = sr25519::Signature;
}

impl<C> frame_system::offchain::CreateTransactionBase<C> for Test
where
	RuntimeCall: From<C>,
{
	type RuntimeCall = RuntimeCall;
	type Extrinsic = Extrinsic;
}

impl<C> frame_system::offchain::CreateTransaction<C> for Test
where
	RuntimeCall: From<C>,
{
	type Extension = ();
	fn create_transaction(call: RuntimeCall, _extension: ()) -> Extrinsic {
		Extrinsic::new_bare(call)
	}
}

impl<C> frame_system::offchain::CreateSignedTransaction<C> for Test
where
	RuntimeCall: From<C>,
{
	fn create_signed_transaction<
		A: frame_system::offchain::AppCrypto<Self::Public, Self::Signature>,
	>(
		call: RuntimeCall,
		_public: Self::Public,
		_account: AccountId,
		nonce: u32,
	) -> Option<Extrinsic> {
		Some(Extrinsic::new_signed(call, nonce.into(), (), ()))
	}
}

parameter_types! {
	pub const MaxLengthURL: u32 = 255;
	pub static Now: u64 = 1_000;
	pub const ExistentialDeposit: u64 = 10;
	pub static LegacyClaims: bool = true;
}

pub struct TestClock;
impl frame_support::traits::UnixTime for TestClock {
	fn now() -> core::time::Duration {
		core::time::Duration::from_millis(Now::get())
	}
}
impl pallet_likes::Config for Test {
	type Clock = TestClock;
	type ReferralActivation = frame_support::traits::ConstU64<1000000>;
	type AllowLocalReferralHosts = frame_support::traits::ConstBool<true>;
	type Currency = Balances;
	type MaxLengthURL = MaxLengthURL;
	type OwnershipRegistry = Owners;
}
impl pallet_owners::Config for Test {
	type PageEconomics = Likes;
	type RuntimeHoldReason = RuntimeHoldReason;
	type Clock = TestClock;
	type AllowLegacyClaims = LegacyClaims;
	type AuthorityId = pallet_owners::crypto::OwnersAppCrypto;
	type Currency = Balances;
	type MaxLengthURL = MaxLengthURL;
	type MaxVerifiers = ConstU32<16>;
	type MaxRequestsPerBlock = ConstU32<4>;
}

pub const UNIT: u64 = 1_000_000_000_000;

pub fn pair(seed: &str) -> sr25519::Pair {
	sr25519::Pair::from_string(&format!("//{seed}"), None).unwrap()
}

pub fn account(seed: &str) -> AccountId {
	pair(seed).public()
}

pub struct Env {
	pub ext: sp_io::TestExternalities,
	pub offchain: Arc<parking_lot::RwLock<testing::OffchainState>>,
	pub pool: Arc<parking_lot::RwLock<testing::PoolState>>,
}

/// Alice is an enabled verifier whose `ownr` key is in the keystore; Bob and Poor are users.
pub fn new_test_ext() -> Env {
	Now::set(1_000);
	LegacyClaims::set(true);
	let mut t = frame_system::GenesisConfig::<Test>::default()
		.build_storage()
		.unwrap();
	pallet_balances::GenesisConfig::<Test> {
		balances: vec![
			(account("Alice"), 1_000 * UNIT),
			(account("Bob"), 1_000 * UNIT),
			(account("Charlie"), 1_000 * UNIT),
			(account("Dave"), 1_000 * UNIT),
			(account("Poor"), UNIT),
			(Owners::pot_id(), UNIT),
			(Owners::escrow_id(), UNIT),
		],
		..Default::default()
	}
	.assimilate_storage(&mut t)
	.unwrap();
	pallet_owners::GenesisConfig::<Test> {
		verifiers: vec![account("Alice")],
	}
	.assimilate_storage(&mut t)
	.unwrap();

	let (offchain, offchain_state) = testing::TestOffchainExt::new();
	let (pool, pool_state) = testing::TestTransactionPoolExt::new();
	let keystore = MemoryKeystore::new();
	keystore
		.sr25519_generate_new(pallet_owners::KEY_TYPE, Some("//Alice"))
		.unwrap();

	let mut ext = sp_io::TestExternalities::new(t);
	ext.register_extension(OffchainWorkerExt::new(offchain.clone()));
	ext.register_extension(OffchainDbExt::new(offchain));
	ext.register_extension(TransactionPoolExt::new(pool));
	ext.register_extension(KeystoreExt::new(keystore));
	ext.execute_with(|| System::set_block_number(1));
	Env {
		ext,
		offchain: offchain_state,
		pool: pool_state,
	}
}

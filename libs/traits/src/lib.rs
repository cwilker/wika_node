#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod page_identity;

/// Who owns a URL. Implemented by the owners pallet and used by the likes
/// pallet to escrow author allocations and pay referrers.
pub trait OwnershipRegistry<AccountId, Balance> {
	/// Fee/treasury pot and fallback for unclaimed referrers. Author rewards use separate escrow.
	fn pot_id() -> AccountId;

	/// The registered owner of `url`, or [`Self::pot_id`] when there is none.
	fn owner_of(url: &[u8]) -> AccountId;

	/// Current verified ownership period eligible to authorize referral hosts.
	fn referral_owner(url: &[u8]) -> Option<(AccountId, u64)>;

	/// Active author entitlement period, independent of host-verification freshness.
	fn ownership_period(url: &[u8]) -> Option<u64> {
		Self::referral_owner(url).map(|(_, p)| p)
	}

	/// Transfer and credit the author allocation atomically; never create an unfunded credit.
	fn pay_author(
		sender: &AccountId,
		url: &[u8],
		amount: Balance,
	) -> Result<(), sp_runtime::DispatchError>;
}

/// Economic authorization is separate from publishing proof. Integer basis points only.
#[derive(
	codec::Encode,
	codec::Decode,
	codec::DecodeWithMemTracking,
	codec::MaxEncodedLen,
	scale_info::TypeInfo,
	Clone,
	PartialEq,
	Eq,
	Debug,
)]
pub struct PolicySelection {
	pub owner_bps: u16,
	pub supporter_bps: u16,
	pub referral_bps: u16,
	pub ruleset: u32,
	pub expected_revision: u64,
	pub expected_hash: [u8; 32],
	pub inherit_if_changed: bool,
}
/// Runtime wiring avoids a circular ownership/likes dependency.
#[derive(
	codec::Encode,
	codec::Decode,
	codec::DecodeWithMemTracking,
	codec::MaxEncodedLen,
	scale_info::TypeInfo,
	Clone,
	Copy,
	PartialEq,
	Eq,
	Debug,
)]
pub enum PolicyOutcomeReason {
	Applied,
	Locked,
	TermsChanged,
}
pub trait PageEconomics<AccountId> {
	fn validate_claim(
		page: &[u8],
		selection: &PolicySelection,
	) -> Result<(), sp_runtime::DispatchError>;
	fn activate_claim(
		page: &[u8],
		owner: &AccountId,
		period: u64,
		selection: &PolicySelection,
	) -> Result<PolicyOutcomeReason, sp_runtime::DispatchError>;
}
impl<AccountId> PageEconomics<AccountId> for () {
	fn validate_claim(_: &[u8], _: &PolicySelection) -> Result<(), sp_runtime::DispatchError> {
		Err(sp_runtime::DispatchError::Other("Economics unavailable"))
	}
	fn activate_claim(
		_: &[u8],
		_: &AccountId,
		_: u64,
		_: &PolicySelection,
	) -> Result<PolicyOutcomeReason, sp_runtime::DispatchError> {
		Ok(PolicyOutcomeReason::TermsChanged)
	}
}

#![cfg_attr(not(feature = "std"), no_std)]

/// Who owns a URL. Implemented by the owners pallet and used by the likes
/// pallet to pay authors and referrers.
pub trait OwnershipRegistry<AccountId> {
	/// The account that receives payments for URLs nobody has claimed.
	fn pot_id() -> AccountId;

	/// The registered owner of `url`, or [`Self::pot_id`] when there is none.
	fn owner_of(url: &[u8]) -> AccountId;
}

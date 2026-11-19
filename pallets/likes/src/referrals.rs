//! Referral protocol v1. Economic terms are resolved by the runtime, never by the host.
use super::*;
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use frame_support::{
	ensure,
	traits::{ConstU32, Get, UnixTime},
	BoundedVec,
};
use scale_info::TypeInfo;
use sp_runtime::{traits::Zero, DispatchError};
use wika_traits::OwnershipRegistry;

pub const DOMAIN: [u8; 18] = *b"WIKA_HOST_REFERRAL";
pub type Page = BoundedVec<u8, ConstU32<254>>;
#[derive(
	Encode, Decode, DecodeWithMemTracking, Clone, PartialEq, Eq, Debug, TypeInfo, MaxEncodedLen,
)]
pub struct ReferralPayload<AccountId> {
	pub domain: [u8; 18],
	pub version: u16,
	pub genesis_hash: [u8; 32],
	pub source_page: Page,
	pub source_period: u64,
	pub authorization_id: u64,
	pub target_page: Page,
	pub payer: AccountId,
	pub count: u32,
	pub request_id: [u8; 32],
	pub issued_at: u64,
	pub expires_at: u64,
	pub terms_version: u16,
	pub terms_hash: [u8; 32],
}
#[derive(
	Encode, Decode, DecodeWithMemTracking, Clone, PartialEq, Eq, Debug, TypeInfo, MaxEncodedLen,
)]
pub struct ReferralTerms<AccountId> {
	pub version: u16,
	pub genesis_hash: [u8; 32],
	pub payer: AccountId,
	pub target: Page,
	pub source: Page,
	pub source_period: u64,
	pub authorization_id: u64,
	pub count: u32,
	pub price: u128,
	pub cost: u128,
	pub recipient: AccountId,
	pub referral_amount: u128,
	pub target_owner: AccountId,
	pub target_period: Option<u64>,
	pub share_author: u8,
	pub share_referrer: u8,
	pub share_supporters: u8,
	pub rounds: u8,
}
sp_api::decl_runtime_apis! {
	pub trait ReferralApi<AccountId> where AccountId: codec::Codec {
		fn quote(payer: AccountId, target: Page, source: Page, count: u32, authorization_id: u64) -> Result<ReferralTerms<AccountId>, DispatchError>;
	}
}
impl<T: Config> Pallet<T> {
	pub fn referral_now() -> u64 {
		T::Clock::now().as_millis().try_into().unwrap_or(u64::MAX)
	}
	pub fn canonical_host(source: &[u8]) -> Result<(), DispatchError> {
		ensure!(
			wika_traits::page_identity::canonicalize(source).is_some_and(|p| p == source),
			Error::<T>::InvalidUrl
		);
		let local = T::AllowLocalReferralHosts::get()
			&& (source.starts_with(b"http://127.0.0.1:")
				|| source.starts_with(b"http://localhost:"));
		ensure!(
			source.starts_with(b"https://") || local,
			Error::<T>::InvalidUrl
		);
		Ok(())
	}
	pub fn referral_terms(
		payer: &T::AccountId,
		target: &[u8],
		source: &[u8],
		count: u32,
		authorization_id: u64,
	) -> Result<ReferralTerms<T::AccountId>, DispatchError> {
		Self::canonical_host(source)?;
		ensure!(
			wika_traits::page_identity::canonicalize(target).is_some_and(|p| p == target),
			Error::<T>::InvalidUrl
		);
		ensure!(count > 0, Error::<T>::NoLikes);
		ensure!(
			count <= MaxLikes::<T>::get() && count <= DEFAULT_MAX_LIKES,
			Error::<T>::TooManyLikes
		);
		let (owner, period) =
			T::OwnershipRegistry::referral_owner(source).ok_or(Error::<T>::HostNotAuthorized)?;
		let source_key: UrlOf<T> = source
			.to_vec()
			.try_into()
			.map_err(|_| Error::<T>::UrlTooLong)?;
		ensure!(
			HostKeys::<T>::get(source_key)
				.iter()
				.any(|(p, id, _)| *p == period && *id == authorization_id),
			Error::<T>::HostNotAuthorized
		);
		let recipient = if owner == *payer {
			T::OwnershipRegistry::pot_id()
		} else {
			owner
		};
		let cost = Self::price_of(count);
		let referral_amount = if recipient == *payer {
			0
		} else {
			cost.saturating_mul(ShareReferrer::<T>::get().into()) / 100
		};
		// Bind active author ownership even when author rewards use escrow.
		Ok(ReferralTerms {
			version: 1,
			genesis_hash: frame_system::Pallet::<T>::block_hash(
				frame_system::pallet_prelude::BlockNumberFor::<T>::zero(),
			)
			.as_ref()
			.try_into()
			.map_err(|_| Error::<T>::InvalidReferral)?,
			payer: payer.clone(),
			target: target
				.to_vec()
				.try_into()
				.map_err(|_| Error::<T>::UrlTooLong)?,
			source: source
				.to_vec()
				.try_into()
				.map_err(|_| Error::<T>::UrlTooLong)?,
			source_period: period,
			authorization_id,
			count,
			price: LikePrice::<T>::get(),
			cost,
			recipient,
			referral_amount,
			target_owner: T::OwnershipRegistry::owner_of(target),
			target_period: T::OwnershipRegistry::referral_owner(target).map(|(_, p)| p),
			share_author: ShareAuthor::<T>::get(),
			share_referrer: ShareReferrer::<T>::get(),
			share_supporters: SharePreviousLikers::<T>::get(),
			rounds: NumRoundsToRewardLikers::<T>::get(),
		})
	}
	pub fn verify_referral(
		sender: &T::AccountId,
		p: &ReferralPayload<T::AccountId>,
		key: [u8; 32],
		signature: [u8; 64],
	) -> Result<(), DispatchError> {
		ensure!(
			p.domain == DOMAIN && p.version == 1 && &p.payer == sender,
			Error::<T>::InvalidReferral
		);
		let genesis = frame_system::Pallet::<T>::block_hash(
			frame_system::pallet_prelude::BlockNumberFor::<T>::zero(),
		);
		ensure!(
			genesis.as_ref() == p.genesis_hash,
			Error::<T>::InvalidReferral
		);
		let now = Self::referral_now();
		ensure!(
			p.issued_at <= now
				&& now < p.expires_at
				&& p.expires_at > p.issued_at
				&& p.expires_at - p.issued_at <= 300_000,
			Error::<T>::ReferralExpired
		);
		let terms = Self::referral_terms(
			sender,
			&p.target_page,
			&p.source_page,
			p.count,
			p.authorization_id,
		)?;
		ensure!(
			terms.source_period == p.source_period,
			Error::<T>::HostNotAuthorized
		);
		let source: UrlOf<T> = p
			.source_page
			.to_vec()
			.try_into()
			.map_err(|_| Error::<T>::UrlTooLong)?;
		ensure!(
			HostKeys::<T>::get(source)
				.iter()
				.any(|(period, id, k)| *period == p.source_period
					&& *id == p.authorization_id
					&& *k == key),
			Error::<T>::HostNotAuthorized
		);
		ensure!(
			sp_io::crypto::sr25519_verify(
				&sp_core::sr25519::Signature::from_raw(signature),
				&sp_io::hashing::blake2_256(&p.encode()),
				&sp_core::sr25519::Public::from_raw(key)
			),
			Error::<T>::InvalidReferral
		);
		ensure!(
			!UsedRequests::<T>::contains_key(p.authorization_id, p.request_id),
			Error::<T>::RequestConsumed
		);
		Ok(())
	}
	pub fn prune_bucket(slot: u32) {
		let now = Self::referral_now();
		ReplayBuckets::<T>::mutate_exists(slot, |bucket| {
			if let Some(entries) = bucket {
				entries.retain(|(id, request, expiry)| {
					if *expiry <= now {
						UsedRequests::<T>::remove(id, request);
						false
					} else {
						true
					}
				});
				if entries.is_empty() {
					*bucket = None;
				}
			}
		});
	}
}

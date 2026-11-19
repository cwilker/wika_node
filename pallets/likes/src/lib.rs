//! # Likes pallet
//!
//! Anyone can like a URL by paying `LikePrice` per like. Each payment is split:
//!
//! - `ShareAuthor`% to funded author escrow for the current ownership period, or the unclaimed page balance;
//! - `ShareReferrer`% to the owner of the referring URL (`url_ref`), or the owners pot if it is
//!   unclaimed or owned by the liker (nobody earns a referral for their own like);
//! - `SharePreviousLikers`% to the people who liked the URL earlier, first come first served;
//! - whatever is left to this pallet's pot.
//!
//! Every liker joins a queue for the URL and is owed `num_likes * NumRoundsToRewardLikers` likes'
//! worth of the previous-likers share, paid out of later likes as they arrive. An account can like
//! a given URL once.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub use pallet::*;
pub mod economics;
pub mod referrals;
pub use referrals::{ReferralPayload, ReferralTerms};

#[cfg(test)]
mod economics_tests;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod referral_tests;
#[cfg(test)]
mod tests;

#[frame_support::pallet]
pub mod pallet {
	use crate::ReferralPayload;
	use alloc::vec::Vec;
	use frame_support::{
		pallet_prelude::*,
		traits::{
			fungible::{Inspect, Mutate},
			tokens::Preservation,
		},
		PalletId,
	};
	use frame_system::pallet_prelude::*;
	use sp_runtime::{
		traits::{AccountIdConversion, Zero},
		SaturatedConversion,
	};
	use wika_traits::OwnershipRegistry;

	/// Pot that receives the remainder of each like and marks the end of a URL's queue.
	pub const PALLET_ID: PalletId = PalletId(*b"LIKE_ME!");

	pub type BalanceOf<T> =
		<<T as Config>::Currency as Inspect<<T as frame_system::Config>::AccountId>>::Balance;

	/// A URL as stored on chain: raw bytes, at most `MaxLengthURL` long.
	pub type UrlOf<T> = BoundedVec<u8, <T as Config>::MaxLengthURL>;

	const STORAGE_VERSION: StorageVersion = StorageVersion::new(2);
	#[pallet::pallet]
	#[pallet::storage_version(STORAGE_VERSION)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The currency likes are paid in.
		type Currency: Mutate<Self::AccountId>;
		type Clock: frame_support::traits::UnixTime;
		#[pallet::constant]
		type ReferralActivation: Get<BlockNumberFor<Self>>;
		#[pallet::constant]
		type AllowLocalReferralHosts: Get<bool>;

		/// URLs must be strictly shorter than this many bytes.
		#[pallet::constant]
		type MaxLengthURL: Get<u32>;

		/// Who gets paid as author and referrer.
		type OwnershipRegistry: OwnershipRegistry<Self::AccountId, BalanceOf<Self>>;
	}

	#[pallet::type_value]
	pub fn DefaultLikePrice() -> u128 {
		1_000_000_000_000
	}
	/// Default `MaxLikes`, also the cap used to price a call's weight.
	pub const DEFAULT_MAX_LIKES: u32 = 100;

	#[pallet::type_value]
	pub fn DefaultMaxLikes() -> u32 {
		DEFAULT_MAX_LIKES
	}
	#[pallet::type_value]
	pub fn DefaultShare() -> u8 {
		33
	}
	#[pallet::type_value]
	pub fn DefaultNumRounds() -> u8 {
		4
	}

	/// Number of distinct URLs liked so far.
	#[pallet::storage]
	pub type UrlCount<T> = StorageValue<_, u128, ValueQuery>;

	/// Price of one like.
	#[pallet::storage]
	pub type LikePrice<T> = StorageValue<_, u128, ValueQuery, DefaultLikePrice>;

	/// Most likes allowed in one call.
	#[pallet::storage]
	pub type MaxLikes<T> = StorageValue<_, u32, ValueQuery, DefaultMaxLikes>;

	/// Percentage of each like paid to the URL's owner.
	#[pallet::storage]
	pub type ShareAuthor<T> = StorageValue<_, u8, ValueQuery, DefaultShare>;

	/// Percentage of each like paid to the referring URL's owner.
	#[pallet::storage]
	pub type ShareReferrer<T> = StorageValue<_, u8, ValueQuery, DefaultShare>;

	/// Percentage of each like paid to earlier likers.
	#[pallet::storage]
	pub type SharePreviousLikers<T> = StorageValue<_, u8, ValueQuery, DefaultShare>;

	/// How many times over a liker is repaid from later likes.
	#[pallet::storage]
	pub type NumRoundsToRewardLikers<T> = StorageValue<_, u8, ValueQuery, DefaultNumRounds>;

	/// Per URL:
	/// - total likes received;
	/// - the liker at the front of the queue, next to be paid;
	/// - the liker at the back of the queue (the pot when the queue has one entry).
	#[pallet::storage]
	pub type Urls<T: Config> =
		StorageMap<_, Blake2_128Concat, UrlOf<T>, (u64, T::AccountId, T::AccountId), OptionQuery>;

	/// Per liker and URL:
	/// - total likes on the URL before this liker's;
	/// - number of likes this liker paid for;
	/// - likes' worth still owed to this liker;
	/// - the next liker in the queue (the pot if none).
	#[pallet::storage]
	pub type Likes<T: Config> = StorageDoubleMap<
		_,
		Blake2_128Concat,
		T::AccountId,
		Blake2_128Concat,
		UrlOf<T>,
		(u64, u32, u32, T::AccountId),
		OptionQuery,
	>;

	#[pallet::storage]
	pub type HostKeys<T: Config> = StorageMap<
		_,
		Blake2_128Concat,
		UrlOf<T>,
		BoundedVec<(u64, u64, [u8; 32]), ConstU32<4>>,
		ValueQuery,
	>;
	#[pallet::storage]
	pub type NextAuthorization<T> = StorageValue<_, u64, ValueQuery>;
	#[pallet::storage]
	pub type UsedRequests<T> =
		StorageDoubleMap<_, Twox64Concat, u64, Blake2_128Concat, [u8; 32], u64, OptionQuery>;
	#[pallet::storage]
	pub type ReplayBuckets<T> = StorageMap<
		_,
		Twox64Concat,
		u32,
		BoundedVec<(u64, [u8; 32], u64), ConstU32<64>>,
		ValueQuery,
	>;
	#[pallet::storage]
	pub type ReplayCursor<T> = StorageValue<_, u32, ValueQuery>;

	/// Absent until governance supplies explicit release configuration and a future boundary.
	#[pallet::storage]
	pub type EconomicRules<T: Config> =
		StorageValue<_, crate::economics::Rules<T::AccountId>, OptionQuery>;
	#[pallet::storage]
	pub type DefaultProfile<T: Config> =
		StorageValue<_, crate::economics::Policy<T::AccountId>, OptionQuery>;
	#[pallet::storage]
	pub type LegacyProfile<T: Config> =
		StorageValue<_, crate::economics::Policy<T::AccountId>, OptionQuery>;
	#[pallet::storage]
	pub type EconomicsBoundary<T: Config> = StorageValue<_, BlockNumberFor<T>, OptionQuery>;
	#[pallet::storage]
	pub type EconomicsActive<T> = StorageValue<_, bool, ValueQuery>;
	#[pallet::storage]
	pub type PagePolicies<T: Config> = StorageMap<
		_,
		Blake2_128Concat,
		UrlOf<T>,
		crate::economics::Policy<T::AccountId>,
		OptionQuery,
	>;
	#[pallet::storage]
	pub type PolicyLocks<T: Config> =
		StorageMap<_, Blake2_128Concat, UrlOf<T>, BlockNumberFor<T>, OptionQuery>;
	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_initialize(now: BlockNumberFor<T>) -> Weight {
			if !EconomicsActive::<T>::get() && EconomicsBoundary::<T>::get() == Some(now) {
				LegacyProfile::<T>::put(Self::legacy_policy());
				EconomicsActive::<T>::put(true);
				Self::deposit_event(Event::EconomicsActivated {
					boundary: now,
					legacy: Self::legacy_policy(),
				});
				T::DbWeight::get().reads_writes(12, 2)
			} else {
				T::DbWeight::get().reads(2)
			}
		}
		fn on_runtime_upgrade() -> Weight {
			if StorageVersion::get::<Pallet<T>>() < STORAGE_VERSION {
				STORAGE_VERSION.put::<Pallet<T>>();
				T::DbWeight::get().reads_writes(1, 1)
			} else {
				T::DbWeight::get().reads(1)
			}
		}
		fn on_idle(_: BlockNumberFor<T>, remaining: Weight) -> Weight {
			let cost = Weight::from_parts(100_000_000, 24_000)
				.saturating_add(T::DbWeight::get().reads_writes(66, 66));
			if !cost.all_lte(remaining) {
				return Weight::zero();
			}
			let slot = ReplayCursor::<T>::get();
			Self::prune_bucket(slot);
			ReplayCursor::<T>::put((slot + 1) % 302);
			cost
		}
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		HostAuthorized {
			source: UrlOf<T>,
			period: u64,
			owner: T::AccountId,
			public_key: [u8; 32],
			authorization_id: u64,
		},
		HostRevoked {
			source: UrlOf<T>,
			period: u64,
			owner: T::AccountId,
			public_key: [u8; 32],
			authorization_id: u64,
		},
		ReferralPaid {
			version: u16,
			payer: T::AccountId,
			target: UrlOf<T>,
			source: UrlOf<T>,
			source_period: u64,
			authorization_id: u64,
			request_id: [u8; 32],
			recipient: T::AccountId,
			amount: u128,
			count: u32,
			terms_version: u16,
			terms_hash: [u8; 32],
		},
		EconomicsScheduled {
			boundary: BlockNumberFor<T>,
			rules: crate::economics::Rules<T::AccountId>,
			profile: crate::economics::Policy<T::AccountId>,
		},
		EconomicsActivated {
			boundary: BlockNumberFor<T>,
			legacy: crate::economics::Policy<T::AccountId>,
		},
		DefaultPublished {
			profile: crate::economics::Policy<T::AccountId>,
		},
		PolicyConfigured {
			page: UrlOf<T>,
			policy_hash: [u8; 32],
			policy: crate::economics::Policy<T::AccountId>,
		},
		PolicyLocked {
			page: UrlOf<T>,
			policy_hash: [u8; 32],
			revision: u64,
		},
		SupporterPaid {
			page: UrlOf<T>,
			payer: T::AccountId,
			recipient: T::AccountId,
			amount: u128,
		},
		PaymentDistribution {
			payer: T::AccountId,
			target: UrlOf<T>,
			terms_hash: [u8; 32],
			receipt: crate::economics::Distribution<T::AccountId>,
		},
		/// `who` liked `url` `num_likes` times.
		Liked {
			who: T::AccountId,
			url: UrlOf<T>,
			num_likes: u32,
		},
	}

	#[pallet::error]
	pub enum Error<T> {
		/// The account cannot pay for this many likes.
		NotEnoughBalanceToLike,
		/// More than `MaxLikes` likes in one call.
		TooManyLikes,
		/// This account has already liked this URL.
		AlreadyInQueue,
		/// The URL is `MaxLengthURL` bytes or longer.
		UrlTooLong,
		/// `num_likes` was zero.
		NoLikes,
		InvalidUrl,
		SignedReferralRequired,
		InvalidReferral,
		NotHostOwner,
		HostKeyLimit,
		HostNotAuthorized,
		AuthorizationOverflow,
		ReferralExpired,
		RequestConsumed,
		ReplayCapacity,
		TermsChanged,
		ReferralsNotActive,
		EconomicsNotActive,
		EconomicTermsRequired,
		InvalidPolicy,
		PolicyLocked,
		EconomicOverflow,
		InvalidQueue,
		InvalidEconomicPayer,
		InheritanceConsentRequired,
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Like `url`, crediting the owner of `url_ref` (the page that led here) as referrer.
		#[pallet::call_index(0)]
		#[pallet::weight(Pallet::<T>::like_weight(*num_likes))]
		#[frame_support::transactional]
		pub fn like(
			origin: OriginFor<T>,
			url: Vec<u8>,
			url_ref: Vec<u8>,
			num_likes: u32,
		) -> DispatchResult {
			ensure!(!Self::economics_active(), Error::<T>::EconomicTermsRequired);
			ensure!(
				url_ref.is_empty()
					|| frame_system::Pallet::<T>::block_number() < T::ReferralActivation::get(),
				Error::<T>::SignedReferralRequired
			);
			Self::execute_like(ensure_signed(origin)?, url, url_ref, num_likes)
		}

		#[pallet::call_index(1)]
		#[pallet::weight(Weight::from_parts(150_000_000, 12_000).saturating_add(T::DbWeight::get().reads_writes(8, 3)))]
		#[frame_support::transactional]
		pub fn authorize_host(
			origin: OriginFor<T>,
			source: UrlOf<T>,
			public_key: [u8; 32],
		) -> DispatchResult {
			let owner = ensure_signed(origin)?;
			Self::canonical_host(&source)?;
			let (current, period) =
				T::OwnershipRegistry::referral_owner(&source).ok_or(Error::<T>::NotHostOwner)?;
			ensure!(owner == current, Error::<T>::NotHostOwner);
			let id = NextAuthorization::<T>::get()
				.checked_add(1)
				.ok_or(Error::<T>::AuthorizationOverflow)?;
			HostKeys::<T>::try_mutate(&source, |keys| -> DispatchResult {
				keys.retain(|(p, _, key)| *p == period && key != &public_key);
				keys.try_push((period, id, public_key))
					.map_err(|_| Error::<T>::HostKeyLimit)?;
				Ok(())
			})?;
			NextAuthorization::<T>::put(id);
			Self::deposit_event(Event::HostAuthorized {
				source,
				period,
				owner,
				public_key,
				authorization_id: id,
			});
			Ok(())
		}

		#[pallet::call_index(2)]
		#[pallet::weight(Weight::from_parts(100_000_000, 12_000).saturating_add(T::DbWeight::get().reads_writes(6, 1)))]
		pub fn revoke_host(
			origin: OriginFor<T>,
			source: UrlOf<T>,
			authorization_id: u64,
		) -> DispatchResult {
			let owner = ensure_signed(origin)?;
			let (current, period) =
				T::OwnershipRegistry::referral_owner(&source).ok_or(Error::<T>::NotHostOwner)?;
			ensure!(owner == current, Error::<T>::NotHostOwner);
			let key = HostKeys::<T>::get(&source)
				.into_iter()
				.find(|(p, id, _)| *p == period && *id == authorization_id)
				.ok_or(Error::<T>::HostNotAuthorized)?
				.2;
			HostKeys::<T>::mutate(&source, |keys| {
				keys.retain(|(_, id, _)| *id != authorization_id)
			});
			Self::deposit_event(Event::HostRevoked {
				source,
				period,
				owner,
				public_key: key,
				authorization_id,
			});
			Ok(())
		}

		#[pallet::call_index(3)]
		#[pallet::weight(Pallet::<T>::like_weight(payload.count).saturating_add(Weight::from_parts(500_000_000, 48_000)).saturating_add(T::DbWeight::get().reads_writes(80, 70)))]
		#[frame_support::transactional]
		pub fn like_with_referral(
			origin: OriginFor<T>,
			payload: ReferralPayload<T::AccountId>,
			public_key: [u8; 32],
			signature: [u8; 64],
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			ensure!(
				frame_system::Pallet::<T>::block_number() >= T::ReferralActivation::get(),
				Error::<T>::ReferralsNotActive
			);
			Self::verify_referral(&sender, &payload, public_key, signature)?;
			let terms = Self::referral_terms(
				&sender,
				&payload.target_page,
				&payload.source_page,
				payload.count,
				payload.authorization_id,
			)?;
			let economic = if Self::economics_active() {
				let quote = Self::economic_terms(
					&sender,
					&payload.target_page,
					&payload.source_page,
					payload.count,
					payload.authorization_id,
				)?;
				ensure!(
					payload.terms_version == 2
						&& payload.terms_hash == sp_io::hashing::blake2_256(&quote.encode()),
					Error::<T>::TermsChanged
				);
				Some(quote)
			} else {
				ensure!(
					payload.terms_version == 1
						&& payload.terms_hash == sp_io::hashing::blake2_256(&terms.encode()),
					Error::<T>::TermsChanged
				);
				None
			};
			let slot = ((payload.expires_at / 1000) % 302) as u32;
			Self::prune_bucket(slot);
			ReplayBuckets::<T>::try_mutate(slot, |bucket| {
				bucket.try_push((
					payload.authorization_id,
					payload.request_id,
					payload.expires_at,
				))
			})
			.map_err(|_| Error::<T>::ReplayCapacity)?;
			if let Some(quote) = economic.as_ref() {
				Self::execute_economic(sender.clone(), quote, payload.terms_hash)?;
			} else {
				Self::execute_like(
					sender.clone(),
					payload.target_page.to_vec(),
					payload.source_page.to_vec(),
					payload.count,
				)?;
			}
			UsedRequests::<T>::insert(
				payload.authorization_id,
				payload.request_id,
				payload.expires_at,
			);
			Self::deposit_event(Event::ReferralPaid {
				version: 1,
				payer: sender,
				target: payload
					.target_page
					.to_vec()
					.try_into()
					.map_err(|_| Error::<T>::UrlTooLong)?,
				source: payload
					.source_page
					.to_vec()
					.try_into()
					.map_err(|_| Error::<T>::UrlTooLong)?,
				source_period: payload.source_period,
				authorization_id: payload.authorization_id,
				request_id: payload.request_id,
				recipient: economic
					.as_ref()
					.map(|t| t.recipient.clone())
					.unwrap_or(terms.recipient),
				amount: economic
					.as_ref()
					.map(|t| {
						if t.referral_to_target && t.retained_owner > 0 {
							0
						} else {
							t.referral_amount
						}
					})
					.unwrap_or(terms.referral_amount),
				count: payload.count,
				terms_version: payload.terms_version,
				terms_hash: payload.terms_hash,
			});
			Ok(())
		}

		/// Reviewed, terms-bound direct payment; normal extrinsic nonce/era handles replay.
		#[pallet::call_index(4)]
		#[pallet::weight(Pallet::<T>::like_weight(*count).saturating_add(Weight::from_parts(300_000_000,48000)))]
		#[frame_support::transactional]
		pub fn like_with_terms(
			origin: OriginFor<T>,
			target: UrlOf<T>,
			count: u32,
			terms_version: u16,
			terms_hash: [u8; 32],
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			let terms = Self::economic_terms(&sender, &target, &[], count, 0)?;
			ensure!(
				terms_version == 2 && terms_hash == sp_io::hashing::blake2_256(&terms.encode()),
				Error::<T>::TermsChanged
			);
			Self::execute_economic(sender, &terms, terms_hash)
		}
		#[pallet::call_index(5)]
		#[pallet::weight(Weight::from_parts(200_000_000,24000).saturating_add(T::DbWeight::get().reads_writes(12,2)))]
		#[frame_support::transactional]
		pub fn configure_page_policy(
			origin: OriginFor<T>,
			page: UrlOf<T>,
			period: u64,
			selection: wika_traits::PolicySelection,
		) -> DispatchResult {
			let owner = ensure_signed(origin)?;
			ensure!(
				T::OwnershipRegistry::referral_owner(&page) == Some((owner.clone(), period)),
				Error::<T>::NotHostOwner
			);
			Self::install_policy(&page, &owner, period, &selection)
		}
		/// One-way boundary. No implicit genesis rates or treasury recipient.
		#[pallet::call_index(6)]
		#[pallet::weight(Weight::from_parts(150_000_000,20000).saturating_add(T::DbWeight::get().reads_writes(4,3)))]
		#[frame_support::transactional]
		pub fn schedule_economics(
			origin: OriginFor<T>,
			boundary: BlockNumberFor<T>,
			rules: crate::economics::Rules<T::AccountId>,
			owner_bps: u16,
			supporter_bps: u16,
			referral_bps: u16,
			rounds: u8,
		) -> DispatchResult {
			ensure_root(origin)?;
			ensure!(
				!Self::economics_active()
					&& EconomicsBoundary::<T>::get().is_none()
					&& boundary > frame_system::Pallet::<T>::block_number(),
				Error::<T>::InvalidPolicy
			);
			Self::validate_split(&rules, owner_bps, supporter_bps, referral_bps)?;
			ensure!(
				rounds > 0
					&& rules.owner_min <= rules.owner_max
					&& rules.supporter_min <= rules.supporter_max
					&& rules.referral_min <= rules.referral_max
					&& rules.owner_max <= 10000
					&& rules.supporter_max <= 10000
					&& rules.referral_max <= 10000
					&& LikePrice::<T>::get() >= rules.minimum_price,
				Error::<T>::InvalidPolicy
			);
			let profile = crate::economics::Policy {
				ruleset: rules.version,
				revision: 0,
				model: 1,
				owner_bps,
				supporter_bps,
				referral_bps,
				protocol_bps: rules.protocol_bps,
				rounds,
				treasury: rules.treasury.clone(),
				configurator: None,
				owner_period: None,
			};
			EconomicRules::<T>::put(&rules);
			DefaultProfile::<T>::put(&profile);
			EconomicsBoundary::<T>::put(boundary);
			Self::deposit_event(Event::EconomicsScheduled {
				boundary,
				rules,
				profile,
			});
			Ok(())
		}
		#[pallet::call_index(7)]
		#[pallet::weight(Weight::from_parts(100_000_000,12000).saturating_add(T::DbWeight::get().reads_writes(3,1)))]
		pub fn publish_default(
			origin: OriginFor<T>,
			owner_bps: u16,
			supporter_bps: u16,
			referral_bps: u16,
		) -> DispatchResult {
			ensure_root(origin)?;
			ensure!(Self::economics_active(), Error::<T>::EconomicsNotActive);
			let rules = EconomicRules::<T>::get().ok_or(Error::<T>::EconomicsNotActive)?;
			Self::validate_split(&rules, owner_bps, supporter_bps, referral_bps)?;
			let mut profile = DefaultProfile::<T>::get().ok_or(Error::<T>::EconomicsNotActive)?;
			profile.revision = profile
				.revision
				.checked_add(1)
				.ok_or(Error::<T>::EconomicOverflow)?;
			profile.owner_bps = owner_bps;
			profile.supporter_bps = supporter_bps;
			profile.referral_bps = referral_bps;
			DefaultProfile::<T>::put(&profile);
			Self::deposit_event(Event::DefaultPublished { profile });
			Ok(())
		}
	}

	impl<T: Config> Pallet<T> {
		pub(crate) fn execute_like(
			sender: T::AccountId,
			url: Vec<u8>,
			url_ref: Vec<u8>,
			num_likes: u32,
		) -> DispatchResult {
			ensure!(num_likes > 0, Error::<T>::NoLikes);

			let total: BalanceOf<T> = Self::price_of(num_likes).saturated_into();
			ensure!(
				T::Currency::balance(&sender) > total,
				Error::<T>::NotEnoughBalanceToLike
			);
			ensure!(
				num_likes <= MaxLikes::<T>::get() && num_likes <= DEFAULT_MAX_LIKES,
				Error::<T>::TooManyLikes
			);
			ensure!(
				url.len() < T::MaxLengthURL::get() as usize,
				Error::<T>::UrlTooLong
			);
			let url =
				wika_traits::page_identity::canonicalize(&url).ok_or(Error::<T>::InvalidUrl)?;
			let url: UrlOf<T> = url.try_into().map_err(|_| Error::<T>::UrlTooLong)?;

			ensure!(
				url_ref.len() < T::MaxLengthURL::get() as usize,
				Error::<T>::UrlTooLong
			);
			let url_ref = if url_ref.is_empty() {
				url_ref
			} else {
				wika_traits::page_identity::canonicalize(&url_ref).ok_or(Error::<T>::InvalidUrl)?
			};

			if let Some(state) = Urls::<T>::get(&url) {
				ensure!(
					!Likes::<T>::contains_key(&sender, &url),
					Error::<T>::AlreadyInQueue
				);
				Self::like_existing_url(&sender, &url, &url_ref, num_likes, state)?;
			} else {
				Self::like_new_url(&sender, &url, &url_ref, num_likes)?;
			}

			Self::deposit_event(Event::Liked {
				who: sender,
				url,
				num_likes,
			});
			Ok(())
		}
	}

	impl<T: Config> Pallet<T> {
		pub fn pot_id() -> T::AccountId {
			PALLET_ID.into_account_truncating()
		}

		/// Worst case: every like pays a different earlier liker.
		fn like_weight(num_likes: u32) -> Weight {
			let n = u64::from(num_likes.min(DEFAULT_MAX_LIKES));
			Weight::from_parts(200_000_000, 16_000)
				.saturating_add(Weight::from_parts(50_000_000, 12_000).saturating_mul(n))
				.saturating_add(T::DbWeight::get().reads_writes(20 + 5 * n, 16 + 5 * n))
		}

		pub(crate) fn price_of(num_likes: u32) -> u128 {
			LikePrice::<T>::get().saturating_mul(num_likes.into())
		}

		fn pay(
			sender: &T::AccountId,
			num_likes: u32,
			recipient: &T::AccountId,
			share: u8,
		) -> DispatchResult {
			let amount: BalanceOf<T> =
				(Self::price_of(num_likes).saturating_mul(share.into()) / 100).saturated_into();
			if amount.is_zero() || sender == recipient {
				return Ok(());
			}
			log::debug!(target: "LIKE", "paying {:?} from {:?} to {:?}", amount, sender, recipient);
			T::Currency::transfer(sender, recipient, amount, Preservation::Preserve)?;
			Ok(())
		}

		fn pay_author_referrer_and_extra(
			sender: &T::AccountId,
			url: &[u8],
			url_ref: &[u8],
			num_likes: u32,
		) -> DispatchResult {
			let amount: BalanceOf<T> = (Self::price_of(num_likes)
				.saturating_mul(((Self::legacy_policy().owner_bps / 100) as u8).into())
				/ 100)
				.saturated_into();
			T::OwnershipRegistry::pay_author(sender, url, amount)?;

			// A liker can't refer themselves. The like still goes through, because wallets fill in
			// `url_ref` from the tab the user came from, which may be the user's own page.
			let mut referrer = T::OwnershipRegistry::owner_of(url_ref);
			if &referrer == sender {
				referrer = T::OwnershipRegistry::pot_id();
			}
			Self::pay(
				sender,
				num_likes,
				&referrer,
				(Self::legacy_policy().referral_bps / 100) as u8,
			)?;

			let extra = 100u8.saturating_sub(
				((Self::legacy_policy().supporter_bps / 100) as u8)
					.saturating_add((Self::legacy_policy().owner_bps / 100) as u8)
					.saturating_add((Self::legacy_policy().referral_bps / 100) as u8),
			);
			Self::pay(sender, num_likes, &Self::pot_id(), extra)
		}

		/// Pays queued likers, front first, until `num_likes` is used up or the queue ends.
		/// Returns the new front of the queue and any likes left over.
		fn pay_previous_likers(
			sender: &T::AccountId,
			url: &UrlOf<T>,
			first_recipient: T::AccountId,
			num_likes: u32,
		) -> Result<(T::AccountId, u32), DispatchError> {
			let pot = Self::pot_id();
			let share = (Self::legacy_policy().supporter_bps / 100) as u8;
			let mut recipient = first_recipient;
			let mut remaining = num_likes;
			while remaining > 0 && recipient != pot {
				let Some((likes_before, count, owed, next)) = Likes::<T>::get(&recipient, url)
				else {
					break;
				};
				if remaining >= owed {
					Self::pay(sender, owed, &recipient, share)?;
					remaining -= owed;
					Likes::<T>::insert(&recipient, url, (likes_before, count, 0, next.clone()));
					recipient = next;
				} else {
					Self::pay(sender, remaining, &recipient, share)?;
					Likes::<T>::insert(
						&recipient,
						url,
						(likes_before, count, owed - remaining, next),
					);
					remaining = 0;
				}
			}
			Ok((recipient, remaining))
		}

		pub(crate) fn add_to_queue(
			sender: &T::AccountId,
			url: &UrlOf<T>,
			first_in_line: T::AccountId,
			last_in_line: T::AccountId,
		) {
			let account = if last_in_line == Self::pot_id() {
				first_in_line
			} else {
				last_in_line
			};
			Likes::<T>::mutate(&account, url, |record| {
				if let Some(record) = record {
					record.3 = sender.clone();
				}
			});
		}

		fn like_existing_url(
			sender: &T::AccountId,
			url: &UrlOf<T>,
			url_ref: &[u8],
			num_likes: u32,
			(total_likes, first, last): (u64, T::AccountId, T::AccountId),
		) -> DispatchResult {
			let pot = Self::pot_id();
			let rounds = u32::from(Self::legacy_policy().rounds);
			Likes::<T>::insert(
				sender,
				url,
				(
					total_likes,
					num_likes,
					num_likes.saturating_mul(rounds),
					pot.clone(),
				),
			);

			let (next_recipient, remaining) =
				Self::pay_previous_likers(sender, url, first.clone(), num_likes)?;
			if remaining > 0 {
				Self::pay(
					sender,
					remaining,
					&pot,
					(Self::legacy_policy().supporter_bps / 100) as u8,
				)?;
			}
			Self::pay_author_referrer_and_extra(sender, url, url_ref, num_likes)?;
			Self::add_to_queue(sender, url, first, last);

			let total = total_likes.saturating_add(num_likes.into());
			if next_recipient == pot {
				// Everyone before the sender has been paid in full.
				Urls::<T>::insert(url, (total, sender.clone(), pot));
			} else {
				Urls::<T>::insert(url, (total, next_recipient, sender.clone()));
			}
			Ok(())
		}

		fn like_new_url(
			sender: &T::AccountId,
			url: &UrlOf<T>,
			url_ref: &[u8],
			num_likes: u32,
		) -> DispatchResult {
			let pot = Self::pot_id();
			let rounds = u32::from(Self::legacy_policy().rounds);
			Urls::<T>::insert(url, (u64::from(num_likes), sender.clone(), pot.clone()));
			Likes::<T>::insert(
				sender,
				url,
				(0, num_likes, num_likes.saturating_mul(rounds), pot.clone()),
			);
			// Nobody to repay yet, so the previous-likers share goes to the pot.
			Self::pay(
				sender,
				num_likes,
				&pot,
				(Self::legacy_policy().supporter_bps / 100) as u8,
			)?;
			Self::pay_author_referrer_and_extra(sender, url, url_ref, num_likes)?;
			UrlCount::<T>::mutate(|n| *n = n.saturating_add(1));
			Ok(())
		}
	}
}

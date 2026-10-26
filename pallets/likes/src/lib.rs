//! # Likes pallet
//!
//! Anyone can like a URL by paying `LikePrice` per like. Each payment is split:
//!
//! - `ShareAuthor`% to the owner of the URL (see the owners pallet), or the owners pot if unclaimed;
//! - `ShareReferrer`% to the owner of the referring URL (`url_ref`), or the owners pot;
//! - `SharePreviousLikers`% to the people who liked the URL earlier, first come first served;
//! - whatever is left to this pallet's pot.
//!
//! Every liker joins a queue for the URL and is owed `num_likes * NumRoundsToRewardLikers` likes'
//! worth of the previous-likers share, paid out of later likes as they arrive. An account can like
//! a given URL once.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub use pallet::*;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

#[frame_support::pallet]
pub mod pallet {
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

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The currency likes are paid in.
		type Currency: Mutate<Self::AccountId>;

		/// URLs must be strictly shorter than this many bytes.
		#[pallet::constant]
		type MaxLengthURL: Get<u32>;

		/// Who gets paid as author and referrer.
		type OwnershipRegistry: OwnershipRegistry<Self::AccountId>;
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

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// `who` liked `url` `num_likes` times.
		Liked { who: T::AccountId, url: UrlOf<T>, num_likes: u32 },
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
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Like `url`, crediting the owner of `url_ref` (the page that led here) as referrer.
		#[pallet::call_index(0)]
		#[pallet::weight(Pallet::<T>::like_weight(*num_likes))]
		pub fn like(
			origin: OriginFor<T>,
			url: Vec<u8>,
			url_ref: Vec<u8>,
			num_likes: u32,
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			ensure!(num_likes > 0, Error::<T>::NoLikes);

			let total: BalanceOf<T> = Self::price_of(num_likes).saturated_into();
			ensure!(T::Currency::balance(&sender) > total, Error::<T>::NotEnoughBalanceToLike);
			ensure!(num_likes <= MaxLikes::<T>::get(), Error::<T>::TooManyLikes);
			ensure!(url.len() < T::MaxLengthURL::get() as usize, Error::<T>::UrlTooLong);
			let url: UrlOf<T> = url.try_into().map_err(|_| Error::<T>::UrlTooLong)?;

			if let Some(state) = Urls::<T>::get(&url) {
				ensure!(!Likes::<T>::contains_key(&sender, &url), Error::<T>::AlreadyInQueue);
				Self::like_existing_url(&sender, &url, &url_ref, num_likes, state)?;
			} else {
				Self::like_new_url(&sender, &url, &url_ref, num_likes)?;
			}

			Self::deposit_event(Event::Liked { who: sender, url, num_likes });
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
			Weight::from_parts(60_000_000, 0)
				.saturating_add(Weight::from_parts(25_000_000, 0).saturating_mul(n))
				.saturating_add(T::DbWeight::get().reads_writes(12 + 3 * n, 10 + 3 * n))
		}

		fn price_of(num_likes: u32) -> u128 {
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
			let author = T::OwnershipRegistry::owner_of(url);
			Self::pay(sender, num_likes, &author, ShareAuthor::<T>::get())?;

			let referrer = T::OwnershipRegistry::owner_of(url_ref);
			Self::pay(sender, num_likes, &referrer, ShareReferrer::<T>::get())?;

			let extra = 100u8.saturating_sub(
				SharePreviousLikers::<T>::get()
					.saturating_add(ShareAuthor::<T>::get())
					.saturating_add(ShareReferrer::<T>::get()),
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
			let share = SharePreviousLikers::<T>::get();
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
					Likes::<T>::insert(&recipient, url, (likes_before, count, owed - remaining, next));
					remaining = 0;
				}
			}
			Ok((recipient, remaining))
		}

		fn add_to_queue(
			sender: &T::AccountId,
			url: &UrlOf<T>,
			first_in_line: T::AccountId,
			last_in_line: T::AccountId,
		) {
			let account = if last_in_line == Self::pot_id() { first_in_line } else { last_in_line };
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
			let rounds = u32::from(NumRoundsToRewardLikers::<T>::get());
			Likes::<T>::insert(
				sender,
				url,
				(total_likes, num_likes, num_likes.saturating_mul(rounds), pot.clone()),
			);

			let (next_recipient, remaining) =
				Self::pay_previous_likers(sender, url, first.clone(), num_likes)?;
			if remaining > 0 {
				Self::pay(sender, remaining, &pot, SharePreviousLikers::<T>::get())?;
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
			let rounds = u32::from(NumRoundsToRewardLikers::<T>::get());
			Urls::<T>::insert(url, (u64::from(num_likes), sender.clone(), pot.clone()));
			Likes::<T>::insert(
				sender,
				url,
				(0, num_likes, num_likes.saturating_mul(rounds), pot.clone()),
			);
			// Nobody to repay yet, so the previous-likers share goes to the pot.
			Self::pay(sender, num_likes, &pot, SharePreviousLikers::<T>::get())?;
			Self::pay_author_referrer_and_extra(sender, url, url_ref, num_likes)?;
			UrlCount::<T>::mutate(|n| *n = n.saturating_add(1));
			Ok(())
		}
	}
}

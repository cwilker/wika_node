//! # Owners pallet
//!
//! Records who owns a URL, so the likes pallet can pay its author.
//!
//! 1. A user pays `RequestPrice` and calls `request_url_check(url)` at block `R`.
//! 2. Every node holding an enabled verifier's `ownr` key fetches the page in its offchain worker.
//!    The page proves ownership if it contains `wika.network/author/` followed, within 128
//!    characters, by the requester's public key in hex. The verifier commits a hash of its vote
//!    during blocks `R+1 ..= R+NumBlocksToCommit`.
//! 3. It reveals the vote during the following `NumBlocksToReveal` blocks. The salt is the
//!    verifier's signature over the vote, so it cannot be guessed and is checked on chain.
//! 4. One block after the reveal window closes, the votes are tallied into `Results`. If the
//!    majority voted yes, holds more than `PrctNeededForAgreement`% of the votes and has at least
//!    `MajorityMin` members, the requester becomes the owner in `Owners`.
//! 5. `NumBlocksToDelete` blocks later the request, commits, reveals and result are deleted.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub use pallet::*;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

use alloc::vec::Vec;
use sp_core::crypto::KeyTypeId;

/// Key type of the verifier key nodes use to sign commits and reveals.
pub const KEY_TYPE: KeyTypeId = KeyTypeId(*b"ownr");

/// Text that introduces the ownership mark on a page.
pub const MARK_PREFIX: &[u8] = b"wika.network/author/";

/// Bytes of the page kept as `intro`.
pub const INTRO_LENGTH: usize = 128;

/// Bytes of the page kept as the mark, starting at `MARK_PREFIX`.
pub const MARK_LENGTH: usize = 128;

const HASH_LENGTH: usize = 32;

pub mod crypto {
	use super::KEY_TYPE;
	use sp_core::sr25519::Signature as Sr25519Signature;
	use sp_runtime::{
		app_crypto::{app_crypto, sr25519},
		traits::Verify,
		MultiSignature, MultiSigner,
	};

	app_crypto!(sr25519, KEY_TYPE);

	pub struct OwnersAppCrypto;

	impl frame_system::offchain::AppCrypto<MultiSigner, MultiSignature> for OwnersAppCrypto {
		type RuntimeAppPublic = Public;
		type GenericSignature = sp_core::sr25519::Signature;
		type GenericPublic = sp_core::sr25519::Public;
	}

	impl frame_system::offchain::AppCrypto<<Sr25519Signature as Verify>::Signer, Sr25519Signature>
		for OwnersAppCrypto
	{
		type RuntimeAppPublic = Public;
		type GenericSignature = sp_core::sr25519::Signature;
		type GenericPublic = sp_core::sr25519::Public;
	}
}

/// Lowercase hex, as it appears in an ownership mark.
pub fn to_hex(bytes: &[u8]) -> Vec<u8> {
	const DIGITS: &[u8; 16] = b"0123456789abcdef";
	bytes.iter().flat_map(|b| [DIGITS[(b >> 4) as usize], DIGITS[(b & 0xf) as usize]]).collect()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
	if needle.is_empty() || haystack.len() < needle.len() {
		return None;
	}
	haystack.windows(needle.len()).position(|w| w == needle)
}

/// Whether `mark` is a valid ownership mark for the account whose encoded id is `account`.
/// Hex is matched case-insensitively.
pub fn mark_proves(mark: &[u8], account: &[u8]) -> bool {
	mark.len() <= MARK_LENGTH &&
		mark.starts_with(MARK_PREFIX) &&
		find(&mark.to_ascii_lowercase(), &to_hex(account)).is_some()
}

/// What a verifier saw on a page: its vote, the first bytes of the page, and the mark when it
/// votes yes (empty otherwise).
pub fn inspect_page(page: &[u8], account: &[u8]) -> (bool, Vec<u8>, Vec<u8>) {
	let intro = page[..page.len().min(INTRO_LENGTH)].to_vec();
	let Some(start) = find(page, MARK_PREFIX) else { return (false, intro, Vec::new()) };
	let mark = &page[start..page.len().min(start + MARK_LENGTH)];
	if mark_proves(mark, account) {
		(true, intro, mark.to_vec())
	} else {
		(false, intro, Vec::new())
	}
}

/// The message a verifier signs to make its salt: `vote,intro[,proof]`.
pub fn vote_message(vote: bool, intro: &[u8], proof: &[u8]) -> Vec<u8> {
	let mut out = Vec::with_capacity(4 + intro.len() + proof.len());
	out.push(if vote { b'1' } else { b'0' });
	out.push(b',');
	out.extend_from_slice(intro);
	if vote {
		out.push(b',');
		out.extend_from_slice(proof);
	}
	out
}

/// The preimage of a commit: `vote,intro[,proof],salt`.
pub fn commit_preimage(vote: bool, intro: &[u8], proof: &[u8], salt: &[u8]) -> Vec<u8> {
	let mut out = vote_message(vote, intro, proof);
	out.push(b',');
	out.extend_from_slice(salt);
	out
}

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use alloc::collections::btree_map::BTreeMap;
	use codec::{Decode, Encode};
	use frame_support::{
		pallet_prelude::*,
		traits::{
			fungible::{Inspect, Mutate},
			tokens::Preservation,
		},
		PalletId,
	};
	use frame_system::{
		offchain::{AppCrypto, CreateSignedTransaction, SendSignedTransaction, Signer},
		pallet_prelude::*,
	};
	use sp_runtime::{
		offchain::{
			http,
			storage::{StorageRetrievalError, StorageValueRef},
			Duration,
		},
		traits::{AccountIdConversion, IdentifyAccount, Saturating, Zero},
		RuntimeAppPublic, SaturatedConversion,
	};

	/// Pot that receives request fees and, through the likes pallet, payments for unclaimed URLs.
	pub const PALLET_ID: PalletId = PalletId(*b"AUTHORS!");

	const FETCH_TIMEOUT_MS: u64 = 15_000;
	const MAX_PAGE_BYTES: usize = 2 * 1024 * 1024;
	const MAX_REDIRECTS: usize = 3;
	const REVEAL_QUEUE_PREFIX: &[u8] = b"ownr/r/";
	const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
		(KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 WikaVerifier/4.0";
	const ACCEPT_HEADER: &str = "text/html,application/xhtml+xml,application/xml";

	pub type BalanceOf<T> =
		<<T as Config>::Currency as Inspect<<T as frame_system::Config>::AccountId>>::Balance;
	type AppPublicOf<T> = <<T as Config>::AuthorityId as AppCrypto<
		<T as frame_system::offchain::SigningTypes>::Public,
		<T as frame_system::offchain::SigningTypes>::Signature,
	>>::RuntimeAppPublic;
	type GenericPublicOf<T> = <<T as Config>::AuthorityId as AppCrypto<
		<T as frame_system::offchain::SigningTypes>::Public,
		<T as frame_system::offchain::SigningTypes>::Signature,
	>>::GenericPublic;
	pub type UrlOf<T> = BoundedVec<u8, <T as Config>::MaxLengthURL>;
	pub type IntroOf = BoundedVec<u8, ConstU32<{ INTRO_LENGTH as u32 }>>;
	pub type MarkOf = BoundedVec<u8, ConstU32<{ MARK_LENGTH as u32 }>>;

	/// Verifier record:
	/// 0. block registered, 1. enabled,
	/// 2. commits, 3. total blocks taken to commit, 4. reveals, 5. total blocks taken to reveal,
	/// 6. votes in valid rounds, 7. yes votes in valid rounds, 8. votes that matched the majority.
	pub type VerifierOf<T> = (BlockNumberFor<T>, bool, u32, u32, u32, u32, u32, u32, u32);

	/// Tally: block, votes, yes votes, majority size, intro, mark, approved.
	pub type ResultOf<T> = (BlockNumberFor<T>, u32, u32, u32, IntroOf, MarkOf, bool);

	/// A reveal a verifier will send once the commit window has closed. Kept in offchain storage.
	#[derive(Encode, Decode, Clone, PartialEq, Debug)]
	pub struct RevealItem {
		pub url: Vec<u8>,
		pub vote: bool,
		pub intro: Vec<u8>,
		pub proof: Vec<u8>,
		pub salt: Vec<u8>,
	}

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: CreateSignedTransaction<Call<Self>> + frame_system::Config {
		/// The verifier key nodes sign commits and reveals with.
		type AuthorityId: AppCrypto<Self::Public, Self::Signature>;

		/// The currency request fees are paid in.
		type Currency: Mutate<Self::AccountId>;

		/// URLs must be strictly shorter than this many bytes.
		#[pallet::constant]
		type MaxLengthURL: Get<u32>;

		/// Most URL checks that can be requested in one block.
		#[pallet::constant]
		type MaxRequestsPerBlock: Get<u32>;
	}

	#[pallet::type_value]
	pub fn DefaultRequestPrice() -> u128 {
		5_000_000_000_000
	}
	#[pallet::type_value]
	pub fn DefaultFive() -> u8 {
		5
	}
	#[pallet::type_value]
	pub fn DefaultNumBlocksToDelete() -> u8 {
		100
	}
	#[pallet::type_value]
	pub fn DefaultPrctNeeded() -> u8 {
		66
	}
	#[pallet::type_value]
	pub fn DefaultMajorityMin() -> u32 {
		1
	}

	/// Number of URLs with a registered owner.
	#[pallet::storage]
	pub type UrlCount<T> = StorageValue<_, u128, ValueQuery>;

	/// Price of one URL check.
	#[pallet::storage]
	pub type RequestPrice<T> = StorageValue<_, u128, ValueQuery, DefaultRequestPrice>;

	/// Blocks after a request during which commits are accepted.
	#[pallet::storage]
	pub type NumBlocksToCommit<T> = StorageValue<_, u8, ValueQuery, DefaultFive>;

	/// Blocks after the commit window during which reveals are accepted.
	#[pallet::storage]
	pub type NumBlocksToReveal<T> = StorageValue<_, u8, ValueQuery, DefaultFive>;

	/// Blocks after the reveal window before a request's data is deleted.
	#[pallet::storage]
	pub type NumBlocksToDelete<T> = StorageValue<_, u8, ValueQuery, DefaultNumBlocksToDelete>;

	/// Share of identical reveals, in percent, that a round needs to count.
	#[pallet::storage]
	pub type PrctNeededForAgreement<T> = StorageValue<_, u8, ValueQuery, DefaultPrctNeeded>;

	/// Fewest verifiers that must agree to approve ownership.
	#[pallet::storage]
	pub type MajorityMin<T> = StorageValue<_, u32, ValueQuery, DefaultMajorityMin>;

	/// Registered verifiers.
	#[pallet::storage]
	pub type Verifiers<T: Config> =
		StorageMap<_, Blake2_128Concat, T::AccountId, VerifierOf<T>, OptionQuery>;

	/// URLs whose check was requested at each block.
	#[pallet::storage]
	pub type History<T: Config> = StorageMap<
		_,
		Twox64Concat,
		BlockNumberFor<T>,
		BoundedVec<UrlOf<T>, T::MaxRequestsPerBlock>,
		ValueQuery,
	>;

	/// Pending check per URL: the block it was requested at and the requester.
	#[pallet::storage]
	pub type Requests<T: Config> =
		StorageMap<_, Blake2_128Concat, UrlOf<T>, (BlockNumberFor<T>, T::AccountId), OptionQuery>;

	/// Commits: keccak-256 of `vote,intro[,proof],salt`.
	#[pallet::storage]
	pub type Commits<T: Config> = StorageDoubleMap<
		_,
		Blake2_128Concat,
		UrlOf<T>,
		Blake2_128Concat,
		T::AccountId,
		[u8; 32],
		OptionQuery,
	>;

	/// Reveals: vote, start of the page, mark found on the page.
	#[pallet::storage]
	pub type Reveals<T: Config> = StorageDoubleMap<
		_,
		Blake2_128Concat,
		UrlOf<T>,
		Blake2_128Concat,
		T::AccountId,
		(bool, IntroOf, MarkOf),
		OptionQuery,
	>;

	/// Outcome of each check.
	#[pallet::storage]
	pub type Results<T: Config> = StorageMap<_, Blake2_128Concat, UrlOf<T>, ResultOf<T>, OptionQuery>;

	/// Registered owner of each URL.
	#[pallet::storage]
	pub type Owners<T: Config> = StorageMap<_, Blake2_128Concat, UrlOf<T>, T::AccountId, OptionQuery>;

	#[pallet::genesis_config]
	#[derive(frame_support::DefaultNoBound)]
	pub struct GenesisConfig<T: Config> {
		/// Accounts that start out as enabled verifiers.
		pub verifiers: Vec<T::AccountId>,
	}

	#[pallet::genesis_build]
	impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
		fn build(&self) {
			for account in &self.verifiers {
				Verifiers::<T>::insert(account, Pallet::<T>::new_verifier(Zero::zero()));
			}
		}
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		VerifierAdded { account: T::AccountId },
		VerifierEnabled { account: T::AccountId },
		VerifierDisabled { account: T::AccountId },
		UrlCheckRequested { account: T::AccountId, url: UrlOf<T> },
		UrlCheckCommitted { account: T::AccountId, url: UrlOf<T> },
		UrlCheckRevealed { account: T::AccountId, url: UrlOf<T> },
		UrlOwnerRegistered { account: T::AccountId, url: UrlOf<T>, block: u32 },
	}

	#[pallet::error]
	pub enum Error<T> {
		UrlTooLong,
		NotEnoughBalanceToRequestUrlCheck,
		UrlCheckAlreadyInQueue,
		UrlCheckNotFound,
		OffTimeToCommit,
		OffTimeToReveal,
		ExpectedHashWith32Bytes,
		InvalidProofOfOwnership,
		MismatchBetweenCommitAndReveal,
		VerifierAlreadyRegistered,
		VerifierNotRegistered,
		OffchainSignedTxError,
		NoLocalAcctForSigning,
		InvalidSalt,
		CantRevealWithoutCommit,
		/// `MaxRequestsPerBlock` checks were already requested in this block.
		TooManyRequests,
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_initialize(now: BlockNumberFor<T>) -> Weight {
			let tallied = Self::aggregate_votes(now);
			let cleaned = Self::clean_up(now);
			let db = T::DbWeight::get();
			db.reads(4)
				.saturating_add(db.reads_writes(8, 4).saturating_mul(tallied.into()))
				.saturating_add(db.reads_writes(1, 5).saturating_mul(cleaned.into()))
		}

		fn offchain_worker(now: BlockNumberFor<T>) {
			let Some(verifier) = Self::local_verifier() else {
				log::trace!(target: "OWNERS", "no enabled verifier key on this node");
				return;
			};
			for url in History::<T>::get(now) {
				if let Some((requested_at, requester)) = Requests::<T>::get(&url) {
					Self::check_url_offchain(&verifier, &url, &requester, requested_at);
				}
			}
			Self::send_due_reveals(&verifier, now);
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Register a verifier.
		#[pallet::call_index(1)]
		#[pallet::weight(T::DbWeight::get().reads_writes(1, 1).saturating_add(Weight::from_parts(10_000_000, 0)))]
		pub fn add_verifier(origin: OriginFor<T>, account: T::AccountId) -> DispatchResult {
			ensure_root(origin)?;
			ensure!(!Verifiers::<T>::contains_key(&account), Error::<T>::VerifierAlreadyRegistered);
			let now = frame_system::Pallet::<T>::block_number();
			Verifiers::<T>::insert(&account, Self::new_verifier(now));
			Self::deposit_event(Event::VerifierAdded { account });
			Ok(())
		}

		/// Stop a verifier from voting.
		#[pallet::call_index(2)]
		#[pallet::weight(T::DbWeight::get().reads_writes(1, 1).saturating_add(Weight::from_parts(10_000_000, 0)))]
		pub fn disable_verifier(origin: OriginFor<T>, account: T::AccountId) -> DispatchResult {
			ensure_root(origin)?;
			Self::set_verifier_enabled(&account, false)?;
			Self::deposit_event(Event::VerifierDisabled { account });
			Ok(())
		}

		/// Let a disabled verifier vote again.
		#[pallet::call_index(3)]
		#[pallet::weight(T::DbWeight::get().reads_writes(1, 1).saturating_add(Weight::from_parts(10_000_000, 0)))]
		pub fn enable_verifier(origin: OriginFor<T>, account: T::AccountId) -> DispatchResult {
			ensure_root(origin)?;
			Self::set_verifier_enabled(&account, true)?;
			Self::deposit_event(Event::VerifierEnabled { account });
			Ok(())
		}

		/// Ask the verifiers to check that the caller owns `url`. Costs `RequestPrice`.
		#[pallet::call_index(4)]
		#[pallet::weight(T::DbWeight::get().reads_writes(5, 5).saturating_add(Weight::from_parts(40_000_000, 0)))]
		pub fn request_url_check(origin: OriginFor<T>, url: Vec<u8>) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			ensure!(url.len() < T::MaxLengthURL::get() as usize, Error::<T>::UrlTooLong);
			let url: UrlOf<T> = url.try_into().map_err(|_| Error::<T>::UrlTooLong)?;

			let price: BalanceOf<T> = RequestPrice::<T>::get().saturated_into();
			ensure!(
				T::Currency::balance(&sender) > price,
				Error::<T>::NotEnoughBalanceToRequestUrlCheck
			);
			ensure!(!Requests::<T>::contains_key(&url), Error::<T>::UrlCheckAlreadyInQueue);

			if !price.is_zero() {
				T::Currency::transfer(&sender, &Self::pot_id(), price, Preservation::Preserve)?;
			}
			let now = frame_system::Pallet::<T>::block_number();
			History::<T>::try_mutate(now, |urls| urls.try_push(url.clone()))
				.map_err(|_| Error::<T>::TooManyRequests)?;
			Requests::<T>::insert(&url, (now, sender.clone()));

			Self::deposit_event(Event::UrlCheckRequested { account: sender, url });
			Ok(())
		}

		/// A verifier's commitment to its vote: keccak-256 of `vote,intro[,proof],salt`.
		#[pallet::call_index(5)]
		#[pallet::weight(T::DbWeight::get().reads_writes(3, 2).saturating_add(Weight::from_parts(20_000_000, 0)))]
		pub fn commit_verification(
			origin: OriginFor<T>,
			url: Vec<u8>,
			hash: Vec<u8>,
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			let hash: [u8; HASH_LENGTH] =
				hash.try_into().map_err(|_| Error::<T>::ExpectedHashWith32Bytes)?;
			ensure!(Self::is_verifier_enabled(&sender), Error::<T>::VerifierNotRegistered);
			let url: UrlOf<T> = url.try_into().map_err(|_| Error::<T>::UrlCheckNotFound)?;
			let (requested_at, _) =
				Requests::<T>::get(&url).ok_or(Error::<T>::UrlCheckNotFound)?;

			let now = frame_system::Pallet::<T>::block_number();
			let commit_end = requested_at.saturating_add(NumBlocksToCommit::<T>::get().into());
			ensure!(now > requested_at && now <= commit_end, Error::<T>::OffTimeToCommit);

			Commits::<T>::insert(&url, &sender, hash);
			Verifiers::<T>::mutate(&sender, |v| {
				if let Some(v) = v {
					v.2.saturating_inc();
					v.3.saturating_accrue((now - requested_at).saturated_into());
				}
			});

			Self::deposit_event(Event::UrlCheckCommitted { account: sender, url });
			Ok(())
		}

		/// A verifier's vote, which must match its earlier commit.
		#[pallet::call_index(6)]
		#[pallet::weight(T::DbWeight::get().reads_writes(4, 2).saturating_add(Weight::from_parts(80_000_000, 0)))]
		pub fn reveal_verification(
			origin: OriginFor<T>,
			url: Vec<u8>,
			vote: bool,
			intro: Vec<u8>,
			proof: Vec<u8>,
			salt: Vec<u8>,
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			ensure!(Self::is_verifier_enabled(&sender), Error::<T>::VerifierNotRegistered);
			let url: UrlOf<T> = url.try_into().map_err(|_| Error::<T>::UrlCheckNotFound)?;
			let (requested_at, requester) =
				Requests::<T>::get(&url).ok_or(Error::<T>::UrlCheckNotFound)?;

			let proof = if vote { proof } else { Vec::new() };
			if vote {
				ensure!(
					mark_proves(&proof, &requester.encode()),
					Error::<T>::InvalidProofOfOwnership
				);
			}
			let intro_b: IntroOf =
				intro.clone().try_into().map_err(|_| Error::<T>::InvalidProofOfOwnership)?;
			let proof_b: MarkOf =
				proof.clone().try_into().map_err(|_| Error::<T>::InvalidProofOfOwnership)?;

			let now = frame_system::Pallet::<T>::block_number();
			let reveal_start = requested_at.saturating_add(NumBlocksToCommit::<T>::get().into());
			let reveal_end = reveal_start.saturating_add(NumBlocksToReveal::<T>::get().into());
			ensure!(now > reveal_start && now <= reveal_end, Error::<T>::OffTimeToReveal);

			let commit =
				Commits::<T>::get(&url, &sender).ok_or(Error::<T>::CantRevealWithoutCommit)?;

			// The salt must be the sender's signature over the vote.
			let signature = sp_core::sr25519::Signature::try_from(salt.as_slice())
				.map_err(|_| Error::<T>::InvalidSalt)?;
			let public = sp_core::sr25519::Public::try_from(sender.encode().as_slice())
				.map_err(|_| Error::<T>::InvalidSalt)?;
			ensure!(
				sp_io::crypto::sr25519_verify(
					&signature,
					&vote_message(vote, &intro, &proof),
					&public
				),
				Error::<T>::InvalidSalt
			);
			ensure!(
				sp_io::hashing::keccak_256(&commit_preimage(vote, &intro, &proof, &salt)) == commit,
				Error::<T>::MismatchBetweenCommitAndReveal
			);

			Reveals::<T>::insert(&url, &sender, (vote, intro_b, proof_b));
			Verifiers::<T>::mutate(&sender, |v| {
				if let Some(v) = v {
					v.4.saturating_inc();
					v.5.saturating_accrue((now - reveal_start).saturated_into());
				}
			});

			Self::deposit_event(Event::UrlCheckRevealed { account: sender, url });
			Ok(())
		}
	}

	impl<T: Config> wika_traits::OwnershipRegistry<T::AccountId> for Pallet<T> {
		fn pot_id() -> T::AccountId {
			Self::pot_id()
		}

		fn owner_of(url: &[u8]) -> T::AccountId {
			UrlOf::<T>::try_from(url.to_vec())
				.ok()
				.and_then(Owners::<T>::get)
				.unwrap_or_else(Self::pot_id)
		}
	}

	impl<T: Config> Pallet<T> {
		pub fn pot_id() -> T::AccountId {
			PALLET_ID.into_account_truncating()
		}

		fn new_verifier(now: BlockNumberFor<T>) -> VerifierOf<T> {
			(now, true, 0, 0, 0, 0, 0, 0, 0)
		}

		fn is_verifier_enabled(who: &T::AccountId) -> bool {
			Verifiers::<T>::get(who).is_some_and(|v| v.1)
		}

		fn set_verifier_enabled(who: &T::AccountId, enabled: bool) -> DispatchResult {
			Verifiers::<T>::try_mutate(who, |v| match v {
				Some(v) => {
					v.1 = enabled;
					Ok(())
				},
				None => Err(Error::<T>::VerifierNotRegistered.into()),
			})
		}

		fn block_offset(blocks: u8) -> BlockNumberFor<T> {
			blocks.into()
		}

		/// Tallies the requests whose reveal window closed in the previous block.
		fn aggregate_votes(now: BlockNumberFor<T>) -> u32 {
			let delta = Self::block_offset(NumBlocksToCommit::<T>::get())
				.saturating_add(Self::block_offset(NumBlocksToReveal::<T>::get()))
				.saturating_add(1u32.into());
			if now <= delta {
				return 0;
			}
			let urls = History::<T>::get(now - delta);
			for url in &urls {
				Self::aggregate_votes_for_request(now, url);
			}
			urls.len() as u32
		}

		fn aggregate_votes_for_request(now: BlockNumberFor<T>, url: &UrlOf<T>) {
			let reveals: Vec<(T::AccountId, (bool, IntroOf, MarkOf))> =
				Reveals::<T>::iter_prefix(url).collect();
			let total = reveals.len() as u32;
			if total == 0 {
				return;
			}

			let mut counts = BTreeMap::new();
			for (_, reveal) in &reveals {
				*counts.entry(reveal).or_insert(0u32) += 1;
			}
			let Some((majority, count_majority)) =
				counts.into_iter().max_by(|a, b| a.1.cmp(&b.1))
			else {
				return;
			};
			let prct = count_majority * 100 / total;
			let count_yes = reveals.iter().filter(|(_, r)| r.0).count() as u32;

			let bar = u32::from(PrctNeededForAgreement::<T>::get());
			let agreed = prct > bar;
			let approved = majority.0 && agreed && count_majority >= MajorityMin::<T>::get();
			Results::<T>::insert(
				url,
				(now, total, count_yes, count_majority, majority.1.clone(), majority.2.clone(), approved),
			);

			// Votes only count toward verifier stats when the round reached agreement.
			if agreed {
				for (account, reveal) in &reveals {
					Verifiers::<T>::mutate(account, |v| {
						if let Some(v) = v {
							v.6.saturating_inc();
							if reveal.0 {
								v.7.saturating_inc();
							}
							if reveal == majority {
								v.8.saturating_inc();
							}
						}
					});
				}
			}

			if approved {
				if let Some((_, owner)) = Requests::<T>::get(url) {
					if Owners::<T>::get(url).is_none() {
						UrlCount::<T>::mutate(|n| *n = n.saturating_add(1));
					}
					Owners::<T>::insert(url, &owner);
					Self::deposit_event(Event::UrlOwnerRegistered {
						account: owner,
						url: url.clone(),
						block: now.saturated_into(),
					});
				}
			}
		}

		/// Deletes the request data of checks that finished `NumBlocksToDelete` blocks ago.
		fn clean_up(now: BlockNumberFor<T>) -> u32 {
			let delta = Self::block_offset(NumBlocksToCommit::<T>::get())
				.saturating_add(Self::block_offset(NumBlocksToReveal::<T>::get()))
				.saturating_add(Self::block_offset(NumBlocksToDelete::<T>::get()))
				.saturating_add(1u32.into());
			if now <= delta {
				return 0;
			}
			let urls = History::<T>::take(now - delta);
			for url in &urls {
				Requests::<T>::remove(url);
				let _ = Commits::<T>::clear_prefix(url, u32::MAX, None);
				let _ = Reveals::<T>::clear_prefix(url, u32::MAX, None);
				Results::<T>::remove(url);
			}
			urls.len() as u32
		}

		// ---- Offchain worker ------------------------------------------------------------

		/// The first `ownr` key in this node's keystore that belongs to an enabled verifier.
		fn local_verifier() -> Option<T::Public> {
			AppPublicOf::<T>::all().into_iter().find_map(|key| {
				let generic: GenericPublicOf<T> = key.into();
				let public: T::Public = generic.into();
				Self::is_verifier_enabled(&public.clone().into_account()).then_some(public)
			})
		}

		fn sign_as(verifier: &T::Public, message: &[u8]) -> Option<Vec<u8>> {
			let generic = GenericPublicOf::<T>::try_from(verifier.clone()).ok()?;
			let key: AppPublicOf<T> = generic.into();
			key.sign(&message).map(|signature| signature.encode())
		}

		fn signer_for(verifier: &T::Public) -> Signer<T, T::AuthorityId, frame_system::offchain::ForAll> {
			Signer::<T, T::AuthorityId>::all_accounts().with_filter(alloc::vec![verifier.clone()])
		}

		fn check_url_offchain(
			verifier: &T::Public,
			url: &UrlOf<T>,
			requester: &T::AccountId,
			requested_at: BlockNumberFor<T>,
		) {
			let now = frame_system::Pallet::<T>::block_number();
			let commit_end =
				requested_at.saturating_add(Self::block_offset(NumBlocksToCommit::<T>::get()));
			if now >= commit_end {
				log::debug!(target: "OWNERS", "too late to commit for {:?}", url);
				return;
			}
			let Some(page) = fetch(url) else {
				// No vote: the page could not be read, which says nothing about ownership.
				return;
			};
			let (vote, intro, proof) = inspect_page(&page, &requester.encode());
			log::debug!(target: "OWNERS", "voting {} on {:?}", vote, url);

			let Some(salt) = Self::sign_as(verifier, &vote_message(vote, &intro, &proof)) else {
				log::error!(target: "OWNERS", "could not sign with the verifier key");
				return;
			};
			let hash = sp_io::hashing::keccak_256(&commit_preimage(vote, &intro, &proof, &salt));

			let results = Self::signer_for(verifier).send_signed_transaction(|_| {
				Call::commit_verification { url: url.to_vec(), hash: hash.to_vec() }
			});
			if results.iter().any(|(_, r)| r.is_ok()) {
				let item = RevealItem { url: url.to_vec(), vote, intro, proof, salt };
				queue_reveal(commit_end.saturated_into(), item);
			} else {
				log::error!(target: "OWNERS", "commit for {:?} was not submitted", url);
			}
		}

		fn send_due_reveals(
			verifier: &T::Public,
			now: BlockNumberFor<T>,
		) {
			let block: u32 = now.saturated_into();
			let items = take_reveals(block);
			for item in items {
				let Ok(url) = UrlOf::<T>::try_from(item.url.clone()) else { continue };
				let Some((requested_at, _)) = Requests::<T>::get(&url) else { continue };
				let reveal_start =
					requested_at.saturating_add(Self::block_offset(NumBlocksToCommit::<T>::get()));
				let reveal_end =
					reveal_start.saturating_add(Self::block_offset(NumBlocksToReveal::<T>::get()));
				// The transaction lands in a later block, so `now == reveal_start` is in time.
				if now < reveal_start || now >= reveal_end {
					log::debug!(target: "OWNERS", "outside the reveal window for {:?}", url);
					continue;
				}
				let results = Self::signer_for(verifier).send_signed_transaction(|_| {
					Call::reveal_verification {
						url: item.url.clone(),
						vote: item.vote,
						intro: item.intro.clone(),
						proof: item.proof.clone(),
						salt: item.salt.clone(),
					}
				});
				if !results.iter().any(|(_, r)| r.is_ok()) {
					log::error!(target: "OWNERS", "reveal for {:?} was not submitted", url);
				}
			}
		}
	}

	fn reveal_key(block: u32) -> Vec<u8> {
		let mut key = REVEAL_QUEUE_PREFIX.to_vec();
		key.extend_from_slice(&block.to_le_bytes());
		key
	}

	fn queue_reveal(block: u32, item: RevealItem) {
		let key = reveal_key(block);
		let store = StorageValueRef::persistent(&key);
		let res = store.mutate(
			|existing: Result<Option<Vec<RevealItem>>, StorageRetrievalError>| -> Result<_, ()> {
				let mut list = existing.ok().flatten().unwrap_or_default();
				list.push(item);
				Ok(list)
			},
		);
		if res.is_err() {
			log::error!(target: "OWNERS", "could not queue a reveal for block {}", block);
		}
	}

	fn take_reveals(block: u32) -> Vec<RevealItem> {
		let key = reveal_key(block);
		let mut store = StorageValueRef::persistent(&key);
		let items = store.get::<Vec<RevealItem>>().ok().flatten().unwrap_or_default();
		store.clear();
		items
	}

	/// GETs `url`, following up to `MAX_REDIRECTS` redirects. Returns the body of a 200 response.
	fn fetch(url: &[u8]) -> Option<Vec<u8>> {
		let mut current = alloc::string::String::from(core::str::from_utf8(url).ok()?);
		let deadline = sp_io::offchain::timestamp().add(Duration::from_millis(FETCH_TIMEOUT_MS));
		for _ in 0..=MAX_REDIRECTS {
			let pending = http::Request::get(&current)
				.add_header("Accept", ACCEPT_HEADER)
				.add_header("User-Agent", USER_AGENT)
				.deadline(deadline)
				.send()
				.map_err(|e| log::debug!(target: "OWNERS", "request failed: {:?}", e))
				.ok()?;
			let mut response = pending
				.try_wait(deadline)
				.map_err(|_| log::debug!(target: "OWNERS", "request timed out"))
				.ok()?
				.map_err(|e| log::debug!(target: "OWNERS", "request failed: {:?}", e))
				.ok()?;
			match response.code {
				200 => return Some(response.body().take(MAX_PAGE_BYTES).collect()),
				301 | 302 | 303 | 307 | 308 => {
					let location = response.headers().find("location")?;
					current = resolve_location(&current, location)?;
				},
				code => {
					log::debug!(target: "OWNERS", "{} answered {}", current, code);
					return None;
				},
			}
		}
		None
	}

	/// Resolves a `Location` header against the URL that returned it.
	fn resolve_location(base: &str, location: &str) -> Option<alloc::string::String> {
		use alloc::string::ToString;
		if location.starts_with("http://") || location.starts_with("https://") {
			return Some(location.to_string());
		}
		let scheme_end = base.find("://")? + 3;
		let origin_end = base[scheme_end..].find('/').map_or(base.len(), |i| scheme_end + i);
		if let Some(rest) = location.strip_prefix("//") {
			return Some(alloc::format!("{}{}", &base[..scheme_end], rest));
		}
		if location.starts_with('/') {
			return Some(alloc::format!("{}{}", &base[..origin_end], location));
		}
		let dir_end = base.rfind('/').filter(|&i| i >= origin_end).map_or(origin_end, |i| i);
		Some(alloc::format!("{}/{}", &base[..dir_end], location))
	}

	#[cfg(test)]
	pub(crate) fn resolve_location_for_test(base: &str, location: &str) -> Option<alloc::string::String> {
		resolve_location(base, location)
	}
}

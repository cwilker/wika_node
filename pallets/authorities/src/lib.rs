//! # Authorities pallet
//!
//! A registry of block authors (Aura, sr25519) and finality voters (GRANDPA, ed25519) that sudo
//! can add to, enable and disable. Every change is pushed to `pallet_aura` straight away and
//! scheduled in `pallet_grandpa` for the next block, so the node picks it up without a restart.

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
	use codec::Decode;
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;
	use sp_consensus_aura::sr25519::AuthorityId as AuraId;
	use sp_consensus_grandpa::AuthorityId as GrandpaId;
	use sp_runtime::traits::Zero;

	/// Block registered, enabled, sr25519 (Aura) public key, ed25519 (GRANDPA) public key.
	pub type AuthorityOf<T> = (BlockNumberFor<T>, bool, [u8; 32], [u8; 32]);

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config:
		frame_system::Config + pallet_aura::Config<AuthorityId = AuraId> + pallet_grandpa::Config
	{
	}

	/// Number of registered authorities, enabled or not.
	#[pallet::storage]
	pub type AuthCount<T> = StorageValue<_, u16, ValueQuery>;

	/// Registered authorities.
	#[pallet::storage]
	pub type Authorities<T: Config> =
		StorageMap<_, Blake2_128Concat, T::AccountId, AuthorityOf<T>, OptionQuery>;

	#[pallet::genesis_config]
	#[derive(frame_support::DefaultNoBound)]
	pub struct GenesisConfig<T: Config> {
		/// (sr25519, ed25519) key pairs. The account is the sr25519 key. Aura and GRANDPA get
		/// their genesis authorities from their own configs, which should list the same keys.
		pub keys: Vec<([u8; 32], [u8; 32])>,
		#[serde(skip)]
		pub _marker: core::marker::PhantomData<T>,
	}

	#[pallet::genesis_build]
	impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
		fn build(&self) {
			for (sr25519, ed25519) in &self.keys {
				let account =
					T::AccountId::decode(&mut &sr25519[..]).expect("an sr25519 key is a valid account");
				Authorities::<T>::insert(&account, (BlockNumberFor::<T>::zero(), true, *sr25519, *ed25519));
			}
			AuthCount::<T>::put(self.keys.len() as u16);
		}
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		AuthorityAdded { account: T::AccountId },
		AuthorityEnabled { account: T::AccountId },
		AuthorityDisabled { account: T::AccountId },
	}

	#[pallet::error]
	pub enum Error<T> {
		AuthorityAlreadyRegistered,
		AuthorityNotRegistered,
		InvalidAddress,
		/// More enabled authorities than Aura or GRANDPA allow.
		TooManyAuthorities,
		/// The change would leave no enabled authority, which halts the chain.
		NoAuthoritiesLeft,
		/// GRANDPA already has a pending authority change; try again in a few blocks.
		ChangePending,
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		#[pallet::call_index(0)]
		#[pallet::weight(T::DbWeight::get().reads_writes(40, 6).saturating_add(Weight::from_parts(50_000_000, 0)))]
		pub fn add_authority(
			origin: OriginFor<T>,
			account: T::AccountId,
			addr_sr25519: [u8; 32],
			addr_ed25519: [u8; 32],
		) -> DispatchResult {
			ensure_root(origin)?;
			ensure!(!Authorities::<T>::contains_key(&account), Error::<T>::AuthorityAlreadyRegistered);
			let now = frame_system::Pallet::<T>::block_number();
			Authorities::<T>::insert(&account, (now, true, addr_sr25519, addr_ed25519));
			AuthCount::<T>::mutate(|n| *n = n.saturating_add(1));
			Self::apply()?;
			Self::deposit_event(Event::AuthorityAdded { account });
			Ok(())
		}

		#[pallet::call_index(1)]
		#[pallet::weight(T::DbWeight::get().reads_writes(40, 6).saturating_add(Weight::from_parts(50_000_000, 0)))]
		pub fn disable_authority(origin: OriginFor<T>, account: T::AccountId) -> DispatchResult {
			ensure_root(origin)?;
			Self::set_enabled(&account, false)?;
			Self::deposit_event(Event::AuthorityDisabled { account });
			Ok(())
		}

		#[pallet::call_index(2)]
		#[pallet::weight(T::DbWeight::get().reads_writes(40, 6).saturating_add(Weight::from_parts(50_000_000, 0)))]
		pub fn enable_authority(origin: OriginFor<T>, account: T::AccountId) -> DispatchResult {
			ensure_root(origin)?;
			Self::set_enabled(&account, true)?;
			Self::deposit_event(Event::AuthorityEnabled { account });
			Ok(())
		}
	}

	impl<T: Config> Pallet<T> {
		fn set_enabled(account: &T::AccountId, enabled: bool) -> DispatchResult {
			Authorities::<T>::try_mutate(account, |a| match a {
				Some(a) => {
					a.1 = enabled;
					Ok(())
				},
				None => Err(DispatchError::from(Error::<T>::AuthorityNotRegistered)),
			})?;
			Self::apply()
		}

		/// Enabled authorities in a stable order.
		pub fn enabled() -> Vec<([u8; 32], [u8; 32])> {
			let mut keys: Vec<_> = Authorities::<T>::iter_values()
				.filter(|a| a.1)
				.map(|(_, _, sr, ed)| (sr, ed))
				.collect();
			keys.sort();
			keys
		}

		/// Pushes the enabled set to Aura now and to GRANDPA from the next block.
		fn apply() -> DispatchResult {
			let keys = Self::enabled();
			ensure!(!keys.is_empty(), Error::<T>::NoAuthoritiesLeft);

			let aura: Vec<AuraId> =
				keys.iter().map(|(sr, _)| sp_core::sr25519::Public::from_raw(*sr).into()).collect();
			let aura = BoundedVec::<_, <T as pallet_aura::Config>::MaxAuthorities>::try_from(aura)
				.map_err(|_| Error::<T>::TooManyAuthorities)?;
			let grandpa: Vec<(GrandpaId, u64)> = keys
				.iter()
				.map(|(_, ed)| (sp_core::ed25519::Public::from_raw(*ed).into(), 1))
				.collect();
			ensure!(
				grandpa.len() as u32 <= <T as pallet_grandpa::Config>::MaxAuthorities::get(),
				Error::<T>::TooManyAuthorities
			);

			pallet_grandpa::Pallet::<T>::schedule_change(grandpa, Zero::zero(), None)
				.map_err(|_| Error::<T>::ChangePending)?;
			pallet_aura::Pallet::<T>::change_authorities(aura);
			Ok(())
		}
	}
}

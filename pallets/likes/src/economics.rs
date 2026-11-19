//! Versioned page policies. No release numbers or activation height are selected implicitly.
use crate::*;
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use frame_support::{
	ensure,
	pallet_prelude::*,
	traits::{
		fungible::{Inspect, Mutate},
		tokens::Preservation,
	},
};
use frame_system::pallet_prelude::*;
use scale_info::TypeInfo;
use sp_runtime::{
	traits::{SaturatedConversion, Zero},
	DispatchError, DispatchResult,
};
use wika_traits::{OwnershipRegistry, PolicyOutcomeReason, PolicySelection};

#[derive(
	Encode, Decode, DecodeWithMemTracking, Clone, PartialEq, Eq, Debug, TypeInfo, MaxEncodedLen,
)]
pub struct Policy<AccountId> {
	pub ruleset: u32,
	pub revision: u64,
	pub model: u8, // 0: frozen legacy; 1: exact-budget FIFO, target referral fallback
	pub owner_bps: u16,
	pub supporter_bps: u16,
	pub referral_bps: u16,
	pub protocol_bps: u16,
	pub rounds: u8,
	pub treasury: AccountId,
	pub configurator: Option<AccountId>,
	pub owner_period: Option<u64>,
}
#[derive(
	Encode, Decode, DecodeWithMemTracking, Clone, PartialEq, Eq, Debug, TypeInfo, MaxEncodedLen,
)]
pub struct Rules<AccountId> {
	pub version: u32,
	pub owner_min: u16,
	pub owner_max: u16,
	pub supporter_min: u16,
	pub supporter_max: u16,
	pub referral_min: u16,
	pub referral_max: u16,
	pub protocol_bps: u16,
	pub minimum_price: u128,
	pub treasury: AccountId,
}
#[derive(
	Encode, Decode, DecodeWithMemTracking, Clone, PartialEq, Eq, Debug, TypeInfo, MaxEncodedLen,
)]
pub struct PageEconomics<AccountId, BlockNumber> {
	pub active: bool,
	pub policy: Policy<AccountId>,
	pub policy_hash: [u8; 32],
	pub locked_at: Option<BlockNumber>,
	pub legacy: bool,
}
#[derive(
	Encode, Decode, DecodeWithMemTracking, Clone, PartialEq, Eq, Debug, TypeInfo, MaxEncodedLen,
)]
pub struct EconomicTerms<AccountId> {
	pub version: u16,
	pub genesis_hash: [u8; 32],
	pub payer: AccountId,
	pub target: referrals::Page,
	pub source: referrals::Page,
	pub source_period: u64,
	pub authorization_id: u64,
	pub count: u32,
	pub price: u128,
	pub cost: u128,
	pub recipient: AccountId,
	pub referral_amount: u128,
	pub target_owner: AccountId,
	pub target_period: Option<u64>,
	pub policy_hash: [u8; 32],
	pub policy: Policy<AccountId>,
	pub owner_amount: u128,
	pub supporter_budget: u128,
	pub protocol_amount: u128,
	pub referral_to_target: bool,
	pub retained_owner: u128,
}
#[derive(
	Encode, Decode, DecodeWithMemTracking, Clone, PartialEq, Eq, Debug, TypeInfo, MaxEncodedLen,
)]
pub struct Distribution<AccountId> {
	pub version: u16,
	pub policy_hash: [u8; 32],
	pub revision: u64,
	pub model: u8,
	pub gross: u128,
	pub debit: u128,
	pub retained: u128,
	pub legacy_dust: u128,
	pub owner_credit: u128,
	pub supporter_budget: u128,
	pub supporter_paid: u128,
	pub supporter_fallback: u128,
	pub referral_recipient: AccountId,
	pub referral_credit: u128,
	pub referral_to_target: bool,
	pub protocol_recipient: AccountId,
	pub protocol_credit: u128,
}
sp_api::decl_runtime_apis! {
	pub trait EconomicsApi<AccountId, BlockNumber> where AccountId: codec::Codec, BlockNumber: codec::Codec {
		fn page(target: referrals::Page) -> Result<PageEconomics<AccountId, BlockNumber>, DispatchError>;
		fn quote(payer: AccountId, target: referrals::Page, source: referrals::Page, count: u32, authorization_id: u64) -> Result<EconomicTerms<AccountId>, DispatchError>;
	}
}
impl<T: Config> Pallet<T> {
	pub fn economics_active() -> bool {
		EconomicsActive::<T>::get()
	}
	pub fn policy_hash(page: &[u8], policy: &Policy<T::AccountId>) -> [u8; 32] {
		sp_io::hashing::blake2_256(&(b"WIKA_PAGE_POLICY_V1", page, policy).encode())
	}
	pub fn validate_split(r: &Rules<T::AccountId>, a: u16, s: u16, f: u16) -> DispatchResult {
		ensure!(
			r.version > 0
				&& r.owner_min <= a
				&& a <= r.owner_max
				&& r.supporter_min <= s
				&& s <= r.supporter_max
				&& r.referral_min > 0
				&& r.referral_min <= f
				&& f <= r.referral_max
				&& u32::from(a) + u32::from(s) + u32::from(f) + u32::from(r.protocol_bps) == 10000
				&& r.minimum_price > 0
				&& Self::fraction(r.minimum_price, r.referral_min)? > 0,
			Error::<T>::InvalidPolicy
		);
		Ok(())
	}
	// Quotient/remainder avoids overflowing u128 even at the currency maximum.
	pub fn fraction(cost: u128, bps: u16) -> Result<u128, DispatchError> {
		ensure!(bps <= 10000, Error::<T>::InvalidPolicy);
		Ok((cost / 10000) * u128::from(bps) + (cost % 10000) * u128::from(bps) / 10000)
	}
	pub fn legacy_policy() -> Policy<T::AccountId> {
		LegacyProfile::<T>::get().unwrap_or_else(|| Policy {
			ruleset: 0,
			revision: 0,
			model: 0,
			owner_bps: u16::from(ShareAuthor::<T>::get()) * 100,
			supporter_bps: u16::from(SharePreviousLikers::<T>::get()) * 100,
			referral_bps: u16::from(ShareReferrer::<T>::get()) * 100,
			protocol_bps: u16::from(
				100u8.saturating_sub(
					ShareAuthor::<T>::get()
						.saturating_add(SharePreviousLikers::<T>::get())
						.saturating_add(ShareReferrer::<T>::get()),
				),
			) * 100,
			rounds: NumRoundsToRewardLikers::<T>::get(),
			treasury: Self::pot_id(),
			configurator: None,
			owner_period: None,
		})
	}
	pub fn page_economics(
		page: &[u8],
	) -> Result<PageEconomics<T::AccountId, BlockNumberFor<T>>, DispatchError> {
		ensure!(
			wika_traits::page_identity::canonicalize(page).is_some_and(|p| p == page),
			Error::<T>::InvalidUrl
		);
		let key: UrlOf<T> = page
			.to_vec()
			.try_into()
			.map_err(|_| Error::<T>::UrlTooLong)?;
		let active = Self::economics_active();
		let legacy =
			!active || (!PagePolicies::<T>::contains_key(&key) && Urls::<T>::contains_key(&key));
		let policy = if legacy {
			Self::legacy_policy()
		} else {
			PagePolicies::<T>::get(&key)
				.or_else(DefaultProfile::<T>::get)
				.ok_or(Error::<T>::EconomicsNotActive)?
		};
		ensure!(matches!(policy.model, 0 | 1), Error::<T>::InvalidPolicy);
		Ok(PageEconomics {
			active,
			policy_hash: Self::policy_hash(page, &policy),
			policy,
			locked_at: PolicyLocks::<T>::get(&key).or_else(|| {
				if legacy && Urls::<T>::contains_key(&key) {
					Some(Zero::zero())
				} else {
					None
				}
			}),
			legacy,
		})
	}
	pub fn validate_selection(page: &[u8], selection: &PolicySelection) -> DispatchResult {
		ensure!(Self::economics_active(), Error::<T>::EconomicsNotActive);
		let state = Self::page_economics(page)?;
		ensure!(
			state.locked_at.is_none() && !state.legacy,
			Error::<T>::PolicyLocked
		);
		let rules = EconomicRules::<T>::get().ok_or(Error::<T>::EconomicsNotActive)?;
		ensure!(
			selection.ruleset == rules.version
				&& selection.expected_revision == state.policy.revision
				&& selection.expected_hash == state.policy_hash,
			Error::<T>::TermsChanged
		);
		Self::validate_split(
			&rules,
			selection.owner_bps,
			selection.supporter_bps,
			selection.referral_bps,
		)
	}
	pub fn install_policy(
		page: &[u8],
		owner: &T::AccountId,
		period: u64,
		selection: &PolicySelection,
	) -> DispatchResult {
		Self::validate_selection(page, selection)?;
		let mut policy = Self::page_economics(page)?.policy;
		policy.revision = policy
			.revision
			.checked_add(1)
			.ok_or(Error::<T>::EconomicOverflow)?;
		policy.owner_bps = selection.owner_bps;
		policy.supporter_bps = selection.supporter_bps;
		policy.referral_bps = selection.referral_bps;
		policy.configurator = Some(owner.clone());
		policy.owner_period = Some(period);
		let hash = Self::policy_hash(page, &policy);
		let key: UrlOf<T> = page
			.to_vec()
			.try_into()
			.map_err(|_| Error::<T>::UrlTooLong)?;
		PagePolicies::<T>::insert(&key, &policy);
		Self::deposit_event(Event::PolicyConfigured {
			page: key,
			policy_hash: hash,
			policy,
		});
		Ok(())
	}
	pub fn economic_terms(
		payer: &T::AccountId,
		target: &[u8],
		source: &[u8],
		count: u32,
		authorization_id: u64,
	) -> Result<EconomicTerms<T::AccountId>, DispatchError> {
		ensure!(Self::economics_active(), Error::<T>::EconomicsNotActive);
		ensure!(count > 0, Error::<T>::NoLikes);
		ensure!(
			count <= MaxLikes::<T>::get() && count <= DEFAULT_MAX_LIKES,
			Error::<T>::TooManyLikes
		);
		let state = Self::page_economics(target)?;
		let p = &state.policy;
		ensure!(
			*payer != Self::pot_id()
				&& *payer != p.treasury
				&& *payer != T::OwnershipRegistry::pot_id(),
			Error::<T>::InvalidEconomicPayer
		);
		let price = LikePrice::<T>::get();
		let cost = price
			.checked_mul(u128::from(count))
			.ok_or(Error::<T>::EconomicOverflow)?;
		if p.model == 1 {
			let rules = EconomicRules::<T>::get().ok_or(Error::<T>::EconomicsNotActive)?;
			ensure!(
				price >= rules.minimum_price && Self::fraction(price, p.referral_bps)? > 0,
				Error::<T>::InvalidPolicy
			);
		}
		let (source_owner, source_period) = if source.is_empty() {
			(T::OwnershipRegistry::pot_id(), 0)
		} else {
			Self::canonical_host(source)?;
			let (owner, period) = T::OwnershipRegistry::referral_owner(source)
				.ok_or(Error::<T>::HostNotAuthorized)?;
			let key: UrlOf<T> = source
				.to_vec()
				.try_into()
				.map_err(|_| Error::<T>::UrlTooLong)?;
			ensure!(
				HostKeys::<T>::get(key)
					.iter()
					.any(|(p, id, _)| *p == period && *id == authorization_id),
				Error::<T>::HostNotAuthorized
			);
			(owner, period)
		};
		let target_owner = T::OwnershipRegistry::owner_of(target);
		let referral_to_target = p.model == 1 && (source.is_empty() || source_owner == *payer);
		let recipient = if referral_to_target {
			target_owner.clone()
		} else if source_owner == *payer {
			T::OwnershipRegistry::pot_id()
		} else {
			source_owner
		};
		let owner_amount = Self::fraction(cost, p.owner_bps)?;
		let supporter_budget = Self::fraction(cost, p.supporter_bps)?;
		let referral_amount = Self::fraction(cost, p.referral_bps)?;
		let protocol_amount = if p.model == 1 {
			cost - owner_amount - supporter_budget - referral_amount
		} else {
			Self::fraction(cost, p.protocol_bps)?
		};
		let retained_owner = if target_owner == *payer {
			owner_amount
				+ if referral_to_target {
					referral_amount
				} else {
					0
				}
		} else {
			0
		};
		Ok(EconomicTerms {
			version: 2,
			genesis_hash: frame_system::Pallet::<T>::block_hash(BlockNumberFor::<T>::zero())
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
			source_period,
			authorization_id,
			count,
			price,
			cost,
			recipient,
			referral_amount,
			target_owner,
			target_period: T::OwnershipRegistry::ownership_period(target),
			policy_hash: state.policy_hash,
			policy: state.policy,
			owner_amount,
			supporter_budget,
			protocol_amount,
			referral_to_target,
			retained_owner,
		})
	}
	pub fn execute_economic(
		sender: T::AccountId,
		terms: &EconomicTerms<T::AccountId>,
		terms_hash: [u8; 32],
	) -> DispatchResult {
		let key: UrlOf<T> = terms
			.target
			.to_vec()
			.try_into()
			.map_err(|_| Error::<T>::UrlTooLong)?;
		ensure!(
			!Likes::<T>::contains_key(&sender, &key),
			Error::<T>::AlreadyInQueue
		);
		let before: u128 = T::Currency::balance(&sender).saturated_into();
		ensure!(before > terms.cost, Error::<T>::NotEnoughBalanceToLike);
		let state = Self::page_economics(&key)?;
		let mut receipt = Distribution {
			version: 2,
			policy_hash: terms.policy_hash,
			revision: terms.policy.revision,
			model: terms.policy.model,
			gross: terms.cost,
			debit: 0,
			retained: terms.retained_owner,
			legacy_dust: 0,
			owner_credit: terms.owner_amount
				- if terms.target_owner == sender {
					terms.owner_amount
				} else {
					0
				},
			supporter_budget: terms.supporter_budget,
			supporter_paid: 0,
			supporter_fallback: 0,
			referral_recipient: terms.recipient.clone(),
			referral_credit: terms.referral_amount,
			referral_to_target: terms.referral_to_target,
			protocol_recipient: terms.policy.treasury.clone(),
			protocol_credit: terms.protocol_amount,
		};
		if terms.policy.model == 0 {
			// Legacy queues and rounding stay byte-for-byte on the old payment path.
			let mut remaining = terms.count;
			let mut recipient = Urls::<T>::get(&key)
				.map(|s| s.1)
				.unwrap_or_else(Self::pot_id);
			for _ in 0..DEFAULT_MAX_LIKES {
				if remaining == 0 || recipient == Self::pot_id() {
					break;
				}
				let (_, _, owed, next) =
					Likes::<T>::get(&recipient, &key).ok_or(Error::<T>::InvalidQueue)?;
				ensure!(owed > 0, Error::<T>::InvalidQueue);
				let n = owed.min(remaining);
				let amount = Self::fraction(
					terms
						.price
						.checked_mul(n.into())
						.ok_or(Error::<T>::EconomicOverflow)?,
					terms.policy.supporter_bps,
				)?;
				receipt.supporter_paid = receipt
					.supporter_paid
					.checked_add(amount)
					.ok_or(Error::<T>::EconomicOverflow)?;
				remaining -= n;
				recipient = next;
			}
			receipt.supporter_fallback = Self::fraction(
				terms
					.price
					.checked_mul(remaining.into())
					.ok_or(Error::<T>::EconomicOverflow)?,
				terms.policy.supporter_bps,
			)?;
			Self::execute_like(
				sender.clone(),
				key.to_vec(),
				terms.source.to_vec(),
				terms.count,
			)?;
		} else {
			if state.locked_at.is_none() {
				PagePolicies::<T>::insert(&key, &terms.policy);
				PolicyLocks::<T>::insert(&key, frame_system::Pallet::<T>::block_number());
				Self::deposit_event(Event::PolicyConfigured {
					page: key.clone(),
					policy_hash: terms.policy_hash,
					policy: terms.policy.clone(),
				});
				Self::deposit_event(Event::PolicyLocked {
					page: key.clone(),
					policy_hash: terms.policy_hash,
					revision: terms.policy.revision,
				});
			}
			let author = terms
				.owner_amount
				.checked_add(if terms.referral_to_target {
					terms.referral_amount
				} else {
					0
				})
				.ok_or(Error::<T>::EconomicOverflow)?;
			Self::author_amount(&sender, &key, author)?;
			if terms.referral_to_target {
				receipt.referral_credit = if terms.target_owner == sender {
					0
				} else {
					terms.referral_amount
				};
			} else {
				Self::transfer_amount(&sender, &terms.recipient, terms.referral_amount)?;
			}
			Self::transfer_amount(&sender, &terms.policy.treasury, terms.protocol_amount)?;
			let pot = Self::pot_id();
			let existing = Urls::<T>::get(&key);
			let mut first = existing
				.as_ref()
				.map(|s| s.1.clone())
				.unwrap_or_else(|| pot.clone());
			let mut remaining = terms.count;
			let mut allocated = 0u128;
			for _ in 0..DEFAULT_MAX_LIKES {
				if remaining == 0 || first == pot {
					break;
				}
				let (before, count, owed, next) =
					Likes::<T>::get(&first, &key).ok_or(Error::<T>::InvalidQueue)?;
				ensure!(owed > 0, Error::<T>::InvalidQueue);
				let units = remaining.min(owed);
				remaining -= units;
				let cumulative = terms.count - remaining;
				let end = Self::proportion(terms.supporter_budget, cumulative, terms.count);
				let amount = end - allocated;
				allocated = end;
				Self::transfer_amount(&sender, &first, amount)?;
				receipt.supporter_paid += amount;
				Self::deposit_event(Event::SupporterPaid {
					page: key.clone(),
					payer: sender.clone(),
					recipient: first.clone(),
					amount,
				});
				Likes::<T>::insert(&first, &key, (before, count, owed - units, next.clone()));
				if owed == units {
					first = next;
				}
			}
			ensure!(remaining == 0 || first == pot, Error::<T>::InvalidQueue);
			receipt.supporter_fallback = terms.supporter_budget - allocated;
			Self::transfer_amount(&sender, &pot, receipt.supporter_fallback)?;
			let total = if let Some((total, old_first, last)) = existing {
				Self::add_to_queue(&sender, &key, old_first, last);
				total
					.checked_add(terms.count.into())
					.ok_or(Error::<T>::EconomicOverflow)?
			} else {
				UrlCount::<T>::mutate(|n| *n = n.saturating_add(1));
				u64::from(terms.count)
			};
			let owed = terms
				.count
				.checked_mul(terms.policy.rounds.into())
				.ok_or(Error::<T>::EconomicOverflow)?;
			Likes::<T>::insert(
				&sender,
				&key,
				(
					total - u64::from(terms.count),
					terms.count,
					owed,
					pot.clone(),
				),
			);
			if first == pot {
				Urls::<T>::insert(&key, (total, sender.clone(), pot));
			} else {
				Urls::<T>::insert(&key, (total, first, sender.clone()));
			}
			Self::deposit_event(Event::Liked {
				who: sender.clone(),
				url: key.clone(),
				num_likes: terms.count,
			});
		}
		receipt.debit = before - T::Currency::balance(&sender).saturated_into::<u128>();
		if terms.policy.model == 0 {
			receipt.legacy_dust = terms
				.cost
				.checked_sub(receipt.debit)
				.and_then(|v| v.checked_sub(receipt.retained))
				.ok_or(Error::<T>::EconomicOverflow)?;
		}
		// Pot/treasury accounts are not valid payers: otherwise nominal credits would be self-retained.
		ensure!(
			sender != Self::pot_id()
				&& sender != terms.policy.treasury
				&& sender != T::OwnershipRegistry::pot_id(),
			Error::<T>::InvalidEconomicPayer
		);
		let funded = receipt
			.owner_credit
			.checked_add(receipt.referral_credit)
			.and_then(|v| v.checked_add(receipt.supporter_paid))
			.and_then(|v| v.checked_add(receipt.supporter_fallback))
			.and_then(|v| v.checked_add(receipt.protocol_credit))
			.ok_or(Error::<T>::EconomicOverflow)?;
		ensure!(
			funded == receipt.debit
				&& receipt.gross == receipt.debit + receipt.retained + receipt.legacy_dust,
			Error::<T>::EconomicOverflow
		);
		Self::deposit_event(Event::PaymentDistribution {
			payer: sender,
			target: key,
			terms_hash,
			receipt,
		});
		Ok(())
	}
	fn proportion(budget: u128, units: u32, count: u32) -> u128 {
		(budget / u128::from(count)) * u128::from(units)
			+ (budget % u128::from(count)) * u128::from(units) / u128::from(count)
	}
	fn transfer_amount(
		sender: &T::AccountId,
		recipient: &T::AccountId,
		amount: u128,
	) -> DispatchResult {
		let converted: BalanceOf<T> = amount.saturated_into();
		ensure!(
			converted.saturated_into::<u128>() == amount,
			Error::<T>::EconomicOverflow
		);
		if amount > 0 && sender != recipient {
			T::Currency::transfer(sender, recipient, converted, Preservation::Preserve)?;
		}
		Ok(())
	}
	fn author_amount(sender: &T::AccountId, page: &[u8], amount: u128) -> DispatchResult {
		let converted: BalanceOf<T> = amount.saturated_into();
		ensure!(
			converted.saturated_into::<u128>() == amount,
			Error::<T>::EconomicOverflow
		);
		T::OwnershipRegistry::pay_author(sender, page, converted)
	}
}
impl<T: Config> wika_traits::PageEconomics<T::AccountId> for Pallet<T> {
	fn validate_claim(page: &[u8], selection: &PolicySelection) -> DispatchResult {
		ensure!(
			selection.inherit_if_changed,
			Error::<T>::InheritanceConsentRequired
		);
		Self::validate_selection(page, selection)
	}
	fn activate_claim(
		page: &[u8],
		owner: &T::AccountId,
		period: u64,
		selection: &PolicySelection,
	) -> Result<PolicyOutcomeReason, DispatchError> {
		match Self::validate_selection(page, selection) {
			Ok(()) => {
				Self::install_policy(page, owner, period, selection)?;
				Ok(PolicyOutcomeReason::Applied)
			}
			Err(_) if selection.inherit_if_changed => {
				Ok(if Self::page_economics(page)?.locked_at.is_some() {
					PolicyOutcomeReason::Locked
				} else {
					PolicyOutcomeReason::TermsChanged
				})
			}
			Err(e) => Err(e),
		}
	}
}

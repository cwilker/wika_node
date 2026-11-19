//! Funded author liabilities and signed page claims. All money mutations run in storage layers.
use crate::*;
use alloc::vec::Vec;
use codec::Encode;
use frame_support::{
	ensure,
	storage::with_storage_layer,
	traits::{
		fungible::{Inspect, Mutate},
		tokens::{Fortitude, Preservation},
		Get, UnixTime,
	},
	transactional, PalletId,
};
use frame_system::pallet_prelude::BlockNumberFor;
use sp_runtime::traits::{AccountIdConversion, CheckedAdd, CheckedSub, SaturatedConversion, Zero};
use sp_runtime::DispatchResult;
use wika_traits::PageEconomics;

pub const ESCROW_ID: PalletId = PalletId(*b"WKESCROW");
pub const FRESHNESS_MS: u64 = 90 * 24 * 60 * 60 * 1000;
/// Development bound; production configuration and benchmarks remain a release requirement.
pub const MAX_INITIAL_LIFETIME_MS: u64 = 24 * 60 * 60 * 1000;

impl<T: Config> Pallet<T> {
	pub fn escrow_id() -> T::AccountId {
		ESCROW_ID.into_account_truncating()
	}
	pub(crate) fn now_ms() -> u64 {
		T::Clock::now().as_millis().try_into().unwrap_or(u64::MAX)
	}
	pub(crate) fn page_url(url: &[u8]) -> Result<UrlOf<T>, sp_runtime::DispatchError> {
		wika_traits::page_identity::canonicalize(url)
			.ok_or(Error::<T>::InvalidPageIdentity)?
			.try_into()
			.map_err(|_| Error::<T>::UrlTooLong.into())
	}
	pub fn ensure_escrow_backed() -> DispatchResult {
		ensure!(
			T::Currency::reducible_balance(
				&Self::escrow_id(),
				Preservation::Preserve,
				Fortitude::Polite
			) >= TotalRewardLiability::<T>::get(),
			Error::<T>::EscrowUnderfunded
		);
		Ok(())
	}
	#[transactional]
	pub(crate) fn accrue_author(
		sender: &T::AccountId,
		url: &[u8],
		amount: BalanceOf<T>,
	) -> DispatchResult {
		if amount.is_zero() {
			return Ok(());
		}
		let url = Self::page_url(url)?;
		// Preserve the existing author self-payment exemption, including its rounding.
		if Owners::<T>::get(&url).as_ref() == Some(sender) {
			return Ok(());
		}
		let liability = TotalRewardLiability::<T>::get()
			.checked_add(&amount)
			.ok_or(Error::<T>::ArithmeticOverflow)?;
		T::Currency::transfer(sender, &Self::escrow_id(), amount, Preservation::Preserve)?;
		let period = CurrentPeriod::<T>::get(&url);
		if let Some(id) = period {
			Periods::<T>::try_mutate(&url, id, |record| -> DispatchResult {
				let record = record.as_mut().ok_or(Error::<T>::PeriodNotFound)?;
				record.balance = record
					.balance
					.checked_add(&amount)
					.ok_or(Error::<T>::ArithmeticOverflow)?;
				Ok(())
			})?;
		} else {
			UnclaimedRewards::<T>::try_mutate(&url, |balance| -> DispatchResult {
				*balance = balance
					.checked_add(&amount)
					.ok_or(Error::<T>::ArithmeticOverflow)?;
				Ok(())
			})?;
		}
		TotalRewardLiability::<T>::put(liability);
		Self::ensure_escrow_backed()?;
		Self::deposit_event(Event::AuthorRewardAccrued {
			url,
			period,
			amount,
		});
		Ok(())
	}
	pub(crate) fn begin_claim(sender: T::AccountId, content: Vec<u8>) -> DispatchResult {
		let envelope = proof::decode_signed(&content).ok_or(Error::<T>::InvalidClaimEnvelope)?;
		ensure!(
			envelope.account.encode() == sender.encode()
				&& envelope.genesis.as_slice()
					== frame_system::Pallet::<T>::block_hash(BlockNumberFor::<T>::zero()).as_ref(),
			Error::<T>::InvalidClaimEnvelope
		);
		let now = Self::now_ms();
		ensure!(
			envelope.expires_at > now
				&& envelope.expires_at <= now.saturating_add(MAX_INITIAL_LIFETIME_MS),
			Error::<T>::ExpiredClaim
		);
		let url = Self::page_url(&envelope.page)?;
		ensure!(
			!PendingReplacements::<T>::contains_key(&url),
			Error::<T>::ReplacementPending
		);
		let purpose = if let Some(id) = CurrentPeriod::<T>::get(&url) {
			ensure!(
				Periods::<T>::get(&url, id)
					.ok_or(Error::<T>::PeriodNotFound)?
					.owner != sender,
				Error::<T>::ReplacementRequired
			);
			CheckPurpose::ReplacementInitial
		} else {
			CheckPurpose::Initial
		};
		ensure!(
			!UsedClaimNonces::<T>::contains_key(&sender, envelope.nonce),
			Error::<T>::ClaimNonceUsed
		);
		let proof: ProofOf = content
			.try_into()
			.map_err(|_| Error::<T>::InvalidClaimEnvelope)?;
		if purpose == CheckPurpose::ReplacementInitial {
			Self::hold_check_bond(&url, &sender)?;
		}
		Self::start_claim_check(
			sender.clone(),
			url,
			ClaimCheck {
				purpose,
				proof,
				expires_at: envelope.expires_at,
				retries_remaining: 2,
			},
			true,
		)?;
		Self::clear_policy_intent(&Self::page_url(&envelope.page)?);
		LastPolicyOutcomes::<T>::remove(Self::page_url(&envelope.page)?);
		UsedClaimNonces::<T>::insert(&sender, envelope.nonce, ());
		Ok(())
	}
	pub(crate) fn begin_refresh(sender: T::AccountId, url: Vec<u8>, period: u64) -> DispatchResult {
		let url = Self::page_url(&url)?;
		ensure!(
			CurrentPeriod::<T>::get(&url) == Some(period),
			Error::<T>::PeriodNotFound
		);
		let record = Periods::<T>::get(&url, period).ok_or(Error::<T>::PeriodNotFound)?;
		ensure!(record.owner == sender, Error::<T>::NotPeriodOwner);
		ensure!(
			!PendingReplacements::<T>::contains_key(&url),
			Error::<T>::ReplacementPending
		);
		let expires_at = Self::now_ms().saturating_add(MAX_INITIAL_LIFETIME_MS);
		Self::start_claim_check(
			sender,
			url.clone(),
			ClaimCheck {
				purpose: CheckPurpose::Refresh(period),
				proof: record.proof,
				expires_at,
				retries_remaining: 2,
			},
			true,
		)?;
		// Invalidate the old authorization at submission; newer failures cannot reuse it.
		Periods::<T>::mutate(&url, period, |p| {
			if let Some(p) = p {
				p.verified_at = None;
			}
		});
		Ok(())
	}
	pub(crate) fn finish_claim_check(url: &UrlOf<T>, outcome: VerificationOutcome) {
		let Some(check) = ClaimChecks::<T>::get(url) else {
			return;
		};
		let outcome = if Self::now_ms() >= check.expires_at {
			VerificationOutcome::Expired
		} else {
			outcome
		};
		let requester = Requests::<T>::get(url).map(|r| r.1);
		let result = with_storage_layer(|| {
			Self::apply_claim_result(url, &check, outcome)?;
			if let Some(account) = requester.as_ref() {
				if check.purpose == CheckPurpose::ReplacementInitial
					&& outcome != VerificationOutcome::Approved
				{
					let bond = CheckBonds::<T>::take(url);
					Self::settle_bond(
						url,
						account,
						bond,
						outcome == VerificationOutcome::InvalidProof,
					)?;
				}
				if outcome == VerificationOutcome::Inconclusive && check.retries_remaining > 0 {
					RetryCredits::<T>::insert(
						url,
						RetryCredit::<T> {
							account: account.clone(),
							check: check.clone(),
						},
					);
				}
			}
			Ok(())
		});
		let outcome = if let Err(error) = result {
			Self::deposit_event(Event::ClaimTransitionFailed {
				url: url.clone(),
				error,
			});
			if check.purpose == CheckPurpose::ReplacementInitial {
				if let Some(account) = requester.as_ref() {
					let bond = CheckBonds::<T>::get(url);
					if with_storage_layer(|| Self::settle_bond(url, account, bond, false)).is_ok() {
						CheckBonds::<T>::remove(url);
					}
				}
			}
			VerificationOutcome::Inconclusive
		} else {
			outcome
		};
		if !(outcome == VerificationOutcome::Inconclusive
			&& check.retries_remaining > 0
			&& Self::now_ms() < check.expires_at)
		{
			Self::clear_policy_intent(url);
		}
		Self::deposit_event(Event::ClaimVerificationFinished {
			url: url.clone(),
			purpose: check.purpose,
			outcome,
			checked_at: Self::now_ms(),
		});
		if let Some(account) = requester {
			LastClaimResults::<T>::insert(
				url,
				ClaimResult::<T> {
					account: account.clone(),
					purpose: check.purpose,
					outcome,
					checked_at: Self::now_ms(),
				},
			);
			ActiveChecksByWallet::<T>::mutate(account, |n| *n = n.saturating_sub(1));
			ActiveChecksTotal::<T>::mutate(|n| *n = n.saturating_sub(1));
		}
		// Finished safe rounds free their slot immediately. Stale cleanup agendas check request block.
		ClaimChecks::<T>::remove(url);
		Requests::<T>::remove(url);
		Rounds::<T>::remove(url);
		Results::<T>::remove(url);
		let _ = Commits::<T>::clear_prefix(url, T::MaxVerifiers::get(), None);
		let _ = Reveals::<T>::clear_prefix(url, T::MaxVerifiers::get(), None);
	}
	fn apply_claim_result(
		url: &UrlOf<T>,
		check: &ClaimCheck,
		outcome: VerificationOutcome,
	) -> DispatchResult {
		if matches!(check.purpose, CheckPurpose::ReplacementFinal(_)) {
			return Self::apply_final_result(url, check, outcome);
		}
		if outcome != VerificationOutcome::Approved {
			return Ok(());
		}
		Self::ensure_escrow_backed()?;
		match check.purpose {
			CheckPurpose::Initial => {
				ensure!(
					!CurrentPeriod::<T>::contains_key(url),
					Error::<T>::ReplacementRequired
				);
				let (_, owner) = Requests::<T>::get(url).ok_or(Error::<T>::UrlCheckNotFound)?;
				let balance = UnclaimedRewards::<T>::take(url);
				let now = Self::now_ms();
				Periods::<T>::insert(
					url,
					1,
					OwnershipPeriod::<T> {
						owner: owner.clone(),
						started_at: now,
						started_block: frame_system::Pallet::<T>::block_number(),
						started_event_index: frame_system::Pallet::<T>::event_count(),
						ended_at: None,
						ended_block: None,
						ended_event_index: None,
						balance,
						proof: check.proof.clone(),
						verified_at: Some(now),
					},
				);
				CurrentPeriod::<T>::insert(url, 1);
				Owners::<T>::insert(url, &owner);
				UrlCount::<T>::mutate(|n| *n = n.saturating_add(1));
				Self::deposit_event(Event::OwnershipPeriodActivated {
					url: url.clone(),
					period: 1,
					account: owner.clone(),
					started_at: now,
					backlog: balance,
					proof_digest: sp_io::hashing::blake2_256(&check.proof),
				});
				if let Some(intent) = PolicyIntents::<T>::get(url) {
					ensure!(
						intent.account == owner
							&& intent.proof_digest == sp_io::hashing::blake2_256(&check.proof)
							&& Self::now_ms() < intent.expires_at,
						Error::<T>::PolicyIntentMismatch
					);
					let reason =
						T::PageEconomics::activate_claim(url, &owner, 1, &intent.selection)?;
					let applied = reason == wika_traits::PolicyOutcomeReason::Applied;
					LastPolicyOutcomes::<T>::insert(
						url,
						PolicyOutcome {
							attempt: intent.attempt,
							account: owner.clone(),
							applied,
							reason,
						},
					);
					Self::deposit_event(Event::ClaimPolicyOutcome {
						url: url.clone(),
						attempt: intent.attempt,
						account: owner.clone(),
						applied,
						reason,
					});
				}
				Self::deposit_event(Event::UrlOwnerRegistered {
					account: owner,
					url: url.clone(),
					block: frame_system::Pallet::<T>::block_number().saturated_into(),
				});
			}
			CheckPurpose::ReplacementInitial => {
				Self::open_transfer(url, check)?;
			}
			CheckPurpose::ReplacementFinal(_) => unreachable!("handled above"),
			CheckPurpose::Refresh(id) => {
				ensure!(
					CurrentPeriod::<T>::get(url) == Some(id),
					Error::<T>::PeriodNotFound
				);
				Periods::<T>::try_mutate(url, id, |p| -> DispatchResult {
					let p = p.as_mut().ok_or(Error::<T>::PeriodNotFound)?;
					ensure!(p.proof == check.proof, Error::<T>::InvalidClaimEnvelope);
					p.verified_at = Some(Self::now_ms());
					Ok(())
				})?;
			}
		}
		Ok(())
	}
	pub(crate) fn withdraw_author(
		sender: T::AccountId,
		url: Vec<u8>,
		period: u64,
		amount: BalanceOf<T>,
	) -> DispatchResult {
		let url = Self::page_url(&url)?;
		if let Some(pending) = PendingReplacements::<T>::get(&url) {
			ensure!(
				Self::now_ms() >= pending.deadline,
				Error::<T>::ReplacementPending
			);
			Self::expire_transfer(url.clone())?;
		}
		let record = Periods::<T>::get(&url, period).ok_or(Error::<T>::PeriodNotFound)?;
		ensure!(record.owner == sender, Error::<T>::NotPeriodOwner);
		if record.ended_at.is_none() {
			ensure!(
				CurrentPeriod::<T>::get(&url) == Some(period),
				Error::<T>::PeriodNotFound
			);
			let verified = record.verified_at.ok_or(Error::<T>::VerificationRequired)?;
			let now = Self::now_ms();
			ensure!(
				now >= verified && now - verified < FRESHNESS_MS,
				Error::<T>::VerificationRequired
			);
		}
		ensure!(
			!amount.is_zero() && amount <= record.balance,
			Error::<T>::InsufficientRewards
		);
		Self::ensure_escrow_backed()?;
		let liability = TotalRewardLiability::<T>::get()
			.checked_sub(&amount)
			.ok_or(Error::<T>::ArithmeticOverflow)?;
		T::Currency::transfer(&Self::escrow_id(), &sender, amount, Preservation::Preserve)?;
		Periods::<T>::mutate(&url, period, |p| {
			if let Some(p) = p {
				p.balance = p
					.balance
					.checked_sub(&amount)
					.expect("checked against record balance");
			}
		});
		TotalRewardLiability::<T>::put(liability);
		Self::ensure_escrow_backed()?;
		Self::deposit_event(Event::AuthorRewardsWithdrawn {
			url,
			period,
			account: sender,
			amount,
		});
		Ok(())
	}
	pub(crate) fn start_claim_check(
		sender: T::AccountId,
		url: UrlOf<T>,
		check: ClaimCheck,
		charge_fee: bool,
	) -> DispatchResult {
		ensure!(
			ActiveChecksByWallet::<T>::get(&sender) < 4 && ActiveChecksTotal::<T>::get() < 256,
			Error::<T>::ActiveCheckLimit
		);
		Self::queue_check(sender.clone(), url.to_vec(), charge_fee)?;
		RetryCredits::<T>::remove(&url);
		ActiveChecksByWallet::<T>::mutate(&sender, |n| *n += 1);
		ActiveChecksTotal::<T>::mutate(|n| *n += 1);
		Self::deposit_event(Event::ClaimVerificationRequested {
			url: url.clone(),
			purpose: check.purpose,
			account: sender,
			expires_at: check.expires_at,
			proof_digest: sp_io::hashing::blake2_256(&check.proof),
		});
		ClaimChecks::<T>::insert(&url, check);
		Ok(())
	}
	pub(crate) fn retry_claim_check(sender: T::AccountId, url: Vec<u8>) -> DispatchResult {
		let url = Self::page_url(&url)?;
		let credit = RetryCredits::<T>::take(&url).ok_or(Error::<T>::RetryUnavailable)?;
		ensure!(
			credit.account == sender
				&& credit.check.retries_remaining > 0
				&& Self::now_ms() < credit.check.expires_at,
			Error::<T>::RetryUnavailable
		);
		let mut check = credit.check;
		match check.purpose {
			CheckPurpose::Initial => ensure!(
				!CurrentPeriod::<T>::contains_key(&url),
				Error::<T>::RetryUnavailable
			),
			CheckPurpose::Refresh(id) => ensure!(
				CurrentPeriod::<T>::get(&url) == Some(id)
					&& !PendingReplacements::<T>::contains_key(&url),
				Error::<T>::RetryUnavailable
			),
			CheckPurpose::ReplacementInitial => {
				ensure!(
					!PendingReplacements::<T>::contains_key(&url)
						&& CurrentPeriod::<T>::contains_key(&url),
					Error::<T>::RetryUnavailable
				);
				Self::hold_check_bond(&url, &sender)?;
			}
			CheckPurpose::ReplacementFinal(id) => {
				Self::eligible_final(&url, id, &sender)?;
			}
		}
		check.retries_remaining -= 1;
		Self::start_claim_check(sender, url, check, false)
	}
}

impl<T: Config> Pallet<T> {
	pub(crate) fn clear_policy_intent(page: &UrlOf<T>) {
		if let Some(intent) = PolicyIntents::<T>::take(page) {
			PolicyIntentSlots::<T>::remove(intent.slot);
		}
	}
	pub(crate) fn prune_policy_slot(slot: u16) {
		if let Some(page) = PolicyIntentSlots::<T>::get(slot) {
			if PolicyIntents::<T>::get(&page).is_none_or(|i| i.expires_at <= Self::now_ms()) {
				Self::clear_policy_intent(&page);
				PolicyIntentSlots::<T>::remove(slot);
			}
		}
	}
	pub(crate) fn begin_policy_claim(
		sender: T::AccountId,
		content: Vec<u8>,
		selection: wika_traits::PolicySelection,
	) -> DispatchResult {
		let envelope = proof::decode_signed(&content).ok_or(Error::<T>::InvalidClaimEnvelope)?;
		let page = Self::page_url(&envelope.page)?;
		// Replacement claims inherit. An incoming owner can configure after activation if still unlocked.
		ensure!(
			!CurrentPeriod::<T>::contains_key(&page),
			Error::<T>::ReplacementRequired
		);
		T::PageEconomics::validate_claim(&page, &selection)?;
		Self::begin_claim(sender.clone(), content.clone())?;
		let attempt = NextPolicyAttempt::<T>::get()
			.checked_add(1)
			.ok_or(Error::<T>::PolicyAttemptOverflow)?;
		let mut available = None;
		for slot in 0..256u16 {
			Self::prune_policy_slot(slot);
			if !PolicyIntentSlots::<T>::contains_key(slot) {
				available = Some(slot);
				break;
			}
		}
		let slot = available.ok_or(Error::<T>::PolicyIntentCapacity)?;
		let digest = sp_io::hashing::blake2_256(&content);
		PolicyIntents::<T>::insert(
			&page,
			PolicyIntent {
				attempt,
				slot,
				account: sender.clone(),
				proof_digest: digest,
				selection: selection.clone(),
				expires_at: envelope.expires_at,
			},
		);
		PolicyIntentSlots::<T>::insert(slot, &page);
		NextPolicyAttempt::<T>::put(attempt);
		Self::deposit_event(Event::ClaimPolicyRequested {
			url: page,
			attempt,
			account: sender,
			proof_digest: digest,
			selection,
		});
		Ok(())
	}
}

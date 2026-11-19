use crate::*;
use alloc::vec::Vec;
use frame_support::{
	ensure,
	traits::{
		fungible::MutateHold,
		tokens::{Fortitude, Precision, Restriction},
	},
};
use sp_runtime::{
	traits::{SaturatedConversion, Zero},
	DispatchResult,
};
pub const TRANSFER_DELAY_MS: u64 = 14 * 24 * 60 * 60 * 1000;
/// Preliminary development allowance after the two-week window; review before public deployment.
pub const COMPLETION_ALLOWANCE_MS: u64 = 7 * 24 * 60 * 60 * 1000;
impl<T: Config> Pallet<T> {
	pub(crate) fn hold_check_bond(url: &UrlOf<T>, account: &T::AccountId) -> DispatchResult {
		let bond: BalanceOf<T> = ReplacementBond::<T>::get().saturated_into();
		T::Currency::hold(&HoldReason::Replacement.into(), account, bond)?;
		CheckBonds::<T>::insert(url, bond);
		Ok(())
	}
	pub(crate) fn settle_bond(
		url: &UrlOf<T>,
		account: &T::AccountId,
		bond: BalanceOf<T>,
		forfeited: bool,
	) -> DispatchResult {
		if !bond.is_zero() {
			if forfeited {
				T::Currency::transfer_on_hold(
					&HoldReason::Replacement.into(),
					account,
					&Self::pot_id(),
					bond,
					Precision::Exact,
					Restriction::Free,
					Fortitude::Polite,
				)?;
			} else {
				T::Currency::release(
					&HoldReason::Replacement.into(),
					account,
					bond,
					Precision::Exact,
				)?;
			}
		}
		Self::deposit_event(Event::BondSettled {
			url: url.clone(),
			account: account.clone(),
			amount: bond,
			forfeited,
		});
		Ok(())
	}
	pub(crate) fn open_transfer(url: &UrlOf<T>, check: &ClaimCheck) -> DispatchResult {
		ensure!(
			!PendingReplacements::<T>::contains_key(url),
			Error::<T>::ReplacementPending
		);
		let previous_period = CurrentPeriod::<T>::get(url).ok_or(Error::<T>::PeriodNotFound)?;
		let (_, candidate) = Requests::<T>::get(url).ok_or(Error::<T>::UrlCheckNotFound)?;
		let envelope =
			proof::decode_signed(&check.proof).ok_or(Error::<T>::InvalidClaimEnvelope)?;
		let now = Self::now_ms();
		let earliest_activation = now.saturating_add(TRANSFER_DELAY_MS);
		let deadline = earliest_activation.saturating_add(COMPLETION_ALLOWANCE_MS);
		let bond = CheckBonds::<T>::take(url);
		Periods::<T>::mutate(url, previous_period, |p| {
			if let Some(p) = p {
				p.verified_at = None;
			}
		});
		PendingReplacements::<T>::insert(
			url,
			Replacement::<T> {
				id: envelope.nonce,
				candidate: candidate.clone(),
				previous_period,
				proof: check.proof.clone(),
				opened_at: now,
				earliest_activation,
				deadline,
				bond,
				owner_approved: false,
				candidate_approved: false,
				objection: false,
			},
		);
		Self::deposit_event(Event::ReplacementOpened {
			url: url.clone(),
			id: envelope.nonce,
			previous_period,
			candidate,
			earliest_activation,
			deadline,
			bond,
		});
		Ok(())
	}
	pub(crate) fn eligible_final(
		url: &UrlOf<T>,
		id: [u8; 32],
		sender: &T::AccountId,
	) -> Result<Replacement<T>, sp_runtime::DispatchError> {
		let p = PendingReplacements::<T>::get(url).ok_or(Error::<T>::ReplacementNotFound)?;
		ensure!(p.id == id, Error::<T>::ReplacementIdMismatch);
		ensure!(&p.candidate == sender, Error::<T>::NotTransferParty);
		let now = Self::now_ms();
		ensure!(now < p.deadline, Error::<T>::TransferExpired);
		ensure!(
			now >= p.earliest_activation || p.owner_approved && p.candidate_approved,
			Error::<T>::TransferWindowOpen
		);
		ensure!(
			CurrentPeriod::<T>::get(url) == Some(p.previous_period),
			Error::<T>::PeriodNotFound
		);
		Ok(p)
	}
	pub(crate) fn begin_final_check(
		sender: T::AccountId,
		url: Vec<u8>,
		id: [u8; 32],
	) -> DispatchResult {
		let url = Self::page_url(&url)?;
		let p = Self::eligible_final(&url, id, &sender)?;
		let expires_at = p
			.deadline
			.min(Self::now_ms().saturating_add(crate::claims::MAX_INITIAL_LIFETIME_MS));
		Self::start_claim_check(
			sender,
			url,
			ClaimCheck {
				purpose: CheckPurpose::ReplacementFinal(id),
				proof: p.proof,
				expires_at,
				retries_remaining: 2,
			},
			true,
		)
	}
	pub(crate) fn apply_final_result(
		url: &UrlOf<T>,
		check: &ClaimCheck,
		outcome: VerificationOutcome,
	) -> DispatchResult {
		let CheckPurpose::ReplacementFinal(id) = check.purpose else {
			return Ok(());
		};
		// Permissionless expiry may already have cleared it while this last round was in flight.
		let Some(p) = PendingReplacements::<T>::get(url) else {
			return Ok(());
		};
		ensure!(p.id == id, Error::<T>::ReplacementIdMismatch);
		if outcome == VerificationOutcome::Inconclusive {
			return Ok(());
		}
		if outcome != VerificationOutcome::Approved {
			Self::close_transfer(url, &p, false, outcome == VerificationOutcome::InvalidProof)?;
			return Ok(());
		}
		let (_, requester) = Requests::<T>::get(url).ok_or(Error::<T>::UrlCheckNotFound)?;
		Self::eligible_final(url, id, &requester)?;
		ensure!(check.proof == p.proof, Error::<T>::InvalidClaimEnvelope);
		Self::ensure_escrow_backed()?;
		let next = p
			.previous_period
			.checked_add(1)
			.ok_or(Error::<T>::ArithmeticOverflow)?;
		let now = Self::now_ms();
		Periods::<T>::try_mutate(url, p.previous_period, |old| -> DispatchResult {
			let old = old.as_mut().ok_or(Error::<T>::PeriodNotFound)?;
			old.ended_at = Some(now);
			old.ended_block = Some(frame_system::Pallet::<T>::block_number());
			old.ended_event_index = Some(frame_system::Pallet::<T>::event_count());
			old.verified_at = None;
			Ok(())
		})?;
		Self::deposit_event(Event::OwnershipPeriodClosed {
			url: url.clone(),
			period: p.previous_period,
			ended_at: now,
		});
		Periods::<T>::insert(
			url,
			next,
			OwnershipPeriod::<T> {
				owner: p.candidate.clone(),
				started_at: now,
				started_block: frame_system::Pallet::<T>::block_number(),
				started_event_index: frame_system::Pallet::<T>::event_count(),
				ended_at: None,
				ended_block: None,
				ended_event_index: None,
				balance: Zero::zero(),
				proof: p.proof.clone(),
				verified_at: Some(now),
			},
		);
		CurrentPeriod::<T>::insert(url, next);
		Owners::<T>::insert(url, &p.candidate);
		Self::deposit_event(Event::OwnershipPeriodActivated {
			url: url.clone(),
			period: next,
			account: p.candidate.clone(),
			started_at: now,
			backlog: Zero::zero(),
			proof_digest: sp_io::hashing::blake2_256(&p.proof),
		});
		Self::deposit_event(Event::UrlOwnerRegistered {
			url: url.clone(),
			account: p.candidate.clone(),
			block: frame_system::Pallet::<T>::block_number().saturated_into(),
		});
		Self::close_transfer(url, &p, true, false)
	}
	fn close_transfer(
		url: &UrlOf<T>,
		p: &Replacement<T>,
		activated: bool,
		forfeited: bool,
	) -> DispatchResult {
		Self::settle_bond(url, &p.candidate, p.bond, forfeited)?;
		PendingReplacements::<T>::remove(url);
		RetryCredits::<T>::remove(url);
		Self::deposit_event(Event::ReplacementClosed {
			url: url.clone(),
			id: p.id,
			activated,
		});
		Ok(())
	}
	pub(crate) fn approve_transfer(
		sender: T::AccountId,
		url: Vec<u8>,
		id: [u8; 32],
	) -> DispatchResult {
		let url = Self::page_url(&url)?;
		PendingReplacements::<T>::try_mutate(&url, |pending| -> DispatchResult {
			let p = pending.as_mut().ok_or(Error::<T>::ReplacementNotFound)?;
			ensure!(p.id == id, Error::<T>::ReplacementIdMismatch);
			ensure!(Self::now_ms() < p.deadline, Error::<T>::TransferExpired);
			let owner = Periods::<T>::get(&url, p.previous_period)
				.ok_or(Error::<T>::PeriodNotFound)?
				.owner;
			if owner == sender {
				p.owner_approved = true;
			} else if p.candidate == sender {
				p.candidate_approved = true;
			} else {
				return Err(Error::<T>::NotTransferParty.into());
			}
			Ok(())
		})?;
		Self::deposit_event(Event::ReplacementApproved {
			url,
			id,
			account: sender,
		});
		Ok(())
	}
	pub(crate) fn object_transfer(
		sender: T::AccountId,
		url: Vec<u8>,
		id: [u8; 32],
	) -> DispatchResult {
		let url = Self::page_url(&url)?;
		PendingReplacements::<T>::try_mutate(&url, |pending| -> DispatchResult {
			let p = pending.as_mut().ok_or(Error::<T>::ReplacementNotFound)?;
			ensure!(p.id == id, Error::<T>::ReplacementIdMismatch);
			ensure!(Self::now_ms() < p.deadline, Error::<T>::TransferExpired);
			ensure!(
				Periods::<T>::get(&url, p.previous_period)
					.ok_or(Error::<T>::PeriodNotFound)?
					.owner == sender,
				Error::<T>::NotPeriodOwner
			);
			p.objection = true;
			Ok(())
		})?;
		Self::deposit_event(Event::ReplacementObjected {
			url,
			id,
			account: sender,
		});
		Ok(())
	}
	pub(crate) fn cancel_transfer(
		sender: T::AccountId,
		url: Vec<u8>,
		id: [u8; 32],
	) -> DispatchResult {
		let url = Self::page_url(&url)?;
		let p = PendingReplacements::<T>::get(&url).ok_or(Error::<T>::ReplacementNotFound)?;
		ensure!(p.id == id, Error::<T>::ReplacementIdMismatch);
		ensure!(p.candidate == sender, Error::<T>::NotTransferParty);
		ensure!(
			!Requests::<T>::contains_key(&url),
			Error::<T>::UrlCheckAlreadyInQueue
		);
		Self::close_transfer(&url, &p, false, false)
	}
	pub(crate) fn expire_transfer(url: UrlOf<T>) -> DispatchResult {
		let p = PendingReplacements::<T>::get(&url).ok_or(Error::<T>::ReplacementNotFound)?;
		ensure!(Self::now_ms() >= p.deadline, Error::<T>::TransferWindowOpen);
		Self::close_transfer(&url, &p, false, false)
	}
}

use crate::mock::Owners;
use crate::{mock::*, *};
use codec::Encode;
use frame_support::{
	assert_noop, assert_ok,
	traits::{fungible::Inspect, Hooks},
};
use sp_core::Pair;
use sp_runtime::{AccountId32, MultiSignature};
const URL: &[u8] = b"https://example.com/post";
fn proof_for(who: &str, nonce: u8, expires_at: u64) -> Vec<u8> {
	let pair = pair(who);
	let envelope = proof::Envelope {
		version: 1,
		method: 1,
		genesis: System::block_hash(0).0,
		page: URL.to_vec().try_into().unwrap(),
		account: AccountId32::from(pair.public().0),
		nonce: [nonce; 32],
		expires_at,
	};
	let signature = MultiSignature::Sr25519(pair.sign(&envelope.message()));
	let mut content = proof::PREFIX.to_vec();
	content.extend(to_hex(&envelope.encode()));
	content.push(b'.');
	content.extend(to_hex(&signature.encode()));
	content
}
fn run_to(block: u64) {
	while System::block_number() < block {
		Owners::on_finalize(System::block_number());
		let next = System::block_number() + 1;
		System::set_block_number(next);
		Owners::on_initialize(next);
	}
	Owners::on_finalize(block);
}

fn finish(vote: bool) {
	let url: UrlOf<Test> = URL.to_vec().try_into().unwrap();
	let (at, requester) = Requests::<Test>::get(&url).unwrap();
	let round = Rounds::<Test>::get(&url).unwrap();
	let evidence = if vote {
		sp_io::hashing::blake2_256(&ClaimChecks::<Test>::get(&url).unwrap().proof).to_vec()
	} else {
		vec![]
	};
	let salt = pair("Alice")
		.sign(&Owners::round_message(
			URL,
			at,
			&requester,
			vote,
			&[],
			&evidence,
		))
		.encode();
	let hash = sp_io::hashing::keccak_256(&commit_preimage(vote, &[], &evidence, &salt));
	run_to(at + 1);
	assert_ok!(Owners::commit_verification(
		RuntimeOrigin::signed(account("Alice")),
		URL.to_vec(),
		hash.to_vec()
	));
	run_to(round.commit_end + 1);
	assert_ok!(Owners::reveal_verification(
		RuntimeOrigin::signed(account("Alice")),
		URL.to_vec(),
		vote,
		vec![],
		evidence,
		salt
	));
	run_to(round.reveal_end + 1);
}
#[test]
fn backed_backlog_is_assigned_once_and_withdrawn_only_by_owner() {
	new_test_ext().ext.execute_with(|| {
		let bob = account("Bob");
		let alice = account("Alice");
		assert_ok!(Likes::like(
			RuntimeOrigin::signed(alice),
			b"HTTPS://EXAMPLE.COM:443/post#comments".to_vec(),
			vec![],
			1
		));
		let url: UrlOf<Test> = URL.to_vec().try_into().unwrap();
		let amount = UNIT * 33 / 100;
		assert_eq!(UnclaimedRewards::<Test>::get(&url), amount);
		assert_eq!(TotalRewardLiability::<Test>::get(), amount);
		assert_eq!(Balances::balance(&Owners::escrow_id()), UNIT + amount);
		assert_ok!(Owners::claim_page(
			RuntimeOrigin::signed(bob),
			proof_for("Bob", 1, 100_000)
		));
		assert!(CurrentPeriod::<Test>::get(&url).is_none());
		finish(true);
		assert_eq!(CurrentPeriod::<Test>::get(&url), Some(1));
		assert_eq!(UnclaimedRewards::<Test>::get(&url), 0);
		assert_eq!(Periods::<Test>::get(&url, 1).unwrap().balance, amount);
		assert_noop!(
			Owners::withdraw_rewards(RuntimeOrigin::signed(alice), URL.to_vec(), 1, amount),
			Error::<Test>::NotPeriodOwner
		);
		let before = Balances::balance(&bob);
		assert_ok!(Owners::withdraw_rewards(
			RuntimeOrigin::signed(bob),
			URL.to_vec(),
			1,
			amount
		));
		assert_eq!(Balances::balance(&bob), before + amount);
		assert_eq!(TotalRewardLiability::<Test>::get(), 0);
		assert_eq!(Balances::balance(&Owners::escrow_id()), UNIT);
		assert_noop!(
			Owners::withdraw_rewards(RuntimeOrigin::signed(bob), URL.to_vec(), 1, amount),
			Error::<Test>::InsufficientRewards
		);
	});
}
#[test]
fn stale_withdrawal_requires_a_new_check_and_failure_preserves_credit() {
	new_test_ext().ext.execute_with(|| {
		let bob = account("Bob");
		let url: UrlOf<Test> = URL.to_vec().try_into().unwrap();
		assert_ok!(Likes::like(
			RuntimeOrigin::signed(account("Alice")),
			URL.to_vec(),
			vec![],
			1
		));
		assert_ok!(Owners::claim_page(
			RuntimeOrigin::signed(bob),
			proof_for("Bob", 1, 100_000)
		));
		finish(true);
		Now::set(1_000 + crate::claims::FRESHNESS_MS);
		assert_noop!(
			Owners::withdraw_rewards(RuntimeOrigin::signed(bob), URL.to_vec(), 1, 1),
			Error::<Test>::VerificationRequired
		);
		assert_ok!(Owners::verify_withdrawal(
			RuntimeOrigin::signed(bob),
			URL.to_vec(),
			1
		));
		finish(false);
		assert_eq!(
			Periods::<Test>::get(&url, 1).unwrap().balance,
			UNIT * 33 / 100
		);
		assert_noop!(
			Owners::withdraw_rewards(RuntimeOrigin::signed(bob), URL.to_vec(), 1, 1),
			Error::<Test>::VerificationRequired
		);
		assert_ok!(Owners::verify_withdrawal(
			RuntimeOrigin::signed(bob),
			URL.to_vec(),
			1
		));
		finish(true);
		assert_ok!(Owners::withdraw_rewards(
			RuntimeOrigin::signed(bob),
			URL.to_vec(),
			1,
			UNIT * 33 / 100
		));
	});
}
#[test]
fn wrong_wallet_expiry_nonce_and_unverified_claims_cannot_assign_backlog() {
	new_test_ext().ext.execute_with(|| {
		let bob = account("Bob");
		assert_noop!(
			Owners::claim_page(
				RuntimeOrigin::signed(account("Alice")),
				proof_for("Bob", 1, 100_000)
			),
			Error::<Test>::InvalidClaimEnvelope
		);
		assert_noop!(
			Owners::claim_page(RuntimeOrigin::signed(bob), proof_for("Bob", 1, 1_000)),
			Error::<Test>::ExpiredClaim
		);
		let proof = proof_for("Bob", 2, 100_000);
		assert_ok!(Owners::claim_page(
			RuntimeOrigin::signed(bob),
			proof.clone()
		));
		finish(false);
		assert_noop!(
			Owners::claim_page(RuntimeOrigin::signed(bob), proof),
			Error::<Test>::ClaimNonceUsed
		);
		let url: UrlOf<Test> = URL.to_vec().try_into().unwrap();
		assert!(CurrentPeriod::<Test>::get(url).is_none());
	});
}
fn deposit(amount: u64) {
	use wika_traits::OwnershipRegistry;
	assert_ok!(<Owners as OwnershipRegistry<AccountId, u64>>::pay_author(
		&account("Alice"),
		URL,
		amount
	));
}
fn initial_owner() {
	deposit(100);
	assert_ok!(Owners::claim_page(
		RuntimeOrigin::signed(account("Bob")),
		proof_for("Bob", 1, Now::get() + 100_000)
	));
	finish(true);
}
fn pending_candidate() {
	assert_ok!(Owners::claim_page(
		RuntimeOrigin::signed(account("Charlie")),
		proof_for("Charlie", 2, Now::get() + 100_000)
	));
	finish(true);
}
#[test]
fn replacement_keeps_all_prior_rewards_and_requires_new_post_window_quorum() {
	new_test_ext().ext.execute_with(|| {
		initial_owner();
		deposit(20);
		let url: UrlOf<Test> = URL.to_vec().try_into().unwrap();
		pending_candidate();
		deposit(10);
		let pending = PendingReplacements::<Test>::get(&url).unwrap();
		assert_eq!(Periods::<Test>::get(&url, 1).unwrap().balance, 130);
		assert_noop!(
			Owners::withdraw_rewards(RuntimeOrigin::signed(account("Bob")), URL.to_vec(), 1, 130),
			Error::<Test>::ReplacementPending
		);
		assert_noop!(
			Owners::finalize_replacement(
				RuntimeOrigin::signed(account("Charlie")),
				URL.to_vec(),
				pending.id
			),
			Error::<Test>::TransferWindowOpen
		);
		assert_ok!(Owners::object_replacement(
			RuntimeOrigin::signed(account("Bob")),
			URL.to_vec(),
			pending.id
		));
		Now::set(pending.earliest_activation);
		assert_ok!(Owners::finalize_replacement(
			RuntimeOrigin::signed(account("Charlie")),
			URL.to_vec(),
			pending.id
		));
		// The first successful round only opened the pending transfer, never activated it.
		assert_eq!(CurrentPeriod::<Test>::get(&url), Some(1));
		finish(true);
		assert_eq!(CurrentPeriod::<Test>::get(&url), Some(2));
		assert_eq!(Periods::<Test>::get(&url, 1).unwrap().balance, 130);
		assert_eq!(Periods::<Test>::get(&url, 2).unwrap().balance, 0);
		deposit(7);
		assert_eq!(Periods::<Test>::get(&url, 2).unwrap().balance, 7);
		assert_noop!(
			Owners::withdraw_rewards(
				RuntimeOrigin::signed(account("Charlie")),
				URL.to_vec(),
				1,
				130
			),
			Error::<Test>::NotPeriodOwner
		);
		// Historical withdrawal needs the original wallet, no current page proof or freshness.
		assert_ok!(Owners::withdraw_rewards(
			RuntimeOrigin::signed(account("Bob")),
			URL.to_vec(),
			1,
			130
		));
		assert_ok!(Owners::withdraw_rewards(
			RuntimeOrigin::signed(account("Charlie")),
			URL.to_vec(),
			2,
			7
		));
		assert_eq!(TotalRewardLiability::<Test>::get(), 0);
		assert_ok!(Owners::ensure_escrow_backed());
	});
}
#[test]
fn cooperative_transfer_requires_both_wallets_and_still_needs_fresh_verification() {
	new_test_ext().ext.execute_with(|| {
		initial_owner();
		pending_candidate();
		let url: UrlOf<Test> = URL.to_vec().try_into().unwrap();
		let p = PendingReplacements::<Test>::get(&url).unwrap();
		assert_noop!(
			Owners::approve_replacement(RuntimeOrigin::signed(account("Dave")), URL.to_vec(), p.id),
			Error::<Test>::NotTransferParty
		);
		assert_noop!(
			Owners::approve_replacement(
				RuntimeOrigin::signed(account("Bob")),
				URL.to_vec(),
				[99; 32]
			),
			Error::<Test>::ReplacementIdMismatch
		);
		assert_ok!(Owners::approve_replacement(
			RuntimeOrigin::signed(account("Bob")),
			URL.to_vec(),
			p.id
		));
		assert_noop!(
			Owners::finalize_replacement(
				RuntimeOrigin::signed(account("Charlie")),
				URL.to_vec(),
				p.id
			),
			Error::<Test>::TransferWindowOpen
		);
		assert_ok!(Owners::approve_replacement(
			RuntimeOrigin::signed(account("Charlie")),
			URL.to_vec(),
			p.id
		));
		assert_ok!(Owners::finalize_replacement(
			RuntimeOrigin::signed(account("Charlie")),
			URL.to_vec(),
			p.id
		));
		assert_eq!(CurrentPeriod::<Test>::get(&url), Some(1));
		finish(true);
		assert_eq!(CurrentPeriod::<Test>::get(&url), Some(2));
	});
}
#[test]
fn invalid_challenger_forfeits_bond_outage_does_not_and_retries_are_bounded_and_fee_exempt() {
	new_test_ext().ext.execute_with(|| {
		use frame_support::traits::fungible::InspectHold;
		initial_owner();
		let candidate = account("Charlie");
		let url: UrlOf<Test> = URL.to_vec().try_into().unwrap();
		assert_ok!(Owners::claim_page(
			RuntimeOrigin::signed(candidate),
			proof_for("Charlie", 2, Now::get() + 100_000)
		));
		assert_eq!(
			Balances::balance_on_hold(&HoldReason::Replacement.into(), &candidate),
			UNIT
		);
		finish(false);
		assert_eq!(
			Balances::balance_on_hold(&HoldReason::Replacement.into(), &candidate),
			0
		);
		assert_eq!(Balances::balance(&Owners::pot_id()), 12 * UNIT); // reserve + first fee + challenger fee + forfeiture
		assert_eq!(TotalRewardLiability::<Test>::get(), 100);
		assert_ok!(Owners::claim_page(
			RuntimeOrigin::signed(candidate),
			proof_for("Charlie", 3, Now::get() + 100_000)
		));
		let fees = Balances::balance(&Owners::pot_id());
		for attempt in 0..3 {
			let end = Rounds::<Test>::get(&url).unwrap().reveal_end;
			run_to(end + 1);
			assert!(PendingReplacements::<Test>::get(&url).is_none());
			assert_eq!(
				Balances::balance_on_hold(&HoldReason::Replacement.into(), &candidate),
				0
			);
			assert_eq!(Balances::balance(&Owners::pot_id()), fees);
			if attempt < 2 {
				assert_ok!(Owners::retry_verification(
					RuntimeOrigin::signed(candidate),
					URL.to_vec()
				));
			}
		}
		assert_noop!(
			Owners::retry_verification(RuntimeOrigin::signed(candidate), URL.to_vec()),
			Error::<Test>::RetryUnavailable
		);
		assert_eq!(ActiveChecksTotal::<Test>::get(), 0);
		// Unproven challengers never froze this owner's funds.
		assert_ok!(Owners::withdraw_rewards(
			RuntimeOrigin::signed(account("Bob")),
			URL.to_vec(),
			1,
			100
		));
	});
}
#[test]
fn expired_pending_transfer_releases_bond_and_old_owner_must_reverify() {
	new_test_ext().ext.execute_with(|| {
		initial_owner();
		pending_candidate();
		let url: UrlOf<Test> = URL.to_vec().try_into().unwrap();
		let p = PendingReplacements::<Test>::get(&url).unwrap();
		assert_noop!(
			Owners::expire_replacement(RuntimeOrigin::signed(account("Dave")), URL.to_vec()),
			Error::<Test>::TransferWindowOpen
		);
		Now::set(p.deadline);
		assert_ok!(Owners::expire_replacement(
			RuntimeOrigin::signed(account("Dave")),
			URL.to_vec()
		));
		assert!(PendingReplacements::<Test>::get(&url).is_none());
		assert_noop!(
			Owners::withdraw_rewards(RuntimeOrigin::signed(account("Bob")), URL.to_vec(), 1, 100),
			Error::<Test>::VerificationRequired
		);
		assert_ok!(Owners::verify_withdrawal(
			RuntimeOrigin::signed(account("Bob")),
			URL.to_vec(),
			1
		));
		finish(true);
		assert_ok!(Owners::withdraw_rewards(
			RuntimeOrigin::signed(account("Bob")),
			URL.to_vec(),
			1,
			100
		));
	});
}
#[test]
fn unfunded_escrow_reserve_rolls_back_the_entire_like_and_other_reward_payments() {
	new_test_ext().ext.execute_with(|| {
		use frame_support::traits::{fungible::Mutate, tokens::Preservation};
		assert_ok!(Balances::transfer(
			&Owners::escrow_id(),
			&account("Dave"),
			UNIT,
			Preservation::Expendable
		));
		assert_noop!(
			Likes::like(
				RuntimeOrigin::signed(account("Alice")),
				URL.to_vec(),
				vec![],
				1
			),
			Error::<Test>::EscrowUnderfunded
		);
		assert_eq!(TotalRewardLiability::<Test>::get(), 0);
		assert_eq!(pallet_likes::UrlCount::<Test>::get(), 0);
	});
}
#[test]
fn below_minimum_withdrawal_keeps_credit_when_owner_account_was_reaped() {
	new_test_ext().ext.execute_with(|| {
		use frame_support::traits::{fungible::Mutate, tokens::Preservation};
		deposit(5);
		let bob = account("Bob");
		let url: UrlOf<Test> = URL.to_vec().try_into().unwrap();
		assert_ok!(Owners::claim_page(
			RuntimeOrigin::signed(bob),
			proof_for("Bob", 1, 100_000)
		));
		finish(true);
		let all = Balances::balance(&bob);
		assert_ok!(Balances::transfer(
			&bob,
			&account("Alice"),
			all,
			Preservation::Expendable
		));
		assert!(Owners::withdraw_rewards(RuntimeOrigin::signed(bob), URL.to_vec(), 1, 5).is_err());
		assert_eq!(Periods::<Test>::get(&url, 1).unwrap().balance, 5);
		assert_eq!(TotalRewardLiability::<Test>::get(), 5);
		assert_ok!(Owners::ensure_escrow_backed());
	});
}
#[test]
fn failed_final_proof_ends_freeze_without_giving_away_earlier_earnings() {
	new_test_ext().ext.execute_with(|| {
		initial_owner();
		pending_candidate();
		let url: UrlOf<Test> = URL.to_vec().try_into().unwrap();
		let p = PendingReplacements::<Test>::get(&url).unwrap();
		Now::set(p.earliest_activation);
		assert_ok!(Owners::finalize_replacement(
			RuntimeOrigin::signed(account("Charlie")),
			URL.to_vec(),
			p.id
		));
		assert_noop!(
			Owners::cancel_replacement(
				RuntimeOrigin::signed(account("Charlie")),
				URL.to_vec(),
				p.id
			),
			Error::<Test>::UrlCheckAlreadyInQueue
		);
		finish(false);
		assert!(PendingReplacements::<Test>::get(&url).is_none());
		assert_eq!(CurrentPeriod::<Test>::get(&url), Some(1));
		assert_eq!(Periods::<Test>::get(&url, 1).unwrap().balance, 100);
		assert_noop!(
			Owners::withdraw_rewards(RuntimeOrigin::signed(account("Bob")), URL.to_vec(), 1, 100),
			Error::<Test>::VerificationRequired
		);
	});
}
#[test]
fn negative_reports_cannot_be_replayed_for_a_different_envelope_even_at_same_page_block_wallet() {
	new_test_ext().ext.execute_with(|| {
		let bob = account("Bob");
		let url: UrlOf<Test> = URL.to_vec().try_into().unwrap();
		assert_ok!(Owners::claim_page(
			RuntimeOrigin::signed(bob),
			proof_for("Bob", 1, 100_000)
		));
		let salt = pair("Alice")
			.sign(&Owners::round_message(URL, 1, &bob, false, &[], &[]))
			.encode();
		let hash = sp_io::hashing::keccak_256(&commit_preimage(false, &[], &[], &salt));
		run_to(2);
		assert_ok!(Owners::commit_verification(
			RuntimeOrigin::signed(account("Alice")),
			URL.to_vec(),
			hash.to_vec()
		));
		// Simulate another fork's same-block request with another signed nonce.
		ClaimChecks::<Test>::mutate(&url, |c| {
			c.as_mut().unwrap().proof = proof_for("Bob", 2, 100_000).try_into().unwrap()
		});
		run_to(7);
		assert_noop!(
			Owners::reveal_verification(
				RuntimeOrigin::signed(account("Alice")),
				URL.to_vec(),
				false,
				vec![],
				vec![],
				salt
			),
			Error::<Test>::InvalidSalt
		);
	});
}
#[test]
fn safe_profile_rejects_legacy_claim_before_any_fee() {
	new_test_ext().ext.execute_with(|| {
		LegacyClaims::set(false);
		assert_noop!(
			Owners::request_url_check(RuntimeOrigin::signed(account("Bob")), URL.to_vec()),
			Error::<Test>::SignedProofRequired
		);
	});
}
#[test]
fn returning_wallet_gets_a_new_period_and_never_merges_its_earlier_earnings() {
	new_test_ext().ext.execute_with(|| {
		initial_owner();
		let url: UrlOf<Test> = URL.to_vec().try_into().unwrap();
		for (candidate, nonce, expected) in [("Charlie", 2u8, 2u64), ("Bob", 3, 3)] {
			let previous = Periods::<Test>::get(&url, expected - 1).unwrap().owner;
			assert_ok!(Owners::claim_page(
				RuntimeOrigin::signed(account(candidate)),
				proof_for(candidate, nonce, Now::get() + 100_000)
			));
			finish(true);
			let p = PendingReplacements::<Test>::get(&url).unwrap();
			assert_ok!(Owners::approve_replacement(
				RuntimeOrigin::signed(previous),
				URL.to_vec(),
				p.id
			));
			assert_ok!(Owners::approve_replacement(
				RuntimeOrigin::signed(account(candidate)),
				URL.to_vec(),
				p.id
			));
			assert_ok!(Owners::finalize_replacement(
				RuntimeOrigin::signed(account(candidate)),
				URL.to_vec(),
				p.id
			));
			finish(true);
			deposit(7);
		}
		assert_eq!(Periods::<Test>::get(&url, 1).unwrap().balance, 100);
		assert_eq!(Periods::<Test>::get(&url, 2).unwrap().balance, 7);
		assert_eq!(Periods::<Test>::get(&url, 3).unwrap().balance, 7);
		run_to(200); // Stale cleanup agendas must not erase periods or revive old claims.
		assert_eq!(CurrentPeriod::<Test>::get(&url), Some(3));
		assert_eq!(TotalRewardLiability::<Test>::get(), 114);
		assert_ok!(Owners::ensure_escrow_backed());
	});
}
#[test]
fn safe_activation_waits_for_finalization_and_uses_this_blocks_time_and_earnings_boundary() {
	new_test_ext().ext.execute_with(|| {
		deposit(100);
		let bob = account("Bob");
		let url: UrlOf<Test> = URL.to_vec().try_into().unwrap();
		assert_ok!(Owners::claim_page(
			RuntimeOrigin::signed(bob),
			proof_for("Bob", 1, 100_000)
		));
		let evidence =
			sp_io::hashing::blake2_256(&ClaimChecks::<Test>::get(&url).unwrap().proof).to_vec();
		let salt = pair("Alice")
			.sign(&Owners::round_message(URL, 1, &bob, true, &[], &evidence))
			.encode();
		let hash = sp_io::hashing::keccak_256(&commit_preimage(true, &[], &evidence, &salt));
		run_to(2);
		assert_ok!(Owners::commit_verification(
			RuntimeOrigin::signed(account("Alice")),
			URL.to_vec(),
			hash.to_vec()
		));
		run_to(7);
		assert_ok!(Owners::reveal_verification(
			RuntimeOrigin::signed(account("Alice")),
			URL.to_vec(),
			true,
			vec![],
			evidence,
			salt
		));
		run_to(11);
		System::set_block_number(12);
		Owners::on_initialize(12);
		assert!(CurrentPeriod::<Test>::get(&url).is_none());
		deposit(20);
		Now::set(9_000);
		Owners::on_finalize(12);
		let p = Periods::<Test>::get(&url, 1).unwrap();
		assert_eq!(p.started_at, 9_000);
		assert_eq!(p.started_block, 12);
		assert!(matches!(
			&System::events()[p.started_event_index as usize].event,
			RuntimeEvent::Owners(Event::OwnershipPeriodActivated { period: 1, .. })
		));
		assert_eq!(p.verified_at, Some(9_000));
		assert_eq!(p.balance, 120);
		deposit(7);
		assert_eq!(Periods::<Test>::get(&url, 1).unwrap().balance, 127);
	});
}
#[test]
fn failed_transition_returns_candidate_bond_without_assigning_unbacked_rewards() {
	new_test_ext().ext.execute_with(|| {
		use frame_support::traits::{
			fungible::{InspectHold, Mutate},
			tokens::Preservation,
		};
		initial_owner();
		let candidate = account("Charlie");
		let url: UrlOf<Test> = URL.to_vec().try_into().unwrap();
		assert_ok!(Owners::claim_page(
			RuntimeOrigin::signed(candidate),
			proof_for("Charlie", 2, 100_000)
		));
		let all = Balances::balance(&Owners::escrow_id());
		assert_ok!(Balances::transfer(
			&Owners::escrow_id(),
			&account("Dave"),
			all,
			Preservation::Expendable
		));
		finish(true);
		assert!(PendingReplacements::<Test>::get(&url).is_none());
		assert_eq!(Periods::<Test>::get(&url, 1).unwrap().balance, 100);
		assert_eq!(
			Balances::balance_on_hold(&HoldReason::Replacement.into(), &candidate),
			0
		);
		assert_eq!(ActiveChecksTotal::<Test>::get(), 0);
	});
}
#[test]
fn draining_free_balance_does_not_evade_an_invalid_challenger_bond() {
	new_test_ext().ext.execute_with(|| {
		use frame_support::traits::{
			fungible::{InspectHold, Mutate},
			tokens::Preservation,
		};
		initial_owner();
		let candidate = account("Charlie");
		assert_ok!(Owners::claim_page(
			RuntimeOrigin::signed(candidate),
			proof_for("Charlie", 2, 100_000)
		));
		let free = Balances::reducible_balance(
			&candidate,
			Preservation::Expendable,
			frame_support::traits::tokens::Fortitude::Polite,
		);
		assert_ok!(Balances::transfer(
			&candidate,
			&account("Dave"),
			free,
			Preservation::Expendable
		));
		finish(false);
		assert_eq!(
			Balances::balance_on_hold(&HoldReason::Replacement.into(), &candidate),
			0
		);
		assert_eq!(Balances::balance(&Owners::pot_id()), 12 * UNIT);
	});
}
#[test]
fn claimed_authors_self_like_keeps_the_existing_author_exemption() {
	new_test_ext().ext.execute_with(|| {
		initial_owner();
		let bob = account("Bob");
		let before = Balances::balance(&bob);
		assert_ok!(Likes::like(
			RuntimeOrigin::signed(bob),
			URL.to_vec(),
			vec![],
			1
		));
		assert_eq!(before - Balances::balance(&bob), UNIT * 67 / 100);
		assert_eq!(TotalRewardLiability::<Test>::get(), 100);
	});
}

fn activate_economics() {
	assert_ok!(Likes::schedule_economics(
		RuntimeOrigin::root(),
		System::block_number() + 1,
		pallet_likes::economics::Rules {
			version: 1,
			owner_min: 0,
			owner_max: 9800,
			supporter_min: 100,
			supporter_max: 9800,
			referral_min: 100,
			referral_max: 9800,
			protocol_bps: 100,
			minimum_price: 100,
			treasury: account("Charlie")
		},
		4500,
		4500,
		900,
		4
	));
	let next = System::block_number() + 1;
	System::set_block_number(next);
	Likes::on_initialize(next);
}
fn custom_selection() -> wika_traits::PolicySelection {
	let state = Likes::page_economics(URL).unwrap();
	wika_traits::PolicySelection {
		owner_bps: 2500,
		supporter_bps: 6000,
		referral_bps: 1400,
		ruleset: 1,
		expected_revision: state.policy.revision,
		expected_hash: state.policy_hash,
		inherit_if_changed: true,
	}
}
#[test]
fn custom_claim_activates_policy_only_with_verified_ownership() {
	new_test_ext().ext.execute_with(|| {
		activate_economics();
		assert_ok!(Owners::claim_page_with_policy(
			RuntimeOrigin::signed(account("Bob")),
			proof_for("Bob", 41, 100_000),
			custom_selection()
		));
		assert_eq!(Likes::page_economics(URL).unwrap().policy.owner_bps, 4500);
		finish(true);
		assert_eq!(
			CurrentPeriod::<Test>::get(Owners::page_url(URL).unwrap()),
			Some(1)
		);
		assert_eq!(Likes::page_economics(URL).unwrap().policy.owner_bps, 2500);
		assert!(
			LastPolicyOutcomes::<Test>::get(Owners::page_url(URL).unwrap())
				.unwrap()
				.applied
		);
		assert!(PolicyIntents::<Test>::get(Owners::page_url(URL).unwrap()).is_none());
	});
}
#[test]
fn first_payment_wins_custom_claim_race_and_backlog_is_funded() {
	new_test_ext().ext.execute_with(|| {
		activate_economics();
		assert_ok!(Owners::claim_page_with_policy(
			RuntimeOrigin::signed(account("Bob")),
			proof_for("Bob", 42, 100_000),
			custom_selection()
		));
		let t = Likes::economic_terms(&account("Alice"), URL, &[], 1, 0).unwrap();
		assert_ok!(Likes::like_with_terms(
			RuntimeOrigin::signed(account("Alice")),
			URL.to_vec().try_into().unwrap(),
			1,
			2,
			sp_io::hashing::blake2_256(&t.encode())
		));
		finish(true);
		assert_eq!(Likes::page_economics(URL).unwrap().policy.owner_bps, 4500);
		assert!(
			!LastPolicyOutcomes::<Test>::get(Owners::page_url(URL).unwrap())
				.unwrap()
				.applied
		);
		assert_eq!(
			Periods::<Test>::get(Owners::page_url(URL).unwrap(), 1)
				.unwrap()
				.balance,
			UNIT * 54 / 100
		);
		assert_ok!(Owners::ensure_escrow_backed());
	});
}
#[test]
fn invalid_policy_rejects_before_verification_charge_and_legacy_claim_cannot_reuse_intent() {
	new_test_ext().ext.execute_with(|| {
		activate_economics();
		let before = Balances::balance(&account("Bob"));
		let mut sel = custom_selection();
		sel.inherit_if_changed = false;
		assert!(Owners::claim_page_with_policy(
			RuntimeOrigin::signed(account("Bob")),
			proof_for("Bob", 43, 100_000),
			sel
		)
		.is_err());
		assert_eq!(Balances::balance(&account("Bob")), before);
		assert_ok!(Owners::claim_page_with_policy(
			RuntimeOrigin::signed(account("Bob")),
			proof_for("Bob", 44, 100_000),
			custom_selection()
		));
		finish(false);
		assert!(PolicyIntents::<Test>::get(Owners::page_url(URL).unwrap()).is_none());
		assert_ok!(Owners::claim_page(
			RuntimeOrigin::signed(account("Bob")),
			proof_for("Bob", 45, 100_000)
		));
		finish(true);
		assert_eq!(Likes::page_economics(URL).unwrap().policy.owner_bps, 4500);
	});
}
#[test]
fn custom_no_quorum_retry_keeps_exact_attempt_and_expired_intents_are_bounded() {
	new_test_ext().ext.execute_with(|| {
		activate_economics();
		assert_ok!(Owners::claim_page_with_policy(
			RuntimeOrigin::signed(account("Bob")),
			proof_for("Bob", 46, 100_000),
			custom_selection()
		));
		let page = Owners::page_url(URL).unwrap();
		let attempt = PolicyIntents::<Test>::get(&page).unwrap().attempt;
		let round = Rounds::<Test>::get(&page).unwrap();
		run_to(round.reveal_end + 1);
		assert!(RetryCredits::<Test>::contains_key(&page));
		assert_eq!(PolicyIntents::<Test>::get(&page).unwrap().attempt, attempt);
		assert_ok!(Owners::retry_verification(
			RuntimeOrigin::signed(account("Bob")),
			URL.to_vec()
		));
		assert_eq!(PolicyIntents::<Test>::get(&page).unwrap().attempt, attempt);
		finish(true);
		assert_eq!(
			LastPolicyOutcomes::<Test>::get(&page).unwrap().attempt,
			attempt
		);
		assert!(PolicyIntents::<Test>::get(&page).is_none());
	});
	new_test_ext().ext.execute_with(|| {
		activate_economics();
		assert_ok!(Owners::claim_page_with_policy(
			RuntimeOrigin::signed(account("Bob")),
			proof_for("Bob", 47, 100_000),
			custom_selection()
		));
		let page = Owners::page_url(URL).unwrap();
		let slot = PolicyIntents::<Test>::get(&page).unwrap().slot;
		Now::set(100_001);
		PolicyIntentCursor::<Test>::put(slot);
		Owners::on_initialize(System::block_number());
		assert!(!PolicyIntents::<Test>::contains_key(&page));
		assert!(!PolicyIntentSlots::<Test>::contains_key(slot));
	});
}
#[test]
fn changed_default_during_claim_reports_terms_inheritance_separately() {
	new_test_ext().ext.execute_with(|| {
		activate_economics();
		assert_ok!(Owners::claim_page_with_policy(
			RuntimeOrigin::signed(account("Bob")),
			proof_for("Bob", 48, 100_000),
			custom_selection()
		));
		assert_ok!(Likes::publish_default(
			RuntimeOrigin::root(),
			3500,
			5000,
			1400
		));
		finish(true);
		let state = Likes::page_economics(URL).unwrap();
		assert_eq!(state.policy.owner_bps, 3500);
		assert!(state.locked_at.is_none());
		let outcome = LastPolicyOutcomes::<Test>::get(Owners::page_url(URL).unwrap()).unwrap();
		assert!(!outcome.applied);
		assert_eq!(
			outcome.reason,
			wika_traits::PolicyOutcomeReason::TermsChanged
		);
	});
}
#[test]
fn stale_verified_owner_cannot_reconfigure_even_without_earnings() {
	new_test_ext().ext.execute_with(|| {
		activate_economics();
		assert_ok!(Owners::claim_page_with_policy(
			RuntimeOrigin::signed(account("Bob")),
			proof_for("Bob", 49, 100_000),
			custom_selection()
		));
		finish(true);
		let before = Likes::page_economics(URL).unwrap().policy_hash;
		Now::set(1_000 + crate::claims::FRESHNESS_MS);
		assert_noop!(
			Likes::configure_page_policy(
				RuntimeOrigin::signed(account("Bob")),
				URL.to_vec().try_into().unwrap(),
				1,
				custom_selection()
			),
			pallet_likes::Error::<Test>::NotHostOwner
		);
		assert_eq!(Likes::page_economics(URL).unwrap().policy_hash, before);
	});
}

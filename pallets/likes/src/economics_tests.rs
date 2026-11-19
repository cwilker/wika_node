use crate::mock::Likes as RuntimeLikes;
use crate::{economics::*, mock::*, *};
use codec::Encode;
use frame_support::{
	assert_noop, assert_ok,
	traits::{fungible::Inspect, Hooks},
};
use wika_traits::PolicySelection;
const URL: &[u8] = b"https://policy.test/post";
fn key() -> UrlOf<Test> {
	URL.to_vec().try_into().unwrap()
}
fn activate() {
	assert_ok!(RuntimeLikes::schedule_economics(
		RuntimeOrigin::root(),
		2,
		Rules {
			version: 1,
			owner_min: 0,
			owner_max: 9800,
			supporter_min: 100,
			supporter_max: 9800,
			referral_min: 100,
			referral_max: 9800,
			protocol_bps: 100,
			minimum_price: 100,
			treasury: 888
		},
		4500,
		4500,
		900,
		4
	));
	System::set_block_number(2);
	RuntimeLikes::on_initialize(2);
}
fn selection(a: u16, s: u16, r: u16) -> PolicySelection {
	let p = RuntimeLikes::page_economics(URL).unwrap();
	PolicySelection {
		owner_bps: a,
		supporter_bps: s,
		referral_bps: r,
		ruleset: 1,
		expected_revision: p.policy.revision,
		expected_hash: p.policy_hash,
		inherit_if_changed: true,
	}
}
fn pay(who: u64, n: u32) {
	let t = RuntimeLikes::economic_terms(&who, URL, &[], n, 0).unwrap();
	assert_ok!(RuntimeLikes::like_with_terms(
		RuntimeOrigin::signed(who),
		key(),
		n,
		2,
		sp_io::hashing::blake2_256(&t.encode())
	));
}
#[test]
fn custom_split_conserves_and_locks_and_direct_referral_goes_to_target() {
	new_test_ext().execute_with(|| {
		activate();
		MockOwners::set(URL, AUTHOR);
		assert_noop!(
			RuntimeLikes::configure_page_policy(
				RuntimeOrigin::signed(BOB),
				key(),
				1,
				selection(2500, 6000, 1400)
			),
			Error::<Test>::NotHostOwner
		);
		assert_ok!(RuntimeLikes::configure_page_policy(
			RuntimeOrigin::signed(AUTHOR),
			key(),
			1,
			selection(2500, 6000, 1400)
		));
		let old = RuntimeLikes::economic_terms(&ALICE, URL, &[], 1, 0).unwrap();
		assert_eq!(old.referral_amount, 140_000_000_000);
		assert!(old.referral_to_target);
		let before = Balances::balance(&ALICE);
		pay(ALICE, 1);
		assert_eq!(Balances::balance(&ALICE), before - UNIT);
		assert_eq!(Balances::balance(&AUTHOR), UNIT + UNIT * 39 / 100);
		assert!(PolicyLocks::<Test>::contains_key(key()));
		assert_noop!(
			RuntimeLikes::configure_page_policy(
				RuntimeOrigin::signed(AUTHOR),
				key(),
				1,
				selection(4500, 4500, 900)
			),
			Error::<Test>::PolicyLocked
		);
		// Lock metadata alone does not invalidate otherwise identical commitments.
		assert_eq!(
			old.encode(),
			RuntimeLikes::economic_terms(&ALICE, URL, &[], 1, 0)
				.unwrap()
				.encode()
		);
		let before = Balances::balance(&ALICE);
		pay(BOB, 3);
		assert_eq!(Balances::balance(&ALICE), before + 3 * UNIT * 60 / 100);
		assert_noop!(
			RuntimeLikes::like(RuntimeOrigin::signed(CHARLIE), URL.to_vec(), vec![], 1),
			Error::<Test>::EconomicTermsRequired
		);
	});
}
#[test]
fn changed_policy_invalidates_quote_and_failed_payment_never_locks() {
	new_test_ext().execute_with(|| {
		activate();
		MockOwners::set(URL, AUTHOR);
		let t = RuntimeLikes::economic_terms(&ALICE, URL, &[], 1, 0).unwrap();
		assert_ok!(RuntimeLikes::configure_page_policy(
			RuntimeOrigin::signed(AUTHOR),
			key(),
			1,
			selection(2500, 6000, 1400)
		));
		assert_noop!(
			RuntimeLikes::like_with_terms(
				RuntimeOrigin::signed(ALICE),
				key(),
				1,
				2,
				sp_io::hashing::blake2_256(&t.encode())
			),
			Error::<Test>::TermsChanged
		);
		let t = RuntimeLikes::economic_terms(&POOR, URL, &[], 1, 0).unwrap();
		assert_noop!(
			RuntimeLikes::like_with_terms(
				RuntimeOrigin::signed(POOR),
				key(),
				1,
				2,
				sp_io::hashing::blake2_256(&t.encode())
			),
			Error::<Test>::NotEnoughBalanceToLike
		);
		assert!(!PolicyLocks::<Test>::contains_key(key()));
		assert!(!Urls::<Test>::contains_key(key()));
	});
}
#[test]
fn legacy_snapshot_keeps_queue_and_global_mutation_cannot_rewrite_it() {
	new_test_ext().execute_with(|| {
		assert_ok!(RuntimeLikes::like(
			RuntimeOrigin::signed(ALICE),
			URL.to_vec(),
			vec![],
			1
		));
		let queue = crate::Likes::<Test>::get(ALICE, key());
		activate();
		assert_eq!(crate::Likes::<Test>::get(ALICE, key()), queue);
		ShareAuthor::<Test>::put(45);
		SharePreviousLikers::<Test>::put(45);
		ShareReferrer::<Test>::put(9);
		NumRoundsToRewardLikers::<Test>::put(10);
		let state = RuntimeLikes::page_economics(URL).unwrap();
		assert!(state.legacy);
		assert_eq!(state.policy.owner_bps, 3300);
		assert_eq!(state.policy.rounds, 4);
		let before = Balances::balance(&ALICE);
		pay(BOB, 1);
		assert_eq!(Balances::balance(&ALICE), before + UNIT * 33 / 100);
		assert_eq!(crate::Likes::<Test>::get(BOB, key()).unwrap().2, 4);
	});
}
#[test]
fn exact_budget_rounding_empty_and_exhausted_queue_and_retention() {
	new_test_ext().execute_with(|| {
		activate();
		LikePrice::<Test>::put(10001);
		MockOwners::set(URL, AUTHOR);
		pay(ALICE, 1);
		let before = Balances::balance(&ALICE);
		pay(BOB, 7);
		assert_eq!(
			Balances::balance(&ALICE) - before,
			(70007u64 * 4500 / 10000) * 4 / 7
		);
		let before = Balances::balance(&AUTHOR);
		pay(AUTHOR, 2);
		// Own author + unused referral allocations are retained, no unfunded escrow.
		let receipt = System::events()
			.into_iter()
			.find_map(|e| match e.event {
				RuntimeEvent::Likes(Event::PaymentDistribution { payer, receipt, .. })
					if payer == AUTHOR =>
				{
					Some(receipt)
				}
				_ => None,
			})
			.unwrap();
		assert_eq!(receipt.gross, 20002);
		assert_eq!(receipt.retained, 9000 + 1800);
		assert_eq!(receipt.debit, receipt.gross - receipt.retained);
		assert_eq!(before - Balances::balance(&AUTHOR), receipt.debit as u64);
	});
}
#[test]
fn price_overflow_invalid_bounds_and_monotonic_revision() {
	new_test_ext().execute_with(|| {
		activate();
		MockOwners::set(URL, AUTHOR);
		assert_noop!(
			RuntimeLikes::configure_page_policy(
				RuntimeOrigin::signed(AUTHOR),
				key(),
				1,
				selection(4500, 4500, 0)
			),
			Error::<Test>::InvalidPolicy
		);
		let original = RuntimeLikes::page_economics(URL).unwrap().policy_hash;
		assert_ok!(RuntimeLikes::configure_page_policy(
			RuntimeOrigin::signed(AUTHOR),
			key(),
			1,
			selection(2500, 6000, 1400)
		));
		assert_ok!(RuntimeLikes::configure_page_policy(
			RuntimeOrigin::signed(AUTHOR),
			key(),
			1,
			selection(4500, 4500, 900)
		));
		let state = RuntimeLikes::page_economics(URL).unwrap();
		assert_eq!(state.policy.revision, 2);
		assert_ne!(original, state.policy_hash);
		LikePrice::<Test>::put(u128::MAX);
		assert!(RuntimeLikes::economic_terms(&ALICE, URL, &[], 2, 0).is_err());
		assert_eq!(RuntimeLikes::fraction(u128::MAX, 10000).unwrap(), u128::MAX);
	});
}
#[test]
fn default_change_only_affects_unselected_unpaid_pages() {
	new_test_ext().execute_with(|| {
		activate();
		MockOwners::set(URL, AUTHOR);
		assert_ok!(RuntimeLikes::configure_page_policy(
			RuntimeOrigin::signed(AUTHOR),
			key(),
			1,
			selection(4500, 4500, 900)
		));
		assert_ok!(RuntimeLikes::publish_default(
			RuntimeOrigin::root(),
			2500,
			6000,
			1400
		));
		assert_eq!(
			RuntimeLikes::page_economics(URL).unwrap().policy.owner_bps,
			4500
		);
		assert_eq!(
			RuntimeLikes::page_economics(b"https://policy.test/other")
				.unwrap()
				.policy
				.owner_bps,
			2500
		);
	});
}
#[test]
fn economic_terms_v2_matches_shared_scale_fixture() {
	use codec::Decode;
	let f: serde_json::Value = serde_json::from_str(include_str!(
		"../../../compatibility/fixtures/economic-terms-v2.json"
	))
	.unwrap();
	let bytes = sp_core::bytes::from_hex(f["termsScale"].as_str().unwrap()).unwrap();
	let terms = EconomicTerms::<sp_runtime::AccountId32>::decode(&mut &bytes[..]).unwrap();
	assert_eq!(terms.encode(), bytes);
	assert_eq!(terms.policy.owner_bps, 2500);
	assert_eq!(terms.policy.supporter_bps, 6000);
	assert_eq!(terms.policy.revision, 1);
	assert_eq!(terms.target.as_slice(), b"https://article.example/");
	assert_eq!(
		terms.policy_hash,
		sp_io::hashing::blake2_256(
			&(
				b"WIKA_PAGE_POLICY_V1",
				terms.target.as_slice(),
				&terms.policy
			)
				.encode()
		)
	);
	assert_eq!(
		sp_io::hashing::blake2_256(&bytes).to_vec(),
		sp_core::bytes::from_hex(f["termsHash"].as_str().unwrap()).unwrap()
	);
}
#[test]
fn failed_transfer_rolls_back_lock_queue_credits_events_and_signed_replay() {
	use frame_support::traits::{LockableCurrency, WithdrawReasons};
	use sp_core::Pair;
	new_test_ext().execute_with(|| {
		activate();
		System::set_block_number(100);
		let source = b"https://host.example/give";
		MockOwners::set(source, DAVE);
		let host = sp_core::sr25519::Pair::from_seed(&[7; 32]);
		assert_ok!(RuntimeLikes::authorize_host(
			RuntimeOrigin::signed(DAVE),
			source.to_vec().try_into().unwrap(),
			host.public().0
		));
		let terms = RuntimeLikes::economic_terms(&ALICE, URL, source, 1, 1).unwrap();
		let p = ReferralPayload {
			domain: referrals::DOMAIN,
			version: 1,
			genesis_hash: System::block_hash(0).0,
			source_page: source.to_vec().try_into().unwrap(),
			source_period: 1,
			authorization_id: 1,
			target_page: URL.to_vec().try_into().unwrap(),
			payer: ALICE,
			count: 1,
			request_id: [9; 32],
			issued_at: 600000,
			expires_at: 900000,
			terms_version: 2,
			terms_hash: sp_io::hashing::blake2_256(&terms.encode()),
		};
		let sig = host.sign(&sp_io::hashing::blake2_256(&p.encode())).0;
		Balances::set_lock(*b"failpay!", &ALICE, 1000 * UNIT, WithdrawReasons::all());
		let before = Balances::balance(&ALICE);
		let events = System::events().len();
		assert!(RuntimeLikes::like_with_referral(
			RuntimeOrigin::signed(ALICE),
			p.clone(),
			host.public().0,
			sig
		)
		.is_err());
		assert_eq!(Balances::balance(&ALICE), before);
		assert_eq!(System::events().len(), events);
		assert!(!PolicyLocks::<Test>::contains_key(key()));
		assert!(!PagePolicies::<Test>::contains_key(key()));
		assert!(!Urls::<Test>::contains_key(key()));
		assert!(!UsedRequests::<Test>::contains_key(1, [9; 32]));
		Balances::remove_lock(*b"failpay!", &ALICE);
		assert_ok!(RuntimeLikes::like_with_referral(
			RuntimeOrigin::signed(ALICE),
			p,
			host.public().0,
			sig
		));
		assert!(UsedRequests::<Test>::contains_key(1, [9; 32]));
	});
}
#[test]
fn unsupported_stored_model_fails_instead_of_falling_forward() {
	new_test_ext().execute_with(|| {
		activate();
		let mut p = RuntimeLikes::page_economics(URL).unwrap().policy;
		p.model = 2;
		PagePolicies::<Test>::insert(key(), p);
		assert!(RuntimeLikes::page_economics(URL).is_err());
		assert!(RuntimeLikes::economic_terms(&ALICE, URL, &[], 1, 0).is_err());
		assert!(!Urls::<Test>::contains_key(key()));
	});
}

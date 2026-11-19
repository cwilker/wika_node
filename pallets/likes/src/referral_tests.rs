use crate::{
	mock::*, referrals, Error, ReferralPayload, ReplayBuckets, ShareReferrer, UsedRequests,
};
use codec::Encode;
use frame_support::{assert_noop, assert_ok, traits::fungible::Inspect};
use sp_core::{sr25519, Pair};
const SOURCE: &[u8] = b"https://host.example/give";
const TARGET: &[u8] = b"https://article.example/";
fn key() -> sr25519::Pair {
	sr25519::Pair::from_seed(&[7; 32])
}
fn setup() {
	System::set_block_number(100);
	MockOwners::set(SOURCE, DAVE);
	assert_ok!(Likes::authorize_host(
		RuntimeOrigin::signed(DAVE),
		SOURCE.to_vec().try_into().unwrap(),
		key().public().0
	));
}
fn envelope() -> (ReferralPayload<u64>, [u8; 64]) {
	let terms = Likes::referral_terms(&ALICE, TARGET, SOURCE, 1, 1).unwrap();
	let p = ReferralPayload {
		domain: referrals::DOMAIN,
		version: 1,
		genesis_hash: System::block_hash(0).0,
		source_page: SOURCE.to_vec().try_into().unwrap(),
		source_period: 1,
		authorization_id: 1,
		target_page: TARGET.to_vec().try_into().unwrap(),
		payer: ALICE,
		count: 1,
		request_id: [8; 32],
		issued_at: 600000,
		expires_at: 900000,
		terms_version: 1,
		terms_hash: sp_io::hashing::blake2_256(&terms.encode()),
	};
	let sig = key().sign(&sp_io::hashing::blake2_256(&p.encode())).0;
	(p, sig)
}
fn submit(p: ReferralPayload<u64>, sig: [u8; 64]) -> frame_support::dispatch::DispatchResult {
	Likes::like_with_referral(RuntimeOrigin::signed(ALICE), p, key().public().0, sig)
}
#[test]
fn referral_pays_once_and_closes_unsigned_bypass() {
	new_test_ext().execute_with(|| {
		setup();
		let (p, s) = envelope();
		let before = Balances::balance(&DAVE);
		assert_ok!(submit(p.clone(), s));
		assert_eq!(Balances::balance(&DAVE) - before, UNIT * 33 / 100);
		assert_noop!(submit(p, s), Error::<Test>::RequestConsumed);
		assert_noop!(
			Likes::like(
				RuntimeOrigin::signed(BOB),
				TARGET.to_vec(),
				SOURCE.to_vec(),
				1
			),
			Error::<Test>::SignedReferralRequired
		);
		assert_ok!(Likes::like(
			RuntimeOrigin::signed(BOB),
			TARGET.to_vec(),
			vec![],
			1
		));
	});
}
#[test]
fn failed_payment_rolls_back_replay_reservation() {
	new_test_ext().execute_with(|| {
		setup();
		let (p, s) = envelope();
		assert_ok!(Likes::like(
			RuntimeOrigin::signed(ALICE),
			TARGET.to_vec(),
			vec![],
			1
		));
		assert_noop!(submit(p, s), Error::<Test>::AlreadyInQueue);
		assert!(!UsedRequests::<Test>::contains_key(1, [8; 32]));
		assert!(ReplayBuckets::<Test>::iter().next().is_none());
	});
}
#[test]
fn altered_fields_wrong_payer_expiry_and_terms_are_rejected() {
	new_test_ext().execute_with(|| {
		setup();
		let (p, s) = envelope();
		let mut altered = p.clone();
		altered.count = 2;
		assert_noop!(submit(altered, s), Error::<Test>::InvalidReferral);
		assert_noop!(
			Likes::like_with_referral(RuntimeOrigin::signed(BOB), p.clone(), key().public().0, s),
			Error::<Test>::InvalidReferral
		);
		ShareReferrer::<Test>::put(9);
		assert_noop!(submit(p.clone(), s), Error::<Test>::TermsChanged);
		ShareReferrer::<Test>::put(33);
		System::set_block_number(150);
		assert_noop!(submit(p, s), Error::<Test>::ReferralExpired);
	});
}
#[test]
fn revocation_reauthorization_and_ownership_invalidate_old_requests() {
	new_test_ext().execute_with(|| {
		setup();
		let (p, s) = envelope();
		assert_ok!(Likes::revoke_host(
			RuntimeOrigin::signed(DAVE),
			SOURCE.to_vec().try_into().unwrap(),
			1
		));
		assert_noop!(submit(p.clone(), s), Error::<Test>::HostNotAuthorized);
		assert_ok!(Likes::authorize_host(
			RuntimeOrigin::signed(DAVE),
			SOURCE.to_vec().try_into().unwrap(),
			key().public().0
		));
		assert_noop!(submit(p.clone(), s), Error::<Test>::HostNotAuthorized);
		MockOwners::set(SOURCE, BOB);
		assert_noop!(
			Likes::authorize_host(
				RuntimeOrigin::signed(DAVE),
				SOURCE.to_vec().try_into().unwrap(),
				key().public().0
			),
			Error::<Test>::NotHostOwner
		);
	});
}
#[test]
fn replay_capacity_and_cleanup_are_bounded() {
	new_test_ext().execute_with(|| {
		setup();
		let (p, s) = envelope();
		let entries: Vec<_> = (0..64).map(|n| (2, [n; 32], 900000)).collect();
		ReplayBuckets::<Test>::insert(
			296,
			frame_support::BoundedVec::<_, frame_support::traits::ConstU32<64>>::try_from(entries)
				.unwrap(),
		);
		// 900000 / 1000 % 302 = 296.
		assert_noop!(submit(p.clone(), s), Error::<Test>::ReplayCapacity);
		assert!(!UsedRequests::<Test>::contains_key(1, p.request_id));
		System::set_block_number(150);
		Likes::prune_bucket(296);
		assert!(!ReplayBuckets::<Test>::contains_key(296));
		assert_noop!(submit(p, s), Error::<Test>::ReferralExpired);
	});
}
#[test]
fn shared_account32_scale_and_signature_vector() {
	use codec::{DecodeAll, Encode};
	use sp_runtime::AccountId32;
	fn bytes(s: &str) -> Vec<u8> {
		s.strip_prefix("0x")
			.unwrap()
			.as_bytes()
			.chunks(2)
			.map(|p| u8::from_str_radix(core::str::from_utf8(p).unwrap(), 16).unwrap())
			.collect()
	}
	let v: serde_json::Value = serde_json::from_str(include_str!(
		"../../../compatibility/fixtures/host-referral-v1.json"
	))
	.unwrap();
	let encoded = bytes(v["encoded"].as_str().unwrap());
	let p = ReferralPayload::<AccountId32>::decode_all(&mut &encoded[..]).unwrap();
	assert_eq!(p.encode(), encoded);
	assert_eq!(p.source_page.as_slice(), SOURCE);
	assert_eq!(p.authorization_id, 17);
	assert_eq!(p.payer, AccountId32::new([1; 32]));
	let terms = crate::ReferralTerms {
		version: 1,
		genesis_hash: [3; 32],
		payer: AccountId32::new([1; 32]),
		target: TARGET.to_vec().try_into().unwrap(),
		source: SOURCE.to_vec().try_into().unwrap(),
		source_period: 2,
		authorization_id: 17,
		count: 1,
		price: 1_000_000_000_000,
		cost: 1_000_000_000_000,
		recipient: AccountId32::new([2; 32]),
		referral_amount: 330_000_000_000,
		target_owner: AccountId32::new([2; 32]),
		target_period: Some(3),
		share_author: 33,
		share_referrer: 33,
		share_supporters: 33,
		rounds: 4,
	};
	assert_eq!(sp_io::hashing::blake2_256(&terms.encode()), p.terms_hash);
	let digest = sp_io::hashing::blake2_256(&encoded);
	assert_eq!(digest.as_slice(), bytes(v["digest"].as_str().unwrap()));
	let sig = sr25519::Signature::from_raw(
		bytes(v["envelope"]["signature"].as_str().unwrap())
			.try_into()
			.unwrap(),
	);
	let public = sr25519::Public::from_raw(
		bytes(v["envelope"]["publicKey"].as_str().unwrap())
			.try_into()
			.unwrap(),
	);
	assert!(sr25519::Pair::verify(&sig, &digest, &public));
}
#[test]
fn host_key_limit_and_reauthorization_generations_are_atomic() {
	new_test_ext().execute_with(|| {
		setup();
		for n in 1..4 {
			assert_ok!(Likes::authorize_host(
				RuntimeOrigin::signed(DAVE),
				SOURCE.to_vec().try_into().unwrap(),
				[n; 32]
			));
		}
		assert_noop!(
			Likes::authorize_host(
				RuntimeOrigin::signed(DAVE),
				SOURCE.to_vec().try_into().unwrap(),
				[9; 32]
			),
			Error::<Test>::HostKeyLimit
		);
		assert_eq!(crate::NextAuthorization::<Test>::get(), 4);
		assert_ok!(Likes::authorize_host(
			RuntimeOrigin::signed(DAVE),
			SOURCE.to_vec().try_into().unwrap(),
			key().public().0
		));
		assert_eq!(crate::NextAuthorization::<Test>::get(), 5);
		let source: crate::UrlOf<Test> = SOURCE.to_vec().try_into().unwrap();
		assert_eq!(crate::HostKeys::<Test>::get(source).len(), 4);
	});
}
#[test]
fn forged_chain_and_unverified_hosts_cannot_authorize_payments() {
	new_test_ext().execute_with(|| {
		assert_noop!(
			Likes::authorize_host(
				RuntimeOrigin::signed(DAVE),
				SOURCE.to_vec().try_into().unwrap(),
				key().public().0
			),
			Error::<Test>::NotHostOwner
		);
		setup();
		let (mut p, _) = envelope();
		p.genesis_hash = [99; 32];
		let sig = key().sign(&sp_io::hashing::blake2_256(&p.encode())).0;
		assert_noop!(submit(p, sig), Error::<Test>::InvalidReferral);
	});
}

#[test]
fn ownership_period_change_invalidates_previous_owner_keys() {
	new_test_ext().execute_with(|| {
		setup();
		let (p, sig) = envelope();
		MockOwners::set_period(SOURCE, BOB, 2);
		assert_noop!(submit(p, sig), Error::<Test>::HostNotAuthorized);
		assert_noop!(
			Likes::authorize_host(
				RuntimeOrigin::signed(DAVE),
				SOURCE.to_vec().try_into().unwrap(),
				key().public().0
			),
			Error::<Test>::NotHostOwner
		);
		assert_ok!(Likes::authorize_host(
			RuntimeOrigin::signed(BOB),
			SOURCE.to_vec().try_into().unwrap(),
			key().public().0
		));
		let source: crate::UrlOf<Test> = SOURCE.to_vec().try_into().unwrap();
		assert_eq!(
			crate::HostKeys::<Test>::get(source).into_inner(),
			vec![(2, 2, key().public().0)]
		);
	});
}

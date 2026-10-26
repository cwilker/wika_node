use crate::{
	commit_preimage, inspect_page, mark_proves, mock::*, pallet::resolve_location_for_test,
	to_hex, vote_message, Error, Event, History, Owners as OwnersStore, Requests, Results,
	UrlCount, Verifiers, MARK_PREFIX,
};
use codec::{Decode, Encode};
use frame_support::{
	assert_noop, assert_ok,
	traits::{fungible::Inspect, Hooks},
};
use sp_core::{offchain::testing, Pair};
use wika_traits::OwnershipRegistry;

const URL: &[u8] = b"https://blog.example/post";

fn mark_for(who: &AccountId) -> Vec<u8> {
	let mut mark = MARK_PREFIX.to_vec();
	mark.extend(to_hex(&who.encode()));
	mark
}

fn page_for(who: &AccountId) -> Vec<u8> {
	let mut page = b"<html><head><title>My post</title></head><body><p>".to_vec();
	page.extend(mark_for(who));
	page.extend(b"</p></body></html>");
	page
}

fn run_to(n: u64) {
	while System::block_number() < n {
		let next = System::block_number() + 1;
		System::set_block_number(next);
		Owners::on_initialize(next);
	}
}

/// A verifier's commit and reveal for `vote` on the page `page`, as the offchain worker builds them.
fn ballot(verifier: &str, requester: &AccountId, page: &[u8]) -> (Vec<u8>, bool, Vec<u8>, Vec<u8>, Vec<u8>) {
	let (vote, intro, proof) = inspect_page(page, &requester.encode());
	let salt = pair(verifier).sign(&vote_message(vote, &intro, &proof)).encode();
	let hash = sp_io::hashing::keccak_256(&commit_preimage(vote, &intro, &proof, &salt)).to_vec();
	(hash, vote, intro, proof, salt)
}

#[test]
fn inspect_page_finds_the_mark_and_survives_short_pages() {
	let bob = account("Bob");
	let (vote, intro, proof) = inspect_page(&page_for(&bob), &bob.encode());
	assert!(vote);
	assert_eq!(intro, page_for(&bob)[..128].to_vec());
	assert!(proof.starts_with(MARK_PREFIX));
	assert!(proof.len() <= 128);

	// Short pages and pages without a mark no longer panic; they vote no.
	assert_eq!(inspect_page(b"hi", &bob.encode()), (false, b"hi".to_vec(), vec![]));
	let alice = account("Alice");
	assert!(!inspect_page(&page_for(&alice), &bob.encode()).0);

	// Upper-case hex is accepted.
	let upper = String::from_utf8(mark_for(&bob)).unwrap().replace("author/", "AUTHOR/");
	assert!(!mark_proves(upper.as_bytes(), &bob.encode()));
	let mut mark = MARK_PREFIX.to_vec();
	mark.extend(to_hex(&bob.encode()).to_ascii_uppercase());
	assert!(mark_proves(&mark, &bob.encode()));
}

#[test]
fn resolves_redirect_locations() {
	let base = "https://a.example/blog/post";
	assert_eq!(resolve_location_for_test(base, "https://b.example/x").unwrap(), "https://b.example/x");
	assert_eq!(resolve_location_for_test(base, "/root").unwrap(), "https://a.example/root");
	assert_eq!(resolve_location_for_test(base, "other").unwrap(), "https://a.example/blog/other");
	assert_eq!(resolve_location_for_test(base, "//c.example/y").unwrap(), "https://c.example/y");
	assert_eq!(resolve_location_for_test("https://a.example", "z").unwrap(), "https://a.example/z");
}

#[test]
fn request_url_check_charges_and_queues() {
	new_test_ext().ext.execute_with(|| {
		let bob = account("Bob");
		assert_ok!(Owners::request_url_check(RuntimeOrigin::signed(bob), URL.to_vec()));
		assert_eq!(Balances::balance(&bob), 995 * UNIT);
		assert_eq!(Balances::balance(&Owners::pot_id()), 6 * UNIT);
		let url: crate::UrlOf<Test> = URL.to_vec().try_into().unwrap();
		assert_eq!(Requests::<Test>::get(&url), Some((1, bob)));
		assert_eq!(History::<Test>::get(1).into_inner(), vec![url.clone()]);
		System::assert_last_event(Event::UrlCheckRequested { account: bob, url }.into());

		assert_noop!(
			Owners::request_url_check(RuntimeOrigin::signed(bob), URL.to_vec()),
			Error::<Test>::UrlCheckAlreadyInQueue
		);
		assert_noop!(
			Owners::request_url_check(RuntimeOrigin::signed(account("Poor")), b"https://x/".to_vec()),
			Error::<Test>::NotEnoughBalanceToRequestUrlCheck
		);
		assert_noop!(
			Owners::request_url_check(RuntimeOrigin::signed(bob), vec![b'a'; 255]),
			Error::<Test>::UrlTooLong
		);
	});
}

#[test]
fn caps_requests_per_block() {
	new_test_ext().ext.execute_with(|| {
		let bob = account("Bob");
		for i in 0..4 {
			assert_ok!(Owners::request_url_check(RuntimeOrigin::signed(bob), format!("https://x/{i}").into_bytes()));
		}
		assert_noop!(
			Owners::request_url_check(RuntimeOrigin::signed(bob), b"https://x/5".to_vec()),
			Error::<Test>::TooManyRequests
		);
	});
}

#[test]
fn commit_reveal_and_tally_register_the_owner() {
	new_test_ext().ext.execute_with(|| {
		let (alice, bob) = (account("Alice"), account("Bob"));
		assert_ok!(Owners::request_url_check(RuntimeOrigin::signed(bob), URL.to_vec()));
		let (hash, vote, intro, proof, salt) = ballot("Alice", &bob, &page_for(&bob));

		// Commits are accepted in blocks 2..=6.
		assert_noop!(
			Owners::commit_verification(RuntimeOrigin::signed(alice), URL.to_vec(), hash.clone()),
			Error::<Test>::OffTimeToCommit
		);
		run_to(2);
		assert_noop!(
			Owners::commit_verification(RuntimeOrigin::signed(alice), URL.to_vec(), vec![0; 31]),
			Error::<Test>::ExpectedHashWith32Bytes
		);
		assert_noop!(
			Owners::commit_verification(RuntimeOrigin::signed(bob), URL.to_vec(), hash.clone()),
			Error::<Test>::VerifierNotRegistered
		);
		assert_ok!(Owners::commit_verification(RuntimeOrigin::signed(alice), URL.to_vec(), hash));

		// Reveals are accepted in blocks 7..=11.
		let reveal = |intro: Vec<u8>, proof: Vec<u8>, salt: Vec<u8>| {
			Owners::reveal_verification(RuntimeOrigin::signed(alice), URL.to_vec(), vote, intro, proof, salt)
		};
		assert_noop!(reveal(intro.clone(), proof.clone(), salt.clone()), Error::<Test>::OffTimeToReveal);
		run_to(7);
		assert_noop!(reveal(intro.clone(), proof.clone(), vec![0; 64]), Error::<Test>::InvalidSalt);
		let mut other_intro = intro.clone();
		other_intro[0] = b'X';
		let forged = pair("Alice").sign(&vote_message(vote, &other_intro, &proof)).encode();
		assert_noop!(
			reveal(other_intro, proof.clone(), forged),
			Error::<Test>::MismatchBetweenCommitAndReveal
		);
		assert_noop!(
			reveal(intro.clone(), mark_for(&alice), salt.clone()),
			Error::<Test>::InvalidProofOfOwnership
		);
		assert_ok!(reveal(intro, proof.clone(), salt));

		// The tally runs at block 12.
		run_to(11);
		let url: crate::UrlOf<Test> = URL.to_vec().try_into().unwrap();
		assert_eq!(OwnersStore::<Test>::get(&url), None);
		run_to(12);
		let result = Results::<Test>::get(&url).unwrap();
		assert_eq!((result.0, result.1, result.2, result.3, result.6), (12, 1, 1, 1, true));
		assert_eq!(result.5.into_inner(), proof);
		assert_eq!(OwnersStore::<Test>::get(&url), Some(bob));
		assert_eq!(UrlCount::<Test>::get(), 1);
		assert_eq!(<Owners as OwnershipRegistry<AccountId>>::owner_of(URL), bob);
		System::assert_has_event(Event::UrlOwnerRegistered { account: bob, url: url.clone(), block: 12 }.into());

		// Alice's stats: one commit, one reveal, one valid yes vote with the majority.
		let stats = Verifiers::<Test>::get(alice).unwrap();
		assert_eq!((stats.2, stats.4, stats.6, stats.7, stats.8), (1, 1, 1, 1, 1));

		// Request data is deleted 100 blocks after the reveal window; ownership stays.
		run_to(111);
		assert!(Requests::<Test>::get(&url).is_some());
		run_to(112);
		assert!(Requests::<Test>::get(&url).is_none());
		assert!(Results::<Test>::get(&url).is_none());
		assert_eq!(OwnersStore::<Test>::get(&url), Some(bob));
	});
}

#[test]
fn a_no_vote_records_a_rejected_result() {
	new_test_ext().ext.execute_with(|| {
		let (alice, bob) = (account("Alice"), account("Bob"));
		assert_ok!(Owners::request_url_check(RuntimeOrigin::signed(bob), URL.to_vec()));
		let (hash, vote, intro, proof, salt) = ballot("Alice", &bob, b"<html>no mark here</html>");
		assert!(!vote);
		run_to(2);
		assert_ok!(Owners::commit_verification(RuntimeOrigin::signed(alice), URL.to_vec(), hash));
		run_to(7);
		assert_ok!(Owners::reveal_verification(RuntimeOrigin::signed(alice), URL.to_vec(), vote, intro, proof, salt));
		run_to(12);
		let url: crate::UrlOf<Test> = URL.to_vec().try_into().unwrap();
		let result = Results::<Test>::get(&url).unwrap();
		assert_eq!((result.1, result.2, result.6), (1, 0, false));
		assert_eq!(OwnersStore::<Test>::get(&url), None);
		assert_eq!(<Owners as OwnershipRegistry<AccountId>>::owner_of(URL), Owners::pot_id());
	});
}

#[test]
fn verifier_admin_is_root_only() {
	new_test_ext().ext.execute_with(|| {
		let bob = account("Bob");
		assert_noop!(
			Owners::add_verifier(RuntimeOrigin::signed(bob), bob),
			sp_runtime::DispatchError::BadOrigin
		);
		assert_ok!(Owners::add_verifier(RuntimeOrigin::root(), bob));
		assert_noop!(Owners::add_verifier(RuntimeOrigin::root(), bob), Error::<Test>::VerifierAlreadyRegistered);
		assert_ok!(Owners::disable_verifier(RuntimeOrigin::root(), bob));
		assert!(!Verifiers::<Test>::get(bob).unwrap().1);
		assert_ok!(Owners::enable_verifier(RuntimeOrigin::root(), bob));
		assert!(Verifiers::<Test>::get(bob).unwrap().1);
		assert_noop!(
			Owners::enable_verifier(RuntimeOrigin::root(), account("Nobody")),
			Error::<Test>::VerifierNotRegistered
		);
	});
}

fn expect_page(state: &mut testing::OffchainState, url: &str, body: Vec<u8>) {
	state.expect_request(testing::PendingRequest {
		method: "GET".into(),
		uri: url.into(),
		headers: vec![
			("Accept".into(), "text/html,application/xhtml+xml,application/xml".into()),
			("User-Agent".into(), USER_AGENT_FOR_TEST.into()),
		],
		response: Some(body),
		sent: true,
		..Default::default()
	});
}

const USER_AGENT_FOR_TEST: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
	(KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 WikaVerifier/4.0";

fn pop_call(pool: &parking_lot::RwLock<testing::PoolState>) -> RuntimeCall {
	let tx = pool.write().transactions.pop().expect("a transaction was submitted");
	Extrinsic::decode(&mut &*tx).unwrap().function
}

#[test]
fn offchain_worker_commits_then_reveals() {
	let Env { mut ext, offchain, pool } = new_test_ext();
	let bob = account("Bob");
	ext.execute_with(|| {
		assert_ok!(Owners::request_url_check(RuntimeOrigin::signed(bob), URL.to_vec()));
	});
	expect_page(&mut offchain.write(), core::str::from_utf8(URL).unwrap(), page_for(&bob));

	let hash = ext.execute_with(|| {
		Owners::offchain_worker(1);
		match pop_call(&pool) {
			RuntimeCall::Owners(crate::Call::commit_verification { url, hash }) => {
				assert_eq!(url, URL.to_vec());
				hash
			},
			other => panic!("unexpected call {other:?}"),
		}
	});

	ext.execute_with(|| {
		// Apply the commit as the chain would, then move to the end of the commit window.
		run_to(2);
		assert_ok!(Owners::commit_verification(RuntimeOrigin::signed(account("Alice")), URL.to_vec(), hash));
		run_to(6);
		Owners::offchain_worker(6);
		match pop_call(&pool) {
			RuntimeCall::Owners(crate::Call::reveal_verification { url, vote, intro, proof, salt }) => {
				assert_eq!(url, URL.to_vec());
				assert!(vote);
				run_to(7);
				assert_ok!(Owners::reveal_verification(
					RuntimeOrigin::signed(account("Alice")),
					url,
					vote,
					intro,
					proof,
					salt
				));
			},
			other => panic!("unexpected call {other:?}"),
		}
		run_to(12);
		let url: crate::UrlOf<Test> = URL.to_vec().try_into().unwrap();
		assert_eq!(OwnersStore::<Test>::get(&url), Some(bob));
	});
}

#[test]
fn offchain_worker_votes_no_when_the_mark_is_missing() {
	let Env { mut ext, offchain, pool } = new_test_ext();
	let bob = account("Bob");
	ext.execute_with(|| {
		assert_ok!(Owners::request_url_check(RuntimeOrigin::signed(bob), URL.to_vec()));
	});
	expect_page(&mut offchain.write(), core::str::from_utf8(URL).unwrap(), b"<html>hello</html>".to_vec());
	ext.execute_with(|| {
		Owners::offchain_worker(1);
		let hash = match pop_call(&pool) {
			RuntimeCall::Owners(crate::Call::commit_verification { hash, .. }) => hash,
			other => panic!("unexpected call {other:?}"),
		};
		run_to(2);
		assert_ok!(Owners::commit_verification(RuntimeOrigin::signed(account("Alice")), URL.to_vec(), hash));
		run_to(6);
		Owners::offchain_worker(6);
		match pop_call(&pool) {
			RuntimeCall::Owners(crate::Call::reveal_verification { vote, intro, proof, .. }) => {
				assert!(!vote);
				assert_eq!(intro, b"<html>hello</html>".to_vec());
				assert!(proof.is_empty());
			},
			other => panic!("unexpected call {other:?}"),
		}
	});
}

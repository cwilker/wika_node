use crate::{mock::*, Error, Event, Likes as LikesStore, UrlCount, Urls};
use frame_support::{assert_noop, assert_ok, traits::fungible::Inspect};

const URL: &[u8] = b"https://example.com/";
const REF: &[u8] = b"https://ref.example/";

fn url() -> crate::UrlOf<Test> {
	URL.to_vec().try_into().unwrap()
}

fn like(who: u64, n: u32) -> frame_support::dispatch::DispatchResult {
	Likes::like(RuntimeOrigin::signed(who), URL.to_vec(), REF.to_vec(), n)
}

fn balance(who: u64) -> u64 {
	Balances::balance(&who)
}

#[test]
fn first_like_creates_the_url_and_pays_every_share() {
	new_test_ext().execute_with(|| {
		MockOwners::set(URL, AUTHOR);
		let pot = Likes::pot_id();
		assert_ok!(like(ALICE, 3));

		// 3 likes at 1 UNIT: 33% author, 33% referrer (unclaimed, so the owners pot),
		// 33% previous likers (none yet, so the likes pot) and the 1% remainder to the likes pot.
		assert_eq!(balance(ALICE), 997 * UNIT);
		assert_eq!(balance(AUTHOR), UNIT + 3 * UNIT * 33 / 100);
		assert_eq!(balance(OWNERS_POT), UNIT + 3 * UNIT * 33 / 100);
		assert_eq!(balance(pot), 3 * UNIT * 34 / 100);

		assert_eq!(Urls::<Test>::get(url()), Some((3, ALICE, pot)));
		assert_eq!(LikesStore::<Test>::get(ALICE, url()), Some((0, 3, 12, pot)));
		assert_eq!(UrlCount::<Test>::get(), 1);
		System::assert_last_event(Event::Liked { who: ALICE, url: url(), num_likes: 3 }.into());
	});
}

#[test]
fn later_likes_repay_the_queue_front_first() {
	new_test_ext().execute_with(|| {
		let pot = Likes::pot_id();
		assert_ok!(like(ALICE, 1)); // Alice is owed 4.
		assert_ok!(like(BOB, 2)); // Bob is owed 8; pays Alice 2.

		let share = UNIT * 33 / 100;
		assert_eq!(balance(ALICE), 999 * UNIT + 2 * share);
		assert_eq!(LikesStore::<Test>::get(ALICE, url()), Some((0, 1, 2, BOB)));
		assert_eq!(LikesStore::<Test>::get(BOB, url()), Some((1, 2, 8, pot)));
		assert_eq!(Urls::<Test>::get(url()), Some((3, ALICE, BOB)));

		// Charlie's 5 likes finish Alice (2) and pay Bob 3.
		assert_ok!(like(CHARLIE, 5));
		assert_eq!(balance(ALICE), 999 * UNIT + 4 * share);
		assert_eq!(balance(BOB), 998 * UNIT + 3 * share);
		assert_eq!(LikesStore::<Test>::get(ALICE, url()), Some((0, 1, 0, BOB)));
		assert_eq!(LikesStore::<Test>::get(BOB, url()), Some((1, 2, 5, CHARLIE)));
		assert_eq!(Urls::<Test>::get(url()), Some((8, BOB, CHARLIE)));
		assert_eq!(UrlCount::<Test>::get(), 1);
	});
}

#[test]
fn likes_beyond_the_queue_go_to_the_pot() {
	new_test_ext().execute_with(|| {
		let pot = Likes::pot_id();
		assert_ok!(like(ALICE, 1)); // owed 4
		let pot_before = balance(pot);
		assert_ok!(like(BOB, 10)); // pays Alice 4, 6 left over

		let share = UNIT * 33 / 100;
		assert_eq!(balance(ALICE), 999 * UNIT + 4 * share);
		assert_eq!(balance(pot) - pot_before, 6 * share + 10 * UNIT / 100);
		// Alice is paid off, so Bob is now at the front.
		assert_eq!(Urls::<Test>::get(url()), Some((11, BOB, pot)));
	});
}

#[test]
fn the_referrer_page_owner_is_paid() {
	new_test_ext().execute_with(|| {
		MockOwners::set(REF, DAVE);
		assert_ok!(like(ALICE, 1));
		assert_eq!(balance(DAVE), 1_000 * UNIT + UNIT * 33 / 100);
	});
}

#[test]
fn one_like_per_account_per_url() {
	new_test_ext().execute_with(|| {
		assert_ok!(like(ALICE, 1));
		assert_noop!(like(ALICE, 1), Error::<Test>::AlreadyInQueue);
	});
}

#[test]
fn rejects_bad_input() {
	new_test_ext().execute_with(|| {
		assert_noop!(like(ALICE, 0), Error::<Test>::NoLikes);
		assert_noop!(like(ALICE, 101), Error::<Test>::TooManyLikes);
		assert_noop!(like(POOR, 1), Error::<Test>::NotEnoughBalanceToLike);
		let long = vec![b'a'; 255];
		assert_noop!(
			Likes::like(RuntimeOrigin::signed(ALICE), long, vec![], 1),
			Error::<Test>::UrlTooLong
		);
		assert_ok!(Likes::like(RuntimeOrigin::signed(ALICE), vec![b'a'; 254], vec![], 1));
	});
}

use crate::{mock::*, AuthCount, Authorities as Registry, Error};
use frame_support::{assert_noop, assert_ok};

fn aura_keys() -> Vec<[u8; 32]> {
	pallet_aura::Authorities::<Test>::get().iter().map(|a| { let raw: &[u8] = a.as_ref(); raw.try_into().unwrap() }).collect()
}

#[test]
fn genesis_registers_the_initial_authority() {
	new_test_ext().execute_with(|| {
		assert_eq!(AuthCount::<Test>::get(), 1);
		assert_eq!(Registry::<Test>::get(account(1)), Some((0, true, keys(1).0, keys(1).1)));
	});
}

#[test]
fn adding_an_authority_updates_aura_and_schedules_grandpa() {
	new_test_ext().execute_with(|| {
		let (sr, ed) = keys(2);
		assert_ok!(Authorities::add_authority(RuntimeOrigin::root(), account(2), sr, ed));
		assert_eq!(AuthCount::<Test>::get(), 2);
		assert_eq!(aura_keys(), vec![keys(1).0, keys(2).0]);
		let pending = pallet_grandpa::PendingChange::<Test>::get().expect("a change is scheduled");
		assert_eq!(pending.next_authorities.len(), 2);

		// GRANDPA accepts one change at a time.
		assert_noop!(Authorities::disable_authority(RuntimeOrigin::root(), account(2)), Error::<Test>::ChangePending);
		assert_noop!(
			Authorities::add_authority(RuntimeOrigin::root(), account(2), sr, ed),
			Error::<Test>::AuthorityAlreadyRegistered
		);
	});
}

#[test]
fn disabled_authorities_leave_the_set() {
	new_test_ext().execute_with(|| {
		let (sr, ed) = keys(2);
		assert_ok!(Authorities::add_authority(RuntimeOrigin::root(), account(2), sr, ed));
		pallet_grandpa::PendingChange::<Test>::kill();
		assert_ok!(Authorities::disable_authority(RuntimeOrigin::root(), account(1)));
		assert_eq!(aura_keys(), vec![keys(2).0]);
		assert!(!Registry::<Test>::get(account(1)).unwrap().1);
		pallet_grandpa::PendingChange::<Test>::kill();
		assert_ok!(Authorities::enable_authority(RuntimeOrigin::root(), account(1)));
		assert_eq!(aura_keys(), vec![keys(1).0, keys(2).0]);
	});
}

#[test]
fn cannot_disable_the_last_authority() {
	new_test_ext().execute_with(|| {
		assert_noop!(
			Authorities::disable_authority(RuntimeOrigin::root(), account(1)),
			Error::<Test>::NoAuthoritiesLeft
		);
		assert_noop!(
			Authorities::enable_authority(RuntimeOrigin::root(), account(9)),
			Error::<Test>::AuthorityNotRegistered
		);
	});
}

#[test]
fn only_root_manages_authorities() {
	new_test_ext().execute_with(|| {
		let (sr, ed) = keys(2);
		assert_noop!(
			Authorities::add_authority(RuntimeOrigin::signed(account(1)), account(2), sr, ed),
			sp_runtime::DispatchError::BadOrigin
		);
	});
}

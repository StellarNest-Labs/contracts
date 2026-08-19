#![cfg(test)]

use crate::types::Beneficiary;
use crate::{errors::Error, TreasuryContract, TreasuryContractClient};
use soroban_sdk::testutils::{Address as _, Ledger};
use soroban_sdk::{token, Address, Env, String, Vec};

fn setup() -> (Env, TreasuryContractClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(TreasuryContract, ());
    let client = TreasuryContractClient::new(&env, &contract_id);

    let asset_admin = Address::generate(&env);
    let sac_id = env.register_stellar_asset_contract_v2(asset_admin);
    let asset = sac_id.address();

    (env, client, asset)
}

fn mint(env: &Env, asset: &Address, to: &Address, amount: i128) {
    token::StellarAssetClient::new(env, asset).mint(to, &amount);
}

fn balance_of(env: &Env, asset: &Address, who: &Address) -> i128 {
    token::Client::new(env, asset).balance(who)
}

#[test]
fn create_treasury_registers_owner() {
    let (env, client, asset) = setup();
    let owner = Address::generate(&env);
    let id = client.create_treasury(
        &owner,
        &String::from_str(&env, "Adeyemi Family"),
        &asset,
        &1_000,
        &2,
    );
    let treasury = client.get_treasury(&id);
    assert_eq!(treasury.owner, owner);
    assert_eq!(treasury.balance, 0);
    let members = client.list_members(&id);
    assert_eq!(members.len(), 1);
}

#[test]
fn deposit_increases_balance() {
    let (env, client, asset) = setup();
    let owner = Address::generate(&env);
    let id = client.create_treasury(
        &owner,
        &String::from_str(&env, "Family"),
        &asset,
        &1_000,
        &2,
    );

    mint(&env, &asset, &owner, 5_000);
    client.deposit(&id, &owner, &2_000);

    let treasury = client.get_treasury(&id);
    assert_eq!(treasury.balance, 2_000);
}

#[test]
fn small_withdrawal_executes_immediately() {
    let (env, client, asset) = setup();
    let owner = Address::generate(&env);
    let child = Address::generate(&env);
    let id = client.create_treasury(
        &owner,
        &String::from_str(&env, "Family"),
        &asset,
        &1_000,
        &2,
    );

    mint(&env, &asset, &owner, 5_000);
    client.deposit(&id, &owner, &5_000);

    let withdrawal_id = client.request_withdrawal(&id, &owner, &child, &500);
    let withdrawal = client.get_withdrawal(&withdrawal_id);
    assert!(withdrawal.executed);
    assert_eq!(balance_of(&env, &asset, &child), 500);
    assert_eq!(client.get_treasury(&id).balance, 4_500);
}

#[test]
fn large_withdrawal_requires_approvals() {
    let (env, client, asset) = setup();
    let owner = Address::generate(&env);
    let parent2 = Address::generate(&env);
    let to = Address::generate(&env);
    let id = client.create_treasury(
        &owner,
        &String::from_str(&env, "Family"),
        &asset,
        &1_000,
        &2,
    );
    client.add_member(&id, &owner, &parent2, &crate::types::Role::Parent, &None);

    mint(&env, &asset, &owner, 10_000);
    client.deposit(&id, &owner, &10_000);

    let withdrawal_id = client.request_withdrawal(&id, &owner, &to, &5_000);
    let pending = client.get_withdrawal(&withdrawal_id);
    assert!(!pending.executed);
    assert_eq!(balance_of(&env, &asset, &to), 0);

    client.approve_withdrawal(&withdrawal_id, &owner);
    let still_pending = client.get_withdrawal(&withdrawal_id);
    assert!(!still_pending.executed);

    client.approve_withdrawal(&withdrawal_id, &parent2);
    let done = client.get_withdrawal(&withdrawal_id);
    assert!(done.executed);
    assert_eq!(balance_of(&env, &asset, &to), 5_000);
}

#[test]
fn double_approval_rejected() {
    let (env, client, asset) = setup();
    let owner = Address::generate(&env);
    let to = Address::generate(&env);
    let id = client.create_treasury(
        &owner,
        &String::from_str(&env, "Family"),
        &asset,
        &1_000,
        &2,
    );
    mint(&env, &asset, &owner, 10_000);
    client.deposit(&id, &owner, &10_000);
    let withdrawal_id = client.request_withdrawal(&id, &owner, &to, &5_000);
    client.approve_withdrawal(&withdrawal_id, &owner);
    let result = client.try_approve_withdrawal(&withdrawal_id, &owner);
    assert_eq!(result, Err(Ok(Error::AlreadyApproved)));
}

#[test]
fn spending_limit_enforced_for_child() {
    let (env, client, asset) = setup();
    let owner = Address::generate(&env);
    let child = Address::generate(&env);
    let to = Address::generate(&env);
    let id = client.create_treasury(
        &owner,
        &String::from_str(&env, "Family"),
        &asset,
        &1_000,
        &2,
    );
    client.add_member(&id, &owner, &child, &crate::types::Role::Child, &Some(100));
    mint(&env, &asset, &owner, 10_000);
    client.deposit(&id, &owner, &10_000);

    let result = client.try_request_withdrawal(&id, &child, &to, &200);
    assert_eq!(result, Err(Ok(Error::SpendingLimitExceeded)));

    let withdrawal_id = client.request_withdrawal(&id, &child, &to, &50);
    assert!(client.get_withdrawal(&withdrawal_id).executed);
}

#[test]
fn frozen_treasury_blocks_withdrawals() {
    let (env, client, asset) = setup();
    let owner = Address::generate(&env);
    let to = Address::generate(&env);
    let id = client.create_treasury(
        &owner,
        &String::from_str(&env, "Family"),
        &asset,
        &1_000,
        &2,
    );
    mint(&env, &asset, &owner, 10_000);
    client.deposit(&id, &owner, &10_000);

    client.freeze_treasury(&id, &owner);
    let result = client.try_request_withdrawal(&id, &owner, &to, &50);
    assert_eq!(result, Err(Ok(Error::TreasuryFrozen)));

    client.unfreeze_treasury(&id, &owner);
    let withdrawal_id = client.request_withdrawal(&id, &owner, &to, &50);
    assert!(client.get_withdrawal(&withdrawal_id).executed);
}

#[test]
fn savings_goal_progress_tracks_contributions() {
    let (env, client, asset) = setup();
    let owner = Address::generate(&env);
    let id = client.create_treasury(
        &owner,
        &String::from_str(&env, "Family"),
        &asset,
        &1_000,
        &2,
    );
    let goal_id = client.create_savings_goal(
        &id,
        &owner,
        &String::from_str(&env, "Emergency Fund"),
        &1_000,
    );

    mint(&env, &asset, &owner, 1_000);
    client.contribute_to_goal(&goal_id, &owner, &400);

    let goal = client.get_savings_goal(&goal_id);
    assert_eq!(goal.current_amount, 400);
    assert_eq!(client.get_treasury(&id).balance, 400);
}

#[test]
fn bill_pays_only_when_due() {
    let (env, client, asset) = setup();
    let owner = Address::generate(&env);
    let payee = Address::generate(&env);
    let id = client.create_treasury(
        &owner,
        &String::from_str(&env, "Family"),
        &asset,
        &1_000,
        &2,
    );
    mint(&env, &asset, &owner, 10_000);
    client.deposit(&id, &owner, &10_000);

    let bill_id = client.create_bill(
        &id,
        &owner,
        &String::from_str(&env, "Rent"),
        &payee,
        &1_000,
        &100,
    );

    let result = client.try_pay_bill(&bill_id);
    assert_eq!(result, Err(Ok(Error::BillNotDue)));

    env.ledger().with_mut(|l| l.sequence_number += 200);
    client.pay_bill(&bill_id);
    assert_eq!(balance_of(&env, &asset, &payee), 1_000);

    let bill = client.get_bill(&bill_id);
    assert!(bill.next_due_ledger > env.ledger().sequence());
}

#[test]
fn inheritance_vault_requires_allocation_sum_to_10000_bps() {
    let (env, client, asset) = setup();
    let owner = Address::generate(&env);
    let child = Address::generate(&env);
    let id = client.create_treasury(
        &owner,
        &String::from_str(&env, "Family"),
        &asset,
        &1_000,
        &2,
    );

    let mut beneficiaries = Vec::new(&env);
    beneficiaries.push_back(Beneficiary {
        address: child,
        allocation_bps: 5_000,
    });

    let result = client.try_create_inheritance_vault(&id, &owner, &beneficiaries, &1000, &1000, &1);
    assert_eq!(result, Err(Ok(Error::InvalidAllocation)));
}

#[test]
fn inheritance_claim_distributes_after_dead_man_switch() {
    let (env, client, asset) = setup();
    let owner = Address::generate(&env);
    let guardian = Address::generate(&env);
    let child1 = Address::generate(&env);
    let child2 = Address::generate(&env);
    let id = client.create_treasury(
        &owner,
        &String::from_str(&env, "Family"),
        &asset,
        &1_000_000,
        &5,
    );
    client.add_member(&id, &owner, &guardian, &crate::types::Role::Guardian, &None);

    mint(&env, &asset, &owner, 10_000);
    client.deposit(&id, &owner, &10_000);

    let mut beneficiaries = Vec::new(&env);
    beneficiaries.push_back(Beneficiary {
        address: child1.clone(),
        allocation_bps: 7_000,
    });
    beneficiaries.push_back(Beneficiary {
        address: child2.clone(),
        allocation_bps: 3_000,
    });

    client.create_inheritance_vault(&id, &owner, &beneficiaries, &100_000_000, &1_000, &1);

    let too_early = client.try_claim_inheritance(&id, &guardian);
    assert_eq!(too_early, Err(Ok(Error::VaultNotClaimable)));

    client.approve_inheritance_claim(&id, &guardian);
    env.ledger().with_mut(|l| l.sequence_number += 2_000);

    client.claim_inheritance(&id, &guardian);

    assert_eq!(balance_of(&env, &asset, &child1), 7_000);
    assert_eq!(balance_of(&env, &asset, &child2), 3_000);
    assert_eq!(client.get_treasury(&id).balance, 0);
    assert!(client.get_inheritance_vault(&id).claimed);
}

#[test]
fn heartbeat_resets_dead_man_switch() {
    let (env, client, asset) = setup();
    let owner = Address::generate(&env);
    let guardian = Address::generate(&env);
    let child = Address::generate(&env);
    let id = client.create_treasury(
        &owner,
        &String::from_str(&env, "Family"),
        &asset,
        &1_000_000,
        &5,
    );
    client.add_member(&id, &owner, &guardian, &crate::types::Role::Guardian, &None);
    mint(&env, &asset, &owner, 1_000);
    client.deposit(&id, &owner, &1_000);

    let mut beneficiaries = Vec::new(&env);
    beneficiaries.push_back(Beneficiary {
        address: child,
        allocation_bps: 10_000,
    });
    client.create_inheritance_vault(&id, &owner, &beneficiaries, &100_000_000, &1_000, &1);

    env.ledger().with_mut(|l| l.sequence_number += 500);
    client.heartbeat(&id, &owner);
    client.approve_inheritance_claim(&id, &guardian);

    env.ledger().with_mut(|l| l.sequence_number += 500);
    let result = client.try_claim_inheritance(&id, &guardian);
    assert_eq!(result, Err(Ok(Error::VaultNotClaimable)));
}

#[test]
fn non_administrator_cannot_add_members() {
    let (env, client, asset) = setup();
    let owner = Address::generate(&env);
    let viewer = Address::generate(&env);
    let stranger = Address::generate(&env);
    let id = client.create_treasury(
        &owner,
        &String::from_str(&env, "Family"),
        &asset,
        &1_000,
        &2,
    );
    client.add_member(&id, &owner, &viewer, &crate::types::Role::Viewer, &None);

    let result = client.try_add_member(&id, &viewer, &stranger, &crate::types::Role::Child, &None);
    assert_eq!(result, Err(Ok(Error::NotAuthorized)));
}

#[test]
fn cancelled_bill_cannot_be_paid() {
    let (env, client, asset) = setup();
    let owner = Address::generate(&env);
    let payee = Address::generate(&env);
    let id = client.create_treasury(
        &owner,
        &String::from_str(&env, "Family"),
        &asset,
        &1_000,
        &2,
    );
    mint(&env, &asset, &owner, 1_000);
    client.deposit(&id, &owner, &1_000);
    let bill_id = client.create_bill(
        &id,
        &owner,
        &String::from_str(&env, "Internet"),
        &payee,
        &100,
        &10,
    );

    client.cancel_bill(&bill_id, &owner);
    env.ledger().with_mut(|l| l.sequence_number += 100);
    let result = client.try_pay_bill(&bill_id);
    assert_eq!(result, Err(Ok(Error::BillNotFound)));
}

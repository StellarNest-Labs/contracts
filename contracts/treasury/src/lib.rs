#![no_std]

mod errors;
mod types;

#[cfg(test)]
mod test;

use errors::Error;
use soroban_sdk::{contract, contractimpl, token, Address, Env, String, Vec};
use types::{Bill, DataKey, InheritanceVault, Member, Role, SavingsGoal, Treasury, Withdrawal};

#[contract]
pub struct TreasuryContract;

#[contractimpl]
impl TreasuryContract {
    // ---------------------------------------------------------------
    // Treasury lifecycle
    // ---------------------------------------------------------------

    /// Create a new family treasury denominated in `asset` (a Stellar Asset Contract address).
    /// `approval_threshold` / `required_approvals` gate withdrawals above that amount.
    pub fn create_treasury(
        env: Env,
        owner: Address,
        name: String,
        asset: Address,
        approval_threshold: i128,
        required_approvals: u32,
    ) -> u64 {
        owner.require_auth();

        let id = next_id(&env, &DataKey::NextTreasuryId);
        let treasury = Treasury {
            id,
            name,
            owner: owner.clone(),
            asset,
            balance: 0,
            frozen: false,
            approval_threshold,
            required_approvals: required_approvals.max(1),
            created_at: env.ledger().timestamp(),
        };
        env.storage()
            .persistent()
            .set(&DataKey::Treasury(id), &treasury);

        let member = Member {
            role: Role::Owner,
            spending_limit: None,
            joined_at: env.ledger().timestamp(),
        };
        env.storage()
            .persistent()
            .set(&DataKey::Member(id, owner.clone()), &member);
        let members: Vec<Address> = Vec::from_array(&env, [owner]);
        env.storage()
            .persistent()
            .set(&DataKey::MemberList(id), &members);

        id
    }

    pub fn get_treasury(env: Env, treasury_id: u64) -> Result<Treasury, Error> {
        load_treasury(&env, treasury_id)
    }

    pub fn freeze_treasury(env: Env, treasury_id: u64, caller: Address) -> Result<(), Error> {
        caller.require_auth();
        let mut treasury = load_treasury(&env, treasury_id)?;
        require_role(&env, &treasury, &caller, |r| {
            r.can_administer() || matches!(r, Role::Guardian)
        })?;
        treasury.frozen = true;
        env.storage()
            .persistent()
            .set(&DataKey::Treasury(treasury_id), &treasury);
        Ok(())
    }

    pub fn unfreeze_treasury(env: Env, treasury_id: u64, caller: Address) -> Result<(), Error> {
        caller.require_auth();
        let mut treasury = load_treasury(&env, treasury_id)?;
        require_role(&env, &treasury, &caller, Role::can_administer)?;
        treasury.frozen = false;
        env.storage()
            .persistent()
            .set(&DataKey::Treasury(treasury_id), &treasury);
        Ok(())
    }

    // ---------------------------------------------------------------
    // Members & roles
    // ---------------------------------------------------------------

    pub fn add_member(
        env: Env,
        treasury_id: u64,
        caller: Address,
        member: Address,
        role: Role,
        spending_limit: Option<i128>,
    ) -> Result<(), Error> {
        caller.require_auth();
        let treasury = load_treasury(&env, treasury_id)?;
        require_role(&env, &treasury, &caller, Role::can_administer)?;

        if env
            .storage()
            .persistent()
            .has(&DataKey::Member(treasury_id, member.clone()))
        {
            return Err(Error::MemberAlreadyExists);
        }

        let record = Member {
            role,
            spending_limit,
            joined_at: env.ledger().timestamp(),
        };
        env.storage()
            .persistent()
            .set(&DataKey::Member(treasury_id, member.clone()), &record);

        let mut list: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::MemberList(treasury_id))
            .unwrap_or_else(|| Vec::new(&env));
        list.push_back(member);
        env.storage()
            .persistent()
            .set(&DataKey::MemberList(treasury_id), &list);
        Ok(())
    }

    pub fn remove_member(
        env: Env,
        treasury_id: u64,
        caller: Address,
        member: Address,
    ) -> Result<(), Error> {
        caller.require_auth();
        let treasury = load_treasury(&env, treasury_id)?;
        require_role(&env, &treasury, &caller, Role::can_administer)?;

        env.storage()
            .persistent()
            .remove(&DataKey::Member(treasury_id, member.clone()));

        let list: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::MemberList(treasury_id))
            .unwrap_or_else(|| Vec::new(&env));
        let mut updated = Vec::new(&env);
        for addr in list.iter() {
            if addr != member {
                updated.push_back(addr);
            }
        }
        env.storage()
            .persistent()
            .set(&DataKey::MemberList(treasury_id), &updated);
        Ok(())
    }

    pub fn get_member(env: Env, treasury_id: u64, member: Address) -> Result<Member, Error> {
        load_member(&env, treasury_id, &member)
    }

    pub fn list_members(env: Env, treasury_id: u64) -> Vec<Address> {
        env.storage()
            .persistent()
            .get(&DataKey::MemberList(treasury_id))
            .unwrap_or_else(|| Vec::new(&env))
    }

    // ---------------------------------------------------------------
    // Rules engine
    // ---------------------------------------------------------------

    pub fn set_approval_rule(
        env: Env,
        treasury_id: u64,
        caller: Address,
        approval_threshold: i128,
        required_approvals: u32,
    ) -> Result<(), Error> {
        caller.require_auth();
        let mut treasury = load_treasury(&env, treasury_id)?;
        require_role(&env, &treasury, &caller, Role::can_administer)?;
        treasury.approval_threshold = approval_threshold;
        treasury.required_approvals = required_approvals.max(1);
        env.storage()
            .persistent()
            .set(&DataKey::Treasury(treasury_id), &treasury);
        Ok(())
    }

    // ---------------------------------------------------------------
    // Deposits & withdrawals
    // ---------------------------------------------------------------

    pub fn deposit(env: Env, treasury_id: u64, from: Address, amount: i128) -> Result<(), Error> {
        from.require_auth();
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let mut treasury = load_treasury(&env, treasury_id)?;

        let token_client = token::Client::new(&env, &treasury.asset);
        token_client.transfer(&from, &env.current_contract_address(), &amount);

        treasury.balance += amount;
        env.storage()
            .persistent()
            .set(&DataKey::Treasury(treasury_id), &treasury);
        Ok(())
    }

    /// Request a withdrawal. If the amount is within the caller's spending limit and below the
    /// treasury's approval threshold, it executes immediately. Otherwise it opens a pending
    /// withdrawal that must collect `required_approvals` from Owner/Parent/Guardian members.
    pub fn request_withdrawal(
        env: Env,
        treasury_id: u64,
        caller: Address,
        to: Address,
        amount: i128,
    ) -> Result<u64, Error> {
        caller.require_auth();
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let treasury = load_treasury(&env, treasury_id)?;
        if treasury.frozen {
            return Err(Error::TreasuryFrozen);
        }
        if treasury.balance < amount {
            return Err(Error::InsufficientBalance);
        }
        let member = load_member(&env, treasury_id, &caller)?;
        if !member.role.can_spend() {
            return Err(Error::NotAuthorized);
        }
        if let Some(limit) = member.spending_limit {
            if amount > limit {
                return Err(Error::SpendingLimitExceeded);
            }
        }

        let id = next_id(&env, &DataKey::NextWithdrawalId);

        if amount < treasury.approval_threshold {
            execute_transfer(&env, treasury_id, &to, amount)?;
            let record = Withdrawal {
                id,
                treasury_id,
                requested_by: caller,
                to,
                amount,
                approvals: Vec::new(&env),
                executed: true,
            };
            env.storage()
                .persistent()
                .set(&DataKey::Withdrawal(id), &record);
        } else {
            let record = Withdrawal {
                id,
                treasury_id,
                requested_by: caller,
                to,
                amount,
                approvals: Vec::new(&env),
                executed: false,
            };
            env.storage()
                .persistent()
                .set(&DataKey::Withdrawal(id), &record);
        }
        Ok(id)
    }

    pub fn approve_withdrawal(
        env: Env,
        withdrawal_id: u64,
        approver: Address,
    ) -> Result<(), Error> {
        approver.require_auth();
        let mut withdrawal: Withdrawal = env
            .storage()
            .persistent()
            .get(&DataKey::Withdrawal(withdrawal_id))
            .ok_or(Error::WithdrawalNotFound)?;

        if withdrawal.executed {
            return Err(Error::WithdrawalAlreadyExecuted);
        }
        let treasury = load_treasury(&env, withdrawal.treasury_id)?;
        if treasury.frozen {
            return Err(Error::TreasuryFrozen);
        }
        let member = load_member(&env, withdrawal.treasury_id, &approver)?;
        if !member.role.can_approve() {
            return Err(Error::NotAuthorized);
        }
        if withdrawal.approvals.contains(&approver) {
            return Err(Error::AlreadyApproved);
        }
        withdrawal.approvals.push_back(approver);

        if withdrawal.approvals.len() >= treasury.required_approvals {
            execute_transfer(
                &env,
                withdrawal.treasury_id,
                &withdrawal.to,
                withdrawal.amount,
            )?;
            withdrawal.executed = true;
        }
        env.storage()
            .persistent()
            .set(&DataKey::Withdrawal(withdrawal_id), &withdrawal);
        Ok(())
    }

    pub fn get_withdrawal(env: Env, withdrawal_id: u64) -> Result<Withdrawal, Error> {
        env.storage()
            .persistent()
            .get(&DataKey::Withdrawal(withdrawal_id))
            .ok_or(Error::WithdrawalNotFound)
    }

    // ---------------------------------------------------------------
    // Savings goals
    // ---------------------------------------------------------------

    pub fn create_savings_goal(
        env: Env,
        treasury_id: u64,
        caller: Address,
        name: String,
        target_amount: i128,
    ) -> Result<u64, Error> {
        caller.require_auth();
        let treasury = load_treasury(&env, treasury_id)?;
        require_role(&env, &treasury, &caller, Role::can_administer)?;

        let id = next_id(&env, &DataKey::NextGoalId);
        let goal = SavingsGoal {
            id,
            treasury_id,
            name,
            target_amount,
            current_amount: 0,
            created_at: env.ledger().timestamp(),
        };
        env.storage().persistent().set(&DataKey::Goal(id), &goal);

        let mut list: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::GoalList(treasury_id))
            .unwrap_or_else(|| Vec::new(&env));
        list.push_back(id);
        env.storage()
            .persistent()
            .set(&DataKey::GoalList(treasury_id), &list);
        Ok(id)
    }

    pub fn contribute_to_goal(
        env: Env,
        goal_id: u64,
        from: Address,
        amount: i128,
    ) -> Result<(), Error> {
        from.require_auth();
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let mut goal: SavingsGoal = env
            .storage()
            .persistent()
            .get(&DataKey::Goal(goal_id))
            .ok_or(Error::GoalNotFound)?;
        let mut treasury = load_treasury(&env, goal.treasury_id)?;

        let token_client = token::Client::new(&env, &treasury.asset);
        token_client.transfer(&from, &env.current_contract_address(), &amount);

        treasury.balance += amount;
        goal.current_amount += amount;

        env.storage()
            .persistent()
            .set(&DataKey::Treasury(goal.treasury_id), &treasury);
        env.storage()
            .persistent()
            .set(&DataKey::Goal(goal_id), &goal);
        Ok(())
    }

    pub fn get_savings_goal(env: Env, goal_id: u64) -> Result<SavingsGoal, Error> {
        env.storage()
            .persistent()
            .get(&DataKey::Goal(goal_id))
            .ok_or(Error::GoalNotFound)
    }

    pub fn list_savings_goals(env: Env, treasury_id: u64) -> Vec<u64> {
        env.storage()
            .persistent()
            .get(&DataKey::GoalList(treasury_id))
            .unwrap_or_else(|| Vec::new(&env))
    }

    // ---------------------------------------------------------------
    // Bills & recurring payments (also used for child allowances)
    // ---------------------------------------------------------------

    pub fn create_bill(
        env: Env,
        treasury_id: u64,
        caller: Address,
        name: String,
        payee: Address,
        amount: i128,
        interval_ledgers: u32,
    ) -> Result<u64, Error> {
        caller.require_auth();
        let treasury = load_treasury(&env, treasury_id)?;
        require_role(&env, &treasury, &caller, Role::can_administer)?;

        let id = next_id(&env, &DataKey::NextBillId);
        let bill = Bill {
            id,
            treasury_id,
            name,
            payee,
            amount,
            interval_ledgers,
            next_due_ledger: env.ledger().sequence() + interval_ledgers,
            active: true,
        };
        env.storage().persistent().set(&DataKey::Bill(id), &bill);

        let mut list: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::BillList(treasury_id))
            .unwrap_or_else(|| Vec::new(&env));
        list.push_back(id);
        env.storage()
            .persistent()
            .set(&DataKey::BillList(treasury_id), &list);
        Ok(id)
    }

    /// Executes a due bill. Callable by anyone (typically a relayer/cron), but only pays out if
    /// the schedule says it's due, keeping payment timing on-chain and tamper-proof.
    pub fn pay_bill(env: Env, bill_id: u64) -> Result<(), Error> {
        let mut bill: Bill = env
            .storage()
            .persistent()
            .get(&DataKey::Bill(bill_id))
            .ok_or(Error::BillNotFound)?;
        if !bill.active {
            return Err(Error::BillNotFound);
        }
        if env.ledger().sequence() < bill.next_due_ledger {
            return Err(Error::BillNotDue);
        }
        execute_transfer(&env, bill.treasury_id, &bill.payee, bill.amount)?;
        bill.next_due_ledger = env.ledger().sequence() + bill.interval_ledgers;
        env.storage()
            .persistent()
            .set(&DataKey::Bill(bill_id), &bill);
        Ok(())
    }

    pub fn cancel_bill(env: Env, bill_id: u64, caller: Address) -> Result<(), Error> {
        caller.require_auth();
        let mut bill: Bill = env
            .storage()
            .persistent()
            .get(&DataKey::Bill(bill_id))
            .ok_or(Error::BillNotFound)?;
        let treasury = load_treasury(&env, bill.treasury_id)?;
        require_role(&env, &treasury, &caller, Role::can_administer)?;
        bill.active = false;
        env.storage()
            .persistent()
            .set(&DataKey::Bill(bill_id), &bill);
        Ok(())
    }

    pub fn get_bill(env: Env, bill_id: u64) -> Result<Bill, Error> {
        env.storage()
            .persistent()
            .get(&DataKey::Bill(bill_id))
            .ok_or(Error::BillNotFound)
    }

    pub fn list_bills(env: Env, treasury_id: u64) -> Vec<u64> {
        env.storage()
            .persistent()
            .get(&DataKey::BillList(treasury_id))
            .unwrap_or_else(|| Vec::new(&env))
    }

    // ---------------------------------------------------------------
    // Inheritance vault
    // ---------------------------------------------------------------

    pub fn create_inheritance_vault(
        env: Env,
        treasury_id: u64,
        caller: Address,
        beneficiaries: Vec<types::Beneficiary>,
        time_lock_ledger: u32,
        dead_man_switch_period: u32,
        guardian_approvals_required: u32,
    ) -> Result<(), Error> {
        caller.require_auth();
        let treasury = load_treasury(&env, treasury_id)?;
        require_role(&env, &treasury, &caller, |r| matches!(r, Role::Owner))?;

        if env.storage().persistent().has(&DataKey::Vault(treasury_id)) {
            return Err(Error::VaultAlreadyExists);
        }

        let mut total_bps: u32 = 0;
        for b in beneficiaries.iter() {
            total_bps += b.allocation_bps;
        }
        if total_bps != 10_000 {
            return Err(Error::InvalidAllocation);
        }

        let vault = InheritanceVault {
            treasury_id,
            beneficiaries,
            time_lock_ledger,
            dead_man_switch_period,
            last_heartbeat_ledger: env.ledger().sequence(),
            guardian_approvals_required,
            guardian_approvals: Vec::new(&env),
            claimed: false,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Vault(treasury_id), &vault);
        Ok(())
    }

    /// The owner calls this periodically to prove they're active and reset the dead-man switch.
    pub fn heartbeat(env: Env, treasury_id: u64, caller: Address) -> Result<(), Error> {
        caller.require_auth();
        let treasury = load_treasury(&env, treasury_id)?;
        require_role(&env, &treasury, &caller, |r| matches!(r, Role::Owner))?;

        let mut vault = load_vault(&env, treasury_id)?;
        vault.last_heartbeat_ledger = env.ledger().sequence();
        env.storage()
            .persistent()
            .set(&DataKey::Vault(treasury_id), &vault);
        Ok(())
    }

    pub fn approve_inheritance_claim(
        env: Env,
        treasury_id: u64,
        guardian: Address,
    ) -> Result<(), Error> {
        guardian.require_auth();
        let treasury = load_treasury(&env, treasury_id)?;
        let member = load_member(&env, treasury_id, &guardian)?;
        if !matches!(member.role, Role::Guardian) {
            return Err(Error::NotAuthorized);
        }
        let _ = treasury;

        let mut vault = load_vault(&env, treasury_id)?;
        if vault.guardian_approvals.contains(&guardian) {
            return Err(Error::AlreadyApproved);
        }
        vault.guardian_approvals.push_back(guardian);
        env.storage()
            .persistent()
            .set(&DataKey::Vault(treasury_id), &vault);
        Ok(())
    }

    /// Distributes the treasury balance to beneficiaries once either the time-lock has passed or
    /// the dead-man switch has expired, and enough guardians have approved the claim.
    pub fn claim_inheritance(env: Env, treasury_id: u64, caller: Address) -> Result<(), Error> {
        caller.require_auth();
        let mut treasury = load_treasury(&env, treasury_id)?;
        let mut vault = load_vault(&env, treasury_id)?;

        if vault.claimed {
            return Err(Error::VaultNotClaimable);
        }

        let now = env.ledger().sequence();
        let time_lock_passed = now >= vault.time_lock_ledger;
        let switch_expired = now >= vault.last_heartbeat_ledger + vault.dead_man_switch_period;
        let enough_guardians = vault.guardian_approvals.len() >= vault.guardian_approvals_required;

        if !((time_lock_passed || switch_expired) && enough_guardians) {
            return Err(Error::VaultNotClaimable);
        }

        let token_client = token::Client::new(&env, &treasury.asset);
        let total = treasury.balance;
        for b in vault.beneficiaries.iter() {
            let share = (total * b.allocation_bps as i128) / 10_000;
            if share > 0 {
                token_client.transfer(&env.current_contract_address(), &b.address, &share);
            }
        }
        treasury.balance = 0;
        vault.claimed = true;

        env.storage()
            .persistent()
            .set(&DataKey::Treasury(treasury_id), &treasury);
        env.storage()
            .persistent()
            .set(&DataKey::Vault(treasury_id), &vault);
        Ok(())
    }

    pub fn get_inheritance_vault(env: Env, treasury_id: u64) -> Result<InheritanceVault, Error> {
        load_vault(&env, treasury_id)
    }
}

// ---------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------

fn next_id(env: &Env, key: &DataKey) -> u64 {
    let current: u64 = env.storage().instance().get(key).unwrap_or(0);
    let next = current + 1;
    env.storage().instance().set(key, &next);
    next
}

fn load_treasury(env: &Env, treasury_id: u64) -> Result<Treasury, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::Treasury(treasury_id))
        .ok_or(Error::TreasuryNotFound)
}

fn load_member(env: &Env, treasury_id: u64, address: &Address) -> Result<Member, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::Member(treasury_id, address.clone()))
        .ok_or(Error::MemberNotFound)
}

fn load_vault(env: &Env, treasury_id: u64) -> Result<InheritanceVault, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::Vault(treasury_id))
        .ok_or(Error::VaultNotFound)
}

fn require_role(
    env: &Env,
    treasury: &Treasury,
    caller: &Address,
    predicate: impl Fn(&Role) -> bool,
) -> Result<(), Error> {
    if caller == &treasury.owner {
        return Ok(());
    }
    let member = load_member(env, treasury.id, caller)?;
    if predicate(&member.role) {
        Ok(())
    } else {
        Err(Error::NotAuthorized)
    }
}

fn execute_transfer(env: &Env, treasury_id: u64, to: &Address, amount: i128) -> Result<(), Error> {
    let mut treasury = load_treasury(env, treasury_id)?;
    if treasury.frozen {
        return Err(Error::TreasuryFrozen);
    }
    if treasury.balance < amount {
        return Err(Error::InsufficientBalance);
    }
    let token_client = token::Client::new(env, &treasury.asset);
    token_client.transfer(&env.current_contract_address(), to, &amount);
    treasury.balance -= amount;
    env.storage()
        .persistent()
        .set(&DataKey::Treasury(treasury_id), &treasury);
    Ok(())
}

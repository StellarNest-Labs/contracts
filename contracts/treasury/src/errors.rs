use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    NotAuthorized = 1,
    TreasuryNotFound = 2,
    MemberNotFound = 3,
    MemberAlreadyExists = 4,
    InsufficientBalance = 5,
    TreasuryFrozen = 6,
    GoalNotFound = 7,
    BillNotFound = 8,
    BillNotDue = 9,
    WithdrawalNotFound = 10,
    AlreadyApproved = 11,
    WithdrawalAlreadyExecuted = 12,
    VaultNotFound = 13,
    VaultAlreadyExists = 14,
    VaultNotClaimable = 15,
    InvalidAllocation = 16,
    InvalidAmount = 17,
    SpendingLimitExceeded = 18,
}

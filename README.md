# <img src="assets/logo.svg" width="32" height="32" align="center" alt="" /> StellarNest — Contracts

Soroban smart contracts powering **StellarNest**, a family financial
coordination platform on Stellar. Instead of a joint bank account or a
shared spreadsheet, a family gets one programmable treasury: shared
savings, bill automation, spend approvals, and inheritance — all enforced
on-chain, not by convention or a support team's goodwill.

> One treasury. One family. Infinite trust.

This repo contains a single contract, `treasury`, plus the tooling to
build, test, and deploy it. It's one of three StellarNest repos:

| Repo | Purpose |
|---|---|
| [`contracts`](https://github.com/StellarNest-Org/contracts) *(this repo)* | The Soroban `treasury` contract |
| [`backend`](https://github.com/StellarNest-Org/backend) | GraphQL API, Postgres data layer, non-custodial Stellar integration |
| [`frontend`](https://github.com/StellarNest-Org/frontend) | Marketing site + product preview (Next.js) |

## Table of contents

- [New to Stellar/Soroban? Start here](#new-to-stellarsoroban-start-here)
- [Why a smart contract, not just a database](#why-a-smart-contract-not-just-a-database)
- [Core model](#core-model)
- [Method reference](#method-reference)
- [Errors](#errors)
- [Storage layout](#storage-layout)
- [A walkthrough: the Adeyemi family treasury](#a-walkthrough-the-adeyemi-family-treasury)
- [Development](#development)
- [Testing](#testing)
- [Deployment](#deployment)
- [Design principles](#design-principles)
- [Contributing](#contributing)

## New to Stellar/Soroban? Start here

You don't need blockchain experience to read this repo, but a few terms
come up constantly. Here's what they mean, in plain language:

- **Stellar** is a public payments network/blockchain — like a shared,
  global ledger that anyone can read and, with the right permission,
  write to. It's optimized for moving money: transactions confirm in
  3–5 seconds and cost a fraction of a cent, which is why it's a
  reasonable foundation for something as routine as a weekly allowance.
- **Soroban** is Stellar's smart contract platform. A "smart contract"
  is just a program that lives on the ledger itself — once deployed,
  its code runs exactly as written, and nobody (including the people
  who wrote it) can quietly change what it does without deploying a new
  version. That's the whole reason this project uses one: a spending
  rule enforced by a smart contract is a rule that actually holds, not
  a policy someone could bend.
- **A contract instance** is one deployment of this code, identified by
  a unique **contract id** (a long string like `CABC...`). StellarNest
  deploys the `treasury` contract *once* and every family's treasury
  lives inside that same instance, distinguished by a `treasury_id`
  number — see [Core model](#core-model) for why.
- **An account / address** on Stellar is identified by a public key
  (starting with `G...` for a regular account, `C...` for a contract).
  Every person interacting with a treasury — Amara, Chidi, a guardian —
  has their own Stellar address, the same way they'd have their own
  bank account number.
- **Signing / authorization.** Nothing moves on Stellar without a
  signature from the relevant private key. In this contract, that shows
  up as `caller.require_auth()` — a line that says "this call only
  proceeds if `caller` has cryptographically proven they authorized it
  in this exact transaction." Nobody, including StellarNest, can forge
  that.
- **XDR** is Stellar's wire format for transactions — a blob of bytes
  that represents "do this operation." You'll see it mentioned in the
  [backend README](https://github.com/StellarNest-Org/backend): the
  usual flow is *build unsigned XDR → the user's wallet signs it →
  submit the signed XDR to the network*. This contract doesn't deal
  with XDR directly; that happens one layer up.
- **The ledger** is Stellar's version of a "block" — a batch of
  confirmed transactions with a sequence number that increases roughly
  every 5 seconds. This contract uses ledger sequence numbers (not wall
  clock time) to track when a bill is due or when a dead-man switch
  expires, because the ledger's sequence number is the one clock every
  node on the network agrees on.
- **A Stellar Asset Contract (SAC)** is how a currency — XLM, USDC,
  EURC, a custom token — shows up to a Soroban contract: as another
  contract with a standard `transfer`/`balance` interface
  (`token::Client` in this codebase). This contract never invents its
  own accounting for money; it always calls into the real asset's
  contract to move it.
- **Testnet vs. mainnet.** Testnet is Stellar's free practice network —
  same software, fake money, safe to break things on. Mainnet is the
  real network with real value. `scripts/deploy.sh` can target either.

If you're comfortable with those seven ideas, the rest of this README
should read like a normal backend service's docs — because that's
mostly what a Soroban contract is: a small, extremely strict backend
that happens to run somewhere nobody (not even its author) can quietly
edit it.

## Why a smart contract, not just a database

A rule that lives in application code is a rule someone can quietly change
— an engineer under pressure, a compromised admin panel, a support ticket
that "just this once" bypasses the approval flow. A rule enforced by a
Soroban contract can't be: the contract checks the approval threshold,
the spending limit, and the freeze state *before it will move a single
token*, regardless of what the backend or frontend tell it to do. That
property — rules even StellarNest itself can't override — is the whole
reason this is a contract and not just a Postgres table.

## Core model

`contracts/treasury` is a **single deployed contract instance** that
manages *many* family treasuries, each identified by a `u64` id (the same
multi-tenant-per-contract pattern used by most production Soroban apps,
rather than deploying a fresh contract per family). It intentionally holds
**no custodial keys** — every state-changing call requires the caller's
own `require_auth()`, and every asset movement happens through the
standard Stellar Asset Contract `token` interface (`token::Client`), never
through an internal ledger only the contract admin can move.

| Concept | What it is |
|---|---|
| **Treasury** | One per family. Denominated in a single Stellar asset (XLM, USDC, EURC, AQUA, or any Stellar Asset Contract). Tracks `balance`, `frozen` state, and the approval rule (`approval_threshold`, `required_approvals`). |
| **Members & roles** | `Owner`, `Parent`, `Guardian`, `Child`, `Advisor`, `Viewer`. Roles gate which calls succeed (see [Method reference](#method-reference)). A `Child` member can additionally carry a per-transaction `spending_limit`. |
| **Rules engine** | Withdrawals *below* `approval_threshold` execute immediately, subject to the caller's spending limit. Withdrawals *at or above* it become a pending `Withdrawal` that needs `required_approvals` signatures from Owner/Parent/Guardian members before funds move. |
| **Savings goals** | Named targets (`Emergency Fund`, `Vacation`, ...) with their own `current_amount`, tracked independently of the treasury's free balance so progress is easy to display without a separate ledger. |
| **Bills** | Recurring payments with an on-chain due schedule (`next_due_ledger`). `pay_bill` is **permissionless** but only succeeds once the schedule says it's due — a cron job or relayer can call it without ever being trusted with payment timing or amounts. |
| **Inheritance vault** | Beneficiaries with *basis-point* allocations — basis points are just percentages with more precision (100 bps = 1%, so 10,000 bps = 100%); the contract requires every beneficiary's share to sum to exactly `10_000` so nothing is over- or under-allocated. A `time_lock_ledger` (see [glossary](#new-to-stellarsoroban-start-here) — this is a ledger *sequence number*, not a calendar date) and a dead-man switch the Owner resets via `heartbeat` both gate the claim. Claiming requires guardian approvals **and** either the time-lock or the dead-man switch to have elapsed, then distributes the full treasury balance pro-rata (proportionally, by each beneficiary's percentage) in one transaction. |

Roles map to permissions like this (see `Role::can_administer`,
`Role::can_approve`, `Role::can_spend` in `types.rs`):

| Role | Administer (members/rules/goals/bills) | Approve withdrawals | Can request a spend | Notes |
|---|:---:|:---:|:---:|---|
| Owner | ✅ | ✅ | ✅ | Also the only role that can create/heartbeat the inheritance vault |
| Parent | ✅ | ✅ | ✅ | |
| Guardian | freeze only | ✅ | ✅ | Also the only role that can approve inheritance claims |
| Child | ❌ | ❌ | ✅ (subject to `spending_limit`) | |
| Advisor | ❌ | ❌ | ✅ | |
| Viewer | ❌ | ❌ | ❌ | Read-only |

## Method reference

All methods live on `TreasuryContract` in `contracts/treasury/src/lib.rs`.
Every state-changing call takes an explicit `caller: Address` parameter
and calls `caller.require_auth()` — there is no `msg.sender` equivalent in
Soroban, so the caller is always passed and authenticated explicitly.

**Treasury lifecycle**

| Method | Auth | Description |
|---|---|---|
| `create_treasury(owner, name, asset, approval_threshold, required_approvals)` | `owner` | Creates a treasury, registers `owner` as its first `Owner` member. Returns the new `treasury_id`. |
| `get_treasury(treasury_id)` | none | Read-only. |
| `freeze_treasury(treasury_id, caller)` | Owner/Parent/Guardian | Blocks new withdrawals immediately. |
| `unfreeze_treasury(treasury_id, caller)` | Owner/Parent | Re-enables withdrawals. |

**Members & roles**

| Method | Auth | Description |
|---|---|---|
| `add_member(treasury_id, caller, member, role, spending_limit)` | Owner/Parent | `spending_limit` is optional, meaningful mainly for `Child`. |
| `remove_member(treasury_id, caller, member)` | Owner/Parent | Revokes access immediately. |
| `get_member(treasury_id, member)` / `list_members(treasury_id)` | none | Read-only. |

**Rules**

| Method | Auth | Description |
|---|---|---|
| `set_approval_rule(treasury_id, caller, approval_threshold, required_approvals)` | Owner/Parent | Changes the rule for future withdrawals; doesn't affect ones already pending. |

**Deposits & withdrawals**

| Method | Auth | Description |
|---|---|---|
| `deposit(treasury_id, from, amount)` | `from` | Transfers `amount` of the treasury's asset from `from` into the contract. |
| `request_withdrawal(treasury_id, caller, to, amount)` | caller must be able to spend | Executes immediately if `amount < approval_threshold` and within any spending limit; otherwise opens a pending `Withdrawal` and returns its id. |
| `approve_withdrawal(withdrawal_id, approver)` | Owner/Parent/Guardian | Adds an approval; once `required_approvals` is reached, the transfer executes in the same call. |
| `get_withdrawal(withdrawal_id)` | none | Read-only. |

**Savings goals**

| Method | Auth | Description |
|---|---|---|
| `create_savings_goal(treasury_id, caller, name, target_amount)` | Owner/Parent | Returns the new `goal_id`. |
| `contribute_to_goal(goal_id, from, amount)` | `from` | Transfers into the contract, increments both the goal and the treasury balance. |
| `get_savings_goal(goal_id)` / `list_savings_goals(treasury_id)` | none | Read-only. |

**Bills**

| Method | Auth | Description |
|---|---|---|
| `create_bill(treasury_id, caller, name, payee, amount, interval_ledgers)` | Owner/Parent | Schedules the first due date `interval_ledgers` from now. |
| `pay_bill(bill_id)` | **none (permissionless)** | Only succeeds once `next_due_ledger` has passed; safe for a public relayer to call. |
| `cancel_bill(bill_id, caller)` | Owner/Parent | Deactivates the bill. |
| `get_bill(bill_id)` / `list_bills(treasury_id)` | none | Read-only. |

**Inheritance vault**

| Method | Auth | Description |
|---|---|---|
| `create_inheritance_vault(treasury_id, caller, beneficiaries, time_lock_ledger, dead_man_switch_period, guardian_approvals_required)` | Owner only | `beneficiaries`' `allocation_bps` must sum to exactly `10_000`. |
| `heartbeat(treasury_id, caller)` | Owner only | Resets the dead-man switch clock. |
| `approve_inheritance_claim(treasury_id, guardian)` | Guardian | Registers one guardian's approval toward `guardian_approvals_required`. |
| `claim_inheritance(treasury_id, caller)` | any | Distributes the full treasury balance pro-rata, **only if** `(time_lock passed OR dead-man switch expired) AND enough guardian approvals`. |
| `get_inheritance_vault(treasury_id)` | none | Read-only. |

## Errors

All fallible methods return `Result<T, Error>`. `Error` (in
`contracts/treasury/src/errors.rs`) is a `#[contracterror]` enum so it
surfaces as a structured error code to callers, not a panic string:

`NotAuthorized`, `TreasuryNotFound`, `MemberNotFound`,
`MemberAlreadyExists`, `InsufficientBalance`, `TreasuryFrozen`,
`GoalNotFound`, `BillNotFound`, `BillNotDue`, `WithdrawalNotFound`,
`AlreadyApproved`, `WithdrawalAlreadyExecuted`, `VaultNotFound`,
`VaultAlreadyExists`, `VaultNotClaimable`, `InvalidAllocation`,
`InvalidAmount`, `SpendingLimitExceeded`.

## Storage layout

Soroban gives a contract three storage "buckets," each with different
cost and lifetime tradeoffs: **instance** storage (cheap, tied to the
contract itself, good for small contract-wide values), **persistent**
storage (the default for real data — it sticks around until explicitly
removed, at a storage-rent cost proportional to how long you keep it),
and **temporary** storage (cheapest, but can expire and disappear — not
used in this contract, since treasury data obviously shouldn't vanish).

Everything a family actually cares about is stored in **persistent**
storage keyed by a `DataKey` enum (`contracts/treasury/src/types.rs`),
namespaced by id so unrelated treasuries never collide:

```
DataKey::Treasury(treasury_id)          -> Treasury
DataKey::Member(treasury_id, address)   -> Member
DataKey::MemberList(treasury_id)        -> Vec<Address>
DataKey::Goal(goal_id)                  -> SavingsGoal
DataKey::GoalList(treasury_id)          -> Vec<u64>
DataKey::Bill(bill_id)                  -> Bill
DataKey::BillList(treasury_id)          -> Vec<u64>
DataKey::Withdrawal(withdrawal_id)      -> Withdrawal
DataKey::Vault(treasury_id)             -> InheritanceVault
```

Auto-incrementing ids (`NextTreasuryId`, `NextGoalId`, `NextBillId`,
`NextWithdrawalId`) live in **instance** storage since they're accessed on
almost every write and are small, contract-wide counters.

## A walkthrough: the Adeyemi family treasury

A concrete sequence of calls, matching the scenario used throughout the
test suite:

```text
1. create_treasury(owner=Amara, name="Adeyemi Family", asset=USDC_SAC,
                    approval_threshold=1000, required_approvals=2)
   -> treasury_id = 1, Amara registered as Owner

2. add_member(1, Amara, Chidi, role=Parent, spending_limit=None)
   add_member(1, Amara, Zainab, role=Child, spending_limit=Some(50))
   add_member(1, Amara, UncleTunde, role=Guardian, spending_limit=None)

3. deposit(1, Amara, 10_000)
   -> treasury balance = 10,000

4. request_withdrawal(1, Zainab, to=ShopAddress, amount=30)
   -> 30 < spending_limit(50) and < approval_threshold(1000)
   -> executes immediately, balance = 9,970

5. request_withdrawal(1, Amara, to=Landlord, amount=1250)
   -> 1250 >= approval_threshold -> pending Withdrawal #1, balance unchanged

6. approve_withdrawal(1, Amara)   -> 1 of 2 approvals
   approve_withdrawal(1, Chidi)   -> 2 of 2 -> executes, balance = 8,720

7. create_savings_goal(1, Amara, "Emergency Fund", target_amount=5000)
   contribute_to_goal(goal_id, Chidi, 500)

8. create_inheritance_vault(1, Amara,
     beneficiaries=[(Zainab, 5000), (Kene, 5000)],  // 50/50, sums to 10,000
     time_lock_ledger=..., dead_man_switch_period=..., guardian_approvals_required=1)

9. heartbeat(1, Amara)   // Amara checks in periodically to reset the switch

   -- years later, no heartbeat --
   approve_inheritance_claim(1, UncleTunde)
   claim_inheritance(1, UncleTunde)
   -> dead-man switch expired + 1 guardian approval -> vault distributes
      50% to Zainab, 50% to Kene, treasury balance -> 0
```

## Development

Requires the [Stellar CLI](https://developers.stellar.org/docs/tools/cli)
and the `wasm32v1-none` Rust target. Soroban contracts compile to
**WebAssembly (wasm)** — a small, sandboxed binary format — rather than
native machine code, which is what makes it safe for a public network to
run arbitrary contracts from anyone without those contracts being able
to touch the host machine directly. `wasm32v1-none` is the specific,
minimal wasm target Soroban expects (no OS, no filesystem — just the
contract's logic):

```bash
rustup target add wasm32v1-none
```

```bash
# run the full test suite (unit tests, in-memory Soroban Env)
cargo test --workspace

# format & lint
cargo fmt --all
cargo build --workspace          # native build, fastest feedback loop

# build the deployable wasm artifact
stellar contract build
# equivalent to: cargo build --target wasm32v1-none --release -p treasury
```

## Testing

`contracts/treasury/src/test.rs` has **14 tests** covering:

- Treasury creation and owner registration
- Deposits increasing balance
- Small withdrawals executing immediately vs. large ones requiring approval
- Rejecting a duplicate approval from the same approver
- Enforcing a child's per-transaction spending limit
- Freezing/unfreezing blocking and re-enabling withdrawals
- Savings goal contribution tracking
- Bills only payable once due, and not payable once cancelled
- Inheritance vault allocation validation (must sum to 10,000 bps)
- Inheritance claims distributing correctly after the dead-man switch expires
- The heartbeat resetting the dead-man switch and blocking a premature claim
- Non-administrators being rejected from adding members

Tests use `soroban_sdk::testutils` (`Env::default()`, `mock_all_auths()`,
`Address::generate`, a locally-registered Stellar Asset Contract for
minting/transfer assertions) — no network or deployed contract required.

## Deployment

```bash
./scripts/deploy.sh testnet <your-source-account>
```

The script builds, optimizes (`stellar contract optimize`), and deploys
the wasm, writing the resulting contract id to `.contract-id.<network>`
(gitignored). Pass `mainnet`, `futurenet`, or `local` as the first
argument for other networks. See `scripts/deploy.sh` for the exact
`stellar contract` invocations.

## Design principles

- **Non-custodial.** The contract never receives or stores a private key.
  It moves funds only via `token::Client::transfer`, authorized by the
  caller's own signature on that specific call — StellarNest the company
  has no more power over a family's funds than any other observer of the
  public ledger.
- **Rules are enforced on-chain**, not in application code. A compromised
  or buggy backend cannot bypass the approval threshold or a child's
  spending limit, because the contract itself checks them before moving
  funds — see [Why a smart contract, not just a database](#why-a-smart-contract-not-just-a-database).
- **Every family shares one contract instance.** Treasuries are
  namespaced by id rather than deployed per-family, which keeps
  deployment and upgrades simple while state stays fully isolated per
  `treasury_id` — one bug in one family's data can't leak into another's.
- **Permissionless where trust isn't needed.** `pay_bill` and
  `claim_inheritance` (once conditions are met) can be called by anyone —
  the contract's own checks are the security boundary, not caller
  identity, so a cron job or a beneficiary can trigger them without being
  granted any special privilege.

## Contributing

Issues and PRs are welcome. Before opening a PR: run `cargo fmt --all`,
`cargo test --workspace`, and make sure `cargo build --target
wasm32v1-none --release --workspace` still succeeds. See
[`StellarNest-Org/backend`](https://github.com/StellarNest-Org/backend)
for how the API builds unsigned XDR against this contract and hands it to
the client for signing, and
[`StellarNest-Org/frontend`](https://github.com/StellarNest-Org/frontend)
for the product surface built on top of it.

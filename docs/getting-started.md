# Getting Started

## Installation

```bash
# Install via cargo (requires Rust 1.75+)
cargo install mvl

# Or build from source
git clone https://github.com/LAB271/mvl_language.git
cd mvl_language
cargo install --path .
```

Verify the installation:

```bash
mvl --version
```

## Hello World

Create `hello.mvl`:

```mvl
partial fn main() -> Unit ! Console {
    println("Hello, verified world!")
}
```

Build and run:

```bash
mvl build hello.mvl
./hello
```

## Complete Example

This example demonstrates 7 of MVL's 11 compile-time requirements in a realistic banking scenario:

```mvl
// MVL example demonstrating 7 of the 11 compile-time requirements

// ── Types ─────────────────────────────────────────────────────────────────────

label Secret;  // IFC label for sensitive data

type Account = struct {
    id: Secret[String],
    balance: Int where balance >= 0,
}

// ── Core functions ────────────────────────────────────────────────────────────

/// Withdraw funds from an account — returns remaining balance.
/// 
/// Requirements proven:
///   - Req 10 (Refinements): amount > 0, balance >= amount
///   - Req 11 (IFC): account_id is Secret, cannot leak to Console
///   - Req 9 (Effects): declares ! Console for logging
///   - Req 8 (Totality): partial — may fail if balance insufficient
///   - Req 3 (No Null): returns Option, not nullable
///
partial fn withdraw(
    account_id: Secret[String],
    balance: Int where balance >= 0,
    amount: Int where amount > 0
) -> Option[Int] ! Console {
    if balance < amount {
        log_info("Insufficient funds")  // OK: no secret in message
        // log_info(account_id)         // REJECTED: Secret cannot flow to Console
        None
    } else {
        let remaining: Int = balance - amount;
        // Compiler proves: remaining >= 0 (from balance >= amount)
        Some(remaining)
    }
}

/// Transfer between accounts — orchestrates withdrawal.
///
/// Requirements proven:
///   - Req 8 (Totality): total — always terminates, no unbounded loops
///   - Req 10 (Refinements): transfer_amount > 0 propagates to withdraw
///   - Req 6 (Ownership): from/to are consumed, not aliased
///
total fn transfer(
    from: Secret[String],
    to: Secret[String],
    from_balance: Int where from_balance >= 0,
    transfer_amount: Int where transfer_amount > 0
) -> Result[Int, String] ! Console {
    match withdraw(from, from_balance, transfer_amount) {
        Some(new_balance) => Ok(new_balance),
        None => Err("Transfer failed: insufficient funds")
    }
    // Compiler proves termination: no recursion, no loops, finite match
}

// ── Loops: partial (while) vs total (for) ─────────────────────────────────────

/// Count transactions until balance is exhausted — partial, may not terminate.
///
/// Requirements proven:
///   - Req 8 (Totality): partial — while loop has no guaranteed bound
///   - Req 10 (Refinements): deduction > 0 ensures progress
///
partial fn count_until_exhausted(
    balance: Int where balance >= 0,
    deduction: Int where deduction > 0
) -> Int ! Console {
    let mut remaining: Int = balance;
    let mut count: Int = 0;
    
    while remaining >= deduction {
        remaining = remaining - deduction;
        count = count + 1;
        // Compiler cannot prove termination — deduction could be modified
        // (in general case), so this requires `partial`
    }
    
    log_info("Transactions counted: " + count.to_string());
    count
}

/// Sum the first n transaction amounts — total, guaranteed termination.
///
/// Requirements proven:
///   - Req 8 (Totality): total — for loop over finite range
///   - Req 10 (Refinements): n >= 0 ensures valid range
///   - Req 5 (Bounds): list access within bounds (via for-in)
///
total fn sum_transactions(
    amounts: List[Int],
    n: Int where n >= 0 && n <= amounts.len()
) -> Int {
    let mut total: Int = 0;
    
    for i in 0..n {
        // Compiler proves: i < n <= amounts.len(), so amounts[i] is safe
        total = total + amounts[i];
    }
    decreases n - i  // termination proof: distance to n shrinks each iteration
    
    total
}

// ── Relabeling: controlled secret disclosure ──────────────────────────────────

/// Mask account ID for safe logging — last 4 chars only.
///
/// Requirements proven:
///   - Req 11 (IFC): relabel trust() crosses the Secret boundary
///   - Audit trail: "masked_for_logging" recorded in assurance report
///
total fn mask_account_id(account_id: Secret[String]) -> String {
    let id: String = relabel trust(account_id, "masked_for_logging");
    // After relabel: id is now String (not Secret[String])
    // The audit tag "masked_for_logging" appears in the assurance report
    
    let len: Int = id.len();
    if len <= 4 {
        "****"
    } else {
        "****" + id.slice(len - 4, len)
    }
}

/// Log account activity safely — masks the secret before output.
///
total fn log_account_activity(
    account_id: Secret[String],
    action: String
) -> Unit ! Console {
    let masked: String = mask_account_id(account_id);
    log_info(action + " for account " + masked);  // OK: masked is not Secret
}

// ── Main ──────────────────────────────────────────────────────────────────────

/// Entry point — demonstrates the banking operations.
///
partial fn main() -> Unit ! Console {
    // Create accounts with secret IDs
    let alice_id: Secret[String] = Secret("ACCT-1234-5678-9012");
    let bob_id: Secret[String] = Secret("ACCT-9876-5432-1098");
    let alice_balance: Int = 1000;
    
    // Log activity with masked account ID (safe — uses relabel)
    log_account_activity(alice_id, "Withdrawal initiated");
    
    // Attempt withdrawal
    match withdraw(alice_id, alice_balance, 250) {
        Some(remaining) => {
            log_info("Withdrawal successful, remaining: " + remaining.to_string());
            
            // Transfer to Bob
            match transfer(alice_id, bob_id, remaining, 100) {
                Ok(final_balance) => {
                    log_info("Transfer complete, final: " + final_balance.to_string());
                    log_account_activity(bob_id, "Deposit received");
                }
                Err(msg) => log_error(msg)
            }
        }
        None => log_error("Withdrawal failed")
    }
    
    // Demonstrate loops
    let tx_count: Int = count_until_exhausted(500, 75);  // partial: while loop
    let amounts: List[Int] = [10, 20, 30, 40, 50];
    let total: Int = sum_transactions(amounts, 3);       // total: for loop
    log_info("Sum of first 3: " + total.to_string());
}
```

### Requirements Demonstrated

| Req | Name | How It's Shown |
|-----|------|----------------|
| 3 | No Null | `Option[Int]`, `Result[Int, String]` — explicit handling required |
| 5 | Bounds Safety | `for i in 0..n` with `n <= amounts.len()` — compiler proves access is safe |
| 6 | Ownership | Values consumed, no aliasing of `from`/`to` possible |
| 8 | Termination | `partial fn` + `while` vs `total fn` + `for` with `decreases` clause |
| 9 | Effect Tracking | `! Console` declared — `log_info`/`log_error` calls are tracked |
| 10 | Refinement Types | `amount > 0`, `balance >= 0`, `n <= amounts.len()` — proven at compile time |
| 11 | Information Flow | `Secret[String]` blocked from Console; `relabel trust()` with audit tag |

### Key Patterns

**Totality and loops:**

- `partial fn` + `while`: No termination guarantee — compiler accepts but doesn't prove termination
- `total fn` + `for`: Bounded iteration with `decreases` clause — compiler proves termination

**Information Flow Control:**

- `Secret[String]` cannot flow to any effect (Console, FileWrite, Net)
- `relabel trust(secret, "audit_tag")` crosses the IFC boundary
- The audit tag appears in the assurance report — every trust boundary is documented

**The commented line `// log_info(account_id)` would be a compile-time error:**

```
error[E0011]: information flow violation
  --> banking.mvl:32:9
   |
32 |         log_info(account_id)
   |         ^^^^^^^^^^^^^^^^^^^^ Secret[String] cannot flow to effect Console
   |
   = help: use `relabel trust(account_id, "reason")` to cross the boundary
   = note: this will be recorded in the assurance report
```

## Next Steps

- [The Eleven Requirements](requirements.md) — what MVL proves at compile time
- [Language Manual](manual/index.md) — complete language reference
- [Standard Library](stdlib.md) — available functions and types
- [Rationale](mvl_rationale.md) — why MVL exists

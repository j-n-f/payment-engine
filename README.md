# Payment Engine

A simple demo program which processes transaction logs, and outputs client balances and account state.

Supports:

* Deposits
* Withdrawals
* Disputes
* Resolutions
* Chargebacks

## Quick Start

```sh
$ cargo run -- some_transaction_log.csv > client_balances.csv
```

An example CSV is provided (`example1.csv`) but it only tests parsing, not behavior.

## Running Tests

A small (and incomplete) set of tests are provided.

```sh
$ cargo test
```

## Assumptions

The spec provided left room for interpretation, so the following assumptions are made:

1. `{Resolve, Chargeback}` on a transaction which doesn't already have a Dispute will have no effect.
2. `Dispute` only applies to transactions with an amount (`{Deposit, Withdrawal}`).
3. Once a client account is locked/frozen, no further transactions will have effect on the output.
4. All transaction amounts are positive values.
5. Transactions with more than 4 decimal places will be rounded to 4 decimal places before processing.
6. All transaction IDs are unique. Because they can be out-of-order, and potentially non- contiguous, we would need to
   maintain a list or bitmap to filter out duplicate use of some transaction ID. For `u32` transaction IDs that bitmap
   would consume ~0.5GB.
8. Rounding uses "banker's rounding" rules. This is the same as normal rounding rules, with the exception that a digit 
   of 5 in the least-significant place will always round to the nearest even value. (e.g. 0.5 rounds to 0.0, but 1.5
   rounds to 2.0)

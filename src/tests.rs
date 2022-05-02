/// Some basic sanity tests on the parser/processor.
///
/// Testing would be easier if the processor was a state machine that could be inspected halfway
/// through execution (i.e. feed transactions in one at a time rather than bulk processing).
use super::*;
use csv::{ReaderBuilder, Trim};
use rust_decimal_macros::dec;

/// Utility function which accepts inline CSV and provides a `csv::Reader` usable for testing.
fn csv_reader_from_str<R>(csv: R) -> csv::Reader<R>
where
    R: std::io::Read,
{
    ReaderBuilder::new()
        // Accept whitespace
        .trim(Trim::All)
        // Parsing is flexible, i.e. TransactionType::{Dispute, Resolve, Chargeback} may not have an
        // amount; any amounts will be ignored)
        .flexible(true)
        // Reading CSV from some path
        .from_reader(csv)
}

/// A client with sufficient available funds can withdraw them.
/// A client without sufficient available funds will maintain their balance.
#[test]
fn no_withdrawal_without_available_funds() {
    let reader = csv_reader_from_str(
        "\
type,       client, tx, amount
deposit,    1,      1,  1.0
withdrawal, 1,      2,  1.0
deposit,    2,      3,  1.0
withdrawal, 2,      4,  2.0
"
        .as_bytes(),
    );

    let records = process_csv(reader).unwrap();

    let client_1: &ClientState = records.get(&1).unwrap();
    let client_2: &ClientState = records.get(&2).unwrap();

    assert_eq!(client_1.available, dec!(0));
    assert_eq!(client_2.available, dec!(1.0));
}

/// The parser is capable of handling disputes and associated resolutions without amounts provided
/// in CSV data.
#[test]
fn can_parse_disputes_without_amount() {
    // 3 cases:
    //
    // 1. fields are zeroed
    // 2. fields are empty with trailing separator
    // 3. fields are empty with no trailing separator
    let reader = csv_reader_from_str(
        "\
type,       client, tx, amount
deposit,    1,      1,  1.0
dispute,    1,      2,  0.0
resolve,    1,      3,  0.0
dispute,    1,      4,
resolve,    1,      5,
dispute,    1,      6
resolve,    1,      7
"
        .as_bytes(),
    );

    // If the parser correctly populates the `Transaction` struct for disputes, we can assume the
    // same is true for the other 2 resolution transactions.
    assert_eq!(process_csv(reader).is_ok(), true);
}

/// The parser makes the `amount` column optional, and transactions should have no effect if they're
/// missing an amount.
#[test]
fn deposit_and_withdrawal_requires_amount() {
    let deposit_reader = csv_reader_from_str(
        "\
type,       client, tx, amount
deposit,    1,      1,
"
        .as_bytes(),
    );

    let deposit_records = process_csv(deposit_reader).unwrap();
    let client_1: &ClientState = deposit_records.get(&1).unwrap();
    assert_eq!(client_1.available, dec!(0));

    let withdrawal_reader = csv_reader_from_str(
        "\
type,       client, tx, amount
deposit,    1,      1,  1.0    
withdrawal, 1,      2,
"
        .as_bytes(),
    );

    let withdrawal_records = process_csv(withdrawal_reader).unwrap();
    let client_1: &ClientState = withdrawal_records.get(&1).unwrap();
    assert_eq!(client_1.available, dec!(1.0));
}

/// Values are only considered to 4 decimal places. Values are rounded before transactions are
/// processed. Rounding is done using "banker's rounding" rules.
#[test]
fn rounds_to_four_decimal_places() {
    // Expected Results:
    // 1.00004  ->  1.0000
    // 1.00005  ->  1.0000
    // 1.00006  ->  1.0001
    // 1.00014  ->  1.0001
    // 1.00015  ->  1.0002
    // 1.00016  ->  1.0002
    let reader = csv_reader_from_str(
        "\
type,       client, tx, amount
deposit,    1,      1,  1.00004
deposit,    1,      2,  1.00005
deposit,    1,      3,  1.00006
deposit,    1,      4,  1.00014
deposit,    1,      5,  1.00015
deposit,    1,      6,  1.00016
"
        .as_bytes(),
    );

    let records = process_csv(reader).unwrap();
    let client_1: &ClientState = records.get(&1).unwrap();
    assert_eq!(client_1.available, dec!(6.0006));
}

/// A dispute moves funds from available to held. This doesn't apply if the transaction hasn't
/// happened yet.
#[test]
fn dispute_moves_funds_from_available_to_held() {
    let reader = csv_reader_from_str(
        "\
type,       client, tx, amount
deposit,    1,      1,  1.0
dispute,    1,      1
dispute,    2,      2
deposit,    2,      2,  1.0
"
        .as_bytes(),
    );

    let records = process_csv(reader).unwrap();
    let client_1: &ClientState = records.get(&1).unwrap();
    assert_eq!(client_1.available, dec!(0));
    assert_eq!(client_1.held, dec!(1.0));
    assert_eq!(client_1.locked, false);

    let client_2: &ClientState = records.get(&2).unwrap();
    assert_eq!(client_2.available, dec!(1.0));
    assert_eq!(client_2.held, dec!(0));
    assert_eq!(client_2.locked, false);
}

/// A resolve moves funds from held to available.
#[test]
fn resolve_moves_funds_from_held_to_available() {
    let reader = csv_reader_from_str(
        "\
type,       client, tx, amount
deposit,    1,      1,  1.0
dispute,    1,      1
resolve,    1,      1
"
        .as_bytes(),
    );

    let records = process_csv(reader).unwrap();
    let client_1: &ClientState = records.get(&1).unwrap();
    assert_eq!(client_1.available, dec!(1.0));
    assert_eq!(client_1.held, dec!(0));
    assert_eq!(client_1.locked, false);
}

/// A dispute causes an account to become locked/frozen, and no further transactions will apply.
#[test]
fn dispute_with_chargeback_locks_account() {
    // 1. Client deposits 1.0, has 1.0 available
    // 2. Client deposits 1.0, has 2.0 available
    // 3. Client disputes second deposit, 1.0 available, 1.0 held
    // 4. Client chargebacks second deposit, 1.0 available, 0.0 held, account locked
    // 5. Deposit of 1.0 will have no effect
    // 6. Withdrawal of 1.0 will have no effect (even though funds are available)
    let reader = csv_reader_from_str(
        "\
type,       client, tx, amount
deposit,    1,      1,  1.0
deposit,    1,      2,  1.0
dispute,    1,      2
chargeback, 1,      2
deposit,    1,      3,  1.0
withdrawal, 1,      4,  1.0
"
        .as_bytes(),
    );

    let records = process_csv(reader).unwrap();
    let client_1: &ClientState = records.get(&1).unwrap();
    assert_eq!(client_1.available, dec!(1.0));
    assert_eq!(client_1.held, dec!(0.0));
    assert_eq!(client_1.locked, true);
}

/// A resolve/chargeback only applies to a disputed transaction.
#[test]
fn resolve_and_chargeback_only_apply_to_disputed_transactions() {
    let reader = csv_reader_from_str(
        "\
type,       client, tx, amount
deposit,    1,      1,  1.0
chargeback, 1,      1
resolve,    1,      1
"
        .as_bytes(),
    );

    let records = process_csv(reader).unwrap();
    let client_1: &ClientState = records.get(&1).unwrap();
    assert_eq!(client_1.available, dec!(1.0));
    assert_eq!(client_1.held, dec!(0.0));
    assert_eq!(client_1.locked, false);
}

// TODO:
// 1. {Dispute, Resolve, Chargeback} reference the same transaction twice (i.e. ensure no
//    double-counting of balance-altering transactions).
// 2. [{Dispute}, {Resolve, Chargeback}] on both of {Deposit, Withdrawal}. (spec wasn't clear on
//    whether each of {Resolve, Chargeback} should only apply to one of {Deposit, Withdrawal}, a
//    test would help clarify what makes sense)

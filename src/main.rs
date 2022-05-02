/// A toy parser/processer for transaction data, as might be used for an ATM.
///
/// John Ferguson, 2022
use rust_decimal::prelude::*;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::{env, io};

use csv::{ReaderBuilder, Trim};
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

/// Maximum size of CSV reader buffer. Useful for larger datasets.
const CSV_READER_BUFFER_SIZE_IN_BYTES: usize = 1024;

/// How many decimal places to handle for transaction amounts.
const TX_AMOUNT_DECIMAL_PLACES: u32 = 4;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum TransactionType {
    /// Credit to a client's account. Increases available and total funds.
    Deposit,
    /// Debit to the client's account. Decreases the available and total funds. Does not apply when
    /// the client lacks the funds for the transaction.
    Withdrawal,
    /// Claim that some transaction was erroneous. Decreases available funds, and increases held
    /// funds. Has no associated amount, and references an amount in another transaction (if it
    /// exists).
    Dispute,
    /// Resolution to a Dispute. Held funds decrease by amount of disputed transaction, available
    /// funds increase by amount of disputed transaction.
    Resolve,
    /// Resolution to a Dispute. Held funds decrease by disputed amount, and client's account is
    /// frozen/locked.
    Chargeback,
}

#[derive(Debug, Deserialize)]
struct Transaction {
    r#type: TransactionType,
    #[serde(rename = "client")]
    client_id: u16,
    #[serde(rename = "tx")]
    tx_id: u32,
    /// Transaction amount, will be rounded to 4 decimal places before handling.
    amount: Option<Decimal>,
}

// TODO: Wrap `Decimal` in a newtype and implement `serde::Serialize` so that decimal place
// handling requires less effort.
#[derive(Debug, Serialize)]
struct ClientState {
    /// This needs to be included for serialization
    #[serde(rename = "client")]
    client_id: u16,
    available: Decimal,
    held: Decimal,
    total: Decimal,
    locked: bool,
    #[serde(skip)]
    disputed_tx_ids: HashSet<u32>,
}

impl Default for ClientState {
    fn default() -> Self {
        ClientState {
            client_id: Default::default(),
            available: Decimal::new(0, 0),
            held: Decimal::new(0, 0),
            total: Decimal::new(0, 0),
            locked: false,
            disputed_tx_ids: Default::default(),
        }
    }
}

/// Get all the transactions in some readable CSV data and return a map of client account states.
fn process_csv<R>(mut reader: csv::Reader<R>) -> Result<HashMap<u16, ClientState>, Box<dyn Error>>
where
    R: std::io::Read,
{
    // Keep track of client states as transactions are processed.
    let mut client_states = HashMap::<u16, ClientState>::new();

    // Keep track of disputable transactions in case they are referenced by later transactions.
    // Only transactions with an amount can be disputed.
    //let mut disputable_transactions = HashMap::<u32, &Transaction>::new();
    let mut disputable_transactions = HashMap::<u32, Transaction>::new();

    for result in reader.deserialize() {
        let tx: Transaction = result?;

        // All clients referenced by any transaction get tracked.
        let state: &mut ClientState = client_states.entry(tx.client_id).or_insert_with(|| {
            let mut empty_state: ClientState = Default::default();
            empty_state.client_id = tx.client_id;

            empty_state
        });

        // Transactions only get applied if the client's account isn't locked/frozen.
        if !state.locked {
            let tx_amount = tx
                .amount
                .unwrap_or_default()
                .round_dp(TX_AMOUNT_DECIMAL_PLACES);

            match tx.r#type {
                TransactionType::Deposit => {
                    state.available += tx_amount;

                    disputable_transactions.insert(tx.tx_id, tx);
                }
                TransactionType::Withdrawal => {
                    if state.available >= tx_amount {
                        state.available -= tx_amount;
                    }

                    disputable_transactions.insert(tx.tx_id, tx);
                }
                TransactionType::Dispute => {
                    // Specification states that "if the transaction specified by the dispute
                    // doesn't exist you can ignore it". Assumption: A `Dispute` can only reference
                    // a transaction which has already occurred, and since transactions in CSV are
                    // in order they occurred, we can skip disputes against transactions we haven't
                    // seen yet.
                    if let Some(disputed_tx) = disputable_transactions.get(&tx.tx_id) {
                        // Assumptions: we don't have to consider the client ID, and differentiate
                        // between disputes on the same tx ID by different clients. If this was
                        // the case then transactions would probably indicate source/destination
                        // clients.
                        //
                        // All disputes are valid as long as the tx ID has already occurred, and no
                        // dispute is already outstanding against some tx ID for this client.
                        //
                        // This implies that the client ID in the dispute should match the client
                        // ID in the disputed transaction, but since it isn't in the spec no check
                        // is made here. If we did want to enforce this, we could store a
                        // collection of `&Transaction` for each client (i.e.
                        // `disputable_transactions` would be per-client)

                        if !state.disputed_tx_ids.contains(&tx.tx_id) {
                            let disputed_amount = disputed_tx
                                .amount
                                .unwrap()
                                .round_dp(TX_AMOUNT_DECIMAL_PLACES);
                            state.available -= disputed_amount;
                            state.held += disputed_amount;

                            state.disputed_tx_ids.insert(tx.tx_id);
                        }
                    }
                }
                TransactionType::Resolve => {
                    // See assumptions for `TransactionType::Dispute` above.
                    if let Some(disputed_tx) = disputable_transactions.get(&tx.tx_id) {
                        if state.disputed_tx_ids.contains(&tx.tx_id) {
                            state.disputed_tx_ids.remove(&tx.tx_id);

                            let disputed_amount = disputed_tx
                                .amount
                                .unwrap()
                                .round_dp(TX_AMOUNT_DECIMAL_PLACES);
                            state.available += disputed_amount;
                            state.held -= disputed_amount;
                        }
                    }
                }
                TransactionType::Chargeback => {
                    // See assumptions for `TransactionType::Dispute` above.
                    if let Some(disputed_tx) = disputable_transactions.get(&tx.tx_id) {
                        if state.disputed_tx_ids.contains(&tx.tx_id) {
                            state.disputed_tx_ids.remove(&tx.tx_id);

                            let disputed_amount = disputed_tx
                                .amount
                                .unwrap()
                                .round_dp(TX_AMOUNT_DECIMAL_PLACES);
                            state.held -= disputed_amount;
                            state.locked = true;
                        }
                    }
                }
            }

            // Update the client's total (serde doesn't allow serialized fields to be computed by
            // combining other fields so we store it explicitly).
            state.total = state.available + state.held;
        }
    }

    Ok(client_states)
}

/// Print client account states to stdout.
fn print_balances(states: &HashMap<u16, ClientState>) -> Result<(), Box<dyn Error>> {
    let mut writer = csv::Writer::from_writer(io::stdout());

    for state in states.values() {
        writer.serialize(state)?;
    }
    writer.flush()?;

    Ok(())
}

fn main() {
    // Ensure user provided a file path as argument to the program.
    if let Some(csv_path) = env::args().nth(1) {
        // Ensure the path provided is a file which exists.
        if std::path::Path::new(&csv_path).exists() {
            // When run from the command line, we parse a CSV file at the given path.
            let reader: csv::Reader<std::fs::File> = ReaderBuilder::new()
                // Avoid using too much memory
                .buffer_capacity(CSV_READER_BUFFER_SIZE_IN_BYTES)
                // Accept whitespace
                .trim(Trim::All)
                // Parsing is flexible, i.e. TransactionType::{Dispute, Resolve, Chargeback} may not have an
                // amount; any amounts will be ignored)
                .flexible(true)
                // Reading CSV from some path
                .from_path(csv_path)
                .unwrap();

            // Process the transaction log and export client balances.
            match process_csv(reader) {
                Ok(client_states) => {
                    if let Err(e) = print_balances(&client_states) {
                        eprintln!("error writing client account states: {:?}", e);
                        std::process::exit(-1);
                    }
                }
                Err(e) => {
                    eprintln!("error handling transaction data: {:?}", e);
                    std::process::exit(-1);
                }
            }
        } else {
            eprintln!("couldn't read CSV: {}", csv_path);
            std::process::exit(-1);
        }
    } else {
        eprintln!("expected path to CSV as first argument, aborting");
        std::process::exit(-1);
    }

    std::process::exit(0);
}

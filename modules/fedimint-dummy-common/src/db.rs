use fedimint_core::db::DatabaseTransaction;
use fedimint_core::encoding::{Decodable, Encodable};
use fedimint_core::{impl_db_lookup, impl_db_record, Amount, OutPoint};
use futures::StreamExt;
use serde::Serialize;
use strum_macros::EnumIter;



/// Namespaces DB keys for this module
#[repr(u8)]
#[derive(Clone, EnumIter, Debug)]
pub enum DbKeyPrefix {
    DummyFunds = 0x01,
    DummyOutputs = 0x02,
}

// TODO: Boilerplate-code
impl std::fmt::Display for DbKeyPrefix {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Key to lookup data
#[derive(Debug, Clone, Encodable, Decodable, Eq, PartialEq, Hash, Serialize)]
pub struct DummyFundsKeyV0(pub String);

/// Prefix to find all keys
#[derive(Debug, Encodable, Decodable)]
pub struct DummyFundsKeyPrefixV0;

// Turns types into DB record
impl_db_record!(
    key = DummyFundsKeyV0,
    value = Amount,
    db_prefix = DbKeyPrefix::DummyFunds,
);
// Associates prefix type
impl_db_lookup!(key = DummyFundsKeyV0, query_prefix = DummyFundsKeyPrefixV0);

/// Example DB migration from version 0 to version 1
pub async fn migrate_to_v1(dbtx: &mut DatabaseTransaction<'_>) -> Result<(), anyhow::Error> {
    // Select old entries
    let v0_entries = dbtx
        .find_by_prefix(&DummyFundsKeyPrefixV0)
        .await
        .collect::<Vec<(DummyFundsKeyV0, Amount)>>()
        .await;

    // Remove old entries
    dbtx.remove_by_prefix(&DummyFundsKeyPrefixV0).await;

    // Migrate to new entries
    for (v0_key, v0_val) in v0_entries {
        let v1_key = DummyFundsKeyV1(v0_key.0);
        dbtx.insert_new_entry(&v1_key, &v0_val).await;
    }
    Ok(())
}

/// Key to lookup outputs
#[derive(Debug, Clone, Encodable, Decodable, Eq, PartialEq, Hash, Serialize)]
pub struct DummyOutputKeyV1(pub OutPoint);

/// Prefix to find all outputs
#[derive(Debug, Encodable, Decodable)]
pub struct DummyOutputKeyV1Prefix;

// Turns types into DB record
impl_db_record!(
    key = DummyOutputKeyV1,
    value = (),
    db_prefix = DbKeyPrefix::DummyOutputs,
);
impl_db_lookup!(
    key = DummyOutputKeyV1,
    query_prefix = DummyOutputKeyV1Prefix
);

/// Key to lookup funds for a user
#[derive(Debug, Clone, Encodable, Decodable, Eq, PartialEq, Hash, Serialize)]
pub struct DummyFundsKeyV1(pub String);

/// Prefix to find all funds
#[derive(Debug, Encodable, Decodable)]
pub struct DummyFundsKeyV1Prefix;

// Turns types into DB record
impl_db_record!(
    key = DummyFundsKeyV1,
    value = Amount,
    db_prefix = DbKeyPrefix::DummyFunds,
);
impl_db_lookup!(key = DummyFundsKeyV1, query_prefix = DummyFundsKeyV1Prefix);

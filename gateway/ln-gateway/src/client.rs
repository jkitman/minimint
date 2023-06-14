use std::collections::BTreeMap;
use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::Arc;

use fedimint_client::module::gen::ClientModuleGenRegistry;
use fedimint_client::secret::PlainRootSecretStrategy;
use fedimint_client::ClientBuilder;
use fedimint_core::api::{DynGlobalApi, GlobalFederationApi, WsClientConnectInfo, WsFederationApi};
use fedimint_core::db::DatabaseTransaction;
use fedimint_core::task::TaskGroup;
use fedimint_dummy_client::DummyClientGen;
use fedimint_mint_client::MintClientGen;
use fedimint_wallet_client::WalletClientGen;
use futures::StreamExt;
use lightning::routing::gossip::RoutingFees;

use crate::db::{FederationConfig, FederationIdKey, FederationIdKeyPrefix};
use crate::lnrpc_client::ILnRpcClient;
use crate::ng::GatewayClientGen;
use crate::{GatewayError, Result};

#[derive(Debug, Clone)]
pub struct StandardGatewayClientBuilder {
    work_dir: PathBuf,
}

impl StandardGatewayClientBuilder {
    pub fn new(work_dir: PathBuf) -> Self {
        Self { work_dir }
    }
}

impl StandardGatewayClientBuilder {
    pub async fn build(
        &self,
        config: FederationConfig,
        lnrpc: Arc<dyn ILnRpcClient>,
        tg: &mut TaskGroup,
    ) -> Result<fedimint_client::Client> {
        let federation_id = config.config.federation_id;

        let db_path = self.work_dir.join(format!("{federation_id}.db"));

        let db =
            fedimint_rocksdb::RocksDb::open(db_path).map_err(|_| GatewayError::DatabaseError)?;

        // TODO: This should come from the outside and should not include the dummy
        // module
        let mut registry = ClientModuleGenRegistry::new();
        registry.attach(MintClientGen);
        registry.attach(DummyClientGen);
        registry.attach(WalletClientGen::default());
        registry.attach(GatewayClientGen {
            lightning_client: lnrpc,
            fees: config.fees,
            timelock_delta: config.timelock_delta,
            mint_channel_id: config.mint_channel_id,
        });

        let mut client_builder = ClientBuilder::default();
        client_builder.with_module_gens(registry);
        client_builder.with_primary_module(1);
        client_builder.with_config(config.config);
        client_builder.with_database(db);

        tracing::info!("STANDARD CLIENT BUILDING");
        client_builder
            // TODO: make this configurable?
            .build::<PlainRootSecretStrategy>(tg)
            .await
            .map_err(|_| GatewayError::ClientNgError)
    }

    pub async fn create_config(
        &self,
        connect: WsClientConnectInfo,
        mint_channel_id: u64,
        fees: RoutingFees,
    ) -> Result<FederationConfig> {
        let api: DynGlobalApi = WsFederationApi::from_connect_info(&[connect.clone()]).into();
        let client_config = api.download_client_config(&connect).await?;
        Ok(FederationConfig {
            mint_channel_id,
            timelock_delta: 10,
            fees,
            config: client_config,
        })
    }

    pub async fn save_config(
        &self,
        config: FederationConfig,
        mut dbtx: DatabaseTransaction<'_>,
    ) -> Result<()> {
        let id = config.config.federation_id;
        dbtx.insert_new_entry(&FederationIdKey { id }, &config)
            .await;
        dbtx.commit_tx_result()
            .await
            .map_err(|_| GatewayError::DatabaseError)
    }

    pub async fn load_configs(
        &self,
        mut dbtx: DatabaseTransaction<'_>,
    ) -> Result<Vec<FederationConfig>> {
        Ok(dbtx
            .find_by_prefix(&FederationIdKeyPrefix)
            .await
            .collect::<BTreeMap<FederationIdKey, FederationConfig>>()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>())
    }
}

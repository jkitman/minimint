use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::string::ToString;


use async_trait::async_trait;
use fedimint_core::config::{
    ClientModuleConfig, ConfigGenModuleParams, DkgResult, ServerModuleConfig,
    ServerModuleConsensusConfig, TypedServerModuleConfig, TypedServerModuleConsensusConfig,
};
use fedimint_core::db::{Database, DatabaseVersion, MigrationMap, ModuleDatabaseTransaction};
use fedimint_core::module::audit::Audit;
use fedimint_core::module::interconnect::ModuleInterconect;
use fedimint_core::module::{
    api_endpoint, ApiEndpoint, ConsensusProposal, CoreConsensusVersion, ExtendsCommonModuleGen,
    InputMeta, IntoModuleError, ModuleConsensusVersion, ModuleError, PeerHandle, ServerModuleGen,
    SupportedModuleApiVersions, TransactionItemAmount,
};
use fedimint_core::server::DynServerModule;
use fedimint_core::task::TaskGroup;
use fedimint_core::{push_db_pair_items, Amount, NumPeers, OutPoint, PeerId, ServerModule};
use fedimint_dummy_common::config::{DummyConfig, DummyConfigConsensus, DummyConfigPrivate};
use fedimint_dummy_common::db::{
    migrate_to_v1, DbKeyPrefix, DummyFundsKeyV1, DummyFundsKeyV1Prefix, DummyOutputKeyV1,
    DummyOutputKeyV1Prefix,
};
use fedimint_dummy_common::{
    DummyCommonGen, DummyConfigGenParams, DummyConsensusItem, DummyError, DummyInput,
    DummyModuleTypes, DummyOutput, DummyOutputOutcome, DummyPrintMoneyRequest, CONSENSUS_VERSION,
};
use fedimint_server::config::distributedgen::PeerHandleOps;
use futures::{FutureExt, StreamExt};
use rand::rngs::OsRng;
use strum::IntoEnumIterator;
use threshold_crypto::serde_impl::SerdeSecret;
use threshold_crypto::{PublicKeySet, SecretKeySet};

/// Special account for creating assets from thin air
pub const FED_ACCOUNT: &str = "Money printer go brr";

/// Generates the module
#[derive(Debug, Clone)]
pub struct DummyServerGen;

// TODO: Boilerplate-code
impl ExtendsCommonModuleGen for DummyServerGen {
    type Common = DummyCommonGen;
}

/// Implementation of server module non-consensus functions
#[async_trait]
impl ServerModuleGen for DummyServerGen {
    const DATABASE_VERSION: DatabaseVersion = DatabaseVersion(1);

    /// Returns the version of this module
    fn versions(&self, _core: CoreConsensusVersion) -> &[ModuleConsensusVersion] {
        &[CONSENSUS_VERSION]
    }

    /// Initialize the module
    async fn init(
        &self,
        cfg: ServerModuleConfig,
        _db: Database,
        _env: &BTreeMap<OsString, OsString>,
        _task_group: &mut TaskGroup,
    ) -> anyhow::Result<DynServerModule> {
        Ok(Dummy::new(cfg.to_typed()?).into())
    }

    /// DB migrations to move from old to newer versions
    fn get_database_migrations(&self) -> MigrationMap {
        let mut migrations = MigrationMap::new();
        migrations.insert(DatabaseVersion(0), move |dbtx| migrate_to_v1(dbtx).boxed());
        migrations
    }

    /// Generates configs for all peers in a trusted manner for testing
    fn trusted_dealer_gen(
        &self,
        peers: &[PeerId],
        params: &ConfigGenModuleParams,
    ) -> BTreeMap<PeerId, ServerModuleConfig> {
        // Coerce config gen params into type
        let params = params.to_typed::<DummyConfigGenParams>().unwrap();
        // Create trusted set of threshold keys
        let sks = SecretKeySet::random(peers.degree(), &mut OsRng);
        let pks: PublicKeySet = sks.public_keys();
        // Generate a config for each peer
        peers
            .iter()
            .map(|&peer| {
                let private_key_share = SerdeSecret(sks.secret_key_share(peer.to_usize()));
                let config = DummyConfig {
                    private: DummyConfigPrivate { private_key_share },
                    consensus: DummyConfigConsensus {
                        public_key_set: pks.clone(),
                        tx_fee: params.example_param,
                    },
                };
                (peer, config.to_erased())
            })
            .collect()
    }

    /// Generates configs for all peers in an untrusted manner
    async fn distributed_gen(
        &self,
        peers: &PeerHandle,
        params: &ConfigGenModuleParams,
    ) -> DkgResult<ServerModuleConfig> {
        // Coerce config gen params into type
        let params = params.to_typed::<DummyConfigGenParams>().unwrap();
        // Runs distributed key generation
        // Could create multiple keys, here we use '()' to create one
        let g1 = peers.run_dkg_g1(()).await?;
        let keys = g1[&()].threshold_crypto();

        Ok(DummyConfig {
            private: DummyConfigPrivate {
                private_key_share: keys.secret_key_share,
            },
            consensus: DummyConfigConsensus {
                public_key_set: keys.public_key_set,
                tx_fee: params.example_param,
            },
        }
        .to_erased())
    }

    // TODO: Boilerplate-code
    fn get_client_config(
        &self,
        config: &ServerModuleConsensusConfig,
    ) -> anyhow::Result<ClientModuleConfig> {
        Ok(DummyConfigConsensus::from_erased(config)?.to_client_config())
    }

    // TODO: Boilerplate-code
    fn validate_config(&self, identity: &PeerId, config: ServerModuleConfig) -> anyhow::Result<()> {
        config.to_typed::<DummyConfig>()?.validate_config(identity)
    }

    /// Dumps all database items for debugging
    async fn dump_database(
        &self,
        dbtx: &mut ModuleDatabaseTransaction<'_>,
        prefix_names: Vec<String>,
    ) -> Box<dyn Iterator<Item = (String, Box<dyn erased_serde::Serialize + Send>)> + '_> {
        // TODO: Boilerplate-code
        let mut items: BTreeMap<String, Box<dyn erased_serde::Serialize + Send>> = BTreeMap::new();
        let filtered_prefixes = DbKeyPrefix::iter().filter(|f| {
            prefix_names.is_empty() || prefix_names.contains(&f.to_string().to_lowercase())
        });

        for table in filtered_prefixes {
            match table {
                DbKeyPrefix::DummyFunds => {
                    push_db_pair_items!(
                        dbtx,
                        DummyFundsKeyV1Prefix,
                        DummyFundsKeyV1,
                        Amount,
                        items,
                        "Dummy Funds"
                    );
                }
                DbKeyPrefix::DummyOutputs => {
                    push_db_pair_items!(
                        dbtx,
                        DummyOutputKeyV1Prefix,
                        DummyOutputKeyV1,
                        (),
                        items,
                        "Dummy Outputs"
                    );
                }
            }
        }

        Box::new(items.into_iter())
    }
}

/// Dummy module
#[derive(Debug)]
pub struct Dummy {
    pub cfg: DummyConfig,
}

/// Implementation of consensus for the server module
#[async_trait]
impl ServerModule for Dummy {
    /// Define the consensus types
    type Common = DummyModuleTypes;
    type Gen = DummyServerGen;
    type VerificationCache = DummyVerificationCache;

    fn supported_api_versions(&self) -> SupportedModuleApiVersions {
        SupportedModuleApiVersions::from_raw(0, 0, &[(0, 0)])
    }

    async fn await_consensus_proposal(&self, _dbtx: &mut ModuleDatabaseTransaction<'_>) {
        std::future::pending().await
    }

    async fn consensus_proposal(
        &self,
        _dbtx: &mut ModuleDatabaseTransaction<'_>,
    ) -> ConsensusProposal<DummyConsensusItem> {
        ConsensusProposal::empty()
    }

    async fn begin_consensus_epoch<'a, 'b>(
        &'a self,
        _dbtx: &mut ModuleDatabaseTransaction<'b>,
        _consensus_items: Vec<(PeerId, DummyConsensusItem)>,
        _consensus_peers: &BTreeSet<PeerId>,
    ) -> Vec<PeerId> {
        vec![]
    }

    fn build_verification_cache<'a>(
        &'a self,
        _inputs: impl Iterator<Item = &'a DummyInput> + Send,
    ) -> Self::VerificationCache {
        DummyVerificationCache
    }

    async fn validate_input<'a, 'b>(
        &self,
        _interconnect: &dyn ModuleInterconect,
        dbtx: &mut ModuleDatabaseTransaction<'b>,
        _verification_cache: &Self::VerificationCache,
        input: &'a DummyInput,
    ) -> Result<InputMeta, ModuleError> {
        // verify user has enough funds
        if input.amount > get_funds(&input.account, dbtx).await {
            return Err(DummyError::NotEnoughFunds).into_module_error_other();
        }

        // return the amount that this represents
        Ok(InputMeta {
            amount: TransactionItemAmount {
                amount: input.amount,
                fee: self.cfg.consensus.tx_fee,
            },
            puk_keys: vec![],
        })
    }

    async fn apply_input<'a, 'b, 'c>(
        &'a self,
        interconnect: &'a dyn ModuleInterconect,
        dbtx: &mut ModuleDatabaseTransaction<'c>,
        input: &'b DummyInput,
        cache: &Self::VerificationCache,
    ) -> Result<InputMeta, ModuleError> {
        // TODO: Boiler-plate code
        let meta = self
            .validate_input(interconnect, dbtx, cache, input)
            .await?;

        // subtract funds from the user's account
        let updated = get_funds(&input.account, dbtx).await - input.amount;
        dbtx.insert_entry(&DummyFundsKeyV1(input.account.clone()), &updated)
            .await;

        Ok(meta)
    }

    async fn validate_output(
        &self,
        _dbtx: &mut ModuleDatabaseTransaction<'_>,
        output: &DummyOutput,
    ) -> Result<TransactionItemAmount, ModuleError> {
        Ok(TransactionItemAmount {
            amount: output.amount,
            fee: self.cfg.consensus.tx_fee,
        })
    }

    async fn apply_output<'a, 'b>(
        &'a self,
        dbtx: &mut ModuleDatabaseTransaction<'b>,
        output: &'a DummyOutput,
        out_point: OutPoint,
    ) -> Result<TransactionItemAmount, ModuleError> {
        // TODO: Boiler-plate code
        let meta = self.validate_output(dbtx, output).await?;

        // add funds to the user's account
        let updated = get_funds(&output.account, dbtx).await - output.amount;
        dbtx.insert_entry(&DummyOutputKeyV1(out_point), &()).await;
        dbtx.insert_entry(&DummyFundsKeyV1(output.account.clone()), &updated)
            .await;

        Ok(meta)
    }

    async fn end_consensus_epoch<'a, 'b>(
        &'a self,
        _consensus_peers: &BTreeSet<PeerId>,
        _dbtx: &mut ModuleDatabaseTransaction<'b>,
    ) -> Vec<PeerId> {
        vec![]
    }

    async fn output_status(
        &self,
        dbtx: &mut ModuleDatabaseTransaction<'_>,
        out_point: OutPoint,
    ) -> Option<DummyOutputOutcome> {
        // check whether or not the output has been processed
        dbtx.get_value(&DummyOutputKeyV1(out_point)).await.map(|_| DummyOutputOutcome)
    }

    async fn audit(&self, dbtx: &mut ModuleDatabaseTransaction<'_>, audit: &mut Audit) {
        audit
            .add_items(dbtx, &DummyFundsKeyV1Prefix, |k, v| match k {
                // special account for creating assets (positive)
                DummyFundsKeyV1(a) if a == FED_ACCOUNT => v.msats as i64,
                // a user's funds are a federation's liability (negative)
                DummyFundsKeyV1(_) => -(v.msats as i64),
            })
            .await;
    }

    fn api_endpoints(&self) -> Vec<ApiEndpoint<Self>> {
        vec![api_endpoint! {
            "print_money",
            async |_module: &Dummy, context, request: DummyPrintMoneyRequest| -> () {
                let dbtx = &mut context.dbtx();
                let amount = get_funds(&request.account, dbtx).await - request.amount;
                dbtx.insert_entry(&DummyFundsKeyV1(request.account), &amount).await;
                // Print fake assets for the fed's balance sheet audit
                dbtx.insert_entry(&DummyFundsKeyV1(FED_ACCOUNT.to_string()), &amount).await;
                Ok(())
            }
        }]
    }
}

/// Helper to get the funds for an account
async fn get_funds<'a>(account: &str, dbtx: &mut ModuleDatabaseTransaction<'a>) -> Amount {
    let funds = dbtx.get_value(&DummyFundsKeyV1(account.to_string())).await;
    funds.unwrap_or(Amount::ZERO)
}

/// An in-memory cache we could use for faster validation
#[derive(Debug, Clone)]
pub struct DummyVerificationCache;

impl fedimint_core::server::VerificationCache for DummyVerificationCache {}

impl Dummy {
    /// Create new module instance
    pub fn new(cfg: DummyConfig) -> Dummy {
        Dummy { cfg }
    }
}

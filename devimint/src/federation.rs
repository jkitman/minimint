use std::collections::BTreeMap;
use std::ops::Range;

use anyhow::{anyhow, Context};
use fedimint_core::core::LEGACY_HARDCODED_INSTANCE_ID_WALLET;
use fedimint_core::db::mem_impl::MemDatabase;
use fedimint_core::PeerId;
use fedimint_server::config::ConfigGenParams;
use fedimint_testing::federation::local_config_gen_params;
use fedimint_wallet_client::config::WalletClientConfig;
use tokio::fs;

use super::*; // TODO: remove this

pub struct Federation {
    // client is only for internal use, use cli commands instead
    client: Arc<UserClient>,
    members: BTreeMap<usize, Fedimintd>,
    bitcoind: Bitcoind,
    params: HashMap<PeerId, ConfigGenParams>,
}

/// base port for running devimint tests
pub const BASE_PORT: u16 = 8173 + 10000;

impl Federation {
    pub async fn new(
        process_mgr: &ProcessManager,
        bitcoind: Bitcoind,
        peer_ids: Range<usize>,
    ) -> Result<Self> {
        info!("Running in {:?}", process_mgr.globals.FM_DATA_DIR);
        let peers: Vec<PeerId> = peer_ids.map(|id| PeerId::from(id as u16)).collect();
        let params = local_config_gen_params(&peers, BASE_PORT, Default::default())?;
        let mut members = BTreeMap::new();
        for peer in peers {
            let vars = vars::Fedimintd::init(&process_mgr.globals, &params[&peer], true).await?;
            members.insert(
                peer.to_usize(),
                Fedimintd::new(process_mgr, bitcoind.clone(), peer.to_usize(), &vars).await?,
            );
        }

        let client_file = process_mgr
            .globals
            .FM_DATA_DIR
            .join("server-0/client-connect");
        while !client_file.exists() {
            sleep(Duration::from_millis(100)).await;
        }
        info!("DKG complete, copying client configs");
        let cfg_dir = &process_mgr.globals.FM_DATA_DIR;
        let out_dir = cfg_dir.join("server-0");
        let out_dir = utf8(&out_dir);
        let cfg_dir = utf8(cfg_dir);
        // copy configs to config directory
        fs::rename(
            format!("{out_dir}/client-connect"),
            format!("{cfg_dir}/client-connect"),
        )
        .await?;
        fs::rename(
            format!("{out_dir}/client.json"),
            format!("{cfg_dir}/client.json"),
        )
        .await?;
        info!("copied client configs");

        let cfg_dir: PathBuf = env::var("FM_DATA_DIR")?.parse()?;
        let cfg_path = cfg_dir.join("client.json");
        let cfg: UserClientConfig = load_from_file(&cfg_path)?;
        let decoders = module_decode_stubs();
        let db = Database::new(MemDatabase::new(), module_decode_stubs());
        let module_gens = ClientModuleGenRegistry::from(vec![
            DynClientModuleGen::from(WalletClientGen),
            DynClientModuleGen::from(MintClientGen),
            DynClientModuleGen::from(LightningClientGen),
        ]);
        let client = UserClient::new(cfg, decoders, module_gens, db, Default::default()).await;
        Ok(Self {
            members,
            bitcoind,
            client: Arc::new(client),
            params,
        })
    }

    pub async fn start_server(
        &mut self,
        process_mgr: &ProcessManager,
        peer_id: usize,
    ) -> Result<()> {
        if self.members.contains_key(&peer_id) {
            return Err(anyhow!("fedimintd-{} already running", peer_id));
        }
        self.members.insert(
            peer_id,
            Fedimintd::new(
                process_mgr,
                self.bitcoind.clone(),
                peer_id,
                &vars::Fedimintd::init(
                    &process_mgr.globals,
                    &self.params[&PeerId::from(peer_id as u16)],
                    true,
                )
                .await?,
            )
            .await?,
        );
        Ok(())
    }

    pub async fn kill_server(&mut self, peer_id: usize) -> Result<()> {
        let Some((_, fedimintd)) = self.members.remove_entry(&peer_id) else {
            return Err(anyhow!("fedimintd-{} does not exist", peer_id));
        };
        fedimintd.kill().await?;
        Ok(())
    }

    pub fn members(&self) -> &BTreeMap<usize, Fedimintd> {
        &self.members
    }

    pub async fn cmd(&self) -> Command {
        let cfg_dir = env::var("FM_DATA_DIR").unwrap();
        cmd!("fedimint-cli", "--data-dir={cfg_dir}")
    }

    pub async fn pegin(&self, amt: u64) -> Result<()> {
        let pegin_addr = cmd!(self, "peg-in-address").out_json().await?["address"]
            .as_str()
            .context("address must be a string")?
            .to_owned();
        let txid = self.bitcoind.send_to(pegin_addr, amt).await?;
        self.bitcoind.mine_blocks(11).await?;
        self.await_block_sync().await?;
        let (txout_proof, raw_tx) = tokio::try_join!(
            self.bitcoind.get_txout_proof(&txid),
            self.bitcoind.get_raw_transaction(&txid),
        )?;
        cmd!(
            self,
            "peg-in",
            "--txout-proof={txout_proof}",
            "--transaction={raw_tx}",
        )
        .run()
        .await?;
        cmd!(self, "fetch").run().await?;
        Ok(())
    }

    pub async fn pegin_gateway(&self, amt: u64, gw_cln: &Gatewayd) -> Result<()> {
        let fed_id = self.federation_id().await;
        let pegin_addr = cmd!(gw_cln, "address", "--federation-id={fed_id}")
            .out_json()
            .await?
            .as_str()
            .context("address must be a string")?
            .to_owned();
        let txid = self.bitcoind.send_to(pegin_addr, amt).await?;
        self.bitcoind.mine_blocks(11).await?;
        self.await_block_sync().await?;
        let (txout_proof, raw_tx) = tokio::try_join!(
            self.bitcoind.get_txout_proof(&txid),
            self.bitcoind.get_raw_transaction(&txid),
        )?;
        cmd!(
            gw_cln,
            "deposit",
            "--federation-id={fed_id}",
            "--txout-proof={txout_proof}",
            "--transaction={raw_tx}"
        )
        .run()
        .await?;
        cmd!(self, "fetch").run().await?;
        Ok(())
    }

    pub async fn federation_id(&self) -> String {
        self.client.config().0.federation_id.to_string()
    }

    pub async fn await_block_sync(&self) -> Result<()> {
        let wallet_cfg: WalletClientConfig = self
            .client
            .config()
            .0
            .get_module(LEGACY_HARDCODED_INSTANCE_ID_WALLET)?;
        let finality_delay = wallet_cfg.finality_delay;
        let btc_height = self.bitcoind.client().get_blockchain_info()?.blocks;
        let expected = btc_height - (finality_delay as u64);
        cmd!(self, "wait-block-height", expected).run().await?;
        Ok(())
    }

    pub async fn await_gateways_registered(&self) -> Result<()> {
        poll("gateways registered", || async {
            Ok(cmd!(self, "list-gateways").out_json().await?["num_gateways"].as_u64() == Some(2))
        })
        .await?;
        Ok(())
    }

    pub async fn await_all_peers(&self) -> Result<()> {
        cmd!(
            self,
            "api",
            "module_{LEGACY_HARDCODED_INSTANCE_ID_WALLET}_block_height"
        )
        .run()
        .await?;
        Ok(())
    }

    pub async fn use_gateway(&self, gw: &Gatewayd) -> Result<()> {
        let pub_key = match &gw.ln {
            Some(LightningNode::Cln(cln)) => cln.pub_key().await?,
            Some(LightningNode::Lnd(lnd)) => lnd.pub_key().await?,
            None => {
                return Err(anyhow::anyhow!(
                    "Gatewayd is disconnected from the Lightning Node"
                ))
            }
        };
        cmd!(self, "switch-gateway", pub_key.clone()).run().await?;
        cmd!(self, "ng", "switch-gateway", pub_key).run().await?;
        info!(
            "Using {name} gateway",
            name = gw.ln.as_ref().unwrap().name()
        );
        Ok(())
    }

    pub async fn generate_epochs(&self, epochs: usize) -> Result<()> {
        for _ in 0..epochs {
            self.bitcoind.mine_blocks(10).await?;
            self.await_block_sync().await?;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct Fedimintd {
    _bitcoind: Bitcoind,
    process: ProcessHandle,
}

impl Fedimintd {
    pub async fn new(
        process_mgr: &ProcessManager,
        bitcoind: Bitcoind,
        peer_id: usize,
        env: &vars::Fedimintd,
    ) -> Result<Self> {
        info!("fedimintd-{peer_id} started");
        let process = process_mgr
            .spawn_daemon(
                &format!("fedimintd-{peer_id}"),
                cmd!("fedimintd").envs(env.vars()),
            )
            .await?;

        Ok(Self {
            _bitcoind: bitcoind,
            process,
        })
    }

    pub async fn kill(self) -> Result<()> {
        self.process.kill().await?;
        Ok(())
    }
}

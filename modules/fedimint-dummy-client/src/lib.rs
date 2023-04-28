use std::sync::Arc;

use anyhow::format_err;
use fedimint_client::derivable_secret::DerivableSecret;
use fedimint_client::module::gen::ClientModuleGen;
use fedimint_client::module::{ClientModule, PrimaryClientModule};
use fedimint_client::sm::{
    ClientSMDatabaseTransaction, Context, DynState, ModuleNotifier, OperationId, State,
    StateTransition,
};
use fedimint_client::transaction::{ClientInput, ClientOutput};
use fedimint_client::DynGlobalClientContext;
use fedimint_core::core::{IntoDynInstance, ModuleInstanceId};
use fedimint_core::db::{Database, ModuleDatabaseTransaction};
use fedimint_core::encoding::{Decodable, Encodable};
use fedimint_core::module::{ExtendsCommonModuleGen, ModuleCommon, TransactionItemAmount};
use fedimint_core::{apply, async_trait_maybe_send, Amount, TransactionId};
pub use fedimint_dummy_common as common;
use fedimint_dummy_common::config::DummyClientConfig;
use fedimint_dummy_common::{DummyCommonGen, DummyInput, DummyModuleTypes, DummyOutput};

use crate::db::DummyClientFundsKeyV0;

mod db;

#[derive(Debug)]
pub struct DummyClientModule {
    cfg: DummyClientConfig,
    account: String,
    notifier: ModuleNotifier<DynGlobalClientContext, DummyClientStateMachine>,
}

#[derive(Debug, Clone)]
pub struct DummyClientContext;

// TODO: Boiler-plate
impl Context for DummyClientContext {}

/// Tracks a state of the client until completion
#[derive(Debug, Clone, Eq, PartialEq, Decodable, Encodable)]
pub struct DummyClientStateMachine {
    pub operation_id: OperationId,
    pub txid: TransactionId,
    pub idx: u64,
    pub amount: Amount,
    pub state: DummyClientState,
}

impl DummyClientStateMachine {
    async fn add_amount(self, dbtx: &mut ClientSMDatabaseTransaction<'_, '_>) -> Self {
        let funds = get_funds(&mut dbtx.module_tx()).await;
        set_funds(&mut dbtx.module_tx(), funds + self.amount).await;
        self.done().await
    }

    async fn done(self) -> Self {
        let mut state_machine = self;
        state_machine.state = DummyClientState::Done;
        state_machine
    }
}

/// Possible states
#[derive(Debug, Clone, Eq, PartialEq, Decodable, Encodable)]
pub enum DummyClientState {
    Input,
    Output,
    Done,
}

impl State for DummyClientStateMachine {
    type ModuleContext = DummyClientContext;
    type GlobalContext = DynGlobalClientContext;

    fn transitions(
        &self,
        _context: &Self::ModuleContext,
        global_context: &Self::GlobalContext,
    ) -> Vec<StateTransition<Self>> {
        match self.state {
            DummyClientState::Input => vec![
                // input rejected, add back our funds
                StateTransition::new(
                    await_tx_rejected(global_context.clone(), self.operation_id, self.txid),
                    |dbtx, (), state: Self| Box::pin(state.add_amount(dbtx)),
                ),
                // input accepted, we are done
                StateTransition::new(
                    await_tx_accepted(global_context.clone(), self.operation_id, self.txid),
                    |_dbtx, (), state: Self| Box::pin(state.done()),
                ),
            ],
            DummyClientState::Output => vec![
                // output rejected, we are done
                StateTransition::new(
                    await_tx_rejected(global_context.clone(), self.operation_id, self.txid),
                    |_dbtx, (), state: Self| Box::pin(state.done()),
                ),
                // output accepted, add to our funds
                StateTransition::new(
                    await_tx_accepted(global_context.clone(), self.operation_id, self.txid),
                    |dbtx, (), state: Self| Box::pin(state.add_amount(dbtx)),
                ),
            ],
            DummyClientState::Done => vec![],
        }
    }

    fn operation_id(&self) -> OperationId {
        self.operation_id
    }
}

// TODO: Boiler-plate
async fn await_tx_rejected(context: DynGlobalClientContext, id: OperationId, txid: TransactionId) {
    context.await_tx_rejected(id, txid).await
}

// TODO: Boiler-plate
async fn await_tx_accepted(context: DynGlobalClientContext, id: OperationId, txid: TransactionId) {
    context.await_tx_accepted(id, txid).await
}

// TODO: Boiler-plate
impl IntoDynInstance for DummyClientStateMachine {
    type DynType = DynState<DynGlobalClientContext>;

    fn into_dyn(self, instance_id: ModuleInstanceId) -> Self::DynType {
        DynState::from_typed(instance_id, self)
    }
}

impl ClientModule for DummyClientModule {
    type Common = DummyModuleTypes;
    type ModuleStateMachineContext = DummyClientContext;
    type States = DummyClientStateMachine;

    fn context(&self) -> Self::ModuleStateMachineContext {
        DummyClientContext
    }

    fn input_amount(&self, input: &<Self::Common as ModuleCommon>::Input) -> TransactionItemAmount {
        TransactionItemAmount {
            amount: input.amount,
            fee: self.cfg.tx_fee,
        }
    }

    fn output_amount(
        &self,
        output: &<Self::Common as ModuleCommon>::Output,
    ) -> TransactionItemAmount {
        TransactionItemAmount {
            amount: output.amount,
            fee: self.cfg.tx_fee,
        }
    }
}

/// Creates exact inputs and outputs for the module
#[apply(async_trait_maybe_send)]
impl PrimaryClientModule for DummyClientModule {
    async fn create_sufficient_input(
        &self,
        dbtx: &mut ModuleDatabaseTransaction<'_>,
        operation_id: OperationId,
        min_amount: Amount,
    ) -> anyhow::Result<ClientInput<<Self::Common as ModuleCommon>::Input, Self::States>> {
        // Check and subtract from our funds
        let funds = get_funds(dbtx).await;
        if funds < min_amount {
            return Err(format_err!("Insufficient funds"));
        }
        set_funds(dbtx, funds - min_amount).await;

        // Construct the state machine to track our input
        let state_machines = Arc::new(move |txid, idx| {
            vec![DummyClientStateMachine {
                operation_id,
                txid,
                idx,
                amount: min_amount,
                state: DummyClientState::Input,
            }]
        });

        Ok(ClientInput {
            input: DummyInput {
                amount: min_amount,
                account: self.account.clone(),
            },
            keys: vec![],
            state_machines,
        })
    }

    async fn create_exact_output(
        &self,
        _dbtx: &mut ModuleDatabaseTransaction<'_>,
        operation_id: OperationId,
        amount: Amount,
    ) -> ClientOutput<<Self::Common as ModuleCommon>::Output, Self::States> {
        // Construct the state machine to track our output
        let state_machines = Arc::new(move |txid, idx| {
            vec![DummyClientStateMachine {
                operation_id,
                txid,
                idx,
                amount,
                state: DummyClientState::Output,
            }]
        });

        ClientOutput {
            output: DummyOutput {
                amount,
                account: self.account.clone(),
            },
            state_machines,
        }
    }
}

async fn set_funds(dbtx: &mut ModuleDatabaseTransaction<'_>, amount: Amount) {
    dbtx.insert_entry(&DummyClientFundsKeyV0, &amount).await;
}

async fn get_funds(dbtx: &mut ModuleDatabaseTransaction<'_>) -> Amount {
    let funds = dbtx.get_value(&DummyClientFundsKeyV0).await;
    funds.unwrap_or(Amount::ZERO)
}

#[derive(Debug, Clone)]
pub struct DummyClientGen;

// TODO: Boilerplate-code
impl ExtendsCommonModuleGen for DummyClientGen {
    type Common = DummyCommonGen;
}

/// Generates the client module
#[apply(async_trait_maybe_send!)]
impl ClientModuleGen for DummyClientGen {
    type Module = DummyClientModule;
    type Config = DummyClientConfig;

    async fn init(
        &self,
        cfg: Self::Config,
        _db: Database,
        _module_root_secret: DerivableSecret,
        notifier: ModuleNotifier<DynGlobalClientContext, <Self::Module as ClientModule>::States>,
    ) -> anyhow::Result<Self::Module> {
        Ok(DummyClientModule {
            cfg,
            account: rand::random::<u64>().to_string(),
            notifier,
        })
    }
}

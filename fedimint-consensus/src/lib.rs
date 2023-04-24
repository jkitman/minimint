use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};

use aleph_bft::{
    DataProvider, FinalizationHandler, Index, Keychain, Network, NodeCount, NodeIndex, Recipient,
};
use async_trait::async_trait;
use bitcoin_hashes::sha256;
use fedimint_core::epoch::SerdeConsensusItem;
use fedimint_core::net::peers::{IPeerConnections, PeerConnections};
use fedimint_core::PeerId;
use serde::{Deserialize, Serialize};

trait BftAlgorithm<SignatureShare, SignatureSet> {
    fn new(
        keychain: impl BftKeychain<SignatureShare, SignatureSet>,
        backup_dir: std::path::Path,
        last_processed_item: u64,
    ) -> BftChannels;

    fn get_items(processed_item: u64) -> dyn futures::Stream<Item = BftBatchMerkleTree<SignatureSet>>;
}

trait BftKeychain<SignatureShare, SignatureSet> {
    fn our_id(&self) -> PeerId;

    fn peers(&self) -> Vec<PeerId>;

    fn sign(&self, msg: &[u8]) -> SignatureShare;

    fn combine(&self, shares: Vec<SignatureShare>) -> SignatureSet;

    fn verify(&self, msg: &[u8], share: &SignatureShare, peer: PeerId) -> bool;
}

struct BftBatchMerkleTree<SignatureSet> {
    sigature: SignatureSet,
    hash: sha256::Hash,
    root: BftBatchMerkleBranch
}

struct BftBatchMerkleBranch {
    hash: sha256::Hash,
    left: Option<Box<BftBatchMerkleBranch>>,
    right: Option<Box<BftBatchMerkleBranch>>,
    item: Option<BftConsensusItem>
}

pub struct BftMessage(pub Vec<u8>);

pub struct BftItem(pub Vec<u8>);

pub struct BftConsensusItem {
    item: BftItem,
    peer: PeerId,
    number: u64,
}

pub struct BftChannels {
    send_to_consensus: Sender<BftItem>,
    receive_from_consensus: Receiver<BftConsensusItem>,
    send_from_network: Sender<(PeerId, BftMessage)>,
    receive_to_network: Receiver<(BftRecipient, BftMessage)>,
}

pub enum BftRecipient {
    AllPeers,
    Peer(PeerId)
}
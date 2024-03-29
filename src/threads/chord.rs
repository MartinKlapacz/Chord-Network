use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use log::{debug, error, info, warn};
use tokio::sync::mpsc;
use tokio::sync::oneshot::Receiver;
use tokio::time::sleep;
use tokio_stream::Stream;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use tonic::transport::Channel;

use chord::utils::types::{Address, HashPos, Key, KvStore};

use crate::node::finger_entry::FingerEntry;
use crate::node::finger_table::FingerTable;
use crate::node::successor_list::SuccessorList;
use crate::threads::chord::chord_proto::{AddressMsg, Empty, FingerEntryMsg, GetKvStoreDataResponse, GetKvStoreSizeResponse, GetPredecessorResponse, GetRequest, GetResponse, GetStatus, HashPosMsg, KvPairDebugMsg, KvPairMsg, NodeSummaryMsg, NotifyRequest, PowTokenMsg, PutRequest, SuccessorListMsg};
use crate::threads::chord::chord_proto::chord_client::ChordClient;
use crate::utils::constants::DEBUG_RPCS_UNAVAILABLE_ERROR_MESSAGE;
use crate::utils::crypto::{hash, HashRingKey, is_between};
use crate::utils::proof_of_work::PowToken;
use crate::utils::time::{has_expired, now};
use crate::utils::types::ExpirationDate;

pub mod chord_proto {
    tonic::include_proto!("chord");
}


/// The struct representing the running node. 
pub struct ChordService {
    /// gRPC address of the node
    address: String,
    /// position in the hash ring
    pos: HashPos,
    /// data structure containing routing information
    finger_table: Arc<Mutex<FingerTable>>,
    /// predecessor handle
    predecessor_option: Arc<Mutex<Option<FingerEntry>>>,
    /// key value storage
    kv_store: Arc<Mutex<KvStore>>,
    /// round-robin pointer to the current finger updated by the fix_finger procedure  
    fix_finger_index: Arc<Mutex<usize>>,
    /// list of the next n successor
    successor_list: Arc<Mutex<SuccessorList>>,
    /// required number of trailing 0 bytes for a POW token to be valid
    pow_difficulty: usize,
    /// flag that enables debugging RPCs
    dev_mode: bool
}

const MAX_RETRIES: u64 = 15;
const CONNECTION_RETRY_SLEEP: u64 = 100;

/// connection helper functions

pub(crate) async fn connect(address: &Address) -> Result<ChordClient<Channel>, tonic::transport::Error> {
    ChordClient::connect(format!("http://{}", address)).await
}

pub(crate) async fn connect_without_retry(address: &Address) -> ChordClient<Channel> {
    connect(address).await.unwrap()
}

pub(crate) async fn connect_with_retry(address: &Address) -> Result<ChordClient<Channel>, Status> {
    let mut retries = 0;
    loop {
        match connect(address).await {
            Ok(client) => return Ok(client),
            Err(e) => {
                retries += 1;
                if retries > MAX_RETRIES {
                    // You can return an error or handle it differently
                    return Err(Status::unavailable("Reached maximum number of connection retries"));
                }
                // Log error or do something with it
                warn!("Failed to connect to {}: {}. Retrying...", address, e);
                sleep(Duration::from_millis(CONNECTION_RETRY_SLEEP)).await; // Wait 100 ms before retrying
            }
        }
    }
}

pub(crate) async fn connect_to_first_reachable_node(address_list: &Vec<Address>) -> Option<(ChordClient<Channel>, Address)> {
    for address in address_list {
        if let Ok(successor_client) = connect_with_retry(address).await {
            return Some((successor_client, address.clone()));
        }
    };
    None
}


impl ChordService {
    pub async fn new(rx: Receiver<(Arc<Mutex<FingerTable>>, Arc<Mutex<Option<FingerEntry>>>, Arc<Mutex<KvStore>>, Arc<Mutex<SuccessorList>>)>, url: &String, pow_difficulty: usize, dev_mode: bool) -> ChordService {
        let (finger_table_arc, predecessor_option_arc, kv_store_arc, successor_list_arc) = rx.await.unwrap();
        ChordService {
            address: url.clone(),
            pos: hash(&url.as_bytes()),
            finger_table: finger_table_arc,
            predecessor_option: predecessor_option_arc,
            kv_store: kv_store_arc,
            fix_finger_index: Arc::new(Mutex::new(0)),
            successor_list: successor_list_arc,
            pow_difficulty,
            dev_mode
        }
    }

    pub async fn get_successor_address(&self) -> Address {
        self.successor_list.lock().unwrap().successors[0].clone()
    }

    pub async fn set_successor(&self, new_successor_address: &Address) -> () {
        self.successor_list.lock().unwrap().successors[0] = new_successor_address.clone();
        self.finger_table.lock().unwrap().fingers[0].address = new_successor_address.clone();
    }


    pub async fn get_client_for_closest_successor(&self) -> (ChordClient<Channel>, Address) {
        let successors = {
            self.successor_list.lock().unwrap().successors.clone()
        };
        if let Some(client_and_address) = connect_to_first_reachable_node(&successors).await {
            return client_and_address;
        } else {
            panic!("All successor in successor list are unreachable")
        }
    }

    pub async fn get_predecessor_client(&self) -> Option<ChordClient<Channel>> {
        let predecessor_option_clone = {
            self.predecessor_option.lock().unwrap().clone()
        };
        if let Some(ref predecessor) = predecessor_option_clone {
            Some(connect_with_retry(&predecessor.address).await.unwrap())
        } else {
            None
        }
    }
}


#[tonic::async_trait]
impl chord_proto::chord_server::Chord for ChordService {
    
    /// finds the next responsible node for a given position in the hash ring
    async fn find_successor(
        &self,
        request: Request<chord_proto::HashPosMsg>,
    ) -> Result<Response<chord_proto::AddressMsg>, Status> {
        let key: HashPos = request.into_inner().into();

        let direct_successor_address = self.get_successor_address().await;
        let successor_pos: HashPos = hash(direct_successor_address.as_bytes());
        let key_pos_msg: HashPosMsg = HashPosMsg {
            key: key.to_be_bytes().to_vec()
        };

        let successor_address_msg: AddressMsg = if is_between(key, self.pos + 1, successor_pos, false, false) {
            direct_successor_address.into()
        } else {
            let closest_preceding_node_address = self.find_closest_preceding_finger(Request::new(key_pos_msg.clone()))
                .await
                .unwrap().into_inner();

            match connect_with_retry(&closest_preceding_node_address.address).await {
                Ok(mut closest_preceding_node_client) => {
                    closest_preceding_node_client.find_successor(Request::new(key.into()))
                        .await?
                        .into_inner()
                }
                Err(status) => {
                    // if node returned by closest_preceding_node_address is unavailable, delegate find_successor call to predecessor
                    let mut counter = 0;
                    loop {
                        if let Some(mut predecessor_client) = self.get_predecessor_client().await {
                            return predecessor_client.find_successor(Request::new(key.into())).await;
                        }
                        if counter > 20 {
                            return Err(status);
                        }
                        counter += 1;
                    }
                }
            }
        };

        debug!("Received find_successor call for {:?}, successor is {:?}", key, successor_address_msg);
        Ok(Response::new(successor_address_msg))
    }


    /// returns current node's value of the predecessor handle
    async fn get_predecessor(&self, _request: Request<Empty>) -> Result<Response<GetPredecessorResponse>, Status> {
        let predecessor = match *self.predecessor_option.lock().unwrap() {
            Some(ref predecessor) => {
                debug!("Received get predecessor call, predecessor is {:?}", predecessor.address);
                predecessor.address.clone()
            }
            None => {
                debug!("Received get predecessor call, predecessor is Nil");
                Address::default()
            }
        };
        Ok(Response::new(GetPredecessorResponse { address_optional: Some(predecessor.into()) }))
    }

    /// returns the current node's successor list
    async fn get_successor_list(&self, _: Request<Empty>) -> Result<Response<SuccessorListMsg>, Status> {
        Ok(Response::new(self.successor_list.lock().unwrap().clone().into()))
    }


    /// find the finger in the finger table that closest precedes the hash position given in the request
    async fn find_closest_preceding_finger(&self, request: Request<HashPosMsg>) -> Result<Response<FingerEntryMsg>, Status> {
        let key = HashPos::from_be_bytes(request.get_ref().clone().key.try_into().unwrap());
        for finger in self.finger_table.lock().unwrap().fingers.iter().rev() {
            if finger.address.eq(&Address::default()) {
                // ignore yet uninitialized entries
                continue;
            }
            let node_pos = hash(finger.get_address().as_bytes());
            if is_between(node_pos, self.pos, key, true, true) {
                return Ok(Response::new(FingerEntryMsg {
                    id: node_pos.to_be_bytes().to_vec(),
                    address: finger.clone().get_address().into(),
                }));
            }
        }
        Ok(Response::new(FingerEntryMsg {
            id: self.pos.to_be_bytes().to_vec(),
            address: self.address.clone().into(),
        }))
    }

    /// returns a human readable node summary (requires dev_mode = true)
    async fn get_node_summary(&self, _: Request<Empty>) -> Result<Response<NodeSummaryMsg>, Status> {
        if !self.dev_mode {
            return Err(Status::unimplemented(DEBUG_RPCS_UNAVAILABLE_ERROR_MESSAGE))
        }
        let finger_table_guard = self.finger_table.lock().unwrap();
        let predecessor_option = self.predecessor_option.lock().unwrap();
        let successor_list = self.successor_list.lock().unwrap();

        Ok(Response::new(NodeSummaryMsg {
            url: self.address.clone(),
            pos: Some(self.pos.into()),
            predecessor: match predecessor_option.clone() {
                Some(predecessor) => Some(predecessor.into()),
                None => None
            },
            finger_entries: finger_table_guard.fingers.iter()
                .map(|finger| finger.clone())
                .map(|finger| finger.into())
                .collect(),
            successor_list: Some(successor_list.clone().into()),
        }))
    }
    
    /// returns the number of key value pairs stored in storage (dev_mode = true)
    async fn get_kv_store_size(&self, _: Request<Empty>) -> Result<Response<GetKvStoreSizeResponse>, Status> {
        if !self.dev_mode {
            return Err(Status::unimplemented(DEBUG_RPCS_UNAVAILABLE_ERROR_MESSAGE))
        }
        Ok(Response::new(GetKvStoreSizeResponse {
            size: self.kv_store.lock().unwrap().len() as u32
        }))
    }

    /// returns the full data stored in storage (dev_mode = true)
    async fn get_kv_store_data(&self, _: Request<Empty>) -> Result<Response<GetKvStoreDataResponse>, Status> {
        if !self.dev_mode {
            return Err(Status::unimplemented(DEBUG_RPCS_UNAVAILABLE_ERROR_MESSAGE))
        }
        let kv_pairs = {
            let one = HashPos::one();
            self.kv_store.lock().unwrap()
                .iter()
                .filter(move |(key, _)| is_between(hash(*key), one + 1, one, false, false))
                .map(|(key, value)| KvPairDebugMsg {
                    key: key.map(|b| b.to_string()).join(" "),
                    value: value.0.clone(),
                }).collect()
        };
        Ok(Response::new(GetKvStoreDataResponse { kv_pairs }))
    }

    /// GET operation on the key value storage 
    async fn get(&self, request: Request<GetRequest>) -> Result<Response<GetResponse>, Status> {
        let key: Key = request.into_inner().key.try_into().unwrap();
        let predecessor_pos = {
            if let Some(finger_entry) = self.predecessor_option.lock().unwrap().clone() {
                hash(finger_entry.address.as_bytes())
            } else {
                return Err(Status::internal("Predecessor not set"));
            }
        };
        if is_between(hash(&key), predecessor_pos + 1, self.pos, false, false) {
            let mut kv_store_guard = self.kv_store.lock().unwrap();

            match kv_store_guard.get(&key).cloned() {
                Some((value, expiration_date)) => {
                    if has_expired(&expiration_date) {
                        let since = now().as_secs() - expiration_date;
                        info!("Received GET request for key {:?}, but value is expired since {} seconds!", key, since);
                        kv_store_guard.remove(&key);
                        info!("Removed expired key {:?}", &key);
                        return Ok(Response::new(GetResponse {
                            value: value.clone(),
                            status: GetStatus::Expired.into(),
                        }));
                    } else {
                        info!("Received GET request for key {:?}, value is: {}", key, value);
                        return Ok(Response::new(GetResponse {
                            value: value.clone(),
                            status: GetStatus::Ok.into(),
                        }));
                    }
                }
                None => {
                    warn!("Received GET request for key {:?}, but not found)", key);
                    return Ok(Response::new(GetResponse {
                        value: String::default(),
                        status: GetStatus::NotFound.into(),
                    }));
                }
            }
        } else {
            error!("This node is responsible for interval ({}, {}] !", predecessor_pos, self.pos);
            let msg = format!("Node ({}, {}) is responsible for range ({}, {}]", self.address, self.pos, predecessor_pos, self.pos);
            return Err(Status::internal(msg));
        };
    }
    
    /// PUT operation on the key value storage 
    async fn put(&self, request: Request<PutRequest>) -> Result<Response<Empty>, Status> {
        let key = request.get_ref().key.clone().try_into().unwrap();
        let ttl = request.get_ref().ttl;
        let replication = request.get_ref().replication;
        let value = &request.get_ref().value;

        // todo: handle replication

        let expiration_date = now().as_secs() + ttl;
        let _ = self.kv_store.lock().unwrap().insert(key, (value.clone(), expiration_date));
        info!("Received PUT request ({:?}, {}) with ttl {} and replication {}", hash(&key), value, ttl, replication);
        Ok(Response::new(Empty {}))
    }
    
    
    /// updates the finger table entries one after another in a round robin fashion by calling 
    /// find_successor for position the finger table entries point to
    async fn fix_fingers(&self, _: Request<Empty>) -> Result<Response<Empty>, Status> {
        let index = (*self.fix_finger_index.lock().unwrap() + 1) % HashPos::finger_count();
        debug!("Fixing finger entry {}", index);
        let lookup_position = self.pos.overflowing_add(HashPos::one().overflowing_shl(index as u32).0).0;

        let responsible_node_for_lookup_pos_response_result = self.find_successor(Request::new(HashPosMsg {
            key: lookup_position.to_be_bytes().to_vec(),
        })).await;

        match responsible_node_for_lookup_pos_response_result {
            Ok(responsible_node_for_lookup_pos_response) => {
                let responsible_node_address: Address = responsible_node_for_lookup_pos_response.into_inner().into();
                if index == 1 {
                    self.successor_list.lock().unwrap().successors[0] = responsible_node_address.clone();
                }
                *self.fix_finger_index.lock().unwrap() = index;
                self.finger_table.lock().unwrap().fingers[index].address = responsible_node_address;
            }
            Err(e) => warn!("An error occurred during fix_fingers: {}", e)
        }
        Ok(Response::new(Empty {}))
    }

    /// updates the successor list and calls notify on the successor
    async fn stabilize(&self, _: Request<Empty>) -> Result<Response<Empty>, Status> {
        let (mut current_successor_client, current_successor_address) = self.get_client_for_closest_successor().await;
        let current_successors_predecessor_address_optional: Option<Address> = current_successor_client.get_predecessor(Request::new(Empty {}))
            .await
            .unwrap().into_inner().address_optional.map(|address| address.into());

        if let Some(current_successors_predecessor_address) = current_successors_predecessor_address_optional {
            if !current_successors_predecessor_address.is_empty() {
                let current_successors_predecessor_pos = hash(current_successors_predecessor_address.as_bytes());
                let successor_pos = hash(current_successor_address.as_bytes());
                if is_between(current_successors_predecessor_pos, self.pos + 1, successor_pos, false, true) {
                    self.set_successor(&current_successors_predecessor_address).await;
                }
            }
        }

        let mut successor_client: ChordClient<Channel> = connect_without_retry(&self.get_successor_address().await)
            .await;

        let notify_request: NotifyRequest = NotifyRequest {
            address: Some(self.address.clone().into()),
            pow_token: Some(PowToken::generate(self.pow_difficulty).into()),
        };

        let mut data_handoff_stream = successor_client.notify(Request::new(notify_request))
            .await?
            .into_inner();

        while let Some(pair) = data_handoff_stream.message().await.unwrap() {
            let key: Key = pair.key.try_into().unwrap();
            self.kv_store.lock().unwrap().insert(key, (pair.value, pair.expiration_date));
        }

        Ok(Response::new(Empty {}))
    }


    type NotifyStream = Pin<Box<dyn Stream<Item=Result<KvPairMsg, Status>> + Send>>;

    /// Notify call that is typically called on the successor to notify it about this node's presence.
    /// If this node just joined the cluster the new successor node needs to update its predecessor
    /// handle and send a part of its data to this very node. The latter part is implemented using
    /// server side streaming rpc.
    async fn notify(&self, request: Request<NotifyRequest>) -> Result<Response<Self::NotifyStream>, Status> {

        let notify_request = request.into_inner();
        let pow_token_msg: PowTokenMsg = notify_request.pow_token.unwrap();
        let pow_token: PowToken = pow_token_msg.into();

        let (has_expired, valid) = pow_token.validate();
        if has_expired {
            return Err(Status::cancelled("Pow token expired"))
        }
        if !valid {
            return Err(Status::cancelled(format!("Invalid pow token: {}", pow_token)))
        }

        let (tx, rx) = mpsc::unbounded_channel();
        let caller_address: &Address = &notify_request.address.unwrap().into();
        let caller_pos = hash(caller_address.as_bytes());

        let mut predecessor_option_guard = self.predecessor_option.lock().unwrap();

        let (update_predecessor_to_caller, lower, upper) = match *predecessor_option_guard {
            Some(ref prev_predecessor) => {
                let lower = hash(prev_predecessor.address.as_bytes());
                let upper = self.pos;
                if is_between(caller_pos, lower + 1, upper, false, true) {
                    (true, lower, caller_pos)
                } else {
                    (false, HashPos::default(), HashPos::default())
                }
            }
            None => {
                (true, self.pos + 1, caller_pos)
            }
        };

        if update_predecessor_to_caller {
            *predecessor_option_guard = Some(FingerEntry {
                key: caller_pos,
                address: caller_address.clone(),
            });
            debug!("Updated predecessor due to notify-call");
        }

        let kv_store_arc = self.kv_store.clone();
        if update_predecessor_to_caller {
            tokio::spawn(async move {
                info!("Handing over data from ({}, {}]", lower, upper);

                let kv_store_lock_result = kv_store_arc.lock();
                let mut kv_store_lock = kv_store_lock_result.unwrap();

                let pairs_to_handoff: Vec<(Vec<u8>, String, ExpirationDate)> = kv_store_lock
                    .iter()
                    .filter(|(key, _)| is_between(hash(*key), lower, upper, false, false))
                    .map(|(key, (value, ttl))| (key.to_vec(), value.clone(), ttl.clone()))
                    .collect();

                for (key, value, expiration_date) in pairs_to_handoff.iter() {
                    let pair = KvPairMsg {
                        key: key.to_vec(),
                        value: value.clone(),
                        expiration_date: expiration_date.clone(),
                    };
                    debug!("Handing over KV pair ({:?}, {})", key, value);
                    match tx.send(Ok(pair)) {
                        Ok(_) => {
                            let key: Key = key.clone().try_into().unwrap();
                            kv_store_lock.remove(&key);
                        }
                        Err(err) => {
                            error!("ERROR: failed to update stream client: {:?}", err)
                        }
                    }
                }
                info!("Data handoff finished, transferred {} pairs", pairs_to_handoff.len())
            });
        };

        let stream = UnboundedReceiverStream::new(rx);
        Ok(Response::new(Box::pin(stream) as Self::NotifyStream))
    }

    /// Receives a stream of key value pairs in a stream. Nodes that are about to shut down use this
    /// call to send their data to their successor, as the successor will be responsible for this 
    /// area in the hash ring.
    async fn handoff(&self, request: Request<Streaming<KvPairMsg>>) -> Result<Response<Empty>, Status> {
        let mut stream = request.into_inner();
        let mut counter = 0;
        info!("Receiving handoff data from predecessor!");
        while let Some(kv_msg) = stream.message().await? {
            let key: Key = kv_msg.key.try_into().unwrap();
            self.kv_store.lock().unwrap().insert(key, (kv_msg.value, kv_msg.expiration_date));
            debug!("Received kv-pair!");
            counter += 1;
        };
        info!("Received {} from predecessor", counter);
        Ok(Response::new(Empty {}))
    }

    /// dummy call, used to check if the receiver node is still available.
    async fn health(&self, _: Request<Empty>) -> Result<Response<Empty>, Status> {
        Ok(Response::new(Empty {}))
    }
}


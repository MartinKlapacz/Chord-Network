use std::error::Error;
use std::sync::{Arc, Mutex};

use log::info;
use tokio::sync::oneshot::Sender;
use tonic::Request;

use crate::node::finger_entry::FingerEntry;
use crate::node::finger_table::FingerTable;
use crate::node::successor_list::SuccessorList;
use crate::threads::chord::chord_proto::{Empty, HashPosMsg};
use crate::threads::chord::connect_with_retry;
use crate::utils::crypto::hash;
use crate::utils::types::{Address, KvStore};

/// Setup function that distinguishes betwenn two scenarios:
/// 1. The node joins an existing cluster (join_address_option is None)
/// 2. The node starts up a new cluster (join_address_option is given)
pub async fn setup(join_address_option: Option<Address>, own_grpc_address_str: &String,
                   tx_grpc_thread: Sender<(Arc<Mutex<FingerTable>>, Arc<Mutex<Option<FingerEntry>>>, Arc<Mutex<KvStore>>, Arc<Mutex<SuccessorList>>)>,
                   tx_handoff_thread: Sender<Arc<Mutex<KvStore>>>,
                   tx_check_predecessor: Sender<Arc<Mutex<Option<FingerEntry>>>>,
                   tx_successor_list: Sender<Arc<Mutex<SuccessorList>>>,
                   tx_web_interface: Sender<Arc<Mutex<FingerTable>>>,
) -> Result<(), Box<dyn Error>> {
    info!("Starting up setup thread");
    let own_id = hash(own_grpc_address_str.as_bytes());

    let finger_table_arc = Arc::new(Mutex::new(FingerTable::new(&own_id)));
    let kv_store_arc = Arc::new(Mutex::new(KvStore::new()));
    let predecessor_option_arc = Arc::new(Mutex::new(None));
    let mut successor_list_arc = Arc::new(Mutex::new(SuccessorList::default()));

    match join_address_option {
        Some(peer_address_str) => {
            info!("Joining existing cluster");
            let mut join_peer_client = connect_with_retry(&peer_address_str)
                .await
                .unwrap();
            let successor_address: Address = join_peer_client.find_successor(Request::new(HashPosMsg {
                key: own_id.to_be_bytes().to_vec(),
            })).await.unwrap().into_inner().into();

            let mut successor_client = connect_with_retry(&successor_address)
                .await
                .unwrap();
            let _: SuccessorList = successor_client.get_successor_list(Request::new(Empty {}))
                .await
                .unwrap().into_inner().into();

            successor_list_arc = Arc::new(Mutex::new(SuccessorList::new(own_grpc_address_str, &successor_address)));
            finger_table_arc.lock().unwrap().fingers[0].address = successor_address;
        }
        None => {
            info!("Starting up a new cluster");
            successor_list_arc = Arc::new(Mutex::new(SuccessorList::new(own_grpc_address_str, own_grpc_address_str)));
            finger_table_arc.lock().unwrap().fingers[0].address = own_grpc_address_str.clone();
        }
    };

    tx_grpc_thread.send((finger_table_arc.clone(), predecessor_option_arc.clone(), kv_store_arc.clone(), successor_list_arc.clone())).unwrap();
    tx_web_interface.send(finger_table_arc.clone()).unwrap();
    tx_handoff_thread.send(kv_store_arc).unwrap();
    tx_check_predecessor.send(predecessor_option_arc).unwrap();
    tx_successor_list.send(successor_list_arc).unwrap();
    Ok(())
}

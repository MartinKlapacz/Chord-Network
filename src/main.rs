use std::error::Error;
use std::process::exit;
use std::time::Duration;

use clap::Parser;
use log::{debug, info, LevelFilter, warn};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tonic::transport::Server;
use tokio::time::sleep;
use tonic::Request;

use crate::threads::chord::{ChordService, Address};
use crate::threads::chord::chord_proto::chord_client::ChordClient;
use crate::threads::chord::chord_proto::chord_server::ChordServer;
use crate::threads::chord::chord_proto::Empty;
use crate::utils::cli::Cli;
use crate::threads::join::process_node_join;
use crate::threads::shutdown_handoff::shutdown_handoff;
use crate::threads::tcp_service::handle_client_connection;

mod node;
mod utils;
mod kv;
mod threads;


pub mod chord_proto {
    pub(crate) const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("chord_descriptor");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Cli::parse();
    simple_logger::SimpleLogger::new().env().with_level(LevelFilter::Debug).init().unwrap();

    let tcp_addr = args.tcp_address;

    let mut thread_handles = Vec::new();

    let peer_address_option = args.peer;
    let cloned_grpc_addr_1 = args.grpc_address.clone();
    let cloned_grpc_addr_2 = args.grpc_address.clone();
    let cloned_grpc_addr_3 = args.grpc_address.clone();
    let cloned_grpc_addr_4 = args.grpc_address.clone();

    let (tx1, rx_grpc_service) = oneshot::channel();
    let (tx2, rx_shutdown_handoff) = oneshot::channel();

    info!("Starting up setup thread");
    thread_handles.push(tokio::spawn(async move {
        process_node_join(peer_address_option, &cloned_grpc_addr_1, tx1, tx2)
            .await
            .unwrap();
    }));


    info!("Starting up tcp main thread on {}", tcp_addr);
    thread_handles.push(tokio::spawn(async move {
        let listener = TcpListener::bind(tcp_addr).await.unwrap();
        loop {
            let grpc_address = cloned_grpc_addr_3.clone();
            let (socket, _) = listener.accept().await.unwrap();
            info!("New client connection established");
            tokio::spawn(async move { handle_client_connection(socket, &grpc_address).await.unwrap() });
        }
    }));

    thread_handles.push(tokio::spawn(async move {
        let chord_service = ChordServer::new(ChordService::new(rx_grpc_service, &cloned_grpc_addr_2).await);
        info!("Starting up gRPC service on {}", cloned_grpc_addr_2);

        let reflection_service = tonic_reflection::server::Builder::configure()
            .register_encoded_file_descriptor_set(chord_proto::FILE_DESCRIPTOR_SET)
            .build()
            .unwrap();

        Server::builder()
            .add_service(chord_service)
            .add_service(reflection_service)
            .serve(cloned_grpc_addr_2.parse().unwrap())
            .await
            .unwrap();
    }));

    thread_handles.push(tokio::spawn(async move {
        shutdown_handoff(rx_shutdown_handoff).await.unwrap();
        exit(0)
    }));

    info!("Starting up periodic fix_fingers call");
    thread_handles.push(tokio::spawn(async move {
        let retry_connection_sleep_millis = 50;
        let retry_fix_fingers_sleep_millis = 1000;
        loop {
            match ChordClient::connect(format!("http://{}", cloned_grpc_addr_4.clone())).await {
                Ok(mut client) => {
                    loop {
                        client.fix_fingers(Request::new(Empty {}))
                            .await
                            .unwrap();
                        sleep(Duration::from_millis(retry_connection_sleep_millis)).await;
                    }
                },
                Err(e) => {
                    debug!("Failed connecting to local grpc service, retrying in {} millis", retry_fix_fingers_sleep_millis);
                    sleep(Duration::from_millis(retry_fix_fingers_sleep_millis)).await
                }
            }

        }
    }));

    for handle in thread_handles {
        handle.await?;
    }

    Ok(())
}





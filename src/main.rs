use std::{net::IpAddr, time::Duration, vec};

use anyhow::anyhow;
use log::{error, info, warn};
use tokio::{net::lookup_host, time::sleep};

use snap_coin::{
    api::api_server::{self},
    build_block,
    crypto::randomx_use_full_mode,
    economics::DEV_WALLET,
    full_node::{
        accept_block, auto_peer::start_auto_peer, connect_peer, create_full_node,
        p2p_server::start_p2p_server,
    },
    node::peer::PeerHandle,
};

use tracing_subscriber::prelude::*;

use crate::{sync::sync_blockchain, tui::run_tui};

mod sync;
mod tui;

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let args = std::env::args().collect::<Vec<_>>();
    let mut peers: Vec<String> = vec![];
    let mut reserved_ips: Vec<String> = vec![];
    let mut node_path = "./node-mainnet";
    let mut start_api = true;
    let mut api_port = 3003;
    let mut node_port = 8998;
    let mut create_genesis = false;
    let mut headless = false;
    let mut no_ibd = false;
    let mut no_auto_peer = false;
    let mut randomx_full_mode = false;

    for arg in args.iter().enumerate() {
        if arg.1 == "--peers" && args.get(arg.0 + 1).is_some() {
            peers = args[arg.0 + 1].split(',').map(|s| s.to_string()).collect();
        }
        if arg.1 == "--reserved-ips" && args.get(arg.0 + 1).is_some() {
            reserved_ips = args[arg.0 + 1].split(',').map(|s| s.to_string()).collect();
        }
        if arg.1 == "--node-path" && args.get(arg.0 + 1).is_some() {
            node_path = &args[arg.0 + 1];
        }
        if arg.1 == "--no-api" {
            start_api = false;
        }
        if arg.1 == "--no-ibd" {
            no_ibd = true;
        }
        if arg.1 == "--no-auto-peer" {
            no_auto_peer = true;
        }
        if arg.1 == "--headless" {
            headless = true;
        }
        if arg.1 == "--create-genesis" {
            create_genesis = true;
        }
        if arg.1 == "--full-memory" {
            randomx_full_mode = true;
        }
        if arg.1 == "--api-port" && args.get(arg.0 + 1).is_some() {
            api_port = args[arg.0 + 1].parse().expect("Invalid api port parameter");
        }
        if arg.1 == "--node-port" && args.get(arg.0 + 1).is_some() {
            node_port = args[arg.0 + 1]
                .parse()
                .expect("Invalid node port parameter");
        }
        if arg.1 == "--debug" {
            if tracing_subscriber::registry()
                .with(console_subscriber::spawn())
                .try_init()
                .is_err()
            {}
        }
    }

    if randomx_full_mode {
        randomx_use_full_mode();
    }

    let mut resolved_peers = Vec::new();

    for seed in &peers {
        match lookup_host(seed).await {
            Ok(addrs) => {
                if let Some(addr) = addrs.into_iter().next() {
                    resolved_peers.push(addr);
                }
            }
            Err(_) => return Err(anyhow!("Failed to resolve or parse seed peer: {seed}")),
        }
    }

    let mut parsed_reserved_ips: Vec<IpAddr> = vec![];
    for reserved_ip in reserved_ips {
        parsed_reserved_ips.push(reserved_ip.parse().expect("Reserved ip is invalid"));
    }

    // Create a node and connect it's initial peers to it
    let (blockchain, node_state) = create_full_node(node_path, !headless);
    for initial_peer in &resolved_peers {
        connect_peer(*initial_peer, &blockchain, &node_state).await?;
    }

    *node_state.is_syncing.write().await = true;

    // If no flags against it, start the Snap Coin API server
    if start_api {
        sleep(Duration::from_secs(1)).await;
        let api_server = api_server::Server::new(api_port, blockchain.clone(), node_state.clone());
        api_server.listen().await?;
    }

    // If the --create-genesis flag passed, create and submit a genesis block
    if create_genesis {
        let mut genesis = build_block(&*blockchain, &vec![], DEV_WALLET).await?;
        #[allow(deprecated)]
        genesis.compute_pow()?;
        accept_block(&blockchain, &node_state, genesis).await?;
    }

    // If an initial peer was passed, and no flags against it, connect to the first connected peer, and IBD from it
    if !resolved_peers.is_empty() && !no_ibd {
        let peer = node_state
            .connected_peers
            .read()
            .await
            .values()
            .collect::<Vec<&PeerHandle>>()[0]
            .clone();
        let blockchain = blockchain.clone();
        let node_state = node_state.clone();
        tokio::spawn(async move {
            sleep(Duration::from_secs(1)).await;
            info!(
                "Blockchain sync status {:?}",
                sync_blockchain(peer, blockchain).await
            );
            *node_state.is_syncing.write().await = false;
        });
    } else {
        *node_state.is_syncing.write().await = false;
    }

    if resolved_peers.len() != 0 {
        let resolved_peers = resolved_peers.clone();

        // Peer complete disconnection watchdog
        let blockchain = blockchain.clone();
        let node_state = node_state.clone();
        tokio::spawn(async move {
            loop {
                sleep(Duration::from_secs(30)).await;
                if node_state.connected_peers.read().await.len() == 0 {
                    warn!("All peers disconnected, trying to reconnect to seed peer");
                    let res = connect_peer(resolved_peers[0], &blockchain, &node_state).await;
                    match res {
                        Ok(peer) => {
                            info!("Reconnection status: OK");
                            info!(
                                "Re-sync status: {:?}",
                                sync_blockchain(peer, blockchain.clone()).await
                            );
                        }
                        Err(e) => {
                            error!("Reconnection status: {}", e);
                        }
                    }
                }
            }
        });
    }

    if !no_auto_peer {
        // No need to capture this join handle
        let _ = start_auto_peer(node_state.clone(), blockchain.clone(), parsed_reserved_ips);
    }

    let p2p_server_handle =
        start_p2p_server(node_port, blockchain.clone(), node_state.clone()).await?;

    if headless {
        info!("{:?}", p2p_server_handle.await);
    } else {
        run_tui(node_state, blockchain, node_port, node_path.to_string()).await?;
    }

    Ok(())
}

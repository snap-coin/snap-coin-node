use std::{time::Duration, vec};

use anyhow::anyhow;
use tokio::{net::lookup_host, time::sleep};


use snap_coin::{
    api::api_server::Server,
    build_block,
    economics::DEV_WALLET,
    node::{
        node::Node,
    },
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
    let mut node_path = "./node-testnet";
    let mut start_api = true;
    let mut api_port = 3003;
    let mut node_port = 8998;
    let mut create_genesis = false;
    let mut headless = false;
    let mut no_ibd = false;

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
        if arg.1 == "--headless" {
            headless = true;
        }
        if arg.1 == "--create-genesis" {
            create_genesis = true;
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

    let mut parsed_reserved_ips = vec![];
    for reserved_ip in reserved_ips {
        parsed_reserved_ips.push(reserved_ip.parse().expect("Reserved ip is invalid"));
    }

    let node = Node::new(node_path, node_port, parsed_reserved_ips);

    let handle = Node::init(node.clone(), resolved_peers.clone()).await?;
    node.write().await.is_syncing = true;

    if start_api {
        sleep(Duration::from_secs(1)).await;
        let api_server = Server::new(api_port, node.clone());
        api_server.listen().await?;
    }

    if create_genesis {
        let mut genesis = {
            let blockchain = &node.read().await.blockchain;
            let transactions = vec![];
            build_block(blockchain, &transactions, DEV_WALLET).await?
        };
        #[allow(deprecated)]
        genesis.compute_pow()?;
        Node::submit_block(node.clone(), genesis).await?;
    }
    if !resolved_peers.is_empty() && !no_ibd {
        let peer = node.read().await.peers[0].clone();
        let node = node.clone();
        tokio::spawn(async move {
            sleep(Duration::from_secs(1)).await;
            Node::log(format!(
                "[SYNC] Blockchain status {:?}",
                sync_blockchain(peer, node.clone()).await
            ));
            node.write().await.is_syncing = false;
        });
    } else {
        node.write().await.is_syncing = false;
    }

    if resolved_peers.len() != 0 {
        let node = node.clone();
        let resolved_peers = resolved_peers.clone();

        // Peer complete disconnection watchdog
        tokio::spawn(async move {
            loop {
                sleep(Duration::from_secs(30)).await;
                if node.read().await.peers.len() == 0 {
                    Node::log(
                        "[WATCHDOG] All peers disconnected, trying to reconnect to seed peer"
                            .into(),
                    );
                    let res = Node::connect_peer(node.clone(), resolved_peers[0]).await;
                    match res {
                        Ok((peer, _handle)) => {
                            node.write().await.peers.push(peer.clone());
                            Node::log("[WATCHDOG] Reconnection status: OK".into());
                            Node::log(format!(
                                "[WATCHDOG] Re-sync status: {:?}",
                                sync_blockchain(peer, node.clone()).await
                            ));
                        }
                        Err(e) => {
                            Node::log(format!("[WATCHDOG] Reconnection status: {}", e));
                        }
                    }
                }
            }
        });
    }

    if headless {
        println!("{:?}", handle.await);
    } else {
        run_tui(node.clone(), node_port, node_path.to_string()).await?;
    }

    Ok(())
}

use std::{sync::Arc};

use anyhow::anyhow;
use snap_coin::node::{message::{Command, Message}, node::Node, peer::Peer};
use tokio::sync::RwLock;

pub async fn sync_blockchain(
    peer: Arc<RwLock<Peer>>,
    node: Arc<RwLock<Node>>,
) -> Result<(), anyhow::Error> {
    Node::log("[SYNC] Starting initial block download".into());
    Node::log("[SYNC] Fetching block hashes".into());
    let local_height = node.read().await.blockchain.get_height();
    let remote_height = match Peer::request(
        peer.clone(),
        Message::new(Command::Ping {
            height: local_height,
        }),
    )
    .await?
    .command
    {
        Command::Pong { height } => height,
        _ => return Err(anyhow!("Could not fetch peer height to sync blockchain")),
    };

    let hashes = match Peer::request(
        peer.clone(),
        Message::new(Command::GetBlockHashes {
            start: local_height,
            end: remote_height,
        }),
    )
    .await?
    .command
    {
        Command::GetBlockHashesResponse { block_hashes } => block_hashes,
        _ => {
            return Err(anyhow!(
                "Could not fetch peer block hashes to sync blockchain"
            ));
        }
    };
    Node::log("[SYNC] Fetched block hashes".into());

    for hash in hashes {
        let block = match Peer::request(
            peer.clone(),
            Message::new(Command::GetBlock { block_hash: hash }),
        )
        .await?
        .command
        {
            Command::GetBlockResponse { block } => block,
            _ => {
                return Err(anyhow!("Could not fetch peer block {}", hash.dump_base36()));
            }
        };

        if let Some(block) = block {
            node.write().await.blockchain.add_block(block.clone())?;
        } else {
            return Err(anyhow!("Could not fetch peer block {}", hash.dump_base36()));
        }
    }

    Ok(())
}
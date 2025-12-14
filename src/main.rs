use std::{fs, sync::Arc, time::Duration, vec};

use anyhow::anyhow;
use tokio::{net::lookup_host, sync::RwLock, time::sleep};

use crossterm::event::{self, Event, KeyCode};

use snap_coin::{
    api::api_server::Server,
    build_block,
    economics::DEV_WALLET,
    node::{
        message::{Command, Message},
        node::Node,
        peer::Peer,
    },
};

use tracing_subscriber::prelude::*;

use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    widgets::{Borders, Paragraph},
};

use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};

async fn sync_blockchain(
    peer: Arc<RwLock<Peer>>,
    node: Arc<RwLock<Node>>,
) -> Result<(), anyhow::Error> {
    Node::log("[SYNC] Starting blockchain sync".into());
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

// ------------------------------------------------------
// NEW TUI
// ------------------------------------------------------

async fn run_tui(node: Arc<RwLock<Node>>, node_port: u16, node_path: String) -> anyhow::Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut log_scroll = 0u16;

    // Cached log refresh timer
    let mut last_log_read = std::time::Instant::now();
    let mut cached_log = String::new();

    loop {
        // --- READ NODE STATE SAFELY (NO ASYNC IN DRAW LOOP) ---
        let node_state = {
            let guard = node.read().await;

            // Blockchain
            let height = guard.blockchain.get_height();
            let last_block = guard
                .blockchain
                .get_block_hash_by_height(height.saturating_sub(1))
                .map(|b| b.dump_base36())
                .unwrap_or("<no block>".to_string());

            // Peer snapshot (NO CLONING PEER)
            let mut peer_snaps = Vec::new();
            for p in guard.peers.clone() {
                // clones Arc, safe
                let p = p.read().await;
                peer_snaps.push((p.address.clone(), p.is_client));
            }

            (height, last_block, peer_snaps)
        };

        let (height, last_block, peer_snaps) = node_state;

        // --- READ LOG (INFREQUENTLY, NON-BLOCKING) ---
        if last_log_read.elapsed() > Duration::from_millis(300) {
            cached_log = fs::read_to_string(format!("{}/info.log", node_path)).unwrap_or_default();
            last_log_read = std::time::Instant::now();
        }

        terminal.draw(|f| {
            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints(
                    [
                        Constraint::Length(3), // bar 1 (increased height for border)
                        Constraint::Length(3), // bar 2
                        Constraint::Min(1),    // log area
                    ]
                    .as_ref(),
                )
                .split(f.area());

            // TOP BAR
            let bar1 = Paragraph::new(format!(
                "PORT: {} | HEIGHT: {} | LAST_BLOCK: {}",
                node_port, height, last_block
            ))
            .block(
                ratatui::widgets::Block::default()
                    .title("NODE STATUS")
                    .borders(Borders::ALL),
            );
            f.render_widget(bar1, layout[0]);

            // PEERS BAR
            let mut peers_line = String::new();
            for (addr, is_client) in &peer_snaps {
                if *is_client {
                    peers_line.push_str(&format!("{}* ", addr));
                } else {
                    peers_line.push_str(&format!("{} ", addr));
                }
            }

            let bar2 = Paragraph::new(peers_line).block(
                ratatui::widgets::Block::default()
                    .title("PEERS")
                    .borders(Borders::ALL),
            );
            f.render_widget(bar2, layout[1]);

            // LOG AREA
            let log_widget = Paragraph::new(cached_log.as_str())
                .block(
                    ratatui::widgets::Block::default()
                        .title("LOGS")
                        .borders(Borders::ALL),
                )
                .scroll((log_scroll, 0));

            f.render_widget(log_widget, layout[2]);
        })?;

        // --- INPUT ---
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(k) = event::read()? {
                match k.code {
                    KeyCode::Char('q') => break,

                    KeyCode::Up => log_scroll = log_scroll.saturating_sub(1),
                    KeyCode::Down => log_scroll = log_scroll.saturating_add(1),

                    KeyCode::Char('c') => {
                        let _ = fs::write(format!("{}/info.log", node_path), "");
                        Node::log("Log cleared".into());
                        cached_log.clear();
                        log_scroll = 0;
                    }

                    _ => {}
                }
            }
        }
    }

    // --- CLEAN EXIT ---
    disable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, LeaveAlternateScreen)?;
    Ok(())
}

// ------------------------------------------------------
// MAIN
// ------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let args = std::env::args().collect::<Vec<_>>();
    let mut peers: Vec<String> = vec![];
    let mut node_path = "./node-testnet";
    let mut start_api = true;
    let mut api_port = 3003;
    let mut node_port = 8998;
    let mut create_genesis = false;

    // ------------------------------------------------------
    // ARGUMENTS
    // ------------------------------------------------------

    for arg in args.iter().enumerate() {
        if arg.1 == "--peers" && args.get(arg.0 + 1).is_some() {
            peers = args[arg.0 + 1].split(',').map(|s| s.to_string()).collect();
        }
        if arg.1 == "--node-path" && args.get(arg.0 + 1).is_some() {
            node_path = &args[arg.0 + 1];
        }
        if arg.1 == "--no-api" {
            start_api = false;
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
            {
                println!("Not using tracer!");
            }
        }
    }

    let mut resolved_peers = Vec::new();

    for seed in &peers {
        match lookup_host(seed).await {
            Ok(addrs) => {
                if let Some(addr) = addrs.into_iter().next() {
                    resolved_peers.push(addr);
                } else {
                    eprintln!("No addresses found for {}", seed);
                }
            }
            Err(e) => {
                eprintln!("Failed to resolve {}: {}", seed, e);
            }
        }
    }

    let node = Node::new(node_path, node_port);
    node.write().await.is_syncing = true;
    let _handle = Node::init(node.clone(), resolved_peers).await?;

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
    if !peers.is_empty() {
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

    // ------------------------------------------------------
    // NEW TUI CALL
    // ------------------------------------------------------

    run_tui(node.clone(), node_port, node_path.to_string()).await?;

    Ok(())
}

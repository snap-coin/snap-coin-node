use std::{
    fs,
    io::{Write, stdout},
    net::SocketAddr,
    str::FromStr,
    time::Duration,
    vec,
};

use tokio::time::sleep;

use crossterm::{
    ExecutableCommand, cursor,
    terminal::{self, ClearType, size},
};
use snap_coin::{api::api_server::Server, build_block, economics::DEV_WALLET, node::node::Node};
use std::io::Read;
use tracing_subscriber::prelude::*;

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let args = std::env::args().collect::<Vec<_>>();
    let mut peers: Vec<String> = vec![];
    let mut node_path = "./node-testnet";
    let mut start_api = true;
    let mut api_port = 3003;
    let mut node_port = 8998;
    let mut create_genesis = false;
    for arg in args.iter().enumerate() {
        if arg.1 == "--peers" && args.get(arg.0 + 1).is_some() {
            peers = args[arg.0 + 1].split(",").map(|s| s.to_string()).collect();
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

    let node = Node::new(node_path, node_port);
    Node::init(
        node.clone(),
        peers
            .iter()
            .map(|seed| SocketAddr::from_str(&seed).unwrap())
            .collect(),
    )
    .await?;

    // Start API
    if start_api {
        sleep(Duration::from_secs(1)).await; // 
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

    // Start logger

    let mut stdout = stdout();

    let _ = stdout.execute(terminal::EnterAlternateScreen);
    let _ = stdout.execute(terminal::Clear(ClearType::All));

    println!("Starting logger");

    loop {
        let _ = stdout.execute(cursor::MoveTo(0, 1));
        let _ = stdout.execute(terminal::Clear(ClearType::FromCursorDown));

        let mut log_file = fs::OpenOptions::new()
            .read(true)
            .open(format!("{}/info.log", node_path))
            .expect("Could not open log file!");

        let mut contents = String::new();
        log_file
            .read_to_string(&mut contents)
            .expect("Failed to read log file");

        println!("{}", contents);

        let _ = stdout.execute(cursor::MoveTo(0, size()?.1));

        {
            let blockchain = &node.read().await.blockchain;
            let blockchain_height = blockchain.get_height();
            let last_block =
                match blockchain.get_block_hash_by_height(blockchain_height.saturating_sub(1)) {
                    Some(block) => block.dump_base36(),
                    None => "<no block>".to_owned(),
                };
            println!(
                "[ HEIGHT: {} LAST_BLOCK: {} ]",
                blockchain_height, last_block
            );
        }

        let _ = stdout.flush();
        sleep(Duration::from_secs_f64(0.3)).await;
    }
}

# Snap Coin Node
## Installation
To install Snap Coin Node, run:
```bash
cargo install snap-coin-node
```
Make sure you have cargo, and rust installed.

## General Information
By default the node is hosted on port `8998`, and the Snap Coin API server is hosted on `3003`. This can be changed with command line arguments, mentioned below.

## Usage
```bash
snap-coin-node <args>
```
Available arguments:

1. `--peers [peers]`
Specified seed nodes, from which this node wil find other nodes to connect too and strengthen its network.

2. `--no-api`
Disable Snap Coin API.

3. `--headless`
Disable terminal ui, doesn't even print to TTY. Only to info.log.

4. `--no-ibd`
Disable initial block download.

5. `--node-path [path]`
Specify path where the node will store its state.

6. `--create-genesis`
Create a new genesis block and add it to the blockchain.

7. `--api-port [port]`
Specify port on which the api is to be hosted.

8. `--node-port [port]`
Specify port on which the node is to be hosted.

9. `--debug`
Enable async debugging. You can access this by using the `tokio-console` command (you might need to install it via `cargo install tokio-console`)
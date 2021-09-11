# turtlescan
Explore from the safety of your shell

Installation: `cargo install turtlescan`

**tui**

Starts a little tui which connects to your JSON-RPC server and allows you to browse the blockchain.

Usage (after running `cargo install turtlescan`): `turtlescan --provider.url=ws://localhost:8545 tui`

Usage (from a source checkout): `cargo run -- --provider.url=ws://localhost:8545 tui`

![Selection_385](https://user-images.githubusercontent.com/466333/132937560-d51657db-8e42-433c-82b1-4f269f2629e1.png)

**tailBlocks**

This subcommand emits blocks as soon as your provider discovers that they exist:

$ cargo run -- --provider.url=ws://localhost:8545 tailBlocks

![image](https://user-images.githubusercontent.com/466333/131605193-4d341af8-2817-4b1d-b029-a2138a5e2649.png)


# turtlescan
Explore from the safety of your shell

Starts a little tui which connects to your JSON-RPC server and allows you to browse the blockchain.

![Selection_390](https://user-images.githubusercontent.com/466333/134072340-0fc69a4a-18b0-447c-960f-6e171fbfb61c.png)

Installation: `cargo install turtlescan` (this exact command is also how you update turtlescan)

Usage (after running `cargo install turtlescan`): `turtlescan --provider.url=ws://localhost:8545 tui`

Usage (from a source checkout): `cargo run -- --provider.url=ws://localhost:8545 tui`

Some notes:
1. Currently it requires that your JSON-RPC server support websocket connections. If it is not possible for you to connect to your server via websocket please leave a comment [here](https://github.com/lithp/turtlescan/issues/25).
    - Turbogeth: when running `rpcdaemon` pass the `--ws` flag
    - Geth: when running `geth` pass the `--ws` flag
2. It is technically possible to connect to Infura but I do not recommend it. Turtlescan does not rate-limit itself and this might get you banned. If you would like to use turtlescan with Infura please leave a comment [here](https://github.com/lithp/turtlescan/issues/24).
3. If you do not want to pass in the `--provider` flag every time you run turtlescan you can set the environment variable `TURTLE_PROVIDER`.

    ```bash
    $ export TURTLE_PROVIDER=ws://localhost:8545
    $ turtlescan tui
    ```


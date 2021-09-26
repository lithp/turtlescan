extern crate simple_error;

use std::error::Error;
use turtlescan::entrypoint;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    entrypoint::main().await
}

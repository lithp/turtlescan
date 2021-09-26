use ethers_core::types::{Block as EthBlock, Transaction, TransactionReceipt, TxHash};
use ethers_providers::StreamExt;
use ethers_providers::{JsonRpcClient, Middleware, Provider, Ws};
use flexbuffers;
use log::debug;
use serde::ser::Serialize;
use simple_error::SimpleError;
use sled;
use std::collections::HashMap;
use std::error::Error;
use std::path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;
use tokio::sync::mpsc as tokio_mpsc;

#[derive(Clone, Debug)]
pub enum RequestStatus<T> {
    Waiting(),
    Started(),
    Completed(T),
    // Failed(io::Error),
}

#[derive(Clone)]
enum Request {
    Block(u64),
    BlockWithTxns(u64),
    BlockReceipts(u64),
}

#[derive(Debug)]
pub enum Response {
    HighestBlockNumber(u64),
    NewBlock(EthBlock<TxHash>),
    BlockNoTx(u64, RequestStatus<EthBlock<TxHash>>),
    BlockTx(u64, RequestStatus<EthBlock<Transaction>>),
    BlockReceipt(u64, RequestStatus<Vec<TransactionReceipt>>),
}

impl Request {
    fn start(&self) -> Response {
        use Request::*;
        match self {
            Block(blocknum) => Response::BlockNoTx(*blocknum, RequestStatus::Started()),
            BlockWithTxns(blocknum) => Response::BlockTx(*blocknum, RequestStatus::Started()),
            BlockReceipts(blocknum) => Response::BlockReceipt(*blocknum, RequestStatus::Started()),
        }
    }

    async fn fetch<T: JsonRpcClient>(
        &self,
        provider: &Provider<T>,
    ) -> Result<Response, Box<dyn Error>> {
        use Request::*;
        match self {
            Block(blocknum) => {
                let network_result = provider.get_block(*blocknum).await;
                let block_opt = network_result?;
                let block = block_opt.ok_or(SimpleError::new(format!(
                    "no such block blocknum={}",
                    blocknum
                )))?;

                return Ok(Response::BlockNoTx(
                    *blocknum,
                    RequestStatus::Completed(block),
                ));
            }
            BlockWithTxns(blocknum) => {
                let network_result = provider.get_block_with_txs(*blocknum).await;
                let block_opt = network_result?;
                let block = block_opt.ok_or(SimpleError::new(format!(
                    "no such block blocknum={}",
                    blocknum
                )))?;

                return Ok(Response::BlockTx(
                    *blocknum,
                    RequestStatus::Completed(block),
                ));
            }
            BlockReceipts(blocknum) => {
                let network_result = provider.get_block_receipts(*blocknum).await;
                let receipts: Vec<TransactionReceipt> = network_result?;

                // annoyingly, there's no way to know whether 0 receipts is an error or
                // not unless we remember how many txns we expect to receive
                // TODO(2021-09-14): add that memory to the fetch!

                return Ok(Response::BlockReceipt(
                    *blocknum,
                    RequestStatus::Completed(receipts),
                ));
            }
        }
    }
}

pub trait Data {
    fn apply_progress(&mut self, progress: Response);
    fn bump_highest_block(&self, blocnum: u64);
    fn get_highest_block(&self) -> Option<u64>;
    fn get_block(&mut self, blocknum: u64) -> RequestStatus<EthBlock<TxHash>>;
    fn get_block_with_transactions(
        &mut self,
        blocknum: u64,
    ) -> RequestStatus<EthBlock<Transaction>>;
    fn get_block_receipts(&mut self, blocknum: u64) -> RequestStatus<Vec<TransactionReceipt>>;
    fn invalidate_block(&mut self, blocknum: u64);
}

pub struct Database {
    sled_tx: crossbeam::channel::Sender<Request>,
    pub results_rx: crossbeam::channel::Receiver<Response>,

    // TODO(2021-09-10) currently these leak memory, use an lru cache or something
    blocks_to_txns: HashMap<u64, RequestStatus<EthBlock<Transaction>>>,
    block_receipts: HashMap<u64, RequestStatus<Vec<TransactionReceipt>>>,
    highest_block: Arc<Mutex<Option<u64>>>,

    blocknum_to_block: HashMap<u64, RequestStatus<EthBlock<TxHash>>>,
}

impl Database {
    /// provider: Ws is required because we watch for new blocks
    /// tx: sends messages back to the UI thread (things like Refresh and NewBlock)
    pub fn start(provider: Provider<Ws>, cache_path: path::PathBuf) -> Database {
        //TODO: return Result
        let cache = sled::open(cache_path).unwrap();

        let (db_results_tx, db_results_rx) = crossbeam::channel::unbounded();
        let (network_requests_tx, mut network_requests_rx) = tokio_mpsc::unbounded_channel();
        let (sled_tx, sled_rx) = crossbeam::channel::unbounded();
        let (network_result_tx, network_result_rx) = crossbeam::channel::unbounded();

        let highest_block = Arc::new(Mutex::new(None));
        let highest_block_send = highest_block.clone();

        /*
         * threads:
         * - all methods on Database are called from the UI thread. In order to give the
         *   UI quick access to all our data this is where the primary HashMap's are kept
         * - run_sled() this thread meters access to the on-disk chaindata cache. It
         *   attempts to satisfy requests
         *   - it also listens to network results
         * - run_networking() this thread runs a couple tasks and talks to the JSON-RPC
         *   server.
         *
         * tx -> send messages back to the UI (e.g. telling it to refresh)
         *
         * sled_(tx,rx) -> send requests to sled
         * network_requests_(tx,rx) -> send requests to networking
         * network_results_(tx,rx) -> networking notifies when requests finish
         */

        let _handle: thread::JoinHandle<()> = thread::spawn(move || {
            run_sled(
                cache,
                sled_rx,
                network_requests_tx,
                network_result_rx,
                db_results_tx,
            );
        });

        // no real need to hold onto this handle, the thread will be killed when this main
        // thread exits.
        let _handle: thread::JoinHandle<()> = thread::spawn(move || {
            run_networking(
                provider,
                highest_block_send,
                &mut network_requests_rx,
                network_result_tx,
            );
        });

        Database {
            sled_tx: sled_tx,
            results_rx: db_results_rx,

            highest_block: highest_block,
            blocks_to_txns: HashMap::new(),
            block_receipts: HashMap::new(),
            blocknum_to_block: HashMap::new(),
        }
    }
}

impl Data for Database {
    fn apply_progress(&mut self, progress: Response) {
        use Response::*;
        match progress {
            HighestBlockNumber(_) => {
                // TODO let's come back to this one
            }
            NewBlock(_) => {
                // TODO: at very least this should update the highest block number,
                //       if appropriate
            }
            BlockNoTx(blocknum, status) => {
                self.blocknum_to_block.insert(blocknum, status);
            }
            BlockTx(blocknum, status) => {
                self.blocks_to_txns.insert(blocknum, status);
            }
            BlockReceipt(blocknum, status) => {
                self.block_receipts.insert(blocknum, status);
            }
        }
    }

    // TODO: return result
    fn bump_highest_block(&self, blocknum: u64) {
        let mut highest_block_opt = self.highest_block.lock().unwrap();

        if let Some(highest_block_number) = *highest_block_opt {
            if blocknum < highest_block_number {
                return;
            }
        }

        *highest_block_opt = Some(blocknum);
    }

    fn get_highest_block(&self) -> Option<u64> {
        let highest_block_opt = self.highest_block.lock().unwrap();
        highest_block_opt.clone()
    }

    fn get_block(&mut self, blocknum: u64) -> RequestStatus<EthBlock<TxHash>> {
        //TODO(2021-09-16) some version of entry().or_insert_with() should be able to
        //                 replace this but I haven't been able to convince the borrow
        //                 checker
        let result = match self.blocknum_to_block.get(&blocknum) {
            None => {
                self.sled_tx.send(Request::Block(blocknum)).unwrap();
                debug!("fired new request for block {}", blocknum);
                self.blocknum_to_block
                    .insert(blocknum, RequestStatus::Waiting());
                self.blocknum_to_block.get(&blocknum).unwrap()
            }
            Some(status) => status,
        };

        result.clone()
    }

    fn get_block_with_transactions(
        &mut self,
        blocknum: u64,
    ) -> RequestStatus<EthBlock<Transaction>> {
        let result = match self.blocks_to_txns.get(&blocknum) {
            None => {
                self.sled_tx.send(Request::BlockWithTxns(blocknum)).unwrap();
                debug!("fired new request for txns for block {}", blocknum);
                self.blocks_to_txns
                    .insert(blocknum, RequestStatus::Waiting());
                self.blocks_to_txns.get(&blocknum).unwrap()
            }
            Some(status) => status,
        };

        // TODO I don't think this clone is necessary
        result.clone()
    }

    // TODO: this is a lot of copying, is that really okay?
    fn get_block_receipts(&mut self, blocknum: u64) -> RequestStatus<Vec<TransactionReceipt>> {
        let result = match self.block_receipts.get(&blocknum) {
            None => {
                self.sled_tx.send(Request::BlockReceipts(blocknum)).unwrap();

                debug!("fired new request for txns for block {}", blocknum);
                self.block_receipts
                    .insert(blocknum, RequestStatus::Waiting());
                self.block_receipts.get(&blocknum).unwrap()
            }
            Some(status) => status,
        };

        result.clone()
    }

    /// this block is no longer valid, likely because a re-org happened, and should be
    /// re-fetched if we ever ask for it again
    fn invalidate_block(&mut self, blocknum: u64) {
        self.blocknum_to_block.remove(&blocknum);
        self.block_receipts.remove(&blocknum);
        self.blocks_to_txns.remove(&blocknum);
    }
}

fn run_sled(
    sled: sled::Db,
    sled_rx: crossbeam::channel::Receiver<Request>,
    network_requests_tx: tokio_mpsc::UnboundedSender<Request>,
    network_results_rx: crossbeam::channel::Receiver<Response>,
    results_tx: crossbeam::channel::Sender<Response>,
) {
    // TODO: don't duplicate things, store the blocks and the txns separately and convert
    //       between them as necessary
    let blocks_tree = sled.open_tree(b"blocks").unwrap();
    let block_txns_tree = sled.open_tree(b"block_with_txns").unwrap();
    let block_receipts_tree = sled.open_tree(b"receipts").unwrap();

    loop {
        crossbeam::channel::select! {
            recv(sled_rx) -> msg_result => {
                // commands
                let msg = msg_result.unwrap();

                use Request::*;
                match msg {
                    Block(blocknum) => {
                        let start = Instant::now();
                        let key: [u8; 8] = blocknum.to_be_bytes();
                        let opt = blocks_tree.get(key).unwrap();
                        match opt {
                            None => {},
                            Some(ivec) => {
                                let block: EthBlock<TxHash> = flexbuffers::from_slice(&ivec).unwrap();
                                results_tx.send(Response::BlockNoTx(blocknum, RequestStatus::Completed(block))).unwrap();
                                let duration = start.elapsed();
                                debug!(" fetched block from db elapsed={:?}", duration);
                                continue
                            }
                        }
                    },
                    BlockWithTxns(blocknum) => {
                        let key: [u8; 8] = blocknum.to_be_bytes();
                        let opt = block_txns_tree.get(key).unwrap();
                        match opt {
                            None => {},
                            Some(ivec) => {
                                let block: EthBlock<Transaction> = flexbuffers::from_slice(&ivec).unwrap();
                                results_tx.send(Response::BlockTx(blocknum, RequestStatus::Completed(block))).unwrap();
                                continue
                            }
                        }
                    }
                    BlockReceipts(blocknum) => {
                        let key: [u8; 8] = blocknum.to_be_bytes();
                        let opt = block_receipts_tree.get(key).unwrap();
                        match opt {
                            None => {},
                            Some(ivec) => {
                                let vec: Vec<TransactionReceipt> = flexbuffers::from_slice(&ivec).unwrap();
                                results_tx.send(Response::BlockReceipt(blocknum, RequestStatus::Completed(vec))).unwrap();
                                continue
                            }
                        }
                    },
                }

                if let Err(_) = network_requests_tx.send(msg) {
                    // Can't use expect() or unwrap() b/c SendError does not implement Debug
                    panic!("remote end closed?");
                }
            }
            recv(network_results_rx) -> msg_result => {
                // results
                let msg = msg_result.unwrap();
                use Response::*;
                match msg {
                    BlockNoTx(blocknum, ref block_status) => {
                        use RequestStatus::*;
                        match block_status {
                            Waiting() | Started() => {},
                            Completed(block) => {
                                // save the block to the database
                                let mut s = flexbuffers::FlexbufferSerializer::new();
                                block.serialize(&mut s).unwrap();

                                let key: [u8; 8] = blocknum.to_be_bytes();

                                blocks_tree.insert(key, s.view()).unwrap();
                                debug!("wrote block to db blocknum={}", blocknum);
                            }
                        };
                    },
                    BlockTx(blocknum, ref block_status) => {
                        use RequestStatus::*;
                        match block_status {
                            Waiting() | Started() => {},
                            Completed(block) => {
                                // save the block to the database
                                let mut s = flexbuffers::FlexbufferSerializer::new();
                                block.serialize(&mut s).unwrap();

                                let key: [u8; 8] = blocknum.to_be_bytes();

                                block_txns_tree.insert(key, s.view()).unwrap();
                                debug!("wrote block to db blocknum={}", blocknum);
                            }
                        };
                    }
                    BlockReceipt(blocknum, ref status) => {
                        use RequestStatus::*;
                        match status {
                            Waiting() | Started() => {},
                            Completed(vec) => {
                                // save the block to the database
                                let mut s = flexbuffers::FlexbufferSerializer::new();
                                vec.serialize(&mut s).unwrap();

                                let key: [u8; 8] = blocknum.to_be_bytes();

                                block_receipts_tree.insert(key, s.view()).unwrap();
                                debug!("wrote block receipts to db blocknum={}", blocknum);
                            }
                        };
                    }
                    HighestBlockNumber(_) => {}
                    NewBlock(_) => {}
                }

                results_tx.send(msg).unwrap();
            }
        }
    }
}

#[tokio::main(worker_threads = 1)]
async fn run_networking(
    provider: Provider<Ws>,
    highest_block: Arc<Mutex<Option<u64>>>,
    request_rx: &mut tokio_mpsc::UnboundedReceiver<Request>,
    result_tx: crossbeam::channel::Sender<Response>,
) {
    debug!("started networking thread");
    let block_number_opt = provider.get_block_number().await;
    match block_number_opt {
        Err(error) => panic!("could not fetch highest block number {}", error),
        Ok(number) => {
            let mut block_number = highest_block.lock().unwrap();
            *block_number = Some(number.low_u64());
            result_tx
                .send(Response::HighestBlockNumber(number.low_u64()))
                .unwrap();
        }
    }
    debug!("updated block number");

    let loop_tx = result_tx.clone();
    let loop_fut = loop_on_network_commands(&provider, loop_tx, request_rx);

    let watch_fut = watch_new_blocks(&provider, result_tx);

    tokio::join!(loop_fut, watch_fut); // neither will exit so this should block forever
}

async fn loop_on_network_commands<T: JsonRpcClient>(
    provider: &Provider<T>,
    result_tx: crossbeam::channel::Sender<Response>,
    network_rx: &mut tokio_mpsc::UnboundedReceiver<Request>,
) {
    loop {
        let request = network_rx.recv().await.unwrap(); // blocks until we have more input

        let progress = request.start();
        result_tx.send(progress).unwrap();

        let result = request.fetch(&provider).await.unwrap();
        result_tx.send(result).unwrap();
    }
}

async fn watch_new_blocks(
    provider: &Provider<Ws>,
    result_tx: crossbeam::channel::Sender<Response>,
) {
    let mut stream = provider.subscribe_blocks().await.unwrap();
    while let Some(block) = stream.next().await {
        debug!("new block {}", block.number.unwrap());
        result_tx.send(Response::NewBlock(block)).unwrap();
    }
}

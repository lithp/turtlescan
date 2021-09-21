use ethers_core::types::{Block as EthBlock, Transaction, TransactionReceipt, TxHash};
use ethers_providers::StreamExt;
use ethers_providers::{JsonRpcClient, Middleware, Provider, Ws};
use log::{debug, warn};
use simple_error::SimpleError;
use std::collections::HashMap;
use std::default::Default;
use std::io;
use std::sync::mpsc;
use std::sync::LockResult;
use std::sync::MutexGuard;
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::sync::mpsc as tokio_mpsc;

// TODO: this does not belong in this module but putting it here allows us to break a
// circular import
pub enum UIMessage {
    // the user has given us some input over stdin
    Key(termion::event::Key),

    // something in the background has updated state and wants the UI to rerender
    Refresh(),

    // networking has noticed a new block and wants the UI to show it
    // TODO(2021-09-09) we really only need the block number
    NewBlock(EthBlock<TxHash>),
}

#[derive(Clone)]
pub enum RequestStatus<T> {
    Waiting(),
    Started(),
    Completed(T),
    // Failed(io::Error),
}

#[derive(Clone)]
pub struct ArcStatus<T>(Arc<Mutex<RequestStatus<T>>>);

impl<T> Default for ArcStatus<T> {
    fn default() -> Self {
        ArcStatus(Arc::new(Mutex::new(RequestStatus::Waiting())))
    }
}

impl<T> ArcStatus<T> {
    // prevents callers from needing to care about .0
    // if I end up wanting to forward more of these Deref might be the better option
    fn lock(&self) -> LockResult<MutexGuard<'_, RequestStatus<T>>> {
        return self.0.lock();
    }

    /// tell the UI that the request is being handled
    /// careful, blocks until it can take out a lock!
    fn start_if_waiting(&self) -> Result<(), SimpleError> {
        let mut fetch = self.lock().unwrap();

        if let RequestStatus::Waiting() = *fetch {
            *fetch = RequestStatus::Started();
            Ok(())
        } else {
            Err(SimpleError::new("arc was in the wrong state"))
        }
    }

    /// careful, blocks until it can take out a lock
    /// overwrites anything which was previously in here
    fn complete(&self, result: T) {
        let mut fetch = self.lock().unwrap();
        *fetch = RequestStatus::Completed(result);
    }
}

type ArcFetchBlock = ArcStatus<EthBlock<TxHash>>;
type ArcFetchTxns = ArcStatus<EthBlock<Transaction>>;
type ArcFetchReceipts = ArcStatus<Vec<TransactionReceipt>>;

#[derive(Clone)]
struct BlockRequest(u64, ArcFetchBlock);

#[derive(Clone)]
struct BlockTxnsRequest(u64, ArcFetchTxns);

#[derive(Clone)]
struct BlockReceiptsRequest(u64, ArcFetchReceipts);

impl BlockRequest {
    fn new(blocknum: u64) -> BlockRequest {
        BlockRequest(blocknum, ArcStatus::default())
    }
}

impl BlockTxnsRequest {
    fn new(blocknum: u64) -> BlockTxnsRequest {
        BlockTxnsRequest(blocknum, ArcStatus::default())
    }
}

impl BlockReceiptsRequest {
    fn new(blocknum: u64) -> BlockReceiptsRequest {
        BlockReceiptsRequest(blocknum, ArcStatus::default())
    }
}

#[derive(Clone)]
enum NetworkRequest {
    // wrapping b/c it is not possible to use an enum variant as a type...
    Block(BlockRequest),
    BlockWithTxns(BlockTxnsRequest),
    BlockReceipts(BlockReceiptsRequest),
}

pub struct Database {
    network_tx: tokio_mpsc::UnboundedSender<NetworkRequest>, // tell network what to fetch

    // TODO(2021-09-10) currently these leak memory, use an lru cache or something
    pub blocks_to_txns: HashMap<u64, ArcFetchTxns>,
    pub block_receipts: HashMap<u64, ArcFetchReceipts>,
    highest_block: Arc<Mutex<Option<u64>>>,

    pub blocknum_to_block: HashMap<u64, ArcFetchBlock>,
}

impl Database {
    pub fn start(
        provider: Provider<Ws>, // Ws is required because we watch for new blocks
        tx: mpsc::Sender<Result<UIMessage, io::Error>>,
    ) -> Database {
        let (network_tx, mut network_rx) = tokio_mpsc::unbounded_channel();

        let highest_block = Arc::new(Mutex::new(None));
        let highest_block_send = highest_block.clone();

        // no real need to hold onto this handle, the thread will be killed when this main
        // thread exits.
        let _handle: thread::JoinHandle<()> = thread::spawn(move || {
            run_networking(provider, highest_block_send, tx, &mut network_rx);
        });

        Database {
            network_tx: network_tx,

            highest_block: highest_block,
            blocks_to_txns: HashMap::new(),
            block_receipts: HashMap::new(),
            blocknum_to_block: HashMap::new(),
        }
    }

    fn fetch(&self, request: NetworkRequest) {
        let cloned = request.clone();
        if let Err(_) = self.network_tx.send(cloned) {
            // TODO(2021-09-09): fetch() should return a Result
            // Can't use expect() or unwrap() b/c SendError does not implement Debug
            panic!("remote end closed?");
        }
    }

    // TODO a macro is probably not the right solution here but this seems like a good
    //      spot to practice generating boilerplate with a macro

    fn fetch_block(&self, block_number: u64) -> BlockRequest {
        let new_request = BlockRequest::new(block_number);
        let result = new_request.clone();

        self.fetch(NetworkRequest::Block(new_request));
        result
    }

    fn fetch_block_with_txns(&self, block_number: u64) -> BlockTxnsRequest {
        let new_request = BlockTxnsRequest::new(block_number);
        let result = new_request.clone();

        self.fetch(NetworkRequest::BlockWithTxns(new_request));
        result
    }

    fn fetch_block_receipts(&self, block_number: u64) -> BlockReceiptsRequest {
        let new_request = BlockReceiptsRequest::new(block_number);
        let result = new_request.clone();

        self.fetch(NetworkRequest::BlockReceipts(new_request));
        result
    }

    // TODO: return result
    pub fn bump_highest_block(&self, blocknum: u64) {
        let mut highest_block_opt = self.highest_block.lock().unwrap();

        if let Some(highest_block_number) = *highest_block_opt {
            if blocknum < highest_block_number {
                return;
            }
        }

        *highest_block_opt = Some(blocknum);
    }

    pub fn get_highest_block(&self) -> Option<u64> {
        let highest_block_opt = self.highest_block.lock().unwrap();
        highest_block_opt.clone()
    }

    pub fn get_block(&mut self, blocknum: u64) -> RequestStatus<EthBlock<TxHash>> {
        //TODO(2021-09-16) some version of entry().or_insert_with() should be able to
        //                 replace this but I haven't been able to convince the borrow
        //                 checker
        let arcfetch = match self.blocknum_to_block.get(&blocknum) {
            None => {
                let new_fetch = self.fetch_block(blocknum);

                debug!("fired new request for block {}", blocknum);
                self.blocknum_to_block.insert(blocknum, new_fetch.1);
                self.blocknum_to_block.get(&blocknum).unwrap()
            }
            Some(arcfetch) => arcfetch,
        };

        let blockfetch = arcfetch.lock().unwrap();
        blockfetch.clone()
    }

    pub fn get_block_with_transactions(
        &mut self,
        blocknum: u64,
    ) -> RequestStatus<EthBlock<Transaction>> {
        let arcfetch = match self.blocks_to_txns.get(&blocknum) {
            None => {
                let new_fetch = self.fetch_block_with_txns(blocknum);

                debug!("fired new request for txns for block {}", blocknum);
                self.blocks_to_txns.insert(blocknum, new_fetch.1);

                self.blocks_to_txns.get(&blocknum).unwrap()
            }
            Some(arcfetch) => arcfetch,
        };

        let blockfetch = arcfetch.lock().unwrap();
        blockfetch.clone()
    }

    // TODO: this is a lot of copying, is that really okay?
    pub fn get_block_receipts(&mut self, blocknum: u64) -> RequestStatus<Vec<TransactionReceipt>> {
        let arcfetch = match self.block_receipts.get(&blocknum) {
            None => {
                let new_fetch = self.fetch_block_receipts(blocknum);

                debug!("fired new request for txns for block {}", blocknum);
                self.block_receipts.insert(blocknum, new_fetch.1);

                self.block_receipts.get(&blocknum).unwrap()
            }
            Some(arcfetch) => arcfetch,
        };

        let fetch = arcfetch.lock().unwrap();
        fetch.clone()
    }

    /// this block is no longer valid, likely because a re-org happened, and should be
    /// re-fetched if we ever ask for it again
    pub fn invalidate_block(&mut self, blocknum: u64) {
        self.blocknum_to_block.remove(&blocknum);
        self.block_receipts.remove(&blocknum);
        self.blocks_to_txns.remove(&blocknum);
    }
}

#[tokio::main(worker_threads = 1)]
async fn run_networking(
    /*
     * Ws is required because we watch for new blocks
     * TODO(2021-09-14) document this limitation somewhere visible
     */
    provider: Provider<Ws>,
    highest_block: Arc<Mutex<Option<u64>>>,
    tx: mpsc::Sender<Result<UIMessage, io::Error>>,
    network_rx: &mut tokio_mpsc::UnboundedReceiver<NetworkRequest>,
) {
    debug!("started networking thread");
    let block_number_opt = provider.get_block_number().await;
    match block_number_opt {
        Err(error) => debug!("{:}", error),
        Ok(number) => {
            let mut block_number = highest_block.lock().unwrap();
            *block_number = Some(number.low_u64());
        }
    }
    tx.send(Ok(UIMessage::Refresh())).unwrap();
    debug!("updated block number");

    let loop_tx = tx.clone();
    let loop_fut = loop_on_network_commands(&provider, loop_tx, network_rx);

    let watch_fut = watch_new_blocks(&provider, tx);

    tokio::join!(loop_fut, watch_fut); // neither will exit so this should block forever
}

async fn loop_on_network_commands<T: JsonRpcClient>(
    provider: &Provider<T>,
    tx: mpsc::Sender<Result<UIMessage, io::Error>>,
    network_rx: &mut tokio_mpsc::UnboundedReceiver<NetworkRequest>,
) {
    loop {
        let request = network_rx.recv().await.unwrap(); // blocks until we have more input

        match request {
            NetworkRequest::Block(BlockRequest(block_number, arc_fetch)) => {
                if let Err(err) = arc_fetch.start_if_waiting() {
                    warn!("arcfetch error: {}", err);
                    continue;
                }
                tx.send(Ok(UIMessage::Refresh())).unwrap();

                let complete_block = provider.get_block(block_number).await.unwrap().unwrap();

                arc_fetch.complete(complete_block);
                tx.send(Ok(UIMessage::Refresh())).unwrap();
            }
            NetworkRequest::BlockWithTxns(BlockTxnsRequest(block_number, arc_fetch)) => {
                if let Err(err) = arc_fetch.start_if_waiting() {
                    warn!("arcfetch error: {}", err);
                    continue;
                }
                tx.send(Ok(UIMessage::Refresh())).unwrap();

                // the first unwrap is b/c the network request might have failed
                // the second unwrap is b/c the requested block number might not exist
                let complete_block = provider
                    .get_block_with_txs(block_number)
                    .await
                    .unwrap()
                    .unwrap();

                arc_fetch.complete(complete_block);
                tx.send(Ok(UIMessage::Refresh())).unwrap();
            }
            NetworkRequest::BlockReceipts(BlockReceiptsRequest(blocknum, arc_fetch)) => {
                if let Err(err) = arc_fetch.start_if_waiting() {
                    warn!("arcfetch error: {}", err);
                    continue;
                }
                tx.send(Ok(UIMessage::Refresh())).unwrap();

                let receipts = provider.get_block_receipts(blocknum).await.unwrap();

                // annoyingly, there's no way to know whether 0 receipts is an error or
                // not unless we remember how many txns we expect to receive
                // TODO(2021-09-14): add that memory to the fetch!

                arc_fetch.complete(receipts);
                tx.send(Ok(UIMessage::Refresh())).unwrap();
            }
        }
    }
}

async fn watch_new_blocks(provider: &Provider<Ws>, tx: mpsc::Sender<Result<UIMessage, io::Error>>) {
    let mut stream = provider.subscribe_blocks().await.unwrap();
    while let Some(block) = stream.next().await {
        debug!("new block {}", block.number.unwrap());
        tx.send(Ok(UIMessage::NewBlock(block))).unwrap();
    }
}

use bench::{create_memo_tx, create_rng, BenchmarkTransactionParams};
use solana_sdk::{
    commitment_config::CommitmentConfig, signature::Keypair,
    transaction::VersionedTransaction,
};

use log::{debug, info, trace};

use std::time::Duration;
use tokio::time::Instant;

use crate::setup::setup_tx_service;

mod setup;

const SAMPLE_SIZE: usize = 10000;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
/// TC 4
///- send txs on LiteRPC broadcast channel and consume them using the Solana quic-streamer
/// - see quic_proxy_tpu_integrationtest.rs (note: not only about proxy)
/// - run cargo test (maybe need to use release build)
/// - Goal: measure performance of LiteRPC internal channel/thread structure and the TPU_service performance
pub async fn txn_broadcast() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    info!("START BENCHMARK: txn_broadcast");

    debug!("spawning tx_service");
    let (transaction_service, data_cache, _tx_jh) = setup_tx_service().await?;
    debug!("tx_service spawned successfully");

    let mut rng = create_rng(None);
    let payer = Keypair::new();
    let params = BenchmarkTransactionParams {
        tx_size: bench::tx_size::TxSize::Small,
        cu_price_micro_lamports: 1,
    };

    let mut i = 0;

    let mut times: Vec<Duration> = vec![];

    // TODO: save stats
    // TODO: txn sink?
    while i < SAMPLE_SIZE {
        let blockhash = data_cache.block_information_store.get_latest_blockhash(CommitmentConfig::confirmed()).await;
        let tx = create_memo_tx(&payer, blockhash, &mut rng, &params);
        let serialized = bincode::serialize::<VersionedTransaction>(&tx)
        .expect("Could not serialize VersionedTransaction");

        info!("Sending txn: {:?} {:?}", tx.signatures[0], i);
        let send_start = Instant::now();
        transaction_service
            .send_transaction(
                serialized,
                Some(1),
            )
            .await?;
        let send_time = send_start.elapsed();
        debug!("sent in {:?}", send_time);
        times.push(send_time);
        i += 1;
    }


    times.sort();
    let median_time = times[times.len() / 2];
    let total_time: Duration = times.iter().sum();
    let max_time = times.iter().max().unwrap();
    let min_time = times.iter().min().unwrap();

    info!("avg send time: {:?}", total_time.div_f64(f64::from(SAMPLE_SIZE as u32) ));
    info!("max_time: {:?}", max_time);
    info!("min_time: {:?}", min_time);
    info!("median_time: {:?}", median_time);

    Ok(())
}

use std::env;
use std::pin::pin;
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;
use futures::{Stream, StreamExt};
use geyser_grpc_connector::experimental::mock_literpc_core::map_produced_block;
use geyser_grpc_connector::grpc_stream_utils::channelize_stream;
use geyser_grpc_connector::grpc_subscription_autoreconnect::{create_geyser_reconnecting_stream, GeyserFilter, GrpcConnectionTimeouts, GrpcSourceConfig};
use geyser_grpc_connector::grpcmultiplex_fastestwins::{create_multiplexed_stream, FromYellowstoneExtractor};
use log::{debug, info, trace};
use merge_streams::MergeStreams;
use solana_sdk::clock::Slot;
use solana_sdk::commitment_config::CommitmentConfig;
use tokio::spawn;
use tokio::sync::broadcast::error::SendError;
use tokio::sync::broadcast::Receiver;
use tokio::task::JoinHandle;
use yellowstone_grpc_proto::geyser::{CommitmentLevel, SubscribeUpdate};
use yellowstone_grpc_proto::geyser::subscribe_update::UpdateOneof;
use yellowstone_grpc_proto::prelude::SubscribeUpdateBlock;
// use solana_rpc_client::nonblocking::rpc_client::RpcClient;
// use solana_lite_rpc_cluster_endpoints::endpoint_stremers::EndpointStreaming;
use solana_lite_rpc_core::AnyhowJoinHandle;
use solana_lite_rpc_core::structures::produced_block::ProducedBlock;
use solana_lite_rpc_core::types::BlockStream;
use crate::grpc_subscription::{create_grpc_subscription, map_block_update};


struct BlockExtractor(CommitmentConfig);

impl FromYellowstoneExtractor for BlockExtractor {
    type Target = ProducedBlock;
    fn map_yellowstone_update(&self, update: SubscribeUpdate) -> Option<(Slot, Self::Target)> {
        match update.update_oneof {
            Some(UpdateOneof::Block(update_block_message)) => {
                let block = map_block_update(update_block_message, self.0);
                Some((block.slot, block))
            }
            _ => None,
        }
    }
}


pub fn create_grpc_multiplex_subscription(
    grpc_addr: String, grpc_x_token: Option<String>,
    grpc_addr2: Option<String>, grpc_x_token2: Option<String>)
    -> (Receiver<ProducedBlock>, AnyhowJoinHandle) {

    info!("Setup grpc multiplexed connection...");
    info!("- configure first connection to {} ({})", grpc_addr, grpc_x_token.is_some());
    if let Some(ref grpc_addr2) = grpc_addr2 {
        info!("- configure second connection to {} ({})", grpc_addr2, grpc_x_token2.is_some());
    } else {
        info!("- no second grpc connection configured");
    }

    let timeouts = GrpcConnectionTimeouts {
        connect_timeout: Duration::from_secs(5),
        request_timeout: Duration::from_secs(5),
        subscribe_timeout: Duration::from_secs(5),
    };

    let multiplex_stream_confirmed = {
        let grpc_addr = grpc_addr.clone();
        let grpc_addr2 = grpc_addr2.clone();
        let grpc_x_token2 = grpc_x_token2.clone();
        let commitment_config = CommitmentConfig::confirmed();
        let first_stream = create_geyser_reconnecting_stream(
            GrpcSourceConfig::new(
                grpc_addr.clone(), grpc_x_token.clone(), None,
                timeouts.clone()),
            GeyserFilter::blocks_and_txs(),
            commitment_config);

        let mut streams = vec![first_stream];

        if let Some(grpc_addr2) = grpc_addr2 {
            let second_stream = create_geyser_reconnecting_stream(
                GrpcSourceConfig::new(
                    grpc_addr2, grpc_x_token2, None,
                    timeouts.clone()),
                GeyserFilter::blocks_and_txs(),
                commitment_config);
            streams.push(second_stream);
        }

        let multiplex_stream = create_multiplexed_stream(
            streams,
            BlockExtractor(commitment_config),
        );

        multiplex_stream
    };

    let multiplex_stream_finalized = {
        let grpc_addr = grpc_addr.clone();
        let grpc_addr2 = grpc_addr2.clone();
        let grpc_x_token2 = grpc_x_token2.clone();
        let commitment_config = CommitmentConfig::finalized();
        let first_stream = create_geyser_reconnecting_stream(
            GrpcSourceConfig::new(
                grpc_addr, grpc_x_token, None,
                timeouts.clone()),
            GeyserFilter::blocks_and_txs(),
            commitment_config);

        let mut streams = vec![first_stream];

        if let Some(grpc_addr2) = grpc_addr2 {
            let second_stream = create_geyser_reconnecting_stream(
                GrpcSourceConfig::new(
                    grpc_addr2, grpc_x_token2, None,
                    timeouts.clone()),
                GeyserFilter::blocks_and_txs(),
                commitment_config);
            streams.push(second_stream);
        }

        let multiplex_stream = create_multiplexed_stream(
            streams,
            BlockExtractor(commitment_config),
        );

        multiplex_stream
    };

    let merged_stream_confirmed_finalize = (multiplex_stream_confirmed, multiplex_stream_finalized).merge();

    // let (tx, multiplexed_finalized_blocks) = tokio::sync::broadcast::channel::<ProducedBlock>(1000);

    let (multiplexed_finalized_blocks, jh_channelizer) = channelize_stream(merged_stream_confirmed_finalize);

    (multiplexed_finalized_blocks, jh_channelizer)
}

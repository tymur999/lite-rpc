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


pub fn create_grpc_multiplex_subscription() -> (Receiver<ProducedBlock>, AnyhowJoinHandle) {

    let grpc_addr_green = env::var("GRPC_ADDR").expect("need grpc url for green");
    let grpc_x_token_green = env::var("GRPC_X_TOKEN").ok();

    let grpc_addr_blue = env::var("GRPC_ADDR2").ok();
    let grpc_x_token_blue = env::var("GRPC_X_TOKEN2").ok();

    info!("Setup grpc multiplexed connection...");
    info!("- using green on {} ({})", grpc_addr_green, grpc_x_token_green.is_some());
    if let Some(ref grpc_addr_blue) = grpc_addr_blue {
        info!("- using blue on {} ({})", grpc_addr_blue, grpc_x_token_blue.is_some());
    } else {
        info!("- no blue grpc connection configured");
    }

    let timeouts = GrpcConnectionTimeouts {
        connect_timeout: Duration::from_secs(5),
        request_timeout: Duration::from_secs(5),
        subscribe_timeout: Duration::from_secs(5),
    };

    let multiplex_stream_confirmed = {
        let grpc_addr_green = grpc_addr_green.clone();
        let grpc_addr_blue = grpc_addr_blue.clone();
        let grpc_x_token_blue = grpc_x_token_blue.clone();
        let commitment_config = CommitmentConfig::confirmed();
        let green_stream = create_geyser_reconnecting_stream(
            GrpcSourceConfig::new(
                grpc_addr_green.clone(), grpc_x_token_green.clone(), None,
                timeouts.clone()),
            GeyserFilter::blocks_and_txs(),
            commitment_config);

        let mut streams = vec![green_stream];

        if let Some(grpc_addr_blue) = grpc_addr_blue {
            let blue_stream = create_geyser_reconnecting_stream(
                GrpcSourceConfig::new(
                    grpc_addr_blue, grpc_x_token_blue, None,
                    timeouts.clone()),
                GeyserFilter::blocks_and_txs(),
                commitment_config);
            streams.push(blue_stream);
        }

        let multiplex_stream = create_multiplexed_stream(
            streams,
            BlockExtractor(commitment_config),
        );

        multiplex_stream
    };

    let multiplex_stream_finalized = {
        let grpc_addr_green = grpc_addr_green.clone();
        let grpc_addr_blue = grpc_addr_blue.clone();
        let grpc_x_token_blue = grpc_x_token_blue.clone();
        let commitment_config = CommitmentConfig::finalized();
        let green_stream = create_geyser_reconnecting_stream(
            GrpcSourceConfig::new(
                grpc_addr_green, grpc_x_token_green, None,
                timeouts.clone()),
            GeyserFilter::blocks_and_txs(),
            commitment_config);

        let mut streams = vec![green_stream];

        if let Some(grpc_addr_blue) = grpc_addr_blue {
            let blue_stream = create_geyser_reconnecting_stream(
                GrpcSourceConfig::new(
                    grpc_addr_blue, grpc_x_token_blue, None,
                    timeouts.clone()),
                GeyserFilter::blocks_and_txs(),
                commitment_config);
            streams.push(blue_stream);
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

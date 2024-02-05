use async_trait::async_trait;
use dashmap::DashMap;
use itertools::Itertools;
use prometheus::{opts, register_int_gauge, IntGauge};
use serde::{Deserialize, Serialize};
use solana_address_lookup_table_program::state::AddressLookupTable;
use solana_lite_rpc_core::traits::address_lookup_table_interface::AddressLookupTableInterface;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{commitment_config::CommitmentConfig, pubkey::Pubkey};
use std::{sync::Arc, time::Duration};

lazy_static::lazy_static! {
    static ref LRPC_ALTS_IN_STORE: IntGauge =
       register_int_gauge!(opts!("literpc_alts_stored", "Alts stored in literpc")).unwrap();
}

#[derive(Clone)]
pub struct ALTStore {
    rpc_client: Arc<RpcClient>,
    pub map: Arc<DashMap<Pubkey, Vec<Pubkey>>>,
}

impl ALTStore {
    pub fn new(rpc_client: Arc<RpcClient>) -> Self {
        Self {
            rpc_client,
            map: Arc::new(DashMap::new()),
        }
    }

    pub async fn load_alts_list(&self, alts_list: &[Pubkey]) {
        log::trace!("Preloading {} ALTs", alts_list.len());

        let alts_list = alts_list
            .iter()
            .filter(|x| !self.map.contains_key(x))
            .cloned()
            .collect_vec();
        if alts_list.is_empty() {
            return;
        }

        for batches in alts_list.chunks(1000).map(|x| x.to_vec()) {
            let tasks = batches.chunks(100).map(|batch| {
                let batch = batch.to_vec();
                let rpc_client = self.rpc_client.clone();
                let this = self.clone();
                tokio::spawn(async move {
                    let data = rpc_client
                        .get_multiple_accounts_with_commitment(
                            &batch,
                            CommitmentConfig::processed(),
                        )
                        .await;

                    match data {
                        Ok(multiple_accounts) => {
                            for (index, acc) in multiple_accounts.value.iter().enumerate() {
                                if let Some(acc) = acc {
                                    this.save_account(&batch[index], &acc.data);
                                }
                            }
                        }
                        Err(e) => {
                            log::error!(
                                "error loading {} alts with error {}",
                                batch.len(),
                                e.to_string()
                            );
                        }
                    };
                })
            });
            if tokio::time::timeout(Duration::from_secs(60), futures::future::join_all(tasks))
                .await
                .is_err()
            {
                log::error!("timeout loading {} alts", alts_list.len());
            }
        }
        LRPC_ALTS_IN_STORE.set(self.map.len() as i64);
    }

    pub fn save_account(&self, address: &Pubkey, data: &[u8]) {
        let lookup_table = AddressLookupTable::deserialize(data).unwrap();
        if self
            .map
            .insert(*address, lookup_table.addresses.to_vec())
            .is_none()
        {
            LRPC_ALTS_IN_STORE.inc();
        }
        drop(lookup_table);
    }

    pub async fn reload_alt_account(&self, address: &Pubkey) {
        let account = match self
            .rpc_client
            .get_account_with_commitment(address, CommitmentConfig::processed())
            .await
        {
            Ok(acc) => acc.value,
            Err(e) => {
                log::error!(
                    "Error for fetching address lookup table {} error :{}",
                    address.to_string(),
                    e.to_string()
                );
                None
            }
        };
        match account {
            Some(account) => {
                self.save_account(address, &account.data);
            }
            None => {
                log::error!("Cannot find address lookup table {}", address.to_string());
            }
        }
    }

    async fn load_accounts(&self, alt: &Pubkey, accounts: &[u8]) -> Option<Vec<Pubkey>> {
        let do_reload = match self.map.get(alt) {
            Some(lookup_table) => accounts.iter().any(|x| *x as usize >= lookup_table.len()),
            None => true,
        };
        if do_reload {
            self.reload_alt_account(alt).await;
        }

        let alt_account = self.map.get(alt);
        match alt_account {
            Some(alt_account) => Some(
                accounts
                    .iter()
                    .map(|i| alt_account[*i as usize])
                    .collect_vec(),
            ),
            None => {
                log::error!("address lookup table {} was not found", alt);
                None
            }
        }
    }

    pub async fn get_accounts(&self, alt: &Pubkey, accounts: &[u8]) -> Vec<Pubkey> {
        match self.load_accounts(alt, accounts).await {
            Some(x) => x,
            None => {
                // forget alt for now, start loading it for next blocks
                // loading should be on its way
                vec![]
            }
        }
    }

    pub fn serialize_binary(&self) -> Vec<u8> {
        bincode::serialize::<BinaryALTData>(&BinaryALTData::new(&self.map)).unwrap()
    }

    pub fn load_binary(&self, binary_data: Vec<u8>) {
        let binary_alt_data = bincode::deserialize::<BinaryALTData>(&binary_data).unwrap();
        for (alt, accounts) in binary_alt_data.data.iter() {
            self.map.insert(*alt, accounts.clone());
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct BinaryALTData {
    pub data: Vec<(Pubkey, Vec<Pubkey>)>,
}

impl BinaryALTData {
    pub fn new(map: &Arc<DashMap<Pubkey, Vec<Pubkey>>>) -> Self {
        let data = map
            .iter()
            .map(|x| (*x.key(), x.value().clone()))
            .collect_vec();
        Self { data }
    }
}

#[async_trait]
impl AddressLookupTableInterface for ALTStore {
    async fn get_address_lookup_table(
        &self,
        message_address_table_lookup: solana_sdk::message::v0::MessageAddressTableLookup,
    ) -> (Vec<Pubkey>, Vec<Pubkey>) {
        (
            self.get_accounts(
                &message_address_table_lookup.account_key,
                &message_address_table_lookup.writable_indexes,
            )
            .await,
            self.get_accounts(
                &message_address_table_lookup.account_key,
                &message_address_table_lookup.readonly_indexes,
            )
            .await,
        )
    }
}

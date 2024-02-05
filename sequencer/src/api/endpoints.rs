//! Sequencer-specific API endpoint handlers.

use super::{
    data_source::{
        SequencerDataSource, StateDataSource, StateSignatureDataSource, SubmitDataSource,
    },
    AppState,
};
use crate::{block::FeeAccountProof, network, Header, NamespaceProofType, SeqTypes, Transaction};
use async_std::sync::{Arc, RwLock};
use commit::Committable;
use ethers::prelude::U256;
use futures::FutureExt;
use hotshot_query_service::{
    availability::{self, AvailabilityDataSource, BlockHash, FetchBlockSnafu},
    Error,
};
use hotshot_types::{data::ViewNumber, traits::state::ConsensusTime};
use jf_primitives::merkle_tree::namespaced_merkle_tree::NamespaceProof;
use serde::{Deserialize, Serialize};
use tide_disco::{
    method::{ReadState, WriteState},
    Api, Error as _, StatusCode,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NamespaceProofQueryData {
    pub proof: NamespaceProofType,
    pub header: Header,
    pub transactions: Vec<Transaction>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeWindowQueryData {
    pub window: Vec<Header>,
    pub prev: Option<Header>,
    pub next: Option<Header>,
}

impl TimeWindowQueryData {
    /// The block height of the block that starts the window.
    ///
    /// If the window is empty, this is the height of the block that ends the window.
    pub fn from(&self) -> Option<u64> {
        self.window
            .first()
            .or(self.next.as_ref())
            .map(|header| header.height)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountQueryData {
    pub balance: U256,
    pub proof: FeeAccountProof,
}

pub(super) type AvailState<N, D> = Arc<RwLock<AppState<N, D>>>;

pub(super) fn availability<N, D>() -> anyhow::Result<Api<AvailState<N, D>, availability::Error>>
where
    N: network::Type,
    D: SequencerDataSource + Send + Sync + 'static,
{
    let mut options = availability::Options::default();
    let extension = toml::from_str(include_str!("../../api/availability.toml"))?;
    options.extensions.push(extension);
    let mut api = availability::define_api::<AvailState<N, D>, SeqTypes>(&options)?;

    api.get("getnamespaceproof", |req, state| {
        async move {
            let height: usize = req.integer_param("height")?;
            let namespace: u64 = req.integer_param("namespace")?;
            let block = state.get_block(height).await.context(FetchBlockSnafu {
                resource: height.to_string(),
            })?;

            let proof = block.payload().get_namespace_proof(namespace.into());
            Ok(NamespaceProofQueryData {
                transactions: proof.get_namespace_leaves().into_iter().cloned().collect(),
                proof,
                header: block.header().clone(),
            })
        }
        .boxed()
    })?
    .get("gettimestampwindow", |req, state| {
        async move {
            let end = req.integer_param("end")?;
            let res = if let Some(height) = req.opt_integer_param("height")? {
                state.inner().window_from::<usize>(height, end).await
            } else if let Some(hash) = req.opt_blob_param("hash")? {
                state
                    .inner()
                    .window_from::<BlockHash<SeqTypes>>(hash, end)
                    .await
            } else {
                let start: u64 = req.integer_param("start")?;
                state.inner().window(start, end).await
            };
            res.map_err(|err| availability::Error::Custom {
                message: err.to_string(),
                status: err.status(),
            })
        }
        .boxed()
    })?;

    Ok(api)
}

pub(super) fn submit<N, S>() -> anyhow::Result<Api<S, Error>>
where
    N: network::Type,
    S: 'static + Send + Sync + WriteState,
    S::State: Send + Sync + SubmitDataSource<N>,
{
    let toml = toml::from_str::<toml::Value>(include_str!("../../api/submit.toml"))?;
    let mut api = Api::<S, Error>::new(toml)?;

    api.post("submit", |req, state| {
        async move {
            let tx = req
                .body_auto::<Transaction>()
                .map_err(Error::from_request_error)?;
            let hash = tx.commit();
            state
                .consensus()
                .submit_transaction(tx)
                .await
                .map_err(|err| Error::internal(err.to_string()))?;
            Ok(hash)
        }
        .boxed()
    })?;

    Ok(api)
}

pub(super) fn state_signature<N, S>() -> anyhow::Result<Api<S, Error>>
where
    N: network::Type,
    S: 'static + Send + Sync + ReadState,
    S::State: Send + Sync + StateSignatureDataSource<N>,
{
    let toml = toml::from_str::<toml::Value>(include_str!("../../api/state_signature.toml"))?;
    let mut api = Api::<S, Error>::new(toml)?;

    api.get("get_state_signature", |req, state| {
        async move {
            let height = req
                .integer_param("height")
                .map_err(Error::from_request_error)?;
            state
                .get_state_signature(height)
                .await
                .ok_or(tide_disco::Error::catch_all(
                    StatusCode::NotFound,
                    "Signature not found.".to_owned(),
                ))
        }
        .boxed()
    })?;

    Ok(api)
}

pub(super) fn state<S>() -> anyhow::Result<Api<S, Error>>
where
    S: 'static + Send + Sync + ReadState,
    S::State: Send + Sync + StateDataSource,
{
    let toml = toml::from_str::<toml::Value>(include_str!("../../api/state.toml"))?;
    let mut api = Api::<S, Error>::new(toml)?;

    api.get("account", |req, state| {
        async move {
            let state = match req
                .opt_integer_param("view")
                .map_err(Error::from_request_error)?
            {
                Some(view) => state
                    .get_undecided_state(ViewNumber::new(view))
                    .await
                    .ok_or(Error::catch_all(
                        StatusCode::NotFound,
                        format!("state not available for view {view}"),
                    ))?,
                None => state.get_decided_state().await,
            };
            let account = req
                .string_param("address")
                .map_err(Error::from_request_error)?;
            let account = account.parse().map_err(|err| {
                Error::catch_all(
                    StatusCode::BadRequest,
                    format!("malformed account {account}: {err}"),
                )
            })?;

            let (balance, proof) =
                FeeAccountProof::prove(&state.fee_merkle_tree, account).ok_or(Error::catch_all(
                    StatusCode::NotFound,
                    format!("account {account} is not in memory"),
                ))?;
            Ok(AccountQueryData { balance, proof })
        }
        .boxed()
    })?;

    Ok(api)
}

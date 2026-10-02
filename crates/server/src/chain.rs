//! Every chain interaction this server makes: the escrow and registry reads
//! the routes price and settle with, and the relayer's attestation write
//! (`web/src/lib/client.ts` plus the `readContract` / `writeContract` calls in
//! `web/src/app/api`).
//!
//! One endpoint for all of it, `ARC_RPC_URL` or the chain's default. A
//! relayer that sends through one provider and waits for receipts through
//! another can broadcast a transaction the wait never sees, so the write and
//! its receipt wait go through the same URL as the reads.
//!
//! Not the product's only route to the chain: a sender paying from their
//! browser wallet broadcasts over whatever RPC the wallet uses. Pointing
//! `ARC_RPC_URL` somewhere bounds what the server does, not what the product
//! does.

use std::time::Duration;

use alloy_network::{EthereumWallet, TransactionBuilder};
use alloy_primitives::{Address, B256, Bytes, TxHash, U256, aliases::U40};
use alloy_provider::transport::{RpcError, TransportError};
use alloy_provider::{Provider, ProviderBuilder, RootProvider};
use alloy_rpc_types_eth::{BlockId, TransactionRequest};
use alloy_signer_local::PrivateKeySigner;
use alloy_sol_types::SolCall;
use postage_core::contracts::{
    ARC_TESTNET, HUMAN_REGISTRY, HumanRegistry, POSTAGE_ESCROW, PostageEscrow,
};
use reqwest::Url;

use crate::config::ConfigError;

/// How long the attestation write waits for its receipt. A sender is watching
/// for this to resolve, so the bound has to be short enough that the fallback
/// is one they actually reach.
pub const ATTESTATION_RECEIPT_TIMEOUT: Duration = Duration::from_secs(20);

/// How often a receipt wait asks again. viem polls `arcTestnet` at its 4s
/// ceiling because the chain declares no block time.
pub const RECEIPT_POLL_INTERVAL: Duration = Duration::from_secs(4);

/// JSON-RPC error codes that say the node is struggling rather than that the
/// request was wrong: limit exceeded, internal error, resource unavailable,
/// resource not found (EIP-1474).
const TRANSIENT_RPC_CODES: [i64; 4] = [-32005, -32603, -32002, -32001];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChainError {
    /// The node could not be reached or answered with something that is not
    /// JSON-RPC: refused connection, timeout, a non-2xx status, an HTML page.
    #[error("the chain could not be reached: {0}")]
    Unreachable(String),
    /// The node answered the request with a JSON-RPC error, a revert among
    /// them.
    #[error("the node returned error {code}: {message}")]
    Rpc { code: i64, message: String },
    /// The call succeeded but its return data is not what the ABI says,
    /// including the empty answer from an address with no contract on it.
    #[error("{function} returned data that does not decode: {reason}")]
    Decode {
        function: &'static str,
        reason: String,
    },
    /// Building, signing or encoding the request failed before it was sent.
    #[error("the request could not be prepared: {0}")]
    Local(String),
    /// A transaction was sent and no receipt appeared within the bound. It
    /// may still land; the outcome is unknown, not failed.
    #[error("transaction {0} did not confirm before the wait bound")]
    ReceiptTimeout(TxHash),
}

impl ChainError {
    /// Whether this failure is the network rather than the request: worth
    /// waiting on, as opposed to a misconfiguration (a wrong address, an ABI
    /// that drifted, a rejected parameter) that should surface.
    pub fn is_transient(&self) -> bool {
        match self {
            ChainError::Unreachable(_) | ChainError::ReceiptTimeout(_) => true,
            ChainError::Rpc { code, .. } => TRANSIENT_RPC_CODES.contains(code),
            ChainError::Decode { .. } | ChainError::Local(_) => false,
        }
    }
}

impl From<TransportError> for ChainError {
    fn from(error: TransportError) -> Self {
        match error {
            RpcError::ErrorResp(payload) => ChainError::Rpc {
                code: payload.code,
                message: payload.message.into_owned(),
            },
            RpcError::Transport(kind) => ChainError::Unreachable(kind.to_string()),
            RpcError::NullResp | RpcError::DeserError { .. } => {
                ChainError::Unreachable(error.to_string())
            }
            RpcError::UnsupportedFeature(_)
            | RpcError::LocalUsageError(_)
            | RpcError::SerError(_) => ChainError::Local(error.to_string()),
        }
    }
}

/// `PostageEscrow.settlementOf(messageId)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settlement {
    pub inbox: Address,
    pub reported: bool,
    pub payer: Address,
}

impl Settlement {
    /// Who paid for the message, or `None` while nobody has: the escrow
    /// reports an unpaid message with a zero payer.
    pub fn payer(&self) -> Option<Address> {
        (!self.payer.is_zero()).then_some(self.payer)
    }
}

/// The JSON-RPC endpoint every server-side read and write goes through.
#[derive(Debug, Clone)]
pub struct Chain {
    url: Url,
    provider: RootProvider,
}

impl Chain {
    pub fn new(url: Url) -> Self {
        Self {
            provider: RootProvider::new_http(url.clone()),
            url,
        }
    }

    /// `ARC_RPC_URL`, or the chain's first default endpoint when it is unset
    /// or empty, so nothing has to be configured for this to run.
    pub fn from_env<F>(env: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        Ok(Self::new(rpc_url(env)?))
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    /// `PostageEscrow.effectiveFloor(inbox)`: the floor the escrow enforces,
    /// which an inbox whose owner never chose a price still has.
    pub async fn effective_floor(&self, inbox: Address) -> Result<U256, ChainError> {
        self.read(POSTAGE_ESCROW, PostageEscrow::effectiveFloorCall { inbox })
            .await
    }

    /// `PostageEscrow.settlementOf(messageId)`.
    pub async fn settlement_of(&self, message_id: B256) -> Result<Settlement, ChainError> {
        let settled = self
            .read(
                POSTAGE_ESCROW,
                PostageEscrow::settlementOfCall {
                    messageId: message_id,
                },
            )
            .await?;
        Ok(Settlement {
            inbox: settled.inbox,
            reported: settled.reported,
            payer: settled.payer,
        })
    }

    /// `HumanRegistry.humanUntil(wallet)`, epoch seconds; 0 for nobody.
    pub async fn human_until(&self, wallet: Address) -> Result<u64, ChainError> {
        let until: U40 = self
            .read(HUMAN_REGISTRY, HumanRegistry::humanUntilCall { wallet })
            .await?;
        Ok(until.to())
    }

    /// Sends `HumanRegistry.attest(wallet, nullifierHash, expiresAt,
    /// signature)` from `relayer`, returning the hash as soon as the node has
    /// accepted it. Nonce, gas and fees are read from the node per call; the
    /// chain id is fixed to Arc's, so a node on another chain rejects the
    /// transaction rather than replaying it there.
    pub async fn attest(
        &self,
        relayer: &PrivateKeySigner,
        wallet: Address,
        nullifier_hash: B256,
        expires_at: u64,
        signature: Bytes,
    ) -> Result<TxHash, ChainError> {
        let expires_at = U40::try_from(expires_at).map_err(|_| {
            ChainError::Local(format!("expiresAt {expires_at} does not fit a uint40"))
        })?;
        let call = HumanRegistry::attestCall {
            wallet,
            nullifierHash: nullifier_hash,
            expiresAt: expires_at,
            signature,
        };
        let provider = ProviderBuilder::new()
            .with_chain_id(ARC_TESTNET.id)
            .wallet(EthereumWallet::from(relayer.clone()))
            .connect_http(self.url.clone());
        let request = TransactionRequest::default()
            .with_to(HUMAN_REGISTRY)
            .with_input(call.abi_encode());
        let pending = provider.send_transaction(request).await?;
        Ok(*pending.tx_hash())
    }

    /// Reads the receipt now and then every `poll_interval` until one exists
    /// or `timeout` passes. `Ok(true)` is a mined success, `Ok(false)` a mined
    /// revert: a receipt is not a success on its own. A failed read ends the
    /// wait with that error, since the outcome is then just as unknown.
    pub async fn wait_for_receipt(
        &self,
        hash: TxHash,
        timeout: Duration,
        poll_interval: Duration,
    ) -> Result<bool, ChainError> {
        let poll = async {
            loop {
                if let Some(receipt) = self.provider.get_transaction_receipt(hash).await? {
                    return Ok(receipt.status());
                }
                tokio::time::sleep(poll_interval).await;
            }
        };
        tokio::time::timeout(timeout, poll)
            .await
            .unwrap_or(Err(ChainError::ReceiptTimeout(hash)))
    }

    /// `eth_call` at the latest block, as viem's `readContract` asks; alloy
    /// would otherwise ask about the pending one.
    async fn read<C: SolCall>(&self, to: Address, call: C) -> Result<C::Return, ChainError> {
        let request = TransactionRequest::default()
            .with_to(to)
            .with_input(call.abi_encode());
        let output = self.provider.call(request).block(BlockId::latest()).await?;
        C::abi_decode_returns(&output).map_err(|error| ChainError::Decode {
            function: C::SIGNATURE,
            reason: error.to_string(),
        })
    }
}

/// `ARC_RPC_URL`, falling back to the chain's first default endpoint.
pub fn rpc_url<F>(env: F) -> Result<Url, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let configured = env("ARC_RPC_URL").filter(|value| !value.is_empty());
    let Some(text) = configured else {
        return Url::parse(ARC_TESTNET.rpc_urls[0])
            .map_err(|_| ConfigError::Invalid("ARC_RPC_URL"));
    };
    Url::parse(&text).map_err(|_| ConfigError::Invalid("ARC_RPC_URL"))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use alloy_consensus::TxEnvelope;
    use alloy_consensus::transaction::SignerRecoverable;
    use alloy_network::eip2718::Decodable2718;
    use alloy_primitives::{address, b256, bytes};
    use axum::Json;
    use axum::extract::State;
    use serde_json::{Value, json};

    use super::*;

    type Answer = Result<Value, (i64, String)>;
    type Responder = Arc<dyn Fn(&str, &Value) -> Answer + Send + Sync>;

    #[derive(Clone)]
    struct Node {
        respond: Responder,
        seen: Arc<Mutex<Vec<(String, Value)>>>,
    }

    struct Stub {
        url: Url,
        seen: Arc<Mutex<Vec<(String, Value)>>>,
    }

    impl Stub {
        fn calls(&self, method: &str) -> Vec<Value> {
            self.seen
                .lock()
                .unwrap()
                .iter()
                .filter(|(name, _)| name == method)
                .map(|(_, params)| params.clone())
                .collect()
        }
    }

    async fn rpc(State(node): State<Node>, Json(request): Json<Value>) -> Json<Value> {
        let method = request["method"].as_str().unwrap_or_default().to_owned();
        let params = request["params"].clone();
        node.seen
            .lock()
            .unwrap()
            .push((method.clone(), params.clone()));
        let id = request["id"].clone();
        Json(match (node.respond)(&method, &params) {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => json!({
                "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message }
            }),
        })
    }

    /// A loopback JSON-RPC node answering each method from `respond`.
    async fn node(respond: impl Fn(&str, &Value) -> Answer + Send + Sync + 'static) -> Stub {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let state = Node {
            respond: Arc::new(respond),
            seen: seen.clone(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let app = axum::Router::new()
            .route("/", axum::routing::post(rpc))
            .with_state(state);
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Stub { url, seen }
    }

    /// A node whose `eth_call` returns `output` and that knows nothing else.
    async fn call_returning(output: Answer) -> Stub {
        node(move |method, _| match method {
            "eth_call" => output.clone(),
            other => Err((-32601, format!("{other} not stubbed"))),
        })
        .await
    }

    fn word(value: u64) -> String {
        format!("{value:064x}")
    }

    fn decode_raw(raw: &str) -> TxEnvelope {
        let bytes = alloy_primitives::hex::decode(raw).unwrap();
        TxEnvelope::decode_2718(&mut bytes.as_slice()).unwrap()
    }

    const INBOX: Address = address!("0x1234567890abcdef1234567890abcdef12345678");

    #[tokio::test]
    async fn effective_floor_calls_the_escrow_and_decodes_the_amount() {
        let stub = call_returning(Ok(json!(format!("0x{}", word(10_000_000_000_000_000))))).await;

        let floor = Chain::new(stub.url.clone())
            .effective_floor(INBOX)
            .await
            .unwrap();

        assert_eq!(floor, U256::from(10_000_000_000_000_000u64));
        let calls = stub.calls("eth_call");
        assert_eq!(calls.len(), 1);
        let request = &calls[0][0];
        assert_eq!(
            request["to"].as_str().unwrap().to_lowercase(),
            POSTAGE_ESCROW.to_string().to_lowercase()
        );
        let input = request["input"]
            .as_str()
            .or(request["data"].as_str())
            .unwrap();
        let expected = PostageEscrow::effectiveFloorCall { inbox: INBOX }.abi_encode();
        assert_eq!(
            input,
            format!("0x{}", alloy_primitives::hex::encode(expected))
        );
        assert_eq!(calls[0][1], "latest");
    }

    #[tokio::test]
    async fn human_until_calls_the_registry_and_decodes_the_uint40() {
        let stub = call_returning(Ok(json!(format!("0x{}", word(1_800_000_000))))).await;

        let until = Chain::new(stub.url.clone())
            .human_until(INBOX)
            .await
            .unwrap();

        assert_eq!(until, 1_800_000_000);
        let request = &stub.calls("eth_call")[0][0];
        assert_eq!(
            request["to"].as_str().unwrap().to_lowercase(),
            HUMAN_REGISTRY.to_string().to_lowercase()
        );
    }

    #[tokio::test]
    async fn settlement_of_reports_the_payer_once_someone_has_paid() {
        let payer = address!("0x00000000000000000000000000000000000000aa");
        let output = format!(
            "0x{:0>64}{}{:0>64}",
            alloy_primitives::hex::encode(INBOX),
            word(1),
            alloy_primitives::hex::encode(payer)
        );
        let stub = call_returning(Ok(json!(output))).await;

        let settlement = Chain::new(stub.url.clone())
            .settlement_of(B256::repeat_byte(0xab))
            .await
            .unwrap();

        assert_eq!(
            settlement,
            Settlement {
                inbox: INBOX,
                reported: true,
                payer
            }
        );
        assert_eq!(settlement.payer(), Some(payer));
    }

    #[tokio::test]
    async fn settlement_of_an_unpaid_message_has_no_payer() {
        let stub = call_returning(Ok(json!(format!("0x{}", "0".repeat(192))))).await;

        let settlement = Chain::new(stub.url.clone())
            .settlement_of(B256::ZERO)
            .await
            .unwrap();

        assert_eq!(settlement.payer(), None);
    }

    #[tokio::test]
    async fn an_empty_answer_from_an_address_without_code_is_a_decode_failure_to_surface() {
        let stub = call_returning(Ok(json!("0x"))).await;

        let error = Chain::new(stub.url.clone())
            .effective_floor(INBOX)
            .await
            .unwrap_err();

        assert!(matches!(error, ChainError::Decode { .. }), "{error}");
        assert!(!error.is_transient());
    }

    #[tokio::test]
    async fn a_revert_is_an_rpc_error_that_is_not_transient() {
        let stub = call_returning(Err((3, "execution reverted".to_owned()))).await;

        let error = Chain::new(stub.url.clone())
            .settlement_of(B256::ZERO)
            .await
            .unwrap_err();

        assert_eq!(
            error,
            ChainError::Rpc {
                code: 3,
                message: "execution reverted".to_owned()
            }
        );
        assert!(!error.is_transient());
    }

    #[tokio::test]
    async fn a_rejected_parameter_is_a_misconfiguration_not_a_wait() {
        let stub = call_returning(Err((-32602, "invalid params".to_owned()))).await;

        let error = Chain::new(stub.url.clone())
            .settlement_of(B256::ZERO)
            .await
            .unwrap_err();

        assert!(!error.is_transient(), "{error}");
    }

    #[tokio::test]
    async fn a_rate_limited_node_is_transient() {
        let stub = call_returning(Err((-32005, "limit exceeded".to_owned()))).await;

        let error = Chain::new(stub.url.clone())
            .settlement_of(B256::ZERO)
            .await
            .unwrap_err();

        assert!(error.is_transient(), "{error}");
    }

    #[tokio::test]
    async fn an_unreachable_node_is_transient() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let closed = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        drop(listener);

        let error = Chain::new(closed).human_until(INBOX).await.unwrap_err();

        assert!(matches!(error, ChainError::Unreachable(_)), "{error}");
        assert!(error.is_transient());
    }

    #[tokio::test]
    async fn a_node_answering_with_an_http_error_is_transient() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let app = axum::Router::new().route(
            "/",
            axum::routing::post(|| async { (axum::http::StatusCode::SERVICE_UNAVAILABLE, "busy") }),
        );
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let error = Chain::new(url).human_until(INBOX).await.unwrap_err();

        assert!(matches!(error, ChainError::Unreachable(_)), "{error}");
        assert!(error.is_transient());
    }

    fn relayer() -> PrivateKeySigner {
        PrivateKeySigner::from_slice(&[0x22; 32]).unwrap()
    }

    /// A node that accepts a transaction and remembers its raw bytes.
    async fn accepting_node() -> (Stub, Arc<Mutex<Option<String>>>) {
        let raw = Arc::new(Mutex::new(None));
        let captured = raw.clone();
        let stub = node(move |method, params| match method {
            "eth_chainId" => Ok(json!(format!("0x{:x}", ARC_TESTNET.id))),
            "eth_getTransactionCount" => Ok(json!("0x7")),
            "eth_estimateGas" => Ok(json!("0x15000")),
            "eth_gasPrice" | "eth_maxPriorityFeePerGas" => Ok(json!("0x5d21dba00")),
            "eth_feeHistory" => Ok(json!({
                "oldestBlock": "0x1",
                "baseFeePerGas": ["0x5d21dba00", "0x5d21dba00"],
                "gasUsedRatio": [0.5],
                "reward": [["0x1"]]
            })),
            "eth_getBlockByNumber" => Err((-32601, "not stubbed".to_owned())),
            "eth_sendRawTransaction" => {
                let raw_tx = params[0].as_str().unwrap_or_default().to_owned();
                *captured.lock().unwrap() = Some(raw_tx);
                Ok(json!(format!("0x{}", "cd".repeat(32))))
            }
            other => Err((-32601, format!("{other} not stubbed"))),
        })
        .await;
        (stub, raw)
    }

    #[tokio::test]
    async fn attest_sends_a_signed_registry_call_from_the_relayer_on_arc() {
        let (stub, raw) = accepting_node().await;
        let wallet = address!("0x00000000000000000000000000000000000000bb");
        let nullifier = b256!("0x0101010101010101010101010101010101010101010101010101010101010101");
        let signature = bytes!("0xdeadbeef");

        let hash = Chain::new(stub.url.clone())
            .attest(
                &relayer(),
                wallet,
                nullifier,
                1_800_000_000,
                signature.clone(),
            )
            .await
            .unwrap();

        assert_eq!(hash, B256::repeat_byte(0xcd));
        let raw = raw.lock().unwrap().clone().unwrap();
        let envelope: TxEnvelope = decode_raw(&raw);
        assert_eq!(envelope.recover_signer().unwrap(), relayer().address());
        assert_eq!(
            alloy_consensus::Transaction::chain_id(&envelope),
            Some(ARC_TESTNET.id)
        );
        assert_eq!(
            alloy_consensus::Transaction::to(&envelope),
            Some(HUMAN_REGISTRY)
        );
        assert_eq!(alloy_consensus::Transaction::nonce(&envelope), 7);
        let expected = HumanRegistry::attestCall {
            wallet,
            nullifierHash: nullifier,
            expiresAt: U40::from(1_800_000_000u64),
            signature,
        }
        .abi_encode();
        assert_eq!(
            alloy_consensus::Transaction::input(&envelope).as_ref(),
            expected.as_slice()
        );
    }

    #[tokio::test]
    async fn attest_refuses_an_expiry_past_uint40_before_sending_anything() {
        let (stub, raw) = accepting_node().await;

        let error = Chain::new(stub.url.clone())
            .attest(&relayer(), INBOX, B256::ZERO, 1 << 40, Bytes::new())
            .await
            .unwrap_err();

        assert!(matches!(error, ChainError::Local(_)), "{error}");
        assert!(raw.lock().unwrap().is_none());
        assert!(stub.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_node_refusing_the_transaction_is_an_rpc_error() {
        let stub = node(|method, _| match method {
            "eth_sendRawTransaction" => Err((-32000, "nonce too low".to_owned())),
            "eth_getTransactionCount" => Ok(json!("0x0")),
            "eth_estimateGas" => Ok(json!("0x15000")),
            "eth_feeHistory" => Ok(json!({
                "oldestBlock": "0x1", "baseFeePerGas": ["0x1", "0x1"],
                "gasUsedRatio": [0.5], "reward": [["0x1"]]
            })),
            "eth_gasPrice" | "eth_maxPriorityFeePerGas" => Ok(json!("0x1")),
            other => Err((-32601, format!("{other} not stubbed"))),
        })
        .await;

        let error = Chain::new(stub.url.clone())
            .attest(&relayer(), INBOX, B256::ZERO, 1, Bytes::new())
            .await
            .unwrap_err();

        assert_eq!(
            error,
            ChainError::Rpc {
                code: -32000,
                message: "nonce too low".to_owned()
            }
        );
    }

    fn receipt(status: &str) -> Value {
        json!({
            "transactionHash": format!("0x{}", "cd".repeat(32)),
            "transactionIndex": "0x0",
            "blockHash": format!("0x{}", "ee".repeat(32)),
            "blockNumber": "0x10",
            "from": "0x00000000000000000000000000000000000000aa",
            "to": HUMAN_REGISTRY.to_string(),
            "cumulativeGasUsed": "0x5208",
            "gasUsed": "0x5208",
            "effectiveGasPrice": "0x1",
            "contractAddress": null,
            "logs": [],
            "logsBloom": format!("0x{}", "00".repeat(256)),
            "type": "0x2",
            "status": status
        })
    }

    /// A node with no receipt for the first `pending` reads, then `status`.
    async fn mining_node(pending: usize, status: &'static str) -> Stub {
        let reads = Arc::new(Mutex::new(0usize));
        node(move |method, _| match method {
            "eth_getTransactionReceipt" => {
                let mut count = reads.lock().unwrap();
                *count += 1;
                if *count <= pending {
                    Ok(Value::Null)
                } else {
                    Ok(receipt(status))
                }
            }
            other => Err((-32601, format!("{other} not stubbed"))),
        })
        .await
    }

    const FAST: Duration = Duration::from_millis(10);

    #[tokio::test]
    async fn a_receipt_wait_reads_until_the_transaction_is_mined() {
        let stub = mining_node(2, "0x1").await;

        let succeeded = Chain::new(stub.url.clone())
            .wait_for_receipt(B256::repeat_byte(0xcd), Duration::from_secs(5), FAST)
            .await
            .unwrap();

        assert!(succeeded);
        assert_eq!(stub.calls("eth_getTransactionReceipt").len(), 3);
    }

    #[tokio::test]
    async fn a_mined_revert_is_a_receipt_but_not_a_success() {
        let stub = mining_node(0, "0x0").await;

        let succeeded = Chain::new(stub.url.clone())
            .wait_for_receipt(B256::repeat_byte(0xcd), Duration::from_secs(5), FAST)
            .await
            .unwrap();

        assert!(!succeeded);
    }

    #[tokio::test]
    async fn no_receipt_within_the_bound_is_an_unknown_outcome_naming_the_hash() {
        let stub = mining_node(usize::MAX, "0x1").await;
        let hash = B256::repeat_byte(0xcd);

        let error = Chain::new(stub.url.clone())
            .wait_for_receipt(hash, Duration::from_millis(100), FAST)
            .await
            .unwrap_err();

        assert_eq!(error, ChainError::ReceiptTimeout(hash));
        assert!(stub.calls("eth_getTransactionReceipt").len() > 1);
    }

    #[test]
    fn the_attestation_wait_bound_and_poll_match_the_typescript() {
        assert_eq!(ATTESTATION_RECEIPT_TIMEOUT, Duration::from_secs(20));
        assert_eq!(RECEIPT_POLL_INTERVAL, Duration::from_secs(4));
    }

    #[test]
    fn an_unset_or_empty_rpc_url_falls_back_to_the_chains_default() {
        for value in [None, Some(String::new())] {
            let url = rpc_url(|_| value.clone()).unwrap();
            assert_eq!(url.as_str(), "https://rpc.testnet.arc.network/");
        }
    }

    #[test]
    fn a_configured_rpc_url_is_used() {
        let url = rpc_url(|_| Some("https://rpc.example.test/key".to_owned())).unwrap();
        assert_eq!(url.as_str(), "https://rpc.example.test/key");
    }

    #[test]
    fn an_rpc_url_that_does_not_parse_is_invalid() {
        assert_eq!(
            rpc_url(|_| Some("not a url".to_owned())),
            Err(ConfigError::Invalid("ARC_RPC_URL"))
        );
    }
}

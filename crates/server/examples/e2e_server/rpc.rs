//! A JSON-RPC node for Arc, answering the reads and writes the flows make:
//! the escrow and registry views, and the relayer's attestation transaction
//! with its receipt. State lives in the [`Hub`]: who has paid for which
//! message, and whether an attestation has been sent.

use alloy_primitives::{Address, B256, U256, hex, keccak256};
use alloy_sol_types::SolCall;
use postage_core::contracts::{HUMAN_REGISTRY, HumanRegistry, POSTAGE_ESCROW, PostageEscrow};
use serde_json::{Value, json};

use crate::hub::Hub;

/// 0.05 USDC in 18-decimal base units, what every inbox is priced at.
pub const FLOOR: u128 = 50_000_000_000_000_000;
/// Arc testnet's chain id, 5042002.
const CHAIN_ID: &str = "0x4cef52";
const GAS_PRICE: &str = "0x3b9aca00";

/// Answers one request, or a batch of them.
pub fn handle(hub: &Hub, request: &Value) -> Value {
    match request {
        Value::Array(batch) => Value::Array(batch.iter().map(|one| answer(hub, one)).collect()),
        one => answer(hub, one),
    }
}

fn answer(hub: &Hub, request: &Value) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let params = request.get("params").cloned().unwrap_or(Value::Null);
    match dispatch(hub, method, &params) {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(message) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": message } })
        }
    }
}

fn dispatch(hub: &Hub, method: &str, params: &Value) -> Result<Value, String> {
    match method {
        "eth_chainId" => Ok(json!(CHAIN_ID)),
        "net_version" => Ok(json!("5042002")),
        "eth_blockNumber" => Ok(json!("0x64")),
        "eth_gasPrice" | "eth_maxPriorityFeePerGas" => Ok(json!(GAS_PRICE)),
        "eth_estimateGas" => Ok(json!("0x30d40")),
        "eth_getTransactionCount" => Ok(json!(format!("0x{:x}", hub.next_nonce()))),
        "eth_feeHistory" => Ok(fee_history(params)),
        "eth_getBlockByNumber" => Ok(latest_block()),
        "eth_call" => eth_call(hub, params),
        "eth_sendRawTransaction" => send_raw(hub, params),
        "eth_getTransactionReceipt" => Ok(receipt(hub, params)),
        other => Err(format!("the e2e node does not implement {other}")),
    }
}

fn eth_call(hub: &Hub, params: &Value) -> Result<Value, String> {
    let call = params.get(0).ok_or("eth_call needs a call object")?;
    let to: Address = call
        .get("to")
        .and_then(Value::as_str)
        .ok_or("eth_call needs a to")?
        .parse()
        .map_err(|_| "to is not an address")?;
    let data_text = call
        .get("data")
        .or_else(|| call.get("input"))
        .and_then(Value::as_str)
        .ok_or("eth_call needs data")?;
    let data = hex::decode(data_text).map_err(|_| "data is not hex")?;
    let selector: [u8; 4] = data
        .get(..4)
        .and_then(|head| head.try_into().ok())
        .ok_or("data is shorter than a selector")?;

    let word = |value: U256| {
        Ok(json!(format!(
            "0x{}",
            hex::encode(value.to_be_bytes::<32>())
        )))
    };
    if to == POSTAGE_ESCROW {
        return match selector {
            s if s == PostageEscrow::effectiveFloorCall::SELECTOR => word(U256::from(FLOOR)),
            s if s == PostageEscrow::floorPriceCall::SELECTOR => word(U256::ZERO),
            s if s == PostageEscrow::earningsCall::SELECTOR => word(U256::ZERO),
            s if s == PostageEscrow::settlementOfCall::SELECTOR => {
                let message = PostageEscrow::settlementOfCall::abi_decode(&data)
                    .map_err(|error| error.to_string())?;
                Ok(settlement(hub, message.messageId))
            }
            _ => Err(format!(
                "escrow selector 0x{} is not stubbed",
                hex::encode(selector)
            )),
        };
    }
    if to == HUMAN_REGISTRY && selector == HumanRegistry::humanUntilCall::SELECTOR {
        return word(U256::from(hub.human_until()));
    }
    Err(format!(
        "call to {to} with selector 0x{} is not stubbed",
        hex::encode(selector)
    ))
}

/// `settlementOf`: the inbox, whether it was reported as spam, and the payer.
fn settlement(hub: &Hub, message_id: B256) -> Value {
    let payer = hub.payer_of(&message_id).unwrap_or(Address::ZERO);
    let mut out = [0_u8; 96];
    out[44..64].copy_from_slice(Address::repeat_byte(0xcd).as_slice());
    out[76..96].copy_from_slice(payer.as_slice());
    json!(format!("0x{}", hex::encode(out)))
}

fn send_raw(hub: &Hub, params: &Value) -> Result<Value, String> {
    let raw = params
        .get(0)
        .and_then(Value::as_str)
        .ok_or("eth_sendRawTransaction needs the raw transaction")?;
    let bytes = hex::decode(raw).map_err(|_| "the raw transaction is not hex")?;
    let hash = keccak256(&bytes);
    hub.record_transaction(hash);
    Ok(json!(hash.to_string()))
}

/// A mined, successful receipt for a hash this node was sent; null otherwise.
fn receipt(hub: &Hub, params: &Value) -> Value {
    let Some(hash) = params
        .get(0)
        .and_then(Value::as_str)
        .and_then(|text| text.parse::<B256>().ok())
        .filter(|hash| hub.knows_transaction(hash))
    else {
        return Value::Null;
    };
    json!({
        "transactionHash": hash.to_string(),
        "transactionIndex": "0x0",
        "blockHash": format!("0x{}", "11".repeat(32)),
        "blockNumber": "0x65",
        "from": Address::repeat_byte(0xaa).to_string(),
        "to": HUMAN_REGISTRY.to_string(),
        "cumulativeGasUsed": "0x12345",
        "gasUsed": "0x12345",
        "effectiveGasPrice": GAS_PRICE,
        "contractAddress": null,
        "logs": [],
        "logsBloom": format!("0x{}", "00".repeat(256)),
        "type": "0x2",
        "status": "0x1",
    })
}

fn fee_history(params: &Value) -> Value {
    let blocks = params
        .get(0)
        .and_then(|count| match count {
            Value::String(text) => u64::from_str_radix(text.trim_start_matches("0x"), 16).ok(),
            Value::Number(number) => number.as_u64(),
            _ => None,
        })
        .unwrap_or(1)
        .clamp(1, 16);
    let count = usize::try_from(blocks).unwrap_or(1);
    json!({
        "oldestBlock": "0x1",
        "baseFeePerGas": vec![GAS_PRICE; count + 1],
        "gasUsedRatio": vec![0.5; count],
        "reward": vec![vec![GAS_PRICE]; count],
    })
}

fn latest_block() -> Value {
    json!({
        "number": "0x64",
        "hash": format!("0x{}", "22".repeat(32)),
        "parentHash": format!("0x{}", "21".repeat(32)),
        "nonce": "0x0000000000000000",
        "sha3Uncles": format!("0x{}", "1d".repeat(32)),
        "logsBloom": format!("0x{}", "00".repeat(256)),
        "transactionsRoot": format!("0x{}", "56".repeat(32)),
        "stateRoot": format!("0x{}", "57".repeat(32)),
        "receiptsRoot": format!("0x{}", "58".repeat(32)),
        "miner": Address::ZERO.to_string(),
        "difficulty": "0x0",
        "totalDifficulty": "0x0",
        "extraData": "0x",
        "size": "0x200",
        "gasLimit": "0x1c9c380",
        "gasUsed": "0x0",
        "timestamp": "0x6000000",
        "transactions": [],
        "uncles": [],
        "baseFeePerGas": GAS_PRICE,
    })
}

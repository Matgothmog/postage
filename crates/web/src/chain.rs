//! Reading the escrow contract from the browser. Replaces the `viem`
//! `publicClient.readContract` calls `InboxPanel.tsx` makes through
//! `web/src/lib/client.ts`.
//!
//! No alloy provider (it would pull a second HTTP stack into the wasm
//! bundle): a read is one JSON-RPC `eth_call` POSTed with `fetch` to Arc
//! testnet's first public RPC URL, at block `latest`. Calls are encoded and
//! results decoded with the same `alloy-sol-types` bindings `postage_core`
//! derives from the escrow ABI, so a selector or a return type cannot drift
//! from the contract the way a hand-written call could.

use alloy_primitives::{Address, Bytes, U256};
use alloy_sol_types::SolCall;
use futures::future::try_join3;
use postage_core::contracts::{ARC_TESTNET, POSTAGE_ESCROW, PostageEscrow};
use serde::Deserialize;
use serde_json::json;

use crate::http::{self, HttpError, HttpRequest, HttpResponse};

/// What the panel shows about an owner, read from the chain rather than from
/// our own copy of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Standing {
    pub earned: u128,
    pub floor: u128,
    /// False while the owner has never called `setFloorPrice`, in which case
    /// `floor` is the default the escrow charges on their behalf.
    pub chosen: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChainError {
    #[error("Could not reach the network to read your balance. Try again in a moment")]
    Unreachable,
    #[error("The network refused the read: {0}")]
    Rpc(String),
    #[error("The network answered with something unreadable")]
    Unreadable,
}

/// The RPC endpoint reads go to: the chain's first public URL, the one
/// viem's `http()` transport used.
pub fn rpc_url() -> &'static str {
    ARC_TESTNET
        .rpc_urls
        .first()
        .copied()
        .unwrap_or("https://rpc.testnet.arc.network")
}

/// The JSON-RPC request for one `eth_call` at block `latest`.
pub fn call_body(to: Address, data: &Bytes) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_call",
        "params": [{ "to": to.to_string(), "data": data.to_string() }, "latest"],
    })
    .to_string()
}

/// The returned bytes of an `eth_call`, or why there are none.
pub fn result_of(answer: Result<HttpResponse, HttpError>) -> Result<Bytes, ChainError> {
    #[derive(Deserialize)]
    struct RpcError {
        message: String,
    }
    #[derive(Deserialize)]
    struct Reply {
        result: Option<String>,
        error: Option<RpcError>,
    }

    let response = answer.map_err(|_| ChainError::Unreachable)?;
    // Nodes answer a JSON-RPC error with 200, but a gateway in front of one
    // may not, so the body is read before the status is held against it.
    let reply = serde_json::from_str::<Reply>(&response.body).ok();
    if let Some(Reply {
        error: Some(error), ..
    }) = &reply
    {
        return Err(ChainError::Rpc(error.message.clone()));
    }
    if !response.is_ok() {
        return Err(ChainError::Rpc(format!("status {}", response.status)));
    }
    reply
        .and_then(|reply| reply.result)
        .ok_or(ChainError::Unreadable)?
        .parse()
        .map_err(|_| ChainError::Unreadable)
}

/// `eth_call` against the escrow, decoded as `C`'s return type.
async fn read<C: SolCall>(call: C) -> Result<C::Return, ChainError> {
    let data = Bytes::from(call.abi_encode());
    let request = HttpRequest::post_json(rpc_url(), call_body(POSTAGE_ESCROW, &data));
    let returned = result_of(http::send(&request, None).await)?;
    C::abi_decode_returns(&returned).map_err(|_| ChainError::Unreadable)
}

/// An on-chain amount as the `u128` the display code takes. Anything above
/// `u128::MAX` base units (3.4e20 whole USDC) is not a real balance; it reads
/// as the largest one rather than failing the panel.
fn amount(value: U256) -> u128 {
    u128::try_from(value).unwrap_or(u128::MAX)
}

/// The panel's numbers from the three reads: `earnings`, `floorPrice` (zero
/// until chosen) and `effectiveFloor` (what is actually charged).
pub fn standing_of(earned: U256, chosen_floor: U256, effective_floor: U256) -> Standing {
    Standing {
        earned: amount(earned),
        floor: amount(effective_floor),
        chosen: !chosen_floor.is_zero(),
    }
}

/// Reads the owner's standing, the three calls concurrently.
pub async fn read_standing(owner: Address) -> Result<Standing, ChainError> {
    let (earned, chosen, floor) = try_join3(
        read(PostageEscrow::earningsCall { inbox: owner }),
        read(PostageEscrow::floorPriceCall { inbox: owner }),
        read(PostageEscrow::effectiveFloorCall { inbox: owner }),
    )
    .await?;
    Ok(standing_of(earned, chosen, floor))
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{address, hex};

    use super::*;

    const OWNER: Address = address!("0x19e7e376e7c213b7e7e7e46cc70a5dd086daff2a");

    fn answer(status: u16, body: &str) -> Result<HttpResponse, HttpError> {
        Ok(HttpResponse {
            status,
            body: body.to_owned(),
        })
    }

    fn word(value: u64) -> String {
        format!("0x{:064x}", value)
    }

    #[test]
    fn reads_go_to_the_first_public_arc_rpc_url() {
        assert_eq!(rpc_url(), "https://rpc.testnet.arc.network");
    }

    #[test]
    fn a_call_is_an_eth_call_to_the_escrow_at_latest() {
        let data = Bytes::from(PostageEscrow::earningsCall { inbox: OWNER }.abi_encode());
        let body: serde_json::Value =
            serde_json::from_str(&call_body(POSTAGE_ESCROW, &data)).unwrap();
        assert_eq!(body["method"], "eth_call");
        assert_eq!(body["params"][1], "latest");
        assert_eq!(
            body["params"][0]["to"].as_str().unwrap().to_lowercase(),
            "0x4469e869433cf6cc08dd54afc6ac7e288b9a38f7"
        );
        let data = body["params"][0]["data"].as_str().unwrap();
        // earnings(address): selector 543fd313, then the owner left-padded.
        assert!(data.starts_with("0x543fd313"), "{data}");
        assert!(
            data.ends_with("19e7e376e7c213b7e7e7e46cc70a5dd086daff2a"),
            "{data}"
        );
        assert_eq!(data.len(), 2 + 8 + 64);
    }

    #[test]
    fn the_three_selectors_are_the_ones_the_typescript_read() {
        let selector = |data: Vec<u8>| hex::encode(&data[..4]);
        assert_eq!(
            selector(PostageEscrow::floorPriceCall { inbox: OWNER }.abi_encode()),
            "2aad9987"
        );
        assert_eq!(
            selector(PostageEscrow::effectiveFloorCall { inbox: OWNER }.abi_encode()),
            "552c804e"
        );
    }

    #[test]
    fn a_result_word_decodes_to_the_amount() {
        let body = format!(r#"{{"jsonrpc":"2.0","id":1,"result":"{}"}}"#, word(10_000));
        let returned = result_of(answer(200, &body)).unwrap();
        let decoded = PostageEscrow::earningsCall::abi_decode_returns(&returned).unwrap();
        assert_eq!(decoded, U256::from(10_000));
    }

    #[test]
    fn a_json_rpc_error_carries_the_nodes_message() {
        let body = r#"{"jsonrpc":"2.0","id":1,"error":{"code":3,"message":"execution reverted"}}"#;
        assert_eq!(
            result_of(answer(200, body)),
            Err(ChainError::Rpc("execution reverted".to_owned()))
        );
    }

    #[test]
    fn a_dead_gateway_or_a_missing_answer_is_not_a_balance() {
        assert_eq!(
            result_of(Err(HttpError::Network("offline".to_owned()))),
            Err(ChainError::Unreachable)
        );
        assert_eq!(
            result_of(answer(502, "<html>bad gateway</html>")),
            Err(ChainError::Rpc("status 502".to_owned()))
        );
        assert_eq!(
            result_of(answer(200, "<html>surprise</html>")),
            Err(ChainError::Unreadable)
        );
        assert_eq!(
            result_of(answer(200, r#"{"jsonrpc":"2.0","id":1}"#)),
            Err(ChainError::Unreadable)
        );
        assert_eq!(
            result_of(answer(200, r#"{"result":"not hex"}"#)),
            Err(ChainError::Unreadable)
        );
    }

    #[test]
    fn a_result_too_short_to_hold_a_word_does_not_decode() {
        let returned = result_of(answer(200, r#"{"result":"0x01"}"#)).unwrap();
        assert!(PostageEscrow::earningsCall::abi_decode_returns(&returned).is_err());
    }

    #[test]
    fn a_floor_of_zero_means_the_owner_never_chose_one() {
        let standing = standing_of(U256::from(7), U256::ZERO, U256::from(10_u128.pow(16)));
        assert_eq!(
            standing,
            Standing {
                earned: 7,
                floor: 10_u128.pow(16),
                chosen: false
            }
        );
    }

    #[test]
    fn a_chosen_floor_is_marked_chosen_and_the_effective_floor_is_what_is_shown() {
        let standing = standing_of(U256::ZERO, U256::from(5), U256::from(5));
        assert!(standing.chosen);
        assert_eq!(standing.floor, 5);
    }

    #[test]
    fn an_amount_beyond_u128_saturates_instead_of_failing() {
        assert_eq!(amount(U256::MAX), u128::MAX);
        assert_eq!(amount(U256::from(42)), 42);
    }
}

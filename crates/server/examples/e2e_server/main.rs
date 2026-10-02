//! A local, non-Vercel Postage server for the end-to-end run: the real Axum
//! router over a real libsql file, the built `crates/web/dist` bundle, and a
//! loopback "hub" standing in for every outside service (Anthropic, Resend,
//! Cloudflare, The Graph, the mail worker, World, the Arc node, Privy's JWKS).
//! Nothing here reaches the network.
//!
//! Run through `node e2e/run.mjs`, which builds and drives it; by hand:
//!
//! ```text
//! cargo run -p postage-server --example e2e_server -- \
//!     --db /tmp/e2e.db --dist crates/web/dist --repo .
//! ```
//!
//! It prints one `E2E_READY {json}` line on stdout once it is serving.

mod hub;
mod identity;
mod rpc;
mod site;

use std::path::PathBuf;
use std::sync::Arc;

use alloy_primitives::hex;
use axum::http::Uri;
use postage_core::quote::QuoteSigner;
use postage_server::cloudflare::Cloudflare;
use postage_server::config::Env;
use postage_server::mail::Mailer;
use postage_server::privy::{PrivyVerifier, system_clock};
use postage_server::world::WorldVerify;
use postage_server::{AppState, router};
use serde_json::json;

use crate::hub::{Hub, now_seconds};
use crate::identity::{MemoryJwks, OWNER, PRIVY_APP_ID, PrivyKey, SENDER};
use crate::site::Site;

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

const WEBHOOK_SECRET: &str = "e2e-webhook-secret";

struct Args {
    db: PathBuf,
    dist: PathBuf,
    repo: PathBuf,
    port: u16,
}

fn parse_args() -> Result<Args, BoxError> {
    let mut values = std::collections::HashMap::new();
    let mut arguments = std::env::args().skip(1);
    while let Some(name) = arguments.next() {
        let value = arguments
            .next()
            .ok_or_else(|| format!("{name} needs a value"))?;
        values.insert(name, value);
    }
    let required = |name: &str| {
        values
            .get(name)
            .map(PathBuf::from)
            .ok_or_else(|| format!("{name} is required"))
    };
    Ok(Args {
        db: required("--db")?,
        dist: required("--dist")?,
        repo: required("--repo")?,
        port: values.get("--port").map_or(Ok(0), |port| port.parse())?,
    })
}

/// A key that is only ever a placeholder for this run: derived from a label,
/// never a secret and never used outside this process.
fn throwaway_key(label: &str) -> String {
    format!(
        "0x{}",
        hex::encode(alloy_primitives::keccak256(label.as_bytes()))
    )
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let args = parse_args()?;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", args.port)).await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let stubs = format!("{origin}/__stub");

    let hub = Arc::new(Hub::default());
    let privy_key = PrivyKey::generate()?;
    let started = i64::try_from(now_seconds())?;
    let config = json!({
        "personas": {
            "owner": persona_json(&privy_key, &OWNER, started),
            "sender": persona_json(&privy_key, &SENDER, started),
        },
        "rpcUrl": format!("{stubs}/rpc/node"),
        "paymentUrl": format!("{stubs}/rpc/payment"),
    });

    let env = Env::fixed([
        ("DATABASE_URL", format!("file:{}", args.db.display())),
        ("IDENTITY_MODE", "mock".to_owned()),
        ("APP_URL", origin.clone()),
        ("NEXT_PUBLIC_PRIVY_APP_ID", PRIVY_APP_ID.to_owned()),
        (
            "MESSAGE_ID_SECRET",
            "e2e-message-id-secret-0123456789abcdef".to_owned(),
        ),
        ("CLASSIFIER_PRIVATE_KEY", throwaway_key("e2e classifier")),
        ("ATTESTER_PRIVATE_KEY", throwaway_key("e2e attester")),
        ("RELAYER_PRIVATE_KEY", throwaway_key("e2e relayer")),
        ("WORLD_RP_SIGNING_KEY", throwaway_key("e2e world rp")),
        ("WORLD_RP_ID", "rp_e2e".to_owned()),
        ("WORLD_ACTION", "send-free".to_owned()),
        ("MAIL_WEBHOOK_SECRET", WEBHOOK_SECRET.to_owned()),
        ("MAIL_WORKER_URL", format!("{stubs}/worker")),
        ("ANTHROPIC_API_KEY", "e2e-anthropic-key".to_owned()),
        ("ANTHROPIC_BASE_URL", format!("{stubs}/anthropic")),
        ("ARC_RPC_URL", format!("{stubs}/rpc/node")),
        ("GRAPH_QUERY_URL", format!("{stubs}/graph/postage")),
        ("GRAPH_API_KEY", "e2e-graph-key".to_owned()),
        ("RESEND_API_KEY", "e2e-resend-key".to_owned()),
        ("MAIL_FROM", "Postage <hello@usepostage.com>".to_owned()),
        ("CLOUDFLARE_ACCOUNT_ID", "e2e-account".to_owned()),
        ("CLOUDFLARE_API_TOKEN", "e2e-cloudflare-token".to_owned()),
    ]);

    let clock_hub = Arc::clone(&hub);
    let system = system_clock();
    let clock: postage_server::privy::Clock = Arc::new(move || system() + clock_hub.clock_offset());
    let state = AppState::builder(env.clone())
        .clock(Arc::clone(&clock))
        .mailer(Mailer::from_env(env.lookup())?.with_endpoint(format!("{stubs}/resend/emails")))
        .cloudflare(
            Cloudflare::from_env(env.lookup())?.with_api_base(format!("{stubs}/cloudflare")),
        )
        .world(WorldVerify::default().with_base(format!("{stubs}/world")))
        .graph(
            postage_server::graph::Graph::from_env(env.lookup())
                .with_gateway_base(format!("{stubs}/graph-gw")),
        )
        .privy(PrivyVerifier::new(
            PRIVY_APP_ID.to_owned(),
            MemoryJwks::serving(&privy_key.jwks()),
            clock,
        ))
        .build();

    let site = Arc::new(Site {
        dist: args.dist,
        repo: args.repo,
        config: config.clone(),
    });
    let app = router(state)
        .merge(hub::router(Arc::clone(&hub)))
        .fallback(move |uri: Uri| site::serve(Arc::clone(&site), uri));

    println!(
        "E2E_READY {}",
        json!({
            "origin": origin,
            "stubs": stubs,
            "db": args.db,
            "webhookSecret": WEBHOOK_SECRET,
            "config": config,
            "classifierAddress": QuoteSigner::from_hex(&throwaway_key("e2e classifier"))
                .map(|signer| signer.address().to_string())
                .unwrap_or_default(),
        })
    );
    axum::serve(listener, app).await?;
    Ok(())
}

fn persona_json(key: &PrivyKey, persona: &identity::Persona, now: i64) -> serde_json::Value {
    json!({
        "userId": persona.user_id,
        "email": persona.email,
        "wallet": persona.wallet,
        "identityToken": key.token_for(persona, now),
    })
}

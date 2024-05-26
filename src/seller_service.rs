use crate::util::healthz;
use crate::{
    buyer,
    provider::Provider,
    util::{env, pubkey_to_string, string_to_pubkey},
};
use actix_web::post;
use actix_web::web::Json;
use actix_web::{App, Error, HttpResponse, HttpServer};
use futures_util::StreamExt;
use log::info;
use serde::{Deserialize, Serialize};
use serde_json::json;
use solana_account_decoder::UiAccountEncoding;
use solana_client::nonblocking::pubsub_client::PubsubClient;
use solana_client::rpc_config::RpcAccountInfoConfig;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::{EncodableKey, Signer};

#[derive(Deserialize, Serialize)]
pub struct SellRequest {
    #[serde(
        serialize_with = "pubkey_to_string",
        deserialize_with = "string_to_pubkey"
    )]
    pub amm_pool: Pubkey,
    #[serde(
        serialize_with = "pubkey_to_string",
        deserialize_with = "string_to_pubkey"
    )]
    pub input_mint: Pubkey,
    #[serde(
        serialize_with = "pubkey_to_string",
        deserialize_with = "string_to_pubkey"
    )]
    pub output_mint: Pubkey,
    pub sol_vault: Pubkey,
    pub sol_pooled_when_bought: f64,
}

#[post("/sell")]
async fn handle_sell(sell_request: Json<SellRequest>) -> Result<HttpResponse, Error> {
    info!(
        "handling sell_request {}",
        serde_json::to_string_pretty(&sell_request)?
    );
    let wallet = Keypair::read_from_file(env("FUND_KEYPAIR_PATH")).expect("read fund keypair");
    let provider = Provider::new(env("RPC_URL"));
    let amount = provider
        .get_spl_balance(&wallet.pubkey(), &sell_request.input_mint)
        .await?;
    let pubsub_client = PubsubClient::new(&env("WS_URL"))
        .await
        .expect("pubsub client async");
    let (mut stream, unsub) = pubsub_client
        .account_subscribe(
            &sell_request.sol_vault,
            Some(RpcAccountInfoConfig {
                commitment: Some(CommitmentConfig::processed()),
                encoding: Some(UiAccountEncoding::Base64),
                ..Default::default()
            }),
        )
        .await
        .expect("subscribe to account");

    while let Some(log) = stream.next().await {
        let sol_pooled = log.value.lamports as f64 / 10u64.pow(9) as f64;
        info!("sol_pooled: {}", sol_pooled);
        if sol_pooled >= sell_request.sol_pooled_when_bought * 1.8
            || sol_pooled <= sell_request.sol_pooled_when_bought * 0.75
        {
            info!("selling");
            break;
        }
    }

    match buyer::buy(
        &sell_request.amm_pool,
        &sell_request.input_mint,
        &sell_request.output_mint,
        amount,
        &wallet,
        &provider,
    )
    .await
    {
        Ok(_) => {
            info!("OK");
            unsub().await;
            Ok(HttpResponse::Ok().json(json!({"status": "OK"})))
        }
        Err(e) => {
            unsub().await;
            Ok(HttpResponse::InternalServerError().body(format!("{}", e)))
        }
    }
}

pub async fn run_seller_service() -> std::io::Result<()> {
    info!("Running seller service on 8081");
    HttpServer::new(move || App::new().service(handle_sell).service(healthz))
        .bind(("0.0.0.0", 8081))?
        .run()
        .await
}

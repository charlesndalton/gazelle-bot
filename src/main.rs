use types::Result;
use std::env;
use std::time::{Duration};
use std::thread;

#[tokio::main]
async fn main() -> Result<()> {
    let telegram_token = env::var("GAZELLE_TELEGRAM_TOKEN").expect("GAZELLE_TELEGRAM_TOKEN not set");
    let infura_api_key = env::var("INFURA_API_KEY").expect("INFURA_API_KEY not set");

    let report = report_creator::create_report(&infura_api_key).await?;
    println!("REPORT: {:?}", report);

    report_publisher::publish_report(report, &telegram_token).await?;

    Ok(())
}


mod report_creator {
    use crate::{
        graph_client,
        types::{Result, Error, BasicAngleReport, CollateralReport}
    };
    use std::fs::File;
    use std::io::prelude::*;
    use serde_json::{Value};
    use ethers::abi::Address;
    use rust_decimal_macros::dec;
    use rust_decimal::Decimal;

    pub async fn create_report(infura_api_key: &str) -> Result<Vec<BasicAngleReport>> {
        let mut report = Vec::new();

        let collateral_data = graph_client::get_collateral_data().await?;
        // hack: unwraps galore
        let pool_datas = collateral_data["data"]["poolDatas"].as_array().unwrap();

        for pool_data in pool_datas {
            let pool_data = pool_data.as_object().unwrap();

            if pool_data["stableName"].as_str().unwrap() != "agEUR" {
                continue; // doesn't matter for now but in case they introduce another stablecoin
            }

            println!("{:?}", pool_data);

            let stock_user: Decimal = pool_data["stockUser"].as_str().unwrap().parse().unwrap();
            let total_hedge_amount: Decimal = pool_data["totalHedgeAmount"].as_str().unwrap().parse().unwrap();
            let hedge_ratio = ((total_hedge_amount / stock_user) * dec!(100)).round();

            report.push(BasicAngleReport::new(pool_data["collatName"].as_str().unwrap().to_string(), hedge_ratio));
        }

        Ok(report) 
    }
}

mod graph_client {
    use crate::types::Result;
    use serde_json::Value;
    // this is very hacky (using string post body and `Value` response instead of using structs &
    // deserializing) but I will hopefully be the only one to ever interact with this bot

    const collateral_data_url: &str = "https://api.thegraph.com/subgraphs/name/picodes/transaction";
    const collateral_data_post_body: &str = "{ \"query\": \"{ poolDatas { stableName, collatName, decimals, stockUser, totalHedgeAmount } }\" }";

    pub async fn get_collateral_data() -> Result<Value> {
        let client = reqwest::Client::new();

        let res = client.post(collateral_data_url)
            .body(collateral_data_post_body)
            .send()
            .await?;

        let res_json: Value = res.json().await?;

        Ok(res_json)
    }
}

mod report_publisher {
    use crate::{
        telegram_client,
        types::{Result, BasicAngleReport}
    };

    pub async fn publish_report(report: Vec<BasicAngleReport>, telegram_token: &str) -> Result<()> {
        for individual_report in report {
            telegram_client::send_message_to_committee(format!("{}'s price risk relative to the euro is hedged away {}%", individual_report.collateral_asset(), individual_report.collateralization_ratio()).as_str(), telegram_token).await?;
        }

        Ok(())
    }
}


mod telegram_client {
    use crate::types::Result;
    use urlencoding::encode;

    const ANGLE_COMMITTEE_TELEGRAM_CHAT_ID: i64 = -1001767497785; 

    pub async fn send_message_to_committee(message: &str, token: &str) -> Result<()> {
        let url = format!("https://api.telegram.org/bot{}/sendMessage?chat_id={}&text={}", token, ANGLE_COMMITTEE_TELEGRAM_CHAT_ID, encode(message));

        reqwest::get(url)
            .await?;

        Ok(())
    }
}

mod types {
    use derive_getters::{Getters};
    use derive_new::new;
    use rust_decimal::Decimal;

    #[derive(Debug, thiserror::Error)]
    pub enum Error {
        #[error(transparent)]
        EthrsContractError(#[from] ethers::contract::ContractError<ethers::providers::Provider<ethers::providers::Http>>),

        #[error(transparent)]
        UrlParseError(#[from] url::ParseError),

        #[error(transparent)]
        AddressParseStringToHexError(#[from] rustc_hex::FromHexError),

        #[error(transparent)]
        EyreReport(#[from] eyre::Report),

        #[error(transparent)]
        IOError(#[from] std::io::Error),

        #[error(transparent)]
        SerdeJsonError(#[from] serde_json::Error),

        #[error(transparent)]
        ReqwestError(#[from] reqwest::Error),

        #[error("We expected to find the following field {expected_field:?} in ethereum.json, but it wasn't there.")]
        EthAddressesError {
            expected_field: String,
        },
    }

    pub type Result<T> = std::result::Result<T, Error>;

    #[derive(Getters, new, Debug)]
    pub struct BasicAngleReport {
        collateral_asset: String,
        collateralization_ratio: Decimal,
    }

    #[derive(Getters, new, Debug)]
    pub struct AngleReport {
        organic_collateralization_ratio: Decimal,
        total_collateralization_ratio: Decimal,
        collateral_reports: Vec<CollateralReport>
    }

    #[derive(Getters, new, Debug)]
    pub struct CollateralReport {
        percentage_hedged: Decimal,
        total_tvl: Decimal
    }

    pub struct TvlSplit {
        tvl_from_users: Decimal,
        tvl_from_hedging_agents: Decimal,
        tvl_from_slps: Decimal
    }

}

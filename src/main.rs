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
        types::{Result, Error, AngleReport, IndividualAssetAngleReport}
    };
    use std::fs::File;
    use std::io::prelude::*;
    use serde_json::{Value};
    use ethers::abi::Address;
    use rust_decimal_macros::dec;
    use rust_decimal::Decimal;
    use num_bigint::BigInt;
    use bigdecimal::{BigDecimal, One};
    use std::str::FromStr;

    pub async fn create_report(infura_api_key: &str) -> Result<AngleReport> {
        let mut collateral_reports = Vec::new();

        let collateral_data = graph_client::get_collateral_data().await?;
        // hack: unwraps galore
        let pool_datas = collateral_data["data"]["poolDatas"].as_array().unwrap();

        for pool_data in pool_datas {
            let pool_data = pool_data.as_object().unwrap();

            if pool_data["stableName"].as_str().unwrap() != "agEUR" {
                continue; // doesn't matter for now but in case they introduce another stablecoin
            }

            println!("{:?}", pool_data);

            let decimals: u32 = pool_data["decimals"].as_str().unwrap().parse().unwrap();
            let slp_divide_by: BigDecimal = (BigInt::one() * 10_u32).pow(18 + decimals).into(); // they format the stockSLP in 18 + decimals
            let stock_slp = BigDecimal::from_str(pool_data["stockSLP"].as_str().unwrap())? / slp_divide_by;

            let stock_user = BigDecimal::from_str(pool_data["stockUser"].as_str().unwrap())? / 10_i64.pow(18); 
            let total_hedge_amount = BigDecimal::from_str(pool_data["totalHedgeAmount"].as_str().unwrap())? / 10_i64.pow(18);
            let hedge_ratio = ((total_hedge_amount / &stock_user) * BigDecimal::from(100));
        
            collateral_reports.push(IndividualAssetAngleReport::new(pool_data["collatName"].as_str().unwrap().to_string(), hedge_ratio.with_scale(0), stock_user.with_scale(0), stock_slp.with_scale(0)));
        }

        Ok(AngleReport::new(BigDecimal::from(1), BigDecimal::from(1), collateral_reports)) 
    }
}

mod graph_client {
    use crate::types::Result;
    use serde_json::Value;
    // this is very hacky (using string post body and `Value` response instead of using structs &
    // deserializing) but I will hopefully be the only one to ever interact with this bot

    const collateral_data_url: &str = "https://api.thegraph.com/subgraphs/name/picodes/transaction";
    const collateral_data_post_body: &str = "{ \"query\": \"{ poolDatas { stableName, collatName, decimals, stockUser, totalHedgeAmount, stockSLP } }\" }";

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
        types::{Result, AngleReport}
    };

    pub async fn publish_report(report: AngleReport, telegram_token: &str) -> Result<()> {
        println!("Daily Angle");
        println!("-----------");
        println!("Total collateralization ratio: {}", report.total_collateralization_ratio());
        println!("Organic collateralization ratio: {}", report.organic_collateralization_ratio());
        for individual_asset_report in report.collateral_reports() {
            println!("-----------");
            println!("Asset – {}", individual_asset_report.collateral_asset());
            println!("Percentage of total TVL: x");
            println!("User TVL: {}", individual_asset_report.user_tvl());
            println!("SLP TVL: {}", individual_asset_report.slp_tvl());
            //telegram_client::send_message_to_committee(format!("{}'s price risk relative to the euro is hedged away {}%", individual_report.collateral_asset(), individual_report.collateralization_ratio()).as_str(), telegram_token).await?;
            //
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
    use bigdecimal::BigDecimal;

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

        #[error(transparent)]
        ParseBigIntError(#[from] num_bigint::ParseBigIntError),

        #[error(transparent)]
        ParseBigDecimalError(#[from] bigdecimal::ParseBigDecimalError),

        #[error("We expected to find the following field {expected_field:?} in ethereum.json, but it wasn't there.")]
        EthAddressesError {
            expected_field: String,
        },
    }

    pub type Result<T> = std::result::Result<T, Error>;

    #[derive(Getters, new, Debug)]
    pub struct IndividualAssetAngleReport {
        collateral_asset: String,
        hedge_ratio: BigDecimal,
        user_tvl: BigDecimal,
        slp_tvl: BigDecimal,
    }

    #[derive(Getters, new, Debug)]
    pub struct AngleReport {
        organic_collateralization_ratio: BigDecimal,
        total_collateralization_ratio: BigDecimal,
        collateral_reports: Vec<IndividualAssetAngleReport>
    }

}

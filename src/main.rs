use types::Result;
use std::env;
use std::time::{Duration};
use std::thread;

#[tokio::main]
async fn main() -> Result<()> {
    let telegram_token = env::var("GAZELLE_TELEGRAM_TOKEN").expect("GAZELLE_TELEGRAM_TOKEN not set");
    let infura_api_key = env::var("INFURA_API_KEY").expect("INFURA_API_KEY not set");

    let report = report_creator::create_report(&infura_api_key).await?;
    report_publisher::publish_report(report, &telegram_token).await?;

    Ok(())
}


mod report_creator {
    use crate::{
        exchange_rate_api_client,
        graph_client,
        types::{Result, Error, AngleStablecoinReport, CollateralReport}
    };
    use serde_json::{Value};
    use num_bigint::BigInt;
    use bigdecimal::{BigDecimal, One};
    use std::str::FromStr;

    pub async fn create_report(infura_api_key: &str) -> Result<AngleStablecoinReport> {
        let mut collateral_reports = Vec::new();

        let stablecoin_data = graph_client::get_stablecoin_data().await?;
        // hack: unwraps galore
        println!("{:?}", stablecoin_data);
        let ag_eur_data = &stablecoin_data["data"]["stableDatas"].as_array().unwrap()[0];
        let total_minted = BigDecimal::from_str(ag_eur_data["totalMinted"].as_str().unwrap())?;
        //let total_minted_value = BigDecimal::from(1);
        //let total_minted = BigDecimal::from(1);
        let total_minted_value = exchange_rate_api_client::convert_eur_to_usd(&total_minted).await?;

        let collateral_datas = ag_eur_data["collaterals"].as_array().unwrap();

        for collateral_data in collateral_datas {
            let collateral_data = collateral_data.as_object().unwrap();


            println!("{:?}", collateral_data);

            let decimals: u32 = collateral_data["decimals"].as_str().unwrap().parse().unwrap();
            let slp_divide_by: BigDecimal = (BigInt::one() * 10_u32).pow(18 + decimals).into(); // they format the stockSLP in 18 + decimals
            let stock_slp = BigDecimal::from_str(collateral_data["stockSLP"].as_str().unwrap())? / slp_divide_by;

            let stock_user = BigDecimal::from_str(collateral_data["stockUser"].as_str().unwrap())? / 10_i64.pow(18); 
            let total_hedge_amount = BigDecimal::from_str(collateral_data["totalHedgeAmount"].as_str().unwrap())? / 10_i64.pow(18);
            let hedge_ratio = ((total_hedge_amount / &stock_user) * BigDecimal::from(100));
        
            collateral_reports.push(CollateralReport::new(collateral_data["collatName"].as_str().unwrap().to_string(), hedge_ratio.with_scale(0), stock_user.with_scale(0), stock_slp.with_scale(0)));
        }

        Ok(AngleStablecoinReport::new(total_minted.with_scale(0), total_minted_value.with_scale(0), BigDecimal::from(1), BigDecimal::from(1), collateral_reports)) 
    }
}

mod graph_client {
    use crate::types::Result;
    use serde_json::Value;
    // this is very hacky (using string post body and `Value` response instead of using structs &
    // deserializing) but I will hopefully be the only one to ever interact with this bot

    const URL: &str = "https://api.thegraph.com/subgraphs/name/picodes/transaction";
    const COLLATERAL_DATA_POST_BODY: &str = "{ \"query\": \"{ poolDatas { stableName, collatName, decimals, stockUser, totalHedgeAmount, stockSLP } }\" }";
    const STABLECOIN_DATA_POST_BODY: &str = "{ \"query\": \"{ stableDatas { name, totalMinted, collatRatio, collaterals { collatName, decimals, stockSLP, stockUser, totalHedgeAmount } } }\" }";

    //pub async fn get_collateral_data() -> Result<Value> {
    //    let client = reqwest::Client::new();

    //    let res = client.post(COLLATERAL_DATA_URL)
    //        .body(COLLATERAL_DATA_POST_BODY)
    //        .send()
    //        .await?;

    //    let res_json: Value = res.json().await?;

    //    Ok(res_json)
    //}

    pub async fn get_stablecoin_data() -> Result<Value> {
        let client = reqwest::Client::new();

        let response = client.post(URL)
            .body(STABLECOIN_DATA_POST_BODY)
            .send()
            .await?;

        let response_json: Value = response.json().await?;

        Ok(response_json)
    }
}


mod report_publisher {
    use crate::{
        telegram_client,
        types::{Result, AngleStablecoinReport}
    };

    pub async fn publish_report(report: AngleStablecoinReport, telegram_token: &str) -> Result<()> {
        println!("Daily Angle");
        println!("-----------");
        println!("Total agEUR minted: {} (${})", report.total_minted(), report.total_minted_value());
        println!("Total collateralization ratio: {}", report.total_collateralization_ratio());
        println!("Organic collateralization ratio: {}", report.organic_collateralization_ratio());
        for collateral_report in report.collateral_reports() {
            println!("-----------");
            println!("Asset – {}", collateral_report.asset_name());
            println!("Percentage of volatility hedged: {}", collateral_report.hedge_ratio());
            println!("Percentage of total TVL: x");
            println!("User TVL: {}", collateral_report.user_tvl());
            println!("SLP TVL: {}", collateral_report.slp_tvl());
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

mod exchange_rate_api_client {
    use crate::types::Result;
    use bigdecimal::BigDecimal;
    use std::str::FromStr;
    use serde_json::{Value};

    pub async fn convert_eur_to_usd(amount: &BigDecimal) -> Result<BigDecimal> {
        let url = format!("https://api.apilayer.com/exchangerates_data/convert?to=USD&from=EUR&amount={}", amount);

        let client = reqwest::Client::new();
        let response = client
            .get(url)
            .header("apikey", "3ZjsVwEmO2v4cOOLl87N8VVev5yeD5GK")
            .send()
            .await?;

        let response_json: Value = response.json().await?;
        println!("{:?}", response_json);

        Ok(BigDecimal::from(response_json["result"].as_i64().unwrap()))
    }
}
mod types {
    use derive_getters::{Getters};
    use derive_new::new;
    use bigdecimal::BigDecimal;

    #[derive(Debug, thiserror::Error)]
    pub enum Error {
        #[error(transparent)]
        UrlParse(#[from] url::ParseError),

        #[error(transparent)]
        SerdeJson(#[from] serde_json::Error),

        #[error(transparent)]
        Reqwest(#[from] reqwest::Error),

        #[error(transparent)]
        ParseBigInt(#[from] num_bigint::ParseBigIntError),

        #[error(transparent)]
        ParseBigDecimal(#[from] bigdecimal::ParseBigDecimalError),
    }

    pub type Result<T> = std::result::Result<T, Error>;

    #[derive(Getters, new, Debug)]
    pub struct CollateralReport { 
        asset_name: String,
        hedge_ratio: BigDecimal,
        user_tvl: BigDecimal,
        slp_tvl: BigDecimal,
    }

    #[derive(Getters, new, Debug)]
    pub struct AngleStablecoinReport {
        total_minted: BigDecimal,
        total_minted_value: BigDecimal,
        organic_collateralization_ratio: BigDecimal,
        total_collateralization_ratio: BigDecimal,
        collateral_reports: Vec<CollateralReport>,
    }

}

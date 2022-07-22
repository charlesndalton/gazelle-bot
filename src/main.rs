use log::{error, info};
use std::env;
use types::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let telegram_token =
        env::var("GAZELLE_TELEGRAM_TOKEN").expect("GAZELLE_TELEGRAM_TOKEN not set");
    let infura_api_key = env::var("INFURA_API_KEY").expect("INFURA_API_KEY not set");

    env_logger::init();

    info!("=============== GAZELLE STARTING ===============");

    let report = match report_creator::create_report(&infura_api_key).await {
        Ok(report) => report,
        Err(e) => {
            error!(
                "Report could not be created, with the following error: {}",
                e
            );
            panic!();
        }
    };

    info!("Report: {:?}", report);

    match report_publisher::publish_report(report, &telegram_token).await {
        Err(e) => error!(
            "Report could not be published, with the following error: {}",
            e
        ),
        Ok(_) => (),
    };

    Ok(())
}

mod report_creator {
    use crate::{
        cryptocurrency_prices_api_client, exchange_rate_api_client, graph_client,
        types::{
            AngleStablecoinReport, CollateralReport,
            Error::{self, MissingField},
            Result,
        },
    };
    use bigdecimal::{BigDecimal, One};
    use num_bigint::BigInt;
    use serde_json::Value;
    use std::str::FromStr;

    pub async fn create_report(_infura_api_key: &str) -> Result<AngleStablecoinReport> {
        let mut collateral_reports = Vec::new();

        let stablecoin_data = graph_client::get_stablecoin_data().await?;
        // hack: unwraps galore
        println!("{:?}", stablecoin_data);
        let ag_eur_data =
            &stablecoin_data["data"]["stableDatas"]
                .as_array()
                .ok_or(MissingField {
                    expected_field: "stableDatas".to_string(),
                })?[0];
        let total_minted =
            (BigDecimal::from_str(ag_eur_data["totalMinted"].as_str().ok_or(MissingField {
                expected_field: "totalMinted".to_string(),
            })?)?
                / 10_i64.pow(18))
            .with_scale(0);
        let exchange_rate = exchange_rate_api_client::get_eur_usd_exchange_rate().await?;
        let total_minted_value = &total_minted * exchange_rate;
        let mut total_collateral_value = BigDecimal::from(0);
        let mut organic_collateral_value = BigDecimal::from(0); // not incl. SLP deposits

        let collateral_datas = ag_eur_data["collaterals"].as_array().ok_or(MissingField {
            expected_field: "collaterals".to_string(),
        })?;

        for collateral_data in collateral_datas {
            let collateral_data = collateral_data.as_object().ok_or(MissingField {
                expected_field: "collaterals[x]".to_string(),
            })?;

            let decimals: u32 = collateral_data["decimals"]
                .as_str()
                .ok_or(MissingField {
                    expected_field: "decimals".to_string(),
                })?
                .parse()?;
            let slp_divide_by: BigDecimal = (BigInt::one() * 10_u32).pow(18 + decimals).into(); // they format the stockSLP in 18 + decimals
            let stock_slp = (BigDecimal::from_str(collateral_data["stockSLP"].as_str().ok_or(
                MissingField {
                    expected_field: "stockSLP".to_string(),
                },
            )?)? / slp_divide_by)
                .with_scale(0);

            let stock_user = (BigDecimal::from_str(collateral_data["stockUser"].as_str().ok_or(
                MissingField {
                    expected_field: "stockUser".to_string(),
                },
            )?)? / 10_i64.pow(18))
            .with_scale(0);
            let total_hedge_amount =
                extract_field_as_decimal(&collateral_data, "totalHedgeAmount")? / 10_i64.pow(18);
            let hedge_ratio = ((total_hedge_amount / &stock_user) * BigDecimal::from(100));
            let total_margin =
                extract_field_as_decimal(collateral_data, "totalMargin")? / 10_i64.pow(decimals);

            let collateral_symbol = collateral_data["collatName"].as_str().ok_or(MissingField {
                expected_field: "collatName".to_string(),
            })?;
            let price = cryptocurrency_prices_api_client::get_usd_price(collateral_symbol).await?;
            let total_assets = BigDecimal::from_str(
                collateral_data["totalAsset"].as_str().ok_or(MissingField {
                    expected_field: "totalAsset".to_string(),
                })?,
            )? / 10_i64.pow(decimals);
            let total_assets_value = &total_assets * &price;
            let stock_slp_value = &stock_slp * &price;

            let organic_assets = if &total_assets > &(&stock_slp + &total_margin) {
                &total_assets - (&stock_slp + &total_margin)
            } else {
                BigDecimal::from(0)
            };
            let organic_assets_value = &organic_assets * &price;

            organic_collateral_value += &organic_assets_value;
            total_collateral_value += &total_assets_value;

            collateral_reports.push(CollateralReport::new(
                collateral_symbol.to_string(),
                hedge_ratio.with_scale(0),
                organic_assets.with_scale(0),
                organic_assets_value.with_scale(0),
                stock_slp.with_scale(0),
                stock_slp_value.with_scale(0),
                total_assets.with_scale(0),
                total_assets_value.with_scale(0),
            ));
        }

        let organic_collateralization_ratio = &organic_collateral_value / &total_minted_value;
        let total_collateralization_ratio = &total_collateral_value / &total_minted_value;

        Ok(AngleStablecoinReport::new(
            total_minted.with_scale(0),
            total_minted_value.with_scale(0),
            organic_collateral_value.with_scale(0),
            total_collateral_value.with_scale(0),
            organic_collateralization_ratio.with_scale(2),
            total_collateralization_ratio.with_scale(2),
            collateral_reports,
        ))
    }

    fn extract_field_as_decimal(
        object: &serde_json::Map<String, Value>,
        field_name: &str,
    ) -> Result<BigDecimal> {
        Ok(
            BigDecimal::from_str(object[field_name].as_str().ok_or(MissingField {
                expected_field: field_name.to_string(),
            })?)?
            .with_scale(0),
        )
    }
}

mod graph_client {
    use crate::types::Result;
    use serde_json::Value;

    const URL: &str = "https://api.thegraph.com/subgraphs/name/picodes/transaction";
    // this is very hacky (using string post body and `Value` response instead of using structs &
    // deserializing) but I will hopefully be the only one to ever interact with this bot
    const STABLECOIN_DATA_POST_BODY: &str = "{ \"query\": \"{ stableDatas { name, totalMinted, collatRatio, collaterals { collatName, decimals, stockSLP, stockUser, totalAsset, totalHedgeAmount, totalMargin } } }\" }";

    pub async fn get_stablecoin_data() -> Result<Value> {
        let client = reqwest::Client::new();

        let response = client
            .post(URL)
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
        types::{AngleStablecoinReport, Result},
    };
    use bigdecimal::BigDecimal;
    use bigdecimal::{FromPrimitive, ToPrimitive};
    use num_format::{Locale, ToFormattedString};

    fn format(num: &BigDecimal) -> String {
        num.to_u128().unwrap().to_formatted_string(&Locale::en)
    }

    pub async fn publish_report(report: AngleStablecoinReport, telegram_token: &str) -> Result<()> {
        println!("REPORT : {:?}", report);
        let mut report_formatted = vec![
            format!("Daily Angle Report"),
            format!("-----------"),
            format!(
                "Total agEUR minted: {} (${})",
                format(report.total_minted()),
                format(report.total_minted_value())
            ),
            format!(
                "Total collateralization ratio: {}",
                report.total_collateralization_ratio()
            ),
            format!(
                "Organic collateralization ratio: {}",
                report.organic_collateralization_ratio()
            ),
        ];
        for collateral_report in report.collateral_reports() {
            report_formatted.push("-----------".to_string());
            report_formatted.push(format!("Asset – {}", collateral_report.asset_name()));
            report_formatted.push(format!(
                "Percentage of volatility hedged: {}%",
                collateral_report.hedge_ratio()
            ));
            report_formatted.push(format!(
                "Percentage of organic TVL: {}%",
                ((collateral_report.organic_tvl_value() / report.organic_tvl())
                    * BigDecimal::from(100))
                .with_scale(0)
            ));
        }

        let report_for_telegram = report_formatted.join("\n");

        println!("{:?}", report_for_telegram);
        println!("{:?}", telegram_token);
        telegram_client::send_message_to_committee(&report_for_telegram, telegram_token).await?;

        Ok(())
    }
}

mod telegram_client {
    use crate::types::Result;
    use urlencoding::encode;

    const ANGLE_COMMITTEE_TELEGRAM_CHAT_ID: i64 = -1001767497785;

    pub async fn send_message_to_committee(message: &str, token: &str) -> Result<()> {
        let url = format!(
            "https://api.telegram.org/bot{}/sendMessage?chat_id={}&text={}",
            token,
            ANGLE_COMMITTEE_TELEGRAM_CHAT_ID,
            encode(message)
        );

        reqwest::get(url).await?;

        Ok(())
    }
}

mod cryptocurrency_prices_api_client {
    use crate::types::{Error, Result};
    use backoff::{future::retry, ExponentialBackoff};
    use bigdecimal::BigDecimal;
    use serde_json::Value;
    use std::str::FromStr;

    // How many USD is one EUR worth?
    pub async fn get_usd_price(symbol: &str) -> Result<BigDecimal> {
        let url = format!(
            "https://pro-api.coinmarketcap.com/v2/cryptocurrency/quotes/latest?symbol={}",
            symbol
        );
        Ok(retry(ExponentialBackoff::default(), || async {
            let client = reqwest::Client::new();

            let response = client
                .get(&url)
                .header("X-CMC_PRO_API_KEY", "61e35d42-f985-45d5-880d-b93a5f76164b")
                .send()
                .await?;
            let response: Value = response.json().await?;
            //println!("{:?}", response);

            let usd_price_raw = response["data"][symbol].as_array().unwrap()[0]["quote"]["USD"]
                ["price"]
                .as_f64()
                .unwrap();
            let usd_price = BigDecimal::from((usd_price_raw * 1_000.0).round() as i64) / 1_000;
            println!("price: {}", usd_price);

            Ok(usd_price)
        })
        .await?)
    }
}

mod exchange_rate_api_client {
    use crate::types::Result;
    use bigdecimal::BigDecimal;
    use serde_json::Value;
    use std::str::FromStr;

    const EXCHANGE_RATE_URL: &str =
        "https://api.apilayer.com/exchangerates_data/convert?to=USD&from=EUR&amount=1";

    // How many USD is one EUR worth?
    pub async fn get_eur_usd_exchange_rate() -> Result<BigDecimal> {
        let client = reqwest::Client::new();
        let response = client
            .get(EXCHANGE_RATE_URL)
            .header("apikey", "3ZjsVwEmO2v4cOOLl87N8VVev5yeD5GK")
            .send()
            .await?;
        let response: Value = response.json().await?;

        // hack: From<f64> not implemented for BigDecimal
        let exchange_rate_raw = response["result"].as_f64().unwrap();
        let exchange_rate = BigDecimal::from((exchange_rate_raw * 1_000.0).round() as i64) / 1_000;
        println!("er: {}", exchange_rate);

        Ok(exchange_rate)
    }
}

mod types {
    use bigdecimal::BigDecimal;
    use derive_getters::Getters;
    use derive_new::new;

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
        ParseInt(#[from] std::num::ParseIntError),

        #[error(transparent)]
        ParseBigDecimal(#[from] bigdecimal::ParseBigDecimalError),

        #[error("Can't deserialize the following field {expected_field:?}")]
        MissingField { expected_field: String },
    }

    pub type Result<T> = std::result::Result<T, Error>;

    #[derive(Getters, new, Debug)]
    pub struct CollateralReport {
        asset_name: String,
        hedge_ratio: BigDecimal,
        organic_tvl: BigDecimal,
        organic_tvl_value: BigDecimal,
        slp_tvl: BigDecimal,
        slp_tvl_value: BigDecimal,
        total_tvl: BigDecimal,
        total_tvl_value: BigDecimal,
    }

    #[derive(Getters, new, Debug)]
    pub struct AngleStablecoinReport {
        total_minted: BigDecimal,
        total_minted_value: BigDecimal,
        organic_tvl: BigDecimal,
        total_tvl: BigDecimal,
        organic_collateralization_ratio: BigDecimal,
        total_collateralization_ratio: BigDecimal,
        collateral_reports: Vec<CollateralReport>,
    }
}

use types::Result;
use std::env;
use std::fs::File;
use std::io::prelude::*;
use serde_json::{Value};

#[tokio::main]
async fn main() -> Result<()> {
    let telegram_token = env::var("DIK_DIK_TELEGRAM_TOKEN").expect("DIK_DIK_TELEGRAM_TOKEN not set");
    let infura_api_key = env::var("INFURA_API_KEY").expect("INFURA_API_KEY not set");

    let mut file = File::open("./src/contract-address-registry/ethereum.json")?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    let v: Value = serde_json::from_str(&contents)?;

    let client = blockchain_client::create_client(&infura_api_key)?;

    let mut committee_report = String::from("[DAILY TOKEMAK REPORT ☢️]:
");

    for (asset, strategy_address) in v["strategies"]["tokemak"].as_object().unwrap().iter() {
        let t_asset_balance = blockchain_client::get_t_asset_balance(&client, &strategy_address.as_str().unwrap()).await?;
        let want_balance_in_pool = blockchain_client::get_liquid_want_in_pool(&client, &strategy_address.as_str().unwrap()).await?;
        let want_balance_in_manager = blockchain_client::get_liquid_want_in_manager(&client, &strategy_address.as_str().unwrap()).await?;
        let tokemak_liquid_assets = want_balance_in_pool + want_balance_in_manager;
        let want_balance_in_manager_uni_lp_tokens = blockchain_client::get_want_in_univ2_pools(&client, &strategy_address.as_str().unwrap()).await?;
        let want_balance_in_manager_curve_lp_tokens = blockchain_client::get_want_in_curve_pools(&client, &strategy_address.as_str().unwrap()).await?;
        let tokemak_all_assets = tokemak_liquid_assets + want_balance_in_manager_uni_lp_tokens + want_balance_in_manager_curve_lp_tokens;
        let t_asset_total_supply = blockchain_client::get_t_asset_total_supply(&client, &strategy_address.as_str().unwrap()).await?;
        let mut health_ratio = tokemak_all_assets / t_asset_total_supply;
        health_ratio.rescale(2);

        committee_report.push_str(&format!(
"Report for {}
 - amount of t{} we own: {}
 - health ratio (A/L): {}
 - tokemak free want: {}
 - tokemak uni LP want: {}
 - tokemak curve LP want: {}
 - tokemak total want: {}
 - tokemak liabilities: {}
", asset, asset, t_asset_balance, health_ratio, tokemak_liquid_assets, want_balance_in_manager_uni_lp_tokens, want_balance_in_manager_curve_lp_tokens, tokemak_all_assets, t_asset_total_supply));
    }

    print!("{}", committee_report);
    //telegram_client::send_message_to_committee(&committee_report, &telegram_token).await?;

    Ok(())
}

async fn daily_check() -> Result<()> {
    Ok(())
}

mod telegram_client {
    use crate::types::Result;
    use urlencoding::encode;

    const TOKEMAK_COMMITTEE_TELEGRAM_CHAT_ID: i64 = -1001175962929;

    pub async fn send_message_to_committee(message: &str, token: &str) -> Result<()> {
        let url = format!("https://api.telegram.org/bot{}/sendMessage?chat_id={}&text={}", token, TOKEMAK_COMMITTEE_TELEGRAM_CHAT_ID, encode(message));

        reqwest::get(url)
            .await?;

        Ok(())
    }
}

mod blockchain_client {
    use ethers::prelude::*;
    use std::sync::Arc;
    use crate::types::{Result, CurvePool as CurvePoolStruct};
    use rust_decimal::prelude::*;
    use rust_decimal_macros::dec;
    abigen!(
        IERC20,
        "./src/abis/IERC20.json",
        event_derives(serde::Deserialize, serde::Serialize)
    );

    abigen!(
        TokemakStrategy,
        "./src/abis/TokemakStrategy.json",
        event_derives(serde::Deserialize, serde::Serialize)
    );

    abigen!(
        CurvePool,
        "./src/abis/CurvePool.json",
        event_derives(serde::Deserialize, serde::Serialize)
    );    

    pub type Client = Arc<Provider::<Http>>;

    pub fn create_client(infura_api_key: &str) -> Result<Client> {
        let infura_url = format!("https://mainnet.infura.io/v3/{}", infura_api_key);
        let client = Provider::<Http>::try_from(infura_url)?;
        Ok(Arc::new(client))
    }

    pub async fn get_t_asset_balance(client: &Client, strategy_address: &str) -> Result<Decimal> {
        let strategy_address = strategy_address.parse::<Address>()?;
        let t_asset_address = get_t_asset_address(client, strategy_address).await?;
        let t_asset = IERC20::new(t_asset_address, Arc::clone(&client));
        let decimals = get_t_asset_decimals(&t_asset).await?;

        let mut t_asset_balance = Decimal::from_i128_with_scale(t_asset.balance_of(strategy_address).call().await?.as_u128().try_into().unwrap(), decimals);
        t_asset_balance.rescale(6);

        Ok(t_asset_balance)
    }

    pub async fn get_t_asset_total_supply(client: &Client, strategy_address: &str) -> Result<Decimal> {
        let strategy_address = strategy_address.parse::<Address>()?;
        let t_asset_address = get_t_asset_address(client, strategy_address).await?;
        let t_asset = IERC20::new(t_asset_address, Arc::clone(&client));
        let decimals = get_t_asset_decimals(&t_asset).await?;

        let mut t_asset_total_supply = Decimal::from_i128_with_scale(t_asset.total_supply().call().await?.as_u128().try_into().unwrap(), decimals);
        t_asset_total_supply.rescale(6);

        Ok(t_asset_total_supply)
    }
        
    pub async fn get_liquid_want_in_pool(client: &Client, strategy_address: &str) -> Result<Decimal> {
        let strategy_address = strategy_address.parse::<Address>()?;
        let t_asset_address = get_t_asset_address(client, strategy_address).await?;

        let want_address = TokemakStrategy::new(strategy_address, Arc::clone(&client)).want().call().await?;
        let want = IERC20::new(want_address, Arc::clone(&client));

        let want_decimals = want.decimals().call().await?.into();
        let mut want_pool_balance = Decimal::from_i128_with_scale(want.balance_of(t_asset_address).call().await?.as_u128().try_into().unwrap(), want_decimals); // t asset address is same as liquidity pool address
        want_pool_balance.rescale(6);

        Ok(want_pool_balance)
    }

    pub async fn get_liquid_want_in_manager(client: &Client, strategy_address: &str) -> Result<Decimal> {
        let manager_address = "0xA86e412109f77c45a3BC1c5870b880492Fb86A14".parse::<Address>()?;
        let strategy_address = strategy_address.parse::<Address>()?;
        let want_address = TokemakStrategy::new(strategy_address, Arc::clone(&client)).want().call().await?;
        let want = IERC20::new(want_address, Arc::clone(&client));
        let want_decimals = want.decimals().call().await?.into();
        let mut want_manager_balance = Decimal::from_i128_with_scale(want.balance_of(manager_address).call().await?.as_u128().try_into().unwrap(), want_decimals);
        want_manager_balance.rescale(6);

        Ok(want_manager_balance)
    }

    pub async fn get_want_in_univ2_pools(client: &Client, strategy_address: &str) -> Result<Decimal> {
        let univ2_pool_addresses = vec!["0x61eB53ee427aB4E007d78A9134AaCb3101A2DC23", "0x470e8de2eBaef52014A47Cb5E6aF86884947F08c", "0x43AE24960e5534731Fc831386c07755A2dc33D47", "0xecBa967D84fCF0405F6b32Bc45F4d36BfDBB2E81", "0xe55c3e83852429334A986B265d03b879a3d188Ac", "0xdC08159A6C82611aEB347BA897d82AC1b80D9419", "0xAd5B1a6ABc1C9598C044cea295488433a3499eFc"];
        let manager_address = "0xA86e412109f77c45a3BC1c5870b880492Fb86A14".parse::<Address>()?;
        let strategy_address = strategy_address.parse::<Address>()?;
        let want_address = TokemakStrategy::new(strategy_address, Arc::clone(&client)).want().call().await?;
        let want = IERC20::new(want_address, Arc::clone(&client));
        let want_decimals = want.decimals().call().await?.into();
        let mut want_owned_in_pools = Decimal::new(0, 6);

        for univ2_pool_address in univ2_pool_addresses {
            let univ2_pool_address = univ2_pool_address.parse::<Address>()?;
            let univ2_lp_token = IERC20::new(univ2_pool_address, Arc::clone(&client));

            let want_in_pool = Decimal::from_i128_with_scale(want.balance_of(univ2_pool_address).call().await?.as_u128().try_into().unwrap(), want_decimals);
            if want_in_pool == dec!(0) {
                continue;
            }
            let owned_lp_tokens = Decimal::from_i128_with_scale(univ2_lp_token.balance_of(manager_address).call().await?.as_u128().try_into().unwrap(), 18);
            let lp_token_total_supply = Decimal::from_i128_with_scale(univ2_lp_token.total_supply().call().await?.as_u128().try_into().unwrap(), 18);
            let share_of_pool = owned_lp_tokens / lp_token_total_supply;
            want_owned_in_pools += want_in_pool * share_of_pool;
        }
        want_owned_in_pools.rescale(6);
        Ok(want_owned_in_pools)
    }
    

    pub async fn get_want_in_curve_pools(client: &Client, strategy_address: &str) -> Result<Decimal> {
        let curve_pools = vec![CurvePoolStruct::new("0xd632f22692FaC7611d2AA1C0D552930D43CAEd3B".to_string(), "0xd632f22692FaC7611d2AA1C0D552930D43CAEd3B".to_string(), Some("0xB900EF131301B307dB5eFcbed9DBb50A3e209B2e".to_string())), CurvePoolStruct::new("0x0437ac6109e8A366A1F4816edF312A36952DB856".to_string(), "0x0437ac6109e8A366A1F4816edF312A36952DB856".to_string(), None), CurvePoolStruct::new("0x50B0D9171160d6EB8Aa39E090Da51E7e078E81c4".to_string(), "0x50B0D9171160d6EB8Aa39E090Da51E7e078E81c4".to_string(), None), CurvePoolStruct::new("0xEd279fDD11cA84bEef15AF5D39BB4d4bEE23F0cA".to_string(), "0xEd279fDD11cA84bEef15AF5D39BB4d4bEE23F0cA".to_string(), Some("0x2ad92A7aE036a038ff02B96c88de868ddf3f8190".to_string()))];
        let manager_address = "0xA86e412109f77c45a3BC1c5870b880492Fb86A14".parse::<Address>()?;
        let coordinator_address = "0x90b6C61B102eA260131aB48377E143D6EB3A9d4B".parse::<Address>()?;
        let strategy_address = strategy_address.parse::<Address>()?;
        let want_address = TokemakStrategy::new(strategy_address, Arc::clone(&client)).want().call().await?;
        let want = IERC20::new(want_address, Arc::clone(&client));
        let mut want_owned_in_pools = Decimal::new(0, 6);

        for curve_pool in curve_pools {
            let pool_address = curve_pool.pool_address().parse::<Address>()?;
            let token_address = curve_pool.token_address().parse::<Address>()?;

            let pool = CurvePool::new(pool_address, Arc::clone(&client));
            let lp_token = IERC20::new(token_address, Arc::clone(&client));
            let lp_token_name: String = lp_token.name().call().await?.into();
            let lp_token_symbol: String = lp_token.symbol().call().await?.into();
            let want_symbol: String = want.symbol().call().await?.into();

            if lp_token_name.as_str().contains(want_symbol.as_str()) || lp_token_symbol.as_str().contains(want_symbol.as_str()) {
                let mut owned_lp_tokens = Decimal::from_i128_with_scale(lp_token.balance_of(manager_address).call().await?.as_u128().try_into().unwrap(), 18);
                owned_lp_tokens += Decimal::from_i128_with_scale(lp_token.balance_of(coordinator_address).call().await?.as_u128().try_into().unwrap(), 18);
                if curve_pool.convex_rewards_pool_address().is_some() {
                    let convex_rewards_pool_address = curve_pool.convex_rewards_pool_address().as_ref().unwrap().parse::<Address>()?;
                    let rewards = IERC20::new(convex_rewards_pool_address, Arc::clone(&client));
                    owned_lp_tokens += Decimal::from_i128_with_scale(rewards.balance_of(manager_address).call().await?.as_u128().try_into().unwrap(), 18);
                    owned_lp_tokens += Decimal::from_i128_with_scale(rewards.balance_of(coordinator_address).call().await?.as_u128().try_into().unwrap(), 18);
                }
                let virtual_price = Decimal::from_i128_with_scale(pool.get_virtual_price().call().await?.as_u128().try_into().unwrap(), 18);

                want_owned_in_pools += owned_lp_tokens * virtual_price;
            }
        }
        want_owned_in_pools.rescale(6);

        Ok(want_owned_in_pools)
    }


    async fn get_t_asset_decimals(t_asset: &IERC20<Provider::<Http>>) -> Result<u32> {
        let decimals: u32 = t_asset.decimals().call().await?.into();
        Ok(decimals)
    }

    async fn get_t_asset_address(client: &Client, strategy_address: Address) -> Result<Address> {
        let weth_strategy_address = "0x2EFB43C8C9AFe71d98B3093C3FD4dEB7Ce543C6D".parse::<Address>()?;

        if strategy_address == weth_strategy_address {
            Ok("0xD3D13a578a53685B4ac36A1Bab31912D2B2A2F36".parse::<Address>()?)
        } else {
            Ok(TokemakStrategy::new(strategy_address, Arc::clone(&client)).t_asset().call().await?)
        }
    }
}

mod types {
    use derive_getters::{Getters};
    use derive_new::new;

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
    }

    pub type Result<T> = std::result::Result<T, Error>;

    #[derive(Getters, new)]
    pub struct CurvePool {
        pool_address: String,
        token_address: String,
        convex_rewards_pool_address: Option<String>,
    }
}

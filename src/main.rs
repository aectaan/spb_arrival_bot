mod gtfs;
mod saved_routes_db;
mod tg_bot;

use crate::gtfs::StaticFeed;
use lazy_static::lazy_static;
use tokio::sync::RwLock;

lazy_static! {
    static ref STATIC_FEED: RwLock<StaticFeed> = RwLock::new(StaticFeed::default());
}

#[tokio::main]
async fn main() {
    let log_config = include_str!("log_config.yaml");
    let config = serde_yaml::from_str(log_config).unwrap();
    log4rs::init_raw_config(config).unwrap();
    log::warn!("Startup");

    {
        let mut feed = STATIC_FEED.write().await;
        *feed = gtfs::static_feed().await.unwrap();
        log::warn!("Feed updated at startup");
    }
    tg_bot::bot().await;
}

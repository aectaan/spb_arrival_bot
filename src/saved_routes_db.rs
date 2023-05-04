use crate::tg_bot::{SavedRouteData, SavedRouteName, SavedRoutes};
use anyhow::{Ok, Result};
use teloxide::types::ChatId;

pub trait SavedRoutesDb {
    fn get_saved_routes(&self) -> Result<SavedRoutes>;
    fn add_route_to_saved(&mut self, name: SavedRouteName, data: SavedRouteData) -> Result<()>;
    fn remove_route_from_saved(&mut self, name: &SavedRouteName) -> Result<()>;
}

impl SavedRoutesDb for ChatId {
    fn get_saved_routes(&self) -> Result<SavedRoutes> {
        log::warn!("Getting saved routes for chat ID {}", self.0);
        let db = sled::Config::new()
            .path("db/saved_routes")
            .cache_capacity(100_000_000)
            .open()?;
        if let Some(ivec) = db.get(bincode::serialize(&self.0)?)? {
            let routes = bincode::deserialize::<SavedRoutes>(&ivec)?;
            log::warn!("Saved routes: {:#?}", routes);
            return Ok(routes);
        }

        log::warn!("No saved routes yet");
        Ok(SavedRoutes::new())
    }

    fn add_route_to_saved(&mut self, name: SavedRouteName, data: SavedRouteData) -> Result<()> {
        log::warn!(
            "Add new saved route for chat ID {}\r\nName: {}, Data: {:#?}",
            self.0,
            name,
            data
        );
        let db = sled::Config::new()
            .path("db/saved_routes")
            .cache_capacity(100_000_000)
            .open()?;
        let key = bincode::serialize(&self.0)?;
        let mut routes;
        if let Some(ivec) = db.get(&key)? {
            routes = bincode::deserialize::<SavedRoutes>(&ivec)?;
        } else {
            routes = SavedRoutes::new();
        }
        routes.insert(name, data);
        db.insert(key, bincode::serialize(&routes)?)?;
        log::warn!("Added succesfully");
        Ok(())
    }

    fn remove_route_from_saved(&mut self, name: &SavedRouteName) -> Result<()> {
        log::warn!("Remove route {name} from chat ID {}", self.0);
        let db = sled::Config::new()
            .path("db/saved_routes")
            .cache_capacity(100_000_000)
            .open()?;
        let key = bincode::serialize(&self.0)?;
        if let Some(ivec) = db.get(&key)? {
            let mut routes = bincode::deserialize::<SavedRoutes>(&ivec)?;
            routes.remove(name);
            db.insert(key, bincode::serialize(&routes)?)?;
        }
        log::warn!("Removed successfully");
        Ok(())
    }
}

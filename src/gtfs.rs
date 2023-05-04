use std::collections::HashMap;
use std::fmt::Display;
use std::io::{Read, Write};
use std::str::FromStr;
use std::time::SystemTime;

use anyhow::{anyhow, Result};
use chrono::Local;
use convert_case::{Case, Casing};
use gtfs_rt::FeedMessage;
use prost::Message;
use tempfile::Builder;

use crate::STATIC_FEED;

#[derive(Debug, PartialEq)]
pub enum Vehicle {
    Bus,
    Tram,
    Trolley,
}

impl Display for Vehicle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tram => {
                write!(f, "Трамвай 🚋")
            }
            Self::Trolley => write!(f, "Троллейбус 🚎"),
            Self::Bus => {
                write!(f, "Автобус 🚌")
            }
        }
    }
}

pub struct ParseVehicleErr;
impl FromStr for Vehicle {
    type Err = ParseVehicleErr;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "bus" => Ok(Self::Bus),
            "tram" => Ok(Self::Tram),
            "trolley" => Ok(Self::Trolley),
            _ => Err(ParseVehicleErr),
        }
    }
}

pub type RouteId = String;
pub type RouteNumber = String;
pub type RouteName = String;

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct RouteInfo {
    pub id: RouteId,
    pub name: RouteName,
}

pub type Tram = HashMap<RouteNumber, RouteInfo>;
pub type Trolley = HashMap<RouteNumber, RouteInfo>;
pub type Bus = HashMap<RouteNumber, RouteInfo>;

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct RoutesFeed {
    pub tram: Tram,
    pub trolley: Trolley,
    pub bus: Bus,
    pub all: HashMap<RouteId, RouteName>,
}

pub type StopId = String;
pub type StopName = String;
pub type StopsFeed = HashMap<StopId, StopName>;

pub type TripId = String;

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct Trips {
    forward_trip: Vec<TripId>,
    backward_trip: Vec<TripId>,
}
pub type TripsFeed = HashMap<RouteId, Trips>;

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct TripStop {
    timestamp: i64,
    stop_id: StopId,
    stop_sequence: u8,
}

pub type TripInfo = HashMap<TripId, Vec<TripStop>>;

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct StaticFeed {
    pub routes: RoutesFeed,
    pub stops: StopsFeed,
    pub trips: TripsFeed,
    pub stop_times: TripInfo,
}

pub async fn static_feed() -> Result<StaticFeed> {
    let mut feed = StaticFeed::default();

    // Temporary storages
    let mut tmp_feed_archive = Builder::new().prefix("feed").suffix(".zip").tempfile()?;

    // Get fresh GTFS feed
    let content =
        reqwest::get("https://transport.orgp.spb.ru/Portal/transport/internalapi/gtfs/feed.zip")
            .await?
            .bytes()
            .await?;

    tmp_feed_archive.write_all(&content)?;

    // Extract required data.
    let zipfile = std::fs::File::open(tmp_feed_archive.path())?;
    let mut archive = zip::ZipArchive::new(zipfile)?;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        if let Some(path) = file.enclosed_name() {
            if let Some(name) = path.file_name() {
                match name.to_str().unwrap() {
                    "routes.txt" => {
                        let mut routes = String::new();
                        file.read_to_string(&mut routes)?;
                        //fill routes part of the feed
                        routes.lines().skip(1).for_each(|line| {
                            let left = line.splitn(4, ',').collect::<Vec<&str>>();
                            let right = left[3].rsplitn(6, ',').collect::<Vec<&str>>();

                            let id = RouteId::from(left[0]);
                            let name = RouteName::from(right[5]);
                            let number = RouteNumber::from(left[2]);
                            let vehicle = Vehicle::from_str(right[3]);

                            feed.routes.all.insert(id.clone(), name.clone());

                            match vehicle {
                                Ok(v) => match v {
                                    Vehicle::Bus => {
                                        if let Some(entry) =
                                            feed.routes.bus.insert(number, RouteInfo { id, name })
                                        {
                                            log::warn!("{entry:#?} already present");
                                        }
                                    }
                                    Vehicle::Trolley => {
                                        if let Some(entry) = feed
                                            .routes
                                            .trolley
                                            .insert(number, RouteInfo { id, name })
                                        {
                                            log::warn!("{entry:#?} already present");
                                        }
                                    }
                                    Vehicle::Tram => {
                                        if let Some(entry) =
                                            feed.routes.tram.insert(number, RouteInfo { id, name })
                                        {
                                            log::warn!("{entry:#?} already present");
                                        }
                                    }
                                },
                                Err(_) => {
                                    log::error!(
                                        "Failed to parse vehicle type {}, entry skipped",
                                        right[3]
                                    );
                                }
                            };
                        });
                    }
                    "stops.txt" => {
                        let mut stops = String::new();
                        file.read_to_string(&mut stops)?;
                        //fill stops part of the feed
                        stops.lines().skip(1).for_each(|line| {
                            let left = line.splitn(3, ',').collect::<Vec<&str>>();
                            let right = left[2].rsplitn(6, ',').collect::<Vec<&str>>();

                            let id = StopId::from(left[0]);
                            let name = StopName::from(right[5]);

                            if let Some(entry) = feed.stops.insert(id, name) {
                                log::warn!("{entry:#?} already present");
                            }
                        });
                    }
                    "trips.txt" => {
                        let mut trips = String::new();
                        file.read_to_string(&mut trips)?;
                        //fill trips part of the feed
                        trips.lines().skip(1).for_each(|line| {
                            let l: Vec<&str> = line.split(',').collect();
                            let route_id = RouteId::from(l[0]);
                            let trip_id = TripId::from(l[2]);
                            let direction = l[3].parse::<u8>().unwrap();

                            if direction == 0 {
                                feed.trips
                                    .entry(route_id)
                                    .or_insert_with(|| Trips::default())
                                    .forward_trip
                                    .push(trip_id);
                            } else {
                                feed.trips
                                    .entry(route_id)
                                    .or_insert_with(|| Trips::default())
                                    .backward_trip
                                    .push(trip_id);
                            }
                        });
                    }
                    "stop_times.txt" => {
                        let mut stop_times = String::new();
                        file.read_to_string(&mut stop_times)?;
                        //fill stop times part of the feed
                        let date = Local::now().date_naive();
                        stop_times.lines().skip(1).for_each(|line| {
                            let l: Vec<&str> = line.split(',').collect();
                            let trip_id = TripId::from(l[0]);
                            let stop_id = StopId::from(l[3]);
                            let stop_sequence = l[4].parse::<u8>().unwrap();

                            let mut hms = l[1]
                                .split(':')
                                .filter_map(|val| val.parse::<u32>().ok())
                                .collect::<Vec<u32>>();

                            let nextday = if hms[0] >= 24 {
                                hms[0] -= 24;
                                true
                            } else {
                                false
                            };

                            let mut timestamp = date
                                .and_hms_opt(hms[0], hms[1], hms[2])
                                .unwrap()
                                .timestamp();
                            if nextday {
                                timestamp += 86400; // 60*60*24
                            }

                            feed.stop_times
                                .entry(trip_id)
                                .or_insert_with(|| Vec::new())
                                .push(TripStop {
                                    timestamp,
                                    stop_id,
                                    stop_sequence,
                                });
                        });
                    }
                    _ => (),
                };
            }
        };
    }

    tmp_feed_archive.close()?;

    Ok(feed)
}

pub async fn route_name(route_id: &RouteId) -> Result<RouteName> {
    let routes = &STATIC_FEED.read().await.routes.all;
    match routes.get(route_id) {
        Some(name) => {
            let mut name = name.to_uppercase();
            name = name.replace('\"', "");

            let res = name
                .split('-')
                .map(|substr| substr.to_case(Case::Title))
                .collect::<Vec<String>>()
                .join("-");
            Ok(res)
        }
        None => Err(anyhow!("Can't find route by ID")),
    }
}

pub async fn stop_name(stop_id: &StopId) -> Result<StopName> {
    let stops = &STATIC_FEED.read().await.stops;
    match stops.get(stop_id) {
        Some(name) => {
            let mut name = name.to_uppercase();
            name = name.replace('\"', "");
            Ok(name.to_case(Case::Title))
        }
        None => Err(anyhow!("failed to get stop by ID")),
    }
}

pub async fn stops_on_route(route_id: &RouteId, direction: &str) -> Result<Vec<StopId>> {
    let feed = STATIC_FEED.read().await;

    let trips = if direction == "0" {
        &feed.trips.get(route_id).unwrap().forward_trip
    } else {
        &feed.trips.get(route_id).unwrap().backward_trip
    };
    let mut res = vec![];

    let stops = trips.iter().find_map(|trip| feed.stop_times.get(trip));
    match stops {
        Some(stops) => {
            stops.iter().for_each(|stop| res.push(stop.stop_id.clone()));
            Ok(res)
        }
        None => Err(anyhow!("Couldn't find stops for this route and direction")),
    }
}

pub async fn arrival_forecast(route_id: &RouteId, stop_id: &StopId) -> Result<Vec<i64>> {
    let url = "https://transport.orgp.spb.ru/Portal/transport/internalapi/gtfs/realtime/stopforecast?stopID=".to_string() + stop_id.as_str();
    let resp = reqwest::get(url).await?.bytes().await?;
    let message = FeedMessage::decode(resp)?;

    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let mut waiting_time = vec![];

    for entity in message.entity {
        if &entity.id == route_id {
            if let Some(update) = entity.trip_update {
                for stop_time in update.stop_time_update {
                    if let Some(arrival) = stop_time.arrival {
                        if let Some(time) = arrival.time {
                            let time_left = time - timestamp;
                            if time_left > 0 {
                                waiting_time.push(time_left);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(waiting_time)
}

pub async fn arrival_timetable(
    route_id: &RouteId,
    direction: &str,
    stop_id: &StopId,
) -> Result<Vec<i64>> {
    let timestamp = Local::now().timestamp();

    let mut timetable = vec![];

    let feed = STATIC_FEED.read().await;

    let trips = feed
        .trips
        .get(route_id)
        .ok_or(anyhow!("Failed to find trips for this route ID"))?;

    let trip_ids = if direction == "0" {
        &trips.forward_trip
    } else {
        &trips.backward_trip
    };

    for trip in trip_ids {
        let trip_info = feed
            .stop_times
            .get(trip)
            .ok_or(anyhow!("Failed to fetch trip info"))?;
        for trip_stop in trip_info {
            if &trip_stop.stop_id == stop_id && trip_stop.timestamp > timestamp {
                timetable.push(trip_stop.timestamp);
            }
        }
    }

    timetable.sort();

    Ok(timetable)
}

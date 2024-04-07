use crate::sqlite::{Database, Location};
use crate::{Bot, Notification, Req};
use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use chrono_humanize::{Accuracy, HumanTime, Tense};
use failure::{bail, err_msg, Error};
use futures::future::try_join_all;
use kuchiki::traits::*;
use openweathermap::blocking::weather;
use openweathermap::CurrentWeather;
use serde::{Deserialize, Deserializer};
use std::cell::RefCell;
use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration as STDDuration;
use tokio::spawn;
use tokio::sync::mpsc;
use urlencoding::encode;
use webpage::{Webpage, WebpageOptions};

enum Task<'a> {
    Ignore,
    Message(&'a str),
    Seen(&'a str),
    Tell(&'a str, &'a str),
    Weather(Option<&'a str>),
    Location(&'a str),
    Coins(&'a str, &'a str),
    Lastfm(&'a str),
    Hang(&'a str),
    HangGuess(&'a str),
    HangStart(&'a str),
}

fn process_commands<'a>(nick: &'a str, msg: &'a str) -> Task<'a> {
    let mut tokens = msg.split_whitespace();
    let next = tokens.next();

    let mut bot_prefix: Option<&str> = None;

    if let Some(n) = next {
        // interactions with the bot i.e., '.help'
        bot_prefix = match n {
            c if c.starts_with("./") => c.strip_prefix("./"),
            // some people like to say just '.' or '!' in irc so
            // we'll check the length to maker sure they're
            // actually trying to interact with the bot
            c if c.starts_with('.') && c.len() > 1 => c.strip_prefix('.'),
            c if c.starts_with('!') && c.len() > 1 => c.strip_prefix('!'),
            c if c.to_lowercase().starts_with(nick) => match tokens.next() {
                Some(n) => Some(n),
                None => Some("help"),
            },
            _ => None,
        }
    }

    // if there's no '`boot:` help' or '`.`help' there's nothing
    // left to do, so continue with our day
    if bot_prefix.is_none() {
        // todo: it's accepting short/medium/long here when it shouldn't
        return match next {
            Some(t) if tokens.count() == 0 => {
                let letter = match t.trim().chars().next() {
                    Some(x) if t.trim().len() == 1 && matches!(x, 'a'..='z') => true,
                    _ => false,
                };

                if letter {
                    Task::Hang(t.trim())
                } else {
                    Task::HangGuess(t.trim())
                }
            }
            _ => Task::Ignore,
        };
    }

    let coins = [
        "btc",
        "bitcoin",
        "btcgbp", // bitcoin
        "eth",
        "ethereum", // ethereum
        "ltc",      // litecoin
        "xmr",
        "monero", // monero
        "doge",   // dogecoin
        "coins",
        "shitcoins",
    ];

    match bot_prefix.unwrap() {
        "help" | "man" | "manual" => {
            let response =
                "Commands: repo | seen <nick> | tell <nick> <message> | weather <location> \
                        | loc <location> | <btc(gbp)|eth|ltc|xmr|doge> \
                        <day|week|fortnight|month|year> \
                        | hang <short|medium|long>";
            Task::Message(response)
        }
        "repo" | "git" => Task::Message("https://github.com/niall-/boot"),
        "seen" => match tokens.next() {
            Some(nick) if !nick.is_empty() => Task::Seen(nick),
            Some(_) => Task::Message("Hint: seen <nick>"),
            None => Task::Message("Hint: seen <nick>"),
        },
        "tell" => match tokens.next() {
            Some(nick) => match tokens.remainder() {
                Some(message) if !message.trim().is_empty() => Task::Tell(nick, message.trim()),
                _ => Task::Message("Hint: tell <nick> <message>"),
            },
            None => Task::Message("Hint: tell <nick> <message>"),
        },
        "weather" => match tokens.remainder() {
            Some(loc) if !loc.trim().is_empty() => Task::Weather(Some(loc.trim())),
            _ => Task::Weather(None),
        },
        "loc" | "location" => match tokens.remainder() {
            Some(loc) if !loc.trim().is_empty() => Task::Location(loc.trim()),
            _ => Task::Message("Hint: loc|location <location>"),
        },
        // TODO: support .spot for current spot price
        c if coins.iter().any(|e| e == &c) => {
            let coin_times = [
                "1d",
                "day",
                "24h",
                "7d",
                "w",
                "1w",
                "week",
                "weekly",
                "14d",
                "2w",
                "fortnight",
                "fortnightly",
                "31d",
                "30d",
                "month",
                "year",
                "1y",
                "3y",
                "5y",
                "spot",
            ];
            let coin_time = match tokens.next() {
                Some(n) if coin_times.iter().any(|e| e.eq_ignore_ascii_case(n)) => {
                    match n.to_lowercase().as_ref() {
                        "7d" | "w" | "1w" | "week" | "weekly" => "7d",
                        "14d" | "2w" | "fortnight" | "fortnightly" => "14d",
                        "31d" | "30d" | "month" => "31d",
                        "year" => "1y",
                        "3y" => "3y",
                        "5y" => "5y",
                        _ => "1d",
                    }
                }
                Some(_) => "1d",
                None => "1d",
            };
            Task::Coins(c, coin_time)
        }
        "lastfm" => match tokens.next() {
            Some(nick) => Task::Lastfm(nick.trim()),
            None => Task::Message("noob"),
        },
        "hang" => match tokens.next() {
            Some(l) => match l.trim().to_lowercase().as_ref() {
                "short" => Task::HangStart("short"),
                "medium" => Task::HangStart("medium"),
                "long" => Task::HangStart("long"),
                _ => Task::HangStart(""),
            },
            None => Task::HangStart(""),
        },
        _ => Task::Ignore,
    }
}

pub async fn process_messages(
    msg: crate::Msg,
    db: &Database,
    client: &crate::Client,
    api_key: Option<String>,
    tx2: &mpsc::Sender<Bot>,
    _req: Req,
) {
    // HACK: check_notification only returns at most 2 notifications
    // if user alice spams user bob with notifications, when bob speaks he will be spammed with all
    // of those notifications at once (with some rate limiting provided by the irc crate), with
    // this hack bob will only ever receive 2 messages when he speaks, giving some end user control
    // for whether the channel is going to be spammed
    // some ways to fix this: some persistence allowing for a user to receive any potential
    // messages over pm, limit number of messages a user can receive, etc
    let notifications = check_notification(&msg.source, db);
    for n in notifications {
        client.send_privmsg(&msg.target, &n).unwrap();
    }

    let nick = client.current_nickname().to_lowercase();

    // easter eggs
    // TODO: add support for parsing from file
    match &msg.content {
        n if n.trim().starts_with("nn ") => {
            let response = match &msg.content {
                c if c.to_lowercase().contains(&nick) => format!("nn {}", &msg.source),
                _ => "nn".to_string(),
            };
            client.send_privmsg(&msg.target, response).unwrap();
            return;
        }
        _ => (),
    }

    let command = process_commands(&nick, &msg.content);

    match command {
        Task::Message(m) => client.send_privmsg(msg.target, m).unwrap(),
        Task::Seen(n) => {
            let response = check_seen(n, db);
            client.send_privmsg(msg.target, response).unwrap()
        }
        Task::Tell(n, m) => {
            let entry = Notification {
                id: 0,
                recipient: n.to_string(),
                via: msg.source,
                message: m.to_string(),
            };
            if let Err(err) = db.add_notification(&entry) {
                println!("SQL error adding notification: {}", err);
                return;
            }
            let response = format!("Ok, I'll tell {} that", n);
            client.send_privmsg(msg.target, response).unwrap();
        }
        // TODO: figure out the borrowowing issue(s?) so code doesn't have to be
        // duplicated as much here, and especially so that it can be
        // separated out into its own functions
        Task::Weather(l) => {
            if api_key.is_none() {
                return;
            }
            let key = api_key.as_ref().unwrap().clone();

            let mut location = String::new();
            let mut coords: Option<String> = None;

            match l {
                // check to see if we have the location already stored
                None => match db.check_weather(&msg.source) {
                    Ok(Some((lat, lon))) => coords = Some(format!("{},{}", lat, lon)),
                    Ok(None) => {
                        let response = "Hint: weather <location>".to_string();
                        client.send_privmsg(&msg.target, response).unwrap();
                        return;
                    }
                    Err(err) => println!("Error checking weather: {}", err),
                },

                // update user's weather preference and fetch coordinates
                Some(l) => {
                    location = l.to_string();
                    let loc = db.check_location(l);
                    match loc {
                        Ok(Some(l)) => {
                            coords = Some(format!("{},{}", &l.lat, &l.lon));
                            tx2.send(Bot::UpdateWeather(msg.source.clone(), l.lat, l.lon))
                                .await
                                .unwrap();
                        }
                        Ok(None) => (),
                        Err(err) => println!("Error checking location: {}", err),
                    }
                }
            }

            match coords {
                // we have the coords already, all we need now is the weather
                Some(coords) => {
                    let tx2 = tx2.clone();
                    let ftarget = msg.target.clone();

                    spawn(async move {
                        let weather = get_weather(&coords, &key).await;
                        match weather {
                            Ok(weather) => {
                                let pretty = print_weather(weather);
                                tx2.send(Bot::Privmsg(ftarget, pretty)).await.unwrap();
                            }
                            Err(err) => {
                                println!("weather isn't initialised: {}", err);
                            }
                        }
                    });
                }

                // we don't have coords for the location
                // this is the worst case scenario
                None => {
                    let tx2 = tx2.clone();
                    let ftarget = msg.target.clone();
                    let fsource = msg.source.clone();

                    spawn(async move {
                        let fetched_location = get_location(&location).await;
                        #[allow(unused_assignments)]
                        let mut coords: Option<String> = None;

                        match fetched_location {
                            Ok(Some(l)) => {
                                let lat = l.lat.clone();
                                let lon = l.lon.clone();

                                coords = Some(format!("{},{}", &lat, &lon));

                                tx2.send(Bot::UpdateWeather(fsource, lat, lon))
                                    .await
                                    .unwrap();
                                tx2.send(Bot::UpdateLocation(location, l)).await.unwrap();
                            }

                            Ok(None) => {
                                let response = format!("Unable to fetch location for {}", location);
                                println!("{}", &response);
                                tx2.send(Bot::Privmsg(ftarget, response)).await.unwrap();
                                return;
                            }
                            Err(err) => {
                                println!("Error fetching location data: {}", err);
                                return;
                            }
                        }

                        match get_weather(&coords.unwrap(), &key).await {
                            //let weather = get_weather(&lcoords.unwrap(), &key).await;
                            //match weather {
                            Ok(weather) => {
                                let pretty = print_weather(weather);
                                tx2.send(Bot::Privmsg(ftarget, pretty)).await.unwrap();
                            }
                            Err(err) => {
                                println!("weather isn't initialised: {}", err);
                            }
                        }
                    });
                }
            }
        }
        Task::Location(l) => match db.check_location(l) {
            Ok(Some(l)) => {
                let response = format!(
                    "https://www.openstreetmap.org/?mlat={}&mlon={}",
                    l.lat, l.lon
                );
                client.send_privmsg(msg.target, response).unwrap();
            }
            Ok(None) => {
                let tx2 = tx2.clone();
                let flocation = l.to_string();
                let ftarget = msg.target.clone();
                let response = format!("No coordinates found for {} in database", l);
                println!("{}", response);
                spawn(async move {
                    let fetched_location = get_location(&flocation).await;
                    match fetched_location {
                        Ok(Some(l)) => {
                            let response = format!(
                                "https://www.openstreetmap.org/?mlat={}&mlon={}",
                                l.lat, l.lon
                            );
                            tx2.send(Bot::UpdateLocation(flocation, l)).await.unwrap();
                            tx2.send(Bot::Privmsg(ftarget, response)).await.unwrap()
                        }
                        Ok(None) => {
                            let response =
                                format!("Unable to fetch location data for {}", flocation);
                            println!("{}", &response);
                            tx2.send(Bot::Privmsg(ftarget, response)).await.unwrap();
                        }
                        Err(err) => {
                            println!("Error fetching location data for {}", err)
                        }
                    }
                });
            }
            Err(err) => println!("Error fetching location from database: {}", err),
        },
        Task::Coins(c, t) => {
            let coin = match c {
                "btc" | "bitcoin" => "XXBTZUSD",
                "btcgbp" => "XXBTZGBP",
                "eth" | "ethereum" => "XETHZUSD",
                "ltc" => "XLTCZUSD",
                "xmr" | "monero" => "XXMRZUSD",
                "doge" => "XDGUSD",
                _ => "XXBTZUSD",
            };

            // todo: we should store the json so that we only need to fetch an updated spot price
            /*let dbcoin = match t {
                "donotcheck" => db.check_coins(&coin),
                _ => Ok(None),
            };

            let check = match dbcoin {
                Ok(Some(c)) => {
                    let now = Utc::now().naive_utc();
                    let date = (c.date / 1000).to_string();
                    let previous = NaiveDateTime::parse_from_str(&date, "%s").unwrap();
                    let duration = now.signed_duration_since(previous);

                    if duration > Duration::seconds(15 * 60 + 30) {
                        true
                    } else {
                        client.send_privmsg(&msg.target, c.data_0).unwrap();
                        client.send_privmsg(&msg.target, c.data_1).unwrap();
                        false
                    }
                }
                Ok(None) => true,
                Err(err) => {
                    println!("error checking coins: {}", err);
                    true
                }
            };*/

            let ftarget = msg.target.clone();
            let tx2 = tx2.clone();
            let time_frame = t.to_string();
            spawn(async move {
                let coins = get_coins(coin, &time_frame).await;
                match coins {
                    Ok(coins) => {
                        let _coin = coins.clone();
                        let coin2 = coins.clone();
                        let coin3 = coins.clone();
                        let ftarget2 = ftarget.clone();
                        //tx2.send(Bot::UpdateCoins(coin)).await.unwrap();
                        tx2.send(Bot::Privmsg(ftarget, coin2.data_0)).await.unwrap();
                        tx2.send(Bot::Privmsg(ftarget2, coin3.data_1))
                            .await
                            .unwrap();
                    }
                    Err(err) => {
                        println!("issue getting shitcoin data: {}", err);
                    }
                }
            });
        }
        Task::Lastfm(n) => match get_lastfm_scrobble(n.to_string(), _req).await {
            Ok(response) => client.send_privmsg(msg.target, response).unwrap(),
            Err(e) => client.send_privmsg(msg.target, e).unwrap(),
        },
        Task::Hang(l) if msg.target == "#games" => {
            tx2.send(Bot::Hang(msg.target, l.to_string()))
                .await
                .unwrap();
        }
        Task::HangGuess(w) if msg.target == "#games" => {
            tx2.send(Bot::HangGuess(msg.target, w.to_string()))
                .await
                .unwrap();
        }
        Task::HangStart(l) if msg.target == "#games" => {
            let target = if l.len() == 0 {
                "<start>".to_string()
            } else {
                l.to_string()
            };

            tx2.send(Bot::HangGuess(msg.target, target)).await.unwrap();
        }
        Task::Ignore => (),
        _ => (),
    }
}

pub async fn process_titles(links: Vec<(String, String)>, req: Req) -> Vec<(String, String)> {
    // the following is adapted from
    // https://stackoverflow.com/questions/63434977/how-can-i-spawn-asynchronous-methods-in-a-loop
    try_join_all(links.into_iter().map(|(t, l)| {
        let req = req.clone();
        spawn(async move {
            if let Ok((target, Some(title))) = fetch_title(t, l, req).await {
                let response = format!("↳ {}", title.replace('\n', " "));
                Some((target, response))
            } else {
                None
            }
        })
    }))
    .await
    .unwrap_or_default()
    .into_iter()
    .flatten()
    .collect()
}

async fn fetch_title(
    target: String,
    url: String,
    req: Req,
) -> Result<(String, Option<String>), Error> {
    let content = req.read(&url, 8192).await?;

    let page = kuchiki::parse_html().one(content);

    let title = page
        .select_first("title")
        .ok()
        .and_then(|t| t.as_node().first_child())
        .and_then(|t| t.as_text().map(RefCell::take));

    let og_title = page
        .select_first(r#"meta[property="og:title"]"#)
        .ok()
        .and_then(|t| {
            t.as_node()
                .as_element()
                .and_then(|t| t.attributes.borrow().get("content").map(|t| t.to_string()))
        });

    Ok(match title {
        // youtube is inconsistent, the best option here would be to use the api, an invidious api,
        // or possibly sed youtube.com with an invidious instance
        Some(t) if t == "YouTube" && og_title.is_some() => (target, og_title),
        Some(t) if t == "Pleroma" && og_title.is_some() => (target, og_title),
        _ => (target, title),
    })
}

pub fn check_seen(nick: &str, db: &Database) -> String {
    match db.check_seen(nick) {
        Ok(Some(p)) => {
            let time = Utc::now();
            let previous = DateTime::parse_from_rfc3339(&p.time).unwrap();
            let duration = time.signed_duration_since(previous);
            let human_time = HumanTime::from(duration).to_text_en(Accuracy::Rough, Tense::Past);
            format!("{} was last seen {} {}", p.username, human_time, p.message)
        }
        Ok(None) => format!("{} has not previously been seen", nick),
        Err(_err) => "SQL error".to_string(),
    }
}

pub fn check_notification(nick: &str, db: &Database) -> Vec<String> {
    let mut notification: Vec<_> = Vec::new();
    match db.check_notification(nick) {
        Ok(n) => {
            for i in n {
                let message = format!("{}, message from {}: {}", nick, i.via, i.message);
                notification.push(message);
                if let Err(err) = db.remove_notification(i.id) {
                    println!("SQL error checking notification: {}", err)
                }
                if notification.len() > 1 {
                    break;
                }
            }
        }
        Err(_err) => (),
    }

    notification
}

pub async fn get_location(loc: &str) -> Result<Option<Location>, Error> {
    // TODO: add this to settings
    let opt = WebpageOptions {
        allow_insecure: true,
        follow_location: true,
        max_redirections: 10,
        timeout: STDDuration::from_secs(10),
        // a legitimate user agent is necessary for some sites (twitter)
        useragent: "Mozilla/5.0 boot-bot-rs/1.3.0".to_string(),
    };

    // TODO: this throws an error when a city doesn't exist for a location (i.e., it's a county)
    // TODO: nominatim has a strict limit of 1 request per second, while the channel I run the
    // bot in most certainly won't exceed this limit and I don't think it's likely many channels
    // will either (how many users are going to request weather before an op kicks the bot?)
    // something should be done about this soon to respect nominatim's TOS
    let url = format!(
        "https://nominatim.openstreetmap.org/search?q={}&format=json&addressdetails=1&limit=1",
        &encode(loc)
    );

    let page = Webpage::from_url(&url, opt)?;

    let mut entry: Vec<Location> = serde_json::from_str(&page.html.text_content)?;

    Ok(entry.pop())
}

pub async fn get_weather(coords: &str, api_key: &str) -> Result<CurrentWeather, String> {
    let w: CurrentWeather = weather(coords, "metric", "en", api_key)?;

    Ok(w)
}

pub fn print_weather(weather: CurrentWeather) -> String {
    // this is dumb, it's only necessary because OpenWeatherMap doesn't fully capitalise weather
    // conditions, see: https://openweathermap.org/weather-conditions
    // https://stackoverflow.com/questions/38406793/why-is-capitalizing-the-first-letter-of-a-string-so-convoluted-in-rust/38406885#38406885
    fn uppercase(s: &str) -> String {
        let mut c = s.chars();
        match c.next() {
            None => String::new(),
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        }
    }

    let location = &format!("{}, {}", weather.name, weather.sys.country);

    // if the weather condition is cloudy add cloud coverage
    // https://openweathermap.org/weather-conditions
    // the 700..=781 range has some conditions like
    // mist/haze/fog but I don't think cloud coverage matters there
    let description = uppercase(&weather.weather[0].description).to_string();
    let description = match weather.weather[0].id {
        // thunderstorms
        200..=232 => format!("{}, {}% cv", description, weather.clouds.all),
        // drizzle
        300..=321 => format!("{}, {}% cv", description, weather.clouds.all),
        // rain
        500..=531 => format!("{}, {}% cv", description, weather.clouds.all),
        // snow
        600..=622 => format!("{}, {}% cv", description, weather.clouds.all),
        // clouds
        801..=804 => format!("{}, {}% cv", description, weather.clouds.all),
        _ => description,
    };

    // OpenWeatherMap provides sunrise/sunset in UTC (Unix time)
    // it also provides an offset in seconds, in practice we can
    // add it to UTC Unix time and get a naive local time but this isn't ideal
    let sunrise = weather.sys.sunrise.wrapping_add(weather.timezone);
    let sunset = weather.sys.sunset.wrapping_add(weather.timezone);
    let sunrise = match NaiveDateTime::parse_from_str(&sunrise.to_string(), "%s") {
        Ok(s) => s.format("%l:%M%p").to_string(),
        Err(_) => "Failed to parse time".to_string(),
    };
    let sunset = match NaiveDateTime::parse_from_str(&sunset.to_string(), "%s") {
        Ok(s) => s.format("%l:%M%p").to_string(),
        Err(_) => "Failed to parse time".to_string(),
    };

    let celsius = weather.main.temp.round() as i64;
    let fahrenheit = ((weather.main.temp * (9.0 / 5.0)) + 32_f64).round() as i64;

    let metric_wind = weather.wind.speed.round();
    let imperial_wind = (weather.wind.speed * 2.2369_f64).round();
    let wind = match weather.wind.gust {
        Some(g) => {
            let metric_gust = g.round();
            let imperial_gust = (g * 2.2369_f64).round();
            format!(
                "Wind: {} mph [{} m/s], Gust: {} mph [{} m/s]",
                imperial_wind, metric_wind, imperial_gust, metric_gust
            )
        }
        None => {
            format!("Wind: {} mph [{} m/s]", metric_wind, imperial_wind)
        }
    };

    let direction = [
        "↓ N", "↙ NE", "← E", "↖ SE", "↑ S", "↗ SW", "→ W", "↘ NW", "↓ N",
    ];
    let degrees = weather.wind.deg.rem_euclid(360.0).round() as usize / 45;

    format!("Weather for {}: {}, {}% Humidity | Temp: {}°C [{}°F] | {} coming from {} - {}° | Sunrise: {} | Sunset: {}",
            location, description, weather.main.humidity,
            celsius, fahrenheit,
            wind, direction[degrees], weather.wind.deg,
            sunrise, sunset)
}

#[derive(Debug, Deserialize, Clone)]
pub struct Coin {
    pub coin: String,
    pub date: i64,
    // both are sent to the channel at the same time
    // XXBTZUSD $41733.5 (05-Tue 02:00:00 UTC) ▂▂▂▂▁▁▁▁▁▂▂▂▃▄▆▇▇▇▇██▇██ spot: $44131.9 (06-Wed 01:06:20 UTC)
    pub data_0: String,
    // XXBTZUSD high: $44192.8 (05-Tue 22:00:00 UTC) // mean: $44444.49 // low: $41529.8 (05-Tue 07:00:00 UTC)
    pub data_1: String,
}

fn from_str<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr,
    T::Err: std::fmt::Display,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    T::from_str(&s).map_err(serde::de::Error::custom)
}

#[derive(Debug, Deserialize)]
struct OhlcData {
    time: i64,
    _open: String,
    high: String,
    low: String,
    _close: String,
    #[serde(deserialize_with = "from_str")]
    vwap: f32,
    _volume: String,
    _count: i64,
}

#[derive(Debug, Deserialize)]
struct OhlcResult {
    #[serde(flatten)]
    data: HashMap<String, Vec<OhlcData>>,
    #[serde(rename = "last")]
    _last: i64,
}

#[derive(Debug, Deserialize)]
struct Ohlc {
    #[serde(rename = "error")]
    _error: Vec<String>,
    result: OhlcResult,
}

#[derive(Debug, Deserialize)]
struct TickerData {
    #[serde(rename = "a")]
    _a: Vec<String>,
    #[serde(rename = "b")]
    _b: Vec<String>,
    c: Vec<String>,
    #[serde(rename = "v")]
    _v: Vec<String>,
    #[serde(rename = "p")]
    _p: Vec<String>,
    #[serde(rename = "t")]
    _t: Vec<i64>,
    #[serde(rename = "l")]
    _l: Vec<String>,
    #[serde(rename = "h")]
    _h: Vec<String>,
    #[serde(rename = "o")]
    _o: String,
}

#[derive(Debug, Deserialize)]
struct TickerResult {
    #[serde(flatten)]
    data: HashMap<String, TickerData>,
}

#[derive(Debug, Deserialize)]
struct Ticker {
    //#[serde(rename = "error")] _error: Vec<String>,
    result: TickerResult,
}

pub async fn get_coins(coin: &str, time_frame: &str) -> Result<Coin, Error> {
    // TODO: add this to settings
    let opt = WebpageOptions {
        allow_insecure: true,
        follow_location: true,
        max_redirections: 10,
        timeout: STDDuration::from_secs(10),
        // a legitimate user agent is necessary for some sites (twitter)
        useragent: "Mozilla/5.0 boot-bot-rs/1.3.0".to_string(),
    };
    // ...
    let opt2 = WebpageOptions {
        allow_insecure: true,
        follow_location: true,
        max_redirections: 10,
        timeout: STDDuration::from_secs(10),
        // a legitimate user agent is necessary for some sites (twitter)
        useragent: "Mozilla/5.0 boot-bot-rs/1.3.0".to_string(),
    };

    let (interval, since) = match time_frame {
        "1d" => (60, Utc::now() - Duration::hours(24)),
        "7d" => (240, Utc::now() - Duration::days(7)),
        "14d" => (240, Utc::now() - Duration::days(14)),
        "31d" => (1440, Utc::now() - Duration::days(31)),
        "1y" => (21600, Utc::now() - Duration::days(365)),
        "3y" => (21600, Utc::now() - Duration::days(1095)),
        "5y" => (21600, Utc::now() - Duration::days(1825)),
        _ => (60, Utc::now() - Duration::hours(24)),
    };

    // https://docs.kraken.com/rest/#tag/Market-Data/operation/getOHLCData
    let ohlc_url = format!(
        "https://api.kraken.com/0/public/OHLC?pair={coin}&interval={interval}&since={}",
        since.timestamp()
    );
    let ticker_url = format!("https://api.kraken.com/0/public/Ticker?pair={coin}");

    println!("ohlc: {ohlc_url}");
    println!("ticker: {ticker_url}");

    let ohlc_page = Webpage::from_url(&ohlc_url, opt)?;
    let ticker_page = Webpage::from_url(&ticker_url, opt2)?;
    let mut coin_json: Ohlc = serde_json::from_str(&ohlc_page.html.text_content)?;
    let mut ticker_json: Ticker = serde_json::from_str(&ticker_page.html.text_content)?;
    let spot_time = Utc::now().timestamp();

    //let json_data = r#"{"error":[],"result":{"XXBTZUSD":[[1701730800,"41970.0","41984.7","41793.6","41984.7","41877.4","135.24641260",1812],[1701734400,"41983.0","41983.0","41750.0","41879.5","41833.9","178.09065890",1197],[1701738000,"41879.5","41904.5","41617.6","41799.9","41745.8","113.18066859",1270],[1701741600,"41800.0","41804.6","41621.0","41729.9","41733.5","51.02022883",863],[1701745200,"41730.3","41826.4","41717.9","41818.0","41793.5","51.86326154",725],[1701748800,"41822.4","41825.0","41721.6","41765.7","41773.6","30.21526676",679],[1701752400,"41765.7","41911.7","41721.1","41909.2","41889.6","91.74214454",779],[1701756000,"41909.2","41917.1","41664.5","41720.0","41822.5","98.96134530",1020],[1701759600,"41720.0","41720.0","41427.1","41515.1","41529.8","124.90751096",1330],[1701763200,"41515.1","41624.8","41447.4","41608.4","41555.8","126.96394249",877],[1701766800,"41612.3","41707.1","41608.2","41706.0","41672.2","12.36149485",655],[1701770400,"41706.1","41755.0","41633.7","41633.7","41709.0","32.74293494",709],[1701774000,"41633.7","41729.6","41568.3","41725.7","41656.5","44.50569904",749],[1701777600,"41725.7","41872.3","41691.8","41872.3","41801.8","44.29458914",770],[1701781200,"41872.3","42050.0","41820.9","41835.9","41950.9","265.79221665",2100],[1701784800,"41835.9","42230.0","41835.8","42222.0","42051.8","209.26798469",2066],[1701788400,"42222.0","42490.3","42110.0","42293.0","42278.0","337.86431557",2457],[1701792000,"42293.0","42787.0","42139.5","42735.0","42534.1","561.04636522",3996],[1701795600,"42735.0","43990.0","42691.6","43394.5","43361.0","1111.03024097",7849],[1701799200,"43386.4","44050.0","43320.0","43725.9","43735.8","364.09461761",3573],[1701802800,"43725.8","43943.5","43620.0","43804.1","43755.3","202.74502157",2999],[1701806400,"43804.0","43836.6","43437.0","43782.3","43647.0","175.58621286",2442],[1701810000,"43785.1","44216.0","43724.0","43912.9","43933.1","343.40651248",3343],[1701813600,"43913.0","44465.0","43809.0","44355.0","44192.3","423.89511718",3326]],"last":1701810000}}"#;
    //let mut coin_json = serde_json::from_str::<OHLC>(json_data)?;
    //let ticker_data = r#"{"error":[],"result":{"XXBTZUSD":{"a":["44100.00000","126","126.000"],"b":["44099.90000","1","1.000"],"c":["44099.90000","0.05668947"],"v":["5287.30231047","5291.47690863"],"p":["42964.97598","42964.18797"],"t":[48035,48215],"l":["41427.10000","41427.10000"],"h":["44465.00000","44465.00000"],"o":"41983.00000"}}}"#;
    //let mut ticker_json = serde_json::from_str::<Ticker>(ticker_data)?;

    let mut coins = coin_json
        .result
        .data
        .remove(coin)
        .ok_or(err_msg("Unable to parse coin data"))?;

    let spot: TickerData = ticker_json
        .result
        .data
        .remove(coin)
        .ok_or(err_msg("Unable to parse spot data"))?;
    let spot = spot.c.first().unwrap();
    let spot: f32 = f32::from_str(spot).unwrap();

    let mut prices = Vec::<f32>::new();

    let mut initial: f32 = 0.0;
    let mut min: (f32, usize, i64) = (0.0, 0, 0); // price, count, time
    let mut max: (f32, usize, i64) = (0.0, 0, 0); // price, count, time
    let mut mean: f32 = 0.0;
    let mut tmp: f32 = 0.0; // tmp value used to sum

    // what we want is the min, max, mean, values the prices
    // for 2 week values we average the data to avoid long graphs
    // the initial value is to colour code the initial bar which
    // will be coins[3] since we're only keeping hourly prices
    for (count, c) in coins.iter().enumerate() {
        if count == 0 {
            initial = c.vwap;
            min = (c.vwap, count, c.time);
            max = (c.vwap, count, c.time);
        } else {
            let high = c.high.parse::<f32>().unwrap_or(c.vwap);
            let low = c.low.parse::<f32>().unwrap_or(c.vwap);

            match time_frame {
                "14d" => {
                    if count % 2 == 0 {
                        prices.push(c.vwap + tmp);
                        tmp = 0.0;
                    } else {
                        tmp += c.vwap;
                    }
                }
                _ => prices.push(c.vwap),
            }
            if high > max.0 {
                max = (high, count, c.time);
            } else if low < min.0 {
                min = (low, count, c.time);
            }
        }
        mean += c.vwap;
    }

    match time_frame {
        // not technically correct but whatever
        "14d" => prices.push(spot * 2.0),
        _ => prices.push(spot),
    }
    if spot > max.0 {
        max = (spot, max.1, spot_time)
    } else if spot < min.0 {
        min = (spot, min.1, spot_time)
    }
    mean += spot;

    let len = coins.len() + 1;
    mean /= len as f32;

    let sign = match coin {
        e if e.ends_with("GBP") => "£",
        _ => "$",
    };

    let colour = matches!(time_frame, "3y" | "5y");

    let graph = graph(initial, prices, !colour);
    let graph = if time_frame != "3y" && time_frame != "5y" {
        format!(
            "{coin} {sign}{} {} {graph} spot: {sign}{} {}",
            coins[0].vwap,
            print_date(coins[0].time, time_frame),
            //coins[len - 1].vwap,
            //print_date(coins[len - 1].time, time_frame),
            spot,
            print_date(spot_time, time_frame)
        )
    } else {
        format!("{coin} {graph}")
    };

    let stats = format!(
        "{coin} high: {sign}{} {} // mean: {sign}{mean} // low: {sign}{} {}",
        max.0,
        print_date(max.2, time_frame),
        min.0,
        print_date(min.2, time_frame),
    );

    let recent = coins.pop().unwrap();
    let result = Coin {
        coin: coin.to_string(),
        date: recent.time,
        data_0: graph,
        data_1: stats,
    };

    Ok(result)
}

fn print_date(date: i64, time_frame: &str) -> String {
    let time = NaiveDateTime::parse_from_str(&date.to_string(), "%s").unwrap();
    match time_frame {
        // 29-Nov-2023
        "7d" | "14d" | "31d" | "1y" | "3y" | "5y" => time.format("(%d-%b-%Y)").to_string(),
        // Tue-05 02:00:00 UTC
        _ => time.format("(%a-%d %T UTC)").to_string(),
    }
}

// the following is adapted from
// https://github.com/jiri/rust-spark
fn graph(initial: f32, prices: Vec<f32>, colour: bool) -> String {
    let ticks = "▁▂▃▄▅▆▇█";
    let colour_red = match colour {
        true => "\x0304",
        false => "",
    };
    let colour_green = match colour {
        true => "\x0303",
        false => "",
    };
    let colour_esc = match colour {
        true => "\x03",
        false => "",
    };

    /* XXX: This doesn't feel like idiomatic Rust */
    let mut min: f32 = f32::MAX;
    let mut max: f32 = 0.0;

    for &i in prices.iter() {
        if i > max {
            max = i;
        }
        if i < min && i > 0.0 {
            min = i;
        }
    }

    let ratio = if max == min {
        1.0
    } else {
        (ticks.chars().count() - 1) as f32 / (max - min)
    };

    let mut v = String::new();
    for (count, p) in prices.iter().enumerate() {
        let ratio = ((p - min) * ratio).round() as usize;

        if count == 0 {
            if *p <= 0.001 {
                v.push_str(" ");
            } else if p > &initial {
                v.push_str(&format!(
                    "{colour_green}{}{colour_esc}",
                    ticks.chars().nth(ratio).unwrap()
                ));
            } else {
                v.push_str(&format!(
                    "{colour_red}{}{colour_esc}",
                    ticks.chars().nth(ratio).unwrap()
                ));
            }
        } else {
            if *p <= 0.001 {
                v.push_str(" ");
            } else if p > &prices[count - 1] {
                // if the current price is higher than the previous price
                // the bar should be green, else red
                v.push_str(&format!(
                    "{colour_green}{}{colour_esc}",
                    ticks.chars().nth(ratio).unwrap()
                ));
            } else {
                v.push_str(&format!(
                    "{colour_red}{}{colour_esc}",
                    ticks.chars().nth(ratio).unwrap()
                ));
            }
        }
    }

    v
}

async fn get_lastfm_scrobble(user: String, req: Req) -> Result<String, Error> {
    let url = format!("https://www.last.fm/user/{}", encode(&user));
    let content = req.read(&url, 8192).await?;

    async fn take_last_played(user: String, html: String) -> Option<String> {
        let page = kuchiki::parse_html().one(html);
        let recent_tracks = page
            .select_first(r#"section[id="recent-tracks-section"]"#)
            .ok()?;
        let chartlist = recent_tracks
            .as_node()
            .select_first(r#"tr[class*="chartlist-row"]"#)
            .ok()?;
        let title = chartlist
            .as_node()
            .select_first(r#"td[class="chartlist-name"]"#)
            .ok()?;
        let artist = chartlist
            .as_node()
            .select_first(r#"td[class="chartlist-artist"]"#)
            .ok()?;
        let played = chartlist
            .as_node()
            .select_first(r#"td[class*="chartlist-timestamp"]"#)
            .ok()?;
        let last_played = match played.text_contents().trim() {
            "Scrobbling now" => format!(
                "{} is now playing {} by {}",
                user,
                title.text_contents().trim(),
                artist.text_contents().trim()
            ),
            _ => format!(
                "{} last played {} by {} {}",
                user,
                title.text_contents().trim(),
                artist.text_contents().trim(),
                played.text_contents().trim()
            ),
        };
        Some(last_played)
    }

    match take_last_played(user, content).await {
        Some(r) => Ok(r),
        None => bail!("No song data found!"),
    }
}

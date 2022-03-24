use crate::sqlite::{Database, Location};
use crate::{BotCommand, Notification};
use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use chrono_humanize::{Accuracy, HumanTime, Tense};
use failure::Error;
use futures::future::try_join_all;
use openweathermap::blocking::weather;
use openweathermap::CurrentWeather;
use serde::Deserialize;
use std::f32::MAX as f32_max;
use std::time::Duration as STDDuration;
use tokio::sync::mpsc;
use tokio::task::spawn_blocking;
use urlencoding::encode;
use webpage::{Webpage, WebpageOptions};

pub async fn process_messages(
    msg: crate::Msg,
    db: &Database,
    client: &crate::Client,
    api_key: Option<String>,
    tx2: mpsc::Sender<BotCommand>,
) {
    // HACK: check_notification only returns at most 2 notifications
    // if user alice spams user bob with notifications, when bob speaks he will be spammed with all
    // of those notifications at once (with some rate limiting provided by the irc crate), with
    // this hack bob will only ever receive 2 messages when he speaks, giving some end user control
    // for whether the channel is going to be spammed
    // some ways to fix this: some persistence allowing for a user to receive any potential
    // messages over pm, limit number of messages a user can receive, etc
    let notifications = check_notification(&msg.source, &db);
    for n in notifications {
        client.send_privmsg(&msg.target, &n).unwrap();
    }

    let nick = client.current_nickname().to_lowercase();

    let mut tokens = msg.content.split_whitespace();
    let next = tokens.next();

    let mut bot_prefix: Option<&str> = None;

    match next {
        // easter eggs
        // TODO: add support for parsing from file
        Some(n) if n == "nn" => {
            let response = match &msg.content {
                c if c.to_lowercase().contains(&nick) => format!("nn {}", &msg.source),
                _ => format!("nn"),
            };
            client.send_privmsg(&msg.target, response).unwrap();
            return ();
        }

        // interactions with the bot i.e., '.help'
        Some(n) => {
            bot_prefix = match n {
                c if c.starts_with("./") => c.strip_prefix("./"),
                // some people like to say just '.' or '!' in irc so
                // we'll check the length to maker sure they're
                // actually trying to interact with the bot
                c if (c.starts_with(".") && c.len() > 1) => c.strip_prefix("."),
                c if (c.starts_with("!") && c.len() > 1) => c.strip_prefix("!"),
                c if c.to_lowercase().starts_with(&nick) => match tokens.next() {
                    Some(n) => Some(n),
                    None => Some("help"),
                },
                _ => None,
            }
        }
        _ => (),
    }

    // if there's no '`boot:` help' or '`.`help' there's nothing
    // left to do, so continue with our day
    if !bot_prefix.is_some() {
        return ();
    }

    // TODO: add more coins https://docs.bitfinex.com/reference#rest-public-tickers
    // https://api-pub.bitfinex.com/v2/conf/pub:list:pair:exchange
    let coins = [
        "btc",
        "bitcoin",
        "eth",
        "ethereum",
        "coin",
        "coins",
        "shitcoins",
        "etc",
        "doge",
    ];
    let coin_times = [
        "w",
        "1w",
        "week",
        "weekly",
        "2w",
        "fortnight",
        "fortnightly",
        "4w",
        "30d",
        "month",
    ];

    match bot_prefix.unwrap() {
        "repo" | "git" => {
            let response = "https://github.com/niall-/boot";
            client.send_privmsg(msg.target, response).unwrap();
        }

        "help" | "man" | "manual" => {
            let response =
                "Commands: repo | seen <nick> | tell <nick> <message> | weather <location> \
                        | loc <location> | <coins|btc|eth|etc|doge> <week|fortnight|month>";
            client.send_privmsg(msg.target, response).unwrap();
        }

        c if coins.iter().any(|e| e == &c) => {
            let coin = match c.as_ref() {
                "btc" | "bitcoin" => "tBTCUSD",
                "eth" | "ethereum" => "tETHUSD",
                "etc" => "tETCUSD",
                "doge" => "tDOGE:USD",
                _ => "tBTCUSD",
            };
            let mut time_frame = "15m";
            match tokens.next() {
                Some(n) if coin_times.iter().any(|e| e == &n.to_lowercase()) => {
                    time_frame = match n.to_lowercase().as_ref() {
                        "w" | "1w" | "week" | "weekly" => "7D",
                        "2w" | "fortnight" | "fortnightly" => "14D",
                        "4w" | "30d" | "month" => "30D",
                        _ => "14D",
                    };
                }
                Some(_) => (),
                None => (),
            }

            let dbcoin = match time_frame {
                "15m" => db.check_coins(&coin),
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
            };

            if check {
                let ftarget = msg.target.clone();
                let tx2 = tx2.clone();
                tokio::spawn(async move {
                    let coins = get_coins(&coin, &time_frame).await;
                    match coins {
                        Ok(coins) => {
                            let coin = coins.clone();
                            let coin2 = coins.clone();
                            let coin3 = coins.clone();
                            let ftarget2 = ftarget.clone();
                            tx2.send(BotCommand::UpdateCoins(coin)).await.unwrap();
                            tx2.send(BotCommand::Privmsg((ftarget, coin2.data_0)))
                                .await
                                .unwrap();
                            tx2.send(BotCommand::Privmsg((ftarget2, coin3.data_1)))
                                .await
                                .unwrap();
                        }
                        Err(err) => {
                            println!("issue getting shitcoin data: {}", err);
                        }
                    }
                });
            }
        }

        // TODO: figure out the borrowowing issue(s?) so code doesn't have to be
        // duplicated as much here, and especially so that it can be
        // separated out into its own functions
        "weather" => {
            if api_key == None {
                return ();
            }
            let key: String = api_key.as_ref().unwrap().to_string();
            let tx2 = tx2.clone();
            let location = tokens.as_str();
            let source = msg.source.clone();
            let mut coords: Option<String> = None;

            match location.is_empty() {
                true => match db.check_weather(&msg.source) {
                    Ok(Some((lat, lon))) => coords = Some(format!("{},{}", lat, lon)),
                    Ok(None) => {
                        let response = format!("Please enter a location");
                        client.send_privmsg(&msg.target, response).unwrap();
                        return ();
                    }
                    Err(err) => println!("Error checking weather: {}", err),
                },
                false => {
                    let loc = db.check_location(&location);
                    match loc {
                        Ok(Some(l)) => {
                            coords = Some(format!("{},{}", &l.lat, &l.lon));
                            tx2.send(BotCommand::UpdateWeather(source, l.lat, l.lon))
                                .await
                                .unwrap();
                        }
                        Ok(None) => (),
                        Err(err) => println!("Error checking location: {}", err),
                    }
                }
            }

            match coords {
                Some(coords) => {
                    let tx2 = tx2.clone();
                    let ftarget = msg.target.clone();

                    tokio::spawn(async move {
                        let weather = get_weather(&coords, &key).await;
                        match weather {
                            Ok(weather) => {
                                let pretty = print_weather(weather);
                                tx2.send(BotCommand::Privmsg((ftarget, pretty)))
                                    .await
                                    .unwrap();
                            }
                            Err(err) => {
                                println!("weather isn't initialised: {}", err);
                            }
                        }
                    });
                }
                None => {
                    let tx2 = tx2.clone();
                    let flocation = location.to_string().clone();
                    let ftarget = msg.target.clone();
                    let ftarget2 = msg.target.clone();
                    let fsource = msg.source.clone();
                    //let key = key.to_string().clone();
                    tokio::spawn(async move {
                        let fetched_location = get_location(&flocation).await;
                        //let key = key.clone();
                        let mut coords: Option<String> = None;

                        match fetched_location {
                            Ok(Some(l)) => {
                                let lat = l.lat.clone();
                                let lon = l.lon.clone();

                                coords = Some(format!("{},{}", &lat, &lon));

                                tx2.send(BotCommand::UpdateWeather(fsource, lat, lon))
                                    .await
                                    .unwrap();
                                tx2.send(BotCommand::UpdateLocation(flocation, l))
                                    .await
                                    .unwrap();
                            }
                            Ok(None) => {
                                let response =
                                    format!("Unable to fetch location for {}", flocation);
                                println!("{}", &response);
                                tx2.send(BotCommand::Privmsg((ftarget, response)))
                                    .await
                                    .unwrap();
                            }
                            Err(err) => {
                                println!("Error fetching location data: {}", err)
                            }
                        }

                        match coords {
                            Some(coords) => {
                                let weather = get_weather(&coords, &key).await;
                                match weather {
                                    Ok(weather) => {
                                        let pretty = print_weather(weather);
                                        tx2.send(BotCommand::Privmsg((ftarget2, pretty)))
                                            .await
                                            .unwrap();
                                    }
                                    Err(err) => {
                                        println!("weather isn't initialised: {}", err);
                                    }
                                }
                            }
                            None => (),
                        }
                    });
                }
            }
        }

        "loc" | "location" => {
            let location = tokens.as_str();
            let loc = db.check_location(location);

            match loc {
                Ok(Some(l)) => {
                    let response = format!(
                        "https://www.openstreetmap.org/?mlat={}&mlon={}",
                        l.lat, l.lon
                    );
                    client.send_privmsg(msg.target, response).unwrap();
                }
                Ok(None) => {
                    let tx2 = tx2.clone();
                    let flocation = location.to_string().clone();
                    let ftarget = msg.target.clone();
                    let response = format!("No coordinates found for {} in database", location);
                    println!("{}", response);
                    tokio::spawn(async move {
                        let fetched_location = get_location(&flocation).await;
                        match fetched_location {
                            Ok(Some(l)) => tx2
                                .send(BotCommand::Location(flocation, ftarget, l))
                                .await
                                .unwrap(),
                            Ok(None) => {
                                let response =
                                    format!("Unable to fetch location data for {}", flocation);
                                println!("{}", &response);
                                tx2.send(BotCommand::Privmsg((ftarget, response)))
                                    .await
                                    .unwrap();
                            }
                            Err(err) => {
                                println!("Error fetching location data for {}", err)
                            }
                        }
                    });
                }
                Err(err) => println!("Error fetching location from database: {}", err),
            }
        }

        "seen" => match tokens.next() {
            Some(nick) => {
                let response = check_seen(nick, &db);
                client.send_privmsg(msg.target, response).unwrap();
            }
            None => {
                let response = "Hint: seen <nick>";
                client.send_privmsg(msg.target, response).unwrap();
            }
        },

        "tell" => match tokens.next() {
            Some(nick) => {
                let entry = Notification {
                    id: 0,
                    recipient: nick.to_string(),
                    via: msg.source.to_string(),
                    message: tokens.as_str().to_string(),
                };
                if let Err(err) = db.add_notification(&entry) {
                    println!("SQL error adding notification: {}", err);
                    return ();
                }
                let response = format!("ok, I'll tell {} that", nick);
                client.send_privmsg(msg.target, response).unwrap();
            }
            None => {
                let response = "Hint: tell <nick> <message";
                client.send_privmsg(msg.target, response).unwrap();
            }
        },
        _ => (),
    }
}

pub async fn process_titles(links: Vec<(String, String)>) -> Vec<(String, String)> {
    // the following is adapted from
    // https://stackoverflow.com/questions/63434977/how-can-i-spawn-asynchronous-methods-in-a-loop
    try_join_all(links.into_iter().map(|(t, l)| {
        spawn_blocking(|| {
            if let (target, Some(title)) = fetch_title(t, l) {
                let response = format!("↪ {}", title.replace("\n", " "));
                Some((target, response))
            } else {
                None
            }
        })
    }))
    .await
    .unwrap_or_default()
    .into_iter()
    .filter_map(|u| u)
    .collect()
}

fn fetch_title(target: String, url: String) -> (String, Option<String>) {
    //let response = reqwest::get(title).await.ok()?.text().await.ok()?;
    //let page = webpage::HTML::from_string(response, None);
    let opt = WebpageOptions {
        allow_insecure: true,
        follow_location: true,
        max_redirections: 10,
        timeout: STDDuration::from_secs(10),
        // a legitimate user agent is necessary for some sites (twitter)
        useragent: format!("Mozilla/5.0 boot-bot-rs/1.3.0"),
    };

    let page = Webpage::from_url(&url, opt);
    let mut title: Option<String> = None;
    let mut og_title: Option<String> = None;
    match page {
        Ok(mut page) => {
            title = page.html.title;
            og_title = page.html.meta.remove("og:title");
        }
        Err(_) => (),
    }

    match title {
        // youtube is inconsistent, the best option here would be to use the api, an invidious api,
        // or possibly sed youtube.com with an invidious instance
        Some(t) if t == "YouTube" => (target, og_title),
        Some(t) if t == "Pleroma" => (target, og_title),
        _ => (target, title),
    }
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
                match db.remove_notification(i.id) {
                    Err(err) => println!("SQL error checking notification: {}", err),
                    _ => (),
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
        useragent: format!("Mozilla/5.0 boot-bot-rs/1.3.0"),
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
    let w: CurrentWeather = weather(&coords, "metric", "en", api_key)?;

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
    let description = format!("{}", &uppercase(&weather.weather[0].description));
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

    let celsius = weather.main.temp.round();
    let fahrenheit = ((weather.main.temp * (9.0 / 5.0)) + 32_f64).round();

    let metric_wind = weather.wind.speed.round();
    let imperial_wind = (weather.wind.speed * 2.2369_f64).round();
    let wind = match weather.wind.gust {
        Some(g) => {
            let metric_gust = g.round();
            let imperial_gust = (g * 2.2369_f64).round();
            format!(
                "Wind: {} mph [{} m/s], Gust: {} mph [{} m/s]",
                metric_wind, imperial_wind, imperial_gust, metric_gust
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
    pub data_0: String,
    pub data_1: String,
}

#[derive(Debug, Deserialize)]
struct Coins {
    mts: i64,
    open: f32,
    close: f32,
    high: f32,
    low: f32,
    volume: f32,
}

pub async fn get_coins(coin: &str, time_frame: &str) -> Result<Coin, Error> {
    // TODO: add this to settings
    let opt = WebpageOptions {
        allow_insecure: true,
        follow_location: true,
        max_redirections: 10,
        timeout: STDDuration::from_secs(10),
        // a legitimate user agent is necessary for some sites (twitter)
        useragent: format!("Mozilla/5.0 boot-bot-rs/1.3.0"),
    };

    let (limit, time) = match time_frame {
        "15m" => (96, "15m"),
        "7D" => (29, "6h"),
        "14D" => (29, "12h"),
        "30D" => (31, "1D"),
        _ => (29, "6h"),
    };

    // we should be getting the correct coin name for this
    let url = format!(
        "https://api-pub.bitfinex.com/v2/candles/trade:{}:{}/hist?limit={}",
        time, coin, limit
    );

    let page = Webpage::from_url(&url, opt)?;

    // TODO - status codes
    //if page.http.response_code == 429 {
    //}

    let mut coins: Vec<Coins> = serde_json::from_str(&page.html.text_content)?;
    coins.reverse();
    let mut prices = Vec::<f32>::new();

    let mut count = 0;
    let mut initial: f32 = 0.0;
    let mut min: (f32, usize) = (0.0, 0);
    let mut max: (f32, usize) = (0.0, 0);
    let mut mean: f32 = 0.0;

    // what we want is the min, max, mean, values the prices
    // we also want the prices every hour (count % 4 == 0) for 15m values
    // the initial value is to colour code the initial bar which
    // will be coins[3] since we're only keeping hourly prices
    //
    // for weekly/fortnight values we collect an extra day for the initial value
    for c in &coins {
        if count == 0 {
            initial = c.close;
            min = (c.close, count);
            max = (c.close, count);
        } else {
            match time_frame {
                "15m" => {
                    if count % 4 == 0 {
                        prices.push(c.close);
                    }
                }
                _ => prices.push(c.close),
            }
            if c.close > max.0 {
                max = (c.close, count);
            }
            if c.close < min.0 {
                min = (c.close, count);
            }
        }
        mean += c.close;
        count += 1;
    }

    let len = coins.len();
    mean = mean / len as f32;

    let graph = graph(initial, prices);
    let graph = format!(
        "{} begin: ${} {} {} end: ${} {}",
        coin,
        coins[0].close,
        print_date(coins[0].mts, time_frame),
        graph,
        coins[len - 1].close,
        print_date(coins[len - 1].mts, time_frame),
    );

    let stats = format!(
        "{} high: ${} {} // mean: ${} // low: ${} {}",
        coin,
        max.0,
        print_date(coins[max.1].mts, time_frame),
        mean,
        min.0,
        print_date(coins[min.1].mts, time_frame),
    );

    let recent = coins.pop().unwrap();
    let result = Coin {
        coin: coin.to_string(),
        date: recent.mts,
        data_0: graph,
        data_1: stats,
    };

    Ok(result)
}

fn print_date(date: i64, time_frame: &str) -> String {
    let date = (date / 1000).to_string();
    let time = NaiveDateTime::parse_from_str(&date, "%s").unwrap();
    match time_frame {
        "7D" | "14D" | "30D" => time.format("(%v)").to_string(),
        _ => time.format("(%a %d %T UTC)").to_string(),
    }
}

// the following is adapted from
// https://github.com/jiri/rust-spark
fn graph(initial: f32, prices: Vec<f32>) -> String {
    let ticks = "▁▂▃▄▅▆▇█";

    /* XXX: This doesn't feel like idiomatic Rust */
    let mut min: f32 = f32_max;
    let mut max: f32 = 0.0;

    for &i in prices.iter() {
        if i > max {
            max = i;
        }
        if i < min {
            min = i;
        }
    }

    let ratio = if max == min {
        1.0
    } else {
        (ticks.chars().count() - 1) as f32 / (max - min)
    };

    let mut v = String::new();
    let mut count = 0;
    for p in prices.iter() {
        let ratio = ((p - min) * ratio).round() as usize;

        if count == 0 {
            if p > &initial {
                v.push_str(&format!("\x0303{}", ticks.chars().nth(ratio).unwrap()));
            } else {
                v.push_str(&format!("\x0304{}\x03", ticks.chars().nth(ratio).unwrap()));
            }
        } else {
            // if the current price is higher than the previous price
            // the bar should be green, else red
            if p > &prices[count - 1] {
                v.push_str(&format!("\x0303{}\x03", ticks.chars().nth(ratio).unwrap()));
            } else {
                v.push_str(&format!("\x0304{}\x03", ticks.chars().nth(ratio).unwrap()));
            }
        }
        count = count + 1;
    }

    v
}

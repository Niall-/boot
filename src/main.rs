#![feature(str_split_whitespace_as_str)]
use futures::prelude::*;
use irc::client::prelude::*;
mod bot;
mod messages;
mod settings;
mod sqlite;
use crate::bot::{check_notification, check_seen, Coin};
use crate::messages::Msg;
use crate::settings::Settings;
use crate::sqlite::{Database, Location, Notification, Seen};
use chrono::{Duration, NaiveDateTime, Utc};
use irc::client::ClientStream;
use messages::process_message;
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum BotCommand {
    Quit(String, String),
    Privmsg((String, String)),
    Links(Vec<(String, String)>),
    Seen(Seen),
    Location(String, String, Location),
    UpdateWeather(String, String, String),
    UpdateLocation(String, Location),
    UpdateCoins(Coin),
    Message(Msg),
}

async fn run_bot(
    mut stream: ClientStream,
    current_nick: &str,
    tx: mpsc::Sender<BotCommand>,
) -> Result<(), failure::Error> {
    while let Some(message) = stream.next().await.transpose()? {
        process_message(current_nick, &message, tx.clone()).await;
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), failure::Error> {
    let settings = Settings::load("config.toml")?;
    let db = if let Some(ref path) = settings.bot.db {
        Database::open(path)?
    } else {
        let path = "./database.sqlite";
        Database::open(path)?
    };
    let api_key = settings.bot.weather_api;
    let mut client = Client::from_config(settings.irc).await?;
    let stream = client.stream()?;
    client.identify()?;

    let (tx, mut rx) = mpsc::channel::<BotCommand>(32);
    let tx2 = tx.clone();

    let nick = client.current_nickname().to_string();
    tokio::spawn(async move { run_bot(stream, &nick, tx.clone()).await });

    while let Some(cmd) = rx.recv().await {
        match cmd {
            BotCommand::Quit(t, m) => {
                // this won't handle sanick, but it should be good enough
                let nick = client.current_nickname().to_string();
                if t == nick {
                    println!("Quit! {}, {}", t, m);
                    break;
                }
            }
            BotCommand::Privmsg(m) => client.send_privmsg(m.0, m.1).unwrap(),
            BotCommand::Links(u) => {
                let tx2 = tx2.clone();
                tokio::spawn(async move {
                    let titles = bot::process_titles(u).await;
                    for t in titles {
                        tx2.send(BotCommand::Privmsg(t)).await.unwrap();
                    }
                });
            }
            BotCommand::Seen(e) => {
                if let Err(err) = db.add_seen(&e) {
                    println!("SQL error adding seen: {}", err);
                };
            }
            BotCommand::Location(location, target, e) => {
                if let Err(err) = db.add_location(&location, &e) {
                    println!("SQL error adding location: {}", err);
                };

                let response = format!(
                    "https://www.openstreetmap.org/?mlat={}&mlon={}",
                    e.lat, e.lon
                );
                tx2.send(BotCommand::Privmsg((target, response)))
                    .await
                    .unwrap();
            }
            BotCommand::UpdateWeather(user, lat, lon) => {
                if let Err(err) = db.add_weather(&user, &lat, &lon) {
                    println!("SQL error updating weather: {}", err);
                };
            }
            BotCommand::UpdateLocation(loc, e) => {
                if let Err(err) = db.add_location(&loc, &e) {
                    println!("SQL error updating location: {}", err);
                };
            }
            BotCommand::UpdateCoins(coin) => {
                if let Err(err) = db.add_coins(&coin) {
                    println!("SQL error updating coins: {}", err);
                };
            }
            BotCommand::Message(msg) => {
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

                if msg.content.contains("oi") {
                    client.send_privmsg(&msg.target, "oi").unwrap();
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
                        continue;
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
                    continue;
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
                        let response = "Commands: repo | seen <nick> | tell <nick> <message> | weather <location> \
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
                                let coins = bot::get_coins(&coin, &time_frame).await;
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
                            continue;
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
                                    continue;
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
                                    let weather = bot::get_weather(&coords, &key).await;
                                    match weather {
                                        Ok(weather) => {
                                            let pretty = bot::print_weather(weather);
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
                                    let fetched_location = bot::get_location(&flocation).await;
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
                                            let response = format!(
                                                "Unable to fetch location for {}",
                                                flocation
                                            );
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
                                            let weather = bot::get_weather(&coords, &key).await;
                                            match weather {
                                                Ok(weather) => {
                                                    let pretty = bot::print_weather(weather);
                                                    tx2.send(BotCommand::Privmsg((
                                                        ftarget2, pretty,
                                                    )))
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
                                let response =
                                    format!("No coordinates found for {} in database", location);
                                println!("{}", response);
                                //client.send_privmsg(msg.target, response).unwrap();
                                tokio::spawn(async move {
                                    let fetched_location = bot::get_location(&flocation).await;
                                    match fetched_location {
                                        Ok(Some(l)) => tx2
                                            .send(BotCommand::Location(flocation, ftarget, l))
                                            .await
                                            .unwrap(),
                                        Ok(None) => {
                                            let response = format!(
                                                "Unable to fetch location data for {}",
                                                flocation
                                            );
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
                                continue;
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
        }
    }

    Ok(())
}

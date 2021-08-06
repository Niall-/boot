#![feature(str_split_whitespace_as_str)]
use futures::prelude::*;
use irc::client::prelude::*;
mod bot;
mod messages;
mod settings;
mod sqlite;
use crate::bot::{check_notification, check_seen};
use crate::messages::Msg;
use crate::settings::Settings;
use crate::sqlite::Database;
use crate::sqlite::{Location, Notification, Seen};
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

                // past this point we only care about interactions with the bot
                let nick = client.current_nickname().to_lowercase();
                let line = match &msg.content {
                    c if c.starts_with("./") => c.strip_prefix("./"),
                    c if c.starts_with(".") => c.strip_prefix("."),
                    c if c.starts_with("!") => c.strip_prefix("!"),
                    c if c.to_lowercase().starts_with(&nick) => {
                        let whitespace = c.find(char::is_whitespace);
                        match whitespace {
                            Some(w) => c.strip_prefix(&c[..w + 1]),
                            None => None,
                        }
                    }
                    _ => None,
                };

                if !line.is_some() {
                    continue;
                }

                let mut tokens = line.unwrap().split_whitespace();

                // i.e., 'boot: command'
                match tokens.next().map(|t| t.to_lowercase()) {
                    Some(c) if c == "repo" => {
                        let response = "https://github.com/niall-/boot";
                        client.send_privmsg(msg.target, response).unwrap();
                    }

                    Some(c) if c == "help" => {
                        let response = "Commands: repo | seen <nick> | tell <nick> <message> | weather <location>";
                        client.send_privmsg(msg.target, response).unwrap();
                    }

                    // TODO: figure out the borrowowing issue(s?) so code doesn't have to be
                    // duplicated as much here, and especially so that it can be
                    // separated out into its own functions
                    Some(c) if c == "weather" => {
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

                    Some(c) if c == "loc" => {
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

                    Some(c) if c == "seen" => match tokens.next() {
                        Some(nick) => {
                            let response = check_seen(nick, &db);
                            client.send_privmsg(msg.target, response).unwrap();
                        }
                        None => {
                            let response = "Hint: seen <nick>";
                            client.send_privmsg(msg.target, response).unwrap();
                        }
                    },

                    Some(c) if c == "tell" => match tokens.next() {
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

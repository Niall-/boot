#![feature(str_split_whitespace_as_str)]
use futures::prelude::*;
use irc::client::prelude::*;
mod bot;
mod messages;
mod settings;
mod sqlite;
//use crate::bot::{check_notification, check_seen, Coin};
use crate::bot::Coin;
use crate::messages::Msg;
use crate::settings::Settings;
use crate::sqlite::{Database, Location, Notification, Seen};
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
                bot::process_messages(msg, &db, &client, api_key.clone(), &tx2).await;
            }
        }
    }

    Ok(())
}

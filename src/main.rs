#![feature(str_split_whitespace_as_str)]
use futures::prelude::*;
use irc::client::prelude::*;
mod bot;
mod http;
mod messages;
mod settings;
mod sqlite;
//use crate::bot::{check_notification, check_seen, Coin};
use crate::bot::Coin;
use crate::http::{Req, ReqBuilder};
use crate::messages::Msg;
use crate::settings::Settings;
use crate::sqlite::{Database, Location, Notification, Seen};
use irc::client::ClientStream;
use messages::process_message;
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum Bot {
    Message(Msg),
    Links(Vec<(String, String)>),
    Privmsg(String, String),
    UpdateSeen(Seen),
    UpdateWeather(String, String, String),
    UpdateLocation(String, Location),
    UpdateCoins(Coin),
    Quit(String, String),
}

async fn run_bot(
    mut stream: ClientStream,
    current_nick: &str,
    tx: mpsc::Sender<Bot>,
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

    let req_client = ReqBuilder::new().build()?;

    let (tx, mut rx) = mpsc::channel::<Bot>(32);
    let tx2 = tx.clone();

    let nick = client.current_nickname().to_string();
    tokio::spawn(async move { run_bot(stream, &nick, tx.clone()).await });

    while let Some(cmd) = rx.recv().await {
        match cmd {
            Bot::Message(msg) => {
                bot::process_messages(msg, &db, &client, api_key.clone(), &tx2, req_client.clone())
                    .await;
            }
            Bot::Links(u) => {
                let tx2 = tx2.clone();
                let req_client = req_client.clone();
                tokio::spawn(async move {
                    let titles = bot::process_titles(u, req_client).await;
                    for t in titles {
                        tx2.send(Bot::Privmsg(t.0, t.1)).await.unwrap();
                    }
                });
            }
            Bot::Privmsg(t, m) => client.send_privmsg(t, m).unwrap(),
            Bot::UpdateSeen(e) => {
                if let Err(err) = db.add_seen(&e) {
                    println!("SQL error adding seen: {}", err);
                };
            }
            Bot::UpdateWeather(user, lat, lon) => {
                if let Err(err) = db.add_weather(&user, &lat, &lon) {
                    println!("SQL error updating weather: {}", err);
                };
            }
            Bot::UpdateLocation(loc, e) => {
                if let Err(err) = db.add_location(&loc, &e) {
                    println!("SQL error updating location: {}", err);
                };
            }
            Bot::UpdateCoins(coin) => {
                if let Err(err) = db.add_coins(&coin) {
                    println!("SQL error updating coins: {}", err);
                };
            }
            Bot::Quit(t, m) => {
                // this won't handle sanick, but it should be good enough
                let nick = client.current_nickname().to_string();
                if t == nick {
                    println!("Quit! {}, {}", t, m);
                    break;
                }
            }
        }
    }

    Ok(())
}

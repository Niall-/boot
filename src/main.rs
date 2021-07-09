#![feature(str_split_whitespace_as_str)]
#![allow(unused_imports)]
use futures::prelude::*;
use irc::client::prelude::*;
mod bot;
mod messages;
mod sqlite;
use crate::bot::{check_notification, check_seen};
use crate::sqlite::Database;
use crate::sqlite::{Notification, Seen};
use irc::client::ClientStream;
use messages::process_message;
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug)]
pub struct BotCommand {
    pub message: String,
    pub target: String,
    pub plugin: String,
    pub links: Option<Vec<String>>,
    pub seen: Option<Seen>,
    pub notification: Option<Notification>,
}
impl BotCommand {
    fn new(
        message: String,
        target: String,
        plugin: String,
        links: Option<Vec<String>>,
        seen: Option<Seen>,
        notification: Option<Notification>,
    ) -> BotCommand {
        BotCommand {
            message,
            target,
            plugin,
            links,
            seen,
            notification,
        }
    }
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
    let path = "./database.sqlite";
    let db = Database::open(&path)?;
    let mut client = Client::new("config.toml").await?;
    let stream = client.stream()?;
    client.identify()?;

    let (tx, mut rx) = mpsc::channel::<BotCommand>(32);
    let tx2 = tx.clone();

    tokio::spawn(async move { run_bot(stream, &"boot", tx.clone()).await });

    // this is starting to become quite the monstrosity
    while let Some(cmd) = rx.recv().await {
        match &cmd.plugin {
            p if p == "privmsg" => client.send_privmsg(cmd.target, cmd.message).unwrap(),
            p if p == "links" => match cmd.links {
                Some(u) => {
                    let tx2 = tx2.clone();
                    let target = cmd.target.to_string().clone();
                    tokio::spawn(async move {
                        let titles = bot::process_titles(u).await;
                        for t in titles {
                            let cmd = BotCommand::new(
                                t.to_string(),
                                target.to_string(),
                                "titles".to_string(),
                                None,
                                None,
                                None,
                            );
                            tx2.send(cmd).await.unwrap();
                        }
                    });
                }
                None => (),
            },
            p if p == "titles" => {
                client.send_privmsg(cmd.target, cmd.message).unwrap();
            }
            p if p == "add-seen" => match &cmd.seen {
                Some(entry) => {
                    if let Err(err) = db.add_seen(&entry) {
                        println!("SQL error adding seen: {}", err);
                    };
                }
                None => println!("Error! add-seen but Seen is empty"),
            },
            p if p == "check-seen" => {
                let response = check_seen(&cmd.message, &db);
                client.send_privmsg(cmd.target, response).unwrap();
            }
            p if p == "add-notification" => match &cmd.notification {
                Some(entry) => {
                    if let Err(err) = db.add_notification(&entry) {
                        println!("SQL error adding notification: {}", err);
                    }
                }
                None => println!("Error! add-notification but Notification is empty"),
            },
            p if p == "check-notification" => {
                let notifications = check_notification(&cmd.message, &db);
                for n in notifications {
                    client.send_privmsg(&cmd.target, &n).unwrap();
                }
            }
            _ => (),
        }
        //println!("Got command: {:?}", cmd);
    }

    Ok(())
}

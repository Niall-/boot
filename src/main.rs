#![feature(str_split_whitespace_as_str)]
use futures::prelude::*;
use irc::client::prelude::*;
mod bot;
mod messages;
mod sqlite;
use crate::bot::{check_notification, check_seen};
use crate::messages::Msg;
use crate::sqlite::Database;
use crate::sqlite::{Notification, Seen};
use irc::client::ClientStream;
use messages::process_message;
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum BotCommand {
    Privmsg((String, String)),
    Links(Vec<(String, String)>),
    Seen(Seen),
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
    let path = "./database.sqlite";
    let db = Database::open(&path)?;
    let mut client = Client::new("config.toml").await?;
    let stream = client.stream()?;
    client.identify()?;

    let (tx, mut rx) = mpsc::channel::<BotCommand>(32);
    let tx2 = tx.clone();

    tokio::spawn(async move { run_bot(stream, &"boot", tx.clone()).await });

    while let Some(cmd) = rx.recv().await {
        match cmd {
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
                let mut tokens = msg.content.split_whitespace();
                let next = tokens.next();
                match next {
                    Some(n)
                        if !n
                            .to_lowercase()
                            .starts_with(&msg.current_nick.to_lowercase()) =>
                    {
                        continue
                    }
                    _ => (),
                }

                // i.e., 'boot: command'
                match tokens.next().map(|t| t.to_lowercase()) {
                    Some(c) if c == "repo" => {
                        let response = "https://github.com/niall-/boot";
                        client.send_privmsg(msg.target, response).unwrap();
                    }

                    Some(c) if c == "help" => {
                        let response = "Commands: repo | seen <nick> | tell <nick> <message>";
                        client.send_privmsg(msg.target, response).unwrap();
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

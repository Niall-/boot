#![feature(str_split_whitespace_as_str)]
#![allow(unused_imports)]
use futures::prelude::*;
use irc::client::prelude::*;
mod bot;
mod messages;
mod sqlite;
use crate::sqlite::Database;
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
    pub priority: Priority,
}
impl BotCommand {
    fn new(message: String, target: String, plugin: String, priority: Priority) -> BotCommand {
        BotCommand {
            message,
            target,
            plugin,
            priority,
        }
    }
}
#[derive(Debug)]
pub enum Priority {
    High,
    Normal,
    Low,
}

async fn run_bot(tx: mpsc::Sender<BotCommand>) -> Result<(), failure::Error> {
    let mut client = Client::new("config.toml").await?;
    let mut stream = client.stream()?;
    client.identify()?;

    while let Some(message) = stream.next().await.transpose()? {
        process_message(&client.current_nickname(), &message, tx.clone()).await;
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), failure::Error> {
    //let path = "./database.sqlite";
    //let db = Database::open(&path)?;
    let (tx, mut rx) = mpsc::channel::<BotCommand>(32);

    tokio::spawn(async move { run_bot(tx.clone()).await });

    while let Some(cmd) = rx.recv().await {
        println!("Got command: {:?}", cmd);
    }

    Ok(())
}

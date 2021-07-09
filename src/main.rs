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

pub struct Commands {
    pub message: String,
    pub plugin: String,
    pub priority: Priority,
}
pub enum Priority {
    High,
    Normal,
    Low,
}

async fn run_bot() -> Result<(), failure::Error> {
    let mut client = Client::new("config.toml").await?;
    let mut stream = client.stream()?;
    client.identify()?;

    while let Some(message) = stream.next().await.transpose()? {
        process_message(&client.current_nickname(), &message).await;
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), failure::Error> {
    //let path = "./database.sqlite";
    //let db = Database::open(&path)?;

    tokio::spawn(async move { run_bot().await });

    loop {
        thread::sleep(Duration::from_millis(1000));
    }
}

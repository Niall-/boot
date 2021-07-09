#![feature(str_split_whitespace_as_str)]
use futures::prelude::*;
use irc::client::prelude::*;
mod bot;
use bot::process_message;
mod sqlite;
use crate::sqlite::Database;
use irc::client::ClientStream;
use std::thread;

async fn run_bot(mut stream: ClientStream) {
    while let Some(message) = stream.next().await.transpose().unwrap() {
        //process_message(&client, &db, &message).await;
    }
}

#[tokio::main]
async fn main() -> Result<(), failure::Error> {
    let path = "./database.sqlite";
    let db = Database::open(&path)?;
    let mut client = Client::new("config.toml").await?;
    let mut stream = client.stream()?;
    client.identify()?;

    tokio::spawn(async move { run_bot(stream).await });

    loop {
        thread::sleep_ms(1000);
    }

    Ok(())
}

use futures::prelude::*;
use irc::client::prelude::*;
mod bot;
use bot::process_message;
mod sqlite;
use crate::sqlite::Database;

#[tokio::main]
async fn main() -> Result<(), failure::Error> {
    let path = "./database.sqlite";
    let db = Database::open(&path)?;
    let mut client = Client::new("config.toml").await?;
    let mut stream = client.stream()?;
    client.identify()?;

    while let Some(message) = stream.next().await.transpose()? {
        process_message(&client, &db, &message).await;
    }

    Ok(())
}

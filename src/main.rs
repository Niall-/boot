use irc::client::prelude::*;
use futures::prelude::*;
mod bot;
use bot::process_message;

#[tokio::main]
async fn main() -> Result<(), failure::Error> {
    let mut client = Client::new("config.toml").await?;
    let mut stream = client.stream()?;
    client.identify()?;


    while let Some(message) = stream.next().await.transpose()? {
        process_message(&client, &message).await;
    }

    Ok(())
}

use crate::sqlite::Seen;
use crate::BotCommand;
use chrono::Utc;
use irc::client::prelude::*;
use linkify::{LinkFinder, LinkKind};
use rand::random;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct Msg {
    pub current_nick: String,
    pub source: String,
    // privmsg target (nick/channel) or target nick for kick/invite
    pub target: String,
    // somewhat confusingly this will be the channel for kick/invite
    // kick could use an additional field for the kick message,
    // however I don't think we'll ever really care about that
    pub content: String,
}
impl Msg {
    fn new(current_nick: String, source: String, target: String, content: String) -> Msg {
        Msg {
            current_nick,
            source,
            target,
            content,
        }
    }
}

pub async fn process_message(current_nick: &str, message: &Message, tx: mpsc::Sender<BotCommand>) {
    let source = message.source_nickname();
    let target = message.response_target();
    let nick = current_nick.to_string();

    match &message.command {
        Command::PRIVMSG(_target, message) => {
            privmsg(
                Msg::new(
                    nick,
                    source.unwrap().to_string(),
                    target.unwrap().to_string(),
                    message.to_string(),
                ),
                tx.clone(),
            )
            .await
        }
        Command::KICK(channel, user, _text) => {
            kick(
                Msg::new(
                    nick,
                    source.unwrap().to_string(),
                    user.to_string(),
                    channel.to_string(),
                ),
                tx.clone(),
            )
            .await
        }
        Command::INVITE(user, channel) => {
            invite(Msg::new(
                nick,
                source.unwrap().to_string(),
                user.to_string(),
                channel.to_string(),
            ))
            .await
        }
        _ => (),
    };
}

async fn privmsg(msg: Msg, tx: mpsc::Sender<BotCommand>) {
    if !msg.target.starts_with("#") {
        return;
    }

    let mut finder = LinkFinder::new();
    finder.kinds(&[LinkKind::Url]);
    let links: Vec<_> = finder.links(&msg.content).collect();
    let urls: Vec<(_, _)> = links
        .into_iter()
        .map(|x| (msg.target.to_string(), x.as_str().to_string()))
        .collect();
    tx.send(BotCommand::Links(urls)).await.unwrap();

    if msg.content.contains("🥾") || msg.content.contains("👢") {
        let y: f64 = random::<f64>();
        if y > 0.975 {
            let response = "https://www.youtube.com/watch?v=tfMcxmOBmpk".to_string();
            let target = msg.target.to_string();
            tx.send(BotCommand::Privmsg((target, response)))
                .await
                .unwrap();
        }
    }

    let entry = Seen {
        username: msg.source.to_string(),
        message: format!("saying: {}", &msg.content),
        time: Utc::now().to_rfc3339(),
    };
    tx.send(BotCommand::Seen(entry)).await.unwrap();

    tx.send(BotCommand::Message(msg)).await.unwrap();
}

async fn kick(msg: Msg, tx: mpsc::Sender<BotCommand>) {
    let entry = Seen {
        username: msg.source.to_string(),
        message: format!("being kicked from {}", &msg.target),
        time: Utc::now().to_rfc3339(),
    };
    tx.send(BotCommand::Seen(entry)).await.unwrap();
}

async fn invite(_msg: Msg) {}

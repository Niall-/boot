use crate::bot::process_titles;
use crate::sqlite::{Database, Notification, Seen};
use crate::BotCommand;
use chrono::{DateTime, Utc};
use chrono_humanize::{Accuracy, HumanTime, Tense};
use irc::client::prelude::*;
use linkify::{Link, LinkFinder, LinkKind};
use std::time::Duration;
use tokio::sync::mpsc;
use webpage::{Webpage, WebpageOptions};

#[derive(Debug)]
pub struct Msg<'a> {
    current_nick: &'a str,
    source: &'a str,
    // privmsg target (nick/channel) or target nick for kick/invite
    target: &'a str,
    // somewhat confusingly this will be the channel for kick/invite
    // kick could use an additional field for the kick message,
    // however I don't think we'll ever really care about that
    content: &'a str,
}
impl<'a> Msg<'a> {
    fn new(current_nick: &'a str, source: &'a str, target: &'a str, content: &'a str) -> Msg<'a> {
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

    match &message.command {
        Command::PRIVMSG(_target, message) => {
            privmsg(
                Msg::new(current_nick, source.unwrap(), target.unwrap(), message),
                tx.clone(),
            )
            .await
        }
        Command::KICK(channel, user, _text) => {
            kick(
                Msg::new(current_nick, source.unwrap(), user, channel),
                tx.clone(),
            )
            .await
        }
        Command::INVITE(nick, channel) => {
            invite(Msg::new(current_nick, source.unwrap(), nick, channel)).await
        }
        _ => (),
    };
}

async fn privmsg(msg: Msg<'_>, tx: mpsc::Sender<BotCommand>) {
    if msg.target.starts_with("#") {
        let mut finder = LinkFinder::new();
        finder.kinds(&[LinkKind::Url]);
        let links: Vec<_> = finder.links(&msg.content).collect();
        let urls: Vec<_> = links.into_iter().map(|x| x.as_str().to_string()).collect();
        let command = BotCommand::new(
            format!(""),
            msg.target.to_string(),
            "links".to_string(),
            Some(urls),
            None,
            None,
        );
        tx.send(command).await.unwrap();
        //process_titles(&msg, links).await;
    }

    let entry = Seen {
        username: msg.source.to_string(),
        message: format!("saying: {}", &msg.content),
        time: Utc::now().to_rfc3339(),
    };
    let command = BotCommand::new(
        format!(""),
        format!(""),
        "add-seen".to_string(),
        None,
        Some(entry),
        None,
    );
    tx.send(command).await.unwrap();
    //if let Err(err) = db.add_seen(&entry) {
    //    println!("SQL error adding seen: {}", err);
    //};

    // HACK: check_notification only returns at most 2 notifications
    // if user alice spams user bob with notifications, when bob speaks he will be spammed with all
    // of those notifications at once (with some rate limiting provided by the irc crate), with
    // this hack bob will only ever receive 2 messages when he speaks, giving some end user control
    // for whether the channel is going to be spammed
    // some ways to fix this: some persistence allowing for a user to receive any potential
    // messages over pm, limit number of messages a user can receive, etc
    //let notification = check_notification(&msg.source, &db);
    //for n in notification {
    //    //client.send_privmsg(&msg.target, &n).unwrap();
    //}

    // past this point we only care about interactions with the bot
    let mut tokens = msg.content.split_whitespace();
    let next = tokens.next();
    match next {
        Some(n)
            if !n
                .to_lowercase()
                .starts_with(&msg.current_nick.to_lowercase()) =>
        {
            return
        }
        _ => (),
    }

    // i.e., 'boot: command'
    match tokens.next().map(|t| t.to_lowercase()) {
        Some(c) if c == "repo" => {
            let command = BotCommand::new(
                format!("https://github.com/niall-/boot"),
                msg.target.to_string(),
                "privmsg".to_string(),
                None,
                None,
                None,
            );
            tx.send(command).await.unwrap();
        }
        Some(c) if c == "help" => {
            let command = BotCommand::new(
                format!("Commands: repo | seen <nick> | tell <nick> <message>"),
                msg.target.to_string(),
                "privmsg".to_string(),
                None,
                None,
                None,
            );
            tx.send(command).await.unwrap();
        }
        Some(c) if c == "seen" => {
            match tokens.next() {
                Some(nick) => {
                    //"".to_string(), //check_seen(nick, &db),
                    let command = BotCommand::new(
                        nick.to_string(),
                        msg.target.to_string(),
                        "check-seen".to_string(),
                        None,
                        None,
                        None,
                    );
                    tx.send(command).await.unwrap();
                }
                None => {
                    let command = BotCommand::new(
                        format!("Hint: seen <nick>"),
                        msg.target.to_string(),
                        "privmsg".to_string(),
                        None,
                        None,
                        None,
                    );
                    tx.send(command).await.unwrap();
                }
            };
            //client.send_privmsg(&msg.target, &response).unwrap();
        }
        Some(c) if c == "tell" => {
            let response = match tokens.next() {
                Some(nick) => {
                    let entry = Notification {
                        id: 0,
                        recipient: nick.to_string(),
                        via: msg.source.to_string(),
                        message: tokens.as_str().to_string(),
                    };
                    //if let Err(err) = db.add_notification(&entry) {
                    //    println!("SQL error adding notification: {}", err);
                    //};
                    format!("ok, I'll tell {} that", nick)
                }
                None => "Hint: tell <nick> <message".to_string(),
            };
            //client.send_privmsg(&msg.target, &response).unwrap();
        }
        _ => (),
    }
}

async fn kick(msg: Msg<'_>, tx: mpsc::Sender<BotCommand>) {
    let entry = Seen {
        username: msg.source.to_string(),
        message: format!("being kicked from {}", &msg.target),
        time: Utc::now().to_rfc3339(),
    };
    let command = BotCommand::new(
        format!(""),
        format!(""),
        "add-seen".to_string(),
        None,
        Some(entry),
        None,
    );
    tx.send(command).await.unwrap();

    //if let Err(err) = db.add_seen(&entry) {
    //    println!("SQL error adding seen: {}", err);
    //};
}

async fn invite(_msg: Msg<'_>) {}

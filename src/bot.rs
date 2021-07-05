use crate::sqlite::{Database, Notification, Seen};
use chrono::{DateTime, Utc};
use chrono_humanize::{Accuracy, HumanTime, Tense};
use irc::client::prelude::*;
use linkify::{Link, LinkFinder, LinkKind};
use std::time::Duration;
use webpage::{Webpage, WebpageOptions};

#[derive(Debug)]
struct Msg<'a> {
    our_nick: &'a str,
    source: &'a str,
    // privmsg target (nick/channel) or target nick for kick/invite
    target: &'a str,
    // somewhat confusingly this will be the channel for kick/invite
    // kick could use an additional field for the kick message,
    // however I don't think we'll ever really care about that
    content: &'a str,
}
impl<'a> Msg<'a> {
    fn new(our_nick: &'a str, source: &'a str, target: &'a str, content: &'a str) -> Msg<'a> {
        Msg {
            our_nick,
            source,
            target,
            content,
        }
    }
}

pub async fn process_message(client: &Client, db: &Database, message: &Message) {
    let our_nick = client.current_nickname();
    let source = message.source_nickname();
    let target = message.response_target();

    match &message.command {
        Command::PRIVMSG(_target, message) => {
            privmsg(
                &client,
                &db,
                Msg::new(our_nick, source.unwrap(), target.unwrap(), message),
            )
            .await
        }
        Command::KICK(channel, user, _text) => {
            kick(
                &client,
                &db,
                Msg::new(our_nick, source.unwrap(), user, channel),
            )
            .await
        }
        Command::INVITE(nick, channel) => {
            invite(
                &client,
                &db,
                Msg::new(our_nick, source.unwrap(), nick, channel),
            )
            .await
        }
        _ => (),
    };
}

async fn process_titles(client: &Client, msg: &Msg<'_>, links: Vec<Link<'_>>) {
    let urls: Vec<_> = links.into_iter().map(|x| x.as_str().to_string()).collect();

    // the following is adapted from
    // https://stackoverflow.com/questions/63434977/how-can-i-spawn-asynchronous-methods-in-a-loop
    // it's also completely overkill, I think the rest of the bot will operate mostly synchronously
    // except in this one extremely specific instance where somebody pasted multiple urls to irc
    let tasks: Vec<_> = urls
        .into_iter()
        .map(|l| tokio::spawn(async { fetch_title(l).await }))
        .collect();

    for task in tasks {
        match task.await.unwrap() {
            Some(title) => {
                let response = format!("↪ {}", title);
                client.send_privmsg(msg.target, response).unwrap();
            }
            None => (),
        }
    }
}

async fn fetch_title(url: String) -> Option<String> {
    //let response = reqwest::get(title).await.ok()?.text().await.ok()?;
    //let page = webpage::HTML::from_string(response, None);
    let opt = WebpageOptions {
        allow_insecure: true,
        follow_location: true,
        max_redirections: 10,
        timeout: Duration::from_secs(10),
        // a legitimate user agent is necessary for some sites (twitter)
        useragent: format!("Mozilla/5.0 boot-bot-rs/1.3.0"),
    };

    let page = Webpage::from_url(&url, opt);
    let mut title: Option<String> = None;
    let mut og_title: Option<String> = None;
    match page {
        Ok(mut page) => {
            title = page.html.title;
            og_title = page.html.meta.remove("og:title");
        }
        Err(_) => (),
    }

    match title {
        // youtube is inconsistent, the best option here would be to use the api, an invidious api,
        // or possibly sed youtube.com with an invidious instance
        Some(t) if t == "YouTube" => og_title,
        Some(t) if t == "Pleroma" => og_title,
        _ => title,
    }
}

fn check_seen(nick: &str, db: &Database) -> String {
    match db.check_seen(nick) {
        Ok(Some(p)) => {
            let time = Utc::now();
            let previous = DateTime::parse_from_rfc3339(&p.time).unwrap();
            let duration = time.signed_duration_since(previous);
            let human_time = HumanTime::from(duration).to_text_en(Accuracy::Rough, Tense::Past);
            format!("{} was last seen {} {}", p.username, human_time, p.message)
        }
        Ok(None) => format!("{} has not previously been seen", nick),
        Err(_err) => "SQL error".to_string(),
    }
}

fn check_notification(nick: &str, db: &Database) -> Vec<String> {
    let mut notification: Vec<_> = Vec::new();
    match db.check_notification(nick) {
        Ok(n) => {
            for i in n {
                let message = format!("{}, message from {}: {}", nick, i.via, i.message);
                notification.push(message);
                match db.remove_notification(i.id) {
                    Err(err) => println!("SQL error checking notification: {}", err),
                    _ => (),
                }
                if notification.len() > 1 {
                    break;
                }
            }
        }
        Err(_err) => (),
    }

    notification
}

async fn privmsg(client: &Client, db: &Database, msg: Msg<'_>) {
    if msg.target.starts_with("#") {
        let mut finder = LinkFinder::new();
        finder.kinds(&[LinkKind::Url]);
        let links: Vec<_> = finder.links(&msg.content).collect();
        process_titles(&client, &msg, links).await;
    }

    let entry = Seen {
        username: msg.source.to_string(),
        message: format!("saying: {}", &msg.content),
        time: Utc::now().to_rfc3339(),
    };
    if let Err(err) = db.add_seen(&entry) {
        println!("SQL error adding seen: {}", err);
    };

    // HACK: check_notification only returns at most 2 notifications
    // if user alice spams user bob with notifications, when bob speaks he will be spammed with all
    // of those notifications at once (with some rate limiting provided by the irc crate), with
    // this hack bob will only ever receive 2 messages when he speaks, giving some end user control
    // for whether the channel is going to be spammed
    // some ways to fix this: some persistence allowing for a user to receive any potential
    // messages over pm, limit number of messages a user can receive, etc
    let notification = check_notification(&msg.source, &db);
    for n in notification {
        client.send_privmsg(&msg.target, &n).unwrap();
    }

    // past this point we only care about interactions with the bot
    let mut tokens = msg.content.split_whitespace();
    let next = tokens.next();
    match next {
        Some(n) if !n.to_lowercase().starts_with(&msg.our_nick.to_lowercase()) => return,
        _ => (),
    }

    // i.e., 'boot: command'
    match tokens.next().map(|t| t.to_lowercase()) {
        Some(c) if c == "repo" => client
            .send_privmsg(&msg.target, "https://github.com/niall-/boot")
            .unwrap(),
        Some(c) if c == "seen" => {
            let response = match tokens.next() {
                Some(nick) => check_seen(nick, &db),
                None => "Hint: seen <nick>".to_string(),
            };
            client.send_privmsg(&msg.target, &response).unwrap();
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
                    if let Err(err) = db.add_notification(&entry) {
                        println!("SQL error adding notification: {}", err);
                    };
                    format!("ok, I'll tell {} that", nick)
                }
                None => "Hint: tell <nick> <message".to_string(),
            };
            client.send_privmsg(&msg.target, &response).unwrap();
        }
        Some(c) if c == "help" => {
            let response = format!("Commands: repo | seen <nick> | tell <nick> <message>");
            client.send_privmsg(&msg.target, &response).unwrap();
        }
        _ => (),
    }
}

async fn kick(_client: &Client, db: &Database, msg: Msg<'_>) {
    let entry = Seen {
        username: msg.source.to_string(),
        message: format!("being kicked from {}", &msg.target),
        time: Utc::now().to_rfc3339(),
    };

    if let Err(err) = db.add_seen(&entry) {
        println!("SQL error adding seen: {}", err);
    };
}

async fn invite(_client: &Client, _db: &Database, _msg: Msg<'_>) {}

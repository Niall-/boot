use crate::sqlite::{Database, Seen};
use chrono::{DateTime, Utc};
use chrono_humanize::{Accuracy, HumanTime, Tense};
use irc::client::prelude::*;
use linkify::{Link, LinkFinder, LinkKind};
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
    let page = Webpage::from_url(&url, WebpageOptions::default());
    match page {
        Ok(page) => page.html.title,
        Err(_) => None,
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

    // past this point we only care about interactions with the bot
    let mut tokens = msg.content.split_whitespace();
    let next = tokens.next();
    match next {
        Some(n) if !n.starts_with(&msg.our_nick.to_lowercase()) => return,
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

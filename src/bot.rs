use crate::messages::Msg;
use crate::sqlite::{Database, Notification, Seen};
use chrono::{DateTime, Utc};
use chrono_humanize::{Accuracy, HumanTime, Tense};
use irc::client::prelude::*;
use linkify::{Link, LinkFinder, LinkKind};
use std::time::Duration;
use webpage::{Webpage, WebpageOptions};

pub async fn process_titles(links: Vec<String>) -> Vec<String> {
    // the following is adapted from
    // https://stackoverflow.com/questions/63434977/how-can-i-spawn-asynchronous-methods-in-a-loop
    // it's also completely overkill, I think the rest of the bot will operate mostly synchronously
    // except in this one extremely specific instance where somebody pasted multiple urls to irc
    let tasks: Vec<_> = links
        .into_iter()
        .map(|l| tokio::spawn(async { fetch_title(l).await }))
        .collect();

    let mut titles = Vec::new();
    for task in tasks {
        match task.await.unwrap() {
            Some(title) => {
                let response = format!("↪ {}", title);
                titles.push(response);
                //client.send_privmsg(msg.target, response).unwrap();
            }
            None => (),
        }
    }

    titles
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

pub fn check_seen(nick: &str, db: &Database) -> String {
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

pub fn check_notification(nick: &str, db: &Database) -> Vec<String> {
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

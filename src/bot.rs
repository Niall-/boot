use crate::sqlite::{Database, Location};
use chrono::{DateTime, Utc};
use chrono_humanize::{Accuracy, HumanTime, Tense};
use failure::Error;
use std::time::Duration;
use urlencoding::encode;
use webpage::{Webpage, WebpageOptions};

pub async fn process_titles(links: Vec<(String, String)>) -> Vec<(String, String)> {
    // the following is adapted from
    // https://stackoverflow.com/questions/63434977/how-can-i-spawn-asynchronous-methods-in-a-loop
    let tasks: Vec<_> = links
        .into_iter()
        .map(|(t, l)| tokio::spawn(async { fetch_title(t, l).await }))
        .collect();

    let mut titles = Vec::new();
    for task in tasks {
        let fetched = task.await.unwrap();
        match fetched.1 {
            Some(title) => {
                let response = format!("↪ {}", title.replace("\n", " "));
                titles.push((fetched.0, response));
            }
            None => (),
        }
    }

    titles
}

async fn fetch_title(target: String, url: String) -> (String, Option<String>) {
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
        Some(t) if t == "YouTube" => (target, og_title),
        Some(t) if t == "Pleroma" => (target, og_title),
        _ => (target, title),
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

pub fn check_location(loc: &str, db: &Database) -> Result<Option<Location>, Error> {
    if let Ok(Some(l)) = db.check_location(loc) {
        println!("in db");
        return Ok(Some(l));
    };

    println!("not in db");
    // TODO: add this to settings
    let opt = WebpageOptions {
        allow_insecure: true,
        follow_location: true,
        max_redirections: 10,
        timeout: Duration::from_secs(10),
        // a legitimate user agent is necessary for some sites (twitter)
        useragent: format!("Mozilla/5.0 boot-bot-rs/1.3.0"),
    };

    // TODO: this throws an error when a city doesn't exist for a location (i.e., it's a county)
    // TODO: nominatim has a strict limit of 1 request per second, while the channel I run the
    // bot in most certainly won't exceed this limit and I don't think it's likely many channels
    // will either (how many users are going to request weather before an op kicks the bot?)
    // something should be done about this soon to respect nominatim's TOS
    let url = format!(
        "https://nominatim.openstreetmap.org/search?q={}&format=json&addressdetails=1&limit=1",
        &encode(loc)
    );

    let page = Webpage::from_url(&url, opt)?;
    let mut entry: Vec<Location> = serde_json::from_str(&page.html.text_content)?;

    if let Err(err) = db.add_location(loc, &entry[0]) {
        println!("SQL error adding location: {}", err);
    };

    Ok(entry.pop())
}

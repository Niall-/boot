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

pub async fn process_message(client: &Client, message: &Message) {
    let our_nick = client.current_nickname();
    let source = message.source_nickname();
    let target = message.response_target();

    match &message.command {
        Command::PRIVMSG(_target, message) => {
            privmsg(
                &client,
                Msg::new(our_nick, source.unwrap(), target.unwrap(), message),
            )
            .await
        }
        Command::KICK(channel, user, _text) => {
            kick(&client, Msg::new(our_nick, source.unwrap(), user, channel)).await
        }
        Command::INVITE(nick, channel) => {
            invite(&client, Msg::new(our_nick, source.unwrap(), nick, channel)).await
        }
        _ => (),
    };
}

async fn privmsg(client: &Client, msg: Msg<'_>) {
    if msg.target.starts_with("#") {
        let mut finder = LinkFinder::new();
        finder.kinds(&[LinkKind::Url]);
        let links: Vec<_> = finder.links(&msg.content).collect();
        process_titles(&client, &msg, links).await;
    }

    // past this point we only care about interactions with the bot
    let mut tokens = msg.content.split_whitespace();
    let next = tokens.next();

    match next {
        Some(n) if !n.starts_with(&msg.our_nick.to_lowercase()) => return,
        _ => (),
    }

    // i.e., 'boot: command'
    match tokens.next().map(|t| t.to_lowercase()) {
        Some(t) if t == "repo" => client
            .send_privmsg(&msg.target, "https://github.com/niall-/boot")
            .unwrap(),
        _ => (),
    }
}

async fn process_titles(client: &Client, msg: &Msg<'_>, links: Vec<Link<'_>>) {
    let urls = links.into_iter().map(|x| x.as_str().to_string());

    // the following is adapted from
    // https://stackoverflow.com/questions/63434977/how-can-i-spawn-asynchronous-methods-in-a-loop
    let tasks: Vec<_> = urls
        .into_iter()
        .map(|l| tokio::spawn(async { fetch_title(l).await }))
        .collect();
    let mut results = vec![];
    for task in tasks {
        results.push(task.await.unwrap());
    }

    for r in results {
        match r {
            Some(title) => {
                let response = format!("↪ {}", title);
                client.send_privmsg(msg.target, response).unwrap();
            }
            None => (),
        }
    }
}

async fn fetch_title(title: String) -> Option<String> {
    //let response = reqwest::get(title).await.ok()?.text().await.ok()?;
    //let page = webpage::HTML::from_string(response, None);
    let page = Webpage::from_url(&title, WebpageOptions::default());
    match page {
        Ok(page) => page.html.title,
        Err(_) => None,
    }
}

async fn kick(_client: &Client, _msg: Msg<'_>) {}

async fn invite(_client: &Client, _msg: Msg<'_>) {}

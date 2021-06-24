use irc::client::prelude::*;

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
        Command::PRIVMSG(_target, message) => privmsg(
            &client,
            Msg::new(our_nick, source.unwrap(), target.unwrap(), message),
        ).await,
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
    let mut tokens = msg.content.split_whitespace();
    let next = tokens.next();

    // past this point we only care about interactions with the bot
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

async fn kick(client: &Client, msg: Msg<'_>) {}

async fn invite(client: &Client, msg: Msg<'_>) {}

#![feature(str_split_whitespace_remainder)]
use futures::prelude::*;
use irc::client::prelude::*;
mod bot;
mod http;
mod messages;
mod settings;
mod sqlite;
//use crate::bot::{check_notification, check_seen, Coin};
use crate::bot::Coin;
use crate::http::{Req, ReqBuilder};
use crate::messages::Msg;
use crate::settings::Settings;
use crate::sqlite::{Database, Location, Notification, Seen};
use irc::client::ClientStream;
use messages::process_message;
use rand::prelude::IteratorRandom;
use rand::{thread_rng, Rng};
use std::fmt::{Display, Error, Formatter, Write};
use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum Bot {
    Message(Msg),
    Links(Vec<(String, String)>),
    Privmsg(String, String),
    UpdateSeen(Seen),
    UpdateWeather(String, String, String),
    UpdateLocation(String, Location),
    UpdateCoins(Coin),
    Quit(String, String),
    Hang(String, String),
    HangGuess(String, String),
}

struct Hang {
    started: bool,
    word: String,
    state: String,
    guesses: Vec<String>,
    attempts: u8,
}

impl Default for Hang {
    fn default() -> Hang {
        Hang {
            started: false,
            word: "".to_string(),
            state: "".to_string(),
            guesses: Vec::new(),
            attempts: 0,
        }
    }
}

// credits: 99% dilflover69, 1% me
pub struct PrintCharsNicely<'a>(&'a Vec<String>);

impl Display for PrintCharsNicely<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.write_char('[')?;

        for (i, c) in self.0.iter().enumerate() {
            if i != 0 {
                f.write_str(", ")?;
            }
            f.write_str(c)?;
        }

        f.write_char(']')
    }
}

enum WordType {
    Short,
    Medium,
    Long,
}

// https://stackoverflow.com/questions/50788009/how-do-i-get-a-random-line-from-a-file
const FILENAME: &str = "/usr/share/dict/british-english";

fn find_word(style: WordType) -> String {
    let f = File::open(FILENAME)
        .unwrap_or_else(|e| panic!("(;_;) file not found: {}: {}", FILENAME, e));
    let f = BufReader::new(f);

    let lines = f
        .lines()
        .map(|l| l.expect("readerror"))
        .filter(|l| !l.ends_with("'s"))
        .filter(|l| match style {
            WordType::Short => l.len() < 6,
            WordType::Medium => (4..9).contains(&l.len()),
            WordType::Long => l.len() > 8,
        });

    lines.choose(&mut rand::thread_rng()).expect("emptyfile")
}

async fn run_bot(
    mut stream: ClientStream,
    current_nick: &str,
    tx: mpsc::Sender<Bot>,
) -> Result<(), failure::Error> {
    while let Some(message) = stream.next().await.transpose()? {
        process_message(current_nick, &message, tx.clone()).await;
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), failure::Error> {
    let settings = Settings::load("config.toml")?;
    let db = if let Some(ref path) = settings.bot.db {
        Database::open(path)?
    } else {
        let path = "./database.sqlite";
        Database::open(path)?
    };
    let api_key = settings.bot.weather_api;
    let mut client = Client::from_config(settings.irc).await?;
    let stream = client.stream()?;
    client.identify()?;

    let req_client = ReqBuilder::new().build()?;

    let (tx, mut rx) = mpsc::channel::<Bot>(32);
    let tx2 = tx.clone();

    let nick = client.current_nickname().to_string();
    tokio::spawn(async move { run_bot(stream, &nick, tx.clone()).await });

    let mut rng = thread_rng();
    let mut hangman: Hang = Hang::default();

    while let Some(cmd) = rx.recv().await {
        match cmd {
            Bot::Message(msg) => {
                bot::process_messages(msg, &db, &client, api_key.clone(), &tx2, req_client.clone())
                    .await;
            }
            Bot::Links(u) => {
                let tx2 = tx2.clone();
                let req_client = req_client.clone();
                tokio::spawn(async move {
                    let titles = bot::process_titles(u, req_client).await;
                    for t in titles {
                        tx2.send(Bot::Privmsg(t.0, t.1)).await.unwrap();
                    }
                });
            }
            Bot::Privmsg(t, m) => client.send_privmsg(t, m).unwrap(),
            Bot::UpdateSeen(e) => {
                if let Err(err) = db.add_seen(&e) {
                    println!("SQL error adding seen: {}", err);
                };
            }
            Bot::UpdateWeather(user, lat, lon) => {
                if let Err(err) = db.add_weather(&user, &lat, &lon) {
                    println!("SQL error updating weather: {}", err);
                };
            }
            Bot::UpdateLocation(loc, e) => {
                if let Err(err) = db.add_location(&loc, &e) {
                    println!("SQL error updating location: {}", err);
                };
            }
            Bot::UpdateCoins(coin) => {
                if let Err(err) = db.add_coins(&coin) {
                    println!("SQL error updating coins: {}", err);
                };
            }
            Bot::Quit(t, m) => {
                // this won't handle sanick, but it should be good enough
                let nick = client.current_nickname().to_string();
                if t == nick {
                    println!("Quit! {}, {}", t, m);
                    break;
                }
            }
            Bot::HangGuess(t, w) => {
                let lengths: [&str; 4] = ["<start>", "short", "medium", "long"];
                if lengths.contains(&&w[..]) {
                    if hangman.started {
                        client
                            .send_privmsg(t, "A game is already in progress!")
                            .unwrap();
                        continue;
                    } else {
                        hangman.started = true;
                        let style = match w.as_ref() {
                            "short" => WordType::Short,
                            "medium" => WordType::Medium,
                            "long" => WordType::Long,
                            _ => WordType::Medium,
                        };
                        hangman.word = find_word(style).to_lowercase();
                        let replaced: String = hangman
                            .word
                            .chars()
                            .map(|x| match x {
                                'a'..='z' => '-',
                                'A'..='Z' => '-',
                                _ => x,
                            })
                            .collect();
                        hangman.state = replaced;
                        client
                            .send_privmsg(
                                t,
                                format!(
                                    "{} {}/7 {}",
                                    &hangman.state,
                                    &hangman.attempts,
                                    PrintCharsNicely(&hangman.guesses)
                                ),
                            )
                            .unwrap();
                        continue;
                    }
                } else if w == hangman.word {
                    client
                        .send_privmsg(
                            t,
                            format!("A winner is you! The word was {}.", &hangman.word),
                        )
                        .unwrap();
                    hangman = Hang::default();
                }
            }
            Bot::Hang(t, l) => {
                if !hangman.started {
                    continue;
                }

                if !hangman.word.contains(&l) {
                    if hangman.guesses.contains(&l) {
                        client
                            .send_privmsg(
                                t,
                                format!(
                                    "{} {}/7 {}",
                                    &hangman.state,
                                    &hangman.attempts,
                                    PrintCharsNicely(&hangman.guesses)
                                ),
                            )
                            .unwrap();
                        continue;
                    }

                    hangman.guesses.push(l);
                    hangman.attempts += 1;

                    if hangman.attempts >= 7 {
                        let n = rng.gen_range(1..100) > 50;
                        let o: u32 = rng.gen_range(1..100);

                        let mut dead: Vec<String> = vec![
                            "  +---+".to_string(),
                            "  |   |".to_string(),
                            "  O   |".to_string(),
                            " /|\\  |".to_string(),
                            " /`\\  |".to_string(),
                            "      |".to_string(),
                            "=======".to_string(),
                        ];

                        if n {
                            dead[4] = " / \\  |".to_string();
                        }

                        if o > 95 {
                            for i in dead {
                                client.send_privmsg(&t, i).unwrap();
                            }
                        }

                        client
                            .send_privmsg(
                                t,
                                format!(
                                    "{} dead, jim! The word was {}.",
                                    if n { "She's" } else { "He's" },
                                    hangman.word
                                ),
                            )
                            .unwrap();

                        hangman = Hang::default();
                        continue;
                    }

                    client
                        .send_privmsg(
                            t,
                            format!(
                                "{} {}/7 {}",
                                &hangman.state,
                                &hangman.attempts,
                                PrintCharsNicely(&hangman.guesses)
                            ),
                        )
                        .unwrap();
                    continue;
                }

                let indices: Vec<_> = hangman.word.match_indices(&l).collect();
                for i in indices {
                    hangman.state.replace_range(i.0..i.0 + 1, i.1);
                }

                if hangman.state == hangman.word {
                    client
                        .send_privmsg(
                            t,
                            format!("A winner is you! The word was {}.", &hangman.word),
                        )
                        .unwrap();
                    hangman = Hang::default();
                    continue;
                }

                client
                    .send_privmsg(
                        t,
                        format!(
                            "{} {}/7 {}",
                            &hangman.state,
                            &hangman.attempts,
                            PrintCharsNicely(&hangman.guesses)
                        ),
                    )
                    .unwrap();
            }
        }
    }

    Ok(())
}

use crate::sqlite::{Database, Location};
use chrono::{DateTime, NaiveDateTime, Utc};
use chrono_humanize::{Accuracy, HumanTime, Tense};
use failure::Error;
use openweathermap::blocking::weather;
use openweathermap::CurrentWeather;
use serde::Deserialize;
use std::f32::MAX as f32_max;
use std::time::Duration as STDDuration;
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
        timeout: STDDuration::from_secs(10),
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

pub async fn get_location(loc: &str) -> Result<Option<Location>, Error> {
    // TODO: add this to settings
    let opt = WebpageOptions {
        allow_insecure: true,
        follow_location: true,
        max_redirections: 10,
        timeout: STDDuration::from_secs(10),
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

    Ok(entry.pop())
}

pub async fn get_weather(coords: &str, api_key: &str) -> Result<CurrentWeather, String> {
    let w: CurrentWeather = weather(&coords, "metric", "en", api_key)?;

    Ok(w)
}

pub fn print_weather(weather: CurrentWeather) -> String {
    // this is dumb, it's only necessary because OpenWeatherMap doesn't fully capitalise weather
    // conditions, see: https://openweathermap.org/weather-conditions
    // https://stackoverflow.com/questions/38406793/why-is-capitalizing-the-first-letter-of-a-string-so-convoluted-in-rust/38406885#38406885
    fn uppercase(s: &str) -> String {
        let mut c = s.chars();
        match c.next() {
            None => String::new(),
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        }
    }

    let location = &format!("{}, {}", weather.name, weather.sys.country);

    // if the weather condition is cloudy add cloud coverage
    // https://openweathermap.org/weather-conditions
    // the 700..=781 range has some conditions like
    // mist/haze/fog but I don't think cloud coverage matters there
    let description = format!("{}", &uppercase(&weather.weather[0].description));
    let description = match weather.weather[0].id {
        // thunderstorms
        200..=232 => format!("{}, {}% cv", description, weather.clouds.all),
        // drizzle
        300..=321 => format!("{}, {}% cv", description, weather.clouds.all),
        // rain
        500..=531 => format!("{}, {}% cv", description, weather.clouds.all),
        // snow
        600..=622 => format!("{}, {}% cv", description, weather.clouds.all),
        // clouds
        801..=804 => format!("{}, {}% cv", description, weather.clouds.all),
        _ => description,
    };

    // OpenWeatherMap provides sunrise/sunset in UTC (Unix time)
    // it also provides an offset in seconds, in practice we can
    // add it to UTC Unix time and get a naive local time but this isn't ideal
    let sunrise = weather.sys.sunrise.wrapping_add(weather.timezone);
    let sunset = weather.sys.sunset.wrapping_add(weather.timezone);
    let sunrise = match NaiveDateTime::parse_from_str(&sunrise.to_string(), "%s") {
        Ok(s) => s.format("%l:%M%p").to_string(),
        Err(_) => "Failed to parse time".to_string(),
    };
    let sunset = match NaiveDateTime::parse_from_str(&sunset.to_string(), "%s") {
        Ok(s) => s.format("%l:%M%p").to_string(),
        Err(_) => "Failed to parse time".to_string(),
    };

    let celsius = weather.main.temp.round();
    let fahrenheit = ((weather.main.temp * (9.0 / 5.0)) + 32_f64).round();

    let metric_wind = weather.wind.speed.round();
    let imperial_wind = (weather.wind.speed * 2.2369_f64).round();
    let wind = match weather.wind.gust {
        Some(g) => {
            let metric_gust = g.round();
            let imperial_gust = (g * 2.2369_f64).round();
            format!(
                "Wind: {} mph [{} m/s], Gust: {} mph [{} m/s]",
                metric_wind, imperial_wind, imperial_gust, metric_gust
            )
        }
        None => {
            format!("Wind: {} mph [{} m/s]", metric_wind, imperial_wind)
        }
    };

    let direction = [
        "↓ N", "↙ NE", "← E", "↖ SE", "↑ S", "↗ SW", "→ W", "↘ NW", "↓ N",
    ];
    let degrees = weather.wind.deg.rem_euclid(360.0).round() as usize / 45;

    format!("Weather for {}: {}, {}% Humidity | Temp: {}°C [{}°F] | {} coming from {} - {}° | Sunrise: {} | Sunset: {}",
            location, description, weather.main.humidity,
            celsius, fahrenheit,
            wind, direction[degrees], weather.wind.deg,
            sunrise, sunset)
}

#[derive(Debug, Deserialize, Clone)]
pub struct Coin {
    pub coin: String,
    pub date: i64,
    pub data_0: String,
    pub data_1: String,
}

#[derive(Debug, Deserialize)]
struct Coins {
    mts: i64,
    open: f32,
    close: f32,
    high: f32,
    low: f32,
    volume: f32,
}

pub async fn get_coins(coin: &str) -> Result<Coin, Error> {
    // TODO: add this to settings
    let opt = WebpageOptions {
        allow_insecure: true,
        follow_location: true,
        max_redirections: 10,
        timeout: STDDuration::from_secs(10),
        // a legitimate user agent is necessary for some sites (twitter)
        useragent: format!("Mozilla/5.0 boot-bot-rs/1.3.0"),
    };

    // we should be getting the correct coin name for this
    let url = format!(
        "https://api-pub.bitfinex.com/v2/candles/trade:15m:{}/hist?limit=96",
        coin
    );

    let page = Webpage::from_url(&url, opt)?;


    // TODO - status codes
    //if page.http.response_code == 429 {
    //}

    let mut coins: Vec<Coins> = serde_json::from_str(&page.html.text_content)?;
    coins.reverse();
    let mut prices = Vec::<f32>::new();

    let mut count = 0;
    let mut initial: f32 = 0.0;
    let mut min: (f32, usize) = (0.0, 0);
    let mut max: (f32, usize) = (0.0, 0);
    let mut mean: f32 = 0.0;

    // what we want is the min, max, mean, values the prices
    // we also want the prices every hour (count % 4 == 0)
    // the initial value is to colour code the initial bar which
    // will be coins[3] since we're only keeping hourly prices
    for c in &coins {
        if count == 0 {
            initial = c.close;
            min = (c.close, count);
            max = (c.close, count);
        } else {
            if count % 4 == 0 {
                prices.push(c.close);
            }
            if c.close > max.0 {
                max = (c.close, count);
            }
            if c.close < min.0 {
                min = (c.close, count);
            }
        }
        mean += c.close;
        count += 1;
    }

    let len = coins.len();
    mean = mean / len as f32;

    let graph = graph(initial, prices);
    let graph = format!(
        "{} begin: ${} {} {} end: ${} {}",
        coin,
        coins[0].close,
        print_date(coins[0].mts),
        graph,
        coins[len - 1].close,
        print_date(coins[len - 1].mts),
    );

    let stats = format!(
        "{} high: ${} {} // mean: ${} // low: ${} {}",
        coin,
        max.0,
        print_date(coins[max.1].mts),
        mean,
        min.0,
        print_date(coins[min.1].mts),
    );

    let recent = coins.pop().unwrap();
    let result = Coin {
        coin: coin.to_string(),
        date: recent.mts,
        data_0: graph,
        data_1: stats,
    };

    Ok(result)
}

fn print_date(date: i64) -> String {
    let date = (date / 1000).to_string();
    let time = NaiveDateTime::parse_from_str(&date, "%s").unwrap();
    time.format("(%a %d %T UTC)").to_string()
}

// the following is adapted from
// https://github.com/jiri/rust-spark
fn graph(initial: f32, prices: Vec<f32>) -> String {
    let ticks = "▁▂▃▄▅▆▇█";

    /* XXX: This doesn't feel like idiomatic Rust */
    let mut min: f32 = f32_max;
    let mut max: f32 = 0.0;

    for &i in prices.iter() {
        if i > max {
            max = i;
        }
        if i < min {
            min = i;
        }
    }

    let ratio = if max == min {
        1.0
    } else {
        (ticks.chars().count() - 1) as f32 / (max - min)
    };

    let mut v = String::new();
    let mut count = 0;
    for p in prices.iter() {
        let ratio = ((p - min) * ratio).round() as usize;

        if count == 0 {
            if p > &initial {
                v.push_str(&format!("\x0303{}", ticks.chars().nth(ratio).unwrap()));
            } else {
                v.push_str(&format!("\x0304{}\x03", ticks.chars().nth(ratio).unwrap()));
            }
        } else {
            // if the current price is higher than the previous price
            // the bar should be green, else red
            if p > &prices[count - 1] {
                v.push_str(&format!("\x0303{}\x03", ticks.chars().nth(ratio).unwrap()));
            } else {
                v.push_str(&format!("\x0304{}\x03", ticks.chars().nth(ratio).unwrap()));
            }
        }
        count = count + 1;
    }

    v
}

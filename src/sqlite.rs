use crate::bot::Coin;
use failure::Error;
use rusqlite::{params, Connection};
use serde::Deserialize;
use std::path::Path;

pub struct Database {
    db: Connection,
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let db = Connection::open(path)?;
        db.execute(
            "CREATE TABLE IF NOT EXISTS seen (
            username    TEXT PRIMARY KEY,
            message     TEXT NOT NULL,
            time        TEXT NOT NULL)",
            [],
        )?;
        db.execute(
            "CREATE TABLE IF NOT EXISTS notifications (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            recipient   TEXT NOT NULL,
            via         TEXT NOT NULL,
            message     TEXT NOT NULL)",
            [],
        )?;
        db.execute(
            "CREATE TABLE IF NOT EXISTS locations (
            loc         TEXT PRIMARY KEY,
            lat         TEXT NOT NULL,
            lon         TEXT NOT NULL,
            city        TEXT,
            country     TEXT NOT NULL)",
            [],
        )?;
        db.execute(
            "CREATE TABLE IF NOT EXISTS weather (
            username    TEXT PRIMARY KEY,
            lat         TEXT NOT NULL,
            lon         TEXT NOT NULL)",
            [],
        )?;
        db.execute(
            "CREATE TABLE IF NOT EXISTS coins (
            coin        TEXT PRIMARY KEY,
            date        INTEGER NOT NULL,
            data_0      TEXT NOT NULL,
            data_1      TEXT NOT NULL)",
            [],
        )?;
        Ok(Self { db })
    }

    pub fn add_seen(&self, entry: &Seen) -> Result<(), Error> {
        self.db.execute(
            "INSERT INTO seen   (username, message, time)
            VALUES              (:username, :message, :time)
            ON CONFLICT (username) DO
            UPDATE SET message=:message,time=:time",
            params!(entry.username, entry.message, entry.time),
        )?;

        Ok(())
    }

    pub fn check_seen(&self, nick: &str) -> Result<Option<Seen>, Error> {
        let mut statement = self.db.prepare(
            "SELECT username, message, time
            FROM seen
            WHERE username = :username
            COLLATE NOCASE",
        )?;
        let rows = statement.query_map(params![nick], |r| {
            Ok(Seen {
                username: r.get(0)?,
                message: r.get(1)?,
                time: r.get(2)?,
            })
        })?;

        // I think there'll only ever be 1 row but this'll be easier
        let mut results = Vec::new();
        for r in rows {
            results.push(r?);
        }
        Ok(results.pop())
    }

    pub fn add_notification(&self, entry: &Notification) -> Result<(), Error> {
        self.db.execute(
            "INSERT INTO notifications  (recipient, via, message)
            VALUES                      (:recipient, :via, :message)",
            params!(entry.recipient, entry.via, entry.message),
        )?;

        Ok(())
    }

    pub fn remove_notification(&self, id: u32) -> Result<(), Error> {
        self.db.execute(
            "DELETE FROM notifications
            WHERE id = :id",
            params!(id),
        )?;

        Ok(())
    }

    pub fn check_notification(&self, nick: &str) -> Result<Vec<Notification>, Error> {
        let mut statement = self.db.prepare(
            "SELECT id, recipient, via, message
            FROM notifications
            WHERE recipient = :nick
            COLLATE NOCASE",
        )?;
        let rows = statement.query_map(params![nick], |r| {
            Ok(Notification {
                id: r.get(0)?,
                recipient: r.get(1)?,
                via: r.get(2)?,
                message: r.get(3)?,
            })
        })?;

        let mut results = Vec::new();
        for r in rows {
            results.push(r?);
        }

        Ok(results)
    }

    pub fn add_location(&self, loc: &str, entry: &Location) -> Result<(), Error> {
        self.db.execute(
            "INSERT INTO locations      (loc, lat, lon, city, country)
            VALUES                      (:loc, :lat, :lon, :city, :country)",
            params!(
                loc,
                entry.lat,
                entry.lon,
                entry.address.city,
                entry.address.country
            ),
        )?;

        Ok(())
    }

    pub fn check_location(&self, loc: &str) -> Result<Option<Location>, Error> {
        let mut statement = self.db.prepare(
            "SELECT lat, lon, city, country
            FROM locations
            WHERE loc = :loc
            COLLATE NOCASE",
        )?;
        let rows = statement.query_map(params![loc], |r| {
            Ok(Location {
                lat: r.get(0)?,
                lon: r.get(1)?,
                address: Address {
                    city: r.get(2)?,
                    country: r.get(3)?,
                },
            })
        })?;

        let mut results = Vec::new();
        for r in rows {
            results.push(r?);
        }

        Ok(results.pop())
    }

    pub fn add_weather(&self, user: &str, lat: &str, lon: &str) -> Result<(), Error> {
        self.db.execute(
            "INSERT INTO weather        (username, lat, lon)
            VALUES                      (:user, :lat, :lon)
            ON CONFLICT (username) DO
            UPDATE SET lat=:lat,lon=:lon",
            params!(user, lat, lon),
        )?;

        Ok(())
    }

    pub fn check_weather(&self, user: &str) -> Result<Option<(String, String)>, Error> {
        let mut statement = self.db.prepare(
            "SELECT lat, lon
            FROM weather
            WHERE username = :user
            COLLATE NOCASE",
        )?;
        let rows = statement.query_map(params![user], |r| Ok((r.get(0)?, r.get(1)?)))?;

        let mut results = Vec::new();
        for r in rows {
            results.push(r?);
        }

        Ok(results.pop())
    }

    pub fn add_coins(&self, coin: &Coin) -> Result<(), Error> {
        self.db.execute(
            "INSERT INTO coins      (coin, date, data_0, data_1)
            VALUES                  (:coin, :date, :data_0, :data_1)
            ON CONFLICT (coin) DO
            UPDATE SET date=:date,data_0=:data_0,data_1=:data_1",
            params!(coin.coin, coin.date, coin.data_0, coin.data_1),
        )?;

        Ok(())
    }

    pub fn _check_coins(&self, coin: &str) -> Result<Option<Coin>, Error> {
        let mut statement = self.db.prepare(
            "SELECT coin, date, data_0, data_1
            FROM coins
            WHERE coin = :coin",
        )?;
        let rows = statement.query_map(params![coin], |r| {
            Ok(Coin {
                coin: r.get(0)?,
                date: r.get(1)?,
                data_0: r.get(2)?,
                data_1: r.get(3)?,
            })
        })?;

        let mut results = Vec::new();
        for r in rows {
            results.push(r?);
        }

        Ok(results.pop())
    }
}

#[derive(Debug)]
pub struct Seen {
    pub username: String,
    pub message: String,
    pub time: String,
}

#[derive(Debug)]
pub struct Notification {
    pub id: u32,
    pub recipient: String,
    pub via: String,
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub struct Address {
    pub city: Option<String>,
    pub country: String,
}

#[derive(Debug, Deserialize)]
pub struct Location {
    pub lat: String,
    pub lon: String,
    pub address: Address,
}

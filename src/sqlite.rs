use failure::Error;
use rusqlite::{params, Connection};

pub struct Database {
    db: Connection,
}

impl Database {
    pub fn open(path: &str) -> Result<Self, Error> {
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
            via        TEXT NOT NULL,
            message     TEXT NOT NULL)",
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
            WHERE username LIKE :username",
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
            WHERE recipient LIKE :nick",
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

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
}

#[derive(Debug)]
pub struct Seen {
    pub username: String,
    pub message: String,
    pub time: String,
}

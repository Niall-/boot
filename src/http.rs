use bytes::BytesMut;
use futures::StreamExt;
use reqwest::{Client, Error, RequestBuilder};
use std::time::Duration;

pub static USER_AGENT: &str = "Mozilla/5.0 boot-bot-rs/1.3.0";

#[derive(Default)]
pub struct ReqBuilder<'a> {
    timeout: Option<Duration>,
    user_agent: Option<&'a str>,
}

impl<'a> ReqBuilder<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn _timeout(mut self, timeout: u64) -> Self {
        self.timeout = Some(Duration::from_secs(timeout));
        self
    }

    pub fn _user_agent(mut self, user_agent: &'a str) -> Self {
        self.user_agent = Some(user_agent);
        self
    }

    pub fn build(&self) -> Result<Req, Error> {
        let timeout = match self.timeout {
            Some(t) => t,
            _ => Duration::from_secs(12),
        };
        let user_agent = match self.user_agent {
            Some(u) => u,
            _ => USER_AGENT,
        };

        let client = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(user_agent)
            .build()?;

        let req = Req { client };

        Ok(req)
    }
}

#[derive(Clone)]
pub struct Req {
    client: Client,
}

impl Req {
    pub fn head(&self, url: &str) -> RequestBuilder {
        self.client.head(url)
    }
    pub fn get(&self, url: &str) -> RequestBuilder {
        self.client.get(url)
    }
    pub async fn read(&self, url: &str, kb: usize) -> Result<String, reqwest::Error> {
        let size = match kb {
            s if s > 0 => s * 1024,
            _ => 0,
        };

        let body = self.get(&url).send().await?;

        let mut stream = body.bytes_stream();
        let mut bytes = BytesMut::new();

        while let Some(i) = stream.next().await {
            bytes.extend_from_slice(&i?);
            if size == 0 {
                continue;
            }
            if bytes.len() >= size {
                break;
            }
        }

        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

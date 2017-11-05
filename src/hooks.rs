use reqwest::{self, Client};
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use config::Config;

#[derive(Debug)]
pub enum HookError {
    Http(reqwest::Error),
    Status(reqwest::StatusCode),
}

fn call_hook<Params: Serialize, Resp: DeserializeOwned>(
    url: &str,
    params: Params,
) -> Result<Resp, HookError> {
    let mut response = Client::new()
        .post(url)
        .json(&params)
        .send()
        .map_err(HookError::Http)?;

    if !response.status().is_success() {
        return Err(HookError::Status(response.status()));
    }

    response.json::<Resp>().map_err(HookError::Http)
}

#[derive(Serialize)]
pub struct StreamStartParams<'a> {
    pub mountpoint: &'a str,
    pub uuid: &'a Uuid,
    pub password: Option<&'a str>,
}

pub enum StreamStart {
    Ok,
    Reject,
}

#[derive(Deserialize)]
struct StreamStartResponse {
    ok: bool,
}

pub fn stream_start<'a>(config: &Config, params: StreamStartParams<'a>) -> Result<StreamStart, HookError> {
    let url = match config.webhooks.stream_start.as_ref() {
        Some(url) => url,
        None => return Ok(StreamStart::Ok),
    };

    let response = call_hook::<_, StreamStartResponse>(url, params)?;

    if response.ok {
        Ok(StreamStart::Ok)
    } else {
        Ok(StreamStart::Reject)
    }
}

#[derive(Serialize)]
pub struct StreamEndParams<'a> {
    pub mountpoint: &'a str,
    pub uuid: &'a Uuid,
}

#[derive(Deserialize)]
struct EmptyResponse {}

pub fn stream_end<'a>(config: &Config, params: StreamEndParams<'a>) -> Result<(), HookError> {
    let url = match config.webhooks.stream_end.as_ref() {
        Some(url) => url,
        None => return Ok(()),
    };

    call_hook::<_, EmptyResponse>(url, params)?;

    Ok(())
}

#[derive(Serialize)]
pub struct ListenerParams<'a> {
    pub uuid: &'a Uuid,
    pub session_cookie: Option<&'a str>,
}

pub fn listener_start<'a>(config: &Config, params: ListenerParams<'a>) -> Result<(), HookError> {
    let url = match config.webhooks.listener_start.as_ref() {
        Some(url) => url,
        None => return Ok(()),
    };

    call_hook::<_, EmptyResponse>(url, params)?;

    Ok(())
}

pub fn listener_end<'a>(config: &Config, params: ListenerParams<'a>) -> Result<(), HookError> {
    let url = match config.webhooks.listener_end.as_ref() {
        Some(url) => url,
        None => return Ok(()),
    };

    call_hook::<_, EmptyResponse>(url, params)?;

    Ok(())
}

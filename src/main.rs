extern crate lame;
extern crate lewton;
extern crate serde_json;
extern crate tiny_http;
extern crate toml;
#[macro_use]
extern crate serde_derive;

mod audio;
mod config;
mod fanout;
mod ogg;
mod server;

use std::env;
use std::path::PathBuf;
use std::process;

fn config_path() -> PathBuf {
    match env::args_os().nth(1) {
        Some(path) => path.into(),
        None => {
            eprintln!("usage: rustcast <config file>");
            process::exit(1);
        }
    }
}

fn main() {
    let config_path = config_path();

    let config = match config::open(&config_path) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Couldn't open config file: {:?}", err);
            process::exit(1);
        }
    };

    server::run(config);
}

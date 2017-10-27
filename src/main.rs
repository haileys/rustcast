extern crate lame;
extern crate lewton;
extern crate serde_json;
extern crate tiny_http;
#[macro_use]
extern crate serde_derive;

mod audio;
mod config;
mod fanout;
mod ogg;
mod server;

fn main() {
    server::run(config::Config);
}

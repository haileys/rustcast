extern crate lame;
extern crate lewton;
extern crate tiny_http;

mod fanout;
mod ogg;

use std::io;
use std::thread;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::ops::Deref;

use lame::Lame;
use lewton::VorbisError;
use tiny_http::{Server, Request, Method, Response};

use fanout::{Channel, Receiver};
use ogg::OggStream;

type StreamData = Arc<Box<[u8]>>;

struct Rustcast {
    streams: RwLock<HashMap<String, Arc<Stream>>>,
}

impl Rustcast {
    pub fn new() -> Rustcast {
        Rustcast {
            streams: RwLock::new(HashMap::new()),
        }
    }

    pub fn get_stream(&self, mountpoint: &str) -> Option<Arc<Stream>> {
        self.streams.read()
            .expect("reader lock on streams")
            .get(mountpoint).cloned()
    }

    pub fn start_stream<'a>(&'a self, mountpoint: &str) -> Option<StreamSource<'a>> {
        let mut streams = self.streams.write()
            .expect("writer lock on streams");

        if let Some(_) = streams.get(mountpoint) {
            None // mountpoint already in use
        } else {
            let stream = Arc::new(Stream::new());
            streams.insert(mountpoint.to_owned(), stream.clone());
            Some(StreamSource {
                rustcast: self,
                mountpoint: mountpoint.to_owned(),
                stream: stream,
            })
        }
    }
}

struct StreamSource<'a> {
    rustcast: &'a Rustcast,
    mountpoint: String,
    stream: Arc<Stream>,
}

impl<'a> Drop for StreamSource<'a> {
    fn drop(&mut self) {
        let mut streams = self.rustcast.streams.write()
            .expect("writer lock on streams");

        streams.remove(&self.mountpoint);
    }
}

impl<'a> Deref for StreamSource<'a> {
    type Target = Arc<Stream>;

    fn deref(&self) -> &Self::Target {
        &self.stream
    }
}

struct Stream {
    channel: Channel<StreamData>,
}

impl Stream {
    pub fn new() -> Stream {
        Stream {
            channel: Channel::new(16),
        }
    }

    pub fn publish(&self, bytes: StreamData) {
        self.channel.publish(bytes);
    }

    pub fn subscribe(&self) -> Receiver<StreamData> {
        self.channel.subscribe()
    }
}

fn handle_source(rustcast: &Rustcast, req: Request) -> io::Result<()> {
    let stream = match rustcast.start_stream(mountpoint_from_path(req.url())) {
        Some(stream) => stream,
        None => {
            // mountpoint name in use:
            return req.respond(Response::from_string("<h1>Stream already live</h1>")
                .with_status_code(409));
        }
    };

    let source = req.upgrade("icecast", Response::empty(200));
    let mut ogg = OggStream::new(source).unwrap();

    let mut lame = Lame::new().unwrap();
    lame.set_sample_rate(ogg.sample_rate()).unwrap();
    lame.set_channels(ogg.channels()).unwrap();
    lame.set_quality(0).unwrap();
    lame.init_params().unwrap();

    loop {
        let packet = match ogg.read_pcm() {
            Ok(None) => break,
            Err(VorbisError::ReadError(_)) => break,
            Err(VorbisError::BadAudio(_)) |
            Err(VorbisError::BadHeader(_)) |
            Err(VorbisError::OggError(_)) => {
                // ignore this packet
                continue;
            },
            Ok(Some(packet)) => packet,
        };

        assert!(packet.len() == (ogg.channels() as usize));

        let (left, right) = match packet.len() {
            1     => (&packet[0], &packet[0]),
            2 | _ => (&packet[0], &packet[1]),
        };

        let num_samples = left.len();

        // vector size calculation is a suggestion from lame/lame.h:
        let mut mp3buff: Vec<u8> = vec![0; (num_samples * 5) / 4 + 7200];

        match lame.encode(left, right, &mut mp3buff) {
            Ok(sz) => {
                mp3buff.resize(sz, 0);
                let arc = Arc::new(mp3buff.into_boxed_slice());
                stream.publish(arc);
            }
            Err(e) => panic!("lame encode error: {:?}", e),
        }
    };

    Ok(())
}

fn mountpoint_from_path(path: &str) -> &str {
    if path.ends_with(".mp3") {
        &path[0..(path.len() - 4)]
    } else {
        path
    }
}

fn handle_client(rustcast: &Rustcast, req: Request) -> io::Result<()> {
    use std::io::prelude::*;

    let stream = match rustcast.get_stream(mountpoint_from_path(req.url())) {
        Some(stream) => stream,
        None => return req.respond(
            Response::from_string("<h1>Not found</h1>\n")
                .with_status_code(404)),
    };

    let mut response = req.into_writer();
    response.write_all(b"HTTP/1.0 200 OK\r\nServer: Rustcast\r\nContent-Type: audio/mpeg\r\n\r\n")?;

    let rx = stream.subscribe();
    while let Some(buffer) = rx.recv() {
        response.write_all(&buffer)?;
    }

    Ok(())
}

fn handle_request(rustcast: Arc<Rustcast>, req: Request) -> io::Result<()> {
    match *req.method() {
        Method::Source => handle_source(&rustcast, req),
        Method::Get => handle_client(&rustcast, req),
        _ => {
            req.respond(Response::from_string("<h1>Method not allowed</h1>\n")
                .with_status_code(405))
        }
    }
}

fn main() {
    let rustcast = Arc::new(Rustcast::new());

    let server = Server::http("0.0.0.0:3001").unwrap();

    for request in server.incoming_requests() {
        let rustcast = rustcast.clone();
        thread::spawn(move || {
            handle_request(rustcast, request)
        });
    }
}

use std::io::{self, Write};
use std::thread;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::ops::Deref;
use std::fs::File;

use lame::Lame;
use serde_json;
use tiny_http::{Server, Request, Method, Response};
use uuid::Uuid;

use config::Config;
use fanout::{Channel, Receiver};
use ogg::OggStream;
use audio::{AudioStream, StreamRead, StreamError, Metadata};

type StreamData = Arc<Box<[u8]>>;

struct Rustcast {
    config: Config,
    streams: RwLock<HashMap<String, Arc<Stream>>>,
}

impl Rustcast {
    pub fn new(config: Config) -> Rustcast {
        Rustcast {
            config: config,
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
    metadata: RwLock<Metadata>,
}

impl Stream {
    pub fn new() -> Stream {
        Stream {
            channel: Channel::new(16),
            metadata: RwLock::new(Metadata { artist: None, title: None }),
        }
    }

    pub fn publish(&self, bytes: StreamData) {
        self.channel.publish(bytes);
    }

    pub fn subscribe(&self) -> Receiver<StreamData> {
        self.channel.subscribe()
    }
}

fn audio_stream(req: Request) -> Box<AudioStream> {
    let source = req.upgrade("icecast", Response::empty(200));
    let ogg = OggStream::new(source).unwrap();
    Box::new(ogg)
}

fn handle_source(rustcast: &Rustcast, req: Request) -> io::Result<()> {
    let stream = match rustcast.start_stream(req.url()) {
        Some(stream) => stream,
        None => {
            // mountpoint name in use:
            return req.respond(Response::from_string("<h1>Stream already live</h1>")
                .with_status_code(409));
        }
    };

    let uuid = Uuid::new_v4();

    let stream_dump_path = rustcast.config.stream_dump.replace("{uuid}",
        &format!("{}", uuid.hyphenated()));

    let mut stream_dump = File::create(stream_dump_path)?;

    let mut audio_stream = audio_stream(req);

    let mut lame = Lame::new().unwrap();
    lame.set_sample_rate(audio_stream.sample_rate()).unwrap();
    lame.set_channels(audio_stream.channels()).unwrap();
    lame.set_quality(0).unwrap();
    lame.init_params().unwrap();

    loop {
        let packet = match audio_stream.read() {
            Err(StreamError::IoError(_)) => break,
            Err(StreamError::BadPacket) => continue,
            Ok(StreamRead::Eof) => break,
            Ok(StreamRead::Audio(packet)) => packet,
            Ok(StreamRead::Metadata(metadata)) => {
                *stream.metadata.write().unwrap() = metadata;
                continue;
            }
        };

        assert!(packet.len() == (audio_stream.channels() as usize));

        let (left, right) = match packet.len() {
            1     => (&packet[0], &packet[0]),
            2 | _ => (&packet[0], &packet[1]),
        };

        let num_samples = left.len();

        // vector size calculation is a suggestion from lame/lame.h:
        let mut mp3buff: Vec<u8> = vec![0; (num_samples * 5) / 4 + 7200];

        let buff = match lame.encode(left, right, &mut mp3buff) {
            Ok(sz) => {
                mp3buff.resize(sz, 0);
                Arc::new(mp3buff.into_boxed_slice())
            }
            Err(e) => panic!("lame encode error: {:?}", e),
        };

        stream_dump.write_all(&buff)?;
        stream.publish(buff);
    };

    Ok(())
}

enum RequestFormat {
    Mp3,
    Json,
}

fn extract_request_format(path: &str) -> (RequestFormat, String) {
    fn chomp<'a>(string: &'a str, suffix: &str) -> Option<&'a str> {
        if string.ends_with(suffix) {
            Some(&string[0..(string.len() - suffix.len())])
        } else {
            None
        }
    }

    if let Some(mountpoint) = chomp(path, ".mp3") {
        (RequestFormat::Mp3, mountpoint.to_owned())
    } else if let Some(mountpoint) = chomp(path, ".json") {
        (RequestFormat::Json, mountpoint.to_owned())
    } else {
        (RequestFormat::Mp3, path.to_owned())
    }
}

#[derive(Serialize)]
struct MountpointJson {
    artist: Option<String>,
    title: Option<String>,
}

fn handle_client(rustcast: &Rustcast, req: Request) -> io::Result<()> {
    use std::io::prelude::*;

    let (format, mountpoint) = extract_request_format(req.url());

    let stream = match rustcast.get_stream(&mountpoint) {
        Some(stream) => stream,
        None => return req.respond(
            Response::from_string("<h1>Not found</h1>\n")
                .with_status_code(404)),
    };

    match format {
        RequestFormat::Mp3 => {
            let mut response = req.into_writer();
            response.write_all(b"HTTP/1.0 200 OK\r\nServer: Rustcast\r\nContent-Type: audio/mpeg\r\n\r\n")?;

            let rx = stream.subscribe();
            while let Some(buffer) = rx.recv() {
                response.write_all(&buffer)?;
            }

            Ok(())
        }
        RequestFormat::Json => {
            let data = {
                let metadata = stream.metadata.read().unwrap();

                MountpointJson {
                    artist: metadata.artist.clone(),
                    title: metadata.title.clone(),
                }
            };

            req.respond(Response::from_string(serde_json::to_string(&data).unwrap())
                .with_status_code(200))
        }
    }
}

fn handle_request(rustcast: Arc<Rustcast>, req: Request) -> io::Result<()> {
    match *req.method() {
        Method::Source => handle_source(&rustcast, req),
        Method::Get => handle_client(&rustcast, req),
        _ => {
            req.respond(Response::from_string("<h1>Method not allowed</h1>\n")
                .with_status_code(404))
        }
    }
}

pub fn run(config: Config) {
    let rustcast = Arc::new(Rustcast::new(config));

    let server = Server::http(&rustcast.config.listen).unwrap();

    println!("Listening on {}", rustcast.config.listen);

    for request in server.incoming_requests() {
        let rustcast = rustcast.clone();
        thread::spawn(move || {
            handle_request(rustcast, request)
        });
    }
}

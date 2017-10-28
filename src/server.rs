use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Write};
use std::ops::Deref;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Instant;

use base64;
use lame::Lame;
use serde_json;
use tiny_http::{Server, Request, Method, Response, Header};
use uuid::Uuid;

use audio::{AudioStream, StreamRead, StreamError, Metadata};
use config::Config;
use fanout::{Channel, Receiver};
use hooks::{self, StreamStart, StreamStartParams, StreamEndParams};
use log::Log;
use ogg::OggStream;

type StreamData = Arc<Box<[u8]>>;

#[derive(Clone)]
enum StreamEntry {
    Starting,
    Live(Arc<Stream>),
}

struct Rustcast {
    log: Log,
    config: Config,
    streams: RwLock<HashMap<String, StreamEntry>>,
}

#[derive(Debug)]
enum StartStreamError {
    AlreadyLive,
    Rejected,
    Hook(hooks::HookError),
}

impl Rustcast {
    pub fn new(config: Config) -> Rustcast {
        Rustcast {
            log: Log::new(),
            config: config,
            streams: RwLock::new(HashMap::new()),
        }
    }

    pub fn get_stream(&self, mountpoint: &str) -> Option<StreamEntry> {
        self.streams.read()
            .expect("reader lock on streams")
            .get(mountpoint).cloned()
    }

    pub fn start_stream<'a>(&'a self, mountpoint: &str, password: Option<&str>) -> Result<StreamSource<'a>, StartStreamError> {
        // insert stream entry in starting state to lock this mountpoint while
        // we auth:
        {
            let mut streams = self.streams.write()
                .expect("writer lock on streams");

            if let Some(_) = streams.get(mountpoint) {
                return Err(StartStreamError::AlreadyLive);
            }

            streams.insert(mountpoint.to_owned(), StreamEntry::Starting);
        }

        // authenticate stream source:
        let stream = Arc::new(Stream::new());

        // StreamSource will remove the mountpoint on drop:
        let stream_source = StreamSource {
            rustcast: self,
            mountpoint: mountpoint.to_owned(),
            stream: Arc::clone(&stream),
        };

        let params = StreamStartParams {
            mountpoint: mountpoint,
            uuid: &stream.uuid,
            password: password,
        };

        match hooks::stream_start(&self.config, params) {
            Ok(StreamStart::Ok) => (),
            Ok(StreamStart::Reject) => return Err(StartStreamError::Rejected),
            Err(e) => return Err(StartStreamError::Hook(e)),
        }

        // auth success, insert live stream entry into mountpoints:
        {
            let mut streams = self.streams.write()
                .expect("writer lock on streams");

            let stream_ref = streams.get_mut(mountpoint)
                .expect("mountpoint to exist in streams in Starting state");

            *stream_ref = StreamEntry::Live(Arc::clone(&stream));
        }

        Ok(stream_source)
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
    uuid: Uuid,
}

impl Stream {
    pub fn new() -> Stream {
        Stream {
            channel: Channel::new(16),
            metadata: RwLock::new(Metadata { artist: None, title: None }),
            uuid: Uuid::new_v4(),
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

fn password_from_headers(headers: &[Header]) -> Option<String> {
    headers.iter()
        .filter(|header| header.field.equiv("Authorization"))
        .filter_map(|header| {
            let mut h = header.value.as_str().split(" ");
            if let Some("Basic") = h.next() {
                h.next()
            } else {
                None
            }
        })
        .nth(0)
        .and_then(|basic| base64::decode(basic).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|creds| creds.split(":").nth(1).map(str::to_owned))
}

fn handle_source(rustcast: &Rustcast, req: Request) -> io::Result<()> {
    let password = password_from_headers(req.headers());
    let password_ref = password.as_ref().map(String::as_str);

    let stream = match rustcast.start_stream(req.url(), password_ref) {
        Ok(stream) => {
            stream
        }
        Err(StartStreamError::AlreadyLive) => {
            rustcast.log.info(&format!("Stream already live on {}, rejecting new source", req.url()));

            return req.respond(Response::from_string("<h1>Stream already live</h1>")
                .with_status_code(409));
        }
        Err(StartStreamError::Rejected) => {
            rustcast.log.info(&format!("Rejecting stream source on {}", req.url()));

            return req.respond(Response::from_string("<h1>Forbidden</h1>")
                .with_status_code(403));
        }
        Err(StartStreamError::Hook(e)) => {
            rustcast.log.error(&format!("stream_start hook failed for {}: {:?}", req.url(), e));

            return req.respond(Response::from_string("<h1>Internal Server Error</h1>")
                .with_status_code(500));
        }
    };

    let stream_dump_path = rustcast.config.stream_dump.replace("{uuid}",
        &format!("{}", stream.uuid.hyphenated()));

    let mut stream_dump = File::create(stream_dump_path)?;

    let mut audio_stream = audio_stream(req);

    // ogg reports bitrate in bits per second, but LAME's idea of bitrate
    // is in kilobits per second:
    let kilobitrate = audio_stream.bitrate_nominal() / 1000;

    let mut lame = Lame::new().unwrap();
    lame.set_sample_rate(audio_stream.sample_rate()).unwrap();
    lame.set_channels(audio_stream.channels()).unwrap();
    lame.set_quality(0).unwrap();
    lame.set_kilobitrate(kilobitrate).unwrap();
    lame.init_params().unwrap();

    let start = Instant::now();

    rustcast.log.info(&format!("Started stream {} on {} ({} {}hz {}ch {}kbps)",
        stream.uuid,
        stream.mountpoint,
        audio_stream.codec_name(),
        audio_stream.sample_rate(),
        audio_stream.channels(),
        audio_stream.bitrate_nominal() / 1000));

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

    rustcast.log.info(&format!("Finished stream {} on {} (duration {} sec)",
        stream.uuid,
        stream.mountpoint,
        start.elapsed().as_secs()));

    let params = StreamEndParams {
        mountpoint: &stream.mountpoint,
        uuid: &stream.uuid,
    };

    match hooks::stream_end(&rustcast.config, params) {
        Ok(()) => (),
        Err(e) => {
            rustcast.log.error(&format!("stream_end hook failed for {}: {:?}", stream.mountpoint, e));
        }
    }

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
        Some(StreamEntry::Live(stream)) => stream,
        Some(StreamEntry::Starting) | None =>
            return req.respond(
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

    rustcast.log.info(&format!("Listening on {}", rustcast.config.listen));

    for request in server.incoming_requests() {
        let rustcast = rustcast.clone();
        thread::spawn(move || {
            handle_request(rustcast, request)
        });
    }
}

extern crate lame;
extern crate lewton;
extern crate tiny_http;

use std::io;
use std::thread;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::sync::mpsc::{sync_channel, SyncSender, Receiver};
use std::sync::Mutex;

use lame::Lame;
use lewton::inside_ogg::OggStreamReader;
use tiny_http::{Server, Request, Method, Response};

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
        self.streams.read().unwrap().get(mountpoint).cloned()
    }

    pub fn start_stream(&self, mountpoint: &str) -> Option<Arc<Stream>> {
        let mut streams = self.streams.write().unwrap();

        if let Some(_) = streams.get(mountpoint) {
            None // mountpoint already in use
        } else {
            let stream = Arc::new(Stream::new());
            streams.insert(mountpoint.to_owned(), stream.clone());
            Some(stream)
        }
    }
}

struct Stream {
    clients: RwLock<Vec<Mutex<SyncSender<Arc<Box<[u8]>>>>>>,
}

impl Stream {
    pub fn new() -> Stream {
        Stream {
            clients: RwLock::new(Vec::new()),
        }
    }

    pub fn publish(&self, bytes: Arc<Box<[u8]>>) {
        let mut dead_clients = Vec::new();

        {
            let clients = self.clients.read().unwrap();

            for (index, client) in clients.iter().enumerate() {
                let tx = client.lock().unwrap();
                if let Err(_) = tx.try_send(bytes.clone()) {
                    dead_clients.push(index);
                }
            }
        }

        if dead_clients.len() > 0 {
            let mut clients = self.clients.write().unwrap();

            for dead_client_index in dead_clients {
                clients.swap_remove(dead_client_index);
            }
        }
    }

    pub fn subscribe(&self) -> Receiver<Arc<Box<[u8]>>> {
        let (tx, rx) = sync_channel(16);

        self.clients.write().unwrap().push(Mutex::new(tx));

        rx
    }
}

struct NonSeekStream<T: io::Read> {
    stream: T,
}

impl<T> io::Read for NonSeekStream<T> where T: io::Read {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.stream.read(buf) {
            Ok(sz) => Ok(sz),
            Err(e) => Err(e),
        }
    }
}

impl<T> io::Seek for NonSeekStream<T> where T: io::Read {
    fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
        panic!("trying to seek NonSeekStream: {:?}", pos);
    }
}

impl<T> NonSeekStream<T> where T: io::Read {
    pub fn new(stream: T) -> NonSeekStream<T> {
        NonSeekStream { stream: stream }
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
    let mut ogg = OggStreamReader::new(NonSeekStream::new(source)).unwrap();

    let mut lame = Lame::new().unwrap();
    lame.set_sample_rate(ogg.ident_hdr.audio_sample_rate).unwrap();
    lame.set_channels(ogg.ident_hdr.audio_channels as u8).unwrap();
    lame.init_params().unwrap();

    loop {
        let packet = ogg.read_dec_packet().unwrap().unwrap();
        let num_samples = packet[0].len();
        let mut mp3buff: Vec<u8> = vec![0; (num_samples * 5) / 4 + 7200];
        match lame.encode(&packet[0], &packet[1], &mut mp3buff) {
            Ok(sz) => {
                mp3buff.resize(sz, 0);
                let arc = Arc::new(mp3buff.into_boxed_slice());
                stream.publish(arc);
            }
            Err(e) => panic!("lame encode error: {:?}", e),
        }
    }
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
    while let Ok(buffer) = rx.recv() {
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

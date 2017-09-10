use std::io;

use lewton::VorbisError;
use lewton::header::IdentHeader;
use lewton::inside_ogg::OggStreamReader;

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

pub struct OggStream<T: io::Read> {
    ogg: OggStreamReader<NonSeekStream<T>>,
}

impl<T: io::Read> OggStream<T> {
    pub fn new(io: T) -> Result<Self, VorbisError> {
        OggStreamReader::new(NonSeekStream::new(io)).map(|ogg|
            OggStream { ogg: ogg })
    }

    pub fn ident_hdr(&self) -> &IdentHeader {
        &self.ogg.ident_hdr
    }

    pub fn read_pcm(&mut self) -> Result<Option<Vec<Vec<i16>>>, VorbisError> {
        self.ogg.read_dec_packet()
    }
}

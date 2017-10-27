use std::io;

#[derive(Debug)]
pub struct Metadata {
    pub artist: Option<String>,
    pub title: Option<String>,
}

type PcmData = Vec<Vec<i16>>;

pub enum StreamRead {
    Eof,
    Audio(PcmData),
    Metadata(Metadata),
}

pub enum StreamError {
    IoError(io::Error),
    BadPacket,
}

pub trait AudioStream {
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> u8;
    fn bitrate_nominal(&self) -> i32;
    fn read(&mut self) -> Result<StreamRead, StreamError>;
}

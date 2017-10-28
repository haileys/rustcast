use chrono::Local;

pub struct Log;

impl Log {
    pub fn new() -> Self {
        Log
    }

    fn emit(&self, level: &str, msg: &str) {
        println!("{:5} [{}] {}", level, Local::now(), msg);
    }

    pub fn info(&self, msg: &str) {
        self.emit("INFO", msg);
    }

    pub fn error(&self, msg: &str) {
        self.emit("ERROR", msg);
    }
}

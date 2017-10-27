use chrono::Local;

pub struct Log;

impl Log {
    pub fn new() -> Self {
        Log
    }

    fn emit(&self, level: &str, msg: &str) {
        println!("{:4} [{}] {}", level, Local::now(), msg);
    }

    pub fn info(&self, msg: &str) {
        self.emit("INFO", msg);
    }
}

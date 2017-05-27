use std::sync::{mpsc, RwLock, Mutex};

pub struct Channel<T> {
    buffer_size: usize,
    txs: RwLock<Vec<Mutex<mpsc::SyncSender<T>>>>,
}

pub struct Receiver<T> {
    rx: mpsc::Receiver<T>,
}

impl<T> Channel<T> where T: Clone {
    pub fn new(buffer_size: usize) -> Channel<T> {
        Channel {
            buffer_size: buffer_size,
            txs: RwLock::new(Vec::new())
        }
    }

    pub fn publish(&self, data: T) {
        let mut dead_txs = Vec::new();

        {
            let txs = self.txs.read()
                .expect("reader lock on txs");

            for (index, tx) in txs.iter().enumerate() {
                let tx = tx.lock().expect("lock on tx");
                if let Err(_) = tx.try_send(data.clone()) {
                    dead_txs.push(index);
                }
            }
        }

        if dead_txs.len() > 0 {
            let mut txs = self.txs.write()
                .expect("writer lock on txs");

            for dead_tx_index in dead_txs {
                txs.swap_remove(dead_tx_index);
            }
        }
    }

    pub fn subscribe(&self) -> Receiver<T> {
        let (tx, rx) = mpsc::sync_channel(self.buffer_size);

        self.txs.write()
            .expect("writer lock on txs")
            .push(Mutex::new(tx));

        Receiver { rx: rx }
    }
}

impl<T> Receiver<T> where T: Clone {
    pub fn recv(&self) -> Option<T> {
        self.rx.recv().ok()
    }
}

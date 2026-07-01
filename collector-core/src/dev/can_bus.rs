use std::{collections::HashMap, sync::Arc};

use parking_lot::Mutex;
use tokio::sync::mpsc;

pub type RawFrameTx = mpsc::UnboundedSender<(u32, Vec<u8>)>;
pub type RawFrameRx = mpsc::UnboundedReceiver<(u32, Vec<u8>)>;

#[derive(Clone, Default)]
pub struct SharedCanBus(Arc<Mutex<HashMap<String, RawFrameTx>>>);

impl SharedCanBus {
    pub fn register(&self, dev_id: &str, tx: RawFrameTx) {
        self.0.lock().insert(dev_id.to_owned(), tx);
    }

    pub fn unregister(&self, dev_id: &str) {
        self.0.lock().remove(dev_id);
    }

    /// 返回 false 表示设备不存在或通道已关闭
    pub fn send(&self, dev_id: &str, frame_id: u32, data: Vec<u8>) -> bool {
        let map = self.0.lock();
        map.get(dev_id)
            .is_some_and(|tx| tx.send((frame_id, data)).is_ok())
    }
}

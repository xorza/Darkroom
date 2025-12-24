use std::sync::Arc;

use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct OutputStream {
    sender: UnboundedSender<String>,
    receiver: Arc<Mutex<UnboundedReceiver<String>>>,
}

impl Default for OutputStream {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputStream {
    pub fn new() -> Self {
        let (sender, receiver) = unbounded_channel();
        Self {
            sender,
            receiver: Arc::new(Mutex::new(receiver)),
        }
    }

    pub fn write<S: Into<String>>(&self, s: S) {
        self.sender
            .send(s.into())
            .expect("OutputStream receiver dropped");
    }

    pub async fn take(&self) -> Vec<String> {
        let mut guard = self.receiver.lock().await;
        let mut output = Vec::new();
        while let Ok(value) = guard.try_recv() {
            output.push(value);
        }
        output
    }
}

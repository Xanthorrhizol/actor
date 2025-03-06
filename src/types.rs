#[derive(Debug)]
pub struct Message {
    inner: Vec<u8>,
    result_tx: Option<tokio::sync::oneshot::Sender<Vec<u8>>>,
}
impl Clone for Message {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            result_tx: None,
        }
    }
}

impl Message {
    pub fn new(inner: Vec<u8>, result_tx: Option<tokio::sync::oneshot::Sender<Vec<u8>>>) -> Self {
        Self { inner, result_tx }
    }

    pub fn inner(&self) -> &Vec<u8> {
        &self.inner
    }

    pub fn result_tx(&mut self) -> Option<tokio::sync::oneshot::Sender<Vec<u8>>> {
        let tx = self.result_tx.take();
        self.result_tx = None;
        tx
    }
}

pub struct JobSpec {
    max_iter: Option<usize>,
    interval: Option<std::time::Duration>,
    start_at: std::time::SystemTime,
}

impl JobSpec {
    pub fn new(
        max_iter: Option<usize>,
        interval: Option<std::time::Duration>,
        start_at: std::time::SystemTime,
    ) -> Self {
        if let None = interval {
            Self {
                max_iter: Some(1),
                interval,
                start_at,
            }
        } else {
            Self {
                max_iter,
                interval,
                start_at,
            }
        }
    }

    pub fn max_iter(&self) -> Option<usize> {
        self.max_iter
    }

    pub fn start_at(&self) -> std::time::SystemTime {
        self.start_at
    }

    pub fn interval(&self) -> Option<std::time::Duration> {
        self.interval
    }
}

impl Default for JobSpec {
    fn default() -> Self {
        Self {
            max_iter: Some(1),
            interval: None,
            start_at: std::time::SystemTime::now(),
        }
    }
}

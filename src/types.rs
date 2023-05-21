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

use itertools::Itertools as _;

pub struct ProgressStatus {
    multi_progress: indicatif::MultiProgress,
    pb: indicatif::ProgressBar,
    queue_size: usize,
    active: Vec<(String, Vec<indicatif::ProgressBar>)>,
    num_done: usize,
    num_cached_done: usize,
}

impl ProgressStatus {
    pub fn new(multi_progress: indicatif::MultiProgress, pb: indicatif::ProgressBar) -> Self {
        let ret = Self {
            multi_progress,
            pb,
            queue_size: 0,
            active: Vec::new(),
            num_done: 0,
            num_cached_done: 0,
        };
        ret.draw();
        ret
    }

    pub fn start(&mut self, name: String, mut pbs: Vec<indicatif::ProgressBar>) {
        if self.active.is_empty() {
            // Nothing is drawn, add this item.
            pbs = pbs
                .into_iter()
                .map(|pb| self.multi_progress.add(pb))
                .collect();
        }
        self.active.push((name, pbs));
        self.draw();
    }

    pub fn finish(&mut self, name: &str) {
        let (idx, _value) = self
            .active
            .iter()
            .find_position(|(n, _pb)| *n == name)
            .expect("name is active");
        // Remove the first occurrence of the name, in case of duplicates.
        self.active.remove(idx);
        if idx == 0
            && let Some((_name, item_pbs)) = self.active.first_mut()
        {
            // Show the first active item, the oldest one.
            let hidden_item_pbs = std::mem::take(item_pbs);
            *item_pbs = hidden_item_pbs
                .into_iter()
                .map(|pb| self.multi_progress.add(pb))
                .collect();
        }
        self.num_done += 1;
        self.pb.inc(1);
        self.draw();
    }

    pub fn set_queue_size(&mut self, queue_size: usize) {
        if queue_size != self.queue_size {
            self.queue_size = queue_size;
            self.draw();
        }
    }

    pub fn inc_num_cached_done(&mut self) {
        self.num_cached_done += 1;
        self.draw();
    }

    fn draw(&self) {
        self.pb
            .set_length((self.num_done + self.active.len() + self.queue_size) as u64);

        let mut msg = String::new();
        if !self.active.is_empty() {
            msg.push_str(": ");
            msg.push_str(&self.active.iter().map(|(name, _pb)| name).join(", "));
        }
        if self.num_cached_done > 0 {
            if msg.is_empty() {
                msg.push_str(": ");
            } else {
                msg.push(' ');
            }
            msg.push_str(&format!("({} cached)", self.num_cached_done));
        }
        self.pb.set_message(msg);
    }
}

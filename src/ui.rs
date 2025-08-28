use itertools::Itertools as _;
use std::sync::Arc;
use std::sync::Mutex;

pub struct ProgressTask {
    pub name: String,
    pub pbs: Vec<indicatif::ProgressBar>,
}

#[derive(Clone)]
pub struct ProgressTaskHandle {
    task: std::sync::Weak<ProgressTask>,
    status: std::sync::Weak<Mutex<ProgressStatusImpl>>,
}

impl Drop for ProgressTaskHandle {
    fn drop(&mut self) {
        if let Some(task) = self.task.upgrade()
            && let Some(status) = self.status.upgrade()
        {
            status.lock().unwrap().finish(&task);
        }
    }
}

impl ProgressTaskHandle {
    pub fn new(task: ProgressTask) -> Self {
        Self {
            task: Arc::downgrade(&Arc::new(task)),
            status: std::sync::Weak::new(),
        }
    }
}

/// A console UI component showing multiple progress bars for the longest
/// running task and a single line with all queued and active tasks.
#[derive(Clone)]
pub struct ProgressStatus(Arc<Mutex<ProgressStatusImpl>>);

struct ProgressStatusImpl {
    me: std::sync::Weak<Mutex<Self>>,
    multi_progress: indicatif::MultiProgress,
    pb: indicatif::ProgressBar,
    queue_size: usize,
    active: Vec<Arc<ProgressTask>>,
    active_pbs: Vec<indicatif::ProgressBar>,
    num_done: usize,
    num_cached_done: usize,
}

impl ProgressStatus {
    pub fn new(multi_progress: indicatif::MultiProgress, pb: indicatif::ProgressBar) -> Self {
        Self(ProgressStatusImpl::new(multi_progress, pb))
    }

    pub fn start(&self, name: String, pbs: Vec<indicatif::ProgressBar>) -> ProgressTaskHandle {
        self.0.lock().unwrap().start(name, pbs)
    }

    pub fn set_queue_size(&self, queue_size: usize) {
        self.0.lock().unwrap().set_queue_size(queue_size)
    }

    pub fn inc_queue_size(&self, delta: isize) {
        self.0.lock().unwrap().inc_queue_size(delta)
    }

    pub fn inc_num_cached_done(&self) {
        self.0.lock().unwrap().inc_num_cached_done()
    }
}

impl ProgressStatusImpl {
    pub fn new(
        multi_progress: indicatif::MultiProgress,
        pb: indicatif::ProgressBar,
    ) -> Arc<Mutex<Self>> {
        let ret = Arc::new_cyclic(|me| {
            Mutex::new(ProgressStatusImpl {
                me: me.clone(),
                multi_progress,
                pb,
                queue_size: 0,
                active: Vec::new(),
                active_pbs: Vec::new(),
                num_done: 0,
                num_cached_done: 0,
            })
        });
        ret.lock().unwrap().draw();
        ret
    }

    pub fn start(
        &mut self,
        name: String,
        mut pbs: Vec<indicatif::ProgressBar>,
    ) -> ProgressTaskHandle {
        if self.active.is_empty() {
            // Nothing is drawn, add this item.
            pbs = pbs
                .into_iter()
                .map(|pb| self.multi_progress.add(pb))
                .collect();
        }
        let task = Arc::new(ProgressTask { name, pbs });
        let weak_task = Arc::downgrade(&task);
        self.active.push(task);
        self.draw();

        ProgressTaskHandle {
            task: weak_task,
            status: self.me.clone(),
        }
    }

    fn finish(&mut self, task: &Arc<ProgressTask>) {
        let (idx, _value) = self
            .active
            .iter()
            .find_position(|&t| Arc::ptr_eq(t, task))
            .expect("name is active");
        // Remove the first occurrence of the name, in case of duplicates.
        self.active.remove(idx);
        if idx == 0
            && let Some(first_task) = self.active.first_mut()
        {
            // Show the first active item, the oldest one.
            self.active_pbs = first_task
                .pbs
                .iter()
                .map(|pb| self.multi_progress.add(pb.clone()))
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

    pub fn inc_queue_size(&mut self, delta: isize) {
        if delta != 0 {
            self.queue_size = self.queue_size.checked_add_signed(delta).unwrap();
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
            msg.push_str(&self.active.iter().map(|task| &task.name).join(", "));
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

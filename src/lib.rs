//! # async_job: a simple async cron runner
//!
//! Use the `Job` trait to create your cron job struct, pass it to the `Runner` and then start it via `run()` method.
//! Runner will spawn new async task where it will start looping through the jobs and will run their handle
//! method once the scheduled time is reached.
//!
//! If your OS has enough threads to spare each job will get its own thread to execute, if not it will be
//! executed in the same thread as the loop but will hold the loop until the job is finished.
//!
//! Please look at the [**`Job trait`**](./trait.Job.html) documentation for more information.
//!
//! ## Example
//! ```
//! use async_job::{Job, Runner, Schedule, async_trait};
//! use tokio::time::Duration;
//! use tokio;
//!
//! struct ExampleJob;
//!
//! #[async_trait]
//! impl Job for ExampleJob {
//!     fn schedule(&self) -> Option<Schedule> {
//!         Some("1/5 * * * * *".parse().unwrap())
//!     }
//!     async fn handle(&mut self) {
//!         println!("Hello, I am a cron job running at: {}", self.now());
//!     }
//! }
//!
//! async fn run() {
//!     let mut runner = Runner::new();
//!
//!     println!("Adding ExampleJob to the Runner");
//!     runner = runner.add(Box::new(ExampleJob));
//!
//!     println!("Starting the Runner for 20 seconds");
//!     runner = runner.run().await;
//!     tokio::time::sleep(Duration::from_millis(20 * 1000)).await;
//!
//!     println!("Stopping the Runner");
//!     runner.stop().await;
//! }
//!
//! #[tokio::main]
//! async fn main() {
//!     run().await;
//! }
//! ```
//!
//! Output:
//! ```shell
//! Adding ExampleJob to the Runner
//! Starting the Runner for 20 seconds
//! Hello, I am a cron job running at: 2021-01-31 03:06:25.908475 UTC
//! Hello, I am a cron job running at: 2021-01-31 03:06:30.912637 UTC
//! Hello, I am a cron job running at: 2021-01-31 03:06:35.926938 UTC
//! Hello, I am a cron job running at: 2021-01-31 03:06:40.962138 UTC
//! Stopping the Runner
//! ```
extern crate chrono;
extern crate cron;

pub use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
pub use cron::Schedule;
use lazy_static::lazy_static;
use log::{debug, error, info};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, RwLock,
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

lazy_static! {
    /// Singleton instance of a tracker that won't allow
    /// same job to run again while its already running
    /// unless you specificly allow the job to run in
    /// parallel with itself
    pub static ref TRACKER: RwLock<Tracker> = RwLock::new(Tracker::new());
}

#[async_trait]
/// A cron job that runs for a website.
pub trait Job: Send + Sync {
    /// Default implementation of is_active method will
    /// make this job always active
    fn is_active(&self) -> bool {
        true
    }

    /// In case your job takes longer to finish and it's scheduled
    /// to start again (while its still running), default behaviour
    /// will skip the next run while one instance is already running.
    /// (if your OS has enough threads, and is spawning a thread for next job)
    ///
    /// To override this behaviour and enable it to run in parallel
    /// with other instances of self, return `true` on this instance.
    fn allow_parallel_runs(&self) -> bool {
        false
    }

    /// Define the run schedule for your job
    fn schedule(&self) -> Option<Schedule>;

    /// This is where your jobs magic happens, define the action that
    /// will happen once the cron start running your job
    ///
    /// If this method panics, your entire job will panic and that may
    /// or may not make the whole runner panic. Handle your errors
    /// properly and don't let it panic.
    async fn handle(&mut self);

    /// Decide wheather or not to start running your job
    fn should_run(&self) -> bool {
        if self.is_active() {
            match self.schedule() {
                Some(schedule) => {
                    for item in schedule.upcoming(Utc).take(1) {
                        let difference = item - Utc::now();
                        if difference <= Duration::milliseconds(100) {
                            return true;
                        }
                    }
                }
                _ => (),
            }
        }

        false
    }

    /// Simple output that will return current time so you don't have to do so
    /// in your job if you wish to display the time of the run.
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Struct for marking jobs running
pub struct Tracker(Vec<usize>);

impl Default for Tracker {
    fn default() -> Self {
        Self::new()
    }
}

impl Tracker {
    /// Return new instance of running
    pub fn new() -> Self {
        Tracker(vec![])
    }

    /// Check if id of the job is marked as running
    pub fn running(&self, id: &usize) -> bool {
        self.0.contains(id)
    }

    /// Set job id as running
    pub fn start(&mut self, id: &usize) -> usize {
        if !self.running(id) {
            self.0.push(*id);
        }
        self.0.len()
    }

    /// Unmark the job from running
    pub fn stop(&mut self, id: &usize) -> usize {
        if self.running(id) {
            match self.0.iter().position(|&r| r == *id) {
                Some(i) => self.0.remove(i),
                None => 0,
            };
        }
        self.0.len()
    }
}

/// Runner that will hold all the jobs and will start up the execution
/// and eventually will stop it.
pub struct Runner {
    /// the current jobs
    pub jobs: Vec<Box<dyn Job>>,
    /// the task that is running the handle
    pub thread: Option<JoinHandle<()>>,
    /// is the task running or not
    pub running: bool,
    /// channel sending message
    pub tx: Option<UnboundedSender<Result<(), ()>>>,
    /// tracker to determine crons working
    pub working: Arc<AtomicBool>,
}

impl Default for Runner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner {
    /// Create new runner
    pub fn new() -> Self {
        Runner {
            jobs: vec![],
            thread: None,
            running: false,
            tx: None,
            working: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Add jobs into the runner
    ///
    /// Does nothing if already running.
    #[allow(clippy::should_implement_trait)]
    pub fn add(mut self, job: Box<dyn Job>) -> Self {
        if !self.running {
            self.jobs.push(job);
        }
        self
    }

    /// Number of jobs ready to start running
    pub fn jobs_to_run(&self) -> usize {
        self.jobs.len()
    }

    /// Start the loop and job execution
    pub async fn run(self) -> Self {
        if self.jobs.is_empty() {
            return self;
        }

        let working = Arc::new(AtomicBool::new(false));
        let (thread, tx) = spawn(self, working.clone()).await;

        Self {
            thread,
            jobs: vec![],
            running: true,
            tx,
            working,
        }
    }

    /// Stop the spawned runner
    pub async fn stop(&mut self) {
        if !self.running {
            return;
        }
        if let Some(thread) = self.thread.take() {
            if let Some(tx) = &self.tx {
                match tx.send(Ok(())) {
                    Ok(_) => (),
                    Err(e) => error!("Could not send stop signal to cron runner thread: {}", e),
                };
            }
            thread.abort()
        }
    }

    /// Lets us know if the cron worker is running
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Lets us know if the worker is in the process of executing a job currently
    pub fn is_working(&self) -> bool {
        self.working.load(Ordering::Relaxed)
    }
}

/// Spawn the thread for the runner and return its sender to stop it
async fn spawn(
    runner: Runner,
    working: Arc<AtomicBool>,
) -> (
    Option<JoinHandle<()>>,
    Option<UnboundedSender<Result<(), ()>>>,
) {
    let (tx, mut rx): (
        UnboundedSender<Result<(), ()>>,
        UnboundedReceiver<Result<(), ()>>,
    ) = unbounded_channel();

    let handler = tokio::spawn(async move {
        let mut jobs = runner.jobs;

        loop {
            if rx.try_recv().is_ok() {
                info!("Stopping the cron runner thread");
                break;
            }

            for (id, job) in jobs.iter_mut().enumerate() {
                let no: String = (id + 1).to_string();

                if job.should_run()
                    && (job.allow_parallel_runs()
                        || match TRACKER.read() {
                            Ok(s) => !s.running(&id),
                            _ => false,
                        })
                {
                    match TRACKER.write() {
                        Ok(mut s) => {
                            s.start(&id);
                        }
                        _ => (),
                    }

                    let now = Utc::now();
                    debug!(
                        "START: {} --- {}",
                        format!("cron-job-thread-{}", no),
                        now.format("%H:%M:%S%.f")
                    );

                    working.store(true, Ordering::Relaxed);

                    job.handle().await;

                    working.store(
                        match TRACKER.write() {
                            Ok(mut s) => s.stop(&id) != 0,
                            _ => false,
                        },
                        Ordering::Relaxed,
                    );

                    debug!(
                        "FINISH: {} --- {}",
                        format!("cron-job-thread-{}", no),
                        now.format("%H:%M:%S%.f")
                    );
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    });

    (Some(handler), Some(tx))
}

#[cfg(test)]
mod tests {
    use super::{Job, Runner};
    use async_trait::async_trait;
    use cron::Schedule;
    use std::str::FromStr;
    struct SomeJob;

    #[async_trait]
    impl Job for SomeJob {
        fn schedule(&self) -> Option<Schedule> {
            Some(Schedule::from_str("0 * * * * *").unwrap())
        }

        async fn handle(&mut self) {}
    }
    struct AnotherJob;
    #[async_trait]
    impl Job for AnotherJob {
        fn schedule(&self) -> Option<Schedule> {
            Some(Schedule::from_str("0 * * * * *").unwrap())
        }

        async fn handle(&mut self) {}
    }
    #[tokio::test]
    async fn create_job() {
        let mut some_job = SomeJob;

        assert_eq!(some_job.handle().await, ());
    }

    #[tokio::test]
    async fn test_adding_jobs_to_runner() {
        let some_job = SomeJob;
        let another_job = AnotherJob;

        let runner = Runner::new()
            .add(Box::new(some_job))
            .add(Box::new(another_job));

        assert_eq!(runner.jobs_to_run(), 2);
    }

    #[tokio::test]
    async fn test_jobs_are_empty_after_runner_starts() {
        let some_job = SomeJob;
        let another_job = AnotherJob;

        let runner = Runner::new()
            .add(Box::new(some_job))
            .add(Box::new(another_job))
            .run()
            .await;

        assert_eq!(runner.jobs_to_run(), 0);
    }

    #[tokio::test]
    async fn test_stopping_the_runner() {
        let some_job = SomeJob;
        let another_job = AnotherJob;

        let mut runner = Runner::new()
            .add(Box::new(some_job))
            .add(Box::new(another_job))
            .run()
            .await;

        assert_eq!(runner.stop().await, ());
    }
}

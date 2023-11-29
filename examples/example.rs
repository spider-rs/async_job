//! `cargo run --example example`
extern crate async_job;

use async_job::{async_trait, Job, Runner, Schedule};
use tokio;
use tokio::time::Duration;

struct ExampleJob;

#[async_trait]
impl Job for ExampleJob {
    fn schedule(&self) -> Option<Schedule> {
        Some("1/5 * * * * *".parse().unwrap())
    }
    async fn handle(&mut self) {
        println!("Hello, I am a cron job running at: {}", self.now());
    }
}

async fn run() {
    let mut runner = Runner::new();
    println!("Adding ExampleJob to the Runner");
    runner = runner.add(Box::new(ExampleJob));
    println!("Starting the Runner for 20 seconds");
    runner = runner.run().await;
    tokio::time::sleep(Duration::from_millis(20 * 1000)).await;
    println!("Stopping the Runner");
    runner.stop().await;
}

#[tokio::main]
async fn main() {
    run().await;
}

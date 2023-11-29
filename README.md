# async_job

A simple trait for async cron jobs in Rust.

## Getting Started

1. `cargo add async_job`

## Usage

```rs
use async_job::{Job, Runner, Schedule, async_trait};

struct ExampleJob;

#[async_trait]
impl Job for ExampleJob {
    fn schedule(&self) -> Option<Schedule> {
        Some("1/5 * * * * *".parse().unwrap())
    }
    // run any async or sync task here with mutation capabilities
    async fn handle(&mut self) {
        println!("Hello, I am a cron job running at: {}", self.now());
    }
}
```

If you need to use a single threaded env disable the default feature and set the feature `rt`,

## Feature Flags

1. `rt`: Single threaded tokio runtime.
1. `rt-multi-thread`: Multi threaded tokio runtime. Enabled by default

## Examples

Run the example with `cargo run --example example`

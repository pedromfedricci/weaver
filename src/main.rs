//! Entry point and CLI parsing for the logo discovery tool.

mod async_executor;
mod coordinator;
mod error;
mod heuristics;
mod messages;
mod progress;
mod url_utils;
mod worker;

use std::path::PathBuf;
use std::thread;

use clap::Parser;
use log::{error, info};

use crate::async_executor::AsyncExecutorConfig;
use crate::coordinator::{Coordinator, CoordinatorConfig};
use crate::progress::{ProgressWriter, generate_csv, load_progress};
use crate::worker::spawn_worker_pool;

#[derive(Parser)]
#[command(name = "weave", about = "Logo discovery tool")]
struct Cli {
    /// Maximum concurrent HTTP requests.
    #[arg(long, default_value_t = 100)]
    concurrency: usize,

    /// Maximum pages to crawl per domain.
    #[arg(long, default_value_t = 50)]
    max_pages: usize,

    /// Maximum link depth from root.
    #[arg(long, default_value_t = 3)]
    max_depth: usize,

    /// Number of CPU worker threads.
    #[arg(long, default_value_t = num_cpus::get())]
    workers: usize,

    /// Skip CSV generation after crawling.
    #[arg(long)]
    no_export: bool,

    /// Path to progress log file.
    #[arg(long, default_value = "progress.log")]
    log_file: PathBuf,

    /// Path to output CSV file.
    #[arg(long, default_value = "output.csv")]
    csv_file: PathBuf,
}

fn main() {
    env_logger::init();
    let cli = Cli::parse();

    // Load progress state for crash recovery through WAL.
    let crawl_state = load_progress(&cli.log_file);
    info!(
        "Recovered state: {} completed, {} in-progress",
        crawl_state.completed().len(),
        crawl_state.in_progress().len()
    );

    // Create progress writer.
    let progress_writer = match ProgressWriter::new(&cli.log_file) {
        Ok(pw) => pw,
        Err(e) => {
            error!("Failed to open progress log: {e}.");
            std::process::exit(1);
        }
    };

    // Channel capacities.
    // Note: Purposely low. This tool is not expected to run actual workloads.
    let channel_capacity = 100;
    let work_channel_capacity = cli.workers * 2;

    // Channel: async executor to coordinator (AsyncToCpu).
    let (async_to_cpu_tx, async_to_cpu_rx) = tokio::sync::mpsc::channel(channel_capacity);

    // Channel: coordinator to async executor (CpuToAsync).
    let (cpu_to_async_tx, cpu_to_async_rx) = tokio::sync::mpsc::channel(channel_capacity);

    // Channel: coordinator to worker pool.
    let (work_tx, work_rx) = crossbeam::channel::bounded(work_channel_capacity);

    // Channel: worker pool to coordinator.
    let (result_tx, result_rx) = crossbeam::channel::unbounded();

    // Spawn worker pool.
    let worker_handles = spawn_worker_pool(cli.workers, &work_rx, &result_tx);

    // Initialize and configure coordinator.
    let coordinator = Coordinator::new(
        async_to_cpu_rx,
        cpu_to_async_tx,
        work_tx,
        result_rx,
        progress_writer,
        CoordinatorConfig::new(cli.max_pages, cli.max_depth),
    );

    // Spawn coordinator on dedicated thread.
    let coordinator_handle = thread::spawn(move || coordinator.run());

    // Build tokio multi thread runtime.
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            error!("Failed to create tokio runtime: {e}.");
            std::process::exit(1);
        }
    };

    // Run async executor on tokio multi thread runtime.
    if let Err(e) = rt.block_on(async_executor::run(
        async_to_cpu_tx,
        cpu_to_async_rx,
        crawl_state,
        AsyncExecutorConfig::new(cli.concurrency),
    )) {
        error!("Async executor failed: {e}.");
        std::process::exit(1);
    }

    // Wait for coordinator to finish.
    if let Err(e) = coordinator_handle.join() {
        error!("Coordinator thread panicked: {e:?}.");
    }

    // Wait for workers to finish.
    for (i, handle) in worker_handles.into_iter().enumerate() {
        if let Err(e) = handle.join() {
            error!("Worker thread {i} panicked: {e:?}.");
        }
    }

    info!("Logo discovery done.");

    // Generate CSV from progress log.
    if !cli.no_export {
        match generate_csv(&cli.log_file, &cli.csv_file) {
            Ok(()) => {
                info!("CSV written to {}.", cli.csv_file.display());
            }
            Err(e) => {
                error!("Failed to generate CSV: {e}.");
                std::process::exit(1);
            }
        }
    }

    info!("Export to {} file done.", cli.csv_file.display());
}

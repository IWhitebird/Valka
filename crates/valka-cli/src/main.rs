use anyhow::Result;
use clap::{Parser, Subcommand};

mod commands;

#[derive(Parser)]
#[command(name = "valka", about = "Valka distributed task queue CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// gRPC server address
    #[arg(long, default_value = "http://[::1]:50051", global = true)]
    server: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Task operations
    Task {
        #[command(subcommand)]
        command: TaskCommands,
    },
    /// Worker operations
    Worker {
        #[command(subcommand)]
        command: WorkerCommands,
    },
    /// Log operations
    Logs {
        #[command(subcommand)]
        command: LogCommands,
    },
    /// Start the server
    Server {
        /// Path to configuration file
        #[arg(long)]
        config: Option<String>,
    },
    /// Cluster operations
    Cluster {
        #[command(subcommand)]
        command: ClusterCommands,
    },
}

#[derive(Subcommand)]
enum TaskCommands {
    /// Create a new task
    Create {
        /// Queue name
        #[arg(long)]
        queue: String,
        /// Task name
        #[arg(long)]
        name: String,
        /// Input JSON
        #[arg(long)]
        input: Option<String>,
        /// Priority
        #[arg(long, default_value = "0")]
        priority: i32,
        /// Max retries
        #[arg(long, default_value = "3")]
        max_retries: i32,
        /// Timeout in seconds
        #[arg(long, default_value = "300")]
        timeout: i32,
    },
    /// Get a task by ID
    Get {
        /// Task ID
        task_id: String,
    },
    /// List tasks
    List {
        /// Filter by queue name
        #[arg(long)]
        queue: Option<String>,
        /// Filter by status
        #[arg(long)]
        status: Option<String>,
        /// Limit
        #[arg(long, default_value = "20")]
        limit: i32,
    },
    /// Cancel a task
    Cancel {
        /// Task ID
        task_id: String,
    },
}

#[derive(Subcommand)]
enum WorkerCommands {
    /// List connected workers
    List,
    /// Drain a worker (graceful shutdown)
    Drain {
        /// Worker ID
        worker_id: String,
    },
}

#[derive(Subcommand)]
enum LogCommands {
    /// Tail logs for a task run
    Tail {
        /// Task run ID
        task_run_id: String,
    },
}

#[derive(Subcommand)]
enum ClusterCommands {
    /// Show cluster status
    Status,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("valka=info")
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Task { command } => match command {
            TaskCommands::Create {
                queue,
                name,
                input,
                priority,
                max_retries,
                timeout,
            } => {
                commands::task::create(
                    &cli.server,
                    &queue,
                    &name,
                    input,
                    priority,
                    max_retries,
                    timeout,
                )
                .await?;
            }
            TaskCommands::Get { task_id } => {
                commands::task::get(&cli.server, &task_id).await?;
            }
            TaskCommands::List {
                queue,
                status,
                limit,
            } => {
                commands::task::list(&cli.server, queue, status, limit).await?;
            }
            TaskCommands::Cancel { task_id } => {
                commands::task::cancel(&cli.server, &task_id).await?;
            }
        },
        Commands::Worker { command } => match command {
            WorkerCommands::List => {
                println!("Worker list not yet implemented (requires admin RPC)");
            }
            WorkerCommands::Drain {
                worker_id: _worker_id,
            } => {
                println!("Worker drain not yet implemented (requires admin RPC)");
            }
        },
        Commands::Logs { command } => match command {
            LogCommands::Tail { task_run_id } => {
                commands::logs::tail(&cli.server, &task_run_id).await?;
            }
        },
        Commands::Server { config: _config } => {
            println!("Use `valka-server` binary to start the server");
            println!("  valka-server [config-path]");
        }
        Commands::Cluster { command } => match command {
            ClusterCommands::Status => {
                println!("Cluster status not yet implemented");
            }
        },
    }

    Ok(())
}

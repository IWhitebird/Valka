use anyhow::Result;
use clap::{Parser, Subcommand};

mod commands;

use commands::Conn;

#[derive(Parser)]
#[command(
    name = "valka",
    version,
    about = "Command-line client for the Valka task queue",
    long_about = "Command-line client for the Valka task queue.\n\nTask and log commands use \
                  the gRPC API (--server); cluster, worker and queue commands use the REST \
                  API (--http). Any node of a cluster answers any request."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// gRPC address of a node; repeat it (or separate with commas) to list several nodes.
    #[arg(
        long,
        env = "VALKA_SERVER",
        default_value = "http://127.0.0.1:50051",
        value_delimiter = ',',
        global = true
    )]
    server: Vec<String>,

    /// REST address of a node (cluster, worker and queue commands).
    #[arg(
        long,
        env = "VALKA_HTTP",
        default_value = "http://127.0.0.1:8989",
        global = true
    )]
    http: String,

    /// API key for the public API (when the server sets VALKA_API_KEYS).
    #[arg(long, env = "VALKA_API_KEY", global = true, hide_env_values = true)]
    api_key: Option<String>,

    /// Admin token for cluster changes (the server's VALKA_ADMIN_TOKEN).
    #[arg(long, env = "VALKA_ADMIN_TOKEN", global = true, hide_env_values = true)]
    admin_token: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Create, inspect, list and cancel tasks
    Task {
        #[command(subcommand)]
        command: TaskCommands,
    },
    /// Follow a task run's logs
    Logs {
        #[command(subcommand)]
        command: LogCommands,
    },
    /// Connected workers
    Worker {
        #[command(subcommand)]
        command: WorkerCommands,
    },
    /// Cluster health, membership and shard placement
    Cluster {
        #[command(subcommand)]
        command: ClusterCommands,
    },
    /// Queue partitions
    Queue {
        #[command(subcommand)]
        command: QueueCommands,
    },
}

#[derive(Subcommand)]
enum TaskCommands {
    /// Create a task
    Create {
        /// Queue name
        #[arg(long)]
        queue: String,
        /// Task name
        #[arg(long)]
        name: String,
        /// Input as JSON
        #[arg(long)]
        input: Option<String>,
        /// Priority (higher runs first within a queue partition)
        #[arg(long, default_value = "0")]
        priority: i32,
        /// Attempts before the task is dead-lettered
        #[arg(long, default_value = "3")]
        max_retries: i32,
        /// Seconds an attempt may run
        #[arg(long, default_value = "300")]
        timeout: i32,
        /// Idempotency key: a second create with the same key returns the first task
        #[arg(long)]
        idempotency_key: Option<String>,
        /// Run no earlier than this RFC 3339 time
        #[arg(long)]
        scheduled_at: Option<String>,
        /// Print only the new task's id
        #[arg(long, short)]
        quiet: bool,
    },
    /// Show a task
    Get {
        /// Task ID
        task_id: String,
    },
    /// List tasks, newest first
    List {
        /// Only this queue
        #[arg(long)]
        queue: Option<String>,
        /// Only this status (PENDING, RUNNING, COMPLETED, FAILED, RETRY, DEAD_LETTER, CANCELLED)
        #[arg(long)]
        status: Option<String>,
        /// How many
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
enum LogCommands {
    /// Print a task run's logs and follow new lines
    Tail {
        /// Task run ID
        task_run_id: String,
    },
}

#[derive(Subcommand)]
enum WorkerCommands {
    /// List the workers connected to every node
    List,
}

#[derive(Subcommand)]
enum ClusterCommands {
    /// Health, nodes and shard ownership
    Status,
    /// Every node's lease as this node sees it
    Members,
    /// Hand every shard of a node over to the others, then stop it (admin)
    Drain {
        /// Node ID
        node: String,
    },
    /// Move a shard to a node and pin it there (admin)
    Move {
        /// Shard number
        shard: u16,
        /// Target node ID
        #[arg(long)]
        to: String,
    },
    /// Return a pinned shard to automatic placement (admin)
    Unpin {
        /// Shard number
        shard: u16,
    },
}

#[derive(Subcommand)]
enum QueueCommands {
    /// Queues and their partitions
    List,
    /// Change a queue's partition count (admin)
    Partitions {
        /// Queue name
        queue: String,
        /// New partition count (1-4096)
        count: u32,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    let conn = Conn {
        servers: cli.server,
        http: cli.http.trim_end_matches('/').to_string(),
        api_key: cli.api_key.filter(|k| !k.is_empty()),
        admin_token: cli.admin_token.filter(|k| !k.is_empty()),
    };
    match cli.command {
        Commands::Task { command } => match command {
            TaskCommands::Create {
                queue,
                name,
                input,
                priority,
                max_retries,
                timeout,
                idempotency_key,
                scheduled_at,
                quiet,
            } => {
                commands::task::create(
                    &conn,
                    commands::task::NewTask {
                        queue,
                        name,
                        input,
                        priority,
                        max_retries,
                        timeout,
                        idempotency_key,
                        scheduled_at,
                    },
                    quiet,
                )
                .await
            }
            TaskCommands::Get { task_id } => commands::task::get(&conn, &task_id).await,
            TaskCommands::List {
                queue,
                status,
                limit,
            } => commands::task::list(&conn, queue, status, limit).await,
            TaskCommands::Cancel { task_id } => commands::task::cancel(&conn, &task_id).await,
        },
        Commands::Logs { command } => match command {
            LogCommands::Tail { task_run_id } => commands::logs::tail(&conn, &task_run_id).await,
        },
        Commands::Worker { command } => match command {
            WorkerCommands::List => commands::cluster::workers(&conn).await,
        },
        Commands::Cluster { command } => match command {
            ClusterCommands::Status => commands::cluster::status(&conn).await,
            ClusterCommands::Members => commands::cluster::members(&conn).await,
            ClusterCommands::Drain { node } => commands::cluster::drain(&conn, &node).await,
            ClusterCommands::Move { shard, to } => {
                commands::cluster::move_shard(&conn, shard, &to).await
            }
            ClusterCommands::Unpin { shard } => commands::cluster::unpin(&conn, shard).await,
        },
        Commands::Queue { command } => match command {
            QueueCommands::List => commands::cluster::queues(&conn).await,
            QueueCommands::Partitions { queue, count } => {
                commands::cluster::set_partitions(&conn, &queue, count).await
            }
        },
    }
}

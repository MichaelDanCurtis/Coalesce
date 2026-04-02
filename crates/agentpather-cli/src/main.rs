mod mcp;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing_subscriber::{fmt, EnvFilter};

#[derive(Parser)]
#[command(name = "agentpather", about = "Smart LLM routing proxy", version)]
struct Cli {
    /// Config file path
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the proxy server
    Serve {
        /// Port to listen on (overrides config)
        #[arg(short, long)]
        port: Option<u16>,
        /// Host to bind to (overrides config)
        #[arg(long)]
        host: Option<String>,
    },
    /// Show available models from all providers
    Models,
    /// Health check all providers
    Doctor,
    /// Generate a default config file
    Init {
        /// Output path (default: ./agentpather.toml)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Show request stats from the database
    Stats,
    /// Start the MCP (Model Context Protocol) server over stdio
    Mcp {
        /// AgentPather proxy URL to connect to
        #[arg(long, default_value = "http://127.0.0.1:8402")]
        proxy_url: String,
    },
    /// Authenticate with a provider
    Auth {
        #[command(subcommand)]
        provider: AuthProvider,
    },
    /// Query request history from the database
    History {
        /// Max number of entries to show
        #[arg(short = 'n', long, default_value = "20")]
        limit: u32,
        /// Filter by provider name
        #[arg(short, long)]
        provider: Option<String>,
        /// Filter by routing tier (SIMPLE, MEDIUM, COMPLEX, REASONING)
        #[arg(short, long)]
        tier: Option<String>,
        /// Show only failed requests
        #[arg(long)]
        failures_only: bool,
    },
    /// Show or edit the configuration file
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Manage model exclusion patterns
    Exclude {
        /// Pattern to add (e.g., "gpt-3.5*" or "ollama:*")
        pattern: Option<String>,
        /// Remove a pattern instead of adding
        #[arg(long)]
        remove: bool,
        /// List current exclusion patterns
        #[arg(short, long)]
        list: bool,
    },
    /// Load test the routing proxy
    Bench {
        /// Number of total requests to send
        #[arg(short = 'n', long, default_value = "100")]
        requests: u32,
        /// Number of concurrent clients
        #[arg(short, long, default_value = "10")]
        concurrency: u32,
        /// Target URL (default: http://127.0.0.1:8402)
        #[arg(long, default_value = "http://127.0.0.1:8402")]
        target: String,
        /// Only test routing decision (POST with dry-run, faster)
        #[arg(long)]
        routing_only: bool,
    },
}

#[derive(Subcommand)]
enum AuthProvider {
    /// Authenticate with GitHub Copilot using device flow
    Copilot,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Show the current configuration as TOML
    Show,
    /// Open the config file in your $EDITOR
    Edit,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Load config first to get logging settings
    let mut config =
        agentpather_core::config::AppConfig::load(cli.config.as_ref()).unwrap_or_default();

    // Init logging based on config
    init_logging(&config.logging);

    match cli.command {
        Some(Commands::Serve { port, host }) => {
            if let Some(p) = port {
                config.server.port = p;
            }
            if let Some(h) = host {
                config.server.host = h;
            }
            agentpather_proxy::start_server(config).await?;
        }
        Some(Commands::Models) => {
            print_models(&config).await?;
        }
        Some(Commands::Doctor) => {
            run_doctor(&config).await?;
        }
        Some(Commands::Init { output }) => {
            let path = output.unwrap_or_else(|| PathBuf::from("agentpather.toml"));
            agentpather_core::config::AppConfig::write_default(&path)?;
            println!("Config template written to {}", path.display());
        }
        Some(Commands::Mcp { proxy_url }) => {
            mcp::run_mcp_server(proxy_url).await?;
        }
        Some(Commands::Stats) => {
            print_stats()?;
        }
        Some(Commands::Auth { provider }) => {
            match provider {
                AuthProvider::Copilot => {
                    run_auth_copilot().await?;
                }
            }
        }
        Some(Commands::History { limit, provider, tier, failures_only }) => {
            print_history(limit, provider.as_deref(), tier.as_deref(), failures_only)?;
        }
        Some(Commands::Config { action }) => {
            match action {
                ConfigAction::Show => {
                    let toml_str = toml::to_string_pretty(&config)?;
                    println!("{}", toml_str);
                }
                ConfigAction::Edit => {
                    run_config_edit()?;
                }
            }
        }
        Some(Commands::Exclude { pattern, remove, list }) => {
            run_exclude(pattern.as_deref(), remove, list)?;
        }
        Some(Commands::Bench { requests, concurrency, target, routing_only }) => {
            run_bench(requests, concurrency, &target, routing_only).await?;
        }
        None => {
            // Default: start server
            agentpather_proxy::start_server(config).await?;
        }
    }

    Ok(())
}

fn init_logging(logging: &agentpather_core::config::LoggingConfig) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| {
            EnvFilter::new(format!("agentpather={}", logging.level))
        });

    match logging.format.as_str() {
        "json" => {
            fmt()
                .json()
                .with_env_filter(filter)
                .with_target(true)
                .with_thread_ids(false)
                .init();
        }
        _ => {
            fmt()
                .with_env_filter(filter)
                .with_target(false)
                .init();
        }
    }
}

async fn print_models(config: &agentpather_core::config::AppConfig) -> anyhow::Result<()> {
    use agentpather_core::providers::copilot::CopilotProvider;
    use agentpather_core::providers::ollama::OllamaProvider;
    use agentpather_core::providers::openai_compat::factories;
    use agentpather_core::providers::openrouter::OpenRouterProvider;
    use agentpather_core::providers::Provider;

    println!("Discovering models...\n");

    let mut total = 0;

    for (name, pconfig) in &config.providers {
        if !pconfig.enabled {
            continue;
        }

        let api_key = pconfig
            .api_key
            .clone()
            .or_else(|| {
                pconfig
                    .api_key_env
                    .as_ref()
                    .and_then(|env| std::env::var(env).ok())
            });

        let provider: Option<Box<dyn Provider>> = match name.as_str() {
            "openrouter" => api_key.map(|k| Box::new(OpenRouterProvider::new(k)) as Box<dyn Provider>),
            "ollama" => Some(Box::new(OllamaProvider::new(pconfig.endpoint.clone()))),
            "copilot" => Some(Box::new(if let Some(t) = api_key {
                CopilotProvider::with_token(t)
            } else {
                CopilotProvider::new()
            })),
            "glm" | "zhipu" => api_key.map(|k| Box::new(factories::glm(k)) as Box<dyn Provider>),
            "kimi" => api_key.map(|k| Box::new(factories::kimi(k)) as Box<dyn Provider>),
            "deepseek" => api_key.map(|k| Box::new(factories::deepseek(k)) as Box<dyn Provider>),
            "openai" => api_key.map(|k| Box::new(factories::openai(k)) as Box<dyn Provider>),
            "xai" => api_key.map(|k| Box::new(factories::xai(k)) as Box<dyn Provider>),
            "google" => api_key.map(|k| Box::new(factories::google(k)) as Box<dyn Provider>),
            _ => None,
        };

        if let Some(p) = provider {
            match p.list_models().await {
                Ok(models) => {
                    println!("{}  ({} models)", name, models.len());
                    for m in &models {
                        println!(
                            "  {:40} {:10} ctx:{:>7}  ${:.2}/${:.2}  {}{}{}",
                            m.id,
                            format!("{}", m.quality_tier),
                            m.context_window,
                            m.input_price_per_m,
                            m.output_price_per_m,
                            if m.reasoning { " [R]" } else { "" },
                            if m.vision { " [V]" } else { "" },
                            if m.tool_calling { " [T]" } else { "" },
                        );
                    }
                    total += models.len();
                    println!();
                }
                Err(e) => {
                    println!("{}  ERROR: {}\n", name, e);
                }
            }
        }
    }

    // Auto-detect Ollama
    if !config.providers.contains_key("ollama") {
        let ollama = OllamaProvider::new(None);
        if let Ok(true) = ollama.health_check().await {
            if let Ok(models) = ollama.list_models().await {
                println!("ollama (auto-detected)  ({} models)", models.len());
                for m in &models {
                    println!(
                        "  {:40} {:10} ctx:{:>7}  FREE",
                        m.id,
                        format!("{}", m.quality_tier),
                        m.context_window,
                    );
                }
                total += models.len();
                println!();
            }
        }
    }

    println!("Total: {} models", total);
    Ok(())
}

async fn run_doctor(config: &agentpather_core::config::AppConfig) -> anyhow::Result<()> {
    use agentpather_core::providers::ollama::OllamaProvider;
    use agentpather_core::providers::Provider;

    println!("AgentPather Doctor\n");
    println!("Checking providers...\n");

    let mut ok_count = 0;
    let mut fail_count = 0;

    for (name, pconfig) in &config.providers {
        if !pconfig.enabled {
            println!("  [ ] {} — disabled", name);
            continue;
        }

        let has_key = pconfig
            .api_key
            .is_some()
            || pconfig
                .api_key_env
                .as_ref()
                .is_some_and(|env| std::env::var(env).is_ok());

        if name != "ollama" && name != "copilot" && !has_key {
            println!("  [!] {} — no API key configured", name);
            fail_count += 1;
            continue;
        }

        println!("  [*] {} — configured (billing: {})", name, pconfig.billing.as_deref().unwrap_or("default"));
        ok_count += 1;
    }

    // Ollama auto-detect
    let ollama = OllamaProvider::new(None);
    match ollama.health_check().await {
        Ok(true) => {
            let model_count = ollama
                .list_models()
                .await
                .map(|m| m.len())
                .unwrap_or(0);
            println!("  [+] ollama — running ({} models)", model_count);
            ok_count += 1;
        }
        _ => {
            println!("  [-] ollama — not running");
        }
    }

    // Check data directory
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("agentpather");
    let db_exists = data_dir.join("agentpather.db").exists();

    println!("\nStorage:");
    println!("  Data dir: {}", data_dir.display());
    println!("  Database: {}", if db_exists { "exists" } else { "will be created" });

    println!("\nSummary: {} ok, {} issues", ok_count, fail_count);

    Ok(())
}

async fn run_bench(total_requests: u32, concurrency: u32, target: &str, routing_only: bool) -> anyhow::Result<()> {
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    println!("AgentPather Benchmark");
    println!("=====================");
    println!("Target:      {}", target);
    println!("Requests:    {}", total_requests);
    println!("Concurrency: {}", concurrency);
    println!("Mode:        {}", if routing_only { "routing only" } else { "full proxy" });
    println!();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    // Test prompts of varying complexity
    let prompts = vec![
        "hi",
        "What is 2+2?",
        "Explain the concept of closures in Rust programming language with examples",
        "Write a comprehensive analysis of microservices architecture patterns including circuit breakers, service mesh, and event-driven communication. Include code examples in Go and Rust.",
        "def fibonacci(n):\n  if n <= 1: return n\n  return fibonacci(n-1) + fibonacci(n-2)\n\nPlease analyze the time complexity and suggest optimizations.",
    ];

    let url = if routing_only {
        format!("{}/api/v1/routing/playground", target)
    } else {
        format!("{}/v1/chat/completions", target)
    };

    let success_count = Arc::new(AtomicU32::new(0));
    let error_count = Arc::new(AtomicU32::new(0));
    let total_latency_us = Arc::new(AtomicU64::new(0));
    let latencies = Arc::new(std::sync::Mutex::new(Vec::with_capacity(total_requests as usize)));

    let start = Instant::now();
    let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency as usize));

    let mut handles = Vec::new();

    for i in 0..total_requests {
        let permit = semaphore.clone().acquire_owned().await?;
        let client = client.clone();
        let url = url.clone();
        let prompt = prompts[i as usize % prompts.len()].to_string();
        let success = success_count.clone();
        let errors = error_count.clone();
        let total_lat = total_latency_us.clone();
        let lats = latencies.clone();

        let handle = tokio::spawn(async move {
            let body = if routing_only {
                serde_json::json!({ "prompt": prompt })
            } else {
                serde_json::json!({
                    "model": "auto",
                    "messages": [{"role": "user", "content": prompt}],
                    "max_tokens": 10,
                    "stream": false
                })
            };

            let req_start = Instant::now();
            let result = client.post(&url).json(&body).send().await;
            let latency = req_start.elapsed();

            match result {
                Ok(resp) if resp.status().is_success() => {
                    success.fetch_add(1, Ordering::Relaxed);
                    let us = latency.as_micros() as u64;
                    total_lat.fetch_add(us, Ordering::Relaxed);
                    lats.lock().unwrap().push(us);
                }
                Ok(_resp) => {
                    errors.fetch_add(1, Ordering::Relaxed);
                    let us = latency.as_micros() as u64;
                    lats.lock().unwrap().push(us);
                }
                Err(_) => {
                    errors.fetch_add(1, Ordering::Relaxed);
                }
            }

            drop(permit);
        });

        handles.push(handle);
    }

    // Wait for all requests
    for handle in handles {
        let _ = handle.await;
    }

    let elapsed = start.elapsed();
    let successes = success_count.load(Ordering::Relaxed);
    let errors = error_count.load(Ordering::Relaxed);

    // Calculate percentiles
    let mut all_latencies = latencies.lock().unwrap().clone();
    all_latencies.sort();

    let p50 = percentile(&all_latencies, 50.0);
    let p95 = percentile(&all_latencies, 95.0);
    let p99 = percentile(&all_latencies, 99.0);
    let min_lat = all_latencies.first().copied().unwrap_or(0);
    let max_lat = all_latencies.last().copied().unwrap_or(0);

    let rps = if elapsed.as_secs_f64() > 0.0 {
        total_requests as f64 / elapsed.as_secs_f64()
    } else {
        0.0
    };

    println!("Results");
    println!("-------");
    println!("Duration:    {:.2}s", elapsed.as_secs_f64());
    println!("Throughput:  {:.1} req/s", rps);
    println!("Success:     {} ({:.1}%)", successes, successes as f64 / total_requests as f64 * 100.0);
    println!("Errors:      {} ({:.1}%)", errors, errors as f64 / total_requests as f64 * 100.0);
    println!();
    println!("Latency");
    println!("  Min:       {:.2}ms", min_lat as f64 / 1000.0);
    println!("  p50:       {:.2}ms", p50 as f64 / 1000.0);
    println!("  p95:       {:.2}ms", p95 as f64 / 1000.0);
    println!("  p99:       {:.2}ms", p99 as f64 / 1000.0);
    println!("  Max:       {:.2}ms", max_lat as f64 / 1000.0);

    Ok(())
}

fn percentile(sorted: &[u64], pct: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((pct / 100.0) * (sorted.len() - 1) as f64).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

async fn run_auth_copilot() -> anyhow::Result<()> {
    use agentpather_core::providers::copilot::CopilotProvider;

    println!("Authenticating with GitHub Copilot...\n");

    let provider = CopilotProvider::new();
    let flow = provider.start_device_flow().await?;

    println!("  Code:  {}", flow.user_code);
    println!("  URL:   {}", flow.verification_uri);
    println!();
    println!("Opening browser... Enter the code above to authorize.");

    // Try to open the browser
    let _ = open::that(&flow.verification_uri);

    println!("Waiting for authorization (expires in {}s)...", flow.expires_in);

    let token = provider.poll_for_token(&flow.device_code, flow.interval).await?;

    // Save token to storage
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("agentpather");
    std::fs::create_dir_all(&data_dir)?;
    let db_path = data_dir.join("agentpather.db");
    let storage = agentpather_core::storage::Storage::open(&db_path)?;
    storage.save_token("copilot", &token, None)?;

    println!("\nAuthentication successful! Token saved.");
    println!("Add this to your config to use Copilot:");
    println!();
    println!("  [providers.copilot]");
    println!("  enabled = true");
    println!("  billing = \"quota_refreshing:50:18000\"");

    Ok(())
}

fn print_history(
    limit: u32,
    provider: Option<&str>,
    tier: Option<&str>,
    failures_only: bool,
) -> anyhow::Result<()> {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("agentpather");
    let db_path = data_dir.join("agentpather.db");

    if !db_path.exists() {
        println!("No database found. Run the server first to create it.");
        return Ok(());
    }

    let storage = agentpather_core::storage::Storage::open(&db_path)?;
    let entries = storage.filtered_requests(limit, provider, tier, failures_only)?;

    if entries.is_empty() {
        println!("No matching requests found.");
        return Ok(());
    }

    println!(
        "{:<10} {:<20} {:<30} {:>8} {:>10} {}",
        "TIER", "PROVIDER", "MODEL", "LATENCY", "COST", "STATUS"
    );
    println!("{}", "-".repeat(90));

    for r in &entries {
        println!(
            "{:<10} {:<20} {:<30} {:>6}ms ${:>8.4} {}",
            r.tier,
            r.provider,
            r.model,
            r.latency_ms.unwrap_or(0),
            r.cost_usd.unwrap_or(0.0),
            if r.success { "OK" } else { "FAIL" },
        );
    }

    println!("\n{} entries shown", entries.len());
    Ok(())
}

fn run_config_edit() -> anyhow::Result<()> {
    // Find config file path
    let search_paths = vec![
        PathBuf::from("agentpather.toml"),
        PathBuf::from("config.toml"),
        dirs::config_dir()
            .unwrap_or_default()
            .join("agentpather")
            .join("config.toml"),
    ];

    let config_path = search_paths.iter().find(|p| p.exists());

    let path = if let Some(p) = config_path {
        p.clone()
    } else {
        // Create default config in config dir
        let dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("agentpather");
        std::fs::create_dir_all(&dir)?;
        let p = dir.join("config.toml");
        agentpather_core::config::AppConfig::write_default(&p)?;
        println!("Created default config at {}", p.display());
        p
    };

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| {
        if cfg!(target_os = "windows") {
            "notepad".to_string()
        } else {
            "vi".to_string()
        }
    });

    println!("Opening {} with {}...", path.display(), editor);
    let status = std::process::Command::new(&editor)
        .arg(&path)
        .status()?;

    if !status.success() {
        anyhow::bail!("Editor exited with status {}", status);
    }
    Ok(())
}

fn run_exclude(pattern: Option<&str>, remove: bool, list: bool) -> anyhow::Result<()> {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("agentpather");
    std::fs::create_dir_all(&data_dir)?;
    let db_path = data_dir.join("agentpather.db");
    let storage = agentpather_core::storage::Storage::open(&db_path)?;

    let key = "exclude_patterns";

    // Load existing patterns
    let existing = storage.get(key)?.unwrap_or_default();
    let mut patterns: Vec<String> = if existing.is_empty() {
        Vec::new()
    } else {
        serde_json::from_str(&existing)?
    };

    if list || pattern.is_none() {
        if patterns.is_empty() {
            println!("No exclusion patterns configured.");
        } else {
            println!("Exclusion patterns:");
            for p in &patterns {
                println!("  - {}", p);
            }
        }
        return Ok(());
    }

    let pat = pattern.unwrap();

    if remove {
        let before = patterns.len();
        patterns.retain(|p| p != pat);
        if patterns.len() < before {
            println!("Removed pattern: {}", pat);
        } else {
            println!("Pattern not found: {}", pat);
            return Ok(());
        }
    } else {
        if patterns.contains(&pat.to_string()) {
            println!("Pattern already exists: {}", pat);
            return Ok(());
        }
        patterns.push(pat.to_string());
        println!("Added exclusion pattern: {}", pat);
    }

    storage.set(key, &serde_json::to_string(&patterns)?)?;
    Ok(())
}

fn print_stats() -> anyhow::Result<()> {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("agentpather");
    let db_path = data_dir.join("agentpather.db");

    if !db_path.exists() {
        println!("No database found. Run the server first to create it.");
        return Ok(());
    }

    let storage = agentpather_core::storage::Storage::open(&db_path)?;
    let stats = storage.stats()?;

    println!("AgentPather Stats\n");
    println!("  Total requests:    {}", stats.total_requests);
    println!("  Successful:        {}", stats.successful_requests);
    println!(
        "  Success rate:      {:.1}%",
        if stats.total_requests > 0 {
            stats.successful_requests as f64 / stats.total_requests as f64 * 100.0
        } else {
            100.0
        }
    );
    println!("  Total cost:        ${:.4}", stats.total_cost_usd);
    println!("  Avg latency:       {:.0}ms", stats.avg_latency_ms);
    println!("  Total tokens:      {} in / {} out", stats.total_input_tokens, stats.total_output_tokens);

    println!("\nRecent requests:");
    let recent = storage.recent_requests(10)?;
    for r in &recent {
        println!(
            "  {:8} {:20} {:30} {:>5}ms {}",
            r.tier,
            r.provider,
            r.model,
            r.latency_ms.unwrap_or(0),
            if r.success { "OK" } else { "FAIL" },
        );
    }

    Ok(())
}

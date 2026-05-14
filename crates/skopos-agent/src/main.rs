use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "skopos-agent")]
#[command(about = "Skopos local background agent")]
struct Args {
    #[arg(long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(args.log_level)
        .init();

    tracing::info!("skopos-agent booted");
    Ok(())
}

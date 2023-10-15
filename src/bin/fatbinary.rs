use anyhow;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
struct Cli {
    /// Create fatbin
    #[arg(long = "create")]
    fatbin: Option<PathBuf>,

    /// Image source
    #[arg(long = "image")]
    images: Vec<String>,
}

fn main() -> anyhow::Result<()> {
    let args = Cli::parse();
    Ok(())
}

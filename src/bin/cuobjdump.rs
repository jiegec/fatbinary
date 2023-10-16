use anyhow;
use clap::Parser;
use fatbinary::FatBinary;
use std::{ffi::OsString, fs::File, io::Write, path::PathBuf};

#[derive(Parser)]
struct Cli {
    /// Extract ptx code
    #[arg(long = "extract-ptx")]
    ptx: Option<String>,

    /// Fatbin file
    fatbin: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let args = Cli::parse();
    let file = File::open(&args.fatbin)?;

    let fatbinary = FatBinary::read(file)?;

    if args.ptx.is_some() {
        let mut i = 1;
        let file_name = args
            .fatbin
            .file_stem()
            .map(OsString::from)
            .unwrap_or(OsString::new());
        for entry in fatbinary.entries() {
            if entry.contains_elf() {
                continue;
            }

            let suffix = format!(".{}.sm_{}.ptx", i, entry.get_sm_arch());
            let mut output_file_name = file_name.clone();
            output_file_name.push(suffix);
            println!(
                "Extracting PTX file and ptxas options {:4}: {} -arch=sm_{}",
                i,
                output_file_name.to_string_lossy(),
                entry.get_sm_arch()
            );

            let mut output_file = File::create(output_file_name)?;
            output_file.write_all(&entry.get_decompressed_payload())?;

            i += 1;
        }
        return Ok(());
    }

    for entry in fatbinary.entries() {
        println!();
        println!(
            "Fatbin {} code:",
            if entry.contains_elf() { "elf" } else { "ptx" }
        );
        println!("=================");
        println!("arch = sm_{}", entry.get_sm_arch());
        println!(
            "code version = [{}, {}]",
            entry.get_version_major(),
            entry.get_version_minor()
        );
        println!("producer = <unknown>");
        println!(
            "host = {}",
            if entry.is_linux() { "linux" } else { "windows" }
        );
        println!(
            "compile_size = {}",
            if entry.is_64bit() { "64bit" } else { "32bit" }
        );

        if entry.has_debug_info() {
            println!("has debug info");
        }

        if entry.is_compressed() {
            println!("compressed");
        }
    }
    Ok(())
}

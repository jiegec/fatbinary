use anyhow;
use clap::Parser;
use fatbinary::{FatBinary, FatBinaryEntry};
use std::{fs::File, io::Read, path::PathBuf};

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
    if let Some(fatbin) = args.fatbin {
        let file = File::create(fatbin)?;
        let mut res = FatBinary::new();

        // profile=sm/compute_{sm_arch},file={file}
        for image in args.images {
            let mut file_name = None;
            let mut sm_arch = 50;
            for part in image.split(",") {
                if let Some((key, value)) = part.split_once("=") {
                    if key == "file" {
                        file_name = Some(value);
                    } else if key == "profile" {
                        if let Some((prefix, arch)) = value.split_once("_") {
                            if prefix == "compute" || prefix == "sm" {
                                sm_arch = u32::from_str_radix(arch, 10)?;
                            }
                        }
                    }
                }
            }

            if let Some(file_name) = file_name {
                let mut payload = vec![];
                File::open(file_name)?.read_to_end(&mut payload)?;

                let is_elf = file_name.ends_with(".o");
                let entry = FatBinaryEntry::new(is_elf, sm_arch, payload);
                res.entries_mut().push(entry);
            }
        }

        res.write(file)?;
    }
    Ok(())
}

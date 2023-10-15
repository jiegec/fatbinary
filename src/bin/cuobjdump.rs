use anyhow;
use fatbinary::FatBinary;
use std::fs::File;

fn main() -> anyhow::Result<()> {
    for arg in std::env::args().skip(1) {
        let file = File::open(arg)?;
        let fatbinary = FatBinary::read(file)?;
        for entry in fatbinary.get_entries() {
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
            println!("host = linux");
            println!(
                "compile_size = {}",
                if entry.compile_size_is_64bit() {
                    "64bit"
                } else {
                    "32bit"
                }
            );

            if entry.is_compressed() {
                println!("compressed");
            }
        }
    }
    Ok(())
}

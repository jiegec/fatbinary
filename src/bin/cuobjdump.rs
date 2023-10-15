use std::fs::File;
use anyhow;
use fatbinary::FatBinary;

fn main() -> anyhow::Result<()> {
    for arg in std::env::args().skip(1) {
        let file = File::open(arg)?;
        let fatbinary = FatBinary::read(file)?;
        println!("{:?}", fatbinary);
    }
    Ok(())
}
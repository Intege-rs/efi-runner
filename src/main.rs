use std::path::PathBuf;
use crate::hypervisor::Hypervisor;

mod hypervisor;

/// Uefi Application Test Tool
#[derive(clap::Parser, Debug)]
#[command(version, about, long_about = None)]
struct CLIArgs {

    /// file to boot the vm from
    efi_file: PathBuf,

    /// attach a vhd disk to the virtual machine
    #[arg(short, long)]
    disks: Vec<PathBuf>,

    /// memory in MB
    #[arg(short, long, default_value_t = 1024)]
    memory: u32,

    /// cpu core count
    #[arg(short, long, default_value_t = 2)]
    cores: u8

}


#[tokio::main]
async fn main() {
    let mut args = <CLIArgs as clap::Parser>::parse();

    if !args.efi_file.is_file() {
        eprintln!("EFI_FILE is not a file!");
        std::process::exit(1);
    }

    for vhd in &args.disks {
        if !vhd.is_file() {
            eprintln!("VHD ({}) is not a file!", vhd.display());
            std::process::exit(1);
        }
    }

    // canonicalize paths
    args.efi_file = dunce::canonicalize(args.efi_file).unwrap();
    args.disks = args.disks.into_iter()
        .map(|p|dunce::canonicalize(p).unwrap()).collect();

    let hypervisor = Hypervisor::build("rust-vm", args).await;
    if let Err(e) = hypervisor {
        eprintln!("failed to make hypervisor: {:?}", e);
        std::process::exit(1);
    }
    tokio::time::sleep(std::time::Duration::MAX).await;
}



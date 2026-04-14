use anyhow::{bail, Context};
use std::path::PathBuf;
use std::process::Command;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut feature = None;
    let mut port = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--feature" => {
                i += 1;
                feature = Some(args[i].clone());
            }
            "--port" => {
                i += 1;
                port = Some(args[i].clone());
            }
            other => bail!("Unknown argument: {}", other),
        }
        i += 1;
    }

    let app_dir = project_root().join("app");
    let mut cmd = Command::new("cargo");
    cmd.arg("espflash").arg("flash").arg("--chip").arg("esp32");

    if let Some(ref f) = feature {
        cmd.arg("--features").arg(f);
    }
    if let Some(ref p) = port {
        cmd.arg("-p").arg(p);
    }

    cmd.current_dir(&app_dir);

    println!("Flashing firmware...");
    println!("  Working dir: {}", app_dir.display());
    if let Some(ref f) = feature {
        println!("  Feature: {}", f);
    }
    if let Some(ref p) = port {
        println!("  Port: {}", p);
    }

    let status = cmd.status().context("Failed to run cargo espflash. Is cargo-espflash installed?")?;

    if status.success() {
        println!("Flash successful.");
        Ok(())
    } else {
        bail!("Flash failed with exit code {:?}", status.code());
    }
}

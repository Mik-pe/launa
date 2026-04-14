use anyhow::Result;

pub fn run(args: &[String]) -> Result<()> {
    println!("Flashing and monitoring...");

    crate::flash::run(args)?;

    crate::monitor::run(args)
}

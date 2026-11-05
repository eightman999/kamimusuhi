use std::{env, fs, io, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    let output = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("deploy/resident/monitoring/kamimusuhi-model-usage.json"));
    if args.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: kamimusuhi-monitoring [output.json]",
        )
        .into());
    }
    let mut json = serde_json::to_string_pretty(&kamimusuhi_monitoring::make_dashboard())?;
    json.push('\n');
    fs::write(output, json)?;
    Ok(())
}

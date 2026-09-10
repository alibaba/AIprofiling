// Smoke example for the dashboard transport.
//
// Dry-run (default): tars a directory in memory and prints the size + the
// target URL that would be POSTed. Doesn't touch the network.
//   cargo run --example usage -p dashrs -- --dir /tmp/out --endpoint http://127.0.0.1:7000
//
// Live: adds --live and actually posts to the collector.
//   cargo run --example usage -p dashrs -- --dir /tmp/out \
//     --endpoint http://127.0.0.1:7000 --task-id demo-uuid --live

use std::path::PathBuf;

use dashrs::{ClientIdentity, DashClient, DashConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut dir: Option<PathBuf> = None;
    let mut endpoint = "http://127.0.0.1:7000".to_string();
    let mut task_id = "dry-run-task".to_string();
    let mut live = false;

    while let Some(a) = args.next() {
        match a.as_str() {
            "--dir" => dir = args.next().map(PathBuf::from),
            "--endpoint" => {
                if let Some(v) = args.next() {
                    endpoint = v;
                }
            }
            "--task-id" => {
                if let Some(v) = args.next() {
                    task_id = v;
                }
            }
            "--live" => live = true,
            "-h" | "--help" => {
                println!(
                    "usage: --dir <path> [--endpoint <url>] [--task-id <id>] [--live]"
                );
                return Ok(());
            }
            _ => eprintln!("ignoring unknown arg: {}", a),
        }
    }

    let dir = dir.ok_or("--dir is required")?;
    let bytes = dashrs::tar_gz_dir(&dir)?;
    println!(
        "dashrs: packaged {:?} → {} bytes tar.gz, task_id={}",
        dir,
        bytes.len(),
        task_id
    );

    if !live {
        println!(
            "dry-run: would POST to {}/api/results/upload (pass --live to actually ship)",
            endpoint.trim_end_matches('/')
        );
        return Ok(());
    }

    let identity = ClientIdentity::resolve(None, None, None, None)?;
    println!("resolved client identity: {:?}", identity);
    let cfg = DashConfig::new_defaults(endpoint, identity);
    let client = DashClient::new(cfg)?;
    let resp = client.upload_tar_gz(bytes, &task_id).await?;
    println!(
        "server response: code={} taskId={} message={}",
        resp.code, resp.task_id, resp.message
    );
    Ok(())
}

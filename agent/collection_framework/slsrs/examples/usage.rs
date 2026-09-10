// Smoke example for the SLS transport. By default runs in dry-run mode:
// builds a LogGroup, signs the request, and prints the resulting headers
// + compressed payload size WITHOUT calling the network.
//
// Live mode (set every SLS_* env var):
//   SLS_ENDPOINT=cn-hangzhou.log.aliyuncs.com \
//   SLS_PROJECT=your-project \
//   SLS_LOGSTORE=your-logstore \
//   SLS_AK=your-access-key-id \
//   SLS_SK=your-access-key-secret \
//   SLS_STS=optional-sts-token \
//   cargo run --example usage -p slsrs -- --live

use slsrs::{LogContent, LogEntry, LogGroup, SlsClient, SlsConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let live = std::env::args().any(|a| a == "--live");

    let entry = LogEntry::new(
        chrono::Utc::now().timestamp() as u32,
        vec![
            LogContent::new("source", "slsrs-example"),
            LogContent::new("level", "info"),
            LogContent::new("msg", "hello from AIProf collection_framework"),
        ],
    );

    if !live {
        let mut group = LogGroup::new()
            .with_topic("cf-example")
            .with_source("dry-run");
        group.add_log(entry);

        let raw = slsrs::protobuf::encode_log_group(&group);
        let compressed = lz4_flex::block::compress(&raw);
        println!("dry-run: 1 entry, raw={} bytes, lz4={} bytes", raw.len(), compressed.len());
        println!(
            "would POST to https://<project>.<endpoint>/logstores/<logstore>/shards/lb"
        );
        println!("pass --live with SLS_* env vars set to actually ship the log.");
        return Ok(());
    }

    let config = SlsConfig {
        endpoint: env("SLS_ENDPOINT")?,
        project: env("SLS_PROJECT")?,
        logstore: env("SLS_LOGSTORE")?,
        access_key_id: env("SLS_AK")?,
        access_key_secret: env("SLS_SK")?,
        sts_token: std::env::var("SLS_STS").ok(),
        source: Some("slsrs-example".into()),
        topic: Some("cf-example".into()),
    };
    let client = SlsClient::new(config)?;
    client.put_logs(vec![entry]).await?;
    println!("PutLogs OK");
    Ok(())
}

fn env(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    std::env::var(name).map_err(|_| format!("{} must be set in --live mode", name).into())
}

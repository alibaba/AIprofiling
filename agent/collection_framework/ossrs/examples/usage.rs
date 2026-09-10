use ossrs::{
    multipart::{FileRange, UploadSource},
    OssClient,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configuration
    let endpoint = "oss-cn-zhangjiakou.aliyuncs.com".to_string();
    let bucket = "my-gpu-profiling-bucket".to_string();

    // Decode Base64 encoded credentials
    let ak_encoded = "";
    let sk_encoded = "";
    let sts_encoded = "";

    // Decode from Base64
    use base64::{engine::general_purpose, Engine as _};
    let ak = String::from_utf8(general_purpose::STANDARD.decode(ak_encoded)?)?;
    let sk = String::from_utf8(general_purpose::STANDARD.decode(sk_encoded)?)?;
    let sts_token = String::from_utf8(general_purpose::STANDARD.decode(sts_encoded)?)?;

    println!("Using Access Key: {}", ak);
    let sts = Some(sts_token.as_str());

    let client = OssClient::new(endpoint, bucket, None)?;
    
    println!("\nExample 1: Upload using FileRange");
    let range = FileRange::full_file("/root/log".to_string())?;
    let result = client
        .async_partial_file(
            "/gpu_profiling/0000test.json.gz",
            vec![UploadSource::File(range)],
            &ak,
            &sk,
            sts,
            Some("private"),
        )
        .await?;
    println!("Upload result: {}", result);

    Ok(())
}

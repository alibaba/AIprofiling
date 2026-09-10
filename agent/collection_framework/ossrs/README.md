# OSS Rust Client (ossrs)

A Rust implementation of Alibaba Cloud OSS client with multipart upload and gzip compression support. This is a rewrite of the original Lua-based `ossCli.lua` module.

## Features

- ✅ Multipart upload support for large files
- ✅ Automatic gzip compression during upload
- ✅ Partial file range upload (upload specific byte ranges from files)
- ✅ Automatic retry mechanism for failed uploads
- ✅ Support for OSS security tokens (STS)
- ✅ Configurable ACL permissions
- ✅ Async/await support with Tokio

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
ossrs = { path = "./source/tools/monitor/liveTrace/lib/ossrs" }
tokio = { version = "1.35", features = ["full"] }
```

## Usage

### Basic File Upload

```rust
use ossrs::OssClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = OssClient::new(
        "oss-cn-hangzhou.aliyuncs.com".to_string(),
        "your-bucket".to_string(),
        None, // proxy (optional)
    )?;

    let result = client.upload_file(
        "/remote/path/file.txt.gz",  // OSS path
        "/local/path/file.txt",       // Local file
        "your-access-key",            // AK
        "your-secret-key",            // SK
        None,                         // STS token (optional)
        Some("private"),              // ACL (optional)
    ).await?;

    println!("{}", result);
    Ok(())
}
```

### Partial File Upload (async_partial_file equivalent)

This is the equivalent of the Lua `async_partial_file` function:

```rust
use ossrs::{OssClient, multipart::FileRange};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = OssClient::new(
        "oss-cn-hangzhou.aliyuncs.com".to_string(),
        "your-bucket".to_string(),
        None,
    )?;

    // Define file ranges to upload
    let file_ranges = vec![
        FileRange::new("/path/to/file1.txt".to_string(), 0, 1024),
        FileRange::new("/path/to/file1.txt".to_string(), 2048, 4096),
        FileRange::new("/path/to/file2.txt".to_string(), 0, 2048),
    ];

    let result = client.async_partial_file(
        "/remote/path/combined.txt.gz",
        file_ranges,
        "your-access-key",
        "your-secret-key",
        None,
        Some("private"),
    ).await?;

    println!("{}", result);
    Ok(())
}
```

### Upload Full File Using FileRange

```rust
use ossrs::{OssClient, multipart::FileRange};

let range = FileRange::full_file("/local/path/file.txt".to_string())?;
let result = client.async_partial_file(
    "/remote/path/file.txt.gz",
    vec![range],
    ak,
    sk,
    None,
    Some("private"),
).await?;
```

## API Reference

### `OssClient`

#### Constructor

```rust
pub fn new(
    endpoint: String,
    bucket_name: String,
    proxy: Option<String>
) -> Result<Self>
```

Creates a new OSS client instance.

**Parameters:**
- `endpoint`: OSS endpoint (e.g., "oss-cn-hangzhou.aliyuncs.com")
- `bucket_name`: OSS bucket name
- `proxy`: Optional HTTP/HTTPS proxy URL

#### Methods

##### `upload_file`

```rust
pub async fn upload_file(
    &self,
    oss_path: &str,
    local_file: &str,
    ak: &str,
    sk: &str,
    sts: Option<&str>,
    acl: Option<&str>,
) -> Result<String>
```

Upload a single file with automatic gzip compression.

**Parameters:**
- `oss_path`: Destination path in OSS (e.g., "/logs/app.log.gz")
- `local_file`: Local file path to upload
- `ak`: Access Key ID
- `sk`: Access Key Secret
- `sts`: Optional STS security token
- `acl`: Optional ACL setting ("private", "public-read", "public-read-write")

**Returns:** Success message string

##### `async_partial_file`

```rust
pub async fn async_partial_file(
    &self,
    oss_path: &str,
    file_ranges: Vec<FileRange>,
    ak: &str,
    sk: &str,
    sts: Option<&str>,
    acl: Option<&str>,
) -> Result<String>
```

Upload partial file ranges with gzip compression. This is equivalent to the Lua `async_partial_file` function.

**Parameters:**
- `oss_path`: Destination path in OSS
- `file_ranges`: Vector of file ranges to upload
- `ak`: Access Key ID
- `sk`: Access Key Secret
- `sts`: Optional STS security token
- `acl`: Optional ACL setting

**Returns:** Success message string

### `FileRange`

#### Constructors

```rust
pub fn new(file_path: String, start_offset: u64, end_offset: u64) -> Self
```

Create a file range with specific byte offsets.

```rust
pub fn full_file(file_path: String) -> Result<Self>
```

Create a file range that covers the entire file.

## Comparison with Lua Implementation

| Feature | Lua (ossCli.lua) | Rust (ossrs) |
|---------|------------------|--------------|
| Multipart Upload | ✅ | ✅ |
| Gzip Compression | ✅ | ✅ |
| Partial File Upload | ✅ | ✅ |
| Auto Retry | ✅ (3 retries) | ✅ (3 retries) |
| STS Token Support | ✅ | ✅ |
| ACL Support | ✅ | ✅ |
| Async/Await | ⚠️ (Coroutines) | ✅ (Native) |
| Type Safety | ❌ | ✅ |
| Memory Safety | ❌ | ✅ |
| Performance | Good | Excellent |

## Building

```bash
# Build release version
make build

# Run tests
make test

# Run example
make example

# Build and install library
make install

# Generate documentation
make doc
```

## Error Handling

All functions return `Result<T, OssError>`. Common errors:

- `FileNotFound`: Local file does not exist
- `FileTooLarge`: File exceeds 100GB limit
- `UploadFailed`: Upload request failed (includes status code and body)
- `NetworkError`: Network connectivity issues
- `AuthError`: Authentication failed

Example error handling:

```rust
match client.upload_file(...).await {
    Ok(result) => println!("Success: {}", result),
    Err(OssError::FileNotFound(path)) => {
        eprintln!("File not found: {}", path);
    },
    Err(OssError::UploadFailed(status, body)) => {
        eprintln!("Upload failed with status {}: {}", status, body);
    },
    Err(e) => eprintln!("Error: {}", e),
}
```

## Configuration

### Block Size

The default block size is 1MB (1024 * 1024 bytes). You can modify this in `src/multipart.rs`:

```rust
pub const BLOCK_SIZE: usize = 1024 * 1024; // 1MB
```

### Max File Size

The default maximum file size is 100GB. You can modify this in `src/multipart.rs`:

```rust
pub const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024 * 1024; // 100GB
```

### Retry Configuration

The default retry count is 3. You can modify this in `src/client.rs`:

```rust
const MAX_RETRIES: u32 = 3;
```

## License

Apache-2.0. See `LICENSE` in the repository root.

## Contributing

Contributions are welcome! Please ensure:
1. Code passes all tests (`make test`)
2. Code follows Rust conventions (`cargo fmt` and `cargo clippy`)
3. Add tests for new features

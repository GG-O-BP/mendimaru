use percent_encoding::percent_decode_str;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const MAX_REQUEST_HEADER_BYTES: usize = 16 * 1024;
const MAX_PATH_BYTES: usize = 2 * 1024;
const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;
const SERVED_PREFIX: &str = "/Data/";

#[derive(Debug)]
pub(crate) struct AssetMirrorServer {
    url: String,
    worker: tokio::task::JoinHandle<()>,
}

impl Drop for AssetMirrorServer {
    fn drop(&mut self) {
        self.worker.abort();
    }
}

impl AssetMirrorServer {
    pub(crate) async fn start(shared_directory: &Path) -> Result<Self, String> {
        let root_metadata = std::fs::symlink_metadata(shared_directory)
            .map_err(|error| format!("the configured shared directory is unavailable: {error}"))?;
        if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
            return Err("the configured shared directory is not direct".to_string());
        }
        let root = shared_directory
            .canonicalize()
            .map_err(|error| format!("the configured shared directory is unavailable: {error}"))?;
        let metadata = std::fs::symlink_metadata(&root)
            .map_err(|error| format!("the configured shared directory is unavailable: {error}"))?;
        if !metadata.is_dir() {
            return Err("the configured shared directory is not direct".to_string());
        }
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|error| format!("the loopback asset mirror could not start: {error}"))?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("the asset mirror address is unavailable: {error}"))?;
        let worker = tokio::spawn(serve(listener, Arc::new(root)));
        Ok(Self {
            url: format!("http://{address}"),
            worker,
        })
    }

    pub(crate) fn url(&self) -> &str {
        &self.url
    }
}

async fn serve(listener: TcpListener, root: Arc<PathBuf>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let root = Arc::clone(&root);
        tokio::spawn(async move {
            let _ = handle_connection(stream, root).await;
        });
    }
}

async fn handle_connection(mut stream: TcpStream, root: Arc<PathBuf>) -> Result<(), String> {
    let request = read_request_head(&mut stream).await?;
    let (method, target) = parse_request_line(&request)?;
    if !matches!(method.as_str(), "GET" | "HEAD") {
        write_simple_response(&mut stream, 405, "Method Not Allowed", b"")
            .await
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
    let path = match safe_mirror_path(&target, &root) {
        Some(path) => path,
        None => {
            write_simple_response(&mut stream, 404, "Not Found", b"")
                .await
                .map_err(|error| error.to_string())?;
            return Ok(());
        }
    };
    let metadata = match tokio::fs::symlink_metadata(&path).await {
        Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_ASSET_BYTES => metadata,
        _ => {
            write_simple_response(&mut stream, 404, "Not Found", b"")
                .await
                .map_err(|error| error.to_string())?;
            return Ok(());
        }
    };
    let file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(_) => {
            write_simple_response(&mut stream, 404, "Not Found", b"")
                .await
                .map_err(|error| error.to_string())?;
            return Ok(());
        }
    };
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_ASSET_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_ASSET_BYTES {
        write_simple_response(&mut stream, 404, "Not Found", b"")
            .await
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\n\
         Cross-Origin-Resource-Policy: cross-origin\r\nConnection: close\r\n\r\n",
        mime_type(&path),
        bytes.len(),
    );
    stream
        .write_all(head.as_bytes())
        .await
        .map_err(|error| error.to_string())?;
    if method != "HEAD" {
        stream
            .write_all(&bytes)
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

async fn read_request_head(stream: &mut TcpStream) -> Result<String, String> {
    let mut bytes = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 1024];
    loop {
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if bytes.len() > MAX_REQUEST_HEADER_BYTES {
            return Err("the asset mirror request header is too large".to_string());
        }
        let count = stream
            .read(&mut chunk)
            .await
            .map_err(|error| error.to_string())?;
        if count == 0 {
            return Err("the asset mirror request ended early".to_string());
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    String::from_utf8(bytes).map_err(|_| "the asset mirror request is not UTF-8".to_string())
}

fn parse_request_line(request: &str) -> Result<(String, String), String> {
    let head_end = request
        .find("\r\n\r\n")
        .ok_or_else(|| "the asset mirror request is malformed".to_string())?;
    let mut lines = request[..head_end].split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| "the asset mirror request is malformed".to_string())?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| "the asset mirror request is malformed".to_string())?
        .to_string();
    let target = parts
        .next()
        .ok_or_else(|| "the asset mirror request is malformed".to_string())?
        .to_string();
    let version = parts
        .next()
        .ok_or_else(|| "the asset mirror request is malformed".to_string())?;
    if version != "HTTP/1.1" || parts.next().is_some() {
        return Err("the asset mirror request is malformed".to_string());
    }
    for header in lines {
        let name = header
            .split_once(':')
            .map(|(name, _)| name.trim().to_ascii_lowercase())
            .ok_or_else(|| "the asset mirror request is malformed".to_string())?;
        if name == "content-length" {
            let value = header
                .rsplit_once(':')
                .map(|(_, value)| value.trim())
                .ok_or_else(|| "the asset mirror request is malformed".to_string())?;
            if value != "0" {
                return Err("the asset mirror request has an unsupported body".to_string());
            }
        }
    }
    Ok((method, target))
}

fn safe_mirror_path(target: &str, root: &Path) -> Option<PathBuf> {
    if target.contains('?') || target.contains('#') {
        return None;
    }
    if target.len() > MAX_PATH_BYTES || !target.starts_with(SERVED_PREFIX) {
        return None;
    }
    let decoded = percent_decode_str(target).decode_utf8().ok()?.into_owned();
    if decoded.contains('\\') || decoded.contains('\0') {
        return None;
    }
    let relative = Path::new(&decoded).strip_prefix(SERVED_PREFIX).ok()?;
    let components = relative
        .components()
        .map(|component| match component {
            Component::Normal(value) => Some(value.to_os_string()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    if components.len() < 4 {
        return None;
    }
    if components[1] != "deployment" || components[2] != "web" {
        return None;
    }
    let mut path = root.to_path_buf();
    for component in components {
        path.push(component);
        let metadata = std::fs::symlink_metadata(&path).ok()?;
        if metadata.file_type().is_symlink() {
            return None;
        }
    }
    Some(path)
}

async fn write_simple_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    body: &[u8],
) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len(),
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await
}

fn mime_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "css" => "text/css; charset=utf-8",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "jpg" | "jpeg" => "image/jpeg",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "wasm" => "application/wasm",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::{safe_mirror_path, AssetMirrorServer};
    use std::fs;

    #[test]
    fn accepts_only_direct_project_web_assets() {
        let temporary = tempfile::tempdir().expect("temporary mirror root");
        let root = temporary.path();
        fs::create_dir_all(root.join("Project/deployment/web/widgets"))
            .expect("project web directory");
        fs::write(
            root.join("Project/deployment/web/widgets/a.mjs"),
            b"export {}",
        )
        .expect("asset");
        assert!(safe_mirror_path("/Data/Project/deployment/web/widgets/a.mjs", root).is_some());
        assert!(safe_mirror_path("/Data/Project/other/web/a.mjs", root).is_none());
        assert!(safe_mirror_path("/Data/Project/deployment/web/../../secret", root).is_none());
        assert!(
            safe_mirror_path("/Data/Project/deployment/web/widgets/a.mjs?cache=1", root).is_none()
        );
        assert!(safe_mirror_path("/other/Project/deployment/web/a.mjs", root).is_none());
    }

    #[tokio::test]
    async fn serves_project_web_assets_on_loopback_only() {
        let temporary = tempfile::tempdir().expect("temporary mirror root");
        let web = temporary.path().join("Project/deployment/web");
        fs::create_dir_all(&web).expect("project web directory");
        fs::write(web.join("widget.mjs"), b"export const ready = true;").expect("widget asset");
        let server = AssetMirrorServer::start(temporary.path())
            .await
            .expect("asset mirror starts");

        let client = reqwest::Client::new();
        let response = client
            .get(format!(
                "{}/Data/Project/deployment/web/widget.mjs",
                server.url()
            ))
            .send()
            .await
            .expect("asset request");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("access-control-allow-origin")
                .unwrap(),
            "*"
        );
        let bytes = response.bytes().await.expect("asset bytes");
        assert_eq!(bytes.as_ref(), b"export const ready = true;");

        let rejected = client
            .get(format!(
                "{}/Data/Project/deployment/model/model.mdp",
                server.url()
            ))
            .send()
            .await
            .expect("rejected asset request");
        assert_eq!(rejected.status(), reqwest::StatusCode::NOT_FOUND);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_assets() {
        use std::os::unix::fs::symlink;
        let temporary = tempfile::tempdir().expect("temporary mirror root");
        let project = temporary.path().join("Project/deployment/web");
        fs::create_dir_all(&project).expect("project web directory");
        fs::write(project.join("real.mjs"), b"export {}").expect("real asset");
        symlink(project.join("real.mjs"), project.join("linked.mjs")).expect("linked asset");

        assert!(
            safe_mirror_path("/Data/Project/deployment/web/real.mjs", temporary.path()).is_some()
        );
        assert!(
            safe_mirror_path("/Data/Project/deployment/web/linked.mjs", temporary.path()).is_none()
        );
    }
}

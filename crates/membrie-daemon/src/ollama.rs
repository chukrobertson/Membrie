use anyhow::{Context, Result, anyhow, bail};
use membrie_core::LocalModel;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

const OLLAMA_ADDRESS: &str = "127.0.0.1:11434";
const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct OllamaClient {
    address: SocketAddr,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_owned(),
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_owned(),
            content: content.into(),
        }
    }
}

impl Default for OllamaClient {
    fn default() -> Self {
        Self {
            address: OLLAMA_ADDRESS
                .parse()
                .expect("the fixed Ollama loopback address must be valid"),
        }
    }
}

impl OllamaClient {
    pub fn list_models(&self) -> Result<Vec<LocalModel>> {
        let response: TagsResponse = self.get_json("/api/tags", Duration::from_secs(3))?;
        Ok(response
            .models
            .into_iter()
            .filter(|model| model.size > 0 && model_is_local(&model.name))
            .map(|model| LocalModel {
                name: model.name,
                size_bytes: model.size,
                parameter_size: model.details.parameter_size,
                quantization: model.details.quantization_level,
            })
            .collect())
    }

    pub fn embed(&self, model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        require_local_model(model)?;
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let response: EmbedResponse = self.post_json(
            "/api/embed",
            &EmbedRequest {
                model,
                input: inputs,
                truncate: false,
                keep_alive: "5m",
            },
            Duration::from_secs(5 * 60),
        )?;
        if response.embeddings.len() != inputs.len() {
            bail!(
                "Ollama returned {} embeddings for {} inputs",
                response.embeddings.len(),
                inputs.len()
            );
        }
        if response
            .embeddings
            .iter()
            .any(|embedding| embedding.is_empty())
        {
            bail!("Ollama returned an empty embedding");
        }
        Ok(response.embeddings)
    }

    pub fn chat(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        context_tokens: u32,
        max_output_tokens: u32,
        temperature: f32,
        format: Option<Value>,
    ) -> Result<String> {
        require_local_model(model)?;
        let response: ChatResponse = self.post_json(
            "/api/chat",
            &ChatRequest {
                model,
                messages,
                stream: false,
                think: false,
                keep_alive: "5m",
                format,
                options: ChatOptions {
                    num_ctx: context_tokens,
                    num_predict: max_output_tokens,
                    temperature,
                },
            },
            Duration::from_secs(15 * 60),
        )?;
        let content = response.message.content.trim().to_owned();
        if content.is_empty() {
            bail!("Ollama returned an empty response");
        }
        Ok(content)
    }

    fn get_json<T: DeserializeOwned>(&self, path: &str, timeout: Duration) -> Result<T> {
        let body = self.request("GET", path, None, timeout)?;
        serde_json::from_slice(&body).context("Ollama returned invalid JSON")
    }

    fn post_json<T: DeserializeOwned>(
        &self,
        path: &str,
        request: &impl Serialize,
        timeout: Duration,
    ) -> Result<T> {
        let request = serde_json::to_vec(request)?;
        let body = self.request("POST", path, Some(&request), timeout)?;
        serde_json::from_slice(&body).context("Ollama returned invalid JSON")
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        let mut stream = TcpStream::connect_timeout(&self.address, Duration::from_secs(2))
            .context("could not connect to local Ollama at 127.0.0.1:11434")?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;
        let body = body.unwrap_or_default();
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )?;
        stream.write_all(body)?;
        stream.flush()?;

        let mut response = Vec::new();
        stream
            .take(MAX_RESPONSE_BYTES)
            .read_to_end(&mut response)
            .context("could not read Ollama response")?;
        parse_http_response(&response)
    }
}

fn model_is_local(model: &str) -> bool {
    !model.to_ascii_lowercase().contains("cloud")
}

fn require_local_model(model: &str) -> Result<()> {
    if !model_is_local(model) {
        bail!("cloud-backed Ollama models are not allowed in Membrie");
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct TagsResponse {
    models: Vec<TagModel>,
}

#[derive(Debug, Deserialize)]
struct TagModel {
    name: String,
    size: u64,
    #[serde(default)]
    details: TagDetails,
}

#[derive(Debug, Default, Deserialize)]
struct TagDetails {
    parameter_size: Option<String>,
    quantization_level: Option<String>,
}

#[derive(Debug, Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a [String],
    truncate: bool,
    keep_alive: &'a str,
}

#[derive(Debug, Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage>,
    stream: bool,
    think: bool,
    keep_alive: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<Value>,
    options: ChatOptions,
}

#[derive(Debug, Serialize)]
struct ChatOptions {
    num_ctx: u32,
    num_predict: u32,
    temperature: f32,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    message: ChatResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ChatResponseMessage {
    content: String,
}

fn parse_http_response(response: &[u8]) -> Result<Vec<u8>> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| anyhow!("Ollama returned an invalid HTTP response"))?;
    let headers = std::str::from_utf8(&response[..header_end])
        .context("Ollama returned invalid HTTP headers")?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or_else(|| anyhow!("Ollama returned an invalid HTTP status"))?;
    let body = &response[header_end + 4..];
    let is_chunked = headers.lines().any(|line| {
        line.to_ascii_lowercase()
            .starts_with("transfer-encoding: chunked")
    });
    let body = if is_chunked {
        decode_chunked(body)?
    } else {
        body.to_vec()
    };
    if !(200..300).contains(&status) {
        let message = serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|value| value.get("error")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| String::from_utf8_lossy(&body).trim().to_owned());
        bail!("Ollama request failed ({status}): {message}");
    }
    Ok(body)
}

fn decode_chunked(mut input: &[u8]) -> Result<Vec<u8>> {
    let mut decoded = Vec::new();
    loop {
        let line_end = input
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| anyhow!("invalid chunked response from Ollama"))?;
        let size_text = std::str::from_utf8(&input[..line_end])?
            .split(';')
            .next()
            .unwrap_or_default();
        let size = usize::from_str_radix(size_text.trim(), 16)
            .context("invalid Ollama response chunk size")?;
        input = &input[line_end + 2..];
        if size == 0 {
            break;
        }
        if input.len() < size + 2 || &input[size..size + 2] != b"\r\n" {
            bail!("truncated chunked response from Ollama");
        }
        decoded.extend_from_slice(&input[..size]);
        input = &input[size + 2..];
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_content_length_response() {
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\n{\"ok\":true}";
        assert_eq!(parse_http_response(response).unwrap(), b"{\"ok\":true}");
    }

    #[test]
    fn parses_chunked_response() {
        let response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        assert_eq!(parse_http_response(response).unwrap(), b"hello world");
    }

    #[test]
    fn rejects_cloud_model_names() {
        assert!(require_local_model("gemma4:12b").is_ok());
        assert!(require_local_model("gemma4:cloud").is_err());
    }
}

//! Cloud speech recognition services. Each one streams 16 kHz mono PCM over a WebSocket and
//! reports sentences as `Update`s, so the runtime treats every provider the same way.

use crate::credentials::CredentialKind;
pub use crate::soniox::Update;
use std::io;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::HeaderValue;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{client_tls_with_config, protocol::WebSocketConfig, WebSocket};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Provider {
    Soniox,
    Doubao,
    Bailian,
    ElevenLabs,
}

impl Provider {
    pub const ALL: [Provider; 4] = [
        Provider::Soniox,
        Provider::Doubao,
        Provider::Bailian,
        Provider::ElevenLabs,
    ];

    pub fn from_engine(engine: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|provider| provider.engine() == engine)
    }

    /// The value stored in `settings.engine` and on each recording run.
    pub fn engine(self) -> &'static str {
        match self {
            Self::Soniox => "soniox",
            Self::Doubao => "doubao",
            Self::Bailian => "bailian",
            Self::ElevenLabs => "elevenlabs",
        }
    }

    /// The name people see in messages.
    pub fn name(self) -> &'static str {
        match self {
            Self::Soniox => "Soniox",
            Self::Doubao => "豆包语音",
            Self::Bailian => "阿里云百炼",
            Self::ElevenLabs => "ElevenLabs",
        }
    }

    pub fn credential(self) -> CredentialKind {
        match self {
            Self::Soniox => CredentialKind::Soniox,
            Self::Doubao => CredentialKind::Doubao,
            Self::Bailian => CredentialKind::Bailian,
            Self::ElevenLabs => CredentialKind::ElevenLabs,
        }
    }
}

pub fn is_cloud_engine(engine: &str) -> bool {
    Provider::from_engine(engine).is_some()
}

/// Everything a connection needs besides the audio.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub key: String,
    /// "auto", "en" or "zh".
    pub language: String,
    /// Course vocabulary, already de-duplicated and bounded.
    pub terms: Vec<String>,
    /// Service region where a provider has several (百炼: "beijing" or "singapore").
    pub region: String,
}

pub trait CloudStream: Send {
    fn send_audio(&mut self, bytes: &[u8]) -> Result<(), String>;
    fn finish_input(&mut self) -> Result<(), String>;
    fn poll(&mut self) -> Result<Vec<Update>, String>;
    fn finished(&self) -> bool;
}

impl CloudStream for Box<dyn CloudStream> {
    fn send_audio(&mut self, bytes: &[u8]) -> Result<(), String> {
        (**self).send_audio(bytes)
    }
    fn finish_input(&mut self) -> Result<(), String> {
        (**self).finish_input()
    }
    fn poll(&mut self) -> Result<Vec<Update>, String> {
        (**self).poll()
    }
    fn finished(&self) -> bool {
        (**self).finished()
    }
}

impl CloudStream for crate::soniox::Client {
    fn send_audio(&mut self, bytes: &[u8]) -> Result<(), String> {
        crate::soniox::Client::send_audio(self, bytes)
    }
    fn finish_input(&mut self) -> Result<(), String> {
        crate::soniox::Client::finish_input(self)
    }
    fn poll(&mut self) -> Result<Vec<Update>, String> {
        crate::soniox::Client::poll(self)
    }
    fn finished(&self) -> bool {
        crate::soniox::Client::finished(self)
    }
}

pub fn connect(provider: Provider, config: &Config) -> Result<Box<dyn CloudStream>, String> {
    validate_key(provider, &config.key)?;
    Ok(match provider {
        Provider::Soniox => Box::new(crate::soniox::Client::connect(
            &config.key,
            &config.language,
            &config.terms,
        )?),
        Provider::Doubao => Box::new(crate::doubao::Client::connect(config)?),
        Provider::Bailian => Box::new(crate::bailian::Client::connect(config)?),
        Provider::ElevenLabs => Box::new(crate::elevenlabs::Client::connect(config)?),
    })
}

pub fn validate_key(provider: Provider, key: &str) -> Result<(), String> {
    if key.is_empty()
        || key.len() > 512
        || key.chars().any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(format!("{} API Key 格式无效", provider.name()));
    }
    Ok(())
}

/// Errors that a new connection can fix: dropped links, read failures, session time limits.
/// The runtime reconnects on these and resumes from the last finished sentence.
pub fn transient(provider: Provider, detail: &str) -> String {
    format!("{} 连接中断：{detail}", provider.name())
}

pub fn is_transient(message: &str) -> bool {
    message.contains(" 连接中断：")
}

pub(crate) type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const IO_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const POLL_TIMEOUT: Duration = Duration::from_millis(35);
const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

/// Opens a TLS WebSocket with the given handshake headers. An HTTP refusal is reported in words
/// a user can act on: a wrong key reads as a wrong key, not as a network failure.
pub(crate) fn open(
    provider: Provider,
    url: &str,
    headers: &[(&'static str, String)],
) -> Result<Socket, String> {
    let name = provider.name();
    let mut request = url
        .into_client_request()
        .map_err(|_| format!("{name} 服务地址无效"))?;
    for (header, value) in headers {
        request.headers_mut().insert(
            *header,
            HeaderValue::from_str(value).map_err(|_| format!("{name} 请求头无效"))?,
        );
    }
    let host = request
        .uri()
        .host()
        .ok_or_else(|| format!("{name} 服务地址无效"))?
        .to_owned();
    let port = request.uri().port_u16().unwrap_or(443);
    let started = Instant::now();
    let addresses = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|_| transient(provider, "无法解析服务地址，请检查网络"))?;
    let mut stream = None;
    for address in addresses {
        let remaining = CONNECT_TIMEOUT.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            break;
        }
        if let Ok(candidate) = TcpStream::connect_timeout(&address, remaining) {
            stream = Some(candidate);
            break;
        }
    }
    let stream = stream.ok_or_else(|| transient(provider, "无法连接服务，请检查网络"))?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(IO_TIMEOUT)))
        .map_err(|_| format!("{name} 无法设置网络超时"))?;
    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(MAX_MESSAGE_BYTES);
    config.max_frame_size = Some(MAX_MESSAGE_BYTES);
    let mut socket = match client_tls_with_config(request, stream, Some(config), None) {
        Ok((socket, _)) => socket,
        Err(tungstenite::handshake::HandshakeError::Failure(tungstenite::Error::Http(response))) => {
            let status = response.status().as_u16();
            let body = response
                .body()
                .as_deref()
                .map(|body| String::from_utf8_lossy(body).chars().take(200).collect::<String>())
                .unwrap_or_default();
            return Err(handshake_refusal(provider, status, &body));
        }
        Err(_) => return Err(transient(provider, "TLS/WebSocket 握手失败，请检查网络")),
    };
    set_timeouts(&mut socket, IO_TIMEOUT, provider)?;
    Ok(socket)
}

pub(crate) fn handshake_refusal(provider: Provider, status: u16, body: &str) -> String {
    let name = provider.name();
    let detail = if body.trim().is_empty() {
        String::new()
    } else {
        format!("（{}）", body.trim())
    };
    match status {
        401 | 403 => format!("{name} 拒绝了这个 API Key，请检查 Key 是否正确、服务是否已开通{detail}"),
        402 => format!("{name} 账户余额或额度不足{detail}"),
        429 => transient(provider, &format!("请求过于频繁或并发已满{detail}")),
        500..=599 => transient(provider, &format!("服务暂时不可用（HTTP {status}）{detail}")),
        _ => format!("{name} 拒绝了连接（HTTP {status}）{detail}"),
    }
}

pub(crate) fn set_timeouts(
    socket: &mut Socket,
    timeout: Duration,
    provider: Provider,
) -> Result<(), String> {
    let stream = match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => stream,
        MaybeTlsStream::Rustls(stream) => &mut stream.sock,
        _ => return Err(format!("{} TLS 类型不受支持", provider.name())),
    };
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|()| stream.set_write_timeout(Some(timeout)))
        .map_err(|_| format!("{} 无法设置网络超时", provider.name()))
}

pub(crate) fn is_poll_timeout(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

pub(crate) fn millis_to_samples(milliseconds: u64) -> u64 {
    milliseconds.saturating_mul(16)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Connects to each real service with a made-up key. A refusal of the key (rather than an
    /// unknown address or a protocol error) shows the endpoint, handshake and auth header are right.
    #[test]
    #[ignore = "contacts the real services"]
    fn real_services_refuse_a_made_up_key() {
        for (provider, region) in [
            (Provider::Doubao, ""),
            (Provider::Bailian, "beijing"),
            (Provider::Bailian, "singapore"),
            (Provider::ElevenLabs, ""),
            (Provider::Soniox, ""),
        ] {
            let config = Config {
                key: "suitang-made-up-key-0000000000000000".into(),
                language: "auto".into(),
                terms: vec!["Tocqueville".into()],
                region: region.into(),
            };
            let outcome = connect(provider, &config).and_then(|mut client| {
                client.send_audio(&[0u8; 6400])?;
                let deadline = Instant::now() + Duration::from_secs(8);
                while Instant::now() < deadline {
                    client.poll()?;
                }
                Ok(())
            });
            println!("PROBE {} {region}: {:?}", provider.name(), outcome.err());
        }
    }

    #[test]
    fn engines_round_trip_and_names_are_distinct() {
        for provider in Provider::ALL {
            assert_eq!(Provider::from_engine(provider.engine()), Some(provider));
            assert!(is_cloud_engine(provider.engine()));
        }
        assert!(!is_cloud_engine("qwen"));
        assert!(!is_cloud_engine(""));
    }

    #[test]
    fn keys_with_spaces_or_newlines_are_rejected_before_connecting() {
        assert!(validate_key(Provider::Doubao, "").is_err());
        assert!(validate_key(Provider::Doubao, "abc def").is_err());
        assert!(validate_key(Provider::Doubao, "abc\n").is_err());
        assert!(validate_key(Provider::Doubao, "sk-0123456789abcdef").is_ok());
    }

    #[test]
    fn a_refused_key_reads_as_a_key_problem_and_busy_servers_retry() {
        let refused = handshake_refusal(Provider::Bailian, 401, "InvalidApiKey");
        assert!(refused.contains("API Key") && !is_transient(&refused));
        assert!(is_transient(&handshake_refusal(Provider::Bailian, 503, "")));
        assert!(is_transient(&handshake_refusal(Provider::ElevenLabs, 429, "")));
    }
}

/// A one-connection WebSocket server on localhost that plays a provider's side of the protocol.
#[cfg(test)]
pub(crate) mod mock {
    use std::collections::HashMap;
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::thread;
    use tungstenite::handshake::server::{Request, Response};
    use tungstenite::WebSocket;

    pub(crate) struct Handshake {
        pub path_and_query: String,
        pub headers: HashMap<String, String>,
    }

    /// Returns the ws:// URL and a receiver for the handshake the client made.
    pub(crate) fn serve<F>(script: F) -> (String, mpsc::Receiver<Handshake>, thread::JoinHandle<()>)
    where
        F: FnOnce(&mut WebSocket<TcpStream>) + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}/mock", listener.local_addr().unwrap());
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut socket = tungstenite::accept_hdr(stream, |request: &Request, response: Response| {
                let headers = request
                    .headers()
                    .iter()
                    .map(|(name, value)| (name.as_str().to_lowercase(), value.to_str().unwrap_or("").to_owned()))
                    .collect();
                let path_and_query = request.uri().path_and_query().map(ToString::to_string).unwrap_or_default();
                sender.send(Handshake { path_and_query, headers }).unwrap();
                Ok(response)
            })
            .unwrap();
            script(&mut socket);
            let _ = socket.close(None);
            let _ = socket.flush();
        });
        (url, receiver, handle)
    }

    /// Drives a client like the runtime does: audio in 200 ms chunks, then finish, polling
    /// throughout, until the stream reports it has finished.
    pub(crate) fn run(client: &mut dyn super::CloudStream, chunks: usize) -> Vec<super::Update> {
        let mut updates = Vec::new();
        for _ in 0..chunks {
            client.send_audio(&[1u8; 6400]).unwrap();
            for _ in 0..5 {
                updates.extend(client.poll().unwrap());
            }
        }
        client.finish_input().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !client.finished() && std::time::Instant::now() < deadline {
            updates.extend(client.poll().unwrap());
        }
        assert!(client.finished(), "stream did not finish");
        updates
    }
}

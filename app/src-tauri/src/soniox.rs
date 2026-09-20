use serde::Deserialize;
use serde_json::{json, Value};
use std::io;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{client_tls_with_config, protocol::WebSocketConfig, Message, WebSocket};

const ENDPOINT: &str = "wss://stt-rt.soniox.com/transcribe-websocket";
const HOST: &str = "stt-rt.soniox.com";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_TIMEOUT: Duration = Duration::from_millis(35);
const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;
const MAX_AUDIO_CHUNK_BYTES: usize = 1024 * 1024;

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    pub index: u64,
    pub start_sample: u64,
    pub end_sample: u64,
    pub text: String,
    pub final_result: bool,
}

pub struct Client {
    socket: Socket,
    parser: Parser,
    input_finished: bool,
}

fn session_configuration(api_key: &str, language: &str, terms: &[String]) -> Result<Value, String> {
    if api_key.is_empty() || api_key.len() > 256 {
        return Err("Soniox API 密钥格式无效".into());
    }
    let language_hints = match language {
        "auto" | "" => None,
        "en" | "zh" => Some(vec![language]),
        _ => return Err("Soniox 识别语言设置无效".into()),
    };
    if terms.len() > 200
        || terms.iter().any(|term| term.is_empty() || term.len() > 200)
        || terms.iter().map(String::len).sum::<usize>() > 10_000
    {
        return Err("Soniox 常用词设置无效".into());
    }
    let mut initial = json!({
        "api_key": api_key,
        "model": "stt-rt-v5",
        "audio_format": "pcm_s16le",
        "sample_rate": 16000,
        "num_channels": 1,
        "enable_endpoint_detection": true
    });
    if let Some(hints) = language_hints {
        initial["language_hints"] = json!(hints);
    }
    if !terms.is_empty() {
        initial["context"] = json!({"terms": terms});
    }
    Ok(initial)
}

impl Client {
    pub fn connect(api_key: &str, language: &str, terms: &[String]) -> Result<Self, String> {
        let initial = session_configuration(api_key, language, terms)?;

        let started = Instant::now();
        let addresses = (HOST, 443)
            .to_socket_addrs()
            .map_err(|_| "无法解析 Soniox 服务地址".to_string())?;
        let mut last_error = None;
        let mut stream = None;
        for address in addresses {
            let remaining = CONNECT_TIMEOUT.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                break;
            }
            match TcpStream::connect_timeout(&address, remaining) {
                Ok(candidate) => {
                    stream = Some(candidate);
                    break;
                }
                Err(error) => last_error = Some(error),
            }
        }
        let stream = stream.ok_or_else(|| {
            if last_error.is_some() {
                "无法连接 Soniox 服务"
            } else {
                "Soniox 服务没有可用地址"
            }
            .to_string()
        })?;
        stream
            .set_read_timeout(Some(IO_TIMEOUT))
            .map_err(|_| "无法设置 Soniox 读取超时".to_string())?;
        stream
            .set_write_timeout(Some(IO_TIMEOUT))
            .map_err(|_| "无法设置 Soniox 写入超时".to_string())?;

        let mut config = WebSocketConfig::default();
        config.max_message_size = Some(MAX_MESSAGE_BYTES);
        config.max_frame_size = Some(MAX_MESSAGE_BYTES);
        let (mut socket, response) = client_tls_with_config(ENDPOINT, stream, Some(config), None)
            .map_err(|_| "Soniox TLS/WebSocket 连接失败".to_string())?;
        if response.status().as_u16() != 101 {
            return Err("Soniox WebSocket 握手失败".into());
        }
        set_socket_timeouts(&mut socket, IO_TIMEOUT)?;

        socket
            .send(Message::Text(initial.to_string().into()))
            .map_err(|_| "Soniox 会话配置发送失败".to_string())?;
        set_socket_timeouts(&mut socket, POLL_TIMEOUT)?;

        Ok(Self {
            socket,
            parser: Parser::default(),
            input_finished: false,
        })
    }

    pub fn send_audio(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.input_finished || self.parser.finished {
            return Err("Soniox 音频输入已经结束".into());
        }
        if bytes.is_empty() || !bytes.len().is_multiple_of(2) || bytes.len() > MAX_AUDIO_CHUNK_BYTES
        {
            return Err("Soniox PCM 音频块格式无效".into());
        }
        set_socket_timeouts(&mut self.socket, IO_TIMEOUT)?;
        let result = self
            .socket
            .send(Message::Binary(bytes.to_vec().into()))
            .map_err(|_| "Soniox 音频发送失败".to_string());
        set_socket_timeouts(&mut self.socket, POLL_TIMEOUT)?;
        result
    }

    pub fn finish_input(&mut self) -> Result<(), String> {
        if self.input_finished {
            return Ok(());
        }
        if self.parser.finished {
            self.input_finished = true;
            return Ok(());
        }
        set_socket_timeouts(&mut self.socket, IO_TIMEOUT)?;
        let result: Result<(), String> = (|| {
            self.socket
                .send(Message::Text(r#"{"type":"finalize"}"#.into()))
                .map_err(|_| "Soniox 最终化请求发送失败".to_string())?;
            self.socket
                .send(Message::Binary(Vec::new().into()))
                .map_err(|_| "Soniox 结束请求发送失败".to_string())?;
            Ok(())
        })();
        let timeout_result = set_socket_timeouts(&mut self.socket, POLL_TIMEOUT);
        result?;
        timeout_result?;
        self.input_finished = true;
        Ok(())
    }

    pub fn poll(&mut self) -> Result<Vec<Update>, String> {
        if self.parser.finished {
            return Ok(Vec::new());
        }
        match self.socket.read() {
            Ok(Message::Text(text)) => self.parser.parse(text.as_str()),
            Ok(Message::Ping(_) | Message::Pong(_)) => Ok(Vec::new()),
            Ok(Message::Close(_)) => {
                if self.parser.finished {
                    Ok(Vec::new())
                } else {
                    Err("Soniox 连接提前关闭".into())
                }
            }
            Ok(_) => Err("Soniox 返回了无效消息".into()),
            Err(tungstenite::Error::Io(error)) if is_poll_timeout(&error) => Ok(Vec::new()),
            Err(tungstenite::Error::ConnectionClosed) if self.parser.finished => Ok(Vec::new()),
            Err(_) => Err("Soniox 消息读取失败".into()),
        }
    }

    pub fn finished(&self) -> bool {
        self.parser.finished
    }
}

fn set_socket_timeouts(socket: &mut Socket, timeout: Duration) -> Result<(), String> {
    let stream = match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => stream,
        MaybeTlsStream::Rustls(stream) => &mut stream.sock,
        _ => return Err("Soniox TLS 类型不受支持".into()),
    };
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|_| stream.set_write_timeout(Some(timeout)))
        .map_err(|_| "无法设置 Soniox 网络超时".to_string())
}

fn is_poll_timeout(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

#[derive(Default)]
struct Parser {
    index: u64,
    finalized_tokens: Vec<FinalToken>,
    provisional_text: String,
    provisional_start: Option<u64>,
    provisional_end: Option<u64>,
    progress_sample: u64,
    last_boundary_sample: u64,
    finished: bool,
}

#[derive(Debug)]
struct FinalToken {
    text: String,
    start_sample: Option<u64>,
    end_sample: Option<u64>,
}

#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    tokens: Vec<Token>,
    final_audio_proc_ms: Option<u64>,
    total_audio_proc_ms: Option<u64>,
    #[serde(default)]
    finished: bool,
    error_code: Option<u16>,
    error_type: Option<String>,
    request_id: Option<String>,
}

#[derive(Deserialize)]
struct Token {
    text: String,
    #[serde(default)]
    is_final: bool,
    start_ms: Option<u64>,
    end_ms: Option<u64>,
}

impl Parser {
    fn parse(&mut self, message: &str) -> Result<Vec<Update>, String> {
        if message.len() > MAX_MESSAGE_BYTES {
            return Err("Soniox 响应超出大小限制".into());
        }
        let response: Response =
            serde_json::from_str(message).map_err(|_| "Soniox 响应格式无效".to_string())?;
        if response.error_code.is_some() {
            return Err(service_error(
                response.error_type.as_deref(),
                response.request_id.as_deref(),
            ));
        }
        if self.finished {
            return Ok(Vec::new());
        }

        let final_progress = millis_to_samples(response.final_audio_proc_ms);
        let total_progress = millis_to_samples(response.total_audio_proc_ms);
        let mut updates = Vec::new();
        let mut provisional = String::new();
        let mut provisional_start = None;
        let mut provisional_end = None;
        let mut saw_content = false;
        let mut saw_tokens_in_utterance = false;
        let last_final_content_index = response
            .tokens
            .iter()
            .rposition(|token| token.is_final && token.text != "<end>" && token.text != "<fin>");

        for (position, token) in response.tokens.into_iter().enumerate() {
            if token.text == "<end>" || token.text == "<fin>" {
                if saw_tokens_in_utterance {
                    self.provisional_text = std::mem::take(&mut provisional);
                    self.provisional_start = provisional_start.take();
                    self.provisional_end = provisional_end.take();
                }
                let marker_progress = if last_final_content_index.is_none_or(|last| last < position)
                {
                    final_progress
                } else {
                    None
                };
                self.finish_utterance(marker_progress, &mut updates)?;
                saw_content = false;
                saw_tokens_in_utterance = false;
                continue;
            }
            if token.text.len() > MAX_TEXT_BYTES {
                return Err("Soniox 转写文本超出大小限制".into());
            }
            let start_sample = millis_to_samples(token.start_ms);
            let end_sample = millis_to_samples(token.end_ms);
            if token.is_final {
                self.push_final_token(token.text, start_sample, end_sample)?;
            } else {
                append_bounded(&mut provisional, &token.text)?;
                if let Some(start) = start_sample {
                    provisional_start =
                        Some(provisional_start.map_or(start, |current: u64| current.min(start)));
                }
                if let Some(end) = end_sample {
                    provisional_end =
                        Some(provisional_end.map_or(end, |current: u64| current.max(end)));
                }
            }
            saw_content = true;
            saw_tokens_in_utterance = true;
        }

        if saw_tokens_in_utterance {
            self.provisional_text = provisional;
            self.provisional_start = provisional_start;
            self.provisional_end = provisional_end;
            self.emit_ready_sentences(!self.provisional_text.is_empty(), &mut updates)?;
        }
        if response.finished {
            self.finish_utterance(final_progress.or(total_progress), &mut updates)?;
            self.finished = true;
        } else if saw_content || !self.provisional_text.is_empty() {
            self.observe_progress(total_progress.or(final_progress));
            if let Some(update) = self.current_update(false)? {
                updates.push(update);
            }
        }
        Ok(updates)
    }

    fn push_final_token(
        &mut self,
        text: String,
        start_sample: Option<u64>,
        end_sample: Option<u64>,
    ) -> Result<(), String> {
        let finalized_bytes = self
            .finalized_tokens
            .iter()
            .try_fold(0usize, |total, token| total.checked_add(token.text.len()))
            .and_then(|total| total.checked_add(text.len()))
            .ok_or("Soniox 转写文本超出大小限制")?;
        if finalized_bytes > MAX_TEXT_BYTES {
            return Err("Soniox 转写文本超出大小限制".into());
        }
        self.finalized_tokens.push(FinalToken {
            text,
            start_sample,
            end_sample,
        });
        Ok(())
    }

    fn observe_progress(&mut self, progress: Option<u64>) {
        if let Some(progress) = progress {
            self.progress_sample = self
                .progress_sample
                .max(progress)
                .max(self.last_boundary_sample);
        }
    }

    fn current_update(&self, final_result: bool) -> Result<Option<Update>, String> {
        let finalized_len = self
            .finalized_tokens
            .iter()
            .map(|token| token.text.len())
            .sum::<usize>();
        let text_len = finalized_len
            .checked_add(self.provisional_text.len())
            .ok_or("Soniox 转写文本超出大小限制")?;
        if text_len == 0 {
            return Ok(None);
        }
        if text_len > MAX_TEXT_BYTES {
            return Err("Soniox 转写文本超出大小限制".into());
        }
        let mut text = String::with_capacity(text_len);
        for token in &self.finalized_tokens {
            text.push_str(&token.text);
        }
        text.push_str(&self.provisional_text);
        let start_sample = self
            .finalized_tokens
            .iter()
            .find_map(|token| token.start_sample)
            .or(self.provisional_start)
            .unwrap_or(self.last_boundary_sample)
            .max(self.last_boundary_sample);
        let timed_end = self
            .finalized_tokens
            .iter()
            .filter_map(|token| token.end_sample)
            .chain(self.provisional_end)
            .max()
            .unwrap_or(start_sample);
        Ok(Some(Update {
            index: self.index,
            start_sample,
            end_sample: timed_end.max(self.progress_sample).max(start_sample),
            text,
            final_result,
        }))
    }

    fn emit_ready_sentences(
        &mut self,
        has_following_text: bool,
        updates: &mut Vec<Update>,
    ) -> Result<(), String> {
        loop {
            let Some(token_count) =
                sentence_token_boundary(&self.finalized_tokens, has_following_text)
            else {
                break;
            };
            updates.push(self.take_final_tokens(token_count, None)?);
        }
        Ok(())
    }

    fn take_final_tokens(
        &mut self,
        token_count: usize,
        fallback_end: Option<u64>,
    ) -> Result<Update, String> {
        let tokens: Vec<_> = self.finalized_tokens.drain(..token_count).collect();
        let text_len = tokens.iter().map(|token| token.text.len()).sum();
        let mut text = String::with_capacity(text_len);
        for token in &tokens {
            text.push_str(&token.text);
        }
        let start_sample = tokens
            .iter()
            .find_map(|token| token.start_sample)
            .unwrap_or(self.last_boundary_sample)
            .max(self.last_boundary_sample);
        let next_start = self
            .finalized_tokens
            .iter()
            .find_map(|token| token.start_sample)
            .or(self.provisional_start);
        let timed_end = tokens
            .iter()
            .filter_map(|token| token.end_sample)
            .max()
            .or(next_start)
            .unwrap_or(start_sample);
        let end_sample = fallback_end
            .map_or(timed_end, |fallback| timed_end.max(fallback))
            .max(start_sample);
        let update = Update {
            index: self.index,
            start_sample,
            end_sample,
            text,
            final_result: true,
        };
        self.index = self.index.checked_add(1).ok_or("Soniox 片段编号溢出")?;
        self.last_boundary_sample = end_sample;
        self.progress_sample = self.progress_sample.max(end_sample);
        Ok(update)
    }

    fn finish_utterance(
        &mut self,
        progress: Option<u64>,
        updates: &mut Vec<Update>,
    ) -> Result<(), String> {
        self.observe_progress(progress);
        while !self.finalized_tokens.is_empty() {
            let count = sentence_token_boundary(&self.finalized_tokens, true)
                .unwrap_or(self.finalized_tokens.len());
            updates.push(self.take_final_tokens(count, Some(self.progress_sample))?);
        }
        if !self.provisional_text.is_empty() {
            let start_sample = self
                .provisional_start
                .unwrap_or(self.last_boundary_sample)
                .max(self.last_boundary_sample);
            let end_sample = self
                .provisional_end
                .unwrap_or(self.progress_sample)
                .max(self.progress_sample)
                .max(start_sample);
            updates.push(Update {
                index: self.index,
                start_sample,
                end_sample,
                text: std::mem::take(&mut self.provisional_text),
                final_result: true,
            });
            self.index = self.index.checked_add(1).ok_or("Soniox 片段编号溢出")?;
            self.last_boundary_sample = end_sample;
        }
        self.provisional_text.clear();
        self.provisional_start = None;
        self.provisional_end = None;
        self.progress_sample = self.last_boundary_sample;
        Ok(())
    }
}

fn sentence_token_boundary(tokens: &[FinalToken], allow_last: bool) -> Option<usize> {
    for index in 0..tokens.len() {
        let mut text = String::new();
        for part in &tokens[..=index] {
            text.push_str(&part.text);
        }
        if !ends_natural_sentence(&text) {
            continue;
        }
        let mut boundary = index + 1;
        while boundary < tokens.len()
            && tokens[boundary]
                .text
                .chars()
                .all(|ch| is_sentence_closer(ch) || ch.is_whitespace())
        {
            boundary += 1;
        }
        if boundary < tokens.len() || (allow_last && boundary == tokens.len()) {
            return Some(boundary);
        }
    }
    None
}

fn ends_natural_sentence(text: &str) -> bool {
    text.trim_end()
        .trim_end_matches(is_sentence_closer)
        .chars()
        .next_back()
        .is_some_and(|ch| matches!(ch, '.' | '!' | '?' | '。' | '！' | '？'))
}

fn is_sentence_closer(ch: char) -> bool {
    matches!(
        ch,
        '"' | '\'' | '”' | '’' | '」' | '』' | ')' | ']' | '}' | '）' | '】' | '》'
    )
}

fn millis_to_samples(milliseconds: Option<u64>) -> Option<u64> {
    milliseconds.and_then(|value| value.checked_mul(16))
}

fn append_bounded(target: &mut String, value: &str) -> Result<(), String> {
    if target
        .len()
        .checked_add(value.len())
        .is_none_or(|length| length > MAX_TEXT_BYTES)
    {
        return Err("Soniox 转写文本超出大小限制".into());
    }
    target.push_str(value);
    Ok(())
}

fn service_error(error_type: Option<&str>, request_id: Option<&str>) -> String {
    let error_type = safe_identifier(error_type).unwrap_or("unknown");
    match safe_identifier(request_id) {
        Some(request_id) => format!("Soniox 错误 {error_type}（请求 {request_id}）"),
        None => format!("Soniox 错误 {error_type}"),
    }
}

fn safe_identifier(value: Option<&str>) -> Option<&str> {
    value.filter(|value| {
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(parser: &mut Parser, value: serde_json::Value) -> Vec<Update> {
        parser.parse(&value.to_string()).unwrap()
    }

    #[test]
    fn provisional_text_is_replaced_while_index_stays_stable() {
        let mut parser = Parser::default();
        let first = parse(
            &mut parser,
            json!({"tokens":[{"text":"How're","is_final":false,"start_ms":100,"end_ms":400}],"total_audio_proc_ms":500}),
        );
        let second = parse(
            &mut parser,
            json!({"tokens":[{"text":"How are","is_final":false,"start_ms":100,"end_ms":600}],"total_audio_proc_ms":650}),
        );
        assert_eq!(first[0].index, 0);
        assert_eq!(second[0].index, 0);
        assert_eq!(second[0].text, "How are");
        assert_eq!(second[0].start_sample, 1600);
        assert_eq!(second[0].end_sample, 10400);
    }

    #[test]
    fn final_token_deltas_accumulate_and_replace_provisional_tail() {
        let mut parser = Parser::default();
        parse(
            &mut parser,
            json!({"tokens":[{"text":"Hello ","is_final":true,"start_ms":0,"end_ms":300},{"text":"wor","is_final":false,"start_ms":300,"end_ms":500}],"final_audio_proc_ms":300,"total_audio_proc_ms":500}),
        );
        let update = parse(
            &mut parser,
            json!({"tokens":[{"text":"world","is_final":true,"start_ms":300,"end_ms":650}],"final_audio_proc_ms":650,"total_audio_proc_ms":650}),
        );
        assert_eq!(update[0].text, "Hello world");
        assert!(!update[0].final_result);
    }

    #[test]
    fn endpoint_finalizes_without_exposing_marker_and_advances_index() {
        let mut parser = Parser::default();
        let final_update = parse(
            &mut parser,
            json!({"tokens":[{"text":"First.","is_final":true,"start_ms":0,"end_ms":700},{"text":"<end>","is_final":true}],"final_audio_proc_ms":800,"total_audio_proc_ms":800}),
        );
        assert_eq!(final_update[0].text, "First.");
        assert!(final_update[0].final_result);
        let next = parse(
            &mut parser,
            json!({"tokens":[{"text":"Second","is_final":false,"start_ms":850,"end_ms":1100}],"total_audio_proc_ms":1200}),
        );
        assert_eq!(next[0].index, 1);
        assert_eq!(next[0].start_sample, 13_600);
        assert_eq!(next[0].end_sample, 19_200);
    }

    #[test]
    fn marker_only_response_finalizes_the_previous_provisional_tail() {
        let mut parser = Parser::default();
        parse(
            &mut parser,
            json!({"tokens":[{"text":"Pending","is_final":false,"start_ms":20,"end_ms":300}],"total_audio_proc_ms":300}),
        );
        let updates = parse(
            &mut parser,
            json!({"tokens":[{"text":"<end>","is_final":true}],"final_audio_proc_ms":350,"total_audio_proc_ms":350}),
        );
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].text, "Pending");
        assert!(updates[0].final_result);
        assert_eq!(updates[0].end_sample, 5_600);
    }

    #[test]
    fn tokens_after_a_marker_use_the_next_index() {
        let mut parser = Parser::default();
        let updates = parse(
            &mut parser,
            json!({"tokens":[
                {"text":"First","is_final":true,"start_ms":0,"end_ms":300},
                {"text":"<end>","is_final":true},
                {"text":"Second","is_final":false,"start_ms":400,"end_ms":700}
            ],"final_audio_proc_ms":350,"total_audio_proc_ms":750}),
        );
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].index, 0);
        assert_eq!(updates[0].text, "First");
        assert!(updates[0].final_result);
        assert_eq!(updates[1].index, 1);
        assert_eq!(updates[1].text, "Second");
        assert!(!updates[1].final_result);
    }

    #[test]
    fn multiple_markers_keep_each_utterances_own_range() {
        let mut parser = Parser::default();
        let updates = parse(
            &mut parser,
            json!({"tokens":[
                {"text":"A","is_final":true,"start_ms":0,"end_ms":100},
                {"text":"<end>","is_final":true},
                {"text":"B","is_final":true,"start_ms":200,"end_ms":300},
                {"text":"<end>","is_final":true}
            ],"final_audio_proc_ms":350,"total_audio_proc_ms":350}),
        );
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].index, 0);
        assert_eq!(updates[0].start_sample, 0);
        assert_eq!(updates[0].end_sample, 1_600);
        assert_eq!(updates[1].index, 1);
        assert_eq!(updates[1].start_sample, 3_200);
        assert_eq!(updates[1].end_sample, 5_600);
    }

    #[test]
    fn processed_audio_is_monotonic_fallback_for_missing_timestamps() {
        let mut parser = Parser::default();
        let first = parse(
            &mut parser,
            json!({"tokens":[{"text":"A","is_final":false}],"total_audio_proc_ms":1000}),
        );
        let second = parse(
            &mut parser,
            json!({"tokens":[{"text":"AB","is_final":false}],"total_audio_proc_ms":900}),
        );
        assert_eq!(first[0].end_sample, 16_000);
        assert_eq!(second[0].end_sample, 16_000);
    }

    #[test]
    fn finished_response_flushes_unfinished_utterance() {
        let mut parser = Parser::default();
        parse(
            &mut parser,
            json!({"tokens":[{"text":"Last words","is_final":false}],"total_audio_proc_ms":1200}),
        );
        let updates = parse(
            &mut parser,
            json!({"tokens":[],"final_audio_proc_ms":1200,"total_audio_proc_ms":1200,"finished":true}),
        );
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].text, "Last words");
        assert!(updates[0].final_result);
        assert!(parser.finished);
    }

    #[test]
    fn confirmed_sentences_use_token_timing_and_leave_a_stable_provisional_tail() {
        let mut parser = Parser::default();
        let updates = parse(
            &mut parser,
            json!({
                "tokens":[
                    {"text":"First sentence.","is_final":true,"start_ms":100,"end_ms":600},
                    {"text":" 第二句！","is_final":true,"start_ms":700,"end_ms":1000},
                    {"text":"Third","is_final":false,"start_ms":1100,"end_ms":1400}
                ],
                "final_audio_proc_ms":1000,
                "total_audio_proc_ms":1500
            }),
        );
        assert_eq!(updates.len(), 3);
        assert_eq!(updates[0].index, 0);
        assert_eq!(updates[0].text, "First sentence.");
        assert_eq!(updates[0].start_sample, 1_600);
        assert_eq!(updates[0].end_sample, 9_600);
        assert!(updates[0].final_result);
        assert_eq!(updates[1].index, 1);
        assert_eq!(updates[1].text, " 第二句！");
        assert_eq!(updates[1].start_sample, 11_200);
        assert_eq!(updates[1].end_sample, 16_000);
        assert!(updates[1].final_result);
        assert_eq!(updates[2].index, 2);
        assert_eq!(updates[2].text, "Third");
        assert!(!updates[2].final_result);

        let replacement = parse(
            &mut parser,
            json!({"tokens":[{"text":"Third sentence","is_final":false,"start_ms":1100,"end_ms":1600}],"total_audio_proc_ms":1650}),
        );
        assert_eq!(replacement.len(), 1);
        assert_eq!(replacement[0].index, 2);
        assert_eq!(replacement[0].text, "Third sentence");
    }

    #[test]
    fn closing_quote_stays_with_the_confirmed_sentence() {
        let mut parser = Parser::default();
        let updates = parse(
            &mut parser,
            json!({"tokens":[
                {"text":"He asked?","is_final":true,"start_ms":0,"end_ms":300},
                {"text":"\u{201d}","is_final":true,"start_ms":300,"end_ms":320},
                {"text":" Next","is_final":true,"start_ms":400,"end_ms":600}
            ],"total_audio_proc_ms":650}),
        );
        assert_eq!(updates[0].text, "He asked?\u{201d}");
        assert_eq!(updates[0].end_sample, 5_120);
        assert!(updates[0].final_result);
        assert_eq!(updates[1].index, 1);
        assert_eq!(updates[1].text, " Next");
        assert!(!updates[1].final_result);
    }

    #[test]
    fn service_errors_are_sanitized_and_do_not_include_server_message() {
        let mut parser = Parser::default();
        let error = parser
            .parse(
                &json!({"tokens":[],"error_code":503,"error_type":"service_unavailable","error_message":"secret transcript","request_id":"request-123"}).to_string(),
            )
            .unwrap_err();
        assert_eq!(error, "Soniox 错误 service_unavailable（请求 request-123）");
        assert!(!error.contains("secret"));
    }

    #[test]
    fn session_configuration_includes_custom_terms() {
        let terms = vec!["Amartya Sen".to_owned(), "Cournot".to_owned()];
        let config = session_configuration("test-key", "en", &terms).unwrap();
        assert_eq!(config["model"], "stt-rt-v5");
        assert_eq!(config["language_hints"], json!(["en"]));
        assert_eq!(config["context"]["terms"], json!(terms));
    }
}

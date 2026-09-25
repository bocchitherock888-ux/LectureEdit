//! ElevenLabs Scribe v2 Realtime speech-to-text.
//! https://elevenlabs.io/docs/api-reference/speech-to-text/v-1-speech-to-text-realtime

use crate::cloud::{self, CloudStream, Config, Provider, Socket, Update};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use tungstenite::Message;

const PROVIDER: Provider = Provider::ElevenLabs;
const ENDPOINT: &str = "wss://api.elevenlabs.io/v1/speech-to-text/realtime";
const MAX_KEYTERMS: usize = 50;
const MAX_KEYTERM_CHARS: usize = 20;

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (byte as char).to_string(),
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

pub(crate) fn url(base: &str, config: &Config) -> String {
    let mut query = vec![
        ("model_id", "scribe_v2_realtime".to_owned()),
        ("audio_format", "pcm_16000".to_owned()),
        // The service ends a sentence after a pause; "manual" would leave that to us.
        ("commit_strategy", "vad".to_owned()),
        ("vad_silence_threshold_secs", "1.0".to_owned()),
        ("include_timestamps", "true".to_owned()),
    ];
    match config.language.as_str() {
        "en" => query.push(("language_code", "en".to_owned())),
        // A Chinese lecture still quotes English terms; naming both helps language identification.
        "zh" => {
            query.push(("language_code", "zh".to_owned()));
            query.push(("secondary_languages", "en".to_owned()));
        }
        _ => {
            query.push(("secondary_languages", "zh".to_owned()));
            query.push(("secondary_languages", "en".to_owned()));
        }
    }
    for term in config
        .terms
        .iter()
        .map(|term| term.trim())
        .filter(|term| !term.is_empty() && term.chars().count() <= MAX_KEYTERM_CHARS)
        .take(MAX_KEYTERMS)
    {
        query.push(("keyterms", term.to_owned()));
    }
    let query: Vec<String> = query
        .into_iter()
        .map(|(key, value)| format!("{key}={}", percent_encode(&value)))
        .collect();
    format!("{base}?{}", query.join("&"))
}

fn service_error(kind: &str, message: &str) -> String {
    let detail = if message.is_empty() { String::new() } else { format!("（{message}）") };
    match kind {
        "auth_error" => format!("ElevenLabs 拒绝了这个 API Key：请检查 Key 是否正确，并确认它开启了 Speech to Text 权限{detail}"),
        "quota_exceeded" => format!("ElevenLabs 账户的转写额度已用完{detail}"),
        "unaccepted_terms" => format!("请先登录 ElevenLabs 网站接受语音转写（Scribe）服务条款{detail}"),
        "invalid_request" | "input_error" | "chunk_size_exceeded" => format!("ElevenLabs 认为请求无效（{kind}）{detail}"),
        _ => cloud::transient(PROVIDER, &format!("{kind}{detail}")),
    }
}

/// Seconds from the start of the connection to 16 kHz samples.
fn seconds_to_samples(seconds: f64) -> u64 {
    (seconds.max(0.0) * 16000.0).round() as u64
}

#[derive(Default)]
pub(crate) struct Parser {
    next_index: u64,
    /// The sentence being spoken: its slot, and where it began in the stream.
    open: Option<(u64, u64)>,
    /// Text committed by the service, held until its word timings arrive.
    committed: Option<String>,
    /// Where the last finished sentence ended.
    committed_end: u64,
}

impl Parser {
    /// `sent` is how many samples have been sent, the best estimate of "now" for partial text.
    pub(crate) fn message(&mut self, message: &Value, sent: u64) -> Result<Vec<Update>, String> {
        let text = message["text"].as_str().unwrap_or("").trim().to_owned();
        let mut updates = Vec::new();
        match message["message_type"].as_str().unwrap_or("") {
            "partial_transcript" => {
                updates.extend(self.flush_committed(sent));
                if !text.is_empty() {
                    let (index, start) = self.open(sent);
                    updates.push(Update { index, start_sample: start, end_sample: sent.max(start), text, final_result: false });
                }
            }
            "committed_transcript" => {
                updates.extend(self.flush_committed(sent));
                if text.is_empty() {
                    self.open = None;
                } else {
                    self.committed = Some(text);
                }
            }
            "committed_transcript_with_timestamps" => {
                self.committed = None;
                if text.is_empty() {
                    self.open = None;
                } else {
                    let words: Vec<&Value> = message["words"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|word| word["type"] != "spacing" && word["start"].is_number())
                        .collect();
                    let (index, open_start) = self.open(sent);
                    let start = words.first().and_then(|w| w["start"].as_f64()).map(seconds_to_samples).unwrap_or(open_start);
                    let end = words.last().and_then(|w| w["end"].as_f64()).map(seconds_to_samples).unwrap_or(sent).max(start);
                    self.open = None;
                    self.committed_end = end;
                    updates.push(Update { index, start_sample: start, end_sample: end, text, final_result: true });
                }
            }
            "session_started" | "warning" | "committed_transcript_entities" | "edited_transcript" => {}
            kind if message.get("error").is_some() => {
                return Err(service_error(kind, message["error"].as_str().unwrap_or("")));
            }
            _ => {}
        }
        Ok(updates)
    }

    fn open(&mut self, sent: u64) -> (u64, u64) {
        if let Some(open) = self.open {
            return open;
        }
        let open = (self.next_index, self.committed_end.min(sent));
        self.next_index += 1;
        self.open = Some(open);
        open
    }

    /// Finishes committed text whose timings never came, so it is not lost.
    fn flush_committed(&mut self, sent: u64) -> Option<Update> {
        let text = self.committed.take()?;
        let (index, start) = self.open(sent);
        self.open = None;
        self.committed_end = sent.max(start);
        Some(Update { index, start_sample: start, end_sample: self.committed_end, text, final_result: true })
    }

    fn idle(&self) -> bool {
        self.open.is_none() && self.committed.is_none()
    }
}

pub struct Client {
    socket: Socket,
    sent: u64,
    input_finished: bool,
    finished: bool,
    parser: Parser,
}

impl Client {
    pub fn connect(config: &Config) -> Result<Self, String> {
        Self::connect_to(ENDPOINT, config)
    }

    pub(crate) fn connect_to(base: &str, config: &Config) -> Result<Self, String> {
        let headers = [("xi-api-key", config.key.clone())];
        let mut socket = cloud::open(PROVIDER, &url(base, config), &headers)?;
        cloud::set_timeouts(&mut socket, cloud::POLL_TIMEOUT, PROVIDER)?;
        Ok(Self { socket, sent: 0, input_finished: false, finished: false, parser: Parser::default() })
    }

    fn send_chunk(&mut self, bytes: &[u8], commit: bool) -> Result<(), String> {
        let chunk = json!({
            "message_type": "input_audio_chunk",
            "audio_base_64": STANDARD.encode(bytes),
            "commit": commit,
            "sample_rate": 16000
        });
        cloud::set_timeouts(&mut self.socket, cloud::IO_TIMEOUT, PROVIDER)?;
        let result = self
            .socket
            .send(Message::Text(chunk.to_string().into()))
            .map_err(|_| cloud::transient(PROVIDER, "音频发送失败"));
        cloud::set_timeouts(&mut self.socket, cloud::POLL_TIMEOUT, PROVIDER)?;
        result
    }
}

impl CloudStream for Client {
    fn send_audio(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.input_finished || self.finished {
            return Err("ElevenLabs 音频输入已经结束".into());
        }
        if bytes.is_empty() || !bytes.len().is_multiple_of(2) {
            return Err("ElevenLabs PCM 音频块格式无效".into());
        }
        self.send_chunk(bytes, false)?;
        self.sent += (bytes.len() / 2) as u64;
        Ok(())
    }

    fn finish_input(&mut self) -> Result<(), String> {
        if self.input_finished || self.finished {
            self.input_finished = true;
            return Ok(());
        }
        // An empty chunk with commit=true closes the sentence in progress.
        self.send_chunk(&[], true)?;
        self.input_finished = true;
        if self.parser.idle() {
            self.finished = true;
        }
        Ok(())
    }

    fn poll(&mut self) -> Result<Vec<Update>, String> {
        if self.finished {
            return Ok(Vec::new());
        }
        match self.socket.read() {
            Ok(Message::Text(text)) => {
                let message: Value = serde_json::from_str(text.as_str())
                    .map_err(|_| cloud::transient(PROVIDER, "返回了无效消息"))?;
                let updates = self.parser.message(&message, self.sent)?;
                if self.input_finished && self.parser.idle() {
                    self.finished = true;
                }
                Ok(updates)
            }
            Ok(Message::Close(_)) | Err(tungstenite::Error::ConnectionClosed) => {
                if self.input_finished {
                    // Keep text the service committed without timings.
                    self.finished = true;
                    Ok(self.parser.flush_committed(self.sent).into_iter().collect())
                } else {
                    Err(cloud::transient(PROVIDER, "连接被服务器关闭"))
                }
            }
            Ok(_) => Ok(Vec::new()),
            Err(tungstenite::Error::Io(error)) if cloud::is_poll_timeout(&error) => Ok(Vec::new()),
            Err(_) => Err(cloud::transient(PROVIDER, "结果读取失败")),
        }
    }

    fn finished(&self) -> bool {
        self.finished
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_names_model_format_languages_and_encoded_keyterms() {
        let config = Config { key: "k".into(), language: "auto".into(), terms: vec!["Tocqueville".into(), "选择 偏差".into(), "a term that is far too long to be accepted".into()], region: String::new() };
        let url = url(ENDPOINT, &config);
        assert!(url.starts_with("wss://api.elevenlabs.io/v1/speech-to-text/realtime?model_id=scribe_v2_realtime&audio_format=pcm_16000&commit_strategy=vad"));
        assert!(url.contains("&include_timestamps=true"));
        assert!(url.contains("&secondary_languages=zh&secondary_languages=en"));
        assert!(!url.contains("language_code"));
        assert!(url.contains("&keyterms=Tocqueville&keyterms=%E9%80%89%E6%8B%A9%20%E5%81%8F%E5%B7%AE"));
        assert!(!url.contains("far"));
        let english = super::url(ENDPOINT, &Config { language: "en".into(), ..config.clone() });
        assert!(english.contains("language_code=en") && !english.contains("secondary_languages"));
        let many = Config { terms: (0..80).map(|i| format!("t{i}")).collect(), ..config };
        assert_eq!(super::url(ENDPOINT, &many).matches("keyterms=").count(), MAX_KEYTERMS);
    }

    #[test]
    fn partials_share_a_slot_until_the_timed_commit_finishes_it() {
        let mut parser = Parser::default();
        let partial = parser.message(&json!({"message_type":"partial_transcript","text":"Today we"}), 16000).unwrap();
        assert_eq!(partial, vec![Update { index: 0, start_sample: 0, end_sample: 16000, text: "Today we".into(), final_result: false }]);
        assert!(parser.message(&json!({"message_type":"committed_transcript","text":"Today we talk about bias."}), 40000).unwrap().is_empty());
        let done = parser.message(&json!({"message_type":"committed_transcript_with_timestamps","text":"Today we talk about bias.","words":[
            {"text":"Today","start":0.5,"end":0.8,"type":"word"},{"text":" ","start":0.8,"end":0.8,"type":"spacing"},{"text":"bias.","start":2.0,"end":2.4,"type":"word"}]}), 40000).unwrap();
        assert_eq!(done, vec![Update { index: 0, start_sample: 8000, end_sample: 38400, text: "Today we talk about bias.".into(), final_result: true }]);
        let next = parser.message(&json!({"message_type":"partial_transcript","text":"Next"}), 48000).unwrap();
        assert_eq!((next[0].index, next[0].start_sample), (1, 38400));
        // Committed text whose timings never arrive is still kept.
        parser.message(&json!({"message_type":"committed_transcript","text":"Next week."}), 56000).unwrap();
        let flushed = parser.message(&json!({"message_type":"partial_transcript","text":"Then"}), 60000).unwrap();
        assert_eq!(flushed[0], Update { index: 1, start_sample: 38400, end_sample: 60000, text: "Next week.".into(), final_result: true });
        assert_eq!(flushed[1].index, 2);
    }

    #[test]
    fn account_problems_stop_and_capacity_problems_retry() {
        let mut parser = Parser::default();
        let error = parser.message(&json!({"message_type":"auth_error","error":"bad key"}), 0).unwrap_err();
        assert!(error.contains("Speech to Text") && !cloud::is_transient(&error));
        assert!(!cloud::is_transient(&service_error("quota_exceeded", "")));
        assert!(service_error("unaccepted_terms", "").contains("条款"));
        assert!(cloud::is_transient(&service_error("session_time_limit_exceeded", "")));
        assert!(cloud::is_transient(&service_error("rate_limited", "")));
    }

    #[test]
    fn a_whole_session_against_a_local_server_speaking_the_protocol() {
        let (url, handshake, server) = cloud::mock::serve(|socket| {
            socket.send(Message::Text(json!({"message_type":"session_started","session_id":"s"}).to_string().into())).unwrap();
            let mut chunks = 0;
            loop {
                let chunk: Value = match socket.read().unwrap() { Message::Text(text) => serde_json::from_str(text.as_str()).unwrap(), _ => continue };
                assert_eq!(chunk["message_type"], "input_audio_chunk");
                assert_eq!(chunk["sample_rate"], 16000);
                if chunk["commit"] == true {
                    assert_eq!(chunk["audio_base_64"], "");
                    for message in [
                        json!({"message_type":"committed_transcript","text":"We start now."}),
                        json!({"message_type":"committed_transcript_with_timestamps","text":"We start now.","words":[{"text":"We","start":0.1,"end":0.2,"type":"word"},{"text":"now.","start":0.5,"end":0.55,"type":"word"}]}),
                    ] {
                        socket.send(Message::Text(message.to_string().into())).unwrap();
                    }
                    break;
                }
                assert_eq!(STANDARD.decode(chunk["audio_base_64"].as_str().unwrap()).unwrap().len(), 6400);
                chunks += 1;
                socket.send(Message::Text(json!({"message_type":"partial_transcript","text":if chunks < 2 {"We"} else {"We start"}}).to_string().into())).unwrap();
            }
        });
        let config = Config { key: "xi-key".into(), language: "en".into(), terms: vec!["Cournot".into()], region: String::new() };
        let mut client = Client::connect_to(&url, &config).unwrap();
        let updates = cloud::mock::run(&mut client, 3);
        server.join().unwrap();
        let handshake = handshake.recv().unwrap();
        assert_eq!(handshake.headers["xi-api-key"], "xi-key");
        assert!(handshake.path_and_query.contains("model_id=scribe_v2_realtime") && handshake.path_and_query.contains("keyterms=Cournot"));
        assert!(updates.iter().any(|u| !u.final_result && u.text == "We start"));
        assert_eq!(updates.last().unwrap(), &Update { index: 0, start_sample: 1600, end_sample: 8800, text: "We start now.".into(), final_result: true });
    }

}

//! 火山引擎豆包语音: 豆包流式语音识别模型 2.0 (bigmodel_async, with the second, non-streaming
//! pass that rewrites each finished sentence more accurately). Protocol:
//! https://www.volcengine.com/docs/6561/1354869 — a binary frame protocol, big-endian throughout.

use crate::cloud::{self, CloudStream, Config, Provider, Socket, Update};
use flate2::read::GzDecoder;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::Read;
use tungstenite::Message;

const ENDPOINT: &str = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async";
/// 豆包流式语音识别模型 2.0, billed by the hour.
const RESOURCE_ID: &str = "volc.seedasr.sauc.duration";
const PROVIDER: Provider = Provider::Doubao;

const FULL_CLIENT_REQUEST: u8 = 0b0001;
const AUDIO_ONLY_REQUEST: u8 = 0b0010;
const FULL_SERVER_RESPONSE: u8 = 0b1001;
const SERVER_ERROR: u8 = 0b1111;
const FLAG_SEQUENCE: u8 = 0b0001;
const FLAG_LAST: u8 = 0b0010;
/// Not in the protocol table; the official demo skips a 4-byte event field when this is set.
const FLAG_EVENT: u8 = 0b0100;
const SERIAL_NONE: u8 = 0b0000;
const SERIAL_JSON: u8 = 0b0001;
const COMPRESSION_GZIP: u8 = 0b0001;
/// Hotwords passed inline on the streaming endpoints are limited to about 100 tokens.
const HOTWORD_TOKEN_BUDGET: usize = 100;
/// 100 ms of silence closes the stream: the last packet must carry audio.
const CLOSING_SILENCE: [u8; 3200] = [0; 3200];

pub(crate) fn frame(
    message_type: u8,
    flags: u8,
    serialization: u8,
    sequence: Option<i32>,
    payload: &[u8],
) -> Vec<u8> {
    let mut out = vec![0x11, (message_type << 4) | flags, serialization << 4, 0];
    if let Some(sequence) = sequence {
        out.extend(sequence.to_be_bytes());
    }
    out.extend((payload.len() as u32).to_be_bytes());
    out.extend(payload);
    out
}

#[derive(Debug, PartialEq)]
pub(crate) enum ServerFrame {
    Response { last: bool, payload: Value },
    Error { code: u32, message: String },
}

fn read_u32(bytes: &[u8], at: usize) -> Result<u32, String> {
    bytes
        .get(at..at + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| "豆包语音返回的数据不完整".to_string())
}

fn body(bytes: &[u8], compression: u8) -> Result<Vec<u8>, String> {
    if compression == COMPRESSION_GZIP {
        let mut out = Vec::new();
        GzDecoder::new(bytes)
            .take(4 * 1024 * 1024)
            .read_to_end(&mut out)
            .map_err(|_| "豆包语音返回的数据无法解压".to_string())?;
        Ok(out)
    } else {
        Ok(bytes.to_vec())
    }
}

pub(crate) fn parse_frame(bytes: &[u8]) -> Result<ServerFrame, String> {
    let header = (*bytes.first().ok_or("豆包语音返回了空消息")? & 0x0f) as usize * 4;
    if header < 4 || bytes.len() < header {
        return Err("豆包语音返回的消息头无效".into());
    }
    let message_type = bytes[1] >> 4;
    let flags = bytes[1] & 0x0f;
    let serialization = bytes[2] >> 4;
    let compression = bytes[2] & 0x0f;
    let mut at = header;
    match message_type {
        FULL_SERVER_RESPONSE => {
            if flags & FLAG_SEQUENCE != 0 {
                at += 4;
            }
            if flags & FLAG_EVENT != 0 {
                at += 4;
            }
            let size = read_u32(bytes, at)? as usize;
            let raw = bytes
                .get(at + 4..at + 4 + size)
                .ok_or("豆包语音返回的数据不完整")?;
            let payload = if raw.is_empty() {
                Value::Null
            } else {
                let text = body(raw, compression)?;
                if serialization == SERIAL_JSON || text.first() == Some(&b'{') {
                    serde_json::from_slice(&text).map_err(|_| "豆包语音返回的结果格式无效")?
                } else {
                    Value::Null
                }
            };
            Ok(ServerFrame::Response {
                last: flags & FLAG_LAST != 0,
                payload,
            })
        }
        SERVER_ERROR => {
            let code = read_u32(bytes, at)?;
            let size = read_u32(bytes, at + 4)? as usize;
            let raw = bytes.get(at + 8..at + 8 + size).unwrap_or(&[]);
            let message = String::from_utf8_lossy(&body(raw, compression).unwrap_or_default())
                .chars()
                .take(300)
                .collect();
            Ok(ServerFrame::Error { code, message })
        }
        _ => Err("豆包语音返回了未知类型的消息".into()),
    }
}

/// Inline hotwords, most important (earliest) first, within the streaming endpoint's budget.
/// A CJK character counts as one token and an English word as two, which errs on the safe side.
fn hotwords(terms: &[String]) -> Option<String> {
    let mut used = 0;
    let mut words = Vec::new();
    for term in terms {
        let cost = term.chars().filter(|c| !c.is_ascii()).count()
            + term.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).count() * 2;
        if cost == 0 || used + cost > HOTWORD_TOKEN_BUDGET {
            continue;
        }
        used += cost;
        words.push(json!({"word": term}));
    }
    (!words.is_empty()).then(|| json!({"hotwords": words}).to_string())
}

pub(crate) fn request(config: &Config) -> Value {
    let mut request = json!({
        "user": {"uid": "suitang"},
        "audio": {"format": "pcm", "codec": "raw", "rate": 16000, "bits": 16, "channel": 1},
        "request": {
            "model_name": "bigmodel",
            "enable_itn": true,
            "enable_punc": true,
            "show_utterances": true,
            "result_type": "single",
            // Second pass: each sentence is re-recognised by the non-streaming model once it ends.
            "enable_nonstream": true,
            "end_window_size": 800
        }
    });
    // Chinese and English (and dialects) are recognised together by default; this endpoint
    // takes no language parameter.
    if let Some(context) = hotwords(&config.terms) {
        request["request"]["corpus"] = json!({"context": context});
    }
    request
}

struct Open {
    index: u64,
    start: u64,
    end: u64,
    text: String,
}

#[derive(Default)]
pub(crate) struct Parser {
    next_index: u64,
    open: Option<Open>,
    finalized: HashSet<(u64, u64, String)>,
}

impl Parser {
    /// Maps a response to updates. A sentence shows as a partial until the second pass marks it
    /// `definite`; the definite text then replaces that partial in the same slot. Utterances
    /// already finalized are ignored if the service sends them again.
    pub(crate) fn responses(&mut self, payload: &Value, last: bool) -> Vec<Update> {
        let mut updates = Vec::new();
        for utterance in payload["result"]["utterances"].as_array().into_iter().flatten() {
            let text = utterance["text"].as_str().unwrap_or("").trim();
            if text.is_empty() {
                continue;
            }
            let start = utterance["start_time"].as_u64().unwrap_or(0);
            let end = utterance["end_time"].as_u64().unwrap_or(start).max(start);
            let key = (start, end, text.to_owned());
            if self.finalized.contains(&key) {
                continue;
            }
            if utterance["definite"] == true {
                let index = match self.open.take() {
                    Some(open) => open.index,
                    None => self.take_index(),
                };
                self.finalized.insert(key);
                updates.push(update(index, start, end, text, true));
            } else {
                let index = match &self.open {
                    Some(open) => open.index,
                    None => self.take_index(),
                };
                self.open = Some(Open { index, start, end, text: text.to_owned() });
                updates.push(update(index, start, end, text, false));
            }
        }
        if last {
            if let Some(open) = self.open.take() {
                updates.push(update(open.index, open.start, open.end, &open.text, true));
            }
        }
        updates
    }

    fn take_index(&mut self) -> u64 {
        let index = self.next_index;
        self.next_index += 1;
        index
    }
}

fn update(index: u64, start_ms: u64, end_ms: u64, text: &str, final_result: bool) -> Update {
    Update {
        index,
        start_sample: cloud::millis_to_samples(start_ms),
        end_sample: cloud::millis_to_samples(end_ms),
        text: text.to_owned(),
        final_result,
    }
}

fn service_error(code: u32, message: &str) -> String {
    let detail = if message.is_empty() { String::new() } else { format!("（{message}）") };
    match code {
        45000081 => cloud::transient(PROVIDER, &format!("等待音频超时{detail}")),
        55000000..=55999999 => cloud::transient(PROVIDER, &format!("服务繁忙或内部错误 {code}{detail}")),
        45000001 => format!("豆包语音认为请求参数无效（45000001）{detail}"),
        45000151 => format!("豆包语音无法识别音频格式（45000151）{detail}"),
        _ => format!("豆包语音返回错误 {code}{detail}"),
    }
}

pub struct Client {
    socket: Socket,
    sequence: i32,
    input_finished: bool,
    finished: bool,
    parser: Parser,
}

impl Client {
    pub fn connect(config: &Config) -> Result<Self, String> {
        Self::connect_to(ENDPOINT, config)
    }

    pub(crate) fn connect_to(url: &str, config: &Config) -> Result<Self, String> {
        let headers = [
            ("X-Api-Key", config.key.clone()),
            ("X-Api-Resource-Id", RESOURCE_ID.to_owned()),
            ("X-Api-Request-Id", uuid::Uuid::new_v4().to_string()),
            ("X-Api-Connect-Id", uuid::Uuid::new_v4().to_string()),
            ("X-Api-Sequence", "-1".to_owned()),
        ];
        let mut socket = cloud::open(PROVIDER, url, &headers)?;
        let payload = request(config).to_string();
        socket
            .send(Message::Binary(
                frame(FULL_CLIENT_REQUEST, FLAG_SEQUENCE, SERIAL_JSON, Some(1), payload.as_bytes()).into(),
            ))
            .map_err(|_| cloud::transient(PROVIDER, "识别参数发送失败"))?;
        cloud::set_timeouts(&mut socket, cloud::POLL_TIMEOUT, PROVIDER)?;
        Ok(Self {
            socket,
            sequence: 1,
            input_finished: false,
            finished: false,
            parser: Parser::default(),
        })
    }

    fn send_frame(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        cloud::set_timeouts(&mut self.socket, cloud::IO_TIMEOUT, PROVIDER)?;
        let result = self
            .socket
            .send(Message::Binary(bytes.into()))
            .map_err(|_| cloud::transient(PROVIDER, "音频发送失败"));
        cloud::set_timeouts(&mut self.socket, cloud::POLL_TIMEOUT, PROVIDER)?;
        result
    }
}

impl CloudStream for Client {
    fn send_audio(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.input_finished || self.finished {
            return Err("豆包语音音频输入已经结束".into());
        }
        if bytes.is_empty() || !bytes.len().is_multiple_of(2) {
            return Err("豆包语音 PCM 音频块格式无效".into());
        }
        self.sequence += 1;
        let packet = frame(AUDIO_ONLY_REQUEST, FLAG_SEQUENCE, SERIAL_NONE, Some(self.sequence), bytes);
        self.send_frame(packet)
    }

    fn finish_input(&mut self) -> Result<(), String> {
        if self.input_finished || self.finished {
            self.input_finished = true;
            return Ok(());
        }
        self.sequence += 1;
        let packet = frame(
            AUDIO_ONLY_REQUEST,
            FLAG_SEQUENCE | FLAG_LAST,
            SERIAL_NONE,
            Some(-self.sequence),
            &CLOSING_SILENCE,
        );
        self.send_frame(packet)?;
        self.input_finished = true;
        Ok(())
    }

    fn poll(&mut self) -> Result<Vec<Update>, String> {
        if self.finished {
            return Ok(Vec::new());
        }
        match self.socket.read() {
            Ok(Message::Binary(bytes)) => match parse_frame(&bytes)? {
                ServerFrame::Response { last, payload } => {
                    let updates = self.parser.responses(&payload, last);
                    if last {
                        self.finished = true;
                    }
                    Ok(updates)
                }
                ServerFrame::Error { code, message } => Err(service_error(code, &message)),
            },
            Ok(Message::Text(_) | Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => Ok(Vec::new()),
            Ok(Message::Close(_)) | Err(tungstenite::Error::ConnectionClosed) => {
                if self.finished {
                    Ok(Vec::new())
                } else {
                    Err(cloud::transient(PROVIDER, "连接被服务器关闭"))
                }
            }
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
    use flate2::{write::GzEncoder, Compression};
    use std::io::Write;

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn server_response(sequence: i32, last: bool, payload: &Value, gzipped: bool) -> Vec<u8> {
        let json = payload.to_string();
        let body = if gzipped { gzip(json.as_bytes()) } else { json.into_bytes() };
        let flags = FLAG_SEQUENCE | if last { FLAG_LAST } else { 0 };
        let mut out = vec![0x11, (FULL_SERVER_RESPONSE << 4) | flags, (SERIAL_JSON << 4) | u8::from(gzipped), 0];
        out.extend(sequence.to_be_bytes());
        out.extend((body.len() as u32).to_be_bytes());
        out.extend(body);
        out
    }

    #[test]
    fn frames_follow_the_documented_byte_layout() {
        // Full client request, JSON, no compression, sequence 1.
        assert_eq!(
            frame(FULL_CLIENT_REQUEST, FLAG_SEQUENCE, SERIAL_JSON, Some(1), b"{}"),
            [0x11, 0x11, 0x10, 0x00, 0, 0, 0, 1, 0, 0, 0, 2, b'{', b'}']
        );
        // Last audio packet: flags 0b0011 and a negative sequence number.
        let last = frame(AUDIO_ONLY_REQUEST, FLAG_SEQUENCE | FLAG_LAST, SERIAL_NONE, Some(-7), &[1, 2]);
        assert_eq!(&last[..4], [0x11, 0x23, 0x00, 0x00]);
        assert_eq!(i32::from_be_bytes([last[4], last[5], last[6], last[7]]), -7);
        assert_eq!(u32::from_be_bytes([last[8], last[9], last[10], last[11]]), 2);
    }

    #[test]
    fn responses_parse_plain_and_gzipped_and_errors_carry_their_code() {
        let payload = json!({"result":{"text":"你好"}});
        for gzipped in [false, true] {
            assert_eq!(
                parse_frame(&server_response(3, true, &payload, gzipped)).unwrap(),
                ServerFrame::Response { last: true, payload: payload.clone() }
            );
        }
        let mut error = vec![0x11, (SERVER_ERROR << 4), SERIAL_JSON << 4, 0];
        error.extend(45000081u32.to_be_bytes());
        error.extend(4u32.to_be_bytes());
        error.extend(b"idle");
        let parsed = parse_frame(&error).unwrap();
        assert_eq!(parsed, ServerFrame::Error { code: 45000081, message: "idle".into() });
        assert!(cloud::is_transient(&service_error(45000081, "idle")));
        assert!(!cloud::is_transient(&service_error(45000001, "bad")));
        assert!(parse_frame(&[0x11, 0x91]).is_err());
    }

    #[test]
    fn a_partial_is_replaced_by_its_definite_sentence_in_the_same_slot() {
        let mut parser = Parser::default();
        let partial = parser.responses(&json!({"result":{"utterances":[{"text":"今天我们讲","start_time":100,"end_time":900,"definite":false}]}}), false);
        assert_eq!(partial, vec![update(0, 100, 900, "今天我们讲", false)]);
        let done = parser.responses(&json!({"result":{"utterances":[
            {"text":"今天我们讲选择偏差。","start_time":100,"end_time":1800,"definite":true},
            {"text":"首先","start_time":2100,"end_time":2400,"definite":false}]}}), false);
        assert_eq!(done, vec![update(0, 100, 1800, "今天我们讲选择偏差。", true), update(1, 2100, 2400, "首先", false)]);
        // A repeated definite sentence (result_type "full") is not written twice.
        let again = parser.responses(&json!({"result":{"utterances":[{"text":"今天我们讲选择偏差。","start_time":100,"end_time":1800,"definite":true}]}}), false);
        assert!(again.is_empty());
        // The last response closes a sentence the second pass never marked.
        let last = parser.responses(&json!({"result":{"utterances":[]}}), true);
        assert_eq!(last, vec![update(1, 2100, 2400, "首先", true)]);
        assert_eq!(update(0, 100, 900, "x", true).start_sample, 1600);
    }

    #[test]
    fn hotwords_are_inline_json_within_the_streaming_budget() {
        let config = Config { key: "k".into(), language: "auto".into(), terms: vec!["Tocqueville".into(), "选择偏差".into()], region: String::new() };
        let context = request(&config)["request"]["corpus"]["context"].as_str().unwrap().to_owned();
        let parsed: Value = serde_json::from_str(&context).unwrap();
        assert_eq!(parsed, json!({"hotwords":[{"word":"Tocqueville"},{"word":"选择偏差"}]}));
        let many: Vec<String> = (0..200).map(|i| format!("术语{i}")).collect();
        let parsed: Value = serde_json::from_str(&hotwords(&many).unwrap()).unwrap();
        let cost: usize = parsed["hotwords"].as_array().unwrap().iter().map(|w| {
            let w = w["word"].as_str().unwrap();
            w.chars().filter(|c| !c.is_ascii()).count() + w.split(|c: char| !c.is_ascii_alphanumeric()).filter(|p| !p.is_empty()).count() * 2
        }).sum();
        assert!(cost <= HOTWORD_TOKEN_BUDGET);
        assert!(request(&Config::default())["request"].get("corpus").is_none());
    }

    #[test]
    fn a_whole_session_against_a_local_server_speaking_the_protocol() {
        let (url, handshake, server) = cloud::mock::serve(|socket| {
            let request = match socket.read().unwrap() { Message::Binary(bytes) => bytes, other => panic!("{other:?}") };
            assert_eq!(&request[..4], [0x11, 0x11, 0x10, 0x00]);
            let json: Value = serde_json::from_slice(&request[12..]).unwrap();
            assert_eq!(json["request"]["model_name"], "bigmodel");
            assert_eq!(json["request"]["enable_nonstream"], true);
            let mut sequence = 1;
            loop {
                let packet = match socket.read().unwrap() { Message::Binary(bytes) => bytes, _ => continue };
                sequence += 1;
                assert_eq!(packet[1] >> 4, AUDIO_ONLY_REQUEST);
                let seq = i32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]);
                if packet[1] & FLAG_LAST != 0 {
                    assert_eq!(seq, -sequence);
                    let done = json!({"result":{"utterances":[{"text":"今天讲选择偏差。","start_time":0,"end_time":1200,"definite":true}]}});
                    socket.send(Message::Binary(server_response(-seq, true, &done, true).into())).unwrap();
                    break;
                }
                assert_eq!(seq, sequence);
                let partial = json!({"result":{"utterances":[{"text":"今天讲","start_time":0,"end_time":400,"definite":false}]}});
                socket.send(Message::Binary(server_response(seq, false, &partial, false).into())).unwrap();
            }
        });
        let config = Config { key: "volc-key".into(), language: "auto".into(), terms: vec![], region: String::new() };
        let mut client = Client::connect_to(&url, &config).unwrap();
        let updates = cloud::mock::run(&mut client, 3);
        server.join().unwrap();
        let handshake = handshake.recv().unwrap();
        assert_eq!(handshake.headers["x-api-key"], "volc-key");
        assert_eq!(handshake.headers["x-api-resource-id"], RESOURCE_ID);
        assert!(updates.iter().any(|u| !u.final_result && u.text == "今天讲"));
        assert_eq!(updates.last().unwrap(), &update(0, 0, 1200, "今天讲选择偏差。", true));
    }

}

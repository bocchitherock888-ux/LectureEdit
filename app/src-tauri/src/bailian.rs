//! 阿里云百炼 real-time speech recognition over the DashScope inference WebSocket:
//! run-task, binary PCM frames, finish-task. Model qwen-audio-3.1-asr-flash-streaming, which
//! takes Chinese and English hints together and inline hotwords, falling back to
//! fun-asr-realtime (same protocol) when an account cannot use it.
//! https://help.aliyun.com/zh/model-studio/fun-asr-realtime-websocket-api

use crate::cloud::{self, CloudStream, Config, Provider, Socket, Update};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tungstenite::Message;

const PROVIDER: Provider = Provider::Bailian;
const MODEL: &str = "qwen-audio-3.1-asr-flash-streaming";
const FALLBACK_MODEL: &str = "fun-asr-realtime";
/// The API-key-only (DashScope) domains time requests out after 600 s. A connection is replaced
/// shortly before that, between sentences; the runtime resumes on a new one.
const ROLLOVER_AFTER: Duration = Duration::from_secs(540);
const START_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) fn endpoint(region: &str) -> &'static str {
    match region {
        "singapore" => "wss://dashscope-intl.aliyuncs.com/api-ws/v1/inference",
        _ => "wss://dashscope.aliyuncs.com/api-ws/v1/inference",
    }
}

/// Inline hotwords as the model accepts them: at most 15 characters when not pure ASCII, at most
/// seven words when ASCII, and 2000 in all. Weight 4 is the documented recommendation.
fn vocabulary(terms: &[String]) -> serde_json::Map<String, Value> {
    terms
        .iter()
        .map(|term| term.trim())
        .filter(|term| {
            !term.is_empty()
                && if term.is_ascii() {
                    term.split_whitespace().count() <= 7
                } else {
                    term.chars().count() <= 15
                }
        })
        .take(2000)
        .map(|term| (term.to_owned(), json!(4)))
        .collect()
}

pub(crate) fn run_task(task_id: &str, model: &str, config: &Config) -> Value {
    let hints = match config.language.as_str() {
        "en" => json!(["en"]),
        // Lectures in Chinese mix in English terms; both hints keep them in the original script.
        _ => json!(["zh", "en"]),
    };
    let mut parameters = json!({
        "format": "pcm",
        "sample_rate": 16000,
        "semantic_punctuation_enabled": true,
        // Keeps the task open through long pauses in a lecture.
        "heartbeat": true
    });
    if model == MODEL {
        parameters["language_hints"] = hints;
        let words = vocabulary(&config.terms);
        if !words.is_empty() {
            parameters["vocabulary"] = Value::Object(words);
        }
    } else {
        // fun-asr uses only the first hint and takes hotwords only as a pre-created vocabulary.
        parameters["language_hints"] = json!([if config.language == "en" { "en" } else { "zh" }]);
    }
    json!({
        "header": {"action": "run-task", "task_id": task_id, "streaming": "duplex"},
        "payload": {
            "task_group": "audio",
            "task": "asr",
            "function": "recognition",
            "model": model,
            "parameters": parameters,
            "input": {}
        }
    })
}

fn failure(code: &str, message: &str) -> String {
    let detail = if message.is_empty() { String::new() } else { format!("（{message}）") };
    let lower = format!("{code} {message}").to_lowercase();
    if lower.contains("invalidapikey") || lower.contains("invalid api-key") {
        format!("阿里云百炼拒绝了这个 API Key：请检查 Key 是否正确，以及所选地域是否与 Key 的地域一致{detail}")
    } else if lower.contains("arrearage") {
        format!("阿里云百炼账户已欠费或额度不足，请充值后重试{detail}")
    } else if lower.contains("accessdenied") || lower.contains("modelnotfound") {
        format!("阿里云百炼账户暂时无法使用实时语音识别模型，请在控制台确认已开通{detail}")
    } else {
        cloud::transient(PROVIDER, &format!("{code}{detail}"))
    }
}

fn model_unavailable(code: &str, message: &str) -> bool {
    let lower = format!("{code} {message}").to_lowercase();
    lower.contains("modelnotfound") || lower.contains("model.accessdenied") || lower.contains("model not")
}

#[derive(Default)]
pub(crate) struct Parser {
    next_index: u64,
    sentences: HashMap<u64, (u64, u64, u64, String)>,
}

impl Parser {
    /// One update per result: sentence_id keeps a sentence in the same slot from its first
    /// partial to its final text.
    pub(crate) fn result(&mut self, sentence: &Value) -> Option<Update> {
        if sentence["heartbeat"] == true {
            return None;
        }
        let text = sentence["text"].as_str().unwrap_or("").trim().to_owned();
        let id = sentence["sentence_id"].as_u64().unwrap_or(0);
        let begin = sentence["begin_time"].as_u64().unwrap_or(0);
        let end = sentence["end_time"].as_u64().unwrap_or(begin).max(begin);
        let final_result = sentence["sentence_end"] == true;
        if text.is_empty() && !final_result {
            return None;
        }
        let index = match self.sentences.get(&id) {
            Some((index, ..)) => *index,
            None => {
                let index = self.next_index;
                self.next_index += 1;
                index
            }
        };
        if final_result {
            self.sentences.remove(&id);
        } else {
            self.sentences.insert(id, (index, begin, end, text.clone()));
        }
        Some(Update {
            index,
            start_sample: cloud::millis_to_samples(begin),
            end_sample: cloud::millis_to_samples(end),
            text,
            final_result,
        })
    }

    fn has_open_sentence(&self) -> bool {
        !self.sentences.is_empty()
    }

    /// Sentences the service never closed become final when the task ends.
    fn close_all(&mut self) -> Vec<Update> {
        let mut open: Vec<_> = self.sentences.drain().map(|(_, sentence)| sentence).collect();
        open.sort_by_key(|(index, ..)| *index);
        open.into_iter()
            .filter(|(.., text)| !text.is_empty())
            .map(|(index, begin, end, text)| Update {
                index,
                start_sample: cloud::millis_to_samples(begin),
                end_sample: cloud::millis_to_samples(end),
                text,
                final_result: true,
            })
            .collect()
    }
}

pub struct Client {
    socket: Socket,
    task_id: String,
    opened: Instant,
    input_finished: bool,
    finished: bool,
    /// Set after the sentence that ends a long-lived connection; the next call asks the runtime
    /// for a new connection.
    rollover: bool,
    parser: Parser,
}

const ROLLOVER: &str = "单次连接到达时长上限，正在续接";

impl Client {
    pub fn connect(config: &Config) -> Result<Self, String> {
        Self::connect_to(endpoint(&config.region), config)
    }

    pub(crate) fn connect_to(url: &str, config: &Config) -> Result<Self, String> {
        match Self::start(url, config, MODEL) {
            Err((error, true)) => Self::start(url, config, FALLBACK_MODEL).map_err(|(error, _)| error).or(Err(error)),
            other => other.map_err(|(error, _)| error),
        }
    }

    /// The flag says whether the failure was the model being unavailable to this account.
    fn start(url: &str, config: &Config, model: &str) -> Result<Self, (String, bool)> {
        let headers = [("Authorization", format!("Bearer {}", config.key))];
        let mut socket = cloud::open(PROVIDER, url, &headers).map_err(|error| (error, false))?;
        let task_id = uuid::Uuid::new_v4().simple().to_string();
        socket
            .send(Message::Text(run_task(&task_id, model, config).to_string().into()))
            .map_err(|_| (cloud::transient(PROVIDER, "识别任务发送失败"), false))?;
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if Instant::now() > deadline {
                return Err((cloud::transient(PROVIDER, "等待识别任务开始超时"), false));
            }
            match socket.read() {
                Ok(Message::Text(text)) => {
                    let event: Value = serde_json::from_str(text.as_str()).unwrap_or(Value::Null);
                    match event["header"]["event"].as_str() {
                        Some("task-started") => break,
                        Some("task-failed") => {
                            let code = event["header"]["error_code"].as_str().unwrap_or("");
                            let message = event["header"]["error_message"].as_str().unwrap_or("");
                            return Err((failure(code, message), model_unavailable(code, message)));
                        }
                        _ => {}
                    }
                }
                Ok(Message::Close(_)) | Err(tungstenite::Error::ConnectionClosed) => {
                    return Err((cloud::transient(PROVIDER, "连接在任务开始前被关闭"), false));
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(error)) if cloud::is_poll_timeout(&error) => {}
                Err(_) => return Err((cloud::transient(PROVIDER, "任务开始消息读取失败"), false)),
            }
        }
        cloud::set_timeouts(&mut socket, cloud::POLL_TIMEOUT, PROVIDER).map_err(|error| (error, false))?;
        Ok(Self {
            socket,
            task_id,
            opened: Instant::now(),
            input_finished: false,
            finished: false,
            rollover: false,
            parser: Parser::default(),
        })
    }

    fn send(&mut self, message: Message) -> Result<(), String> {
        cloud::set_timeouts(&mut self.socket, cloud::IO_TIMEOUT, PROVIDER)?;
        let result = self
            .socket
            .send(message)
            .map_err(|_| cloud::transient(PROVIDER, "数据发送失败"));
        cloud::set_timeouts(&mut self.socket, cloud::POLL_TIMEOUT, PROVIDER)?;
        result
    }
}

impl CloudStream for Client {
    fn send_audio(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.rollover {
            return Err(cloud::transient(PROVIDER, ROLLOVER));
        }
        if self.input_finished || self.finished {
            return Err("阿里云百炼音频输入已经结束".into());
        }
        if bytes.is_empty() || !bytes.len().is_multiple_of(2) {
            return Err("阿里云百炼 PCM 音频块格式无效".into());
        }
        self.send(Message::Binary(bytes.to_vec().into()))
    }

    fn finish_input(&mut self) -> Result<(), String> {
        if self.input_finished || self.finished {
            self.input_finished = true;
            return Ok(());
        }
        let finish = json!({
            "header": {"action": "finish-task", "task_id": self.task_id, "streaming": "duplex"},
            "payload": {"input": {}}
        });
        self.send(Message::Text(finish.to_string().into()))?;
        self.input_finished = true;
        Ok(())
    }

    fn poll(&mut self) -> Result<Vec<Update>, String> {
        if self.rollover {
            return Err(cloud::transient(PROVIDER, ROLLOVER));
        }
        if self.finished {
            return Ok(Vec::new());
        }
        match self.socket.read() {
            Ok(Message::Text(text)) => {
                let event: Value = serde_json::from_str(text.as_str())
                    .map_err(|_| cloud::transient(PROVIDER, "返回了无效消息"))?;
                match event["header"]["event"].as_str() {
                    Some("result-generated") => {
                        let update = self.parser.result(&event["payload"]["output"]["sentence"]);
                        let rollover = !self.input_finished
                            && !self.parser.has_open_sentence()
                            && self.opened.elapsed() > ROLLOVER_AFTER;
                        match (update, rollover) {
                            // The finished sentence is kept; the next connection starts after it.
                            (Some(update), true) if update.final_result => {
                                self.rollover = true;
                                Ok(vec![update])
                            }
                            (update, _) => Ok(update.into_iter().collect()),
                        }
                    }
                    Some("task-finished") => {
                        self.finished = true;
                        Ok(self.parser.close_all())
                    }
                    Some("task-failed") => Err(failure(
                        event["header"]["error_code"].as_str().unwrap_or(""),
                        event["header"]["error_message"].as_str().unwrap_or(""),
                    )),
                    _ => Ok(Vec::new()),
                }
            }
            Ok(Message::Close(_)) | Err(tungstenite::Error::ConnectionClosed) => {
                if self.finished {
                    Ok(Vec::new())
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
    fn run_task_matches_the_documented_shape() {
        let config = Config { key: "sk-x".into(), language: "auto".into(), terms: vec!["Tocqueville".into(), "difference in differences estimator for panel data sets".into(), "一个超过十五个字符的很长很长很长的术语名称".into(), "选择偏差".into()], region: String::new() };
        let task = run_task("abc", MODEL, &config);
        assert_eq!(task["header"], json!({"action":"run-task","task_id":"abc","streaming":"duplex"}));
        assert_eq!(task["payload"]["task_group"], "audio");
        assert_eq!(task["payload"]["function"], "recognition");
        assert_eq!(task["payload"]["input"], json!({}));
        let parameters = &task["payload"]["parameters"];
        assert_eq!(parameters["format"], "pcm");
        assert_eq!(parameters["sample_rate"], 16000);
        assert_eq!(parameters["language_hints"], json!(["zh", "en"]));
        assert_eq!(parameters["vocabulary"], json!({"Tocqueville": 4, "选择偏差": 4}));
        let fallback = run_task("abc", FALLBACK_MODEL, &config);
        assert_eq!(fallback["payload"]["parameters"]["language_hints"], json!(["zh"]));
        assert!(fallback["payload"]["parameters"].get("vocabulary").is_none());
        assert_eq!(endpoint("singapore"), "wss://dashscope-intl.aliyuncs.com/api-ws/v1/inference");
        assert_eq!(endpoint(""), "wss://dashscope.aliyuncs.com/api-ws/v1/inference");
    }

    #[test]
    fn a_sentence_keeps_its_slot_from_partial_to_final_and_heartbeats_are_skipped() {
        let mut parser = Parser::default();
        assert!(parser.result(&json!({"heartbeat":true,"sentence_id":0,"text":""})).is_none());
        let partial = parser.result(&json!({"sentence_id":1,"begin_time":170,"end_time":null,"text":"好，我","sentence_end":false})).unwrap();
        assert_eq!((partial.index, partial.start_sample, partial.end_sample, partial.final_result), (0, 2720, 2720, false));
        let done = parser.result(&json!({"sentence_id":1,"begin_time":170,"end_time":920,"text":"好，我知道了","sentence_end":true})).unwrap();
        assert_eq!((done.index, done.end_sample, done.final_result, done.text.as_str()), (0, 14720, true, "好，我知道了"));
        let next = parser.result(&json!({"sentence_id":2,"begin_time":1000,"text":"下面","sentence_end":false})).unwrap();
        assert_eq!(next.index, 1);
        assert_eq!(parser.close_all(), vec![Update { index: 1, start_sample: 16000, end_sample: 16000, text: "下面".into(), final_result: true }]);
    }

    #[test]
    fn key_and_region_problems_are_not_retried() {
        assert!(!cloud::is_transient(&failure("InvalidApiKey", "Invalid API-key provided.")));
        assert!(failure("InvalidApiKey", "").contains("地域"));
        assert!(!cloud::is_transient(&failure("Arrearage", "")));
        assert!(cloud::is_transient(&failure("CLIENT_ERROR", "request timeout after 23 seconds.")));
        assert!(model_unavailable("ModelNotFound", ""));
    }

    #[test]
    fn a_whole_session_against_a_local_server_speaking_the_protocol() {
        let (url, handshake, server) = cloud::mock::serve(|socket| {
            let task: Value = match socket.read().unwrap() { Message::Text(text) => serde_json::from_str(text.as_str()).unwrap(), other => panic!("{other:?}") };
            assert_eq!(task["header"]["action"], "run-task");
            assert_eq!(task["payload"]["model"], MODEL);
            let id = task["header"]["task_id"].clone();
            socket.send(Message::Text(json!({"header":{"task_id":id,"event":"task-started","attributes":{}},"payload":{}}).to_string().into())).unwrap();
            let mut audio = 0;
            loop {
                match socket.read().unwrap() {
                    Message::Binary(bytes) => {
                        audio += bytes.len();
                        let sentence = if audio >= 12800 {
                            json!({"sentence_id":1,"begin_time":100,"end_time":500,"text":"好，我知道了","sentence_end":true})
                        } else {
                            json!({"sentence_id":1,"begin_time":100,"end_time":null,"text":"好，我","sentence_end":false})
                        };
                        socket.send(Message::Text(json!({"header":{"task_id":id,"event":"result-generated"},"payload":{"output":{"sentence":sentence}}}).to_string().into())).unwrap();
                    }
                    Message::Text(text) => {
                        let finish: Value = serde_json::from_str(text.as_str()).unwrap();
                        assert_eq!(finish["header"]["action"], "finish-task");
                        assert_eq!(finish["header"]["task_id"], id);
                        socket.send(Message::Text(json!({"header":{"task_id":id,"event":"task-finished"},"payload":{}}).to_string().into())).unwrap();
                        break;
                    }
                    _ => {}
                }
            }
        });
        let config = Config { key: "sk-test".into(), language: "zh".into(), terms: vec!["选择偏差".into()], region: String::new() };
        let mut client = Client::connect_to(&url, &config).unwrap();
        let updates = cloud::mock::run(&mut client, 3);
        server.join().unwrap();
        assert_eq!(handshake.recv().unwrap().headers["authorization"], "Bearer sk-test");
        assert_eq!(updates.first().map(|u| (u.index, u.final_result)), Some((0, false)));
        assert!(updates.iter().any(|u| u.final_result && u.text == "好，我知道了" && u.index == 0));
    }

    #[test]
    fn an_account_without_the_new_model_falls_back_to_fun_asr() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}/mock", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut models = Vec::new();
            for _ in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                let mut socket = tungstenite::accept(stream).unwrap();
                let task: Value = match socket.read().unwrap() { Message::Text(text) => serde_json::from_str(text.as_str()).unwrap(), other => panic!("{other:?}") };
                let model = task["payload"]["model"].as_str().unwrap().to_owned();
                let reply = if model == MODEL {
                    json!({"header":{"event":"task-failed","error_code":"ModelNotFound","error_message":"Model not found"},"payload":{}})
                } else {
                    json!({"header":{"event":"task-started"},"payload":{}})
                };
                socket.send(Message::Text(reply.to_string().into())).unwrap();
                models.push(model);
                let _ = socket.close(None);
                let _ = socket.flush();
            }
            models
        });
        let config = Config { key: "sk-test".into(), language: "auto".into(), terms: vec![], region: String::new() };
        assert!(Client::connect_to(&url, &config).is_ok());
        assert_eq!(server.join().unwrap(), [MODEL, FALLBACK_MODEL]);
    }

}

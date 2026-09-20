use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::{blocking::Client as HttpClient, redirect::Policy, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{io::Read, time::Duration};

const ENDPOINT: &str = "https://api.deepseek.com/chat/completions";
const MODEL: &str = "deepseek-flash";
const MAX_API_KEY_BYTES: usize = 512;
const MAX_TRANSLATION_CHARS: usize = 12_000;
const MAX_POLISH_CHARS: usize = 12_000;
const MAX_RESULT_CHARS: usize = 64_000;
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
const MAX_RESPONSE_BYTES: u64 = 256 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslateRequest {
    pub text: String,
    pub target_language: String,
    pub consent: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct TranslateResult {
    pub text: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolishRequest {
    pub text: String,
    pub consent: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct PolishResult {
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct PolishSegmentsRequest {
    pub segments: Vec<String>,
    pub consent: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PolishedSegment {
    pub source_start: usize,
    pub source_end: usize,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct PolishSegmentsResult {
    pub segments: Vec<PolishedSegment>,
}

#[derive(Deserialize)]
struct PolishSegmentsPayload {
    segments: Vec<PolishedSegment>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormulaRequest {
    pub image_data: String,
    pub consent: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct FormulaResult {
    pub latex: String,
}

#[derive(Deserialize)]
struct CompletionResponse {
    choices: Vec<CompletionChoice>,
}

#[derive(Deserialize)]
struct CompletionChoice {
    finish_reason: Option<String>,
    message: CompletionMessage,
}

#[derive(Deserialize)]
struct CompletionMessage {
    content: String,
}

#[derive(Deserialize)]
struct TranslationPayload {
    text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FormulaPayload {
    has_formula: bool,
    latex: String,
}

pub struct Client {
    http: HttpClient,
    endpoint: String,
}

impl Client {
    pub fn new() -> Result<Self, String> {
        let http = HttpClient::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .redirect(Policy::none())
            .https_only(true)
            .build()
            .map_err(|_| "无法初始化 DeepSeek 客户端".to_string())?;
        Ok(Self {
            http,
            endpoint: ENDPOINT.to_owned(),
        })
    }

    #[cfg(test)]
    fn for_test(endpoint: String) -> Self {
        Self {
            http: HttpClient::builder()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(2))
                .redirect(Policy::none())
                .no_proxy()
                .build()
                .unwrap(),
            endpoint,
        }
    }

    pub fn translate(
        &self,
        api_key: &str,
        request: TranslateRequest,
    ) -> Result<TranslateResult, String> {
        validate_key(api_key)?;
        if !request.consent {
            return Err("请确认仅将当前选中文字发送到 DeepSeek".into());
        }
        let text = request.text.trim();
        if text.is_empty() || text.chars().count() > MAX_TRANSLATION_CHARS {
            return Err("翻译文本需为 1–12000 个字符".into());
        }
        let language = match request.target_language.as_str() {
            "zh" => "简体中文",
            "en" => "English",
            _ => return Err("翻译目标语言无效".into()),
        };
        let body = json!({
            "model": MODEL,
            "messages": [
                {
                    "role": "system",
                    "content": "You translate lecture text faithfully. Preserve terminology, equations, numbers, names, and paragraph structure. Return one JSON object with exactly one string field named text. Do not add commentary."
                },
                {
                    "role": "user",
                    "content": format!("Translate the following text into {language}:\n\n{text}")
                }
            ],
            "thinking": {"type":"disabled"},
            "stream": false,
            "response_format": {"type":"json_object"}
        });
        let content = self.complete(api_key, body)?;
        let payload: TranslationPayload =
            serde_json::from_str(&content).map_err(|_| "DeepSeek 翻译结果格式无效".to_string())?;
        let translated = payload.text.trim();
        if translated.is_empty() || translated.chars().count() > MAX_RESULT_CHARS {
            return Err("DeepSeek 翻译结果长度无效".into());
        }
        Ok(TranslateResult {
            text: translated.to_owned(),
        })
    }

    pub fn polish(&self, api_key: &str, request: PolishRequest) -> Result<PolishResult, String> {
        validate_key(api_key)?;
        if !request.consent {
            return Err("请确认将已完成的转写句段发送到 DeepSeek".into());
        }
        let text = request.text.trim();
        let input_chars = text.chars().count();
        if text.is_empty() || input_chars > MAX_POLISH_CHARS {
            return Err("待整理转写需为 1–12000 个字符".into());
        }
        let body = json!({
            "model": MODEL,
            "messages": [
                {
                    "role": "system",
                    "content": "You lightly clean a finalized lecture transcript. Make the smallest possible edit. You may only join an obvious accidental sentence split, remove isolated speech fillers such as um, uh, er, or you know, and fix clear punctuation, capitalization, or spacing errors. Preserve every fact, claim, qualification, example, term, name, number, equation, language choice, emphasis, and tone. Preserve deliberate repetition and discourse markers. Never summarize, paraphrase, expand, translate, explain, censor, or add information. If no permitted edit is needed, return the input unchanged. Return one JSON object with exactly one string field named text."
                },
                {
                    "role": "user",
                    "content": format!("Lightly clean this finalized transcript segment:\n\n{text}")
                }
            ],
            "thinking": {"type":"disabled"},
            "stream": false,
            "response_format": {"type":"json_object"}
        });
        let content = self.complete(api_key, body)?;
        let payload: TranslationPayload =
            serde_json::from_str(&content).map_err(|_| "DeepSeek 整理结果格式无效".to_string())?;
        let polished = payload.text.trim();
        let allowed_growth = 64usize.max(input_chars / 5);
        if polished.is_empty()
            || polished.chars().count() > input_chars.saturating_add(allowed_growth)
        {
            return Err("DeepSeek 整理结果改动范围过大".into());
        }
        Ok(PolishResult {
            text: polished.to_owned(),
        })
    }

    pub fn polish_segments(
        &self,
        api_key: &str,
        request: PolishSegmentsRequest,
    ) -> Result<PolishSegmentsResult, String> {
        validate_key(api_key)?;
        if !request.consent {
            return Err("请确认将已完成的转写句段发送到 DeepSeek".into());
        }
        if request.segments.is_empty() || request.segments.len() > 8 {
            return Err("待整理转写片段数量无效".into());
        }
        let normalized: Vec<String> = request
            .segments
            .iter()
            .map(|segment| segment.trim().to_owned())
            .collect();
        let input_chars: usize = normalized
            .iter()
            .map(|segment| segment.chars().count())
            .sum();
        if normalized.iter().any(String::is_empty) || input_chars > MAX_POLISH_CHARS {
            return Err("待整理转写需为 1–12000 个字符".into());
        }
        let input =
            serde_json::to_string(&normalized).map_err(|_| "待整理转写格式无效".to_string())?;
        let body = json!({
            "model": MODEL,
            "messages": [
                {
                    "role": "system",
                    "content": "You lightly clean consecutive finalized lecture transcript segments. Make the smallest possible edit. You may join adjacent source segments only when they are an obvious accidental ASR split, remove isolated speech fillers such as um, uh, er, or you know, and fix clear punctuation, capitalization, or spacing errors. Preserve every fact, claim, qualification, example, term, name, number, equation, language choice, emphasis, and tone. Preserve deliberate repetition and discourse markers. Never summarize, paraphrase, expand, translate, explain, censor, or add information. A period and capitalization at an input boundary may be ASR mistakes: for example, inputs ending with `what the aristocratic.` and beginning with `Values are all about.` should be joined as `what the aristocratic values are all about.` Return one JSON object with exactly one field named segments. segments must partition every input source index exactly once and in order. Each item must contain sourceStart (inclusive zero-based index), sourceEnd (exclusive index), and text. Use one source index per output item unless adjacent inputs clearly form one sentence. Never split one source index across outputs."
                },
                {
                    "role": "user",
                    "content": format!("Lightly clean these consecutive finalized transcript segments. Input JSON array:\n{input}")
                }
            ],
            "thinking": {"type":"disabled"},
            "stream": false,
            "response_format": {"type":"json_object"}
        });
        let content = self.complete(api_key, body)?;
        let payload: PolishSegmentsPayload =
            serde_json::from_str(&content).map_err(|_| "DeepSeek 整理结果格式无效".to_string())?;
        let mut cursor = 0usize;
        let mut output_chars = 0usize;
        for segment in &payload.segments {
            let text = segment.text.trim();
            if segment.source_start != cursor
                || segment.source_end <= segment.source_start
                || segment.source_end > normalized.len()
                || text.is_empty()
            {
                return Err("DeepSeek 整理结果片段映射无效".into());
            }
            cursor = segment.source_end;
            output_chars = output_chars.saturating_add(text.chars().count());
        }
        let allowed_growth = 64usize.max(input_chars / 5);
        if cursor != normalized.len() || output_chars > input_chars.saturating_add(allowed_growth) {
            return Err("DeepSeek 整理结果片段映射无效".into());
        }
        Ok(PolishSegmentsResult {
            segments: coalesce_sentence_continuations(payload.segments),
        })
    }

    pub fn recognize_formula(
        &self,
        api_key: &str,
        request: FormulaRequest,
    ) -> Result<FormulaResult, String> {
        validate_key(api_key)?;
        if !request.consent {
            return Err("请确认仅将当前图片发送到 DeepSeek".into());
        }
        validate_image_data(&request.image_data)?;
        let body = json!({
            "model": MODEL,
            "messages": [
                {
                    "role": "system",
                    "content": "You transcribe mathematical formulas from images into KaTeX-compatible LaTeX. Transcribe only; never solve, simplify, explain, or infer missing work. Return one JSON object with exactly hasFormula (boolean) and latex (string). When no formula is visible, return {\"hasFormula\":false,\"latex\":\"\"}. Do not wrap LaTeX in Markdown fences or dollar signs."
                },
                {
                    "role": "user",
                    "content": [
                        {"type":"text","text":"Transcribe every visible mathematical formula exactly into KaTeX-compatible LaTeX. Preserve line breaks with LaTeX line breaks when needed."},
                        {"type":"image_url","image_url":{"url":request.image_data}}
                    ]
                }
            ],
            "thinking": {"type":"disabled"},
            "stream": false,
            "response_format": {"type":"json_object"}
        });
        let content = self.complete(api_key, body)?;
        let payload: FormulaPayload = serde_json::from_str(&content)
            .map_err(|_| "DeepSeek 公式识别结果格式无效".to_string())?;
        if !payload.has_formula {
            return Err("图片中未识别到公式".into());
        }
        let latex = trim_latex(&payload.latex);
        if latex.is_empty() || latex.chars().count() > MAX_RESULT_CHARS {
            return Err("DeepSeek 公式识别结果长度无效".into());
        }
        Ok(FormulaResult { latex })
    }

    fn complete(&self, api_key: &str, body: Value) -> Result<String, String> {
        let response = self
            .http
            .post(&self.endpoint)
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .map_err(|_| "无法连接 DeepSeek 服务".to_string())?;
        let status = response.status();
        if !status.is_success() {
            return Err(clean_status_error(status));
        }
        let mut response = response;
        let mut bytes = Vec::new();
        Read::take(&mut response, MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "DeepSeek 响应读取失败".to_string())?;
        if bytes.len() as u64 > MAX_RESPONSE_BYTES {
            return Err("DeepSeek 响应超出大小限制".into());
        }
        let parsed: CompletionResponse =
            serde_json::from_slice(&bytes).map_err(|_| "DeepSeek 返回格式无效".to_string())?;
        let choice = parsed
            .choices
            .into_iter()
            .next()
            .ok_or("DeepSeek 未返回结果")?;
        match choice.finish_reason.as_deref() {
            Some("length") => return Err("DeepSeek 输出达到长度限制，请缩短输入后重试".into()),
            Some("stop") => {}
            _ => return Err("DeepSeek 未能完成请求".into()),
        }
        if choice.message.content.len() > MAX_RESPONSE_BYTES as usize {
            return Err("DeepSeek 响应超出大小限制".into());
        }
        Ok(choice.message.content)
    }
}

fn validate_key(api_key: &str) -> Result<(), String> {
    if api_key.is_empty()
        || api_key.len() > MAX_API_KEY_BYTES
        || api_key.chars().any(char::is_whitespace)
    {
        return Err("DeepSeek API 密钥格式无效".into());
    }
    Ok(())
}

fn clean_status_error(status: StatusCode) -> String {
    match status.as_u16() {
        401 | 403 => "DeepSeek API 密钥无效或无权访问".into(),
        429 => "DeepSeek 请求过于频繁，请稍后重试".into(),
        code => format!("DeepSeek 服务返回状态 {code}"),
    }
}

fn validate_image_data(value: &str) -> Result<(), String> {
    let (kind, payload) = [
        ("png", "data:image/png;base64,"),
        ("jpeg", "data:image/jpeg;base64,"),
        ("webp", "data:image/webp;base64,"),
    ]
    .into_iter()
    .find_map(|(kind, prefix)| value.strip_prefix(prefix).map(|payload| (kind, payload)))
    .ok_or("请选择 PNG、JPEG 或 WebP 图片")?;
    if payload.len() > (MAX_IMAGE_BYTES * 4 / 3) + 8 {
        return Err("图片大小不能超过 5 MiB".into());
    }
    let decoded = STANDARD
        .decode(payload)
        .map_err(|_| "图片数据格式无效".to_string())?;
    if decoded.is_empty() || decoded.len() > MAX_IMAGE_BYTES {
        return Err("图片大小不能超过 5 MiB".into());
    }
    let valid = match kind {
        "png" => decoded.starts_with(b"\x89PNG\r\n\x1a\n"),
        "jpeg" => decoded.starts_with(&[0xff, 0xd8, 0xff]),
        "webp" => decoded.starts_with(b"RIFF") && decoded.get(8..12) == Some(b"WEBP"),
        _ => false,
    };
    if !valid {
        return Err("图片文件签名无效".into());
    }
    Ok(())
}

fn trim_latex(value: &str) -> String {
    let mut text = value.trim();
    if text.starts_with("```") {
        if let Some(newline) = text.find('\n') {
            text = text[newline + 1..].trim();
        }
        if let Some(stripped) = text.strip_suffix("```") {
            text = stripped.trim();
        }
    }
    if let Some(stripped) = text
        .strip_prefix("$$")
        .and_then(|text| text.strip_suffix("$$"))
        .or_else(|| {
            text.strip_prefix("\\[")
                .and_then(|text| text.strip_suffix("\\]"))
        })
        .or_else(|| {
            text.strip_prefix("\\(")
                .and_then(|text| text.strip_suffix("\\)"))
        })
    {
        text = stripped.trim();
    }
    text.to_owned()
}

fn coalesce_sentence_continuations(segments: Vec<PolishedSegment>) -> Vec<PolishedSegment> {
    let mut output: Vec<PolishedSegment> = Vec::with_capacity(segments.len());
    for mut segment in segments {
        segment.text = segment.text.trim().to_owned();
        let continuation = segment
            .text
            .chars()
            .find(|character| character.is_alphabetic())
            .is_some_and(char::is_lowercase);
        if continuation {
            if let Some(previous) = output.last_mut() {
                if previous.source_end == segment.source_start {
                    let mut left = previous.text.trim_end().to_owned();
                    if left.chars().next_back().is_some_and(|character| {
                        matches!(character, '.' | '!' | '?' | '。' | '！' | '？')
                    }) {
                        left.pop();
                    }
                    previous.text = format!("{} {}", left.trim_end(), segment.text.trim_start());
                    previous.source_end = segment.source_end;
                    continue;
                }
            }
        }
        output.push(segment);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        thread,
    };

    fn fake_server(status: u16, response_body: Vec<u8>) -> (String, mpsc::Receiver<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            let header_end = loop {
                let count = stream.read(&mut buffer).unwrap();
                if count == 0 {
                    return;
                }
                request.extend_from_slice(&buffer[..count]);
                if let Some(position) = request.windows(4).position(|value| value == b"\r\n\r\n") {
                    break position + 4;
                }
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.split_once(':').and_then(|(name, value)| {
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                })
                .unwrap_or(0);
            while request.len() < header_end + content_length {
                let count = stream.read(&mut buffer).unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);
            }
            let _ = sender.send(request);
            let reason = if status == 200 { "OK" } else { "Error" };
            write!(
                stream,
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response_body.len()
            )
            .unwrap();
            stream.write_all(&response_body).unwrap();
        });
        (format!("http://{address}/chat/completions"), receiver)
    }

    fn completion(content: Value, finish_reason: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "choices":[{"finish_reason":finish_reason,"message":{"content":content.to_string()}}]
        }))
        .unwrap()
    }

    #[test]
    fn translation_uses_fixed_model_disabled_thinking_and_bearer_key() {
        let secret = "test-key-never-persist";
        let (endpoint, request) = fake_server(200, completion(json!({"text":"Hello"}), "stop"));
        let result = Client::for_test(endpoint)
            .translate(
                secret,
                TranslateRequest {
                    text: "你好".into(),
                    target_language: "en".into(),
                    consent: true,
                },
            )
            .unwrap();
        assert_eq!(result.text, "Hello");
        let request = String::from_utf8(request.recv().unwrap()).unwrap();
        assert!(request
            .to_ascii_lowercase()
            .contains("authorization: bearer test-key-never-persist"));
        assert!(request.contains(r#""model":"deepseek-flash""#));
        assert!(request.contains(r#""thinking":{"type":"disabled"}"#));
        assert!(request.contains(r#""stream":false"#));
        assert!(request.contains(r#""response_format":{"type":"json_object"}"#));
    }

    #[test]
    fn polish_uses_minimal_edit_prompt_and_fixed_flash_model() {
        let (endpoint, request) = fake_server(
            200,
            completion(
                json!({"text":"The demand curve, um, shifts when income changes."}),
                "stop",
            ),
        );
        let result = Client::for_test(endpoint)
            .polish(
                "test-key",
                PolishRequest {
                    text: "The demand curve. Um, shifts when income changes.".into(),
                    consent: true,
                },
            )
            .unwrap();
        assert_eq!(
            result.text,
            "The demand curve, um, shifts when income changes."
        );
        let request = String::from_utf8(request.recv().unwrap()).unwrap();
        assert!(request.contains(r#""model":"deepseek-flash""#));
        assert!(request.contains(r#""thinking":{"type":"disabled"}"#));
        assert!(request.contains("smallest possible edit"));
        assert!(request.contains("Never summarize"));
    }

    #[test]
    fn polish_rejects_missing_consent_and_excessive_growth() {
        let client = Client::for_test("http://127.0.0.1:1".into());
        assert!(client
            .polish(
                "test-key",
                PolishRequest {
                    text: "A short lecture sentence.".into(),
                    consent: false,
                },
            )
            .unwrap_err()
            .contains("确认"));

        let (endpoint, _) = fake_server(200, completion(json!({"text":"x".repeat(200)}), "stop"));
        assert!(Client::for_test(endpoint)
            .polish(
                "test-key",
                PolishRequest {
                    text: "Brief.".into(),
                    consent: true,
                },
            )
            .unwrap_err()
            .contains("改动范围"));
    }

    #[test]
    fn segment_polish_returns_a_valid_source_partition_for_joined_text() {
        let (endpoint, request) = fake_server(
            200,
            completion(
                json!({"segments":[{"sourceStart":0,"sourceEnd":2,"text":"The demand curve shifts when income changes."}]}),
                "stop",
            ),
        );
        let result = Client::for_test(endpoint)
            .polish_segments(
                "test-key",
                PolishSegmentsRequest {
                    segments: vec![
                        "The demand curve".into(),
                        "shifts when income changes.".into(),
                    ],
                    consent: true,
                },
            )
            .unwrap();
        assert_eq!(result.segments.len(), 1);
        assert_eq!(result.segments[0].source_start, 0);
        assert_eq!(result.segments[0].source_end, 2);
        let request = String::from_utf8(request.recv().unwrap()).unwrap();
        assert!(request.contains("sourceStart"));
        assert!(request.contains("obvious accidental ASR split"));
    }

    #[test]
    fn segment_polish_rejects_gaps_in_source_mapping() {
        let (endpoint, _) = fake_server(
            200,
            completion(
                json!({"segments":[{"sourceStart":1,"sourceEnd":2,"text":"Second."}]}),
                "stop",
            ),
        );
        assert!(Client::for_test(endpoint)
            .polish_segments(
                "test-key",
                PolishSegmentsRequest {
                    segments: vec!["First".into(), "Second.".into()],
                    consent: true,
                },
            )
            .unwrap_err()
            .contains("映射"));
    }

    #[test]
    fn segment_polish_maps_false_period_and_capitalization_across_sources() {
        let (endpoint, request) = fake_server(
            200,
            completion(
                json!({"segments":[{"sourceStart":0,"sourceEnd":2,"text":"That is what the aristocratic values are all about. The lecture continues."}]}),
                "stop",
            ),
        );
        let result = Client::for_test(endpoint)
            .polish_segments(
                "test-key",
                PolishSegmentsRequest {
                    segments: vec![
                        "That is what the aristocratic.".into(),
                        "Values are all about. The lecture continues.".into(),
                    ],
                    consent: true,
                },
            )
            .unwrap();
        assert_eq!(
            result.segments,
            vec![PolishedSegment {
                source_start: 0,
                source_end: 2,
                text: "That is what the aristocratic values are all about. The lecture continues."
                    .into()
            }]
        );
        let request = String::from_utf8(request.recv().unwrap()).unwrap();
        assert!(request.contains("what the aristocratic."));
        assert!(request.contains("Values are all about."));
    }

    #[test]
    fn segment_polish_coalesces_lowercase_continuation_returned_as_separate_items() {
        let (endpoint, _request) = fake_server(
            200,
            completion(
                json!({"segments":[
                    {"sourceStart":0,"sourceEnd":1,"text":"That is what the aristocratic."},
                    {"sourceStart":1,"sourceEnd":2,"text":"values are all about."}
                ]}),
                "stop",
            ),
        );
        let result = Client::for_test(endpoint)
            .polish_segments(
                "test-key",
                PolishSegmentsRequest {
                    segments: vec![
                        "That is what the aristocratic.".into(),
                        "Values are all about.".into(),
                    ],
                    consent: true,
                },
            )
            .unwrap();
        assert_eq!(
            result.segments,
            vec![PolishedSegment {
                source_start: 0,
                source_end: 2,
                text: "That is what the aristocratic values are all about.".into(),
            }]
        );
    }

    #[test]
    fn formula_validates_signature_and_trims_katex_wrappers() {
        let image = format!(
            "data:image/png;base64,{}",
            STANDARD.encode(b"\x89PNG\r\n\x1a\nsmall-test")
        );
        let (endpoint, request) = fake_server(
            200,
            completion(
                json!({"hasFormula":true,"latex":"```latex\n$$x^2 + y^2$$\n```"}),
                "stop",
            ),
        );
        let result = Client::for_test(endpoint)
            .recognize_formula(
                "test-key",
                FormulaRequest {
                    image_data: image,
                    consent: true,
                },
            )
            .unwrap();
        assert_eq!(result.latex, "x^2 + y^2");
        let request = String::from_utf8(request.recv().unwrap()).unwrap();
        assert!(request.contains(r#""type":"image_url""#));
        assert!(request.contains("data:image/png;base64,"));
        let invalid = format!("data:image/png;base64,{}", STANDARD.encode(b"not png"));
        assert!(validate_image_data(&invalid).unwrap_err().contains("签名"));
    }

    #[test]
    fn consent_and_input_bounds_are_enforced_before_network() {
        let client = Client::for_test("http://127.0.0.1:1".into());
        assert!(client
            .translate(
                "test-key",
                TranslateRequest {
                    text: "private".into(),
                    target_language: "zh".into(),
                    consent: false,
                }
            )
            .unwrap_err()
            .contains("确认"));
        assert!(client
            .translate(
                "test-key",
                TranslateRequest {
                    text: "x".repeat(MAX_TRANSLATION_CHARS + 1),
                    target_language: "zh".into(),
                    consent: true,
                }
            )
            .unwrap_err()
            .contains("12000"));
    }

    #[test]
    fn service_errors_and_length_finish_are_sanitized() {
        let secret = "server echoed secret test-key";
        let (endpoint, _) = fake_server(500, secret.as_bytes().to_vec());
        let error = Client::for_test(endpoint)
            .translate(
                "test-key",
                TranslateRequest {
                    text: "hello".into(),
                    target_language: "zh".into(),
                    consent: true,
                },
            )
            .unwrap_err();
        assert_eq!(error, "DeepSeek 服务返回状态 500");
        assert!(!error.contains(secret));

        let (endpoint, _) = fake_server(200, completion(json!({"text":"partial"}), "length"));
        assert!(Client::for_test(endpoint)
            .translate(
                "test-key",
                TranslateRequest {
                    text: "hello".into(),
                    target_language: "zh".into(),
                    consent: true,
                },
            )
            .unwrap_err()
            .contains("长度限制"));
    }

    #[test]
    fn oversized_response_is_rejected() {
        let (endpoint, _) = fake_server(200, vec![b'x'; MAX_RESPONSE_BYTES as usize + 1]);
        assert!(Client::for_test(endpoint)
            .translate(
                "test-key",
                TranslateRequest {
                    text: "hello".into(),
                    target_language: "zh".into(),
                    consent: true,
                },
            )
            .unwrap_err()
            .contains("大小限制"));
    }
}

use lectureedit_lib::runtime::Runtime;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

fn command_id() -> String {
    Uuid::new_v4().to_string()
}
fn require(condition: bool, message: impl Into<String>) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn sha256(path: &Path) -> Result<String, String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(fs::read(path).map_err(|e| e.to_string())?)
    ))
}

fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    if !(3..=4).contains(&args.len()) {
        return Err("usage: lectureedit-probe DATA_DIRECTORY AUTHORIZED_WAV [auto|en|zh]".into());
    }
    let root = PathBuf::from(&args[1]);
    let runtime = Runtime::new(root.clone(), PathBuf::new())?;
    if let Some(language) = args.get(3) {
        require(
            ["auto", "en", "zh"].contains(&language.as_str()),
            "Invalid language",
        )?;
        let mut settings = runtime.dispatch(json!({"type":"snapshot"}))?.settings;
        settings.language = language.clone();
        runtime
            .dispatch(json!({"type":"settings","commandId":command_id(),"settings":settings}))?;
    }
    let state = runtime.dispatch(json!({"type":"createSession","commandId":command_id(),"title":"Local integration test","mode":"live"}))?;
    let sid = state.selected_session_id.ok_or("No session selected")?;
    let start = Instant::now();
    runtime.dispatch(
        json!({"type":"importAudio","commandId":command_id(),"sessionId":sid,"path":args[2]}),
    )?;

    let completed = loop {
        let state = runtime.dispatch(json!({"type":"snapshot"}))?;
        let session = state
            .sessions
            .iter()
            .find(|s| s.id == sid)
            .ok_or("Created session disappeared")?;
        let queued = fs::read_dir(root.join("jobs"))
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .filter(|f| f.path().extension().is_some_and(|e| e == "json"))
            .count();
        if queued == 0 && !session.segments.is_empty() {
            break state;
        }
        if start.elapsed() > Duration::from_secs(180) {
            runtime.shutdown();
            return Err(format!(
                "Timeout; segments={}, inference={}, error={:?}",
                session.segments.len(),
                session.inference_state,
                session.error
            ));
        }
        thread::sleep(Duration::from_millis(250));
    };

    let session = completed
        .sessions
        .iter()
        .find(|s| s.id == sid)
        .ok_or("Completed session disappeared")?;
    require(
        session.inference_state == "ready",
        format!(
            "Probe finished with inference state {}",
            session.inference_state
        ),
    )?;
    require(
        session.error.is_none(),
        format!("Probe retained an error: {:?}", session.error),
    )?;
    require(
        session.segments.iter().all(|s| s.final_),
        "Probe completed with a non-final segment",
    )?;
    let segment = session
        .segments
        .iter()
        .find(|s| !s.display_text.trim().is_empty())
        .ok_or("Authorized WAV produced no speech text")?;
    let segment_id = segment.id.clone();
    let corrected = format!("[Verified] <probe&> {}", segment.display_text);
    let state = runtime.dispatch(
        json!({"type":"beginEdit","commandId":command_id(),"sessionId":sid,"segmentId":segment_id}),
    )?;
    let session = state
        .sessions
        .iter()
        .find(|s| s.id == sid)
        .ok_or("Session disappeared while editing")?;
    let draft = session.drafts.last().ok_or("beginEdit created no draft")?;
    runtime.dispatch(json!({"type":"commitEdit","commandId":command_id(),"sessionId":sid,"draftId":draft.id,"expectedUserSeq":draft.expected_user_seq,"text":corrected}))?;

    for (format, extension) in [
        ("markdown", "md"),
        ("html", "html"),
        ("wav", "wav"),
        ("lecture", "lecture"),
    ] {
        runtime.dispatch(json!({"type":"export","commandId":command_id(),"sessionId":sid,"format":format,"path":root.join(format!("roundtrip.{extension}"))}))?;
    }
    let markdown = fs::read_to_string(root.join("roundtrip.md")).map_err(|e| e.to_string())?;
    require(
        markdown.contains(&corrected),
        "Markdown omitted the committed correction",
    )?;
    let html = fs::read_to_string(root.join("roundtrip.html")).map_err(|e| e.to_string())?;
    require(
        html.contains("[Verified] &lt;probe&amp;&gt;") && !html.contains("<probe&>"),
        "HTML did not preserve and escape the correction",
    )?;

    let state = runtime.dispatch(json!({"type":"snapshot"}))?;
    let original = state
        .sessions
        .iter()
        .find(|s| s.id == sid)
        .ok_or("Original session disappeared")?;
    let segment_count = original.segments.len();
    let expected_samples: u64 = original.runs.iter().map(|r| r.samples).sum();
    require(
        expected_samples > 0,
        "Imported WAV produced no stored audio",
    )?;
    let original_pcm: Vec<(String, String)> = original
        .runs
        .iter()
        .map(|run| {
            sha256(
                &root
                    .join("audio")
                    .join(&sid)
                    .join(format!("{}.pcm", run.id)),
            )
            .map(|hash| (run.id.clone(), hash))
        })
        .collect::<Result<_, _>>()?;
    let wav = hound::WavReader::open(root.join("roundtrip.wav"))
        .map_err(|e| format!("Exported WAV is invalid: {e}"))?;
    require(
        wav.spec().channels == 1
            && wav.spec().sample_rate == 16_000
            && wav.spec().bits_per_sample == 16
            && wav.spec().sample_format == hound::SampleFormat::Int,
        "Exported WAV format is not 16 kHz mono 16-bit PCM",
    )?;
    require(
        u64::from(wav.duration()) == expected_samples,
        format!(
            "Exported WAV duration {} != {expected_samples}",
            wav.duration()
        ),
    )?;

    let imported_state = runtime.dispatch(json!({"type":"importPackage","commandId":command_id(),"path":root.join("roundtrip.lecture")}))?;
    let imported_id = imported_state
        .selected_session_id
        .clone()
        .ok_or("Imported package selected no session")?;
    require(
        imported_id != sid,
        "Package import reused the original session ID",
    )?;
    let imported = imported_state
        .sessions
        .iter()
        .find(|s| s.id == imported_id)
        .ok_or("Imported session is missing")?;
    require(
        imported.segments == original.segments,
        "Package round trip changed transcript segments",
    )?;
    require(
        imported.runs == original.runs,
        "Package round trip changed audio run metadata",
    )?;
    require(
        imported
            .segments
            .iter()
            .any(|s| s.display_text == corrected && s.corrected),
        "Imported package lost the correction",
    )?;
    for (run_id, expected_hash) in &original_pcm {
        require(
            sha256(
                &root
                    .join("audio")
                    .join(&imported_id)
                    .join(format!("{run_id}.pcm")),
            )? == *expected_hash,
            format!("Imported PCM hash changed for run {run_id}"),
        )?;
    }

    runtime.shutdown();
    drop(runtime);
    let reopened = Runtime::new(root.clone(), PathBuf::new())?;
    let durable = reopened.dispatch(json!({"type":"snapshot"}))?;
    for session_id in [&sid, &imported_id] {
        let session = durable
            .sessions
            .iter()
            .find(|s| &s.id == session_id)
            .ok_or_else(|| format!("Session {session_id} was lost after restart"))?;
        require(
            session
                .segments
                .iter()
                .any(|s| s.display_text == corrected && s.corrected),
            format!("Session {session_id} lost its correction after restart"),
        )?;
    }
    reopened.shutdown();
    println!("{}", serde_json::to_string_pretty(&json!({"status":"passed","elapsedSeconds":start.elapsed().as_secs_f64(),"segments":segment_count,"allFinal":true,"samples":expected_samples,"corrected":true,"exportsValidated":["markdown","html","wav","lecture"],"packageRoundTrip":true,"restartDurable":true,"sessionId":sid,"importedSessionId":imported_id})).map_err(|e| e.to_string())?);
    Ok(())
}

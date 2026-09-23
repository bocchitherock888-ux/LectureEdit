use crate::domain::{safe_id, validate_session, Session};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const MAX_ENTRIES: usize = 4_096;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;
const MAX_AUDIO_ENTRY_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LecturePackage {
    schema_version: u32,
    session: Session,
}

pub fn export_markdown(session: &Session) -> String {
    let mut output = format!("# {}\n\n", session.title);
    for segment in &session.segments {
        output.push_str(&segment.display_text);
        output.push_str("\n\n");
        for note in session
            .notes
            .iter()
            .filter(|note| note.segment_id == segment.id)
        {
            let label = match note.kind.as_str() {
                "formula" => "Formula",
                "example" => "Example",
                "image" => "Image",
                _ => "Note",
            };
            output.push_str(&format!("> **{label}:** {}\n\n", note.text));
        }
    }
    output
}

pub fn export_html(session: &Session) -> String {
    let mut body = format!("<h1>{}</h1>\n", html(&session.title));
    for segment in &session.segments {
        body.push_str(&format!(
            "<section data-segment-id=\"{}\"><p>{}</p>",
            html(&segment.id),
            html(&segment.display_text).replace('\n', "<br>\n")
        ));
        for note in session
            .notes
            .iter()
            .filter(|note| note.segment_id == segment.id)
        {
            body.push_str(&format!(
                "<aside class=\"{}\"><strong>{}</strong><p>{}</p></aside>",
                html(&note.kind),
                html(note.source_label.as_deref().unwrap_or(&note.kind)),
                html(&note.text).replace('\n', "<br>\n")
            ));
        }
        body.push_str("</section>\n");
    }
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{}</title><style>body{{max-width:48rem;margin:3rem auto;padding:0 1.25rem;font:17px/1.65 system-ui,sans-serif;color:#232323}}section{{margin:1.5rem 0}}aside{{margin:1rem 0;padding:.75rem 1rem;background:#f7f7f5;border-left:3px solid #3c6e71}}aside p{{margin:.25rem 0}}</style></head><body>{}</body></html>\n",
        html(&session.title), body
    )
}

pub fn export_lecture(session: &Session, audio_root: &Path, output: &Path) -> Result<(), String> {
    validate_session(session)?;
    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(io_error)?;
    }
    let temporary = output.with_extension(format!("part-{}", Uuid::new_v4()));
    let result = (|| {
        let file = File::options()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(io_error)?;
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .unix_permissions(0o600);
        let mut exported_session = session.clone();
        exported_session.project_id = None;
        let package = LecturePackage {
            schema_version: 1,
            session: exported_session,
        };
        let manifest = serde_json::to_vec_pretty(&package).map_err(json_error)?;
        zip.start_file("lecture.json", options).map_err(zip_error)?;
        zip.write_all(&manifest).map_err(io_error)?;

        for run in &session.runs {
            if !safe_id(&run.id) {
                return Err("INVALID_RUN_ID".into());
            }
            let source = audio_root.join(&session.id).join(format!("{}.pcm", run.id));
            if !source.exists() {
                if session.mode == "demo" {
                    continue;
                }
                return Err("MISSING_AUDIO_FILE".into());
            }
            let metadata = fs::symlink_metadata(&source).map_err(io_error)?;
            if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                return Err("INVALID_AUDIO_FILE".into());
            }
            let expected = run.samples.checked_mul(2).ok_or("AUDIO_TOO_LARGE")?;
            if expected > MAX_AUDIO_ENTRY_BYTES
                || metadata.len() < expected
                || metadata.len() % 2 != 0
            {
                return Err("AUDIO_LENGTH_MISMATCH".into());
            }
            zip.start_file(format!("audio/{}.pcm", run.id), options)
                .map_err(zip_error)?;
            let input = File::open(source).map_err(io_error)?;
            let copied = std::io::copy(&mut input.take(expected), &mut zip).map_err(io_error)?;
            if copied != expected {
                return Err("AUDIO_LENGTH_MISMATCH".into());
            }
        }
        let file = zip.finish().map_err(zip_error)?;
        file.sync_all().map_err(io_error)?;
        fs::rename(&temporary, output).map_err(io_error)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub fn import_lecture(input: &Path, audio_root: &Path) -> Result<Session, String> {
    let file = File::open(input).map_err(io_error)?;
    let mut zip = ZipArchive::new(file).map_err(zip_error)?;
    if zip.is_empty() || zip.len() > MAX_ENTRIES {
        return Err("INVALID_ARCHIVE_ENTRY_COUNT".into());
    }
    let mut seen = HashSet::new();
    let mut total = 0_u64;
    for index in 0..zip.len() {
        let entry = zip.by_index(index).map_err(zip_error)?;
        let name = entry.name().to_string();
        if entry.enclosed_name().as_deref() != Some(Path::new(&name))
            || !seen.insert(name.clone())
            || (name != "lecture.json" && !valid_audio_entry(&name))
        {
            return Err("UNSAFE_ARCHIVE_PATH".into());
        }
        let limit = if name == "lecture.json" {
            MAX_MANIFEST_BYTES
        } else {
            MAX_AUDIO_ENTRY_BYTES
        };
        if entry.size() > limit {
            return Err("ARCHIVE_ENTRY_TOO_LARGE".into());
        }
        total = total.checked_add(entry.size()).ok_or("ARCHIVE_TOO_LARGE")?;
        if total > MAX_TOTAL_BYTES {
            return Err("ARCHIVE_TOO_LARGE".into());
        }
    }

    let mut manifest_entry = zip
        .by_name("lecture.json")
        .map_err(|_| "MISSING_MANIFEST".to_string())?;
    let mut manifest = Vec::with_capacity(manifest_entry.size().min(1024 * 1024) as usize);
    manifest_entry
        .by_ref()
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut manifest)
        .map_err(io_error)?;
    if manifest.len() as u64 > MAX_MANIFEST_BYTES {
        return Err("MANIFEST_TOO_LARGE".into());
    }
    drop(manifest_entry);
    let mut package: LecturePackage = serde_json::from_slice(&manifest).map_err(json_error)?;
    if package.schema_version != 1 {
        return Err("UNSUPPORTED_ARCHIVE".into());
    }
    package.session.project_id = None;
    validate_session(&package.session)?;
    package.session.runs.iter().try_fold(0_u64, |total, run| {
        run.samples
            .checked_mul(2)
            .filter(|bytes| *bytes <= MAX_AUDIO_ENTRY_BYTES)
            .and_then(|bytes| total.checked_add(bytes))
            .filter(|total| *total <= MAX_TOTAL_BYTES)
            .ok_or("ARCHIVE_TOO_LARGE")
    })?;

    let old_id = package.session.id.clone();
    let new_id = Uuid::new_v4().to_string();
    let temporary = audio_root.join(format!(".import-{}", Uuid::new_v4()));
    let destination = audio_root.join(&new_id);
    if destination.exists() {
        return Err("SESSION_AUDIO_EXISTS".into());
    }
    fs::create_dir_all(&temporary).map_err(io_error)?;
    let extracted = match extract_audio(&mut zip, &package.session, &temporary) {
        Ok(extracted) => extracted,
        Err(error) => {
            let _ = fs::remove_dir_all(&temporary);
            return Err(error);
        }
    };
    if package.session.mode == "live"
        && package
            .session
            .runs
            .iter()
            .any(|run| !extracted.contains(run.id.as_str()))
    {
        let _ = fs::remove_dir_all(&temporary);
        return Err("MISSING_AUDIO_FILE".into());
    }
    fs::create_dir_all(audio_root).map_err(io_error)?;
    if let Err(error) = fs::rename(&temporary, &destination) {
        let _ = fs::remove_dir_all(&temporary);
        return Err(io_error(error));
    }
    let mut session = package.session;
    session.id = new_id;
    session.project_id = None;
    session.recording_state = "stopped".into();
    session.inference_state = "ready".into();
    for run in &mut session.runs {
        if matches!(run.state.as_str(), "recording" | "starting") {
            run.state = "closed".into();
            run.ended_at = Some(run.ended_at.unwrap_or(run.started_at));
        }
    }
    if session.error.as_deref() == Some("imported") {
        session.error = None;
    }
    debug_assert_ne!(session.id, old_id);
    Ok(session)
}

fn extract_audio(
    zip: &mut ZipArchive<File>,
    session: &Session,
    temporary: &Path,
) -> Result<HashSet<String>, String> {
    let expected: HashSet<&str> = session.runs.iter().map(|run| run.id.as_str()).collect();
    let mut extracted = HashSet::new();
    let mut written = 0_u64;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(zip_error)?;
        let name = entry.name().to_string();
        if name == "lecture.json" {
            continue;
        }
        let run_id = name
            .strip_prefix("audio/")
            .and_then(|name| name.strip_suffix(".pcm"))
            .ok_or("UNSAFE_ARCHIVE_PATH")?;
        if !expected.contains(run_id) {
            return Err("UNKNOWN_AUDIO_RUN".into());
        }
        let run = session.runs.iter().find(|run| run.id == run_id).unwrap();
        let expected_bytes = run.samples.checked_mul(2).ok_or("AUDIO_TOO_LARGE")?;
        // The central directory's size is attacker-controlled; bound what is actually inflated.
        if entry.size() != expected_bytes || expected_bytes > MAX_AUDIO_ENTRY_BYTES {
            return Err("AUDIO_LENGTH_MISMATCH".into());
        }
        let target = temporary.join(format!("{run_id}.pcm"));
        let mut output = File::create(target).map_err(io_error)?;
        let copied = std::io::copy(&mut entry.by_ref().take(expected_bytes + 1), &mut output)
            .map_err(io_error)?;
        written = written.checked_add(copied).ok_or("ARCHIVE_TOO_LARGE")?;
        if written > MAX_TOTAL_BYTES {
            return Err("ARCHIVE_TOO_LARGE".into());
        }
        if copied != expected_bytes || copied % 2 != 0 {
            return Err("AUDIO_LENGTH_MISMATCH".into());
        }
        output.sync_all().map_err(io_error)?;
        extracted.insert(run_id.to_string());
    }
    Ok(extracted)
}

fn valid_audio_entry(name: &str) -> bool {
    name.strip_prefix("audio/")
        .and_then(|name| name.strip_suffix(".pcm"))
        .is_some_and(safe_id)
}

fn html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn io_error(error: std::io::Error) -> String {
    format!("IO: {error}")
}

fn zip_error(error: zip::result::ZipError) -> String {
    format!("ZIP: {error}")
}

fn json_error(error: serde_json::Error) -> String {
    format!("JSON: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{MachineHypothesis, Run, Segment, Session};

    fn fixture() -> Session {
        let mut session = Session::new("Language & Motion".into(), "live".into());
        session.runs.push(Run {
            id: "run-1".into(),
            engine: "qwen".into(),
            source: "microphone".into(),
            started_at: 1,
            ended_at: Some(2),
            samples: 4,
            offset_ms: 0,
            state: "complete".into(),
        });
        session.segments.push(Segment {
            id: "segment-1".into(),
            run_id: "run-1".into(),
            start_sample: 0,
            end_sample: 4,
            machine_text: "Talmy <term>".into(),
            machine_revision: 1,
            final_: true,
            worker_epoch: 0,
            user_seq: 0,
            display_text: "Talmy <term>".into(),
            pending_machine: None,
            corrected: false,
            history: Vec::new(),
            history_index: -1,
            machine_history: vec![MachineHypothesis {
                revision: 1,
                worker_epoch: 0,
                text: "Talmy <term>".into(),
                final_: true,
            }],
        });
        session
    }

    #[test]
    fn html_escapes_untrusted_content() {
        let html = export_html(&fixture());
        assert!(html.contains("Language &amp; Motion"));
        assert!(html.contains("Talmy &lt;term&gt;"));
        assert!(!html.contains("<term>"));
    }

    #[test]
    fn lecture_round_trip_remaps_id_and_copies_audio() {
        let root = std::env::temp_dir().join(format!("lectureedit-archive-{}", Uuid::new_v4()));
        let source_audio = root.join("source");
        let imported_audio = root.join("imported");
        let mut session = fixture();
        session.project_id = Some("project-economics".into());
        fs::create_dir_all(source_audio.join(&session.id)).unwrap();
        fs::write(
            source_audio.join(&session.id).join("run-1.pcm"),
            [1, 2, 3, 4, 5, 6, 7, 8],
        )
        .unwrap();
        let archive = root.join("course.lecture");
        export_lecture(&session, &source_audio, &archive).unwrap();
        {
            let mut zip = ZipArchive::new(File::open(&archive).unwrap()).unwrap();
            let mut manifest = String::new();
            zip.by_name("lecture.json")
                .unwrap()
                .read_to_string(&mut manifest)
                .unwrap();
            let package: LecturePackage = serde_json::from_str(&manifest).unwrap();
            assert_eq!(package.session.project_id, None);
        }
        let imported = import_lecture(&archive, &imported_audio).unwrap();
        assert_ne!(imported.id, session.id);
        assert_eq!(imported.project_id, None);
        assert_eq!(
            fs::read(imported_audio.join(&imported.id).join("run-1.pcm")).unwrap(),
            [1, 2, 3, 4, 5, 6, 7, 8]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_package_without_project_membership_imports_ungrouped() {
        let root = std::env::temp_dir().join(format!("lectureedit-legacy-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let archive = root.join("legacy.lecture");
        let mut session = fixture();
        session.mode = "demo".into();
        session.runs.clear();
        let mut manifest = serde_json::to_value(LecturePackage {
            schema_version: 1,
            session,
        })
        .unwrap();
        manifest["session"]
            .as_object_mut()
            .unwrap()
            .remove("projectId");
        let file = File::create(&archive).unwrap();
        let mut writer = ZipWriter::new(file);
        writer
            .start_file("lecture.json", SimpleFileOptions::default())
            .unwrap();
        writer
            .write_all(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        writer.finish().unwrap();

        let imported = import_lecture(&archive, &root.join("audio")).unwrap();
        assert_eq!(imported.project_id, None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn import_rejects_parent_traversal() {
        let root = std::env::temp_dir().join(format!("lectureedit-unsafe-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let archive = root.join("unsafe.lecture");
        let file = File::create(&archive).unwrap();
        let mut writer = ZipWriter::new(file);
        writer
            .start_file("../escape.pcm", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"bad").unwrap();
        writer.finish().unwrap();
        assert_eq!(
            import_lecture(&archive, &root.join("audio")).unwrap_err(),
            "UNSAFE_ARCHIVE_PATH"
        );
        assert!(!root.join("escape.pcm").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn live_export_requires_complete_even_pcm() {
        let root = std::env::temp_dir().join(format!("lectureedit-missing-{}", Uuid::new_v4()));
        let session = fixture();
        fs::create_dir_all(root.join("audio").join(&session.id)).unwrap();
        let archive = root.join("course.lecture");
        assert_eq!(
            export_lecture(&session, &root.join("audio"), &archive).unwrap_err(),
            "MISSING_AUDIO_FILE"
        );
        assert!(!archive.exists());
        fs::write(
            root.join("audio").join(&session.id).join("run-1.pcm"),
            [1, 2, 3],
        )
        .unwrap();
        assert_eq!(
            export_lecture(&session, &root.join("audio"), &archive).unwrap_err(),
            "AUDIO_LENGTH_MISMATCH"
        );
        assert!(!archive.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn live_import_rejects_missing_audio_and_cleans_temporary_directory() {
        let root = std::env::temp_dir().join(format!("lectureedit-incomplete-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let archive = root.join("incomplete.lecture");
        let file = File::create(&archive).unwrap();
        let mut writer = ZipWriter::new(file);
        writer
            .start_file("lecture.json", SimpleFileOptions::default())
            .unwrap();
        writer
            .write_all(
                &serde_json::to_vec(&LecturePackage {
                    schema_version: 1,
                    session: fixture(),
                })
                .unwrap(),
            )
            .unwrap();
        writer.finish().unwrap();
        let audio = root.join("audio");
        assert_eq!(
            import_lecture(&archive, &audio).unwrap_err(),
            "MISSING_AUDIO_FILE"
        );
        assert_eq!(fs::read_dir(&audio).unwrap().count(), 0);
        let _ = fs::remove_dir_all(root);
    }

    fn package_with_audio(root: &Path, session: Session, audio: &[u8]) -> std::path::PathBuf {
        fs::create_dir_all(root).unwrap();
        let archive = root.join("package.lecture");
        let mut writer = ZipWriter::new(File::create(&archive).unwrap());
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        writer.start_file("lecture.json", options).unwrap();
        writer
            .write_all(
                &serde_json::to_vec(&LecturePackage {
                    schema_version: 1,
                    session,
                })
                .unwrap(),
            )
            .unwrap();
        writer.start_file("audio/run-1.pcm", options).unwrap();
        writer.write_all(audio).unwrap();
        writer.finish().unwrap();
        archive
    }

    #[test]
    fn import_rejects_audio_larger_than_the_manifest_before_inflating_it() {
        let root = std::env::temp_dir().join(format!("lectureedit-bomb-{}", Uuid::new_v4()));
        let archive = package_with_audio(&root, fixture(), &vec![0; 4 * 1024 * 1024]);
        let audio = root.join("audio");
        assert_eq!(
            import_lecture(&archive, &audio).unwrap_err(),
            "AUDIO_LENGTH_MISMATCH"
        );
        assert_eq!(fs::read_dir(&audio).unwrap().count(), 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn import_rejects_declared_audio_beyond_the_total_budget() {
        let root = std::env::temp_dir().join(format!("lectureedit-budget-{}", Uuid::new_v4()));
        let mut session = fixture();
        session.runs[0].samples = MAX_AUDIO_ENTRY_BYTES;
        let archive = package_with_audio(&root, session, &[0; 8]);
        assert_eq!(
            import_lecture(&archive, &root.join("audio")).unwrap_err(),
            "ARCHIVE_TOO_LARGE"
        );
        assert!(!root.join("audio").exists());
        let _ = fs::remove_dir_all(root);
    }
}

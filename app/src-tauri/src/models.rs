use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};
const REPO:&str="https://huggingface.co/ggml-org/Qwen3-ASR-0.6B-GGUF/resolve/928ab958557df9aa2ef1c93e0e83c7ad0933fae2";
const FILES: [(&str, u64, &str); 2] = [
    (
        "Qwen3-ASR-0.6B-Q8_0.gguf",
        804749248,
        "bca259818b50ca7c4c05e9bdb35a5dc04fa039653a6d6f3f0f331f96f6aa1971",
    ),
    (
        "mmproj-Qwen3-ASR-0.6B-Q8_0.gguf",
        214392480,
        "41a342b5e4c514e968cb756de6cd1b7be39eff43c44c57a2ef5fc6522e36603d",
    ),
];

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallProgress {
    pub phase: &'static str,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub file_name: Option<String>,
    pub error: Option<String>,
}

struct FetchResponse {
    status: u16,
    content_range: Option<String>,
    body: Box<dyn Read + Send>,
}

pub fn install_total_bytes() -> u64 {
    FILES.iter().map(|(_, size, _)| size).sum()
}

pub fn uses_download_manifest(model_path: &Path, mmproj_path: &Path) -> bool {
    model_path
        .file_name()
        .is_some_and(|name| name == FILES[0].0)
        && mmproj_path
            .file_name()
            .is_some_and(|name| name == FILES[1].0)
}

pub fn downloaded_files_verified(model_path: &Path, mmproj_path: &Path) -> bool {
    verified(model_path, FILES[0].1, FILES[0].2).unwrap_or(false)
        && verified(mmproj_path, FILES[1].1, FILES[1].2).unwrap_or(false)
}

pub fn downloaded_file_sizes_match(model_path: &Path, mmproj_path: &Path) -> bool {
    model_path
        .metadata()
        .is_ok_and(|value| value.len() == FILES[0].1)
        && mmproj_path
            .metadata()
            .is_ok_and(|value| value.len() == FILES[1].1)
}

pub fn defaults(root: &Path, resources: &Path) -> Value {
    // Developer cache only; without HOME (typical on Windows) fall back to the app's own root
    // rather than a path relative to whatever the working directory happens to be.
    let cache = std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".cache/lectureedit"))
        .unwrap_or_else(|| root.to_path_buf());
    let exe_name = if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    };
    let candidates = [
        resources.join("native/qwen").join(exe_name),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources/native/qwen")
            .join(exe_name),
        cache.join("builds/llama.cpp/bin").join(exe_name),
    ];
    let executable = candidates
        .iter()
        .find(|p| p.is_file())
        .cloned()
        .unwrap_or_else(|| candidates[0].clone());
    let cached = cache.join("models/qwen3-asr-0.6b-q8");
    let model_dir = if FILES.iter().all(|(n, _, _)| cached.join(n).is_file()) {
        cached
    } else {
        root.join("models/qwen3-asr-0.6b-q8")
    };
    json!({"engine":"qwen","executable":executable,"modelPath":model_dir.join(FILES[0].0),"mmprojPath":model_dir.join(FILES[1].0),"language":"auto"})
}
fn verified(path: &Path, size: u64, hash: &str) -> Result<bool, String> {
    if !path.is_file() {
        return Ok(false);
    }
    let mut f = File::open(path).map_err(|e| e.to_string())?;
    let metadata = f.metadata().map_err(|e| e.to_string())?;
    if metadata.len() != size {
        return Ok(false);
    }
    // Hashing ~1 GB on every launch competes with model loading; a stamp keyed on the expected hash,
    // size and modification time lets unchanged files skip it. Any rewrite changes the mtime.
    let stamp_path = verification_stamp_path(path);
    let stamp = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|age| format!("{hash} {size} {}", age.as_nanos()));
    if stamp.is_some() && fs::read_to_string(&stamp_path).ok() == stamp {
        return Ok(true);
    }
    let mut digest = Sha256::new();
    let mut buf = vec![0; 1024 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        digest.update(&buf[..n]);
    }
    let matches = format!("{:x}", digest.finalize()) == hash;
    if let (true, Some(stamp)) = (matches, stamp) {
        let _ = fs::write(&stamp_path, stamp);
    }
    Ok(matches)
}

fn verification_stamp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".verified");
    path.with_file_name(name)
}

fn content_range_matches(value: Option<&str>, start: u64, total: u64) -> bool {
    let Some(value) = value else { return false };
    let Some(value) = value.strip_prefix("bytes ") else {
        return false;
    };
    let Some((range, declared_total)) = value.split_once('/') else {
        return false;
    };
    let Some((declared_start, declared_end)) = range.split_once('-') else {
        return false;
    };
    matches!(
        (
            declared_start.parse::<u64>(),
            declared_end.parse::<u64>(),
            declared_total.parse::<u64>(),
        ),
        (Ok(actual_start), Ok(end), Ok(actual_total))
            if actual_start == start && end >= actual_start && end < total && actual_total == total
    )
}

fn partial_len(path: &Path, expected_size: u64) -> Result<u64, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.file_type().is_symlink() {
        fs::remove_file(path).map_err(|error| error.to_string())?;
        return Ok(0);
    }
    if !metadata.is_file() {
        return Err("模型临时下载路径不是普通文件".into());
    }
    if metadata.len() > expected_size {
        fs::remove_file(path).map_err(|error| error.to_string())?;
        return Ok(0);
    }
    Ok(metadata.len())
}

fn install_manifest<F, P>(
    directory: &Path,
    files: &[(&str, u64, &str)],
    mut fetch: F,
    on_progress: &mut P,
) -> Result<(), String>
where
    F: FnMut(&str, Option<u64>) -> Result<FetchResponse, String>,
    P: FnMut(InstallProgress),
{
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let total_bytes = files.iter().map(|(_, size, _)| size).sum();
    let mut completed_bytes = 0u64;
    for &(name, size, expected_hash) in files {
        let target = directory.join(name);
        on_progress(InstallProgress {
            phase: "verifying",
            downloaded_bytes: completed_bytes,
            total_bytes,
            file_name: Some(name.to_owned()),
            error: None,
        });
        if verified(&target, size, expected_hash)? {
            completed_bytes += size;
            let _ = fs::remove_file(directory.join(format!("{name}.download")));
            on_progress(InstallProgress {
                phase: "verifying",
                downloaded_bytes: completed_bytes,
                total_bytes,
                file_name: Some(name.to_owned()),
                error: None,
            });
            continue;
        }

        let part = directory.join(format!("{name}.download"));
        let mut offset = partial_len(&part, size)?;
        if offset == size && !verified(&part, size, expected_hash)? {
            OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&part)
                .map_err(|error| error.to_string())?;
            offset = 0;
        }
        on_progress(InstallProgress {
            phase: "downloading",
            downloaded_bytes: completed_bytes + offset,
            total_bytes,
            file_name: Some(name.to_owned()),
            error: None,
        });

        if offset < size {
            let mut response = fetch(name, (offset > 0).then_some(offset))?;
            let append = if offset > 0 && response.status == 206 {
                if !content_range_matches(response.content_range.as_deref(), offset, size) {
                    return Err("模型续传响应范围无效；已保留现有下载进度".into());
                }
                true
            } else if response.status == 200 {
                offset = 0;
                false
            } else if offset == 0
                && response.status == 206
                && content_range_matches(response.content_range.as_deref(), 0, size)
            {
                true
            } else {
                return Err(format!("模型下载服务返回状态 {}", response.status));
            };
            let mut output = OpenOptions::new()
                .write(true)
                .create(true)
                .append(append)
                .truncate(!append)
                .open(&part)
                .map_err(|error| format!("模型保存失败：{error}"))?;
            let mut written = offset;
            let mut buffer = vec![0; 1024 * 1024];
            loop {
                let count = response
                    .body
                    .read(&mut buffer)
                    .map_err(|error| format!("模型下载中断：{error}"))?;
                if count == 0 {
                    break;
                }
                let next = written
                    .checked_add(count as u64)
                    .ok_or("模型文件大小与清单不符")?;
                if next > size {
                    return Err("模型文件大小与清单不符".into());
                }
                output
                    .write_all(&buffer[..count])
                    .map_err(|error| format!("模型保存失败：{error}"))?;
                written = next;
                on_progress(InstallProgress {
                    phase: "downloading",
                    downloaded_bytes: completed_bytes + written,
                    total_bytes,
                    file_name: Some(name.to_owned()),
                    error: None,
                });
            }
            output.sync_all().map_err(|error| error.to_string())?;
            if written != size {
                return Err("模型下载尚未完成；已保留进度，可重新下载继续".into());
            }
        }

        on_progress(InstallProgress {
            phase: "verifying",
            downloaded_bytes: completed_bytes + size,
            total_bytes,
            file_name: Some(name.to_owned()),
            error: None,
        });
        if !verified(&part, size, expected_hash)? {
            return Err("模型文件校验失败；已保留下载文件，重试时会安全重下".into());
        }
        #[cfg(windows)]
        if fs::symlink_metadata(&target).is_ok() {
            fs::remove_file(&target).map_err(|error| error.to_string())?;
        }
        fs::rename(&part, &target).map_err(|error| error.to_string())?;
        #[cfg(unix)]
        File::open(directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string())?;
        completed_bytes += size;
    }
    on_progress(InstallProgress {
        phase: "complete",
        downloaded_bytes: total_bytes,
        total_bytes,
        file_name: None,
        error: None,
    });
    Ok(())
}

pub fn install<P>(root: &Path, resources: &Path, mut on_progress: P) -> Result<Value, String>
where
    P: FnMut(InstallProgress),
{
    let mut settings = defaults(root, resources);
    let total_bytes = install_total_bytes();
    let configured_paths = [
        PathBuf::from(settings["modelPath"].as_str().unwrap_or("")),
        PathBuf::from(settings["mmprojPath"].as_str().unwrap_or("")),
    ];
    let directory = root.join("models/qwen3-asr-0.6b-q8");
    if configured_paths
        .iter()
        .all(|path| path.parent().is_some_and(|parent| parent != directory))
    {
        let mut all_verified = true;
        let mut verified_bytes = 0;
        for (index, path) in configured_paths.iter().enumerate() {
            let (name, size, hash) = FILES[index];
            on_progress(InstallProgress {
                phase: "verifying",
                downloaded_bytes: verified_bytes,
                total_bytes,
                file_name: Some(name.to_owned()),
                error: None,
            });
            if verified(path, size, hash)? {
                verified_bytes += size;
                on_progress(InstallProgress {
                    phase: "verifying",
                    downloaded_bytes: verified_bytes,
                    total_bytes,
                    file_name: Some(name.to_owned()),
                    error: None,
                });
            } else {
                all_verified = false;
            }
        }
        if all_verified {
            on_progress(InstallProgress {
                phase: "complete",
                downloaded_bytes: total_bytes,
                total_bytes,
                file_name: None,
                error: None,
            });
            return Ok(settings);
        }
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3600))
        .redirect(reqwest::redirect::Policy::custom(|a| {
            let host = a.url().host_str().unwrap_or("");
            if a.previous().len() > 5
                || a.url().scheme() != "https"
                || !(host == "huggingface.co"
                    || host.ends_with(".huggingface.co")
                    || host.ends_with(".hf.co"))
            {
                a.error("模型下载重定向来源无效")
            } else {
                a.follow()
            }
        }))
        .build()
        .map_err(|e| e.to_string())?;
    install_manifest(
        &directory,
        &FILES,
        |name, range_start| {
            let mut request = client.get(format!("{REPO}/{name}"));
            if let Some(start) = range_start {
                request = request.header(reqwest::header::RANGE, format!("bytes={start}-"));
            }
            let response = request
                .send()
                .map_err(|error| format!("模型下载连接失败：{error}"))?;
            let status = response.status();
            if !status.is_success() {
                return Err(format!("模型下载服务返回状态 {status}"));
            }
            let content_range = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            Ok(FetchResponse {
                status: status.as_u16(),
                content_range,
                body: Box::new(response),
            })
        },
        &mut on_progress,
    )?;
    settings["modelPath"] = json!(directory.join(FILES[0].0));
    settings["mmprojPath"] = json!(directory.join(FILES[1].0));
    Ok(settings)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::VecDeque,
        io::{self, Cursor},
        sync::{Arc, Mutex},
    };

    fn hash(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn response(status: u16, content_range: Option<&str>, bytes: &[u8]) -> FetchResponse {
        FetchResponse {
            status,
            content_range: content_range.map(str::to_owned),
            body: Box::new(Cursor::new(bytes.to_vec())),
        }
    }

    struct InterruptedReader {
        bytes: Cursor<Vec<u8>>,
        failed: bool,
    }

    impl Read for InterruptedReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.failed {
                return Err(io::Error::new(io::ErrorKind::ConnectionReset, "offline"));
            }
            let capacity = buffer.len().min(3);
            let count = self.bytes.read(&mut buffer[..capacity])?;
            if count == 0 {
                self.failed = true;
                return Err(io::Error::new(io::ErrorKind::ConnectionReset, "offline"));
            }
            Ok(count)
        }
    }

    #[test]
    fn known_hash_and_size_required() {
        let root = tempfile::tempdir().unwrap();
        let p = root.path().join("x");
        fs::write(&p, b"abc").unwrap();
        let hash = format!("{:x}", Sha256::digest(b"abc"));
        assert!(verified(&p, 3, &hash).unwrap());
        assert!(!verified(&p, 2, &hash).unwrap());
        assert!(!verified(&p, 3, "incorrect").unwrap());
    }

    #[test]
    fn resumes_from_a_valid_partial_with_strict_content_range() {
        let root = tempfile::tempdir().unwrap();
        let bytes = b"abcdefghij";
        let expected_hash = hash(bytes);
        let manifest = [("model.bin", bytes.len() as u64, expected_hash.as_str())];
        fs::write(root.path().join("model.bin.download"), &bytes[..4]).unwrap();
        let requested = Arc::new(Mutex::new(Vec::new()));
        let observed = requested.clone();
        let mut progress = Vec::new();

        install_manifest(
            root.path(),
            &manifest,
            move |name, start| {
                observed.lock().unwrap().push((name.to_owned(), start));
                Ok(response(206, Some("bytes 4-9/10"), &bytes[4..]))
            },
            &mut |event| progress.push(event),
        )
        .unwrap();

        assert_eq!(
            *requested.lock().unwrap(),
            vec![("model.bin".into(), Some(4))]
        );
        assert_eq!(fs::read(root.path().join("model.bin")).unwrap(), bytes);
        assert!(!root.path().join("model.bin.download").exists());
        assert_eq!(progress.last().unwrap().phase, "complete");
        assert_eq!(progress.last().unwrap().downloaded_bytes, 10);
    }

    #[test]
    fn range_ignored_by_server_truncates_before_restarting() {
        let root = tempfile::tempdir().unwrap();
        let bytes = b"abcdefghij";
        let expected_hash = hash(bytes);
        let manifest = [("model.bin", bytes.len() as u64, expected_hash.as_str())];
        fs::write(root.path().join("model.bin.download"), b"abcd").unwrap();

        install_manifest(
            root.path(),
            &manifest,
            |_name, start| {
                assert_eq!(start, Some(4));
                Ok(response(200, None, bytes))
            },
            &mut |_| {},
        )
        .unwrap();

        assert_eq!(fs::read(root.path().join("model.bin")).unwrap(), bytes);
    }

    #[test]
    fn invalid_content_range_keeps_the_original_partial() {
        let root = tempfile::tempdir().unwrap();
        let bytes = b"abcdefghij";
        let expected_hash = hash(bytes);
        let manifest = [("model.bin", bytes.len() as u64, expected_hash.as_str())];
        let part = root.path().join("model.bin.download");
        fs::write(&part, b"abcd").unwrap();

        let error = install_manifest(
            root.path(),
            &manifest,
            |_name, start| {
                assert_eq!(start, Some(4));
                Ok(response(206, Some("bytes 3-9/10"), &bytes[4..]))
            },
            &mut |_| {},
        )
        .unwrap_err();

        assert!(error.contains("范围无效"));
        assert_eq!(fs::read(part).unwrap(), b"abcd");
    }

    #[test]
    fn interrupted_download_is_preserved_and_the_next_attempt_resumes() {
        let root = tempfile::tempdir().unwrap();
        let bytes = b"abcdefghij";
        let expected_hash = hash(bytes);
        let manifest = [("model.bin", bytes.len() as u64, expected_hash.as_str())];
        let attempts = Arc::new(Mutex::new(VecDeque::from([0u8, 1u8])));

        let first_attempts = attempts.clone();
        let error = install_manifest(
            root.path(),
            &manifest,
            move |_name, start| {
                assert_eq!(start, None);
                assert_eq!(first_attempts.lock().unwrap().pop_front(), Some(0));
                Ok(FetchResponse {
                    status: 200,
                    content_range: None,
                    body: Box::new(InterruptedReader {
                        bytes: Cursor::new(bytes[..6].to_vec()),
                        failed: false,
                    }),
                })
            },
            &mut |_| {},
        )
        .unwrap_err();
        assert!(error.contains("中断"));
        assert_eq!(
            fs::read(root.path().join("model.bin.download")).unwrap(),
            &bytes[..6]
        );

        let second_attempts = attempts.clone();
        install_manifest(
            root.path(),
            &manifest,
            move |_name, start| {
                assert_eq!(start, Some(6));
                assert_eq!(second_attempts.lock().unwrap().pop_front(), Some(1));
                Ok(response(206, Some("bytes 6-9/10"), &bytes[6..]))
            },
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(fs::read(root.path().join("model.bin")).unwrap(), bytes);
    }

    #[test]
    fn verification_stamp_skips_rehash_until_the_file_changes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("model.bin");
        fs::write(&path, b"weights").unwrap();
        let expected = hash(b"weights");
        assert!(verified(&path, 7, &expected).unwrap());
        assert!(verification_stamp_path(&path).is_file());
        assert!(verified(&path, 7, &expected).unwrap());
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&path, b"WEIGHTS").unwrap();
        assert!(!verified(&path, 7, &expected).unwrap());
    }

    #[test]
    fn verified_existing_files_contribute_to_total_progress() {
        let root = tempfile::tempdir().unwrap();
        let first = b"first";
        let second = b"second";
        let first_hash = hash(first);
        let second_hash = hash(second);
        let manifest = [
            ("first.bin", first.len() as u64, first_hash.as_str()),
            ("second.bin", second.len() as u64, second_hash.as_str()),
        ];
        fs::write(root.path().join("first.bin"), first).unwrap();
        let mut progress = Vec::new();

        install_manifest(
            root.path(),
            &manifest,
            |name, start| {
                assert_eq!(name, "second.bin");
                assert_eq!(start, None);
                Ok(response(200, None, second))
            },
            &mut |event| progress.push(event),
        )
        .unwrap();

        assert!(progress
            .iter()
            .any(|event| event.downloaded_bytes == first.len() as u64));
        assert_eq!(progress.last().unwrap().downloaded_bytes, 11);
        assert_eq!(progress.last().unwrap().total_bytes, 11);
    }

    #[test]
    fn full_size_bad_hash_partial_is_restarted_on_explicit_retry() {
        let root = tempfile::tempdir().unwrap();
        let bytes = b"abcdefghij";
        let expected_hash = hash(bytes);
        let manifest = [("model.bin", bytes.len() as u64, expected_hash.as_str())];
        fs::write(root.path().join("model.bin.download"), b"0123456789").unwrap();

        install_manifest(
            root.path(),
            &manifest,
            |_name, start| {
                assert_eq!(start, None);
                Ok(response(200, None, bytes))
            },
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(fs::read(root.path().join("model.bin")).unwrap(), bytes);
    }

    #[test]
    fn oversized_body_is_rejected_without_replacing_an_existing_target() {
        let root = tempfile::tempdir().unwrap();
        let bytes = b"abcdefghij";
        let expected_hash = hash(bytes);
        let manifest = [("model.bin", bytes.len() as u64, expected_hash.as_str())];
        let target = root.path().join("model.bin");
        fs::write(&target, b"old-valid-user-file").unwrap();

        let error = install_manifest(
            root.path(),
            &manifest,
            |_name, _start| Ok(response(200, None, b"abcdefghijk")),
            &mut |_| {},
        )
        .unwrap_err();

        assert!(error.contains("大小"));
        assert_eq!(fs::read(target).unwrap(), b"old-valid-user-file");
    }
}

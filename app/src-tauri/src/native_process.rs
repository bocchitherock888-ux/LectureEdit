//! Native sidecars keep their pipes while avoiding an extra Windows console.
use std::ffi::OsStr;
use std::process::{Child, Command};

pub(crate) fn command(program: impl AsRef<OsStr>) -> Command {
    let mut process = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        process.creation_flags(CREATE_NO_WINDOW);
    }
    // Keep one construction path across targets; Windows sets creation flags.
    let _ = &mut process;
    process
}

/// Spawns a sidecar that cannot outlive the app. On Windows a crash or Task Manager kill would
/// otherwise orphan llama-server.exe, which keeps ~1 GB of RAM and locks files the installer replaces.
pub(crate) trait SpawnTied {
    fn spawn_tied(&mut self) -> std::io::Result<Child>;
}
impl SpawnTied for Command {
    fn spawn_tied(&mut self) -> std::io::Result<Child> {
        let child = self.spawn()?;
        #[cfg(windows)]
        job::assign(&child);
        Ok(child)
    }
}
#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use std::sync::OnceLock;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    // The handle is never closed; Windows closes it when the app exits, which kills the job.
    static JOB: OnceLock<usize> = OnceLock::new();
    pub(super) fn assign(child: &Child) {
        let job = *JOB.get_or_init(|| unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return 0;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            job as usize
        });
        if job != 0 {
            // Best effort: failure only loses orphan cleanup, never the sidecar itself.
            unsafe { AssignProcessToJobObject(job as _, child.as_raw_handle() as _) };
        }
    }
}

/// llama-server placement flags. The GPU run offloads everything and lets llama.cpp pick the
/// device (Metal on macOS; Vulkan on Windows when a driver provides it, otherwise the CPU).
/// The CPU run keeps the model and the audio encoder off every GPU backend.
pub(crate) const fn qwen_device_args(gpu: bool) -> &'static [&'static str] {
    if gpu {
        // On Windows, Vulkan "pinned" host buffers are used as CPU memory, and llama.cpp aborts when a
        // driver maps them at less than 32-byte alignment (Mesa's software device does). Integrated
        // graphics share memory with the CPU anyway, so skipping them costs little.
        if cfg!(windows) {
            &["-ngl", "99", "--no-host"]
        } else {
            &["-ngl", "99"]
        }
    } else {
        &["-ngl", "0", "-dev", "none", "--no-mmproj-offload"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_paths_remain_single_arguments() {
        let process = command("C:\\Program Files\\随堂\\native\\qwen\\llama-server.exe");
        assert_eq!(process.get_args().count(), 0);
        assert!(process.get_program().to_string_lossy().ends_with("llama-server.exe"));
    }

    #[cfg(windows)]
    #[test]
    fn hidden_windows_process_preserves_stdout_pipe() {
        let output = command("cmd.exe")
            .args(["/d", "/c", "echo lectureedit-pipe-ok"])
            .output()
            .expect("spawn a hidden test process");
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("lectureedit-pipe-ok"));
    }

    /// Every flag the app passes must exist in the packaged llama-server: an unknown one makes
    /// the GPU start fail and silently sends everyone to the CPU. Skipped when not yet built.
    #[test]
    fn the_packaged_server_accepts_every_placement_flag() {
        let name = if cfg!(windows) { "llama-server.exe" } else { "llama-server" };
        let server = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/native/qwen").join(name);
        if !server.is_file() {
            return;
        }
        let help = command(&server).arg("--help").output().expect("run llama-server --help");
        let help = String::from_utf8_lossy(&help.stdout).into_owned() + &String::from_utf8_lossy(&help.stderr);
        // Both platforms' placement flags, plus the ones Model::load always passes.
        let fixed = ["--no-host", "--mmproj", "--host", "--port", "--no-webui", "--jinja", "-c", "-np", "--cache-ram"];
        for flag in qwen_device_args(true).iter().chain(qwen_device_args(false)).chain(&fixed).filter(|arg| arg.starts_with('-')) {
            assert!(help.contains(flag), "llama-server does not know {flag}");
        }
    }

    #[test]
    fn cpu_fallback_keeps_every_part_of_the_model_off_the_gpu() {
        assert_eq!(&qwen_device_args(true)[..2], ["-ngl", "99"]);
        assert_eq!(qwen_device_args(true).contains(&"--no-host"), cfg!(windows));
        let cpu = qwen_device_args(false);
        assert!(cpu.windows(2).any(|pair| pair == ["-ngl", "0"]));
        assert!(cpu.windows(2).any(|pair| pair == ["-dev", "none"]));
        assert!(cpu.contains(&"--no-mmproj-offload"));
    }
}

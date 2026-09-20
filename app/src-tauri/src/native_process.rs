//! Native sidecars keep their pipes while avoiding an extra Windows console.
use std::ffi::OsStr;
use std::process::Command;

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

pub(crate) const fn qwen_gpu_layers() -> &'static str {
    if cfg!(windows) { "0" } else { "99" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_paths_remain_single_arguments() {
        let process = command("C:\\Program Files\\LectureEdit\\native\\qwen\\llama-server.exe");
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

    #[test]
    fn windows_release_uses_cpu_and_macos_keeps_its_acceleration_setting() {
        assert_eq!(qwen_gpu_layers(), if cfg!(windows) { "0" } else { "99" });
    }
}

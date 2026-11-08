//! Process trees. Killing a shell leaves the children it started running, so a `ProcessGroup` ends the
//! whole tree at once: a Job Object on Windows, a process group elsewhere.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::io;
    use std::mem::{size_of, zeroed};
    use std::ptr::null;

    use tokio::process::Child;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
        TerminateJobObject,
    };

    /// A Job Object. Closing its last handle kills every process still in it.
    pub struct ProcessGroup(*mut c_void);

    // The handle is only used through the job functions, and closed once, on drop.
    unsafe impl Send for ProcessGroup {}
    unsafe impl Sync for ProcessGroup {}

    impl ProcessGroup {
        pub fn new() -> io::Result<ProcessGroup> {
            unsafe {
                let job = CreateJobObjectW(null(), null());
                if job.is_null() {
                    return Err(io::Error::last_os_error());
                }
                // Dropped on the error paths below, which closes the handle.
                let group = ProcessGroup(job);
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let set = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if set == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(group)
            }
        }

        pub fn adopt(&self, child: &Child) -> io::Result<()> {
            let process = child.raw_handle().ok_or_else(|| io::Error::other("the child has no handle"))?;
            if unsafe { AssignProcessToJobObject(self.0, process) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        pub fn kill_all(&self) -> io::Result<()> {
            if unsafe { TerminateJobObject(self.0, 1) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        /// Nothing to forget here: the job holds its processes, not their ids.
        pub fn release(&self, _pid: u32) {}
    }

    impl Drop for ProcessGroup {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(unix)]
mod imp {
    use std::io;
    use std::sync::{Mutex, PoisonError};

    use tokio::process::Child;

    /// The ids of the adopted children, each the leader of its own process group. The shell runner starts
    /// every child that way.
    #[derive(Default)]
    pub struct ProcessGroup {
        leaders: Mutex<Vec<libc::pid_t>>,
    }

    impl ProcessGroup {
        pub fn new() -> io::Result<ProcessGroup> {
            Ok(ProcessGroup::default())
        }

        pub fn adopt(&self, child: &Child) -> io::Result<()> {
            let pid = child.id().ok_or_else(|| io::Error::other("the child has already exited"))?;
            self.leaders.lock().unwrap_or_else(PoisonError::into_inner).push(pid as libc::pid_t);
            Ok(())
        }

        /// Forgets a child once it has been reaped. Its id may be reused after that, and ending its group
        /// later could then hit an unrelated process. Processes still in the group are no longer ended with it.
        pub fn release(&self, pid: u32) {
            let pid = pid as libc::pid_t;
            self.leaders.lock().unwrap_or_else(PoisonError::into_inner).retain(|leader| *leader != pid);
        }

        pub fn kill_all(&self) -> io::Result<()> {
            let leaders = std::mem::take(&mut *self.leaders.lock().unwrap_or_else(PoisonError::into_inner));
            let mut result = Ok(());
            for leader in leaders {
                if unsafe { libc::kill(-leader, libc::SIGKILL) } != 0 {
                    let error = io::Error::last_os_error();
                    // A group that already ended is what we wanted.
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        result = Err(error);
                    }
                }
            }
            result
        }
    }

    impl Drop for ProcessGroup {
        fn drop(&mut self) {
            let _ = self.kill_all();
        }
    }
}

pub use imp::ProcessGroup;

/// Resolve Windows npm/npx launchers to node.exe plus npm's installed CLI file.
/// Never runs or parses a .cmd shim and never builds a shell command string.
pub fn node_package_runner(program:&str,args:&[String],path:&OsStr)->Result<Option<(PathBuf,Vec<String>)>,String>{
    let spelling=Path::new(program);
    let name=spelling.file_name().and_then(OsStr::to_str).unwrap_or(program).to_ascii_lowercase();
    let runner=match name.as_str(){"npx"|"npx.cmd"|"npx.bat"=>"npx-cli.js","npm"|"npm.cmd"|"npm.bat"=>"npm-cli.js",_=>return Ok(None)};
    let dirs:Vec<PathBuf>=std::env::split_paths(path).filter(|dir|dir.is_absolute()).collect();
    let node=dirs.iter().map(|dir|dir.join("node.exe")).find(|node|node.is_file()).ok_or("Node.js wasn't found. Configure node.exe and the package entry point explicitly.")?;
    let script=if spelling.is_absolute(){
        spelling.parent().map(|dir|dir.join("node_modules/npm/bin").join(runner)).filter(|file|file.is_file())
    }else{
        dirs.iter().map(|dir|dir.join("node_modules/npm/bin").join(runner)).find(|file|file.is_file())
    }.ok_or("npm's native JavaScript entry point wasn't found. Configure node.exe and the installed package entry point explicitly.")?;
    let mut native_args=vec![script.to_string_lossy().into_owned()];native_args.extend_from_slice(args);
    Ok(Some((node,native_args)))
}

/// The bash that runs commands. Git Bash is preferred. The WSL launcher in `System32` and the Store
/// aliases in `WindowsApps` never count, since they work on another file system than the project's.
pub fn find_bash(path_variable: &OsStr) -> Option<PathBuf> {
    let name = if cfg!(windows) { "bash.exe" } else { "bash" };
    let mut candidates = vec![];
    for directory in std::env::split_paths(path_variable) {
        // Git for Windows puts only its cmd folder on PATH by default, and bash lives in the sibling bin.
        let in_git_cmd = cfg!(windows) && directory.file_name().is_some_and(|name| name.eq_ignore_ascii_case("cmd"));
        if let Some(git) = directory.parent().filter(|_| in_git_cmd) {
            candidates.push(git.join("bin").join(name));
        }
        candidates.push(directory.join(name));
    }
    let usable: Vec<PathBuf> = candidates.into_iter().filter(|path| path.is_file() && !is_forbidden(path)).collect();
    usable.iter().find(|path| is_git(path)).or(usable.first()).cloned()
}

fn is_forbidden(path: &Path) -> bool {
    if !cfg!(windows) {
        return false;
    }
    let parts: Vec<String> = path.components().map(|part| part.as_os_str().to_string_lossy().to_lowercase()).collect();
    parts.iter().any(|part| part == "windowsapps")
        || parts.windows(2).any(|pair| pair[0] == "windows" && pair[1] == "system32")
}

fn is_git(path: &Path) -> bool {
    cfg!(windows) && path.components().any(|part| part.as_os_str().eq_ignore_ascii_case("git"))
}

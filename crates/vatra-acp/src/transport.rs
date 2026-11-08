use std::io::ErrorKind;
use std::process::Stdio;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use vatra_core::{AgentError, ProcessGroup};

use crate::AcpConfig;

pub(crate) const MAX_FRAME: usize = 1024 * 1024;

pub(crate) fn failed(message: impl Into<String>) -> AgentError {
    AgentError::Failed(message.into())
}

pub(crate) struct Connection {
    pub child: Child,
    pub tree: ProcessGroup,
    pub session: Option<String>,
    pub pending_permission: Option<Value>,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl Connection {
    pub fn spawn(config: &AcpConfig) -> Result<Self, AgentError> {
        if config.binary.extension().is_some_and(|extension| {
            extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
        }) {
            return Err(failed(
                "ACP needs an executable, not a shell shim. For npm agents use node.exe and the installed JavaScript entry point.",
            ));
        }
        if !config.cwd.is_absolute() {
            return Err(failed("ACP requires an absolute working folder."));
        }
        if config.cwd.to_str().is_none() {
            return Err(failed(
                "ACP requires a working folder path that is valid UTF-8.",
            ));
        }
        // Use an executable (for npm agents: node.exe plus the installed JS entry point).
        // Shell shims are intentionally not expanded into shell command strings.
        let mut command = Command::new(&config.binary);
        command.args(&config.args).current_dir(&config.cwd);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command.env_remove("VATRA_API_KEY");
        for name in &config.hidden_env {
            command.env_remove(name);
        }
        if let Some(setup) = &config.child_setup {
            setup.apply(&mut command);
        }
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        let tree =
            ProcessGroup::new().map_err(|_| failed("Couldn't create the ACP process group."))?;
        let mut child = command.spawn().map_err(|error| {
            if error.kind() == ErrorKind::NotFound {
                AgentError::NotInstalled(config.binary.display().to_string())
            } else {
                failed("Couldn't start the ACP agent. Check its executable and working folder.")
            }
        })?;
        tree.adopt(&child)
            .map_err(|_| failed("Couldn't track the ACP agent's process tree."))?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| failed("The ACP agent has no stdin."))?;
        let output = BufReader::new(
            child
                .stdout
                .take()
                .ok_or_else(|| failed("The ACP agent has no stdout."))?,
        );
        Ok(Self {
            child,
            tree,
            session: None,
            pending_permission: None,
            input,
            output,
        })
    }

    pub async fn send(&mut self, message: Value) -> Result<(), AgentError> {
        let mut bytes =
            serde_json::to_vec(&message).map_err(|_| failed("Couldn't encode an ACP message."))?;
        if bytes.len() > MAX_FRAME {
            return Err(failed("The ACP request exceeds the 1 MiB message limit."));
        }
        bytes.push(b'\n');
        self.input
            .write_all(&bytes)
            .await
            .map_err(|_| failed("The ACP agent closed its input."))?;
        self.input
            .flush()
            .await
            .map_err(|_| failed("Couldn't flush the ACP agent's input."))
    }

    pub async fn read(&mut self) -> Result<Value, AgentError> {
        // One extra byte for the newline, so a message of exactly MAX_FRAME bytes is still read whole.
        let mut bytes = Vec::new();
        (&mut self.output)
            .take((MAX_FRAME + 2) as u64)
            .read_until(b'\n', &mut bytes)
            .await
            .map_err(|_| failed("Couldn't read the ACP agent's output."))?;
        let terminated = bytes.last() == Some(&b'\n');
        if terminated {
            bytes.pop();
        }
        if bytes.len() > MAX_FRAME {
            return Err(failed("The ACP agent exceeded the 1 MiB message limit."));
        }
        if !terminated {
            return Err(failed(
                "The ACP agent exited or closed its output before replying.",
            ));
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
            failed(
                "The ACP agent sent invalid JSON. Its stdout must contain only JSON-RPC messages.",
            )
        })?;
        if value["jsonrpc"] != "2.0" || !value.is_object() {
            return Err(failed("The ACP agent sent an invalid JSON-RPC envelope."));
        }
        Ok(value)
    }

    pub async fn cancel(&mut self) {
        if let Some(id) = self.pending_permission.take() {
            let _ = self
                .send(json!({"jsonrpc":"2.0","id":id,"result":{"outcome":{"outcome":"cancelled"}}}))
                .await;
        }
        if let Some(session) = &self.session {
            let _ = self
                .send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}}))
                .await;
        }
    }
}

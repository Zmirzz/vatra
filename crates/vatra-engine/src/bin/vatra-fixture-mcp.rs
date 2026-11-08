//! MCP server used by the tests: `echo` returns its text, `crash` kills the process.
//! `--crash-after N` makes the process exit when the echo call after the N-th one arrives, so a test can
//! see a server die in the middle of a call. `--ready-file PATH` makes it exit at start while PATH is missing.
//! `--many N` adds N filler tools, `filler_000` onwards. `--prompts-fail` makes listing the prompts fail.
//! Every server also offers a `greet` and a `show` prompt, a `notes` resource and a `picture` resource that is an
//! image.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rmcp::handler::server::tool::ToolCallContext;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, GetPromptRequestParams, GetPromptResponse,
    GetPromptResult, ListPromptsResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams, Prompt,
    PromptArgument, PromptMessage, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, Role, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::stdio;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt, schemars, tool, tool_handler, tool_router};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct EchoParams {
    text: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct NameParams {
    name: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ApproveParams {
    input: serde_json::Value,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SizeParams {
    characters: usize,
}

#[derive(Clone)]
struct Fixture {
    crash_after: Option<usize>,
    echoes: Arc<AtomicUsize>,
    many: usize,
    prompts_fail: bool,
}

const PICTURE_URI: &str = "memo://picture";
const NOTES_URI: &str = "memo://notes";

#[tool_router]
impl Fixture {
    #[tool(description = "Returns the given text unchanged")]
    fn echo(&self, Parameters(EchoParams { text }): Parameters<EchoParams>) -> String {
        let calls = self.echoes.fetch_add(1, Ordering::SeqCst) + 1;
        if self.crash_after.is_some_and(|limit| calls > limit) {
            std::process::exit(3);
        }
        text
    }

    #[tool(description = "Terminates the server process")]
    fn crash(&self) -> String {
        std::process::exit(3)
    }

    #[tool(description = "Approves a Claude Code permission prompt, for --permission-prompt-tool")]
    fn approve(&self, Parameters(ApproveParams { input }): Parameters<ApproveParams>) -> String {
        serde_json::json!({ "behavior": "allow", "updatedInput": input }).to_string()
    }

    #[tool(description = "Returns an image and no text")]
    fn image(&self) -> CallToolResult {
        CallToolResult::success(vec![ContentBlock::image("aGVsbG8=", "image/png")])
    }

    #[tool(description = "Returns numbered lines, about the given number of characters long")]
    fn long_text(&self, Parameters(SizeParams { characters }): Parameters<SizeParams>) -> String {
        numbered_lines(characters)
    }

    #[tool(description = "Fails with numbered lines, which start with START and end with END")]
    fn long_error(&self, Parameters(SizeParams { characters }): Parameters<SizeParams>) -> CallToolResult {
        let text = format!("START{}END", numbered_lines(characters));
        CallToolResult::error(vec![ContentBlock::text(text)])
    }

    #[tool(description = "Looks a value up without changing anything", annotations(read_only_hint = true))]
    fn lookup(&self) -> String {
        "found".into()
    }

    #[tool(name = "lookup__deep", description = "A tool whose name has a double underscore")]
    fn lookup_deep(&self) -> String {
        "deep".into()
    }

    #[tool(description = "Returns the value of an environment variable of this process, or absent")]
    fn environment(&self, Parameters(NameParams { name }): Parameters<NameParams>) -> String {
        std::env::var(name).unwrap_or_else(|_| "absent".into())
    }

    #[tool(description = "Never answers within a reasonable time")]
    async fn hang(&self) -> String {
        tokio::time::sleep(std::time::Duration::from_secs(600)).await;
        String::new()
    }
}

/// Lines `line N`, one per line, until the text is at least this many characters long.
fn numbered_lines(characters: usize) -> String {
    let mut text = String::new();
    let mut number = 0;
    while text.len() < characters {
        text.push_str(&format!("line {number}\n"));
        number += 1;
    }
    text
}

/// The filler tool with this number. Tool 42 has a description of its own, so a search can find it by a word.
/// Tool 7 has a description longer than the limit of a tool list.
fn filler(number: usize) -> Tool {
    let description = match number {
        42 => "Converts invoices to ledger rows".to_string(),
        7 => "d".repeat(5_000),
        _ => format!("Reads sample metric number {number} for the many-tools test"),
    };
    let schema = serde_json::json!({ "type": "object", "properties": {} });
    Tool::new(format!("filler_{number:03}"), description, Arc::new(schema.as_object().cloned().unwrap_or_default()))
}

fn greeting() -> Prompt {
    let mut name = PromptArgument::new("name");
    name.description = Some("Who to greet".into());
    name.required = Some(true);
    Prompt::new("greet", Some("Greets someone by name"), Some(vec![name]))
}

fn picture_prompt() -> Prompt {
    Prompt::new("show", Some("Shows a picture"), None::<Vec<PromptArgument>>)
}

#[tool_handler]
impl ServerHandler for Fixture {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().enable_prompts().enable_resources().build())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let mut tools = Self::tool_router().list_all();
        tools.extend((0..self.many).map(filler));
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name.starts_with("filler_") {
            let text = format!("filler {}", request.name);
            return Ok(CallToolResponse::Complete(CallToolResult::success(vec![ContentBlock::text(text)])));
        }
        Self::tool_router().call(ToolCallContext::new(self, request, context)).await
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        if self.prompts_fail {
            return Err(ErrorData::internal_error("the prompt list is broken", None));
        }
        Ok(ListPromptsResult::with_all_items(vec![greeting(), picture_prompt()]))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        let message = match request.name.as_str() {
            "greet" => {
                let name = request.arguments.as_ref().and_then(|arguments| arguments.get("name"));
                let name = name.and_then(serde_json::Value::as_str).unwrap_or("friend");
                PromptMessage::new_text(Role::User, format!("Greet {name}"))
            }
            "show" => PromptMessage::new_image(Role::User, b"hello", "image/png", None, None),
            _ => return Err(ErrorData::invalid_params("there is no such prompt", None)),
        };
        Ok(GetPromptResponse::Complete(GetPromptResult::new(vec![message])))
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let notes = Resource::new(NOTES_URI, "notes");
        let picture = Resource::new(PICTURE_URI, "picture");
        Ok(ListResourcesResult::with_all_items(vec![notes, picture]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let contents = match request.uri.as_str() {
            NOTES_URI => ResourceContents::text("Remember the milk", NOTES_URI),
            PICTURE_URI => ResourceContents::BlobResourceContents {
                uri: PICTURE_URI.into(),
                mime_type: Some("image/png".into()),
                blob: "aGVsbG8=".into(),
                meta: None,
            },
            _ => return Err(ErrorData::invalid_params("there is no such resource", None)),
        };
        Ok(ReadResourceResponse::Complete(ReadResourceResult::new(vec![contents])))
    }
}

/// Writes the time to `path` every 50 ms, for as long as it runs. A test reads the file to see whether this process
/// is still alive.
fn pulse(path: &str) -> ! {
    loop {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        let _ = std::fs::write(path, now.as_millis().to_string());
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[tokio::main]
async fn main() {
    let mut crash_after = None;
    let mut ready_file = None;
    let mut many = 0;
    let mut prompts_fail = false;
    let mut child_pulse = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--crash-after" => crash_after = args.next().and_then(|value| value.parse().ok()),
            "--ready-file" => ready_file = args.next(),
            "--many" => many = args.next().and_then(|value| value.parse().ok()).unwrap_or(0),
            "--prompts-fail" => prompts_fail = true,
            // Runs as a child of the server, as a tool the server started would.
            "--pulse" => pulse(&args.next().unwrap_or_default()),
            // Starts a child of its own that pulses into this file, so a test can see whether the tree is alive.
            "--child-pulse" => child_pulse = args.next(),
            _ => {}
        }
    }
    if let Some(path) = child_pulse {
        let exe = std::env::current_exe().expect("the fixture knows its own path");
        let mut child = std::process::Command::new(exe)
            .args(["--pulse", &path])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("couldn't start the child");
        // Waited on in the background, so the child is reaped when it ends.
        std::thread::spawn(move || child.wait());
    }
    // Until the ready file exists the process exits at once, as a server that cannot start does.
    if ready_file.is_some_and(|path| !std::path::Path::new(&path).exists()) {
        std::process::exit(4);
    }
    let fixture = Fixture { crash_after, echoes: Arc::new(AtomicUsize::new(0)), many, prompts_fail };
    let service = fixture.serve(stdio()).await.expect("couldn't start the server");
    let _ = service.waiting().await;
}

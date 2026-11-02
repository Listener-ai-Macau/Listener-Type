use std::time::Duration;

use crate::windows_ime_protocol::ImeSubmitStatus;

pub const IME_CLIENT_WAIT_TIMEOUT: Duration = Duration::from_millis(700);
const IME_OWNER_THREAD_MESSAGE_TIMEOUT_MS: u64 = 3000;
const IME_ASYNC_EDIT_SESSION_TIMEOUT_MS: u64 = 3000;
const IME_SUBMIT_TIMEOUT_MARGIN_MS: u64 = 1000;
const IME_NATIVE_ASYNC_COMMIT_TIMEOUT_MS: u64 =
    IME_OWNER_THREAD_MESSAGE_TIMEOUT_MS + IME_ASYNC_EDIT_SESSION_TIMEOUT_MS;

// Must exceed the IME DLL owner-thread SendMessageTimeoutW wait plus the
// async edit session wait, otherwise Rust can fall back while the DLL later
// commits and duplicates insertion.
pub const IME_SUBMIT_TIMEOUT: Duration =
    Duration::from_millis(IME_NATIVE_ASYNC_COMMIT_TIMEOUT_MS + IME_SUBMIT_TIMEOUT_MARGIN_MS);
const IME_PIPE_RETRY_INTERVAL: Duration = Duration::from_millis(25);

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_PATH_NOT_FOUND: u32 = 3;
const ERROR_SEM_TIMEOUT: u32 = 121;
const ERROR_PIPE_BUSY: u32 = 231;
const NMPWAIT_NOWAIT: u32 = 0x00000001;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowsImeIpcError {
    Unavailable(String),
    NoReadyClient,
    Timeout,
    Protocol(String),
    Io(String),
    /// A caller guard was revoked before the request was written.
    NotDispatched,
    /// A request may have reached the target; failure is not proof that it
    /// did not execute, so callers must not send a second insertion.
    AfterDispatch(String),
}

impl std::fmt::Display for WindowsImeIpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(message) | Self::Protocol(message) | Self::Io(message) | Self::AfterDispatch(message) => {
                write!(f, "{message}")
            }
            Self::NoReadyClient => write!(f, "no Listener Type IME client is ready"),
            Self::NotDispatched => write!(f, "IME request cancelled before dispatch"),
            Self::Timeout => write!(f, "Listener Type IME IPC timed out"),
        }
    }
}

impl std::error::Error for WindowsImeIpcError {}

pub type WindowsImeIpcResult<T> = Result<T, WindowsImeIpcError>;

fn map_wait_named_pipe_error(error_code: Option<u32>) -> WindowsImeIpcError {
    match error_code {
        Some(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_PIPE_BUSY) => {
            WindowsImeIpcError::NoReadyClient
        }
        Some(ERROR_SEM_TIMEOUT) => WindowsImeIpcError::Timeout,
        Some(code) => WindowsImeIpcError::Io(format!("WaitNamedPipeW failed with OS error {code}")),
        None => WindowsImeIpcError::Io("WaitNamedPipeW failed without OS error".to_string()),
    }
}

fn is_retryable_pipe_error(error_code: Option<u32>) -> bool {
    matches!(
        error_code,
        Some(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_PIPE_BUSY | ERROR_SEM_TIMEOUT)
    )
}

fn classify_native_result(status: ImeSubmitStatus, error_code: Option<&str>) -> ImeSubmitStatus {
    if status == ImeSubmitStatus::Rejected && error_code.is_some_and(|code|
        code.starts_with("uncertain:") || code.eq_ignore_ascii_case("hresult:0x800705B4")) {
        ImeSubmitStatus::Unconfirmed
    } else {
        status
    }
}

#[derive(Debug)]
pub struct PendingImeSubmit {
    session_id: String,
    completed: bool,
}

impl PendingImeSubmit {
    pub fn new(session_id: String) -> Self {
        Self {
            session_id,
            completed: false,
        }
    }

    pub fn accept_result(
        &mut self,
        session_id: &str,
        status: ImeSubmitStatus,
    ) -> WindowsImeIpcResult<ImeSubmitStatus> {
        if self.completed {
            return Err(WindowsImeIpcError::Protocol(
                "submit result arrived after completion".to_string(),
            ));
        }
        if self.session_id != session_id {
            return Err(WindowsImeIpcError::Protocol(
                "submit result belongs to a different session".to_string(),
            ));
        }
        self.completed = true;
        Ok(status)
    }
}

#[derive(Debug, Clone)]
pub struct ImeSubmitRequest {
    pub session_id: String,
    pub text: String,
    pub created_at: String,
    pub target: Option<ImeSubmitTarget>,
}

/// 组字流式操作(2026-09-22 讯飞式):Update=原地替换组字;Commit=终稿落定;
/// Cancel=清空。会话边界由调用方保证:新会话首个 Update 前必发 Cancel。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImeStreamOp {
    Update,
    Commit,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImeSubmitTarget {
    pub process_id: u32,
    pub thread_id: u32,
}

#[derive(Clone)]
pub struct WindowsImeIpcServer {
    inner: std::sync::Arc<parking_lot::Mutex<WindowsImeIpcState>>,
}

#[derive(Debug, Default)]
struct WindowsImeIpcState {
    ready_client_id: Option<String>,
}

impl WindowsImeIpcServer {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Arc::new(parking_lot::Mutex::new(WindowsImeIpcState::default())),
        }
    }

    pub fn mark_client_ready_for_test(&self, client_id: String) {
        self.inner.lock().ready_client_id = Some(client_id);
    }

    pub fn has_ready_client(&self) -> bool {
        self.inner.lock().ready_client_id.is_some()
    }

    pub async fn submit_text(
        &self,
        request: ImeSubmitRequest,
    ) -> WindowsImeIpcResult<ImeSubmitStatus> {
        #[cfg(target_os = "windows")]
        {
            let _ = self;
            submit_text_to_platform(request).await
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = self;
            let _ = request;
            Err(WindowsImeIpcError::Unavailable(
                "Listener Type IME IPC is only available on Windows".to_string(),
            ))
        }
    }

    /// A streaming append must address the exact focused GUI thread and
    /// recheck its session/target after waiting for the client but before any
    /// bytes are written. It never searches other threads in the process.
    #[cfg(target_os = "windows")]
    pub async fn submit_text_exact(
        &self,
        request: ImeSubmitRequest,
        before_write: impl FnOnce() -> bool + Send,
    ) -> WindowsImeIpcResult<ImeSubmitStatus> {
        windows_pipe::submit_text_exact_over_pipe(request, before_write).await
    }

    #[cfg(target_os = "windows")]
    pub async fn submit_terminal_exact(
        &self,
        request: ImeSubmitRequest,
        expected_body: String,
        before_write: impl FnOnce() -> bool + Send,
    ) -> WindowsImeIpcResult<ImeSubmitStatus> {
        windows_pipe::submit_guarded_exact_over_pipe(request, Some(expected_body), before_write).await
    }

    pub async fn submit_stream_op(
        &self,
        op: ImeStreamOp,
        request: ImeSubmitRequest,
    ) -> WindowsImeIpcResult<ImeSubmitStatus> {
        #[cfg(target_os = "windows")]
        {
            let _ = self;
            submit_stream_op_to_platform(op, request).await
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = (self, op, request);
            Err(WindowsImeIpcError::Unavailable(
                "Listener Type IME IPC is only available on Windows".to_string(),
            ))
        }
    }
}

impl Default for WindowsImeIpcServer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "windows")]
async fn submit_text_to_platform(
    request: ImeSubmitRequest,
) -> WindowsImeIpcResult<ImeSubmitStatus> {
    windows_pipe::submit_text_over_pipe(request).await
}

#[cfg(target_os = "windows")]
async fn submit_stream_op_to_platform(
    op: ImeStreamOp,
    request: ImeSubmitRequest,
) -> WindowsImeIpcResult<ImeSubmitStatus> {
    windows_pipe::submit_stream_op_over_pipe(op, request).await
}

#[cfg(target_os = "windows")]
mod windows_pipe {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::time::{Duration, Instant};

    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

    use super::{
        classify_native_result, ImeSubmitRequest, PendingImeSubmit, WindowsImeIpcError, WindowsImeIpcResult,
        IME_CLIENT_WAIT_TIMEOUT, IME_PIPE_RETRY_INTERVAL, IME_SUBMIT_TIMEOUT,
    };
    use crate::windows_ime_protocol::{
        decode_message, encode_message, ime_pipe_candidate_names_for_target,
        ime_pipe_name_for_target, ImePipeMessage, LISTENER_TYPE_IME_PROTOCOL_VERSION,
    };

    extern "system" {
        fn WaitNamedPipeW(lpNamedPipeName: *const u16, nTimeOut: u32) -> i32;
    }

    pub async fn submit_text_over_pipe(
        request: ImeSubmitRequest,
    ) -> WindowsImeIpcResult<crate::windows_ime_protocol::ImeSubmitStatus> {
        let ImeSubmitRequest {
            session_id,
            text,
            created_at,
            target,
        } = request;
        let message = ImePipeMessage::SubmitText {
            protocol_version: LISTENER_TYPE_IME_PROTOCOL_VERSION,
            session_id,
            text,
            created_at,
        };
        submit_message_over_pipe(message, target, false, || true).await
    }

    pub async fn submit_text_exact_over_pipe(
        request: ImeSubmitRequest,
        before_write: impl FnOnce() -> bool + Send,
    ) -> WindowsImeIpcResult<crate::windows_ime_protocol::ImeSubmitStatus> {
        submit_guarded_exact_over_pipe(request, None, before_write).await
    }

    pub async fn submit_guarded_exact_over_pipe(
        request: ImeSubmitRequest,
        expected_body: Option<String>,
        before_write: impl FnOnce() -> bool + Send,
    ) -> WindowsImeIpcResult<crate::windows_ime_protocol::ImeSubmitStatus> {
        let ImeSubmitRequest { session_id: _, text, created_at, target } = request;
        if let Some(expected) = expected_body.as_ref() {
            if expected.is_empty() || !crate::transcript_boundary::terminal_punctuation_only(&text) {
                return Err(WindowsImeIpcError::NotDispatched);
            }
        }
        let target = target.ok_or(WindowsImeIpcError::NoReadyClient)?;
        // A unique token prevents a delayed or concurrent bind from ever
        // authorizing another request to a different context.
        let token = format!("direct-{}", uuid::Uuid::new_v4());
        let binding = ImePipeMessage::BindDirect {
            protocol_version: LISTENER_TYPE_IME_PROTOCOL_VERSION,
            session_id: token.clone(), text: String::new(),
        };
        // A timeout here carries no text. A late binding can only invalidate
        // another unique token, never authorize it.
        match tokio::time::timeout(Duration::from_millis(250),
            submit_message_over_pipe(binding, Some(target), true, || true)).await {
            Ok(Ok(crate::windows_ime_protocol::ImeSubmitStatus::Committed)) => {},
            _ => return Err(WindowsImeIpcError::NoReadyClient),
        }
        let message = match expected_body {
            Some(expected_body) => ImePipeMessage::SubmitTerminal {
                protocol_version: LISTENER_TYPE_IME_PROTOCOL_VERSION,
                session_id: token, text, expected_body, created_at,
            },
            None => ImePipeMessage::SubmitGuarded {
                protocol_version: LISTENER_TYPE_IME_PROTOCOL_VERSION,
                session_id: token, text, created_at,
            },
        };
        submit_message_over_pipe(message, Some(target), true, before_write).await
    }

    pub async fn submit_stream_op_over_pipe(
        op: super::ImeStreamOp,
        request: ImeSubmitRequest,
    ) -> WindowsImeIpcResult<crate::windows_ime_protocol::ImeSubmitStatus> {
        let ImeSubmitRequest {
            session_id,
            text,
            created_at,
            target,
        } = request;
        let message = match op {
            super::ImeStreamOp::Update => ImePipeMessage::StreamUpdate {
                protocol_version: LISTENER_TYPE_IME_PROTOCOL_VERSION,
                session_id,
                text,
                created_at,
            },
            super::ImeStreamOp::Commit => ImePipeMessage::StreamCommit {
                protocol_version: LISTENER_TYPE_IME_PROTOCOL_VERSION,
                session_id,
                text,
                created_at,
            },
            // DLL 协议要求 text 字段存在;cancel 恒发空串。
            super::ImeStreamOp::Cancel => ImePipeMessage::StreamCancel {
                protocol_version: LISTENER_TYPE_IME_PROTOCOL_VERSION,
                session_id,
                text: String::new(),
                created_at,
            },
        };
        submit_message_over_pipe(message, target, false, || true).await
    }

    async fn submit_message_over_pipe(
        message: ImePipeMessage,
        target: Option<super::ImeSubmitTarget>,
        exact_only: bool,
        before_write: impl FnOnce() -> bool + Send,
    ) -> WindowsImeIpcResult<crate::windows_ime_protocol::ImeSubmitStatus> {
        let request_session_id = match &message {
            ImePipeMessage::SubmitText { session_id, .. }
            | ImePipeMessage::BindDirect { session_id, .. }
            | ImePipeMessage::SubmitGuarded { session_id, .. }
            | ImePipeMessage::SubmitTerminal { session_id, .. }
            | ImePipeMessage::StreamUpdate { session_id, .. }
            | ImePipeMessage::StreamCommit { session_id, .. }
            | ImePipeMessage::StreamCancel { session_id, .. } => session_id.clone(),
            _ => return Err(WindowsImeIpcError::Protocol("not a submit message".into())),
        };
        let target = target.ok_or(WindowsImeIpcError::NoReadyClient)?;
        let mut pending = PendingImeSubmit::new(request_session_id);
        let (pipe_name, pipe) = open_pipe_with_retry(target, exact_only).await?;
        let (read_half, mut write_half) = tokio::io::split(pipe);
        let mut reader = BufReader::new(read_half);

        let line = encode_message(&message)
            .map_err(|error| WindowsImeIpcError::Protocol(error.to_string()))?;
        if !before_write() { return Err(WindowsImeIpcError::NotDispatched); }

        let response = tokio::time::timeout(IME_SUBMIT_TIMEOUT, async {
            log::debug!("[windows-ime] submitting text over pipe {pipe_name}");
            write_half
                .write_all(line.as_bytes())
                .await
                .map_err(|error| WindowsImeIpcError::Io(error.to_string()))?;
            write_half
                .flush()
                .await
                .map_err(|error| WindowsImeIpcError::Io(error.to_string()))?;

            let mut response = String::new();
            let bytes_read = reader
                .read_line(&mut response)
                .await
                .map_err(|error| WindowsImeIpcError::Io(error.to_string()))?;

            if bytes_read == 0 {
                return Err(WindowsImeIpcError::Io(
                    "IME pipe closed before submit result".to_string(),
                ));
            }

            Ok(response)
        })
        .await
        .map_err(|_| WindowsImeIpcError::AfterDispatch("IME result timed out after dispatch".into()))?
        .map_err(|error| WindowsImeIpcError::AfterDispatch(error.to_string()))?;

        match decode_message(response.trim_end())
            .map_err(|error| WindowsImeIpcError::AfterDispatch(error.to_string()))?
        {
            ImePipeMessage::SubmitResult {
                protocol_version,
                session_id,
                status,
                error_code,
            } if protocol_version == LISTENER_TYPE_IME_PROTOCOL_VERSION => {
                if status != crate::windows_ime_protocol::ImeSubmitStatus::Committed {
                    log::warn!(
                        "[windows-ime] submit result status={status:?} error_code={error_code:?}"
                    );
                }
                let status = classify_native_result(status, error_code.as_deref());
                let accepted = pending.accept_result(&session_id, status)
                    .map_err(|error| WindowsImeIpcError::AfterDispatch(error.to_string()))?;
                if exact_only && accepted == crate::windows_ime_protocol::ImeSubmitStatus::Failed
                    && error_code.as_deref() == Some("guardMismatch") {
                    return Err(WindowsImeIpcError::NotDispatched);
                }
                Ok(accepted)
            }
            ImePipeMessage::SubmitResult {
                protocol_version, ..
            } => Err(WindowsImeIpcError::AfterDispatch(format!(
                "unsupported IME protocol version {protocol_version}"
            ))),
            _ => Err(WindowsImeIpcError::AfterDispatch(
                "message is not a submit result".to_string(),
            )),
        }
    }

    async fn open_pipe_with_retry(
        target: super::ImeSubmitTarget,
        exact_only: bool,
    ) -> WindowsImeIpcResult<(String, NamedPipeClient)> {
        let deadline = Instant::now() + if exact_only { Duration::from_millis(75) } else { IME_CLIENT_WAIT_TIMEOUT };
        let exact_pipe_name = ime_pipe_name_for_target(target.process_id, target.thread_id);

        loop {
            let mut retry_error = WindowsImeIpcError::NoReadyClient;

            let candidates = if exact_only { vec![exact_pipe_name.clone()] } else { pipe_names_for_target(target) };
            for pipe_name in candidates {
                retry_error = match wait_for_pipe_client(&pipe_name) {
                    Ok(()) => match ClientOptions::new().open(&pipe_name) {
                        Ok(pipe) => {
                            if pipe_name != exact_pipe_name {
                                log::info!(
                                    "[windows-ime] exact target pipe {exact_pipe_name} was not ready; using same-process pipe {pipe_name}"
                                );
                            }
                            return Ok((pipe_name, pipe));
                        }
                        Err(error) => {
                            let error_code = error.raw_os_error().map(|code| code as u32);
                            if !super::is_retryable_pipe_error(error_code) {
                                return Err(WindowsImeIpcError::Io(error.to_string()));
                            }
                            super::map_wait_named_pipe_error(error_code)
                        }
                    },
                    Err(error) => {
                        if !is_retryable_wait_error(&error) {
                            return Err(error);
                        }
                        error
                    }
                };
            }

            if Instant::now() >= deadline {
                return Err(retry_error);
            }
            tokio::time::sleep(next_retry_delay(deadline)).await;
        }
    }

    fn pipe_names_for_target(target: super::ImeSubmitTarget) -> Vec<String> {
        ime_pipe_candidate_names_for_target(
            target.process_id,
            target.thread_id,
            available_pipe_names(),
        )
    }

    fn available_pipe_names() -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(r"\\.\pipe\") else {
            return Vec::new();
        };

        entries
            .filter_map(Result::ok)
            .map(|entry| format!(r"\\.\pipe\{}", entry.file_name().to_string_lossy()))
            .collect()
    }

    fn wait_for_pipe_client(pipe_name: &str) -> WindowsImeIpcResult<()> {
        let pipe_name = OsStr::new(pipe_name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>();

        let is_ready = unsafe { WaitNamedPipeW(pipe_name.as_ptr(), super::NMPWAIT_NOWAIT) };
        if is_ready != 0 {
            return Ok(());
        }

        Err(super::map_wait_named_pipe_error(
            std::io::Error::last_os_error()
                .raw_os_error()
                .map(|code| code as u32),
        ))
    }

    fn is_retryable_wait_error(error: &WindowsImeIpcError) -> bool {
        matches!(
            error,
            WindowsImeIpcError::NoReadyClient | WindowsImeIpcError::Timeout
        )
    }

    fn next_retry_delay(deadline: Instant) -> std::time::Duration {
        deadline
            .saturating_duration_since(Instant::now())
            .min(IME_PIPE_RETRY_INTERVAL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "windows")]
    use crate::windows_ime_protocol::ImePipeMessage;

    #[test]
    fn pending_submit_accepts_only_matching_session() {
        let mut pending = PendingImeSubmit::new("session-1".to_string());
        assert!(pending
            .accept_result("session-2", ImeSubmitStatus::Committed)
            .is_err());
        assert_eq!(
            pending.accept_result("session-1", ImeSubmitStatus::Committed),
            Ok(ImeSubmitStatus::Committed)
        );
    }

    #[test]
    fn pending_submit_rejects_second_result_after_completion() {
        let mut pending = PendingImeSubmit::new("session-1".to_string());
        assert_eq!(
            pending.accept_result("session-1", ImeSubmitStatus::Committed),
            Ok(ImeSubmitStatus::Committed)
        );
        assert!(pending
            .accept_result("session-1", ImeSubmitStatus::Committed)
            .is_err());
    }

    #[test]
    fn submit_timeout_covers_native_async_commit_path() {
        assert!(IME_SUBMIT_TIMEOUT > Duration::from_millis(IME_NATIVE_ASYNC_COMMIT_TIMEOUT_MS));
    }

    #[test]
    fn wait_pipe_error_mapping_treats_missing_or_busy_pipe_as_no_ready_client() {
        assert_eq!(
            map_wait_named_pipe_error(Some(2)),
            WindowsImeIpcError::NoReadyClient
        );
        assert_eq!(
            map_wait_named_pipe_error(Some(231)),
            WindowsImeIpcError::NoReadyClient
        );
    }

    #[test]
    fn wait_pipe_error_mapping_treats_wait_timeout_as_timeout() {
        assert_eq!(
            map_wait_named_pipe_error(Some(121)),
            WindowsImeIpcError::Timeout
        );
    }

    #[test]
    fn missing_busy_and_timeout_pipe_errors_are_retryable_before_deadline() {
        assert!(is_retryable_pipe_error(Some(2)));
        assert!(is_retryable_pipe_error(Some(3)));
        assert!(is_retryable_pipe_error(Some(121)));
        assert!(is_retryable_pipe_error(Some(231)));
        assert!(!is_retryable_pipe_error(Some(5)));
        assert!(!is_retryable_pipe_error(None));
    }

    #[test]
    fn rejected_results_distinguish_legacy_safe_fallback_from_possible_late_commit() {
        assert_eq!(classify_native_result(ImeSubmitStatus::Rejected, Some("hresult:0x80004005")), ImeSubmitStatus::Rejected);
        assert_eq!(classify_native_result(ImeSubmitStatus::Rejected, Some("hresult:0x800705B4")), ImeSubmitStatus::Unconfirmed);
        assert_eq!(classify_native_result(ImeSubmitStatus::Rejected, Some("uncertain:hresult:0x80004005")), ImeSubmitStatus::Unconfirmed);
        assert_eq!(classify_native_result(ImeSubmitStatus::Failed, Some("protocolError")), ImeSubmitStatus::Failed);
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn lost_result_after_target_received_request_is_unconfirmed() {
        use tokio::io::{AsyncBufReadExt, BufReader};
        use tokio::net::windows::named_pipe::ServerOptions;
        let process_id = std::process::id();
        let thread_id = 4_000_000_001;
        let name = crate::windows_ime_protocol::ime_pipe_name_for_target(process_id, thread_id);
        let server = ServerOptions::new().first_pipe_instance(true).create(&name).unwrap();
        let received = tokio::spawn(async move {
            server.connect().await.unwrap();
            let mut reader = BufReader::new(server);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let request = crate::windows_ime_protocol::decode_message(line.trim()).unwrap();
            assert!(matches!(request, crate::windows_ime_protocol::ImePipeMessage::SubmitText { ref text, .. } if text == "synthetic"));
            // A target may already have accepted the text even though its
            // result is lost. Closing this private test pipe models that case.
        });
        let result = super::windows_pipe::submit_text_over_pipe(ImeSubmitRequest {
            session_id: "synthetic-session".into(), text: "synthetic".into(),
            created_at: "test".into(),
            target: Some(ImeSubmitTarget { process_id, thread_id }),
        }).await;
        received.await.unwrap();
        assert!(matches!(result, Err(WindowsImeIpcError::AfterDispatch(_))));
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn exact_streaming_target_never_borrows_another_thread_pipe() {
        use tokio::net::windows::named_pipe::ServerOptions;
        let process_id = std::process::id();
        let other = crate::windows_ime_protocol::ime_pipe_name_for_target(process_id, 4_000_000_002);
        let _other_server = ServerOptions::new().first_pipe_instance(true).create(&other).unwrap();
        let result = super::windows_pipe::submit_text_exact_over_pipe(ImeSubmitRequest {
            session_id: "test".into(), text: "synthetic".into(), created_at: "test".into(),
            target: Some(ImeSubmitTarget { process_id, thread_id: 4_000_000_003 }),
        }, || panic!("no target pipe means no write guard is needed")).await;
        assert!(matches!(result, Err(WindowsImeIpcError::NoReadyClient | WindowsImeIpcError::Timeout)));
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn exact_streaming_guard_cancels_without_sending_any_text() {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
        use tokio::net::windows::named_pipe::ServerOptions;
        let process_id = std::process::id();
        let thread_id = 4_000_000_004;
        let name = crate::windows_ime_protocol::ime_pipe_name_for_target(process_id, thread_id);
        let mut server = ServerOptions::new().first_pipe_instance(true).create(&name).unwrap();
        let received = tokio::spawn(async move {
            server.connect().await.unwrap();
            let mut probe = String::new();
            BufReader::new(&mut server).read_line(&mut probe).await.unwrap();
            let token = match crate::windows_ime_protocol::decode_message(probe.trim()).unwrap() {
                ImePipeMessage::BindDirect { session_id, text, .. } => { assert!(text.is_empty()); session_id },
                _ => panic!("expected a text-free binding"),
            };
            assert!(!probe.contains("synthetic"));
            let reply = format!("{{\"type\":\"submitResult\",\"protocolVersion\":1,\"sessionId\":\"{token}\",\"status\":\"committed\"}}\n");
            server.write_all(reply.as_bytes()).await.unwrap();
            server.flush().await.unwrap();
            let mut ignored = Vec::new();
            let _ = server.read_to_end(&mut ignored).await;
            server.disconnect().unwrap();
            server.connect().await.unwrap();
            let mut bytes = Vec::new();
            let _ = server.read_to_end(&mut bytes).await;
            bytes
        });
        let result = super::windows_pipe::submit_text_exact_over_pipe(ImeSubmitRequest {
            session_id: "test".into(), text: "synthetic".into(), created_at: "test".into(),
            target: Some(ImeSubmitTarget { process_id, thread_id }),
        }, || false).await;
        assert!(matches!(result, Err(WindowsImeIpcError::NotDispatched)));
        assert!(received.await.unwrap().is_empty());
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn legacy_client_never_receives_streaming_text() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::windows::named_pipe::ServerOptions;
        let process_id = std::process::id();
        let thread_id = 4_000_000_005;
        let name = crate::windows_ime_protocol::ime_pipe_name_for_target(process_id, thread_id);
        let mut server = ServerOptions::new().first_pipe_instance(true).create(&name).unwrap();
        let received = tokio::spawn(async move {
            server.connect().await.unwrap();
            let mut probe = String::new();
            BufReader::new(&mut server).read_line(&mut probe).await.unwrap();
            let token = match crate::windows_ime_protocol::decode_message(probe.trim()).unwrap() {
                ImePipeMessage::BindDirect { session_id, text, .. } => { assert!(text.is_empty()); session_id },
                _ => panic!("expected a text-free binding"),
            };
            let reply = format!("{{\"type\":\"submitResult\",\"protocolVersion\":1,\"sessionId\":\"{token}\",\"status\":\"failed\",\"errorCode\":\"protocolError\"}}\n");
            server.write_all(reply.as_bytes()).await.unwrap();
            server.flush().await.unwrap();
            probe
        });
        let result = super::windows_pipe::submit_text_exact_over_pipe(ImeSubmitRequest {
            session_id: "test".into(), text: "sensitive-synthetic".into(), created_at: "test".into(),
            target: Some(ImeSubmitTarget { process_id, thread_id }),
        }, || panic!("legacy client must not receive user text")).await;
        assert!(matches!(result, Err(WindowsImeIpcError::NoReadyClient)));
        let probe = received.await.unwrap();
        assert!(probe.contains("bindDirect") && !probe.contains("sensitive-synthetic"));
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn native_focus_mismatch_cancels_without_fallback() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::windows::named_pipe::ServerOptions;
        let process_id = std::process::id();
        let thread_id = 4_000_000_006;
        let name = crate::windows_ime_protocol::ime_pipe_name_for_target(process_id, thread_id);
        let mut server = ServerOptions::new().first_pipe_instance(true).create(&name).unwrap();
        let received = tokio::spawn(async move {
            server.connect().await.unwrap();
            let mut line = String::new();
            BufReader::new(&mut server).read_line(&mut line).await.unwrap();
            let token = match crate::windows_ime_protocol::decode_message(line.trim()).unwrap() {
                ImePipeMessage::BindDirect { session_id, text, .. } => { assert!(text.is_empty()); session_id },
                _ => panic!("expected binding"),
            };
            // Keep a distinct second instance listening before responding.
            // Reconnecting one test instance can race its disconnect with the
            // client's next connection and silently consume the next request.
            let mut second_server = ServerOptions::new().create(&name).unwrap();
            let reply = format!("{{\"type\":\"submitResult\",\"protocolVersion\":1,\"sessionId\":\"{token}\",\"status\":\"committed\"}}\n");
            server.write_all(reply.as_bytes()).await.unwrap();
            server.flush().await.unwrap();
            second_server.connect().await.unwrap();
            let mut next = String::new();
            BufReader::new(&mut second_server).read_line(&mut next).await.unwrap();
            match crate::windows_ime_protocol::decode_message(next.trim()).unwrap() {
                ImePipeMessage::SubmitGuarded { session_id, text, .. } => {
                    assert_eq!(session_id, token);
                    assert_eq!(text, "synthetic");
                },
                _ => panic!("expected guarded submit"),
            }
            let reply = format!("{{\"type\":\"submitResult\",\"protocolVersion\":1,\"sessionId\":\"{token}\",\"status\":\"failed\",\"errorCode\":\"guardMismatch\"}}\n");
            second_server.write_all(reply.as_bytes()).await.unwrap();
            second_server.flush().await.unwrap();
        });
        let result = super::windows_pipe::submit_text_exact_over_pipe(ImeSubmitRequest {
            session_id: "test".into(), text: "synthetic".into(), created_at: "test".into(),
            target: Some(ImeSubmitTarget { process_id, thread_id }),
        }, || true).await;
        assert!(matches!(result, Err(WindowsImeIpcError::NotDispatched)));
        received.await.unwrap();
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn guarded_terminal_rejects_non_punctuation_before_contacting_target() {
        let request = ImeSubmitRequest {
            session_id: "test".into(), text: "not punctuation".into(), created_at: "test".into(),
            target: Some(ImeSubmitTarget { process_id: std::process::id(), thread_id: 4_000_000_007 }),
        };
        let result = WindowsImeIpcServer::new().submit_terminal_exact(request, "synthetic".into(),
            || panic!("invalid suffix must not contact the target")).await;
        assert!(matches!(result, Err(WindowsImeIpcError::NotDispatched)));
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    #[ignore = "requires an isolated station and a dedicated synthetic TSF host"]
    async fn exact_streaming_submits_two_segments_to_private_native_tsf() {
        use std::ffi::c_void;
        #[link(name = "user32")]
        extern "system" {
            fn GetProcessWindowStation() -> *mut c_void;
            fn GetUserObjectInformationW(handle: *mut c_void, index: i32,
                info: *mut c_void, length: u32, needed: *mut u32) -> i32;
        }
        let mut name = [0u16; 128];
        let mut needed = 0;
        assert_ne!(unsafe { GetUserObjectInformationW(GetProcessWindowStation(), 2,
            name.as_mut_ptr().cast(), (name.len() * 2) as u32, &mut needed) }, 0);
        let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
        assert!(String::from_utf16_lossy(&name[..end]).starts_with("ListenerNativeReplay_"));
        let process_id: u32 = std::env::var("LISTENER_PRIVATE_TSF_PID").unwrap().parse().unwrap();
        let thread_id: u32 = std::env::var("LISTENER_PRIVATE_TSF_TID").unwrap().parse().unwrap();
        let mut clipboard = arboard::Clipboard::new().unwrap();
        clipboard.set_text("private previous clipboard").unwrap();
        let inserter = crate::insertion::TextInserter::new();
        let origin = inserter.clipboard_retention_ticket();
        let sequence_before = unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() };
        let server = WindowsImeIpcServer::new();
        for text in ["合成 test 𝄞", " 第二段"] {
            let result = server.submit_text_exact(ImeSubmitRequest {
                session_id: "private-synthetic".into(), text: text.into(), created_at: "test".into(),
                target: Some(ImeSubmitTarget { process_id, thread_id }),
            }, || true).await;
            assert_eq!(result.unwrap(), ImeSubmitStatus::Committed);
        }
        for _ in 0..2 {
            let result = server.submit_terminal_exact(ImeSubmitRequest {
                session_id: "private-synthetic".into(), text: "。".into(), created_at: "test".into(),
                target: Some(ImeSubmitTarget { process_id, thread_id }),
            }, "合成 test 𝄞 第二段".into(), || true).await;
            assert_eq!(result.unwrap(), ImeSubmitStatus::Committed);
        }
        let sequence_after = unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() };
        assert_eq!(sequence_before, sequence_after, "real-time TSF must not borrow the clipboard");
        let full = "合成 test 𝄞 第二段。";
        assert_eq!(inserter.retain_recording_clipboard(origin, uuid::Uuid::new_v4(), full, false),
            crate::insertion::RetentionResult::Stored);
        assert_eq!(clipboard.get_text().unwrap(), full);

        // A later copy must win even if it happens after a new recording began.
        let next_origin = inserter.clipboard_retention_ticket();
        clipboard.set_text("private newer user copy").unwrap();
        assert_eq!(inserter.retain_recording_clipboard(next_origin, uuid::Uuid::new_v4(), full, false),
            crate::insertion::RetentionResult::Superseded);
        assert_eq!(clipboard.get_text().unwrap(), "private newer user copy");
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    #[ignore = "requires an isolated station and a two-context synthetic TSF host"]
    async fn exact_streaming_rejects_private_tsf_context_switch() {
        use std::ffi::c_void;
        #[link(name = "user32")]
        extern "system" {
            fn GetProcessWindowStation() -> *mut c_void;
            fn GetUserObjectInformationW(handle: *mut c_void, index: i32,
                info: *mut c_void, length: u32, needed: *mut u32) -> i32;
        }
        let mut name = [0u16; 128];
        let mut needed = 0;
        assert_ne!(unsafe { GetUserObjectInformationW(GetProcessWindowStation(), 2,
            name.as_mut_ptr().cast(), (name.len() * 2) as u32, &mut needed) }, 0);
        let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
        assert!(String::from_utf16_lossy(&name[..end]).starts_with("ListenerNativeReplay_"));
        let process_id = std::env::var("LISTENER_PRIVATE_TSF_PID").unwrap().parse().unwrap();
        let thread_id = std::env::var("LISTENER_PRIVATE_TSF_TID").unwrap().parse().unwrap();
        let switch_file = std::env::var("LISTENER_PRIVATE_TSF_SWITCH_FILE").unwrap();
        let switch_ack = std::env::var("LISTENER_PRIVATE_TSF_SWITCH_ACK").unwrap();
        let result = WindowsImeIpcServer::new().submit_text_exact(ImeSubmitRequest {
            session_id: "private-focus-switch".into(), text: "synthetic".into(), created_at: "test".into(),
            target: Some(ImeSubmitTarget { process_id, thread_id }),
        }, move || {
            std::fs::write(&switch_file, "switch").unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while !std::path::Path::new(&switch_ack).exists() && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(std::path::Path::new(&switch_ack).exists());
            true // Model an unchanged root HWND and GUI thread.
        }).await;
        assert!(matches!(result, Err(WindowsImeIpcError::NotDispatched)), "{result:?}");
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    #[ignore = "requires an isolated station and a synthetic TSF host without focus"]
    async fn exact_streaming_no_native_document_is_safe_to_fallback() {
        use std::ffi::c_void;
        #[link(name = "user32")]
        extern "system" {
            fn GetProcessWindowStation() -> *mut c_void;
            fn GetUserObjectInformationW(handle: *mut c_void, index: i32,
                info: *mut c_void, length: u32, needed: *mut u32) -> i32;
        }
        let mut name = [0u16; 128];
        let mut needed = 0;
        assert_ne!(unsafe { GetUserObjectInformationW(GetProcessWindowStation(), 2,
            name.as_mut_ptr().cast(), (name.len() * 2) as u32, &mut needed) }, 0);
        let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
        assert!(String::from_utf16_lossy(&name[..end]).starts_with("ListenerNativeReplay_"));
        let process_id: u32 = std::env::var("LISTENER_PRIVATE_TSF_PID").unwrap().parse().unwrap();
        let thread_id: u32 = std::env::var("LISTENER_PRIVATE_TSF_TID").unwrap().parse().unwrap();
        let result = WindowsImeIpcServer::new().submit_text_exact(ImeSubmitRequest {
            session_id: "private-no-focus".into(), text: "synthetic".into(), created_at: "test".into(),
            target: Some(ImeSubmitTarget { process_id, thread_id }),
        }, || true).await;
        assert!(matches!(result, Err(WindowsImeIpcError::NoReadyClient)));
    }
}

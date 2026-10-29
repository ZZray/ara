//! Real normal ARA children + approved private Bun primitives. FakeUpstream
//! controls model tool requests only; image/PDF process effects remain real.
//! Root must supply explicit D runtime, fixtures and output roots. Browser group
//! is ignored by default and requires Root's explicit Chromium execution gate.
use ara_cli::host_browser::{HostBrowser, HostBrowserOptions, ReadHostSnapshot};
use ara_testkit::{
    FakeUpstream, Script,
    chunks::{done, finish, text, tool_call},
};
use ara_tools::read_image::{ImageCodecPort, ImageEncodeBatch, ImageEncoding, ReadImagePolicyPort};
use base64::Engine;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

const BIN: &str = env!("CARGO_BIN_EXE_ara");
const CASE_BOUND: Duration = Duration::from_secs(120);

fn root() -> PathBuf {
    let root: PathBuf = std::env::var_os("ARA_TEST_NATIVE_HOST_OUT")
        .expect("Root must bind native Host evidence output")
        .into();
    assert!(
        root.is_absolute()
            && root
                .to_string_lossy()
                .to_ascii_lowercase()
                .starts_with("d:")
    );
    std::fs::create_dir_all(&root).unwrap();
    root
}
fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        PathBuf::from(
            std::env::var_os("ARA_TEST_IMAGE_FIXTURE_DIR")
                .expect("Root native fixtures are required"),
        )
        .join(name),
    )
    .unwrap()
}
fn directory(label: &str) -> PathBuf {
    let path = root().join(format!("{label}-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&path).unwrap();
    path
}
fn sha(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn record(name: &str, value: Value) {
    std::fs::write(
        root().join(name),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .unwrap();
}
fn tool_response(id: &str, path: &str) -> Value {
    json!({"events":[tool_call(0,id,"read",&json!({"path":path}).to_string()),finish("tool_calls"),done()]})
}
fn answer() -> Value {
    json!({"events":[text("Native artifact inspected."),finish("stop"),done()]})
}
async fn upstream(responses: Vec<Value>) -> FakeUpstream {
    FakeUpstream::start(
        serde_json::from_value::<Script>(json!({"responses":responses})).unwrap(),
        None,
    )
    .await
    .unwrap()
}

// Test-framing boundary only: exact raw NDJSON remains independently owned.
// This is not an Agent content-type or production publication conversion.
fn profiler_wire_control_projection(value: &ara_rpc::WireValue) -> Value {
    use ara_rpc::WireValue;
    match value {
        WireValue::Null => Value::Null,
        WireValue::Bool(value) => json!(value),
        WireValue::Number(value) => json!(value),
        WireValue::String(value) => json!(String::from_utf16_lossy(value.units())),
        WireValue::Array(values) => Value::Array(values.iter().map(profiler_wire_control_projection).collect()),
        WireValue::Object(values) => Value::Object(values.iter().map(|(key, value)|
            (String::from_utf16_lossy(key.units()), profiler_wire_control_projection(value))).collect()),
    }
}

struct RpcChild {
    child: Child,
    stdin: Option<ChildStdin>,
    frames: mpsc::Receiver<Value>,
    seen: Vec<Value>,
    /// Preserve source NDJSON for exact UTF16 profiler assertions. Existing
    /// ordinary serde framing remains the fast control path.
    raw_frames: Arc<Mutex<Vec<String>>>,
    stderr: Arc<Mutex<String>>,
    readers: Vec<std::thread::JoinHandle<()>>,
    deadline: Instant,
    browser_expected: bool,
    media_temp: PathBuf,
}
impl RpcChild {
    fn start(upstream: &FakeUpstream, project: &Path, label: &str) -> Self {
        Self::start_with_browser(upstream, project, label, true)
    }
    fn start_with_browser(
        upstream: &FakeUpstream,
        project: &Path,
        label: &str,
        browser_expected: bool,
    ) -> Self {
        let home = directory(&format!("home-{label}"));
        let media_temp = home.join("media-workspaces");
        // Chrome requires Windows known AppData locations even for an explicit
        // isolated profile; keep both under this private D test home.
        let local_app_data = home.join("AppData/Local");
        let roaming_app_data = home.join("AppData/Roaming");
        std::fs::create_dir_all(&local_app_data).unwrap();
        std::fs::create_dir_all(&roaming_app_data).unwrap();
        let mut command = Command::new(BIN);
        command.env_clear();
        for key in [
            "PATH",
            "Path",
            "SystemRoot",
            "WINDIR",
            "COMSPEC",
            "PATHEXT",
            "TEMP",
            "TMP",
            "ARA_PDF_BUN",
            "ARA_PDF_PACKAGE_ROOT",
            "ARA_PDF_HOST_ROOT",
            "ARA_PDF_TEMP",
            "PUPPETEER_EXECUTABLE_PATH",
            "PI_NATIVE_VARIANT",
            "OMP_NO_WEBP",
        ] {
            if !browser_expected
                && (key.starts_with("ARA_PDF_") || key == "PUPPETEER_EXECUTABLE_PATH")
            {
                continue;
            }
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        if !browser_expected {
            let ffmpeg = media_binary("ARA_TEST_FFMPEG");
            let ffprobe = media_binary("ARA_TEST_FFPROBE");
            let mut paths = vec![
                ffmpeg.parent().unwrap().to_path_buf(),
                ffprobe.parent().unwrap().to_path_buf(),
            ];
            if let Some(existing) = std::env::var_os("PATH") {
                paths.extend(std::env::split_paths(&existing));
            }
            command.env("PATH", std::env::join_paths(paths).unwrap());
        }
        command
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("LOCALAPPDATA", &local_app_data)
            .env("APPDATA", &roaming_app_data)
            .env("ARA_HOME", &home)
            .env("ARA_VIDEO_TEMP", &media_temp)
            .env("ARA_API_KEY", "sk-native-host-controlled-fixture")
            .args([
                "--mode",
                "rpc",
                "--model",
                "fake-model",
                "--base-url",
                &upstream.base_url(),
                "--no-skills",
                "--tools",
                "read",
                "--compact-threshold",
                "0",
                "--cwd",
            ])
            .arg(project)
            .arg("--session-dir")
            .arg(home.join("sessions"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let mut error_output = child.stderr.take().unwrap();
        let (sender, frames) = mpsc::channel();
        let raw_frames = Arc::new(Mutex::new(Vec::<String>::new()));
        let raw_output = raw_frames.clone();
        let profiler_framing = label == "profiler";
        let output_reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                if profiler_framing { raw_output.lock().unwrap().push(line.clone()); }
                let frame = match serde_json::from_str(&line) {
                    Ok(frame) => frame,
                    Err(_) if profiler_framing => match ara_rpc::WireValue::parse(&line) {
                        // Control fields are projected only after retaining the
                        // full exact wire. The profiler group reads raw_frames.
                        Ok(value) => profiler_wire_control_projection(&value),
                        Err(_) => break,
                    },
                    Err(_) => break,
                };
                if sender.send(frame).is_err() {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        let captured = stderr.clone();
        let error_reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = error_output.read_to_end(&mut bytes);
            *captured.lock().unwrap() = String::from_utf8_lossy(&bytes).into_owned();
        });
        let mut owner = Self {
            child,
            stdin,
            frames,
            seen: Vec::new(),
            raw_frames,
            stderr,
            readers: vec![output_reader, error_reader],
            deadline: Instant::now() + CASE_BOUND,
            browser_expected,
            media_temp,
        };
        owner.until(|frame| frame["type"] == "ready");
        owner
    }
    fn send(&mut self, frame: Value) {
        writeln!(self.stdin.as_mut().unwrap(), "{frame}").unwrap();
        self.stdin.as_mut().unwrap().flush().unwrap();
    }
    fn until(&mut self, matches: impl Fn(&Value) -> bool) -> Value {
        loop {
            let frame = self
                .frames
                .recv_timeout(self.deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| {
                    panic!("normal ARA wait {error}: {}", self.stderr.lock().unwrap())
                });
            self.seen.push(frame.clone());
            if matches(&frame) {
                return frame;
            }
        }
    }
    fn run(&mut self, run_id: &str, tool_id: &str) -> Value {
        self.send(
            json!({"id":run_id,"type":"prompt","message":"Read the selected actual artifact"}),
        );
        let receipt = self
            .until(|frame| frame["type"] == "tool_execution_end" && frame["toolCallId"] == tool_id);
        self.until(|frame| frame["type"] == "agent_end");
        receipt
    }
    fn finish(&mut self) -> Value {
        self.stdin.take();
        while let Ok(frame) = self
            .frames
            .recv_timeout(self.deadline.saturating_duration_since(Instant::now()))
        {
            self.seen.push(frame);
        }
        let exit = loop {
            if let Some(exit) = self.child.try_wait().unwrap() {
                break exit;
            }
            assert!(
                Instant::now() < self.deadline,
                "normal ARA did not settle EOF"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        for reader in self.readers.drain(..) {
            reader.join().unwrap();
        }
        let stderr = self.stderr.lock().unwrap().clone();
        assert!(exit.success(), "normal ARA failed cleanup: {stderr}");
        if self.browser_expected {
            assert!(
                stderr.contains("PDF Host cleanup"),
                "normal ARA lacks actual wrapper cleanup receipt: {stderr}"
            );
        } else {
            assert!(
                !stderr.contains("PDF Host cleanup"),
                "media-only child must not acquire a Browser owner: {stderr}"
            );
        }
        assert!(
            stderr.contains("READ media Host cleanup completed"),
            "normal ARA lacks actual media settlement: {stderr}"
        );
        assert!(
            std::fs::read_dir(&self.media_temp)
                .unwrap()
                .next()
                .is_none(),
            "normal ARA retained media outputs after successful EOF settlement"
        );
        assert_eq!(
            self.seen
                .iter()
                .filter(|frame| frame["type"] == "session_shutdown")
                .count(),
            1
        );
        json!({"pid":self.child.id(),"exit":exit.code(),"stderr":stderr,"frames":self.seen})
    }
}
impl Drop for RpcChild {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            self.stdin.take();
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_image_codec_normal_cli_failure_and_lifetime_group() {
    let project = directory("image-project");
    let input = fixture("smallPng.bin");
    std::fs::write(project.join("native.png"), &input).unwrap();
    std::fs::write(
        project.join("corrupt.png"),
        b"\x89PNG\r\n\x1a\ninvalid native raster",
    )
    .unwrap();
    let options = HostBrowserOptions::from_environment()
        .unwrap()
        .expect("approved private Host options required");
    let host = HostBrowser::start(options, &project).await.unwrap();
    let session = host
        .session(&project, ReadHostSnapshot::default())
        .await
        .unwrap();
    session
        .bind_session("native-codec-fixture-session")
        .unwrap();
    let metadata = session
        .decoded_metadata(&input, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!((metadata.width, metadata.height), (200.0, 200.0));
    session
        .decode_probe_1x1(&input, CancellationToken::new())
        .await
        .unwrap();
    let encoded = session
        .encode_many(
            &input,
            ImageEncodeBatch {
                width: 100.0,
                height: 100.0,
                quality: 70.0,
                formats: vec![ImageEncoding::Png, ImageEncoding::Jpeg, ImageEncoding::WebP],
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        encoded
            .iter()
            .map(|value| value.mime_type.as_str())
            .collect::<Vec<_>>(),
        ["image/png", "image/jpeg", "image/webp"]
    );
    let invalid = session
        .encode_many(
            &input,
            ImageEncodeBatch {
                width: f64::NAN,
                height: f64::INFINITY,
                quality: f64::NEG_INFINITY,
                formats: vec![ImageEncoding::Png],
            },
            CancellationToken::new(),
        )
        .await;
    // Direct pinned Bun 1.4.0 native oracle succeeds for these values. Exact
    // bytes prove the tagged non-finite values reached the native primitive.
    let nonfinite = invalid.unwrap();
    assert_eq!(nonfinite.len(), 1);
    assert_eq!(nonfinite[0].buffer.len(), 6735);
    assert_eq!(sha(&nonfinite[0].buffer), "dd278ea0aa3a85ccd8d3592b19784b1c36caefed3363854f5e711d74aea573c5");
    let corrupt_codec = session.encode_many(
        b"actual corrupt raster bytes",
        ImageEncodeBatch { width: 1.0, height: 1.0, quality: 70.0, formats: vec![ImageEncoding::Png] },
        CancellationToken::new(),
    ).await;
    assert!(corrupt_codec.is_err(), "native corrupt codec must remain an error");
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(session.decoded_metadata(&input, cancelled).await.is_err());
    assert_eq!(
        session.current_state().no_webp_env,
        std::env::var("OMP_NO_WEBP").ok()
    );
    let wrapper = session.wrapper_pid();
    drop(session);
    let close = host.close().await.unwrap();
    assert_eq!(close.len(), 1);
    assert_eq!(close[0].cleanup["sharedBrowserStopped"], false);
    assert_eq!(close[0].cleanup["remainingBrowsers"], 0);
    assert_eq!(close[0].cleanup["remainingTabs"], 0);
    let up = upstream(vec![
        tool_response("image-real", "native.png"),
        answer(),
        tool_response("image-corrupt", "corrupt.png"),
        answer(),
    ])
    .await;
    let mut child = RpcChild::start(&up, &project, "images");
    let image = child.run("run-image", "image-real");
    assert_ne!(image["isError"], true, "{image}");
    assert!(
        image["result"]["content"]
            .as_array()
            .is_some_and(|items| items.iter().any(|block| block["type"] == "image")),
        "actual normal CLI must return image content: {image}"
    );
    let corruption = child.run("run-corrupt", "image-corrupt");
    assert_eq!(
        corruption["isError"], true,
        "corrupt actual raster cannot be tool success: {corruption}"
    );
    let child_receipt = child.finish();
    record(
        "native-image-host-group.json",
        json!({"normalBinary":BIN,"normalBinarySha256":sha(&std::fs::read(BIN).unwrap()),"wrapperPid":wrapper,"nativeMetadata":{"width":metadata.width,"height":metadata.height,"format":metadata.format},"encoded":encoded.iter().map(|value|json!({"mime":value.mime_type,"bytes":value.buffer.len(),"sha256":sha(&value.buffer)})).collect::<Vec<_>>(),"nonfiniteNativeResult":{"bytes":nonfinite[0].buffer.len(),"sha256":sha(&nonfinite[0].buffer)},"corruptCodecFailure":corrupt_codec.unwrap_err().0,"cleanup":close[0].cleanup,"child":child_receipt,"browserExecuted":false,"model":"controlled-fake-upstream-only"}),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Root explicit actual Chromium PDF/shared-client gate required"]
async fn native_pdf_normal_cli_two_clients_preserve_shared_browser_and_cleanup() {
    assert_eq!(std::env::var("ARA_TEST_NATIVE_BROWSER").as_deref(), Ok("1"));
    assert!(cfg!(windows), "original Chrome HANDLE identity gate requires Windows; unsupported is not a skip");
    let project = directory("pdf-project");
    write_pdf(&project.join("native.pdf"));
    let up_a = upstream(vec![
        tool_response("pdf-a", "native.pdf:p1-img0.png"),
        answer(),
    ])
    .await;
    let up_b = upstream(vec![
        tool_response("pdf-b1", "native.pdf:p1-img0.png"),
        answer(),
        tool_response("pdf-b2", "native.pdf:p2-img0.png"),
        answer(),
    ])
    .await;
    let mut a = RpcChild::start(&up_a, &project, "pdf-a");
    let mut b = RpcChild::start(&up_b, &project, "pdf-b");
    assert_ne!(a.child.id(), b.child.id());
    let first = a.run("pdf-a-run", "pdf-a");
    assert_pdf_image(&first);
    let before = b.run("pdf-b1-run", "pdf-b1");
    assert_pdf_image(&before);
    let page_one = save_image("native-pdf-page-one", &before);
    let chrome_before_a_close = chrome_daemon_snapshot(&project);
    let original_chrome = chrome_process_gate::OriginalChrome::bind(&chrome_before_a_close);
    let handle_before_a_close = original_chrome.wait(0);
    let mut identity_receipt = json!({"beforeAClose":chrome_before_a_close,
        "boundOriginalChromeImage":original_chrome.image,"originalHandleBeforeAClose":handle_before_a_close});
    record("native-pdf-chrome-gate.json", identity_receipt.clone());
    assert_eq!(handle_before_a_close, chrome_process_gate::OriginalChrome::alive_status());
    let a_close = a.finish();
    let handle_after_a_close = original_chrome.wait(0);
    identity_receipt["originalHandleAfterAClose"] = json!(handle_after_a_close);
    record("native-pdf-chrome-gate.json", identity_receipt.clone());
    assert_eq!(handle_after_a_close, chrome_process_gate::OriginalChrome::alive_status(),
        "original Chrome died when A exited; replacement Chrome cannot satisfy shared lifetime");
    let chrome_after_a_close = chrome_daemon_snapshot(&project);
    identity_receipt["afterAClose"] = chrome_after_a_close.clone();
    record("native-pdf-chrome-gate.json", identity_receipt.clone());
    assert_same_chrome(&chrome_before_a_close, &chrome_after_a_close);
    let after = b.run("pdf-b2-run", "pdf-b2");
    assert_pdf_image(&after);
    let page_two = save_image("native-pdf-page-two", &after);
    assert_ne!(
        page_one["sha256"], page_two["sha256"],
        "different generated page colors must produce different actual images"
    );
    let chrome_after_b_read = chrome_daemon_snapshot(&project);
    let handle_after_b_read = original_chrome.wait(0);
    identity_receipt["afterBPageTwoRead"] = chrome_after_b_read.clone();
    identity_receipt["originalHandleAfterBPageTwoRead"] = json!(handle_after_b_read);
    record("native-pdf-chrome-gate.json", identity_receipt.clone());
    assert_same_chrome(&chrome_before_a_close, &chrome_after_b_read);
    assert_eq!(handle_after_b_read, chrome_process_gate::OriginalChrome::alive_status());
    let b_close = b.finish();
    // Fixed broker idle grace is 3000ms. Keep the original process object bound
    // through last-client exit so stale metadata or PID reuse cannot pass.
    let chrome_exit_started = Instant::now();
    let handle_after_b_close = original_chrome.wait(10_000);
    identity_receipt["originalHandleAfterBClose"] = json!(handle_after_b_close);
    identity_receipt["lastClientExitToChromeWaitMs"] = json!(chrome_exit_started.elapsed().as_millis());
    identity_receipt["lastClientChromeExitBoundMs"] = json!(10_000);
    record("native-pdf-chrome-gate.json", identity_receipt.clone());
    assert_eq!(handle_after_b_close, chrome_process_gate::OriginalChrome::exited_status(),
        "last client exited but original broker-owned Chrome did not exit within 10s");
    record(
        "native-pdf-host-group.json",
        json!({"normalBinary":BIN,"normalBinarySha256":sha(&std::fs::read(BIN).unwrap()),"project":project,"pageOne":page_one,"pageTwo":page_two,"clientA":a_close,"clientB":b_close,"secondClientContinuedAfterFirstClose":true,"browserExecuted":true,"sameChrome":identity_receipt,"model":"controlled-fake-upstream-only"}),
    );
}
fn assert_pdf_image(receipt: &Value) {
    assert_ne!(
        receipt["isError"], true,
        "real PDF render failed: {receipt}"
    );
    assert!(
        receipt["result"]["content"]
            .as_array()
            .is_some_and(|items| items.iter().any(|block| block["type"] == "image")),
        "actual PDF image block required: {receipt}"
    );
}
fn save_image(label: &str, receipt: &Value) -> Value {
    let image = receipt["result"]["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|block| block["type"] == "image")
        .unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(
            image["data"]
                .as_str()
                .expect("actual image base64 required"),
        )
        .unwrap();
    let suffix = match image["mimeType"].as_str().unwrap() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        other => panic!("unexpected screenshot MIME {other}"),
    };
    let path = root().join(format!("{label}.{suffix}"));
    std::fs::write(&path, &bytes).unwrap();
    json!({"sourcePath":path,"bytes":bytes.len(),"sha256":sha(&bytes),"mimeType":image["mimeType"]})
}
fn write_pdf(path: &Path) {
    // Generated real two-page rasterizable PDF, not a stub/native codec fixture.
    let blue = "0 0 1 rg 0 0 1200 900 re f\n";
    let red = "1 0 0 rg 0 0 1200 900 re f\n";
    let bodies = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1200 900] /Contents 4 0 R >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{blue}endstream", blue.len()),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1200 900] /Contents 6 0 R >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{red}endstream", red.len()),
    ];
    let mut pdf = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (index, body) in bodies.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.push_str(&format!("{} 0 obj\n{}\nendobj\n", index + 1, body));
    }
    let xref = pdf.len();
    pdf.push_str("xref\n0 7\n0000000000 65535 f \n");
    for offset in offsets {
        pdf.push_str(&format!("{offset:010} 00000 n \n"));
    }
    pdf.push_str(&format!(
        "trailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
    ));
    std::fs::write(path, pdf).unwrap();
}

fn media_binary(key: &str) -> PathBuf {
    let path: PathBuf = std::env::var_os(key)
        .expect("Root must pin actual D media executables")
        .into();
    assert!(
        path.is_absolute()
            && path
                .to_string_lossy()
                .to_ascii_lowercase()
                .starts_with("d:")
            && path.is_file()
    );
    path
}
fn media_process(binary: &Path, args: &[&str]) -> std::process::Output {
    let mut child = Command::new(binary)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let err = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let bound = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= bound {
            let _ = child.kill();
            let _ = child.wait();
            let _ = out.join();
            let _ = err.join();
            panic!("actual media fixture process exceeded 30 seconds");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let output = std::process::Output {
        status,
        stdout: out.join().unwrap(),
        stderr: err.join().unwrap(),
    };
    assert!(
        output.status.success(),
        "actual media command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}
fn actual_image_pixels(ffmpeg: &Path, image: &Value, label: &str) -> Value {
    let path = PathBuf::from(image["sourcePath"].as_str().unwrap());
    let pixels = media_process(
        ffmpeg,
        &[
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            path.to_str().unwrap(),
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "gray",
            "-",
        ],
    )
    .stdout;
    assert!(
        !pixels.is_empty(),
        "actual returned image cannot decode to zero pixels"
    );
    let mean = pixels.iter().map(|byte| u64::from(*byte)).sum::<u64>() as f64 / pixels.len() as f64;
    assert!(mean > 5.0, "actual returned video image is blank");
    let output = root().join(format!("{label}.gray"));
    std::fs::write(&output, &pixels).unwrap();
    json!({"path":output,"bytes":pixels.len(),"sha256":sha(&pixels),"meanGray":mean})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_video_normal_cli_media_and_settlement_group() {
    let ffmpeg = media_binary("ARA_TEST_FFMPEG");
    let ffprobe = media_binary("ARA_TEST_FFPROBE");
    let version = media_process(&ffmpeg, &["-version"]);
    assert!(
        String::from_utf8_lossy(&version.stdout).starts_with("ffmpeg version 9.0.1"),
        "Root must supply pinned ffmpeg 9.0.1"
    );
    let probe_version = media_process(&ffprobe, &["-version"]);
    assert!(
        String::from_utf8_lossy(&probe_version.stdout).starts_with("ffprobe version 9.0.1"),
        "Root must supply pinned ffprobe 9.0.1"
    );
    assert_eq!(
        ffmpeg.parent(),
        ffprobe.parent(),
        "normal Host PATH must resolve both pinned media binaries from the same directory"
    );
    let project = directory("video-project");
    // Preserve native output dimensions and exercise no-Browser policy/runtime.
    std::fs::create_dir(project.join(".ara")).unwrap();
    std::fs::write(
        project.join(".ara/config.yml"),
        "images: {autoResize: false}\n",
    )
    .unwrap();
    let clip = project.join("native.mp4");
    let setup = media_process(
        &ffmpeg,
        &[
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x240:rate=30",
            "-t",
            "2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-y",
            clip.to_str().unwrap(),
        ],
    );
    std::fs::write(project.join("corrupt.mp4"), [0, 1, 2, 3, 4, 5]).unwrap();
    let up = upstream(vec![
        tool_response("video-frame", "native.mp4:12"),
        answer(),
        tool_response("video-sheet", "native.mp4"),
        answer(),
        tool_response("video-corrupt", "corrupt.mp4"),
        answer(),
    ])
    .await;
    let mut child = RpcChild::start_with_browser(&up, &project, "video", false);
    let frame = child.run("run-video-frame", "video-frame");
    assert_pdf_image(&frame);
    let frame_image = save_image("native-video-frame", &frame);
    let frame_pixels = actual_image_pixels(&ffmpeg, &frame_image, "native-video-frame");
    assert_eq!(
        frame_pixels["bytes"],
        320 * 240,
        "normal native frame must retain original dimensions"
    );
    let sheet = child.run("run-video-sheet", "video-sheet");
    assert_pdf_image(&sheet);
    let sheet_image = save_image("native-video-sheet", &sheet);
    let sheet_pixels = actual_image_pixels(&ffmpeg, &sheet_image, "native-video-sheet");
    assert_eq!(
        sheet_pixels["bytes"],
        960 * 480,
        "normal native contact sheet must be 3x2 actual 320x240 cells"
    );
    let corruption = child.run("run-video-corrupt", "video-corrupt");
    assert_eq!(
        corruption["isError"], true,
        "actual corrupt video cannot become native tool success: {corruption}"
    );
    assert!(
        corruption.to_string().contains("Could not probe video 'corrupt.mp4':"),
        "normal corrupt video must fail at actual ffprobe: {corruption}"
    );
    let receipt = child.finish();
    record(
        "native-video-host-group.json",
        json!({"normalBinary":BIN,"normalBinarySha256":sha(&std::fs::read(BIN).unwrap()),"ffmpeg":ffmpeg,"ffprobe":ffprobe,"ffmpegVersion":String::from_utf8_lossy(&version.stdout),"ffprobeVersion":String::from_utf8_lossy(&probe_version.stdout),"fixture":{"path":clip,"seconds":2,"sha256":sha(&std::fs::read(&clip).unwrap()),"setupExit":setup.status.code()},"frameImage":frame_image,"framePixels":frame_pixels,"sheetImage":sheet_image,"sheetPixels":sheet_pixels,"corruptReceipt":corruption,"child":receipt,"browserExecuted":false,"effects":"real production Host ffmpeg/ffprobe and normal native READ","model":"controlled-fake-upstream-only","unknownSettlementFaultExecuted":false}),
    );
}

// One canonical project scope and the original OS process object, not merely a
// PID that could be recycled after a broker silently recreates Chrome.
fn chrome_daemon_snapshot(project: &Path) -> Value {
    let project = std::fs::canonicalize(project).unwrap();
    let runtime = PathBuf::from(
        std::env::var_os("ARA_PDF_HOST_ROOT").expect("private PDF runtime required"),
    ).join("run/daemons");
    let mut matches = Vec::new();
    for entry in std::fs::read_dir(&runtime).unwrap() {
        let entry = entry.unwrap();
        let scope_file = entry.path().join("scope.json");
        if !scope_file.is_file() { continue; }
        let scope: Value = serde_json::from_slice(&std::fs::read(&scope_file).unwrap()).unwrap();
        let Some(scope_project) = scope["projectDir"].as_str() else { continue; };
        let Ok(scope_project) = std::fs::canonicalize(scope_project) else { continue; };
        if !same_canonical_path(&project, &scope_project) { continue; }
        let meta_file = entry.path().join("daemons/ara.browser.headless/meta.json");
        let meta: Value = serde_json::from_slice(&std::fs::read(&meta_file).unwrap()).unwrap();
        let daemon = &meta["daemon"];
        assert_eq!(daemon["state"], "ready", "actual Chrome daemon not ready: {meta}");
        let id = daemon["id"].as_str().expect("actual Chrome daemon id required");
        assert!(!id.is_empty());
        let pid = daemon["pid"].as_u64().expect("actual Chrome daemon pid required");
        assert!(pid > 0 && pid <= u32::MAX as u64);
        matches.push(json!({"scopePath":entry.path(),"scope":scope,"metaPath":meta_file,
            "daemonId":id,"pid":pid,"metadata":meta}));
    }
    assert_eq!(matches.len(), 1, "expected one canonical project Chrome scope: {matches:?}");
    matches.pop().unwrap()
}
fn same_canonical_path(left: &Path, right: &Path) -> bool {
    if cfg!(windows) {
        left.to_string_lossy().eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}
fn assert_same_chrome(original: &Value, current: &Value) {
    for key in ["scopePath", "daemonId", "pid"] {
        assert_eq!(original[key], current[key], "Chrome identity changed at {key}: {original} -> {current}");
    }
}

#[cfg(windows)]
mod chrome_process_gate {
    use super::*;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::Threading::{OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE},
    };
    pub struct OriginalChrome {
        handle: OwnedHandle,
        pub image: PathBuf,
    }
    impl OriginalChrome {
        pub fn bind(snapshot: &Value) -> Self {
            let pid = u32::try_from(snapshot["pid"].as_u64().unwrap()).unwrap();
            // SAFETY: access is query/synchronize only; a successful owned
            // process HANDLE is closed by OwnedHandle, and never used to kill.
            let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            assert!(!raw.is_null(), "cannot bind original Chrome {pid}: {}", std::io::Error::last_os_error());
            let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
            let mut image = vec![0_u16; 32768];
            let mut length = image.len() as u32;
            let ok = unsafe { QueryFullProcessImageNameW(handle.as_raw_handle(), 0, image.as_mut_ptr(), &mut length) };
            assert_ne!(ok, 0, "original Chrome image unknown: {}", std::io::Error::last_os_error());
            let image = PathBuf::from(String::from_utf16(&image[..length as usize]).unwrap());
            let expected = PathBuf::from(std::env::var_os("PUPPETEER_EXECUTABLE_PATH")
                .expect("approved Chromium executable required"));
            assert!(same_canonical_path(&std::fs::canonicalize(&image).unwrap(),
                &std::fs::canonicalize(&expected).unwrap()),
                "original daemon image is not approved Chromium: {image:?} vs {expected:?}");
            Self { handle, image }
        }
        pub fn wait(&self, milliseconds: u32) -> u32 {
            let status = unsafe { WaitForSingleObject(self.handle.as_raw_handle(), milliseconds) };
            assert!(status == WAIT_OBJECT_0 || status == WAIT_TIMEOUT,
                "original Chrome wait is unknown: status={status}, error={}", std::io::Error::last_os_error());
            status
        }
        pub fn alive_status() -> u32 { WAIT_TIMEOUT }
        pub fn exited_status() -> u32 { WAIT_OBJECT_0 }
    }
}
#[cfg(not(windows))]
mod chrome_process_gate {
    use super::*;
    pub struct OriginalChrome { pub image: PathBuf }
    impl OriginalChrome {
        pub fn bind(_: &Value) -> Self { panic!("Windows original Chrome HANDLE gate unsupported on this platform") }
        pub fn wait(&self, _: u32) -> u32 { panic!("Windows original Chrome HANDLE gate unsupported on this platform") }
        pub fn alive_status() -> u32 { panic!("Windows original Chrome HANDLE gate unsupported on this platform") }
        pub fn exited_status() -> u32 { panic!("Windows original Chrome HANDLE gate unsupported on this platform") }
    }
}


/// Original read-local-image.test.ts:164-172 direct handler assertion group.
/// No ReadTool/fake ContentUriPort substitutes for the actual Session owner.
#[tokio::test]
async fn native_local_file_resource_original08_and_source_branches() {
    use ara_cli::{host_edit_session::HostEditSession, session_artifacts::SessionArtifacts};
    use ara_session::SessionJournal;
    let project = directory("direct-local-project");
    let journal = SessionJournal::create(&project, &project).unwrap();
    let artifacts = SessionArtifacts::for_journal(&journal);
    assert!(journal.is_persistent() && artifacts.directory().is_some());
    let host = HostEditSession::new(directory("direct-local-home"), Default::default());
    assert_eq!(host.resolve_local_file_resource("local://archive.zip", CancellationToken::new()).await.unwrap_err(),
        "No session - local:// unavailable");
    host.bind(&journal, artifacts.clone()).unwrap();
    let local = host.local_root().unwrap();
    assert_eq!(local, ara_cli::mcp_native::local::session_root(artifacts.directory(), journal.session_id()).unwrap());
    std::fs::create_dir_all(&local).unwrap();
    std::fs::write(local.join("archive.zip"), [0, 1, 2, 3, 4, 5]).unwrap();
    let resource = host.resolve_local_file_resource("local://archive.zip", CancellationToken::new()).await.unwrap().unwrap();
    assert!(resource.content.contains("Cannot read binary local:// file"));
    assert!(resource.content.contains("archive.zip"));
    assert!(!resource.content.contains('\u{0000}'));
    assert_eq!(resource.url, "local://archive.zip");
    assert_eq!(resource.size, resource.content.len());
    assert_eq!(resource.content_type, "text/plain");
    assert_eq!(resource.notes, ["Use write path local://<file> to persist large intermediate artifacts across turns."]);
    assert_eq!(resource.source_path, ara_tools::internal_urls::search_path_string(&std::fs::canonicalize(local.join("archive.zip")).unwrap()));
    let mut observed = vec![serde_json::to_value(&resource).unwrap()];
    for (name, bytes, fragment, kind) in [
        ("ASCII.ZIP", vec![b'x'; 1280], "1.3 KiB", "text/plain"),
        ("unknown.dat", vec![0, 1], "content is not valid UTF-8 text", "text/plain"),
        ("invalid.dat", vec![0xff], "content is not valid UTF-8 text", "text/plain"),
        ("notes.MD", "# 中文\n".as_bytes().to_vec(), "# 中文", "text/markdown"),
        ("state.JSON", b"{}".to_vec(), "{}", "application/json"),
        ("Dockerfile.md", b"FROM scratch".to_vec(), "FROM scratch", "text/plain"),
        (".env.md", b"A=B".to_vec(), "A=B", "text/plain"),
        ("large.txt", vec![b'x'; 1048577], "1.0 MiB exceeds 1.0 MiB", "text/plain"),
    ] {
        std::fs::write(local.join(name), &bytes).unwrap();
        let actual = host.resolve_local_file_resource(&format!("local://{name}"), CancellationToken::new()).await.unwrap().unwrap();
        assert!(actual.content.contains(fragment), "{name}: {}", actual.content);
        assert_eq!(actual.content_type, kind);
        assert_eq!(actual.size, actual.content.len());
        observed.push(serde_json::to_value(&actual).unwrap());
    }
    let cancelled = CancellationToken::new(); cancelled.cancel();
    assert_eq!(host.resolve_local_file_resource("local://missing.txt", cancelled).await.unwrap_err(), "local:// resource cancelled");
    assert!(host.resolve_local_file_resource("local://", CancellationToken::new()).await.unwrap().is_none());
    std::fs::create_dir(local.join("directory")).unwrap();
    assert!(host.resolve_local_file_resource("local://directory", CancellationToken::new()).await.unwrap().is_none());
    record("direct-local-file-resource.json", json!({"original":"read-local-image.original08", "sessionId":journal.session_id(),
        "journal":journal.path(), "artifacts":artifacts.directory(), "localRoot":local, "observed":observed,
        "boundaries":{"unbound":"error","preCancel":"error","listing":"None/open","directory":"None/open"}}));
}

// Actual Windows OS pipe fault at the production HostPort layer. This is not
// a normal CLI failure receipt and does not replace native pixel acceptance.
#[cfg(windows)]
mod media_unknown_os {
    use super::*;
    use anyhow::{Context, Result, bail, ensure};
    use ara_cli::host_media::HostMediaPort;
    use ara_tools::read_video_port::{
        MediaBinary, MediaCommand, ReadVideoHostPort, ReadVideoWorkspace,
    };
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, FILETIME, HANDLE, WAIT_OBJECT_0},
        System::{
            Console::{GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE},
            Threading::{
                GetCurrentProcess, GetProcessTimes, OpenProcess, PROCESS_DUP_HANDLE,
                PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW,
                WaitForSingleObject,
            },
        },
    };

    const HELPER: &str = "native_media_pipe_fixture_child";
    const PIPE_BYTES: usize = 128 * 1024;

    fn video<T>(
        result: std::result::Result<T, ara_tools::read_video_port::VideoError>,
    ) -> Result<T> {
        result.map_err(|error| anyhow::anyhow!("actual Host media error: {error:?}"))
    }

    fn creation_time(process: HANDLE) -> Result<u64> {
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: all outputs are initialized FILETIME values; process is a
        // live borrowed handle or the documented current-process pseudo handle.
        ensure!(
            unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) }
                != 0,
            "GetProcessTimes: {}",
            std::io::Error::last_os_error()
        );
        Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
    }

    fn owned(raw: HANDLE, operation: &str) -> Result<OwnedHandle> {
        ensure!(
            !raw.is_null() && raw as isize != -1,
            "{operation}: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: caller transfers a fresh successful OpenProcess/DuplicateHandle
        // result. This function is the only ownership conversion for that result.
        Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
    }

    fn process_image(process: HANDLE) -> Result<PathBuf> {
        let mut name = vec![0_u16; 32_768];
        let mut length = name.len() as u32;
        // SAFETY: name is a writable buffer of length supplied to the API.
        ensure!(
            unsafe { QueryFullProcessImageNameW(process, 0, name.as_mut_ptr(), &mut length) } != 0,
            "QueryFullProcessImageNameW: {}",
            std::io::Error::last_os_error()
        );
        Ok(PathBuf::from(String::from_utf16(&name[..length as usize])?).canonicalize()?)
    }

    fn duplicate_writer(process: HANDLE, raw: u64) -> Result<OwnedHandle> {
        let raw = usize::try_from(raw).context("writer handle does not fit this OS")? as HANDLE;
        let mut duplicate = std::ptr::null_mut();
        // SAFETY: process identity is held and verified before the handshake
        // release. raw names that live child's standard writer. The duplicate
        // is noninheritable and owned solely by this test process.
        ensure!(
            unsafe {
                DuplicateHandle(
                    process,
                    raw,
                    GetCurrentProcess(),
                    &mut duplicate,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS,
                )
            } != 0,
            "DuplicateHandle: {}",
            std::io::Error::last_os_error()
        );
        owned(duplicate, "duplicated writer")
    }

    fn atomic_record(path: &Path, value: &Value) -> Result<()> {
        let pending = path.with_extension("pending");
        std::fs::write(&pending, serde_json::to_vec(value)?)?;
        std::fs::rename(pending, path)?;
        Ok(())
    }

    pub(super) fn fixture() -> Result<()> {
        let executable = std::env::current_exe()?.canonicalize()?;
        ensure!(
            executable.file_stem().and_then(|v| v.to_str()) == Some("ffmpeg"),
            "this ignored helper must be explicitly launched as the private ffmpeg test copy"
        );
        let args = std::env::args().collect::<Vec<_>>();
        ensure!(
            args.windows(2)
                .any(|pair| pair[0] == "--exact" && pair[1] == HELPER)
                && args.iter().any(|arg| arg == "--ignored")
                && args.iter().any(|arg| arg == "--nocapture"),
            "missing explicit child protocol"
        );
        let logfile = args
            .windows(2)
            .find(|pair| pair[0] == "--logfile")
            .context("helper needs the standard libtest logfile workspace argument")?;
        let log = PathBuf::from(&logfile[1]);
        ensure!(log.is_absolute(), "child workspace must be absolute");
        let workspace = log
            .parent()
            .context("missing child workspace")?
            .canonicalize()?;
        let stdout = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
        let stderr = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
        ensure!(
            !stdout.is_null()
                && stdout as isize != -1
                && !stderr.is_null()
                && stderr as isize != -1,
            "child needs actual inherited stdout/stderr writers"
        );
        let identity = json!({"pid":std::process::id(),
            "creationTime":creation_time(unsafe { GetCurrentProcess() })?,
            "stdoutHandle":stdout as usize as u64,"stderrHandle":stderr as usize as u64,
            "image":executable,"imageSha256":sha(&std::fs::read(&executable)?),
            "workspace":workspace,"spawnCount":1});
        atomic_record(&workspace.join("started.json"), &identity)?;
        let ready = workspace.join("release-child");
        let started = Instant::now();
        while !ready.exists() && started.elapsed() < Duration::from_secs(12) {
            std::thread::sleep(Duration::from_millis(5));
        }
        ensure!(
            ready.exists(),
            "parent did not duplicate and release the child within 12s"
        );
        let out = vec![b'O'; PIPE_BYTES];
        let err = vec![b'E'; PIPE_BYTES];
        std::io::stdout().write_all(&out)?;
        std::io::stdout().flush()?;
        std::io::stderr().write_all(&err)?;
        std::io::stderr().flush()?;
        atomic_record(
            &workspace.join("exit.json"),
            &json!({
            "pid":std::process::id(),"creationTime":identity["creationTime"],
            "stdoutWritten":out.len(),"stderrWritten":err.len(),
            "stdoutWrittenSha256":sha(&out),"stderrWrittenSha256":sha(&err),
            "spawnCount":1,"exitCount":1,"normalReturn":true}),
        )?;
        Ok(())
    }

    struct FaultHold {
        release: PathBuf,
        writers: Vec<OwnedHandle>,
    }
    impl FaultHold {
        fn release(&mut self) {
            // Closing the real OS writer duplicates is the only fault release.
            // There is no production runtime switch and no actor abort.
            self.writers.clear();
        }
    }
    impl Drop for FaultHold {
        fn drop(&mut self) {
            self.release();
            // An early error also releases a child still in its bounded setup
            // handshake before the outer finally attempts Host settlement.
            let _ = std::fs::write(&self.release, b"release");
        }
    }

    async fn started(path: &Path) -> Result<Value> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
        loop {
            if path.exists() {
                return Ok(serde_json::from_slice(&std::fs::read(path)?)?);
            }
            ensure!(
                tokio::time::Instant::now() < deadline,
                "actual child did not publish its atomic OS identity"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn retained(workspace: &Arc<dyn ReadVideoWorkspace>, hash: &str) -> Result<()> {
        ensure!(
            workspace.path().is_dir(),
            "UNKNOWN removed the owned workspace"
        );
        ensure!(
            sha(&std::fs::read(workspace.path().join("sentinel.bin"))?) == hash,
            "UNKNOWN removed or changed an existing output"
        );
        Ok(())
    }

    async fn fault_case(
        host: &Arc<HostMediaPort>,
        workspace: &Arc<dyn ReadVideoWorkspace>,
        helper: &Path,
        trace: &mut Value,
    ) -> Result<()> {
        let sentinel = b"existing media output must survive UNKNOWN";
        std::fs::write(workspace.path().join("sentinel.bin"), sentinel)?;
        let sentinel_hash = sha(sentinel);
        trace["sentinelSha256"] = json!(sentinel_hash);
        let mut hold = FaultHold {
            release: workspace.path().join("release-child"),
            writers: Vec::new(),
        };
        let command = MediaCommand {
            executable: helper.to_owned(),
            args: vec![
                "--ignored".into(),
                "--exact".into(),
                HELPER.into(),
                "--nocapture".into(),
                "--test-threads=1".into(),
                "--logfile".into(),
                workspace
                    .path()
                    .join("child-control.log")
                    .to_string_lossy()
                    .into_owned(),
            ],
        };
        trace["argv"] = json!(command.args);
        let execution = host.execute(command, None);
        tokio::pin!(execution);
        let started_path = workspace.path().join("started.json");
        let identity = tokio::select! {
            result = &mut execution => bail!("media child settled before the duplicate handshake: {result:?}"),
            identity = started(&started_path) => identity?,
        };
        trace["childIdentity"] = identity.clone();
        let pid = u32::try_from(identity["pid"].as_u64().context("missing child PID")?)?;
        // SAFETY: OpenProcess produces a new handle. The child cannot finish its
        // handshake until this process identity and writer handles are checked.
        let process = owned(
            unsafe {
                OpenProcess(
                    PROCESS_DUP_HANDLE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    0,
                    pid,
                )
            },
            "OpenProcess child",
        )?;
        let creation = creation_time(process.as_raw_handle())?;
        ensure!(
            identity["creationTime"].as_u64() == Some(creation),
            "child PID/creation-time identity changed"
        );
        let image = process_image(process.as_raw_handle())?;
        ensure!(
            image == helper
                && identity["imageSha256"].as_str() == Some(sha(&std::fs::read(helper)?).as_str()),
            "OS process image is not the delivered private test copy"
        );
        ensure!(
            identity["workspace"] == json!(workspace.path()) && identity["spawnCount"] == 1,
            "child atomic identity has the wrong workspace or invocation count"
        );
        hold.writers.push(duplicate_writer(
            process.as_raw_handle(),
            identity["stdoutHandle"]
                .as_u64()
                .context("missing stdout writer")?,
        )?);
        hold.writers.push(duplicate_writer(
            process.as_raw_handle(),
            identity["stderrHandle"]
                .as_u64()
                .context("missing stderr writer")?,
        )?);
        trace["duplicatedRealWriters"] = json!(hold.writers.len());
        trace["osImage"] = json!(image);
        trace["osCreationTime"] = json!(creation);
        let elapsed = Instant::now();
        std::fs::write(&hold.release, b"release")?;
        let error = match execution.await {
            Ok(_) => bail!("UNKNOWN pipe fault unexpectedly succeeded"),
            Err(error) => error,
        };
        trace["executeElapsedMs"] = json!(elapsed.elapsed().as_millis() as u64);
        trace["executeError"] =
            json!({"kind":format!("{:?}",error.kind),"message":error.message.to_utf8()?});
        ensure!(
            error.is_unknown() && error.message.to_utf8()?.contains("pipe settlement unknown"),
            "actual retained pipe must produce typed UNKNOWN, got {error:?}"
        );
        ensure!(
            elapsed.elapsed() >= Duration::from_millis(2500),
            "pipe UNKNOWN was fabricated before its real 3s bound"
        );
        // SAFETY: process is the same stable, owned OS process handle verified
        // above; zero wait observes exit without blocking or opening a reused PID.
        let exited = unsafe { WaitForSingleObject(process.as_raw_handle(), 0) };
        trace["originalProcessSignaledAtUnknown"] = json!(exited == WAIT_OBJECT_0);
        ensure!(
            exited == WAIT_OBJECT_0,
            "UNKNOWN is not the planned settled-child/pending-pipe fault"
        );
        let exit: Value =
            serde_json::from_slice(&std::fs::read(workspace.path().join("exit.json"))?)?;
        ensure!(
            exit["pid"] == pid
                && exit["creationTime"] == creation
                && exit["stdoutWritten"] == PIPE_BYTES
                && exit["stderrWritten"] == PIPE_BYTES
                && exit["spawnCount"] == 1
                && exit["exitCount"] == 1
                && exit["normalReturn"] == true,
            "real child did not write both actual pipes once and return normally"
        );
        trace["actualChildExitReceipt"] = exit;
        retained(workspace, &sentinel_hash)?;
        let elapsed = Instant::now();
        let error = match workspace.cleanup().await {
            Ok(()) => bail!("pending real pipes allowed workspace cleanup"),
            Err(error) => error,
        };
        trace["workspaceCleanupElapsedMs"] = json!(elapsed.elapsed().as_millis() as u64);
        trace["workspaceCleanupError"] =
            json!({"kind":format!("{:?}",error.kind),"message":error.message.to_utf8()?});
        ensure!(
            error.is_unknown() && elapsed.elapsed() >= Duration::from_millis(4500),
            "cleanup did not wait for the real 5s UNKNOWN bound"
        );
        retained(workspace, &sentinel_hash)?;
        trace["sentinelRetainedAfterCleanupUnknown"] = json!(true);
        let elapsed = Instant::now();
        let error = match host.close().await {
            Ok(()) => bail!("pending real pipes allowed successful Host close"),
            Err(error) => error,
        };
        trace["hostCloseElapsedMs"] = json!(elapsed.elapsed().as_millis() as u64);
        trace["hostCloseError"] =
            json!({"kind":format!("{:?}",error.kind),"message":error.message.to_utf8()?});
        ensure!(
            error.is_unknown() && elapsed.elapsed() >= Duration::from_millis(9500),
            "close did not wait for the real 10s UNKNOWN bound"
        );
        retained(workspace, &sentinel_hash)?;
        trace["sentinelRetainedAfterCloseUnknown"] = json!(true);
        ensure!(
            host.cleanup_errors().is_empty(),
            "transient UNKNOWN poisoned durable cleanup errors"
        );
        hold.release();
        trace["realWriterDuplicatesReleased"] = json!(true);
        let elapsed = Instant::now();
        video(
            tokio::time::timeout(Duration::from_secs(10), host.close())
                .await
                .context("actual late EOF did not settle retained owner")?,
        )?;
        trace["lateCloseElapsedMs"] = json!(elapsed.elapsed().as_millis() as u64);
        ensure!(
            !workspace.path().exists(),
            "late settled close did not remove owned outputs"
        );
        ensure!(
            host.cleanup_errors().is_empty(),
            "late success retained a permanent transient UNKNOWN"
        );
        video(host.close().await)?;
        trace["lateCloseSucceeded"] = json!(true);
        trace["repeatCloseSucceeded"] = json!(true);
        trace["workspaceRemovedOnlyAfterRealEof"] = json!(true);
        trace["executeCalls"] = json!(1);
        trace["mediaArgvReplays"] = json!(0);
        Ok(())
    }

    pub(super) async fn run() -> Result<()> {
        let elapsed = Instant::now();
        let helper = PathBuf::from(std::env::var_os("ARA_TEST_MEDIA_UNKNOWN_HELPER")
            .context("Root must copy this native test executable to a private D/bin/ffmpeg.exe and pin its PATH for this group")?).canonicalize()?;
        ensure!(
            helper.file_stem().and_then(|value| value.to_str()) == Some("ffmpeg"),
            "wrong copied helper name"
        );
        let test_binary = std::env::current_exe()?.canonicalize()?;
        ensure!(
            sha(&std::fs::read(&helper)?) == sha(&std::fs::read(&test_binary)?),
            "helper must be an exact copy of this delivered test artifact"
        );
        let case = directory("native-media-unknown-os");
        let temp = case.join("owned-workspaces");
        let host = video(HostMediaPort::new(&case, &temp))?;
        let issued = video(host.find_binary(MediaBinary::Ffmpeg).await)?
            .context("private ffmpeg helper was not discovered")?;
        ensure!(
            issued == helper,
            "Root must prepend only this group's private helper directory to its process PATH"
        );
        let workspace = video(host.create_workspace("actual-pipe-fault-").await)?;
        let mut trace = json!({"evidence":"actual Windows OS process and pipe ownership at production HostPort",
            "normalCliUnknownExecuted":false,"unixExecuted":false,"helper":helper,
            "testBinary":test_binary,"testBinarySha256":sha(&std::fs::read(&test_binary)?),
            "case":case,"workspace":workspace.path(),"caseBoundSeconds":45,
            "caseActionBoundSeconds":32,"finallyCloseBoundSeconds":10});
        let outcome = tokio::time::timeout(
            Duration::from_secs(32),
            fault_case(&host, &workspace, &helper, &mut trace),
        )
        .await;
        // Dropping the case future releases every duplicated writer and child
        // handshake guard first. This final close retains the production actor;
        // it never aborts it or deletes a workspace ahead of actual settlement.
        let final_close = tokio::time::timeout(Duration::from_secs(10), host.close()).await;
        trace["totalElapsedMs"] = json!(elapsed.elapsed().as_millis() as u64);
        trace["finallyClose"] = json!(match &final_close {
            Ok(Ok(())) => "settled",
            Ok(Err(_)) => "error",
            Err(_) => "timeout",
        });
        trace["outcome"] = json!(match &outcome {
            Ok(Ok(())) => "pass",
            Ok(Err(_)) => "fail",
            Err(_) => "case-timeout",
        });
        if let Ok(Err(error)) = &outcome {
            trace["failure"] = json!(format!("{error:#}"));
        }
        record("native-media-unknown-os-pipe-group.json", trace);
        video(final_close.context("bounded finally close timed out")?)?;
        outcome.context("real media UNKNOWN case exceeded 32s action bound")??;
        ensure!(
            elapsed.elapsed() < Duration::from_secs(45),
            "real UNKNOWN case exceeded its total bound"
        );
        Ok(())
    }
}

#[cfg(windows)]
#[tokio::test]
async fn native_media_unknown_os_pipe_late_settlement_group() {
    media_unknown_os::run().await.unwrap();
}

#[cfg(windows)]
#[test]
#[ignore = "explicit real child protocol, started only by the media UNKNOWN parent group"]
fn native_media_pipe_fixture_child() {
    media_unknown_os::fixture().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_profiler_normal_cli_exact_publication_group() {
    use ara_rpc::{WireString, WireValue};
    fn field<'a>(value: &'a WireValue, key: &str) -> Option<&'a WireValue> {
        let WireValue::Object(items) = value else { return None };
        items.iter().find(|(name, _)| name.equals_ascii(key)).map(|(_, value)| value)
    }
    fn exact_text(value: &WireValue) -> WireString {
        let Some(WireValue::Array(content)) = field(value, "content") else { panic!("original content array") };
        let mut units = Vec::new();
        for block in content {
            if let Some(WireValue::String(text)) = field(block, "text") {
                if !units.is_empty() { units.push(10); }
                units.extend_from_slice(text.units());
            }
        }
        WireString::from_units(units)
    }
    fn frame_result(lines: &[String], call: &str) -> WireValue {
        let matched = lines.iter().filter_map(|line| WireValue::parse(line).ok()).filter_map(|frame| {
            let Some(WireValue::String(kind)) = field(&frame, "type") else { return None };
            let Some(WireValue::String(id)) = field(&frame, "toolCallId") else { return None };
            (kind.equals_ascii("tool_execution_end") && id.equals_ascii(call))
                .then(|| field(&frame, "result").unwrap().clone())
        }).collect::<Vec<_>>();
        assert_eq!(matched.len(), 1, "one actual terminal frame per original model tool call {call}");
        matched[0].clone()
    }
    fn journal_receipt(entries: &[WireValue], call: &str) -> WireValue {
        let matched = entries.iter().filter_map(|entry| field(entry, "message")).filter(|message| {
            matches!(field(message, "role"), Some(WireValue::String(role)) if role.equals_ascii("toolResult"))
                && matches!(field(message, "toolCallId"), Some(WireValue::String(id)) if id.equals_ascii(call))
        }).collect::<Vec<_>>();
        assert_eq!(matched.len(), 1, "one durable receipt per original model tool call {call}"); matched[0].clone()
    }
    let started = Instant::now();
    let project = directory("profiler-project");
    std::fs::create_dir(project.join("nested")).unwrap();
    let sample = [
        "Analysis of sampling demo (pid 42) every 1 millisecond",
        "Process: demo [42]", "Path: /tmp/demo", "Physical footprint: 10M", "",
        "Call graph:", "    100 Thread_1: main", "    + 100 start  (in dyld) + 8  [0x100]",
        "    +   100 work  (in demo) + 8  [0x200]", "",
        "Total number in stack (recursive counted multiple, when >=5):", "",
    ].join("\n");
    let mut sample_file = vec![0xef, 0xbb, 0xbf]; sample_file.extend_from_slice(sample.as_bytes());
    std::fs::write(project.join("nested/demo.sample.txt"), &sample_file).unwrap();
    // Explicit lone units and the astral pair crossing the159-unit tree cutoff
    // remain exact in display details; Agent content follows fixed OMP sanitizeText.
    let split_label = format!("{}😀z", "x".repeat(158));
    let cpu = format!(r#"{{"nodes":[{{"id":1,"callFrame":{{"functionName":"(root)"}},"hitCount":0,"children":[2,3]}},{{"id":2,"callFrame":{{"functionName":"\ud800worker","url":"file:///tmp/work.js","lineNumber":0}},"hitCount":3,"children":[]}},{{"id":3,"callFrame":{{"functionName":"{split_label}"}},"hitCount":2,"children":[]}}],"startTime":0,"endTime":5000}}"#);
    std::fs::write(project.join("unicode.cpuprofile"), &cpu).unwrap();
    std::fs::write(project.join("wearing.cpuprofile"), "not a profiler: ordinary notes\n").unwrap();
    let failure = r#"{"nodes":[{"id":1,"callFrame":{"functionName":"f"},"hitCount":1,"children":{}}],"startTime":0,"endTime":10}"#;
    std::fs::write(project.join("failure.cpuprofile"), failure).unwrap();
    let expected_sample = ara_tools::read_profile::render_sample_profile(&sample).unwrap();
    let expected_cpu = ara_tools::read_profile::render_cpu_profile(&cpu).unwrap().unwrap();
    // Independent fixture oracle for fixed OMP sanitize-text.ts:23-37. There
    // are no original replacement/control units here; omit each decode error
    // (three lone units), and round-trip every valid scalar, including the pair.
    assert_eq!(expected_cpu.units().len(), 722, "original exact display length");
    assert!(!expected_cpu.units().iter().any(|unit| matches!(*unit,
        0xfffd | 0x00..=0x08 | 0x0b..=0x1f | 0x7f..=0x9f)));
    let decoded_cpu = char::decode_utf16(expected_cpu.units().iter().copied()).collect::<Vec<_>>();
    assert_eq!(decoded_cpu.iter().filter(|scalar| scalar.is_err()).count(), 3);
    let expected_agent_cpu_string = decoded_cpu.into_iter().filter_map(Result::ok).collect::<String>();
    let expected_agent_cpu = WireString::from_units(expected_agent_cpu_string.encode_utf16().collect());
    assert_eq!(expected_agent_cpu.units().len(), 719, "only three lone units are removed");
    assert!(expected_agent_cpu.units().windows(2).any(|pair| pair == [0xd83d, 0xde00]),
        "valid emoji pair survives the independent sanitizer oracle");
    let mut sample_lines = expected_sample.units().split(|unit| *unit == 10).map(<[u16]>::to_vec).collect::<Vec<_>>();
    if sample_lines.last().is_some_and(Vec::is_empty) { sample_lines.pop(); }
    let tail_start = sample_lines.len().saturating_sub(4).saturating_sub(1);
    let mut tail_units = Vec::new();
    for (index, line) in sample_lines[tail_start..].iter().enumerate() {
        if index != 0 { tail_units.push(10); }
        tail_units.extend_from_slice(line);
    }
    let responses = vec![
        tool_response("profiler-sample", "demo.sample.txt:-4"), answer(),
        tool_response("profiler-cpu", "unicode.cpuprofile"), answer(),
        tool_response("profiler-raw", "nested/demo.sample.txt:raw:1-1;unicode.cpuprofile:raw:1-1"), answer(),
        tool_response("profiler-fallback-fault", "wearing.cpuprofile;failure.cpuprofile"), answer(),
    ];
    let up = upstream(responses).await;
    let mut child = RpcChild::start_with_browser(&up, &project, "profiler", false);
    for (run, call) in [
        ("profiler-run-1", "profiler-sample"), ("profiler-run-2", "profiler-cpu"),
        ("profiler-run-3", "profiler-raw"), ("profiler-run-4", "profiler-fallback-fault"),
    ] {
        let receipt = child.run(run, call); assert_ne!(receipt["isError"], true, "original worker run remains reviewable: {receipt}");
    }
    child.send(json!({"id":"profiler-state","type":"get_state"}));
    let state = child.until(|frame| frame["type"] == "response" && frame["id"] == "profiler-state");
    assert_eq!(state["success"], true);
    let journal_path = PathBuf::from(state["data"]["sessionFile"].as_str().unwrap());
    for (id, command) in [("profiler-messages-live", "get_messages"), ("profiler-page-live", "get_messages_page")] {
        child.send(json!({"id":id,"type":command,"limit":100}));
        let reply = child.until(|frame| frame["type"] == "response" && frame["id"] == id);
        assert_eq!(reply["success"], true, "live exact public cache response");
    }
    child.send(json!({"id":"profiler-reopen","type":"switch_session","sessionPath":journal_path}));
    let reopened = child.until(|frame| frame["type"] == "response" && frame["id"] == "profiler-reopen");
    assert_eq!(reopened["success"], true);
    assert_eq!(reopened["data"]["cancelled"], false, "same physical journal is loaded through normal RPC");
    for (id, command) in [("profiler-messages-reopen", "get_messages"), ("profiler-page-reopen", "get_messages_page")] {
        child.send(json!({"id":id,"type":command,"limit":100}));
        let reply = child.until(|frame| frame["type"] == "response" && frame["id"] == id);
        assert_eq!(reply["success"], true, "reopened exact public cache response");
    }
    let settlement = child.finish();
    assert!(started.elapsed() <= CASE_BOUND, "four profiler Runs exceeded the declared120s bound");
    assert_eq!(up.served(), 8, "exactly eight semantic localhost responses; no replay");
    let raw_frames = child.raw_frames.lock().unwrap().clone();
    let journal_raw = std::fs::read_to_string(&journal_path).unwrap();
    let requests = up.requests.lock().await.clone();
    let evidence = directory("profiler-evidence");
    // Persist original receipts before publication assertions can fail.
    std::fs::write(evidence.join("native-frames.ndjson"), raw_frames.join("\n") + "\n").unwrap();
    std::fs::write(evidence.join("native-journal.jsonl"), &journal_raw).unwrap();
    std::fs::write(evidence.join("sample-input.sample.txt"), &sample_file).unwrap();
    std::fs::write(evidence.join("cpu-input.cpuprofile"), &cpu).unwrap();
    std::fs::write(evidence.join("wearing.cpuprofile"), b"not a profiler: ordinary notes\n").unwrap();
    std::fs::write(evidence.join("failure.cpuprofile"), failure).unwrap();
    std::fs::write(evidence.join("model-requests.json"), serde_json::to_vec_pretty(&requests).unwrap()).unwrap();
    std::fs::write(evidence.join("settlement.json"), serde_json::to_vec_pretty(&settlement).unwrap()).unwrap();
    for id in ["profiler-messages-live", "profiler-page-live", "profiler-messages-reopen", "profiler-page-reopen"] {
        let replies = raw_frames.iter().filter_map(|line| WireValue::parse(line).ok()).filter(|frame| {
            matches!(field(frame, "type"), Some(WireValue::String(kind)) if kind.equals_ascii("response"))
                && matches!(field(frame, "id"), Some(WireValue::String(found)) if found.equals_ascii(id))
        }).collect::<Vec<_>>();
        assert_eq!(replies.len(), 1, "one actual cache response for {id}");
        assert_eq!(field(&replies[0], "success"), Some(&WireValue::Bool(true)));
        let data = field(&replies[0], "data").unwrap();
        let Some(WireValue::Array(messages)) = field(data, "messages") else { panic!("exact cached messages {id}") };
        let receipts = messages.iter().filter(|message| {
            matches!(field(message, "role"), Some(WireValue::String(role)) if role.equals_ascii("toolResult"))
                && matches!(field(message, "toolCallId"), Some(WireValue::String(call)) if call.equals_ascii("profiler-cpu"))
        }).collect::<Vec<_>>();
        assert_eq!(receipts.len(), 1, "one original CPU receipt in live/reopened cache {id}");
        let cached_text = exact_text(receipts[0]);
        assert_eq!(cached_text, expected_agent_cpu, "cache transports exact sanitized Agent content {id}");
        assert!(!cached_text.units().contains(&0xd800));
        assert!(cached_text.units().windows(2).any(|pair| pair == [0xd83d, 0xde00]));
        let display = field(field(receipts[0], "details").unwrap(), "displayContent").unwrap();
        assert_eq!(field(display, "text"), Some(&WireValue::String(expected_cpu.clone())));
    }
    let entries = journal_raw.lines().map(|line| WireValue::parse(line).expect("strict original journal wire")).collect::<Vec<_>>();
    let sample_result = frame_result(&raw_frames, "profiler-sample");
    let cpu_result = frame_result(&raw_frames, "profiler-cpu");
    let raw_result = frame_result(&raw_frames, "profiler-raw");
    let fault_result = frame_result(&raw_frames, "profiler-fallback-fault");
    for (call, result) in [("profiler-sample", &sample_result), ("profiler-cpu", &cpu_result),
        ("profiler-raw", &raw_result), ("profiler-fallback-fault", &fault_result)] {
        let durable = journal_receipt(&entries, call);
        assert_eq!(exact_text(&durable), exact_text(result), "durable wire text matches the actual terminal {call}");
    }
    let sample_details = field(&sample_result, "details").unwrap();
    let cpu_details = field(&cpu_result, "details").unwrap();
    assert!(field(sample_details, "suffixResolution").is_none(), "successful profile suffix recovery is omitted");
    for details in [sample_details, cpu_details] {
        assert!(field(details, "contentType").is_none());
        assert!(field(details, "resolvedPath").is_some());
        let meta = field(details, "meta").unwrap();
        let source = field(meta, "source").unwrap();
        assert!(matches!(field(source, "type"), Some(WireValue::String(kind)) if kind.equals_ascii("path")));
    }
    let sample_display = field(sample_details, "displayContent").unwrap();
    assert_eq!(field(sample_display, "text"), Some(&WireValue::String(WireString::from_units(tail_units))));
    let cpu_display = field(cpu_details, "displayContent").unwrap();
    assert_eq!(field(cpu_display, "text"), Some(&WireValue::String(expected_cpu.clone())));
    let model_cpu = exact_text(&cpu_result);
    assert_eq!(model_cpu, expected_agent_cpu, "terminal publishes exact fixed sanitized Agent content");
    assert!(!model_cpu.units().contains(&0xd800), "lone JSON units are removed from Agent content");
    assert!(model_cpu.units().windows(2).any(|pair| pair == [0xd83d, 0xde00]),
        "valid emoji pair remains in Agent content");
    let Some(WireValue::String(display_cpu)) = field(cpu_display, "text") else { panic!("exact display") };
    assert!(display_cpu.units().contains(&0xd800) && display_cpu.units().contains(&0xd83d));
    let raw_text = exact_text(&raw_result).to_utf8().unwrap();
    assert!(raw_text.contains("Analysis of sampling demo") && raw_text.contains("callFrame"));
    assert!(!raw_text.contains("Hot paths"), "compound raw requests retain original bytes and child continuations");
    let fallback_fault = exact_text(&fault_result).to_utf8().unwrap();
    assert!(fallback_fault.contains("not a profiler: ordinary notes"));
    assert!(fallback_fault.contains("children") && fallback_fault.contains("iterable"),
        "original optional-field TypeError crosses the native failure/compound continuation: {fallback_fault}");
    assert_eq!(requests.len(), 8);
    // FakeUpstream preserves raw body text when serde cannot encode lone units.
    let cpu_followup = match &requests[3]["body"] {
        Value::String(body) => body.clone(), value => serde_json::to_string(value).unwrap(),
    };
    assert!(cpu_followup.contains("worker"), "model receives the completed CPU result");
    let cpu_followup_wire = WireValue::parse(&cpu_followup).expect("strict original model request wire");
    let Some(WireValue::Array(messages)) = field(&cpu_followup_wire, "messages") else { panic!("model messages") };
    let tool_messages = messages.iter().filter(|message| {
        matches!(field(message, "role"), Some(WireValue::String(role)) if role.equals_ascii("tool"))
            && matches!(field(message, "tool_call_id"), Some(WireValue::String(id)) if id.equals_ascii("profiler-cpu"))
    }).collect::<Vec<_>>();
    assert_eq!(tool_messages.len(), 1, "one original CPU result in model follow-up");
    assert_eq!(field(tool_messages[0], "content"), Some(&WireValue::String(expected_agent_cpu.clone())),
        "model receives exact fixed sanitized Agent content");
    for (name, value) in [("sample-result.json", sample_result), ("cpu-result.json", cpu_result),
        ("raw-result.json", raw_result), ("fallback-fault-result.json", fault_result)] {
        std::fs::write(evidence.join(name), value.stringify()).unwrap();
    }
    record("native-profiler-host.json", json!({"state":"completed_pending_independent_EXEC", "project":project,
        "evidence":evidence,"normalExecutable":BIN,"fourRuns":4,"localhostResponses":up.served(),
        "elapsedSeconds":started.elapsed().as_secs_f64(),"browser":false,"cas":0,"sourceSha":"596f2da7101178214aa27a753529d15e6b7ad91d",
        "sampleInputSha256":sha(&sample_file),"cpuInputSha256":sha(cpu.as_bytes()),"physicalJournal":journal_path,
        "settlement":settlement}));
}

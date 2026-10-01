//! Whole-module fixed OMP ThinkingLoop scenarios. A scripted stream and one
//! controlled HTTP provider exercise delivery/cancellation, not a live model.
//! Source: packages/ai/test/thinking-loop.test.ts at
//! 596f2da7101178214aa27a753529d15e6b7ad91d (MIT; see THIRD_PARTY_NOTICES.md).

use ara_ai::providers::openai_completions;
use ara_ai::retry_classification::{classify_retry, flag};
use ara_ai::thinking_loop::{
    GEMINI_HEADER_RUNAWAY_THRESHOLD, GeminiHeaderRunDetector, LoopGuardOptions, LoopGuardPolicy,
    THINKING_LOOP_ERROR_MARKER, ThinkingLoopDetector, complete_with_thinking_loop_retry,
    complete_with_thinking_loop_retry_observed, is_reasoning_summary_header, with_thinking_loop_guard,
};
use ara_ai::{
    AssistantBlock, AssistantMessage, AssistantMessageEvent as Event, AssistantStream, CallOptions, Context, EventSink,
    Model, ModelProvider, ProviderError, StopReason,
};
use ara_testkit::chunks::{done, finish, text};
use ara_testkit::{FakeUpstream, Script};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const OBSERVED_KIRO_CYCLE: &str = "% shipped. 100% delivered. 100% verified. 100% validated. 100% approved. 100% accepted. 100% merged. 100% deployed. 100% live. 100% operational. 100% successful. 100% excellent. 100% perfect. 100% final. 100% absolute. 100% total. 100% whole. 100% full. 100% entire. 100% complete. 100% done. 100% finished. 100";

fn near_duplicate_loop(paragraphs: usize) -> String {
    let variants = [
        "I am now verifying the test module to guarantee there are no compile errors and the code is completely safe.",
        "I am now verifying the test module once more to ensure there are no compile errors and the code stays completely safe.",
        "I am now re-verifying the test module to confirm there are no compile errors and the code remains completely safe.",
    ];
    (0..paragraphs)
        .map(|i| format!("**Confirming Safety {i}**\n\n{}", variants[i % variants.len()]))
        .collect::<Vec<_>>()
        .join("\n\n\n")
}

fn distinct_reasoning() -> String {
    [
        "First I read the agent loop to understand how the stream wrapper commits the final assistant message.",
        "Next I traced the retry classifier to see which error shapes are treated as transient by the session.",
        "The Vertex transport needs ADC, so its credential path differs from the OpenRouter completions route entirely.",
        "I should add a regression covering the empty-content terminal so the auto-retry gate stays satisfied later.",
        "The tokenizer counts code points, which matters for the wide-emoji case in the truncation helper above here.",
        "Compaction journals are per-request, so they never persist into the on-disk session transcript at all ever.",
        "The mock provider emits one delta per block, so streaming nuances need a direct detector unit test instead.",
        "Finally I will run the focused package suite and the type checker before touching the changelog entries now.",
        "Telemetry spans wrap each provider call, so failing one must still close its span in the catch branch too.",
        "A device default of CPU keeps the tiny model worker from crashing Bun on teardown across every platform now.",
    ].join("\n\n\n")
}

fn progress_lexicon_loop() -> String {
    let paragraphs = [
        (
            "Commencing Forward Movement",
            "I'm now moving forward, proceeding with the task. I'm focusing my attention on moving ahead, and executing. I am not dwelling, but taking action and moving onward. My focus is on doing what needs to be done.",
        ),
        (
            "Accelerating Execution Rhythm",
            "I'm now in a state of rapid execution, focused entirely on maintaining this rapid momentum. I'm channeling the energy to keep going, and am focused on executing this task.",
        ),
        (
            "Maintaining Momentum",
            "I'm completely locked in, and focused on proceeding. I'm relentlessly pushing forward, dedicated to this task. I'm maintaining momentum, and ensuring continued rapid execution. I'm just doing it, pushing ahead and proceeding!",
        ),
        (
            "Executing Task Forward",
            "I'm completely dedicated to pushing forward with this task, focused only on proceeding. I'm relentless in my focus, and driven to just do it. I am focused on the task, and just proceeding with it!",
        ),
        (
            "Continuing Forward Progress",
            "I'm focused on moving this forward, and just doing it! I am relentless and driven, dedicated to doing what needs to be done. I'm proceeding, and just executing what is needed to advance. I'm moving forward now, focused on just doing it, and proceeding.",
        ),
        (
            "Sustaining Task Focus",
            "I'm laser-focused on moving ahead; dedicated and driven, I am just relentlessly pushing forward. I'm maintaining momentum, and ensuring continued rapid execution, and am proceeding.",
        ),
        (
            "Continuing Forward Task",
            "I'm focused on just doing it, pushing ahead relentlessly. Proceeding forward with singular focus, dedicated to execution. I'm maintaining momentum, committed to continued forward progress!",
        ),
        (
            "Persevering Forward Action",
            "I'm focused entirely on moving ahead now, and just doing the task. I'm relentless in my focus, just pushing forward and proceeding. I am dedicated to executing what needs to be done. I'm moving forward, focused only on proceeding. I'm not stopping!",
        ),
        (
            "Advancing Through Task",
            "I'm focused on the task at hand and dedicated to seeing it through. I'm proceeding relentlessly, and am completely dedicated to maintaining this forward momentum. I am now just doing it, pushing ahead with singular focus and just proceeding.",
        ),
        (
            "Executing Task Again",
            "I'm focused on moving this forward, I'm dedicated to the task. I am relentless in my focus, and driven to just do it. I am focused on the task, and am just proceeding with it! I am just doing it, pushing ahead, and proceeding!",
        ),
        (
            "Commencing Further Action",
            "I'm now fully engaged, and focused on proceeding. I'm relentless in my focus, and driven to just do it. I am focused on the task, and just proceeding with it! I am just doing it, pushing ahead, and proceeding!",
        ),
        (
            "Initiating Focused Execution",
            "I'm now fully immersed in the process, relentlessly focused on proceeding with the task. The plan is to continue this trajectory and to just execute! I am now just doing it, pushing ahead, and proceeding! This is not stopping!",
        ),
    ];
    paragraphs.iter().map(|(title, body)| format!("**{title}**\n\n{body}")).collect::<Vec<_>>().join("\n\n\n")
}

fn fixed_anchor_stall(paragraphs: usize) -> String {
    let lex = [
        "just",
        "doing",
        "it",
        "proceeding",
        "pushing",
        "ahead",
        "forward",
        "focused",
        "keeping",
        "momentum",
        "relentless",
        "dedicated",
        "driven",
        "moving",
        "executing",
        "onward",
        "staying",
        "locked",
        "committed",
        "grinding",
    ];
    (0..paragraphs)
        .map(|i| {
            let rotated = lex.iter().cycle().skip(i % lex.len()).take(lex.len()).copied().collect::<Vec<_>>().join(" ");
            format!("Working src/memory.rs {rotated} src/memory.rs and again.")
        })
        .collect::<Vec<_>>()
        .join("\n\n\n")
}

fn per_file_templates() -> String {
    [
        ("approval-mode.test.ts", "AgentSession"), ("gh.test.ts", "TempDir"),
        ("todo.test.ts", "renderResult"), ("hook-editor.test.ts", "requestRender"),
        ("mcp-client.test.ts", "McpSession"), ("lsp-pool.test.ts", "LspWorker"),
        ("dap-session.test.ts", "DapBridge"), ("stats-sync.test.ts", "SyncWorker"),
    ].iter().enumerate().map(|(i, (file, symbol))| format!("{}. Subagent Refactor{i}:\n  - target: packages/coding-agent/test/{file}\n  - assignment: Replace the ReturnType annotation in packages/coding-agent/test/{file} with the explicit type {symbol}. Verify where {symbol} is imported from, then run biome check write unsafe on the file.", i + 1)).collect::<Vec<_>>().join("\n\n")
}

fn distinct_planning_runaway(headers: usize) -> String {
    (0..headers).map(|i| format!("**Refining Stage {i}**\n\nI am now reworking module_{i} so that handler_{i} routes Stage{i}Result through render_{i}.")).collect::<Vec<_>>().join("\n\n")
}

fn feed(text: &str, step: usize, semantic: bool) -> Option<String> {
    let mut detector = ThinkingLoopDetector::new(semantic);
    // Rust strings cannot carry unpaired surrogates. Split on scalar boundaries;
    // all detector windows and period thresholds themselves use UTF-16 units.
    for chunk in text.chars().collect::<Vec<_>>().chunks(step) {
        if let Some(reason) = detector.push(&chunk.iter().collect::<String>()) {
            return Some(reason);
        }
    }
    detector.flush()
}

#[test]
fn fixed_detectors_cover_all_source_shapes_thresholds_and_header_runs() {
    let tight = (0..12)
        .map(|i| {
            format!(
                "Confirming the change is safe and the whole suite is green; pass number {i} of the final review sweep."
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n\n");
    for (fixture, marker) in [
        (tight, "near-identical segments"),
        (progress_lexicon_loop(), "low-information"),
        (fixed_anchor_stall(12), "low-information"),
        ("🌊 ".repeat(120), "back-to-back"),
        (format!("Healthy lead sentence. {}", OBSERVED_KIRO_CYCLE.repeat(6)), "311-character cycle"),
    ] {
        for step in [17, 23, 4096] {
            assert!(feed(&fixture, step, true).is_some_and(|reason| reason.contains(marker)), "{marker}, step={step}");
        }
    }
    let focused = [
        "Reading src/memory.rs I see the AsRawFd import was dropped during the last edit near the top of the file.",
        "The mmap call in src/memory.rs passes MAP_PRIVATE, so the page cache stays per process rather than shared.",
        "Down in src/memory.rs the unmap path forgets to check the length argument before calling munmap on it.",
        "A second look at src/memory.rs shows the guard page is allocated but never released on the error branch.",
        "The fault handler referenced from src/memory.rs assumes a tiny page, which breaks on the larger Apple platform.",
        "Tracing ownership through src/memory.rs reveals the Arc is cloned twice but only dropped once on shutdown.",
        "The harness around src/memory.rs maps a zero length region, which the freshly added assertion now rejects.",
        "Finally src/memory.rs needs the volatile read restored or the optimiser elides the probe under release builds.",
    ].join("\n\n\n");
    let anchor_free = [
        "First I think about how the retry path should behave when the model returns nothing useful at all.",
        "The summariser tends to repeat encouragement instead of describing any concrete next move it will take.",
        "A genuine plan keeps introducing fresh nouns because each paragraph advances toward a different sub goal.",
        "When the writer is stuck it recycles the same handful of motivational words over and over without progress.",
        "Distinguishing the two cases matters because discarding good reasoning would waste an entire sampled turn.",
        "The novelty measure should stay high whenever the author keeps naming new ideas rather than rephrasing one.",
        "Conversely a stall shows up as many paragraphs that together introduce almost no words the reader had seen.",
        "I want the guard to remain quiet during long but legitimate deliberation about a single hard design issue.",
        "Finally the threshold has to leave a wide margin so ordinary focused thinking never trips the detector ever.",
    ]
    .join("\n\n\n");
    for fixture in
        [distinct_reasoning(), per_file_templates(), focused, anchor_free, "00 ".repeat(200), "🌊 ".repeat(26)]
    {
        assert!(feed(&fixture, 23, true).is_none(), "healthy fixture: {fixture}");
    }
    assert!(feed(&near_duplicate_loop(12), 17, false).is_none(), "unrelated models retain exact-only detection");
    assert!(feed(&"🌊 ".repeat(59), 1, false).is_none());
    assert!(feed(&"🌊 ".repeat(60), 1, false).is_some(), "180 UTF-16 units, including pictographs");
    assert!(feed(&OBSERVED_KIRO_CYCLE.repeat(3), 23, false).is_none());
    assert!(feed(&OBSERVED_KIRO_CYCLE.repeat(4), 23, false).is_some());
    assert!(feed(&format!("a{}", "0".repeat(1023)).repeat(4), 4096, false).is_some());
    assert!(feed(&format!("a{}", "0".repeat(1024)).repeat(4), 4096, false).is_none(), "maximum period remains 1024");
    let mut trailing = ThinkingLoopDetector::default();
    let block = format!(
        "{}\n\n\nI am now verifying the test module to guarantee there are no compile errors and the code is completely safe.",
        near_duplicate_loop(7)
    );
    assert!(trailing.push(&block).is_none());
    assert!(trailing.flush().is_some_and(|reason| reason.contains("near-identical segments")));
    // The 700-character forced segmentation also applies to a wall of prose.
    let wall = (0..100).map(|i| format!("Confirming the change is safe and the whole suite is green; pass number {i} of the final review sweep. ")).collect::<String>();
    assert!(feed(&wall, 23, true).is_some());
    for heading in [
        "## Examining Result Handling",
        "### Refining Grammar Expansion",
        "**Defining ApplyResult Details**",
        "***Adapting Renderer***",
    ] {
        assert!(is_reasoning_summary_header(heading));
    }
    for prose in [
        "I'm now incorporating **targetPath** into the result.",
        "**bold start** but the rest is prose",
        "*single asterisk italic*",
        "#hashtag-not-a-heading",
        "plain reasoning line",
    ] {
        assert!(!is_reasoning_summary_header(prose));
    }
    assert!(feed(&distinct_planning_runaway(38), 17, true).is_none());
    let mut headers = GeminiHeaderRunDetector::default();
    assert!(!headers.push(&distinct_planning_runaway(10)));
    headers.reset();
    assert_eq!(headers.count(), 0);
    for i in 0..GEMINI_HEADER_RUNAWAY_THRESHOLD {
        assert_eq!(headers.push(&format!("**Summary {i}**\n")), i + 1 == GEMINI_HEADER_RUNAWAY_THRESHOLD);
        assert!(!headers.push("Some distinct reasoning paragraph here.\n\n"));
    }
    assert_eq!(headers.count(), GEMINI_HEADER_RUNAWAY_THRESHOLD);
    assert!(!headers.push("**Another Header**\n"));
    headers.reset();
    let runaway = distinct_planning_runaway(GEMINI_HEADER_RUNAWAY_THRESHOLD);
    let mut fired = false;
    for chunk in runaway.as_bytes().chunks(13) {
        fired |= headers.push(std::str::from_utf8(chunk).unwrap());
    }
    assert!(fired);
    headers.reset();
    assert!(!headers.push(&distinct_planning_runaway(GEMINI_HEADER_RUNAWAY_THRESHOLD - 1)));
    assert!(!GeminiHeaderRunDetector::default().push(&distinct_reasoning()));
}

fn model() -> Model {
    Model {
        id: "fixture".into(),
        api: "openai-completions".into(),
        provider: "fixture".into(),
        base_url: "https://unused.example".into(),
        reasoning: true,
        max_tokens: None,
        tokenizer: None,
    }
}

fn partial() -> AssistantMessage {
    AssistantMessage::empty("openai-completions", "fixture", "fixture")
}

fn thinking(delta: &str) -> Event {
    Event::ThinkingDelta { content_index: 0, delta: delta.into(), partial: partial() }
}

fn visible(delta: &str) -> Event {
    Event::TextDelta { content_index: 1, delta: delta.into(), partial: partial() }
}

fn completed() -> Event {
    let mut message = partial();
    message.content.push(AssistantBlock::text("Healthy final answer."));
    Event::Done { reason: StopReason::Stop, message }
}

fn scripted(events: Vec<Event>, cancel: CancellationToken) -> AssistantStream {
    let (sink, stream) = EventSink::channel();
    tokio::spawn(async move {
        for event in events {
            if !sink.push_or_cancel(event, &cancel).await {
                return;
            }
        }
    });
    stream
}

async fn collect(mut stream: AssistantStream) -> Vec<Event> {
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut events = Vec::new();
        while let Some(event) = stream.recv().await {
            events.push(event);
        }
        events
    })
    .await
    .expect("guarded stream must settle")
}

fn assert_loop_error(events: &[Event]) -> &AssistantMessage {
    assert_eq!(events.iter().filter(|event| event.is_terminal()).count(), 1);
    assert!(!events.iter().any(|event| matches!(event, Event::Done { .. })));
    let Event::Error { reason: StopReason::Error, error } = events.last().expect("terminal") else {
        panic!("expected loop error");
    };
    assert!(error.content.is_empty());
    assert!(error.provider_payload.is_none());
    assert!(error.error_message.as_deref().unwrap().contains(THINKING_LOOP_ERROR_MARKER));
    assert!(error.error_message.as_deref().unwrap().contains("stream stall"));
    assert_eq!(error.failure_evidence.as_ref().unwrap().error_id, flag::THINKING_LOOP | flag::CLASS);
    assert!(classify_retry(error, "openai-completions").retriable);
    error
}

#[derive(Default)]
struct ResultProvider {
    calls: AtomicUsize,
    contentful_error: bool,
    regular_error: bool,
    cancel_on_third: bool,
}

impl ModelProvider for ResultProvider {
    fn stream(&self, model: &Model, _context: &Context, options: CallOptions) -> AssistantStream {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        assert_ne!(options.loop_guard.as_ref().and_then(|guard| guard.enabled), Some(false));
        if self.cancel_on_third && call == 3 {
            options.cancel.cancel();
        }
        let events = if self.contentful_error || self.regular_error {
            let mut error = partial();
            error.stop_reason = StopReason::Error;
            error.error_message = Some("provider receipt".into());
            if self.contentful_error {
                error.content.push(AssistantBlock::text("Looping visible reasoning garbage."));
                error.failure_evidence = Some(ProviderError::Stream("provider receipt".into()).failure_evidence(false));
                error.failure_evidence.as_mut().unwrap().error_id = flag::THINKING_LOOP | flag::CLASS;
            }
            vec![Event::Error { reason: StopReason::Error, error }]
        } else {
            vec![thinking(&near_duplicate_loop(12)), completed()]
        };
        with_thinking_loop_guard(
            model,
            options,
            LoopGuardPolicy { semantic_heuristics: true, ..Default::default() },
            |_| {
                let (sink, stream) = EventSink::channel();
                // Like OMP's mock, publish the fixed terminal despite a caller
                // abort on attempt three, exercising the final abort priority.
                tokio::spawn(async move {
                    for event in events {
                        if !sink.push(event).await {
                            return;
                        }
                    }
                });
                stream
            },
        )
    }
}

#[tokio::test]
async fn stream_latches_and_overrides_preserve_source_event_boundaries() {
    let semantic = LoopGuardPolicy { semantic_heuristics: true, ..Default::default() };
    let tool = || Event::ToolcallStart { content_index: 1, partial: partial() };
    let end = || Event::ThinkingEnd { content_index: 0, content: String::new(), partial: partial() };
    let start = || Event::TextStart { content_index: 1, partial: partial() };
    let loop_text = near_duplicate_loop(12);
    for events in [
        vec![thinking("I'll search the cargo gate next.\n\n"), tool(), thinking(&loop_text), end(), completed()],
        vec![
            thinking("I'll read the thumper cargo function.\n\n"),
            end(),
            tool(),
            thinking(&loop_text),
            end(),
            completed(),
        ],
        vec![tool(), start(), visible(""), thinking(&loop_text), end(), completed()],
        vec![visible("First healthy text sentence. "), visible(&loop_text), completed()],
    ] {
        let caller = CancellationToken::new();
        let mut owned = None;
        let stream = with_thinking_loop_guard(
            &model(),
            CallOptions { cancel: caller.clone(), ..Default::default() },
            semantic,
            |options| {
                owned = Some(options.cancel.clone());
                scripted(events, options.cancel)
            },
        );
        let output = collect(stream).await;
        assert_loop_error(&output);
        assert!(!caller.is_cancelled());
        assert!(owned.unwrap().is_cancelled());
    }
    for (events, policy, overrides) in [
        (
            vec![
                thinking(&distinct_reasoning()),
                end(),
                visible("Here is the final answer."),
                thinking(&loop_text),
                end(),
                completed(),
            ],
            semantic,
            None,
        ),
        (
            vec![tool(), start(), visible("Here is the final answer."), thinking(&loop_text), end(), completed()],
            semantic,
            None,
        ),
        (vec![tool(), visible(&loop_text), completed()], semantic, None),
        (
            vec![
                Event::ToolcallDelta { content_index: 1, delta: "{".into(), partial: partial() },
                visible(&loop_text),
                completed(),
            ],
            semantic,
            None,
        ),
        (
            vec![visible(&loop_text), completed()],
            semantic,
            Some(LoopGuardOptions { check_assistant_content: Some(false), ..Default::default() }),
        ),
        (vec![thinking(&loop_text), completed()], LoopGuardPolicy::default(), None),
        (
            vec![thinking(&loop_text), completed()],
            semantic,
            Some(LoopGuardOptions { enabled: Some(false), ..Default::default() }),
        ),
        (vec![thinking(&loop_text), completed()], LoopGuardPolicy { enabled: false, ..semantic }, None),
    ] {
        let expected = events.clone();
        let stream = with_thinking_loop_guard(
            &model(),
            CallOptions { loop_guard: overrides, ..Default::default() },
            policy,
            |options| scripted(events, options.cancel),
        );
        assert_eq!(collect(stream).await, expected);
    }
    // Upstream enabled=true cannot add semantic checks to unrelated models.
    let stream = with_thinking_loop_guard(
        &model(),
        CallOptions {
            loop_guard: Some(LoopGuardOptions { enabled: Some(true), ..Default::default() }),
            ..Default::default()
        },
        LoopGuardPolicy::default(),
        |options| scripted(vec![thinking(&loop_text), completed()], options.cancel),
    );
    assert!(matches!(collect(stream).await.last(), Some(Event::Done { .. })));
    // A provider terminal error, including replay-unsafe content, passes through
    // unchanged; the guard does not implement result-path re-sampling itself.
    let mut provider_error = partial();
    provider_error.content.push(AssistantBlock::text("Existing failed provider receipt."));
    provider_error.stop_reason = StopReason::Error;
    let terminal = Event::Error { reason: StopReason::Error, error: provider_error };
    let stream = with_thinking_loop_guard(&model(), CallOptions::default(), semantic, |options| {
        scripted(vec![terminal.clone()], options.cancel)
    });
    assert_eq!(collect(stream).await, vec![terminal]);
}

#[tokio::test]
async fn guarded_provider_failure_ownership_usage_and_environment_are_complete() {
    if std::env::var("ARA_THINKING_LOOP_ENV_TEST").is_ok_and(|value| value == "1") {
        assert_eq!(std::env::var("ARA_NO_THINKING_LOOP_GUARD").unwrap(), "1");
        let events = vec![visible(&OBSERVED_KIRO_CYCLE.repeat(6)), completed()];
        let expected = events.clone();
        let stream =
            with_thinking_loop_guard(&model(), CallOptions::default(), LoopGuardPolicy::default(), |options| {
                scripted(events, options.cancel)
            });
        assert_eq!(collect(stream).await, expected);
        return;
    }
    let mut frames = vec![text("Healthy lead sentence. ")];
    frames.extend(
        OBSERVED_KIRO_CYCLE.repeat(6).as_bytes().chunks(23).map(|chunk| text(std::str::from_utf8(chunk).unwrap())),
    );
    frames.extend([finish("stop"), done()]);
    let fake =
        FakeUpstream::start(serde_json::from_value::<Script>(json!({"responses":[{"events":frames}]})).unwrap(), None)
            .await
            .unwrap();
    let model = Model { base_url: fake.base_url(), reasoning: false, ..model() };
    let caller = CancellationToken::new();
    let mut owned = None;
    let guarded = with_thinking_loop_guard(
        &model,
        CallOptions { cancel: caller.clone(), ..Default::default() },
        LoopGuardPolicy::default(),
        |options| {
            owned = Some(options.cancel.clone());
            openai_completions::stream(
                reqwest::Client::new(),
                model.clone(),
                Context::default(),
                openai_completions::StreamOptions {
                    cancel: options.cancel,
                    api_key: Some("synthetic-key".into()),
                    ..Default::default()
                },
            )
        },
    );
    let events = collect(guarded).await;
    assert!(events.iter().any(|event| matches!(event, Event::TextDelta { .. })));
    assert_loop_error(&events);
    assert!(!caller.is_cancelled());
    assert!(owned.unwrap().is_cancelled());
    assert_eq!(fake.served(), 1, "guard must not dispatch provider re-sampling");
    for usage in [None, Some(37)] {
        let mut observed = partial();
        observed.usage.input = usage;
        observed.provider_payload = Some(json!({"items":["discard this opaque failed attempt"]}));
        let event = Event::ThinkingDelta { content_index: 0, delta: "🌊 ".repeat(120), partial: observed };
        let guarded = with_thinking_loop_guard(&model, CallOptions::default(), LoopGuardPolicy::default(), |options| {
            scripted(vec![event, completed()], options.cancel)
        });
        let events = collect(guarded).await;
        let error = assert_loop_error(&events);
        assert_eq!(error.usage.input, usage);
        assert!(error.usage.output.is_none() && error.usage.cost.is_none());
    }
    // EOF without an upstream terminal cannot become success.
    let eof = with_thinking_loop_guard(&model, CallOptions::default(), LoopGuardPolicy::default(), |options| {
        scripted(vec![visible("partial")], options.cancel)
    });
    let events = collect(eof).await;
    let Event::Error { error, .. } = events.last().unwrap() else {
        panic!("EOF must fail");
    };
    assert!(error.failure_evidence.as_ref().unwrap().same_route_blocked);
    assert!(error.usage.is_unknown());
    // Caller cancellation reaches the provider child; a provider's Aborted
    // terminal survives unchanged and there is no new dispatch.
    let caller = CancellationToken::new();
    let mut owned = None;
    let stream = with_thinking_loop_guard(
        &model,
        CallOptions { cancel: caller.clone(), ..Default::default() },
        LoopGuardPolicy::default(),
        |options| {
            owned = Some(options.cancel.clone());
            let (sink, stream) = EventSink::channel();
            tokio::spawn(async move {
                options.cancel.cancelled().await;
                let mut error = partial();
                error.stop_reason = StopReason::Aborted;
                error.error_message = Some("User cancelled".into());
                sink.push(Event::Error { reason: StopReason::Aborted, error }).await;
            });
            stream
        },
    );
    caller.cancel();
    assert!(owned.unwrap().is_cancelled());
    let events = collect(stream).await;
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], Event::Error { reason: StopReason::Aborted, .. }));
    // Dropping an unread outer receiver must also release a parked inner stream.
    let caller = CancellationToken::new();
    let mut owned = None;
    let (cancelled, settled) = tokio::sync::oneshot::channel();
    let outer = with_thinking_loop_guard(
        &model,
        CallOptions { cancel: caller.clone(), ..Default::default() },
        LoopGuardPolicy::default(),
        |options| {
            owned = Some(options.cancel.clone());
            let (sink, inner) = EventSink::channel();
            tokio::spawn(async move {
                options.cancel.cancelled().await;
                drop(sink);
                let _ = cancelled.send(());
            });
            inner
        },
    );
    drop(outer);
    tokio::time::timeout(Duration::from_secs(2), settled).await.unwrap().unwrap();
    assert!(owned.unwrap().is_cancelled());
    assert!(!caller.is_cancelled());
    // Fixed stream.ts complete / completeSimple retries: three guarded attempts
    // and exact 500ms + 1000ms backoff, without an unguarded fourth attempt.
    let provider = ResultProvider::default();
    let mut attempts = Vec::new();
    let started = Instant::now();
    let result = complete_with_thinking_loop_retry_observed(
        &provider,
        &model,
        &Context::default(),
        CallOptions::default(),
        |message| attempts.push(message.clone()),
    )
    .await
    .unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
    assert_eq!(attempts.len(), 3);
    assert!(started.elapsed() >= Duration::from_millis(1500));
    assert!(attempts.iter().all(|message| message.content.is_empty() && message.stop_reason == StopReason::Error));
    assert_loop_error(&[Event::Error { reason: StopReason::Error, error: result }]);
    let caller = CancellationToken::new();
    let abort = caller.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        abort.cancel();
    });
    let provider = ResultProvider::default();
    let result = complete_with_thinking_loop_retry(
        &provider,
        &model,
        &Context::default(),
        CallOptions { cancel: caller, ..Default::default() },
    )
    .await;
    assert_eq!(result, Err(ProviderError::Aborted));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    let provider = ResultProvider { cancel_on_third: true, ..Default::default() };
    assert_eq!(
        complete_with_thinking_loop_retry(&provider, &model, &Context::default(), CallOptions::default()).await,
        Err(ProviderError::Aborted)
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
    for contentful in [true, false] {
        let provider =
            ResultProvider { contentful_error: contentful, regular_error: !contentful, ..Default::default() };
        let result = complete_with_thinking_loop_retry(&provider, &model, &Context::default(), CallOptions::default())
            .await
            .unwrap();
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(result.stop_reason, StopReason::Error);
        assert_eq!(result.content.is_empty(), !contentful);
    }
    // Test process-wide env disable in a bounded isolated process, avoiding
    // unsafe global environment mutation while the Rust tests run concurrently.
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap());
    child
        .args(["--exact", "guarded_provider_failure_ownership_usage_and_environment_are_complete", "--nocapture"])
        .env("ARA_THINKING_LOOP_ENV_TEST", "1")
        .env("ARA_NO_THINKING_LOOP_GUARD", "1")
        .kill_on_drop(true);
    let result = tokio::time::timeout(Duration::from_secs(5), child.output()).await.unwrap().unwrap();
    assert!(result.status.success(), "env-disable subprocess failed: {}", String::from_utf8_lossy(&result.stderr));
}

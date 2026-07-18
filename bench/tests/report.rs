// Kent Beck desiderata: readable, writable, and predictive receipts lead; fast, deterministic, isolated, behavior-sensitive, structure-insensitive, specific, and inspiring checks protect publication.
#![forbid(unsafe_code)]

use bench::{
    percentiles, render_json, render_markdown, BenchmarkConfig, BenchmarkReport, Machine,
    Percentiles, ScenarioResult, TuiScenarioResult,
};

#[test]
fn percentile_receipts_use_nearest_rank() {
    let samples = (1..=100).collect::<Vec<_>>();

    assert_eq!(
        percentiles(&samples),
        Ok(Percentiles {
            p50: 50,
            p95: 95,
            p99: 99,
        })
    );
    assert!(percentiles(&[]).is_err());
}

#[test]
fn every_report_row_carries_its_reproduction_command() {
    let result = ScenarioResult {
        model: "socket_supervisor".into(),
        sessions: 20,
        events: 120,
        processes_spawned: 21,
        per_event_forks: 0,
        peak_rss_bytes: 10 * 1024 * 1024,
        events_per_second: 20.0,
        latency_us: Percentiles {
            p50: 100,
            p95: 200,
            p99: 300,
        },
        cpu_seconds: 0.1,
        cpu_source: "sampled cumulative process-tree CPU via ps".into(),
        wall_seconds: 6.0,
        command: "scripts/with_scorer_lock.sh cargo run -p bench".into(),
        interpretation: "One socket, no event forks.".into(),
    };
    let report = BenchmarkReport {
        run_id: "fixture".into(),
        generated_unix_seconds: 1,
        machine: Machine {
            os: "test-os".into(),
            architecture: "test-arch".into(),
            rustc: "rustc-test".into(),
        },
        config: BenchmarkConfig {
            sessions: 20,
            events_per_session: 6,
            rate: 20,
            fork_hold_ms: 360,
            fork_cpu_ms: 30,
            fork_rss_mib: 18,
        },
        results: vec![result],
        tui_result: Some(TuiScenarioResult {
            model: "tui_real_shell".into(),
            sessions: 20,
            events: 120,
            processes_spawned: 21,
            per_event_forks: 0,
            tui_peak_rss_bytes: 8 * 1024 * 1024,
            child_agent_peak_rss_bytes: 20 * 1024 * 1024,
            total_peak_rss_bytes: 28 * 1024 * 1024,
            idle_window_seconds: 60.0,
            idle_cpu_seconds: 0.01,
            idle_cpu_percent: 0.016,
            idle_frames_rendered: 0,
            redraw_latency_us: Percentiles {
                p50: 150,
                p95: 500,
                p99: 900,
            },
            frames_rendered: 7,
            wall_seconds: 67.0,
            command: "scripts/with_scorer_lock.sh cargo run -p bench".into(),
            interpretation: "Event-driven tracer tabs.".into(),
        }),
    };

    let markdown = render_markdown(&report, "results/latest.json");
    let json = render_json(&report);

    assert!(markdown.contains("| `scripts/with_scorer_lock.sh cargo run -p bench` |"));
    assert!(markdown.contains("One socket, no event forks."));
    assert!(markdown.contains("Distinct processes measured"));
    assert!(markdown.contains("sampled cumulative process-tree CPU via ps"));
    assert!(markdown.contains("TUI-only RSS (MiB)"));
    assert!(markdown.contains("Event-driven tracer tabs."));
    assert!(json.contains("\"schema_version\": 3"));
    assert!(json.contains("\"per_event_forks\": 0"));
    assert!(json.contains("\"tui_peak_rss_bytes\": 8388608"));
    assert!(json.contains("\"idle_cpu_percent\": 0.016000"));
    assert!(json.contains("\"idle_frames_rendered\": 0"));
    assert!(json.contains("\"reproduce\": \"scripts/with_scorer_lock.sh cargo run -p bench\""));
}

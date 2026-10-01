// Copyright 2019-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use super::*;
use std::sync::mpsc;

const WAIT: Duration = Duration::from_secs(3);

fn snapshot(width: u32) -> Snapshot {
    HashMap::from([(
        "main".into(),
        WindowState {
            width,
            ..WindowState::default()
        },
    )])
}

fn width(snapshot: &Snapshot) -> u32 {
    snapshot["main"].width
}

#[test]
fn blocked_writer_is_ordered_and_pending_captures_coalesce() {
    let (started, writes) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let writer = Writer::start(move |snapshot| {
        let value = width(snapshot);
        started.send(value).unwrap();
        if value == 1 {
            blocked.recv_timeout(WAIT).unwrap();
        }
        Ok(())
    })
    .unwrap();
    writer.enqueue(snapshot(1)).unwrap();
    assert_eq!(writes.recv_timeout(WAIT).unwrap(), 1);
    // Submission can acquire the queue while the disk operation is blocked.
    writer.enqueue(snapshot(2)).unwrap();
    writer.enqueue(snapshot(3)).unwrap();
    assert!(writes.try_recv().is_err());
    release.send(()).unwrap();
    assert_eq!(writes.recv_timeout(WAIT).unwrap(), 3);
    assert!(writer.shutdown(WAIT));
    assert!(writes.try_recv().is_err());
}

#[test]
fn failed_write_retries_without_another_enqueue_and_preserves_valid_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.json");
    atomic_write(&path, &snapshot(10)).unwrap();
    let saved = std::fs::read(&path).unwrap();
    let destination = path.clone();
    let (started, attempts) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let mut attempt = 0;
    let writer = Writer::start(move |snapshot| {
        attempt += 1;
        started.send(attempt).unwrap();
        if attempt == 1 {
            return Err(io::Error::other("injected disk failure"));
        }
        blocked.recv_timeout(WAIT).unwrap();
        atomic_write(&destination, snapshot)
    })
    .unwrap();
    writer.enqueue(snapshot(20)).unwrap();
    assert_eq!(attempts.recv_timeout(WAIT).unwrap(), 1);
    assert_eq!(attempts.recv_timeout(WAIT).unwrap(), 2);
    assert_eq!(std::fs::read(&path).unwrap(), saved);
    release.send(()).unwrap();
    assert!(writer.shutdown(WAIT));
    let restored: Snapshot = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(width(&restored), 20);
}

#[test]
fn failed_older_snapshot_never_replaces_newer_pending_capture() {
    let (started, writes) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let writer = Writer::start(move |snapshot| {
        let value = width(snapshot);
        started.send(value).unwrap();
        if value == 1 {
            blocked.recv_timeout(WAIT).unwrap();
            return Err(io::Error::other("injected failure"));
        }
        Ok(())
    })
    .unwrap();
    writer.enqueue(snapshot(1)).unwrap();
    assert_eq!(writes.recv_timeout(WAIT).unwrap(), 1);
    writer.enqueue(snapshot(2)).unwrap();
    release.send(()).unwrap();
    assert_eq!(writes.recv_timeout(WAIT).unwrap(), 2);
    assert!(writer.shutdown(WAIT));
    assert!(writes.try_recv().is_err());
}

#[test]
fn shutdown_drains_latest_capture_and_rejects_later_saves() {
    let (started, writes) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let writer = Arc::new(
        Writer::start(move |snapshot| {
            let value = width(snapshot);
            started.send(value).unwrap();
            if value == 1 {
                blocked.recv_timeout(WAIT).unwrap();
            }
            Ok(())
        })
        .unwrap(),
    );
    writer.enqueue(snapshot(1)).unwrap();
    assert_eq!(writes.recv_timeout(WAIT).unwrap(), 1);
    writer.enqueue(snapshot(2)).unwrap();
    let drain = writer.clone();
    let finished = std::thread::spawn(move || drain.shutdown(WAIT));
    release.send(()).unwrap();
    assert!(finished.join().unwrap());
    assert_eq!(writes.recv_timeout(WAIT).unwrap(), 2);
    assert!(writer.enqueue(snapshot(3)).is_err());
}

#[test]
fn shutdown_attempts_final_capture_immediately_after_backed_off_failure() {
    let (started, writes) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let writer = Arc::new(
        Writer::start(move |snapshot| {
            let value = width(snapshot);
            started.send(value).unwrap();
            if value != 99 {
                blocked.recv_timeout(WAIT).unwrap();
                return Err(io::Error::other("injected disk failure"));
            }
            Ok(())
        })
        .unwrap(),
    );
    writer.enqueue(snapshot(1)).unwrap();
    for value in 1..=4 {
        assert_eq!(writes.recv_timeout(WAIT).unwrap(), value);
        writer.enqueue(snapshot(value + 1)).unwrap();
        release.send(()).unwrap();
    }
    assert_eq!(writes.recv_timeout(WAIT).unwrap(), 5);
    writer.enqueue(snapshot(99)).unwrap();
    let drain = writer.clone();
    let finished = std::thread::spawn(move || drain.shutdown(Duration::from_millis(750)));
    let queue = writer.shared.queue.lock().unwrap();
    let (queue, result) = writer
        .shared
        .changed
        .wait_timeout_while(queue, WAIT, |queue| queue.deadline.is_none())
        .unwrap();
    assert!(!result.timed_out());
    drop(queue);
    release.send(()).unwrap();
    assert_eq!(writes.recv_timeout(WAIT).unwrap(), 99);
    assert!(finished.join().unwrap());
}

#[test]
fn shutdown_timeout_does_not_wait_for_blocked_io_or_start_pending_io() {
    let (started, writes) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let writer = Writer::start(move |snapshot| {
        started.send(width(snapshot)).unwrap();
        blocked.recv_timeout(WAIT).unwrap();
        Ok(())
    })
    .unwrap();
    writer.enqueue(snapshot(1)).unwrap();
    assert_eq!(writes.recv_timeout(WAIT).unwrap(), 1);
    writer.enqueue(snapshot(2)).unwrap();
    let begin = Instant::now();
    assert!(!writer.shutdown(Duration::from_millis(30)));
    assert!(begin.elapsed() < Duration::from_secs(1));
    assert!(writer.enqueue(snapshot(3)).is_err());
    release.send(()).unwrap();
    assert!(!writer.shutdown(WAIT));
    assert!(writes.try_recv().is_err());
}

#[test]
fn shutdown_with_persistent_failure_is_bounded() {
    let writer = Writer::start(|_| Err(io::Error::other("disk unavailable"))).unwrap();
    writer.enqueue(snapshot(1)).unwrap();
    let begin = Instant::now();
    assert!(!writer.shutdown(Duration::from_millis(30)));
    assert!(begin.elapsed() < Duration::from_secs(1));
}

#[test]
fn upstream_json_roundtrip_keeps_all_fields_and_window_labels() {
    let json = r#"{
        "main": {"width":1200,"height":800,"x":-30,"y":40,"prev_x":-50,
          "prev_y":60,"maximized":true,"visible":false,"decorated":true,"fullscreen":false},
        "secondary": {"width":400,"height":300,"x":1,"y":2,"prev_x":3,
          "prev_y":4,"maximized":false,"visible":true,"decorated":false,"fullscreen":true}
    }"#;
    let original: Snapshot = serde_json::from_str(json).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nested/.window-state.json");
    atomic_write(&path, &original).unwrap();
    let restored: Snapshot = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(original, restored);
}

#[test]
fn atomic_write_overwrites_existing_file_without_trailing_bytes_or_temp_files() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.json");
    atomic_write(&path, &snapshot(12345)).unwrap();
    atomic_write(&path, &Snapshot::new()).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"{}");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn failed_atomic_replacement_keeps_destination_and_cleans_temp_file() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("state.json");
    std::fs::create_dir(&destination).unwrap();
    let existing = destination.join("previous.json");
    atomic_write(&existing, &snapshot(1)).unwrap();
    let bytes = std::fs::read(&existing).unwrap();
    assert!(atomic_write(&destination, &snapshot(2)).is_err());
    assert_eq!(std::fs::read(existing).unwrap(), bytes);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn persistent_failure_backoff_increases_and_is_capped() {
    let mut delay = RETRY_DELAY;
    for expected in [200, 400, 800, 1600, 3200, 5000, 5000] {
        delay = next_retry_delay(delay);
        assert_eq!(delay, Duration::from_millis(expected));
    }
    assert_eq!(next_retry_delay(Duration::MAX), MAX_RETRY_DELAY);
}

#[test]
fn default_builder_keeps_synchronous_mode() {
    assert!(crate::Builder::default().background_save.is_none());
    let timeout = Duration::from_millis(750);
    assert_eq!(
        crate::Builder::new()
            .with_background_save(timeout)
            .background_save,
        Some(timeout)
    );
}

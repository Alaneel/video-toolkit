//! Crash-conservative execution accounting, separate from task publication.
//!
//! The caller must hold the task's exclusive OS lock for this clock's lifetime.
//! A reservation is atomically synced before any bounded expensive step starts.
//! Only a normally returning owner may replace it with measured time. A later
//! owner charges the full abandoned bound once, without trusting wall clocks.
use crate::domain::*;
use crate::store::{self, event, invalid, now};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const FILE_NAME: &str = "execution-ledger.json";
const MIN_STEP_MS: u64 = 5;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reservation {
    reserved_ms: u64,
    started_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ledger {
    schema_version: u32,
    task_id: String,
    settled_ms: u64,
    conservative_ms: u64,
    reservation: Option<Reservation>,
}

fn read(root: &Path, task_id: &str, limit_ms: u64) -> Result<Option<Ledger>> {
    let path = root.join(FILE_NAME);
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
        Ok(_) => store::regular(&path)?,
    }
    if path.metadata()?.len() > 16 * 1024 {
        return Err(invalid("Execution accounting journal exceeds size limit"));
    }
    let ledger: Ledger = serde_json::from_reader(File::open(path)?)?;
    if ledger.schema_version != 1
        || ledger.task_id != task_id
        || ledger.conservative_ms > ledger.settled_ms
        || ledger.reservation.as_ref().is_some_and(|r| {
            r.reserved_ms < MIN_STEP_MS
                || r.reserved_ms > limit_ms
                || r.started_at.len() > 100
                || r.started_at.is_empty()
        })
    {
        return Err(invalid("Invalid task execution accounting journal"));
    }
    Ok(Some(ledger))
}

fn save(root: &Path, ledger: &Ledger) -> Result<()> {
    let path = root.join(FILE_NAME);
    if fs::symlink_metadata(&path).is_ok() {
        store::regular(&path)?;
    }
    let mut temporary = tempfile::NamedTempFile::new_in(root)?;
    serde_json::to_writer_pretty(&mut temporary, ledger)?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|e| e.error)?;
    #[cfg(unix)]
    File::open(root)?.sync_all()?;
    Ok(())
}

/// Read-only snapshots expose the current reservation without settling a live
/// worker. The journal wins over a task snapshot written before its checkpoint.
pub(crate) fn apply_snapshot(root: &Path, task: &mut Task) -> Result<()> {
    if let Some(ledger) = read(root, &task.id, task.limits.max_execution_secs * 1000)? {
        task.usage.elapsed_ms = task.usage.elapsed_ms.max(ledger.settled_ms);
        task.usage.conservative_elapsed_ms = ledger.conservative_ms;
        task.usage.execution_reserved_ms = ledger.reservation.map_or(0, |r| r.reserved_ms);
        task.usage.execution_accounting_version = 1;
    } else {
        if task.usage.execution_accounting_version != 0 {
            return Err(invalid("Required execution accounting journal is missing; refusing to reset this task's budget"));
        }
        task.usage.execution_reserved_ms = 0;
    }
    Ok(())
}

struct State {
    ledger: Ledger,
    started: Instant,
    finished: bool,
}

pub(crate) struct Clock {
    root: PathBuf,
    limit_ms: u64,
    state: RefCell<State>,
}

impl Clock {
    /// Existing pre-journal tasks migrate using their recorded counter. Their
    /// uncheckpointed historical work cannot retrospectively be reconstructed.
    pub(crate) fn new(root: &Path, task: &mut Task) -> Result<Self> {
        let limit_ms = task.limits.max_execution_secs * 1000;
        let existing = read(root, &task.id, limit_ms)?;
        if existing.is_none() && task.usage.execution_accounting_version != 0 {
            return Err(invalid("Required execution accounting journal is missing; refusing to reset this task's budget"));
        }
        let migrated = existing.is_none();
        let mut ledger = existing.unwrap_or_else(|| Ledger {
            schema_version: 1,
            task_id: task.id.clone(),
            settled_ms: task.usage.elapsed_ms,
            conservative_ms: task.usage.conservative_elapsed_ms,
            reservation: None,
        });
        ledger.settled_ms = ledger.settled_ms.max(task.usage.elapsed_ms);
        if let Some(reservation) = ledger.reservation.take() {
            ledger.settled_ms = ledger.settled_ms.saturating_add(reservation.reserved_ms);
            ledger.conservative_ms = ledger
                .conservative_ms
                .saturating_add(reservation.reserved_ms);
            event(task, "execution_reservation_recovered", format!(
                "Previous owner stopped before settling its bounded execution step; {} ms charged conservatively once. Actual interrupted time is unknown.",
                reservation.reserved_ms
            ));
        }
        if migrated {
            event(task, "execution_accounting_started", "Durable step reservations start now. Any unrecorded execution before this journal existed cannot be recovered retrospectively.");
        }
        save(root, &ledger)?;
        task.usage.elapsed_ms = ledger.settled_ms;
        task.usage.conservative_elapsed_ms = ledger.conservative_ms;
        task.usage.execution_reserved_ms = 0;
        task.usage.execution_accounting_version = 1;
        store::save(root, task)?;
        Self::from_ledger(root, limit_ms, ledger)
    }

    /// Initial source inspection also journals before hashing/probing. Until
    /// inspection finishes there is intentionally no valid/openable task.json.
    pub(crate) fn initial(root: &Path, task_id: String, limits: &Limits) -> Result<Self> {
        let ledger = Ledger {
            schema_version: 1,
            task_id,
            settled_ms: 0,
            conservative_ms: 0,
            reservation: None,
        };
        save(root, &ledger)?;
        Self::from_ledger(root, limits.max_execution_secs * 1000, ledger)
    }

    fn from_ledger(root: &Path, limit_ms: u64, ledger: Ledger) -> Result<Self> {
        if ledger.settled_ms.saturating_add(MIN_STEP_MS) > limit_ms {
            return Err(AgentError::TimedOut);
        }
        Ok(Self {
            root: root.into(),
            limit_ms,
            state: RefCell::new(State {
                ledger,
                started: Instant::now(),
                finished: false,
            }),
        })
    }

    fn settle(state: &mut State) {
        let elapsed = state.started.elapsed().as_nanos().div_ceil(1_000_000);
        state.ledger.settled_ms = state
            .ledger
            .settled_ms
            .saturating_add(elapsed.min(u128::from(u64::MAX)) as u64);
        state.ledger.reservation = None;
        state.started = Instant::now();
    }

    /// Each caller must pass the returned bound to the actual hash/media/model
    /// operation immediately; nested work shares that bound, not a fresh one.
    pub(crate) fn timeout(&self, cap_secs: u64) -> Result<Duration> {
        let mut state = self.state.borrow_mut();
        if state.finished {
            return Err(invalid("Execution clock has already been settled"));
        }
        Self::settle(&mut state);
        let reserved_ms = self
            .limit_ms
            .saturating_sub(state.ledger.settled_ms)
            .min(cap_secs.saturating_mul(1000));
        if reserved_ms < MIN_STEP_MS {
            save(&self.root, &state.ledger)?;
            return Err(AgentError::TimedOut);
        }
        state.ledger.reservation = Some(Reservation {
            reserved_ms,
            started_at: now(),
        });
        save(&self.root, &state.ledger)?;
        let timeout = Duration::from_millis(reserved_ms).saturating_sub(state.started.elapsed());
        if timeout < Duration::from_millis(MIN_STEP_MS) {
            return Err(AgentError::TimedOut);
        }
        Ok(timeout)
    }

    pub(crate) fn finish(&self, task: &mut Task) -> Result<()> {
        let mut state = self.state.borrow_mut();
        if !state.finished {
            Self::settle(&mut state);
            save(&self.root, &state.ledger)?;
            state.finished = true;
        }
        task.usage.elapsed_ms = state.ledger.settled_ms;
        task.usage.conservative_elapsed_ms = state.ledger.conservative_ms;
        task.usage.execution_reserved_ms = 0;
        task.usage.execution_accounting_version = 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{media, Workspace};

    fn fixture(root: &Path) -> Task {
        let input = root.join("inert-source.bin");
        fs::write(&input, b"inert original, not a semantic or media fixture").unwrap();
        let task = Task {
            schema_version: 1,
            id: "execution-fixture".into(),
            created_at: now(),
            intent: "Execution accounting test only".into(),
            target_duration_ms: 1000,
            assets: vec![Asset {
                id: "source-1".into(),
                path: input.clone(),
                sha256: media::hash_file(&input).unwrap(),
                size: input.metadata().unwrap().len(),
                modified_ns: 0,
                duration_ms: 2000,
                width: 16,
                height: 16,
                has_audio: false,
            }],
            samples: vec![],
            model_calls: vec![],
            plan: None,
            approval: None,
            status: TaskStatus::Created,
            last_error: None,
            limits: Limits::default(),
            usage: Usage::default(),
            cloud_consent: false,
            export: None,
            shards: vec![],
            history: vec![],
        };
        store::save(root, &task).unwrap();
        task
    }

    #[test]
    fn normal_steps_refund_unused_bounds_and_finishing_twice_does_not_charge_twice() {
        let root = tempfile::tempdir().unwrap();
        let mut task = fixture(root.path());
        let _lock = store::lock(root.path()).unwrap();
        let clock = Clock::new(root.path(), &mut task).unwrap();
        assert!(clock.timeout(2).unwrap() > Duration::from_millis(1500));
        let snapshot = store::read(root.path()).unwrap();
        assert_eq!(snapshot.usage.execution_reserved_ms, 2000);
        assert!(snapshot.usage.elapsed_ms < 500);
        std::thread::sleep(Duration::from_millis(15));
        clock.timeout(1).unwrap();
        clock.finish(&mut task).unwrap();
        let measured = task.usage.elapsed_ms;
        assert!((15..1000).contains(&measured));
        assert_eq!(task.usage.execution_reserved_ms, 0);
        assert_eq!(task.usage.conservative_elapsed_ms, 0);
        clock.finish(&mut task).unwrap();
        assert_eq!(task.usage.elapsed_ms, measured);
        assert!(clock.timeout(1).is_err());
        store::save(root.path(), &task).unwrap();
        assert_eq!(store::read(root.path()).unwrap().usage, task.usage);
    }

    #[test]
    fn abandoned_bound_is_charged_once_and_cannot_be_refunded_by_an_old_task_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let mut task = fixture(root.path());
        let original = task.clone();
        let _lock = store::lock(root.path()).unwrap();
        let clock = Clock::new(root.path(), &mut task).unwrap();
        clock.timeout(1).unwrap();
        drop(clock); // Real OS-kill coverage is a separate ignored media test below.
        let mut reopened = store::read(root.path()).unwrap();
        assert_eq!(reopened.usage.execution_reserved_ms, 1000);
        let resumed = Clock::new(root.path(), &mut reopened).unwrap();
        assert_eq!(reopened.usage.conservative_elapsed_ms, 1000);
        assert!(reopened.usage.elapsed_ms >= 1000);
        resumed.finish(&mut reopened).unwrap();
        store::save(root.path(), &original).unwrap();
        let mut old_snapshot = store::read(root.path()).unwrap();
        assert_eq!(old_snapshot.usage.conservative_elapsed_ms, 1000);
        assert!(old_snapshot.usage.elapsed_ms >= reopened.usage.elapsed_ms);
        let next = Clock::new(root.path(), &mut old_snapshot).unwrap();
        next.finish(&mut old_snapshot).unwrap();
        assert_eq!(old_snapshot.usage.conservative_elapsed_ms, 1000);
        assert_eq!(old_snapshot.usage.execution_reserved_ms, 0);
    }

    #[test]
    fn exhausted_budget_recovery_is_durable_even_when_no_new_step_can_start() {
        let root = tempfile::tempdir().unwrap();
        let mut task = fixture(root.path());
        task.limits.max_execution_secs = 1;
        task.limits.process_timeout_secs = 1;
        task.limits.model_timeout_secs = 1;
        let _lock = store::lock(root.path()).unwrap();
        let clock = Clock::new(root.path(), &mut task).unwrap();
        clock.timeout(1).unwrap();
        let reserved = store::read(root.path())
            .unwrap()
            .usage
            .execution_reserved_ms;
        drop(clock);
        let mut reopened = store::read(root.path()).unwrap();
        assert!(matches!(
            Clock::new(root.path(), &mut reopened),
            Err(AgentError::TimedOut)
        ));
        let after = store::read(root.path()).unwrap();
        assert_eq!(after.usage.conservative_elapsed_ms, reserved);
        assert_eq!(after.usage.execution_reserved_ms, 0);
        assert!(after.usage.elapsed_ms >= 1000);
        assert!(matches!(
            Clock::new(root.path(), &mut reopened),
            Err(AgentError::TimedOut)
        ));
        assert_eq!(
            store::read(root.path())
                .unwrap()
                .usage
                .conservative_elapsed_ms,
            reserved
        );
    }

    #[test]
    fn foreign_oversized_or_invalid_journals_fail_closed() {
        for case in ["foreign", "oversized", "invalid", "symlink", "missing"] {
            let root = tempfile::tempdir().unwrap();
            let mut task = fixture(root.path());
            let _lock = store::lock(root.path()).unwrap();
            let clock = Clock::new(root.path(), &mut task).unwrap();
            clock.finish(&mut task).unwrap();
            store::save(root.path(), &task).unwrap();
            let path = root.path().join(FILE_NAME);
            match case {
                "foreign" => {
                    let mut ledger = read(root.path(), &task.id, 600_000).unwrap().unwrap();
                    ledger.task_id = "other-task".into();
                    save(root.path(), &ledger).unwrap();
                }
                "oversized" => fs::write(path, vec![b' '; 16 * 1024 + 1]).unwrap(),
                "invalid" => fs::write(path, b"{\"schema_version\":1}").unwrap(),
                "missing" => fs::remove_file(path).unwrap(),
                "symlink" => {
                    #[cfg(unix)]
                    {
                        let foreign = root.path().join("foreign.json");
                        fs::rename(&path, &foreign).unwrap();
                        std::os::unix::fs::symlink(foreign, path).unwrap();
                    }
                    #[cfg(not(unix))]
                    continue;
                }
                _ => unreachable!(),
            }
            assert!(store::read(root.path()).is_err(), "{case}");
            assert!(Clock::new(root.path(), &mut task).is_err(), "{case}");
        }
    }

    #[test]
    fn exhausted_tasks_remain_viewable_without_fresh_verification_or_processing() {
        let root = tempfile::tempdir().unwrap();
        let mut task = fixture(root.path());
        task.usage.elapsed_ms = task.limits.max_execution_secs * 1000;
        // A deliberately false fingerprint proves the view-only branch did
        // not run hashing and then mislabel this source as stale.
        task.assets[0].sha256 = "0".repeat(64);
        store::save(root.path(), &task).unwrap();
        let workspace = Workspace::open(root.path()).unwrap();
        let viewed = workspace.load().unwrap();
        assert_eq!(viewed.status, TaskStatus::Failed);
        assert!(viewed.last_error.unwrap().contains("view-only"));
        assert!(matches!(
            workspace.sample(&CancelToken::default()),
            Err(AgentError::TimedOut)
        ));
        assert_eq!(
            Workspace::snapshot(root.path()).unwrap().status,
            TaskStatus::Created
        );
    }

    #[cfg(unix)]
    #[test]
    fn dangling_task_lock_symlink_cannot_create_an_external_lock_target() {
        let root = tempfile::tempdir().unwrap();
        let task_dir = root.path().join("task");
        fs::create_dir(&task_dir).unwrap();
        let escaped = root.path().join("outside-task.lock");
        std::os::unix::fs::symlink(&escaped, task_dir.join("task.lock")).unwrap();
        assert!(store::lock(&task_dir).is_err());
        assert!(!escaped.exists());
    }

    /// Executes this same test binary as two independent processes. The first
    /// really runs FFmpeg and is SIGKILLed with its media process group. The
    /// second opens the task through production load(), not a fabricated JSON
    /// recovery fixture. This proves crash accounting, not semantic selection.
    #[cfg(unix)]
    #[test]
    #[ignore = "requires real FFmpeg/ffprobe and process-group SIGKILL; no model is invoked"]
    fn hard_kill_real_media_worker_reopens_with_durable_charge() {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        const MODE: &str = "VIDEO_AGENT_BUDGET_TEST_MODE";
        const ROOT: &str = "VIDEO_AGENT_BUDGET_TEST_ROOT";
        if let Ok(mode) = std::env::var(MODE) {
            let dir = PathBuf::from(std::env::var_os(ROOT).unwrap());
            if mode == "worker" {
                let _lock = store::lock(&dir).unwrap();
                let mut task = store::read(&dir).unwrap();
                let clock = Clock::new(&dir, &mut task).unwrap();
                let timeout = clock.timeout(3).unwrap();
                let result = media::run(
                    Command::new("ffmpeg")
                        .args([
                            "-v",
                            "error",
                            "-nostdin",
                            "-n",
                            "-re",
                            "-stream_loop",
                            "-1",
                            "-i",
                        ])
                        .arg(&task.assets[0].path)
                        .args([
                            "-t",
                            "30",
                            "-c",
                            "copy",
                            "-flush_packets",
                            "1",
                            "-f",
                            "mpegts",
                        ])
                        .arg(dir.join("live-media-before-kill.ts")),
                    &CancelToken::default(),
                    timeout,
                );
                panic!("Worker should have been SIGKILLed during FFmpeg, got {result:?}");
            }
            assert_eq!(mode, "reopen");
            let workspace = Workspace::open(&dir).unwrap();
            let task = workspace.load().unwrap();
            fs::write(
                dir.join("independent-reopen.json"),
                serde_json::to_vec(&task).unwrap(),
            )
            .unwrap();
            return;
        }

        let root = tempfile::tempdir().unwrap();
        let input = root.path().join("public-synthetic-input.mp4");
        assert!(Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-nostdin",
                "-n",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:rate=30:duration=2",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-threads",
                "1"
            ])
            .arg(&input)
            .status()
            .unwrap()
            .success());
        let original_sha = media::hash_file(&input).unwrap();
        let limits = Limits {
            initial_frames: 1,
            max_frames: 2,
            max_execution_secs: 12,
            process_timeout_secs: 3,
            model_timeout_secs: 3,
            ..Limits::default()
        };
        let workspace = Workspace::create(
            root.path(),
            std::slice::from_ref(&input),
            "Crash accounting, not semantic selection".into(),
            1000,
            limits,
        )
        .unwrap();
        let before = Workspace::snapshot(workspace.task_dir()).unwrap();
        let test_name = "execution::tests::hard_kill_real_media_worker_reopens_with_durable_charge";
        let executable = std::env::current_exe().unwrap();
        let mut worker = Command::new(&executable)
            .args(["--exact", test_name, "--ignored", "--nocapture"])
            .env(MODE, "worker")
            .env(ROOT, workspace.task_dir())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let ready = workspace.task_dir().join("live-media-before-kill.ts");
        let deadline = Instant::now() + Duration::from_secs(2);
        while ready.metadata().map_or(true, |m| m.len() == 0) {
            assert!(
                Instant::now() < deadline,
                "Worker never started real media output"
            );
            assert!(
                worker.try_wait().unwrap().is_none(),
                "Worker exited before kill"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let live = Workspace::snapshot(workspace.task_dir()).unwrap();
        assert_eq!(live.usage.execution_reserved_ms, 3000);
        assert!(store::lock(workspace.task_dir()).is_err());
        let worker_pid = worker.id();
        assert!(worker_pid > 1);
        assert_ne!(worker_pid, std::process::id());
        let process_group_of = |pid: u32| {
            let output = Command::new("ps")
                .args(["-o", "pgid=", "-p", &pid.to_string()])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "Could not verify owned process group"
            );
            String::from_utf8(output.stdout)
                .unwrap()
                .trim()
                .parse::<u32>()
                .unwrap()
        };
        assert_eq!(process_group_of(worker_pid), worker_pid);
        assert_ne!(process_group_of(std::process::id()), worker_pid);
        assert!(Command::new("/bin/kill")
            .args(["-KILL", "--", &format!("-{worker_pid}")])
            .status()
            .unwrap()
            .success());
        let killed = worker.wait().unwrap();
        assert!(!killed.success());
        assert!(
            ready.metadata().unwrap().len() > 0,
            "A real FFmpeg process wrote media before SIGKILL"
        );
        assert!(Command::new(&executable)
            .args(["--exact", test_name, "--ignored", "--nocapture"])
            .env(MODE, "reopen")
            .env(ROOT, workspace.task_dir())
            .status()
            .unwrap()
            .success());
        let after: Task = serde_json::from_reader(
            File::open(workspace.task_dir().join("independent-reopen.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(after.usage.execution_reserved_ms, 0);
        assert_eq!(after.usage.conservative_elapsed_ms, 3000);
        assert!(after.usage.elapsed_ms >= before.usage.elapsed_ms + 3000);
        assert!(after
            .history
            .iter()
            .any(|e| e.kind == "execution_reservation_recovered"));
        let repeated = workspace.load().unwrap();
        assert_eq!(repeated.usage.conservative_elapsed_ms, 3000);
        assert_eq!(repeated.usage.model_calls, 0);
        assert_eq!(media::hash_file(&input).unwrap(), original_sha);
    }
}

use super::{
    HistoryDatabase,
    incident_query::{IncidentQuery, IncidentRequest, SavedIncident},
    query::{
        Query, QueryError, QueryRequest, QueryResult,
        summary::{PerformanceSummary, SummaryQuery, SummaryRequest},
    },
};
use crate::{incidents::IncidentTransition, monitor::MetricsSample};
use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager, Runtime, WebviewWindow, async_runtime};

const MAX_PENDING_EVENTS: usize = 32;
const RECORDING_INTERVAL_MS: u64 = 5_000;
const RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
const PRUNE_INTERVAL: Duration = Duration::from_secs(60 * 60);
const RECORDING_RETRY: Duration = Duration::from_secs(60);
const UNAVAILABLE: &str = "Local history storage is unavailable.";

type ClearResult = Result<(), String>;
type QueryReply = Result<QueryResult, QueryError>;
type SummaryReply = Result<PerformanceSummary, QueryError>;
type IncidentReply = Result<Vec<SavedIncident>, QueryError>;

#[derive(Default)]
struct Pending {
    enabled: bool,
    generation: u64,
    last_sample_ms: Option<u64>,
    sample: Option<(u64, MetricsSample)>,
    events: VecDeque<(u64, IncidentTransition)>,
    writing: bool,
    clear: Option<mpsc::Sender<ClearResult>>,
    clearing: bool,
    query: Option<Read>,
    querying: bool,
    stopped: bool,
}

impl Pending {
    fn accepts(&self, generation: u64) -> bool {
        self.enabled && !self.stopped && !self.clearing && self.generation == generation
    }
}

#[derive(Default)]
struct Shared {
    pending: Mutex<Pending>,
    wake: Condvar,
}

#[derive(Clone)]
pub(crate) struct HistoryRecorder(Arc<Shared>);

impl HistoryRecorder {
    pub(crate) fn generation(&self) -> Option<u64> {
        let pending = self.0.pending.lock().ok()?;
        (pending.enabled && !pending.stopped && !pending.clearing).then_some(pending.generation)
    }

    pub(crate) fn offer_events(
        &self,
        generation: u64,
        events: impl IntoIterator<Item = IncidentTransition>,
    ) {
        let Ok(mut pending) = self.0.pending.lock() else {
            return;
        };
        if !pending.accepts(generation) {
            return;
        }
        let available = MAX_PENDING_EVENTS - pending.events.len();
        pending.events.extend(
            events
                .into_iter()
                .take(available)
                .map(|event| (generation, event)),
        );
        self.0.wake.notify_one();
    }

    pub(crate) fn offer(
        &self,
        generation: u64,
        sample: &MetricsSample,
        collection_interval_ms: u64,
    ) {
        let Ok(mut pending) = self.0.pending.lock() else {
            return;
        };
        if !pending.accepts(generation)
            || pending.sample.is_some()
            || pending.last_sample_ms.is_some_and(|last| {
                sample.monotonic_ms.saturating_sub(last) < RECORDING_INTERVAL_MS
            })
        {
            return;
        }
        pending.last_sample_ms = Some(sample.monotonic_ms);
        let mut sample = sample.clone();
        sample.interval_ms = collection_interval_ms;
        pending.sample = Some((generation, sample));
        self.0.wake.notify_one();
    }
}

pub struct HistoryState {
    recorder: HistoryRecorder,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl HistoryState {
    pub fn start<R: Runtime>(app: &AppHandle<R>, enabled: bool) -> Result<Self, String> {
        let path = app
            .path()
            .app_data_dir()
            .map_err(|error| error.to_string())?
            .join("history.sqlite3");
        Self::at_path(path, enabled)
    }

    pub(crate) fn at_path(path: PathBuf, enabled: bool) -> Result<Self, String> {
        let shared = Arc::new(Shared {
            pending: Mutex::new(Pending {
                enabled,
                ..Pending::default()
            }),
            ..Shared::default()
        });
        let worker = thread::Builder::new()
            .name("history-storage".into())
            .spawn({
                let shared = Arc::clone(&shared);
                move || run(&shared, path)
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            recorder: HistoryRecorder(shared),
            worker: Mutex::new(Some(worker)),
        })
    }

    pub(crate) fn recorder(&self) -> HistoryRecorder {
        self.recorder.clone()
    }

    #[cfg(test)]
    pub(crate) fn wait_until_idle(&self) {
        let shared = &self.recorder.0;
        let pending = shared.pending.lock().unwrap();
        let (_guard, result) = shared
            .wake
            .wait_timeout_while(pending, Duration::from_secs(2), |pending| {
                pending.sample.is_some() || !pending.events.is_empty() || pending.writing
            })
            .unwrap();
        assert!(
            !result.timed_out(),
            "history worker did not drain recording"
        );
    }

    pub fn set_enabled(&self, enabled: bool) {
        let shared = &self.recorder.0;
        let Ok(mut pending) = shared.pending.lock() else {
            return;
        };
        if pending.enabled == enabled || pending.stopped {
            return;
        }
        pending.enabled = enabled;
        pending.generation += 1;
        pending.sample = None;
        pending.events.clear();
        if !enabled {
            drop(
                shared
                    .wake
                    .wait_while(pending, |pending| pending.writing && !pending.stopped),
            );
        }
    }

    pub(crate) fn clear(&self) -> ClearResult {
        self.queue_clear()?.recv().map_err(|_| UNAVAILABLE)?
    }

    fn queue_clear(&self) -> Result<mpsc::Receiver<ClearResult>, String> {
        let shared = &self.recorder.0;
        let (reply, response) = mpsc::channel();
        let mut pending = shared.pending.lock().map_err(|_| UNAVAILABLE)?;
        if pending.stopped {
            return Err(UNAVAILABLE.into());
        }
        if pending.clearing {
            return Err("History is already being cleared.".into());
        }
        pending.generation += 1;
        pending.sample = None;
        pending.events.clear();
        pending.clearing = true;
        pending.clear = Some(reply);
        shared.wake.notify_one();
        Ok(response)
    }

    fn queue_read<T>(
        &self,
        make_read: impl FnOnce(async_runtime::Sender<T>) -> Read,
    ) -> Result<async_runtime::Receiver<T>, QueryError> {
        let shared = &self.recorder.0;
        let mut pending = shared.pending.lock().map_err(|_| QueryError::Unavailable)?;
        if pending.stopped {
            return Err(QueryError::Unavailable);
        }
        if pending.querying || pending.query.is_some() {
            return Err(QueryError::Busy);
        }
        let (reply, response) = async_runtime::channel(1);
        pending.query = Some(make_read(reply));
        shared.wake.notify_one();
        Ok(response)
    }

    pub fn stop(&self) {
        let shared = &self.recorder.0;
        if let Ok(mut pending) = shared.pending.lock() {
            pending.stopped = true;
            pending.sample = None;
            pending.events.clear();
            pending.clear = None;
            pending.query = None;
            shared.wake.notify_all();
        }
        if let Ok(mut worker) = self.worker.lock()
            && let Some(worker) = worker.take()
        {
            let _ = worker.join();
        }
    }
}

impl Drop for HistoryState {
    fn drop(&mut self) {
        self.stop();
    }
}

#[tauri::command]
pub async fn clear_history<R: Runtime>(
    app: AppHandle<R>,
    window: WebviewWindow<R>,
) -> Result<(), String> {
    if window.label() != "settings" {
        return Err("History can only be cleared from Settings.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        app.try_state::<HistoryState>().ok_or(UNAVAILABLE)?.clear()
    })
    .await
    .map_err(|_| UNAVAILABLE)?
}

#[tauri::command]
pub async fn query_history<R: Runtime>(app: AppHandle<R>, request: QueryRequest) -> QueryReply {
    let query = request.validate()?;
    let mut response = app
        .try_state::<HistoryState>()
        .ok_or(QueryError::Unavailable)?
        .queue_read(|reply| Read::Metrics(query, reply))?;
    response.recv().await.ok_or(QueryError::Unavailable)?
}

#[tauri::command]
pub async fn query_incidents<R: Runtime>(
    app: AppHandle<R>,
    request: IncidentRequest,
) -> IncidentReply {
    let query = request.validate()?;
    let mut response = app
        .try_state::<HistoryState>()
        .ok_or(QueryError::Unavailable)?
        .queue_read(|reply| Read::Incidents(query, reply))?;
    response.recv().await.ok_or(QueryError::Unavailable)?
}

#[tauri::command]
pub async fn query_performance_summary<R: Runtime>(
    app: AppHandle<R>,
    request: SummaryRequest,
) -> SummaryReply {
    let query = request.validate()?;
    let mut response = app
        .try_state::<HistoryState>()
        .ok_or(QueryError::Unavailable)?
        .queue_read(|reply| Read::Summary(query, reply))?;
    response.recv().await.ok_or(QueryError::Unavailable)?
}

enum Read {
    Summary(SummaryQuery, async_runtime::Sender<SummaryReply>),
    Metrics(Query, async_runtime::Sender<QueryReply>),
    Incidents(IncidentQuery, async_runtime::Sender<IncidentReply>),
}

fn finish_read<T>(shared: &Shared, reply: async_runtime::Sender<T>, result: T) {
    if let Ok(mut pending) = shared.pending.lock() {
        pending.querying = false;
        let _ = reply.try_send(result);
    }
}

enum Work {
    Read(Read),
    Clear(mpsc::Sender<ClearResult>),
    Prune(Option<u64>),
    Write(u64, RecordingWrite),
}

enum RecordingWrite {
    Sample(MetricsSample),
    Event(IncidentTransition),
}

fn next_work(shared: &Shared, next_prune: Instant) -> Option<Work> {
    let mut pending = shared.pending.lock().ok()?;
    loop {
        if pending.stopped {
            return None;
        }
        if let Some(reply) = pending.clear.take() {
            return Some(Work::Clear(reply));
        }
        let wait = next_prune.saturating_duration_since(Instant::now());
        if wait.is_zero() {
            return Some(Work::Prune(pending.enabled.then_some(pending.generation)));
        }
        if let Some((generation, sample)) = pending.sample.take() {
            pending.writing = true;
            return Some(Work::Write(generation, RecordingWrite::Sample(sample)));
        }
        if let Some((generation, event)) = pending.events.pop_front() {
            pending.writing = true;
            return Some(Work::Write(generation, RecordingWrite::Event(event)));
        }
        if let Some(read) = pending.query.take() {
            pending.querying = true;
            return Some(Work::Read(read));
        }
        (pending, _) = shared.wake.wait_timeout(pending, wait).ok()?;
    }
}

fn run(shared: &Shared, path: PathBuf) {
    let mut storage = Storage {
        path,
        database: None,
        session: None,
        writer_lock: None,
    };
    let mut next_prune = Instant::now();
    let mut retry_at = next_prune;
    while let Some(work) = next_work(shared, next_prune) {
        match work {
            Work::Read(Read::Metrics(query, reply)) => {
                let result = if let Some(database) = &storage.database {
                    query.run(database)
                } else {
                    query.read_existing(&storage.path)
                };
                finish_read(shared, reply, result);
            }
            Work::Read(Read::Incidents(query, reply)) => {
                let result = match &storage.database {
                    Some(database) => query.run(database),
                    None => query.read_existing(&storage.path),
                };
                finish_read(shared, reply, result);
            }
            Work::Read(Read::Summary(query, reply)) => {
                let result = match &storage.database {
                    Some(database) => query.run(database),
                    None => query.read_existing(&storage.path),
                };
                finish_read(shared, reply, result);
            }
            Work::Clear(reply) => {
                let result = storage.clear();
                if result.is_ok() {
                    retry_at = Instant::now();
                }
                let Ok(mut pending) = shared.pending.lock() else {
                    break;
                };
                pending.clearing = false;
                let _ = reply.send(result);
            }
            Work::Prune(generation) => {
                if let Err(error) = storage.prune(generation, retention_cutoff()) {
                    eprintln!("Could not expire local history: {error}");
                }
                next_prune = Instant::now() + PRUNE_INTERVAL;
            }
            Work::Write(generation, write) => {
                if Instant::now() >= retry_at {
                    let result = match write {
                        RecordingWrite::Sample(sample) => storage.record(generation, &sample),
                        RecordingWrite::Event(event) => storage.record_event(generation, event),
                    };
                    if let Err(error) = result {
                        eprintln!("Could not record local history: {error}");
                        retry_at = Instant::now() + RECORDING_RETRY;
                    }
                }
                let Ok(mut pending) = shared.pending.lock() else {
                    break;
                };
                pending.writing = false;
                shared.wake.notify_all();
            }
        }
    }
}

struct Storage {
    path: PathBuf,
    database: Option<HistoryDatabase>,
    session: Option<(u64, i64)>,
    writer_lock: Option<fs::File>,
}

impl Storage {
    fn open_existing(&mut self) -> Result<(), String> {
        if self.database.is_none() && self.path.try_exists().map_err(|error| error.to_string())? {
            self.attach_database(HistoryDatabase::open_existing)?;
        }
        Ok(())
    }

    fn attach_database(
        &mut self,
        open: impl FnOnce(&Path) -> Result<HistoryDatabase, super::HistoryError>,
    ) -> Result<(), String> {
        let lock_path = self.path.with_extension("sqlite3.lock");
        create_private_file(&lock_path).map_err(|error| error.to_string())?;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| error.to_string())?;
        lock.try_lock()
            .map_err(|error| format!("Could not lock local history writer: {error}"))?;
        let mut database = open(&self.path).map_err(|error| error.to_string())?;
        database
            .interrupt_open_events()
            .map_err(|error| error.to_string())?;
        self.database = Some(database);
        self.writer_lock = Some(lock);
        Ok(())
    }

    fn open_for_write(&mut self) -> Result<&mut HistoryDatabase, String> {
        if self.database.is_none() {
            let parent = self.path.parent().ok_or(UNAVAILABLE)?;
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            create_private_file(&self.path).map_err(|error| error.to_string())?;
            self.attach_database(HistoryDatabase::open)?;
        }
        self.database.as_mut().ok_or_else(|| UNAVAILABLE.into())
    }

    fn record_event(&mut self, generation: u64, event: IncidentTransition) -> Result<(), String> {
        let id = self
            .session
            .filter(|(active, _)| *active == generation)
            .map(|(_, id)| id);
        if id.is_none() && matches!(event, IncidentTransition::Recovered { .. }) {
            return Ok(());
        }
        let id = self
            .open_for_write()?
            .record_event(id, event)
            .map_err(|error| error.to_string())?;
        self.session = id.map(|id| (generation, id));
        Ok(())
    }

    fn record(&mut self, generation: u64, sample: &MetricsSample) -> Result<(), String> {
        let id = self
            .session
            .filter(|(active, _)| *active == generation)
            .map(|(_, id)| id);
        self.session = Some((
            generation,
            self.open_for_write()?
                .record(id, sample)
                .map_err(|error| error.to_string())?,
        ));
        Ok(())
    }

    fn prune(&mut self, generation: Option<u64>, cutoff: i64) -> Result<(), String> {
        self.open_existing()?;
        if let Some(database) = self.database.as_mut() {
            let id = self
                .session
                .filter(|(active, _)| Some(*active) == generation)
                .map(|(_, id)| id);
            database
                .prune(cutoff, id)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn clear(&mut self) -> Result<(), String> {
        self.open_existing()?;
        if let Some(database) = self.database.as_mut() {
            database.clear().map_err(|error| error.to_string())?;
        }
        self.session = None;
        Ok(())
    }
}

fn create_private_file(path: &Path) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

fn retention_cutoff() -> i64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(elapsed.as_millis())
        .unwrap_or(i64::MAX)
        .saturating_sub(RETENTION_MS)
}

#[cfg(test)]
mod tests;

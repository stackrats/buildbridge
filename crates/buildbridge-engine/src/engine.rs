//! The engine: everything buildbridge does, behind one value any client can hold. The desktop
//! forwards its events to a window; a command line prints them; a daemon would relay them.
//! Nothing here knows which.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;

use crate::ops::AppState;

/// Where the engine's events go. Payloads are the same JSON the desktop's window receives.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: &str, payload: Value);
}

/// A sink that drops everything, for clients that read results and nothing else.
pub struct NoEvents;

impl EventSink for NoEvents {
    fn emit(&self, _event: &str, _payload: Value) {}
}

/// What an engine needs from its host: the two directories it owns (the desktop's are Tauri's
/// app config and local data directories, and a command line must use the same ones to see
/// the same machines) and somewhere to send events.
pub struct EngineDeps {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub events: Arc<dyn EventSink>,
}

type Listener = Arc<dyn Fn(&Value) + Send + Sync>;

/// An in-process subscription to one event name, for engine code that watches its own
/// progress — the remote runner forwards build events as log lines this way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListenerId(u64);

struct EngineInner {
    deps: EngineDeps,
    state: AppState,
    listeners: Mutex<Vec<(ListenerId, &'static str, Listener)>>,
    next_listener: AtomicU64,
}

/// A handle to the engine; cheap to clone, shared by every operation in a process.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<EngineInner>,
}

impl Engine {
    pub fn new(deps: EngineDeps) -> Self {
        Self {
            inner: Arc::new(EngineInner {
                deps,
                state: AppState::default(),
                listeners: Mutex::new(Vec::new()),
                next_listener: AtomicU64::new(1),
            }),
        }
    }

    pub fn config_dir(&self) -> PathBuf {
        self.inner.deps.config_dir.clone()
    }

    pub fn data_dir(&self) -> PathBuf {
        self.inner.deps.data_dir.clone()
    }

    pub(crate) fn state(&self) -> &AppState {
        &self.inner.state
    }

    /// Sends an event to every in-process listener and then to the host's sink.
    pub fn emit<T: Serialize>(&self, event: &str, payload: T) -> Result<(), String> {
        let value = serde_json::to_value(payload).map_err(|error| error.to_string())?;
        let listeners = self
            .inner
            .listeners
            .lock()
            .map(|listeners| {
                listeners
                    .iter()
                    .filter(|(_, name, _)| *name == event)
                    .map(|(_, _, listener)| Arc::clone(listener))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for listener in listeners {
            listener(&value);
        }
        self.inner.deps.events.emit(event, value);
        Ok(())
    }

    /// The machine list changed shape: a machine was created, deleted, started or stopped.
    pub fn notify_machines_changed(&self) {
        let _ = self.emit(
            crate::MACHINE_CHANGED_EVENT,
            crate::MachineChangedEvent { machine_id: None },
        );
    }

    pub fn listen(
        &self,
        event: &'static str,
        listener: impl Fn(&Value) + Send + Sync + 'static,
    ) -> ListenerId {
        let id = ListenerId(self.inner.next_listener.fetch_add(1, Ordering::AcqRel));
        if let Ok(mut listeners) = self.inner.listeners.lock() {
            listeners.push((id, event, Arc::new(listener)));
        }
        id
    }

    pub fn unlisten(&self, id: ListenerId) {
        if let Ok(mut listeners) = self.inner.listeners.lock() {
            listeners.retain(|(listed, _, _)| *listed != id);
        }
    }
}

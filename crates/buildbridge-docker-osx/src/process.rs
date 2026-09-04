//! Cancellation: the operation scope every spawned process registers with.

use super::*;

/// One long-running operation on one machine, as the thing a **Stop** button acts on.
///
/// Every child process the crate starts while the scope is entered — every `ssh`, `docker`
/// and `tar` — is registered here, so cancelling kills what is actually running rather than
/// leaving a build to finish in the background. The scope is thread-local because each
/// operation already runs on its own blocking thread; nothing in the crate's signatures has to
/// know about it.
#[derive(Debug, Default)]
pub struct OperationScope {
    pub(crate) cancelled: AtomicBool,
    pub(crate) children: Mutex<Vec<u32>>,
}

impl OperationScope {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Marks the operation cancelled and terminates every child it has running. Later spawns
    /// under this scope refuse to start.
    pub fn cancel(&self) -> usize {
        self.cancelled.store(true, Ordering::Release);
        let pids = self
            .children
            .lock()
            .map(|children| children.clone())
            .unwrap_or_default();
        for pid in &pids {
            terminate_process(*pid);
        }

        pids.len()
    }

    pub(crate) fn register(&self, pid: u32) {
        if let Ok(mut children) = self.children.lock() {
            children.push(pid);
        }
    }

    pub(crate) fn unregister(&self, pid: u32) {
        if let Ok(mut children) = self.children.lock() {
            children.retain(|child| *child != pid);
        }
    }
}

pub(crate) fn terminate_process(pid: u32) {
    #[cfg(windows)]
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    #[cfg(not(windows))]
    let _ = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

thread_local! {
    static CURRENT_SCOPE: std::cell::RefCell<Option<Arc<OperationScope>>> =
        const { std::cell::RefCell::new(None) };
}

/// Makes `scope` the current operation on this thread until the returned guard drops.
pub fn enter_operation(scope: Arc<OperationScope>) -> OperationGuard {
    CURRENT_SCOPE.with(|current| *current.borrow_mut() = Some(scope));
    OperationGuard(())
}

pub struct OperationGuard(());

impl Drop for OperationGuard {
    fn drop(&mut self) {
        CURRENT_SCOPE.with(|current| *current.borrow_mut() = None);
    }
}

pub(crate) fn current_scope() -> Option<Arc<OperationScope>> {
    CURRENT_SCOPE.with(|current| current.borrow().clone())
}

/// A child process that the current operation scope knows about. The child is optional only
/// so `wait_with_output`, which consumes it, can take it out before the drop unregisters it.
pub(crate) struct TrackedChild {
    pub(crate) child: Option<Child>,
    pub(crate) scope: Option<Arc<OperationScope>>,
}

impl TrackedChild {
    pub(crate) fn wait_with_output(mut self) -> std::io::Result<Output> {
        let child = self
            .child
            .take()
            .expect("a tracked child is present until consumed");
        let pid = child.id();
        let output = child.wait_with_output();
        if let Some(scope) = &self.scope {
            scope.unregister(pid);
        }
        output
    }
}

impl std::ops::Deref for TrackedChild {
    type Target = Child;

    fn deref(&self) -> &Child {
        self.child
            .as_ref()
            .expect("a tracked child is present until consumed")
    }
}

impl std::ops::DerefMut for TrackedChild {
    fn deref_mut(&mut self) -> &mut Child {
        self.child
            .as_mut()
            .expect("a tracked child is present until consumed")
    }
}

impl Drop for TrackedChild {
    fn drop(&mut self) {
        if let (Some(scope), Some(child)) = (&self.scope, &self.child) {
            scope.unregister(child.id());
        }
    }
}

pub(crate) trait TrackedCommand {
    fn tracked_spawn(&mut self) -> std::io::Result<TrackedChild>;
    fn tracked_output(&mut self) -> std::io::Result<Output>;
}

impl TrackedCommand for Command {
    fn tracked_spawn(&mut self) -> std::io::Result<TrackedChild> {
        let scope = current_scope();
        if scope.as_ref().is_some_and(|scope| scope.is_cancelled()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "the operation was stopped",
            ));
        }
        let child = self.spawn()?;
        if let Some(scope) = &scope {
            scope.register(child.id());
        }

        Ok(TrackedChild {
            child: Some(child),
            scope,
        })
    }

    fn tracked_output(&mut self) -> std::io::Result<Output> {
        self.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .tracked_spawn()?
            .wait_with_output()
    }
}

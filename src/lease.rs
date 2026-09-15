use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::{Mutex, OwnedMutexGuard, OwnedSemaphorePermit, Semaphore};

const DEFAULT_LEASE_SECONDS: u64 = 300;
const MAX_LEASE_SECONDS: u64 = 600;
const RUN_QUEUE_DEPTH: usize = 4;
/// How long a call waits for the one before it to let go of the machine.
/// Past this it reports the wait instead of joining a queue nobody can see.
const ACTION_WAIT: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub struct MachineAccess {
    action: Arc<Mutex<()>>,
    lease: Arc<Mutex<Option<ControlLease>>>,
    queue: Arc<Semaphore>,
    queue_holder: Arc<Mutex<Option<String>>>,
    /// The display whose bar shows who holds the machine; none in tests.
    display: Option<Arc<str>>,
    /// What the bar was last told, so a test can see a lapse was announced.
    told: Arc<std::sync::Mutex<Option<Option<String>>>>,
}

struct ControlLease {
    holder: String,
    expires: Instant,
}

pub struct RunPermit {
    _queue: OwnedSemaphorePermit,
    _action: OwnedMutexGuard<()>,
    queue_holder: Arc<Mutex<Option<String>>>,
    holder: String,
}

impl Drop for RunPermit {
    fn drop(&mut self) {
        let queue_holder = Arc::clone(&self.queue_holder);
        let holder = self.holder.clone();
        tokio::spawn(async move {
            let mut current = queue_holder.lock().await;
            if current.as_deref() == Some(&holder) {
                *current = None;
            }
        });
    }
}

impl MachineAccess {
    pub fn new() -> Self {
        Self {
            action: Arc::new(Mutex::new(())),
            lease: Arc::new(Mutex::new(None)),
            queue: Arc::new(Semaphore::new(RUN_QUEUE_DEPTH)),
            queue_holder: Arc::new(Mutex::new(None)),
            display: None,
            told: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// The bar on `display` learns who holds the machine whenever it changes.
    pub fn on_display(mut self, display: &str) -> Self {
        self.display = Some(display.into());
        self
    }

    /// Who holds the machine and for how many more seconds, if anyone does.
    pub async fn holder(&self) -> Option<(String, u64)> {
        let lease = self.lease.lock().await;
        lease
            .as_ref()
            .filter(|lease| lease.expires > Instant::now())
            .map(|lease| {
                (
                    lease.holder.clone(),
                    lease
                        .expires
                        .saturating_duration_since(Instant::now())
                        .as_secs(),
                )
            })
    }

    /// Tell the desktop's bar, off the runtime's threads, and never wait on it.
    fn publish(&self, lease: Option<&ControlLease>) {
        if let Ok(mut told) = self.told.lock() {
            *told = Some(lease.map(|lease| lease.holder.clone()));
        }
        let Some(display) = self.display.clone() else {
            return;
        };
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let holder = lease.map(|lease| {
            let remaining = lease.expires.saturating_duration_since(Instant::now());
            let expires = SystemTime::now() + remaining;
            let expires_ms = expires
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64);
            (lease.holder.clone(), expires_ms)
        });
        handle.spawn_blocking(move || {
            let _ = crate::x11::publish_holder(
                &display,
                holder
                    .as_ref()
                    .map(|(holder, expires)| (holder.as_str(), *expires)),
            );
        });
    }

    pub async fn mutate(&self, holder: &str) -> Result<OwnedMutexGuard<()>, String> {
        self.check_lease(holder).await?;
        let guard = self.action_within(ACTION_WAIT).await?;
        self.check_lease(holder).await?;
        Ok(guard)
    }

    async fn action_within(&self, wait: Duration) -> Result<OwnedMutexGuard<()>, String> {
        tokio::time::timeout(wait, Arc::clone(&self.action).lock_owned())
            .await
            .map_err(|_| "an earlier call still holds the machine; try again shortly".to_owned())
    }

    pub async fn run(&self, holder: &str) -> Result<RunPermit, String> {
        let queue = match Arc::clone(&self.queue).try_acquire_owned() {
            Ok(queue) => queue,
            Err(_) => {
                let active = self.queue_holder.lock().await;
                return Err(active.as_ref().map_or_else(
                    || "run queue full".to_owned(),
                    |active| format!("run queue full — {active} holds the machine"),
                ));
            }
        };
        self.check_lease(holder).await?;
        *self.queue_holder.lock().await = Some(holder.to_owned());
        let action = self.action_within(ACTION_WAIT).await?;
        self.check_lease(holder).await?;
        Ok(RunPermit {
            _queue: queue,
            _action: action,
            queue_holder: Arc::clone(&self.queue_holder),
            holder: holder.to_owned(),
        })
    }

    async fn check_lease(&self, holder: &str) -> Result<(), String> {
        let mut lease = self.lease.lock().await;
        if lease
            .as_ref()
            .is_some_and(|lease| lease.expires <= Instant::now())
        {
            *lease = None;
            self.publish(None);
        }
        if let Some(lease) = lease.as_ref().filter(|lease| lease.holder != holder) {
            let remaining = lease.expires.saturating_duration_since(Instant::now());
            return Err(format!(
                "machine under {}'s control — lease expires in {}m {}s",
                lease.holder,
                remaining.as_secs() / 60,
                remaining.as_secs() % 60
            ));
        }
        Ok(())
    }

    pub async fn control(
        &self,
        holder: &str,
        seconds: Option<u64>,
    ) -> Result<(u64, SystemTime), String> {
        let seconds = seconds.unwrap_or(DEFAULT_LEASE_SECONDS);
        if seconds > MAX_LEASE_SECONDS {
            return Err(format!("max lease duration is {MAX_LEASE_SECONDS} seconds"));
        }
        let mut active = self.lease.lock().await;
        if active
            .as_ref()
            .is_some_and(|lease| lease.expires > Instant::now())
        {
            let lease = active.as_ref().expect("checked above");
            let remaining = lease.expires.saturating_duration_since(Instant::now());
            return Err(format!(
                "control lease already held by {}, expires in {}s",
                lease.holder,
                remaining.as_secs()
            ));
        }
        *active = Some(ControlLease {
            holder: holder.to_owned(),
            expires: Instant::now() + Duration::from_secs(seconds),
        });
        self.publish(active.as_ref());
        Ok((seconds, SystemTime::now() + Duration::from_secs(seconds)))
    }

    /// A driving socket may renew its own lease, but cannot steal a newer one.
    pub async fn renew(&self, holder: &str, seconds: u64) -> bool {
        let mut active = self.lease.lock().await;
        if active
            .as_ref()
            .is_some_and(|lease| lease.expires > Instant::now() && lease.holder != holder)
        {
            return false;
        }
        let changed = active.as_ref().is_none_or(|lease| lease.holder != holder);
        *active = Some(ControlLease {
            holder: holder.to_owned(),
            expires: Instant::now() + Duration::from_secs(seconds),
        });
        if changed {
            self.publish(active.as_ref());
        }
        true
    }

    /// The person at the screen outranks every lease: their hands on the
    /// desktop hold it for `seconds` from now, whoever held it before.
    pub async fn seize(&self, holder: &str, seconds: u64) {
        let mut active = self.lease.lock().await;
        *active = Some(ControlLease {
            holder: holder.to_owned(),
            expires: Instant::now() + Duration::from_secs(seconds),
        });
        self.publish(active.as_ref());
    }

    pub async fn release(&self, holder: &str) -> Result<bool, String> {
        let mut active = self.lease.lock().await;
        let Some(lease) = active.as_ref() else {
            return Ok(false);
        };
        if lease.expires <= Instant::now() {
            // A lapsed hold is still what the bar last heard, so its
            // ending is announced like any other.
            *active = None;
            self.publish(None);
            return Ok(false);
        }
        if lease.holder != holder {
            return Err(format!(
                "control lease is held by {} — only the holder can release it",
                lease.holder
            ));
        }
        *active = None;
        self.publish(None);
        Ok(true)
    }
}

impl Default for MachineAccess {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_another_holder_is_refused() {
        let access = MachineAccess::new();
        access.control("alice", Some(60)).await.unwrap();
        assert!(access.mutate("alice").await.is_ok());
        assert!(
            access
                .mutate("mallory")
                .await
                .unwrap_err()
                .contains("alice")
        );
        assert!(
            access
                .release("mallory")
                .await
                .unwrap_err()
                .contains("alice")
        );
        assert!(access.release("alice").await.unwrap());
        assert!(access.holder().await.is_none());
        access.seize("person-1", 10).await;
        assert_eq!(access.holder().await.unwrap().0, "person-1");
    }

    #[tokio::test]
    async fn a_hold_that_lapsed_is_taken_off_the_bar_when_it_ends() {
        let access = MachineAccess::new();
        access.seize("person-1", 0).await;
        assert_eq!(
            access.told.lock().unwrap().clone(),
            Some(Some("person-1".to_owned()))
        );
        // The person sat still past the hold, then handed the screen back.
        assert!(!access.release("person-1").await.unwrap());
        assert_eq!(access.told.lock().unwrap().clone(), Some(None));
        assert!(access.holder().await.is_none());
        // The agent's first tool call after a lapse announces it too.
        access.seize("person-1", 0).await;
        assert!(access.mutate("teammate").await.is_ok());
        assert_eq!(access.told.lock().unwrap().clone(), Some(None));
    }

    #[tokio::test]
    async fn a_call_stuck_behind_another_reports_the_wait() {
        let access = MachineAccess::new();
        let _held = access.mutate("alice").await.unwrap();
        let refused = access
            .action_within(Duration::from_millis(50))
            .await
            .map(|_| ())
            .unwrap_err();
        assert!(refused.contains("earlier call"), "{refused}");
    }
}

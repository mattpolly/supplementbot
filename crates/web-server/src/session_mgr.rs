use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use chrono::{Datelike, Local};
use tokio::sync::RwLock;
use uuid::Uuid;

use intake_agent::session::IntakeSession;

// ---------------------------------------------------------------------------
// Session manager — handles creation, lookup, limits, and cleanup.
//
// Rate limits ($100/month budget protection):
//   - max_concurrent: how many sessions can be active at once
//   - daily_cap: max new sessions per day (resets at wall-clock midnight)
//   - monthly_cap: max new sessions per month (resets on the 1st)
//   - session_timeout: auto-archive after inactivity
// ---------------------------------------------------------------------------

struct ManagedSession {
    session: IntakeSession,
    last_activity: Instant,
}

pub struct SessionManager {
    sessions: RwLock<HashMap<Uuid, ManagedSession>>,
    max_concurrent: usize,
    daily_cap: usize,
    monthly_cap: usize,
    session_timeout: Duration,
    daily_count: AtomicUsize,
    monthly_count: AtomicUsize,
    /// Wall-clock day/month markers for detecting rollovers
    current_day: RwLock<u32>,   // day-of-month (1–31)
    current_month: RwLock<u32>, // month (1–12)
}

/// Why a session could not be created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionDenied {
    AtCapacity,
    DailyLimitReached,
    MonthlyLimitReached,
}

impl std::fmt::Display for SessionDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionDenied::AtCapacity => write!(
                f,
                "We're at capacity right now. Please try again in a few minutes."
            ),
            SessionDenied::DailyLimitReached => write!(
                f,
                "We've reached our daily session limit. Please come back tomorrow."
            ),
            SessionDenied::MonthlyLimitReached => write!(
                f,
                "Free sessions for this month have been used up. Please come back next month."
            ),
        }
    }
}

impl SessionManager {
    pub fn new(
        max_concurrent: usize,
        daily_cap: usize,
        monthly_cap: usize,
        session_timeout_secs: u64,
    ) -> Self {
        let now = Local::now();
        Self {
            sessions: RwLock::new(HashMap::new()),
            max_concurrent,
            daily_cap,
            monthly_cap,
            session_timeout: Duration::from_secs(session_timeout_secs),
            daily_count: AtomicUsize::new(0),
            monthly_count: AtomicUsize::new(0),
            current_day: RwLock::new(now.day()),
            current_month: RwLock::new(now.month()),
        }
    }

    /// Try to create a new session. Returns the session ID or a denial reason.
    /// Donors bypass all caps as a thank-you for supporting the project.
    pub async fn create_session(&self, donor: bool) -> Result<Uuid, SessionDenied> {
        self.maybe_reset_counters().await;

        if !donor {
            // Check monthly cap first (most restrictive)
            if self.monthly_count.load(Ordering::Relaxed) >= self.monthly_cap {
                return Err(SessionDenied::MonthlyLimitReached);
            }

            // Check daily cap
            if self.daily_count.load(Ordering::Relaxed) >= self.daily_cap {
                return Err(SessionDenied::DailyLimitReached);
            }

            // Check concurrent limit
            let sessions = self.sessions.read().await;
            if sessions.len() >= self.max_concurrent {
                return Err(SessionDenied::AtCapacity);
            }
            drop(sessions);
        }

        // Create the session
        let session = IntakeSession::new();
        let id = session.id;

        let managed = ManagedSession {
            session,
            last_activity: Instant::now(),
        };

        self.sessions.write().await.insert(id, managed);
        self.daily_count.fetch_add(1, Ordering::Relaxed);
        self.monthly_count.fetch_add(1, Ordering::Relaxed);

        Ok(id)
    }

    /// Get mutable access to a session. Updates last_activity.
    /// Returns None if the session doesn't exist or has timed out.
    pub async fn with_session<F, R>(&self, id: &Uuid, f: F) -> Option<R>
    where
        F: FnOnce(&mut IntakeSession) -> R,
    {
        let mut sessions = self.sessions.write().await;
        let managed = sessions.get_mut(id)?;

        // Check timeout
        if managed.last_activity.elapsed() > self.session_timeout {
            sessions.remove(id);
            return None;
        }

        managed.last_activity = Instant::now();
        Some(f(&mut managed.session))
    }

    /// Remove a session (e.g., after recommendation or emergency exit).
    pub async fn remove_session(&self, id: &Uuid) {
        self.sessions.write().await.remove(id);
    }

    /// Clean up timed-out sessions. Call periodically.
    pub async fn cleanup_expired(&self) -> usize {
        let mut sessions = self.sessions.write().await;
        let before = sessions.len();
        sessions.retain(|_, managed| managed.last_activity.elapsed() <= self.session_timeout);
        before - sessions.len()
    }

    /// Current stats for the health endpoint.
    pub async fn stats(&self) -> SessionStats {
        SessionStats {
            active_sessions: self.sessions.read().await.len(),
            max_concurrent: self.max_concurrent,
            daily_used: self.daily_count.load(Ordering::Relaxed),
            daily_cap: self.daily_cap,
            monthly_used: self.monthly_count.load(Ordering::Relaxed),
            monthly_cap: self.monthly_cap,
        }
    }

    /// Reset daily/monthly counters when the wall-clock day or month rolls over.
    async fn maybe_reset_counters(&self) {
        let now = Local::now();
        let today = now.day();
        let this_month = now.month();

        {
            let current = *self.current_day.read().await;
            if current != today {
                self.daily_count.store(0, Ordering::Relaxed);
                *self.current_day.write().await = today;
                eprintln!("[session_mgr] daily counter reset (new day: {today})");
            }
        }

        {
            let current = *self.current_month.read().await;
            if current != this_month {
                self.monthly_count.store(0, Ordering::Relaxed);
                *self.current_month.write().await = this_month;
                eprintln!("[session_mgr] monthly counter reset (new month: {this_month})");
            }
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionStats {
    pub active_sessions: usize,
    pub max_concurrent: usize,
    pub daily_used: usize,
    pub daily_cap: usize,
    pub monthly_used: usize,
    pub monthly_cap: usize,
}

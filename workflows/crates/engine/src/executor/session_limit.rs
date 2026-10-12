//! Claude session limits: detect them in a failed agent run and wait for the reset instead of
//! failing the job.
//!
//! The failure text looks like
//! `... You've hit your session limit · resets 12:50am (UTC): { "errorKind": "rate_limit" }`.
//! [`classify`] recognizes it and parses the reset time (a clock time with no date, so the next
//! time it comes round). [`Executor::session_limit_outcome`] then re-queues the job for just
//! after the reset without spending its retry budget, and holds the Project's other agent jobs
//! (they share one sandbox and one Claude token) until then.

use std::time::Duration;

use chrono::{DateTime, NaiveTime, TimeDelta, Utc};
use chrono_tz::Tz;

use super::Executor;
use crate::error::Error;

/// `jobs.waiting_for` while a job waits for a session limit.
pub(crate) const WAITING_FOR: &str = "session_limit";
/// Added to the parsed reset time, so the run does not start on the very second.
const RESET_MARGIN: TimeDelta = TimeDelta::seconds(60);
/// A parsed reset time is clamped to `[now + 1 min, now + 24 h + 1 min]`.
const MAX_PARSED_WAIT: TimeDelta = TimeDelta::hours(24);
/// Back-off when the reset time is unknown: 5 min, doubling, up to 60 min.
const BACKOFF_BASE: Duration = Duration::from_secs(5 * 60);
const BACKOFF_MAX: Duration = Duration::from_secs(60 * 60);
/// A job fails once it has waited this long since its first session-limit hit.
const MAX_TOTAL_WAIT: TimeDelta = TimeDelta::days(7);

/// A failed run that hit the session limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionLimit {
    /// When the limit resets (plus a margin), if the message said so and it could be parsed.
    pub resets_at: Option<DateTime<Utc>>,
}

/// What to do with a failed agent run.
#[derive(Debug)]
pub(crate) enum LimitOutcome {
    /// Not a session limit; fail as usual.
    NotALimit,
    /// The job was re-queued until the limit resets.
    Waiting,
    /// The attempt is no longer the running one (or lost a race); nothing was done.
    Stale,
    /// A session limit, but the job has waited too long; fail with this message.
    GaveUp(String),
}

/// Is `summary` a session-limit failure? `now` anchors the reset time.
pub(crate) fn classify(summary: &str, now: DateTime<Utc>) -> Option<SessionLimit> {
    let squashed: String = summary.chars().filter(|c| !c.is_whitespace()).collect();
    let is_limit = squashed.contains("\"errorKind\":\"rate_limit\"")
        || summary.to_lowercase().contains("hit your session limit");
    if !is_limit {
        return None;
    }
    let resets_at = parse_reset(summary, now).map(|t| {
        let clamped = t.clamp(
            now + TimeDelta::minutes(1),
            now + MAX_PARSED_WAIT + TimeDelta::minutes(1),
        );
        tracing::info!(parsed = %t, resets_at = %clamped, "session limit: reset time parsed");
        clamped
    });
    if resets_at.is_none() {
        tracing::info!(summary, "session limit: no usable reset time; backing off");
    }
    Some(SessionLimit { resets_at })
}

/// The reset time in `... resets <time> (<zone>) ...`: the next occurrence of that clock time
/// in that zone, plus [`RESET_MARGIN`].
pub(crate) fn parse_reset(text: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let lower = text.to_ascii_lowercase();
    let at = lower.find("resets ")? + "resets ".len();
    let rest = &text[at..];
    let open = rest.find('(')?;
    let close = open + rest[open..].find(')')?;
    let time = parse_clock(&rest[..open])?;
    let zone: Tz = rest[open + 1..close].trim().parse().ok()?;

    let today = now.with_timezone(&zone).date_naive();
    for days in 0..3 {
        let date = today + TimeDelta::days(days);
        // `None` for a time that does not exist (DST gap): try the next day.
        let Some(local) = date.and_time(time).and_local_timezone(zone).earliest() else {
            continue;
        };
        let local = local.with_timezone(&Utc);
        if local > now {
            return Some(local + RESET_MARGIN);
        }
    }
    None
}

/// `12:50am`, `1pm`, `13:05`, `1:05 PM`.
fn parse_clock(s: &str) -> Option<NaiveTime> {
    let s: String = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '·')
        .collect::<String>()
        .to_lowercase();
    let (digits, meridiem) = if let Some(d) = s.strip_suffix("am") {
        (d, Some(false))
    } else if let Some(d) = s.strip_suffix("pm") {
        (d, Some(true))
    } else {
        (s.as_str(), None)
    };
    let (h, m) = match digits.split_once(':') {
        Some((h, m)) => (h, m),
        None => (digits, "0"),
    };
    let h: u32 = h.parse().ok()?;
    let m: u32 = m.parse().ok()?;
    let h = match meridiem {
        Some(pm) => {
            if !(1..=12).contains(&h) {
                return None;
            }
            h % 12 + if pm { 12 } else { 0 }
        }
        None => h,
    };
    NaiveTime::from_hms_opt(h, m, 0)
}

/// Delay before the next try when the reset time is unknown; `waits` is how many session-limit
/// waits the job has had so far: 5, 10, 20, 40, 60, 60, ... minutes.
pub(crate) fn backoff(waits: i32) -> Duration {
    let exp = u32::try_from(waits).unwrap_or(0).min(16);
    BACKOFF_BASE
        .saturating_mul(2u32.saturating_pow(exp))
        .min(BACKOFF_MAX)
}

/// The wait for a job with `waits` earlier waits that first hit the limit at `started`:
/// `Ok(resume_at)`, or `Err(())` when that would pass the 7 day cap.
pub(crate) fn plan_wait(
    limit: &SessionLimit,
    waits: i32,
    started: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>, ()> {
    let resume_at = limit.resets_at.unwrap_or_else(|| {
        now + TimeDelta::from_std(backoff(waits)).unwrap_or(TimeDelta::hours(1))
    });
    let cap = started.unwrap_or(now) + MAX_TOTAL_WAIT;
    if now >= cap || resume_at > cap {
        Err(())
    } else {
        Ok(resume_at)
    }
}

/// The key a session-limit hold is stored under. One Project is one sandbox and one Claude
/// token; widen the hold by changing only this.
fn hold_key(project_id: Option<&str>) -> Option<&str> {
    project_id
}

impl Executor {
    /// A running agent job's attempt failed with `summary`. If that is a session limit, put the
    /// job back to `queued` until the reset (refunding the attempt) and hold its Project.
    /// `failure` is the error the job would otherwise fail with, quoted if the cap is reached.
    pub(crate) async fn session_limit_outcome(
        &self,
        job_id: i64,
        attempt: i32,
        summary: &str,
        failure: &str,
    ) -> Result<LimitOutcome, Error> {
        let now = Utc::now();
        let Some(limit) = classify(summary, now) else {
            return Ok(LimitOutcome::NotALimit);
        };
        let row: Option<(Option<String>, Option<DateTime<Utc>>, i32)> = sqlx::query_as(
            "SELECT project_id, limit_wait_started_at, limit_wait_count FROM jobs
             WHERE id = $1 AND status = 'running' AND attempt = $2",
        )
        .bind(job_id)
        .bind(attempt)
        .fetch_optional(&self.pool)
        .await?;
        // A stale attempt: nothing to do, and ingesting a result would drop it anyway.
        let Some((project, started, waits)) = row else {
            return Ok(LimitOutcome::Stale);
        };
        let Ok(resume_at) = plan_wait(&limit, waits, started, now) else {
            return Ok(LimitOutcome::GaveUp(format!(
                "session limit never reset after 7 days of waiting (last error: {failure})"
            )));
        };

        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE jobs SET status = 'queued', lease_owner = NULL, lease_until = NULL,
                    deadline = NULL, run_after = $3, max_attempts = max_attempts + 1,
                    limit_wait_started_at = COALESCE(limit_wait_started_at, $4),
                    limit_wait_count = limit_wait_count + 1,
                    waiting_for = $5, resumes_at = $3, input = input - 'recovery_used',
                    updated_at = now()
             WHERE id = $1 AND status = 'running' AND attempt = $2",
        )
        .bind(job_id)
        .bind(attempt)
        .bind(resume_at)
        .bind(now)
        .bind(WAITING_FOR)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            return Ok(LimitOutcome::Stale);
        }
        if let Some(key) = hold_key(project.as_deref()) {
            sqlx::query(
                "INSERT INTO project_holds (project_id, reason, blocked_until)
                 VALUES ($1, $2, $3)
                 ON CONFLICT (project_id) DO UPDATE
                 SET blocked_until = GREATEST(project_holds.blocked_until, EXCLUDED.blocked_until),
                     reason = EXCLUDED.reason, set_at = now()",
            )
            .bind(key)
            .bind(WAITING_FOR)
            .bind(resume_at)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        tracing::warn!(job_id, %resume_at, known_reset = limit.resets_at.is_some(), "agent run hit the session limit; job requeued");
        Ok(LimitOutcome::Waiting)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    const UI48: &str = "session/prompt: Internal error: You've hit your session limit · resets 12:50am (UTC): {\n  \"errorKind\": \"rate_limit\"\n}";

    #[test]
    fn classifies_the_ui_48_message() {
        let now = at(2026, 10, 11, 1, 0);
        let l = classify(UI48, now).unwrap();
        // 00:50 has passed, so the next one is tomorrow, plus the margin.
        assert_eq!(l.resets_at, Some(at(2026, 10, 12, 0, 51)));
    }

    #[test]
    fn falls_back_to_the_text_and_ignores_other_failures() {
        let now = at(2026, 10, 11, 1, 0);
        assert!(classify("You've hit your session limit, try later", now).is_some());
        assert!(classify("{\"errorKind\":\"rate_limit\"}", now).is_some());
        assert!(classify("exit code 1: compile error", now).is_none());
        assert!(classify("errorKind: other", now).is_none());
    }

    #[test]
    fn reset_before_midnight_rolls_over() {
        let now = at(2026, 10, 11, 23, 0);
        assert_eq!(
            parse_reset("resets 12:50am (UTC)", now),
            Some(at(2026, 10, 12, 0, 51))
        );
        assert_eq!(
            parse_reset("resets 12:50pm (UTC)", now),
            Some(at(2026, 10, 12, 12, 51))
        );
        let noon = at(2026, 10, 11, 5, 0);
        assert_eq!(
            parse_reset("resets 12:50pm (UTC)", noon),
            Some(at(2026, 10, 11, 12, 51))
        );
    }

    #[test]
    fn time_formats() {
        let now = at(2026, 10, 11, 0, 0);
        for (s, want) in [
            ("resets 1pm (UTC)", at(2026, 10, 11, 13, 1)),
            ("resets 13:05 (UTC)", at(2026, 10, 11, 13, 6)),
            ("resets 1:05 PM (UTC)", at(2026, 10, 11, 13, 6)),
            ("resets 12am (GMT)", at(2026, 10, 12, 0, 1)),
        ] {
            assert_eq!(parse_reset(s, now), Some(want), "{s}");
        }
    }

    #[test]
    fn iana_zone() {
        // 10:00 UTC on 11 Oct 2026 is 06:00 in New York (EDT, UTC-4).
        let now = at(2026, 10, 11, 10, 0);
        assert_eq!(
            parse_reset("resets 7am (America/New_York)", now),
            Some(at(2026, 10, 11, 11, 1))
        );
    }

    #[test]
    fn bad_time_or_zone_means_no_reset_time() {
        let now = at(2026, 10, 11, 0, 0);
        for s in [
            "resets soon (UTC)",
            "resets 13pm (UTC)",
            "resets 25:00 (UTC)",
            "resets 1pm (Mars/Olympus)",
            "resets 1pm",
            "no time here",
        ] {
            assert_eq!(parse_reset(s, now), None, "{s}");
        }
        let l = classify("hit your session limit · resets 1pm (Mars/Olympus)", now).unwrap();
        assert_eq!(l.resets_at, None);
    }

    #[test]
    fn parsed_reset_is_clamped() {
        // 1 s before the reset: the margin keeps it past now + 1 min anyway.
        let now = at(2026, 10, 11, 12, 59);
        let l = classify("hit your session limit · resets 1pm (UTC)", now).unwrap();
        assert_eq!(l.resets_at, Some(now + TimeDelta::minutes(2)));
        // Never more than 24 h + 1 min away.
        let now = at(2026, 10, 11, 13, 0);
        let l = classify("hit your session limit · resets 1pm (UTC)", now).unwrap();
        assert!(l.resets_at.unwrap() <= now + TimeDelta::hours(24) + TimeDelta::minutes(1));
        let l = classify("hit your session limit · resets 12:59pm (UTC)", now).unwrap();
        assert_eq!(
            l.resets_at,
            Some(now + TimeDelta::hours(24) + TimeDelta::minutes(0))
        );
    }

    #[test]
    fn backoff_progression() {
        let mins: Vec<u64> = (0..6).map(|w| backoff(w).as_secs() / 60).collect();
        assert_eq!(mins, [5, 10, 20, 40, 60, 60]);
        assert_eq!(backoff(1000).as_secs(), 3600);
    }

    #[test]
    fn seven_day_cap() {
        let now = at(2026, 10, 11, 12, 0);
        let known = SessionLimit {
            resets_at: Some(now + TimeDelta::hours(2)),
        };
        let unknown = SessionLimit { resets_at: None };
        // First hit.
        assert_eq!(
            plan_wait(&known, 0, None, now),
            Ok(now + TimeDelta::hours(2))
        );
        assert_eq!(
            plan_wait(&unknown, 2, None, now),
            Ok(now + TimeDelta::minutes(20))
        );
        // Started 6 days 23 h ago: a 2 h wait would pass the cap.
        let started = now - TimeDelta::days(6) - TimeDelta::hours(23);
        assert_eq!(plan_wait(&known, 9, Some(started), now), Err(()));
        assert!(plan_wait(&unknown, 9, Some(started), now).is_ok());
        // Past the cap already.
        let old = now - TimeDelta::days(7);
        assert_eq!(plan_wait(&unknown, 9, Some(old), now), Err(()));
    }
}

//! Calendar and decision contract for owner-authorized recurring allowances.
//! Pure descriptions only: callers must authenticate owner consent and commit
//! each period atomically. No clock, process, database or spending authority.
use chrono::{DateTime, Datelike, Days, NaiveDate, TimeZone, Utc};
use chrono_tz::{GapInfo, Tz};
use serde::{Deserialize, Serialize};

const MAX_EXACT: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cadence {
    Daily,
    Weekly,
    Monthly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Calendar {
    pub cadence: Cadence,
    /// Owner-selected IANA name, independent of the executor's machine zone.
    pub time_zone: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Period {
    /// Local date at the start (Monday for weeks), or YYYY-MM for months.
    pub key: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenewalMode {
    Automatic,
    Owner,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Terms {
    pub calendar: Calendar,
    pub tokens_per_period: u64,
    pub renewal_mode: RenewalMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("Select a valid IANA time zone")]
    TimeZone,
    #[error("Calendar date is outside the supported 1970–9998 range")]
    Date,
    #[error("Token allowance must be a positive exact integer")]
    Tokens,
    #[error("The stored period does not match its approved calendar")]
    Period,
    #[error("A newer period has already been authorized")]
    ClockReversed,
    #[error("Usage evidence is inconsistent or exceeds the exact token range")]
    Usage,
}

fn timestamp(value: DateTime<Tz>) -> Result<u64, Error> {
    u64::try_from(value.timestamp_millis())
        .ok()
        .filter(|n| *n <= MAX_EXACT)
        .ok_or(Error::Date)
}

/// Earliest valid boundary. Some zones change offset at midnight; adding 24 h
/// or requiring midnight to be unique would issue the wrong period there.
fn midnight(zone: Tz, date: NaiveDate) -> Result<u64, Error> {
    let local = date.and_hms_opt(0, 0, 0).ok_or(Error::Date)?;
    timestamp(
        zone.from_local_datetime(&local)
            .earliest()
            .or_else(|| GapInfo::new(&local, &zone).and_then(|gap| gap.end))
            .ok_or(Error::Date)?,
    )
}

impl Calendar {
    pub fn validate(&self) -> Result<(), Error> {
        self.zone().map(|_| ())
    }

    fn zone(&self) -> Result<Tz, Error> {
        if self.time_zone.is_empty() || self.time_zone.len() > 128 {
            return Err(Error::TimeZone);
        }
        self.time_zone.parse().map_err(|_| Error::TimeZone)
    }

    pub fn period_at(&self, now_ms: u64) -> Result<Period, Error> {
        let zone = self.zone()?;
        let now = i64::try_from(now_ms)
            .ok()
            .and_then(DateTime::<Utc>::from_timestamp_millis)
            .filter(|_| now_ms <= MAX_EXACT)
            .ok_or(Error::Date)?
            .with_timezone(&zone);
        if !(1970..=9998).contains(&now.year()) {
            return Err(Error::Date);
        }
        let date = now.date_naive();
        let start = match self.cadence {
            Cadence::Daily => date,
            Cadence::Weekly => date
                .checked_sub_days(Days::new(date.weekday().num_days_from_monday().into()))
                .ok_or(Error::Date)?,
            Cadence::Monthly => date.with_day(1).ok_or(Error::Date)?,
        };
        let end = match self.cadence {
            Cadence::Daily => start.checked_add_days(Days::new(1)),
            Cadence::Weekly => start.checked_add_days(Days::new(7)),
            Cadence::Monthly => {
                let (year, month) = if start.month() == 12 {
                    (start.year() + 1, 1)
                } else {
                    (start.year(), start.month() + 1)
                };
                NaiveDate::from_ymd_opt(year, month, 1)
            }
        }
        .ok_or(Error::Date)?;
        let period = Period {
            key: match self.cadence {
                Cadence::Monthly => start.format("%Y-%m").to_string(),
                _ => start.format("%Y-%m-%d").to_string(),
            },
            start_ms: midnight(zone, start)?,
            end_ms: midnight(zone, end)?,
        };
        if period.start_ms > now_ms || now_ms >= period.end_ms {
            return Err(Error::Period);
        }
        Ok(period)
    }

    /// Recompute rather than trusting a remote key or claimed UTC duration.
    pub fn validate_period(&self, period: &Period) -> Result<(), Error> {
        if self.period_at(period.start_ms)? != *period {
            return Err(Error::Period);
        }
        Ok(())
    }

    /// The next real window; a completely skipped local date gets no credit.
    pub fn next_period(&self, period: &Period) -> Result<Period, Error> {
        self.validate_period(period)?;
        self.period_at(period.end_ms)
    }
}

impl Terms {
    pub fn validate(&self) -> Result<(), Error> {
        self.calendar.validate()?;
        if self.tokens_per_period == 0 || self.tokens_per_period > MAX_EXACT {
            return Err(Error::Tokens);
        }
        Ok(())
    }

    /// Plans only the current window, never a sum of missed allowances. The
    /// previous row must be the latest committed row under these same terms.
    /// `owner_approved` is supplied by trusted authorization code, not a UI flag.
    pub fn renewal_due(
        &self,
        now_ms: u64,
        previous: Option<&Period>,
        owner_approved: bool,
    ) -> Result<Renewal, Error> {
        self.validate()?;
        let period = self.calendar.period_at(now_ms)?;
        if let Some(previous) = previous {
            self.calendar.validate_period(previous)?;
            if previous.start_ms > period.start_ms {
                return Err(Error::ClockReversed);
            }
            if previous == &period {
                return Ok(Renewal::AlreadyAuthorized);
            }
        }
        // Standing automatic consent applies to renewals, not first approval.
        if owner_approved || previous.is_some() && self.renewal_mode == RenewalMode::Automatic {
            Ok(Renewal::Authorize {
                period,
                tokens: self.tokens_per_period,
            })
        } else {
            Ok(Renewal::AwaitingOwner { period })
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Renewal {
    AlreadyAuthorized,
    AwaitingOwner { period: Period },
    Authorize { period: Period, tokens: u64 },
}

/// The period is pinned when a call is admitted, not when its usage arrives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Charge {
    pub period: Period,
    pub reserved_tokens: u64,
    pub consumed_tokens: u64,
    pub state: ChargeState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChargeState {
    /// Includes issued, running, unknown and settled-with-incomplete-usage.
    Unsettled,
    /// Qualified, final usage; this is the only state releasing a call's hold.
    Settled,
    CancelledBeforeIssue,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Budget {
    pub tokens_per_period: u64,
    pub consumed_tokens: u64,
    pub retained_tokens: u64,
    pub remaining_tokens: u64,
    /// Prior-period unknown work remains a resource liability, but late known
    /// usage stays in its original period instead of charging the new period.
    pub prior_unsettled_tokens: u64,
    pub usage_complete: bool,
}

fn add_tokens(total: u64, amount: u64) -> Result<u64, Error> {
    total
        .checked_add(amount)
        .filter(|v| *v <= MAX_EXACT)
        .ok_or(Error::Usage)
}

impl Terms {
    /// A projection of already verified, unique call records. Callers must
    /// recompute inside the admission transaction; this value cannot authorize
    /// spending, renewal or release of unresolved prior resource liabilities.
    pub fn budget<'a>(
        &self,
        period: &Period,
        charges: impl IntoIterator<Item = &'a Charge>,
    ) -> Result<Budget, Error> {
        self.validate()?;
        self.calendar.validate_period(period)?;
        let mut out = Budget {
            tokens_per_period: self.tokens_per_period,
            consumed_tokens: 0,
            retained_tokens: 0,
            remaining_tokens: self.tokens_per_period,
            prior_unsettled_tokens: 0,
            usage_complete: true,
        };
        for charge in charges {
            self.calendar.validate_period(&charge.period)?;
            if charge.period.start_ms > period.start_ms
                || charge.reserved_tokens > MAX_EXACT
                || charge.consumed_tokens > MAX_EXACT
            {
                return Err(Error::Usage);
            }
            let retained = match charge.state {
                ChargeState::Unsettled => charge.reserved_tokens.max(charge.consumed_tokens),
                ChargeState::Settled => charge.consumed_tokens,
                ChargeState::CancelledBeforeIssue if charge.consumed_tokens == 0 => 0,
                ChargeState::CancelledBeforeIssue => return Err(Error::Usage),
            };
            if charge.period == *period {
                out.consumed_tokens = add_tokens(out.consumed_tokens, charge.consumed_tokens)?;
                out.retained_tokens = add_tokens(out.retained_tokens, retained)?;
            } else if charge.state == ChargeState::Unsettled {
                out.prior_unsettled_tokens = add_tokens(out.prior_unsettled_tokens, retained)?;
            }
            out.usage_complete &= charge.state != ChargeState::Unsettled;
        }
        out.remaining_tokens = self.tokens_per_period.saturating_sub(out.retained_tokens);
        Ok(out)
    }
}

#[cfg(test)]
mod tests;

use super::*;

fn ms(value: &str) -> u64 {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .timestamp_millis() as u64
}
fn calendar(cadence: Cadence, zone: &str) -> Calendar {
    Calendar {
        cadence,
        time_zone: zone.into(),
    }
}
fn terms(mode: RenewalMode) -> Terms {
    Terms {
        calendar: calendar(Cadence::Monthly, "America/Los_Angeles"),
        tokens_per_period: 1_000_000,
        renewal_mode: mode,
    }
}

#[test]
fn los_angeles_spring_day_has_23_hours() {
    let c = calendar(Cadence::Daily, "America/Los_Angeles");
    let p = c.period_at(ms("2026-03-08T16:00:00Z")).unwrap();
    assert_eq!(p.key, "2026-03-08");
    assert_eq!(p.start_ms, ms("2026-03-08T08:00:00Z"));
    assert_eq!(p.end_ms, ms("2026-03-09T07:00:00Z"));
    assert_eq!(p.end_ms - p.start_ms, 23 * 3_600_000);
    assert_eq!(c.period_at(p.end_ms - 1).unwrap(), p);
    assert_eq!(c.period_at(p.end_ms).unwrap().key, "2026-03-09");
}

#[test]
fn los_angeles_autumn_day_has_25_hours_and_one_allowance() {
    let c = calendar(Cadence::Daily, "America/Los_Angeles");
    let first = c.period_at(ms("2026-11-01T01:30:00-07:00")).unwrap();
    let repeated = c.period_at(ms("2026-11-01T01:30:00-08:00")).unwrap();
    assert_eq!(first, repeated);
    assert_eq!(first.end_ms - first.start_ms, 25 * 3_600_000);
}

#[test]
fn weeks_start_on_monday_and_follow_dst() {
    let c = calendar(Cadence::Weekly, "America/Los_Angeles");
    let p = c.period_at(ms("2026-11-01T23:59:59-08:00")).unwrap();
    assert_eq!(p.key, "2026-10-26");
    assert_eq!(p.end_ms, ms("2026-11-02T00:00:00-08:00"));
    assert_eq!(p.end_ms - p.start_ms, 169 * 3_600_000);
    assert_eq!(c.next_period(&p).unwrap().key, "2026-11-02");
}

#[test]
fn week_crossing_year_uses_monday_date() {
    let p = calendar(Cadence::Weekly, "Asia/Shanghai")
        .period_at(ms("2027-01-01T12:00:00+08:00"))
        .unwrap();
    assert_eq!(p.key, "2026-12-28");
    assert_eq!(p.end_ms, ms("2027-01-04T00:00:00+08:00"));
}

#[test]
fn months_use_owner_midnight_not_utc_or_machine_zone() {
    let c = calendar(Cadence::Monthly, "America/Los_Angeles");
    let before = c.period_at(ms("2026-11-01T06:59:59Z")).unwrap();
    assert_eq!(before.key, "2026-10");
    let p = c.next_period(&before).unwrap();
    assert_eq!(p.key, "2026-11");
    assert_eq!(p.start_ms, ms("2026-11-01T07:00:00Z"));
    assert_eq!(p.end_ms, ms("2026-12-01T08:00:00Z"));
    assert_eq!(p.end_ms - p.start_ms, (30 * 24 + 1) * 3_600_000);
}

#[test]
fn leap_month_and_year_end() {
    let c = calendar(Cadence::Monthly, "UTC");
    let leap = c.period_at(ms("2028-02-29T12:00:00Z")).unwrap();
    assert_eq!(leap.end_ms - leap.start_ms, 29 * 24 * 3_600_000);
    assert_eq!(c.next_period(&leap).unwrap().key, "2028-03");
    let december = c.period_at(ms("2026-12-31T23:59:59Z")).unwrap();
    assert_eq!(c.next_period(&december).unwrap().key, "2027-01");
}

#[test]
fn fractional_offset_uses_local_date() {
    let p = calendar(Cadence::Daily, "Asia/Kathmandu")
        .period_at(ms("2026-11-01T00:00:00+05:45"))
        .unwrap();
    assert_eq!(p.start_ms, ms("2026-10-31T18:15:00Z"));
}

#[test]
fn nonexistent_midnight_starts_at_first_valid_instant() {
    let c = calendar(Cadence::Daily, "America/Sao_Paulo");
    let p = c.period_at(ms("2018-11-04T12:00:00-02:00")).unwrap();
    assert_eq!(p.start_ms, ms("2018-11-04T01:00:00-02:00"));
    assert_eq!(p.end_ms - p.start_ms, 23 * 3_600_000);
    c.validate_period(&p).unwrap();
}

#[test]
fn repeated_midnight_starts_at_first_occurrence() {
    let c = calendar(Cadence::Daily, "America/Havana");
    let p = c.period_at(ms("2026-11-01T12:00:00-05:00")).unwrap();
    assert_eq!(p.start_ms, ms("2026-11-01T00:00:00-04:00"));
    assert_eq!(p.end_ms - p.start_ms, 25 * 3_600_000);
}

#[test]
fn skipped_day_is_not_a_period_to_fund() {
    let c = calendar(Cadence::Daily, "Pacific/Apia");
    let p = c.period_at(ms("2011-12-29T12:00:00-10:00")).unwrap();
    assert_eq!(c.next_period(&p).unwrap().key, "2011-12-31");
}

#[test]
fn rejects_fabricated_calendar_windows() {
    let c = calendar(Cadence::Monthly, "America/Los_Angeles");
    let p = c.period_at(ms("2026-11-10T00:00:00Z")).unwrap();
    for altered in [
        Period {
            start_ms: p.start_ms + 1,
            ..p.clone()
        },
        Period {
            end_ms: p.end_ms + 1,
            ..p.clone()
        },
        Period {
            key: "2026-12".into(),
            ..p
        },
    ] {
        assert!(c.validate_period(&altered).is_err());
    }
}

#[test]
fn invalid_zone_time_and_amount_fail_without_defaults() {
    for zone in ["", "local", "+08:00", "Mars/Olympus", " Asia/Shanghai"] {
        assert_eq!(
            calendar(Cadence::Daily, zone).validate(),
            Err(Error::TimeZone)
        );
    }
    assert!(calendar(Cadence::Daily, "UTC").period_at(u64::MAX).is_err());
    for tokens in [0, MAX_EXACT + 1, u64::MAX] {
        assert_eq!(
            Terms {
                tokens_per_period: tokens,
                ..terms(RenewalMode::Owner)
            }
            .validate(),
            Err(Error::Tokens)
        );
    }
}

#[test]
fn both_modes_require_initial_owner_approval() {
    let now = ms("2026-11-10T00:00:00Z");
    for mode in [RenewalMode::Automatic, RenewalMode::Owner] {
        let t = terms(mode);
        assert!(matches!(
            t.renewal_due(now, None, false).unwrap(),
            Renewal::AwaitingOwner { .. }
        ));
        assert!(matches!(
            t.renewal_due(now, None, true).unwrap(),
            Renewal::Authorize {
                tokens: 1_000_000,
                ..
            }
        ));
    }
}

#[test]
fn manual_and_automatic_cannot_refill_current_period() {
    let now = ms("2026-11-10T00:00:00Z");
    for mode in [RenewalMode::Automatic, RenewalMode::Owner] {
        let t = terms(mode);
        let p = t.calendar.period_at(now).unwrap();
        for owner in [false, true] {
            assert_eq!(
                t.renewal_due(now, Some(&p), owner).unwrap(),
                Renewal::AlreadyAuthorized
            );
        }
    }
}

#[test]
fn automatic_renews_exactly_at_boundary_not_before() {
    let t = terms(RenewalMode::Automatic);
    let p = t.calendar.period_at(ms("2026-10-20T00:00:00Z")).unwrap();
    assert_eq!(
        t.renewal_due(p.end_ms - 1, Some(&p), false).unwrap(),
        Renewal::AlreadyAuthorized
    );
    assert_eq!(
        t.renewal_due(p.end_ms, Some(&p), false).unwrap(),
        Renewal::Authorize {
            period: t.calendar.next_period(&p).unwrap(),
            tokens: 1_000_000,
        }
    );
}

#[test]
fn offline_skipped_periods_get_no_accumulated_credit() {
    let t = terms(RenewalMode::Automatic);
    let p = t.calendar.period_at(ms("2026-01-10T00:00:00Z")).unwrap();
    let now = ms("2026-11-10T00:00:00Z");
    assert_eq!(
        t.renewal_due(now, Some(&p), false).unwrap(),
        Renewal::Authorize {
            period: t.calendar.period_at(now).unwrap(),
            tokens: 1_000_000,
        }
    );
}

#[test]
fn owner_mode_waits_for_new_owner_decision() {
    let t = terms(RenewalMode::Owner);
    let p = t.calendar.period_at(ms("2026-10-20T00:00:00Z")).unwrap();
    assert!(matches!(
        t.renewal_due(p.end_ms, Some(&p), false).unwrap(),
        Renewal::AwaitingOwner { .. }
    ));
    assert!(matches!(
        t.renewal_due(p.end_ms, Some(&p), true).unwrap(),
        Renewal::Authorize { .. }
    ));
}

#[test]
fn clock_rollback_cannot_reissue_older_period() {
    let t = terms(RenewalMode::Automatic);
    let p = t.calendar.period_at(ms("2026-11-10T00:00:00Z")).unwrap();
    assert_eq!(
        t.renewal_due(p.start_ms - 1, Some(&p), true),
        Err(Error::ClockReversed)
    );
}

#[test]
fn wire_contract_rejects_unknown_modes_and_fields() {
    let original = terms(RenewalMode::Automatic);
    let encoded = serde_json::to_string(&original).unwrap();
    assert_eq!(serde_json::from_str::<Terms>(&encoded).unwrap(), original);
    assert!(serde_json::from_str::<Terms>(&encoded.replace("automatic", "unlimited")).is_err());
    assert!(
        serde_json::from_str::<Terms>(&encoded.replace("tokensPerPeriod", "tokenLimit")).is_err()
    );
}

fn charge(t: &Terms, date: &str, reserved: u64, consumed: u64, state: ChargeState) -> Charge {
    Charge {
        period: t.calendar.period_at(ms(date)).unwrap(),
        reserved_tokens: reserved,
        consumed_tokens: consumed,
        state,
    }
}

#[test]
fn unused_tokens_do_not_carry_and_prior_spend_does_not_reduce_new_allowance() {
    let t = terms(RenewalMode::Automatic);
    let old = charge(
        &t,
        "2026-10-10T00:00:00Z",
        600_000,
        400_000,
        ChargeState::Settled,
    );
    let new = t.calendar.next_period(&old.period).unwrap();
    let b = t.budget(&new, [&old]).unwrap();
    assert_eq!(b.remaining_tokens, 1_000_000);
    assert_eq!(b.consumed_tokens, 0);
    assert_eq!(b.prior_unsettled_tokens, 0);
}

#[test]
fn late_old_usage_never_spends_or_refunds_new_period() {
    let t = terms(RenewalMode::Automatic);
    let mut old = charge(
        &t,
        "2026-10-10T00:00:00Z",
        600_000,
        50_000,
        ChargeState::Unsettled,
    );
    let new = charge(
        &t,
        "2026-11-10T00:00:00Z",
        400_000,
        300_000,
        ChargeState::Settled,
    );
    let before = t.budget(&new.period, [&old, &new]).unwrap();
    assert_eq!(before.remaining_tokens, 700_000);
    assert_eq!(before.prior_unsettled_tokens, 600_000);
    assert!(!before.usage_complete);
    old.consumed_tokens = 80_000;
    old.state = ChargeState::Settled;
    let after = t.budget(&new.period, [&old, &new]).unwrap();
    assert_eq!(after.remaining_tokens, before.remaining_tokens);
    assert_eq!(after.prior_unsettled_tokens, 0);
    assert!(after.usage_complete);
}

#[test]
fn unknown_usage_holds_at_least_reservation_and_overrun_is_not_negative_credit() {
    let t = terms(RenewalMode::Owner);
    let mut c = charge(
        &t,
        "2026-11-10T00:00:00Z",
        1_000_000,
        0,
        ChargeState::Unsettled,
    );
    let b = t.budget(&c.period, [&c]).unwrap();
    assert_eq!((b.retained_tokens, b.remaining_tokens), (1_000_000, 0));
    c.consumed_tokens = 1_200_000;
    let b = t.budget(&c.period, [&c]).unwrap();
    assert_eq!((b.retained_tokens, b.remaining_tokens), (1_200_000, 0));
}

#[test]
fn cancellation_only_releases_an_unissued_call_with_zero_consumption() {
    let t = terms(RenewalMode::Owner);
    let mut c = charge(
        &t,
        "2026-11-10T00:00:00Z",
        1_000_000,
        0,
        ChargeState::CancelledBeforeIssue,
    );
    assert_eq!(
        t.budget(&c.period, [&c]).unwrap().remaining_tokens,
        1_000_000
    );
    c.consumed_tokens = 1;
    assert_eq!(t.budget(&c.period, [&c]), Err(Error::Usage));
}

#[test]
fn corrupt_and_future_usage_cannot_be_treated_as_zero() {
    let t = terms(RenewalMode::Owner);
    let c = charge(
        &t,
        "2026-11-10T00:00:00Z",
        MAX_EXACT,
        MAX_EXACT,
        ChargeState::Settled,
    );
    assert_eq!(t.budget(&c.period, [&c, &c]), Err(Error::Usage));
    let old = t.calendar.period_at(ms("2026-10-10T00:00:00Z")).unwrap();
    assert_eq!(t.budget(&old, [&c]), Err(Error::Usage));
    let bad = Charge {
        reserved_tokens: u64::MAX,
        ..c
    };
    assert_eq!(t.budget(&bad.period, [&bad]), Err(Error::Usage));
}

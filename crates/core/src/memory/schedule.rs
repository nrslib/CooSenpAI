use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime};

#[derive(Debug, Clone)]
pub struct ScheduleInput {
    pub local_now: NaiveDateTime,
    pub grace_minutes: u64,
    pub available_daily_periods: Vec<NaiveDate>,
    pub current_daily_periods: Vec<NaiveDate>,
    pub failed_or_stale_daily_periods: Vec<NaiveDate>,
    pub daily_jobs_today: Vec<String>,
    pub weekly_jobs_this_week: Vec<String>,
    pub stale_weekly_periods: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySchedule {
    pub daily: Vec<String>,
    pub weekly: Vec<String>,
    pub delayed_daily: usize,
    pub delayed_weekly: usize,
}

pub fn select_schedule(input: &ScheduleInput) -> MemorySchedule {
    let day = input.local_now.date();
    let grace = Duration::minutes(input.grace_minutes.min(i64::MAX as u64) as i64);
    let previous_day = day.pred_opt();
    let mut needed = input
        .available_daily_periods
        .iter()
        .copied()
        .filter(|period| {
            Some(*period) != previous_day
                && (!input.current_daily_periods.contains(period)
                    || input.failed_or_stale_daily_periods.contains(period))
        })
        .filter(|period| *period < day)
        .collect::<Vec<_>>();
    needed.sort_unstable();
    needed.dedup();
    let mut daily = Vec::new();
    if let Some(previous) = previous_day.filter(|period| {
        daily_period_is_ready(*period, input.local_now, grace)
            && input.available_daily_periods.contains(period)
            && (!input.current_daily_periods.contains(period)
                || input.failed_or_stale_daily_periods.contains(period))
    }) {
        push_if_not_run(&mut daily, previous.to_string(), &input.daily_jobs_today);
    }
    if let Some(catch_up) = needed
        .iter()
        .find(|period| daily_period_is_ready(**period, input.local_now, grace))
    {
        push_if_not_run(&mut daily, catch_up.to_string(), &input.daily_jobs_today);
    }
    daily.truncate(2);

    let week_start = day - Duration::days(i64::from(day.weekday().num_days_from_monday()));
    let weekly_deadline = week_start.and_time(NaiveTime::MIN) + Duration::hours(2) + grace;
    let weekly_ready = input.local_now >= weekly_deadline;
    let previous_week = previous_iso_week(day);
    let mut weekly = Vec::new();
    if weekly_ready {
        push_if_not_run(&mut weekly, previous_week, &input.weekly_jobs_this_week);
        if let Some(stale) = input
            .stale_weekly_periods
            .iter()
            .filter(|period| !weekly.contains(period))
            .min()
        {
            push_if_not_run(&mut weekly, stale.clone(), &input.weekly_jobs_this_week);
        }
    }
    weekly.truncate(2);

    let scheduled_backlog = daily
        .iter()
        .filter(|period| previous_day.is_none_or(|day| period.as_str() != day.to_string()))
        .count();
    MemorySchedule {
        delayed_daily: needed.len().saturating_sub(scheduled_backlog),
        delayed_weekly: input
            .stale_weekly_periods
            .len()
            .saturating_sub(weekly.len()),
        daily,
        weekly,
    }
}

fn push_if_not_run(output: &mut Vec<String>, period: String, completed_slots: &[String]) {
    if !completed_slots.contains(&period) && !output.contains(&period) {
        output.push(period);
    }
}

fn daily_period_is_ready(period: NaiveDate, local_now: NaiveDateTime, grace: Duration) -> bool {
    period
        .succ_opt()
        .is_some_and(|next_day| local_now >= next_day.and_time(NaiveTime::MIN) + grace)
}

fn previous_iso_week(day: NaiveDate) -> String {
    let previous = day - Duration::days(7);
    let week = previous.iso_week();
    format!("{}-W{:02}", week.year(), week.week())
}

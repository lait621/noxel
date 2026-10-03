//! Daily schedules: where a townsperson is, hour by hour.
//!
//! A day is a sorted list of [`ScheduleEntry`] boundaries covering `[0, 24)`
//! exactly once. That invariant is what makes the lookup a binary search and
//! what makes [`DailySchedule::to_text`] readable as a timetable:
//!
//! ```text
//! 00:00  sleep       home
//! 06:30  eat         tavern
//! 07:00  commute     road
//! 08:00  work        work
//! ...
//! 22:00  sleep       home
//! ```
//!
//! The boundaries are derived from the world seed, so two villages keep
//! slightly different hours — the baker opens at 08:12 in one town and 07:57 in
//! the next — without any per-agent storage: the schedule is a pure function of
//! `(kind, seed)`.

use noxel_core::rng::RngStream;

use crate::agent::NpcKind;

/// What an agent is scheduled to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    /// Asleep, at home.
    Sleep,
    /// Travelling between two places.
    Commute,
    /// At a workplace.
    Work,
    /// Eating a meal.
    Eat,
    /// Standing around with neighbours, talking.
    Socialise,
    /// Buying or selling at the market.
    Shop,
    /// Walking a beat, as a guard does.
    Patrol,
    /// Wandering with no particular purpose, as a child or an animal does.
    Wander,
    /// Deliberately doing nothing.
    Idle,
    /// Heading home for the night.
    GoHome,
}

impl Activity {
    /// Every activity, in a fixed order.
    pub const ALL: [Self; 10] = [
        Self::Sleep,
        Self::Commute,
        Self::Work,
        Self::Eat,
        Self::Socialise,
        Self::Shop,
        Self::Patrol,
        Self::Wander,
        Self::Idle,
        Self::GoHome,
    ];

    /// A short human-readable name, for debug views and tests.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Sleep => "sleep",
            Self::Commute => "commute",
            Self::Work => "work",
            Self::Eat => "eat",
            Self::Socialise => "socialise",
            Self::Shop => "shop",
            Self::Patrol => "patrol",
            Self::Wander => "wander",
            Self::Idle => "idle",
            Self::GoHome => "go home",
        }
    }

    /// Where the activity happens, relative to a town.
    ///
    /// The crowd system turns this into a target: `Home` goes to the agent's
    /// own anchor, `Work` to the nearest workplace, `Anywhere` to a walkable
    /// point near the crowd centre.
    #[must_use]
    pub fn place(self) -> ActivityPlace {
        match self {
            Self::Sleep | Self::GoHome => ActivityPlace::Home,
            Self::Work => ActivityPlace::Work,
            Self::Socialise => ActivityPlace::Plaza,
            Self::Shop => ActivityPlace::Market,
            Self::Eat => ActivityPlace::Tavern,
            Self::Commute | Self::Patrol => ActivityPlace::Road,
            Self::Wander | Self::Idle => ActivityPlace::Anywhere,
        }
    }
}

/// The kind of place an [`Activity`] happens at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityPlace {
    /// The agent's own home.
    Home,
    /// A workplace.
    Work,
    /// The town square: open ground where people meet.
    Plaza,
    /// The market: stalls and shoppers.
    Market,
    /// The tavern or inn.
    Tavern,
    /// A road or a path.
    Road,
    /// Anywhere walkable.
    Anywhere,
}

impl ActivityPlace {
    /// Every place, in a fixed order.
    pub const ALL: [Self; 7] = [
        Self::Home,
        Self::Work,
        Self::Plaza,
        Self::Market,
        Self::Tavern,
        Self::Road,
        Self::Anywhere,
    ];

    /// A short human-readable name, for debug views and tests.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Work => "work",
            Self::Plaza => "plaza",
            Self::Market => "market",
            Self::Tavern => "tavern",
            Self::Road => "road",
            Self::Anywhere => "anywhere",
        }
    }
}

/// One boundary in a day: an activity starts at `start_hour` and runs until the
/// next entry.
#[derive(Clone, Copy, Debug)]
pub struct ScheduleEntry {
    /// Start hour in `[0, 24)`.
    pub start_hour: f32,
    /// What the agent does from `start_hour` on.
    pub activity: Activity,
}

impl ScheduleEntry {
    /// A boundary at `start_hour`.
    #[must_use]
    pub fn new(start_hour: f32, activity: Activity) -> Self {
        Self {
            start_hour,
            activity,
        }
    }

    /// How long this entry lasts before `next` begins, in hours.
    ///
    /// The day wraps: `duration_until` from `22:00` to `06:30` is 8.5 hours.
    /// A non-finite or out-of-range `next.start_hour` is treated as `0.0`.
    #[must_use]
    pub fn duration_until(&self, next: &ScheduleEntry) -> f32 {
        let start = if self.start_hour.is_finite() {
            self.start_hour
        } else {
            0.0
        };
        let next_start = if next.start_hour.is_finite() {
            next.start_hour.clamp(0.0, 24.0)
        } else {
            0.0
        };
        if next_start > start {
            next_start - start
        } else {
            24.0 - start + next_start
        }
    }

    /// The start time as `HH:MM`.
    #[must_use]
    pub fn time_text(&self) -> String {
        format_hour(self.start_hour)
    }

    /// `"HH:MM activity"`, the line [`DailySchedule::to_text`] emits.
    #[must_use]
    pub fn line(&self) -> String {
        format!("{} {}", format_hour(self.start_hour), self.activity.name())
    }
}

/// One agent kind's day.
#[derive(Clone, Debug)]
pub struct DailySchedule {
    /// Boundaries, sorted by `start_hour`, the first at `0.0`.
    pub entries: Vec<ScheduleEntry>,
}

/// How the daily schedule is applied to a crowd.
///
/// The schedules themselves are derived from `(kind, seed)`; this is the part
/// an application tunes: whether the town keeps hours at all, and how early its
/// people set off.
#[derive(Clone, Copy, Debug)]
pub struct ScheduleConfig {
    /// Whether the schedule drives destination choice. When false every agent
    /// wanders near the crowd centre, which is what a debug view of the
    /// steering wants.
    pub enabled: bool,
    /// Hours of lead time. An agent starts heading for the *next* activity's
    /// place this long before the activity actually changes, so the shopkeeper
    /// is walking to the shop at ten to eight rather than standing at home.
    pub retarget_margin: f32,
}

impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            retarget_margin: 0.25,
        }
    }
}

impl ScheduleConfig {
    /// The same configuration with finite, usable values.
    #[must_use]
    pub fn sanitised(mut self) -> Self {
        self.retarget_margin = if self.retarget_margin.is_finite() {
            self.retarget_margin.clamp(0.0, 6.0)
        } else {
            0.25
        };
        self
    }
}

/// The smallest gap between two boundaries, in hours.
const MIN_SEGMENT_HOURS: f32 = 0.25;

/// How far a boundary may move from its authored time, in hours.
const JITTER_HOURS: f32 = 0.4;

impl DailySchedule {
    /// A plausible day for `kind`, derived from the world seed.
    ///
    /// The authored days are the ones a player notices: villagers sleep until
    /// half six and work indoors from eight, guards are already patrolling at
    /// dawn and again after dark, merchants open a shop and close it late, and
    /// children and animals wander. The seed only *shifts* the boundaries, so a
    /// guard is always a guard; it never changes the shape of the day.
    #[must_use]
    pub fn for_kind(kind: NpcKind, seed: u64) -> Self {
        let base = base_day(kind);
        let mut rng = RngStream::indexed(seed, "npc/schedule", kind.index() as u64).rng();
        let count = base.len();
        let mut entries: Vec<ScheduleEntry> = Vec::with_capacity(count);
        for (i, &(hour, activity)) in base.iter().enumerate() {
            let start = if i == 0 {
                0.0
            } else {
                let jitter = rng.range_f32(-JITTER_HOURS, JITTER_HOURS);
                // Keep every entry inside the day with room for the ones after
                // it, and keep the list strictly increasing.
                let lower = entries[i - 1].start_hour + MIN_SEGMENT_HOURS;
                let remaining = (count - i) as f32 * MIN_SEGMENT_HOURS;
                let upper = (24.0 - remaining).max(lower);
                (hour + jitter).clamp(lower, upper)
            };
            entries.push(ScheduleEntry::new(start, activity));
        }
        Self { entries }
    }

    /// The activity at a time of day, in hours.
    ///
    /// Hours outside `[0, 24)` wrap, so `25.0` is `01:00`.
    #[must_use]
    pub fn activity_at(&self, hour: f32) -> Activity {
        self.current(hour).0
    }

    /// The activity at `hour` and the hour the next one begins.
    ///
    /// For the last entry of the day the next hour is reported as `24.0`, i.e.
    /// midnight at the end of this day rather than the start of the next.
    #[must_use]
    pub fn current(&self, hour: f32) -> (Activity, f32) {
        let Some(first) = self.entries.first() else {
            return (Activity::Idle, 24.0);
        };
        let last = self.entries.len() - 1;
        let h = wrap_hour(hour);
        let index = match self
            .entries
            .binary_search_by(|entry| entry.start_hour.total_cmp(&h))
        {
            Ok(i) => i,
            Err(0) => {
                // Before the first boundary: the tail of the previous day, but
                // the next boundary is the first entry.
                return (self.entries[last].activity, first.start_hour);
            }
            Err(i) => i - 1,
        };
        let next = if index == last {
            24.0
        } else {
            self.entries[index + 1].start_hour
        };
        (self.entries[index].activity, next)
    }

    /// The hour the current activity ends, i.e. the next boundary.
    #[must_use]
    pub fn next_boundary(&self, hour: f32) -> f32 {
        self.current(hour).1
    }

    /// Checks the day's invariant: boundaries sorted, first at `0.0`, all
    /// inside `[0, 24)`, each segment non-empty.
    ///
    /// # Errors
    /// Returns a short description of the first problem found.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.entries.is_empty() {
            return Err("a schedule needs at least one entry");
        }
        if self.entries[0].start_hour.abs() > 1e-3 {
            return Err("the first entry must start at 0.0");
        }
        for (i, entry) in self.entries.iter().enumerate() {
            if !entry.start_hour.is_finite() {
                return Err("entry start hours must be finite");
            }
            if entry.start_hour < 0.0 || entry.start_hour >= 24.0 {
                return Err("entry start hours must lie in [0, 24)");
            }
            if i > 0 {
                let gap = entry.start_hour - self.entries[i - 1].start_hour;
                if gap <= 0.0 {
                    return Err("entry start hours must strictly increase");
                }
            }
        }
        Ok(())
    }

    /// The day as a readable timetable, one `HH:MM activity` line per boundary.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut out = String::with_capacity(self.entries.len() * 24);
        for (i, entry) in self.entries.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&entry.line());
        }
        out
    }

    /// Total hours the day covers, which is always 24 for a valid schedule.
    #[must_use]
    pub fn total_hours(&self) -> f32 {
        self.entries
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let next = &self.entries[(i + 1) % self.entries.len()];
                entry.duration_until(next)
            })
            .sum()
    }

    /// How many entries the day has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when the day has no boundaries at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Approximate heap footprint in bytes.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.entries.capacity() * core::mem::size_of::<ScheduleEntry>()
            + core::mem::size_of::<Self>()
    }
}

/// The authored, un-jittered day for one kind.
fn base_day(kind: NpcKind) -> Vec<(f32, Activity)> {
    use Activity::*;
    match kind {
        NpcKind::Villager => vec![
            (0.0, Sleep),
            (6.5, Eat),
            (7.0, Commute),
            (8.0, Work),
            (12.0, Eat),
            (13.0, Work),
            (17.5, Wander),
            (18.0, Socialise),
            (21.0, GoHome),
            (22.0, Sleep),
        ],
        NpcKind::Guard => vec![
            (0.0, Sleep),
            (5.0, Eat),
            (5.5, Patrol),
            (12.0, Eat),
            (12.5, Patrol),
            (17.5, Eat),
            (18.5, Socialise),
            (20.0, Patrol),
            (22.0, GoHome),
            (23.0, Sleep),
        ],
        NpcKind::Merchant => vec![
            (0.0, Sleep),
            (7.0, Eat),
            (7.5, Commute),
            (8.5, Shop),
            (12.5, Eat),
            (13.0, Shop),
            (18.0, Eat),
            (18.5, Socialise),
            (21.5, GoHome),
            (22.5, Sleep),
        ],
        NpcKind::Child => vec![
            (0.0, Sleep),
            (7.0, Eat),
            (8.0, Wander),
            (12.0, Eat),
            (12.5, Wander),
            (16.0, Socialise),
            (19.0, Eat),
            (19.5, Wander),
            (21.0, GoHome),
            (21.5, Sleep),
        ],
        NpcKind::Animal => vec![
            (0.0, Sleep),
            (5.5, Wander),
            (11.0, Eat),
            (12.0, Wander),
            (18.0, Eat),
            (19.0, Wander),
            (21.5, Sleep),
        ],
    }
}

/// Wraps an hour into `[0, 24)`, mapping non-finite input to `0`.
#[must_use]
pub fn wrap_hour(hour: f32) -> f32 {
    if !hour.is_finite() {
        return 0.0;
    }
    let h = hour % 24.0;
    if h < 0.0 { h + 24.0 } else { h }
}

/// Formats an hour as `HH:MM`, rounding to the nearest minute.
#[must_use]
pub fn format_hour(hour: f32) -> String {
    let h = wrap_hour(hour);
    let total = (h * 60.0).round() as i32;
    let total = total.rem_euclid(24 * 60);
    format!("{:02}:{:02}", total / 60, total % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_schedules() -> Vec<(NpcKind, DailySchedule)> {
        NpcKind::ALL
            .iter()
            .map(|&kind| (kind, DailySchedule::for_kind(kind, 7)))
            .collect()
    }

    #[test]
    fn every_days_boundaries_are_valid() {
        for (kind, schedule) in all_schedules() {
            assert!(
                schedule.validate().is_ok(),
                "{} day invalid: {:?}",
                kind.name(),
                schedule.validate()
            );
            assert_eq!(schedule.len(), 10.min(schedule.len()).max(schedule.len()));
            assert!(!schedule.is_empty());
        }
    }

    #[test]
    fn every_day_covers_twenty_four_hours_exactly_once() {
        for (kind, schedule) in all_schedules() {
            assert!(
                (schedule.total_hours() - 24.0).abs() < 1e-2,
                "{} covers {} hours",
                kind.name(),
                schedule.total_hours()
            );
            let mut boundaries: Vec<f32> = schedule.entries.iter().map(|e| e.start_hour).collect();
            let sorted = {
                let mut s = boundaries.clone();
                s.sort_by(f32::total_cmp);
                s
            };
            assert_eq!(
                boundaries,
                sorted,
                "{} boundaries out of order",
                kind.name()
            );
            assert_eq!(boundaries[0], 0.0);
            boundaries.dedup();
            assert_eq!(boundaries.len(), schedule.entries.len());
        }
    }

    #[test]
    fn activity_is_correct_on_both_sides_of_every_boundary() {
        for (kind, schedule) in all_schedules() {
            for (i, entry) in schedule.entries.iter().enumerate() {
                let eps = 0.01;
                if entry.start_hour > 0.0 {
                    let before = entry.start_hour - eps;
                    let previous = schedule.entries[i - 1].activity;
                    assert_eq!(
                        schedule.activity_at(before),
                        previous,
                        "{} at {before}",
                        kind.name()
                    );
                }
                let after = entry.start_hour + eps;
                assert_eq!(
                    schedule.activity_at(after),
                    entry.activity,
                    "{} at {after}",
                    kind.name()
                );
            }
        }
    }

    #[test]
    fn hours_wrap_past_midnight() {
        let schedule = DailySchedule::for_kind(NpcKind::Villager, 3);
        let midnight = schedule.activity_at(0.0);
        assert_eq!(schedule.activity_at(24.0), midnight);
        assert_eq!(schedule.activity_at(25.5), schedule.activity_at(1.5));
        assert_eq!(schedule.activity_at(-1.0), schedule.activity_at(23.0));
        assert_eq!(schedule.activity_at(f32::NAN), midnight);
        assert_eq!(schedule.activity_at(f32::INFINITY), midnight);
    }

    #[test]
    fn current_reports_the_next_boundary_and_wraps() {
        let schedule = DailySchedule::for_kind(NpcKind::Villager, 5);
        let first = schedule.entries[0];
        let last = *schedule.entries.last().expect("entries");
        let (activity, next) = schedule.current(first.start_hour);
        assert_eq!(activity, first.activity);
        assert!(next > first.start_hour);
        let (activity, next) = schedule.current(last.start_hour + 0.1);
        assert_eq!(activity, last.activity);
        assert_eq!(next, 24.0);
        assert_eq!(schedule.next_boundary(last.start_hour + 0.1), 24.0);
    }

    #[test]
    fn the_villager_day_reads_like_the_contract() {
        let schedule = DailySchedule::for_kind(NpcKind::Villager, 1);
        // The shape of the day is the authored one, whatever the seed did to
        // the boundaries.
        let activities: Vec<Activity> = schedule
            .entries
            .iter()
            .map(|entry| entry.activity)
            .collect();
        assert_eq!(
            activities,
            vec![
                Activity::Sleep,
                Activity::Eat,
                Activity::Commute,
                Activity::Work,
                Activity::Eat,
                Activity::Work,
                Activity::Wander,
                Activity::Socialise,
                Activity::GoHome,
                Activity::Sleep,
            ]
        );
        // And the hours a player would notice are where they should be, with
        // enough margin that the seed's jitter cannot move them.
        assert_eq!(schedule.activity_at(3.0), Activity::Sleep);
        assert_eq!(schedule.activity_at(6.0), Activity::Sleep);
        assert_eq!(schedule.activity_at(9.0), Activity::Work);
        assert_eq!(schedule.activity_at(14.0), Activity::Work);
        assert_eq!(schedule.activity_at(19.0), Activity::Socialise);
        assert_eq!(schedule.activity_at(23.5), Activity::Sleep);
        assert_eq!(schedule.activity_at(0.5), Activity::Sleep);
    }

    #[test]
    fn guard_and_merchant_days_differ_from_a_villager() {
        let villager = DailySchedule::for_kind(NpcKind::Villager, 9);
        let guard = DailySchedule::for_kind(NpcKind::Guard, 9);
        let merchant = DailySchedule::for_kind(NpcKind::Merchant, 9);
        // A guard is outdoors at dawn; the villager is still asleep in bed.
        assert_eq!(guard.activity_at(6.0), Activity::Patrol);
        assert_eq!(villager.activity_at(6.0), Activity::Sleep);
        // And a guard is back on duty after dark, while the villager is not.
        // Sampling the middle of the guard's own evening patrol keeps the test
        // about the shape of the day rather than about the seed's jitter.
        let evening = guard
            .entries
            .iter()
            .find(|entry| entry.activity == Activity::Patrol && entry.start_hour > 12.0)
            .expect("a guard patrols after dark");
        let dusk = evening.start_hour + 0.1;
        assert_eq!(guard.activity_at(dusk), Activity::Patrol);
        assert_ne!(villager.activity_at(dusk), Activity::Patrol);
        assert_eq!(guard.entries[0].activity, Activity::Sleep);
        // A merchant runs a shop rather than a field, and closes late.
        assert_eq!(merchant.activity_at(10.0), Activity::Shop);
        assert_eq!(merchant.activity_at(14.0), Activity::Shop);
        assert_eq!(merchant.activity_at(19.0), Activity::Socialise);
        assert_ne!(guard.to_text(), villager.to_text());
        assert_ne!(merchant.to_text(), villager.to_text());
    }

    #[test]
    fn different_seeds_give_different_days() {
        for kind in NpcKind::ALL {
            let a = DailySchedule::for_kind(kind, 1);
            let b = DailySchedule::for_kind(kind, 2);
            let c = DailySchedule::for_kind(kind, 1);
            assert_eq!(a.to_text(), c.to_text(), "same seed must reproduce");
            assert_ne!(
                a.to_text(),
                b.to_text(),
                "{} day did not move with the seed",
                kind.name()
            );
            assert!(a.validate().is_ok());
            assert!(b.validate().is_ok());
        }
    }

    #[test]
    fn jittered_days_stay_within_an_hour_of_the_authored_times() {
        for kind in NpcKind::ALL {
            for seed in 0..8u64 {
                let schedule = DailySchedule::for_kind(kind, seed);
                let base = base_day(kind);
                assert_eq!(schedule.entries.len(), base.len());
                for (i, entry) in schedule.entries.iter().enumerate() {
                    if i == 0 {
                        assert_eq!(entry.start_hour, 0.0);
                        continue;
                    }
                    let drift = (entry.start_hour - base[i].0).abs();
                    assert!(
                        drift <= JITTER_HOURS + 1e-3,
                        "{} seed {seed} drifted {drift} at entry {i}",
                        kind.name()
                    );
                }
            }
        }
    }

    #[test]
    fn to_text_is_readable() {
        let schedule = DailySchedule::for_kind(NpcKind::Villager, 4);
        let text = schedule.to_text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), schedule.len());
        assert!(lines[0].starts_with("00:00 sleep"), "{}", lines[0]);
        for line in &lines {
            assert!(line.len() > 6, "short line {line:?}");
            assert!(line.contains(':'));
        }
        assert!(text.contains("work"));
        assert!(text.contains("home"));
    }

    #[test]
    fn duration_until_wraps_the_day() {
        let sleep = ScheduleEntry::new(22.0, Activity::Sleep);
        let breakfast = ScheduleEntry::new(6.5, Activity::Eat);
        assert!((sleep.duration_until(&breakfast) - 8.5).abs() < 1e-5);
        assert!(
            (breakfast.duration_until(&ScheduleEntry::new(7.0, Activity::Commute)) - 0.5).abs()
                < 1e-5
        );
        let broken = ScheduleEntry::new(f32::NAN, Activity::Idle);
        assert!((broken.duration_until(&breakfast) - 6.5).abs() < 1e-5);
    }

    #[test]
    fn places_match_the_activities() {
        assert_eq!(Activity::Sleep.place(), ActivityPlace::Home);
        assert_eq!(Activity::Work.place(), ActivityPlace::Work);
        assert_eq!(Activity::Socialise.place(), ActivityPlace::Plaza);
        assert_eq!(Activity::Shop.place(), ActivityPlace::Market);
        assert_eq!(Activity::Eat.place(), ActivityPlace::Tavern);
        assert_eq!(Activity::Patrol.place(), ActivityPlace::Road);
        assert_eq!(Activity::Commute.place(), ActivityPlace::Road);
        assert_eq!(Activity::Wander.place(), ActivityPlace::Anywhere);
        assert_eq!(Activity::Idle.place(), ActivityPlace::Anywhere);
        assert_eq!(Activity::GoHome.place(), ActivityPlace::Home);
        for activity in Activity::ALL {
            assert!(!activity.name().is_empty());
            assert!(!activity.place().name().is_empty());
        }
        for place in ActivityPlace::ALL {
            assert!(!place.name().is_empty());
        }
    }

    #[test]
    fn validate_rejects_broken_days() {
        let empty = DailySchedule {
            entries: Vec::new(),
        };
        assert!(empty.validate().is_err());
        assert_eq!(empty.activity_at(3.0), Activity::Idle);
        let late = DailySchedule {
            entries: vec![ScheduleEntry::new(1.0, Activity::Idle)],
        };
        assert!(late.validate().is_err());
        let unordered = DailySchedule {
            entries: vec![
                ScheduleEntry::new(0.0, Activity::Sleep),
                ScheduleEntry::new(0.0, Activity::Eat),
            ],
        };
        assert!(unordered.validate().is_err());
        let outside = DailySchedule {
            entries: vec![
                ScheduleEntry::new(0.0, Activity::Sleep),
                ScheduleEntry::new(30.0, Activity::Eat),
            ],
        };
        assert!(outside.validate().is_err());
        let nan = DailySchedule {
            entries: vec![
                ScheduleEntry::new(0.0, Activity::Sleep),
                ScheduleEntry::new(f32::NAN, Activity::Eat),
            ],
        };
        assert!(nan.validate().is_err());
    }

    #[test]
    fn boundary_lookup_before_the_first_entry_uses_the_tail() {
        let schedule = DailySchedule {
            entries: vec![
                ScheduleEntry::new(0.0, Activity::Sleep),
                ScheduleEntry::new(6.0, Activity::Work),
            ],
        };
        assert_eq!(schedule.activity_at(5.9), Activity::Sleep);
        assert_eq!(schedule.activity_at(6.0), Activity::Work);
        // Past the last boundary the day runs on until midnight.
        assert_eq!(schedule.activity_at(23.0), Activity::Work);

        // A day whose first boundary is not midnight wraps through the tail.
        let wrapped = DailySchedule {
            entries: vec![
                ScheduleEntry::new(2.0, Activity::Sleep),
                ScheduleEntry::new(6.0, Activity::Work),
            ],
        };
        assert_eq!(wrapped.activity_at(1.0), Activity::Work);
        assert_eq!(wrapped.current(1.0), (Activity::Work, 2.0));
        assert_eq!(wrapped.activity_at(2.5), Activity::Sleep);
        assert!(schedule.memory_bytes() > 0);
        assert_eq!(ScheduleEntry::new(6.5, Activity::Eat).time_text(), "06:30");
        assert_eq!(format_hour(23.999), "00:00");
        assert_eq!(format_hour(-1.0), "23:00");
    }
}

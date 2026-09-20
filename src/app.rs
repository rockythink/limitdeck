use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    sync::{
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
        Arc,
    },
    thread,
    time::{Duration, SystemTime},
};

use crate::{
    adapter::{AdapterError, AdapterErrorKind, PlanAdapter},
    adapters::{self, DiscoveryEntry},
    domain::{CodingPlan, ModelUsage, PlanIdentity},
    locale::Language,
    model_usage::ModelUsageEvent,
    theme::Theme,
};

const MAX_SNAPSHOT_AGE: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanPhase {
    Loading,
    Refreshing,
    Ready,
    Stale,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanState {
    pub identity: PlanIdentity,
    pub plan: Option<CodingPlan>,
    pub phase: PlanPhase,
    pub error: Option<AdapterError>,
}

impl PlanState {
    fn new(identity: PlanIdentity) -> Self {
        Self {
            identity,
            plan: None,
            phase: PlanPhase::Loading,
            error: None,
        }
    }

    fn start_refresh(&mut self) {
        self.phase = if self.plan.is_some() {
            PlanPhase::Refreshing
        } else {
            PlanPhase::Loading
        };
        self.error = None;
    }

    fn apply(&mut self, result: Result<CodingPlan, AdapterError>) {
        match result {
            Ok(plan)
                if plan.id == self.identity.id && plan.provider_id == self.identity.provider_id =>
            {
                let expired = plan.is_older_than(MAX_SNAPSHOT_AGE);
                self.phase = if expired {
                    PlanPhase::Stale
                } else {
                    PlanPhase::Ready
                };
                self.error = expired.then_some(AdapterError::new(
                    AdapterErrorKind::SnapshotExpired,
                    "LimitDeck cache",
                ));
                self.plan = Some(plan);
            }
            Ok(_) => self.fail(AdapterError::new(
                AdapterErrorKind::ProtocolChanged,
                "LimitDeck adapter",
            )),
            Err(error) => self.fail(error),
        }
    }

    fn fail(&mut self, error: AdapterError) {
        self.phase = if self.plan.is_some() {
            PlanPhase::Stale
        } else {
            PlanPhase::Unavailable
        };
        self.error = Some(error);
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DashboardView {
    #[default]
    Quotas,
    Models,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelTimeRange {
    Hours24,
    Days7,
    #[default]
    Days30,
    All,
}

impl ModelTimeRange {
    pub const fn next(self) -> Self {
        match self {
            Self::Hours24 => Self::Days7,
            Self::Days7 => Self::Days30,
            Self::Days30 => Self::All,
            Self::All => Self::Hours24,
        }
    }

    pub const fn label(self, language: Language) -> &'static str {
        match (self, language) {
            (Self::Hours24, _) => "24h",
            (Self::Days7, _) => "7d",
            (Self::Days30, _) => "30d",
            (Self::All, Language::English) => "All",
            (Self::All, Language::Chinese) => "全部",
        }
    }

    fn includes(self, last_used_at: Option<SystemTime>, now: SystemTime) -> bool {
        let duration = match self {
            Self::Hours24 => Duration::from_secs(24 * 60 * 60),
            Self::Days7 => Duration::from_secs(7 * 24 * 60 * 60),
            Self::Days30 => Duration::from_secs(30 * 24 * 60 * 60),
            Self::All => return true,
        };
        last_used_at.is_none_or(|last| now.duration_since(last).map_or(true, |age| age <= duration))
    }
}
pub struct App {
    plans: Vec<PlanState>,
    selected: usize,
    model_usage: Vec<ModelUsage>,
    selected_model: usize,
    visible_model_indices: Vec<usize>,
    model_time_range: ModelTimeRange,
    view: DashboardView,
    help_open: bool,
    detail_open: bool,
    discovery_open: bool,
    discovery_entries: Option<Vec<DiscoveryEntry>>,
    theme: Theme,
    language: Language,
    secondary_limits_visible: bool,
}

impl App {
    pub fn new(identities: impl IntoIterator<Item = PlanIdentity>) -> Self {
        let mut seen = HashSet::new();
        let plans = identities
            .into_iter()
            .filter(|identity| seen.insert(identity.id.clone()))
            .map(PlanState::new)
            .collect();
        Self {
            plans,
            selected: 0,
            model_usage: Vec::new(),
            selected_model: 0,
            visible_model_indices: Vec::new(),
            model_time_range: ModelTimeRange::default(),
            view: DashboardView::default(),
            help_open: false,
            detail_open: false,
            discovery_open: false,
            discovery_entries: None,
            theme: Theme::default(),
            language: Language::detect(),
            secondary_limits_visible: false,
        }
    }

    pub fn plans(&self) -> &[PlanState] {
        &self.plans
    }
    pub fn dashboard_view(&self) -> DashboardView {
        self.view
    }

    pub fn toggle_dashboard_view(&mut self) {
        self.view = match self.view {
            DashboardView::Quotas => DashboardView::Models,
            DashboardView::Models => DashboardView::Quotas,
        };
        self.detail_open = false;
    }

    pub fn visible_model_usage(&self) -> impl ExactSizeIterator<Item = &ModelUsage> {
        self.visible_model_indices
            .iter()
            .map(|&index| &self.model_usage[index])
    }

    pub fn visible_model_count(&self) -> usize {
        self.visible_model_indices.len()
    }

    pub fn hidden_model_count(&self) -> usize {
        self.model_usage
            .len()
            .saturating_sub(self.visible_model_indices.len())
    }

    pub fn selected_model_usage(&self) -> Option<&ModelUsage> {
        self.visible_model_indices
            .get(self.selected_model)
            .map(|&index| &self.model_usage[index])
    }

    pub fn model_time_range(&self) -> ModelTimeRange {
        self.model_time_range
    }

    pub fn cycle_model_time_range(&mut self) {
        let selected_raw = self.visible_model_indices.get(self.selected_model).copied();
        self.model_time_range = self.model_time_range.next();
        self.rebuild_visible_models(selected_raw);
    }

    pub fn selected_model_index(&self) -> usize {
        self.selected_model
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected_plan(&self) -> Option<&PlanState> {
        self.plans.get(self.selected)
    }

    pub fn is_detail_open(&self) -> bool {
        self.detail_open
    }
    pub fn is_help_open(&self) -> bool {
        self.help_open
    }

    pub fn toggle_help(&mut self) {
        self.help_open = !self.help_open;
    }

    pub fn close_help(&mut self) -> bool {
        let was_open = self.help_open;
        self.help_open = false;
        was_open
    }

    pub fn theme(&self) -> Theme {
        self.theme
    }

    pub fn cycle_theme(&mut self) {
        self.theme = self.theme.next();
    }

    pub fn language(&self) -> Language {
        self.language
    }

    pub fn is_discovery_open(&self) -> bool {
        self.discovery_open
    }

    pub fn open_discovery(&mut self) {
        self.discovery_open = true;
    }

    pub fn close_discovery(&mut self) -> bool {
        let was_open = self.discovery_open;
        self.discovery_open = false;
        was_open
    }

    pub fn discovery_entries(&self) -> Option<&[DiscoveryEntry]> {
        self.discovery_entries.as_deref()
    }

    /// A requested rescan clears previous results so the panel can show that
    /// the scan is running again.
    pub fn retry_discovery(&mut self) {
        self.discovery_entries = None;
    }

    /// Merge a fresh discovery scan: newly found plans join monitoring in
    /// the loading phase, and the panel data is replaced wholesale.
    pub fn apply_discovery(
        &mut self,
        identities: impl IntoIterator<Item = PlanIdentity>,
        entries: Vec<DiscoveryEntry>,
    ) {
        let mut seen = self
            .plans
            .iter()
            .map(|plan| plan.identity.id.clone())
            .collect::<HashSet<_>>();
        for identity in identities {
            if seen.insert(identity.id.clone()) {
                self.plans.push(PlanState::new(identity));
            }
        }
        self.discovery_entries = Some(entries);
    }

    pub fn cycle_language(&mut self) {
        self.language = self.language.next();
    }

    pub fn secondary_limits_visible(&self) -> bool {
        self.secondary_limits_visible
    }

    pub fn toggle_secondary_limits(&mut self) {
        self.secondary_limits_visible = !self.secondary_limits_visible;
    }
    pub(crate) fn apply_preferences(
        &mut self,
        theme: Theme,
        language: Language,
        secondary_limits_visible: bool,
        model_time_range: ModelTimeRange,
    ) {
        self.theme = theme;
        self.language = language;
        self.secondary_limits_visible = secondary_limits_visible;
        self.model_time_range = model_time_range;
        self.rebuild_visible_models(None);
    }

    #[cfg(test)]
    pub(crate) fn set_language(&mut self, language: Language) {
        self.language = language;
    }
    pub fn select_next(&mut self) {
        match self.view {
            DashboardView::Quotas if !self.plans.is_empty() => {
                self.selected = (self.selected + 1) % self.plans.len();
            }
            DashboardView::Models if !self.visible_model_indices.is_empty() => {
                self.selected_model = (self.selected_model + 1) % self.visible_model_indices.len();
            }
            _ => {}
        }
    }

    pub fn select_previous(&mut self) {
        match self.view {
            DashboardView::Quotas if !self.plans.is_empty() => {
                self.selected = (self.selected + self.plans.len() - 1) % self.plans.len();
            }
            DashboardView::Models if !self.visible_model_indices.is_empty() => {
                self.selected_model = (self.selected_model + self.visible_model_indices.len() - 1)
                    % self.visible_model_indices.len();
            }
            _ => {}
        }
    }

    pub fn toggle_detail(&mut self) {
        if self.view == DashboardView::Quotas && !self.plans.is_empty() {
            self.detail_open = !self.detail_open;
        }
    }

    pub fn close_detail(&mut self) -> bool {
        let was_open = self.detail_open;
        self.detail_open = false;
        was_open
    }

    pub fn start_refresh(&mut self) {
        for plan in &mut self.plans {
            plan.start_refresh();
        }
    }

    pub fn apply_event(&mut self, event: PlanEvent) {
        match event {
            PlanEvent::Discovery {
                identities,
                entries,
            } => {
                self.apply_discovery(identities, entries);
            }
            PlanEvent::Fetched { identity, result } => {
                if let Some(plan) = self
                    .plans
                    .iter_mut()
                    .find(|plan| plan.identity.id == identity.id)
                {
                    plan.apply(result);
                }
            }
        }
    }
    pub fn apply_model_usage_event(&mut self, event: ModelUsageEvent) {
        let Ok(snapshot) = event.result else {
            return;
        };
        if snapshot.source_id != event.source_id {
            return;
        }
        self.model_usage
            .retain(|usage| usage.agent_id != event.source_id);
        for usage in snapshot
            .models
            .into_iter()
            .filter(|usage| usage.agent_id == event.source_id)
        {
            if let Some(existing) = self.model_usage.iter_mut().find(|existing| {
                existing.agent_id == usage.agent_id
                    && existing.provider_id == usage.provider_id
                    && existing.model_id == usage.model_id
            }) {
                existing.merge(&usage);
            } else {
                self.model_usage.push(usage);
            }
        }
        self.model_usage.sort_by(|left, right| {
            right
                .total_tokens()
                .cmp(&left.total_tokens())
                .then_with(|| left.agent_name.cmp(&right.agent_name))
                .then_with(|| left.model_id.cmp(&right.model_id))
        });
        self.rebuild_visible_models(None);
    }

    fn rebuild_visible_models(&mut self, selected_raw: Option<usize>) {
        let now = SystemTime::now();
        self.visible_model_indices.clear();
        self.visible_model_indices
            .extend(
                self.model_usage
                    .iter()
                    .enumerate()
                    .filter_map(|(index, usage)| {
                        self.model_time_range
                            .includes(usage.last_used_at, now)
                            .then_some(index)
                    }),
            );
        self.selected_model = selected_raw
            .and_then(|raw| {
                self.visible_model_indices
                    .iter()
                    .position(|&index| index == raw)
            })
            .unwrap_or_else(|| {
                self.selected_model
                    .min(self.visible_model_indices.len().saturating_sub(1))
            });
    }
    pub fn mark_worker_disconnected(&mut self) {
        for plan in &mut self.plans {
            if matches!(plan.phase, PlanPhase::Loading | PlanPhase::Refreshing) {
                plan.apply(Err(AdapterError::new(
                    AdapterErrorKind::ProtocolChanged,
                    "LimitDeck worker",
                )));
            }
        }
    }
}

#[derive(Clone, Copy)]
enum WorkerCommand {
    Refresh,
    Discover,
}

pub enum PlanEvent {
    Fetched {
        identity: PlanIdentity,
        result: Result<CodingPlan, AdapterError>,
    },
    Discovery {
        identities: Vec<PlanIdentity>,
        entries: Vec<DiscoveryEntry>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkerStopped;

pub struct PlanWorker {
    commands: SyncSender<WorkerCommand>,
    events: Receiver<PlanEvent>,
}

impl PlanWorker {
    pub fn spawn(adapters: Vec<Box<dyn PlanAdapter>>) -> Self {
        let mut adapters: Vec<Arc<dyn PlanAdapter>> = adapters.into_iter().map(Arc::from).collect();
        let (command_tx, command_rx) = mpsc::sync_channel(1);
        let (event_tx, event_rx) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(command) = command_rx.recv() {
                match command {
                    WorkerCommand::Discover => {
                        let (discovered, report) = adapters::discover_with_report();
                        let identities = discovered
                            .iter()
                            .map(|adapter| adapter.identity())
                            .collect::<Vec<_>>();
                        adapters = discovered.into_iter().map(Arc::from).collect();
                        let _ = event_tx.send(PlanEvent::Discovery {
                            identities,
                            entries: report.entries,
                        });
                        fetch_all(&adapters, &event_tx);
                    }
                    WorkerCommand::Refresh => {
                        fetch_all(&adapters, &event_tx);
                    }
                }
            }
        });
        Self {
            commands: command_tx,
            events: event_rx,
        }
    }

    pub fn request_refresh(&self) -> Result<bool, WorkerStopped> {
        self.request(WorkerCommand::Refresh)
    }

    pub fn request_discover(&self) -> Result<bool, WorkerStopped> {
        self.request(WorkerCommand::Discover)
    }

    fn request(&self, command: WorkerCommand) -> Result<bool, WorkerStopped> {
        match self.commands.try_send(command) {
            Ok(()) => Ok(true),
            Err(TrySendError::Full(_)) => Ok(false),
            Err(TrySendError::Disconnected(_)) => Err(WorkerStopped),
        }
    }

    pub fn try_recv(&self) -> Result<Option<PlanEvent>, WorkerStopped> {
        match self.events.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(WorkerStopped),
        }
    }
}

fn fetch_all(adapters: &[Arc<dyn PlanAdapter>], events: &mpsc::Sender<PlanEvent>) {
    thread::scope(|scope| {
        for adapter in adapters {
            let events = events.clone();
            scope.spawn(move || {
                let identity = adapter.identity();
                let result = adapter.fetch();
                let _ = events.send(PlanEvent::Fetched { identity, result });
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_merges_new_plans_without_duplicating_existing_ones() {
        let mut app = App::new([PlanIdentity::new("claude", "anthropic", "Claude")]);

        app.apply_discovery(
            [
                PlanIdentity::new("claude", "anthropic", "Claude"),
                PlanIdentity::new("kimi-code", "moonshot", "Kimi"),
            ],
            vec![DiscoveryEntry {
                label: "Kimi".to_owned(),
                provider_id: "moonshot".to_owned(),
                source: "OMP account",
                monitored: true,
                note: None,
            }],
        );

        assert_eq!(app.plans().len(), 2);
        assert_eq!(app.plans()[1].identity.id, "kimi-code");
        assert_eq!(app.plans()[1].phase, PlanPhase::Loading);
        assert_eq!(
            app.discovery_entries().map(<[DiscoveryEntry]>::len),
            Some(1)
        );

        app.apply_discovery(
            [PlanIdentity::new("kimi-code", "moonshot", "Kimi")],
            Vec::new(),
        );
        assert_eq!(app.plans().len(), 2);
        assert!(app
            .discovery_entries()
            .is_some_and(<[DiscoveryEntry]>::is_empty));
    }

    use crate::domain::{UsageStatus, UsageWindow};
    use std::time::SystemTime;
    struct TestAdapter {
        identity: PlanIdentity,
        delay: Duration,
        result: Result<CodingPlan, AdapterError>,
    }

    impl PlanAdapter for TestAdapter {
        fn identity(&self) -> PlanIdentity {
            self.identity.clone()
        }

        fn fetch(&self) -> Result<CodingPlan, AdapterError> {
            thread::sleep(self.delay);
            self.result.clone()
        }
    }

    fn plan(id: &str, provider_id: &str, fetched_at: SystemTime) -> CodingPlan {
        CodingPlan {
            id: id.to_owned(),
            provider_id: provider_id.to_owned(),
            display_name: id.to_owned(),
            fetched_at,
            windows: vec![UsageWindow {
                id: format!("{id}:primary"),
                label: "Primary".to_owned(),
                period: Some(Duration::from_secs(5 * 60 * 60)),
                remaining_percent: 70,
                resets_at: None,
                status: UsageStatus::Available,
            }],
        }
    }

    #[test]
    fn plan_results_are_independent() {
        let first = PlanIdentity::new("first", "openai", "First");
        let second = PlanIdentity::new("second", "anthropic", "Second");
        let mut app = App::new([first.clone(), second.clone()]);

        app.apply_event(PlanEvent::Fetched {
            identity: first,
            result: Ok(plan("first", "openai", SystemTime::now())),
        });
        app.apply_event(PlanEvent::Fetched {
            identity: second,
            result: Err(AdapterError::new(
                AdapterErrorKind::TimedOut,
                "test adapter",
            )),
        });

        assert_eq!(app.plans()[0].phase, PlanPhase::Ready);
        assert_eq!(app.plans()[0].error, None);
        assert_eq!(app.plans()[1].phase, PlanPhase::Unavailable);
        assert_eq!(
            app.plans()[1].error,
            Some(AdapterError::new(
                AdapterErrorKind::TimedOut,
                "test adapter",
            ))
        );
    }

    #[test]
    fn failed_refresh_preserves_the_last_snapshot_as_stale() {
        let identity = PlanIdentity::new("first", "openai", "First");
        let mut app = App::new([identity.clone()]);
        app.apply_event(PlanEvent::Fetched {
            identity: identity.clone(),
            result: Ok(plan("first", "openai", SystemTime::now())),
        });
        app.start_refresh();
        app.apply_event(PlanEvent::Fetched {
            identity,
            result: Err(AdapterError::new(
                AdapterErrorKind::TimedOut,
                "test adapter",
            )),
        });

        assert_eq!(app.plans()[0].phase, PlanPhase::Stale);
        assert!(app.plans()[0].plan.is_some());
        assert_eq!(
            app.plans()[0].error,
            Some(AdapterError::new(
                AdapterErrorKind::TimedOut,
                "test adapter",
            ))
        );
    }

    #[test]
    fn old_cached_snapshot_is_marked_stale() {
        let identity = PlanIdentity::new("first", "openai", "First");
        let mut app = App::new([identity.clone()]);
        app.apply_event(PlanEvent::Fetched {
            identity,
            result: Ok(plan(
                "first",
                "openai",
                SystemTime::now() - Duration::from_secs(16 * 60),
            )),
        });

        assert_eq!(app.plans()[0].phase, PlanPhase::Stale);
        assert_eq!(
            app.plans()[0].error,
            Some(AdapterError::new(
                AdapterErrorKind::SnapshotExpired,
                "LimitDeck cache",
            ))
        );
    }

    #[test]
    fn refresh_clears_old_diagnostic_and_fresh_success_stays_clear() {
        let identity = PlanIdentity::new("first", "openai", "First");
        let mut app = App::new([identity.clone()]);
        app.apply_event(PlanEvent::Fetched {
            identity: identity.clone(),
            result: Err(AdapterError::new(
                AdapterErrorKind::NotAuthenticated,
                "test adapter",
            )),
        });
        assert_eq!(app.plans()[0].phase, PlanPhase::Unavailable);
        assert!(app.plans()[0].error.is_some());

        app.start_refresh();
        assert_eq!(app.plans()[0].phase, PlanPhase::Loading);
        assert_eq!(app.plans()[0].error, None);

        app.apply_event(PlanEvent::Fetched {
            identity,
            result: Ok(plan("first", "openai", SystemTime::now())),
        });
        assert_eq!(app.plans()[0].phase, PlanPhase::Ready);
        assert_eq!(app.plans()[0].error, None);
    }

    #[test]
    fn selection_wraps_and_detail_requires_a_plan() {
        let mut app = App::new([
            PlanIdentity::new("first", "openai", "First"),
            PlanIdentity::new("second", "anthropic", "Second"),
        ]);
        app.select_previous();
        assert_eq!(app.selected_index(), 1);
        app.select_next();
        assert_eq!(app.selected_index(), 0);
        app.toggle_detail();
        assert!(app.is_detail_open());
        assert!(app.close_detail());
        assert!(!app.close_detail());

        let mut empty = App::new(std::iter::empty::<PlanIdentity>());
        empty.toggle_detail();
        assert!(!empty.is_detail_open());
    }
    #[test]
    fn theme_cycle_starts_at_rainbow_and_wraps() {
        let mut app = App::new(std::iter::empty::<PlanIdentity>());
        assert_eq!(app.theme(), Theme::Rainbow);
        app.cycle_theme();
        assert_eq!(app.theme(), Theme::Midnight);
        app.cycle_theme();
        assert_eq!(app.theme(), Theme::Mono);
        app.cycle_theme();
        assert_eq!(app.theme(), Theme::Rainbow);
    }

    #[test]
    fn secondary_limits_are_hidden_by_default_and_toggle_for_the_session() {
        let mut app = App::new(std::iter::empty::<PlanIdentity>());

        assert!(!app.secondary_limits_visible());
        app.toggle_secondary_limits();
        assert!(app.secondary_limits_visible());
        app.toggle_secondary_limits();
        assert!(!app.secondary_limits_visible());
    }

    #[test]
    fn language_cycle_switches_and_wraps() {
        let mut app = App::new(std::iter::empty::<PlanIdentity>());
        app.set_language(Language::English);
        assert_eq!(app.language(), Language::English);
        app.cycle_language();
        assert_eq!(app.language(), Language::Chinese);
        app.cycle_language();
        assert_eq!(app.language(), Language::English);
    }
    #[test]
    fn worker_fetches_adapters_in_parallel() {
        let delay = Duration::from_millis(80);
        let first_identity = PlanIdentity::new("first", "openai", "First");
        let second_identity = PlanIdentity::new("second", "anthropic", "Second");
        let worker = PlanWorker::spawn(vec![
            Box::new(TestAdapter {
                identity: first_identity,
                delay,
                result: Ok(plan("first", "openai", SystemTime::now())),
            }),
            Box::new(TestAdapter {
                identity: second_identity,
                delay,
                result: Ok(plan("second", "anthropic", SystemTime::now())),
            }),
        ]);
        let started = std::time::Instant::now();
        assert_eq!(worker.request_refresh(), Ok(true));

        let first = worker.events.recv_timeout(Duration::from_secs(1));
        let second = worker.events.recv_timeout(Duration::from_secs(1));

        assert!(first.is_ok() && second.is_ok());
        assert!(started.elapsed() < Duration::from_millis(150));
    }
    #[test]
    fn model_snapshots_replace_each_source_and_keep_agent_attribution() {
        fn usage(agent: &str, model: &str, tokens: u64) -> ModelUsage {
            ModelUsage {
                agent_id: agent.to_owned(),
                agent_name: agent.to_owned(),
                provider_id: "openai".to_owned(),
                model_id: model.to_owned(),
                requests: 1,
                failed_requests: 0,
                input_tokens: tokens,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: None,
                first_used_at: None,
                last_used_at: None,
            }
        }

        let mut app = App::new(std::iter::empty::<PlanIdentity>());
        app.apply_model_usage_event(ModelUsageEvent {
            source_id: "omp",
            result: Ok(crate::domain::ModelUsageSnapshot {
                source_id: "omp".to_owned(),
                fetched_at: SystemTime::now(),
                models: vec![usage("omp", "gpt", 10), usage("omp", "gpt", 20)],
            }),
        });
        assert_eq!(app.visible_model_count(), 1);
        assert_eq!(app.selected_model_usage().unwrap().input_tokens, 30);

        app.apply_model_usage_event(ModelUsageEvent {
            source_id: "omp",
            result: Ok(crate::domain::ModelUsageSnapshot {
                source_id: "omp".to_owned(),
                fetched_at: SystemTime::now(),
                models: vec![usage("omp", "gpt", 7)],
            }),
        });
        assert_eq!(app.selected_model_usage().unwrap().input_tokens, 7);
        app.toggle_dashboard_view();
        assert_eq!(app.dashboard_view(), DashboardView::Models);
        app.toggle_detail();
        assert!(!app.is_detail_open());
    }
    #[test]
    fn model_time_ranges_hide_old_usage_without_deleting_it() {
        let now = SystemTime::now();
        let usage = |model: &str, tokens: u64, age: Duration| ModelUsage {
            agent_id: "pi".to_owned(),
            agent_name: "Pi".to_owned(),
            provider_id: "openai".to_owned(),
            model_id: model.to_owned(),
            requests: 1,
            failed_requests: 0,
            input_tokens: tokens,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: None,
            first_used_at: Some(now - age),
            last_used_at: Some(now - age),
        };
        let mut app = App::new([]);
        app.apply_model_usage_event(ModelUsageEvent {
            source_id: "pi",
            result: Ok(crate::domain::ModelUsageSnapshot {
                source_id: "pi".to_owned(),
                fetched_at: now,
                models: vec![
                    usage("recent", 3, Duration::from_secs(60 * 60)),
                    usage("week", 2, Duration::from_secs(3 * 24 * 60 * 60)),
                    usage("old", 1, Duration::from_secs(60 * 24 * 60 * 60)),
                ],
            }),
        });

        assert_eq!(app.model_time_range(), ModelTimeRange::Days30);
        assert_eq!(app.visible_model_count(), 2);
        assert_eq!(app.hidden_model_count(), 1);

        app.cycle_model_time_range();
        assert_eq!(app.model_time_range(), ModelTimeRange::All);
        assert_eq!(app.visible_model_count(), 3);
        app.cycle_model_time_range();
        assert_eq!(app.model_time_range(), ModelTimeRange::Hours24);
        assert_eq!(app.visible_model_count(), 1);
        assert_eq!(app.hidden_model_count(), 2);
        assert_eq!(app.selected_model_usage().unwrap().model_id, "recent");
    }
}

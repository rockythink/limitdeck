use std::{
    collections::HashSet,
    sync::{
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
        Arc,
    },
    thread,
    time::Duration,
};

use crate::{
    adapter::{AdapterError, AdapterErrorKind, PlanAdapter},
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
pub struct App {
    plans: Vec<PlanState>,
    selected: usize,
    model_usage: Vec<ModelUsage>,
    selected_model: usize,
    view: DashboardView,
    detail_open: bool,
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
            view: DashboardView::default(),
            detail_open: false,
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

    pub fn model_usage(&self) -> &[ModelUsage] {
        &self.model_usage
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

    pub fn theme(&self) -> Theme {
        self.theme
    }

    pub fn cycle_theme(&mut self) {
        self.theme = self.theme.next();
    }

    pub fn language(&self) -> Language {
        self.language
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

    #[cfg(test)]
    pub(crate) fn set_language(&mut self, language: Language) {
        self.language = language;
    }
    pub fn select_next(&mut self) {
        match self.view {
            DashboardView::Quotas if !self.plans.is_empty() => {
                self.selected = (self.selected + 1) % self.plans.len();
            }
            DashboardView::Models if !self.model_usage.is_empty() => {
                self.selected_model = (self.selected_model + 1) % self.model_usage.len();
            }
            _ => {}
        }
    }

    pub fn select_previous(&mut self) {
        match self.view {
            DashboardView::Quotas if !self.plans.is_empty() => {
                self.selected = (self.selected + self.plans.len() - 1) % self.plans.len();
            }
            DashboardView::Models if !self.model_usage.is_empty() => {
                self.selected_model =
                    (self.selected_model + self.model_usage.len() - 1) % self.model_usage.len();
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
        let PlanEvent::Fetched { identity, result } = event;
        if let Some(plan) = self
            .plans
            .iter_mut()
            .find(|plan| plan.identity.id == identity.id)
        {
            plan.apply(result);
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
        self.selected_model = self
            .selected_model
            .min(self.model_usage.len().saturating_sub(1));
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
}

pub enum PlanEvent {
    Fetched {
        identity: PlanIdentity,
        result: Result<CodingPlan, AdapterError>,
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
        let adapters: Vec<Arc<dyn PlanAdapter>> = adapters.into_iter().map(Arc::from).collect();
        let (command_tx, command_rx) = mpsc::sync_channel(1);
        let (event_tx, event_rx) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(WorkerCommand::Refresh) = command_rx.recv() {
                thread::scope(|scope| {
                    for adapter in &adapters {
                        let events = event_tx.clone();
                        scope.spawn(move || {
                            let identity = adapter.identity();
                            let result = adapter.fetch();
                            let _ = events.send(PlanEvent::Fetched { identity, result });
                        });
                    }
                });
            }
        });
        Self {
            commands: command_tx,
            events: event_rx,
        }
    }

    pub fn request_refresh(&self) -> Result<bool, WorkerStopped> {
        match self.commands.try_send(WorkerCommand::Refresh) {
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

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(app.model_usage().len(), 1);
        assert_eq!(app.model_usage()[0].input_tokens, 30);

        app.apply_model_usage_event(ModelUsageEvent {
            source_id: "omp",
            result: Ok(crate::domain::ModelUsageSnapshot {
                source_id: "omp".to_owned(),
                fetched_at: SystemTime::now(),
                models: vec![usage("omp", "gpt", 7)],
            }),
        });
        assert_eq!(app.model_usage()[0].input_tokens, 7);
        app.toggle_dashboard_view();
        assert_eq!(app.dashboard_view(), DashboardView::Models);
        app.toggle_detail();
        assert!(!app.is_detail_open());
    }
}

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
    domain::{CodingPlan, PlanIdentity},
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

pub struct App {
    plans: Vec<PlanState>,
    selected: usize,
    detail_open: bool,
    worker_disconnected: bool,
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
            detail_open: false,
            worker_disconnected: false,
        }
    }

    pub fn plans(&self) -> &[PlanState] {
        &self.plans
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

    pub fn select_next(&mut self) {
        if !self.plans.is_empty() {
            self.selected = (self.selected + 1) % self.plans.len();
        }
    }

    pub fn select_previous(&mut self) {
        if !self.plans.is_empty() {
            self.selected = (self.selected + self.plans.len() - 1) % self.plans.len();
        }
    }

    pub fn toggle_detail(&mut self) {
        if !self.plans.is_empty() {
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

    pub fn mark_worker_disconnected(&mut self) {
        self.worker_disconnected = true;
        for plan in &mut self.plans {
            if matches!(plan.phase, PlanPhase::Loading | PlanPhase::Refreshing) {
                plan.apply(Err(AdapterError::new(
                    AdapterErrorKind::ProtocolChanged,
                    "LimitDeck worker",
                )));
            }
        }
    }

    pub fn worker_disconnected(&self) -> bool {
        self.worker_disconnected
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
}

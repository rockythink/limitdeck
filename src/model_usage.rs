use std::{
    sync::{
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
        Arc,
    },
    thread,
};

use crate::{
    adapter::{AdapterError, ModelUsageAdapter},
    domain::ModelUsageSnapshot,
};

#[derive(Clone, Copy)]
enum WorkerCommand {
    Refresh,
}

pub struct ModelUsageEvent {
    pub source_id: &'static str,
    pub result: Result<ModelUsageSnapshot, AdapterError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkerStopped;

pub struct ModelUsageWorker {
    commands: SyncSender<WorkerCommand>,
    events: Receiver<ModelUsageEvent>,
}

impl ModelUsageWorker {
    pub fn spawn(adapters: Vec<Box<dyn ModelUsageAdapter>>) -> Self {
        let adapters: Vec<Arc<dyn ModelUsageAdapter>> =
            adapters.into_iter().map(Arc::from).collect();
        let (command_tx, command_rx) = mpsc::sync_channel(1);
        let (event_tx, event_rx) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(WorkerCommand::Refresh) = command_rx.recv() {
                thread::scope(|scope| {
                    for adapter in &adapters {
                        let events = event_tx.clone();
                        scope.spawn(move || {
                            let source_id = adapter.source_id();
                            let result = adapter.fetch();
                            let _ = events.send(ModelUsageEvent { source_id, result });
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

    pub fn try_recv(&self) -> Result<Option<ModelUsageEvent>, WorkerStopped> {
        match self.events.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(WorkerStopped),
        }
    }
}

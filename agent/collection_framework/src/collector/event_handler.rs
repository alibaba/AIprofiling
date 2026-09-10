use crate::collector::collector_name::CollectorName;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{mpsc, Mutex, OnceLock};

/*
* 1. Instantly triggered collectors:
*   1.1. pyki collector, instantly triggered: init->StartCollector->StopAllCollector(pid)->WritingFinish(pid)
*   1.2. When the pyki collector is not enabled, other collectors are also instantly triggered: init->StartCollector->StopAllCollector->WritingFinish(pid)
* 2. Signal-triggered collectors: init->StartPendingCollector(pid)->StopCollector(pid)->WritingFinish(pid)
*   2.1. When the pyki collector is also enabled, other collectors wait for the pyki collector's signal to start and stop
*/
#[derive(Debug, Clone)]
pub enum SchedulerEvent {
    StartCollector(CollectorName, i32),
    StopTimedCollector(i32),

    StartPendingCollector(CollectorName, i32),
    StopCollector(CollectorName, i32),
    CollectFailed(CollectorName, i32),
    WritingFinish(CollectorName, i32),

    Exit,
}

// Global event channel
static EVENT_SENDER: OnceLock<Sender<SchedulerEvent>> = OnceLock::new();
static EVENT_RECEIVER: OnceLock<Mutex<Receiver<SchedulerEvent>>> = OnceLock::new();

pub struct EventHandler;

impl EventHandler {
    pub fn global_sender() -> &'static Sender<SchedulerEvent> {
        EVENT_SENDER.get_or_init(|| {
            let (sender, receiver) = mpsc::channel();
            EVENT_RECEIVER.set(Mutex::new(receiver)).ok();
            sender
        })
    }

    pub fn sender() -> Sender<SchedulerEvent> {
        Self::global_sender().clone()
    }

    pub fn receiver() -> &'static Mutex<Receiver<SchedulerEvent>> {
        EVENT_RECEIVER
            .get()
            .expect("Event receiver not initialized")
    }
}

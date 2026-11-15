//! In-memory operation ownership mirrors the durable store fence. A late
//! completion can never release a newer operation's busy state.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tandem_core::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActiveOperation {
    pub id: String,
    pub session_id: String,
    pub kind: SessionOperationKind,
    pub phase: OperationPhase,
    pub started_at_ms: i64,
    pub cancel_requested: bool,
}
#[derive(Clone, Default)]
pub struct OperationSlot(Arc<Mutex<Option<OperationState>>>);
type OperationState = (ActiveOperation, Arc<AtomicBool>);
impl OperationSlot {
    pub fn active(&self) -> Option<ActiveOperation> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|(op, _)| op.clone())
    }
    pub(crate) fn begin(&self, op: ActiveOperation, cancel: Arc<AtomicBool>) -> Result<()> {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        ensure!(slot.is_none(), "Another AI operation is active");
        *slot = Some((op, cancel));
        Ok(())
    }
    /// UI cancellation is operation-scoped, including broker/public subturns.
    pub fn cancel(&self, id: &str) -> bool {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((op, cancel)) = slot.as_mut().filter(|(op, _)| op.id == id) {
            cancel.store(true, Ordering::Release);
            op.cancel_requested = true;
            op.phase = OperationPhase::Stopping;
            true
        } else {
            false
        }
    }
    pub(crate) fn phase(&self, id: &str, phase: OperationPhase) -> Result<ActiveOperation> {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let (op, cancel) = slot
            .as_mut()
            .filter(|(op, _)| op.id == id)
            .ok_or_else(|| anyhow::anyhow!("Operation was superseded"))?;
        op.cancel_requested = cancel.load(Ordering::Acquire);
        op.phase = if op.cancel_requested {
            OperationPhase::Stopping
        } else {
            phase
        };
        Ok(op.clone())
    }
    /// Cancellation and durable terminal publication share this operation-ID
    /// lock. A cancellation accepted first wins; cancellation after a committed
    /// terminal result cannot retroactively revoke that result.
    pub(crate) fn settle<T>(&self, id: &str, publish: impl FnOnce(bool) -> Result<T>) -> Result<T> {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let (op, cancel) = slot
            .as_ref()
            .filter(|(op, _)| op.id == id)
            .ok_or_else(|| anyhow::anyhow!("Operation was superseded"))?;
        let cancelled = op.cancel_requested || cancel.load(Ordering::Acquire);
        let result = publish(cancelled);
        *slot = None;
        result
    }
    #[cfg(test)]
    pub(crate) fn finish(&self, id: &str) -> bool {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if slot.as_ref().is_some_and(|(op, _)| op.id == id) {
            *slot = None;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_cleanup_and_cancel_cannot_change_a_new_operation() {
        let slot = OperationSlot::default();
        let operation = |id: &str| ActiveOperation {
            id: id.into(),
            session_id: "session".into(),
            kind: SessionOperationKind::Discuss,
            phase: OperationPhase::Discussing,
            started_at_ms: 0,
            cancel_requested: false,
        };
        slot.begin(operation("old"), Arc::new(AtomicBool::new(false)))
            .unwrap();
        assert!(slot.finish("old"));
        slot.begin(operation("new"), Arc::new(AtomicBool::new(false)))
            .unwrap();
        assert!(!slot.finish("old"));
        assert!(!slot.cancel("old"));
        assert!(slot.phase("old", OperationPhase::Stopping).is_err());
        assert_eq!(slot.active().unwrap().id, "new");
        assert!(!slot.active().unwrap().cancel_requested);
        assert!(
            slot.settle("old", |_| -> Result<()> {
                panic!("stale operation must never publish")
            })
            .is_err()
        );
        assert_eq!(slot.active().unwrap().id, "new");
    }

    #[test]
    fn accepted_cancel_wins_and_terminal_commit_precedes_late_cancel() {
        let slot = OperationSlot::default();
        let operation = |id: &str| ActiveOperation {
            id: id.into(),
            session_id: "session".into(),
            kind: SessionOperationKind::DraftSpec,
            phase: OperationPhase::DraftingSpec,
            started_at_ms: 0,
            cancel_requested: false,
        };
        slot.begin(operation("cancelled"), Arc::new(AtomicBool::new(false)))
            .unwrap();
        assert!(slot.cancel("cancelled"));
        slot.settle("cancelled", |cancelled| {
            assert!(cancelled);
            Ok(())
        })
        .unwrap();

        slot.begin(operation("completed"), Arc::new(AtomicBool::new(false)))
            .unwrap();
        let late = slot.clone();
        let cancel_thread = slot
            .settle("completed", |cancelled| {
                assert!(!cancelled);
                let (started, ready) = std::sync::mpsc::channel();
                let thread = std::thread::spawn(move || {
                    started.send(()).unwrap();
                    late.cancel("completed")
                });
                ready.recv().unwrap();
                // The cancellation cannot be accepted while terminal publication
                // owns this operation. Releasing the lock is the commit boundary.
                Ok(thread)
            })
            .unwrap();
        assert!(!cancel_thread.join().unwrap());
        assert!(slot.active().is_none());
    }
}

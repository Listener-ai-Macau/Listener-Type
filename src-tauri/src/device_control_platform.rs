//! Listener's thin BLE adapter for the shared device-control contract.
//!
//! Existing recovery evidence decides whether Type may attempt PairAsync. This
//! module does not discover devices or call Windows APIs; it makes that one
//! authorized attempt an identifiable, bounded, terminal transaction.

use denzic_device_control_v1_core::{
    DeviceControlCore, ErrorCategory, LifecycleState, OperationDecision, OperationKind,
    OperationRequest, OperationResult, OwnershipState, Transport,
};
use std::sync::atomic::{AtomicU64, Ordering};

const LISTENER_BACKGROUND_PAIRING_TARGET: u32 = 1;
const LISTENER_BACKGROUND_PAIRING_TIMEOUT_MS: u32 = 6_000;
static NEXT_BACKGROUND_PAIRING_OPERATION_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) struct BackgroundPairingRecovery {
    core: DeviceControlCore,
    request: OperationRequest,
    decision: OperationDecision,
}

impl BackgroundPairingRecovery {
    pub(crate) fn begin(
        type_controlled_recovery: bool,
        manual_unpair_hold: bool,
        automatic_cleanup_allowed: bool,
    ) -> Self {
        let mut core = DeviceControlCore::new(Transport::Ble);
        core.set_lifecycle(LifecycleState::Recovering);
        if manual_unpair_hold {
            core.set_ownership(OwnershipState::ManualUnpaired);
        } else if type_controlled_recovery {
            core.set_ownership(OwnershipState::Local);
        }
        let operation_id = NEXT_BACKGROUND_PAIRING_OPERATION_ID.fetch_add(1, Ordering::Relaxed);
        let operation_id = if operation_id == 0 {
            NEXT_BACKGROUND_PAIRING_OPERATION_ID.fetch_add(1, Ordering::Relaxed)
        } else {
            operation_id
        };
        let request = OperationRequest {
            // The coordinator guard permits one recovery at a time, and this
            // process-wide sequence keeps each guarded attempt traceable.
            operation_id,
            idempotency_key: operation_id,
            target_id: LISTENER_BACKGROUND_PAIRING_TARGET,
            expected_settings_revision: 0,
            timeout_ms: LISTENER_BACKGROUND_PAIRING_TIMEOUT_MS,
            kind: OperationKind::Connect,
            automatic: true,
            recovery_authorized: automatic_cleanup_allowed,
        };
        let decision = core.begin(request, 0);
        Self {
            core,
            request,
            decision,
        }
    }

    pub(crate) fn may_execute(&self) -> bool {
        self.decision.execute
    }

    pub(crate) fn decision(&self) -> OperationDecision {
        self.decision
    }

    pub(crate) fn complete_pairing(&mut self) -> OperationDecision {
        self.core.complete(
            self.request.operation_id,
            self.request.idempotency_key,
            OperationResult::Succeeded,
            ErrorCategory::None,
        )
    }

    pub(crate) fn fail_without_reclaim(&mut self, error: ErrorCategory) -> OperationDecision {
        self.core.complete(
            self.request.operation_id,
            self.request.idempotency_key,
            OperationResult::Failed,
            error,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorized_type_owned_recovery_executes_once_and_reaches_terminal_success() {
        let mut transaction = BackgroundPairingRecovery::begin(true, false, true);
        assert!(transaction.may_execute());
        assert_eq!(
            transaction.complete_pairing().result,
            OperationResult::Succeeded
        );
    }

    #[test]
    fn manual_unpair_and_unproven_recovery_cannot_run_pairasync() {
        let manual = BackgroundPairingRecovery::begin(true, true, true);
        assert!(!manual.may_execute());
        assert_eq!(manual.decision().error, ErrorCategory::Ownership);

        let unproven = BackgroundPairingRecovery::begin(false, false, false);
        assert!(!unproven.may_execute());
        assert_eq!(unproven.decision().error, ErrorCategory::Ownership);
    }

    #[test]
    fn failed_pairing_is_terminal_and_not_a_reclaim_loop() {
        let mut transaction = BackgroundPairingRecovery::begin(true, false, true);
        let terminal = transaction.fail_without_reclaim(ErrorCategory::Ownership);
        assert_eq!(terminal.result, OperationResult::Failed);
        assert_eq!(terminal.error, ErrorCategory::Ownership);
        assert!(!terminal.execute);
    }
}

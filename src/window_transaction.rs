//! Pure coordination logic for recoverable multi-window operations.
//!
//! This module deliberately has no dependency on platform window APIs.
//! Callers provide preflight, apply, and rollback operations.

#[derive(Debug, PartialEq)]
pub struct RollbackFailure<R> {
    pub window_id: u32,
    pub source: R,
}

#[derive(Debug, PartialEq)]
pub enum MultiWindowTransactionError<E, R> {
    InvalidWindowId {
        index: usize,
    },
    DuplicateWindowId {
        window_id: u32,
    },
    PreflightFailed {
        window_id: u32,
        source: E,
    },
    ApplyFailed {
        window_id: u32,
        source: E,
        rollback_failures: Vec<RollbackFailure<R>>,
    },
}

/// Execute a recoverable multi-window operation.
///
/// All IDs are validated and all preflight callbacks complete before the
/// first apply callback is invoked.
///
/// If an apply callback fails, rollback is attempted for the failing target
/// first, followed by previously applied targets in reverse order. Rollback
/// continues after individual failures so that every eligible target gets
/// a recovery attempt.
///
/// This coordinates recovery attempts; it does not provide OS-level
/// atomicity. Preflight is a batch check, so apply callbacks must revalidate
/// identity immediately before each mutation. Rollback callbacks must
/// independently revalidate identity before restoring platform state.
pub fn execute_multi_window_transaction<T, E, R, Id, Preflight, Apply, Rollback>(
    targets: &[T],
    mut window_id: Id,
    mut preflight: Preflight,
    mut apply: Apply,
    mut rollback: Rollback,
) -> Result<(), MultiWindowTransactionError<E, R>>
where
    Id: FnMut(&T) -> u32,
    Preflight: FnMut(&T) -> Result<(), E>,
    Apply: FnMut(&T) -> Result<(), E>,
    Rollback: FnMut(&T) -> Result<(), R>,
{
    let mut ids = Vec::with_capacity(targets.len());

    for (index, target) in targets.iter().enumerate() {
        let id = window_id(target);

        if id == 0 {
            return Err(MultiWindowTransactionError::InvalidWindowId { index });
        }

        if ids.contains(&id) {
            return Err(MultiWindowTransactionError::DuplicateWindowId { window_id: id });
        }

        ids.push(id);
    }

    // Complete every preflight before allowing any mutation.
    for (target, id) in targets.iter().zip(ids.iter().copied()) {
        preflight(target).map_err(|source| MultiWindowTransactionError::PreflightFailed {
            window_id: id,
            source,
        })?;
    }

    let mut applied_indices = Vec::with_capacity(targets.len());

    for (index, (target, id)) in targets.iter().zip(ids.iter().copied()).enumerate() {
        if let Err(source) = apply(target) {
            let mut rollback_failures = Vec::new();

            // A failed operation may still have partially mutated its target.
            if let Err(rollback_error) = rollback(target) {
                rollback_failures.push(RollbackFailure {
                    window_id: id,
                    source: rollback_error,
                });
            }

            // Recover previously applied targets in reverse execution order.
            for applied_index in applied_indices.iter().rev().copied() {
                if let Err(rollback_error) = rollback(&targets[applied_index]) {
                    rollback_failures.push(RollbackFailure {
                        window_id: ids[applied_index],
                        source: rollback_error,
                    });
                }
            }

            return Err(MultiWindowTransactionError::ApplyFailed {
                window_id: id,
                source,
                rollback_failures,
            });
        }

        applied_indices.push(index);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Target {
        id: u32,
    }

    fn targets(ids: &[u32]) -> Vec<Target> {
        ids.iter().map(|id| Target { id: *id }).collect()
    }

    #[test]
    fn rejects_zero_window_id_before_callbacks() {
        let input = targets(&[1, 0, 3]);
        let events = RefCell::new(Vec::new());

        let result = execute_multi_window_transaction(
            &input,
            |target| target.id,
            |_| {
                events.borrow_mut().push("preflight");
                Ok::<_, &'static str>(())
            },
            |_| {
                events.borrow_mut().push("apply");
                Ok::<_, &'static str>(())
            },
            |_| {
                events.borrow_mut().push("rollback");
                Ok::<_, &'static str>(())
            },
        );

        assert_eq!(
            result,
            Err(MultiWindowTransactionError::InvalidWindowId { index: 1 })
        );
        assert!(events.borrow().is_empty());
    }

    #[test]
    fn rejects_duplicate_ids_before_callbacks() {
        let input = targets(&[1, 2, 1]);
        let events = RefCell::new(Vec::new());

        let result = execute_multi_window_transaction(
            &input,
            |target| target.id,
            |_| {
                events.borrow_mut().push("preflight");
                Ok::<_, &'static str>(())
            },
            |_| {
                events.borrow_mut().push("apply");
                Ok::<_, &'static str>(())
            },
            |_| {
                events.borrow_mut().push("rollback");
                Ok::<_, &'static str>(())
            },
        );

        assert_eq!(
            result,
            Err(MultiWindowTransactionError::DuplicateWindowId { window_id: 1 })
        );
        assert!(events.borrow().is_empty());
    }

    #[test]
    fn preflight_failure_prevents_all_mutations() {
        let input = targets(&[1, 2, 3]);
        let events = RefCell::new(Vec::new());

        let result = execute_multi_window_transaction(
            &input,
            |target| target.id,
            |target| {
                events.borrow_mut().push(format!("preflight-{}", target.id));
                if target.id == 2 {
                    Err("preflight failure")
                } else {
                    Ok(())
                }
            },
            |target| {
                events.borrow_mut().push(format!("apply-{}", target.id));
                Ok::<_, &'static str>(())
            },
            |target| {
                events.borrow_mut().push(format!("rollback-{}", target.id));
                Ok::<_, &'static str>(())
            },
        );

        assert_eq!(
            result,
            Err(MultiWindowTransactionError::PreflightFailed {
                window_id: 2,
                source: "preflight failure",
            })
        );
        assert_eq!(*events.borrow(), vec!["preflight-1", "preflight-2"]);
    }

    #[test]
    fn successful_transaction_preflights_every_target_before_applying() {
        let input = targets(&[1, 2, 3]);
        let events = RefCell::new(Vec::new());

        let result = execute_multi_window_transaction(
            &input,
            |target| target.id,
            |target| {
                events.borrow_mut().push(format!("preflight-{}", target.id));
                Ok::<_, &'static str>(())
            },
            |target| {
                events.borrow_mut().push(format!("apply-{}", target.id));
                Ok::<_, &'static str>(())
            },
            |target| {
                events.borrow_mut().push(format!("rollback-{}", target.id));
                Ok::<_, &'static str>(())
            },
        );

        assert_eq!(result, Ok(()));
        assert_eq!(
            *events.borrow(),
            vec![
                "preflight-1",
                "preflight-2",
                "preflight-3",
                "apply-1",
                "apply-2",
                "apply-3",
            ]
        );
    }

    #[test]
    fn first_apply_failure_rolls_back_failing_target() {
        let input = targets(&[1, 2]);
        let events = RefCell::new(Vec::new());

        let result = execute_multi_window_transaction(
            &input,
            |target| target.id,
            |_| Ok::<_, &'static str>(()),
            |target| {
                events.borrow_mut().push(format!("apply-{}", target.id));
                Err("apply failure")
            },
            |target| {
                events.borrow_mut().push(format!("rollback-{}", target.id));
                Ok::<_, &'static str>(())
            },
        );

        assert_eq!(
            result,
            Err(MultiWindowTransactionError::ApplyFailed {
                window_id: 1,
                source: "apply failure",
                rollback_failures: vec![],
            })
        );
        assert_eq!(*events.borrow(), vec!["apply-1", "rollback-1"]);
    }

    #[test]
    fn third_apply_failure_rolls_back_in_reverse_order() {
        let input = targets(&[1, 2, 3]);
        let events = RefCell::new(Vec::new());

        let result = execute_multi_window_transaction(
            &input,
            |target| target.id,
            |_| Ok::<_, &'static str>(()),
            |target| {
                events.borrow_mut().push(format!("apply-{}", target.id));
                if target.id == 3 {
                    Err("window 3 apply failure")
                } else {
                    Ok(())
                }
            },
            |target| {
                events.borrow_mut().push(format!("rollback-{}", target.id));
                Ok::<_, &'static str>(())
            },
        );

        assert_eq!(
            result,
            Err(MultiWindowTransactionError::ApplyFailed {
                window_id: 3,
                source: "window 3 apply failure",
                rollback_failures: vec![],
            })
        );
        assert_eq!(
            *events.borrow(),
            vec![
                "apply-1",
                "apply-2",
                "apply-3",
                "rollback-3",
                "rollback-2",
                "rollback-1",
            ]
        );
    }

    #[test]
    fn rollback_failures_are_collected_without_stopping_recovery() {
        let input = targets(&[1, 2, 3]);
        let events = RefCell::new(Vec::new());

        let result = execute_multi_window_transaction(
            &input,
            |target| target.id,
            |_| Ok::<_, &'static str>(()),
            |target| {
                events.borrow_mut().push(format!("apply-{}", target.id));
                if target.id == 3 {
                    Err("original apply failure")
                } else {
                    Ok(())
                }
            },
            |target| {
                events.borrow_mut().push(format!("rollback-{}", target.id));
                match target.id {
                    3 => Err("rollback 3 failed"),
                    2 => Err("rollback 2 failed"),
                    _ => Ok(()),
                }
            },
        );

        assert_eq!(
            result,
            Err(MultiWindowTransactionError::ApplyFailed {
                window_id: 3,
                source: "original apply failure",
                rollback_failures: vec![
                    RollbackFailure {
                        window_id: 3,
                        source: "rollback 3 failed",
                    },
                    RollbackFailure {
                        window_id: 2,
                        source: "rollback 2 failed",
                    },
                ],
            })
        );

        assert_eq!(
            *events.borrow(),
            vec![
                "apply-1",
                "apply-2",
                "apply-3",
                "rollback-3",
                "rollback-2",
                "rollback-1",
            ]
        );
    }

    #[test]
    fn empty_target_list_is_a_successful_noop() {
        let events = RefCell::new(Vec::new());
        let input: Vec<Target> = Vec::new();

        let result = execute_multi_window_transaction(
            &input,
            |target| target.id,
            |_| {
                events.borrow_mut().push("preflight");
                Ok::<_, &'static str>(())
            },
            |_| {
                events.borrow_mut().push("apply");
                Ok::<_, &'static str>(())
            },
            |_| {
                events.borrow_mut().push("rollback");
                Ok::<_, &'static str>(())
            },
        );

        assert_eq!(result, Ok(()));
        assert!(events.borrow().is_empty());
    }
}

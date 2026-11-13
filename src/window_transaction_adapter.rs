//! Immutable window snapshots and phase-aware identity validation.
//!
//! This module is pure logic. It does not call CoreGraphics or Accessibility
//! APIs and does not perform window mutations.

use crate::window_discovery::{DiscoveredWindow, WindowBounds};
use crate::window_dry_run::WindowDryRunReport;

const GEOMETRY_TOLERANCE: f64 = 2.0;

#[derive(Debug, Clone, PartialEq)]
pub struct WindowSnapshot {
    pub window_id: u32,
    pub pid: u32,
    pub owner_name: Option<String>,
    pub title: String,
    pub original_bounds: WindowBounds,
    pub target_bounds: WindowBounds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowTransactionPhase {
    Original,
    Target,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotError {
    InvalidWindowId,
    InvalidPid,
    MissingTitle,
    InvalidOriginalGeometry,
    InvalidTargetGeometry,
    AmbiguousGeometry,
    IdentityChanged(&'static str),
    GeometryDoesNotMatch(WindowTransactionPhase),
}

impl WindowSnapshot {
    pub fn new(
        original: &DiscoveredWindow,
        target_bounds: WindowBounds,
    ) -> Result<Self, SnapshotError> {
        if original.window_id == 0 {
            return Err(SnapshotError::InvalidWindowId);
        }
        if original.pid == 0 {
            return Err(SnapshotError::InvalidPid);
        }

        let title = original
            .title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .ok_or(SnapshotError::MissingTitle)?;

        if !valid_bounds(&original.bounds) {
            return Err(SnapshotError::InvalidOriginalGeometry);
        }
        if !valid_bounds(&target_bounds) {
            return Err(SnapshotError::InvalidTargetGeometry);
        }

        // Original and target must be distinguishable under the geometry
        // tolerance used by classify_fresh(). Identical geometry is a
        // deliberate no-op and remains valid.
        if !geometries_identical(&original.bounds, &target_bounds)
            && geometry_regions_overlap(&original.bounds, &target_bounds)
        {
            return Err(SnapshotError::AmbiguousGeometry);
        }

        Ok(Self {
            window_id: original.window_id,
            pid: original.pid,
            owner_name: original.owner_name.clone(),
            title: title.to_owned(),
            original_bounds: original.bounds.clone(),
            target_bounds,
        })
    }

    fn validate_identity(&self, current: &DiscoveredWindow) -> Result<(), SnapshotError> {
        if current.window_id != self.window_id {
            return Err(SnapshotError::IdentityChanged("window ID"));
        }
        if current.pid != self.pid {
            return Err(SnapshotError::IdentityChanged("PID"));
        }
        if current.owner_name != self.owner_name {
            return Err(SnapshotError::IdentityChanged("owner"));
        }
        if current.title.as_deref() != Some(self.title.as_str()) {
            return Err(SnapshotError::IdentityChanged("title"));
        }

        Ok(())
    }

    /// Determine which known stable geometry is currently observed.
    ///
    /// Original is preferred when original and target geometries are equal.
    /// This makes an already-satisfied layout a safe no-op during rollback.
    pub fn classify_fresh(
        &self,
        current: &DiscoveredWindow,
    ) -> Result<WindowTransactionPhase, SnapshotError> {
        self.validate_identity(current)?;

        if geometry_matches(&current.bounds, &self.original_bounds) {
            return Ok(WindowTransactionPhase::Original);
        }

        if geometry_matches(&current.bounds, &self.target_bounds) {
            return Ok(WindowTransactionPhase::Target);
        }

        Err(SnapshotError::GeometryDoesNotMatch(
            WindowTransactionPhase::Original,
        ))
    }

    pub fn validate_fresh(
        &self,
        current: &DiscoveredWindow,
        phase: WindowTransactionPhase,
    ) -> Result<(), SnapshotError> {
        self.validate_identity(current)?;

        // An identical original/target geometry is a deliberate no-op.
        // It is valid as either the requested Original or Target phase.
        if geometries_identical(&self.original_bounds, &self.target_bounds)
            && geometry_matches(&current.bounds, &self.original_bounds)
        {
            return Ok(());
        }

        let observed = self.classify_fresh(current)?;

        if observed != phase {
            return Err(SnapshotError::GeometryDoesNotMatch(phase));
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotBatchError {
    DuplicateWindowId { window_id: u32 },
    InvalidEntry { index: usize, source: SnapshotError },
}

/// Convert a dry-run report into immutable transaction snapshots.
///
/// Accessibility indices are intentionally discarded: they are only valid
/// for the AX enumeration from which the report was produced.
pub fn snapshots_from_dry_run(
    report: &WindowDryRunReport,
) -> Result<Vec<WindowSnapshot>, SnapshotBatchError> {
    let mut snapshots = Vec::with_capacity(report.entries.len());
    let mut ids = Vec::with_capacity(report.entries.len());

    for (index, entry) in report.entries.iter().enumerate() {
        if ids.contains(&entry.window_id) {
            return Err(SnapshotBatchError::DuplicateWindowId {
                window_id: entry.window_id,
            });
        }

        let discovered = DiscoveredWindow {
            window_id: entry.window_id,
            pid: entry.pid,
            owner_name: if entry.owner_name.is_empty() {
                None
            } else {
                Some(entry.owner_name.clone())
            },
            title: Some(entry.title.clone()),
            bounds: entry.current_bounds.clone(),
        };

        let snapshot = WindowSnapshot::new(&discovered, entry.target_bounds.clone())
            .map_err(|source| SnapshotBatchError::InvalidEntry { index, source })?;

        ids.push(entry.window_id);
        snapshots.push(snapshot);
    }

    Ok(snapshots)
}

fn valid_bounds(bounds: &WindowBounds) -> bool {
    let right = bounds.x + bounds.width;
    let bottom = bounds.y + bounds.height;

    bounds.x.is_finite()
        && bounds.y.is_finite()
        && bounds.width.is_finite()
        && bounds.height.is_finite()
        && right.is_finite()
        && bottom.is_finite()
        && bounds.width > 0.0
        && bounds.height > 0.0
}

fn geometries_identical(left: &WindowBounds, right: &WindowBounds) -> bool {
    left.x == right.x
        && left.y == right.y
        && left.width == right.width
        && left.height == right.height
}

/// Two geometry match regions overlap when every corresponding component
/// differs by no more than twice the matching tolerance.
fn geometry_regions_overlap(left: &WindowBounds, right: &WindowBounds) -> bool {
    let overlap_limit = 2.0 * GEOMETRY_TOLERANCE;

    [
        (left.x, right.x),
        (left.y, right.y),
        (left.width, right.width),
        (left.height, right.height),
    ]
    .iter()
    .all(|(left, right)| (left - right).abs() <= overlap_limit)
}

fn geometry_matches(actual: &WindowBounds, expected: &WindowBounds) -> bool {
    valid_bounds(actual)
        && valid_bounds(expected)
        && [
            (actual.x, expected.x),
            (actual.y, expected.y),
            (actual.width, expected.width),
            (actual.height, expected.height),
        ]
        .iter()
        .all(|(actual, expected)| (actual - expected).abs() <= GEOMETRY_TOLERANCE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(x: f64, y: f64, width: f64, height: f64) -> WindowBounds {
        WindowBounds {
            x,
            y,
            width,
            height,
        }
    }

    fn window() -> DiscoveredWindow {
        DiscoveredWindow {
            window_id: 123,
            pid: 456,
            owner_name: Some("Minecraft".to_owned()),
            title: Some("Minecraft 1.21.4".to_owned()),
            bounds: bounds(100.0, 80.0, 800.0, 600.0),
        }
    }

    fn snapshot() -> WindowSnapshot {
        WindowSnapshot::new(&window(), bounds(0.0, 24.0, 700.0, 900.0)).unwrap()
    }

    fn dry_run_report(
        entries: Vec<crate::window_dry_run::WindowDryRunEntry>,
    ) -> WindowDryRunReport {
        WindowDryRunReport {
            display_id: 1,
            direction: crate::window_layout::LayoutDirection::Horizontal,
            entries,
        }
    }

    fn dry_run_entry(window_id: u32) -> crate::window_dry_run::WindowDryRunEntry {
        crate::window_dry_run::WindowDryRunEntry {
            window_id,
            pid: 456,
            owner_name: "Minecraft".to_owned(),
            title: "Minecraft 1.21.4".to_owned(),
            current_bounds: bounds(100.0, 80.0, 800.0, 600.0),
            target_bounds: bounds(0.0, 24.0, 700.0, 900.0),
            accessibility_index: 99,
        }
    }

    #[test]
    fn builds_snapshots_from_dry_run_and_discards_ax_index() {
        let report = dry_run_report(vec![dry_run_entry(123)]);
        let snapshots = snapshots_from_dry_run(&report).unwrap();

        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].window_id, 123);
        assert_eq!(snapshots[0].owner_name.as_deref(), Some("Minecraft"));
    }

    #[test]
    fn rejects_duplicate_ids_in_dry_run_report() {
        let report = dry_run_report(vec![dry_run_entry(123), dry_run_entry(123)]);

        assert_eq!(
            snapshots_from_dry_run(&report),
            Err(SnapshotBatchError::DuplicateWindowId { window_id: 123 })
        );
    }

    #[test]
    fn rejects_invalid_dry_run_entry_with_its_index() {
        let mut invalid = dry_run_entry(123);
        invalid.title.clear();
        let report = dry_run_report(vec![dry_run_entry(122), invalid]);

        assert_eq!(
            snapshots_from_dry_run(&report),
            Err(SnapshotBatchError::InvalidEntry {
                index: 1,
                source: SnapshotError::MissingTitle,
            })
        );
    }

    #[test]
    fn classifies_original_geometry_before_target_geometry() {
        let snapshot = WindowSnapshot::new(&window(), bounds(100.0, 80.0, 800.0, 600.0)).unwrap();

        assert_eq!(
            snapshot.classify_fresh(&window()),
            Ok(WindowTransactionPhase::Original)
        );
    }

    #[test]
    fn allows_identical_original_and_target_geometry_as_noop() {
        let original = window();
        let snapshot = WindowSnapshot::new(&original, original.bounds.clone()).unwrap();

        assert_eq!(
            snapshot.classify_fresh(&original),
            Ok(WindowTransactionPhase::Original)
        );
    }

    #[test]
    fn rejects_geometry_regions_with_one_point_overlap() {
        let result = WindowSnapshot::new(&window(), bounds(101.0, 80.0, 800.0, 600.0));

        assert_eq!(result, Err(SnapshotError::AmbiguousGeometry));
    }

    #[test]
    fn rejects_geometry_regions_touching_at_tolerance_boundary() {
        let result = WindowSnapshot::new(&window(), bounds(104.0, 80.0, 800.0, 600.0));

        assert_eq!(result, Err(SnapshotError::AmbiguousGeometry));
    }

    #[test]
    fn allows_geometry_regions_separated_on_one_component() {
        let original = window();
        let target = bounds(105.0, 80.0, 800.0, 600.0);
        let snapshot = WindowSnapshot::new(&original, target.clone()).unwrap();

        assert_eq!(
            snapshot.classify_fresh(&original),
            Ok(WindowTransactionPhase::Original)
        );

        let mut at_target = original;
        at_target.bounds = target;

        assert_eq!(
            snapshot.classify_fresh(&at_target),
            Ok(WindowTransactionPhase::Target)
        );
    }

    #[test]
    fn classifies_target_geometry() {
        let snapshot = snapshot();
        let mut current = window();
        current.bounds = bounds(0.0, 24.0, 700.0, 900.0);

        assert_eq!(
            snapshot.classify_fresh(&current),
            Ok(WindowTransactionPhase::Target)
        );
    }

    #[test]
    fn rejects_intermediate_geometry_during_classification() {
        let snapshot = snapshot();
        let mut current = window();
        current.bounds = bounds(50.0, 40.0, 750.0, 700.0);

        assert_eq!(
            snapshot.classify_fresh(&current),
            Err(SnapshotError::GeometryDoesNotMatch(
                WindowTransactionPhase::Original
            ))
        );
    }

    #[test]
    fn captures_immutable_original_and_target_geometry() {
        let result = snapshot();

        assert_eq!(result.window_id, 123);
        assert_eq!(result.pid, 456);
        assert_eq!(result.original_bounds, bounds(100.0, 80.0, 800.0, 600.0));
        assert_eq!(result.target_bounds, bounds(0.0, 24.0, 700.0, 900.0));
    }

    #[test]
    fn accepts_fresh_original_state() {
        assert!(
            snapshot()
                .validate_fresh(&window(), WindowTransactionPhase::Original)
                .is_ok()
        );
    }

    #[test]
    fn accepts_fresh_target_state() {
        let mut current = window();
        current.bounds = bounds(0.0, 24.0, 700.0, 900.0);

        assert!(
            snapshot()
                .validate_fresh(&current, WindowTransactionPhase::Target)
                .is_ok()
        );
    }

    #[test]
    fn rejects_pid_change() {
        let mut current = window();
        current.pid += 1;

        assert_eq!(
            snapshot().validate_fresh(&current, WindowTransactionPhase::Original),
            Err(SnapshotError::IdentityChanged("PID"))
        );
    }

    #[test]
    fn rejects_owner_change() {
        let mut current = window();
        current.owner_name = Some("Other".to_owned());

        assert_eq!(
            snapshot().validate_fresh(&current, WindowTransactionPhase::Original),
            Err(SnapshotError::IdentityChanged("owner"))
        );
    }

    #[test]
    fn rejects_title_change() {
        let mut current = window();
        current.title = Some("Different window".to_owned());

        assert_eq!(
            snapshot().validate_fresh(&current, WindowTransactionPhase::Original),
            Err(SnapshotError::IdentityChanged("title"))
        );
    }

    #[test]
    fn rejects_geometry_that_matches_neither_phase() {
        let mut current = window();
        current.bounds.x += 10.0;

        assert_eq!(
            snapshot().validate_fresh(&current, WindowTransactionPhase::Original),
            Err(SnapshotError::GeometryDoesNotMatch(
                WindowTransactionPhase::Original
            ))
        );
    }

    #[test]
    fn rejects_invalid_original_geometry() {
        let mut current = window();
        current.bounds.width = 0.0;

        assert_eq!(
            WindowSnapshot::new(&current, bounds(0.0, 0.0, 700.0, 900.0)),
            Err(SnapshotError::InvalidOriginalGeometry)
        );
    }

    #[test]
    fn rejects_invalid_target_geometry() {
        assert_eq!(
            WindowSnapshot::new(&window(), bounds(0.0, 0.0, f64::NAN, 900.0)),
            Err(SnapshotError::InvalidTargetGeometry)
        );
    }

    #[test]
    fn rejects_missing_title() {
        let mut current = window();
        current.title = None;

        assert_eq!(
            WindowSnapshot::new(&current, bounds(0.0, 0.0, 700.0, 900.0)),
            Err(SnapshotError::MissingTitle)
        );
    }

    #[test]
    fn rejects_zero_window_id() {
        let mut current = window();
        current.window_id = 0;

        assert_eq!(
            WindowSnapshot::new(&current, bounds(0.0, 0.0, 700.0, 900.0)),
            Err(SnapshotError::InvalidWindowId)
        );
    }

    #[test]
    fn rejects_zero_pid() {
        let mut current = window();
        current.pid = 0;

        assert_eq!(
            WindowSnapshot::new(&current, bounds(0.0, 0.0, 700.0, 900.0)),
            Err(SnapshotError::InvalidPid)
        );
    }
}

/// Errors produced while adapting immutable snapshots to platform operations.
#[derive(Debug, PartialEq)]
pub enum WindowAdapterError<D, S> {
    Discovery(D),
    Snapshot(SnapshotError),
    WindowNotFound { window_id: u32 },
    AmbiguousWindow { window_id: u32, count: usize },
    Setter(S),
}

fn find_fresh_window<D, S>(
    windows: &[DiscoveredWindow],
    window_id: u32,
) -> Result<DiscoveredWindow, WindowAdapterError<D, S>> {
    let mut matches = windows
        .iter()
        .filter(|window| window.window_id == window_id);

    let Some(first) = matches.next() else {
        return Err(WindowAdapterError::WindowNotFound { window_id });
    };

    let result = first.clone();
    let count = 1 + matches.count();

    if count != 1 {
        return Err(WindowAdapterError::AmbiguousWindow { window_id, count });
    }

    Ok(result)
}

/// Execute snapshots using injected discovery and mutation operations.
///
/// Discovery is repeated during preflight, immediately before each apply,
/// after each successful apply, and independently before and after rollback.
/// The function itself does not call operating-system APIs.
pub fn execute_window_snapshots<D, S, Discover, SetBounds>(
    snapshots: &[WindowSnapshot],
    mut discover: Discover,
    mut set_bounds: SetBounds,
) -> Result<
    (),
    crate::window_transaction::MultiWindowTransactionError<
        WindowAdapterError<D, S>,
        WindowAdapterError<D, S>,
    >,
>
where
    Discover: FnMut() -> Result<Vec<DiscoveredWindow>, D>,
    SetBounds: FnMut(u32, &DiscoveredWindow, &WindowBounds) -> Result<(), S>,
{
    use std::cell::RefCell;

    let discover = RefCell::new(&mut discover);
    let set_bounds = RefCell::new(&mut set_bounds);

    // Record an apply attempt immediately before invoking the setter.
    // This includes setters that partially mutate and then return an error.
    let apply_attempted = RefCell::new(Vec::<u32>::new());

    crate::window_transaction::execute_multi_window_transaction(
        snapshots,
        |snapshot| snapshot.window_id,
        |snapshot| {
            let windows = (discover.borrow_mut())().map_err(WindowAdapterError::Discovery)?;
            let fresh = find_fresh_window(&windows, snapshot.window_id)?;

            snapshot
                .validate_fresh(&fresh, WindowTransactionPhase::Original)
                .map_err(WindowAdapterError::Snapshot)
        },
        |snapshot| {
            let windows = (discover.borrow_mut())().map_err(WindowAdapterError::Discovery)?;
            let fresh = find_fresh_window(&windows, snapshot.window_id)?;

            snapshot
                .validate_fresh(&fresh, WindowTransactionPhase::Original)
                .map_err(WindowAdapterError::Snapshot)?;

            // Do not issue an OS mutation for an already-satisfied snapshot.
            if !geometries_identical(&snapshot.original_bounds, &snapshot.target_bounds) {
                apply_attempted.borrow_mut().push(snapshot.window_id);

                (set_bounds.borrow_mut())(snapshot.pid, &fresh, &snapshot.target_bounds)
                    .map_err(WindowAdapterError::Setter)?;
            }

            let windows = (discover.borrow_mut())().map_err(WindowAdapterError::Discovery)?;
            let fresh = find_fresh_window(&windows, snapshot.window_id)?;

            snapshot
                .validate_fresh(&fresh, WindowTransactionPhase::Target)
                .map_err(WindowAdapterError::Snapshot)
        },
        |snapshot| {
            let windows = (discover.borrow_mut())().map_err(WindowAdapterError::Discovery)?;
            let fresh = find_fresh_window(&windows, snapshot.window_id)?;

            let phase = match snapshot.classify_fresh(&fresh) {
                Ok(phase) => phase,
                Err(SnapshotError::GeometryDoesNotMatch(_))
                    if apply_attempted.borrow().contains(&snapshot.window_id) =>
                {
                    // The setter may have changed only part of the geometry.
                    // Identity has already been validated by classify_fresh().
                    // Restore the original snapshot, then independently verify it.
                    WindowTransactionPhase::Target
                }
                Err(error) => return Err(WindowAdapterError::Snapshot(error)),
            };

            match phase {
                WindowTransactionPhase::Original => Ok(()),
                WindowTransactionPhase::Target => {
                    (set_bounds.borrow_mut())(snapshot.pid, &fresh, &snapshot.original_bounds)
                        .map_err(WindowAdapterError::Setter)?;

                    let windows =
                        (discover.borrow_mut())().map_err(WindowAdapterError::Discovery)?;
                    let fresh = find_fresh_window(&windows, snapshot.window_id)?;

                    snapshot
                        .validate_fresh(&fresh, WindowTransactionPhase::Original)
                        .map_err(WindowAdapterError::Snapshot)
                }
            }
        },
    )
}

#[cfg(test)]
mod execution_tests {
    use super::*;
    use std::cell::RefCell;

    fn bounds(x: f64, y: f64, width: f64, height: f64) -> WindowBounds {
        WindowBounds {
            x,
            y,
            width,
            height,
        }
    }

    fn window(id: u32) -> DiscoveredWindow {
        DiscoveredWindow {
            window_id: id,
            pid: 456 + id,
            owner_name: Some("Minecraft".to_owned()),
            title: Some(format!("Minecraft {id}")),
            bounds: bounds(100.0, 80.0, 800.0, 600.0),
        }
    }

    fn snapshot(id: u32) -> WindowSnapshot {
        WindowSnapshot::new(&window(id), bounds(0.0, 24.0, 700.0, 900.0)).unwrap()
    }

    #[test]
    fn executes_multiple_windows_and_verifies_target_geometry() {
        let state = RefCell::new(vec![window(1), window(2)]);
        let snapshots = vec![snapshot(1), snapshot(2)];

        let result = execute_window_snapshots(
            &snapshots,
            || Ok::<_, &'static str>(state.borrow().clone()),
            |_, fresh, desired| {
                let mut state = state.borrow_mut();
                let current = state
                    .iter_mut()
                    .find(|item| item.window_id == fresh.window_id)
                    .unwrap();
                current.bounds = desired.clone();
                Ok::<_, &'static str>(())
            },
        );

        assert_eq!(result, Ok(()));

        let state = state.borrow();
        assert!(snapshots.iter().all(|snapshot| {
            let current = state
                .iter()
                .find(|item| item.window_id == snapshot.window_id)
                .unwrap();
            snapshot
                .validate_fresh(current, WindowTransactionPhase::Target)
                .is_ok()
        }));
    }

    #[test]
    fn preflight_failure_prevents_all_setter_calls() {
        let mut changed = window(2);
        changed.title = Some("Unexpected title".to_owned());

        let state = RefCell::new(vec![window(1), changed]);
        let setter_calls = RefCell::new(0usize);
        let snapshots = vec![snapshot(1), snapshot(2)];

        let result = execute_window_snapshots(
            &snapshots,
            || Ok::<_, &'static str>(state.borrow().clone()),
            |_, _, _| {
                *setter_calls.borrow_mut() += 1;
                Ok::<_, &'static str>(())
            },
        );

        assert!(matches!(
            result,
            Err(
                crate::window_transaction::MultiWindowTransactionError::PreflightFailed {
                    window_id: 2,
                    source: WindowAdapterError::Snapshot(SnapshotError::IdentityChanged("title")),
                }
            )
        ));
        assert_eq!(*setter_calls.borrow(), 0);
    }

    #[test]
    fn detects_successful_setter_that_did_not_apply_geometry() {
        let state = RefCell::new(vec![window(1)]);
        let snapshots = vec![snapshot(1)];

        let result = execute_window_snapshots(
            &snapshots,
            || Ok::<_, &'static str>(state.borrow().clone()),
            |_, _, _| Ok::<_, &'static str>(()),
        );

        assert!(matches!(
            result,
            Err(crate::window_transaction::MultiWindowTransactionError::ApplyFailed {
                window_id: 1,
                source: WindowAdapterError::Snapshot(
                    SnapshotError::GeometryDoesNotMatch(WindowTransactionPhase::Target)
                ),
                rollback_failures,
            }) if rollback_failures.is_empty()
        ));

        assert_eq!(state.borrow()[0].bounds, snapshots[0].original_bounds);
    }

    #[test]
    fn partially_applied_failure_rolls_back_in_reverse_order_and_verifies() {
        let state = RefCell::new(vec![window(1), window(2)]);
        let snapshots = vec![snapshot(1), snapshot(2)];
        let setter_calls = RefCell::new(Vec::<(u32, f64)>::new());

        let result = execute_window_snapshots(
            &snapshots,
            || Ok::<_, &'static str>(state.borrow().clone()),
            |_, fresh, desired| {
                setter_calls.borrow_mut().push((fresh.window_id, desired.x));

                let mut state = state.borrow_mut();
                let current = state
                    .iter_mut()
                    .find(|item| item.window_id == fresh.window_id)
                    .unwrap();

                // Simulate a setter that changes geometry but reports failure
                // for the second window during its apply phase.
                current.bounds = desired.clone();

                if fresh.window_id == 2 && desired.x == 0.0 {
                    return Err("simulated partial setter failure");
                }

                Ok(())
            },
        );

        assert!(matches!(
            result,
            Err(crate::window_transaction::MultiWindowTransactionError::ApplyFailed {
                window_id: 2,
                source: WindowAdapterError::Setter("simulated partial setter failure"),
                rollback_failures,
            }) if rollback_failures.is_empty()
        ));

        assert_eq!(
            *setter_calls.borrow(),
            vec![(1, 0.0), (2, 0.0), (2, 100.0), (1, 100.0)]
        );

        let state = state.borrow();
        for snapshot in &snapshots {
            let current = state
                .iter()
                .find(|item| item.window_id == snapshot.window_id)
                .unwrap();
            assert!(
                snapshot
                    .validate_fresh(current, WindowTransactionPhase::Original)
                    .is_ok()
            );
        }
    }

    #[test]
    fn rejects_ambiguous_window_id_during_preflight() {
        let duplicate = window(1);
        let state = vec![duplicate.clone(), duplicate];
        let setter_calls = RefCell::new(0usize);

        let result = execute_window_snapshots(
            &[snapshot(1)],
            || Ok::<_, &'static str>(state.clone()),
            |_, _, _| {
                *setter_calls.borrow_mut() += 1;
                Ok::<_, &'static str>(())
            },
        );

        assert!(matches!(
            result,
            Err(
                crate::window_transaction::MultiWindowTransactionError::PreflightFailed {
                    window_id: 1,
                    source: WindowAdapterError::AmbiguousWindow {
                        window_id: 1,
                        count: 2,
                    },
                }
            )
        ));
        assert_eq!(*setter_calls.borrow(), 0);
    }

    #[test]
    fn missing_window_during_apply_is_reported_as_rollback_failure_if_still_missing() {
        use std::cell::Cell;

        let discovery_calls = Cell::new(0usize);

        let result = execute_window_snapshots(
            &[snapshot(1)],
            || {
                let call = discovery_calls.get() + 1;
                discovery_calls.set(call);

                match call {
                    1 => Ok::<_, &'static str>(vec![window(1)]),
                    2 | 3 => Ok(Vec::new()),
                    _ => unreachable!(),
                }
            },
            |_, _, _| Ok::<_, &'static str>(()),
        );

        assert!(matches!(
            result,
            Err(crate::window_transaction::MultiWindowTransactionError::ApplyFailed {
                window_id: 1,
                source: WindowAdapterError::WindowNotFound { window_id: 1 },
                rollback_failures,
            }) if rollback_failures.len() == 1
                && rollback_failures[0].window_id == 1
                && rollback_failures[0].source
                    == WindowAdapterError::WindowNotFound { window_id: 1 }
        ));

        assert_eq!(discovery_calls.get(), 3);
    }

    #[test]
    fn identity_change_during_apply_prevents_mutation_and_rollback() {
        use std::cell::Cell;

        let discovery_calls = Cell::new(0usize);

        let result = execute_window_snapshots::<&str, &str, _, _>(
            &[snapshot(1)],
            || {
                let call = discovery_calls.get() + 1;
                discovery_calls.set(call);

                let mut current = window(1);
                if call >= 2 {
                    current.title = Some("Replaced window".to_owned());
                }

                Ok::<_, &'static str>(vec![current])
            },
            |_, _, _| panic!("setter must not run after identity validation fails"),
        );

        assert!(matches!(
            result,
            Err(crate::window_transaction::MultiWindowTransactionError::ApplyFailed {
                window_id: 1,
                source: WindowAdapterError::Snapshot(
                    SnapshotError::IdentityChanged("title")
                ),
                rollback_failures,
            }) if rollback_failures.len() == 1
                && rollback_failures[0].source
                    == WindowAdapterError::Snapshot(
                        SnapshotError::IdentityChanged("title")
                    )
        ));

        assert_eq!(discovery_calls.get(), 3);
    }

    #[test]
    fn rollback_failure_is_reported_and_does_not_stop_prior_recovery() {
        let state = RefCell::new(vec![window(1), window(2)]);
        let snapshots = vec![snapshot(1), snapshot(2)];
        let setter_calls = RefCell::new(Vec::<(u32, f64)>::new());

        let result = execute_window_snapshots(
            &snapshots,
            || Ok::<_, &'static str>(state.borrow().clone()),
            |_, fresh, desired| {
                setter_calls.borrow_mut().push((fresh.window_id, desired.x));

                if fresh.window_id == 2 && desired.x == 100.0 {
                    return Err("rollback failure");
                }

                let mut state = state.borrow_mut();
                let current = state
                    .iter_mut()
                    .find(|item| item.window_id == fresh.window_id)
                    .unwrap();

                current.bounds = desired.clone();

                if fresh.window_id == 2 && desired.x == 0.0 {
                    return Err("apply failure");
                }

                Ok(())
            },
        );

        assert!(matches!(
            result,
            Err(crate::window_transaction::MultiWindowTransactionError::ApplyFailed {
                window_id: 2,
                source: WindowAdapterError::Setter("apply failure"),
                rollback_failures,
            }) if rollback_failures.len() == 1
                && rollback_failures[0].window_id == 2
                && rollback_failures[0].source
                    == WindowAdapterError::Setter("rollback failure")
        ));

        assert_eq!(
            *setter_calls.borrow(),
            vec![(1, 0.0), (2, 0.0), (2, 100.0), (1, 100.0)]
        );

        let state = state.borrow();
        assert_eq!(state[0].bounds, snapshots[0].original_bounds);
        assert_eq!(state[1].bounds, snapshots[1].target_bounds);
    }

    #[test]
    fn validate_target_accepts_identical_original_geometry() {
        let original = window(1);
        let snapshot = WindowSnapshot::new(&original, original.bounds.clone()).unwrap();

        assert_eq!(
            snapshot.validate_fresh(&original, WindowTransactionPhase::Target),
            Ok(())
        );
    }

    #[test]
    fn executes_noop_snapshot_without_setter_call() {
        let original = window(1);
        let state = RefCell::new(vec![original.clone()]);
        let noop = WindowSnapshot::new(&original, original.bounds.clone()).unwrap();
        let setter_calls = RefCell::new(0usize);

        let result = execute_window_snapshots(
            &[noop],
            || Ok::<_, &'static str>(state.borrow().clone()),
            |_, _, _| {
                *setter_calls.borrow_mut() += 1;
                Ok::<_, &'static str>(())
            },
        );

        assert_eq!(result, Ok(()));
        assert_eq!(*setter_calls.borrow(), 0);
        assert_eq!(state.borrow()[0].bounds, original.bounds);
    }

    #[test]
    fn partially_mutated_setter_failure_restores_original_geometry() {
        let state = RefCell::new(vec![window(1)]);
        let snapshots = vec![snapshot(1)];
        let setter_calls = RefCell::new(Vec::<(u32, f64)>::new());

        let result = execute_window_snapshots(
            &snapshots,
            || Ok::<_, &'static str>(state.borrow().clone()),
            |_, fresh, desired| {
                setter_calls.borrow_mut().push((fresh.window_id, desired.x));

                let mut state = state.borrow_mut();
                let current = state
                    .iter_mut()
                    .find(|item| item.window_id == fresh.window_id)
                    .unwrap();

                if desired.x == 0.0 {
                    // Simulate a partial position/size update before failure.
                    current.bounds = bounds(50.0, 40.0, 750.0, 700.0);
                    return Err("partial setter failure");
                }

                current.bounds = desired.clone();
                Ok(())
            },
        );

        assert!(matches!(
            result,
            Err(crate::window_transaction::MultiWindowTransactionError::ApplyFailed {
                window_id: 1,
                source: WindowAdapterError::Setter("partial setter failure"),
                rollback_failures,
            }) if rollback_failures.is_empty()
        ));

        assert_eq!(*setter_calls.borrow(), vec![(1, 0.0), (1, 100.0)]);
        assert_eq!(state.borrow()[0].bounds, snapshots[0].original_bounds);
    }

    #[test]
    fn post_apply_discovery_failure_still_restores_original_geometry() {
        use std::cell::Cell;

        let state = RefCell::new(vec![window(1)]);
        let discovery_calls = Cell::new(0usize);
        let setter_calls = RefCell::new(Vec::<(u32, f64)>::new());

        let result = execute_window_snapshots(
            &[snapshot(1)],
            || {
                let call = discovery_calls.get() + 1;
                discovery_calls.set(call);

                if call == 3 {
                    return Err("post-apply discovery failure");
                }

                Ok::<_, &'static str>(state.borrow().clone())
            },
            |_, fresh, desired| {
                setter_calls.borrow_mut().push((fresh.window_id, desired.x));

                let mut state = state.borrow_mut();
                let current = state
                    .iter_mut()
                    .find(|item| item.window_id == fresh.window_id)
                    .unwrap();

                if desired.x == 0.0 {
                    // Apply leaves an intermediate geometry, then succeeds.
                    current.bounds = bounds(50.0, 40.0, 750.0, 700.0);
                } else {
                    current.bounds = desired.clone();
                }

                Ok::<_, &'static str>(())
            },
        );

        assert!(matches!(
            result,
            Err(crate::window_transaction::MultiWindowTransactionError::ApplyFailed {
                window_id: 1,
                source: WindowAdapterError::Discovery("post-apply discovery failure"),
                rollback_failures,
            }) if rollback_failures.is_empty()
        ));

        assert_eq!(discovery_calls.get(), 5);
        assert_eq!(*setter_calls.borrow(), vec![(1, 0.0), (1, 100.0)]);
        assert_eq!(state.borrow()[0].bounds, snapshot(1).original_bounds);
    }

    #[test]
    fn discovery_failure_preserves_its_own_error_type() {
        let result = execute_window_snapshots::<&str, u8, _, _>(
            &[snapshot(1)],
            || Err("discovery failed"),
            |_, _, _| Ok(()),
        );

        assert!(matches!(
            result,
            Err(
                crate::window_transaction::MultiWindowTransactionError::PreflightFailed {
                    window_id: 1,
                    source: WindowAdapterError::Discovery("discovery failed"),
                }
            )
        ));
    }

    #[test]
    fn discovery_and_setter_may_use_different_error_types() {
        let result = execute_window_snapshots::<&str, u8, _, _>(
            &[],
            || Ok::<_, &str>(Vec::new()),
            |_, _, _| Ok::<_, u8>(()),
        );

        assert_eq!(result, Ok(()));
    }
}

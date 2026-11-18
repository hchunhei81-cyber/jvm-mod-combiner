use std::collections::HashMap;

use crate::display_geometry;
use crate::window_accessibility;
use crate::window_discovery::{self, DiscoveredWindow, WindowBounds};
use crate::window_layout::{self, LayoutConstraints, LayoutDirection, WindowLayoutPlan};

#[derive(Debug, Clone)]
pub struct WindowDryRunEntry {
    pub window_id: u32,
    pub pid: u32,
    pub owner_name: String,
    pub title: String,
    pub current_bounds: WindowBounds,
    pub target_bounds: WindowBounds,
    pub accessibility_index: usize,
}

#[derive(Debug, Clone)]
pub struct WindowDryRunReport {
    pub display_id: u32,
    pub direction: LayoutDirection,
    pub entries: Vec<WindowDryRunEntry>,
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

fn geometry_stable(current: &WindowBounds, original: &WindowBounds) -> bool {
    [
        (current.x, original.x),
        (current.y, original.y),
        (current.width, original.width),
        (current.height, original.height),
    ]
    .iter()
    .all(|(actual, expected)| actual.is_finite() && (actual - expected).abs() <= 2.0)
        && valid_bounds(current)
}

fn find_unique_window(
    windows: &[DiscoveredWindow],
    window_id: u32,
) -> Result<DiscoveredWindow, String> {
    let matches: Vec<&DiscoveredWindow> = windows
        .iter()
        .filter(|window| window.window_id == window_id)
        .collect();

    match matches.as_slice() {
        [window] => Ok((*window).clone()),
        [] => Err(format!("Window ID {window_id} was not found")),
        _ => Err(format!(
            "Window ID {window_id} appeared {} times",
            matches.len()
        )),
    }
}

fn verify_window_identity(
    original: &DiscoveredWindow,
    current: &DiscoveredWindow,
) -> Result<(), String> {
    if current.window_id != original.window_id {
        return Err(format!(
            "Window ID changed: {} -> {}",
            original.window_id, current.window_id
        ));
    }

    if current.pid != original.pid {
        return Err(format!(
            "Window {} PID changed: {} -> {}",
            original.window_id, original.pid, current.pid
        ));
    }

    if current.title != original.title {
        return Err(format!(
            "Window {} title changed during verification",
            original.window_id
        ));
    }

    if current.owner_name != original.owner_name {
        return Err(format!(
            "Window {} owner changed during verification",
            original.window_id
        ));
    }

    if !geometry_stable(&current.bounds, &original.bounds) {
        return Err(format!(
            "Window {} geometry changed during verification",
            original.window_id
        ));
    }

    Ok(())
}

fn check_deadline(deadline: Option<std::time::Instant>) -> Result<(), String> {
    if deadline.is_some_and(|limit| std::time::Instant::now() >= limit) {
        return Err("dry-run execution deadline exceeded".to_owned());
    }

    Ok(())
}

fn run_deadline_stage_with_check<T>(
    check: &mut impl FnMut() -> Result<(), String>,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    check()?;
    let result = operation();
    check()?;
    result
}

fn run_deadline_stage<T>(
    deadline: Option<std::time::Instant>,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let mut check = || check_deadline(deadline);
    run_deadline_stage_with_check(&mut check, operation)
}

pub fn build_window_dry_run(
    window_ids: &[u32],
    direction: LayoutDirection,
    display_id: Option<u32>,
    constraints: LayoutConstraints,
) -> Result<WindowDryRunReport, String> {
    build_window_dry_run_with_deadline(window_ids, direction, display_id, constraints, None)
}

pub fn build_window_dry_run_with_deadline(
    window_ids: &[u32],
    direction: LayoutDirection,
    display_id: Option<u32>,
    constraints: LayoutConstraints,
    deadline: Option<std::time::Instant>,
) -> Result<WindowDryRunReport, String> {
    check_deadline(deadline)?;
    if window_ids.len() < 2 {
        return Err("at least two Window IDs are required".to_owned());
    }

    let mut unique_ids = std::collections::HashSet::new();

    for id in window_ids {
        if *id == 0 {
            return Err("Window IDs must be greater than zero".to_owned());
        }

        if !unique_ids.insert(*id) {
            return Err(format!("duplicate Window ID: {id}"));
        }
    }

    let displays = run_deadline_stage(deadline, || {
        display_geometry::discover_displays()
            .map_err(|error| format!("display discovery failed: {error}"))
    })?;

    let plan: WindowLayoutPlan = run_deadline_stage(deadline, || {
        match display_id {
            Some(id) => window_layout::plan_on_display_id(
                &displays,
                id,
                window_ids.len(),
                direction,
                constraints,
            ),
            None => window_layout::plan_on_main_display(
                &displays,
                window_ids.len(),
                direction,
                constraints,
            ),
        }
        .map_err(|error| format!("layout planning failed: {error}"))
    })?;

    let initial_windows = run_deadline_stage(deadline, || {
        window_discovery::discover_windows()
            .map_err(|error| format!("initial window discovery failed: {error}"))
    })?;

    let mut selected = Vec::with_capacity(window_ids.len());

    // First pass: validate and collect every requested window.
    for id in window_ids {
        let window = find_unique_window(&initial_windows, *id)?;

        if window.pid == 0 {
            return Err(format!("Window {id} has an invalid PID"));
        }

        if window.title.as_deref().is_none_or(str::is_empty) {
            return Err(format!("Window {id} has no usable title"));
        }

        if !valid_bounds(&window.bounds) {
            return Err(format!("Window {id} has invalid geometry"));
        }

        selected.push(window);
    }

    // Second pass: inspect each PID once, using all requested titles
    // belonging to that process.
    let mut accessibility_by_pid =
        HashMap::<u32, Vec<window_accessibility::AccessibilityWindow>>::new();

    for window in &selected {
        if accessibility_by_pid.contains_key(&window.pid) {
            continue;
        }

        let expected_titles: Vec<String> = selected
            .iter()
            .filter(|candidate| candidate.pid == window.pid)
            .filter_map(|candidate| candidate.title.clone())
            .collect();

        let candidates = run_deadline_stage(deadline, || {
            window_accessibility::inspect_application_windows_for_titles_until(
                window.pid,
                &expected_titles,
                deadline,
            )
            .map_err(|error| {
                format!(
                    "Accessibility inspection failed for PID {}: {error}",
                    window.pid
                )
            })
        })?;

        accessibility_by_pid.insert(window.pid, candidates);
    }

    // Third pass: retain the existing exact title + geometry matching.
    let mut accessibility_indices = Vec::with_capacity(selected.len());

    for window in &selected {
        let candidates = accessibility_by_pid
            .get(&window.pid)
            .ok_or_else(|| "AX inspection cache entry unexpectedly missing".to_owned())?;

        let ax_index = run_deadline_stage(deadline, || {
            window_accessibility::match_accessibility_window(window, candidates).map_err(|error| {
                format!(
                    "Accessibility identity match failed for Window {}: {error}",
                    window.window_id
                )
            })
        })?;

        accessibility_indices.push(ax_index);
    }

    // Re-discover all selected windows before producing a report.
    // A changed or replaced window invalidates the entire plan.
    let final_windows = run_deadline_stage(deadline, || {
        window_discovery::discover_windows()
            .map_err(|error| format!("final window discovery failed: {error}"))
    })?;

    let mut entries = Vec::with_capacity(selected.len());

    for (index, original) in selected.iter().enumerate() {
        check_deadline(deadline)?;
        let current = find_unique_window(&final_windows, original.window_id)?;

        run_deadline_stage(deadline, || verify_window_identity(original, &current))?;

        let placement = plan
            .placements
            .get(index)
            .ok_or_else(|| "layout planner returned an incomplete plan".to_owned())?;

        if placement.index != index {
            return Err(format!(
                "layout planner returned placement index {} for window position {}",
                placement.index, index,
            ));
        }

        entries.push(WindowDryRunEntry {
            window_id: original.window_id,
            pid: original.pid,
            owner_name: original.owner_name.clone().unwrap_or_default(),
            title: original.title.clone().unwrap_or_default(),
            current_bounds: current.bounds,
            target_bounds: placement.bounds.clone(),
            accessibility_index: accessibility_indices[index],
        });
    }

    check_deadline(deadline)?;

    Ok(WindowDryRunReport {
        display_id: plan.display_id,
        direction,
        entries,
    })
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

    fn discovered_window(
        window_id: u32,
        pid: u32,
        owner_name: Option<&str>,
        title: Option<&str>,
        bounds: WindowBounds,
    ) -> DiscoveredWindow {
        DiscoveredWindow {
            window_id,
            pid,
            owner_name: owner_name.map(str::to_owned),
            title: title.map(str::to_owned),
            bounds,
        }
    }

    #[test]
    fn finds_exactly_one_window() {
        let windows = vec![
            discovered_window(
                123,
                1000,
                Some("Terminal"),
                Some("First"),
                bounds(0.0, 0.0, 800.0, 600.0),
            ),
            discovered_window(
                456,
                2000,
                Some("Finder"),
                Some("Second"),
                bounds(20.0, 20.0, 800.0, 600.0),
            ),
        ];

        let result = find_unique_window(&windows, 456).unwrap();
        assert_eq!(result.window_id, 456);
        assert_eq!(result.pid, 2000);
    }

    #[test]
    fn rejects_missing_window() {
        let windows = vec![discovered_window(
            123,
            1000,
            Some("Terminal"),
            Some("First"),
            bounds(0.0, 0.0, 800.0, 600.0),
        )];

        let error = find_unique_window(&windows, 456).unwrap_err();
        assert!(error.contains("was not found"));
    }

    #[test]
    fn rejects_duplicate_discovered_windows() {
        let window = discovered_window(
            123,
            1000,
            Some("Terminal"),
            Some("First"),
            bounds(0.0, 0.0, 800.0, 600.0),
        );
        let windows = vec![window.clone(), window];

        let error = find_unique_window(&windows, 123).unwrap_err();
        assert!(error.contains("appeared 2 times"));
    }

    #[test]
    fn accepts_unchanged_window_identity() {
        let original = discovered_window(
            123,
            1000,
            Some("Terminal"),
            Some("First"),
            bounds(100.0, 80.0, 800.0, 600.0),
        );
        let current = discovered_window(
            123,
            1000,
            Some("Terminal"),
            Some("First"),
            bounds(101.0, 80.0, 800.0, 600.0),
        );

        assert!(verify_window_identity(&original, &current).is_ok());
    }

    #[test]
    fn rejects_pid_change() {
        let original = discovered_window(
            123,
            1000,
            Some("Terminal"),
            Some("First"),
            bounds(100.0, 80.0, 800.0, 600.0),
        );
        let current = discovered_window(
            123,
            2000,
            Some("Terminal"),
            Some("First"),
            bounds(100.0, 80.0, 800.0, 600.0),
        );

        assert!(verify_window_identity(&original, &current)
            .unwrap_err()
            .contains("PID changed"));
    }

    #[test]
    fn rejects_title_change() {
        let original = discovered_window(
            123,
            1000,
            Some("Terminal"),
            Some("First"),
            bounds(100.0, 80.0, 800.0, 600.0),
        );
        let current = discovered_window(
            123,
            1000,
            Some("Terminal"),
            Some("Changed"),
            bounds(100.0, 80.0, 800.0, 600.0),
        );

        assert!(verify_window_identity(&original, &current)
            .unwrap_err()
            .contains("title changed"));
    }

    #[test]
    fn rejects_owner_change() {
        let original = discovered_window(
            123,
            1000,
            Some("Terminal"),
            Some("First"),
            bounds(100.0, 80.0, 800.0, 600.0),
        );
        let current = discovered_window(
            123,
            1000,
            Some("Finder"),
            Some("First"),
            bounds(100.0, 80.0, 800.0, 600.0),
        );

        assert!(verify_window_identity(&original, &current)
            .unwrap_err()
            .contains("owner changed"));
    }

    #[test]
    fn rejects_window_id_change() {
        let original = discovered_window(
            123,
            1000,
            Some("Terminal"),
            Some("First"),
            bounds(100.0, 80.0, 800.0, 600.0),
        );
        let current = discovered_window(
            456,
            1000,
            Some("Terminal"),
            Some("First"),
            bounds(100.0, 80.0, 800.0, 600.0),
        );

        assert!(verify_window_identity(&original, &current)
            .unwrap_err()
            .contains("Window ID changed"));
    }

    #[test]
    fn rejects_geometry_change_during_identity_check() {
        let original = discovered_window(
            123,
            1000,
            Some("Terminal"),
            Some("First"),
            bounds(100.0, 80.0, 800.0, 600.0),
        );
        let current = discovered_window(
            123,
            1000,
            Some("Terminal"),
            Some("First"),
            bounds(104.0, 80.0, 800.0, 600.0),
        );

        assert!(verify_window_identity(&original, &current)
            .unwrap_err()
            .contains("geometry changed"));
    }

    #[test]
    fn accepts_valid_geometry() {
        assert!(valid_bounds(&bounds(0.0, 0.0, 800.0, 600.0)));
    }

    #[test]
    fn rejects_zero_or_negative_dimensions() {
        assert!(!valid_bounds(&bounds(0.0, 0.0, 0.0, 600.0)));
        assert!(!valid_bounds(&bounds(0.0, 0.0, 800.0, -1.0)));
    }

    #[test]
    fn rejects_non_finite_geometry() {
        assert!(!valid_bounds(&bounds(f64::NAN, 0.0, 800.0, 600.0)));
        assert!(!valid_bounds(&bounds(0.0, 0.0, f64::INFINITY, 600.0)));
    }

    #[test]
    fn accepts_geometry_within_two_points() {
        assert!(geometry_stable(
            &bounds(101.5, 80.0, 800.0, 600.0),
            &bounds(100.0, 80.0, 800.0, 600.0),
        ));
    }

    #[test]
    fn rejects_geometry_outside_two_points() {
        assert!(!geometry_stable(
            &bounds(102.1, 80.0, 800.0, 600.0),
            &bounds(100.0, 80.0, 800.0, 600.0),
        ));
    }

    #[test]
    fn rejects_duplicate_window_ids() {
        let result = build_window_dry_run(
            &[123, 123],
            LayoutDirection::Horizontal,
            None,
            LayoutConstraints::default(),
        );

        assert!(result.unwrap_err().contains("duplicate Window ID"));
    }

    #[test]
    fn requires_at_least_two_windows() {
        let result = build_window_dry_run(
            &[123],
            LayoutDirection::Horizontal,
            None,
            LayoutConstraints::default(),
        );

        assert!(result.unwrap_err().contains("at least two"));
    }

    #[test]
    fn expired_deadline_is_rejected_immediately() {
        let deadline = std::time::Instant::now() - std::time::Duration::from_secs(1);

        assert_eq!(
            check_deadline(Some(deadline)),
            Err("dry-run execution deadline exceeded".to_owned())
        );
    }

    #[test]
    fn unexpired_deadline_passes() {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);

        assert_eq!(check_deadline(Some(deadline)), Ok(()));
    }

    #[test]
    fn deadline_after_ax_inspection_prevents_final_discovery() {
        let mut check_count = 0;
        let mut check_deadline = || {
            check_count += 1;
            if check_count == 2 {
                Err("dry-run execution deadline exceeded".to_owned())
            } else {
                Ok(())
            }
        };
        let mut ax_inspection_returned = false;
        let mut final_discovery_called = false;

        let result = (|| {
            run_deadline_stage_with_check(&mut check_deadline, || {
                ax_inspection_returned = true;
                Ok(())
            })?;
            run_deadline_stage_with_check(&mut check_deadline, || {
                final_discovery_called = true;
                Ok(())
            })?;
            Ok::<(), String>(())
        })();

        assert!(result.unwrap_err().contains("deadline exceeded"));
        assert!(ax_inspection_returned);
        assert!(!final_discovery_called);
    }
}

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

pub fn build_window_dry_run(
    window_ids: &[u32],
    direction: LayoutDirection,
    display_id: Option<u32>,
    constraints: LayoutConstraints,
) -> Result<WindowDryRunReport, String> {
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

    let displays = display_geometry::discover_displays()
        .map_err(|error| format!("display discovery failed: {error}"))?;

    let plan: WindowLayoutPlan = match display_id {
        Some(id) => window_layout::plan_on_display_id(
            &displays,
            id,
            window_ids.len(),
            direction,
            constraints,
        ),
        None => {
            window_layout::plan_on_main_display(&displays, window_ids.len(), direction, constraints)
        }
    }
    .map_err(|error| format!("layout planning failed: {error}"))?;

    let initial_windows = window_discovery::discover_windows()
        .map_err(|error| format!("initial window discovery failed: {error}"))?;

    let mut selected = Vec::with_capacity(window_ids.len());
    let mut accessibility_indices = Vec::with_capacity(window_ids.len());

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

        let candidates =
            window_accessibility::inspect_application_windows(window.pid).map_err(|error| {
                format!(
                    "Accessibility inspection failed for Window {id}, PID {}: {error}",
                    window.pid
                )
            })?;

        let ax_index = window_accessibility::match_accessibility_window(&window, &candidates)
            .map_err(|error| {
                format!("Accessibility identity match failed for Window {id}: {error}")
            })?;

        selected.push(window);
        accessibility_indices.push(ax_index);
    }

    // Re-discover all selected windows before producing a report.
    // A changed or replaced window invalidates the entire plan.
    let final_windows = window_discovery::discover_windows()
        .map_err(|error| format!("final window discovery failed: {error}"))?;

    let mut entries = Vec::with_capacity(selected.len());

    for (index, original) in selected.iter().enumerate() {
        let current = find_unique_window(&final_windows, original.window_id)?;

        verify_window_identity(original, &current)?;

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

        assert!(
            verify_window_identity(&original, &current)
                .unwrap_err()
                .contains("PID changed")
        );
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

        assert!(
            verify_window_identity(&original, &current)
                .unwrap_err()
                .contains("title changed")
        );
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

        assert!(
            verify_window_identity(&original, &current)
                .unwrap_err()
                .contains("owner changed")
        );
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

        assert!(
            verify_window_identity(&original, &current)
                .unwrap_err()
                .contains("Window ID changed")
        );
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

        assert!(
            verify_window_identity(&original, &current)
                .unwrap_err()
                .contains("geometry changed")
        );
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
}

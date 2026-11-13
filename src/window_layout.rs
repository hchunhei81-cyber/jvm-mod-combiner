use std::error::Error;
use std::fmt;

use crate::display_geometry::DisplayGeometry;
use crate::window_discovery::WindowBounds;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutDirection {
    Horizontal,
    Vertical,
}

impl fmt::Display for LayoutDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Horizontal => write!(f, "horizontal"),
            Self::Vertical => write!(f, "vertical"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutConstraints {
    pub margin_x: f64,
    pub margin_y: f64,
    pub gap: f64,
    pub min_width: f64,
    pub min_height: f64,
}

impl Default for LayoutConstraints {
    fn default() -> Self {
        Self {
            margin_x: 24.0,
            margin_y: 24.0,
            gap: 16.0,
            min_width: 480.0,
            min_height: 320.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowPlacement {
    pub index: usize,
    pub bounds: WindowBounds,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowLayoutPlan {
    pub display_id: u32,
    pub direction: LayoutDirection,
    pub placements: Vec<WindowPlacement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutError {
    NoDisplays,
    NoMainDisplay,
    MultipleMainDisplays(usize),
    DisplayNotFound(u32),
    InvalidDisplayId(u32),
    InvalidDisplayBounds,
    InvalidConstraints,
    ZeroWindows,
    InsufficientDisplaySpace,
    CalculationOverflow,
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDisplays => write!(f, "no displays are available"),
            Self::NoMainDisplay => write!(f, "no main display was found"),
            Self::MultipleMainDisplays(count) => {
                write!(f, "expected one main display, found {count}")
            }
            Self::DisplayNotFound(id) => write!(f, "display {id} was not found"),
            Self::InvalidDisplayId(id) => write!(f, "display ID {id} is invalid"),
            Self::InvalidDisplayBounds => write!(f, "display bounds are invalid"),
            Self::InvalidConstraints => write!(f, "layout constraints are invalid"),
            Self::ZeroWindows => write!(f, "window count must be greater than zero"),
            Self::InsufficientDisplaySpace => {
                write!(
                    f,
                    "display space cannot satisfy the requested minimum window size"
                )
            }
            Self::CalculationOverflow => write!(f, "layout calculation overflowed"),
        }
    }
}

impl Error for LayoutError {}

fn validate_constraints(c: LayoutConstraints) -> Result<(), LayoutError> {
    let values = [c.margin_x, c.margin_y, c.gap, c.min_width, c.min_height];

    if values.iter().any(|v| !v.is_finite())
        || c.margin_x < 0.0
        || c.margin_y < 0.0
        || c.gap < 0.0
        || c.min_width <= 0.0
        || c.min_height <= 0.0
    {
        return Err(LayoutError::InvalidConstraints);
    }

    Ok(())
}

fn validate_display(display: &DisplayGeometry) -> Result<(), LayoutError> {
    let b = &display.bounds;
    let right = b.x + b.width;
    let bottom = b.y + b.height;

    if display.display_id == 0 {
        return Err(LayoutError::InvalidDisplayId(display.display_id));
    }

    if !b.x.is_finite()
        || !b.y.is_finite()
        || !b.width.is_finite()
        || !b.height.is_finite()
        || !right.is_finite()
        || !bottom.is_finite()
        || b.width <= 0.0
        || b.height <= 0.0
    {
        return Err(LayoutError::InvalidDisplayBounds);
    }

    Ok(())
}

pub fn plan_layout(
    display: &DisplayGeometry,
    window_count: usize,
    direction: LayoutDirection,
    constraints: LayoutConstraints,
) -> Result<WindowLayoutPlan, LayoutError> {
    if window_count == 0 {
        return Err(LayoutError::ZeroWindows);
    }

    validate_display(display)?;
    validate_constraints(constraints)?;

    let b = &display.bounds;
    let c = constraints;

    let count = window_count as f64;
    if !count.is_finite() {
        return Err(LayoutError::CalculationOverflow);
    }

    let horizontal_margins = c.margin_x * 2.0;
    let vertical_margins = c.margin_y * 2.0;
    let total_gaps = c.gap * (count - 1.0);

    if !horizontal_margins.is_finite() || !vertical_margins.is_finite() || !total_gaps.is_finite() {
        return Err(LayoutError::CalculationOverflow);
    }

    let usable_width = b.width - horizontal_margins;
    let usable_height = b.height - vertical_margins;

    if !usable_width.is_finite() || !usable_height.is_finite() {
        return Err(LayoutError::CalculationOverflow);
    }

    let (slot_width, slot_height) = match direction {
        LayoutDirection::Horizontal => {
            let distributable = usable_width - total_gaps;
            if !distributable.is_finite() {
                return Err(LayoutError::CalculationOverflow);
            }
            (distributable / count, usable_height)
        }
        LayoutDirection::Vertical => {
            let distributable = usable_height - total_gaps;
            if !distributable.is_finite() {
                return Err(LayoutError::CalculationOverflow);
            }
            (usable_width, distributable / count)
        }
    };

    if !slot_width.is_finite() || !slot_height.is_finite() {
        return Err(LayoutError::CalculationOverflow);
    }

    if slot_width < c.min_width || slot_height < c.min_height {
        return Err(LayoutError::InsufficientDisplaySpace);
    }

    let mut placements = Vec::with_capacity(window_count);

    for index in 0..window_count {
        let offset = index as f64;
        let (x, y) = match direction {
            LayoutDirection::Horizontal => (
                b.x + c.margin_x + offset * (slot_width + c.gap),
                b.y + c.margin_y,
            ),
            LayoutDirection::Vertical => (
                b.x + c.margin_x,
                b.y + c.margin_y + offset * (slot_height + c.gap),
            ),
        };

        let right = x + slot_width;
        let bottom = y + slot_height;

        if !x.is_finite() || !y.is_finite() || !right.is_finite() || !bottom.is_finite() {
            return Err(LayoutError::CalculationOverflow);
        }

        placements.push(WindowPlacement {
            index,
            bounds: WindowBounds {
                x,
                y,
                width: slot_width,
                height: slot_height,
            },
        });
    }

    Ok(WindowLayoutPlan {
        display_id: display.display_id,
        direction,
        placements,
    })
}

pub fn plan_on_main_display(
    displays: &[DisplayGeometry],
    window_count: usize,
    direction: LayoutDirection,
    constraints: LayoutConstraints,
) -> Result<WindowLayoutPlan, LayoutError> {
    if displays.is_empty() {
        return Err(LayoutError::NoDisplays);
    }

    let main: Vec<&DisplayGeometry> = displays.iter().filter(|display| display.is_main).collect();

    match main.as_slice() {
        [] => Err(LayoutError::NoMainDisplay),
        [display] => plan_layout(display, window_count, direction, constraints),
        _ => Err(LayoutError::MultipleMainDisplays(main.len())),
    }
}

pub fn plan_on_display_id(
    displays: &[DisplayGeometry],
    display_id: u32,
    window_count: usize,
    direction: LayoutDirection,
    constraints: LayoutConstraints,
) -> Result<WindowLayoutPlan, LayoutError> {
    if displays.is_empty() {
        return Err(LayoutError::NoDisplays);
    }

    let display = displays
        .iter()
        .find(|display| display.display_id == display_id)
        .ok_or(LayoutError::DisplayNotFound(display_id))?;

    plan_layout(display, window_count, direction, constraints)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window_discovery::WindowBounds;

    fn display(id: u32, main: bool, x: f64, y: f64, width: f64, height: f64) -> DisplayGeometry {
        DisplayGeometry {
            display_id: id,
            bounds: WindowBounds {
                x,
                y,
                width,
                height,
            },
            pixel_width: 1000,
            pixel_height: 800,
            cg_pixel_width: 1000,
            cg_pixel_height: 800,
            scale_x: 1.0,
            scale_y: 1.0,
            is_main: main,
            is_builtin: main,
            rotation_degrees: 0.0,
        }
    }

    fn constraints() -> LayoutConstraints {
        LayoutConstraints {
            margin_x: 10.0,
            margin_y: 10.0,
            gap: 20.0,
            min_width: 100.0,
            min_height: 100.0,
        }
    }

    #[test]
    fn horizontal_two_window_layout() {
        let d = display(1, true, 0.0, 0.0, 1000.0, 800.0);
        let plan = plan_layout(&d, 2, LayoutDirection::Horizontal, constraints()).unwrap();

        assert_eq!(plan.placements.len(), 2);
        assert_eq!(plan.placements[0].bounds.x, 10.0);
        assert_eq!(plan.placements[0].bounds.width, 480.0);
        assert_eq!(plan.placements[1].bounds.x, 510.0);
        assert_eq!(plan.placements[1].bounds.height, 780.0);
    }

    #[test]
    fn vertical_two_window_layout() {
        let d = display(1, true, 0.0, 0.0, 1000.0, 800.0);
        let plan = plan_layout(&d, 2, LayoutDirection::Vertical, constraints()).unwrap();

        assert_eq!(plan.placements[0].bounds.y, 10.0);
        assert_eq!(plan.placements[0].bounds.height, 380.0);
        assert_eq!(plan.placements[1].bounds.y, 410.0);
        assert_eq!(plan.placements[1].bounds.width, 980.0);
    }

    #[test]
    fn supports_more_than_two_windows() {
        let d = display(1, true, 0.0, 0.0, 1600.0, 900.0);
        let plan = plan_layout(&d, 3, LayoutDirection::Horizontal, constraints()).unwrap();

        assert_eq!(plan.placements.len(), 3);
        assert_eq!(plan.placements[0].index, 0);
        assert_eq!(plan.placements[1].index, 1);
        assert_eq!(plan.placements[2].index, 2);
        assert_eq!(plan.placements[0].bounds.width, 513.3333333333334);
    }

    #[test]
    fn supports_negative_global_display_coordinates() {
        let d = display(2, true, -1600.0, -200.0, 1600.0, 900.0);
        let plan = plan_layout(&d, 2, LayoutDirection::Horizontal, constraints()).unwrap();

        assert_eq!(plan.placements[0].bounds.x, -1590.0);
        assert_eq!(plan.placements[0].bounds.y, -190.0);
    }

    #[test]
    fn main_display_is_selected_by_default() {
        let displays = vec![
            display(2, false, -1000.0, 0.0, 1000.0, 800.0),
            display(1, true, 0.0, 0.0, 1200.0, 800.0),
        ];

        let plan =
            plan_on_main_display(&displays, 2, LayoutDirection::Horizontal, constraints()).unwrap();

        assert_eq!(plan.display_id, 1);
    }

    #[test]
    fn can_select_non_main_display_by_id() {
        let displays = vec![
            display(1, true, 0.0, 0.0, 1200.0, 800.0),
            display(2, false, -1000.0, 0.0, 1000.0, 800.0),
        ];

        let plan = plan_on_display_id(&displays, 2, 2, LayoutDirection::Horizontal, constraints())
            .unwrap();

        assert_eq!(plan.display_id, 2);
        assert_eq!(plan.placements[0].bounds.x, -990.0);
    }

    #[test]
    fn rejects_zero_windows() {
        let d = display(1, true, 0.0, 0.0, 1000.0, 800.0);
        assert_eq!(
            plan_layout(&d, 0, LayoutDirection::Horizontal, constraints()),
            Err(LayoutError::ZeroWindows)
        );
    }

    #[test]
    fn rejects_insufficient_width() {
        let d = display(1, true, 0.0, 0.0, 200.0, 800.0);
        assert_eq!(
            plan_layout(&d, 2, LayoutDirection::Horizontal, constraints()),
            Err(LayoutError::InsufficientDisplaySpace)
        );
    }

    #[test]
    fn rejects_insufficient_height() {
        let d = display(1, true, 0.0, 0.0, 1000.0, 200.0);
        assert_eq!(
            plan_layout(&d, 2, LayoutDirection::Vertical, constraints()),
            Err(LayoutError::InsufficientDisplaySpace)
        );
    }

    #[test]
    fn rejects_non_finite_constraints() {
        let d = display(1, true, 0.0, 0.0, 1000.0, 800.0);
        let mut c = constraints();
        c.gap = f64::NAN;

        assert_eq!(
            plan_layout(&d, 2, LayoutDirection::Horizontal, c),
            Err(LayoutError::InvalidConstraints)
        );
    }

    #[test]
    fn rejects_negative_margins() {
        let d = display(1, true, 0.0, 0.0, 1000.0, 800.0);
        let mut c = constraints();
        c.margin_x = -1.0;

        assert_eq!(
            plan_layout(&d, 2, LayoutDirection::Horizontal, c),
            Err(LayoutError::InvalidConstraints)
        );
    }

    #[test]
    fn rejects_invalid_display_geometry() {
        let d = display(1, true, 0.0, 0.0, f64::INFINITY, 800.0);
        assert_eq!(
            plan_layout(&d, 2, LayoutDirection::Horizontal, constraints()),
            Err(LayoutError::InvalidDisplayBounds)
        );
    }

    #[test]
    fn rejects_zero_display_id() {
        let d = display(0, true, 0.0, 0.0, 1000.0, 800.0);
        assert_eq!(
            plan_layout(&d, 2, LayoutDirection::Horizontal, constraints()),
            Err(LayoutError::InvalidDisplayId(0))
        );
    }

    #[test]
    fn rejects_overflowing_display_edges() {
        let d = display(1, true, f64::MAX, 0.0, f64::MAX, 800.0);
        assert_eq!(
            plan_layout(&d, 2, LayoutDirection::Horizontal, constraints()),
            Err(LayoutError::InvalidDisplayBounds)
        );
    }

    #[test]
    fn rejects_missing_main_display() {
        let displays = vec![display(1, false, 0.0, 0.0, 1000.0, 800.0)];
        assert_eq!(
            plan_on_main_display(&displays, 2, LayoutDirection::Horizontal, constraints()),
            Err(LayoutError::NoMainDisplay)
        );
    }

    #[test]
    fn rejects_multiple_main_displays() {
        let displays = vec![
            display(1, true, 0.0, 0.0, 1000.0, 800.0),
            display(2, true, 1000.0, 0.0, 1000.0, 800.0),
        ];
        assert_eq!(
            plan_on_main_display(&displays, 2, LayoutDirection::Horizontal, constraints()),
            Err(LayoutError::MultipleMainDisplays(2))
        );
    }

    #[test]
    fn rejects_unknown_display_id() {
        let displays = vec![display(1, true, 0.0, 0.0, 1000.0, 800.0)];
        assert_eq!(
            plan_on_display_id(&displays, 99, 2, LayoutDirection::Horizontal, constraints()),
            Err(LayoutError::DisplayNotFound(99))
        );
    }

    #[test]
    fn rejects_empty_display_list() {
        assert_eq!(
            plan_on_main_display(&[], 2, LayoutDirection::Horizontal, constraints()),
            Err(LayoutError::NoDisplays)
        );
    }

    #[test]
    fn preserves_input_order() {
        let d = display(1, true, 0.0, 0.0, 1600.0, 900.0);
        let plan = plan_layout(&d, 3, LayoutDirection::Vertical, constraints()).unwrap();

        assert_eq!(
            plan.placements.iter().map(|p| p.index).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }
}

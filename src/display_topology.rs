use std::error::Error;
use std::fmt;

use crate::display_geometry::DisplayGeometry;
use crate::window_discovery::WindowBounds;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelativePosition {
    Left,
    Right,
    Above,
    Below,
    Diagonal,
    Overlapping,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DisplayPairRelation {
    pub first_id: u32,
    pub second_id: u32,

    // Signed logical-coordinate gaps.
    // Positive means separated; negative means the second interval
    // lies before the first interval; zero means touching or overlapping.
    pub horizontal_gap: f64,
    pub vertical_gap: f64,

    pub overlap_width: f64,
    pub overlap_height: f64,

    pub relative_position: RelativePosition,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DisplayOffset {
    pub display_id: u32,
    pub offset_x: f64,
    pub offset_y: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DisplayTopology {
    pub main_display_id: u32,
    pub relations: Vec<DisplayPairRelation>,
    pub virtual_bounds: WindowBounds,
    pub main_relative_offsets: Vec<DisplayOffset>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopologyError {
    NoDisplays,
    DuplicateDisplayId(u32),
    InvalidDisplayGeometry(u32),
    InvalidMainDisplayCount(usize),
    VirtualBoundsOverflow,
}

impl fmt::Display for TopologyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDisplays => write!(f, "no displays supplied"),
            Self::DuplicateDisplayId(id) => {
                write!(f, "duplicate display ID: {id}")
            }
            Self::InvalidDisplayGeometry(id) => {
                write!(f, "display {id} has invalid geometry or scale")
            }
            Self::InvalidMainDisplayCount(count) => {
                write!(f, "expected exactly one main display, found {count}")
            }
            Self::VirtualBoundsOverflow => {
                write!(f, "virtual desktop bounds overflowed")
            }
        }
    }
}

impl Error for TopologyError {}

fn interval_gap(first_start: f64, first_size: f64, second_start: f64, second_size: f64) -> f64 {
    let first_end = first_start + first_size;
    let second_end = second_start + second_size;

    if second_start >= first_end {
        second_start - first_end
    } else if second_end <= first_start {
        second_end - first_start
    } else {
        0.0
    }
}

fn interval_overlap(first_start: f64, first_size: f64, second_start: f64, second_size: f64) -> f64 {
    let first_end = first_start + first_size;
    let second_end = second_start + second_size;

    (first_end.min(second_end) - first_start.max(second_start)).max(0.0)
}

fn relative_position(
    first: &DisplayGeometry,
    second: &DisplayGeometry,
    overlap_width: f64,
    overlap_height: f64,
) -> RelativePosition {
    if overlap_width > 0.0 && overlap_height > 0.0 {
        return RelativePosition::Overlapping;
    }

    let first_center_x = first.bounds.x + first.bounds.width / 2.0;
    let second_center_x = second.bounds.x + second.bounds.width / 2.0;
    let first_center_y = first.bounds.y + first.bounds.height / 2.0;
    let second_center_y = second.bounds.y + second.bounds.height / 2.0;

    if overlap_width == 0.0 && overlap_height > 0.0 {
        return if second_center_x < first_center_x {
            RelativePosition::Left
        } else {
            RelativePosition::Right
        };
    }

    if overlap_width > 0.0 && overlap_height == 0.0 {
        return if second_center_y < first_center_y {
            RelativePosition::Above
        } else {
            RelativePosition::Below
        };
    }

    RelativePosition::Diagonal
}

fn validate_display(display: &DisplayGeometry) -> Result<(), TopologyError> {
    let bounds = &display.bounds;

    let right = bounds.x + bounds.width;
    let bottom = bounds.y + bounds.height;

    if !bounds.x.is_finite()
        || !bounds.y.is_finite()
        || !bounds.width.is_finite()
        || !bounds.height.is_finite()
        || !right.is_finite()
        || !bottom.is_finite()
        || bounds.width <= 0.0
        || bounds.height <= 0.0
        || display.pixel_width == 0
        || display.pixel_height == 0
        || !display.scale_x.is_finite()
        || !display.scale_y.is_finite()
        || display.scale_x <= 0.0
        || display.scale_y <= 0.0
        || !display.rotation_degrees.is_finite()
    {
        return Err(TopologyError::InvalidDisplayGeometry(display.display_id));
    }

    Ok(())
}

/// Calculate the union of all display bounds in global logical coordinates.
fn calculate_virtual_bounds(displays: &[&DisplayGeometry]) -> Result<WindowBounds, TopologyError> {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;

    for display in displays {
        let bounds = &display.bounds;
        min_x = min_x.min(bounds.x);
        min_y = min_y.min(bounds.y);
        max_x = max_x.max(bounds.x + bounds.width);
        max_y = max_y.max(bounds.y + bounds.height);
    }

    let width = max_x - min_x;
    let height = max_y - min_y;

    if !min_x.is_finite()
        || !min_y.is_finite()
        || !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
    {
        return Err(TopologyError::VirtualBoundsOverflow);
    }

    Ok(WindowBounds {
        x: min_x,
        y: min_y,
        width,
        height,
    })
}

/// Calculate each display's origin relative to the main display's origin.
fn calculate_main_relative_offsets(
    displays: &[&DisplayGeometry],
    main_display: &DisplayGeometry,
) -> Vec<DisplayOffset> {
    displays
        .iter()
        .map(|display| DisplayOffset {
            display_id: display.display_id,
            offset_x: display.bounds.x - main_display.bounds.x,
            offset_y: display.bounds.y - main_display.bounds.y,
        })
        .collect()
}

/// Analyze display relationships using global logical coordinates.
///
/// Negative coordinates are valid. Pixel dimensions and per-display scale
/// values are validated but are not mixed into logical-coordinate geometry.
pub fn analyze_displays(displays: &[DisplayGeometry]) -> Result<DisplayTopology, TopologyError> {
    if displays.is_empty() {
        return Err(TopologyError::NoDisplays);
    }

    let mut ordered: Vec<&DisplayGeometry> = displays.iter().collect();
    ordered.sort_by_key(|display| display.display_id);

    for (index, display) in ordered.iter().enumerate() {
        if index > 0 && ordered[index - 1].display_id == display.display_id {
            return Err(TopologyError::DuplicateDisplayId(display.display_id));
        }

        validate_display(display)?;
    }

    let main_displays: Vec<&DisplayGeometry> = ordered
        .iter()
        .copied()
        .filter(|display| display.is_main)
        .collect();

    if main_displays.len() != 1 {
        return Err(TopologyError::InvalidMainDisplayCount(main_displays.len()));
    }

    let main_display = main_displays[0];
    let main_display_id = main_display.display_id;
    let virtual_bounds = calculate_virtual_bounds(&ordered)?;
    let main_relative_offsets = calculate_main_relative_offsets(&ordered, main_display);
    let mut relations = Vec::new();

    for first_index in 0..ordered.len() {
        for second_index in (first_index + 1)..ordered.len() {
            let first = ordered[first_index];
            let second = ordered[second_index];

            let horizontal_gap = interval_gap(
                first.bounds.x,
                first.bounds.width,
                second.bounds.x,
                second.bounds.width,
            );

            let vertical_gap = interval_gap(
                first.bounds.y,
                first.bounds.height,
                second.bounds.y,
                second.bounds.height,
            );

            let overlap_width = interval_overlap(
                first.bounds.x,
                first.bounds.width,
                second.bounds.x,
                second.bounds.width,
            );

            let overlap_height = interval_overlap(
                first.bounds.y,
                first.bounds.height,
                second.bounds.y,
                second.bounds.height,
            );

            relations.push(DisplayPairRelation {
                first_id: first.display_id,
                second_id: second.display_id,
                horizontal_gap,
                vertical_gap,
                overlap_width,
                overlap_height,
                relative_position: relative_position(first, second, overlap_width, overlap_height),
            });
        }
    }

    Ok(DisplayTopology {
        main_display_id,
        relations,
        virtual_bounds,
        main_relative_offsets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window_discovery::WindowBounds;

    fn display(
        id: u32,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        scale_x: f64,
        scale_y: f64,
        is_main: bool,
    ) -> DisplayGeometry {
        DisplayGeometry {
            display_id: id,
            bounds: WindowBounds {
                x,
                y,
                width,
                height,
            },
            pixel_width: (width * scale_x) as usize,
            pixel_height: (height * scale_y) as usize,
            cg_pixel_width: width as usize,
            cg_pixel_height: height as usize,
            scale_x,
            scale_y,
            is_main,
            is_builtin: is_main,
            rotation_degrees: 0.0,
        }
    }

    #[test]
    fn single_display_has_no_pair_relations() {
        let displays = [display(1, 0.0, 0.0, 1470.0, 956.0, 2.0, 2.0, true)];

        let topology = analyze_displays(&displays).unwrap();

        assert_eq!(topology.main_display_id, 1);
        assert!(topology.relations.is_empty());
    }

    #[test]
    fn detects_display_to_the_right() {
        let displays = [
            display(1, 0.0, 0.0, 1000.0, 800.0, 2.0, 2.0, true),
            display(2, 1000.0, 0.0, 1920.0, 1080.0, 1.0, 1.0, false),
        ];

        let topology = analyze_displays(&displays).unwrap();
        let relation = &topology.relations[0];

        assert_eq!(relation.relative_position, RelativePosition::Right);
        assert_eq!(relation.horizontal_gap, 0.0);
        assert_eq!(relation.overlap_height, 800.0);
    }

    #[test]
    fn detects_display_to_the_left_with_negative_coordinates() {
        let displays = [
            display(1, 0.0, 0.0, 1000.0, 800.0, 2.0, 2.0, true),
            display(2, -1920.0, 0.0, 1920.0, 1080.0, 1.0, 1.0, false),
        ];

        let topology = analyze_displays(&displays).unwrap();
        let relation = &topology.relations[0];

        assert_eq!(relation.relative_position, RelativePosition::Left);
        assert_eq!(relation.horizontal_gap, 0.0);
        assert_eq!(relation.overlap_height, 800.0);
    }

    #[test]
    fn detects_display_above() {
        let displays = [
            display(1, 0.0, 0.0, 1000.0, 800.0, 2.0, 2.0, true),
            display(2, 0.0, -900.0, 1200.0, 900.0, 1.0, 1.0, false),
        ];

        let topology = analyze_displays(&displays).unwrap();
        let relation = &topology.relations[0];

        assert_eq!(relation.relative_position, RelativePosition::Above);
        assert_eq!(relation.vertical_gap, 0.0);
        assert_eq!(relation.overlap_width, 1000.0);
    }

    #[test]
    fn signed_gaps_preserve_relative_direction() {
        let displays = [
            display(1, 0.0, 0.0, 100.0, 100.0, 2.0, 2.0, true),
            display(2, -150.0, 0.0, 100.0, 100.0, 1.0, 1.0, false),
            display(3, 0.0, -150.0, 100.0, 100.0, 1.0, 1.0, false),
            display(4, 150.0, 0.0, 100.0, 100.0, 1.0, 1.0, false),
            display(5, 0.0, 150.0, 100.0, 100.0, 1.0, 1.0, false),
        ];

        let topology = analyze_displays(&displays).unwrap();

        let relation = |second_id| {
            topology
                .relations
                .iter()
                .find(|item| item.first_id == 1 && item.second_id == second_id)
                .unwrap()
        };

        assert_eq!(relation(2).horizontal_gap, -50.0);
        assert_eq!(relation(3).vertical_gap, -50.0);
        assert_eq!(relation(4).horizontal_gap, 50.0);
        assert_eq!(relation(5).vertical_gap, 50.0);
    }

    #[test]
    fn detects_diagonal_separation() {
        let displays = [
            display(1, 0.0, 0.0, 100.0, 100.0, 2.0, 2.0, true),
            display(2, 150.0, 180.0, 100.0, 100.0, 1.0, 1.0, false),
        ];

        let topology = analyze_displays(&displays).unwrap();
        let relation = &topology.relations[0];

        assert_eq!(relation.relative_position, RelativePosition::Diagonal);
        assert_eq!(relation.horizontal_gap, 50.0);
        assert_eq!(relation.vertical_gap, 80.0);
    }

    #[test]
    fn detects_partial_overlap() {
        let displays = [
            display(1, 0.0, 0.0, 100.0, 100.0, 2.0, 2.0, true),
            display(2, 50.0, 40.0, 100.0, 100.0, 1.0, 1.0, false),
        ];

        let topology = analyze_displays(&displays).unwrap();
        let relation = &topology.relations[0];

        assert_eq!(relation.relative_position, RelativePosition::Overlapping);
        assert_eq!(relation.overlap_width, 50.0);
        assert_eq!(relation.overlap_height, 60.0);
    }

    #[test]
    fn mixed_scaling_does_not_change_logical_geometry() {
        let displays = [
            display(1, 0.0, 0.0, 1000.0, 800.0, 2.0, 2.0, true),
            display(2, 1000.0, 0.0, 1920.0, 1080.0, 1.0, 1.0, false),
        ];

        let topology = analyze_displays(&displays).unwrap();
        let relation = &topology.relations[0];

        assert_eq!(relation.horizontal_gap, 0.0);
        assert_eq!(relation.overlap_height, 800.0);
    }

    #[test]
    fn rejects_duplicate_display_ids() {
        let displays = [
            display(1, 0.0, 0.0, 100.0, 100.0, 2.0, 2.0, true),
            display(1, 100.0, 0.0, 100.0, 100.0, 1.0, 1.0, false),
        ];

        assert_eq!(
            analyze_displays(&displays),
            Err(TopologyError::DuplicateDisplayId(1))
        );
    }

    #[test]
    fn rejects_missing_main_display() {
        let displays = [display(1, 0.0, 0.0, 100.0, 100.0, 2.0, 2.0, false)];

        assert_eq!(
            analyze_displays(&displays),
            Err(TopologyError::InvalidMainDisplayCount(0))
        );
    }

    #[test]
    fn rejects_multiple_main_displays() {
        let displays = [
            display(1, 0.0, 0.0, 100.0, 100.0, 2.0, 2.0, true),
            display(2, 100.0, 0.0, 100.0, 100.0, 1.0, 1.0, true),
        ];

        assert_eq!(
            analyze_displays(&displays),
            Err(TopologyError::InvalidMainDisplayCount(2))
        );
    }

    #[test]
    fn rejects_non_finite_geometry() {
        let displays = [display(1, f64::NAN, 0.0, 100.0, 100.0, 2.0, 2.0, true)];

        assert_eq!(
            analyze_displays(&displays),
            Err(TopologyError::InvalidDisplayGeometry(1))
        );
    }

    #[test]
    fn calculates_virtual_bounds_with_negative_coordinates() {
        let displays = [
            display(1, 0.0, 0.0, 1470.0, 956.0, 2.0, 2.0, true),
            display(2, -1920.0, 0.0, 1920.0, 1080.0, 1.0, 1.0, false),
            display(3, 1470.0, -200.0, 1600.0, 900.0, 1.0, 1.0, false),
        ];

        let topology = analyze_displays(&displays).unwrap();

        assert_eq!(topology.virtual_bounds.x, -1920.0);
        assert_eq!(topology.virtual_bounds.y, -200.0);
        assert_eq!(topology.virtual_bounds.width, 4990.0);
        assert_eq!(topology.virtual_bounds.height, 1280.0);
    }

    #[test]
    fn calculates_offsets_relative_to_main_display() {
        let displays = [
            display(1, 100.0, 200.0, 1000.0, 800.0, 2.0, 2.0, true),
            display(2, -1820.0, 200.0, 1920.0, 1080.0, 1.0, 1.0, false),
            display(3, 100.0, -700.0, 1200.0, 900.0, 1.0, 1.0, false),
            display(4, 1100.0, 200.0, 1600.0, 900.0, 1.0, 1.0, false),
        ];

        let topology = analyze_displays(&displays).unwrap();

        let offset = |id| {
            topology
                .main_relative_offsets
                .iter()
                .find(|item| item.display_id == id)
                .unwrap()
        };

        assert_eq!((offset(1).offset_x, offset(1).offset_y), (0.0, 0.0));
        assert_eq!((offset(2).offset_x, offset(2).offset_y), (-1920.0, 0.0));
        assert_eq!((offset(3).offset_x, offset(3).offset_y), (0.0, -900.0));
        assert_eq!((offset(4).offset_x, offset(4).offset_y), (1000.0, 0.0));
    }

    #[test]
    fn single_display_virtual_bounds_match_display_bounds() {
        let displays = [display(1, 0.0, 0.0, 1470.0, 956.0, 2.0, 2.0, true)];

        let topology = analyze_displays(&displays).unwrap();

        assert_eq!(topology.virtual_bounds.x, 0.0);
        assert_eq!(topology.virtual_bounds.y, 0.0);
        assert_eq!(topology.virtual_bounds.width, 1470.0);
        assert_eq!(topology.virtual_bounds.height, 956.0);
        assert_eq!(topology.main_relative_offsets.len(), 1);
        assert_eq!(topology.main_relative_offsets[0].offset_x, 0.0);
        assert_eq!(topology.main_relative_offsets[0].offset_y, 0.0);
    }

    #[test]
    fn rejects_virtual_bounds_width_overflow() {
        let displays = [
            display(1, -f64::MAX, 0.0, f64::MAX, 100.0, 1.0, 1.0, true),
            display(
                2,
                f64::MAX / 2.0,
                0.0,
                f64::MAX / 2.0,
                100.0,
                1.0,
                1.0,
                false,
            ),
        ];

        assert_eq!(
            analyze_displays(&displays),
            Err(TopologyError::VirtualBoundsOverflow)
        );
    }

    #[test]
    fn rejects_layout_when_virtual_bounds_width_overflows() {
        let displays = [
            display(1, -f64::MAX, 0.0, 1.0, 100.0, 1.0, 1.0, true),
            display(2, f64::MAX / 2.0, 0.0, 1.0, 100.0, 1.0, 1.0, false),
        ];

        assert_eq!(
            analyze_displays(&displays),
            Err(TopologyError::VirtualBoundsOverflow)
        );
    }
}

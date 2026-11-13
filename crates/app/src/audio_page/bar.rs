//! How the bar over the Audio page's bottom views fits its width: the
//! view toggle, Show and the lines tool always; then the timeline's tools
//! in a row, or folded into one dropdown when the row doesn't fit; then,
//! on the right, the mix's loudness, the playhead's time and the edit
//! needle's, as many as fit whole. Nothing is ever drawn over anything else.

/// What the bar shows after its fixed part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// The timeline's tools as a row (else one dropdown).
    pub tools_in_a_row: bool,
    /// How many of the right-hand items show, in order of importance.
    pub right: usize,
}

/// Plans `room` points (what is left after the fixed part) for the tools
/// (`row` wide, or `folded` as a dropdown) and the right-hand items, each
/// with its width, most important first. `gap` goes between the tools and
/// the right-hand items.
pub fn plan(room: f32, row: f32, folded: f32, right: &[f32], gap: f32) -> Plan {
    let fitting = |tools: f32| {
        let mut used = tools + gap;
        right
            .iter()
            .take_while(|w| {
                used += **w;
                used <= room
            })
            .count()
    };
    let all: f32 = right.iter().sum();
    if row + gap + all <= room {
        Plan {
            tools_in_a_row: true,
            right: right.len(),
        }
    } else {
        Plan {
            tools_in_a_row: false,
            right: fitting(folded),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tools_fold_first_then_the_least_important_items_go() {
        let right = [90.0, 100.0, 150.0];
        assert_eq!(
            plan(800.0, 300.0, 80.0, &right, 10.0),
            Plan {
                tools_in_a_row: true,
                right: 3
            }
        );
        assert_eq!(
            plan(500.0, 300.0, 80.0, &right, 10.0),
            Plan {
                tools_in_a_row: false,
                right: 3
            }
        );
        assert_eq!(
            plan(300.0, 300.0, 80.0, &right, 10.0),
            Plan {
                tools_in_a_row: false,
                right: 2
            }
        );
        assert_eq!(
            plan(150.0, 300.0, 80.0, &right, 10.0),
            Plan {
                tools_in_a_row: false,
                right: 0
            }
        );
        // Whatever it plans fits.
        for room in (0..1000).step_by(7) {
            let room = room as f32;
            let p = plan(room, 300.0, 80.0, &right, 10.0);
            let tools = if p.tools_in_a_row { 300.0 } else { 80.0 };
            let used: f32 = tools + 10.0 + right[..p.right].iter().sum::<f32>();
            assert!(p.right == 0 || used <= room, "{room}: {p:?}");
        }
    }
}

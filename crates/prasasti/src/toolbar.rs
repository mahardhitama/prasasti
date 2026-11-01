//! The floating glass toolbar: one segmented control, Preview | Edit.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2_app_kit::{NSControlBorderShape, NSControlSize, NSSegmentStyle, NSSegmentSwitchTracking, NSSegmentedControl, NSView};
use objc2_foundation::{MainThreadMarker, NSArray, NSString};

use crate::content::Mode;
use crate::glass::GlassView;

/// Track padding around the segments, and each segment's width.
const PAD: f64 = 4.0;
const SEGMENT_WIDTH: f64 = 110.0;

pub struct Toolbar {
    glass: Retained<GlassView>,
    segment: Retained<NSSegmentedControl>,
}

impl Toolbar {
    /// Clicking a segment sends `action` to the target (`set_target`), which reads `selected()`.
    pub fn new(mtm: MainThreadMarker, action: Sel) -> Self {
        let labels = NSArray::from_retained_slice(&[NSString::from_str("Preview"), NSString::from_str("Edit")]);
        // SAFETY: no target yet; `action` is an action selector.
        let segment = unsafe {
            NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
                &labels,
                NSSegmentSwitchTracking::SelectOne,
                None,
                Some(action),
                mtm,
            )
        };
        // macOS 26's native capsule look: a raised glass thumb on a capsule track.
        segment.setControlSize(NSControlSize::ExtraLarge);
        segment.setSegmentStyle(NSSegmentStyle::Capsule);
        segment.setBorderShape(NSControlBorderShape::Capsule);
        for i in 0..2 {
            segment.setWidth_forSegment(SEGMENT_WIDTH, i);
        }
        let glass = GlassView::new(mtm);
        glass.setContentView(Some(&segment));
        glass.setCornerRadius((segment.fittingSize().height + 2.0 * PAD) / 2.0);
        segment.setTranslatesAutoresizingMaskIntoConstraints(false);
        for c in [
            segment.leadingAnchor().constraintEqualToAnchor_constant(&glass.leadingAnchor(), PAD),
            glass.trailingAnchor().constraintEqualToAnchor_constant(&segment.trailingAnchor(), PAD),
            segment.topAnchor().constraintEqualToAnchor_constant(&glass.topAnchor(), PAD),
            glass.bottomAnchor().constraintEqualToAnchor_constant(&segment.bottomAnchor(), PAD),
        ] {
            c.setActive(true);
        }
        let this = Self { glass, segment };
        this.set_selected(Mode::Edit);
        this
    }

    /// `target` implements the action and owns the toolbar, so it outlives the control.
    pub fn set_target(&self, target: &AnyObject) {
        // SAFETY: as above; NSControl holds its target weakly.
        unsafe { self.segment.setTarget(Some(target)) };
    }

    pub fn view(&self) -> &NSView {
        &self.glass
    }

    pub fn segment(&self) -> Retained<NSSegmentedControl> {
        self.segment.clone()
    }

    pub fn selected(&self) -> Mode {
        if self.segment.selectedSegment() == 0 { Mode::Preview } else { Mode::Edit }
    }

    pub fn set_selected(&self, m: Mode) {
        self.segment.setSelectedSegment(if m == Mode::Preview { 0 } else { 1 });
    }
}

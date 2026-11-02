//! The window toolbar's Preview | Edit tab control: a native segmented control,
//! as an NSTabView draws its tabs (HIG: Tab views).

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::sel;
use objc2_app_kit::{NSSegmentSwitchTracking, NSSegmentedControl, NSView};
use objc2_foundation::{MainThreadMarker, NSArray, NSString};

use crate::content::Mode;

pub struct Toolbar {
    control: Retained<NSSegmentedControl>,
}

fn index(m: Mode) -> isize {
    if m == Mode::Preview { 0 } else { 1 }
}

impl Toolbar {
    /// The control sends selectMode: to the target (`set_target`), which calls `set_selected`.
    pub fn new(mtm: MainThreadMarker) -> Self {
        let labels = NSArray::from_retained_slice(&[NSString::from_str("Preview"), NSString::from_str("Edit")]);
        // SAFETY: no target yet (`set_target`); `selectMode:` is an action selector.
        let control = unsafe {
            NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
                &labels,
                NSSegmentSwitchTracking::SelectOne,
                None,
                Some(sel!(selectMode:)),
                mtm,
            )
        };
        let this = Self { control };
        this.set_selected(Mode::Edit);
        this
    }

    /// `target` implements selectMode: and owns the toolbar, so it outlives the control.
    pub fn set_target(&self, target: &AnyObject) {
        // SAFETY: NSControl holds its target weakly.
        unsafe { self.control.setTarget(Some(target)) };
    }

    pub fn view(&self) -> &NSView {
        &self.control
    }

    pub fn control(&self) -> Retained<NSSegmentedControl> {
        self.control.clone()
    }

    pub fn selected(&self) -> Mode {
        if self.control.selectedSegment() == 0 { Mode::Preview } else { Mode::Edit }
    }

    /// Selects `m`'s segment. Never sends an action.
    pub fn set_selected(&self, m: Mode) {
        self.control.setSelectedSegment(index(m));
    }
}

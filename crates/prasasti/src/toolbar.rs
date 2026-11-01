//! The floating glass toolbar: a Preview | Edit toggle built from glass views, a
//! capsule track and a clear glass thumb that slides under the selected label.
//! NSSegmentedControl draws an accent-filled selection and never animates it.

use std::cell::Cell;
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{AnyThread, sel};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityRadioButtonRole, NSAnimatablePropertyContainer, NSAppearanceCustomization, NSAnimationContext, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSButton, NSColor, NSFont, NSFontAttributeName, NSFontWeightBold, NSFontWeightMedium, NSForegroundColorAttributeName,
    NSGlassEffectContainerView, NSGlassEffectViewStyle, NSView, NSWindowOcclusionState,
};
use objc2_foundation::{MainThreadMarker, NSArray, NSAttributedString, NSDictionary, NSPoint, NSRect, NSSize, NSString};

use crate::content::Mode;
use crate::glass::GlassView;
use crate::util::guarded;

/// Track padding around the thumb, each label's width, and the thumb's height.
const PAD: f64 = 4.0;
const SEGMENT_WIDTH: f64 = 110.0;
const THUMB_HEIGHT: f64 = 36.0;
const WIDTH: f64 = 2.0 * PAD + 2.0 * SEGMENT_WIDTH;
const HEIGHT: f64 = 2.0 * PAD + THUMB_HEIGHT;

pub struct Toolbar {
    root: Retained<NSView>,
    track: Retained<GlassView>,
    thumb: Retained<GlassView>,
    buttons: [Retained<NSButton>; 2],
    selected: Cell<Mode>,
}

fn slot(i: usize) -> NSRect {
    NSRect::new(NSPoint::new(PAD + i as f64 * SEGMENT_WIDTH, PAD), NSSize::new(SEGMENT_WIDTH, THUMB_HEIGHT))
}

fn index(m: Mode) -> usize {
    if m == Mode::Preview { 0 } else { 1 }
}

impl Toolbar {
    /// The buttons send showPreview:/showEditor: to the target (`set_target`), which calls `set_selected`.
    pub fn new(mtm: MainThreadMarker) -> Self {
        let bounds = NSRect::new(NSPoint::ZERO, NSSize::new(WIDTH, HEIGHT));
        let root = NSView::initWithFrame(mtm.alloc(), bounds);
        let container = NSGlassEffectContainerView::initWithFrame(mtm.alloc(), bounds);
        let holder = NSView::initWithFrame(mtm.alloc(), bounds);
        container.setContentView(Some(&holder));
        let track = GlassView::initWithFrame(mtm.alloc(), bounds);
        track.setCornerRadius(HEIGHT / 2.0);
        // Clear glass inside the container: a crisp rim over the track (spike variant C4).
        let thumb = GlassView::initWithFrame(mtm.alloc(), slot(1));
        thumb.setCornerRadius(THUMB_HEIGHT / 2.0);
        thumb.setStyle(NSGlassEffectViewStyle::Clear);
        holder.addSubview(&track);
        holder.addSubview(&thumb);
        root.addSubview(&container);
        let buttons = [(0, "Preview", sel!(showPreview:)), (1, "Edit", sel!(showEditor:))].map(|(i, t, action): (usize, _, Sel)| {
            // SAFETY: no target yet (`set_target`); `action` is an action selector.
            let b = unsafe { NSButton::buttonWithTitle_target_action(&NSString::from_str(t), None, Some(action), mtm) };
            b.setBordered(false);
            // SAFETY: a plain extern static.
            b.setAccessibilityRole(Some(unsafe { NSAccessibilityRadioButtonRole }));
            b.setFrame(slot(i));
            // Above the container: it raises glass over its own content view's subviews.
            root.addSubview(&b);
            b
        });
        root.setTranslatesAutoresizingMaskIntoConstraints(false);
        root.widthAnchor().constraintEqualToConstant(WIDTH).setActive(true);
        root.heightAnchor().constraintEqualToConstant(HEIGHT).setActive(true);
        let this = Self { root, track, thumb, buttons, selected: Cell::new(Mode::Edit) };
        this.set_selected(Mode::Edit);
        this
    }

    /// `target` implements the actions and owns the toolbar, so it outlives the buttons.
    pub fn set_target(&self, target: &AnyObject) {
        for b in &self.buttons {
            // SAFETY: as above; NSControl holds its target weakly.
            unsafe { b.setTarget(Some(target)) };
        }
    }

    pub fn view(&self) -> &NSView {
        &self.root
    }

    pub fn buttons(&self) -> [Retained<NSButton>; 2] {
        self.buttons.clone()
    }

    pub fn thumb(&self) -> Retained<GlassView> {
        self.thumb.clone()
    }

    pub fn selected(&self) -> Mode {
        self.selected.get()
    }

    /// Moves the thumb (sliding while the window is actually drawn) and restyles the labels.
    /// Never sends an action.
    pub fn set_selected(&self, m: Mode) {
        self.selected.set(m);
        let frame = slot(index(m));
        // Occluded, locked or asleep: Core Animation doesn't advance, so the thumb would stay put.
        if self.root.window().is_some_and(|w| w.isVisible() && w.occlusionState().contains(NSWindowOcclusionState::Visible)) {
            let thumb = self.thumb.clone();
            NSAnimationContext::runAnimationGroup(&RcBlock::new(move |ctx: NonNull<NSAnimationContext>| {
                guarded(|| {
                    // SAFETY: AppKit passes the current, live context.
                    let ctx = unsafe { ctx.as_ref() };
                    ctx.setDuration(0.35);
                    ctx.setAllowsImplicitAnimation(true);
                    thumb.animator().setFrame(frame);
                });
            }));
        } else {
            self.thumb.setFrame(frame);
        }
        self.apply_appearance();
    }

    /// Track tint and label styles; call again when the effective appearance changes.
    pub fn apply_appearance(&self) {
        let names = NSArray::from_slice(&unsafe { [NSAppearanceNameAqua, NSAppearanceNameDarkAqua] });
        let best = self.root.effectiveAppearance().bestMatchFromAppearancesWithNames(&names);
        let dark = best.is_some_and(|n| &*n == unsafe { NSAppearanceNameDarkAqua });
        let base = if dark { NSColor::whiteColor() } else { NSColor::blackColor() };
        self.track.setTintColor(Some(&base.colorWithAlphaComponent(0.08)));
        let on = index(self.selected.get());
        for (i, b) in self.buttons.iter().enumerate() {
            let sel = i == on;
            // SAFETY: plain extern statics.
            let weight = unsafe { if sel { NSFontWeightBold } else { NSFontWeightMedium } };
            let font = NSFont::systemFontOfSize_weight(15.0, weight);
            let color = if sel { NSColor::labelColor() } else { NSColor::secondaryLabelColor() };
            let attrs = NSDictionary::from_slices(
                &unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] },
                &[&*font as &AnyObject, &*color as &AnyObject],
            );
            // SAFETY: font and colour values for their own keys.
            let title = unsafe {
                NSAttributedString::initWithString_attributes(NSAttributedString::alloc(), &b.title(), Some(&attrs))
            };
            b.setAttributedTitle(&title);
            b.setAccessibilitySelected(sel);
        }
    }
}


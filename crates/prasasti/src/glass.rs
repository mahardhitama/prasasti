//! Liquid Glass chrome. objc2-app-kit binds NSGlassEffectView (macOS 26), which already has
//! `new(mtm)`, `setContentView` and `setCornerRadius`, so no wrapper is needed.
pub type GlassView = objc2_app_kit::NSGlassEffectView;

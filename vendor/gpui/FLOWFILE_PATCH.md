# FlowFile App Store patch

This is `gpui` 0.2.2 from crates.io with one App Store compatibility feature:

- `app-store` removes the private `CGSMainConnectionID` and
  `CGSSetWindowBackgroundBlurRadius` references from macOS binaries.
- App Store builds use GPUI's existing public `NSVisualEffectView` path. Their
  minimum supported macOS version is 12.0, where that is already GPUI's normal
  path.
- Direct-distribution builds do not enable the feature and retain upstream
  behavior.

Remove this override after an upstream GPUI release provides an equivalent
feature or no longer links the private APIs.

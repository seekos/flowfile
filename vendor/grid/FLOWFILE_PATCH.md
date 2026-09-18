# FlowFile compatibility patch

GPUI 0.2.2 depends on `grid` 0.18 and cannot resolve the patched upstream
`grid` 1.0.1 release. This vendored copy retains the 0.18 public API and
backports checked dimension arithmetic in `expand_rows` and `expand_cols`.

The check prevents backing-storage length overflow before any grid dimensions
are changed, addressing GHSA-38c5-483c-4qqp while keeping Taffy/GPUI compatible.
Remove this override after GPUI accepts `grid >= 1.0.1`.

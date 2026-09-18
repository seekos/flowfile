/// Compile-time distribution policy. Keeping this in one module makes it
/// difficult for a Mac App Store build to accidentally re-enable a feature
/// that relies on Developer ID distribution semantics.
pub const fn is_app_store() -> bool {
    cfg!(feature = "app-store")
}

pub const fn allows_direct_updates() -> bool {
    !is_app_store()
}

pub const fn allows_external_terminal() -> bool {
    !is_app_store()
}

pub const fn allows_direct_executable_launch() -> bool {
    !is_app_store()
}

pub const fn allows_scripted_volume_mounts() -> bool {
    !is_app_store()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_policy_is_internally_consistent() {
        if is_app_store() {
            assert!(!allows_direct_updates());
            assert!(!allows_external_terminal());
            assert!(!allows_direct_executable_launch());
            assert!(!allows_scripted_volume_mounts());
        } else {
            assert!(allows_direct_updates());
            assert!(allows_external_terminal());
            assert!(allows_direct_executable_launch());
            assert!(allows_scripted_volume_mounts());
        }
    }
}

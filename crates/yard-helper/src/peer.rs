//! Who may talk to the helper: the user it was installed for, and root.

pub fn allowed(peer_uid: u32, owner_uid: u32) -> bool {
    peer_uid == owner_uid || peer_uid == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_owner_and_root_are_served() {
        assert!(allowed(501, 501));
        assert!(allowed(0, 501));
        assert!(!allowed(502, 501));
    }
}

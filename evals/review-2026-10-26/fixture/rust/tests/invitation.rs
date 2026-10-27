use review_cross_language_fixture::{handle_invitation, InvitationService};

#[test]
fn handler_sends_invitation() {
    assert!(handle_invitation(&InvitationService, "person@example.test"));
}

pub struct InvitationService;

impl InvitationService {
    pub fn send(&self, email: &str) -> bool {
        !email.is_empty()
    }
}

pub fn handle_invitation(service: &InvitationService, email: &str) -> bool {
    service.send(email)
}

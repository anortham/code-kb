package review;

public final class InvitationController {
    private final InvitationService service;

    public InvitationController(InvitationService service) {
        this.service = service;
    }

    public String post(String email) {
        return service.send(email);
    }
}

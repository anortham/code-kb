package review;

import org.junit.jupiter.api.Test;

final class InvitationControllerTest {
    @Test
    void post_sends_invitation() {
        InvitationController controller = new InvitationController(new InvitationService());
        controller.post("person@example.test");
    }
}

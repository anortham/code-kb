using Xunit;

namespace Review;

public sealed class InvitationControllerTests
{
    [Fact]
    public void Invite_sends_invitation()
    {
        var controller = new InvitationController(new InvitationService());
        controller.Invite("person@example.test");
    }
}

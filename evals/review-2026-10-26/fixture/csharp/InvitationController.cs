namespace Review;

public sealed class InvitationController(InvitationService service)
{
    public bool Invite(string email) => service.Send(email);
}

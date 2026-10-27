namespace Review;

public sealed class InvitationService
{
    public bool Send(string email, string? note = null) => email.Contains('@');
}

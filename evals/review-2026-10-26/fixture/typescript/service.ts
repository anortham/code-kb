export type InvitationRequest = { email: string; displayName?: string };

export class InvitationService {
  send(input: InvitationRequest): boolean {
    return input.email.includes("@");
  }
}

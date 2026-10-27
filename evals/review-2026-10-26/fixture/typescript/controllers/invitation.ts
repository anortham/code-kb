import { InvitationRequest, InvitationService } from "../service";

export function submitInvitation(service: InvitationService, input: InvitationRequest): boolean {
  return service.send(input);
}

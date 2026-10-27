import { InvitationRequest, InvitationService } from "./service";
import { submitInvitation } from "./controllers/invitation";

export function postInvitation(service: InvitationService, input: InvitationRequest): boolean {
  return submitInvitation(service, input);
}

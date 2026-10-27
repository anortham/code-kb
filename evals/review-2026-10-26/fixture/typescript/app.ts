import { InvitationRequest, InvitationService } from "./service";
import { postInvitation } from "./routes";

export function post(service: InvitationService, input: InvitationRequest): boolean {
  return postInvitation(service, input);
}

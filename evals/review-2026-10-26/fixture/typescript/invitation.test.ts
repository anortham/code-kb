import { InvitationService } from "./service";
import { post } from "./app";

test("invitation route passes display name", () => {
  expect(post(new InvitationService(), { email: "person@example.test", displayName: "Pat" })).toBe(true);
});

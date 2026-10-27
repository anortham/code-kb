package invitation

import (
    "net/http/httptest"
    "testing"
)

func TestInvitationRoute(t *testing.T) {
    handler := &InvitationHandler{service: &InvitationService{}}
    handler.ServeHTTP(httptest.NewRecorder(), httptest.NewRequest("POST", "/invite?email=person%40example.test", nil))
}

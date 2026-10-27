package invitation

import "net/http"

type InvitationService struct{}

func (s *InvitationService) Send(email string) error {
    return nil
}

type MailService struct{}

func (s *MailService) Send(email string) error {
    return nil
}

type InvitationHandler struct{ service *InvitationService }

func (h *InvitationHandler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
    _ = h.service.Send(r.URL.Query().Get("email"))
}

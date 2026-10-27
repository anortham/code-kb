def test_register_account_route(client):
    response = client.post("/register", json={"email": "person@example.test"})
    assert response.status_code == 201

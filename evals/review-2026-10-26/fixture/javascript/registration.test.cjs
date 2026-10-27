const { createApp } = require("./app.cjs");

test("registration route uses the exported handler", async () => {
  const app = createApp(makeTestApp());
  const response = await request(app).post("/register").send({ email: "person@example.test" });
  expect(response.status).toBe(201);
});

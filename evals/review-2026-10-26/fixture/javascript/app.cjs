const { registerAccount } = require("./registration.cjs");

function createApp(app) {
  app.post("/register", registerAccount);
  return app;
}

module.exports = { createApp };

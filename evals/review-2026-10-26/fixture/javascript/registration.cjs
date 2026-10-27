function registerAccount(request, response) {
  return response.status(201).json({ email: request.body.email.trim().toLowerCase() });
}

module.exports = { registerAccount };
